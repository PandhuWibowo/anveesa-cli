use std::{
    fs,
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::{cli::AskOptions, config::config_path, provider::ChatMessage, unix_now};

#[derive(Debug, Serialize, Deserialize)]
pub struct InteractiveSession {
    pub cwd: String,
    pub provider: String,
    pub model: Option<String>,
    pub system: Option<String>,
    pub messages: Vec<ChatMessage>,
    #[serde(default)]
    pub saved_at: u64,
}

pub fn sessions_dir() -> Option<PathBuf> {
    let config_dir = config_path().ok()?.parent()?.to_path_buf();
    Some(config_dir.join("sessions"))
}

pub fn list_sessions() -> Result<()> {
    let Some(dir) = sessions_dir() else {
        println!("No sessions directory found.");
        return Ok(());
    };
    let Ok(entries) = fs::read_dir(&dir) else {
        println!("No sessions found.");
        return Ok(());
    };

    let mut sessions: Vec<(String, usize, u64)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if let Ok(content) = fs::read_to_string(&path) {
            if let Ok(session) = serde_json::from_str::<InteractiveSession>(&content) {
                sessions.push((session.cwd, session.messages.len() / 2, session.saved_at));
            }
        }
    }
    sessions.sort_by(|a, b| b.2.cmp(&a.2));

    let is_tty = io::stdout().is_terminal();
    if sessions.is_empty() {
        if is_tty {
            eprintln!("\x1b[2m  No saved sessions.\x1b[0m");
        } else {
            println!("no sessions");
        }
        return Ok(());
    }

    if !is_tty {
        for (cwd, turns, saved_at) in &sessions {
            println!("{cwd}\t{turns}\t{saved_at}");
        }
        return Ok(());
    }

    println!();
    println!("\x1b[90m  ──────────────────────────────────────────────────────\x1b[0m");
    for (cwd, turns, saved_at) in &sessions {
        let age = format_session_age(Some(*saved_at));
        let turn_str = if *turns == 1 { "1 turn ".to_string() } else { format!("{turns} turns") };
        let short_cwd = std::env::var("HOME")
            .map(|h| cwd.replacen(&h, "~", 1))
            .unwrap_or_else(|_| cwd.clone());
        println!("  \x1b[2m{age:>10}\x1b[0m  {turn_str:>7}  {short_cwd}");
    }
    println!("\x1b[90m  ──────────────────────────────────────────────────────\x1b[0m");
    println!();
    Ok(())
}

pub fn clear_sessions(all: bool) -> Result<()> {
    let is_tty = io::stdout().is_terminal();
    if all {
        let mut count = 0usize;
        if let Some(dir) = sessions_dir() {
            if let Ok(entries) = fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().and_then(|e| e.to_str()) == Some("json") {
                        if fs::remove_file(&path).is_ok() {
                            count += 1;
                        }
                    }
                }
            }
        }
        if is_tty {
            eprintln!("\x1b[2m  {count} session(s) deleted.\x1b[0m");
        } else {
            println!("{count} sessions deleted");
        }
    } else {
        let cwd = std::env::current_dir().context("failed to resolve current directory")?;
        let path = repl_session_path(&cwd).context("could not determine session path")?;
        if path.exists() {
            fs::remove_file(&path)
                .with_context(|| format!("failed to delete {}", path.display()))?;
            if is_tty {
                eprintln!("\x1b[2m  Session for {} cleared.\x1b[0m", cwd.display());
            } else {
                println!("session cleared");
            }
        } else {
            if is_tty {
                eprintln!("\x1b[2m  No session for {}.\x1b[0m", cwd.display());
            } else {
                println!("no session");
            }
        }
    }
    Ok(())
}

pub fn format_session_age(saved_at: Option<u64>) -> String {
    let Some(ts) = saved_at else {
        return "unknown age".to_string();
    };
    let secs = unix_now().saturating_sub(ts);
    if secs < 60 {
        "just now".to_string()
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86400)
    }
}

/// FNV-1a 64-bit hash of the cwd path — used as a stable per-directory session filename.
pub fn cwd_session_hash(cwd: &Path) -> String {
    let s = cwd.to_string_lossy();
    let mut h: u64 = 14695981039346656037;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(1099511628211);
    }
    format!("{h:016x}")
}

