use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use serde_json::{Value, json};
use tokio::sync::{mpsc::UnboundedSender, oneshot};

use crate::{
    config::OpenAiCompatibleProviderConfig,
    provider::{
        ApprovalDecision, ApprovalPolicy, ChatRole, DiffKind, DiffLine, PromptRequest, StreamEvent,
        ToolConfirmPreview, TurnResult, Usage,
    },
    tools,
};

const DEFAULT_MAX_TOOL_ROUNDS: usize = 32;
const HARD_MAX_TOOL_ROUNDS: usize = 256;
const MAX_TOOL_ROUNDS_ENV: &str = "ANVEESA_MAX_TOOL_ROUNDS";
const MAX_RETRIES: usize = 2;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// How many times the model may call the exact same (tool, arguments) pair before we refuse.
const MAX_IDENTICAL_CALLS: usize = 3;
const MAX_TOOL_INTENT_REPROMPTS: usize = 2;
/// How many times we ask the model to continue after its output was cut off by the
/// provider's token limit (`finish_reason == "length"`) before giving up.
const MAX_LENGTH_CONTINUATIONS: usize = 8;

pub async fn ask(
    provider_name: &str,
    config: &OpenAiCompatibleProviderConfig,
    request: PromptRequest,
    policy: ApprovalPolicy,
    events: &UnboundedSender<StreamEvent>,
) -> Result<TurnResult> {
    let model = request
        .model
        .clone()
        .or_else(|| config.default_model.clone())
        .with_context(|| {
            format!("provider '{provider_name}' requires --model or default_model in config")
        })?;

    // Use the explicit config flag if set; otherwise auto-enable for Anthropic endpoints.
    let prompt_cache = config
        .prompt_cache
        .unwrap_or_else(|| is_anthropic_url(&config.base_url));
    let headers = build_headers(config, prompt_cache)?;
    let mut messages = build_messages(&request, policy, prompt_cache);

    let client = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(Duration::from_secs(300)) // 5-minute read timeout for long streams
        .build()
        .context("failed to build HTTP client")?;
    let url = format!("{}/chat/completions", config.base_url.trim_end_matches('/'));

    let mut tools_enabled = true;
    let mut usage_requested = true;
    let mut tool_rounds = 0usize;
    let max_tool_rounds = max_tool_rounds();
    let mut approval_state = ToolApprovalState::default();
    let mut full_text = String::new();
    let mut last_usage: Option<Usage> = None;
    let mut tool_intent_reprompts = 0usize;
    let mut length_continuations = 0usize;

    loop {
        let _ = events.send(StreamEvent::Status {
            message: if tool_rounds == 0 {
                format!("Waiting for {provider_name} response")
            } else {
                format!("Sending tool results to {provider_name}")
            },
        });

        let mut body = json!({
            "model": model,
            "messages": messages,
            "stream": true,
        });
        if usage_requested {
            body["stream_options"] = json!({ "include_usage": true });
        }
        if let Some(max_tokens) = config.max_tokens {
            body["max_tokens"] = json!(max_tokens);
        }
        if tools_enabled {
            body["tools"] = json!(tools::definitions(policy.allows_write_tools()));
            body["tool_choice"] = json!("auto");
        }

        let response = send_with_retry(&client, &url, &headers, &body)
            .await
            .with_context(|| format!("request to provider '{provider_name}' failed"))?;

        let status = response.status();
        if !status.is_success() {
            let response_body = response.text().await.unwrap_or_default();
            if tools_enabled && is_tool_parameter_error(&response_body) {
                tools_enabled = false;
                continue;
            }
            if usage_requested && is_stream_options_error(&response_body) {
                usage_requested = false;
                continue;
            }
            bail!(
                "provider '{provider_name}' HTTP {status}: {}",
                extract_api_error(&response_body)
            );
        }

        let mut state = StreamState::default();
        stream_response(response, &mut state, events).await?;

        if let Some(usage) = state.usage {
            last_usage = Some(usage);
        }

        // The provider cut the response off at its output-token limit. Treating the
        // partial text (or partial tool call) as final is what makes Anveesa appear to
        // "stop suddenly" mid-task — instead, keep what we have and ask it to continue.
        if state.finish_reason.as_deref() == Some("length")
            && length_continuations < MAX_LENGTH_CONTINUATIONS
        {
            length_continuations += 1;
            full_text.push_str(&state.content);
            let _ = events.send(StreamEvent::Status {
                message: "Response hit the output token limit; asking the model to continue"
                    .to_string(),
            });
            // Drop any partial tool call: a length-truncated call has incomplete
            // arguments and can't be dispatched. The continuation nudge tells the
            // model to re-issue it.
            if !state.content.is_empty() {
                messages.push(json!({
                    "role": "assistant",
                    "content": state.content,
                }));
            }
            messages.push(length_continuation_message());
            continue;
        }

        if state.tool_calls.is_empty() {
            if tools_enabled
                && tool_intent_reprompts < MAX_TOOL_INTENT_REPROMPTS
                && looks_like_unfinished_tool_intent(&state.content)
            {
                tool_intent_reprompts += 1;
                messages.push(json!({
                    "role": "assistant",
                    "content": state.content,
                }));
                messages.push(tool_intent_reprompt_message());
                continue;
            }

            full_text.push_str(&state.content);
            break;
        }

        if !tools_enabled {
            break;
        }
        tool_rounds += 1;

        messages.push(assistant_tool_message(&state));
        for call in &state.tool_calls {
            let content = dispatch_tool(call, policy, &mut approval_state, events).await;
            messages.push(json!({
                "role": "tool",
                "tool_call_id": call.id,
                "name": call.name,
                "content": content,
            }));
        }

        let _ = events.send(StreamEvent::Status {
            message: "Tool results sent; waiting for the next model response".to_string(),
        });

        if tool_rounds >= max_tool_rounds {
            tools_enabled = false;
            messages.push(tool_limit_message(max_tool_rounds));
        }
    }

    if let Some(usage) = last_usage {
        let _ = events.send(StreamEvent::Usage(usage));
    }

    Ok(TurnResult {
        text: full_text,
        usage: last_usage,
    })
}

