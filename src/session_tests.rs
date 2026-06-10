//! Tests for session.rs

use crate::session::*;
use std::path::Path;

// ============ cwd_session_hash tests ============

#[test]
fn cwd_session_hash_basic() {
    let h = cwd_session_hash(Path::new("/project"));
    assert_eq!(h.len(), 16);
    assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn cwd_session_hash_consistent() {
    let h1 = cwd_session_hash(Path::new("/home/user/project"));
    let h2 = cwd_session_hash(Path::new("/home/user/project"));
    assert_eq!(h1, h2);
}

#[test]
fn cwd_session_hash_different_paths() {
    let h1 = cwd_session_hash(Path::new("/project1"));
    let h2 = cwd_session_hash(Path::new("/project2"));
    assert_ne!(h1, h2);
}

#[test]
fn cwd_session_hash_current_dir() {
    let h = cwd_session_hash(Path::new("."));
    assert_eq!(h.len(), 16);
}

#[test]
fn cwd_session_hash_root() {
    let h = cwd_session_hash(Path::new("/"));
    assert_eq!(h.len(), 16);
}

#[test]
fn cwd_session_hash_long_path() {
    let long = "/".to_string() + &"a/".repeat(100);
    let h = cwd_session_hash(Path::new(&long));
    assert_eq!(h.len(), 16);
}

#[test]
fn cwd_session_hash_unicode_path() {
    let h = cwd_session_hash(Path::new("/プロジェクト"));
    assert_eq!(h.len(), 16);
}

#[test]
fn cwd_session_hash_special_chars() {
    let h = cwd_session_hash(Path::new("/project with spaces & symbols"));
    assert_eq!(h.len(), 16);
}

#[test]
fn cwd_session_hash_empty() {
    let h = cwd_session_hash(Path::new(""));
    assert_eq!(h.len(), 16);
}

#[test]
fn cwd_session_hash_hidden_dir() {
    let h = cwd_session_hash(Path::new("/.hidden"));
    assert_eq!(h.len(), 16);
}

#[test]
fn cwd_session_hash_deeply_nested() {
    let h = cwd_session_hash(Path::new("/a/b/c/d/e/f/g/h/i/j"));
    assert_eq!(h.len(), 16);
}

#[test]
fn cwd_session_hash_with_dots() {
    let h1 = cwd_session_hash(Path::new("./foo"));
    let h2 = cwd_session_hash(Path::new("foo"));
    assert_eq!(h1.len(), 16);
    assert_eq!(h2.len(), 16);
}

#[test]
fn cwd_session_hash_symlink_not_followed() {
    let h = cwd_session_hash(Path::new("/symlink/path"));
    assert_eq!(h.len(), 16);
}

#[test]
fn cwd_session_hash_fstab_prefix() {
    let h = cwd_session_hash(Path::new("/dev/fd/0"));
    assert_eq!(h.len(), 16);
}

#[test]
fn cwd_session_hash_home_expanded() {
    let h = cwd_session_hash(Path::new("/home/user"));
    assert_eq!(h.len(), 16);
}

#[test]
fn cwd_session_hash_tabs() {
    let h = cwd_session_hash(Path::new("/project\twith\ttabs"));
    assert_eq!(h.len(), 16);
}

#[test]
fn cwd_session_hash_newlines() {
    let h = cwd_session_hash(Path::new("/project\nwith\nnewlines"));
    assert_eq!(h.len(), 16);
}

#[test]
fn cwd_session_hash_null_byte() {
    let h = cwd_session_hash(Path::new("/project\x00with\x00nulls"));
    assert_eq!(h.len(), 16);
}

#[test]
fn cwd_session_hash_percent_encoding() {
    let h = cwd_session_hash(Path::new("/project%20with%20encoding"));
    assert_eq!(h.len(), 16);
}

#[test]
fn cwd_session_hash_url_like() {
    let h = cwd_session_hash(Path::new("https://example.com/path"));
    assert_eq!(h.len(), 16);
}

// ============ format_session_age tests ============

#[test]
fn format_session_age_none() {
    assert_eq!(format_session_age(None), "unknown age");
}

#[test]
fn format_session_age_zero() {
    let now = crate::unix_now();
    assert_eq!(format_session_age(Some(now)), "just now");
}

#[test]
fn format_session_age_30_seconds() {
    let now = crate::unix_now();
    let age = format_session_age(Some(now - 30));
    assert_eq!(age, "just now");
}

#[test]
fn format_session_age_59_seconds() {
    let now = crate::unix_now();
    let age = format_session_age(Some(now - 59));
    assert_eq!(age, "just now");
}

#[test]
fn format_session_age_1_minute() {
    let now = crate::unix_now();
    let age = format_session_age(Some(now - 60));
    assert_eq!(age, "1m ago");
}

#[test]
fn format_session_age_30_minutes() {
    let now = crate::unix_now();
    let age = format_session_age(Some(now - 1800));
    assert_eq!(age, "30m ago");
}

#[test]
fn format_session_age_59_minutes() {
    let now = crate::unix_now();
    let age = format_session_age(Some(now - 3540));
    assert_eq!(age, "59m ago");
}

#[test]
fn format_session_age_1_hour() {
    let now = crate::unix_now();
    let age = format_session_age(Some(now - 3600));
    assert_eq!(age, "1h ago");
}

#[test]
fn format_session_age_23_hours() {
    let now = crate::unix_now();
    let age = format_session_age(Some(now - 82800));
    assert_eq!(age, "23h ago");
}

#[test]
fn format_session_age_1_day() {
    let now = crate::unix_now();
    let age = format_session_age(Some(now - 86400));
    assert_eq!(age, "1d ago");
}

#[test]
fn format_session_age_30_days() {
    let now = crate::unix_now();
    let age = format_session_age(Some(now - 2592000));
    assert_eq!(age, "30d ago");
}

#[test]
fn format_session_age_365_days() {
    let now = crate::unix_now();
    let age = format_session_age(Some(now - 31536000));
    assert_eq!(age, "365d ago");
}

#[test]
fn format_session_age_1000_days() {
    let now = crate::unix_now();
    let age = format_session_age(Some(now - 86400000));
    assert_eq!(age, "1000d ago");
}

#[test]
fn format_session_age_very_old() {
    let now = crate::unix_now();
    let age = format_session_age(Some(now.saturating_sub(2000000000)));
    assert!(age.ends_with("d ago"));
}

#[test]
fn format_session_age_epoch() {
    let age = format_session_age(Some(0));
    assert!(age.ends_with("d ago"));
}

#[test]
fn format_session_age_boundary_59_sec() {
    let now = crate::unix_now();
    let age = format_session_age(Some(now - 59));
    assert_eq!(age, "just now");
}

#[test]
fn format_session_age_boundary_60_sec() {
    let now = crate::unix_now();
    let age = format_session_age(Some(now - 60));
    assert_eq!(age, "1m ago");
}

#[test]
fn format_session_age_boundary_59_min() {
    let now = crate::unix_now();
    let age = format_session_age(Some(now - 3539));
    assert_eq!(age, "58m ago");
}

#[test]
fn format_session_age_boundary_60_min() {
    let now = crate::unix_now();
    let age = format_session_age(Some(now - 3600));
    assert_eq!(age, "1h ago");
}

#[test]
fn format_session_age_boundary_23h() {
    let now = crate::unix_now();
    let age = format_session_age(Some(now - 86399));
    assert_eq!(age, "23h ago");
}

#[test]
fn format_session_age_boundary_24h() {
    let now = crate::unix_now();
    let age = format_session_age(Some(now - 86400));
    assert_eq!(age, "1d ago");
}

#[test]
fn format_session_age_future() {
    let now = crate::unix_now();
    let age = format_session_age(Some(now + 100));
    assert_eq!(age, "just now");
}

#[test]
fn format_session_age_future_huge() {
    let age = format_session_age(Some(u64::MAX));
    assert_eq!(age, "just now");
}

#[test]
fn format_session_age_2_minutes() {
    let now = crate::unix_now();
    let age = format_session_age(Some(now - 120));
    assert_eq!(age, "2m ago");
}

#[test]
fn format_session_age_2_hours() {
    let now = crate::unix_now();
    let age = format_session_age(Some(now - 7200));
    assert_eq!(age, "2h ago");
}

#[test]
fn format_session_age_2_days() {
    let now = crate::unix_now();
    let age = format_session_age(Some(now - 172800));
    assert_eq!(age, "2d ago");
}

// ============ append_repl_history tests ============

#[test]
fn append_repl_history_creates_file() {
    let dir = std::env::temp_dir().join(format!(
        "anveesa_test_{}_{}_{}",
        crate::unix_now(),
        std::process::id(),
        line!()
    ));
    let path = dir.join("history.txt");
    append_repl_history(&path, "test prompt").unwrap();
    let content = std::fs::read_to_string(&path).unwrap();
    assert_eq!(content, "test prompt\n");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn append_repl_history_appends() {
    let dir = std::env::temp_dir().join(format!(
        "anveesa_test_{}_{}_{}",
        crate::unix_now(),
        std::process::id(),
        line!()
    ));
    let path = dir.join("history.txt");
    append_repl_history(&path, "first").unwrap();
    append_repl_history(&path, "second").unwrap();
    let content = std::fs::read_to_string(&path).unwrap();
    assert_eq!(content, "first\nsecond\n");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn append_repl_history_empty_prompt() {
    let dir = std::env::temp_dir().join(format!(
        "anveesa_test_{}_{}_{}",
        crate::unix_now(),
        std::process::id(),
        line!()
    ));
    let path = dir.join("history.txt");
    append_repl_history(&path, "").unwrap();
    let content = std::fs::read_to_string(&path).unwrap();
    assert_eq!(content, "\n");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn append_repl_history_unicode() {
    let dir = std::env::temp_dir().join(format!(
        "anveesa_test_{}_{}_{}",
        crate::unix_now(),
        std::process::id(),
        line!()
    ));
    let path = dir.join("history.txt");
    append_repl_history(&path, "こんにちは").unwrap();
    let content = std::fs::read_to_string(&path).unwrap();
    assert_eq!(content, "こんにちは\n");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn append_repl_history_multiline() {
    let dir = std::env::temp_dir().join(format!(
        "anveesa_test_{}_{}_{}",
        crate::unix_now(),
        std::process::id(),
        line!()
    ));
    let path = dir.join("history.txt");
    append_repl_history(&path, "line1\nline2").unwrap();
    let content = std::fs::read_to_string(&path).unwrap();
    assert_eq!(content, "line1\nline2\n");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn append_repl_history_multiple_entries() {
    let dir = std::env::temp_dir().join(format!(
        "anveesa_test_{}_{}_{}",
        crate::unix_now(),
        std::process::id(),
        line!()
    ));
    let path = dir.join("history.txt");
    for i in 0..10 {
        append_repl_history(&path, &format!("prompt {}", i)).unwrap();
    }
    let content = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = content.lines().collect();
    assert_eq!(lines.len(), 10);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn append_repl_history_special_chars() {
    let dir = std::env::temp_dir().join(format!(
        "anveesa_test_{}_{}_{}",
        crate::unix_now(),
        std::process::id(),
        line!()
    ));
    let path = dir.join("history.txt");
    append_repl_history(&path, "test\twith\ttabs").unwrap();
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains('\t'));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn append_repl_history_quotes() {
    let dir = std::env::temp_dir().join(format!(
        "anveesa_test_{}_{}_{}",
        crate::unix_now(),
        std::process::id(),
        line!()
    ));
    let path = dir.join("history.txt");
    append_repl_history(&path, "hello \"world\"").unwrap();
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains('"'));
    std::fs::remove_dir_all(&dir).ok();
}

// ============ purge_stale_sessions tests ============

#[test]
fn purge_stale_sessions_noop_when_no_dir() {
    purge_stale_sessions();
}

#[test]
fn purge_stale_sessions_safe() {
    for _ in 0..10 {
        purge_stale_sessions();
    }
}

// ============ InteractiveSession tests ============

#[test]
fn interactive_session_serialize() {
    let session = InteractiveSession {
        cwd: "/project".to_string(),
        provider: "openai".to_string(),
        model: Some("gpt-4".to_string()),
        system: None,
        messages: vec![],
        saved_at: 1234567890,
    };
    let s = serde_json::to_string(&session).unwrap();
    assert!(s.contains("/project"));
    assert!(s.contains("openai"));
}

#[test]
fn interactive_session_deserialize() {
    let json =
        r#"{"cwd":"/p","provider":"o","model":null,"system":null,"messages":[],"saved_at":0}"#;
    let session: InteractiveSession = serde_json::from_str(json).unwrap();
    assert_eq!(session.cwd, "/p");
}

#[test]
fn interactive_session_deserialize_no_saved_at() {
    let json = r#"{"cwd":"/p","provider":"o","model":null,"system":null,"messages":[]}"#;
    let session: InteractiveSession = serde_json::from_str(json).unwrap();
    assert_eq!(session.saved_at, 0);
}

#[test]
fn interactive_session_roundtrip() {
    let original = InteractiveSession {
        cwd: "/test".to_string(),
        provider: "test".to_string(),
        model: Some("model".to_string()),
        system: Some("sys".to_string()),
        messages: vec![],
        saved_at: 42,
    };
    let s = serde_json::to_string(&original).unwrap();
    let restored: InteractiveSession = serde_json::from_str(&s).unwrap();
    assert_eq!(restored.cwd, original.cwd);
    assert_eq!(restored.provider, original.provider);
    assert_eq!(restored.model, original.model);
}

#[test]
fn interactive_session_empty() {
    let session = InteractiveSession {
        cwd: String::new(),
        provider: String::new(),
        model: None,
        system: None,
        messages: vec![],
        saved_at: 0,
    };
    assert!(session.model.is_none());
    assert!(session.messages.is_empty());
}

#[test]
fn interactive_session_large_messages() {
    let messages: Vec<crate::provider::ChatMessage> = (0..100)
        .map(|i| crate::provider::ChatMessage::user(format!("msg {}", i)))
        .collect();
    let session = InteractiveSession {
        cwd: "/project".to_string(),
        provider: "openai".to_string(),
        model: None,
        system: None,
        messages,
        saved_at: 0,
    };
    assert_eq!(session.messages.len(), 100);
}

#[test]
fn interactive_session_debug() {
    let session = InteractiveSession {
        cwd: "/project".to_string(),
        provider: "openai".to_string(),
        model: None,
        system: None,
        messages: vec![],
        saved_at: 0,
    };
    let debug = format!("{:?}", session);
    assert!(debug.contains("InteractiveSession"));
}

#[test]
fn interactive_session_with_both_messages() {
    let messages = vec![
        crate::provider::ChatMessage::user("q".to_string()),
        crate::provider::ChatMessage::assistant("a".to_string()),
    ];
    let session = InteractiveSession {
        cwd: "/project".to_string(),
        provider: "test".to_string(),
        model: None,
        system: None,
        messages,
        saved_at: 0,
    };
    assert_eq!(session.messages.len(), 2);
}

#[test]
fn interactive_session_serialize_null_model() {
    let session = InteractiveSession {
        cwd: "/p".to_string(),
        provider: "p".to_string(),
        model: None,
        system: None,
        messages: vec![],
        saved_at: 0,
    };
    let s = serde_json::to_string(&session).unwrap();
    assert!(s.contains("null"));
}

#[test]
fn interactive_session_serialize_with_system() {
    let session = InteractiveSession {
        cwd: "/p".to_string(),
        provider: "p".to_string(),
        model: None,
        system: Some("be helpful".to_string()),
        messages: vec![],
        saved_at: 0,
    };
    let s = serde_json::to_string(&session).unwrap();
    assert!(s.contains("be helpful"));
}
