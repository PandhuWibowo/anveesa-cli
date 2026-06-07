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

    tokio::spawn(async move {
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

    tokio::spawn(async move {
        let mut rx = stream_rx_inner;
        while let Some(ev) = rx.recv().await {
            let tui_ev = match ev {
                StreamEvent::Token(t) => TuiEvent::Token(t),
                StreamEvent::Thinking(t) => TuiEvent::Thinking(t),
                StreamEvent::Status { message } => TuiEvent::Status(message),
                StreamEvent::ToolCall { summary } => TuiEvent::ToolCall(summary),
                StreamEvent::ToolResult { summary, ok, .. } => TuiEvent::ToolDone { summary, ok },
                StreamEvent::FileOp {
                    verb,
                    path,
                    added,
                    removed,
                    preview,
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
            commit_pending_tool(app, true);
            app.live.pending_tool = Some(PendingTool {
                summary: summary.clone(),
            });
            app.live.tool_started_at = Some(Instant::now());
            app.live.tool_status = summary;
        }
        TuiEvent::ToolDone { summary, ok } => {
            let elapsed_ms = app
                .live
                .tool_started_at
                .take()
                .map(|t| t.elapsed().as_millis());
            record_seen_path(&mut app.conv.seen_paths, &summary);
            app.live.pending_tool = Some(PendingTool { summary });
            commit_pending_tool_timed(app, ok, elapsed_ms);
            app.live.tool_status = "Thinking".to_string();
        }
        TuiEvent::FileOp {
            verb,
            path,
            added,
            removed,
            diff,
        } => {
            flush_streaming_buf(app);
            commit_pending_tool(app, true);
            let old_content = std::fs::read_to_string(&path).ok();
            if app.conv.undo_stack.len() >= 20 {
                app.conv.undo_stack.remove(0);
            }
            app.conv.undo_stack.push((path.clone(), old_content));
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
            commit_pending_tool(app, true);
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
            flush_streaming_buf(app);
            app.view.messages.push(Msg::Error(msg));
            app.mode = Mode::Input;
            app.view.auto_scroll = true;   // Reset for next turn.
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

pub(super) fn commit_pending_tool(app: &mut App, ok: bool) {
    let elapsed = app
        .live
        .tool_started_at
        .take()
        .map(|t| t.elapsed().as_millis());
    commit_pending_tool_timed(app, ok, elapsed);
}

pub(super) fn commit_pending_tool_timed(app: &mut App, ok: bool, elapsed_ms: Option<u128>) {
    if let Some(tool) = app.live.pending_tool.take() {
        app.view.messages.push(Msg::Tool {
            done: true,
            ok,
            text: tool.summary,
            elapsed_ms,
        });
    }
}

pub(super) fn finish_turn(app: &mut App) {
    commit_pending_tool(app, true);
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
    app.view.auto_scroll = true;   // Next turn should auto-scroll by default.
    app.view.scroll = usize::MAX;  // Reset scroll so render.rs picks the bottom.
    app.live.tool_status.clear();
    app.live.streaming_started_at = None;
    app.live.tool_started_at = None;
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
    let drop_msgs = drop_turns * 2;
    app.conv.history.drain(..drop_msgs);
    app.conv.seen_paths.clear();
    app.conv.history.insert(
        0,
        ChatMessage::user(format!(
            "[Context note: {drop_turns} earlier turn(s) were auto-compacted. \
         Use read_file / list_dir to re-inspect any files if needed.]"
        )),
    );
    app.view.messages.push(Msg::System(format!(
        "Auto-compacted: dropped {drop_turns} older turn(s) (~{}k est. tokens). Use /compact for manual control.",
        estimated / 1000
    )));
}