#[derive(Debug, Default)]
struct ToolApprovalState {
    allow_for_turn: bool,
    /// Tracks how many times each identical (name, arguments) pair has been called this turn.
    call_counts: std::collections::HashMap<(String, String), usize>,
}

async fn dispatch_tool(
    call: &PartialToolCall,
    policy: ApprovalPolicy,
    approval_state: &mut ToolApprovalState,
    events: &UnboundedSender<StreamEvent>,
) -> String {
    let summary = tools::describe_call(&call.name, &call.arguments);

    // Plan tools — display only, no approval or filesystem access needed.
    if call.name == "set_plan" {
        if let Ok(args) = serde_json::from_str::<serde_json::Value>(&call.arguments) {
            let tasks = args["steps"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            let _ = events.send(StreamEvent::PlanSet { tasks });
        }
        return json!({"ok": true}).to_string();
    }
    if call.name == "complete_task" {
        if let Ok(args) = serde_json::from_str::<serde_json::Value>(&call.arguments) {
            if let Some(index) = args["index"].as_u64() {
                let _ = events.send(StreamEvent::PlanTaskDone {
                    index: index as usize,
                });
            }
        }
        return json!({"ok": true}).to_string();
    }

    // Anti-loop guard: refuse if the model is calling the exact same (tool, args) repeatedly.
    {
        let key = (call.name.clone(), call.arguments.clone());
        let count = approval_state.call_counts.entry(key).or_insert(0);
        *count += 1;
        if *count > MAX_IDENTICAL_CALLS {
            return json!({
                "ok": false,
                "error": format!(
                    "Refusing to run '{}' again: this identical call has already been made {} time(s) \
                    this turn. Do NOT retry — stop and report the failure to the user.",
                    call.name, *count - 1
                )
            })
            .to_string();
        }
    }

    if tools::is_write_tool(&call.name) {
        if !policy.allows_write_tools() {
            return denied_message("write tools are disabled (pass --yes or run interactively)");
        }
    } else {
        let _ = events.send(StreamEvent::ToolCall {
            summary: summary.clone(),
        });
    }

    // Snapshot BEFORE the tool runs — needed both for preview and for post-run diff.
    let file_op_snapshot = capture_file_op_snapshot(&call.name, &call.arguments);

    let mut preview_was_shown = false;

    if tools::is_write_tool(&call.name)
        && policy == ApprovalPolicy::Prompt
        && !approval_state.allow_for_turn
    {
        let preview = build_confirm_preview(&call.name, &call.arguments, &file_op_snapshot);
        preview_was_shown = true;
        match request_approval_with_preview(preview, events).await {
            ApprovalDecision::AllowOnce => {}
            ApprovalDecision::AllowForTurn => approval_state.allow_for_turn = true,
            ApprovalDecision::Deny => return denied_message("user declined this action"),
        }
        let _ = events.send(StreamEvent::Status {
            message: format!("Applying approved action: {summary}"),
        });
    } else if tools::is_write_tool(&call.name) {
        let _ = events.send(StreamEvent::ToolCall {
            summary: summary.clone(),
        });
    }

    let tool_started = Instant::now();
    let result = tools::run(&call.name, &call.arguments).await;
    let (ok, error) = parse_tool_result_status(&result);
    let _ = events.send(StreamEvent::ToolResult {
        summary: summary.clone(),
        ok,
        elapsed_ms: tool_started.elapsed().as_millis(),
        error,
    });

    // When the user already reviewed the diff in the approval preview, skip the
    // post-run FileOp so the same diff isn't printed twice.
    if !preview_was_shown {
        if let Some(snapshot) = file_op_snapshot {
            if let Ok(result_json) = serde_json::from_str::<serde_json::Value>(&result) {
                if result_json["ok"].as_bool().unwrap_or(false) {
                    emit_file_op_event(snapshot, &result_json, events);
                }
            }
        }
    }

    result
}

fn parse_tool_result_status(result: &str) -> (bool, Option<String>) {
    let Ok(json) = serde_json::from_str::<Value>(result) else {
        return (true, None);
    };
    let ok = json.get("ok").and_then(Value::as_bool).unwrap_or(true);
    let error = json
        .get("error")
        .and_then(Value::as_str)
        .map(str::to_string);
    (ok, error)
}

// ── File-op diff helpers ──────────────────────────────────────────────────────

enum FileOpSnapshot {
    Write {
        path: String,
        lines: Vec<String>,
        total: usize,
    },
    Edit {
        path: String,
        start_line: usize,
        old_lines: Vec<String>,
        new_lines: Vec<String>,
    },
    CreateDir {
        path: String,
    },
}

fn capture_file_op_snapshot(tool_name: &str, arguments: &str) -> Option<FileOpSnapshot> {
    let args: serde_json::Value = serde_json::from_str(arguments).ok()?;
    match tool_name {
        "write_file" => {
            let path = args["path"].as_str()?.to_string();
            let content = args["content"].as_str().unwrap_or("");
            let all: Vec<String> = content.lines().map(str::to_string).collect();
            let total = all.len();
            Some(FileOpSnapshot::Write {
                path,
                lines: all.into_iter().take(20).collect(),
                total,
            })
        }
        "edit_file" => {
            let path = args["path"].as_str()?.to_string();
            let old = args["old_string"].as_str().unwrap_or("");
            let new = args["new_string"].as_str().unwrap_or("");
            let start_line = std::fs::read_to_string(&path)
                .ok()
                .and_then(|content| {
                    let pos = content.find(old)?;
                    Some(content[..pos].lines().count() + 1)
                })
                .unwrap_or(1);
            Some(FileOpSnapshot::Edit {
                path,
                start_line,
                old_lines: old.lines().map(str::to_string).collect(),
                new_lines: new.lines().map(str::to_string).collect(),
            })
        }
        "create_dir" => {
            let path = args["path"].as_str()?.to_string();
            Some(FileOpSnapshot::CreateDir { path })
        }
        _ => None,
    }
}

fn emit_file_op_event(
    snapshot: FileOpSnapshot,
    result: &serde_json::Value,
    events: &tokio::sync::mpsc::UnboundedSender<StreamEvent>,
) {
    const MAX_PREVIEW: usize = 20;

    let event = match snapshot {
        FileOpSnapshot::Write { path, lines, total } => {
            let verb = if result["created"].as_bool().unwrap_or(true) {
                "Create"
            } else {
                "Update"
            }
            .to_string();
            let truncated = total > MAX_PREVIEW;
            let preview = lines
                .into_iter()
                .enumerate()
                .map(|(i, text)| DiffLine {
                    kind: DiffKind::Add,
                    line_no: i + 1,
                    text,
                })
                .collect();
            StreamEvent::FileOp {
                verb,
                path,
                added: total,
                removed: 0,
                preview,
                truncated,
            }
        }
        FileOpSnapshot::Edit {
            path,
            start_line,
            old_lines,
            new_lines,
        } => {
            let added = new_lines.len();
            let removed = old_lines.len();
            // Cap: show at most MAX_PREVIEW removed + MAX_PREVIEW added
            let cap = MAX_PREVIEW;
            let truncated = old_lines.len() > cap || new_lines.len() > cap;
            let mut preview: Vec<DiffLine> = old_lines
                .into_iter()
                .take(cap)
                .enumerate()
                .map(|(i, text)| DiffLine {
                    kind: DiffKind::Remove,
                    line_no: start_line + i,
                    text,
                })
                .collect();
            for (i, text) in new_lines.into_iter().take(cap).enumerate() {
                preview.push(DiffLine {
                    kind: DiffKind::Add,
                    line_no: start_line + i,
                    text,
                });
            }
            StreamEvent::FileOp {
                verb: "Update".to_string(),
                path,
                added,
                removed,
                preview,
                truncated,
            }
        }
        FileOpSnapshot::CreateDir { path } => StreamEvent::FileOp {
            verb: "Create dir".to_string(),
            path,
            added: 0,
            removed: 0,
            preview: vec![],
            truncated: false,
        },
    };

    let _ = events.send(event);
}

fn max_tool_rounds() -> usize {
    parse_tool_round_limit(std::env::var(MAX_TOOL_ROUNDS_ENV).ok().as_deref())
}

fn parse_tool_round_limit(value: Option<&str>) -> usize {
    value
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_MAX_TOOL_ROUNDS)
        .min(HARD_MAX_TOOL_ROUNDS)
}

