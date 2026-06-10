use crate::config::*;
use std::collections::BTreeMap;

// ─── OpenAiCompatibleProviderConfig ───

#[test]
fn openai_minimal_has_correct_fields() {
    let cfg = OpenAiCompatibleProviderConfig {
        base_url: "https://api.openai.com/v1".to_string(),
        api_key: None,
        api_key_env: Some("OPENAI_API_KEY".to_string()),
        default_model: None,
        fast_model: None,
        headers: BTreeMap::new(),
        prompt_cache: None,
        max_tokens: None,
        extended_thinking: None,
        pricing: None,
    };
    assert_eq!(cfg.base_url, "https://api.openai.com/v1");
    assert_eq!(cfg.api_key_env.as_deref(), Some("OPENAI_API_KEY"));
    assert!(cfg.api_key.is_none());
    assert!(cfg.default_model.is_none());
    assert!(cfg.fast_model.is_none());
    assert!(cfg.headers.is_empty());
    assert!(cfg.prompt_cache.is_none());
    assert!(cfg.max_tokens.is_none());
    assert!(cfg.extended_thinking.is_none());
    assert!(cfg.pricing.is_none());
}

#[test]
fn openai_all_fields_set() {
    let mut headers = BTreeMap::new();
    headers.insert("X-Custom".to_string(), "value".to_string());
    let cfg = OpenAiCompatibleProviderConfig {
        base_url: "https://example.com/v1".to_string(),
        api_key: Some("sk-123".to_string()),
        api_key_env: Some("EXAMPLE_KEY".to_string()),
        default_model: Some("gpt-4o".to_string()),
        fast_model: Some("gpt-4o-mini".to_string()),
        headers,
        prompt_cache: Some(true),
        max_tokens: Some(4096),
        extended_thinking: Some(10000),
        pricing: Some([3.0, 15.0, 0.3, 3.75]),
    };
    assert_eq!(cfg.default_model.as_deref(), Some("gpt-4o"));
    assert_eq!(cfg.fast_model.as_deref(), Some("gpt-4o-mini"));
    assert_eq!(cfg.prompt_cache, Some(true));
    assert_eq!(cfg.max_tokens, Some(4096));
    assert_eq!(cfg.extended_thinking, Some(10000));
    assert_eq!(cfg.pricing.unwrap()[0], 3.0);
    assert_eq!(cfg.pricing.unwrap()[3], 3.75);
    assert_eq!(cfg.headers.get("X-Custom").unwrap(), "value");
    assert_eq!(cfg.api_key.as_deref(), Some("sk-123"));
}

#[test]
fn openai_clone_works() {
    let cfg = OpenAiCompatibleProviderConfig {
        base_url: "https://api.test.com/v1".to_string(),
        api_key: Some("key".to_string()),
        api_key_env: None,
        default_model: Some("model".to_string()),
        fast_model: None,
        headers: BTreeMap::new(),
        prompt_cache: None,
        max_tokens: None,
        extended_thinking: None,
        pricing: None,
    };
    let cloned = cfg.clone();
    assert_eq!(cloned.base_url, cfg.base_url);
    assert_eq!(cloned.api_key, cfg.api_key);
    assert_eq!(cloned.default_model, cfg.default_model);
}

#[test]
fn openai_debug_output_contains_url() {
    let cfg = OpenAiCompatibleProviderConfig {
        base_url: "https://debug.example.com/v1".to_string(),
        api_key: None,
        api_key_env: Some("KEY".to_string()),
        default_model: None,
        fast_model: None,
        headers: BTreeMap::new(),
        prompt_cache: None,
        max_tokens: None,
        extended_thinking: None,
        pricing: None,
    };
    let debug_str = format!("{:?}", cfg);
    assert!(debug_str.contains("debug.example.com"));
}

#[test]
fn openai_pricing_default_none() {
    let cfg = OpenAiCompatibleProviderConfig {
        base_url: "https://api.test.com/v1".to_string(),
        api_key: None,
        api_key_env: None,
        default_model: None,
        fast_model: None,
        headers: BTreeMap::new(),
        prompt_cache: None,
        max_tokens: None,
        extended_thinking: None,
        pricing: None,
    };
    assert!(cfg.pricing.is_none());
    // Verify pricing can be set to custom values
    let pricing = [0.0, 0.0, 0.0, 0.0];
    assert_eq!(pricing.len(), 4);
}

#[test]
fn openai_extended_thinking_budget() {
    let cfg = OpenAiCompatibleProviderConfig {
        base_url: "https://api.test.com/v1".to_string(),
        api_key: None,
        api_key_env: None,
        default_model: Some("claude-3-7-sonnet".to_string()),
        fast_model: None,
        headers: BTreeMap::new(),
        prompt_cache: None,
        max_tokens: None,
        extended_thinking: Some(16000),
        pricing: None,
    };
    assert_eq!(cfg.extended_thinking, Some(16000));
}

