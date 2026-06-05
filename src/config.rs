use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

pub const SAMPLE_CONFIG: &str = r#"# Anveesa config.
# Path can be overridden with ANVEESA_CONFIG.

default_provider = "sumopod"

[providers.openai]
kind = "openai-compatible"
base_url = "https://api.openai.com/v1"
api_key_env = "OPENAI_API_KEY"

[providers.openrouter]
kind = "openai-compatible"
base_url = "https://openrouter.ai/api/v1"
api_key_env = "OPENROUTER_API_KEY"
# default_model = "openai/gpt-4.1-mini"
# Raise the per-response output cap to reduce truncation on long answers.
# Anveesa continues truncated answers automatically either way.
# max_tokens = 8192

[providers.sumopod]
kind = "openai-compatible"
base_url = "https://ai.sumopod.com/v1"
api_key_env = "SUMOPOD_API_KEY"
# default_model = "your-sumopod-model"

[providers.glm]
kind = "openai-compatible"
base_url = "https://api.z.ai/api/paas/v4"
api_key_env = "ZAI_API_KEY"
# default_model = "glm-5.1"

[providers.glm-coding]
kind = "openai-compatible"
base_url = "https://api.z.ai/api/coding/paas/v4"
api_key_env = "ZAI_API_KEY"

[providers.deepseek]
kind = "openai-compatible"
base_url = "https://api.deepseek.com"
api_key_env = "DEEPSEEK_API_KEY"

[providers.gemini]
kind = "openai-compatible"
base_url = "https://generativelanguage.googleapis.com/v1beta/openai"
api_key_env = "GEMINI_API_KEY"

[providers.github-models]
kind = "openai-compatible"
base_url = "https://models.github.ai/inference"
api_key_env = "GITHUB_TOKEN"

[providers.github-models.headers]
Accept = "application/vnd.github+json"
X-GitHub-Api-Version = "2026-03-10"

[providers.groq]
kind = "openai-compatible"
base_url = "https://api.groq.com/openai/v1"
api_key_env = "GROQ_API_KEY"

[providers.mistral]
kind = "openai-compatible"
base_url = "https://api.mistral.ai/v1"
api_key_env = "MISTRAL_API_KEY"

[providers.xai]
kind = "openai-compatible"
base_url = "https://api.x.ai/v1"
api_key_env = "XAI_API_KEY"

[providers.together]
kind = "openai-compatible"
base_url = "https://api.together.ai/v1"
api_key_env = "TOGETHER_API_KEY"

[providers.fireworks]
kind = "openai-compatible"
base_url = "https://api.fireworks.ai/inference/v1"
api_key_env = "FIREWORKS_API_KEY"

[providers.cerebras]
kind = "openai-compatible"
base_url = "https://api.cerebras.ai/v1"
api_key_env = "CEREBRAS_API_KEY"

[providers.sambanova]
kind = "openai-compatible"
base_url = "https://api.sambanova.ai/v1"
api_key_env = "SAMBANOVA_API_KEY"

[providers.nvidia]
kind = "openai-compatible"
base_url = "https://integrate.api.nvidia.com/v1"
api_key_env = "NVIDIA_API_KEY"

[providers.dashscope]
kind = "openai-compatible"
base_url = "https://dashscope.aliyuncs.com/compatible-mode/v1"
api_key_env = "DASHSCOPE_API_KEY"

[providers.qwen]
kind = "openai-compatible"
base_url = "https://dashscope.aliyuncs.com/compatible-mode/v1"
api_key_env = "DASHSCOPE_API_KEY"

[providers.huggingface]
kind = "openai-compatible"
base_url = "https://router.huggingface.co/v1"
api_key_env = "HF_TOKEN"

[providers.vercel-ai-gateway]
kind = "openai-compatible"
base_url = "https://ai-gateway.vercel.sh/v1"
api_key_env = "AI_GATEWAY_API_KEY"

[providers.perplexity]
kind = "openai-compatible"
base_url = "https://api.perplexity.ai"
api_key_env = "PPLX_API_KEY"

