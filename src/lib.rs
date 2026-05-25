pub mod cli;
pub mod config;
pub mod provider;
pub mod tools;

use std::{
    fs,
    io::{self, IsTerminal, Read, Write},
    path::{Path, PathBuf},
    process::Command as ProcessCommand,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use clap::{CommandFactory, Parser};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use base64::{Engine, engine::general_purpose::STANDARD as BASE64};

use crate::{
    cli::{AskOptions, Cli, Command, ConfigCommand},
    config::{
        AppConfig, ProviderConfig, config_path, init_config, print_path, set_default_model,
        set_default_provider,
    },
    provider::{
        ApprovalDecision, ApprovalPolicy, ChatMessage, DiffKind, ImageAttachment, PromptRequest,
        StreamEvent, ToolConfirmPreview, TurnResult, Usage,
    },
};

#[derive(Debug, Clone, Copy)]
enum RenderMode {
    Interactive,
    OneShot,
}

#[derive(Debug, Serialize, Deserialize)]
struct InteractiveSession {
    cwd: String,
    provider: String,
    model: Option<String>,
    system: Option<String>,
    messages: Vec<ChatMessage>,
}

pub async fn run_anveesa() -> Result<()> {
    run_cli(Cli::parse()).await
}

async fn run_cli(cli: Cli) -> Result<()> {
    match cli.command {
        Some(Command::Ask(args)) => run_ask(args.options, args.prompt).await,
        Some(Command::Providers) => list_providers(),
        Some(Command::Config(args)) => run_config(args.command),
        None if cli.prompt.is_empty() && cli.ask_options.stdin => {
            run_ask(cli.ask_options, cli.prompt).await
        }
        None if cli.prompt.is_empty() && std::io::stdin().is_terminal() => {
            run_interactive(cli.ask_options).await
        }
        None if cli.prompt.is_empty() => {
            Cli::command().print_help()?;
            println!();
            Ok(())
        }
        None => run_ask(cli.ask_options, cli.prompt).await,
    }
}

async fn run_interactive(options: AskOptions) -> Result<()> {
    let config = AppConfig::load()?;
    let provider_name = config
        .provider_name(options.provider.as_deref())?
        .to_string();
    let provider = config
        .providers
        .get(&provider_name)
        .with_context(|| format!("unknown provider '{provider_name}'"))?;
    let tools_available = matches!(provider, ProviderConfig::OpenAiCompatible(_));
    let model = options
        .model
        .clone()
        .or_else(|| provider.default_model().map(str::to_string));
    let cwd = std::env::current_dir().context("failed to resolve current directory")?;
    let workspace_context = workspace_context_for(&cwd).ok();
    let policy = if options.yes {
        ApprovalPolicy::Allow
    } else {
        ApprovalPolicy::Prompt
    };

    let session_options = AskOptions {
        provider: Some(provider_name.clone()),
        model,
        system: options.system,
        stdin: false,
        yes: options.yes,
    };

    let session_path = repl_session_path();
    let mut history = session_path
        .as_deref()
        .and_then(|path| load_interactive_session(path, &cwd, &provider_name, &session_options))
        .unwrap_or_default();
    let history_path = repl_history_path();
    print_session_header(
        &provider_name,
        session_options.model.as_deref().unwrap_or("-"),
        history.len() / 2,
        workspace_context.is_some(),
        tools_available,
        policy,
        !history.is_empty(),
    );

    let is_tty = io::stdout().is_terminal();
    let width = term_width();
    let label = prompt_label(is_tty);
    // Fingerprint of the last clipboard image we attached — prevents re-attaching
    // the same screenshot on every subsequent turn until the user copies something new.
    let mut last_image_fp: Option<String> = None;
    let mut paste_count = 0usize;

    loop {
        print_input_separator(is_tty, width);
        let line = match read_prompt_line(&label, width, &mut paste_count) {
            Ok(PromptRead::Line(line)) => line,
            Ok(PromptRead::Interrupted) => continue,
            Ok(PromptRead::Eof) => {
                println!();
                break;
            }
            Err(error) => return Err(error).context("failed to read interactive prompt"),
        };

        let prompt = line.trim().to_string();
        if prompt.is_empty() {
            continue;
        }

        print_input_separator(is_tty, width);

        match prompt.as_str() {
            "/exit" | "/quit" | ":q" => break,
            "/clear" => {
                history.clear();
                last_image_fp = None;
                paste_count = 0;
                if let Some(path) = &session_path {
                    let _ = fs::remove_file(path);
                }
                println!("context cleared; memory reset");
                continue;
            }
            _ => {}
        }
        if let Some(path) = &history_path {
            let _ = append_repl_history(path, prompt.as_str());
        }

        // Check clipboard for a new screenshot. Skip if it's the same image as last turn.
        let image = if is_tty {
            grab_clipboard_image().and_then(|img| {
                let fp = image_fingerprint(&img);
                if last_image_fp.as_deref() == Some(&fp) {
                    None // same image — don't re-attach
                } else {
                    last_image_fp = Some(fp);
                    Some(img)
                }
            })
        } else {
            None
        };
        if is_tty && image.is_some() {
            eprintln!("\x1b[90m  [📎 screenshot from clipboard attached]\x1b[0m");
        }

        match ask_streaming(
            &config,
            &session_options,
            prompt.clone(),
            &history,
            workspace_context.as_deref(),
            policy,
            image,
            RenderMode::Interactive,
        )
        .await
        {
            Ok(result) => {
                println!();
                history.push(ChatMessage::user(prompt));
                history.push(ChatMessage::assistant(result.text));
                if let Some(path) = &session_path {
                    let _ = save_interactive_session(
                        path,
                        &cwd,
                        &provider_name,
                        &session_options,
                        &history,
                    );
                }
            }
            Err(error) => {
                eprintln!("error: {error:#}");
                println!();
                history.push(ChatMessage::user(prompt));
                history.push(ChatMessage::assistant(format!(
                    "The previous turn failed inside Anveesa before a final answer was produced: {error:#}"
                )));
                if let Some(path) = &session_path {
                    let _ = save_interactive_session(
                        path,
                        &cwd,
                        &provider_name,
                        &session_options,
                        &history,
                    );
                }
            }
        }
    }

    if let Some(path) = &session_path {
        let _ = save_interactive_session(path, &cwd, &provider_name, &session_options, &history);
    }
    Ok(())
}

async fn run_ask(options: AskOptions, prompt_parts: Vec<String>) -> Result<()> {
    let config = AppConfig::load()?;
    let provider_name = config
        .provider_name(options.provider.as_deref())?
        .to_string();
    config
        .providers
        .get(&provider_name)
        .with_context(|| format!("unknown provider '{provider_name}'"))?;
    let prompt = build_prompt(prompt_parts, options.stdin)?;
    let workspace_context = workspace_context().ok();
    let policy = one_shot_policy(options.yes, io::stdin().is_terminal());

    ask_streaming(
        &config,
        &options,
        prompt,
        &[],
        workspace_context.as_deref(),
        policy,
        None,
        RenderMode::OneShot,
    )
    .await?;
    Ok(())
}

async fn ask_streaming(
    config: &AppConfig,
    options: &AskOptions,
    prompt: String,
    history: &[ChatMessage],
    workspace_context: Option<&str>,
    policy: ApprovalPolicy,
    image: Option<ImageAttachment>,
    mode: RenderMode,
) -> Result<TurnResult> {
    let provider_name = config
        .provider_name(options.provider.as_deref())?
        .to_string();
    let (tx, rx) = mpsc::unbounded_channel();
    let started = Instant::now();
    let renderer = tokio::spawn(render_stream(rx, mode, started));

    let request = PromptRequest {
        prompt,
        model: options.model.clone(),
        system: options.system.clone(),
        workspace_context: workspace_context.map(str::to_string),
        history: history.to_vec(),
        image,
    };

    let result = provider::ask(config, &provider_name, request, policy, &tx).await;
    drop(tx);
    let _ = renderer.await;
    result
}

async fn render_stream(
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
    let mut usage: Option<Usage> = None;
    let mut plan_tasks: Vec<String> = vec![];
    let mut plan_done: Vec<bool> = vec![];

    static TIPS: &[&str] = &[
        "Tip: type /clear to reset context",
        "Tip: paste a screenshot and ask about it",
        "Tip: use --yes to auto-approve file edits",
        "Tip: type /exit to leave the session",
    ];

    loop {
        tokio::select! {
            maybe = rx.recv() => match maybe {
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
                    print!("{text}");
                    let _ = io::stdout().flush();
                }
                Some(StreamEvent::Usage(value)) => usage = Some(value),
                Some(StreamEvent::Confirm { preview, reply }) => {
                    clear_spinner(spinner, spinner_active);
                    spinner_active = false;
                    let decision = tokio::task::block_in_place(|| {
                        show_confirm_preview(&preview, spinner);
                        prompt_confirm_decision(spinner)
                    });
                    let _ = reply.send(decision);
                    // Re-arm the spinner for the next API round.
                    first_token = true;
                    frame = 0;
                }
                Some(StreamEvent::FileOp { verb, path, added, removed, preview, truncated }) => {
                    clear_spinner(spinner, spinner_active);
                    spinner_active = false;
                    print_file_op(&verb, &path, added, removed, &preview, truncated, spinner);
                    // Re-arm the spinner for the next API round.
                    first_token = true;
                    frame = 0;
                }
                Some(StreamEvent::PlanSet { tasks }) => {
                    clear_spinner(spinner, spinner_active);
                    spinner_active = false;
                    plan_done = vec![false; tasks.len()];
                    plan_tasks = tasks;
                    print_plan_list(&plan_tasks, &plan_done, spinner);
                    first_token = true;
                    frame = 0;
                }
                Some(StreamEvent::PlanTaskDone { index }) => {
                    clear_spinner(spinner, spinner_active);
                    spinner_active = false;
                    if index < plan_done.len() {
                        plan_done[index] = true;
                    }
                    print_plan_list(&plan_tasks, &plan_done, spinner);
                    first_token = true;
                    frame = 0;
                }
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

                if !spinner_active {
                    // First paint — just print 2 lines (no overwrite needed).
                    eprint!(
                        "\x1b[1;32m+\x1b[0m Thinking{dots} \x1b[2m({time_str})\x1b[0m\n  \x1b[90m└\x1b[0m \x1b[2m{tip}\x1b[0m"
                    );
                    spinner_active = true;
                } else {
                    // Overwrite: move up 1 line, clear both lines, reprint.
                    eprint!(
                        "\r\x1b[2K\x1b[1A\x1b[2K\r\x1b[1;32m+\x1b[0m Thinking{dots} \x1b[2m({time_str})\x1b[0m\n  \x1b[90m└\x1b[0m \x1b[2m{tip}\x1b[0m"
                    );
                }
                let _ = io::stderr().flush();
                frame += 1;
            }
        }
    }

    if produced {
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
                "[tokens: {} in / {} out / {} total | cache: {} read / {} write]",
                usage.prompt_tokens,
                usage.completion_tokens,
                usage.total_tokens,
                usage.cache_read_tokens,
                usage.cache_write_tokens,
            );
        } else {
            eprintln!(
                "[tokens: {} in / {} out / {} total]",
                usage.prompt_tokens, usage.completion_tokens, usage.total_tokens
            );
        }
    }
}