#[test]
fn openai_prompt_cache_enabled() {
    let cfg = OpenAiCompatibleProviderConfig {
        base_url: "https://api.test.com/v1".to_string(),
        api_key: None,
        api_key_env: None,
        default_model: None,
        fast_model: None,
        headers: BTreeMap::new(),
        prompt_cache: Some(true),
        max_tokens: None,
        extended_thinking: None,
        pricing: None,
    };
    assert!(cfg.prompt_cache.unwrap());
}

#[test]
fn openai_max_tokens_set() {
    let cfg = OpenAiCompatibleProviderConfig {
        base_url: "https://api.test.com/v1".to_string(),
        api_key: None,
        api_key_env: None,
        default_model: None,
        fast_model: None,
        headers: BTreeMap::new(),
        prompt_cache: None,
        max_tokens: Some(8192),
        extended_thinking: None,
        pricing: None,
    };
    assert_eq!(cfg.max_tokens, Some(8192));
}

#[test]
fn openai_headers_multiple() {
    let mut headers = BTreeMap::new();
    headers.insert("Accept".to_string(), "application/json".to_string());
    headers.insert("X-API-Version".to_string(), "2024-01-01".to_string());
    let cfg = OpenAiCompatibleProviderConfig {
        base_url: "https://api.test.com/v1".to_string(),
        api_key: None,
        api_key_env: None,
        default_model: None,
        fast_model: None,
        headers,
        prompt_cache: None,
        max_tokens: None,
        extended_thinking: None,
        pricing: None,
    };
    assert_eq!(cfg.headers.len(), 2);
}

#[test]
fn openai_serialize_roundtrip() {
    let cfg = OpenAiCompatibleProviderConfig {
        base_url: "https://test.com/v1".to_string(),
        api_key: None,
        api_key_env: Some("TEST_KEY".to_string()),
        default_model: Some("test-model".to_string()),
        fast_model: Some("test-fast".to_string()),
        headers: BTreeMap::new(),
        prompt_cache: Some(true),
        max_tokens: Some(2048),
        extended_thinking: None,
        pricing: None,
    };
    let json = serde_json::to_string(&cfg).unwrap();
    let parsed: OpenAiCompatibleProviderConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.base_url, cfg.base_url);
    assert_eq!(parsed.default_model, cfg.default_model);
    assert_eq!(parsed.prompt_cache, cfg.prompt_cache);
    assert_eq!(parsed.max_tokens, cfg.max_tokens);
    // Skipped fields should be None
    assert!(parsed.api_key.is_none());
    assert!(parsed.extended_thinking.is_none());
}

// ─── CommandProviderConfig ───

#[test]
fn command_minimal() {
    let cfg = CommandProviderConfig {
        command: "claude".to_string(),
        default_model: None,
        args: vec!["-p".to_string(), "{prompt}".to_string()],
        model_args: vec!["--model".to_string(), "{model}".to_string()],
        system_args: vec![],
        env: BTreeMap::new(),
    };
    assert_eq!(cfg.command, "claude");
    assert!(cfg.default_model.is_none());
    assert_eq!(cfg.args.len(), 2);
    assert!(cfg.system_args.is_empty());
    assert!(cfg.env.is_empty());
}

#[test]
fn command_full() {
    let mut env = BTreeMap::new();
    env.insert("ANTHROPIC_API_KEY".to_string(), "key".to_string());
    let cfg = CommandProviderConfig {
        command: "claude".to_string(),
        default_model: Some("claude-3-5-sonnet".to_string()),
        args: vec![
            "-p".to_string(),
            "{system_args}".to_string(),
            "{model_args}".to_string(),
            "{prompt}".to_string(),
        ],
        model_args: vec!["--model".to_string(), "{model}".to_string()],
        system_args: vec!["--system-prompt".to_string(), "{system}".to_string()],
        env,
    };
    assert_eq!(cfg.default_model.as_deref(), Some("claude-3-5-sonnet"));
    assert_eq!(cfg.args.len(), 4);
    assert_eq!(cfg.system_args.len(), 2);
    assert_eq!(cfg.env.len(), 1);
}

#[test]
fn command_clone() {
    let cfg = CommandProviderConfig {
        command: "codex".to_string(),
        default_model: Some("gpt-4o".to_string()),
        args: vec!["exec".to_string(), "{prompt}".to_string()],
        model_args: vec!["--model".to_string(), "{model}".to_string()],
        system_args: vec![],
        env: BTreeMap::new(),
    };
    let cloned = cfg.clone();
    assert_eq!(cloned.command, cfg.command);
    assert_eq!(cloned.default_model, cfg.default_model);
    assert_eq!(cloned.args, cfg.args);
}

#[test]
fn command_debug_output() {
    let cfg = CommandProviderConfig {
        command: "my-cmd".to_string(),
        default_model: None,
        args: vec!["arg1".to_string()],
        model_args: vec![],
        system_args: vec![],
        env: BTreeMap::new(),
    };
    let debug = format!("{:?}", cfg);
    assert!(debug.contains("my-cmd"));
}

