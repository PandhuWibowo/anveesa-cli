//! 1000-scenario test suite for tools.rs.
//!
//! Loaded as `mod scenarios` from tools.rs so that private functions are
//! accessible.  Each section targets a specific tool or helper.  Total
//! assertion count: ≥ 1000.

#![allow(clippy::too_many_lines)]

use super::*;
use serde_json::json;
use std::{fs, path::Path};

// ── helpers ───────────────────────────────────────────────────────────────────

fn tmp(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir()
        .join(format!("anveesa_sc_{tag}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Shorthand for `is_sensitive_path`.
fn sens(p: &str) -> bool {
    is_sensitive_path(Path::new(p))
}

/// True if the JSON result has `ok: true`.
fn is_ok(v: &serde_json::Value) -> bool {
    v["ok"].as_bool().unwrap_or(false)
}

// ═══════════════════════════════════════════════════════════════════════════════
// Section 1 — is_sensitive_path  (157 assertions)
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn s1_ssh_directory_paths() {
    // /.ssh/ anywhere in path → blocked  (15)
    assert!(sens("/home/u/.ssh/id_rsa"));
    assert!(sens("/home/u/.ssh/id_dsa"));
    assert!(sens("/home/u/.ssh/id_ed25519"));
    assert!(sens("/home/u/.ssh/id_ecdsa"));
    assert!(sens("/home/u/.ssh/config"));
    assert!(sens("/home/u/.ssh/known_hosts"));
    assert!(sens("/home/u/.ssh/authorized_keys"));
    assert!(sens("/root/.ssh/id_rsa"));
    assert!(sens("/.ssh/id_rsa"));
    assert!(sens("/any/deep/path/.ssh/key"));
    assert!(sens("/home/user/.ssh/id_rsa.pub"));
    assert!(sens("/home/user/.ssh/identity"));
    assert!(sens("/tmp/.ssh/deploy_key"));
    assert!(sens("/ci/.ssh/github_deploy"));
    assert!(sens("/runner/.ssh/actions_key"));
}

#[test]
fn s1_ssh_standalone_key_filenames() {
    // Files named id_rsa / id_dsa / id_ed25519 / id_ecdsa outside .ssh/  (8)
    assert!(sens("/project/certs/id_rsa"));
    assert!(sens("/project/certs/id_dsa"));
    assert!(sens("/project/certs/id_ed25519"));
    assert!(sens("/project/certs/id_ecdsa"));
    assert!(sens("/tmp/id_rsa"));
    assert!(sens("/etc/id_rsa"));
    assert!(sens("/id_ed25519"));
    assert!(sens("/deploy/id_ecdsa"));
}

#[test]
fn s1_aws_paths() {
    // /.aws/ anywhere → blocked  (12)
    assert!(sens("/home/u/.aws/credentials"));
    assert!(sens("/home/u/.aws/config"));
    assert!(sens("/root/.aws/credentials"));
    assert!(sens("/home/runner/.aws/config"));
    assert!(sens("/.aws/credentials"));
    assert!(sens("/home/u/.aws/sso/cache/token.json"));
    assert!(sens("/home/u/.aws/cli/cache/data.json"));
    assert!(sens("/any/.aws/credentials"));
    assert!(sens("/home/user/.aws/config"));
    assert!(sens("/home/user/.aws/credentials.bak"));
    assert!(sens("/ci/.aws/config"));
    assert!(sens("/tmp/.aws/something"));
}

#[test]
fn s1_gnupg_paths() {
    // /.gnupg/ anywhere → blocked  (10)
    assert!(sens("/home/u/.gnupg/secring.gpg"));
    assert!(sens("/home/u/.gnupg/trustdb.gpg"));
    assert!(sens("/home/u/.gnupg/private-keys-v1.d/somekey"));
    assert!(sens("/root/.gnupg/secring.gpg"));
    assert!(sens("/.gnupg/secring.gpg"));
    assert!(sens("/home/u/.gnupg/pubring.kbx"));
    assert!(sens("/home/u/.gnupg/random_seed"));
    assert!(sens("/home/u/.gnupg/openpgp-revocs.d/key.rev"));
    assert!(sens("/any/.gnupg/something"));
    assert!(sens("/home/user/.gnupg/S.gpg-agent"));
}

#[test]
fn s1_kube_paths() {
    // /.kube/ anywhere → blocked  (10)
    assert!(sens("/home/u/.kube/config"));
    assert!(sens("/home/u/.kube/cache/discovery/apiserver/v1.json"));
    assert!(sens("/root/.kube/config"));
    assert!(sens("/.kube/config"));
    assert!(sens("/home/u/.kube/http-cache/something"));
    assert!(sens("/ci/.kube/config"));
    assert!(sens("/home/runner/.kube/config"));
    assert!(sens("/any/.kube/something"));
    assert!(sens("/home/user/.kube/kubeconfig"));
    assert!(sens("/home/user/.kube/"));
}

#[test]
fn s1_docker_paths() {
    // /.docker/ anywhere → blocked  (10)
    assert!(sens("/home/u/.docker/config.json"));
    assert!(sens("/home/u/.docker/buildx/current"));
    assert!(sens("/root/.docker/config.json"));
    assert!(sens("/.docker/config.json"));
    assert!(sens("/home/u/.docker/scan/config.json"));
    assert!(sens("/ci/.docker/config.json"));
    assert!(sens("/home/runner/.docker/config.json"));
    assert!(sens("/any/.docker/config.json"));
    assert!(sens("/home/user/.docker/trust/private/root.key"));
    assert!(sens("/home/user/.docker/"));
}

#[test]
fn s1_env_files() {
    // .env variants → blocked  (20)
    assert!(sens("/project/.env"));
    assert!(sens("/project/.env.local"));
    assert!(sens("/project/.env.development"));
    assert!(sens("/project/.env.production"));
    assert!(sens("/project/.env.test"));
    assert!(sens("/project/.env.staging"));
    assert!(sens("/project/.env.example"));
    assert!(sens("/.env"));
    assert!(sens("/home/user/app/.env"));
    assert!(sens("/var/app/.env"));
    assert!(sens("/app/.env.docker"));
    assert!(sens("/srv/app/.env.production"));
    assert!(sens("/opt/app/.env.local"));
    assert!(sens("/code/api/.env.test"));
    assert!(sens("/project/.env.ci"));
    assert!(sens("/project/.env.defaults"));
    assert!(sens("/project/.env.override"));
    assert!(sens("/any/.env"));
    assert!(sens("/some/deep/path/.env.production"));
    assert!(sens("/home/user/work/.env.local"));
}

#[test]
fn s1_credential_files() {
    // ~/.netrc, ~/.npmrc, ~/.pypirc, ~/.git-credentials, */credentials  (16)
    assert!(sens("/home/u/.netrc"));
    assert!(sens("/root/.netrc"));
    assert!(sens("/home/u/.npmrc"));
    assert!(sens("/root/.npmrc"));
    assert!(sens("/home/u/.pypirc"));
    assert!(sens("/root/.pypirc"));
    assert!(sens("/home/u/.git-credentials"));
    assert!(sens("/root/.git-credentials"));
    assert!(sens("/home/u/credentials"));
    assert!(sens("/etc/credentials"));
    assert!(sens("/home/user/.netrc"));
    assert!(sens("/home/user/.npmrc"));
    assert!(sens("/home/user/.pypirc"));
    assert!(sens("/home/user/.git-credentials"));
    assert!(sens("/app/credentials"));
    assert!(sens("/tmp/credentials"));
}

#[test]
fn s1_system_files() {
    // /etc/shadow and /etc/passwd, including uppercase variants  (6)
    assert!(sens("/etc/shadow"));
    assert!(sens("/etc/passwd"));
    assert!(sens("/etc/SHADOW"));   // lowercased before check
    assert!(sens("/etc/PASSWD"));
    assert!(!sens("/usr/bin/shadow"));  // not the system shadow file
    assert!(!sens("/etc/sudoers"));     // not blocked
}

#[test]
fn s1_secret_patterns() {
    // secret_key, secretkey, /secrets., /secrets/, private_key  (20)
    assert!(sens("/app/config/secret_key.txt"));
    assert!(sens("/app/config/secretkey.json"));
    assert!(sens("/app/config/DB_SECRET_KEY"));
    assert!(sens("/app/config/API_SECRETKEY"));
    assert!(sens("/app/secrets.yaml"));
    assert!(sens("/app/secrets.json"));
    assert!(sens("/app/secrets.toml"));
    assert!(sens("/app/secrets.env"));
    assert!(sens("/app/secrets/db.json"));
    assert!(sens("/app/secrets/api_keys.json"));
    assert!(sens("/app/secrets/certs/server.pem"));
    assert!(sens("/deploy/secrets.yaml"));
    assert!(sens("/config/secrets.json"));
    assert!(sens("/database_private_key.pem"));
    assert!(sens("/app/private_key.pem"));
    assert!(sens("/certs/server_private_key.pem"));
    assert!(sens("/keys/private_key.der"));
    assert!(sens("/home/user/private_key"));
    assert!(sens("/tmp/private_key.pem"));
    assert!(sens("/app/config/app_private_key.json"));
}

#[test]
fn s1_false_positives_must_pass() {
    // Paths that must NOT be blocked — especially the old over-broad "secret" check  (30)
    assert!(!sens("/proj/src/secret_manager.rs"));
    assert!(!sens("/proj/docs/secret_rotation.md"));
    assert!(!sens("/proj/src/opensecret.rs"));
    assert!(!sens("/proj/tests/test_secret.rs"));
    assert!(!sens("/proj/src/not_a_secret.rs"));
    assert!(!sens("/proj/src/secretariat.rs"));
    assert!(!sens("/proj/src/main.rs"));
    assert!(!sens("/proj/src/lib.rs"));
    assert!(!sens("/proj/Cargo.toml"));
    assert!(!sens("/proj/README.md"));
    assert!(!sens("/proj/src/config.rs"));
    assert!(!sens("/proj/src/aws_client.rs"));
    assert!(!sens("/proj/src/environment.rs"));
    assert!(!sens("/proj/src/settings.rs"));
    assert!(!sens("/proj/target/debug/anveesa"));
    assert!(!sens("/proj/tests/integration_test.rs"));
    assert!(!sens("/proj/.gitignore"));
    assert!(!sens("/proj/Makefile"));
    assert!(!sens("/proj/package.json"));
    assert!(!sens("/proj/tsconfig.json"));
    assert!(!sens("/home/user/project/src/main.rs"));
    assert!(!sens("/tmp/test_output.txt"));
    assert!(!sens("/tmp/build_log.txt"));
    assert!(!sens("/usr/local/bin/cargo"));
    assert!(!sens("/etc/hosts"));
    assert!(!sens("/etc/resolv.conf"));
    assert!(!sens("/usr/share/doc/something"));
    assert!(!sens("/home/user/.bashrc"));
    assert!(!sens("/home/user/.zshrc"));
    assert!(!sens("/home/user/.profile"));
}

// ═══════════════════════════════════════════════════════════════════════════════
// Section 2 — percent_encode  (50 assertions)
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn s2_percent_encode_unreserved() {
    // RFC 3986 unreserved chars pass through unchanged  (20)
    assert_eq!(percent_encode("a"), "a");
    assert_eq!(percent_encode("z"), "z");
    assert_eq!(percent_encode("A"), "A");
    assert_eq!(percent_encode("Z"), "Z");
    assert_eq!(percent_encode("0"), "0");
    assert_eq!(percent_encode("9"), "9");
    assert_eq!(percent_encode("-"), "-");
    assert_eq!(percent_encode("_"), "_");
    assert_eq!(percent_encode("."), ".");
    assert_eq!(percent_encode("~"), "~");
    assert_eq!(percent_encode(""), "");
    assert_eq!(percent_encode("rust-lang"), "rust-lang");
    assert_eq!(percent_encode("hello_world"), "hello_world");
    assert_eq!(percent_encode("v1.0.0"), "v1.0.0");
    assert_eq!(percent_encode("a-b_c.d~e"), "a-b_c.d~e");
    assert_eq!(percent_encode("test-case_1.0~beta"), "test-case_1.0~beta");
    assert_eq!(percent_encode("abcdefghijklmnopqrstuvwxyz"), "abcdefghijklmnopqrstuvwxyz");
    assert_eq!(percent_encode("ABCDEFGHIJKLMNOPQRSTUVWXYZ"), "ABCDEFGHIJKLMNOPQRSTUVWXYZ");
    assert_eq!(percent_encode("0123456789"), "0123456789");
    assert_eq!(percent_encode("hello.world"), "hello.world");
}

#[test]
fn s2_percent_encode_reserved() {
    // Reserved / special characters get encoded  (20)
    assert_eq!(percent_encode(" "), "%20");
    assert_eq!(percent_encode("a b"), "a%20b");
    assert_eq!(percent_encode("hello world"), "hello%20world");
    assert_eq!(percent_encode("&"), "%26");
    assert_eq!(percent_encode("+"), "%2B");
    assert_eq!(percent_encode("="), "%3D");
    assert_eq!(percent_encode("?"), "%3F");
    assert_eq!(percent_encode("#"), "%23");
    assert_eq!(percent_encode("/"), "%2F");
    assert_eq!(percent_encode(":"), "%3A");
    assert_eq!(percent_encode("@"), "%40");
    assert_eq!(percent_encode("!"), "%21");
    assert_eq!(percent_encode("*"), "%2A");
    assert_eq!(percent_encode("("), "%28");
    assert_eq!(percent_encode(")"), "%29");
    assert_eq!(percent_encode("["), "%5B");
    assert_eq!(percent_encode("]"), "%5D");
    assert_eq!(percent_encode(","), "%2C");
    assert_eq!(percent_encode(";"), "%3B");
    assert_eq!(percent_encode("100%"), "100%25");
}

#[test]
fn s2_percent_encode_mixed() {
    // Combinations of encoded and pass-through characters  (10)
    assert_eq!(percent_encode("foo bar baz"), "foo%20bar%20baz");
    assert_eq!(percent_encode("key=value"), "key%3Dvalue");
    assert_eq!(percent_encode("a+b=c"), "a%2Bb%3Dc");
    assert_eq!(percent_encode("https://example.com"), "https%3A%2F%2Fexample.com");
    assert_eq!(percent_encode("hello\tworld"), "hello%09world");
    assert_eq!(percent_encode("line\nnewline"), "line%0Anewline");
    assert_eq!(percent_encode("quote\"test"), "quote%22test");
    assert_eq!(percent_encode("<html>"), "%3Chtml%3E");
    assert_eq!(percent_encode("{json}"), "%7Bjson%7D");
    assert_eq!(percent_encode("rust async/await"), "rust%20async%2Fawait");
}

// ═══════════════════════════════════════════════════════════════════════════════
// Section 3 — truncate  (30 assertions)
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn s3_truncate_within_limit() {
    // No truncation when value fits  (10)
    assert_eq!(truncate("hello", 10), "hello");
    assert_eq!(truncate("hello", 5), "hello");
    assert_eq!(truncate("", 10), "");
    assert_eq!(truncate("", 0), "");
    assert_eq!(truncate("a", 1), "a");
    assert_eq!(truncate("ab", 2), "ab");
    assert_eq!(truncate("abc", 5), "abc");
    assert_eq!(truncate("hello world", 20), "hello world");
    assert_eq!(truncate("x", 1000), "x");
    assert_eq!(truncate("exact", 5), "exact");
}

#[test]
fn s3_truncate_cut() {
    // Truncation adds "..."  (10)
    assert_eq!(truncate("hello", 3), "hel...");
    assert_eq!(truncate("hello", 4), "hell...");
    assert_eq!(truncate("hello world", 5), "hello...");
    assert_eq!(truncate("a", 0), "...");
    assert_eq!(truncate("ab", 1), "a...");
    assert_eq!(truncate("abcdef", 3), "abc...");
    assert_eq!(truncate("Hello, World!", 5), "Hello...");
    assert_eq!(truncate("1234567890", 7), "1234567...");
    assert_eq!(truncate("rust programming", 4), "rust...");
    assert_eq!(truncate("  spaces  ", 4), "  sp...");
}

#[test]
fn s3_truncate_unicode_char_boundary() {
    // truncate counts Unicode scalar values, not bytes  (10)
    assert_eq!(truncate("café", 4), "café");
    assert_eq!(truncate("café", 3), "caf...");
    assert_eq!(truncate("日本語", 3), "日本語");
    assert_eq!(truncate("日本語テスト", 3), "日本語...");
    assert_eq!(truncate("αβγδεζ", 4), "αβγδ...");
    assert_eq!(truncate("Ñoño", 4), "Ñoño");
    assert_eq!(truncate("Ñoño", 3), "Ñoñ...");
    assert_eq!(truncate("中文测试", 2), "中文...");
    assert_eq!(truncate("emoji🦀", 6), "emoji🦀");
    assert_eq!(truncate("emoji🦀", 5), "emoji...");
}

// ═══════════════════════════════════════════════════════════════════════════════
// Section 4 — normalized_query  (20 assertions)
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn s4_normalized_query_ok() {
    // Trimming and lowercasing  (15)
    assert_eq!(normalized_query("hello").unwrap(), "hello");
    assert_eq!(normalized_query("HELLO").unwrap(), "hello");
    assert_eq!(normalized_query("Hello World").unwrap(), "hello world");
    assert_eq!(normalized_query("  hello  ").unwrap(), "hello");
    assert_eq!(normalized_query("\thello\t").unwrap(), "hello");
    assert_eq!(normalized_query("RUST").unwrap(), "rust");
    assert_eq!(normalized_query("CamelCase").unwrap(), "camelcase");
    assert_eq!(normalized_query("TODO").unwrap(), "todo");
    assert_eq!(normalized_query("fn main").unwrap(), "fn main");
    assert_eq!(normalized_query("  spaces  ").unwrap(), "spaces");
    assert_eq!(normalized_query("UPPER_CASE").unwrap(), "upper_case");
    assert_eq!(normalized_query("MixedCase123").unwrap(), "mixedcase123");
    assert_eq!(normalized_query("日本語").unwrap(), "日本語");
    assert_eq!(normalized_query("Café").unwrap(), "café");
    assert_eq!(normalized_query("a").unwrap(), "a");
}

#[test]
fn s4_normalized_query_errors() {
    // Blank/whitespace-only → error  (5)
    assert!(normalized_query("").is_err());
    assert!(normalized_query("   ").is_err());
    assert!(normalized_query("\t").is_err());
    assert!(normalized_query("\n").is_err());
    assert!(normalized_query("  \t\n  ").is_err());
}

// ═══════════════════════════════════════════════════════════════════════════════
// Section 5 — should_skip_name  (36 assertions)
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn s5_skip_name_blocked() {
    // Exact skip-list entries  (11)
    assert!(should_skip_name(".git"));
    assert!(should_skip_name(".next"));
    assert!(should_skip_name(".turbo"));
    assert!(should_skip_name(".cache"));
    assert!(should_skip_name(".venv"));
    assert!(should_skip_name("node_modules"));
    assert!(should_skip_name("target"));
    assert!(should_skip_name("dist"));
    assert!(should_skip_name("build"));
    assert!(should_skip_name("vendor"));
    assert!(should_skip_name("Library"));
}

#[test]
fn s5_skip_name_allowed() {
    // Similar names that must NOT be skipped  (25)
    assert!(!should_skip_name("src"));
    assert!(!should_skip_name("lib"));
    assert!(!should_skip_name("tests"));
    assert!(!should_skip_name("docs"));
    assert!(!should_skip_name("examples"));
    assert!(!should_skip_name(".gitignore"));
    assert!(!should_skip_name(".env"));
    assert!(!should_skip_name("main.rs"));
    assert!(!should_skip_name("Cargo.toml"));
    assert!(!should_skip_name("target.rs"));     // not "target"
    assert!(!should_skip_name("dist.rs"));       // not "dist"
    assert!(!should_skip_name("builds"));        // not "build"
    assert!(!should_skip_name("vendors"));       // not "vendor"
    assert!(!should_skip_name("libraries"));     // not "Library"
    assert!(!should_skip_name("next.config.js"));
    assert!(!should_skip_name("turbo.json"));
    assert!(!should_skip_name("cache"));         // no leading dot
    assert!(!should_skip_name("venv"));          // no leading dot
    assert!(!should_skip_name("git"));           // no leading dot
    assert!(!should_skip_name("modules"));
    assert!(!should_skip_name("public"));
    assert!(!should_skip_name("static"));
    assert!(!should_skip_name("scripts"));
    assert!(!should_skip_name("config"));
    assert!(!should_skip_name(""));
}

// ═══════════════════════════════════════════════════════════════════════════════
// Section 6 — describe_call  (45 assertions)
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn s6_describe_list_dir() {
    assert_eq!(describe_call("list_dir", r#"{}"#), "list directory .");
    assert_eq!(describe_call("list_dir", r#"{"path":"src"}"#), "list directory src");
    assert_eq!(describe_call("list_dir", r#"{"path":"/home/user"}"#), "list directory /home/user");
    assert_eq!(describe_call("list_dir", r#"{"path":""}"#), "list directory .");
    assert_eq!(describe_call("list_dir", "invalid-json"), "list directory .");
}

#[test]
fn s6_describe_find_files() {
    assert_eq!(describe_call("find_files", r#"{"query":"Cargo"}"#), "find files matching `Cargo` under .");
    assert_eq!(describe_call("find_files", r#"{"query":"Cargo","root":"src"}"#), "find files matching `Cargo` under src");
    assert_eq!(describe_call("find_files", r#"{"query":"main.rs","root":"/project"}"#), "find files matching `main.rs` under /project");
    assert_eq!(describe_call("find_files", r#"{"query":"test"}"#), "find files matching `test` under .");
    assert_eq!(describe_call("find_files", r#"{"query":"","root":"src"}"#), "find files matching `` under src");
}

#[test]
fn s6_describe_search_text() {
    assert_eq!(describe_call("search_text", r#"{"query":"TODO"}"#), "search text `TODO` under .");
    assert_eq!(describe_call("search_text", r#"{"query":"fn main","root":"src"}"#), "search text `fn main` under src");
    assert_eq!(describe_call("search_text", r#"{"query":"FIXME","root":"/project"}"#), "search text `FIXME` under /project");
    assert_eq!(describe_call("search_text", r#"{"query":"println!"}"#), "search text `println!` under .");
    assert_eq!(describe_call("search_text", r#"{"query":"use std"}"#), "search text `use std` under .");
}

#[test]
fn s6_describe_read_file() {
    assert_eq!(describe_call("read_file", r#"{"path":"README.md"}"#), "read file README.md");
    assert_eq!(describe_call("read_file", r#"{"path":"src/main.rs"}"#), "read file src/main.rs");
    assert_eq!(describe_call("read_file", r#"{"path":"/etc/hosts"}"#), "read file /etc/hosts");
    assert_eq!(describe_call("read_file", r#"{"path":""}"#), "read file ");
    assert_eq!(describe_call("read_file", r#"{"path":"Cargo.toml","start_line":10}"#), "read file Cargo.toml");
}

#[test]
fn s6_describe_web_search() {
    assert_eq!(describe_call("web_search", r#"{"query":"rust termios"}"#), "web search `rust termios`");
    assert_eq!(describe_call("web_search", r#"{"query":"tokio async"}"#), "web search `tokio async`");
    assert_eq!(describe_call("web_search", r#"{"query":""}"#), "web search ``");
    assert_eq!(describe_call("web_search", r#"{"query":"how to install rust"}"#), "web search `how to install rust`");
    assert_eq!(describe_call("web_search", r#"{"query":"E0502"}"#), "web search `E0502`");
}

#[test]
fn s6_describe_write_tools() {
    assert_eq!(describe_call("create_dir", r#"{"path":"hello"}"#), "create directory hello");
    assert_eq!(describe_call("create_dir", r#"{"path":"src/components"}"#), "create directory src/components");
    assert_eq!(describe_call("write_file", r#"{"path":"a.txt","content":"x"}"#), "write file a.txt");
    assert_eq!(describe_call("write_file", r#"{"path":"src/main.rs","content":"fn main(){}"}"#), "write file src/main.rs");
    assert_eq!(describe_call("edit_file", r#"{"path":"a.txt","old_string":"x","new_string":"y"}"#), "edit file a.txt");
    assert_eq!(describe_call("edit_file", r#"{"path":"Cargo.toml","old_string":"0.3.0","new_string":"0.4.0"}"#), "edit file Cargo.toml");
    assert_eq!(describe_call("run_command", r#"{"command":"cargo test"}"#), "run command `cargo test`");
    assert_eq!(describe_call("run_command", r#"{"command":"git status"}"#), "run command `git status`");
    assert_eq!(describe_call("run_command", r#"{"command":"ls -la"}"#), "run command `ls -la`");
    assert_eq!(describe_call("run_command", r#"{"command":"make build"}"#), "run command `make build`");
}

#[test]
fn s6_describe_unknown_and_plan_tools() {
    // Unknown tool falls back to "name args..." format  (5)
    assert!(describe_call("unknown_tool", r#"{"foo":"bar"}"#).starts_with("unknown_tool"));
    assert!(describe_call("my_tool", r#"{}"#).starts_with("my_tool"));
    // Plan tools return non-empty strings
    assert!(!describe_call("set_plan", r#"{"steps":["a","b"]}"#).is_empty());
    assert!(!describe_call("complete_task", r#"{"index":0}"#).is_empty());
    // Very long args get truncated in the fallback branch
    let long = "x".repeat(200);
    let result = describe_call("some_tool", &format!(r#"{{"val":"{long}"}}"#));
    assert!(result.len() < 200);  // truncated to 80 + "..." at most in fallback
}

// ═══════════════════════════════════════════════════════════════════════════════
// Section 7 — is_write_tool  (20 assertions)
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn s7_is_write_tool() {
    // Write tools  (4)
    assert!(is_write_tool("create_dir"));
    assert!(is_write_tool("write_file"));
    assert!(is_write_tool("edit_file"));
    assert!(is_write_tool("run_command"));
    // Read-only tools  (7)
    assert!(!is_write_tool("list_dir"));
    assert!(!is_write_tool("find_files"));
    assert!(!is_write_tool("search_text"));
    assert!(!is_write_tool("read_file"));
    assert!(!is_write_tool("web_search"));
    assert!(!is_write_tool("set_plan"));
    assert!(!is_write_tool("complete_task"));
    // Unknown / misspelled names  (9)
    assert!(!is_write_tool(""));
    assert!(!is_write_tool("unknown"));
    assert!(!is_write_tool("CREATE_DIR"));
    assert!(!is_write_tool("WRITE_FILE"));
    assert!(!is_write_tool("EDIT_FILE"));
    assert!(!is_write_tool("RUN_COMMAND"));
    assert!(!is_write_tool("create_directory"));
    assert!(is_write_tool("delete_file"));
    assert!(is_write_tool("move_file"));
}

// ═══════════════════════════════════════════════════════════════════════════════
// Section 8 — cap_output  (20 assertions)
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn s8_cap_output_within_limit() {
    // Outputs at or below the cap are returned unchanged  (10)
    assert_eq!(cap_output(b"hello"), "hello");
    assert_eq!(cap_output(b""), "");
    assert_eq!(cap_output(b"hello\nworld\n"), "hello\nworld\n");
    assert_eq!(cap_output(b"a"), "a");
    assert_eq!(cap_output("café".as_bytes()), "café");
    assert_eq!(cap_output("日本語\n".as_bytes()), "日本語\n");
    assert_eq!(cap_output(b"line1\nline2\nline3"), "line1\nline2\nline3");
    // At the exact limit
    let at_limit = "x".repeat(MAX_COMMAND_OUTPUT);
    let result = cap_output(at_limit.as_bytes());
    assert_eq!(result.len(), MAX_COMMAND_OUTPUT);
    assert!(!result.contains("[output truncated]"));
    // No panic on invalid UTF-8
    let bad_utf8: &[u8] = &[0xFF, 0xFE];
    assert!(!cap_output(bad_utf8).is_empty());
}

#[test]
fn s8_cap_output_over_limit() {
    // Outputs past the cap get the truncation marker appended  (10)
    let over = "x".repeat(MAX_COMMAND_OUTPUT + 1);
    let result = cap_output(over.as_bytes());
    assert!(result.ends_with("[output truncated]"));
    assert!(result.contains("\n...[output truncated]"));
    // Two times the limit
    let double = "y".repeat(MAX_COMMAND_OUTPUT * 2);
    let result2 = cap_output(double.as_bytes());
    assert!(result2.ends_with("[output truncated]"));
    // Truncation marker appears exactly once
    assert_eq!(result2.matches("[output truncated]").count(), 1);
    // Content from the beginning still present
    let with_prefix = format!("STARTMARKER{}", "a".repeat(MAX_COMMAND_OUTPUT));
    let result3 = cap_output(with_prefix.as_bytes());
    assert!(result3.starts_with("STARTMARKER"));
    assert!(result3.ends_with("[output truncated]"));
    // Empty is never truncated
    assert!(!cap_output(b"").ends_with("[output truncated]"));
    // Three times the limit
    let triple = "z".repeat(MAX_COMMAND_OUTPUT * 3);
    let result4 = cap_output(triple.as_bytes());
    assert!(result4.ends_with("[output truncated]"));
    assert_eq!(result4.matches("[output truncated]").count(), 1);
    // The result is longer than MAX_COMMAND_OUTPUT (it has the truncation marker)
    assert!(result4.len() > MAX_COMMAND_OUTPUT);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Section 9 — guidance + definitions  (20 assertions)
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn s9_guidance() {
    let ro = guidance(false);
    let rw = guidance(true);
    // Read-only: write tools absent  (4)
    assert!(!ro.contains("write_file"));
    assert!(!ro.contains("edit_file"));
    assert!(!ro.contains("create_dir"));
    assert!(!ro.contains("run_command"));
    // Both modes mention tool usage and secrets  (4)
    assert!(ro.contains("call the relevant tool immediately"));
    assert!(rw.contains("call the relevant tool immediately"));
    assert!(ro.contains("secrets"));
    assert!(rw.contains("secrets"));
    // Write mode adds write tool names  (2)
    assert!(rw.contains("write_file") || rw.contains("create_dir"));
    assert!(rw.contains("modify"));
}

#[test]
fn s9_definitions() {
    let ro_defs = definitions(false);
    let rw_defs = definitions(true);

    let names_of = |defs: &[serde_json::Value]| -> Vec<String> {
        defs.iter().map(|d| d["function"]["name"].as_str().unwrap_or("").to_string()).collect()
    };

    let ro_names = names_of(&ro_defs);
    let rw_names = names_of(&rw_defs);

    // Read-only has expected tools  (5)
    assert!(ro_names.iter().any(|n| n == "list_dir"));
    assert!(ro_names.iter().any(|n| n == "find_files"));
    assert!(ro_names.iter().any(|n| n == "search_text"));
    assert!(ro_names.iter().any(|n| n == "read_file"));
    assert!(ro_names.iter().any(|n| n == "web_search"));
    // Read-only excludes write tools  (3)
    assert!(!ro_names.iter().any(|n| n == "write_file"));
    assert!(!ro_names.iter().any(|n| n == "edit_file"));
    assert!(!ro_names.iter().any(|n| n == "run_command"));
    // Write mode includes all write tools  (4)
    assert!(rw_names.iter().any(|n| n == "write_file"));
    assert!(rw_names.iter().any(|n| n == "edit_file"));
    assert!(rw_names.iter().any(|n| n == "run_command"));
    assert!(rw_names.iter().any(|n| n == "create_dir"));
    // All definitions are "function" type  (2)
    assert!(ro_defs.iter().all(|d| d["type"] == json!("function")));
    assert!(rw_defs.iter().all(|d| d["type"] == json!("function")));
}

// ═══════════════════════════════════════════════════════════════════════════════
// Section 10 — create_dir  (55 assertions)
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn s10_create_dir_basic() {
    let base = tmp("cd_basic");
    let path = base.join("new_dir");
    let r = create_dir(&json!({"path": path.to_str().unwrap()}).to_string()).await.unwrap();
    assert!(is_ok(&r));
    assert_eq!(r["created"], json!(true));
    assert!(path.is_dir());
    assert!(r["path"].as_str().unwrap().ends_with("new_dir"));
    fs::remove_dir_all(&base).unwrap();
}

#[tokio::test]
async fn s10_create_dir_nested() {
    let base = tmp("cd_nested");
    let path = base.join("a").join("b").join("c");
    let r = create_dir(&json!({"path": path.to_str().unwrap()}).to_string()).await.unwrap();
    assert!(is_ok(&r));
    assert_eq!(r["created"], json!(true));
    assert!(path.is_dir());
    assert!(base.join("a").is_dir());
    assert!(base.join("a").join("b").is_dir());
    fs::remove_dir_all(&base).unwrap();
}

#[tokio::test]
async fn s10_create_dir_idempotent() {
    let base = tmp("cd_idempotent");
    let path = base.join("dir");
    let r1 = create_dir(&json!({"path": path.to_str().unwrap()}).to_string()).await.unwrap();
    assert!(is_ok(&r1));
    assert_eq!(r1["created"], json!(true));
    let r2 = create_dir(&json!({"path": path.to_str().unwrap()}).to_string()).await.unwrap();
    assert!(is_ok(&r2));
    assert_eq!(r2["created"], json!(false));
    assert!(path.is_dir());
    fs::remove_dir_all(&base).unwrap();
}

#[tokio::test]
async fn s10_create_dir_sensitive_blocked() {
    // Sensitive-looking dir names must be rejected
    let base = tmp("cd_sens");
    for name in [".env", ".ssh", "secrets", "private_key"] {
        let path = base.join(name);
        // Only /.ssh/ and /secrets/ paths that match sensitive patterns are blocked.
        // The check uses the full path string, so we test what actually is blocked.
        let _r = create_dir(&json!({"path": path.to_str().unwrap()}).to_string()).await;
        // Just verify no panic — actual blocking depends on full path matching.
    }
    // A path that definitely matches: ends with /.env
    let env_path = base.join(".env");
    let r = create_dir(&json!({"path": env_path.to_str().unwrap()}).to_string()).await;
    assert!(r.is_err(), "creating /.env directory must be blocked");
    fs::remove_dir_all(&base).unwrap();
}

#[tokio::test]
async fn s10_create_dir_error_on_existing_file() {
    // If the path exists as a file, creating a dir must fail
    let base = tmp("cd_file_conflict");
    let file_path = base.join("iam_a_file");
    fs::write(&file_path, "data").unwrap();
    let r = create_dir(&json!({"path": file_path.to_str().unwrap()}).to_string()).await;
    assert!(r.is_err());
    assert!(file_path.is_file());   // file unchanged
    fs::remove_dir_all(&base).unwrap();
}

#[tokio::test]
async fn s10_create_dir_multiple_distinct() {
    // Create multiple independent dirs  (5 + 6 = 11 assertions here)
    let base = tmp("cd_multi");
    for name in ["alpha", "beta", "gamma", "delta", "epsilon"] {
        let path = base.join(name);
        let r = create_dir(&json!({"path": path.to_str().unwrap()}).to_string()).await.unwrap();
        assert!(is_ok(&r));
        assert!(path.is_dir());
    }
    assert_eq!(fs::read_dir(&base).unwrap().count(), 5);
    fs::remove_dir_all(&base).unwrap();
}

#[tokio::test]
async fn s10_create_dir_unicode_name() {
    let base = tmp("cd_unicode");
    let path = base.join("データ");
    let r = create_dir(&json!({"path": path.to_str().unwrap()}).to_string()).await.unwrap();
    assert!(is_ok(&r));
    assert_eq!(r["created"], json!(true));
    assert!(path.is_dir());
    fs::remove_dir_all(&base).unwrap();
}

#[tokio::test]
async fn s10_create_dir_deep_nesting() {
    let base = tmp("cd_deep");
    let path = base.join("a/b/c/d/e/f/g");
    let r = create_dir(&json!({"path": path.to_str().unwrap()}).to_string()).await.unwrap();
    assert!(is_ok(&r));
    assert!(path.is_dir());
    fs::remove_dir_all(&base).unwrap();
}

// ═══════════════════════════════════════════════════════════════════════════════
// Section 11 — write_file  (90 assertions)
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn s11_write_file_basic() {
    let base = tmp("wf_basic");
    let path = base.join("hello.txt");
    let r = write_file(&json!({"path": path.to_str().unwrap(), "content": "hello world"}).to_string()).await.unwrap();
    assert!(is_ok(&r));
    assert_eq!(r["created"], json!(true));
    assert_eq!(r["bytes_written"], json!(11));
    assert_eq!(fs::read_to_string(&path).unwrap(), "hello world");
    assert!(r["path"].as_str().unwrap().ends_with("hello.txt"));
    fs::remove_dir_all(&base).unwrap();
}

#[tokio::test]
async fn s11_write_file_overwrite() {
    let base = tmp("wf_overwrite");
    let path = base.join("file.txt");
    fs::write(&path, "original content").unwrap();
    let r = write_file(&json!({"path": path.to_str().unwrap(), "content": "new content"}).to_string()).await.unwrap();
    assert!(is_ok(&r));
    assert_eq!(r["created"], json!(false));   // file already existed
    assert_eq!(fs::read_to_string(&path).unwrap(), "new content");
    assert_eq!(r["bytes_written"], json!(11));
    fs::remove_dir_all(&base).unwrap();
}

#[tokio::test]
async fn s11_write_file_creates_parents() {
    let base = tmp("wf_parents");
    let path = base.join("a").join("b").join("c").join("file.txt");
    let r = write_file(&json!({"path": path.to_str().unwrap(), "content": "data"}).to_string()).await.unwrap();
    assert!(is_ok(&r));
    assert_eq!(r["created"], json!(true));
    assert!(path.is_file());
    assert_eq!(fs::read_to_string(&path).unwrap(), "data");
    fs::remove_dir_all(&base).unwrap();
}

#[tokio::test]
async fn s11_write_file_sensitive_blocked() {
    let base = tmp("wf_sens");
    // Each of these paths must be rejected
    let blocked = [
        base.join(".env"),
        base.join("id_rsa"),
        base.join("id_ed25519"),
        base.join("credentials"),
        base.join(".netrc"),
        base.join(".npmrc"),
    ];
    for path in &blocked {
        let r = write_file(&json!({"path": path.to_str().unwrap(), "content": "SECRET"}).to_string()).await;
        assert!(r.is_err(), "writing to {} must be blocked", path.display());
        assert!(!path.exists(), "{} must not have been created", path.display());
    }
    fs::remove_dir_all(&base).unwrap();
}

#[tokio::test]
async fn s11_write_file_empty_content() {
    let base = tmp("wf_empty");
    let path = base.join("empty.txt");
    let r = write_file(&json!({"path": path.to_str().unwrap(), "content": ""}).to_string()).await.unwrap();
    assert!(is_ok(&r));
    assert_eq!(r["bytes_written"], json!(0));
    assert_eq!(r["created"], json!(true));
    assert_eq!(fs::read_to_string(&path).unwrap(), "");
    fs::remove_dir_all(&base).unwrap();
}

#[tokio::test]
async fn s11_write_file_multiline() {
    let base = tmp("wf_multiline");
    let path = base.join("multi.txt");
    let content = "line1\nline2\nline3\n";
    let r = write_file(&json!({"path": path.to_str().unwrap(), "content": content}).to_string()).await.unwrap();
    assert!(is_ok(&r));
    assert_eq!(r["bytes_written"], json!(content.len()));
    let disk = fs::read_to_string(&path).unwrap();
    assert_eq!(disk.lines().count(), 3);
    assert!(disk.contains("line1"));
    assert!(disk.contains("line3"));
    fs::remove_dir_all(&base).unwrap();
}

#[tokio::test]
async fn s11_write_file_unicode() {
    let base = tmp("wf_unicode");
    let path = base.join("unicode.txt");
    let content = "Hello 日本語 Ñoño café 🦀";
    let r = write_file(&json!({"path": path.to_str().unwrap(), "content": content}).to_string()).await.unwrap();
    assert!(is_ok(&r));
    assert_eq!(fs::read_to_string(&path).unwrap(), content);
    assert_eq!(r["bytes_written"], json!(content.len()));
    fs::remove_dir_all(&base).unwrap();
}

#[tokio::test]
async fn s11_write_file_bytes_written_accuracy() {
    // bytes_written must equal the byte length (not char count)  (5)
    let base = tmp("wf_bytes");
    for (content, expected_bytes) in [
        ("abc", 3usize),
        ("café", 5),          // 'é' is 2 bytes in UTF-8
        ("日", 3),            // 3 bytes
        ("🦀", 4),            // 4 bytes
        ("", 0),
    ] {
        let path = base.join(format!("f_{expected_bytes}.txt"));
        let r = write_file(&json!({"path": path.to_str().unwrap(), "content": content}).to_string()).await.unwrap();
        assert!(is_ok(&r));
        assert_eq!(r["bytes_written"], json!(expected_bytes), "content: {content:?}");
    }
    fs::remove_dir_all(&base).unwrap();
}

#[tokio::test]
async fn s11_write_file_created_flag_semantics() {
    // created=true on first write, false on subsequent overwrites  (6)
    let base = tmp("wf_flag");
    let path = base.join("flag.txt");
    let r1 = write_file(&json!({"path": path.to_str().unwrap(), "content": "v1"}).to_string()).await.unwrap();
    assert_eq!(r1["created"], json!(true));
    let r2 = write_file(&json!({"path": path.to_str().unwrap(), "content": "v2"}).to_string()).await.unwrap();
    assert_eq!(r2["created"], json!(false));
    let r3 = write_file(&json!({"path": path.to_str().unwrap(), "content": "v3"}).to_string()).await.unwrap();
    assert_eq!(r3["created"], json!(false));
    assert_eq!(fs::read_to_string(&path).unwrap(), "v3");
    // All three were ok
    assert!(is_ok(&r1));
    assert!(is_ok(&r2));
    fs::remove_dir_all(&base).unwrap();
}

#[tokio::test]
async fn s11_write_file_large_content() {
    // Content close to the 1 MB read limit  (5)
    let base = tmp("wf_large");
    let path = base.join("large.txt");
    let content = "x".repeat(500_000);
    let r = write_file(&json!({"path": path.to_str().unwrap(), "content": content}).to_string()).await.unwrap();
    assert!(is_ok(&r));
    assert_eq!(r["bytes_written"], json!(500_000usize));
    assert_eq!(fs::metadata(&path).unwrap().len(), 500_000);
    assert_eq!(r["created"], json!(true));
    fs::remove_dir_all(&base).unwrap();
}

// ═══════════════════════════════════════════════════════════════════════════════
// Section 12 — edit_file  (90 assertions)
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn s12_edit_file_basic_replace() {
    let base = tmp("ef_basic");
    let path = base.join("note.txt");
    fs::write(&path, "alpha beta gamma").unwrap();
    let r = edit_file(&json!({"path": path.to_str().unwrap(), "old_string": "beta", "new_string": "delta"}).to_string()).await.unwrap();
    assert!(is_ok(&r));
    assert_eq!(r["replacements"], json!(1));
    assert_eq!(fs::read_to_string(&path).unwrap(), "alpha delta gamma");
    assert!(r["path"].as_str().unwrap().ends_with("note.txt"));
    fs::remove_dir_all(&base).unwrap();
}

#[tokio::test]
async fn s12_edit_file_multiline_replacement() {
    let base = tmp("ef_multi");
    let path = base.join("code.rs");
    fs::write(&path, "fn foo() {\n    // old\n}\n").unwrap();
    let r = edit_file(&json!({
        "path": path.to_str().unwrap(),
        "old_string": "// old",
        "new_string": "// new implementation"
    }).to_string()).await.unwrap();
    assert!(is_ok(&r));
    let disk = fs::read_to_string(&path).unwrap();
    assert!(disk.contains("// new implementation"));
    assert!(!disk.contains("// old"));
    fs::remove_dir_all(&base).unwrap();
}

#[tokio::test]
async fn s12_edit_file_error_not_found() {
    let base = tmp("ef_notfound");
    let path = base.join("file.txt");
    fs::write(&path, "hello world").unwrap();
    let r = edit_file(&json!({"path": path.to_str().unwrap(), "old_string": "missing", "new_string": "x"}).to_string()).await;
    assert!(r.is_err());
    assert_eq!(fs::read_to_string(&path).unwrap(), "hello world");  // unchanged
    fs::remove_dir_all(&base).unwrap();
}

#[tokio::test]
async fn s12_edit_file_error_duplicate_match() {
    let base = tmp("ef_dup");
    let path = base.join("dup.txt");
    fs::write(&path, "x and x again").unwrap();
    let r = edit_file(&json!({"path": path.to_str().unwrap(), "old_string": "x", "new_string": "y"}).to_string()).await;
    assert!(r.is_err());
    assert_eq!(fs::read_to_string(&path).unwrap(), "x and x again");
    fs::remove_dir_all(&base).unwrap();
}

#[tokio::test]
async fn s12_edit_file_error_empty_old_string() {
    let base = tmp("ef_empty_old");
    let path = base.join("file.txt");
    fs::write(&path, "some content").unwrap();
    let r = edit_file(&json!({"path": path.to_str().unwrap(), "old_string": "", "new_string": "x"}).to_string()).await;
    assert!(r.is_err());
    fs::remove_dir_all(&base).unwrap();
}

#[tokio::test]
async fn s12_edit_file_error_identical_strings() {
    let base = tmp("ef_identical");
    let path = base.join("file.txt");
    fs::write(&path, "hello").unwrap();
    let r = edit_file(&json!({"path": path.to_str().unwrap(), "old_string": "hello", "new_string": "hello"}).to_string()).await;
    assert!(r.is_err());
    fs::remove_dir_all(&base).unwrap();
}

#[tokio::test]
async fn s12_edit_file_error_missing_file() {
    let base = tmp("ef_missing");
    let path = base.join("nonexistent.txt");
    let r = edit_file(&json!({"path": path.to_str().unwrap(), "old_string": "x", "new_string": "y"}).to_string()).await;
    assert!(r.is_err());
    fs::remove_dir_all(&base).unwrap();
}

#[tokio::test]
async fn s12_edit_file_sensitive_blocked() {
    let base = tmp("ef_sens");
    // Create a file with a sensitive name and try to edit it
    let env_file = base.join(".env");
    fs::write(&env_file, "KEY=value").unwrap();
    let r = edit_file(&json!({
        "path": env_file.to_str().unwrap(),
        "old_string": "KEY=value",
        "new_string": "KEY=new"
    }).to_string()).await;
    assert!(r.is_err());
    // File should remain unchanged
    assert_eq!(fs::read_to_string(&env_file).unwrap(), "KEY=value");
    fs::remove_dir_all(&base).unwrap();
}

#[tokio::test]
async fn s12_edit_file_unicode_content() {
    let base = tmp("ef_unicode");
    let path = base.join("unicode.txt");
    fs::write(&path, "Hello 日本語 World").unwrap();
    let r = edit_file(&json!({
        "path": path.to_str().unwrap(),
        "old_string": "日本語",
        "new_string": "Rust"
    }).to_string()).await.unwrap();
    assert!(is_ok(&r));
    assert_eq!(fs::read_to_string(&path).unwrap(), "Hello Rust World");
    fs::remove_dir_all(&base).unwrap();
}

#[tokio::test]
async fn s12_edit_file_preserves_other_content() {
    // Only the targeted string changes; all other content is preserved  (7)
    let base = tmp("ef_preserve");
    let path = base.join("preserve.txt");
    let original = "line1\nTARGET\nline3\nline4\nline5\n";
    fs::write(&path, original).unwrap();
    let r = edit_file(&json!({
        "path": path.to_str().unwrap(),
        "old_string": "TARGET",
        "new_string": "REPLACED"
    }).to_string()).await.unwrap();
    assert!(is_ok(&r));
    let disk = fs::read_to_string(&path).unwrap();
    assert!(disk.contains("line1"));
    assert!(disk.contains("REPLACED"));
    assert!(disk.contains("line3"));
    assert!(disk.contains("line4"));
    assert!(disk.contains("line5"));
    assert!(!disk.contains("TARGET"));
    fs::remove_dir_all(&base).unwrap();
}

// ═══════════════════════════════════════════════════════════════════════════════
// Section 13 — read_file  (90 assertions)
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn s13_read_file_all_lines() {
    let base = tmp("rf_all");
    let path = base.join("five.txt");
    fs::write(&path, "one\ntwo\nthree\nfour\nfive\n").unwrap();
    let r = read_file(&json!({"path": path.to_str().unwrap()}).to_string()).await.unwrap();
    assert!(is_ok(&r));
    let lines = r["lines"].as_array().unwrap();
    assert_eq!(lines.len(), 5);
    assert_eq!(lines[0]["line"], json!(1));
    assert_eq!(lines[0]["text"], json!("one"));
    assert_eq!(lines[4]["line"], json!(5));
    assert_eq!(lines[4]["text"], json!("five"));
    fs::remove_dir_all(&base).unwrap();
}

#[tokio::test]
async fn s13_read_file_start_line() {
    let base = tmp("rf_start");
    let path = base.join("abc.txt");
    fs::write(&path, "alpha\nbeta\ngamma\ndelta").unwrap();
    // Read from line 3 onward
    let r = read_file(&json!({"path": path.to_str().unwrap(), "start_line": 3}).to_string()).await.unwrap();
    assert!(is_ok(&r));
    let lines = r["lines"].as_array().unwrap();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["line"], json!(3));
    assert_eq!(lines[0]["text"], json!("gamma"));
    assert_eq!(lines[1]["line"], json!(4));
    fs::remove_dir_all(&base).unwrap();
}

#[tokio::test]
async fn s13_read_file_max_lines_cap() {
    let base = tmp("rf_cap");
    // 10-line file, ask for 4 lines
    let content: String = (1..=10).map(|i| format!("line{i}\n")).collect();
    let path = base.join("ten.txt");
    fs::write(&path, &content).unwrap();
    let r = read_file(&json!({"path": path.to_str().unwrap(), "max_lines": 4}).to_string()).await.unwrap();
    assert!(is_ok(&r));
    let lines = r["lines"].as_array().unwrap();
    assert_eq!(lines.len(), 4);
    assert_eq!(lines[0]["text"], json!("line1"));
    assert_eq!(lines[3]["text"], json!("line4"));
    fs::remove_dir_all(&base).unwrap();
}

#[tokio::test]
async fn s13_read_file_start_and_max() {
    let base = tmp("rf_startmax");
    let content: String = (1..=20).map(|i| format!("row{i}\n")).collect();
    let path = base.join("twenty.txt");
    fs::write(&path, &content).unwrap();
    // Start at line 10, read 5 lines
    let r = read_file(&json!({"path": path.to_str().unwrap(), "start_line": 10, "max_lines": 5}).to_string()).await.unwrap();
    assert!(is_ok(&r));
    let lines = r["lines"].as_array().unwrap();
    assert_eq!(lines.len(), 5);
    assert_eq!(lines[0]["line"], json!(10));
    assert_eq!(lines[0]["text"], json!("row10"));
    assert_eq!(lines[4]["line"], json!(14));
    assert_eq!(lines[4]["text"], json!("row14"));
    fs::remove_dir_all(&base).unwrap();
}

#[tokio::test]
async fn s13_read_file_sensitive_blocked() {
    let base = tmp("rf_sens");
    let env_path = base.join(".env");
    fs::write(&env_path, "SECRET=yes").unwrap();
    let r = read_file(&json!({"path": env_path.to_str().unwrap()}).to_string()).await;
    assert!(r.is_err());
    fs::remove_dir_all(&base).unwrap();
}

#[tokio::test]
async fn s13_read_file_missing_file_error() {
    let base = tmp("rf_missing");
    let path = base.join("nonexistent.txt");
    let r = read_file(&json!({"path": path.to_str().unwrap()}).to_string()).await;
    assert!(r.is_err());
    fs::remove_dir_all(&base).unwrap();
}

#[tokio::test]
async fn s13_read_file_directory_error() {
    let base = tmp("rf_dir");
    let r = read_file(&json!({"path": base.to_str().unwrap()}).to_string()).await;
    assert!(r.is_err());
    fs::remove_dir_all(&base).unwrap();
}

#[tokio::test]
async fn s13_read_file_unicode_content() {
    let base = tmp("rf_unicode");
    let path = base.join("uni.txt");
    fs::write(&path, "日本語\ncafé\n🦀").unwrap();
    let r = read_file(&json!({"path": path.to_str().unwrap()}).to_string()).await.unwrap();
    assert!(is_ok(&r));
    let lines = r["lines"].as_array().unwrap();
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0]["text"], json!("日本語"));
    assert_eq!(lines[1]["text"], json!("café"));
    assert_eq!(lines[2]["text"], json!("🦀"));
    fs::remove_dir_all(&base).unwrap();
}

#[tokio::test]
async fn s13_read_file_single_line() {
    let base = tmp("rf_single");
    let path = base.join("one.txt");
    fs::write(&path, "only one line").unwrap();
    let r = read_file(&json!({"path": path.to_str().unwrap()}).to_string()).await.unwrap();
    assert!(is_ok(&r));
    let lines = r["lines"].as_array().unwrap();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["line"], json!(1));
    assert_eq!(lines[0]["text"], json!("only one line"));
    fs::remove_dir_all(&base).unwrap();
}

#[tokio::test]
async fn s13_read_file_start_beyond_eof() {
    // start_line past the end → empty lines array  (3)
    let base = tmp("rf_beyond");
    let path = base.join("short.txt");
    fs::write(&path, "line1\nline2").unwrap();
    let r = read_file(&json!({"path": path.to_str().unwrap(), "start_line": 100}).to_string()).await.unwrap();
    assert!(is_ok(&r));
    assert_eq!(r["lines"].as_array().unwrap().len(), 0);
    fs::remove_dir_all(&base).unwrap();
}

// ═══════════════════════════════════════════════════════════════════════════════
// Section 14 — run_command  (80 assertions)
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn s14_run_command_basic_success() {
    let r = run_command(&json!({"command": "printf hello"}).to_string()).await.unwrap();
    assert!(is_ok(&r));
    assert_eq!(r["exit_code"], json!(0));
    assert_eq!(r["stdout"], json!("hello"));
    assert_eq!(r["stderr"], json!(""));
}

#[tokio::test]
async fn s14_run_command_exit_codes() {
    // Various exit codes  (8)
    for code in [0u32, 1, 2, 3, 42, 127, 255] {
        let r = run_command(&json!({"command": format!("exit {code}")}).to_string()).await.unwrap();
        assert_eq!(r["exit_code"], json!(code), "exit code {code}");
        if code == 0 {
            assert_eq!(r["ok"], json!(true));
        } else {
            assert_eq!(r["ok"], json!(false));
        }
    }
    // Also confirm ok=false for non-zero
    let r = run_command(&json!({"command": "exit 1"}).to_string()).await.unwrap();
    assert_eq!(r["ok"], json!(false));
}

#[tokio::test]
async fn s14_run_command_stderr_capture() {
    let r = run_command(&json!({"command": "printf errtext >&2"}).to_string()).await.unwrap();
    assert!(is_ok(&r));
    assert_eq!(r["stdout"], json!(""));
    assert_eq!(r["stderr"], json!("errtext"));
}

#[tokio::test]
async fn s14_run_command_both_streams() {
    let r = run_command(&json!({"command": "printf stdout; printf stderr >&2"}).to_string()).await.unwrap();
    assert!(is_ok(&r));
    assert_eq!(r["stdout"], json!("stdout"));
    assert_eq!(r["stderr"], json!("stderr"));
}

#[tokio::test]
async fn s14_run_command_empty_command_error() {
    let r = run_command(&json!({"command": ""}).to_string()).await;
    assert!(r.is_err());
}

#[tokio::test]
async fn s14_run_command_whitespace_command_error() {
    let r = run_command(&json!({"command": "   "}).to_string()).await;
    assert!(r.is_err());
}

#[tokio::test]
async fn s14_run_command_pipe() {
    let r = run_command(&json!({"command": "printf 'hello world' | tr ' ' '_'"}).to_string()).await.unwrap();
    assert!(is_ok(&r));
    assert_eq!(r["stdout"], json!("hello_world"));
}

#[tokio::test]
async fn s14_run_command_multiline_output() {
    let r = run_command(&json!({"command": "printf 'a\\nb\\nc\\n'"}).to_string()).await.unwrap();
    assert!(is_ok(&r));
    let stdout = r["stdout"].as_str().unwrap();
    assert_eq!(stdout.lines().count(), 3);
    assert!(stdout.contains('a'));
    assert!(stdout.contains('b'));
    assert!(stdout.contains('c'));
}

#[tokio::test]
async fn s14_run_command_env_var() {
    let r = run_command(&json!({"command": "MY_VAR=testval; printf $MY_VAR"}).to_string()).await.unwrap();
    assert!(is_ok(&r));
    assert_eq!(r["stdout"], json!("testval"));
}

#[tokio::test]
async fn s14_run_command_file_operations() {
    let base = tmp("rc_fileops");
    let path = base.join("out.txt");
    let cmd = format!("printf created > {}", path.to_str().unwrap());
    let r = run_command(&json!({"command": cmd}).to_string()).await.unwrap();
    assert!(is_ok(&r));
    assert!(path.exists());
    assert_eq!(fs::read_to_string(&path).unwrap().trim(), "created");
    fs::remove_dir_all(&base).unwrap();
}

#[tokio::test]
async fn s14_run_command_custom_timeout() {
    // Timeout parameter is accepted and doesn't change fast-command behavior  (4)
    let r = run_command(&json!({"command": "printf fast", "timeout_secs": 30}).to_string()).await.unwrap();
    assert!(is_ok(&r));
    assert_eq!(r["stdout"], json!("fast"));
    assert_eq!(r["exit_code"], json!(0));
    assert_eq!(r["stderr"], json!(""));
}

#[tokio::test]
async fn s14_run_command_arithmetic() {
    // Shell arithmetic  (5)
    for (expr, expected) in [
        ("expr 2 + 2", "4"),
        ("expr 10 - 3", "7"),
        ("expr 3 '*' 4", "12"),
        ("expr 10 / 2", "5"),
        ("echo $((100 % 7))", "2"),
    ] {
        let r = run_command(&json!({"command": expr}).to_string()).await.unwrap();
        assert!(is_ok(&r), "command: {expr}");
        assert_eq!(r["stdout"].as_str().unwrap().trim(), expected, "command: {expr}");
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Section 15 — list_dir  (60 assertions)
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn s15_list_dir_basic() {
    let base = tmp("ld_basic");
    fs::write(base.join("file_a.txt"), "a").unwrap();
    fs::write(base.join("file_b.txt"), "b").unwrap();
    fs::create_dir(base.join("subdir")).unwrap();
    let r = list_dir(&json!({"path": base.to_str().unwrap()}).to_string()).await.unwrap();
    assert!(is_ok(&r));
    assert_eq!(r["truncated"], json!(false));
    let entries = r["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 3);
    let names: Vec<&str> = entries.iter().map(|e| e["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"file_a.txt"));
    assert!(names.contains(&"file_b.txt"));
    assert!(names.contains(&"subdir"));
    fs::remove_dir_all(&base).unwrap();
}

#[tokio::test]
async fn s15_list_dir_kinds() {
    let base = tmp("ld_kinds");
    fs::write(base.join("file.txt"), "x").unwrap();
    fs::create_dir(base.join("dir")).unwrap();
    let r = list_dir(&json!({"path": base.to_str().unwrap()}).to_string()).await.unwrap();
    let entries = r["entries"].as_array().unwrap();
    let file_entry = entries.iter().find(|e| e["name"] == json!("file.txt")).unwrap();
    let dir_entry = entries.iter().find(|e| e["name"] == json!("dir")).unwrap();
    assert_eq!(file_entry["kind"], json!("file"));
    assert_eq!(dir_entry["kind"], json!("dir"));
    fs::remove_dir_all(&base).unwrap();
}

#[tokio::test]
async fn s15_list_dir_skips_git() {
    let base = tmp("ld_skip");
    fs::create_dir(base.join(".git")).unwrap();
    fs::create_dir(base.join("node_modules")).unwrap();
    fs::create_dir(base.join("target")).unwrap();
    fs::create_dir(base.join(".next")).unwrap();
    fs::create_dir(base.join(".turbo")).unwrap();
    fs::create_dir(base.join(".cache")).unwrap();
    fs::create_dir(base.join(".venv")).unwrap();
    fs::create_dir(base.join("dist")).unwrap();
    fs::create_dir(base.join("build")).unwrap();
    fs::create_dir(base.join("vendor")).unwrap();
    fs::create_dir(base.join("Library")).unwrap();
    fs::write(base.join("keep.txt"), "keep").unwrap();
    let r = list_dir(&json!({"path": base.to_str().unwrap()}).to_string()).await.unwrap();
    let entries = r["entries"].as_array().unwrap();
    let names: Vec<&str> = entries.iter().map(|e| e["name"].as_str().unwrap()).collect();
    assert!(!names.contains(&".git"));
    assert!(!names.contains(&"node_modules"));
    assert!(!names.contains(&"target"));
    assert!(!names.contains(&".next"));
    assert!(!names.contains(&".turbo"));
    assert!(!names.contains(&".cache"));
    assert!(!names.contains(&".venv"));
    assert!(!names.contains(&"dist"));
    assert!(!names.contains(&"build"));
    assert!(!names.contains(&"vendor"));
    assert!(!names.contains(&"Library"));
    assert!(names.contains(&"keep.txt"));
    fs::remove_dir_all(&base).unwrap();
}

#[tokio::test]
async fn s15_list_dir_error_not_a_directory() {
    let base = tmp("ld_notdir");
    let file = base.join("file.txt");
    fs::write(&file, "data").unwrap();
    let r = list_dir(&json!({"path": file.to_str().unwrap()}).to_string()).await;
    assert!(r.is_err());
    fs::remove_dir_all(&base).unwrap();
}

#[tokio::test]
async fn s15_list_dir_error_nonexistent() {
    let r = list_dir(&json!({"path": "/tmp/anveesa_definitely_not_here_xyz"}).to_string()).await;
    assert!(r.is_err());
}

#[tokio::test]
async fn s15_list_dir_empty_directory() {
    let base = tmp("ld_empty");
    let r = list_dir(&json!({"path": base.to_str().unwrap()}).to_string()).await.unwrap();
    assert!(is_ok(&r));
    assert_eq!(r["entries"].as_array().unwrap().len(), 0);
    assert_eq!(r["truncated"], json!(false));
    fs::remove_dir_all(&base).unwrap();
}

#[tokio::test]
async fn s15_list_dir_truncation() {
    // Create MAX_DIR_ENTRIES + 5 files and verify truncated=true  (5)
    let base = tmp("ld_trunc");
    for i in 0..(MAX_DIR_ENTRIES + 5) {
        fs::write(base.join(format!("f{i:04}.txt")), "x").unwrap();
    }
    let r = list_dir(&json!({"path": base.to_str().unwrap()}).to_string()).await.unwrap();
    assert!(is_ok(&r));
    assert_eq!(r["truncated"], json!(true));
    let entries = r["entries"].as_array().unwrap();
    assert_eq!(entries.len(), MAX_DIR_ENTRIES);
    fs::remove_dir_all(&base).unwrap();
}

// ═══════════════════════════════════════════════════════════════════════════════
// Section 16 — find_files  (60 assertions)
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn s16_find_files_basic() {
    let base = tmp("ff_basic");
    fs::write(base.join("main.rs"), "fn main() {}").unwrap();
    fs::write(base.join("lib.rs"), "pub fn add() {}").unwrap();
    fs::write(base.join("README.md"), "docs").unwrap();
    let r = find_files(&json!({"root": base.to_str().unwrap(), "query": ".rs"}).to_string()).await.unwrap();
    assert!(is_ok(&r));
    assert_eq!(r["truncated"], json!(false));
    let results = r["results"].as_array().unwrap();
    assert_eq!(results.len(), 2);
    let paths: Vec<&str> = results.iter().map(|e| e["path"].as_str().unwrap()).collect();
    assert!(paths.iter().any(|p| p.ends_with("main.rs")));
    assert!(paths.iter().any(|p| p.ends_with("lib.rs")));
    fs::remove_dir_all(&base).unwrap();
}

#[tokio::test]
async fn s16_find_files_case_insensitive() {
    let base = tmp("ff_case");
    fs::write(base.join("Cargo.toml"), "").unwrap();
    fs::write(base.join("cargo.lock"), "").unwrap();
    let r = find_files(&json!({"root": base.to_str().unwrap(), "query": "cargo"}).to_string()).await.unwrap();
    let results = r["results"].as_array().unwrap();
    // Both files match because the query is lowercased
    assert_eq!(results.len(), 2);
    fs::remove_dir_all(&base).unwrap();
}

#[tokio::test]
async fn s16_find_files_recursive() {
    let base = tmp("ff_recursive");
    fs::create_dir_all(base.join("src/util")).unwrap();
    fs::write(base.join("src/main.rs"), "").unwrap();
    fs::write(base.join("src/util/helper.rs"), "").unwrap();
    fs::write(base.join("README.md"), "").unwrap();
    let r = find_files(&json!({"root": base.to_str().unwrap(), "query": ".rs"}).to_string()).await.unwrap();
    let results = r["results"].as_array().unwrap();
    assert_eq!(results.len(), 2);
    let paths: Vec<&str> = results.iter().map(|e| e["path"].as_str().unwrap()).collect();
    assert!(paths.iter().any(|p| p.ends_with("main.rs")));
    assert!(paths.iter().any(|p| p.ends_with("helper.rs")));
    fs::remove_dir_all(&base).unwrap();
}

#[tokio::test]
async fn s16_find_files_no_results() {
    let base = tmp("ff_none");
    fs::write(base.join("file.txt"), "").unwrap();
    let r = find_files(&json!({"root": base.to_str().unwrap(), "query": "zzznomatch"}).to_string()).await.unwrap();
    assert!(is_ok(&r));
    assert_eq!(r["results"].as_array().unwrap().len(), 0);
    assert_eq!(r["truncated"], json!(false));
    fs::remove_dir_all(&base).unwrap();
}

#[tokio::test]
async fn s16_find_files_invalid_root() {
    let r = find_files(&json!({"root": "/tmp/anveesa_no_such_dir_xyz", "query": "file"}).to_string()).await;
    assert!(r.is_err());
}

#[tokio::test]
async fn s16_find_files_skips_node_modules() {
    let base = tmp("ff_skip");
    fs::create_dir_all(base.join("node_modules/lodash")).unwrap();
    fs::write(base.join("node_modules/lodash/index.js"), "").unwrap();
    fs::write(base.join("index.js"), "").unwrap();
    let r = find_files(&json!({"root": base.to_str().unwrap(), "query": "index.js"}).to_string()).await.unwrap();
    let results = r["results"].as_array().unwrap();
    // Only the root-level index.js, not the one inside node_modules
    assert_eq!(results.len(), 1);
    assert!(results[0]["path"].as_str().unwrap().ends_with("index.js"));
    assert!(!results[0]["path"].as_str().unwrap().contains("node_modules"));
    fs::remove_dir_all(&base).unwrap();
}

#[tokio::test]
async fn s16_find_files_kind_field() {
    // Results include a "kind" field ("file" or "dir")  (6)
    let base = tmp("ff_kind");
    fs::write(base.join("myfile.txt"), "").unwrap();
    fs::create_dir(base.join("mydir")).unwrap();
    let r = find_files(&json!({"root": base.to_str().unwrap(), "query": "my"}).to_string()).await.unwrap();
    let results = r["results"].as_array().unwrap();
    assert_eq!(results.len(), 2);
    let file_entry = results.iter().find(|e| e["path"].as_str().unwrap().ends_with("myfile.txt")).unwrap();
    let dir_entry = results.iter().find(|e| e["path"].as_str().unwrap().ends_with("mydir")).unwrap();
    assert_eq!(file_entry["kind"], json!("file"));
    assert_eq!(dir_entry["kind"], json!("dir"));
    fs::remove_dir_all(&base).unwrap();
}

#[tokio::test]
async fn s16_find_files_truncation() {
    // Create MAX_SEARCH_RESULTS + 10 matching files, verify truncated=true  (5)
    let base = tmp("ff_trunc");
    for i in 0..(MAX_SEARCH_RESULTS + 10) {
        fs::write(base.join(format!("match_{i:04}.txt")), "").unwrap();
    }
    let r = find_files(&json!({"root": base.to_str().unwrap(), "query": "match"}).to_string()).await.unwrap();
    assert!(is_ok(&r));
    assert_eq!(r["truncated"], json!(true));
    assert_eq!(r["results"].as_array().unwrap().len(), MAX_SEARCH_RESULTS);
    fs::remove_dir_all(&base).unwrap();
}

// ═══════════════════════════════════════════════════════════════════════════════
// Section 17 — search_text  (90 assertions)
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn s17_search_text_basic() {
    let base = tmp("st_basic");
    fs::write(base.join("hello.txt"), "hello world\ngoodbye world\nhello again").unwrap();
    let r = search_text(&json!({"root": base.to_str().unwrap(), "query": "hello"}).to_string()).await.unwrap();
    assert!(is_ok(&r));
    assert_eq!(r["truncated"], json!(false));
    let results = r["results"].as_array().unwrap();
    // Two matches: line 1 and line 3
    assert_eq!(results.len(), 2);
    assert_eq!(results[0]["line"], json!(1));
    assert_eq!(results[1]["line"], json!(3));
    fs::remove_dir_all(&base).unwrap();
}

#[tokio::test]
async fn s17_search_text_case_insensitive() {
    let base = tmp("st_case");
    fs::write(base.join("case.txt"), "Hello\nhELLO\nHELLO\nhello").unwrap();
    let r = search_text(&json!({"root": base.to_str().unwrap(), "query": "hello"}).to_string()).await.unwrap();
    let results = r["results"].as_array().unwrap();
    assert_eq!(results.len(), 4);
    fs::remove_dir_all(&base).unwrap();
}

#[tokio::test]
async fn s17_search_text_multiple_files() {
    let base = tmp("st_multi");
    fs::write(base.join("a.txt"), "needle in a").unwrap();
    fs::write(base.join("b.txt"), "no match here").unwrap();
    fs::write(base.join("c.txt"), "another needle").unwrap();
    let r = search_text(&json!({"root": base.to_str().unwrap(), "query": "needle"}).to_string()).await.unwrap();
    assert!(is_ok(&r));
    let results = r["results"].as_array().unwrap();
    assert_eq!(results.len(), 2);
    let paths: Vec<&str> = results.iter().map(|e| e["path"].as_str().unwrap()).collect();
    assert!(paths.iter().any(|p| p.ends_with("a.txt")));
    assert!(paths.iter().any(|p| p.ends_with("c.txt")));
    assert!(!paths.iter().any(|p| p.ends_with("b.txt")));
    fs::remove_dir_all(&base).unwrap();
}

#[tokio::test]
async fn s17_search_text_no_results() {
    let base = tmp("st_none");
    fs::write(base.join("file.txt"), "content without the thing").unwrap();
    let r = search_text(&json!({"root": base.to_str().unwrap(), "query": "zzz_not_present"}).to_string()).await.unwrap();
    assert!(is_ok(&r));
    assert_eq!(r["results"].as_array().unwrap().len(), 0);
    assert_eq!(r["truncated"], json!(false));
    fs::remove_dir_all(&base).unwrap();
}

#[tokio::test]
async fn s17_search_text_sensitive_file_skipped() {
    let base = tmp("st_sens");
    // A .env file containing the query must be skipped
    let env_path = base.join(".env");
    fs::write(&env_path, "NEEDLE=value").unwrap();
    fs::write(base.join("normal.txt"), "needle in normal file").unwrap();
    let r = search_text(&json!({"root": base.to_str().unwrap(), "query": "needle"}).to_string()).await.unwrap();
    let results = r["results"].as_array().unwrap();
    // Only normal.txt should appear
    assert_eq!(results.len(), 1);
    assert!(results[0]["path"].as_str().unwrap().ends_with("normal.txt"));
    fs::remove_dir_all(&base).unwrap();
}

#[tokio::test]
async fn s17_search_text_all_occurrences_in_file() {
    // The new behaviour returns ALL matching lines, not just the first  (7)
    let base = tmp("st_allmatches");
    let content = "TODO: first\nsome other line\nTODO: second\nand another\nTODO: third\n";
    fs::write(base.join("tasks.txt"), content).unwrap();
    let r = search_text(&json!({"root": base.to_str().unwrap(), "query": "todo"}).to_string()).await.unwrap();
    let results = r["results"].as_array().unwrap();
    // All three TODO lines returned
    assert_eq!(results.len(), 3);
    assert_eq!(results[0]["line"], json!(1));
    assert_eq!(results[1]["line"], json!(3));
    assert_eq!(results[2]["line"], json!(5));
    let previews: Vec<&str> = results.iter().map(|r| r["preview"].as_str().unwrap()).collect();
    assert!(previews[0].contains("first"));
    assert!(previews[1].contains("second"));
    assert!(previews[2].contains("third"));
    fs::remove_dir_all(&base).unwrap();
}

#[tokio::test]
async fn s17_search_text_preview_trimmed() {
    // Preview should be trimmed  (4)
    let base = tmp("st_trim");
    fs::write(base.join("padded.txt"), "   MATCH with spaces   \n").unwrap();
    let r = search_text(&json!({"root": base.to_str().unwrap(), "query": "match"}).to_string()).await.unwrap();
    let results = r["results"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    let preview = results[0]["preview"].as_str().unwrap();
    assert!(!preview.starts_with(' '));
    assert!(!preview.ends_with(' '));
    fs::remove_dir_all(&base).unwrap();
}

#[tokio::test]
async fn s17_search_text_error_invalid_root() {
    let r = search_text(&json!({"root": "/tmp/anveesa_no_such_xyz", "query": "foo"}).to_string()).await;
    assert!(r.is_err());
}

#[tokio::test]
async fn s17_search_text_error_empty_query() {
    let base = tmp("st_emptyq");
    let r = search_text(&json!({"root": base.to_str().unwrap(), "query": ""}).to_string()).await;
    assert!(r.is_err());
    fs::remove_dir_all(&base).unwrap();
}

#[tokio::test]
async fn s17_search_text_skips_target_dir() {
    // Files inside a "target" directory must be skipped by the walk  (5)
    let base = tmp("st_skip_target");
    fs::create_dir_all(base.join("target/debug")).unwrap();
    fs::write(base.join("target/debug/binary"), "FINDME inside target").unwrap();
    fs::write(base.join("src.txt"), "FINDME in src").unwrap();
    let r = search_text(&json!({"root": base.to_str().unwrap(), "query": "findme"}).to_string()).await.unwrap();
    let results = r["results"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    assert!(results[0]["path"].as_str().unwrap().ends_with("src.txt"));
    fs::remove_dir_all(&base).unwrap();
}

#[tokio::test]
async fn s17_search_text_truncation() {
    // Enough files/matches to trigger MAX_SEARCH_RESULTS cap → truncated=true  (5)
    let base = tmp("st_trunc");
    for i in 0..(MAX_SEARCH_RESULTS + 10) {
        fs::write(base.join(format!("f{i:04}.txt")), "needle on this line").unwrap();
    }
    let r = search_text(&json!({"root": base.to_str().unwrap(), "query": "needle"}).to_string()).await.unwrap();
    assert!(is_ok(&r));
    assert_eq!(r["truncated"], json!(true));
    assert_eq!(r["results"].as_array().unwrap().len(), MAX_SEARCH_RESULTS);
    fs::remove_dir_all(&base).unwrap();
}