fn tool_limit_message(max_tool_rounds: usize) -> Value {
    json!({
        "role": "system",
        "content": format!(
            "Anveesa has already run {max_tool_rounds} tool rounds for this answer. Do not call tools again. Use the tool results already provided to produce the best final answer. If the requested work is not complete, say exactly what remains."
        )
    })
}

fn length_continuation_message() -> Value {
    json!({
        "role": "system",
        "content": "Your previous response was cut off because it reached the output token limit. Continue from exactly where you left off. Do not repeat text you already produced and do not restart the answer. If you were in the middle of a tool call, re-issue that complete tool call now."
    })
}

fn tool_intent_reprompt_message() -> Value {
    json!({
        "role": "system",
        "content": "Your previous message said you would inspect/read/check the workspace, but it did not call any tool or provide a final answer. Do not narrate future tool use. If you need information, call the relevant Anveesa tools now. Otherwise, answer the user directly."
    })
}

fn looks_like_unfinished_tool_intent(text: &str) -> bool {
    let lower = text.trim().to_lowercase();
    if lower.is_empty() || lower.len() > 600 {
        return false;
    }

    let has_intent = [
        "let me inspect",
        "let me check",
        "let me look",
        "let me read",
        "let me search",
        "let me peek",
        "let me also peek",
        "i'll inspect",
        "i'll check",
        "i'll look",
        "i'll read",
        "i'll search",
        "i will inspect",
        "i will check",
        "i will look",
        "i will read",
        "i will search",
        "i'm going to inspect",
        "i'm going to check",
        "i'm going to look",
        "i'm going to read",
        "i need to inspect",
        "i need to check",
    ]
    .iter()
    .any(|needle| lower.contains(needle));

    has_intent && (lower.ends_with(':') || lower.ends_with('.') || lower.ends_with("first"))
}