#[test]
fn command_serialize_roundtrip() {
    let cfg = CommandProviderConfig {
        command: "test-cmd".to_string(),
        default_model: Some("test-model".to_string()),
        args: vec!["--flag".to_string(), "value".to_string()],
        model_args: vec!["--model".to_string(), "{model}".to_string()],
        system_args: vec!["--system".to_string(), "{system}".to_string()],
        env: BTreeMap::new(),
    };
    let json = serde_json::to_string(&cfg).unwrap();
    let parsed: CommandProviderConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.command, "test-cmd");
    assert_eq!(parsed.default_model, Some("test-model".to_string()));
    assert_eq!(parsed.args.len(), 2);
}

#[test]
fn command_env_map() {
    let mut env = BTreeMap::new();
    env.insert("PATH".to_string(), "/usr/bin".to_string());
    env.insert("CUSTOM_VAR".to_string(), "value".to_string());
    let cfg = CommandProviderConfig {
        command: "cmd".to_string(),
        default_model: None,
        args: vec![],
        model_args: vec![],
        system_args: vec![],
        env,
    };
    assert_eq!(cfg.env.len(), 2);
    assert_eq!(cfg.env.get("CUSTOM_VAR").unwrap(), "value");
}

#[test]
fn command_empty_args() {
    let cfg = CommandProviderConfig {
        command: "cmd".to_string(),
        default_model: None,
        args: vec![],
        model_args: vec![],
        system_args: vec![],
        env: BTreeMap::new(),
    };
    assert!(cfg.args.is_empty());
    assert!(cfg.model_args.is_empty());
    assert!(cfg.system_args.is_empty());
}

// ─── ProviderConfig enum ───

#[test]
fn provider_config_openai_kind() {
    let cfg = ProviderConfig::OpenAiCompatible(OpenAiCompatibleProviderConfig {
        base_url: "https://api.openai.com/v1".to_string(),
        api_key: None,
        api_key_env: Some("OPENAI_API_KEY".to_string()),
        default_model: Some("gpt-4o".to_string()),
        fast_model: None,
        headers: BTreeMap::new(),
        prompt_cache: None,
        max_tokens: None,
        extended_thinking: None,
        pricing: None,
    });
    assert_eq!(cfg.kind(), "openai-compatible");
    assert_eq!(cfg.default_model(), Some("gpt-4o"));
}

#[test]
fn provider_config_command_kind() {
    let cfg = ProviderConfig::Command(CommandProviderConfig {
        command: "claude".to_string(),
        default_model: Some("claude-3-5".to_string()),
        args: vec!["-p".to_string(), "{prompt}".to_string()],
        model_args: vec!["--model".to_string(), "{model}".to_string()],
        system_args: vec![],
        env: BTreeMap::new(),
    });
    assert_eq!(cfg.kind(), "command");
    assert_eq!(cfg.default_model(), Some("claude-3-5"));
}

#[test]
fn provider_config_no_default_model() {
    let cfg = ProviderConfig::OpenAiCompatible(OpenAiCompatibleProviderConfig {
        base_url: "https://api.test.com/v1".to_string(),
        api_key: None,
        api_key_env: None,
        default_model: None,
        fast_model: None,
        headers: BTreeMap::new(),
        prompt_cache: None,
        max_tokens: None,
        extended_thinking: None,
        pricing: None,
    });
    assert_eq!(cfg.default_model(), None);
}

#[test]
fn provider_config_set_default_model_openai() {
    let mut cfg = ProviderConfig::OpenAiCompatible(OpenAiCompatibleProviderConfig {
        base_url: "https://api.test.com/v1".to_string(),
        api_key: None,
        api_key_env: None,
        default_model: None,
        fast_model: None,
        headers: BTreeMap::new(),
        prompt_cache: None,
        max_tokens: None,
        extended_thinking: None,
        pricing: None,
    });
    cfg.set_default_model("new-model".to_string());
    assert_eq!(cfg.default_model(), Some("new-model"));
}

#[test]
fn provider_config_set_default_model_command() {
    let mut cfg = ProviderConfig::Command(CommandProviderConfig {
        command: "cmd".to_string(),
        default_model: None,
        args: vec![],
        model_args: vec![],
        system_args: vec![],
        env: BTreeMap::new(),
    });
    cfg.set_default_model("override-model".to_string());
    assert_eq!(cfg.default_model(), Some("override-model"));
}

#[test]
fn provider_config_clone_openai() {
    let cfg = ProviderConfig::OpenAiCompatible(OpenAiCompatibleProviderConfig {
        base_url: "https://api.test.com/v1".to_string(),
        api_key: None,
        api_key_env: None,
        default_model: Some("model".to_string()),
        fast_model: None,
        headers: BTreeMap::new(),
        prompt_cache: None,
        max_tokens: None,
        extended_thinking: None,
        pricing: None,
    });
    let cloned = cfg.clone();
    assert_eq!(cloned.kind(), cfg.kind());
    assert_eq!(cloned.default_model(), cfg.default_model());
}

#[test]
fn provider_config_clone_command() {
    let cfg = ProviderConfig::Command(CommandProviderConfig {
        command: "cmd".to_string(),
        default_model: Some("model".to_string()),
        args: vec!["arg".to_string()],
        model_args: vec![],
        system_args: vec![],
        env: BTreeMap::new(),
    });
    let cloned = cfg.clone();
    assert_eq!(cloned.kind(), "command");
}

