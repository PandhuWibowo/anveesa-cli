use std::{
    env,
    ffi::OsString,
    path::{Path, PathBuf},
    process::Stdio,
};

use anyhow::{Context, Result, bail};
use tokio::{io::AsyncWriteExt, process::Command, sync::mpsc::UnboundedSender};

use crate::{
    config::CommandProviderConfig,
    provider::{ChatRole, PromptRequest, StreamEvent, TurnResult},
};

pub async fn ask(
    config: &CommandProviderConfig,
    request: PromptRequest,
    events: &UnboundedSender<StreamEvent>,
) -> Result<TurnResult> {
    let command_prompt = command_prompt(config, &request);
    let prompt_in_args = config.args.iter().any(|arg| arg.contains("{prompt}"));
    let args = build_args(config, &command_prompt, &request);

    let executable = resolve_command(&config.command)?;
    let mut command = Command::new(&executable);
    command
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    for (key, value) in &config.env {
        command.env(key, expand_arg(value, &command_prompt, &request));
    }

    if !prompt_in_args {
        command.stdin(Stdio::piped());
    }

    let mut child = command.spawn().with_context(|| {
        format!(
            "failed to spawn command provider '{}' at {}",
            config.command,
            executable.display()
        )
    })?;

    if !prompt_in_args {
        let mut stdin = child.stdin.take().context("failed to open command stdin")?;
        stdin
            .write_all(command_prompt.as_bytes())
            .await
            .context("failed to write prompt to command stdin")?;
        drop(stdin);
    }

    let output = child
        .wait_with_output()
        .await
        .context("failed to wait for command provider")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "command provider '{}' exited with {}: {}",
            config.command,
            output.status,
            stderr.trim()
        );
    }

    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let _ = events.send(StreamEvent::Token(text.clone()));
    Ok(TurnResult { text, usage: None })
}

fn command_prompt(config: &CommandProviderConfig, request: &PromptRequest) -> String {
    let system_is_native = request.system.is_some() && !config.system_args.is_empty();
    if request.history.is_empty()
        && request.workspace_context.is_none()
        && (request.system.is_none() || system_is_native)
    {
        return request.prompt.clone();
    }

    let mut prompt = String::new();

    if let (Some(system), false) = (&request.system, system_is_native) {
        prompt.push_str("System:\n");
        prompt.push_str(system);
        prompt.push_str("\n\n");
    }

    if let Some(workspace_context) = &request.workspace_context {
        prompt.push_str("System:\n");
        prompt.push_str(workspace_context);
        prompt.push_str("\n\n");
    }

    for message in &request.history {
        match message.role {
            ChatRole::User => prompt.push_str("User:\n"),
            ChatRole::Assistant => prompt.push_str("Assistant:\n"),
        }
        prompt.push_str(&message.content);
        prompt.push_str("\n\n");
    }

    prompt.push_str("User:\n");
    prompt.push_str(&request.prompt);
    prompt
}

fn build_args(
    config: &CommandProviderConfig,
    command_prompt: &str,
    request: &PromptRequest,
) -> Vec<String> {
    let mut args = Vec::new();
    let mut expanded_model_args = false;
    let mut expanded_system_args = false;

    for arg in &config.args {
        match arg.as_str() {
            "{model_args}" => {
                expanded_model_args = true;
                append_optional_args(
                    &mut args,
                    &config.model_args,
                    request.model.is_some(),
                    command_prompt,
                    request,
                );
            }
            "{system_args}" => {
                expanded_system_args = true;
                append_optional_args(
                    &mut args,
                    &config.system_args,
                    request.system.is_some(),
                    command_prompt,
                    request,
                );
            }
            _ => args.push(expand_arg(arg, command_prompt, request)),
        }
    }

    if !expanded_model_args {
        append_optional_args(
            &mut args,
            &config.model_args,
            request.model.is_some(),
            command_prompt,
            request,
        );
    }

    if !expanded_system_args {
        append_optional_args(
            &mut args,
            &config.system_args,
            request.system.is_some(),
            command_prompt,
            request,
        );
    }

    args
}

fn append_optional_args(
    args: &mut Vec<String>,
    templates: &[String],
    include: bool,
    command_prompt: &str,
    request: &PromptRequest,
) {
    if !include {
        return;
    }

    args.extend(
        templates
            .iter()
            .map(|arg| expand_arg(arg, command_prompt, request)),
    );
}

fn expand_arg(value: &str, prompt: &str, request: &PromptRequest) -> String {
    value
        .replace("{prompt}", prompt)
        .replace("{model}", request.model.as_deref().unwrap_or_default())
        .replace("{system}", request.system.as_deref().unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ChatMessage;

    fn config() -> CommandProviderConfig {
        CommandProviderConfig {
            command: "codex".into(),
            default_model: None,
            args: vec!["exec".into(), "{model_args}".into(), "{prompt}".into()],
            model_args: vec!["--model".into(), "{model}".into()],
            system_args: vec![],
            env: Default::default(),
        }
    }

    fn request(prompt: &str) -> PromptRequest {
        PromptRequest {
            prompt: prompt.into(),
            model: None,
            system: None,
            workspace_context: None,
            history: vec![],
            image: None,
        }
    }

    #[test]
    fn plain_prompt_when_no_context() {
        let req = request("hello");
        assert_eq!(command_prompt(&config(), &req), "hello");
    }

    #[test]
    fn prepends_history_and_context() {
        let mut req = request("now what?");
        req.workspace_context = Some("cwd: /tmp".into());
        req.history = vec![
            ChatMessage::user("first".into()),
            ChatMessage::assistant("answer".into()),
        ];
        let prompt = command_prompt(&config(), &req);
        assert!(prompt.contains("System:\ncwd: /tmp"));
        assert!(prompt.contains("User:\nfirst"));
        assert!(prompt.contains("Assistant:\nanswer"));
        assert!(prompt.trim_end().ends_with("User:\nnow what?"));
    }

    #[test]
    fn omits_model_args_without_model() {
        let args = build_args(&config(), "hi", &request("hi"));
        assert_eq!(args, vec!["exec", "hi"]);
    }

    #[test]
    fn expands_model_args_with_model() {
        let mut req = request("hi");
        req.model = Some("gpt-5.1-codex".into());
        let args = build_args(&config(), "hi", &req);
        assert_eq!(args, vec!["exec", "--model", "gpt-5.1-codex", "hi"]);
    }
}

fn resolve_command(command: &str) -> Result<PathBuf> {
    let command_path = Path::new(command);
    if command_path.components().count() > 1 {
        return Ok(command_path.to_path_buf());
    }

    let Some(current_name) = env::current_exe()
        .ok()
        .and_then(|path| path.file_name().map(OsString::from))
    else {
        return Ok(command_path.to_path_buf());
    };

    let command_name = OsString::from(command);
    if current_name != command_name {
        return Ok(command_path.to_path_buf());
    }

    let current_exe = env::current_exe()
        .ok()
        .and_then(|path| path.canonicalize().ok());

    for dir in env::split_paths(&env::var_os("PATH").unwrap_or_default()) {
        let candidate = dir.join(command);
        if !candidate.is_file() {
            continue;
        }

        let canonical_candidate = candidate.canonicalize().ok();
        if current_exe.is_some() && canonical_candidate == current_exe {
            continue;
        }

        return Ok(candidate);
    }

    bail!(
        "command provider '{}' resolves to this Anveesa alias; set providers.{}.command to the real executable path",
        command,
        command
    )
}