[providers.ollama]
kind = "openai-compatible"
base_url = "http://localhost:11434/v1"

[providers.lm-studio]
kind = "openai-compatible"
base_url = "http://localhost:1234/v1"

[providers.vllm]
kind = "openai-compatible"
base_url = "http://localhost:8000/v1"

[providers.litellm]
kind = "openai-compatible"
base_url = "http://localhost:4000/v1"

[providers.localai]
kind = "openai-compatible"
base_url = "http://localhost:8080/v1"

[providers.claude-code]
kind = "command"
command = "claude"
args = ["-p", "{system_args}", "{model_args}", "{prompt}"]
model_args = ["--model", "{model}"]
system_args = ["--system-prompt", "{system}"]

[providers.codex]
kind = "command"
command = "codex"
args = ["exec", "{model_args}", "{prompt}"]
model_args = ["--model", "{model}"]

[providers.copilot]
kind = "command"
command = "copilot"
args = ["-p", "{prompt}", "{model_args}"]
model_args = ["--model", "{model}"]
"#;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_provider: Option<String>,

    #[serde(default)]
    pub providers: BTreeMap<String, ProviderConfig>,

    /// MCP servers to connect to on startup.
    /// Example config:
    ///   [mcp.filesystem]
    ///   command = "npx"
    ///   args = ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub mcp: BTreeMap<String, McpServerConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

impl AppConfig {
    pub fn built_in() -> Self {
        let mut providers = BTreeMap::new();
        insert_openai_provider(
            &mut providers,
            "openai",
            "https://api.openai.com/v1",
            Some("OPENAI_API_KEY"),
        );
        insert_openai_provider(
            &mut providers,
            "openrouter",
            "https://openrouter.ai/api/v1",
            Some("OPENROUTER_API_KEY"),
        );
        insert_openai_provider(
            &mut providers,
            "sumopod",
            "https://ai.sumopod.com/v1",
            Some("SUMOPOD_API_KEY"),
        );
        insert_openai_provider(
            &mut providers,
            "glm",
            "https://api.z.ai/api/paas/v4",
            Some("ZAI_API_KEY"),
        );
        insert_openai_provider(
            &mut providers,
            "glm-coding",
            "https://api.z.ai/api/coding/paas/v4",
            Some("ZAI_API_KEY"),
        );
        insert_openai_provider(
            &mut providers,
            "deepseek",
            "https://api.deepseek.com",
            Some("DEEPSEEK_API_KEY"),
        );
        insert_openai_provider(
            &mut providers,
            "gemini",
            "https://generativelanguage.googleapis.com/v1beta/openai",
            Some("GEMINI_API_KEY"),
        );
        insert_openai_provider(
            &mut providers,
            "github-models",
            "https://models.github.ai/inference",
            Some("GITHUB_TOKEN"),
        );
        insert_headers(
            &mut providers,
            "github-models",
            [
                ("Accept", "application/vnd.github+json"),
                ("X-GitHub-Api-Version", "2026-03-10"),
            ],
        );
        insert_openai_provider(
            &mut providers,
            "groq",
            "https://api.groq.com/openai/v1",
            Some("GROQ_API_KEY"),
        );
        insert_openai_provider(
            &mut providers,
            "mistral",
            "https://api.mistral.ai/v1",
            Some("MISTRAL_API_KEY"),
        );
        insert_openai_provider(
            &mut providers,
            "xai",
            "https://api.x.ai/v1",
            Some("XAI_API_KEY"),
        );
        insert_openai_provider(
            &mut providers,
            "together",
            "https://api.together.ai/v1",
            Some("TOGETHER_API_KEY"),
        );
        insert_openai_provider(
            &mut providers,
            "fireworks",
            "https://api.fireworks.ai/inference/v1",
            Some("FIREWORKS_API_KEY"),
        );
        insert_openai_provider(
            &mut providers,
            "cerebras",
            "https://api.cerebras.ai/v1",
            Some("CEREBRAS_API_KEY"),
        );
        insert_openai_provider(
            &mut providers,
            "sambanova",
            "https://api.sambanova.ai/v1",
            Some("SAMBANOVA_API_KEY"),
        );
        insert_openai_provider(
            &mut providers,
            "nvidia",
            "https://integrate.api.nvidia.com/v1",
            Some("NVIDIA_API_KEY"),
        );
        insert_openai_provider(
            &mut providers,
            "dashscope",
            "https://dashscope.aliyuncs.com/compatible-mode/v1",
            Some("DASHSCOPE_API_KEY"),
        );
        insert_openai_provider(
            &mut providers,
            "qwen",
            "https://dashscope.aliyuncs.com/compatible-mode/v1",
            Some("DASHSCOPE_API_KEY"),
        );
        insert_openai_provider(
            &mut providers,
            "huggingface",
            "https://router.huggingface.co/v1",
            Some("HF_TOKEN"),
        );
        insert_openai_provider(
            &mut providers,
            "vercel-ai-gateway",
            "https://ai-gateway.vercel.sh/v1",
            Some("AI_GATEWAY_API_KEY"),
        );
        insert_openai_provider(
            &mut providers,
            "perplexity",
            "https://api.perplexity.ai",
            Some("PPLX_API_KEY"),
        );
        insert_openai_provider(&mut providers, "ollama", "http://localhost:11434/v1", None);
        insert_openai_provider(
            &mut providers,
            "lm-studio",
            "http://localhost:1234/v1",
            None,
        );
        insert_openai_provider(&mut providers, "vllm", "http://localhost:8000/v1", None);
        insert_openai_provider(&mut providers, "litellm", "http://localhost:4000/v1", None);
        insert_openai_provider(&mut providers, "localai", "http://localhost:8080/v1", None);
        insert_command_provider(
            &mut providers,
            "claude-code",
            "claude",
            ["-p", "{system_args}", "{model_args}", "{prompt}"],
            ["--model", "{model}"],
            ["--system-prompt", "{system}"],
        );
        insert_command_provider(
            &mut providers,
            "codex",
            "codex",
            ["exec", "{model_args}", "{prompt}"],
            ["--model", "{model}"],
            [],
        );
        insert_command_provider(
            &mut providers,
            "copilot",
            "copilot",
            ["-p", "{prompt}", "{model_args}"],
            ["--model", "{model}"],
            [],
        );

        Self {
            default_provider: Some("sumopod".to_string()),
            providers,
            mcp: BTreeMap::new(),
        }
    }