fn denied_message(reason: &str) -> String {
    json!({ "ok": false, "error": reason }).to_string()
}

fn build_confirm_preview(
    tool_name: &str,
    arguments: &str,
    snapshot: &Option<FileOpSnapshot>,
) -> ToolConfirmPreview {
    const CAP: usize = 20;
    match snapshot {
        Some(FileOpSnapshot::Write { path, lines, total }) => {
            let verb = if std::path::Path::new(path).exists() {
                "Update"
            } else {
                "Create"
            };
            let truncated = *total > CAP;
            let diff = lines
                .iter()
                .enumerate()
                .map(|(i, text)| DiffLine {
                    kind: DiffKind::Add,
                    line_no: i + 1,
                    text: text.clone(),
                })
                .collect();
            ToolConfirmPreview::FileOp {
                verb: verb.to_string(),
                path: path.clone(),
                added: *total,
                removed: 0,
                diff,
                truncated,
            }
        }
        Some(FileOpSnapshot::Edit {
            path,
            start_line,
            old_lines,
            new_lines,
        }) => {
            let truncated = old_lines.len() > CAP || new_lines.len() > CAP;
            let mut diff: Vec<DiffLine> = old_lines
                .iter()
                .take(CAP)
                .enumerate()
                .map(|(i, text)| DiffLine {
                    kind: DiffKind::Remove,
                    line_no: start_line + i,
                    text: text.clone(),
                })
                .collect();
            for (i, text) in new_lines.iter().take(CAP).enumerate() {
                diff.push(DiffLine {
                    kind: DiffKind::Add,
                    line_no: start_line + i,
                    text: text.clone(),
                });
            }
            ToolConfirmPreview::FileOp {
                verb: "Update".to_string(),
                path: path.clone(),
                added: new_lines.len(),
                removed: old_lines.len(),
                diff,
                truncated,
            }
        }
        Some(FileOpSnapshot::CreateDir { path }) => {
            ToolConfirmPreview::CreateDir { path: path.clone() }
        }
        None => ToolConfirmPreview::Generic {
            summary: tools::describe_call(tool_name, arguments),
        },
    }
}

