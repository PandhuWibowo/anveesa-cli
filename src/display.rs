use std::{
    io::{self, IsTerminal, Write},
    path::Path,
    time::{Duration, Instant},
};

use tokio::sync::mpsc;

use crate::{
    provider::{ApprovalDecision, DiffKind, StreamEvent, ToolConfirmPreview, Usage},
    session::format_session_age,
};

use crate::RenderMode;

pub async fn render_stream(
    mut rx: mpsc::UnboundedReceiver<StreamEvent>,
    mode: RenderMode,
    started: Instant,
) {
    let spinner = io::stderr().is_terminal();
    let mut frame = 0usize;
    // True only when the 2-line spinner is currently painted on screen.
    // Used by clear_spinner to avoid wiping lines that belong to the response text.
    let mut spinner_active = false;
    let mut first_token = true;
    let mut produced = false;
    let mut line_open = false;
    let mut usage: Option<Usage> = None;
    let mut plan_tasks: Vec<String> = vec![];
    let mut plan_done: Vec<bool> = vec![];
    let mut status_message = "Waiting for response".to_string();

    static TIPS: &[&str] = &[
        "/clear  reset context",
        "/attach  clipboard image",
        "/exit  leave session",
        "--yes  auto-approve edits",
    ];

    loop {
        tokio::select! {
            maybe = rx.recv() => match maybe {
                Some(StreamEvent::Status { message }) => {
                    clear_spinner(spinner, spinner_active);
                    spinner_active = false;
                    if line_open {
                        println!();
                        line_open = false;
                    }
                    status_message = message;
                    print_status(&status_message, spinner);
                    first_token = true;
                    frame = 0;
                }
                Some(StreamEvent::Token(text)) => {
                    if first_token {
                        clear_spinner(spinner, spinner_active);
                        spinner_active = false;
                        if matches!(mode, RenderMode::Interactive) {
                            print_assistant_header(started);
                        }
                        first_token = false;
                    }
                    produced = true;
                    line_open = true;
                    print!("{text}");
                    let _ = io::stdout().flush();
                }
                Some(StreamEvent::Usage(value)) => usage = Some(value),
                Some(StreamEvent::ToolCall { summary }) => {
                    clear_spinner(spinner, spinner_active);
                    spinner_active = false;
                    if line_open {
                        println!();
                        line_open = false;
                    }
                    status_message = format!("Running {summary}");
                    print_tool_call(&summary, spinner);
                    first_token = true;
                    frame = 0;
                }
                Some(StreamEvent::ToolResult { summary, ok, elapsed_ms, error }) => {
                    clear_spinner(spinner, spinner_active);
                    spinner_active = false;
                    if line_open {
                        println!();
                        line_open = false;
                    }
                    print_tool_result(&summary, ok, elapsed_ms, error.as_deref(), spinner);
                    status_message = if ok {
                        "Continuing".to_string()
                    } else {
                        "Handling tool error".to_string()
                    };
                    first_token = true;
                    frame = 0;
                }
                Some(StreamEvent::Confirm { preview, reply }) => {
                    clear_spinner(spinner, spinner_active);
                    spinner_active = false;
                    if line_open {
                        println!();
                        line_open = false;
                    }
                    let decision = tokio::task::block_in_place(|| {
                        show_confirm_preview(&preview, spinner);
                        prompt_confirm_decision(spinner)
                    });
                    match decision {
                        ApprovalDecision::AllowOnce => {
                            print_status("Applying action", spinner);
                            status_message = "Applying action".to_string();
                        }
                        ApprovalDecision::AllowForTurn => {
                            print_status("Applying action (all approved for this turn)", spinner);
                            status_message = "Applying action".to_string();
                        }
                        ApprovalDecision::Deny => {
                            print_status("Action declined", spinner);
                            status_message = "Continuing".to_string();
                        }
                    }
                    let _ = reply.send(decision);
                    // Re-arm the spinner for the next API round.
                    first_token = true;
                    frame = 0;
                }
                Some(StreamEvent::FileOp { verb, path, added, removed, preview, truncated, after_approval, .. }) => {
                    clear_spinner(spinner, spinner_active);
                    spinner_active = false;
                    if line_open {
                        println!();
                        line_open = false;
                    }
                    // The approval preview already printed this diff — just
                    // confirm the apply with a header line.
                    let preview = if after_approval { vec![] } else { preview };
                    print_file_op(&verb, &path, added, removed, &preview, truncated, spinner);
                    // Re-arm the spinner for the next API round.
                    first_token = true;
                    frame = 0;
                }
                Some(StreamEvent::PlanSet { tasks }) => {
                    clear_spinner(spinner, spinner_active);
                    spinner_active = false;
                    if line_open {
                        println!();
                        line_open = false;
                    }
                    plan_done = vec![false; tasks.len()];
                    plan_tasks = tasks;
                    print_plan_list(&plan_tasks, &plan_done, spinner);
                    first_token = true;
                    frame = 0;
                }
                Some(StreamEvent::PlanTaskDone { index }) => {
                    clear_spinner(spinner, spinner_active);
                    spinner_active = false;
                    if line_open {
                        println!();
                        line_open = false;
                    }
                    if index < plan_done.len() {
                        plan_done[index] = true;
                    }
                    print_plan_list(&plan_tasks, &plan_done, spinner);
                    first_token = true;
                    frame = 0;
                }
                Some(StreamEvent::Thinking(_)) => {} // thinking blocks shown in TUI only
                None => break,
            },
            // 100 ms tick
            _ = tokio::time::sleep(Duration::from_millis(100)), if first_token && spinner => {
                let elapsed = started.elapsed().as_secs_f32();
                let time_str = format_elapsed(elapsed);
                // Dots cycle: "" → "." → ".." → "…" (every 3 frames ≈ 300 ms)
                let dots = ["", ".", "..", "…"][frame % 4];
                // Tip rotates every 40 frames (~4 s)
                let tip = TIPS[(frame / 40) % TIPS.len()];
                let status = truncate_for_status(&status_message, 76);

                if !spinner_active {
                    // First paint — just print 2 lines (no overwrite needed).
                    eprint!(
                        "\x1b[1;32m+\x1b[0m {status}{dots} \x1b[2m({time_str})\x1b[0m\n  \x1b[90m└\x1b[0m \x1b[2m{tip}\x1b[0m"
                    );
                    spinner_active = true;
                } else {
                    // Overwrite: move up 1 line, clear both lines, reprint.
                    eprint!(
                        "\r\x1b[2K\x1b[1A\x1b[2K\r\x1b[1;32m+\x1b[0m {status}{dots} \x1b[2m({time_str})\x1b[0m\n  \x1b[90m└\x1b[0m \x1b[2m{tip}\x1b[0m"
                    );
                }
                let _ = io::stderr().flush();
                frame += 1;
            }
        }
    }

    if produced && line_open {
        println!();
    } else {
        clear_spinner(spinner, spinner_active);
    }

    if spinner
        && let Some(usage) = usage
        && usage.total_tokens > 0
    {
        if usage.cache_read_tokens > 0 || usage.cache_write_tokens > 0 {
            eprintln!(
                "\x1b[2m  {} in · {} out · {} total  (cache: {} hit · {} write)\x1b[0m",
                usage.prompt_tokens,
                usage.completion_tokens,
                usage.total_tokens,
                usage.cache_read_tokens,
                usage.cache_write_tokens,
            );
        } else {
            eprintln!(
                "\x1b[2m  {} in · {} out · {} total\x1b[0m",
                usage.prompt_tokens, usage.completion_tokens, usage.total_tokens
            );
        }
    }
}

