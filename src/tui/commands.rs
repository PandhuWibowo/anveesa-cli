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
                 /add <file>   inject file into context\n\
                 /agent [on|off]  toggle auto-approve\n\
                 /branch         show git branch info\n\
                 /commit [msg] stage all + commit\n\
                 /cost           token usage & estimated cost\n\
                 /diff         show git diff HEAD\n\
                 /memory <txt> save a note to .anveesa.md\n\
                 /note set k v   save a persistent note\n\
                 /note get k     load note into context\n\
                 /notes          list saved notes\n\
                 /clear        reset conversation\n\
                 /undo         restore last file changed by AI\n\
                 /compact      drop old turns to free context\n\
                 /copy         copy last response to clipboard\n\
                 /export [path] save conversation as markdown\n\
                 /model [name] · /provider [name] · /status · /exit\n\
                 \n\
                 Keys: ↑/↓ history  ←/→ cursor  Shift+Enter newline\n\
                 Tab     complete /command, /provider, or file path\n\
                 Ctrl+R  search  [ ] expand/collapse diffs\n\
                 j/k scroll  PageUp/Dn scroll\n\
                 ⌘V/Ctrl+V paste image  Ctrl+W del-word  Ctrl+U clear\n\
                 \n\
                 Custom: .anveesa/commands/<name>.md\n\
                 Search: set BRAVE_SEARCH_API_KEY or SERPER_API_KEY"
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

        // ── /note set <key> <value> — persist a project note ────────────────────
        s if s.starts_with("/note set ") => {
            let rest = s.strip_prefix("/note set ").unwrap().trim();
            if let Some((key, value)) = rest.split_once(' ') {
                let key = key.trim();
                let value = value.trim();
                if let Ok(path) = crate::config::config_path() {
                    let notes_dir = path.parent().unwrap().join("notes");
                    let _ = fs::create_dir_all(&notes_dir);
                    let note_path = notes_dir.join(format!("{key}.md"));
                    match fs::write(&note_path, format!("# {}\n\n{}\n", key, value)) {
                        Ok(()) => app.view.messages.push(Msg::System(format!(
                            "Note saved: {key}"
                        ))),
                        Err(e) => app.view.messages.push(Msg::Error(format!(
                            "Cannot save note: {e}"
                        ))),
                    }
                } else {
                    app.view.messages.push(Msg::Error("Cannot locate config directory.".into()));
                }
            } else {
                app.view.messages.push(Msg::System(
                    "Usage: /note set <key> <value>".into(),
                ));
            }
            app.kbd.input.clear();
            app.kbd.input_cursor = 0;
            true
        }
        // ── /note get <key> — read a saved note into context ────────────────────
        s if s.starts_with("/note get ") => {
            let key = s.strip_prefix("/note get ").unwrap().trim();
            if let Ok(path) = crate::config::config_path() {
                let note_path = path.parent().unwrap().join("notes").join(format!("{key}.md"));
                match fs::read_to_string(&note_path) {
                    Ok(content) => {
                        let capped: String = content.chars().take(8_000).collect();
                        app.conv
                            .history
                            .push(ChatMessage::user(format!(
                                "[Context note ({key}):\n\n{capped}]"
                            )));
                        app.view.messages.push(Msg::System(format!(
                            "Loaded note: {key} ({})",
                            capped.lines().count()
                        )));
                    }
                    Err(_) => app.view.messages.push(Msg::Error(format!(
                        "Note not found: {key}"
                    ))),
                }
            } else {
                app.view.messages.push(Msg::Error("Cannot locate config directory.".into()));
            }
            app.kbd.input.clear();
            app.kbd.input_cursor = 0;
            true
        }
        // ── /note list — show all saved notes ───────────────────────────────────
        "/note list" | "/notes" => {
            if let Ok(path) = crate::config::config_path() {
                let notes_dir = path.parent().unwrap().join("notes");
                if notes_dir.exists() {
                    let keys: Vec<String> = fs::read_dir(&notes_dir)
                        .into_iter()
                        .flatten()
                        .filter_map(|e| {
                            let p = e.ok()?.path();
                            if p.extension().and_then(|e| e.to_str()) == Some("md") {
                                p.file_stem().and_then(|s| s.to_str().map(str::to_string))
                            } else {
                                None
                            }
                        })
                        .collect();
                    if keys.is_empty() {
                        app.view.messages.push(Msg::System("No saved notes.".into()));
                    } else {
                        app.view.messages.push(Msg::System(format!(
                            "Saved notes ({}) — use /note get <key> to load: {}",
                            keys.len(),
                            keys.join(", ")
                        )));
                    }
                } else {
                    app.view.messages.push(Msg::System("No saved notes yet.".into()));
                }
            } else {
                app.view.messages.push(Msg::Error("Cannot locate config directory.".into()));
            }
            app.kbd.input.clear();
            app.kbd.input_cursor = 0;
            true
        }
        // ── /note delete <key> — remove a saved note ────────────────────────────
        s if s.starts_with("/note delete ") => {
            let key = s.strip_prefix("/note delete ").unwrap().trim();
            if let Ok(path) = crate::config::config_path() {
                let note_path = path.parent().unwrap().join("notes").join(format!("{key}.md"));
                match fs::remove_file(&note_path) {
                    Ok(()) => app.view.messages.push(Msg::System(format!(
                        "Deleted note: {key}"
                    ))),
                    Err(_) => app.view.messages.push(Msg::Error(format!(
                        "Note not found: {key}"
                    ))),
                }
            } else {
                app.view.messages.push(Msg::Error("Cannot locate config directory.".into()));
            }
            app.kbd.input.clear();
            app.kbd.input_cursor = 0;
            true
        }

        // ── /agent [on|off|status] — toggle auto-approve mode ──────────────────
        s if s.starts_with("/agent") => {
            let arg = s.strip_prefix("/agent").unwrap().trim().to_lowercase();
            match arg.as_str() {
                "on" => {
                    app.policy = crate::provider::ApprovalPolicy::Allow;
                    app.view.messages.push(Msg::System(
                        "Agent mode ON: tools will auto-approve. Use /agent off to disable."
                            .into(),
                    ));
                }
                "off" => {
                    app.policy = crate::provider::ApprovalPolicy::Prompt;
                    app.view
                        .messages
                        .push(Msg::System("Agent mode OFF: will ask before write/run tools.".into()));
                }
                "" => {
                    let status = match app.policy {
                        crate::provider::ApprovalPolicy::Allow => "ON (auto-approve)",
                        crate::provider::ApprovalPolicy::Prompt => "OFF (ask before tools)",
                        crate::provider::ApprovalPolicy::Deny => "DENIED (no tools)",
                    };
                    app.view.messages.push(Msg::System(format!("Agent mode: {status}")));
                }
                _ => {
                    app.view.messages.push(Msg::System(
                        "Usage: /agent [on|off|status]".into(),
                    ));
                }
            }
            app.kbd.input.clear();
            app.kbd.input_cursor = 0;
            true
        }

        // ── /branch — show git branch info ──────────────────────────────────────
        "/branch" => {
            let branch = git(&["branch", "--show-current"]).unwrap_or_default();
            let remote = git(&["config", "--get", "branch", &branch, "remote"])
                .unwrap_or_else(|| "origin".to_string());
            let upstream = git(&["rev-parse", "--abbrev-ref", format!("{remote}/{}", branch).as_str()])
                .unwrap_or_else(|| "no upstream".to_string());
            let ahead = git(&["rev-list", "--count", "--right-only", format!("{branch}...{}", upstream).as_str()])
                .unwrap_or_default();
            let behind = git(&["rev-list", "--count", "--left-only", format!("{branch}...{}", upstream).as_str()])
                .unwrap_or_default();
            let status = format!(
                "Branch: {} (remote: {remote}/{upstream})  ahead: {}  behind: {}",
                branch, ahead, behind
            );
            app.view.messages.push(Msg::System(status));
            app.kbd.input.clear();
            app.kbd.input_cursor = 0;
            true
        }

        // ── /cost — show session cost breakdown ─────────────────────────────────
        "/cost" => {
            let u = &app.usage;
            let est = app.session_cost_usd;
            let prompt_cost = u.prompt_tokens as f64 / 1_000_000.0 * 5.0; // rough: $5/M prompt
            let comp_cost = u.completion_tokens as f64 / 1_000_000.0 * 15.0; // rough: $15/M completion
            let total_est = prompt_cost + comp_cost;
            let cost_line = format!(
                "Tokens: {}↓ {}↑ {} total\n\
                 Estimated cost: ~${:.4} (${:.2}/M prompt, ${:.2}/M completion)\n\
                 Session cost: ${:.4}",
                u.prompt_tokens, u.completion_tokens, u.total_tokens,
                total_est, 5.0, 15.0, est.max(total_est),
            );
            app.view.messages.push(Msg::System(cost_line));
            app.kbd.input.clear();
            app.kbd.input_cursor = 0;
            true
        }

        // ── /plan <prompt> — AI generates an execution plan ──────────────────────
        s if s.starts_with("/plan ") => {
            let prompt = s.strip_prefix("/plan ").unwrap().trim().to_string();
            if prompt.is_empty() {
                app.view.messages.push(Msg::System(
                    "Usage: /plan <what to build>".into(),
                ));
            } else {
                app.view.messages.push(Msg::System(format!(
                    "Generating plan for: {prompt}...",
                )));
                let tx = app.stream_tx.clone();
                let config = app.config.clone();
                let provider_name = app.provider.clone();
                let ctx = app.workspace_context.clone();
                tokio::spawn(async move {
                    let (dummy_tx, _) =
                        tokio::sync::mpsc::unbounded_channel::<crate::provider::StreamEvent>();
                    let request = PromptRequest {
                        prompt: format!(
                            "Create a numbered execution plan for: {prompt}\n\nRules:\n1. Number each step 1, 2, 3\n2. Each step is one actionable task\n3. Include file paths when relevant\n4. Prefix each step with [read] [write] or [run]\n5. Output ONLY the numbered list, no preamble\nProject context:\n{}" ,
                            ctx.as_deref().unwrap_or("")
                        ),
                        model: None,
                        system: Some("You are a precise task planner. Output only numbered steps.".into()),
                        workspace_context: None,
                        history: vec![],
                        images: vec![],
                        mcp: None,
                    };
                    if let Ok(result) = crate::provider::ask(
                        &config, &provider_name, request,
                        ApprovalPolicy::Deny, &dummy_tx,
                    ).await {
                        let steps: Vec<String> = result.text.lines()
                            .filter_map(|l| {
                                let t = l.trim();
                                if t.starts_with(|c: char| c.is_ascii_digit()) && t.contains('.') {
                                    Some(t.trim_start_matches(|c: char| c.is_ascii_digit() || c == '.').trim().to_string())
                                } else { None }
                            })
                            .collect();
                        if steps.is_empty() {
                            let _ = tx.send(TuiEvent::SystemMsg(format!("Plan:\n{}", result.text.trim())));
                        } else {
                            let _ = tx.send(TuiEvent::PlanSet(steps.clone()));
                            let numbered = steps.iter()
                                .map(|s| format!("  [ ] {}", s))
                                .collect::<Vec<_>>().join("\n");
                            let _ = tx.send(TuiEvent::SystemMsg(format!(
                                "Plan created ({} steps):\n\n{}", steps.len(), numbered
                            )));
                        }
                    }
                });
            }
            app.kbd.input.clear();
            app.kbd.input_cursor = 0;
            true
        }

        // ── /brain — AI-powered project knowledge dump ───────────────────────────
        "/brain" => {
            app.view.messages.push(Msg::System("Analyzing project context…".into()));
            let tx = app.stream_tx.clone();
            let config = app.config.clone();
            let provider_name = app.provider.clone();
            let ctx = app.workspace_context.clone();
            tokio::spawn(async move {
                let (dummy_tx, _) =
                    tokio::sync::mpsc::unbounded_channel::<crate::provider::StreamEvent>();
                let request = PromptRequest {
                    prompt: format!(
                        "Analyze this project: purpose, tech stack, architecture, key files, build/test/run instructions, conventions, limitations. Be concise. Use markdown.\n\nProject context:\n{}",
                        ctx.as_deref().unwrap_or("No context.")
                    ),
                    model: None,
                    system: Some("Project analyst: create a concise knowledge base.".into()),
                    workspace_context: None,
                    history: vec![],
                    images: vec![],
                    mcp: None,
                };
                if let Ok(result) = crate::provider::ask(
                    &config, &provider_name, request,
                    ApprovalPolicy::Deny, &dummy_tx,
                ).await {
                    let _ = tx.send(TuiEvent::SystemMsg(format!(
                        "Project Brain:\n\n{}", result.text.trim()
                    )));
                }
            });
            app.kbd.input.clear();
            app.kbd.input_cursor = 0;
            true
        }

        // ── /arch — ASCII codebase architecture map ──────────────────────────────
        "/arch" => {
            app.view.messages.push(Msg::System("Generating architecture map…".into()));
            let tx = app.stream_tx.clone();
            let config = app.config.clone();
            let provider_name = app.provider.clone();
            let ctx = app.workspace_context.clone();
            tokio::spawn(async move {
                let (dummy_tx, _) =
                    tokio::sync::mpsc::unbounded_channel::<crate::provider::StreamEvent>();
                let request = PromptRequest {
                    prompt: format!(
                        "ASCII architecture diagram: modules, dependencies, data flow, entry points. Use box-drawing chars and arrows. Under 40 lines.\n\nContext:\n{}",
                        ctx.as_deref().unwrap_or("No context.")
                    ),
                    model: None,
                    system: Some("Create ASCII architecture diagrams with box-drawing chars.".into()),
                    workspace_context: None,
                    history: vec![],
                    images: vec![],
                    mcp: None,
                };
                if let Ok(result) = crate::provider::ask(
                    &config, &provider_name, request,
                    ApprovalPolicy::Deny, &dummy_tx,
                ).await {
                    let _ = tx.send(TuiEvent::SystemMsg(format!(
                        "Architecture Map:\n\n```\n{}\n```", result.text.trim()
                    )));
                }
            });
            app.kbd.input.clear();
            app.kbd.input_cursor = 0;
            true
        }

        // ── /export text — export conversation as plain text ─────────────────────
        s if s.starts_with("/export text") => {
            let arg = s.strip_prefix("/export text").unwrap().trim();
            let path = if arg.is_empty() {
                std::path::PathBuf::from(format!("anveesa-export-{}.txt", unix_now()))
            } else {
                std::path::PathBuf::from(arg)
            };
            let text = app.conv.history.iter().map(|m| {
                let role = match m.role {
                    crate::provider::ChatRole::User => "[You]",
                    crate::provider::ChatRole::Assistant => "[AI]",
                };
                format!("{} {}", role, m.content)
            }).collect::<Vec<_>>().join("\n\n");
            match std::fs::write(&path, &text) {
                Ok(()) => app.view.messages.push(Msg::System(format!(
                    "Exported plain text → {} ({} chars)", path.display(), text.len()
                ))),
                Err(e) => app.view.messages.push(Msg::Error(format!("export failed: {e:#}"))),
            }
            app.kbd.input.clear();
            app.kbd.input_cursor = 0;
            true
        }

        // ── /help - already handled above, fall through ─────────────────────────

        // ── /help - already handled above, fall through ─────────────────────────

        // ── Custom slash commands from .anveesa/commands/*.md ───────────────────
        s if s.starts_with('/') => {
            // Extract command name
            let cmd_name = s.split_whitespace().next().unwrap_or(s).trim_end_matches('/');
            let cmd_arg = s.strip_prefix(cmd_name).map(|s| s.trim()).unwrap_or("");

            // Look for custom command definition
            if let Some(cmd) = load_custom_command(cmd_name) {
                let expanded = if cmd_arg.is_empty() {
                    cmd.action.clone()
                } else {
                    cmd.action.replace("ARG", cmd_arg)
                };
                if cmd.description.is_some() {
                    app.view.messages.push(Msg::System(format!(
                        "📌 Running custom command: {}",
                        cmd.description.unwrap_or_default()
                    )));
                }
                if expanded.starts_with('/') {
                    handle_slash_command(app, &expanded)
                } else {
                    app.kbd.input = expanded;
                    app.kbd.input_cursor = app.kbd.input.len();
                    true
                }
            } else {
                false // No custom command found, fall through to default
            }
        }

        _ => false,
    }
}

