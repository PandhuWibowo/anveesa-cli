pub mod cli;
pub mod config;
pub mod display;
pub mod image;
pub mod mcp;
pub mod prompt;
pub mod provider;
pub mod session;
pub mod tools;
pub mod tui;
pub mod web;
pub mod workspace;

use std::{
    fs,
    io::{self, IsTerminal},
    time::Instant,
};

use anyhow::{Context, Result};
use clap::{CommandFactory, Parser};
use tokio::sync::mpsc;

use crate::{
    cli::{AskOptions, Cli, Command, ConfigCommand, SessionsCommand},
    config::{
        AppConfig, ProviderConfig, config_path, init_config, print_path, set_default_model,
        set_default_provider,
    },
    display::{
        print_help_inline, print_input_separator, print_session_header, print_session_info,
        print_status_inline, prompt_label, render_stream, term_width,
    },
    image::{attach_image, grab_clipboard_image, image_fingerprint, parse_attach_command},
    prompt::{PromptRead, read_prompt_line},
    provider::{
        ApprovalPolicy, ChatMessage, ChatRole, ImageAttachment, PromptRequest, TurnResult, Usage,
    },
    session::{
        append_repl_history, legacy_session_path, load_interactive_session, purge_stale_sessions,
        repl_history_path, repl_session_path, save_interactive_session,
    },
    workspace::workspace_context_for,
};

#[derive(Debug, Clone, Copy)]
pub enum RenderMode {
    Interactive,
    OneShot,
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
        Some(Command::Web(args)) => web::run_web(args.options, args.port).await,
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
    let mut history = loaded_session
        .as_ref()
        .map(|s| s.messages.clone())
        .unwrap_or_default();
    // saved_at at load time — used only for the startup header so it shows when the previous
    // run ended, not the current run's save time.
    let session_saved_at = loaded_session
        .as_ref()
        .filter(|s| s.saved_at > 0)
        .map(|s| s.saved_at);
    // tracks the most recent successful save this run — kept fresh for /session display
    let mut last_saved_at: u64 = session_saved_at.unwrap_or(0);
    // Per-project config: .anveesa.toml (extended) or .anveesa (plain system prompt)
    if let Ok(raw) = fs::read_to_string(cwd.join(".anveesa.toml")) {
        if let Ok(cfg) = toml::from_str::<toml::Value>(&raw) {
            if session_options.system.is_none()
                && let Some(sp) = cfg.get("system_prompt").and_then(|v| v.as_str())
            {
                session_options.system = Some(sp.trim().to_string());
            }
            // Override model if not set by CLI
            if session_options.model.is_none()
                && let Some(m) = cfg.get("model").and_then(|v| v.as_str())
            {
                session_options.model = Some(m.to_string());
            }
            // auto_approve
            if let Some(true) = cfg.get("auto_approve").and_then(|v| v.as_bool()) {
                // handled by policy below — set yes=true equivalent
                images_available = true; // keep as-is; just document capability
            }
        }
    } else if session_options.system.is_none()
        && let Ok(text) = fs::read_to_string(cwd.join(".anveesa"))
    {
        let trimmed = text.trim().to_string();
        if !trimmed.is_empty() {
            session_options.system = Some(trimmed);
        }
    }

    let history_path = repl_history_path();
    // Load prompt history for ↑/↓ recall (one entry per line, newest at end).
    let input_history: Vec<String> = history_path
        .as_deref()
        .and_then(|p| fs::read_to_string(p).ok())
        .map(|c| {
            c.lines()
                .filter(|l| !l.is_empty())
                .map(String::from)
                .collect()
        })
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
            while let Ok(ev) = crossterm::event::read() {
                if key_tx.send(ev).is_err() {
                    break;
                }
            }
        });

        let short_cwd = std::env::var("HOME")
            .map(|h| cwd.display().to_string().replacen(&h, "~", 1))
            .unwrap_or_else(|_| cwd.display().to_string());

        let app = tui::App::new(
            provider_name.clone(),
            session_options
                .model
                .clone()
                .unwrap_or_else(|| "-".to_string()),
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
        let (line, ctrl_v_image) = match read_prompt_line(
            &label,
            width,
            &mut paste_count,
            images_available,
            &input_history,
        ) {
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
                    let current = session_options
                        .model
                        .as_deref()
                        .unwrap_or("(provider default)");
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
                        println!(
                            "\x1b[2m  provider: {provider_name}  model: {}\x1b[0m",
                            session_options.model.as_deref().unwrap_or("(default)")
                        );
                    } else {
                        println!("provider: {provider_name}");
                    }
                } else if !config.providers.contains_key(arg) {
                    if is_tty {
                        eprintln!(
                            "\x1b[1;31m✗\x1b[0m unknown provider '{arg}' — run: anveesa providers"
                        );
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
                        println!(
                            "\x1b[2m  Switched to provider: {arg}  model: {model_display}\x1b[0m"
                        );
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
                    eprintln!(
                        "\x1b[1;31m✗\x1b[0m image attachments require an openai-compatible provider"
                    );
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
                if let Some(path) = &session_path
                    && save_interactive_session(
                        path,
                        &cwd,
                        &provider_name,
                        &session_options,
                        &history,
                    )
                    .is_ok()
                {
                    last_saved_at = unix_now();
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
                if let Some(path) = &session_path
                    && save_interactive_session(
                        path,
                        &cwd,
                        &provider_name,
                        &session_options,
                        &history,
                    )
                    .is_ok()
                {
                    last_saved_at = unix_now();
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
                    let _ = save_interactive_session(
                        path,
                        &cwd,
                        &provider_name,
                        &session_options,
                        &history,
                    );
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
        SessionsCommand::List => session::list_sessions(),
        SessionsCommand::Clear { all } => session::clear_sessions(all),
    }
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
    let workspace_context = workspace::workspace_context().ok();
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

#[allow(clippy::too_many_arguments)]
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

/// Export a conversation history as markdown to the given path.
///
/// # Examples
///
/// ```
/// use anveesa::export_conversation;
/// use anveesa::provider::{ChatMessage, ChatRole};
/// use std::path::Path;
///
/// let history = vec![
///     ChatMessage { role: ChatRole::User, content: "hello".into() },
///     ChatMessage { role: ChatRole::Assistant, content: "hi".into() },
/// ];
/// let path = Path::new("/tmp/anveesa-export-test.md");
/// export_conversation(path, &history).ok();
/// ```
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
    fs::write(path, out.trim_end()).with_context(|| format!("failed to write {}", path.display()))
}

fn list_providers() -> Result<()> {
    let config = AppConfig::load()?;
    let is_tty = io::stdout().is_terminal();

    if !is_tty {
        for (name, provider) in &config.providers {
            let is_default = config.default_provider.as_deref() == Some(name.as_str());
            let model = provider.default_model().unwrap_or("-");
            println!(
                "{}  {name}  {model}  {}",
                if is_default { "*" } else { " " },
                provider.kind()
            );
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
    use std::io::Read;
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
        anyhow::bail!("prompt is empty; pass text arguments or pipe input with --stdin")
    }

    Ok(prompt)
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

/// Return the current Unix timestamp in seconds.
pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
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
}