pub fn print_tool_call(summary: &str, is_tty: bool) {
    if is_tty {
        eprintln!("\x1b[90m  └─ {summary}\x1b[0m");
    } else {
        eprintln!("tool: {summary}");
    }
}

pub fn print_status(message: &str, is_tty: bool) {
    if is_tty {
        eprintln!("\x1b[90m  · {message}\x1b[0m");
    } else {
        eprintln!("status: {message}");
    }
}

pub fn print_tool_result(
    summary: &str,
    ok: bool,
    elapsed_ms: u128,
    error: Option<&str>,
    is_tty: bool,
) {
    let elapsed = format_duration_ms(elapsed_ms);
    if is_tty {
        if ok {
            eprintln!("\x1b[1;32m  ✓\x1b[0m \x1b[90m{summary} completed in {elapsed}\x1b[0m");
        } else if let Some(error) = error {
            eprintln!("\x1b[1;31m  ✗\x1b[0m \x1b[90m{summary} failed in {elapsed}: {error}\x1b[0m");
        } else {
            eprintln!("\x1b[1;31m  ✗\x1b[0m \x1b[90m{summary} failed in {elapsed}\x1b[0m");
        }
    } else if ok {
        eprintln!("tool ok: {summary} ({elapsed})");
    } else if let Some(error) = error {
        eprintln!("tool failed: {summary} ({elapsed}): {error}");
    } else {
        eprintln!("tool failed: {summary} ({elapsed})");
    }
}

