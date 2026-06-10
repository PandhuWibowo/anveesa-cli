use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tokio::sync::mpsc;

use crate::provider::{
    ChatMessage, DiffKind, ImageAttachment, PromptRequest, StreamEvent, ToolConfirmPreview,
};

use super::{App, Mode, Msg, PendingConfirm, PendingTool, TuiEvent};

pub(super) async fn submit_prompt(app: &mut App, text: String) -> Result<()> {
    if app.kbd.input_history.last().map(|s| s.as_str()) != Some(&text) {
        app.kbd.input_history.push(text.clone());
    }
    app.kbd.hist_idx = None;
    app.live.pending_prompt = text.clone();
    app.live.accumulated_response.clear();

    let images: Vec<ImageAttachment> = if !app.kbd.pending_images.is_empty() {
        std::mem::take(&mut app.kbd.pending_images)
    } else if app.images_available {
        if let Some(img) = crate::image::grab_clipboard_image() {
            let fp = crate::image::image_fingerprint(&img);
            if app.kbd.last_image_fp.as_deref() == Some(&fp) {
                vec![]
            } else {
                app.kbd.last_image_fp = Some(fp);
                vec![img]
            }
        } else {
            vec![]
        }
    } else {
        vec![]
    };

    app.view.messages.push(Msg::User { text: text.clone() });
    app.kbd.input.clear();
    app.kbd.input_cursor = 0;
    app.view.auto_scroll = true;
    app.mode = Mode::Streaming;
    app.live.tool_status = "Thinking".to_string();
    app.spinner_frame = 0;

    let provider_name = app
        .config
        .provider_name(app.options.provider.as_deref())
        .context("unknown provider")?
        .to_string();

    let (stream_tx_inner, stream_rx_inner) = mpsc::unbounded_channel::<StreamEvent>();

    let config = app.config.clone();
    let options = app.options.clone();
    let history = app.conv.history.clone();
    let workspace_context =
        augmented_workspace_context(app.workspace_context.as_deref(), &app.conv.seen_paths);
    let policy = app.policy;
    let mcp_arc = app.mcp.clone();
    let tui_tx = app.stream_tx.clone();
    let tui_tx2 = app.stream_tx.clone();

    let turn_task = tokio::spawn(async move {
        let request = PromptRequest {
            prompt: text,
            model: options.model.clone(),
            system: options.system.clone(),
            workspace_context,
            history,
            images,
            mcp: mcp_arc,
        };
        let result =
            crate::provider::ask(&config, &provider_name, request, policy, &stream_tx_inner).await;
        drop(stream_tx_inner);
        match result {
            Ok(turn) => {
                if let Some(m) = turn.model_used {
                    let _ = tui_tx.send(TuiEvent::ModelUsed(m));
                }
                let _ = tui_tx.send(TuiEvent::Usage(turn.usage.unwrap_or_default()));
            }
            Err(e) => {
                let _ = tui_tx.send(TuiEvent::Error(format!("{e:#}")));
            }
        }
    });
    app.live.turn_abort = Some(turn_task.abort_handle());

    tokio::spawn(async move {
        let mut rx = stream_rx_inner;
        while let Some(ev) = rx.recv().await {
            let tui_ev = match ev {
                StreamEvent::Token(t) => TuiEvent::Token(t),
                StreamEvent::Thinking(t) => TuiEvent::Thinking(t),
                StreamEvent::Status { message } => TuiEvent::Status(message),
                StreamEvent::ToolCall { summary } => TuiEvent::ToolCall(summary),
                StreamEvent::ToolResult {
                    summary,
                    ok,
                    elapsed_ms,
                    ..
                } => TuiEvent::ToolDone {
                    summary,
                    ok,
                    elapsed_ms,
                },
                StreamEvent::FileOp {
                    verb,
                    path,
                    added,
                    removed,
                    preview,
                    old_content,
                    ..
                } => {
                    let diff = preview
                        .into_iter()
                        .map(|dl| {
                            let is_add = matches!(dl.kind, DiffKind::Add);
                            (is_add, dl.text)
                        })
                        .collect();
                    TuiEvent::FileOp {
                        verb,
                        path,
                        added,
                        removed,
                        diff,
                        old_content,
                    }
                }
                StreamEvent::Confirm { preview, reply } => {
                    let (summary, diff) = match preview {
                        ToolConfirmPreview::FileOp {
                            verb,
                            path,
                            added,
                            removed,
                            diff,
                            ..
                        } => (
                            format!("{verb} {path}  +{added} -{removed}"),
                            diff.into_iter()
                                .map(|dl| (matches!(dl.kind, DiffKind::Add), dl.text))
                                .collect(),
                        ),
                        ToolConfirmPreview::CreateDir { path } => (format!("mkdir {path}"), vec![]),
                        ToolConfirmPreview::Generic { summary } => (summary, vec![]),
                    };
                    TuiEvent::Confirm {
                        summary,
                        diff,
                        reply,
                    }
                }
                StreamEvent::Usage(u) => TuiEvent::Usage(u),
                StreamEvent::PlanSet { tasks } => TuiEvent::PlanSet(tasks),
                StreamEvent::PlanTaskDone { index } => TuiEvent::PlanTaskDone(index),
            };
            if tui_tx2.send(tui_ev).is_err() {
                break;
            }
        }
    });

    Ok(())
}

