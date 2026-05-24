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
use rustyline::{DefaultEditor, error::ReadlineError};
use tokio::sync::mpsc;

use crate::{
    cli::{AskOptions, Cli, Command, ConfigCommand},
    config::{
        AppConfig, ProviderConfig, config_path, init_config, print_path, set_default_model,
        set_default_provider,
    },
    provider::{ApprovalPolicy, ChatMessage, PromptRequest, StreamEvent, TurnResult, Usage},
};

#[derive(Debug, Clone, Copy)]
enum RenderMode {
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
    let workspace_context = workspace_context().ok();
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

    let mut history: Vec<ChatMessage> = Vec::new();
    let mut editor =
        DefaultEditor::new().context("failed to start interactive editor")?;
    let history_path = repl_history_path();
    if let Some(path) = &history_path {
        let _ = editor.load_history(path);
    }

    loop {
        print_banner(
            &provider_name,
            session_options.model.as_deref().unwrap_or("-"),
            history.len() / 2,
            workspace_context.is_some(),
            tools_available,
            policy,
        );

        let line = match editor.readline("> ") {
            Ok(line) => line,
            Err(ReadlineError::Interrupted) => continue,
            Err(ReadlineError::Eof) => {
                println!();
                break;
            }
            Err(error) => return Err(error).context("failed to read interactive prompt"),
        };

        let prompt = line.trim().to_string();
        match prompt.as_str() {
            "" => continue,
            "/exit" | "/quit" | ":q" => break,
            "/clear" => {
                history.clear();
                println!("context cleared");
                continue;
            }
            _ => {}
        }
        let _ = editor.add_history_entry(prompt.as_str());

        match ask_streaming(
            &config,
            &session_options,
            prompt.clone(),
            &history,
            workspace_context.as_deref(),
            policy,
            RenderMode::Interactive,
        )
        .await
        {
            Ok(result) => {
                println!();
                history.push(ChatMessage::user(prompt));
                history.push(ChatMessage::assistant(result.text));
            }
            Err(error) => {
                eprintln!("error: {error:#}");
                println!();
            }
        }
    }

    if let Some(path) = &history_path {
        let _ = editor.save_history(path);
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
    let policy = if options.yes {
        ApprovalPolicy::Allow
    } else {
        ApprovalPolicy::Deny
    };

    ask_streaming(
        &config,
        &options,
        prompt,
        &[],
        workspace_context.as_deref(),
        policy,
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
    let frames = ['-', '\\', '|', '/'];
    let mut frame = 0usize;
    let mut first_token = true;
    let mut produced = false;
    let mut usage: Option<Usage> = None;

    loop {
        tokio::select! {
            maybe = rx.recv() => match maybe {
                Some(StreamEvent::Token(text)) => {
                    if first_token {
                        clear_spinner(spinner);
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
                Some(StreamEvent::Confirm { summary, reply }) => {
                    clear_spinner(spinner);
                    let approved = tokio::task::block_in_place(|| prompt_yes_no(&summary));
                    let _ = reply.send(approved);
                }
                None => break,
            },
            _ = tokio::time::sleep(Duration::from_millis(120)), if first_token && spinner => {
                eprint!(
                    "\r{} thinking... {:.0}s",
                    frames[frame % frames.len()],
                    started.elapsed().as_secs_f32()
                );
                let _ = io::stderr().flush();
                frame += 1;
            }
        }
    }

    if produced {
        println!();
    } else {
        clear_spinner(spinner);
    }

    if spinner
        && let Some(usage) = usage
        && usage.total_tokens > 0
    {
        eprintln!(
            "[tokens: {} in / {} out / {} total]",
            usage.prompt_tokens, usage.completion_tokens, usage.total_tokens
        );
    }
}

fn print_assistant_header(started: Instant) {
    println!();
    println!("assistant ({:.1}s)", started.elapsed().as_secs_f32());
    println!("{}", "-".repeat(16));
}

fn clear_spinner(enabled: bool) {
    if enabled {
        eprint!("\r{}\r", " ".repeat(60));
        let _ = io::stderr().flush();
    }
}

fn prompt_yes_no(summary: &str) -> bool {
    let mut err = io::stderr();
    let _ = write!(err, "allow {summary}? [y/N] ");
    let _ = err.flush();

    let mut answer = String::new();
    if io::stdin().read_line(&mut answer).is_err() {
        return false;
    }
    matches!(answer.trim().to_lowercase().as_str(), "y" | "yes")
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

fn print_banner(
    provider: &str,
    model: &str,
    turns: usize,
    has_workspace_context: bool,
    tools_available: bool,
    policy: ApprovalPolicy,
) {
    let title = format!("anveesa | provider: {provider} | model: {model}");
    let context = if has_workspace_context {
        "ctx:on"
    } else {
        "ctx:off"
    };
    let tools = if tools_available {
        "tools:on"
    } else {
        "tools:off"
    };
    let writes = if !tools_available {
        "writes:n/a"
    } else {
        match policy {
            ApprovalPolicy::Allow => "writes:auto",
            ApprovalPolicy::Prompt => "writes:ask",
            ApprovalPolicy::Deny => "writes:off",
        }
    };
    let hint = format!("turns: {turns} | {context} | {tools} | {writes} | /clear /exit");
    let width = title.len().max(hint.len()).max(28);
    let border = format!("+{}+", "-".repeat(width + 2));

    println!("{border}");
    println!("| {title:width$} |");
    println!("| {hint:width$} |");
    println!("{border}");
}

fn repl_history_path() -> Option<PathBuf> {
    let path = config_path().ok()?;
    let dir = path.parent()?;
    let _ = fs::create_dir_all(dir);
    Some(dir.join("history"))
}

fn workspace_context() -> Result<String> {
    let cwd = std::env::current_dir().context("failed to resolve current directory")?;
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

    let entries = directory_entries(&cwd)?;
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
}