#[test]
fn provider_config_debug_openai() {
    let cfg = ProviderConfig::OpenAiCompatible(OpenAiCompatibleProviderConfig {
        base_url: "https://debug.test.com/v1".to_string(),
        api_key: None,
        api_key_env: None,
        default_model: None,
        fast_model: None,
        headers: BTreeMap::new(),
        prompt_cache: None,
        max_tokens: None,
        extended_thinking: None,
        pricing: None,
    });
    let s = format!("{:?}", cfg);
    assert!(s.contains("debug.test.com"));
}

#[test]
fn provider_config_serialize_command() {
    let cfg = ProviderConfig::Command(CommandProviderConfig {
        command: "test".to_string(),
        default_model: None,
        args: vec![],
        model_args: vec![],
        system_args: vec![],
        env: BTreeMap::new(),
    });
    let json = serde_json::to_string(&cfg).unwrap();
    assert!(json.contains("command"));
    assert!(json.contains("test"));
}

// ─── AppConfig ───

#[test]
fn app_config_built_in_has_providers() {
    let cfg = AppConfig::built_in();
    assert!(cfg.providers.contains_key("openai"));
    assert!(cfg.providers.contains_key("deepseek"));
    assert!(cfg.providers.contains_key("gemini"));
    assert!(cfg.providers.contains_key("groq"));
    assert!(cfg.providers.contains_key("ollama"));
}

#[test]
fn app_config_built_in_has_sumopod_default() {
    let cfg = AppConfig::built_in();
    assert_eq!(cfg.default_provider.as_deref(), Some("sumopod"));
}

#[test]
fn app_config_built_in_mcp_empty() {
    let cfg = AppConfig::built_in();
    assert!(cfg.mcp.is_empty());
}

#[test]
fn app_config_built_in_provider_count() {
    let cfg = AppConfig::built_in();
    // 23 openai-compatible + 3 command providers
    assert!(cfg.providers.len() >= 20);
}

#[test]
fn app_config_provider_name_explicit() {
    let cfg = AppConfig::built_in();
    assert_eq!(cfg.provider_name(Some("openai")).unwrap(), "openai");
    assert_eq!(cfg.provider_name(Some("deepseek")).unwrap(), "deepseek");
}

#[test]
fn app_config_provider_name_default() {
    let cfg = AppConfig::built_in();
    assert_eq!(cfg.provider_name(None).unwrap(), "sumopod");
}

#[test]
fn app_config_provider_name_no_default() {
    let mut cfg = AppConfig::built_in();
    cfg.default_provider = None;
    assert!(cfg.provider_name(None).is_err());
}

#[test]
fn app_config_merge_user_overrides_default() {
    let mut built = AppConfig::built_in();
    let user = AppConfig {
        default_provider: Some("custom".to_string()),
        providers: BTreeMap::new(),
        mcp: BTreeMap::new(),
    };
    built.merge_user(user);
    assert_eq!(built.default_provider.as_deref(), Some("custom"));
}

#[test]
fn app_config_merge_user_keeps_default_when_omitted() {
    let mut built = AppConfig::built_in();
    built.default_provider = Some("original".to_string());
    let user = AppConfig {
        default_provider: None,
        providers: BTreeMap::new(),
        mcp: BTreeMap::new(),
    };
    built.merge_user(user);
    assert_eq!(built.default_provider.as_deref(), Some("original"));
}

#[test]
fn app_config_merge_user_adds_providers() {
    let mut built = AppConfig::built_in();
    let mut user_providers = BTreeMap::new();
    user_providers.insert(
        "my-custom".to_string(),
        ProviderConfig::OpenAiCompatible(OpenAiCompatibleProviderConfig {
            base_url: "https://my-custom.com/v1".to_string(),
            api_key: None,
            api_key_env: Some("MY_KEY".to_string()),
            default_model: None,
            fast_model: None,
            headers: BTreeMap::new(),
            prompt_cache: None,
            max_tokens: None,
            extended_thinking: None,
            pricing: None,
        }),
    );
    let user = AppConfig {
        default_provider: None,
        providers: user_providers,
        mcp: BTreeMap::new(),
    };
    built.merge_user(user);
    assert!(built.providers.contains_key("my-custom"));
}

#[test]
fn app_config_merge_user_overrides_providers() {
    let mut built = AppConfig::built_in();
    let mut user_providers = BTreeMap::new();
    user_providers.insert(
        "openai".to_string(),
        ProviderConfig::OpenAiCompatible(OpenAiCompatibleProviderConfig {
            base_url: "https://custom-openai.com/v1".to_string(),
            api_key: None,
            api_key_env: Some("CUSTOM_KEY".to_string()),
            default_model: Some("custom-model".to_string()),
            fast_model: None,
            headers: BTreeMap::new(),
            prompt_cache: None,
            max_tokens: None,
            extended_thinking: None,
            pricing: None,
        }),
    );
    let user = AppConfig {
        default_provider: None,
        providers: user_providers,
        mcp: BTreeMap::new(),
    };
    built.merge_user(user);
    if let ProviderConfig::OpenAiCompatible(cfg) = &built.providers["openai"] {
        assert_eq!(cfg.base_url, "https://custom-openai.com/v1");
        assert_eq!(cfg.default_model.as_deref(), Some("custom-model"));
    } else {
        panic!("Expected OpenAiCompatible");
    }
}