async fn request_approval_with_preview(
    preview: ToolConfirmPreview,
    events: &UnboundedSender<StreamEvent>,
) -> ApprovalDecision {
    let (reply, answer) = oneshot::channel();
    if events
        .send(StreamEvent::Confirm { preview, reply })
        .is_err()
    {
        return ApprovalDecision::Deny;
    }
    answer.await.unwrap_or(ApprovalDecision::Deny)
}

fn is_anthropic_url(base_url: &str) -> bool {
    base_url.contains("anthropic.com")
}

fn build_headers(config: &OpenAiCompatibleProviderConfig, prompt_cache: bool) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

    let resolved_key = if let Some(key) = &config.api_key {
        Some(key.clone())
    } else if let Some(env_var) = &config.api_key_env {
        Some(
            std::env::var(env_var)
                .with_context(|| format!("environment variable {env_var} is required"))?,
        )
    } else {
        None
    };
    if let Some(api_key) = resolved_key {
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {api_key}"))
                .context("failed to build authorization header")?,
        );
    }

    for (name, value) in &config.headers {
        headers.insert(
            HeaderName::from_bytes(name.as_bytes())
                .with_context(|| format!("invalid header name '{name}'"))?,
            HeaderValue::from_str(value)
                .with_context(|| format!("invalid header value for '{name}'"))?,
        );
    }

    if prompt_cache {
        headers.insert(
            HeaderName::from_static("anthropic-beta"),
            HeaderValue::from_static("prompt-caching-2024-07-31"),
        );
    }

    Ok(headers)
}

fn build_messages(
    request: &PromptRequest,
    policy: ApprovalPolicy,
    prompt_cache: bool,
) -> Vec<Value> {
    let mut messages = Vec::new();
    if let Some(system) = &request.system {
        messages.push(json!({ "role": "system", "content": system }));
    }
    if let Some(workspace_context) = &request.workspace_context {
        messages.push(json!({ "role": "system", "content": workspace_context }));
    }
    messages
        .push(json!({ "role": "system", "content": tools::guidance(policy.allows_write_tools()) }));
    for message in &request.history {
        let role = match message.role {
            ChatRole::User => "user",
            ChatRole::Assistant => "assistant",
        };
        messages.push(json!({ "role": role, "content": message.content }));
    }

    // Current user turn — multimodal when a clipboard image is attached.
    let user_content = match &request.image {
        Some(img) => json!([
            { "type": "text", "text": &request.prompt },
            { "type": "image_url", "image_url": { "url": format!("data:{};base64,{}", img.mime, img.data) } }
        ]),
        None => json!(&request.prompt),
    };
    messages.push(json!({ "role": "user", "content": user_content }));

    if prompt_cache {
        apply_cache_breakpoints(&mut messages);
    }

    messages
}

/// Add `cache_control: {type: "ephemeral"}` to two breakpoints in the message list:
/// 1. The last system message (tools guidance — large, fully static).
/// 2. The last history message before the current user turn (grows each turn).
///
/// Everything up to each breakpoint is served from cache on subsequent turns.
fn apply_cache_breakpoints(messages: &mut Vec<Value>) {
    let current_turn_idx = messages.len() - 1;

    // Breakpoint 1: last system message
    if let Some(idx) = messages[..current_turn_idx]
        .iter()
        .rposition(|m| m["role"] == "system")
    {
        add_cache_control(&mut messages[idx]);
    }

    // Breakpoint 2: last history message (user or assistant before the current turn)
    if let Some(idx) = messages[..current_turn_idx]
        .iter()
        .rposition(|m| m["role"] != "system")
    {
        add_cache_control(&mut messages[idx]);
    }
}

/// Convert a message's `content` to the array form required by Anthropic caching,
/// then inject `cache_control: {type: "ephemeral"}` on the last content block.
fn add_cache_control(message: &mut Value) {
    let content = match message.get("content").cloned() {
        Some(c) => c,
        None => return,
    };

    let cached = match content {
        Value::String(s) => json!([{
            "type": "text",
            "text": s,
            "cache_control": { "type": "ephemeral" }
        }]),
        Value::Array(mut arr) => {
            if let Some(last) = arr.last_mut().and_then(Value::as_object_mut) {
                last.insert("cache_control".to_string(), json!({ "type": "ephemeral" }));
            }
            Value::Array(arr)
        }
        other => other,
    };

    message["content"] = cached;
}