    pub fn load() -> Result<Self> {
        let path = config_path()?;
        if !path.exists() {
            return Ok(Self::built_in());
        }

        let raw = fs::read_to_string(&path)
            .with_context(|| format!("failed to read config {}", path.display()))?;
        let user_config: AppConfig = toml::from_str(&raw)
            .with_context(|| format!("failed to parse config {}", path.display()))?;

        let mut config = Self::built_in();
        config.merge_user(user_config);
        Ok(config)
    }

    fn merge_user(&mut self, user_config: AppConfig) {
        if user_config.default_provider.is_some() {
            self.default_provider = user_config.default_provider;
        }
        self.providers.extend(user_config.providers);
        self.mcp.extend(user_config.mcp);
    }

    pub fn provider_name<'a>(&'a self, requested: Option<&'a str>) -> Result<&'a str> {
        if let Some(provider) = requested {
            return Ok(provider);
        }

        self.default_provider
            .as_deref()
            .context("no provider passed and no default_provider configured")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum ProviderConfig {
    #[serde(rename = "openai-compatible")]
    OpenAiCompatible(OpenAiCompatibleProviderConfig),
    #[serde(rename = "command")]
    Command(CommandProviderConfig),
}

impl ProviderConfig {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::OpenAiCompatible(_) => "openai-compatible",
            Self::Command(_) => "command",
        }
    }