pub fn print_file_op(
    verb: &str,
    path: &str,
    added: usize,
    removed: usize,
    preview: &[crate::provider::DiffLine],
    truncated: bool,
    is_tty: bool,
) {
    if !is_tty {
        println!("{verb}({path}): +{added} -{removed}");
        return;
    }

    // Shorten path relative to cwd when possible
    let display_path = std::env::current_dir()
        .ok()
        .and_then(|cwd| {
            let abs = std::path::Path::new(path);
            abs.strip_prefix(&cwd).ok().map(|r| r.display().to_string())
        })
        .unwrap_or_else(|| path.to_string());

    // Header: ● Update(src/lib.rs)
    println!("\n\x1b[1;32m●\x1b[0m \x1b[1;32m{verb}\x1b[0m\x1b[2m({display_path})\x1b[0m");

    // Summary: └ Added N lines, removed M lines
    let summary = match (added, removed) {
        (0, 0) => String::new(),
        (a, 0) => format!("Added {} {}", a, if a == 1 { "line" } else { "lines" }),
        (0, r) => format!("Removed {} {}", r, if r == 1 { "line" } else { "lines" }),
        (a, r) => format!(
            "Added {} {}, removed {} {}",
            a,
            if a == 1 { "line" } else { "lines" },
            r,
            if r == 1 { "line" } else { "lines" }
        ),
    };
    if !summary.is_empty() {
        println!("\x1b[90m  └\x1b[0m \x1b[2m{summary}\x1b[0m");
    }

    // Diff lines with colored backgrounds
    for dl in preview {
        let (bg, fg, prefix) = match dl.kind {
            DiffKind::Add => ("\x1b[48;5;22m", "\x1b[92m", "+"),
            DiffKind::Remove => ("\x1b[48;5;52m", "\x1b[91m", "-"),
        };
        // \x1b[K fills the remainder of the line with the current background colour
        println!(
            "{bg}\x1b[90m {:4} {fg}{prefix} {}\x1b[K\x1b[0m",
            dl.line_no, dl.text
        );
    }

    if truncated {
        println!("\x1b[90m       … (preview truncated)\x1b[0m");
    }
    println!();
}

pub fn print_plan_list(tasks: &[String], done: &[bool], is_tty: bool) {
    eprintln!();
    for (i, task) in tasks.iter().enumerate() {
        let is_done = done.get(i).copied().unwrap_or(false);
        if is_tty {
            if is_done {
                eprintln!("\x1b[1;32m[✓]\x1b[0m \x1b[2m{task}\x1b[0m");
            } else {
                eprintln!("\x1b[90m[ ]\x1b[0m {task}");
            }
        } else {
            eprintln!("[{}] {task}", if is_done { "✓" } else { " " });
        }
    }
    eprintln!();
}