/// Custom command loaded from `.anveesa/commands/<name>.md`
#[derive(Debug)]
struct CustomCommand {
    action: String,
    description: Option<String>,
}

/// Load a custom slash command from `.anveesa/commands/<name>.md`
/// Files should have format:
/// ```text
/// # command description
/// action: /commit "fix: ARG"
/// ```
fn load_custom_command(name: &str) -> Option<CustomCommand> {
    let mut path = std::path::PathBuf::from(".anveesa/commands");
    path.push(format!("{name}.md"));

    let content: String = (if fs::read_to_string(&path).is_ok() {
        fs::read_to_string(&path).ok()
    } else {
        // Also check at git root
        std::process::Command::new("git")
            .args(["rev-parse", "--show-toplevel"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|git_root| {
                let root = String::from_utf8_lossy(&git_root.stdout).trim().to_string();
                path = std::path::PathBuf::from(&root)
                    .join(".anveesa/commands")
                    .join(format!("{name}.md"));
                if path.exists() {
                    fs::read_to_string(&path).ok()
                } else {
                    None
                }
            })
    })?;

    let mut action = String::new();
    let mut description = None;

    for line in content.lines() {
        let line = line.trim();
        // First # heading becomes description
        if line.starts_with('#') && description.is_none() {
            description = Some(line.trim_start_matches('#').trim().to_string());
        }
        // action: <value> sets the command
        if let Some(rest) = line.strip_prefix("action:") {
            action = rest.trim().to_string();
        }
    }

    if action.is_empty() {
        None
    } else {
        Some(CustomCommand { action, description })
    }
}