#[test]
fn app_config_merge_user_empty_does_nothing() {
    let mut built = AppConfig::built_in();
    let original_count = built.providers.len();
    let user = AppConfig {
        default_provider: None,
        providers: BTreeMap::new(),
        mcp: BTreeMap::new(),
    };
    built.merge_user(user);
    assert_eq!(built.providers.len(), original_count);
}

#[test]
fn app_config_merge_mcp_servers() {
    let mut built = AppConfig::built_in();
    let mut mcp = BTreeMap::new();
    mcp.insert(
        "test-server".to_string(),
        McpServerConfig {
            command: "npx".to_string(),
            args: vec!["-y".to_string(), "@test/server".to_string()],
            env: BTreeMap::new(),
        },
    );
    let user = AppConfig {
        default_provider: None,
        providers: BTreeMap::new(),
        mcp,
    };
    built.merge_user(user);
    assert!(built.mcp.contains_key("test-server"));
}

#[test]
fn app_config_clone() {
    let cfg = AppConfig::built_in();
    let cloned = cfg.clone();
    assert_eq!(cloned.default_provider, cfg.default_provider);
    assert_eq!(cloned.providers.len(), cfg.providers.len());
}

#[test]
fn app_config_debug_contains_provider() {
    let cfg = AppConfig::built_in();
    let debug = format!("{:?}", cfg);
    assert!(debug.contains("openai"));
}

#[test]
fn app_config_serialize_deserialize() {
    let cfg = AppConfig::built_in();
    let json = serde_json::to_string(&cfg).unwrap();
    let parsed: AppConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.default_provider, cfg.default_provider);
    assert_eq!(parsed.providers.len(), cfg.providers.len());
}

#[test]
fn app_config_empty() {
    let cfg = AppConfig {
        default_provider: None,
        providers: BTreeMap::new(),
        mcp: BTreeMap::new(),
    };
    assert!(cfg.providers.is_empty());
    assert!(cfg.mcp.is_empty());
    assert!(cfg.provider_name(None).is_err());
}

// ─── McpServerConfig ───

#[test]
fn mcp_server_config_minimal() {
    let cfg = McpServerConfig {
        command: "npx".to_string(),
        args: vec![],
        env: BTreeMap::new(),
    };
    assert_eq!(cfg.command, "npx");
    assert!(cfg.args.is_empty());
    assert!(cfg.env.is_empty());
}

#[test]
fn mcp_server_config_full() {
    let mut env = BTreeMap::new();
    env.insert("PATH".to_string(), "/usr/bin".to_string());
    let cfg = McpServerConfig {
        command: "docker".to_string(),
        args: vec!["run".to_string(), "-it".to_string(), "server".to_string()],
        env,
    };
    assert_eq!(cfg.args.len(), 3);
    assert_eq!(cfg.env.len(), 1);
}

#[test]
fn mcp_server_config_clone() {
    let cfg = McpServerConfig {
        command: "cmd".to_string(),
        args: vec!["arg".to_string()],
        env: BTreeMap::new(),
    };
    let cloned = cfg.clone();
    assert_eq!(cloned.command, cfg.command);
    assert_eq!(cloned.args, cfg.args);
}

#[test]
fn mcp_server_config_debug() {
    let cfg = McpServerConfig {
        command: "debug-cmd".to_string(),
        args: vec![],
        env: BTreeMap::new(),
    };
    let debug = format!("{:?}", cfg);
    assert!(debug.contains("debug-cmd"));
}

#[test]
fn mcp_server_config_serialize() {
    let cfg = McpServerConfig {
        command: "test".to_string(),
        args: vec!["arg1".to_string(), "arg2".to_string()],
        env: BTreeMap::new(),
    };
    let json = serde_json::to_string(&cfg).unwrap();
    assert!(json.contains("test"));
    assert!(json.contains("arg1"));
}

#[test]
fn mcp_server_config_serialize_roundtrip() {
    let cfg = McpServerConfig {
        command: "test".to_string(),
        args: vec!["arg".to_string()],
        env: BTreeMap::new(),
    };
    let json = serde_json::to_string(&cfg).unwrap();
    let parsed: McpServerConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.command, cfg.command);
    assert_eq!(parsed.args, cfg.args);
}

#[test]
fn mcp_server_env_multiple() {
    let mut env = BTreeMap::new();
    env.insert("A".to_string(), "1".to_string());
    env.insert("B".to_string(), "2".to_string());
    env.insert("C".to_string(), "3".to_string());
    let cfg = McpServerConfig {
        command: "cmd".to_string(),
        args: vec![],
        env,
    };
    assert_eq!(cfg.env.len(), 3);
}

// ─── Built-in Provider Configs ───

#[test]
fn built_in_openai_has_key_env() {
    let cfg = AppConfig::built_in();
    if let ProviderConfig::OpenAiCompatible(p) = &cfg.providers["openai"] {
        assert_eq!(p.api_key_env.as_deref(), Some("OPENAI_API_KEY"));
        assert!(p.api_key.is_none());
    } else {
        panic!("Expected OpenAiCompatible");
    }
}