pub fn print_assistant_header(started: Instant) {
    let secs = started.elapsed().as_secs_f32();
    println!();
    if io::stdout().is_terminal() {
        println!("\x1b[1;32m❯\x1b[0m \x1b[2m{secs:.1}s\x1b[0m");
    } else {
        println!("({secs:.1}s)");
    }
}

pub fn clear_spinner(enabled: bool, active: bool) {
    if !enabled || !active {
        return;
    }
    // Clear the tip line, move up, clear the status line, return to column 0.
    eprint!("\r\x1b[2K\x1b[1A\x1b[2K\r");
    let _ = io::stderr().flush();
}

pub fn format_elapsed(secs: f32) -> String {
    let s = secs as u64;
    if s >= 60 {
        format!("{}m {}s", s / 60, s % 60)
    } else {
        format!("{s}s")
    }
}

pub fn format_duration_ms(ms: u128) -> String {
    if ms >= 1000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        format!("{ms}ms")
    }
}

pub fn truncate_for_status(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let mut output = String::new();
    for _ in 0..max_chars {
        let Some(ch) = chars.next() else {
            return output;
        };
        output.push(ch);
    }
    if chars.next().is_some() {
        output.push('…');
    }
    output
}

pub fn show_confirm_preview(preview: &ToolConfirmPreview, is_tty: bool) {
    match preview {
        ToolConfirmPreview::FileOp {
            verb,
            path,
            added,
            removed,
            diff,
            truncated,
        } => {
            eprint_file_op(verb, path, *added, *removed, diff, *truncated, is_tty);
        }
        ToolConfirmPreview::CreateDir { path } => {
            if is_tty {
                eprintln!(
                    "\n\x1b[1;32m●\x1b[0m \x1b[1;32mCreate dir\x1b[0m\x1b[2m({path})\x1b[0m\n"
                );
            } else {
                eprintln!("Create dir: {path}");
            }
        }
        ToolConfirmPreview::Generic { summary } => {
            if is_tty {
                eprintln!("\n\x1b[1;32m●\x1b[0m \x1b[1;32m{summary}\x1b[0m\n");
            } else {
                eprintln!("{summary}");
            }
        }
    }
}

/// Like `print_file_op` but writes to stderr — used for pre-approval previews so
/// the diff, the spinner clear, and the approval prompt all share the same stream.
pub fn eprint_file_op(
    verb: &str,
    path: &str,
    added: usize,
    removed: usize,
    diff: &[crate::provider::DiffLine],
    truncated: bool,
    is_tty: bool,
) {
    if !is_tty {
        eprintln!("{verb}({path}): +{added} -{removed}");
        return;
    }

    let display_path = std::env::current_dir()
        .ok()
        .and_then(|cwd| {
            std::path::Path::new(path)
                .strip_prefix(&cwd)
                .ok()
                .map(|r| r.display().to_string())
        })
        .unwrap_or_else(|| path.to_string());

    eprintln!("\n\x1b[1;32m●\x1b[0m \x1b[1;32m{verb}\x1b[0m\x1b[2m({display_path})\x1b[0m");

    let summary = match (added, removed) {
        (0, 0) => String::new(),
        (a, 0) => format!("Added {} {}", a, if a == 1 { "line" } else { "lines" }),
        (0, r) => format!("Removed {} {}", r, if r == 1 { "line" } else { "lines" }),
        (a, r) => format!(
            "Added {} {}, removed {} {}",
            a,
            if a == 1 { "line" } else { "lines" },
            r,
            if r == 1 { "line" } else { "lines" }
        ),
    };
    if !summary.is_empty() {
        eprintln!("\x1b[90m  └\x1b[0m \x1b[2m{summary}\x1b[0m");
    }

    for dl in diff {
        let (bg, fg, prefix) = match dl.kind {
            DiffKind::Add => ("\x1b[48;5;22m", "\x1b[92m", "+"),
            DiffKind::Remove => ("\x1b[48;5;52m", "\x1b[91m", "-"),
        };
        eprintln!(
            "{bg}\x1b[90m {:4} {fg}{prefix} {}\x1b[K\x1b[0m",
            dl.line_no, dl.text
        );
    }

    if truncated {
        eprintln!("\x1b[90m       … (preview truncated)\x1b[0m");
    }
    eprintln!();
}