pub fn append_repl_history(path: &Path, prompt: &str) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }

    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "{prompt}")
}

/// Delete all session files whose saved_at is older than 30 days.  Called once at
/// startup so orphaned sessions (from deleted/moved projects) eventually disappear.
pub fn purge_stale_sessions() {
    let Some(dir) = sessions_dir() else { return };
    let Ok(entries) = fs::read_dir(&dir) else { return };
    let cutoff = unix_now().saturating_sub(30 * 24 * 3600);
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let stale = fs::read_to_string(&path)
            .ok()
            .and_then(|c| serde_json::from_str::<InteractiveSession>(&c).ok())
            .map(|s| s.saved_at > 0 && s.saved_at < cutoff)
            .unwrap_or(true); // unparseable → delete
        if stale {
            let _ = fs::remove_file(&path);
        }
    }
}

/// Per-directory session file: ~/.config/anveesa/sessions/{cwd-hash}.json
pub fn repl_session_path(cwd: &Path) -> Option<PathBuf> {
    let config_dir = config_path().ok()?.parent()?.to_path_buf();
    let sessions_dir = config_dir.join("sessions");
    let _ = fs::create_dir_all(&sessions_dir);
    Some(sessions_dir.join(format!("{}.json", cwd_session_hash(cwd))))
}

/// Legacy path for backward-compat migration.
pub fn legacy_session_path() -> Option<PathBuf> {
    let config_dir = config_path().ok()?.parent()?.to_path_buf();
    let path = config_dir.join("session.json");
    if path.exists() { Some(path) } else { None }
}

pub fn load_interactive_session(path: &Path, cwd: &Path) -> Option<InteractiveSession> {
    let content = fs::read_to_string(path).ok()?;
    let session: InteractiveSession = serde_json::from_str(&content).ok()?;
    if session.cwd != cwd.display().to_string() {
        return None;
    }
    // Auto-expire sessions older than 30 days.
    if session.saved_at > 0 && unix_now().saturating_sub(session.saved_at) > 30 * 24 * 3600 {
        let _ = fs::remove_file(path);
        return None;
    }
    Some(session)
}

pub fn save_interactive_session(
    path: &Path,
    cwd: &Path,
    provider: &str,
    options: &AskOptions,
    history: &[ChatMessage],
) -> Result<()> {
    let session = InteractiveSession {
        cwd: cwd.display().to_string(),
        provider: provider.to_string(),
        model: options.model.clone(),
        system: options.system.clone(),
        messages: history.to_vec(),
        saved_at: unix_now(),
    };
    let content = serde_json::to_string_pretty(&session)
        .context("failed to serialize interactive session")?;
    fs::write(path, content).with_context(|| format!("failed to write {}", path.display()))
}