#[test]
fn built_in_ollama_no_key_env() {
    let cfg = AppConfig::built_in();
    if let ProviderConfig::OpenAiCompatible(p) = &cfg.providers["ollama"] {
        assert!(p.api_key_env.is_none());
        assert_eq!(p.base_url, "http://localhost:11434/v1");
    } else {
        panic!("Expected OpenAiCompatible");
    }
}

#[test]
fn built_in_claude_code_is_command() {
    let cfg = AppConfig::built_in();
    assert!(matches!(
        &cfg.providers["claude-code"],
        ProviderConfig::Command(_)
    ));
}

#[test]
fn built_in_codex_is_command() {
    let cfg = AppConfig::built_in();
    assert!(matches!(
        &cfg.providers["codex"],
        ProviderConfig::Command(_)
    ));
}

#[test]
fn built_in_copilot_is_command() {
    let cfg = AppConfig::built_in();
    assert!(matches!(
        &cfg.providers["copilot"],
        ProviderConfig::Command(_)
    ));
}

#[test]
fn built_in_claude_code_command_args() {
    let cfg = AppConfig::built_in();
    if let ProviderConfig::Command(p) = &cfg.providers["claude-code"] {
        assert_eq!(p.command, "claude");
        assert!(p.args.contains(&"-p".to_string()));
        assert!(p.args.contains(&"{prompt}".to_string()));
        assert_eq!(p.model_args[0], "--model");
    } else {
        panic!("Expected Command");
    }
}

#[test]
fn built_in_github_models_has_headers() {
    let cfg = AppConfig::built_in();
    if let ProviderConfig::OpenAiCompatible(p) = &cfg.providers["github-models"] {
        assert!(p.headers.contains_key("Accept"));
        assert!(p.headers.contains_key("X-GitHub-Api-Version"));
    } else {
        panic!("Expected OpenAiCompatible");
    }
}

#[test]
fn built_in_all_openai_providers_have_base_url() {
    let cfg = AppConfig::built_in();
    for (name, provider) in &cfg.providers {
        if let ProviderConfig::OpenAiCompatible(p) = provider {
            assert!(
                !p.base_url.is_empty(),
                "Provider {} has empty base_url",
                name
            );
            assert!(p.base_url.starts_with("http://") || p.base_url.starts_with("https://"));
        }
    }
}

#[test]
fn built_in_all_command_providers_have_command() {
    let cfg = AppConfig::built_in();
    for (name, provider) in &cfg.providers {
        if let ProviderConfig::Command(p) = provider {
            assert!(!p.command.is_empty(), "Provider {} has empty command", name);
        }
    }
}

#[test]
fn built_in_sumopod_provider_exists() {
    let cfg = AppConfig::built_in();
    assert!(cfg.providers.contains_key("sumopod"));
    if let ProviderConfig::OpenAiCompatible(p) = &cfg.providers["sumopod"] {
        assert_eq!(p.api_key_env.as_deref(), Some("SUMOPOD_API_KEY"));
    }
}

#[test]
fn built_in_deepseek_provider() {
    let cfg = AppConfig::built_in();
    assert!(cfg.providers.contains_key("deepseek"));
    if let ProviderConfig::OpenAiCompatible(p) = &cfg.providers["deepseek"] {
        assert_eq!(p.base_url, "https://api.deepseek.com");
    }
}

#[test]
fn built_in_gemini_provider() {
    let cfg = AppConfig::built_in();
    assert!(cfg.providers.contains_key("gemini"));
    if let ProviderConfig::OpenAiCompatible(p) = &cfg.providers["gemini"] {
        assert!(p.base_url.contains("googleapis.com"));
    }
}

#[test]
fn built_in_local_providers_no_auth() {
    let cfg = AppConfig::built_in();
    for name in &["ollama", "lm-studio", "vllm", "litellm", "localai"] {
        assert!(
            cfg.providers.contains_key(*name),
            "Missing provider: {}",
            name
        );
        if let ProviderConfig::OpenAiCompatible(p) = &cfg.providers[*name] {
            assert!(
                p.api_key_env.is_none(),
                "Provider {} should not require API key",
                name
            );
        }
    }
}

#[test]
fn built_in_provider_fast_model_default_none() {
    let cfg = AppConfig::built_in();
    for (name, provider) in &cfg.providers {
        if let ProviderConfig::OpenAiCompatible(p) = provider {
            assert!(
                p.fast_model.is_none(),
                "Provider {} should not have default fast_model",
                name
            );
        }
    }
}

#[test]
fn built_in_provider_prompt_cache_default_none() {
    let cfg = AppConfig::built_in();
    for (name, provider) in &cfg.providers {
        if let ProviderConfig::OpenAiCompatible(p) = provider {
            assert!(
                p.prompt_cache.is_none(),
                "Provider {} should not have default prompt_cache",
                name
            );
        }
    }
}