pub(super) async fn handle_stream_event(app: &mut App, ev: TuiEvent) {
    // After a cancel the provider task is gone, but already-spawned tool tasks
    // may still emit events. Drop the ephemeral ones; ToolDone/FileOp are kept
    // because they describe effects that really happened.
    if app.mode == Mode::Input
        && matches!(
            ev,
            TuiEvent::Token(_)
                | TuiEvent::Thinking(_)
                | TuiEvent::Status(_)
                | TuiEvent::ToolCall(_)
        )
    {
        return;
    }
    match ev {
        TuiEvent::Token(text) => {
            if app.live.streaming_started_at.is_none() {
                app.live.streaming_started_at = Some(Instant::now());
            }
            if !app.live.thinking_buf.is_empty() {
                let text = std::mem::take(&mut app.live.thinking_buf);
                app.view.messages.push(Msg::Thinking {
                    text,
                    collapsed: true,
                });
            }
            app.live.streaming_buf.push_str(&text);
            if app.view.auto_scroll {
                app.view.scroll = usize::MAX;
            } else {
                app.live.unread_count += 1;
            }
        }
        TuiEvent::Thinking(text) => {
            if app.live.streaming_started_at.is_none() {
                app.live.streaming_started_at = Some(Instant::now());
            }
            app.live.thinking_buf.push_str(&text);
        }
        TuiEvent::ModelUsed(m) => {
            app.last_model_used = Some(m);
        }
        TuiEvent::SystemMsg(msg) => {
            app.view.messages.push(Msg::System(msg));
        }
        TuiEvent::Status(msg) => {
            app.live.tool_status = msg;
        }
        TuiEvent::ToolCall(summary) => {
            flush_streaming_buf(app);
            app.live.pending_tools.push(PendingTool {
                summary: summary.clone(),
                started_at: Instant::now(),
            });
            app.live.tool_status = summary;
        }
        TuiEvent::ToolDone {
            summary,
            ok,
            elapsed_ms,
        } => {
            record_seen_path(&mut app.conv.seen_paths, &summary);
            // Concurrent tools finish out of order — remove the matching
            // pending entry (by summary), not whichever started last.
            if let Some(i) = app
                .live
                .pending_tools
                .iter()
                .position(|t| t.summary == summary)
            {
                app.live.pending_tools.remove(i);
            }
            app.view.messages.push(Msg::Tool {
                done: true,
                ok,
                text: summary,
                elapsed_ms: Some(elapsed_ms),
            });
            app.live.tool_status = match app.live.pending_tools.len() {
                0 => "Thinking".to_string(),
                1 => app.live.pending_tools[0].summary.clone(),
                n => format!("{n} tools running"),
            };
        }
        TuiEvent::FileOp {
            verb,
            path,
            added,
            removed,
            diff,
            old_content,
        } => {
            flush_streaming_buf(app);
            // old_content was captured BEFORE the tool ran (the file already
            // has the new content by the time this event arrives). None +
            // "Create" means new file (undo = delete); None on an update means
            // no snapshot was possible (e.g. file too large) — skip undo then.
            if old_content.is_some() || verb == "Create" {
                if app.conv.undo_stack.len() >= 20 {
                    app.conv.undo_stack.remove(0);
                }
                app.conv.undo_stack.push((path.clone(), old_content));
            }
            let collapsed = diff.len() > 8;
            app.view.messages.push(Msg::FileOp {
                verb,
                path,
                added,
                removed,
                diff,
                collapsed,
            });
        }
        TuiEvent::Confirm {
            summary,
            diff,
            reply,
        } => {
            flush_streaming_buf(app);
            app.confirm = Some(PendingConfirm {
                summary,
                diff,
                reply,
            });
            app.mode = Mode::Confirming;
        }
        TuiEvent::Usage(u) => {
            app.usage.prompt_tokens += u.prompt_tokens;
            app.usage.completion_tokens += u.completion_tokens;
            app.usage.total_tokens += u.total_tokens;
            app.usage.cache_read_tokens += u.cache_read_tokens;
            app.usage.cache_write_tokens += u.cache_write_tokens;
            let (in_price, out_price, cr_price, cw_price) = {
                // Check if active provider has custom pricing configured
                let custom = app
                    .options
                    .provider
                    .as_deref()
                    .and_then(|p| app.config.providers.get(p))
                    .or_else(|| app.config.providers.get(&app.provider));
                if let Some(crate::config::ProviderConfig::OpenAiCompatible(cfg)) = custom {
                    if let Some(p) = cfg.pricing {
                        (p[0], p[1], p[2], p[3])
                    } else {
                        super::render::get_model_pricing(&app.model)
                    }
                } else {
                    super::render::get_model_pricing(&app.model)
                }
            };
            app.session_cost_usd +=
                (u.prompt_tokens as f64 - u.cache_read_tokens as f64 - u.cache_write_tokens as f64)
                    .max(0.0)
                    * in_price
                    / 1_000_000.0
                    + u.completion_tokens as f64 * out_price / 1_000_000.0
                    + u.cache_read_tokens as f64 * cr_price / 1_000_000.0
                    + u.cache_write_tokens as f64 * cw_price / 1_000_000.0;
            finish_turn(app);
        }
        TuiEvent::Error(msg) => {
            app.live.turn_abort = None;
            flush_streaming_buf(app);
            commit_all_pending_tools(app, false);
            app.view.messages.push(Msg::Error(msg));
            app.mode = Mode::Input;
            app.view.auto_scroll = true; // Reset for next turn.
            app.view.scroll = usize::MAX;
            app.live.tool_status.clear();
        }
        TuiEvent::PlanSet(tasks) => {
            app.live.plan_done = vec![false; tasks.len()];
            app.live.plan_tasks = tasks;
        }
        TuiEvent::PlanTaskDone(i) => {
            if i < app.live.plan_done.len() {
                app.live.plan_done[i] = true;
            }
        }
        TuiEvent::SetInput(text) => {
            app.kbd.input = text;
            app.kbd.input_cursor = app.kbd.input.len();
            app.kbd.hist_idx = None;
            app.kbd.tab_state = None;
        }
        TuiEvent::CompactDone {
            drop_msgs,
            first_msg,
            summary,
        } => {
            app.live.compact_in_flight = false;
            // /clear, /undo, or another compaction may have raced the
            // summarizer — only splice if the snapshot prefix is intact.
            if app.conv.history.len() < drop_msgs
                || app.conv.history.first().map(|m| m.content.as_str()) != Some(first_msg.as_str())
            {
                app.view.messages.push(Msg::System(
                    "Compaction skipped — history changed while summarizing.".to_string(),
                ));
                return;
            }
            let drop_turns = drop_msgs / 2;
            let had_summary = summary.is_some();
            let note = compact_note(drop_turns, summary.as_deref());
            app.conv.history.drain(..drop_msgs);
            app.conv.history.insert(0, ChatMessage::user(note));
            app.conv.seen_paths.clear();
            app.view.messages.push(Msg::System(if had_summary {
                format!("Compacted {drop_turns} older turn(s) into a summary.")
            } else {
                format!("Compacted: dropped {drop_turns} older turn(s) (summary unavailable).")
            }));
        }
    }
}