async fn send_with_retry(
    client: &reqwest::Client,
    url: &str,
    headers: &HeaderMap,
    body: &Value,
) -> Result<reqwest::Response> {
    let mut attempt = 0usize;
    loop {
        match client
            .post(url)
            .headers(headers.clone())
            .json(body)
            .send()
            .await
        {
            Ok(response) => {
                if response.status().is_server_error() && attempt < MAX_RETRIES {
                    attempt += 1;
                    backoff(attempt).await;
                    continue;
                }
                return Ok(response);
            }
            Err(error) => {
                let retryable = error.is_connect() || error.is_timeout();
                if retryable && attempt < MAX_RETRIES {
                    attempt += 1;
                    backoff(attempt).await;
                    continue;
                }
                return Err(error.into());
            }
        }
    }
}

async fn backoff(attempt: usize) {
    let millis = 250u64 * (1u64 << (attempt - 1));
    tokio::time::sleep(Duration::from_millis(millis)).await;
}

async fn stream_response(
    mut response: reqwest::Response,
    state: &mut StreamState,
    events: &UnboundedSender<StreamEvent>,
) -> Result<()> {
    let mut buffer = String::new();
    let mut consecutive_errors: usize = 0;
    const MAX_CONSECUTIVE_ERRORS: usize = 3;

    loop {
        let chunk_result = response.chunk().await;

        match chunk_result {
            Ok(Some(chunk)) => {
                consecutive_errors = 0; // Reset error counter on successful read
                buffer.push_str(&String::from_utf8_lossy(&chunk));

                while let Some(newline) = buffer.find('\n') {
                    let line: String = buffer.drain(..=newline).collect();
                    if let Some(token) = state.ingest_line(line.trim_end_matches(['\r', '\n'])) {
                        let _ = events.send(StreamEvent::Token(token));
                    }
                }
            }
            Ok(None) => {
                // Stream ended normally
                break;
            }
            Err(error) => {
                consecutive_errors += 1;
                if consecutive_errors >= MAX_CONSECUTIVE_ERRORS {
                    bail!(
                        "stream interrupted while reading provider response after {} consecutive errors: {}",
                        consecutive_errors,
                        error
                    );
                }
                // Try to continue reading - transient network hiccups happen
                continue;
            }
        }
    }

    // Process any remaining data in the buffer
    if !buffer.is_empty()
        && let Some(token) = state.ingest_line(buffer.trim())
    {
        let _ = events.send(StreamEvent::Token(token));
    }

    Ok(())
}

#[derive(Debug, Default)]
struct StreamState {
    content: String,
    tool_calls: Vec<PartialToolCall>,
    usage: Option<Usage>,
    finish_reason: Option<String>,
    done: bool,
}

#[derive(Debug, Default, Clone)]
struct PartialToolCall {
    id: String,
    name: String,
    arguments: String,
}

impl StreamState {
    /// Process a single SSE line, returning any new assistant text to display.
    fn ingest_line(&mut self, line: &str) -> Option<String> {
        if self.done {
            return None;
        }
        let data = line.strip_prefix("data:")?.trim();
        if data.is_empty() {
            return None;
        }
        if data == "[DONE]" {
            self.done = true;
            return None;
        }
        let chunk: Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(_) => return None, // Skip malformed lines instead of bailing
        };
        self.apply_chunk(&chunk)
    }

    fn apply_chunk(&mut self, chunk: &Value) -> Option<String> {
        if let Some(usage) = chunk.get("usage").filter(|value| value.is_object()) {
            self.usage = parse_usage(usage);
        }

        let Some(choices) = chunk.get("choices") else {
            return None;
        };
        let Some(first_choice) = choices.get(0) else {
            return None;
        };

        // `finish_reason` is a sibling of `delta` and only carries a string on the
        // final chunk for the choice (it's null on every intermediate chunk).
        if let Some(reason) = first_choice.get("finish_reason").and_then(Value::as_str) {
            self.finish_reason = Some(reason.to_string());
        }

        let Some(delta) = first_choice.get("delta") else {
            return None;
        };

        if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
            for call in tool_calls {
                self.apply_tool_call_delta(call);
            }
        }

        // Content may be absent (e.g., tool-call-only chunks) — don't bail.
        let text = delta.get("content").and_then(Value::as_str).unwrap_or("");
        if text.is_empty() {
            return None;
        }
        self.content.push_str(text);
        Some(text.to_string())
    }

    fn apply_tool_call_delta(&mut self, call: &Value) {
        let index = call.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
        while self.tool_calls.len() <= index {
            self.tool_calls.push(PartialToolCall::default());
        }
        let slot = &mut self.tool_calls[index];

        if let Some(id) = call.get("id").and_then(Value::as_str)
            && !id.is_empty()
        {
            slot.id = id.to_string();
        }
        if let Some(function) = call.get("function") {
            if let Some(name) = function.get("name").and_then(Value::as_str) {
                slot.name.push_str(name);
            }
            if let Some(arguments) = function.get("arguments").and_then(Value::as_str) {
                slot.arguments.push_str(arguments);
            }
        }
    }
}