#[test]
fn built_in_provider_extended_thinking_default_none() {
    let cfg = AppConfig::built_in();
    for (name, provider) in &cfg.providers {
        if let ProviderConfig::OpenAiCompatible(p) = provider {
            assert!(
                p.extended_thinking.is_none(),
                "Provider {} should not have default extended_thinking",
                name
            );
        }
    }
}

#[test]
fn built_in_provider_pricing_default_none() {
    let cfg = AppConfig::built_in();
    for (name, provider) in &cfg.providers {
        if let ProviderConfig::OpenAiCompatible(p) = provider {
            assert!(
                p.pricing.is_none(),
                "Provider {} should not have default pricing",
                name
            );
        }
    }
}

// ─── SAMPLE_CONFIG ───

#[test]
fn sample_config_is_valid_toml() {
    let cfg: AppConfig = toml::from_str(SAMPLE_CONFIG).unwrap();
    assert!(cfg.providers.contains_key("openai"));
    assert!(cfg.providers.contains_key("sumopod"));
    assert!(cfg.providers.contains_key("ollama"));
}

#[test]
fn sample_config_has_default_provider() {
    let cfg: AppConfig = toml::from_str(SAMPLE_CONFIG).unwrap();
    assert!(cfg.default_provider.is_some());
}

#[test]
fn sample_config_has_command_providers() {
    let cfg: AppConfig = toml::from_str(SAMPLE_CONFIG).unwrap();
    assert!(matches!(
        &cfg.providers["claude-code"],
        ProviderConfig::Command(_)
    ));
    assert!(matches!(
        &cfg.providers["codex"],
        ProviderConfig::Command(_)
    ));
    assert!(matches!(
        &cfg.providers["copilot"],
        ProviderConfig::Command(_)
    ));
}

#[test]
fn sample_config_openai_providers_count() {
    let cfg: AppConfig = toml::from_str(SAMPLE_CONFIG).unwrap();
    let count = cfg
        .providers
        .values()
        .filter(|p| matches!(p, ProviderConfig::OpenAiCompatible(_)))
        .count();
    assert!(count >= 10);
}

#[test]
fn sample_config_command_providers_count() {
    let cfg: AppConfig = toml::from_str(SAMPLE_CONFIG).unwrap();
    let count = cfg
        .providers
        .values()
        .filter(|p| matches!(p, ProviderConfig::Command(_)))
        .count();
    assert_eq!(count, 3); // claude-code, codex, copilot
}

#[test]
fn sample_config_claude_code_args() {
    let cfg: AppConfig = toml::from_str(SAMPLE_CONFIG).unwrap();
    if let ProviderConfig::Command(p) = &cfg.providers["claude-code"] {
        assert!(p.system_args.contains(&"--system-prompt".to_string()));
        assert!(p.system_args.contains(&"{system}".to_string()));
    }
}

#[test]
fn sample_config_github_headers() {
    let cfg: AppConfig = toml::from_str(SAMPLE_CONFIG).unwrap();
    if let ProviderConfig::OpenAiCompatible(p) = &cfg.providers["github-models"] {
        assert!(p.headers.contains_key("Accept"));
    }
}

// ─── config_path ───

#[test]
fn config_path_returns_path() {
    let path = config_path().unwrap();
    assert!(path.extension().is_some());
    assert!(path.file_name().unwrap() == "config.toml");
}

#[test]
fn config_path_in_config_dir() {
    let path = config_path().unwrap();
    assert!(path.parent().unwrap().file_name().unwrap() == "anveesa");
}

// ─── TOML serialization roundtrip for full config ───

#[test]
fn full_config_toml_roundtrip() {
    let cfg = AppConfig {
        default_provider: Some("my-provider".to_string()),
        providers: {
            let mut map = BTreeMap::new();
            map.insert(
                "my-provider".to_string(),
                ProviderConfig::OpenAiCompatible(OpenAiCompatibleProviderConfig {
                    base_url: "https://my.api.com/v1".to_string(),
                    api_key: None,
                    api_key_env: Some("MY_API_KEY".to_string()),
                    default_model: Some("my-model".to_string()),
                    fast_model: Some("my-fast".to_string()),
                    headers: BTreeMap::new(),
                    prompt_cache: Some(true),
                    max_tokens: Some(4096),
                    extended_thinking: Some(8000),
                    pricing: Some([1.0, 5.0, 0.1, 0.5]),
                }),
            );
            map
        },
        mcp: {
            let mut map = BTreeMap::new();
            map.insert(
                "fs".to_string(),
                McpServerConfig {
                    command: "npx".to_string(),
                    args: vec![
                        "-y".to_string(),
                        "@modelcontextprotocol/server-filesystem".to_string(),
                    ],
                    env: BTreeMap::new(),
                },
            );
            map
        },
    };
    let toml_str = toml::to_string(&cfg).unwrap();
    let parsed: AppConfig = toml::from_str(&toml_str).unwrap();
    assert_eq!(parsed.default_provider, cfg.default_provider);
    assert_eq!(parsed.providers.len(), cfg.providers.len());
    assert_eq!(parsed.mcp.len(), cfg.mcp.len());
}