fn record_seen_path(seen: &mut std::collections::BTreeSet<String>, summary: &str) {
    for prefix in &["read file ", "list directory "] {
        if let Some(path) = summary.strip_prefix(prefix) {
            let path = path.trim().to_string();
            if !path.is_empty() {
                seen.insert(path);
            }
            return;
        }
    }
}

fn augmented_workspace_context(
    base: Option<&str>,
    seen: &std::collections::BTreeSet<String>,
) -> Option<String> {
    if seen.is_empty() {
        return base.map(str::to_string);
    }
    let seen_note = format!(
        "\nAlready inspected this session (do NOT re-read these):\n{}",
        seen.iter()
            .map(|p| format!("  - {p}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    Some(match base {
        Some(b) => format!("{b}{seen_note}"),
        None => seen_note,
    })
}

pub(super) fn flush_streaming_buf(app: &mut App) {
    if !app.live.streaming_buf.is_empty() {
        let text = std::mem::take(&mut app.live.streaming_buf);
        app.live.accumulated_response.push_str(&text);
        app.view.messages.push(Msg::Assistant { text });
    }
}

/// Cancel the in-flight turn (Esc / Ctrl+C during streaming). Aborts the
/// provider task — the HTTP request is dropped mid-stream — and returns the
/// UI to input mode. Partial output stays visible but is NOT saved to
/// history, matching the Error path.
pub(super) fn cancel_turn(app: &mut App) {
    let Some(handle) = app.live.turn_abort.take() else {
        return;
    };
    handle.abort();
    flush_streaming_buf(app);
    commit_all_pending_tools(app, false);
    if !app.live.thinking_buf.is_empty() {
        let text = std::mem::take(&mut app.live.thinking_buf);
        app.view.messages.push(Msg::Thinking {
            text,
            collapsed: true,
        });
    }
    app.live.accumulated_response.clear();
    app.live.pending_prompt.clear();
    app.view
        .messages
        .push(Msg::System("Cancelled. (response not saved)".to_string()));
    app.mode = Mode::Input;
    app.view.auto_scroll = true;
    app.view.scroll = usize::MAX;
    app.live.tool_status.clear();
    app.live.streaming_started_at = None;
}

/// Commit any tools still marked pending (e.g. the stream ended or errored
/// before their ToolDone arrived) so no spinner is left behind.
pub(super) fn commit_all_pending_tools(app: &mut App, ok: bool) {
    for tool in app.live.pending_tools.drain(..) {
        app.view.messages.push(Msg::Tool {
            done: true,
            ok,
            text: tool.summary,
            elapsed_ms: Some(tool.started_at.elapsed().as_millis()),
        });
    }
}

pub(super) fn finish_turn(app: &mut App) {
    app.live.turn_abort = None;
    commit_all_pending_tools(app, true);
    if !app.live.thinking_buf.is_empty() {
        let text = std::mem::take(&mut app.live.thinking_buf);
        app.view.messages.push(Msg::Thinking {
            text,
            collapsed: true,
        });
    }
    flush_streaming_buf(app);
    let response = std::mem::take(&mut app.live.accumulated_response);
    if !response.is_empty() {
        let prompt = std::mem::take(&mut app.live.pending_prompt);
        app.conv.history.push(ChatMessage::user(prompt));
        app.conv.history.push(ChatMessage::assistant(response));
        if let Some(path) = &app.conv.session_path
            && let Ok(cwd) = std::env::current_dir()
        {
            let _ = crate::session::save_interactive_session(
                path,
                &cwd,
                &app.provider,
                &app.options,
                &app.conv.history,
            );
            app.conv.last_saved_at = crate::unix_now();
        }
    }
    if let Some(started) = app.live.streaming_started_at
        && started.elapsed() > Duration::from_secs(8)
    {
        super::render::send_desktop_notification("anveesa", "Task complete");
    }
    app.mode = Mode::Input;
    app.view.auto_scroll = true; // Next turn should auto-scroll by default.
    app.view.scroll = usize::MAX; // Reset scroll so render.rs picks the bottom.
    app.live.tool_status.clear();
    app.live.streaming_started_at = None;
    if !app.conv.history.is_empty() {
        app.view.messages.push(Msg::Separator);
    }
    auto_compact_if_needed(app);
}

fn auto_compact_if_needed(app: &mut App) {
    const TOKEN_THRESHOLD: usize = 40_000;
    let estimated: usize = app.conv.history.iter().map(|m| m.content.len() / 4).sum();
    if estimated < TOKEN_THRESHOLD || app.conv.history.len() < 8 {
        return;
    }
    let total_turns = app.conv.history.len() / 2;
    let drop_turns = (total_turns / 4).max(1).min(total_turns.saturating_sub(4));
    start_compaction(app, drop_turns * 2);
}

/// System prompt for the compaction summarizer.
const SUMMARIZER_SYSTEM: &str = "You summarize coding-assistant conversations for context \
compaction. Produce a dense summary that preserves: file paths and code identifiers, \
decisions made and why, what was implemented or changed, unresolved tasks and known \
issues, and user preferences or constraints. Use short bullet points. Output only the \
summary, no preamble.";

const COMPACT_MAX_MSG_CHARS: usize = 4_000;
const COMPACT_MAX_TRANSCRIPT_CHARS: usize = 24_000;

/// Kick off background summarization of the oldest `drop_msgs` history messages.
/// The summary is spliced into history when CompactDone arrives; the old turns
/// stay in place until then so nothing is lost if summarization fails mid-turn.
pub(super) fn start_compaction(app: &mut App, drop_msgs: usize) {
    if app.live.compact_in_flight || drop_msgs == 0 || app.conv.history.len() < drop_msgs {
        return;
    }
    let dropped: Vec<ChatMessage> = app.conv.history[..drop_msgs].to_vec();
    let first_msg = dropped[0].content.clone();
    app.live.compact_in_flight = true;
    app.view.messages.push(Msg::System(format!(
        "Compacting: summarizing {} older turn(s) in the background…",
        drop_msgs / 2
    )));

    let tx = app.stream_tx.clone();
    let summarizer = summarizer_config(app);
    tokio::spawn(async move {
        let summary = match summarizer {
            Some((cfg, model)) => crate::provider::openai_compatible::complete_text(
                &cfg,
                &model,
                SUMMARIZER_SYSTEM,
                &compact_transcript(&dropped),
            )
            .await
            .ok(),
            None => None,
        };
        let _ = tx.send(TuiEvent::CompactDone {
            drop_msgs,
            first_msg,
            summary,
        });
    });
}

/// Provider config + model to use for summarization: the provider's fast_model
/// when configured, otherwise the session model. Command providers can't
/// summarize (no direct completion API) — they fall back to a plain note.
fn summarizer_config(app: &App) -> Option<(crate::config::OpenAiCompatibleProviderConfig, String)> {
    let name = app
        .config
        .provider_name(app.options.provider.as_deref())
        .ok()?;
    match app.config.providers.get(name)? {
        crate::config::ProviderConfig::OpenAiCompatible(cfg) => {
            let model = cfg
                .fast_model
                .clone()
                .or_else(|| app.options.model.clone())
                .or_else(|| cfg.default_model.clone())?;
            Some((cfg.clone(), model))
        }
        _ => None,
    }
}

/// Render dropped turns as a plain transcript, capped so the summarization
/// request itself stays small.
fn compact_transcript(turns: &[ChatMessage]) -> String {
    use crate::provider::ChatRole;
    let mut out = String::new();
    for m in turns {
        let role = match m.role {
            ChatRole::User => "User",
            ChatRole::Assistant => "Assistant",
        };
        let snippet: String = m.content.chars().take(COMPACT_MAX_MSG_CHARS).collect();
        let truncated = if snippet.len() < m.content.len() {
            " …[truncated]"
        } else {
            ""
        };
        out.push_str(role);
        out.push_str(":\n");
        out.push_str(&snippet);
        out.push_str(truncated);
        out.push_str("\n\n");
        if out.len() > COMPACT_MAX_TRANSCRIPT_CHARS {
            out.push_str("…[transcript truncated]\n");
            break;
        }
    }
    out
}

/// The history message that replaces the compacted turns.
fn compact_note(drop_turns: usize, summary: Option<&str>) -> String {
    match summary {
        Some(s) => format!("[Summary of {drop_turns} earlier compacted turn(s):]\n{s}"),
        None => format!(
            "[Context note: {drop_turns} earlier turn(s) were compacted. \
             Use read_file / list_dir to re-inspect any files if needed.]"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ChatMessage;

    #[test]
    fn transcript_labels_roles() {
        let turns = vec![
            ChatMessage::user("fix the bug".into()),
            ChatMessage::assistant("done, edited src/lib.rs".into()),
        ];
        let t = compact_transcript(&turns);
        assert!(t.contains("User:\nfix the bug"));
        assert!(t.contains("Assistant:\ndone, edited src/lib.rs"));
    }

    #[test]
    fn transcript_truncates_long_messages() {
        let turns = vec![ChatMessage::user("x".repeat(10_000))];
        let t = compact_transcript(&turns);
        assert!(t.contains("…[truncated]"));
        assert!(t.len() < 5_000);
    }

    #[test]
    fn transcript_caps_total_size() {
        let turns: Vec<ChatMessage> = (0..20)
            .map(|i| ChatMessage::user(format!("{i}-{}", "y".repeat(3_000))))
            .collect();
        let t = compact_transcript(&turns);
        assert!(t.contains("…[transcript truncated]"));
        assert!(t.len() < COMPACT_MAX_TRANSCRIPT_CHARS + COMPACT_MAX_MSG_CHARS + 100);
    }

    #[test]
    fn transcript_handles_multibyte_truncation() {
        // char-based truncation must not split a multi-byte char
        let turns = vec![ChatMessage::user("é".repeat(5_000))];
        let t = compact_transcript(&turns);
        assert!(t.contains("…[truncated]"));
    }

    #[test]
    fn note_with_summary_embeds_it() {
        let n = compact_note(3, Some("- did things in src/lib.rs"));
        assert!(n.contains("Summary of 3 earlier compacted turn(s)"));
        assert!(n.contains("src/lib.rs"));
    }

    #[test]
    fn note_without_summary_is_plain_breadcrumb() {
        let n = compact_note(5, None);
        assert!(n.contains("5 earlier turn(s) were compacted"));
        assert!(n.contains("read_file"));
    }
}