    pub fn default_model(&self) -> Option<&str> {
        match self {
            Self::OpenAiCompatible(config) => config.default_model.as_deref(),
            Self::Command(config) => config.default_model.as_deref(),
        }
    }

    pub fn set_default_model(&mut self, model: String) {
        match self {
            Self::OpenAiCompatible(config) => config.default_model = Some(model),
            Self::Command(config) => config.default_model = Some(model),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAiCompatibleProviderConfig {
    pub base_url: String,

    /// Inline API key. Prefer `api_key_env` to avoid storing secrets in the config file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,

    /// Lightweight model for read-only tool-reasoning rounds (saves cost).
    /// e.g. "gpt-4o-mini" while default_model = "gpt-4o"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fast_model: Option<String>,

    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,

    /// Enable prompt caching. When true, adds `cache_control` markers to the
    /// last static system message and the last history message so the provider
    /// can cache those prefixes across turns.
    /// For Anthropic models this also sends the `anthropic-beta: prompt-caching-2024-07-31` header.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_cache: Option<bool>,

    /// Upper bound on tokens the model may generate per response. When unset the
    /// provider default applies. Raising this reduces how often long answers are
    /// truncated by the output limit (Anveesa continues truncated answers either way).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandProviderConfig {
    pub command: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,

    #[serde(default)]
    pub args: Vec<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub model_args: Vec<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub system_args: Vec<String>,

    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
}

fn insert_openai_provider(
    providers: &mut BTreeMap<String, ProviderConfig>,
    name: &str,
    base_url: &str,
    api_key_env: Option<&str>,
) {
    providers.insert(
        name.to_string(),
        ProviderConfig::OpenAiCompatible(OpenAiCompatibleProviderConfig {
            base_url: base_url.to_string(),
            api_key: None,
            api_key_env: api_key_env.map(str::to_string),
            default_model: None,
            fast_model: None,
            headers: BTreeMap::new(),
            prompt_cache: None,
            max_tokens: None,
        }),
    );
}

fn insert_headers<const N: usize>(
    providers: &mut BTreeMap<String, ProviderConfig>,
    name: &str,
    headers: [(&str, &str); N],
) {
    let Some(ProviderConfig::OpenAiCompatible(provider)) = providers.get_mut(name) else {
        return;
    };

    provider.headers.extend(
        headers
            .into_iter()
            .map(|(name, value)| (name.to_string(), value.to_string())),
    );
}

fn insert_command_provider<const A: usize, const M: usize, const S: usize>(
    providers: &mut BTreeMap<String, ProviderConfig>,
    name: &str,
    command: &str,
    args: [&str; A],
    model_args: [&str; M],
    system_args: [&str; S],
) {
    providers.insert(
        name.to_string(),
        ProviderConfig::Command(CommandProviderConfig {
            command: command.to_string(),
            default_model: None,
            args: args.into_iter().map(str::to_string).collect(),
            model_args: model_args.into_iter().map(str::to_string).collect(),
            system_args: system_args.into_iter().map(str::to_string).collect(),
            env: BTreeMap::new(),
        }),
    );
}

pub fn config_path() -> Result<PathBuf> {
    if let Some(path) = env::var_os("ANVEESA_CONFIG") {
        return Ok(PathBuf::from(path));
    }

    if let Some(path) = env::var_os("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(path).join("anveesa").join("config.toml"));
    }

    if let Some(home) = env::var_os("HOME") {
        return Ok(PathBuf::from(home)
            .join(".config")
            .join("anveesa")
            .join("config.toml"));
    }

    bail!("cannot resolve config path; set ANVEESA_CONFIG")
}

pub fn init_config(force: bool) -> Result<PathBuf> {
    let path = config_path()?;
    if path.exists() && !force {
        bail!(
            "config already exists at {}; pass --force to overwrite",
            path.display()
        );
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create config directory {}", parent.display()))?;
    }

    fs::write(&path, SAMPLE_CONFIG)
        .with_context(|| format!("failed to write config {}", path.display()))?;
    Ok(path)
}

pub fn set_default_model(provider: Option<&str>, model: String) -> Result<(PathBuf, String)> {
    let path = config_path()?;
    let mut user_config = load_user_config_for_write(&path)?;
    let effective_config = effective_config_from_user(user_config.clone());
    let provider_name = effective_config.provider_name(provider)?.to_string();

    if !user_config.providers.contains_key(&provider_name) {
        let provider_config = effective_config
            .providers
            .get(&provider_name)
            .with_context(|| format!("unknown provider '{provider_name}'"))?
            .clone();
        user_config
            .providers
            .insert(provider_name.clone(), provider_config);
    }

    let provider_config = user_config
        .providers
        .get_mut(&provider_name)
        .with_context(|| format!("unknown provider '{provider_name}'"))?;
    provider_config.set_default_model(model);

    if user_config.default_provider.is_none() {
        user_config.default_provider = Some(provider_name.clone());
    }

    write_user_config(&path, &user_config)?;
    Ok((path, provider_name))
}

pub fn set_default_provider(provider: String) -> Result<PathBuf> {
    let path = config_path()?;
    let mut user_config = load_user_config_for_write(&path)?;
    let effective_config = effective_config_from_user(user_config.clone());

    if !effective_config.providers.contains_key(&provider) {
        bail!("unknown provider '{provider}'");
    }

    user_config.default_provider = Some(provider);
    write_user_config(&path, &user_config)?;
    Ok(path)
}

pub fn print_path(path: &Path) -> String {
    path.display().to_string()
}

fn load_user_config_for_write(path: &Path) -> Result<AppConfig> {
    if !path.exists() {
        return Ok(AppConfig {
            default_provider: Some("sumopod".to_string()),
            providers: BTreeMap::new(),
            mcp: BTreeMap::new(),
        });
    }

    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read config {}", path.display()))?;
    toml::from_str(&raw).with_context(|| format!("failed to parse config {}", path.display()))
}

fn effective_config_from_user(user_config: AppConfig) -> AppConfig {
    let mut config = AppConfig::built_in();
    config.merge_user(user_config);
    config
}

fn write_user_config(path: &Path, config: &AppConfig) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create config directory {}", parent.display()))?;
    }

    let raw = toml::to_string_pretty(config).context("failed to serialize config")?;
    fs::write(path, raw).with_context(|| format!("failed to write config {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_has_core_providers() {
        let config = AppConfig::built_in();
        assert_eq!(config.default_provider.as_deref(), Some("sumopod"));
        assert!(config.providers.contains_key("openai"));
        assert!(config.providers.contains_key("codex"));
        assert!(config.providers.contains_key("claude-code"));
    }

    #[test]
    fn merge_overrides_default_and_adds_providers() {
        let mut config = AppConfig::built_in();
        let user: AppConfig = toml::from_str(
            r#"
default_provider = "myllm"

[providers.myllm]
kind = "openai-compatible"
base_url = "http://localhost:9000/v1"
default_model = "local-7b"
"#,
        )
        .unwrap();

        config.merge_user(user);
        assert_eq!(config.default_provider.as_deref(), Some("myllm"));
        let provider = config.providers.get("myllm").expect("custom provider kept");
        assert_eq!(provider.default_model(), Some("local-7b"));
        // built-ins remain available after merge
        assert!(config.providers.contains_key("openai"));
    }

    #[test]
    fn merge_keeps_existing_default_when_user_omits_it() {
        let mut config = AppConfig::built_in();
        let user: AppConfig = toml::from_str(
            r#"
[providers.extra]
kind = "openai-compatible"
base_url = "http://localhost:1/v1"
"#,
        )
        .unwrap();

        config.merge_user(user);
        assert_eq!(config.default_provider.as_deref(), Some("sumopod"));
        assert!(config.providers.contains_key("extra"));
    }

    #[test]
    fn sample_config_parses() {
        let parsed: Result<AppConfig, _> = toml::from_str(SAMPLE_CONFIG);
        assert!(parsed.is_ok(), "sample config should parse: {parsed:?}");
    }
}