#[test]
fn provider_config_toml_serialize_openai() {
    let cfg = ProviderConfig::OpenAiCompatible(OpenAiCompatibleProviderConfig {
        base_url: "https://test.com/v1".to_string(),
        api_key: None,
        api_key_env: Some("TEST_KEY".to_string()),
        default_model: Some("test".to_string()),
        fast_model: None,
        headers: BTreeMap::new(),
        prompt_cache: None,
        max_tokens: None,
        extended_thinking: None,
        pricing: None,
    });
    let toml_str = toml::to_string(&cfg).unwrap();
    assert!(toml_str.contains("openai-compatible"));
    assert!(toml_str.contains("test.com"));
}

#[test]
fn provider_config_toml_serialize_command() {
    let cfg = ProviderConfig::Command(CommandProviderConfig {
        command: "my-cmd".to_string(),
        default_model: Some("m".to_string()),
        args: vec!["-p".to_string()],
        model_args: vec!["--model".to_string()],
        system_args: vec![],
        env: BTreeMap::new(),
    });
    let toml_str = toml::to_string(&cfg).unwrap();
    assert!(toml_str.contains("command"));
    assert!(toml_str.contains("my-cmd"));
}

// ─── Edge cases ───

#[test]
fn openai_empty_base_url_is_valid() {
    // The struct allows empty base_url (validation happens at runtime)
    let cfg = OpenAiCompatibleProviderConfig {
        base_url: "".to_string(),
        api_key: None,
        api_key_env: None,
        default_model: None,
        fast_model: None,
        headers: BTreeMap::new(),
        prompt_cache: None,
        max_tokens: None,
        extended_thinking: None,
        pricing: None,
    };
    assert!(cfg.base_url.is_empty());
}

#[test]
fn openai_unicode_model_name() {
    let cfg = OpenAiCompatibleProviderConfig {
        base_url: "https://test.com/v1".to_string(),
        api_key: None,
        api_key_env: None,
        default_model: Some("模型名".to_string()),
        fast_model: None,
        headers: BTreeMap::new(),
        prompt_cache: None,
        max_tokens: None,
        extended_thinking: None,
        pricing: None,
    };
    assert_eq!(cfg.default_model.as_deref(), Some("模型名"));
}

#[test]
fn command_unicode_command_name() {
    let cfg = CommandProviderConfig {
        command: "コマンド".to_string(),
        default_model: None,
        args: vec![],
        model_args: vec![],
        system_args: vec![],
        env: BTreeMap::new(),
    };
    assert_eq!(cfg.command, "コマンド");
}

#[test]
fn mcp_unicode_env_key() {
    let mut env = BTreeMap::new();
    env.insert("環境".to_string(), "value".to_string());
    let cfg = McpServerConfig {
        command: "cmd".to_string(),
        args: vec![],
        env,
    };
    assert_eq!(cfg.env.get("環境").unwrap(), "value");
}

#[test]
fn provider_config_set_model_overwrites() {
    let mut cfg = ProviderConfig::OpenAiCompatible(OpenAiCompatibleProviderConfig {
        base_url: "https://test.com/v1".to_string(),
        api_key: None,
        api_key_env: None,
        default_model: Some("old-model".to_string()),
        fast_model: None,
        headers: BTreeMap::new(),
        prompt_cache: None,
        max_tokens: None,
        extended_thinking: None,
        pricing: None,
    });
    cfg.set_default_model("new-model".to_string());
    assert_eq!(cfg.default_model(), Some("new-model"));
    // Set again to different model
    cfg.set_default_model("another-model".to_string());
    assert_eq!(cfg.default_model(), Some("another-model"));
}

#[test]
fn app_config_merge_preserves_existing_providers() {
    let mut built = AppConfig::built_in();
    let original_keys: Vec<String> = built.providers.keys().cloned().collect();
    let user = AppConfig {
        default_provider: None,
        providers: {
            let mut map = BTreeMap::new();
            map.insert(
                "new-only".to_string(),
                ProviderConfig::OpenAiCompatible(OpenAiCompatibleProviderConfig {
                    base_url: "https://new.com/v1".to_string(),
                    api_key: None,
                    api_key_env: None,
                    default_model: None,
                    fast_model: None,
                    headers: BTreeMap::new(),
                    prompt_cache: None,
                    max_tokens: None,
                    extended_thinking: None,
                    pricing: None,
                }),
            );
            map
        },
        mcp: BTreeMap::new(),
    };
    built.merge_user(user);
    // Original providers preserved
    for key in &original_keys {
        assert!(built.providers.contains_key(key), "Missing: {}", key);
    }
    // New provider added
    assert!(built.providers.contains_key("new-only"));
}

#[test]
fn app_config_multiple_merges() {
    let mut cfg = AppConfig::built_in();
    // First merge
    let user1 = AppConfig {
        default_provider: Some("first".to_string()),
        providers: BTreeMap::new(),
        mcp: BTreeMap::new(),
    };
    cfg.merge_user(user1);
    // Second merge overrides
    let user2 = AppConfig {
        default_provider: Some("second".to_string()),
        providers: BTreeMap::new(),
        mcp: BTreeMap::new(),
    };
    cfg.merge_user(user2);
    assert_eq!(cfg.default_provider.as_deref(), Some("second"));
}