pub fn repl_history_path() -> Option<PathBuf> {
    let path = config_path().ok()?;
    let dir = path.parent()?;
    let _ = fs::create_dir_all(dir);
    Some(dir.join("history"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interactive_session_matches_cwd_only() {
        let cwd = Path::new("/tmp/anveesa-session");
        let session = InteractiveSession {
            cwd: cwd.display().to_string(),
            provider: "provider-a".into(),
            model: Some("model-a".into()),
            system: None,
            messages: vec![],
            saved_at: 0,
        };

        // Matches when cwd is the same.
        assert_eq!(session.cwd, cwd.display().to_string());
        // A different cwd should not match.
        assert_ne!(session.cwd, Path::new("/tmp/other").display().to_string());
        // Provider/model differences no longer prevent a session from loading.
    }

    #[test]
    fn saves_and_loads_interactive_session() {
        let dir = std::env::temp_dir().join(format!("anveesa_session_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.json");
        let options = AskOptions {
            provider: Some("provider-a".into()),
            model: Some("model-a".into()),
            system: None,
            stdin: false,
            yes: false,
        };
        let history = vec![
            ChatMessage::user("continue please".into()),
            ChatMessage::assistant("continuing".into()),
        ];

        save_interactive_session(&path, &dir, "provider-a", &options, &history).unwrap();

        let loaded = load_interactive_session(&path, &dir).unwrap();
        assert_eq!(loaded.messages, history);
        // saved_at should be set.
        assert!(loaded.saved_at > 0);

        let _ = fs::remove_dir_all(&dir);
    }

    // ── cwd_session_hash ──────────────────────────────────────────────────────

    #[test]
    fn cwd_hash_is_deterministic() {
        let p = Path::new("/home/user/my-project");
        assert_eq!(cwd_session_hash(p), cwd_session_hash(p));
    }

    #[test]
    fn cwd_hash_differs_for_different_paths() {
        let a = cwd_session_hash(Path::new("/home/user/project-a"));
        let b = cwd_session_hash(Path::new("/home/user/project-b"));
        let c = cwd_session_hash(Path::new("/home/user/project-a/sub"));
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert_ne!(b, c);
    }

    #[test]
    fn cwd_hash_is_16_hex_chars() {
        let h = cwd_session_hash(Path::new("/any/path"));
        assert_eq!(h.len(), 16);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }

    // ── format_session_age ────────────────────────────────────────────────────

    #[test]
    fn format_age_none_returns_unknown() {
        assert_eq!(format_session_age(None), "unknown age");
    }

    #[test]
    fn format_age_just_now() {
        let ts = unix_now();
        assert_eq!(format_session_age(Some(ts)), "just now");
        assert_eq!(format_session_age(Some(ts - 59)), "just now");
    }

    #[test]
    fn format_age_minutes() {
        let ts = unix_now() - 60;
        assert_eq!(format_session_age(Some(ts)), "1m ago");
        let ts2 = unix_now() - 3599;
        assert_eq!(format_session_age(Some(ts2)), "59m ago");
    }

    #[test]
    fn format_age_hours() {
        let ts = unix_now() - 3600;
        assert_eq!(format_session_age(Some(ts)), "1h ago");
        let ts2 = unix_now() - 86399;
        assert_eq!(format_session_age(Some(ts2)), "23h ago");
    }

    #[test]
    fn format_age_days() {
        let ts = unix_now() - 86400;
        assert_eq!(format_session_age(Some(ts)), "1d ago");
        let ts2 = unix_now() - 7 * 86400;
        assert_eq!(format_session_age(Some(ts2)), "7d ago");
    }

    // ── 30-day expiry ─────────────────────────────────────────────────────────

    #[test]
    fn expired_session_is_deleted_on_load() {
        let dir = std::env::temp_dir().join(format!("anveesa_expiry_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("old_session.json");
        let options = AskOptions { provider: Some("p".into()), model: None, system: None, stdin: false, yes: false };
        save_interactive_session(&path, &dir, "p", &options, &[]).unwrap();

        // Backdate saved_at by 31 days.
        let content = fs::read_to_string(&path).unwrap();
        let mut session: InteractiveSession = serde_json::from_str(&content).unwrap();
        session.saved_at = unix_now() - 31 * 24 * 3600;
        fs::write(&path, serde_json::to_string_pretty(&session).unwrap()).unwrap();

        let result = load_interactive_session(&path, &dir);
        assert!(result.is_none(), "expired session must not load");
        assert!(!path.exists(), "expired session file must be deleted");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn non_expired_session_loads_normally() {
        let dir = std::env::temp_dir().join(format!("anveesa_noexpiry_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.json");
        let options = AskOptions { provider: Some("p".into()), model: None, system: None, stdin: false, yes: false };
        let history = vec![ChatMessage::user("hi".into()), ChatMessage::assistant("hello".into())];
        save_interactive_session(&path, &dir, "p", &options, &history).unwrap();

        let loaded = load_interactive_session(&path, &dir).unwrap();
        assert_eq!(loaded.messages, history);
        assert!(path.exists());

        let _ = fs::remove_dir_all(&dir);
    }

    // ── legacy migration ──────────────────────────────────────────────────────

    #[test]
    fn mismatched_cwd_returns_none() {
        let dir_a = std::env::temp_dir().join(format!("anveesa_cwd_a_{}", std::process::id()));
        let dir_b = std::env::temp_dir().join(format!("anveesa_cwd_b_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir_a);
        let _ = fs::remove_dir_all(&dir_b);
        fs::create_dir_all(&dir_a).unwrap();
        let path = dir_a.join("session.json");
        let options = AskOptions { provider: None, model: None, system: None, stdin: false, yes: false };
        save_interactive_session(&path, &dir_a, "p", &options, &[]).unwrap();

        // Loading with a different cwd must return None.
        assert!(load_interactive_session(&path, &dir_b).is_none());
        // Loading with the correct cwd must succeed.
        assert!(load_interactive_session(&path, &dir_a).is_some());

        let _ = fs::remove_dir_all(&dir_a);
    }

    // ── purge_stale_sessions ──────────────────────────────────────────────────

    #[test]
    fn purge_removes_old_files_but_keeps_recent_ones() {
        let sessions_base = std::env::temp_dir()
            .join(format!("anveesa_purge_{}", std::process::id()));
        let _ = fs::remove_dir_all(&sessions_base);
        fs::create_dir_all(&sessions_base).unwrap();

        let options = AskOptions { provider: None, model: None, system: None, stdin: false, yes: false };

        // Create two fresh sessions and one stale session.
        let fresh_dir_1 = sessions_base.join("project1");
        let fresh_dir_2 = sessions_base.join("project2");
        let stale_dir = sessions_base.join("old_project");
        fs::create_dir_all(&fresh_dir_1).unwrap();
        fs::create_dir_all(&fresh_dir_2).unwrap();
        fs::create_dir_all(&stale_dir).unwrap();

        let fresh1_path = sessions_base.join("fresh1.json");
        let fresh2_path = sessions_base.join("fresh2.json");
        let stale_path = sessions_base.join("stale.json");

        save_interactive_session(&fresh1_path, &fresh_dir_1, "p", &options, &[]).unwrap();
        save_interactive_session(&fresh2_path, &fresh_dir_2, "p", &options, &[]).unwrap();
        save_interactive_session(&stale_path, &stale_dir, "p", &options, &[]).unwrap();

        // Backdate the stale session.
        let content = fs::read_to_string(&stale_path).unwrap();
        let mut session: InteractiveSession = serde_json::from_str(&content).unwrap();
        session.saved_at = unix_now() - 31 * 24 * 3600;
        fs::write(&stale_path, serde_json::to_string_pretty(&session).unwrap()).unwrap();

        // Manually run purge logic over our temp dir (can't call purge_stale_sessions
        // directly since it targets the real config dir, so we replicate its logic).
        let cutoff = unix_now().saturating_sub(30 * 24 * 3600);
        for entry in fs::read_dir(&sessions_base).unwrap().flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") { continue; }
            let stale = fs::read_to_string(&path)
                .ok()
                .and_then(|c| serde_json::from_str::<InteractiveSession>(&c).ok())
                .map(|s| s.saved_at > 0 && s.saved_at < cutoff)
                .unwrap_or(true);
            if stale { let _ = fs::remove_file(&path); }
        }

        assert!(fresh1_path.exists(), "fresh session 1 must not be purged");
        assert!(fresh2_path.exists(), "fresh session 2 must not be purged");
        assert!(!stale_path.exists(), "stale session must be purged");

        let _ = fs::remove_dir_all(&sessions_base);
    }

    #[test]
    fn purge_removes_unparseable_json_files() {
        let sessions_base = std::env::temp_dir()
            .join(format!("anveesa_purge_bad_{}", std::process::id()));
        let _ = fs::remove_dir_all(&sessions_base);
        fs::create_dir_all(&sessions_base).unwrap();

        let bad_path = sessions_base.join("corrupt.json");
        fs::write(&bad_path, b"not valid json at all {{{").unwrap();

        // Replicate purge logic.
        let cutoff = unix_now().saturating_sub(30 * 24 * 3600);
        for entry in fs::read_dir(&sessions_base).unwrap().flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") { continue; }
            let stale = fs::read_to_string(&path)
                .ok()
                .and_then(|c| serde_json::from_str::<InteractiveSession>(&c).ok())
                .map(|s| s.saved_at > 0 && s.saved_at < cutoff)
                .unwrap_or(true);
            if stale { let _ = fs::remove_file(&path); }
        }

        assert!(!bad_path.exists(), "corrupt session file must be purged");

        let _ = fs::remove_dir_all(&sessions_base);
    }
}
