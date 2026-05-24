use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use serde_json::{Value, json};
use tokio::sync::{mpsc::UnboundedSender, oneshot};

use crate::{
    config::OpenAiCompatibleProviderConfig,
    provider::{ApprovalPolicy, ChatRole, PromptRequest, StreamEvent, TurnResult, Usage},
    tools,
};

const MAX_TOOL_ROUNDS: usize = 8;
const MAX_RETRIES: usize = 2;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

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

    let headers = build_headers(config)?;
    let mut messages = build_messages(&request, policy);

    let client = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .build()
        .context("failed to build HTTP client")?;
    let url = format!("{}/chat/completions", config.base_url.trim_end_matches('/'));

    let mut tools_enabled = true;
    let mut usage_requested = true;
    let mut tool_rounds = 0usize;
    let mut full_text = String::new();
    let mut last_usage: Option<Usage> = None;

    loop {
        let mut body = json!({
            "model": model,
            "messages": messages,
            "stream": true,
        });
        if usage_requested {
            body["stream_options"] = json!({ "include_usage": true });
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
            bail!("provider '{provider_name}' returned HTTP {status}: {response_body}");
        }

        let mut state = StreamState::default();
        stream_response(response, &mut state, events).await?;

        full_text.push_str(&state.content);
        if let Some(usage) = state.usage {
            last_usage = Some(usage);
        }

        if state.tool_calls.is_empty() {
            break;
        }

        if !tools_enabled {
            break;
        }
        if tool_rounds >= MAX_TOOL_ROUNDS {
            bail!("tool call limit reached ({MAX_TOOL_ROUNDS} rounds)");
        }
        tool_rounds += 1;

        messages.push(assistant_tool_message(&state));
        for call in &state.tool_calls {
            let content = dispatch_tool(call, policy, events).await;
            messages.push(json!({
                "role": "tool",
                "tool_call_id": call.id,
                "name": call.name,
                "content": content,
            }));
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

async fn dispatch_tool(
    call: &PartialToolCall,
    policy: ApprovalPolicy,
    events: &UnboundedSender<StreamEvent>,
) -> String {
    if tools::is_write_tool(&call.name) {
        if !policy.allows_write_tools() {
            return denied_message("write tools are disabled (pass --yes or run interactively)");
        }
        if policy == ApprovalPolicy::Prompt && !request_approval(call, events).await {
            return denied_message("user declined this action");
        }
    }

    tools::run(&call.name, &call.arguments).await
}

fn denied_message(reason: &str) -> String {
    json!({ "ok": false, "error": reason }).to_string()
}

async fn request_approval(
    call: &PartialToolCall,
    events: &UnboundedSender<StreamEvent>,
) -> bool {
    let (reply, answer) = oneshot::channel();
    let event = StreamEvent::Confirm {
        summary: tools::describe_call(&call.name, &call.arguments),
        reply,
    };
    if events.send(event).is_err() {
        return false;
    }
    answer.await.unwrap_or(false)
}

fn build_headers(config: &OpenAiCompatibleProviderConfig) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

    if let Some(api_key_env) = &config.api_key_env {
        let api_key = std::env::var(api_key_env)
            .with_context(|| format!("environment variable {api_key_env} is required"))?;
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

    Ok(headers)
}

fn build_messages(request: &PromptRequest, policy: ApprovalPolicy) -> Vec<Value> {
    let mut messages = Vec::new();
    if let Some(system) = &request.system {
        messages.push(json!({ "role": "system", "content": system }));
    }
    if let Some(workspace_context) = &request.workspace_context {
        messages.push(json!({ "role": "system", "content": workspace_context }));
    }
    messages.push(json!({ "role": "system", "content": tools::guidance(policy.allows_write_tools()) }));
    for message in &request.history {
        let role = match message.role {
            ChatRole::User => "user",
            ChatRole::Assistant => "assistant",
        };
        messages.push(json!({ "role": role, "content": message.content }));
    }
    messages.push(json!({ "role": "user", "content": request.prompt }));
    messages
}

async fn send_with_retry(
    client: &reqwest::Client,
    url: &str,
    headers: &HeaderMap,
    body: &Value,
) -> Result<reqwest::Response> {
    let mut attempt = 0usize;
    loop {
        match client.post(url).headers(headers.clone()).json(body).send().await {
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

    while let Some(chunk) = response
        .chunk()
        .await
        .context("failed to read streamed response chunk")?
    {
        buffer.push_str(&String::from_utf8_lossy(&chunk));

        while let Some(newline) = buffer.find('\n') {
            let line: String = buffer.drain(..=newline).collect();
            if let Some(token) = state.ingest_line(line.trim_end_matches(['\r', '\n'])) {
                let _ = events.send(StreamEvent::Token(token));
            }
        }
    }

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
        let chunk: Value = serde_json::from_str(data).ok()?;
        self.apply_chunk(&chunk)
    }

    fn apply_chunk(&mut self, chunk: &Value) -> Option<String> {
        if let Some(usage) = chunk.get("usage").filter(|value| value.is_object()) {
            self.usage = parse_usage(usage);
        }

        let delta = chunk.get("choices")?.get(0)?.get("delta")?;

        if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
            for call in tool_calls {
                self.apply_tool_call_delta(call);
            }
        }

        let text = delta.get("content").and_then(Value::as_str)?;
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
    Some(Usage {
        prompt_tokens: object.get("prompt_tokens").and_then(Value::as_u64).unwrap_or(0),
        completion_tokens: object
            .get("completion_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        total_tokens: object.get("total_tokens").and_then(Value::as_u64).unwrap_or(0),
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
        assert_eq!(state.apply_chunk(&json!({ "choices": [{ "delta": {} }] })), None);
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
        assert_eq!(state.ingest_line("data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}"), Some("hi".to_string()));
        assert_eq!(state.ingest_line(""), None);
        assert_eq!(state.ingest_line("data: [DONE]"), None);
        assert!(state.done);
        assert_eq!(state.ingest_line("data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}]}"), None);
    }

    #[test]
    fn detects_parameter_errors() {
        assert!(is_tool_parameter_error("This model does not support tools"));
        assert!(is_stream_options_error("Unknown field stream_options"));
        assert!(!is_stream_options_error("rate limit exceeded"));
    }
}