pub fn prompt_confirm_decision(is_tty: bool) -> ApprovalDecision {
    let mut err = io::stderr();
    if is_tty {
        let _ = write!(
            err,
            "\x1b[1;32m❯\x1b[0m Apply? \x1b[2m[y]es / [a]ll this turn / [N]o\x1b[0m  "
        );
    } else {
        let _ = write!(err, "Apply? [y]es/[a]ll this turn/[N]o  ");
    }
    let _ = err.flush();

    let mut answer = String::new();
    if io::stdin().read_line(&mut answer).is_err() {
        return ApprovalDecision::Deny;
    }
    match answer.trim().to_lowercase().as_str() {
        "y" | "yes" => ApprovalDecision::AllowOnce,
        "a" | "all" => ApprovalDecision::AllowForTurn,
        _ => ApprovalDecision::Deny,
    }
}

pub fn print_status_inline(
    is_tty: bool,
    provider: &str,
    model: Option<&str>,
    cwd: &std::path::Path,
    turns: usize,
    usage: &Usage,
) {
    let model_display = model.unwrap_or("(default)");
    let short_cwd = std::env::var("HOME")
        .map(|h| cwd.display().to_string().replacen(&h, "~", 1))
        .unwrap_or_else(|_| cwd.display().to_string());

    if !is_tty {
        println!("provider: {provider}  model: {model_display}");
        println!("cwd: {short_cwd}");
        println!("turns: {turns}");
        if usage.total_tokens > 0 {
            println!(
                "tokens: {} in / {} out / {} total",
                usage.prompt_tokens, usage.completion_tokens, usage.total_tokens
            );
        }
        return;
    }

    println!();
    println!("\x1b[90m  ──────────────────────────────────────\x1b[0m");
    println!(
        "  \x1b[2mprovider\x1b[0m  \x1b[1m{provider}\x1b[0m  \x1b[2m·\x1b[0m  \x1b[1m{model_display}\x1b[0m"
    );
    println!("  \x1b[2mcwd     \x1b[0m  \x1b[2m{short_cwd}\x1b[0m");
    println!("  \x1b[2mturns   \x1b[0m  {turns}");
    if usage.total_tokens > 0 {
        println!(
            "  \x1b[2mtokens  \x1b[0m  {} in  ·  {} out  ·  {} total",
            usage.prompt_tokens, usage.completion_tokens, usage.total_tokens
        );
        if usage.cache_read_tokens > 0 || usage.cache_write_tokens > 0 {
            println!(
                "  \x1b[2mcache   \x1b[0m  {} read  ·  {} write",
                usage.cache_read_tokens, usage.cache_write_tokens
            );
        }
    }
    println!("\x1b[90m  ──────────────────────────────────────\x1b[0m");
    println!();
}