fn print_file_op(
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
        (a, 0) if a == 0 => String::new(),
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

fn print_plan_list(tasks: &[String], done: &[bool], is_tty: bool) {
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

fn print_assistant_header(started: Instant) {
    let secs = started.elapsed().as_secs_f32();
    println!();
    if io::stdout().is_terminal() {
        println!("\x1b[1;32m❯\x1b[0m \x1b[2m{secs:.1}s\x1b[0m");
    } else {
        println!("({secs:.1}s)");
    }
}

fn clear_spinner(enabled: bool, active: bool) {
    if !enabled || !active {
        return;
    }
    // Clear the tip line, move up, clear the status line, return to column 0.
    eprint!("\r\x1b[2K\x1b[1A\x1b[2K\r");
    let _ = io::stderr().flush();
}

fn format_elapsed(secs: f32) -> String {
    let s = secs as u64;
    if s >= 60 {
        format!("{}m {}s", s / 60, s % 60)
    } else {
        format!("{s}s")
    }
}

fn show_confirm_preview(preview: &ToolConfirmPreview, is_tty: bool) {
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
fn eprint_file_op(
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

fn prompt_confirm_decision(is_tty: bool) -> ApprovalDecision {
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

fn one_shot_policy(auto_approve: bool, stdin_is_terminal: bool) -> ApprovalPolicy {
    if auto_approve {
        ApprovalPolicy::Allow
    } else if stdin_is_terminal {
        ApprovalPolicy::Prompt
    } else {
        ApprovalPolicy::Deny
    }
}

fn list_providers() -> Result<()> {
    let config = AppConfig::load()?;
    println!("providers:");
    for (name, provider) in config.providers {
        let default_marker = if config.default_provider.as_deref() == Some(name.as_str()) {
            " default"
        } else {
            ""
        };
        let model = provider.default_model().unwrap_or("-");
        println!(
            "- {name} ({kind}, model: {model}){default_marker}",
            kind = provider.kind()
        );
    }
    Ok(())
}

fn run_config(command: ConfigCommand) -> Result<()> {
    match command {
        ConfigCommand::Init { force } => {
            let path = init_config(force)?;
            println!("created {}", print_path(&path));
            Ok(())
        }
        ConfigCommand::SetModel { provider, model } => {
            let (path, provider_name) = set_default_model(provider.as_deref(), model)?;
            println!(
                "set default model for {provider_name} in {}",
                print_path(&path)
            );
            Ok(())
        }
        ConfigCommand::SetProvider { provider } => {
            let path = set_default_provider(provider.clone())?;
            println!(
                "set default provider to {provider} in {}",
                print_path(&path)
            );
            Ok(())
        }
        ConfigCommand::Path => {
            println!("{}", print_path(&config_path()?));
            Ok(())
        }
        ConfigCommand::Show => {
            let config = AppConfig::load()?;
            println!("{}", toml::to_string_pretty(&config)?);
            Ok(())
        }
    }
}

fn build_prompt(prompt_parts: Vec<String>, force_stdin: bool) -> Result<String> {
    let mut prompt = prompt_parts.join(" ");

    if force_stdin || (prompt.is_empty() && !std::io::stdin().is_terminal()) {
        let mut stdin = String::new();
        std::io::stdin()
            .read_to_string(&mut stdin)
            .context("failed to read stdin")?;

        prompt = match (prompt.trim().is_empty(), stdin.trim().is_empty()) {
            (true, true) => String::new(),
            (true, false) => stdin,
            (false, true) => prompt,
            (false, false) => format!("{prompt}\n\n{stdin}"),
        };
    }

    if prompt.trim().is_empty() {
        bail!("prompt is empty; pass text arguments or pipe input with --stdin")
    }

    Ok(prompt)
}

fn print_session_header(
    provider: &str,
    model: &str,
    _turns: usize,
    _has_workspace_context: bool,
    _tools_available: bool,
    policy: ApprovalPolicy,
    resumed: bool,
) {
    fn pad_to(s: &str, w: usize) -> String {
        let n = s.chars().count();
        if n >= w {
            s.chars().take(w).collect()
        } else {
            format!("{}{}", s, " ".repeat(w - n))
        }
    }
    fn center_in(s: &str, w: usize) -> String {
        let n = s.chars().count();
        if n >= w {
            return s.chars().take(w).collect();
        }
        let pad = w - n;
        let lp = pad / 2;
        format!("{}{}{}", " ".repeat(lp), s, " ".repeat(pad - lp))
    }
    fn trunc(s: &str, max: usize) -> String {
        let v: Vec<char> = s.chars().collect();
        if v.len() <= max {
            return s.to_string();
        }
        let mut r: String = v[..max - 1].iter().collect();
        r.push('…');
        r
    }

    let is_tty = io::stdout().is_terminal();
    let version = env!("CARGO_PKG_VERSION");

    // Fit the box to the actual terminal width
    let total: usize = if is_tty {
        term_width().clamp(80, 220)
    } else {
        90
    };
    let left_w: usize = 38;
    let right_w: usize = total.saturating_sub(left_w + 3);

    let cwd = std::env::current_dir()
        .ok()
        .map(|p| {
            let s = p.to_string_lossy().into_owned();
            std::env::var("HOME")
                .map(|h| s.replacen(&h, "~", 1))
                .unwrap_or(s)
        })
        .unwrap_or_else(|| "~".to_string());

    let rs = if is_tty { "\x1b[0m" } else { "" };
    let br = if is_tty { "\x1b[36m" } else { "" }; // cyan  — border
    let bg = if is_tty { "\x1b[1;32m" } else { "" }; // bold green — section headers
    let cy = if is_tty { "\x1b[36m" } else { "" }; // cyan  — body text
    let gr = if is_tty { "\x1b[32m" } else { "" }; // green — robot art (distinct from border)
    let dm = if is_tty { "\x1b[2m" } else { "" }; // dim   — secondary info

    let row = |lp: &str, lc: &str, rp: &str, rc: &str| {
        let l = pad_to(lp, left_w);
        let r = pad_to(rp, right_w);
        let ld = if is_tty && !lc.is_empty() {
            format!("{lc}{l}{rs}")
        } else {
            l
        };
        let rd = if is_tty && !rc.is_empty() {
            format!("{rc}{r}{rs}")
        } else {
            r
        };
        println!("{br}│{rs}{ld}{br}│{rs}{rd}{br}│{rs}");
    };

    // Top border: ┌── Anveesa vX.Y.Z ─────...─┐  (full terminal width)
    let title = format!(" Anveesa v{version} ");
    let tlen = title.chars().count();
    let dashes_str = "─".repeat(total.saturating_sub(4 + tlen));
    println!("{br}┌──{title}{dashes_str}┐{rs}");

    let greeting = if resumed { "Welcome back!" } else { "Hello!" };
    let info = trunc(&format!("  {provider}  ·  {model}"), left_w);
    let cwd_line = trunc(&format!("  {cwd}"), left_w);

    // Robot art — pure ASCII so width is always 1 char per glyph, no box-char conflict
    // Each string is exactly 11 chars wide
    let art = [
        "  .------. ", // head top
        "  | o  o | ", // eyes
        "  |  __  | ", // mouth
        "  '------' ", // head bottom
        "    |  |   ", // legs
    ];

    let approve = if matches!(policy, ApprovalPolicy::Prompt) {
        "  y/a      approve tools"
    } else {
        ""
    };

    row("", "", "", "");
    row(
        &center_in(greeting, left_w),
        bg,
        "  Tips for getting started",
        bg,
    );
    row("", "", "  /clear   reset context", cy);
    row(
        &center_in(art[0], left_w),
        gr,
        "  /exit or /quit to leave",
        cy,
    );
    row(
        &center_in(art[1], left_w),
        gr,
        "  anveesa ask <q>  one-shot",
        cy,
    );
    row(&center_in(art[2], left_w), gr, "", "");

    // Right-panel section separator
    {
        let l = pad_to(&center_in(art[3], left_w), left_w);
        let sep = "─".repeat(right_w);
        let l_colored = if is_tty { format!("{gr}{l}{rs}") } else { l };
        let rd = if is_tty {
            format!("{dm}{sep}{rs}")
        } else {
            sep
        };
        println!("{br}│{rs}{l_colored}{br}│{rs}{rd}{br}│{rs}");
    }

    row(&center_in(art[4], left_w), gr, "", "");
    row("", "", "  Commands", bg);
    row(&info, dm, "  /clear   reset memory", cy);
    row(&cwd_line, dm, "  /exit    quit session", cy);
    row("", "", approve, cy);
    row("", "", "", "");

    // Bottom border (full terminal width)
    let bot = "─".repeat(total.saturating_sub(2));
    println!("{br}└{bot}┘{rs}");
    println!();
}

enum PromptRead {
    Line(String),
    Interrupted,
    Eof,
}

struct PromptSegment {
    full: String,
    display: String,
    hidden: bool,
}

#[derive(Default)]
struct PromptBuffer {
    full: String,
    display: String,
    segments: Vec<PromptSegment>,
}

impl PromptBuffer {
    fn is_empty(&self) -> bool {
        self.full.is_empty()
    }

    fn push_text(&mut self, text: &str) {
        self.full.push_str(text);
        self.display.push_str(text);

        if let Some(segment) = self.segments.last_mut()
            && !segment.hidden
        {
            segment.full.push_str(text);
            segment.display.push_str(text);
            return;
        }

        self.segments.push(PromptSegment {
            full: text.to_string(),
            display: text.to_string(),
            hidden: false,
        });
    }

    fn push_hidden_paste(&mut self, text: String, display: String) {
        self.full.push_str(&text);
        self.display.push_str(&display);
        self.segments.push(PromptSegment {
            full: text,
            display,
            hidden: true,
        });
    }

    fn pop_last(&mut self) {
        let Some(segment) = self.segments.last_mut() else {
            return;
        };

        if segment.hidden {
            let full_len = segment.full.len();
            let display_len = segment.display.len();
            self.full.truncate(self.full.len().saturating_sub(full_len));
            self.display
                .truncate(self.display.len().saturating_sub(display_len));
            self.segments.pop();
            return;
        }

        let _ = segment.full.pop();
        let _ = segment.display.pop();
        let _ = self.full.pop();
        let _ = self.display.pop();

        if segment.full.is_empty() {
            self.segments.pop();
        }
    }
}

struct RawPromptMode {
    fd: i32,
    saved: libc::termios,
}

impl RawPromptMode {
    fn enter() -> Result<Self> {
        let fd = libc::STDIN_FILENO;
        let mut saved = std::mem::MaybeUninit::<libc::termios>::uninit();
        if unsafe { libc::tcgetattr(fd, saved.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error()).context("failed to read terminal mode");
        }

        let saved = unsafe { saved.assume_init() };
        let mut raw = saved;
        raw.c_iflag &= !(libc::BRKINT | libc::ICRNL | libc::INPCK | libc::ISTRIP | libc::IXON);
        raw.c_oflag &= !libc::OPOST;
        raw.c_cflag |= libc::CS8;
        raw.c_lflag &= !(libc::ECHO | libc::ICANON | libc::IEXTEN | libc::ISIG);
        raw.c_cc[libc::VMIN] = 1;
        raw.c_cc[libc::VTIME] = 0;

        if unsafe { libc::tcsetattr(fd, libc::TCSAFLUSH, &raw) } != 0 {
            return Err(io::Error::last_os_error()).context("failed to set terminal raw mode");
        }

        print!("\x1b[?2004h");
        let _ = io::stdout().flush();

        Ok(Self { fd, saved })
    }
}

impl Drop for RawPromptMode {
    fn drop(&mut self) {
        print!("\x1b[?2004l");
        let _ = io::stdout().flush();

        unsafe {
            libc::tcsetattr(self.fd, libc::TCSAFLUSH, &self.saved);
        }
    }
}

fn read_prompt_line(label: &str, width: usize, paste_count: &mut usize) -> Result<PromptRead> {
    let _raw_mode = RawPromptMode::enter()?;
    let mut input = io::stdin().lock();
    let mut buffer = PromptBuffer::default();
    let mut display_rows = 1usize;

    print!("{label}");
    io::stdout().flush().context("failed to write prompt")?;

    loop {
        let mut byte = [0u8; 1];
        input
            .read_exact(&mut byte)
            .context("failed to read prompt input")?;

        match byte[0] {
            b'\r' | b'\n' => {
                println!();
                return Ok(PromptRead::Line(buffer.full));
            }
            3 => {
                println!("^C");
                return Ok(PromptRead::Interrupted);
            }
            4 if buffer.is_empty() => return Ok(PromptRead::Eof),
            8 | 127 => {
                buffer.pop_last();
                display_rows = redraw_prompt_line(label, &buffer.display, display_rows, width)?;
            }
            0x1b => {
                let sequence = read_escape_sequence(&mut input)?;
                if sequence == b"[200~" {
                    let paste = normalize_pasted_text(read_bracketed_paste(&mut input)?);
                    push_paste(&mut buffer, paste, paste_count);
                    display_rows = redraw_prompt_line(label, &buffer.display, display_rows, width)?;
                }
            }
            byte if byte >= 0x20 && byte != 0x7f => {
                if let Some(ch) = read_utf8_char(byte, &mut input)? {
                    buffer.push_text(ch.encode_utf8(&mut [0; 4]));
                    display_rows = redraw_prompt_line(label, &buffer.display, display_rows, width)?;
                }
            }
            _ => {}
        }
    }
}

fn push_paste(buffer: &mut PromptBuffer, text: String, paste_count: &mut usize) {
    let line_count = pasted_line_count(&text);
    if should_collapse_paste(&text) {
        *paste_count += 1;
        buffer.push_hidden_paste(
            text,
            pasted_text_display_placeholder(*paste_count, line_count),
        );
    } else {
        buffer.push_text(&text);
    }
}

fn redraw_prompt_line(
    label: &str,
    display: &str,
    previous_rows: usize,
    width: usize,
) -> Result<usize> {
    if previous_rows > 1 {
        print!("\x1b[{}A", previous_rows - 1);
    }
    print!("\r\x1b[J{label}{display}");
    io::stdout().flush().context("failed to redraw prompt")?;
    Ok(input_screen_rows(display, width, 2))
}

fn read_escape_sequence(input: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut sequence = Vec::new();
    let mut byte = [0u8; 1];

    input.read_exact(&mut byte)?;
    sequence.push(byte[0]);

    if byte[0] == b'[' {
        loop {
            input.read_exact(&mut byte)?;
            sequence.push(byte[0]);
            if (0x40..=0x7e).contains(&byte[0]) {
                break;
            }
            if sequence.len() >= 16 {
                break;
            }
        }
    }

    Ok(sequence)
}

fn read_bracketed_paste(input: &mut impl Read) -> io::Result<String> {
    const END: &[u8] = b"\x1b[201~";

    let mut bytes = Vec::new();
    let mut byte = [0u8; 1];

    loop {
        input.read_exact(&mut byte)?;
        bytes.push(byte[0]);
        if bytes.ends_with(END) {
            let new_len = bytes.len() - END.len();
            bytes.truncate(new_len);
            return Ok(String::from_utf8_lossy(&bytes).into_owned());
        }
    }
}

fn read_utf8_char(first: u8, input: &mut impl Read) -> io::Result<Option<char>> {
    let expected_len = match first {
        0x00..=0x7f => 1,
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => return Ok(None),
    };

    let mut bytes = vec![first];
    if expected_len > 1 {
        let mut rest = vec![0u8; expected_len - 1];
        input.read_exact(&mut rest)?;
        bytes.extend(rest);
    }

    Ok(std::str::from_utf8(&bytes)
        .ok()
        .and_then(|text| text.chars().next()))
}

fn normalize_pasted_text(text: String) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

fn should_collapse_paste(text: &str) -> bool {
    pasted_line_count(text) > 3 || text.len() > 200
}

fn pasted_line_count(text: &str) -> usize {
    text.lines().count().max(1)
}

fn pasted_text_display_placeholder(paste_count: usize, line_count: usize) -> String {
    format!("[Pasted text #{paste_count} +{line_count} lines]")
}

fn prompt_label(is_tty: bool) -> String {
    if is_tty {
        "\x1b[1;32m❯\x1b[0m ".to_string()
    } else {
        "> ".to_string()
    }
}

fn print_input_separator(is_tty: bool, width: usize) {
    let line = "─".repeat(width);
    if is_tty {
        println!("\x1b[90m{line}\x1b[0m");
    } else {
        println!("{line}");
    }
}

fn input_screen_rows(input: &str, terminal_width: usize, first_row_prefix_width: usize) -> usize {
    let width = terminal_width.max(1);

    input
        .split('\n')
        .enumerate()
        .map(|(index, line)| {
            let prompt_prefix_width = if index == 0 {
                first_row_prefix_width
            } else {
                0
            };
            let columns = line.chars().count() + prompt_prefix_width;
            columns.div_ceil(width).max(1)
        })
        .sum::<usize>()
        .max(1)
}

fn term_width() -> usize {
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

/// Cheap fingerprint for deduplication: length + first 64 base64 chars.
fn image_fingerprint(img: &ImageAttachment) -> String {
    let prefix: String = img.data.chars().take(64).collect();
    format!("{}:{}", img.data.len(), prefix)
}

/// Try to grab an image from the system clipboard and return it base64-encoded.
/// Only supported on macOS; returns None on other platforms or when no image is present.
#[cfg(target_os = "macos")]
fn grab_clipboard_image() -> Option<ImageAttachment> {
    let tmp = format!("/tmp/anveesa_clip_{}.png", std::process::id());

    // AppleScript: cast clipboard to PNG and write to a temp file.
    let script = format!(
        "try\n\
         set d to (the clipboard as \u{00AB}class PNGf\u{00BB})\n\
         set f to open for access POSIX file \"{tmp}\" with write permission\n\
         write d to f\n\
         close access f\n\
         return \"ok\"\n\
         on error\n\
         return \"none\"\n\
         end try"
    );

    let out = std::process::Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .output()
        .ok()?;

    if String::from_utf8_lossy(&out.stdout).trim() != "ok" {
        return None;
    }

    let bytes = std::fs::read(&tmp).ok()?;
    let _ = std::fs::remove_file(&tmp);

    if bytes.len() < 8 {
        return None;
    }

    Some(ImageAttachment {
        mime: "image/png".to_string(),
        data: BASE64.encode(&bytes),
    })
}

#[cfg(not(target_os = "macos"))]
fn grab_clipboard_image() -> Option<ImageAttachment> {
    None
}

fn repl_history_path() -> Option<PathBuf> {
    let path = config_path().ok()?;
    let dir = path.parent()?;
    let _ = fs::create_dir_all(dir);
    Some(dir.join("history"))
}

fn append_repl_history(path: &Path, prompt: &str) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }

    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "{prompt}")
}

fn repl_session_path() -> Option<PathBuf> {
    let path = config_path().ok()?;
    let dir = path.parent()?;
    let _ = fs::create_dir_all(dir);
    Some(dir.join("session.json"))
}

fn load_interactive_session(
    path: &Path,
    cwd: &Path,
    provider: &str,
    options: &AskOptions,
) -> Option<Vec<ChatMessage>> {
    let content = fs::read_to_string(path).ok()?;
    let session: InteractiveSession = serde_json::from_str(&content).ok()?;
    if !session_matches(&session, cwd, provider, options) {
        return None;
    }
    Some(session.messages)
}

fn save_interactive_session(
    path: &Path,
    cwd: &Path,
    provider: &str,
    options: &AskOptions,
    history: &[ChatMessage],
) -> Result<()> {
    let session = InteractiveSession {
        cwd: cwd.display().to_string(),
        provider: provider.to_string(),
        model: options.model.clone(),
        system: options.system.clone(),
        messages: history.to_vec(),
    };
    let content = serde_json::to_string_pretty(&session)
        .context("failed to serialize interactive session")?;
    fs::write(path, content).with_context(|| format!("failed to write {}", path.display()))
}

fn session_matches(
    session: &InteractiveSession,
    cwd: &Path,
    provider: &str,
    options: &AskOptions,
) -> bool {
    session.cwd == cwd.display().to_string()
        && session.provider == provider
        && session.model == options.model
        && session.system == options.system
}

fn workspace_context() -> Result<String> {
    let cwd = std::env::current_dir().context("failed to resolve current directory")?;
    workspace_context_for(&cwd)
}

fn workspace_context_for(cwd: &Path) -> Result<String> {
    let mut context = String::new();

    context.push_str("You are running inside the user's terminal through the Anveesa CLI.\n");
    context.push_str("Use this workspace context when answering questions about where you are, what project this is, or what files are nearby.\n");
    context.push_str(
        "Do not claim you lack terminal location context when the answer is available below.\n\n",
    );
    context.push_str("Workspace:\n");
    context.push_str(&format!("- cwd: {}\n", cwd.display()));
    if let Some(parent) = cwd.parent() {
        context.push_str(&format!("- parent: {}\n", parent.display()));
    }

    if let Some(git_root) = git_output(&cwd, ["rev-parse", "--show-toplevel"]) {
        context.push_str(&format!("- git_root: {git_root}\n"));
        if let Some(branch) = git_output(&cwd, ["branch", "--show-current"])
            && !branch.is_empty()
        {
            context.push_str(&format!("- git_branch: {branch}\n"));
        }
        if let Some(status) = git_output(&cwd, ["status", "--short"]) {
            if status.is_empty() {
                context.push_str("- git_status: clean\n");
            } else {
                context.push_str("- git_status:\n");
                for line in status.lines().take(20) {
                    context.push_str(&format!("  {line}\n"));
                }
            }
        }
    } else {
        context.push_str("- git: not inside a git repository\n");
    }

    let entries = directory_entries(cwd)?;
    if entries.is_empty() {
        context.push_str("- directory_entries: empty\n");
    } else {
        context.push_str("- directory_entries:\n");
        for entry in entries {
            context.push_str(&format!("  {entry}\n"));
        }
    }

    Ok(context)
}

fn directory_entries(cwd: &Path) -> Result<Vec<String>> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(cwd).with_context(|| format!("failed to read {}", cwd.display()))? {
        let entry = entry?;
        let path = entry.path();
        let file_name = entry.file_name().to_string_lossy().into_owned();
        if file_name == ".git" {
            continue;
        }

        let kind = if path.is_dir() {
            "dir"
        } else if path.is_file() {
            "file"
        } else {
            "other"
        };
        entries.push(format!("{file_name}/ ({kind})").replace("/ (file)", " (file)"));
    }

    entries.sort();
    entries.truncate(40);
    Ok(entries)
}

fn git_output<const N: usize>(cwd: &Path, args: [&str; N]) -> Option<String> {
    let output = ProcessCommand::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_prompt_joins_parts() {
        let prompt = build_prompt(vec!["hello".into(), "world".into()], false).unwrap();
        assert_eq!(prompt, "hello world");
    }

    #[test]
    fn one_shot_policy_prompts_only_when_terminal_can_answer() {
        assert_eq!(one_shot_policy(true, false), ApprovalPolicy::Allow);
        assert_eq!(one_shot_policy(false, true), ApprovalPolicy::Prompt);
        assert_eq!(one_shot_policy(false, false), ApprovalPolicy::Deny);
    }

    #[test]
    fn pasted_input_screen_rows_accounts_for_prompt_and_wrapping() {
        assert_eq!(input_screen_rows("hello", 80, 2), 1);
        assert_eq!(input_screen_rows("one\ntwo\nthree", 80, 2), 3);
        assert_eq!(input_screen_rows(&"x".repeat(78), 80, 2), 1);
        assert_eq!(input_screen_rows(&"x".repeat(79), 80, 2), 2);
        assert_eq!(input_screen_rows("", 80, 2), 1);
    }

    #[test]
    fn pasted_text_placeholder_does_not_look_like_a_prompt() {
        let placeholder = pasted_text_display_placeholder(2, 157);

        assert!(placeholder.contains("[Pasted text #2 +157 lines]"));
        assert!(!placeholder.contains("❯"));
    }

    #[test]
    fn prompt_buffer_hidden_paste_preserves_full_text() {
        let mut buffer = PromptBuffer::default();
        let mut paste_count = 0;
        let pasted = "warning: one\nwarning: two\nwarning: three\nwarning: four".to_string();

        buffer.push_text("please read this: ");
        push_paste(&mut buffer, pasted.clone(), &mut paste_count);

        assert_eq!(buffer.full, format!("please read this: {pasted}"));
        assert_eq!(
            buffer.display,
            "please read this: [Pasted text #1 +4 lines]"
        );
    }

    #[test]
    fn interactive_session_matches_same_scope_only() {
        let options = AskOptions {
            provider: Some("provider-a".into()),
            model: Some("model-a".into()),
            system: None,
            stdin: false,
            yes: false,
        };
        let cwd = Path::new("/tmp/anveesa-session");
        let session = InteractiveSession {
            cwd: cwd.display().to_string(),
            provider: "provider-a".into(),
            model: Some("model-a".into()),
            system: None,
            messages: vec![],
        };

        assert!(session_matches(&session, cwd, "provider-a", &options));
        assert!(!session_matches(
            &session,
            Path::new("/tmp/other"),
            "provider-a",
            &options
        ));
        assert!(!session_matches(&session, cwd, "provider-b", &options));
    }

    #[test]
    fn saves_and_loads_interactive_session() {
        let dir = std::env::temp_dir().join(format!("anveesa_session_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.json");
        let options = AskOptions {
            provider: Some("provider-a".into()),
            model: Some("model-a".into()),
            system: None,
            stdin: false,
            yes: false,
        };
        let history = vec![
            ChatMessage::user("continue please".into()),
            ChatMessage::assistant("continuing".into()),
        ];

        save_interactive_session(&path, &dir, "provider-a", &options, &history).unwrap();

        assert_eq!(
            load_interactive_session(&path, &dir, "provider-a", &options),
            Some(history)
        );

        let _ = fs::remove_dir_all(&dir);
    }
}
