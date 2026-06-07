use std::{fs, process::Command as Proc};

use super::{App, Mode, Msg, TuiEvent};
use crate::{
    export_conversation,
    provider::{ApprovalPolicy, ChatMessage, PromptRequest},
    unix_now,
};

fn git(args: &[&str]) -> Option<String> {
    let out = Proc::new("git").args(args).output().ok()?;
    if out.status.success() {
        Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        None
    }
}

// Returns true if the command was consumed (don't send to AI).
pub(super) fn handle_slash_command(app: &mut App, text: &str) -> bool {
    match text {
        "/exit" | "/quit" | ":q" => {
            app.quit = true;
            true
        }
        "/clear" => {
            app.view.messages.clear();
            app.conv.history.clear();
            app.live.streaming_buf.clear();
            app.live.accumulated_response.clear();
            app.usage = crate::provider::Usage::default();
            app.kbd.pending_images.clear();
            app.conv.seen_paths.clear();
            app.conv.undo_stack.clear();
            app.kbd.input.clear();
            app.kbd.input_cursor = 0;
            if let Some(path) = &app.conv.session_path {
                let _ = fs::remove_file(path);
            }
            true
        }
        "/help" => {
            app.view.messages.push(Msg::System(
                "Commands:\n\
                 /add <file>   inject a file into context (Aider-style)\n\
                 /diff         show git diff HEAD\n\
                 /commit [msg] stage all + commit (omit msg to ask AI)\n\
                 /memory <txt> save a note to .anveesa.md\n\
                 /clear        reset conversation\n\
                 /undo         restore last file changed by AI\n\
                 /compact      drop old turns to free context\n\
                 /copy         copy last response to clipboard\n\
                 /export [path] save conversation as markdown\n\
                 /model [name] · /provider [name] · /status · /exit\n\
                 \n\
                 Keys: ↑/↓ history  ←/→ cursor  Home/End  Shift+Enter newline\n\
                 Tab     complete /command, /provider name, or file path\n\
                 Ctrl+R  search conversation  (or /search)\n\
                 [ ]     navigate between diffs/thinking  Enter expand/collapse\n\
                 j/k scroll (when input empty)  PageUp/Dn scroll\n\
                 ⌘V (macOS) / Ctrl+V  paste image or text\n\
                 Ctrl+W delete-word  Ctrl+U clear line\n\
                 \n\
                 Search: set BRAVE_SEARCH_API_KEY or SERPER_API_KEY for better results"
                    .into(),
            ));
            app.kbd.input.clear();
            app.kbd.input_cursor = 0;
            true
        }
        "/status" => {
            let u = &app.usage;
            app.view.messages.push(Msg::System(format!(
                "provider: {}  model: {}  turns: {}  tokens: {}↓ {}↑ {} total",
                app.provider,
                app.model,
                app.conv.history.len() / 2,
                u.prompt_tokens,
                u.completion_tokens,
                u.total_tokens,
            )));
            app.kbd.input.clear();
            app.kbd.input_cursor = 0;
            true
        }
        "/copy" => {
            let last = app.view.messages.iter().rev().find_map(|m| {
                if let Msg::Assistant { text } = m {
                    Some(text.clone())
                } else {
                    None
                }
            });
            match last {
                Some(text) => {
                    if super::render::write_to_clipboard(&text) {
                        app.view
                            .messages
                            .push(Msg::System("Last response copied to clipboard.".into()));
                    } else {
                        app.view.messages.push(Msg::Error(
                            "Could not write to clipboard (pbcopy/xclip/wl-copy not found).".into(),
                        ));
                    }
                }
                None => app
                    .view
                    .messages
                    .push(Msg::System("No assistant response to copy yet.".into())),
            }
            app.kbd.input.clear();
            app.kbd.input_cursor = 0;
            true
        }
        "/undo" => {
            match app.conv.undo_stack.pop() {
                None => app
                    .view
                    .messages
                    .push(Msg::System("Nothing to undo.".into())),
                Some((path, Some(old_content))) => match std::fs::write(&path, &old_content) {
                    Ok(()) => app
                        .view
                        .messages
                        .push(Msg::System(format!("Restored {path}"))),
                    Err(e) => app
                        .view
                        .messages
                        .push(Msg::Error(format!("Undo failed: {e}"))),
                },
                Some((path, None)) => {
                    // File was newly created — delete it
                    match std::fs::remove_file(&path) {
                        Ok(()) => app
                            .view
                            .messages
                            .push(Msg::System(format!("Deleted {path} (undo create)"))),
                        Err(e) => app
                            .view
                            .messages
                            .push(Msg::Error(format!("Undo failed: {e}"))),
                    }
                }
            }
            app.kbd.input.clear();
            app.kbd.input_cursor = 0;
            true
        }
        "/compact" => {
            let keep = 10usize;
            let total_turns = app.conv.history.len() / 2;
            if total_turns <= keep {
                app.view.messages.push(Msg::System(format!(
                    "Conversation has {total_turns} turn(s) — nothing to compact yet (threshold: {keep})."
                )));
            } else {
                let drop_turns = total_turns - keep;
                let drop_msgs = drop_turns * 2;
                // Summarise dropped content before removing it
                let dropped_text: String = app.conv.history[..drop_msgs]
                    .iter()
                    .map(|m| m.content.as_str())
                    .collect::<Vec<_>>()
                    .join(" ");
                // Extract file paths mentioned in dropped turns
                let mut files: Vec<&str> = dropped_text
                    .split_whitespace()
                    .filter(|w| {
                        w.contains('/')
                            || w.ends_with(".rs")
                            || w.ends_with(".ts")
                            || w.ends_with(".py")
                            || w.ends_with(".js")
                            || w.ends_with(".go")
                    })
                    .collect::<std::collections::HashSet<_>>()
                    .into_iter()
                    .collect();
                files.sort();
                files.truncate(12);
                let file_hint = if files.is_empty() {
                    String::new()
                } else {
                    format!("  Files discussed: {}.", files.join(", "))
                };
                app.conv.history.drain(..drop_msgs);
                let msg_count = app.view.messages.len();
                if msg_count > keep * 3 {
                    app.view.messages.drain(..(msg_count - keep * 3));
                }
                app.conv.seen_paths.clear();
                // Inject a context breadcrumb so the model knows what was compacted
                app.conv.history.insert(
                    0,
                    crate::provider::ChatMessage::user(format!(
                        "[Context note: {drop_turns} earlier turn(s) were compacted.{file_hint} \
                     Use read_file / list_dir to re-inspect any files if needed.]"
                    )),
                );
                app.view.messages.insert(0, Msg::System(format!(
                    "Context compacted: dropped {drop_turns} older turn(s), keeping the last {keep}.{file_hint}"
                )));
                app.view.messages.push(Msg::Separator);

                // Spawn AI-powered summary of dropped content
                let tx = app.stream_tx.clone();
                let config = app.config.clone();
                let provider_name = app.provider.clone();
                let sample: String = dropped_text.chars().take(5_000).collect();
                tokio::spawn(async move {
                    let (dummy_tx, _) =
                        tokio::sync::mpsc::unbounded_channel::<crate::provider::StreamEvent>();
                    let request = PromptRequest {
                        prompt: format!(
                            "Summarize the following conversation in 2-3 concise sentences. \
                             Focus on: which files were discussed or modified, what was decided, \
                             what was implemented. Output ONLY the summary, no preamble:\n\n{sample}"
                        ),
                        model: None,
                        system: None,
                        workspace_context: None,
                        history: vec![],
                        images: vec![],
                        mcp: None,
                    };
                    if let Ok(result) = crate::provider::ask(
                        &config,
                        &provider_name,
                        request,
                        ApprovalPolicy::Deny,
                        &dummy_tx,
                    )
                    .await
                        && !result.text.is_empty()
                    {
                        let _ = tx.send(TuiEvent::SystemMsg(format!(
                            "Compact summary: {}",
                            result.text.trim()
                        )));
                    }
                });
            }
            app.kbd.input.clear();
            app.kbd.input_cursor = 0;
            true
        }
        "/search" => {
            app.view.search_scroll_saved = app.view.scroll;
            app.view.search_query.clear();
            app.view.search_results.clear();
            app.view.search_idx = 0;
            app.mode = Mode::Search;
            app.kbd.input.clear();
            app.kbd.input_cursor = 0;
            true
        }
        s if s.starts_with("/export") => {
            let arg = s.strip_prefix("/export").unwrap().trim();
            let path = if arg.is_empty() {
                std::path::PathBuf::from(format!("anveesa-export-{}.md", unix_now()))
            } else {
                std::path::PathBuf::from(arg)
            };
            match export_conversation(&path, &app.conv.history) {
                Ok(()) => app
                    .view
                    .messages
                    .push(Msg::System(format!("Exported → {}", path.display()))),
                Err(e) => app
                    .view
                    .messages
                    .push(Msg::Error(format!("export failed: {e:#}"))),
            }
            app.kbd.input.clear();
            app.kbd.input_cursor = 0;
            true
        }
        s if s.starts_with("/model") => {
            let arg = s.strip_prefix("/model").unwrap().trim();
            if arg.is_empty() {
                let current = app.model.clone();
                app.view
                    .messages
                    .push(Msg::System(format!("current model: {current}")));
            } else {
                app.model = arg.to_string();
                app.options.model = Some(arg.to_string());
                app.view
                    .messages
                    .push(Msg::System(format!("switched to model: {arg}")));
            }
            app.kbd.input.clear();
            app.kbd.input_cursor = 0;
            true
        }
        s if s.starts_with("/provider") => {
            let arg = s.strip_prefix("/provider").unwrap().trim();
            if arg.is_empty() {
                let current = app.provider.clone();
                app.view
                    .messages
                    .push(Msg::System(format!("current provider: {current}")));
            } else {
                // Validate provider exists
                if app.config.providers.contains_key(arg) {
                    app.provider = arg.to_string();
                    app.options.provider = Some(arg.to_string());
                    // Update model to provider default
                    if let Some(m) = app
                        .config
                        .providers
                        .get(arg)
                        .and_then(|p| p.default_model())
                    {
                        app.model = m.to_string();
                        app.options.model = Some(m.to_string());
                    }
                    app.view
                        .messages
                        .push(Msg::System(format!("switched to provider: {arg}")));
                } else {
                    app.view
                        .messages
                        .push(Msg::Error(format!("unknown provider '{arg}'")));
                }
            }
            app.kbd.input.clear();
            app.kbd.input_cursor = 0;
            true
        }
        // ── /add <path> — inject a file into the conversation context ─────────
        s if s.starts_with("/add ") => {
            let path_str = s.strip_prefix("/add ").unwrap().trim();
            let path = std::path::Path::new(path_str);
            match fs::read_to_string(path) {
                Ok(content) => {
                    let lang = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                    let line_count = content.lines().count();
                    let capped: String = content.chars().take(40_000).collect();
                    let truncated = capped.len() < content.len();
                    let note = if truncated {
                        " (truncated to 40k chars)"
                    } else {
                        ""
                    };
                    let injected = format!(
                        "File added to context: {path_str}{note}\n\n```{lang}\n{capped}\n```"
                    );
                    app.conv.history.push(ChatMessage::user(injected));
                    app.view.messages.push(Msg::System(format!(
                        "Added {path_str} ({line_count} lines){note}"
                    )));
                }
                Err(e) => app
                    .view
                    .messages
                    .push(Msg::Error(format!("Cannot read '{path_str}': {e}"))),
            }
            app.kbd.input.clear();
            app.kbd.input_cursor = 0;
            true
        }

        // ── /diff — show git diff ──────────────────────────────────────────────
        "/diff" => {
            let diff = git(&["diff", "HEAD"]).unwrap_or_default();
            if diff.is_empty() {
                let staged = git(&["diff", "--cached"]).unwrap_or_default();
                if staged.is_empty() {
                    app.view
                        .messages
                        .push(Msg::System("No changes (working tree clean).".into()));
                } else {
                    let capped: String = staged.chars().take(8_000).collect();
                    app.view
                        .messages
                        .push(Msg::System(format!("Staged diff:\n{capped}")));
                }
            } else {
                let capped: String = diff.chars().take(8_000).collect();
                app.view
                    .messages
                    .push(Msg::System(format!("git diff HEAD:\n{capped}")));
            }
            app.kbd.input.clear();
            app.kbd.input_cursor = 0;
            true
        }

        // ── /commit [msg] — stage all and commit ──────────────────────────────
        s if s == "/commit" || s.starts_with("/commit ") => {
            let status = git(&["status", "--short"]).unwrap_or_default();
            if status.is_empty() {
                app.view.messages.push(Msg::System(
                    "Nothing to commit (working tree clean).".into(),
                ));
                app.kbd.input.clear();
                app.kbd.input_cursor = 0;
                return true;
            }

            let commit_msg = if s == "/commit" {
                None
            } else {
                Some(s.strip_prefix("/commit ").unwrap().trim().to_string())
            };

            if let Some(msg) = commit_msg {
                // Stage all and commit with provided message
                match Proc::new("git").args(["add", "-A"]).status() {
                    Ok(s) if s.success() => {}
                    _ => {
                        app.view
                            .messages
                            .push(Msg::Error("git add -A failed.".into()));
                        app.kbd.input.clear();
                        app.kbd.input_cursor = 0;
                        return true;
                    }
                }
                match Proc::new("git").args(["commit", "-m", &msg]).output() {
                    Ok(out) if out.status.success() => {
                        let stdout = String::from_utf8_lossy(&out.stdout);
                        app.view
                            .messages
                            .push(Msg::System(format!("Committed: {msg}\n{}", stdout.trim())));
                    }
                    Ok(out) => {
                        let stderr = String::from_utf8_lossy(&out.stderr);
                        app.view
                            .messages
                            .push(Msg::Error(format!("git commit failed: {}", stderr.trim())));
                    }
                    Err(e) => app
                        .view
                        .messages
                        .push(Msg::Error(format!("git commit error: {e}"))),
                }
            } else {
                // No message — ask AI to generate one
                let diff_stat = git(&["diff", "HEAD", "--stat"])
                    .or_else(|| git(&["diff", "--cached", "--stat"]))
                    .unwrap_or_else(|| status.clone());
                app.view.messages.push(Msg::System(format!(
                    "Changes to commit:\n{diff_stat}\n\nGenerating commit message…"
                )));
                let tx = app.stream_tx.clone();
                let config = app.config.clone();
                let provider_name = app.provider.clone();
                tokio::spawn(async move {
                    let (dummy_tx, _) =
                        tokio::sync::mpsc::unbounded_channel::<crate::provider::StreamEvent>();
                    let request = PromptRequest {
                        prompt: format!(
                            "Generate a concise git commit message for these changes.\n\
                             Rules: imperative mood, ≤72 chars, no trailing period.\n\
                             Output ONLY the commit message text, nothing else:\n\n{diff_stat}"
                        ),
                        model: None,
                        system: None,
                        workspace_context: None,
                        history: vec![],
                        images: vec![],
                        mcp: None,
                    };
                    if let Ok(result) = crate::provider::ask(
                        &config,
                        &provider_name,
                        request,
                        ApprovalPolicy::Deny,
                        &dummy_tx,
                    )
                    .await
                    {
                        let msg = result.text.trim().to_string();
                        let _ = tx.send(TuiEvent::SetInput(format!("/commit {}", msg.trim())));
                    }
                });
            }
            app.kbd.input.clear();
            app.kbd.input_cursor = 0;
            true
        }

        // ── /memory <note> — append a note to .anveesa.md ────────────────────
        s if s.starts_with("/memory ") => {
            let note = s.strip_prefix("/memory ").unwrap().trim();
            let path = std::path::Path::new(".anveesa.md");
            let header = if !path.exists() {
                "# Anveesa project notes\n\n"
            } else {
                ""
            };
            let entry = format!("{header}- {note}\n");
            match fs::OpenOptions::new().create(true).append(true).open(path) {
                Ok(mut f) => {
                    use std::io::Write;
                    let _ = f.write_all(entry.as_bytes());
                    app.view
                        .messages
                        .push(Msg::System(format!("Saved to .anveesa.md: {note}")));
                }
                Err(e) => app
                    .view
                    .messages
                    .push(Msg::Error(format!("Cannot write .anveesa.md: {e}"))),
            }
            app.kbd.input.clear();
            app.kbd.input_cursor = 0;
            true
        }

        _ => false,
    }
}