pub fn print_help_inline(is_tty: bool) {
    if !is_tty {
        println!("commands: /clear, /export [path], /session, /attach [path], /exit, /quit, /help");
        println!("keys: ↑/↓ history  ←/→ cursor  Home/End  Ctrl+W delete-word  Ctrl+U clear-line");
        println!("images: Ctrl+V to paste clipboard image, or copy then send to auto-attach");
        return;
    }
    println!();
    println!("\x1b[2m  Commands\x1b[0m");
    println!("\x1b[90m  ──────────────────────────────────────\x1b[0m");
    println!("  \x1b[1;32m/status\x1b[0m             provider, model, turns, token usage");
    println!("  \x1b[1;32m/session\x1b[0m            show session file, age, and turn count");
    println!(
        "  \x1b[1;32m/export\x1b[0m \x1b[2m[path]\x1b[0m     save conversation to a markdown file"
    );
    println!("  \x1b[1;32m/model\x1b[0m \x1b[2m[name]\x1b[0m      switch or show current model");
    println!("  \x1b[1;32m/provider\x1b[0m \x1b[2m[name]\x1b[0m   switch or show current provider");
    println!("  \x1b[1;32m/clear\x1b[0m              reset conversation and delete saved session");
    println!(
        "  \x1b[1;32m/attach\x1b[0m \x1b[2m[path]\x1b[0m     attach image from file or clipboard"
    );
    println!("  \x1b[1;32m/exit\x1b[0m, \x1b[1;32m/quit\x1b[0m       leave the session");
    println!("  \x1b[1;32m/help\x1b[0m               show this message");
    println!();
    println!("\x1b[2m  Keyboard\x1b[0m");
    println!("\x1b[90m  ──────────────────────────────────────\x1b[0m");
    println!("  \x1b[2m↑ / ↓\x1b[0m          recall previous / next prompt");
    println!("  \x1b[2m← / →\x1b[0m          move cursor left / right");
    println!("  \x1b[2mHome / End\x1b[0m      jump to start / end of line");
    println!("  \x1b[2mCtrl+W\x1b[0m          delete word before cursor");
    println!("  \x1b[2mCtrl+U\x1b[0m          clear entire line  \x1b[2m(also Cmd+Delete)\x1b[0m");
    println!("  \x1b[2mCtrl+V\x1b[0m          paste image from clipboard");
    println!();
    println!("\x1b[2m  Images\x1b[0m");
    println!("\x1b[90m  ──────────────────────────────────────\x1b[0m");
    println!(
        "  \x1b[2mCtrl+V\x1b[0m to paste a clipboard image inline (shows \x1b[2m[📎]\x1b[0m indicator)."
    );
    println!("  Or Cmd+C an image and send any message — it attaches automatically.");
    println!(
        "  Or use \x1b[1;32m/attach\x1b[0m \x1b[2mpath/to/file.png\x1b[0m for a specific file."
    );
    println!("  For broadest clipboard support: \x1b[2mbrew install pngpaste\x1b[0m");
    println!();
}

pub fn print_session_info(is_tty: bool, path: Option<&Path>, turns: usize, saved_at: Option<u64>) {
    let Some(path) = path else {
        if is_tty {
            eprintln!("\x1b[2m  no session path available\x1b[0m");
        } else {
            println!("no session path available");
        }
        return;
    };

    let short_path = std::env::var("HOME")
        .map(|h| path.display().to_string().replacen(&h, "~", 1))
        .unwrap_or_else(|_| path.display().to_string());

    if !is_tty {
        println!("session: {short_path}");
        println!("turns: {turns}");
        if let Some(ts) = saved_at {
            println!("saved: {}", format_session_age(Some(ts)));
        }
        return;
    }

    println!();
    println!("\x1b[90m  ──────────────────────────────────────\x1b[0m");
    println!("  \x1b[2mfile  \x1b[0m  \x1b[2m{short_path}\x1b[0m");
    println!("  \x1b[2mturns \x1b[0m  {turns}");
    if let Some(ts) = saved_at {
        println!("  \x1b[2msaved \x1b[0m  {}", format_session_age(Some(ts)));
    } else {
        println!("  \x1b[2msaved \x1b[0m  not yet");
    }
    println!("\x1b[90m  ──────────────────────────────────────\x1b[0m");
    println!();
}

