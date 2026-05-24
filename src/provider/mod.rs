mod command;
mod openai_compatible;

use anyhow::{Result, anyhow};
use tokio::sync::{mpsc::UnboundedSender, oneshot};

use crate::config::{AppConfig, ProviderConfig};

#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
}

impl ChatMessage {
    pub fn user(content: String) -> Self {
        Self {
            role: ChatRole::User,
            content,
        }
    }

    pub fn assistant(content: String) -> Self {
        Self {
            role: ChatRole::Assistant,
            content,
        }
    }
}

#[derive(Debug, Clone)]
pub enum ChatRole {
    User,
    Assistant,
}

#[derive(Debug, Clone)]
pub struct PromptRequest {
    pub prompt: String,
    pub model: Option<String>,
    pub system: Option<String>,
    pub workspace_context: Option<String>,
    pub history: Vec<ChatMessage>,
}

/// How tool calls that modify the system (write/edit/run) should be handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalPolicy {
    /// Never run write/run tools (and do not advertise them).
    Deny,
    /// Ask the user on the terminal before each write/run tool call.
    Prompt,
    /// Run write/run tools without asking.
    Allow,
}

impl ApprovalPolicy {
    /// Whether write/run tools should be advertised to the model at all.
    pub fn allows_write_tools(self) -> bool {
        !matches!(self, ApprovalPolicy::Deny)
    }
}

/// Token accounting reported by a provider, when available.
#[derive(Debug, Clone, Copy, Default)]
pub struct Usage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
}

/// Events streamed from a provider back to the renderer, which owns the terminal.
#[derive(Debug)]
pub enum StreamEvent {
    /// A chunk of assistant text to display as it arrives.
    Token(String),
    /// Final token accounting for the turn.
    Usage(Usage),
    /// A write/run tool needs the user's approval. The renderer prompts on the
    /// terminal and sends the decision back through the reply channel.
    Confirm {
        summary: String,
        reply: oneshot::Sender<bool>,
    },
}

/// What the provider produced for a single turn.
#[derive(Debug, Clone, Default)]
pub struct TurnResult {
    pub text: String,
    pub usage: Option<Usage>,
}

pub async fn ask(
    config: &AppConfig,
    provider_name: &str,
    mut request: PromptRequest,
    policy: ApprovalPolicy,
    events: &UnboundedSender<StreamEvent>,
) -> Result<TurnResult> {
    let provider = config
        .providers
        .get(provider_name)
        .ok_or_else(|| unknown_provider_error(config, provider_name))?;

    if request.model.is_none() {
        request.model = provider.default_model().map(str::to_string);
    }

    match provider {
        ProviderConfig::OpenAiCompatible(provider_config) => {
            openai_compatible::ask(provider_name, provider_config, request, policy, events).await
        }
        ProviderConfig::Command(provider_config) => {
            command::ask(provider_config, request, events).await
        }
    }
}

fn unknown_provider_error(config: &AppConfig, provider_name: &str) -> anyhow::Error {
    let mut names = config.providers.keys().cloned().collect::<Vec<_>>();
    names.sort();
    anyhow!(
        "unknown provider '{}'; available providers: {}",
        provider_name,
        names.join(", ")
    )
}