fn parse_usage(value: &Value) -> Option<Usage> {
    let object = value.as_object()?;

    // Anthropic uses top-level cache fields; OpenAI nests them under prompt_tokens_details.
    let details = object.get("prompt_tokens_details");
    let cache_read_tokens = object
        .get("cache_read_input_tokens")
        .and_then(Value::as_u64)
        .or_else(|| details?.get("cached_tokens").and_then(Value::as_u64))
        .unwrap_or(0);
    let cache_write_tokens = object
        .get("cache_creation_input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);

    Some(Usage {
        prompt_tokens: object
            .get("prompt_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        completion_tokens: object
            .get("completion_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        total_tokens: object
            .get("total_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        cache_read_tokens,
        cache_write_tokens,
    })
}

fn assistant_tool_message(state: &StreamState) -> Value {
    let tool_calls = state
        .tool_calls
        .iter()
        .filter(|call| !call.name.is_empty())
        .map(|call| {
            json!({
                "id": call.id,
                "type": "function",
                "function": {
                    "name": call.name,
                    "arguments": call.arguments,
                }
            })
        })
        .collect::<Vec<_>>();

    json!({
        "role": "assistant",
        "content": state.content,
        "tool_calls": tool_calls,
    })
}

fn is_tool_parameter_error(body: &str) -> bool {
    let lower = body.to_lowercase();
    lower.contains("tool") || lower.contains("function call")
}

fn is_stream_options_error(body: &str) -> bool {
    let lower = body.to_lowercase();
    lower.contains("stream_options") || lower.contains("include_usage")
}

/// Extract a concise, human-readable error message from a provider HTTP error body.
/// Parses `{"error":{"message":"..."}}`, strips verbose class prefixes (e.g. litellm.*),
/// takes only the first line, and truncates to 120 chars.
fn extract_api_error(body: &str) -> String {
    // Try to pull error.message out of the JSON
    let extracted = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| {
            v.pointer("/error/message")
                .or_else(|| v.get("message"))
                .and_then(|m| m.as_str())
                .map(str::to_string)
        });

    let raw = extracted.as_deref().unwrap_or(body);

    // First line only
    let line = raw.lines().next().unwrap_or(raw).trim();

    // Strip a leading "Namespace.ErrorClass: " or "ClassName: " prefix once
    // (covers litellm.BadRequestError, OpenAIException, etc.)
    let stripped = if let Some(colon) = line.find(": ") {
        let prefix = &line[..colon];
        if !prefix.is_empty()
            && prefix
                .chars()
                .all(|c| c.is_alphanumeric() || c == '.' || c == '_')
        {
            line[colon + 2..].trim_start()
        } else {
            line
        }
    } else {
        line
    };

    if stripped.len() > 120 {
        format!("{}…", &stripped[..120])
    } else {
        stripped.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(content: &str) -> Value {
        json!({ "choices": [{ "delta": { "content": content } }] })
    }

    #[test]
    fn accumulates_content_tokens() {
        let mut state = StreamState::default();
        assert_eq!(state.apply_chunk(&chunk("Hel")), Some("Hel".to_string()));
        assert_eq!(state.apply_chunk(&chunk("lo")), Some("lo".to_string()));
        assert_eq!(state.content, "Hello");
        assert!(state.tool_calls.is_empty());
    }

    #[test]
    fn ignores_empty_content_delta() {
        let mut state = StreamState::default();
        assert_eq!(state.apply_chunk(&chunk("")), None);
        assert_eq!(
            state.apply_chunk(&json!({ "choices": [{ "delta": {} }] })),
            None
        );
    }

    #[test]
    fn merges_fragmented_tool_call_deltas() {
        let mut state = StreamState::default();
        state.apply_chunk(&json!({
            "choices": [{ "delta": { "tool_calls": [{
                "index": 0, "id": "call_1", "function": { "name": "read_file", "arguments": "{\"pa" }
            }] } }]
        }));
        state.apply_chunk(&json!({
            "choices": [{ "delta": { "tool_calls": [{
                "index": 0, "function": { "arguments": "th\":\"x\"}" }
            }] } }]
        }));
        assert_eq!(state.tool_calls.len(), 1);
        assert_eq!(state.tool_calls[0].id, "call_1");
        assert_eq!(state.tool_calls[0].name, "read_file");
        assert_eq!(state.tool_calls[0].arguments, "{\"path\":\"x\"}");
    }

    #[test]
    fn captures_finish_reason_from_final_chunk() {
        let mut state = StreamState::default();
        // Intermediate chunk: finish_reason is null and must not be recorded.
        state.apply_chunk(&json!({
            "choices": [{ "delta": { "content": "partial" }, "finish_reason": null }]
        }));
        assert_eq!(state.finish_reason, None);
        // Final chunk reports truncation.
        state.apply_chunk(&json!({
            "choices": [{ "delta": {}, "finish_reason": "length" }]
        }));
        assert_eq!(state.finish_reason.as_deref(), Some("length"));
        assert_eq!(state.content, "partial");
    }

    #[test]
    fn length_continuation_message_asks_to_resume_without_repeating() {
        let message = length_continuation_message();
        assert_eq!(message["role"], json!("system"));
        let content = message["content"].as_str().unwrap();
        assert!(content.contains("cut off"));
        assert!(content.contains("Do not repeat"));
    }

    #[test]
    fn parses_usage_chunk() {
        let mut state = StreamState::default();
        state.apply_chunk(&json!({
            "choices": [],
            "usage": { "prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15 }
        }));
        let usage = state.usage.expect("usage parsed");
        assert_eq!(usage.prompt_tokens, 10);
        assert_eq!(usage.completion_tokens, 5);
        assert_eq!(usage.total_tokens, 15);
    }

    #[test]
    fn ingest_line_handles_sse_framing() {
        let mut state = StreamState::default();
        assert_eq!(
            state.ingest_line("data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}"),
            Some("hi".to_string())
        );
        assert_eq!(state.ingest_line(""), None);
        assert_eq!(state.ingest_line("data: [DONE]"), None);
        assert!(state.done);
        assert_eq!(
            state.ingest_line("data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}]}"),
            None
        );
    }

    #[test]
    fn detects_parameter_errors() {
        assert!(is_tool_parameter_error("This model does not support tools"));
        assert!(is_stream_options_error("Unknown field stream_options"));
        assert!(!is_stream_options_error("rate limit exceeded"));
    }

    #[test]
    fn parses_tool_round_limit() {
        assert_eq!(parse_tool_round_limit(None), DEFAULT_MAX_TOOL_ROUNDS);
        assert_eq!(parse_tool_round_limit(Some("12")), 12);
        assert_eq!(parse_tool_round_limit(Some("0")), DEFAULT_MAX_TOOL_ROUNDS);
        assert_eq!(
            parse_tool_round_limit(Some("nope")),
            DEFAULT_MAX_TOOL_ROUNDS
        );
        assert_eq!(parse_tool_round_limit(Some("999")), HARD_MAX_TOOL_ROUNDS);
    }

    #[test]
    fn tool_limit_message_forces_final_answer() {
        let message = tool_limit_message(3);
        assert_eq!(message["role"], json!("system"));
        assert!(
            message["content"]
                .as_str()
                .unwrap()
                .contains("Do not call tools again")
        );
    }

    #[test]
    fn detects_unfinished_tool_intent() {
        assert!(looks_like_unfinished_tool_intent(
            "Let me inspect the workspace structure more thoroughly."
        ));
        assert!(looks_like_unfinished_tool_intent(
            "Let me also peek at the key files to understand the project:"
        ));
        assert!(!looks_like_unfinished_tool_intent(
            "The project is a Rust CLI with an npm wrapper."
        ));
        assert!(!looks_like_unfinished_tool_intent(""));
    }

    #[test]
    fn parses_tool_result_status() {
        assert_eq!(parse_tool_result_status(r#"{"ok":true}"#), (true, None));
        assert_eq!(
            parse_tool_result_status(r#"{"ok":false,"error":"boom"}"#),
            (false, Some("boom".to_string()))
        );
        assert_eq!(
            parse_tool_result_status(r#"{"content":"no explicit ok"}"#),
            (true, None)
        );
    }
}
