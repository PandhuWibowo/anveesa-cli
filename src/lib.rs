pub mod cli;
pub mod config;
pub mod mcp;
pub mod provider;
pub mod tools;
pub mod tui;

use std::{
    fs,
    io::{self, IsTerminal, Read, Write},
    path::{Path, PathBuf},
    process::Command as ProcessCommand,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use clap::{CommandFactory, Parser};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::{
    cli::{AskOptions, Cli, Command, ConfigCommand, SessionsCommand},
    config::{
        AppConfig, ProviderConfig, config_path, init_config, print_path, set_default_model,
        set_default_provider,
    },
    provider::{
        ApprovalDecision, ApprovalPolicy, ChatMessage, ChatRole, DiffKind, ImageAttachment,
        PromptRequest, StreamEvent, ToolConfirmPreview, TurnResult, Usage,
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
    #[serde(default)]
    saved_at: u64,
}

pub async fn run_anveesa() -> Result<()> {
    run_cli(Cli::parse()).await
}

async fn run_cli(cli: Cli) -> Result<()> {
    match cli.command {
        Some(Command::Ask(args)) => run_ask(args.options, args.prompt).await,
        Some(Command::Providers) => list_providers(),
        Some(Command::Config(args)) => run_config(args.command),
        Some(Command::Sessions(args)) => run_sessions(args.command),
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
    let mut provider_name = config
        .provider_name(options.provider.as_deref())?
        .to_string();
    let provider = config
        .providers
        .get(&provider_name)
        .with_context(|| format!("unknown provider '{provider_name}'"))?;
    let _tools_available = matches!(provider, ProviderConfig::OpenAiCompatible(_));
    let mut images_available = matches!(provider, ProviderConfig::OpenAiCompatible(_));
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

    let mut session_options = AskOptions {
        provider: Some(provider_name.clone()),
        model,
        system: options.system,
        stdin: false,
        yes: options.yes,
    };

    let mut accumulated_usage = Usage::default();

    purge_stale_sessions();

    let session_path = repl_session_path(&cwd);
    let loaded_session = session_path
        .as_deref()
        .and_then(|path| load_interactive_session(path, &cwd))
        .or_else(|| {
            // Migrate from the legacy single session.json if it matches our cwd.
            let legacy = legacy_session_path()?;
            let session = load_interactive_session(&legacy, &cwd)?;
            let _ = fs::remove_file(&legacy);
            Some(session)
        });
    let mut history = loaded_session.as_ref().map(|s| s.messages.clone()).unwrap_or_default();
    // saved_at at load time — used only for the startup header so it shows when the previous
    // run ended, not the current run's save time.
    let session_saved_at = loaded_session.as_ref().filter(|s| s.saved_at > 0).map(|s| s.saved_at);
    // tracks the most recent successful save this run — kept fresh for /session display
    let mut last_saved_at: u64 = session_saved_at.unwrap_or(0);
    // Per-project config: .anveesa.toml (extended) or .anveesa (plain system prompt)
    if let Ok(raw) = fs::read_to_string(cwd.join(".anveesa.toml")) {
        if let Ok(cfg) = toml::from_str::<toml::Value>(&raw) {
            if session_options.system.is_none() {
                if let Some(sp) = cfg.get("system_prompt").and_then(|v| v.as_str()) {
                    session_options.system = Some(sp.trim().to_string());
                }
            }
            // Override model if not set by CLI
            if session_options.model.is_none() {
                if let Some(m) = cfg.get("model").and_then(|v| v.as_str()) {
                    session_options.model = Some(m.to_string());
                }
            }
            // auto_approve
            if let Some(true) = cfg.get("auto_approve").and_then(|v| v.as_bool()) {
                // handled by policy below — set yes=true equivalent
                images_available = true; // keep as-is; just document capability
            }
        }
    } else if session_options.system.is_none() {
        if let Ok(text) = fs::read_to_string(cwd.join(".anveesa")) {
            let trimmed = text.trim().to_string();
            if !trimmed.is_empty() {
                session_options.system = Some(trimmed);
            }
        }
    }

    let history_path = repl_history_path();
    // Load prompt history for ↑/↓ recall (one entry per line, newest at end).
    let input_history: Vec<String> = history_path
        .as_deref()
        .and_then(|p| fs::read_to_string(p).ok())
        .map(|c| c.lines().filter(|l| !l.is_empty()).map(String::from).collect())
        .unwrap_or_default();

    print_session_header(
        &provider_name,
        session_options.model.as_deref().unwrap_or("-"),
        history.len() / 2,
        !history.is_empty(),
        session_saved_at,
    );

    let is_tty = io::stdout().is_terminal();

    // ── TUI mode ──────────────────────────────────────────────────────────────
    if is_tty {
        // Connect to any configured MCP servers.
        let mcp_manager = if !config.mcp.is_empty() {
            let m = mcp::McpManager::connect(&config.mcp).await;
            Some(std::sync::Arc::new(m))
        } else {
            None
        };

        // Spawn a background task to read keyboard events (crossterm::event::read is blocking).
        let (key_tx, key_rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::task::spawn_blocking(move || {
            loop {
                match crossterm::event::read() {
                    Ok(ev) => { if key_tx.send(ev).is_err() { break; } }
                    Err(_) => break,
                }
            }
        });

        let short_cwd = std::env::var("HOME")
            .map(|h| cwd.display().to_string().replacen(&h, "~", 1))
            .unwrap_or_else(|_| cwd.display().to_string());

        let app = tui::App::new(
            provider_name.clone(),
            session_options.model.clone().unwrap_or_else(|| "-".to_string()),
            short_cwd,
            history,
            images_available,
            session_path.clone(),
            last_saved_at,
            input_history,
            config,
            session_options,
            workspace_context,
            policy,
            key_rx,
            mcp_manager,
        );

        tui::run(app).await?;
        return Ok(());
    }
    // ── Fallback: plain REPL (non-TTY / piped) ────────────────────────────────

    let width = term_width();
    let label = prompt_label(is_tty);
    // Fingerprint of the last clipboard image we attached — prevents re-attaching
    // the same screenshot on every subsequent turn until the user copies something new.
    let mut last_image_fp: Option<String> = None;
    let mut pending_image: Option<ImageAttachment> = None;
    let mut paste_count = 0usize;

    loop {
        print_input_separator(is_tty, width);
        let (line, ctrl_v_image) =
            match read_prompt_line(&label, width, &mut paste_count, images_available, &input_history) {
                Ok(PromptRead::Line(line, img)) => (line, img),
                Ok(PromptRead::Interrupted) => continue,
                Ok(PromptRead::Eof) => {
                    println!();
                    break;
                }
                Err(error) => return Err(error).context("failed to read interactive prompt"),
            };

        // Ctrl+V image takes precedence over a previously pending image.
        if let Some(img) = ctrl_v_image {
            last_image_fp = Some(image_fingerprint(&img));
            pending_image = Some(img);
        }

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
                pending_image = None;
                paste_count = 0;
                if let Some(path) = &session_path {
                    let _ = fs::remove_file(path);
                }
                if is_tty {
                    println!("\x1b[2m  Conversation cleared.\x1b[0m");
                } else {
                    println!("conversation cleared");
                }
                continue;
            }
            "/help" => {
                print_help_inline(is_tty);
                continue;
            }
            "/session" => {
                print_session_info(
                    is_tty,
                    session_path.as_deref(),
                    history.len() / 2,
                    Some(last_saved_at).filter(|&t| t > 0),
                );
                continue;
            }
            s if s.starts_with("/export") => {
                let arg = s.strip_prefix("/export").unwrap().trim();
                let path = if arg.is_empty() {
                    cwd.join(format!("anveesa-export-{}.md", unix_now()))
                } else {
                    std::path::PathBuf::from(arg)
                };
                match export_conversation(&path, &history) {
                    Ok(()) => {
                        if is_tty {
                            eprintln!("\x1b[2m  Exported to {}\x1b[0m", path.display());
                        } else {
                            println!("exported to {}", path.display());
                        }
                    }
                    Err(e) => eprintln!("\x1b[1;31m✗\x1b[0m {e:#}"),
                }
                continue;
            }
            "/status" => {
                print_status_inline(
                    is_tty,
                    &provider_name,
                    session_options.model.as_deref(),
                    &cwd,
                    history.len() / 2,
                    &accumulated_usage,
                );
                continue;
            }
            s if s.starts_with("/model") => {
                let arg = s.strip_prefix("/model").unwrap().trim();
                if arg.is_empty() {
                    let current = session_options.model.as_deref().unwrap_or("(provider default)");
                    if is_tty {
                        println!("\x1b[2m  model: {current}\x1b[0m");
                    } else {
                        println!("model: {current}");
                    }
                } else {
                    session_options.model = Some(arg.to_string());
                    if is_tty {
                        println!("\x1b[2m  Switched to model: {arg}\x1b[0m");
                    } else {
                        println!("switched model: {arg}");
                    }
                }
                continue;
            }
            s if s.starts_with("/provider") => {
                let arg = s.strip_prefix("/provider").unwrap().trim();
                if arg.is_empty() {
                    if is_tty {
                        println!("\x1b[2m  provider: {provider_name}  model: {}\x1b[0m",
                            session_options.model.as_deref().unwrap_or("(default)"));
                    } else {
                        println!("provider: {provider_name}");
                    }
                } else if !config.providers.contains_key(arg) {
                    if is_tty {
                        eprintln!("\x1b[1;31m✗\x1b[0m unknown provider '{arg}' — run: anveesa providers");
                    } else {
                        eprintln!("error: unknown provider '{arg}'");
                    }
                } else {
                    let new_cfg = config.providers.get(arg).unwrap();
                    images_available = matches!(new_cfg, ProviderConfig::OpenAiCompatible(_));
                    // Reset model to new provider's default
                    session_options.model = new_cfg.default_model().map(str::to_string);
                    provider_name = arg.to_string();
                    session_options.provider = Some(arg.to_string());
                    let model_display = session_options.model.as_deref().unwrap_or("(default)");
                    if is_tty {
                        println!("\x1b[2m  Switched to provider: {arg}  model: {model_display}\x1b[0m");
                    } else {
                        println!("switched provider: {arg}  model: {model_display}");
                    }
                }
                continue;
            }
            _ => {}
        }
        if let Some(path) = parse_attach_command(&prompt) {
            if !images_available {
                if is_tty {
                    eprintln!("\x1b[1;31m✗\x1b[0m image attachments require an openai-compatible provider");
                } else {
                    eprintln!("error: image attachments require an openai-compatible provider");
                }
                continue;
            }

            match attach_image(path.as_deref()) {
                Ok(image) => {
                    last_image_fp = Some(image_fingerprint(&image));
                    pending_image = Some(image);
                    if is_tty {
                        eprintln!("\x1b[2m  Image attached.\x1b[0m");
                    } else {
                        eprintln!("image attached");
                    }
                }
                Err(error) => {
                    if is_tty {
                        eprintln!("\x1b[1;31m✗\x1b[0m {error:#}");
                    } else {
                        eprintln!("error: {error:#}");
                    }
                }
            }
            continue;
        }
        if let Some(path) = &history_path {
            let _ = append_repl_history(path, prompt.as_str());
        }

        // Use an explicitly attached image first. Otherwise, keep the legacy
        // convenience behavior: attach a newly copied clipboard image once.
        let image = if pending_image.is_some() {
            pending_image.take()
        } else if is_tty && images_available {
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
            eprintln!("\x1b[2m  Screenshot from clipboard attached.\x1b[0m");
        }

        let ask_result = tokio::select! {
            r = ask_streaming(
                &config,
                &session_options,
                prompt.clone(),
                &history,
                workspace_context.as_deref(),
                policy,
                image,
                RenderMode::Interactive,
            ) => Some(r),
            _ = tokio::signal::ctrl_c() => None,
        };

        match ask_result {
            Some(Ok(result)) => {
                println!();
                if let Some(u) = result.usage {
                    accumulated_usage.prompt_tokens += u.prompt_tokens;
                    accumulated_usage.completion_tokens += u.completion_tokens;
                    accumulated_usage.total_tokens += u.total_tokens;
                    accumulated_usage.cache_read_tokens += u.cache_read_tokens;
                    accumulated_usage.cache_write_tokens += u.cache_write_tokens;
                }
                history.push(ChatMessage::user(prompt));
                history.push(ChatMessage::assistant(result.text));
                if let Some(path) = &session_path {
                    if save_interactive_session(path, &cwd, &provider_name, &session_options, &history).is_ok() {
                        last_saved_at = unix_now();
                    }
                }
            }
            Some(Err(error)) => {
                if is_tty {
                    eprintln!("\x1b[1;31m✗\x1b[0m {error:#}");
                } else {
                    eprintln!("error: {error:#}");
                }
                println!();
                history.push(ChatMessage::user(prompt));
                history.push(ChatMessage::assistant(format!(
                    "The previous turn failed inside Anveesa before a final answer was produced: {error:#}"
                )));
                if let Some(path) = &session_path {
                    if save_interactive_session(path, &cwd, &provider_name, &session_options, &history).is_ok() {
                        last_saved_at = unix_now();
                    }
                }
            }
            None => {
                // Ctrl+C during streaming — save current history and exit cleanly.
                println!();
                if is_tty {
                    eprintln!("\x1b[2m  ^C  Session saved.\x1b[0m");
                } else {
                    eprintln!("interrupted");
                }
                if let Some(path) = &session_path {
                    let _ = save_interactive_session(path, &cwd, &provider_name, &session_options, &history);
                }
                break;
            }
        }
    }

    if let Some(path) = &session_path {
        let _ = save_interactive_session(path, &cwd, &provider_name, &session_options, &history);
    }
    Ok(())
}

fn run_sessions(command: SessionsCommand) -> Result<()> {
    match command {
        SessionsCommand::List => list_sessions(),
        SessionsCommand::Clear { all } => clear_sessions(all),
    }
}

fn sessions_dir() -> Option<PathBuf> {
    let config_dir = config_path().ok()?.parent()?.to_path_buf();
    Some(config_dir.join("sessions"))
}

fn list_sessions() -> Result<()> {
    let Some(dir) = sessions_dir() else {
        println!("No sessions directory found.");
        return Ok(());
    };
    let Ok(entries) = fs::read_dir(&dir) else {
        println!("No sessions found.");
        return Ok(());
    };

    let mut sessions: Vec<(String, usize, u64)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if let Ok(content) = fs::read_to_string(&path) {
            if let Ok(session) = serde_json::from_str::<InteractiveSession>(&content) {
                sessions.push((session.cwd, session.messages.len() / 2, session.saved_at));
            }
        }
    }
    sessions.sort_by(|a, b| b.2.cmp(&a.2));

    let is_tty = io::stdout().is_terminal();
    if sessions.is_empty() {
        if is_tty {
            eprintln!("\x1b[2m  No saved sessions.\x1b[0m");
        } else {
            println!("no sessions");
        }
        return Ok(());
    }

    if !is_tty {
        for (cwd, turns, saved_at) in &sessions {
            println!("{cwd}\t{turns}\t{saved_at}");
        }
        return Ok(());
    }

    println!();
    println!("\x1b[90m  ──────────────────────────────────────────────────────\x1b[0m");
    for (cwd, turns, saved_at) in &sessions {
        let age = format_session_age(Some(*saved_at));
        let turn_str = if *turns == 1 { "1 turn ".to_string() } else { format!("{turns} turns") };
        let short_cwd = std::env::var("HOME")
            .map(|h| cwd.replacen(&h, "~", 1))
            .unwrap_or_else(|_| cwd.clone());
        println!("  \x1b[2m{age:>10}\x1b[0m  {turn_str:>7}  {short_cwd}");
    }
    println!("\x1b[90m  ──────────────────────────────────────────────────────\x1b[0m");
    println!();
    Ok(())
}

fn clear_sessions(all: bool) -> Result<()> {
    let is_tty = io::stdout().is_terminal();
    if all {
        let mut count = 0usize;
        if let Some(dir) = sessions_dir() {
            if let Ok(entries) = fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().and_then(|e| e.to_str()) == Some("json") {
                        if fs::remove_file(&path).is_ok() {
                            count += 1;
                        }
                    }
                }
            }
        }
        if is_tty {
            eprintln!("\x1b[2m  {count} session(s) deleted.\x1b[0m");
        } else {
            println!("{count} sessions deleted");
        }
    } else {
        let cwd = std::env::current_dir().context("failed to resolve current directory")?;
        let path = repl_session_path(&cwd).context("could not determine session path")?;
        if path.exists() {
            fs::remove_file(&path)
                .with_context(|| format!("failed to delete {}", path.display()))?;
            if is_tty {
                eprintln!("\x1b[2m  Session for {} cleared.\x1b[0m", cwd.display());
            } else {
                println!("session cleared");
            }
        } else {
            if is_tty {
                eprintln!("\x1b[2m  No session for {}.\x1b[0m", cwd.display());
            } else {
                println!("no session");
            }
        }
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
    image: Option<ImageAttachment>, // single-image path kept for REPL compatibility
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
        images: image.into_iter().collect(),
        mcp: None, // REPL path: MCP not yet wired here
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
                Some(StreamEvent::FileOp { verb, path, added, removed, preview, truncated }) => {
                    clear_spinner(spinner, spinner_active);
                    spinner_active = false;
                    if line_open {
                        println!();
                        line_open = false;
                    }
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

fn print_tool_call(summary: &str, is_tty: bool) {
    if is_tty {
        eprintln!("\x1b[90m  └─ {summary}\x1b[0m");
    } else {
        eprintln!("tool: {summary}");
    }
}

fn print_status(message: &str, is_tty: bool) {
    if is_tty {
        eprintln!("\x1b[90m  · {message}\x1b[0m");
    } else {
        eprintln!("status: {message}");
    }
}

fn print_tool_result(summary: &str, ok: bool, elapsed_ms: u128, error: Option<&str>, is_tty: bool) {
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

fn format_duration_ms(ms: u128) -> String {
    if ms >= 1000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        format!("{ms}ms")
    }
}

fn truncate_for_status(value: &str, max_chars: usize) -> String {
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

fn print_status_inline(
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

fn print_help_inline(is_tty: bool) {
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
    println!("  \x1b[1;32m/export\x1b[0m \x1b[2m[path]\x1b[0m     save conversation to a markdown file");
    println!("  \x1b[1;32m/model\x1b[0m \x1b[2m[name]\x1b[0m      switch or show current model");
    println!("  \x1b[1;32m/provider\x1b[0m \x1b[2m[name]\x1b[0m   switch or show current provider");
    println!("  \x1b[1;32m/clear\x1b[0m              reset conversation and delete saved session");
    println!("  \x1b[1;32m/attach\x1b[0m \x1b[2m[path]\x1b[0m     attach image from file or clipboard");
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
    println!("  \x1b[2mCtrl+V\x1b[0m to paste a clipboard image inline (shows \x1b[2m[📎]\x1b[0m indicator).");
    println!("  Or Cmd+C an image and send any message — it attaches automatically.");
    println!("  Or use \x1b[1;32m/attach\x1b[0m \x1b[2mpath/to/file.png\x1b[0m for a specific file.");
    println!("  For broadest clipboard support: \x1b[2mbrew install pngpaste\x1b[0m");
    println!();
}

fn print_session_info(is_tty: bool, path: Option<&Path>, turns: usize, saved_at: Option<u64>) {
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

pub fn export_conversation(path: &std::path::Path, history: &[ChatMessage]) -> Result<()> {
    let mut out = String::new();
    for msg in history {
        match msg.role {
            ChatRole::User => {
                out.push_str("## You\n\n");
                out.push_str(&msg.content);
                out.push_str("\n\n");
            }
            ChatRole::Assistant => {
                out.push_str("## Assistant\n\n");
                out.push_str(&msg.content);
                out.push_str("\n\n");
            }
        }
    }
    fs::write(path, out.trim_end())
        .with_context(|| format!("failed to write {}", path.display()))
}

fn list_providers() -> Result<()> {
    let config = AppConfig::load()?;
    let is_tty = io::stdout().is_terminal();

    if !is_tty {
        for (name, provider) in &config.providers {
            let is_default = config.default_provider.as_deref() == Some(name.as_str());
            let model = provider.default_model().unwrap_or("-");
            println!("{}  {name}  {model}  {}", if is_default { "*" } else { " " }, provider.kind());
        }
        return Ok(());
    }

    println!();
    for (name, provider) in &config.providers {
        let is_default = config.default_provider.as_deref() == Some(name.as_str());
        let model = provider.default_model().unwrap_or("-");
        let default_tag = if is_default {
            "  \x1b[1;32m●  default\x1b[0m"
        } else {
            ""
        };
        println!(
            "  \x1b[1m{name}\x1b[0m  \x1b[2m{model}  {}\x1b[0m{default_tag}",
            provider.kind()
        );
    }
    println!();
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
    turns: usize,
    resumed: bool,
    saved_at: Option<u64>,
) {
    let is_tty = io::stdout().is_terminal();
    let version = env!("CARGO_PKG_VERSION");

    if !is_tty {
        let tag = if resumed {
            format!(" (resumed · {turns} turns · {})", format_session_age(saved_at))
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
        format!(" · Resumed ({turns} turns · {})", format_session_age(saved_at))
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

enum PromptRead {
    Line(String, Option<ImageAttachment>),
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
    /// Byte offset into `full` — where the next insertion goes.
    cursor: usize,
}

impl PromptBuffer {
    fn is_empty(&self) -> bool {
        self.full.is_empty()
    }

    /// Char offset in `display` that corresponds to the current cursor position in `full`.
    /// Used to position the terminal cursor after a redraw.
    fn display_cursor_char(&self) -> usize {
        let mut full_pos = 0usize;
        let mut disp_chars = 0usize;
        for seg in &self.segments {
            let seg_len = seg.full.len();
            let next_pos = full_pos + seg_len;
            if self.cursor <= next_pos {
                let offset = self.cursor - full_pos;
                return if seg.hidden {
                    // Hidden spans are atomic: cursor snaps to end of placeholder.
                    disp_chars + seg.display.chars().count()
                } else {
                    disp_chars + seg.full[..offset].chars().count()
                };
            }
            full_pos = next_pos;
            disp_chars += seg.display.chars().count();
        }
        disp_chars
    }

    fn push_text(&mut self, text: &str) {
        // Find the segment containing the cursor and insert there.
        let mut pos = 0usize;
        for seg in self.segments.iter_mut() {
            let seg_len = seg.full.len();
            if !seg.hidden && self.cursor >= pos && self.cursor <= pos + seg_len {
                let offset = self.cursor - pos;
                seg.full.insert_str(offset, text);
                seg.display.insert_str(offset, text);
                self.cursor += text.len();
                self.rebuild_flat();
                return;
            }
            pos += seg_len;
        }
        // Cursor is at end or after a hidden segment — append to last visible segment.
        if let Some(seg) = self.segments.last_mut().filter(|s| !s.hidden) {
            seg.full.push_str(text);
            seg.display.push_str(text);
        } else {
            self.segments.push(PromptSegment {
                full: text.to_string(),
                display: text.to_string(),
                hidden: false,
            });
        }
        self.cursor += text.len();
        self.rebuild_flat();
    }

    fn push_hidden_paste(&mut self, text: String, display: String) {
        self.full.push_str(&text);
        self.display.push_str(&display);
        self.cursor = self.full.len();
        self.segments.push(PromptSegment {
            full: text,
            display,
            hidden: true,
        });
    }

    /// Delete the character immediately before the cursor.
    /// Deletes the entire span atomically if the cursor is just past a hidden span.
    fn delete_before_cursor(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let mut pos = 0usize;
        let mut remove_idx: Option<usize> = None;
        for (i, seg) in self.segments.iter_mut().enumerate() {
            let seg_len = seg.full.len();
            let next_pos = pos + seg_len;
            if seg.hidden && next_pos == self.cursor {
                // cursor is right after a hidden span — delete the whole span
                self.cursor -= seg_len;
                remove_idx = Some(i);
                break;
            }
            if !seg.hidden && self.cursor > pos && self.cursor <= next_pos {
                let offset = self.cursor - pos;
                if let Some(ch) = seg.full[..offset].chars().next_back() {
                    let ch_len = ch.len_utf8();
                    seg.full.drain((offset - ch_len)..offset);
                    seg.display.drain((offset - ch_len)..offset);
                    self.cursor -= ch_len;
                    if seg.full.is_empty() {
                        remove_idx = Some(i);
                    }
                }
                break;
            }
            pos = next_pos;
        }
        if let Some(i) = remove_idx {
            self.segments.remove(i);
        }
        self.rebuild_flat();
    }

    fn move_left(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let mut pos = 0usize;
        for seg in &self.segments {
            let next_pos = pos + seg.full.len();
            if seg.hidden && next_pos == self.cursor {
                self.cursor = pos;
                return;
            }
            if !seg.hidden && self.cursor > pos && self.cursor <= next_pos {
                let offset = self.cursor - pos;
                if let Some(ch) = seg.full[..offset].chars().next_back() {
                    self.cursor -= ch.len_utf8();
                }
                return;
            }
            pos = next_pos;
        }
    }

    fn move_right(&mut self) {
        if self.cursor >= self.full.len() {
            return;
        }
        let mut pos = 0usize;
        for seg in &self.segments {
            let seg_len = seg.full.len();
            if seg.hidden && pos == self.cursor {
                self.cursor += seg_len;
                return;
            }
            if !seg.hidden && self.cursor >= pos && self.cursor < pos + seg_len {
                let offset = self.cursor - pos;
                if let Some(ch) = seg.full[offset..].chars().next() {
                    self.cursor += ch.len_utf8();
                }
                return;
            }
            pos += seg_len;
        }
    }

    fn move_home(&mut self) {
        self.cursor = 0;
    }

    fn move_end(&mut self) {
        self.cursor = self.full.len();
    }

    /// Ctrl+U / Cmd+Delete — erase the entire line.
    fn clear_all(&mut self) {
        self.full.clear();
        self.display.clear();
        self.segments.clear();
        self.cursor = 0;
    }

    /// Ctrl+W / Option+Delete — erase the last word before the cursor.
    fn pop_word(&mut self) {
        while self.cursor > 0 && self.full[..self.cursor].ends_with(' ') {
            self.delete_before_cursor();
        }
        while self.cursor > 0 && !self.full[..self.cursor].ends_with(' ') {
            self.delete_before_cursor();
        }
    }

    fn rebuild_flat(&mut self) {
        self.full = self.segments.iter().map(|s| s.full.as_str()).collect();
        self.display = self.segments.iter().map(|s| s.display.as_str()).collect();
    }
}

#[cfg(unix)]
struct RawPromptMode {
    fd: i32,
    saved: libc::termios,
}

#[cfg(unix)]
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

#[cfg(unix)]
impl Drop for RawPromptMode {
    fn drop(&mut self) {
        print!("\x1b[?2004l");
        let _ = io::stdout().flush();

        unsafe {
            libc::tcsetattr(self.fd, libc::TCSAFLUSH, &self.saved);
        }
    }
}

#[cfg(not(unix))]
struct RawPromptMode;

#[cfg(not(unix))]
impl RawPromptMode {
    fn enter() -> Result<Self> {
        Ok(Self)
    }
}

/// After a redraw (which leaves the terminal cursor at end of display), move it
/// back to the buffer's logical cursor position.
fn position_prompt_cursor(display: &str, cursor_char: usize) -> io::Result<()> {
    let back = display.chars().count().saturating_sub(cursor_char);
    if back > 0 {
        print!("\x1b[{}D", back);
        io::stdout().flush()?;
    }
    Ok(())
}

fn read_prompt_line(
    label: &str,
    width: usize,
    paste_count: &mut usize,
    images_available: bool,
    input_history: &[String],
) -> Result<PromptRead> {
    let _raw_mode = RawPromptMode::enter()?;
    let mut input = io::stdin().lock();
    let mut buffer = PromptBuffer::default();
    let mut display_rows = 1usize;
    let mut ctrl_v_image: Option<ImageAttachment> = None;

    // History navigation state.
    let mut hist_idx: Option<usize> = None; // None = current live input
    let mut saved_input = String::new();    // stash live input when navigating into history

    // Compose the visible prompt label, optionally prefixed with an image indicator.
    let effective_label = |img: &Option<ImageAttachment>| -> String {
        if img.is_some() {
            format!("\x1b[2m[📎]\x1b[0m {label}")
        } else {
            label.to_string()
        }
    };

    // Redraw the line and position the cursor, returning the new row count.
    macro_rules! redraw {
        () => {{
            let lbl = effective_label(&ctrl_v_image);
            let rows = redraw_prompt_line(&lbl, &buffer.display, display_rows, width)?;
            let _ = position_prompt_cursor(&buffer.display, buffer.display_cursor_char());
            rows
        }};
    }

    print!("{}", effective_label(&ctrl_v_image));
    io::stdout().flush().context("failed to write prompt")?;

    loop {
        let mut byte = [0u8; 1];
        input
            .read_exact(&mut byte)
            .context("failed to read prompt input")?;

        match byte[0] {
            b'\r' | b'\n' => {
                println!();
                return Ok(PromptRead::Line(buffer.full, ctrl_v_image));
            }
            3 => {
                println!("^C");
                return Ok(PromptRead::Interrupted);
            }
            4 if buffer.is_empty() => return Ok(PromptRead::Eof),
            8 | 127 => {
                // Backspace
                buffer.delete_before_cursor();
                display_rows = redraw!();
            }
            21 => {
                // Ctrl+U / Cmd+Delete — erase entire line
                buffer.clear_all();
                display_rows = redraw!();
            }
            22 => {
                // Ctrl+V — universal paste: image first, then clipboard text
                if images_available {
                    if let Some(img) = grab_clipboard_image() {
                        ctrl_v_image = Some(img);
                        display_rows = redraw!();
                        continue;
                    }
                }
                // Fall back to clipboard text via pbpaste / xclip
                if let Some(text) = read_clipboard_text() {
                    if !text.is_empty() {
                        buffer.push_text(&text.replace('\r', "\n"));
                        display_rows = redraw!();
                    }
                }
            }
            23 => {
                // Ctrl+W / Option+Delete — erase last word
                buffer.pop_word();
                display_rows = redraw!();
            }
            0x1b => {
                let sequence = read_escape_sequence(&mut input)?;
                match sequence.as_slice() {
                    b"[200~" => {
                        // Bracketed paste
                        let paste = normalize_pasted_text(read_bracketed_paste(&mut input)?);
                        push_paste(&mut buffer, paste, paste_count);
                        display_rows = redraw!();
                    }
                    b"[A" => {
                        // Up arrow — previous history entry
                        if input_history.is_empty() {
                            continue;
                        }
                        let new_idx = match hist_idx {
                            None => {
                                saved_input = buffer.full.clone();
                                input_history.len() - 1
                            }
                            Some(0) => 0,
                            Some(i) => i - 1,
                        };
                        hist_idx = Some(new_idx);
                        buffer = PromptBuffer::default();
                        buffer.push_text(&input_history[new_idx].clone());
                        display_rows = redraw!();
                    }
                    b"[B" => {
                        // Down arrow — next history entry / back to live input
                        match hist_idx {
                            None => {}
                            Some(i) if i + 1 >= input_history.len() => {
                                hist_idx = None;
                                let text = std::mem::take(&mut saved_input);
                                buffer = PromptBuffer::default();
                                buffer.push_text(&text);
                                display_rows = redraw!();
                            }
                            Some(i) => {
                                hist_idx = Some(i + 1);
                                buffer = PromptBuffer::default();
                                buffer.push_text(&input_history[i + 1].clone());
                                display_rows = redraw!();
                            }
                        }
                    }
                    b"[C" => {
                        // Right arrow
                        buffer.move_right();
                        let _ = position_prompt_cursor(
                            &buffer.display,
                            buffer.display_cursor_char(),
                        );
                    }
                    b"[D" => {
                        // Left arrow
                        buffer.move_left();
                        let _ = position_prompt_cursor(
                            &buffer.display,
                            buffer.display_cursor_char(),
                        );
                    }
                    b"[H" | b"[1~" => {
                        // Home
                        buffer.move_home();
                        let _ = position_prompt_cursor(&buffer.display, 0);
                    }
                    b"[F" | b"[4~" => {
                        // End
                        buffer.move_end();
                        let _ = position_prompt_cursor(
                            &buffer.display,
                            buffer.display_cursor_char(),
                        );
                    }
                    _ => {}
                }
            }
            byte if byte >= 0x20 && byte != 0x7f => {
                if let Some(ch) = read_utf8_char(byte, &mut input)? {
                    buffer.push_text(ch.encode_utf8(&mut [0; 4]));
                    display_rows = redraw!();
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
pub fn image_fingerprint(img: &ImageAttachment) -> String {
    let prefix: String = img.data.chars().take(64).collect();
    format!("{}:{}", img.data.len(), prefix)
}

fn parse_attach_command(prompt: &str) -> Option<Option<String>> {
    for command in ["/attach", "/image", "/img"] {
        if prompt == command {
            return Some(None);
        }
        if let Some(rest) = prompt.strip_prefix(command)
            && rest.chars().next().is_some_and(char::is_whitespace)
        {
            let path = unquote_path(rest.trim());
            if !path.is_empty() {
                return Some(Some(path.to_string()));
            }
            return Some(None);
        }
    }
    None
}

fn unquote_path(path: &str) -> &str {
    let trimmed = path.trim();
    if trimmed.len() >= 2 {
        let bytes = trimmed.as_bytes();
        if (bytes[0] == b'"' && bytes[trimmed.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[trimmed.len() - 1] == b'\'')
        {
            return &trimmed[1..trimmed.len() - 1];
        }
    }
    trimmed
}

fn attach_image(path: Option<&str>) -> Result<ImageAttachment> {
    match path {
        Some(path) => load_image_file(Path::new(path)),
        None => read_clipboard_image().context(
            "no image found in clipboard — copy an image first, or for broader format support: brew install pngpaste",
        ),
    }
}

fn load_image_file(path: &Path) -> Result<ImageAttachment> {
    const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;

    let metadata =
        fs::metadata(path).with_context(|| format!("failed to read {}", path.display()))?;
    if !metadata.is_file() {
        bail!("{} is not a file", path.display());
    }
    if metadata.len() > MAX_IMAGE_BYTES {
        bail!(
            "{} is too large for an image attachment ({} MB max)",
            path.display(),
            MAX_IMAGE_BYTES / 1024 / 1024
        );
    }

    let mime = image_mime_for_path(path)
        .with_context(|| format!("unsupported image type for {}", path.display()))?;
    let bytes = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    if bytes.is_empty() {
        bail!("{} is empty", path.display());
    }

    Ok(ImageAttachment {
        mime: mime.to_string(),
        data: BASE64.encode(&bytes),
    })
}

fn image_mime_for_path(path: &Path) -> Option<&'static str> {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .as_deref()
    {
        Some("png") => Some("image/png"),
        Some("jpg") | Some("jpeg") => Some("image/jpeg"),
        Some("webp") => Some("image/webp"),
        Some("gif") => Some("image/gif"),
        _ => None,
    }
}

/// Try to grab an image from the system clipboard and return it base64-encoded.
/// Only supported on macOS; returns None on other platforms or when no image is present.
#[cfg(target_os = "macos")]
pub fn grab_clipboard_image() -> Option<ImageAttachment> {
    read_clipboard_image().ok()
}

/// Try to grab an image from the system clipboard and return it base64-encoded.
#[cfg(target_os = "macos")]
fn read_clipboard_image() -> Result<ImageAttachment> {
    // pngpaste handles all modern macOS clipboard formats (install: brew install pngpaste)
    if let Ok(bytes) = read_clipboard_via_pngpaste() {
        return Ok(ImageAttachment {
            mime: "image/png".to_string(),
            data: BASE64.encode(&bytes),
        });
    }

    // JXA via NSPasteboard: catches public.png (browsers, web apps)
    if let Ok(bytes) = read_clipboard_via_jxa("public.png") {
        return Ok(ImageAttachment {
            mime: "image/png".to_string(),
            data: BASE64.encode(&bytes),
        });
    }

    // JXA via NSPasteboard: catches public.tiff (screenshots, Preview, most macOS apps)
    if let Ok(tiff) = read_clipboard_via_jxa("public.tiff") {
        let png = convert_tiff_to_png(&tiff)?;
        return Ok(ImageAttachment {
            mime: "image/png".to_string(),
            data: BASE64.encode(&png),
        });
    }

    // Legacy AppleScript class-code fallback
    if let Ok(bytes) = read_clipboard_class_bytes("PNGf", "png") {
        return Ok(ImageAttachment {
            mime: "image/png".to_string(),
            data: BASE64.encode(&bytes),
        });
    }
    if let Ok(bytes) = read_clipboard_class_bytes("JPEG", "jpg") {
        return Ok(ImageAttachment {
            mime: "image/jpeg".to_string(),
            data: BASE64.encode(&bytes),
        });
    }
    if let Ok(tiff) = read_clipboard_class_bytes("TIFF", "tiff") {
        let png = convert_tiff_to_png(&tiff)?;
        return Ok(ImageAttachment {
            mime: "image/png".to_string(),
            data: BASE64.encode(&png),
        });
    }

    bail!("no image found in clipboard — copy an image first, or use: /attach path/to/image.png")
}

/// Read clipboard image using pngpaste (brew install pngpaste) — most reliable option.
#[cfg(target_os = "macos")]
fn read_clipboard_via_pngpaste() -> Result<Vec<u8>> {
    let tmp = std::env::temp_dir().join(format!("anveesa_pp_{}.png", std::process::id()));
    let status = std::process::Command::new("pngpaste")
        .arg(&tmp)
        .status()
        .context("pngpaste not available")?;
    if !status.success() {
        let _ = fs::remove_file(&tmp);
        bail!("pngpaste: no image in clipboard");
    }
    let bytes = fs::read(&tmp)?;
    let _ = fs::remove_file(&tmp);
    if bytes.len() < 8 {
        bail!("empty image from pngpaste");
    }
    Ok(bytes)
}

/// Read clipboard image via JXA + NSPasteboard using a modern UTI type.
/// This correctly handles images copied from browsers and web apps.
#[cfg(target_os = "macos")]
fn read_clipboard_via_jxa(pb_type: &str) -> Result<Vec<u8>> {
    let script = format!(
        "ObjC.import('AppKit'); \
         var d = $.NSPasteboard.generalPasteboard.dataForType('{pb_type}'); \
         d && d.length > 0 ? d.base64EncodedStringWithOptions(0).js : 'none'"
    );
    let out = std::process::Command::new("osascript")
        .arg("-l")
        .arg("JavaScript")
        .arg("-e")
        .arg(&script)
        .output()
        .context("osascript not available")?;
    let result = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !out.status.success() || result == "none" || result.is_empty() {
        bail!("no {pb_type} data in clipboard");
    }
    let clean: String = result.chars().filter(|c| !c.is_whitespace()).collect();
    BASE64
        .decode(clean.as_bytes())
        .context("failed to decode clipboard image data from JXA")
}

#[cfg(target_os = "macos")]
fn read_clipboard_class_bytes(class_code: &str, extension: &str) -> Result<Vec<u8>> {
    let tmp = std::env::temp_dir().join(format!(
        "anveesa_clip_{}_{}.{}",
        std::process::id(),
        class_code,
        extension
    ));
    let tmp_display = tmp.display();
    let script = format!(
        "try\n\
         set d to (the clipboard as \u{00AB}class {class_code}\u{00BB})\n\
         set f to open for access POSIX file \"{tmp_display}\" with write permission\n\
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
        .context("failed to read macOS clipboard with osascript")?;

    if String::from_utf8_lossy(&out.stdout).trim() != "ok" {
        let _ = fs::remove_file(&tmp);
        bail!("clipboard does not contain {class_code} image data");
    }

    let bytes = fs::read(&tmp).with_context(|| format!("failed to read {tmp_display}"))?;
    let _ = fs::remove_file(&tmp);

    if bytes.len() < 8 {
        bail!("clipboard {class_code} image data is empty");
    }

    Ok(bytes)
}

#[cfg(target_os = "macos")]
fn convert_tiff_to_png(tiff: &[u8]) -> Result<Vec<u8>> {
    let base = std::env::temp_dir().join(format!("anveesa_clip_{}", std::process::id()));
    let tiff_path = base.with_extension("tiff");
    let png_path = base.with_extension("png");
    fs::write(&tiff_path, tiff).context("failed to write temporary TIFF clipboard image")?;

    let status = std::process::Command::new("sips")
        .arg("-s")
        .arg("format")
        .arg("png")
        .arg(&tiff_path)
        .arg("--out")
        .arg(&png_path)
        .status()
        .context("failed to convert clipboard TIFF to PNG with sips")?;

    let _ = fs::remove_file(&tiff_path);
    if !status.success() {
        let _ = fs::remove_file(&png_path);
        bail!("failed to convert clipboard TIFF image to PNG");
    }

    let bytes = fs::read(&png_path).context("failed to read converted clipboard PNG")?;
    let _ = fs::remove_file(&png_path);
    if bytes.len() < 8 {
        bail!("converted clipboard PNG is empty");
    }
    Ok(bytes)
}

#[cfg(not(target_os = "macos"))]
pub fn grab_clipboard_image() -> Option<ImageAttachment> {
    None
}

#[cfg(not(target_os = "macos"))]
fn read_clipboard_image() -> Result<ImageAttachment> {
    bail!("clipboard image attachment is only supported on macOS; use /attach path/to/image.png")
}

fn repl_history_path() -> Option<PathBuf> {
    let path = config_path().ok()?;
    let dir = path.parent()?;
    let _ = fs::create_dir_all(dir);
    Some(dir.join("history"))
}

pub fn read_clipboard_text() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("pbpaste").output().ok()?;
        if out.status.success() {
            let text = String::from_utf8_lossy(&out.stdout).into_owned();
            if !text.is_empty() { return Some(text); }
        }
    }
    #[cfg(not(target_os = "macos"))]
    for (cmd, args) in &[
        ("wl-paste", vec!["--no-newline"]),
        ("xclip",   vec!["-o", "-selection", "clipboard"]),
        ("xsel",    vec!["--clipboard", "--output"]),
    ] {
        if let Ok(out) = std::process::Command::new(cmd).args(args).output() {
            if out.status.success() {
                let text = String::from_utf8_lossy(&out.stdout).into_owned();
                if !text.is_empty() { return Some(text); }
            }
        }
    }
    None
}

pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn format_session_age(saved_at: Option<u64>) -> String {
    let Some(ts) = saved_at else {
        return "unknown age".to_string();
    };
    let secs = unix_now().saturating_sub(ts);
    if secs < 60 {
        "just now".to_string()
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86400)
    }
}

/// FNV-1a 64-bit hash of the cwd path — used as a stable per-directory session filename.
fn cwd_session_hash(cwd: &Path) -> String {
    let s = cwd.to_string_lossy();
    let mut h: u64 = 14695981039346656037;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(1099511628211);
    }
    format!("{h:016x}")
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

/// Delete all session files whose saved_at is older than 30 days.  Called once at
/// startup so orphaned sessions (from deleted/moved projects) eventually disappear.
fn purge_stale_sessions() {
    let Some(dir) = sessions_dir() else { return };
    let Ok(entries) = fs::read_dir(&dir) else { return };
    let cutoff = unix_now().saturating_sub(30 * 24 * 3600);
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let stale = fs::read_to_string(&path)
            .ok()
            .and_then(|c| serde_json::from_str::<InteractiveSession>(&c).ok())
            .map(|s| s.saved_at > 0 && s.saved_at < cutoff)
            .unwrap_or(true); // unparseable → delete
        if stale {
            let _ = fs::remove_file(&path);
        }
    }
}

/// Per-directory session file: ~/.config/anveesa/sessions/{cwd-hash}.json
fn repl_session_path(cwd: &Path) -> Option<PathBuf> {
    let config_dir = config_path().ok()?.parent()?.to_path_buf();
    let sessions_dir = config_dir.join("sessions");
    let _ = fs::create_dir_all(&sessions_dir);
    Some(sessions_dir.join(format!("{}.json", cwd_session_hash(cwd))))
}

/// Legacy path for backward-compat migration.
fn legacy_session_path() -> Option<PathBuf> {
    let config_dir = config_path().ok()?.parent()?.to_path_buf();
    let path = config_dir.join("session.json");
    if path.exists() { Some(path) } else { None }
}

fn load_interactive_session(path: &Path, cwd: &Path) -> Option<InteractiveSession> {
    let content = fs::read_to_string(path).ok()?;
    let session: InteractiveSession = serde_json::from_str(&content).ok()?;
    if session.cwd != cwd.display().to_string() {
        return None;
    }
    // Auto-expire sessions older than 30 days.
    if session.saved_at > 0 && unix_now().saturating_sub(session.saved_at) > 30 * 24 * 3600 {
        let _ = fs::remove_file(path);
        return None;
    }
    Some(session)
}

pub fn save_interactive_session_pub(
    path: &Path,
    cwd: &Path,
    provider: &str,
    options: &AskOptions,
    history: &[ChatMessage],
) -> Result<()> {
    save_interactive_session(path, cwd, provider, options, history)
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
        saved_at: unix_now(),
    };
    let content = serde_json::to_string_pretty(&session)
        .context("failed to serialize interactive session")?;
    fs::write(path, content).with_context(|| format!("failed to write {}", path.display()))
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

    // .anveesa.md — project-level instructions (highest priority context)
    let project_md_paths = [cwd.join(".anveesa.md"), cwd.join("ANVEESA.md")];
    for md_path in &project_md_paths {
        if let Ok(content) = fs::read_to_string(md_path) {
            if !content.trim().is_empty() {
                context.push_str("\nProject instructions (.anveesa.md):\n");
                let capped: String = content.chars().take(8_000).collect();
                context.push_str(&capped);
                context.push('\n');
            }
            break;
        }
    }

    // README — auto-inject up to 3 000 chars for project overview
    for readme in &["README.md", "readme.md", "Readme.md"] {
        if let Ok(content) = fs::read_to_string(cwd.join(readme)) {
            if !content.trim().is_empty() {
                context.push_str("\nProject README (first 3000 chars):\n");
                let capped: String = content.chars().take(3_000).collect();
                context.push_str(&capped);
                context.push('\n');
            }
            break;
        }
    }

    if let Some(git_root) = git_output(&cwd, ["rev-parse", "--show-toplevel"]) {
        // Also check git root for .anveesa.md if different from cwd
        let git_root_path = std::path::Path::new(&git_root);
        if git_root_path != cwd {
            for md_path in &[git_root_path.join(".anveesa.md"), git_root_path.join("ANVEESA.md")] {
                if let Ok(content) = fs::read_to_string(md_path) {
                    if !content.trim().is_empty() {
                        context.push_str("\nProject instructions (from git root):\n");
                        let capped: String = content.chars().take(8_000).collect();
                        context.push_str(&capped);
                        context.push('\n');
                    }
                    break;
                }
            }
        }
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
        // Recent commits give the model useful project history context
        if let Some(log) = git_output(&cwd, ["log", "--oneline", "--decorate", "-8"]) {
            if !log.is_empty() {
                context.push_str("- recent_commits:\n");
                for line in log.lines() {
                    context.push_str(&format!("  {line}\n"));
                }
            }
        }
    } else {
        context.push_str("- git: not inside a git repository\n");
    }

    // Available notes
    let notes_dir = config_path().ok()
        .and_then(|p| p.parent().map(|d| d.join("notes")));
    if let Some(dir) = notes_dir.filter(|d| d.exists()) {
        let note_keys: Vec<String> = fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let path = e.path();
                if path.extension()?.to_str()? == "md" {
                    path.file_stem()?.to_str().map(str::to_string)
                } else { None }
            })
            .collect();
        if !note_keys.is_empty() {
            context.push_str(&format!("- saved_notes: {}\n", note_keys.join(", ")));
        }
    }

    // Project metadata from package.json / Cargo.toml
    if let Ok(raw) = fs::read_to_string(cwd.join("package.json")) {
        if let Ok(pkg) = serde_json::from_str::<serde_json::Value>(&raw) {
            if let Some(name) = pkg["name"].as_str() {
                context.push_str(&format!("- project_name: {name}\n"));
            }
            if let Some(ver) = pkg["version"].as_str() {
                context.push_str(&format!("- project_version: {ver}\n"));
            }
            if let Some(desc) = pkg["description"].as_str() {
                context.push_str(&format!("- project_description: {desc}\n"));
            }
        }
    } else if let Ok(raw) = fs::read_to_string(cwd.join("Cargo.toml")) {
        for line in raw.lines().take(15) {
            if line.starts_with("name") || line.starts_with("version") || line.starts_with("description") {
                context.push_str(&format!("- cargo_{}\n", line.trim()));
            }
        }
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
    fn parses_attach_commands() {
        assert_eq!(parse_attach_command("/attach"), Some(None));
        assert_eq!(
            parse_attach_command("/attach screenshot.png"),
            Some(Some("screenshot.png".into()))
        );
        assert_eq!(
            parse_attach_command("/attach \"folder/my image.jpg\""),
            Some(Some("folder/my image.jpg".into()))
        );
        assert_eq!(
            parse_attach_command("/img '/tmp/capture.webp'"),
            Some(Some("/tmp/capture.webp".into()))
        );
        assert_eq!(parse_attach_command("/attachment nope"), None);
    }

    #[test]
    fn detects_image_mime_from_path() {
        assert_eq!(image_mime_for_path(Path::new("a.png")), Some("image/png"));
        assert_eq!(image_mime_for_path(Path::new("a.JPEG")), Some("image/jpeg"));
        assert_eq!(image_mime_for_path(Path::new("a.webp")), Some("image/webp"));
        assert_eq!(image_mime_for_path(Path::new("a.txt")), None);
    }

    #[test]
    fn interactive_session_matches_cwd_only() {
        let cwd = Path::new("/tmp/anveesa-session");
        let session = InteractiveSession {
            cwd: cwd.display().to_string(),
            provider: "provider-a".into(),
            model: Some("model-a".into()),
            system: None,
            messages: vec![],
            saved_at: 0,
        };

        // Matches when cwd is the same.
        assert_eq!(session.cwd, cwd.display().to_string());
        // A different cwd should not match.
        assert_ne!(session.cwd, Path::new("/tmp/other").display().to_string());
        // Provider/model differences no longer prevent a session from loading.
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

        let loaded = load_interactive_session(&path, &dir).unwrap();
        assert_eq!(loaded.messages, history);
        // saved_at should be set.
        assert!(loaded.saved_at > 0);

        let _ = fs::remove_dir_all(&dir);
    }

    // ── cwd_session_hash ──────────────────────────────────────────────────────

    #[test]
    fn cwd_hash_is_deterministic() {
        let p = Path::new("/home/user/my-project");
        assert_eq!(cwd_session_hash(p), cwd_session_hash(p));
    }

    #[test]
    fn cwd_hash_differs_for_different_paths() {
        let a = cwd_session_hash(Path::new("/home/user/project-a"));
        let b = cwd_session_hash(Path::new("/home/user/project-b"));
        let c = cwd_session_hash(Path::new("/home/user/project-a/sub"));
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert_ne!(b, c);
    }

    #[test]
    fn cwd_hash_is_16_hex_chars() {
        let h = cwd_session_hash(Path::new("/any/path"));
        assert_eq!(h.len(), 16);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }

    // ── format_session_age ────────────────────────────────────────────────────

    #[test]
    fn format_age_none_returns_unknown() {
        assert_eq!(format_session_age(None), "unknown age");
    }

    #[test]
    fn format_age_just_now() {
        let ts = unix_now();
        assert_eq!(format_session_age(Some(ts)), "just now");
        assert_eq!(format_session_age(Some(ts - 59)), "just now");
    }

    #[test]
    fn format_age_minutes() {
        let ts = unix_now() - 60;
        assert_eq!(format_session_age(Some(ts)), "1m ago");
        let ts2 = unix_now() - 3599;
        assert_eq!(format_session_age(Some(ts2)), "59m ago");
    }

    #[test]
    fn format_age_hours() {
        let ts = unix_now() - 3600;
        assert_eq!(format_session_age(Some(ts)), "1h ago");
        let ts2 = unix_now() - 86399;
        assert_eq!(format_session_age(Some(ts2)), "23h ago");
    }

    #[test]
    fn format_age_days() {
        let ts = unix_now() - 86400;
        assert_eq!(format_session_age(Some(ts)), "1d ago");
        let ts2 = unix_now() - 7 * 86400;
        assert_eq!(format_session_age(Some(ts2)), "7d ago");
    }

    // ── 30-day expiry ─────────────────────────────────────────────────────────

    #[test]
    fn expired_session_is_deleted_on_load() {
        let dir = std::env::temp_dir().join(format!("anveesa_expiry_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("old_session.json");
        let options = AskOptions { provider: Some("p".into()), model: None, system: None, stdin: false, yes: false };
        save_interactive_session(&path, &dir, "p", &options, &[]).unwrap();

        // Backdate saved_at by 31 days.
        let content = fs::read_to_string(&path).unwrap();
        let mut session: InteractiveSession = serde_json::from_str(&content).unwrap();
        session.saved_at = unix_now() - 31 * 24 * 3600;
        fs::write(&path, serde_json::to_string_pretty(&session).unwrap()).unwrap();

        let result = load_interactive_session(&path, &dir);
        assert!(result.is_none(), "expired session must not load");
        assert!(!path.exists(), "expired session file must be deleted");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn non_expired_session_loads_normally() {
        let dir = std::env::temp_dir().join(format!("anveesa_noexpiry_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.json");
        let options = AskOptions { provider: Some("p".into()), model: None, system: None, stdin: false, yes: false };
        let history = vec![ChatMessage::user("hi".into()), ChatMessage::assistant("hello".into())];
        save_interactive_session(&path, &dir, "p", &options, &history).unwrap();

        let loaded = load_interactive_session(&path, &dir).unwrap();
        assert_eq!(loaded.messages, history);
        assert!(path.exists());

        let _ = fs::remove_dir_all(&dir);
    }

    // ── legacy migration ──────────────────────────────────────────────────────

    #[test]
    fn mismatched_cwd_returns_none() {
        let dir_a = std::env::temp_dir().join(format!("anveesa_cwd_a_{}", std::process::id()));
        let dir_b = std::env::temp_dir().join(format!("anveesa_cwd_b_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir_a);
        let _ = fs::remove_dir_all(&dir_b);
        fs::create_dir_all(&dir_a).unwrap();
        let path = dir_a.join("session.json");
        let options = AskOptions { provider: None, model: None, system: None, stdin: false, yes: false };
        save_interactive_session(&path, &dir_a, "p", &options, &[]).unwrap();

        // Loading with a different cwd must return None.
        assert!(load_interactive_session(&path, &dir_b).is_none());
        // Loading with the correct cwd must succeed.
        assert!(load_interactive_session(&path, &dir_a).is_some());

        let _ = fs::remove_dir_all(&dir_a);
    }

    // ── purge_stale_sessions ──────────────────────────────────────────────────

    #[test]
    fn purge_removes_old_files_but_keeps_recent_ones() {
        let sessions_base = std::env::temp_dir()
            .join(format!("anveesa_purge_{}", std::process::id()));
        let _ = fs::remove_dir_all(&sessions_base);
        fs::create_dir_all(&sessions_base).unwrap();

        let options = AskOptions { provider: None, model: None, system: None, stdin: false, yes: false };

        // Create two fresh sessions and one stale session.
        let fresh_dir_1 = sessions_base.join("project1");
        let fresh_dir_2 = sessions_base.join("project2");
        let stale_dir = sessions_base.join("old_project");
        fs::create_dir_all(&fresh_dir_1).unwrap();
        fs::create_dir_all(&fresh_dir_2).unwrap();
        fs::create_dir_all(&stale_dir).unwrap();

        let fresh1_path = sessions_base.join("fresh1.json");
        let fresh2_path = sessions_base.join("fresh2.json");
        let stale_path = sessions_base.join("stale.json");

        save_interactive_session(&fresh1_path, &fresh_dir_1, "p", &options, &[]).unwrap();
        save_interactive_session(&fresh2_path, &fresh_dir_2, "p", &options, &[]).unwrap();
        save_interactive_session(&stale_path, &stale_dir, "p", &options, &[]).unwrap();

        // Backdate the stale session.
        let content = fs::read_to_string(&stale_path).unwrap();
        let mut session: InteractiveSession = serde_json::from_str(&content).unwrap();
        session.saved_at = unix_now() - 31 * 24 * 3600;
        fs::write(&stale_path, serde_json::to_string_pretty(&session).unwrap()).unwrap();

        // Manually run purge logic over our temp dir (can't call purge_stale_sessions
        // directly since it targets the real config dir, so we replicate its logic).
        let cutoff = unix_now().saturating_sub(30 * 24 * 3600);
        for entry in fs::read_dir(&sessions_base).unwrap().flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") { continue; }
            let stale = fs::read_to_string(&path)
                .ok()
                .and_then(|c| serde_json::from_str::<InteractiveSession>(&c).ok())
                .map(|s| s.saved_at > 0 && s.saved_at < cutoff)
                .unwrap_or(true);
            if stale { let _ = fs::remove_file(&path); }
        }

        assert!(fresh1_path.exists(), "fresh session 1 must not be purged");
        assert!(fresh2_path.exists(), "fresh session 2 must not be purged");
        assert!(!stale_path.exists(), "stale session must be purged");

        let _ = fs::remove_dir_all(&sessions_base);
    }

    #[test]
    fn purge_removes_unparseable_json_files() {
        let sessions_base = std::env::temp_dir()
            .join(format!("anveesa_purge_bad_{}", std::process::id()));
        let _ = fs::remove_dir_all(&sessions_base);
        fs::create_dir_all(&sessions_base).unwrap();

        let bad_path = sessions_base.join("corrupt.json");
        fs::write(&bad_path, b"not valid json at all {{{").unwrap();

        // Replicate purge logic.
        let cutoff = unix_now().saturating_sub(30 * 24 * 3600);
        for entry in fs::read_dir(&sessions_base).unwrap().flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") { continue; }
            let stale = fs::read_to_string(&path)
                .ok()
                .and_then(|c| serde_json::from_str::<InteractiveSession>(&c).ok())
                .map(|s| s.saved_at > 0 && s.saved_at < cutoff)
                .unwrap_or(true);
            if stale { let _ = fs::remove_file(&path); }
        }

        assert!(!bad_path.exists(), "corrupt session file must be purged");

        let _ = fs::remove_dir_all(&sessions_base);
    }
}
