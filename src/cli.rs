use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "anveesa",
    version,
    about = "Terminal AI wrapper for multiple providers"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    #[command(flatten)]
    pub ask_options: AskOptions,

    #[arg(value_name = "PROMPT", trailing_var_arg = true)]
    pub prompt: Vec<String>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Ask the configured AI provider.
    Ask(AskArgs),
    /// List available providers.
    Providers,
    /// Manage the Anveesa config file.
    Config(ConfigArgs),
    /// Manage saved interactive sessions.
    Sessions(SessionsArgs),
}

#[derive(Debug, Args)]
pub struct SessionsArgs {
    #[command(subcommand)]
    pub command: SessionsCommand,
}

#[derive(Debug, Subcommand)]
pub enum SessionsCommand {
    /// List all saved sessions.
    List,
    /// Delete sessions. Without flags, deletes the session for the current directory.
    Clear {
        /// Delete all saved sessions.
        #[arg(long)]
        all: bool,
    },
}

#[derive(Debug, Args)]
pub struct AskArgs {
    #[command(flatten)]
    pub options: AskOptions,

    #[arg(value_name = "PROMPT", trailing_var_arg = true)]
    pub prompt: Vec<String>,
}

#[derive(Debug, Args, Clone, Default)]
pub struct AskOptions {
    /// Provider name from the config.
    #[arg(short, long)]
    pub provider: Option<String>,

    /// Model name to send to the provider.
    #[arg(short, long)]
    pub model: Option<String>,

    /// Optional system instruction.
    #[arg(long)]
    pub system: Option<String>,

    /// Append stdin to the prompt.
    #[arg(long)]
    pub stdin: bool,

    /// Auto-approve file writes and command execution without prompting.
    #[arg(short = 'y', long)]
    pub yes: bool,
}

#[derive(Debug, Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub command: ConfigCommand,
}

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Create a starter config at the default path.
    Init {
        /// Overwrite an existing config file.
        #[arg(long)]
        force: bool,
    },
    /// Set the default model for a provider.
    SetModel {
        /// Provider name from the config. Defaults to default_provider.
        #[arg(short, long)]
        provider: Option<String>,

        /// Model name to use by default.
        model: String,
    },
    /// Set the default provider.
    SetProvider {
        /// Provider name from the config.
        provider: String,
    },
    /// Print the config path.
    Path,
    /// Print the effective config.
    Show,
}
