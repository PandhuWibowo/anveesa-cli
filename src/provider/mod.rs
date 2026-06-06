mod command;
pub mod openai_compatible;

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc::UnboundedSender, oneshot};

use crate::config::{AppConfig, ProviderConfig};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ChatRole {
    User,
    Assistant,
}

/// A base64-encoded image attached to the current user turn.
#[derive(Debug, Clone)]
pub struct ImageAttachment {
    pub mime: String, // e.g. "image/png"
    pub data: String, // base64-encoded bytes
}

#[derive(Debug, Clone)]
pub struct PromptRequest {
    pub prompt: String,
    pub model: Option<String>,
    pub system: Option<String>,
    pub workspace_context: Option<String>,
    pub history: Vec<ChatMessage>,
    /// Images attached to the current turn (clipboard paste or explicit attach).
    pub images: Vec<ImageAttachment>,
    /// Connected MCP servers (runtime only, not part of session history).
    pub mcp: Option<std::sync::Arc<crate::mcp::McpManager>>,
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

/// User decision for a write/run tool confirmation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    Deny,
    AllowOnce,
    AllowForTurn,
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
    /// Tokens served from the prompt cache (Anthropic: cache_read_input_tokens; OpenAI: cached_tokens).
    pub cache_read_tokens: u64,
    /// Tokens written into the prompt cache this turn (Anthropic: cache_creation_input_tokens).
    pub cache_write_tokens: u64,
}

/// What to show in the approval dialog before running a write/run tool.
#[derive(Debug)]
pub enum ToolConfirmPreview {
    /// A file write or edit — diff lines already computed from the arguments.
    FileOp {
        verb: String,
        path: String,
        added: usize,
        removed: usize,
        diff: Vec<DiffLine>,
        truncated: bool,
    },
    /// A directory that will be created.
    CreateDir { path: String },
    /// Any other write/run tool — show a plain-text description.
    Generic { summary: String },
}

/// Events streamed from a provider back to the renderer, which owns the terminal.
#[derive(Debug)]
pub enum StreamEvent {
    /// Durable progress/status message for long waits between model/tool phases.
    Status { message: String },
    /// A chunk of assistant text to display as it arrives.
    Token(String),
    /// Final token accounting for the turn.
    Usage(Usage),
    /// A read-only tool is running. Used to make multi-round inspection visible.
    ToolCall { summary: String },
    /// A tool finished running. Used to show explicit success/failure after approval.
    ToolResult {
        summary: String,
        ok: bool,
        elapsed_ms: u128,
        error: Option<String>,
    },
    /// A write/run tool needs the user's approval. The renderer shows the
    /// preview, prompts for a decision, and sends it back through the reply channel.
    Confirm {
        preview: ToolConfirmPreview,
        reply: oneshot::Sender<ApprovalDecision>,
    },
    /// A file was created or edited — show a diff-style summary.
    FileOp {
        verb: String,
        path: String,
        added: usize,
        removed: usize,
        preview: Vec<DiffLine>,
        truncated: bool,
    },
    /// The model announced a multi-step plan.
    PlanSet { tasks: Vec<String> },
    /// The model marked one plan step as complete.
    PlanTaskDone { index: usize },
}

#[derive(Debug)]
pub enum DiffKind {
    Add,
    Remove,
}

#[derive(Debug)]
pub struct DiffLine {
    pub kind: DiffKind,
    pub line_no: usize,
    pub text: String,
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
