//! Per-project persisted tool permissions — `.anveesa/settings.json`.
//!
//! When the user picks "[s] always" in an approval prompt, a rule is saved so
//! the same kind of action never prompts again in this project. Rules have
//! the form `"<tool>:<pattern>"`, where the pattern is matched against the
//! call's primary argument (the path for file tools, the full command line
//! for run_command). A trailing `*` makes the pattern a prefix match:
//!
//! ```json
//! {
//!   "permissions": {
//!     "allow": [
//!       "run_command:cargo *",
//!       "write_file:src/main.rs",
//!       "edit_file:src/*"
//!     ]
//!   }
//! }
//! ```

use std::path::{Path, PathBuf};

pub fn settings_path(root: &Path) -> PathBuf {
    root.join(".anveesa").join("settings.json")
}

/// Load the project's saved allow rules. Missing or malformed file = no rules.
pub fn load_rules(root: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(settings_path(root)) else {
        return vec![];
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return vec![];
    };
    v["permissions"]["allow"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Append a rule to the project's settings, preserving any other keys in the
/// file. No-op if the rule is already present.
pub fn save_rule(root: &Path, rule: &str) -> std::io::Result<()> {
    let path = settings_path(root);
    let mut v: serde_json::Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    let mut arr = v["permissions"]["allow"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if !arr.iter().any(|x| x.as_str() == Some(rule)) {
        arr.push(serde_json::Value::String(rule.to_string()));
    }
    v["permissions"]["allow"] = serde_json::Value::Array(arr);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(&v).unwrap_or_default())
}

/// Does any saved rule allow this (tool, primary argument) pair?
pub fn rule_allows(rules: &[String], tool: &str, arg: &str) -> bool {
    rules.iter().any(|r| {
        let Some((rule_tool, pattern)) = r.split_once(':') else {
            return false;
        };
        rule_tool == tool && pattern_matches(pattern.trim(), arg.trim())
    })
}

/// Exact match, or prefix match when the pattern ends with `*`.
/// `"cargo *"` allows `"cargo build"` and bare `"cargo"`, but not `"cargogo"`.
fn pattern_matches(pattern: &str, arg: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => arg.starts_with(prefix) || arg == prefix.trim_end(),
        None => arg == pattern,
    }
}

/// The call's primary argument: the path for file tools, the command line
/// for run_command. None when the arguments don't parse.
pub fn primary_arg(tool: &str, arguments: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(arguments).ok()?;
    if tool == "run_command" {
        return v["command"].as_str().map(|s| s.trim().to_string());
    }
    v.get("path")
        .or_else(|| v.get("destination"))
        .or_else(|| v.get("dest"))
        .and_then(|x| x.as_str())
        .map(str::to_string)
}

/// The rule persisted when the user picks "always allow" for this call:
/// run_command saves the program name as a prefix rule ("cargo *"); file
/// tools save the exact path.
pub fn derive_rule(tool: &str, arg: &str) -> String {
    if tool == "run_command" {
        let program = arg.split_whitespace().next().unwrap_or(arg);
        format!("{tool}:{program} *")
    } else {
        format!("{tool}:{arg}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_rule_matches_only_exact() {
        let rules = vec!["write_file:src/main.rs".to_string()];
        assert!(rule_allows(&rules, "write_file", "src/main.rs"));
        assert!(!rule_allows(&rules, "write_file", "src/lib.rs"));
        assert!(!rule_allows(&rules, "edit_file", "src/main.rs"));
    }

    #[test]
    fn star_rule_is_prefix_match() {
        let rules = vec!["run_command:cargo *".to_string()];
        assert!(rule_allows(&rules, "run_command", "cargo build --release"));
        assert!(rule_allows(&rules, "run_command", "cargo"));
        assert!(!rule_allows(&rules, "run_command", "cargogo build"));
        assert!(!rule_allows(&rules, "run_command", "rm -rf /"));
    }

    #[test]
    fn path_star_rule_matches_directory() {
        let rules = vec!["edit_file:src/*".to_string()];
        assert!(rule_allows(&rules, "edit_file", "src/main.rs"));
        assert!(rule_allows(&rules, "edit_file", "src/tui/render.rs"));
        assert!(!rule_allows(&rules, "edit_file", "Cargo.toml"));
    }

    #[test]
    fn malformed_rules_never_match() {
        let rules = vec!["no-colon-here".to_string(), String::new()];
        assert!(!rule_allows(&rules, "write_file", "no-colon-here"));
        assert!(!rule_allows(&rules, "", ""));
    }

    #[test]
    fn derive_rule_for_run_command_uses_program_prefix() {
        assert_eq!(
            derive_rule("run_command", "cargo build --release"),
            "run_command:cargo *"
        );
        assert_eq!(derive_rule("write_file", "src/x.rs"), "write_file:src/x.rs");
    }

    #[test]
    fn primary_arg_extracts_command_and_path() {
        assert_eq!(
            primary_arg("run_command", r#"{"command":"cargo test"}"#),
            Some("cargo test".to_string())
        );
        assert_eq!(
            primary_arg("write_file", r#"{"path":"a.rs","content":"x"}"#),
            Some("a.rs".to_string())
        );
        assert_eq!(primary_arg("write_file", "not json"), None);
    }

    #[test]
    fn save_and_load_roundtrip_preserves_other_keys() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // Pre-existing settings with an unrelated key
        std::fs::create_dir_all(root.join(".anveesa")).unwrap();
        std::fs::write(
            settings_path(root),
            r#"{"theme":"dark","permissions":{"allow":["run_command:ls *"]}}"#,
        )
        .unwrap();

        save_rule(root, "write_file:src/main.rs").unwrap();
        save_rule(root, "write_file:src/main.rs").unwrap(); // dedupe

        let rules = load_rules(root);
        assert_eq!(rules.len(), 2);
        assert!(rules.contains(&"run_command:ls *".to_string()));
        assert!(rules.contains(&"write_file:src/main.rs".to_string()));

        // unrelated key survived
        let text = std::fs::read_to_string(settings_path(root)).unwrap();
        assert!(text.contains("\"theme\""));
    }

    #[test]
    fn load_rules_missing_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_rules(dir.path()).is_empty());
    }
}