pub fn print_session_header(
    provider: &str,
    model: &str,
    turns: usize,
    resumed: bool,
    saved_at: Option<u64>,
) {
    let is_tty = io::stdout().is_terminal();
    let version = env!("CARGO_PKG_VERSION");

    if !is_tty {
        let tag = if resumed {
            format!(
                " (resumed · {turns} turns · {})",
                format_session_age(saved_at)
            )
        } else {
            String::new()
        };
        println!("anveesa v{version}{tag} | {provider} · {model}");
        return;
    }

    let width = term_width().clamp(50, 220);

    let cwd = std::env::current_dir()
        .ok()
        .map(|p| {
            let s = p.to_string_lossy().into_owned();
            std::env::var("HOME")
                .map(|h| s.replacen(&h, "~", 1))
                .unwrap_or(s)
        })
        .unwrap_or_else(|| "~".to_string());

    fn trunc_to(s: &str, max: usize) -> String {
        let v: Vec<char> = s.chars().collect();
        if v.len() <= max {
            return s.to_string();
        }
        let mut r: String = v[..max.saturating_sub(1)].iter().collect();
        r.push('…');
        r
    }

    let greeting = if resumed {
        format!(
            " · Resumed ({turns} turns · {})",
            format_session_age(saved_at)
        )
    } else {
        String::new()
    };
    let title = format!(" anveesa v{version}{greeting} ");
    let title_len = title.chars().count();
    let right_dashes = width.saturating_sub(2 + title_len);
    println!(
        "\x1b[90m──\x1b[0m\x1b[1;32m{title}\x1b[0m\x1b[90m{}\x1b[0m",
        "─".repeat(right_dashes)
    );

    let info = trunc_to(&format!("  {provider} · {model} · {cwd}"), width);
    println!("\x1b[2m{info}\x1b[0m");

    println!("\x1b[2m  /help for commands\x1b[0m");
    println!();
}

pub fn print_input_separator(is_tty: bool, width: usize) {
    let line = "─".repeat(width);
    if is_tty {
        println!("\x1b[90m{line}\x1b[0m");
    } else {
        println!("{line}");
    }
}

pub fn prompt_label(is_tty: bool) -> String {
    if is_tty {
        "\x1b[1;32m❯\x1b[0m ".to_string()
    } else {
        "> ".to_string()
    }
}

pub fn term_width() -> usize {
    std::env::var("COLUMNS")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|&n: &usize| n > 0)
        .unwrap_or_else(|| {
            std::process::Command::new("tput")
                .arg("cols")
                .output()
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .and_then(|s| s.trim().parse().ok())
                .filter(|&n: &usize| n > 0)
                .unwrap_or(90)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_elapsed_sub_minute() {
        assert_eq!(format_elapsed(0.0), "0s");
        assert_eq!(format_elapsed(45.9), "45s");
        assert_eq!(format_elapsed(59.9), "59s");
    }

    #[test]
    fn format_elapsed_minutes() {
        assert_eq!(format_elapsed(60.0), "1m 0s");
        assert_eq!(format_elapsed(90.0), "1m 30s");
        assert_eq!(format_elapsed(125.0), "2m 5s");
    }

    #[test]
    fn format_duration_millis() {
        assert_eq!(format_duration_ms(0), "0ms");
        assert_eq!(format_duration_ms(500), "500ms");
        assert_eq!(format_duration_ms(999), "999ms");
    }

    #[test]
    fn format_duration_seconds() {
        assert_eq!(format_duration_ms(1000), "1.0s");
        assert_eq!(format_duration_ms(1500), "1.5s");
        assert_eq!(format_duration_ms(2000), "2.0s");
    }

    #[test]
    fn truncate_for_status_short() {
        assert_eq!(truncate_for_status("hello", 10), "hello");
        assert_eq!(truncate_for_status("", 5), "");
    }

    #[test]
    fn truncate_for_status_exact_limit() {
        assert_eq!(truncate_for_status("hello", 5), "hello");
    }

    #[test]
    fn truncate_for_status_over_limit() {
        assert_eq!(truncate_for_status("abcdef", 5), "abcde…");
        assert_eq!(truncate_for_status("hello world", 5), "hello…");
    }
}
