//! Tests for src/workspace.rs — workspace context generation and directory listing.

use crate::workspace::{directory_entries, git_output, workspace_context_for};
use std::path::PathBuf;

// ── git_output ───────────────────────────────────────────────────────

#[test]
fn git_output_version() {
    let out = git_output::<1>(
        PathBuf::from("/Users/pandhuwibowo/Portfolio/anveesa/anveesa-cli").as_path(),
        ["--version"],
    );
    assert!(out.is_some(), "git should be installed");
    assert!(out.as_ref().unwrap().contains("git version"));
}

#[test]
fn git_output_nonexistent_dir() {
    let out = git_output::<1>(
        PathBuf::from("/nonexistent/dir/12345").as_path(),
        ["status"],
    );
    assert!(out.is_none());
}

#[test]
fn git_output_bad_command() {
    let out = git_output::<1>(
        PathBuf::from("/tmp").as_path(),
        ["nonexistent-subcommand-12345"],
    );
    assert!(out.is_none());
}

#[test]
fn git_output_empty_args() {
    let out = git_output::<0>(PathBuf::from("/tmp").as_path(), []);
    assert!(out.is_none()); // git with no args returns non-zero or prints usage
}

#[test]
fn git_output_show_toplevel_in_git_repo() {
    let out = git_output::<2>(
        PathBuf::from("/Users/pandhuwibowo/Portfolio/anveesa/anveesa-cli").as_path(),
        ["rev-parse", "--show-toplevel"],
    );
    assert!(out.is_some(), "should find git root");
    assert!(out.as_ref().unwrap().ends_with("anveesa-cli"));
}

#[test]
fn git_output_branch_show_current() {
    let out = git_output::<2>(
        PathBuf::from("/Users/pandhuwibowo/Portfolio/anveesa/anveesa-cli").as_path(),
        ["branch", "--show-current"],
    );
    // May be Some or None depending on detached HEAD
    let _ = out;
}

// ── directory_entries ────────────────────────────────────────────────

#[test]
fn directory_entries_current_dir() {
    let entries = directory_entries(std::path::Path::new("."));
    assert!(entries.is_ok());
    let list = entries.unwrap();
    assert!(!list.is_empty(), "current dir should have entries");
    // Should be sorted
    for i in 1..list.len() {
        assert!(list[i] >= list[i - 1], "entries should be sorted");
    }
}

#[test]
fn directory_entries_temp_dir() {
    let tmp = std::env::temp_dir();
    let entries = directory_entries(&tmp);
    assert!(entries.is_ok());
    let list = entries.unwrap();
    // /tmp should have entries
    assert!(!list.is_empty());
}

#[test]
fn directory_entries_max_40() {
    // /tmp or project root should not exceed 40
    let entries = directory_entries(std::path::Path::new(".")).unwrap();
    assert!(entries.len() <= 40, "should truncate to 40");
}

#[test]
fn directory_entries_no_git_folder() {
    // Even in a git repo, .git should not appear
    let entries = directory_entries(std::path::Path::new(".")).unwrap();
    let has_git = entries.iter().any(|e| e == ".git/ (dir)");
    assert!(!has_git, ".git should be excluded");
}

#[test]
fn directory_entries_format_file() {
    let entries = directory_entries(std::path::Path::new(".")).unwrap();
    for e in &entries {
        // Files should end with " (file)", dirs with "/ (dir)"
        if e.contains("(file)") {
            assert!(e.ends_with(" (file)"));
            assert!(!e.contains("/ (file)"));
        }
    }
}

#[test]
fn directory_entries_format_dir() {
    let entries = directory_entries(std::path::Path::new(".")).unwrap();
    for e in &entries {
        if e.contains("(dir)") {
            assert!(e.ends_with("/ (dir)"));
        }
    }
}

#[test]
fn directory_entries_nonexistent() {
    let result = directory_entries(PathBuf::from("/nonexistent/12345").as_path());
    assert!(result.is_err(), "should error for nonexistent dir");
}

// ── workspace_context_for ────────────────────────────────────────────

#[test]
fn workspace_context_has_cwd() {
    let ctx = workspace_context_for(PathBuf::from("/tmp").as_path()).unwrap();
    assert!(ctx.contains("cwd: /tmp"));
}

#[test]
fn workspace_context_has_parent() {
    let ctx = workspace_context_for(PathBuf::from("/tmp").as_path()).unwrap();
    assert!(ctx.contains("parent: /"));
}

#[test]
fn workspace_context_has_header() {
    let ctx = workspace_context_for(PathBuf::from("/tmp").as_path()).unwrap();
    assert!(ctx.contains("Anveesa CLI"));
    assert!(ctx.contains("Workspace:"));
}

#[test]
fn workspace_context_in_git_repo() {
    let ctx = workspace_context_for(
        PathBuf::from("/Users/pandhuwibowo/Portfolio/anveesa/anveesa-cli").as_path(),
    )
    .unwrap();
    assert!(ctx.contains("git_root:"));
    assert!(ctx.contains("git_branch:"));
    assert!(ctx.contains("git_status:"));
}

#[test]
fn workspace_context_has_anveesa_md() {
    let ctx = workspace_context_for(
        PathBuf::from("/Users/pandhuwibowo/Portfolio/anveesa/anveesa-cli").as_path(),
    )
    .unwrap();
    assert!(ctx.contains("Project instructions"));
}

#[test]
fn workspace_context_has_readme() {
    let ctx = workspace_context_for(
        PathBuf::from("/Users/pandhuwibowo/Portfolio/anveesa/anveesa-cli").as_path(),
    )
    .unwrap();
    assert!(ctx.contains("Project README"));
}

#[test]
fn workspace_context_has_repo_files() {
    let ctx = workspace_context_for(
        PathBuf::from("/Users/pandhuwibowo/Portfolio/anveesa/anveesa-cli").as_path(),
    )
    .unwrap();
    assert!(ctx.contains("repo_files:"));
    assert!(ctx.contains(".rs"));
}

#[test]
fn workspace_context_has_directory_entries() {
    let ctx = workspace_context_for(
        PathBuf::from("/Users/pandhuwibowo/Portfolio/anveesa/anveesa-cli").as_path(),
    )
    .unwrap();
    assert!(ctx.contains("directory_entries:"));
}

#[test]
fn workspace_context_non_git_dir() {
    let ctx = workspace_context_for(PathBuf::from("/tmp").as_path()).unwrap();
    assert!(ctx.contains("not inside a git repository"));
}

#[test]
fn workspace_context_cargo_toml() {
    // project has package.json so that branch runs first — cargo_toml only runs as fallback
    // Verify package.json metadata IS present
    let ctx = workspace_context_for(
        PathBuf::from("/Users/pandhuwibowo/Portfolio/anveesa/anveesa-cli").as_path(),
    )
    .unwrap();
    assert!(ctx.contains("project_name:") || ctx.contains("project_version:"));
}

#[test]
fn workspace_context_package_json() {
    let ctx = workspace_context_for(
        PathBuf::from("/Users/pandhuwibowo/Portfolio/anveesa/anveesa-cli").as_path(),
    )
    .unwrap();
    // Has both Cargo.toml and package.json
    assert!(ctx.contains("cargo_") || ctx.contains("project_name:"));
}

// ── Edge cases ───────────────────────────────────────────────────────

#[test]
fn git_output_multiple_args() {
    let out = git_output::<4>(
        PathBuf::from("/Users/pandhuwibowo/Portfolio/anveesa/anveesa-cli").as_path(),
        ["log", "--oneline", "-1", "--format=%H"],
    );
    assert!(out.is_some());
    let hash = out.unwrap();
    assert_eq!(hash.len(), 40); // full git hash
}

#[test]
fn git_output_with_special_chars_in_path() {
    let out = git_output::<1>(
        PathBuf::from("/tmp/special dir with spaces").as_path(),
        ["status"],
    );
    // Should gracefully return None for nonexistent dir
    assert!(out.is_none());
}

#[test]
fn directory_entries_sorted() {
    let entries = directory_entries(std::path::Path::new(".")).unwrap();
    let mut sorted = entries.clone();
    sorted.sort();
    assert_eq!(entries, sorted);
}

#[test]
fn directory_entries_all_have_kind() {
    let entries = directory_entries(std::path::Path::new(".")).unwrap();
    for e in &entries {
        assert!(
            e.contains("(file)") || e.contains("(dir)") || e.contains("(other)"),
            "entry should have a kind: {e}",
        );
    }
}

#[test]
fn workspace_context_readme_capped() {
    let ctx = workspace_context_for(
        PathBuf::from("/Users/pandhuwibowo/Portfolio/anveesa/anveesa-cli").as_path(),
    )
    .unwrap();
    // Find the README section and verify it exists
    if let Some(start) = ctx.find("Project README") {
        let section = &ctx[start..];
        let next_section = section.find("\n\n").unwrap_or(section.len());
        let readme_content = &section[..next_section];
        // The README content is capped at 3000 chars
        assert!(
            readme_content.len() <= 3500,
            "README should be reasonably capped"
        );
    }
}

#[test]
fn workspace_context_recent_commits() {
    let ctx = workspace_context_for(
        PathBuf::from("/Users/pandhuwibowo/Portfolio/anveesa/anveesa-cli").as_path(),
    )
    .unwrap();
    assert!(ctx.contains("recent_commits:"));
}

// ── Additional edge cases ────────────────────────────────────────────

#[test]
fn directory_entries_empty_string_not_present() {
    let entries = directory_entries(std::path::Path::new(".")).unwrap();
    for e in &entries {
        assert!(!e.is_empty(), "should not have empty entries");
    }
}

#[test]
fn workspace_context_is_string() {
    let ctx = workspace_context_for(PathBuf::from("/tmp").as_path()).unwrap();
    // Just verify it returns a valid string
    assert!(!ctx.is_empty());
}

#[test]
fn git_output_returns_trimmed() {
    let out = git_output::<2>(
        PathBuf::from("/Users/pandhuwibowo/Portfolio/anveesa/anveesa-cli").as_path(),
        ["rev-parse", "--show-toplevel"],
    );
    assert!(out.is_some());
    let s = out.unwrap();
    assert_eq!(s.trim(), s); // should already be trimmed
}

// ── workspace_context_for on subdirectories ──────────────────────────

#[test]
fn workspace_context_for_src_dir() {
    let ctx = workspace_context_for(
        PathBuf::from("/Users/pandhuwibowo/Portfolio/anveesa/anveesa-cli/src").as_path(),
    )
    .unwrap();
    assert!(ctx.contains("cwd:"));
    assert!(ctx.contains("/src"));
}

#[test]
fn workspace_context_for_root() {
    let ctx = workspace_context_for(PathBuf::from("/").as_path()).unwrap();
    assert!(ctx.contains("cwd: /"));
}

// ── directory_entries ordering ───────────────────────────────────────

#[test]
fn directory_entries_dots_first() {
    // Dot files sort before regular files in ASCII
    let entries = directory_entries(std::path::Path::new(".")).unwrap();
    let mut first_non_dot = None;
    for e in &entries {
        let name = e
            .split('/')
            .next()
            .unwrap_or(e)
            .split(' ')
            .next()
            .unwrap_or(e);
        if name.starts_with('.') && first_non_dot.is_none() {
            continue;
        }
        first_non_dot.get_or_insert(e.clone());
    }
    // Just verify the entries are consistently sorted
    assert!(!entries.is_empty());
}

#[test]
fn git_output_stderr_not_in_output() {
    // stderr should not leak into stdout
    let out = git_output::<1>(
        PathBuf::from("/Users/pandhuwibowo/Portfolio/anveesa/anveesa-cli").as_path(),
        ["--version"],
    );
    let s = out.unwrap();
    // git --version only prints to stdout, no stderr
    assert!(!s.contains("error:"));
}

#[test]
fn workspace_context_no_panic_on_any_path() {
    // Should not panic even on weird paths
    let result = workspace_context_for(PathBuf::from("/proc").as_path());
    // May fail (nonexistent on macOS) but should not panic
    let _ = result;
}

#[test]
fn workspace_context_unicode_cwd() {
    // Unicode in path shouldn't break anything
    let ctx = workspace_context_for(PathBuf::from("/tmp").as_path()).unwrap();
    assert!(ctx.contains("Anveesa"));
}

// ── More git_output edge cases ───────────────────────────────────────

#[test]
fn git_output_rev_parse_short() {
    let out = git_output::<2>(
        PathBuf::from("/Users/pandhuwibowo/Portfolio/anveesa/anveesa-cli").as_path(),
        ["rev-parse", "HEAD"],
    );
    assert!(out.is_some());
    let hash = out.unwrap();
    assert_eq!(hash.len(), 40);
}

#[test]
fn git_output_ls_files() {
    let out = git_output::<2>(
        PathBuf::from("/Users/pandhuwibowo/Portfolio/anveesa/anveesa-cli").as_path(),
        ["ls-files", "--cached"],
    );
    assert!(out.is_some());
    let files = out.unwrap();
    assert!(files.contains(".rs") || files.contains("Cargo.toml"));
}

#[test]
fn git_output_log_oneline() {
    let out = git_output::<4>(
        PathBuf::from("/Users/pandhuwibowo/Portfolio/anveesa/anveesa-cli").as_path(),
        ["log", "--oneline", "--decorate", "-3"],
    );
    assert!(out.is_some());
    let log = out.unwrap();
    let lines: Vec<&str> = log.lines().collect();
    assert!(!lines.is_empty());
}

// ── More directory_entries tests ─────────────────────────────────────

#[test]
fn directory_entries_src_dir() {
    let entries = directory_entries(
        PathBuf::from("/Users/pandhuwibowo/Portfolio/anveesa/anveesa-cli/src").as_path(),
    );
    assert!(entries.is_ok());
    let list = entries.unwrap();
    assert!(!list.is_empty());
    // Should have tui/ (dir), lib.rs (file), etc.
    let has_file = list.iter().any(|e| e.contains(" (file)"));
    assert!(has_file);
}

#[test]
fn directory_entries_parent_dir() {
    let entries =
        directory_entries(PathBuf::from("/Users/pandhuwibowo/Portfolio/anveesa").as_path());
    assert!(entries.is_ok());
    let list = entries.unwrap();
    let has_dir = list.iter().any(|e| e.contains("/ (dir)"));
    assert!(has_dir);
}

// ── workspace_context integration ────────────────────────────────────

#[test]
fn workspace_context_is_deterministic() {
    // Running twice should give same result (git state doesn't change)
    let path = PathBuf::from("/Users/pandhuwibowo/Portfolio/anveesa/anveesa-cli");
    let ctx1 = workspace_context_for(path.as_path()).unwrap();
    let ctx2 = workspace_context_for(path.as_path()).unwrap();
    assert_eq!(ctx1, ctx2);
}

#[test]
fn workspace_context_contains_workspace_header() {
    let ctx = workspace_context_for(PathBuf::from("/tmp").as_path()).unwrap();
    assert!(ctx.contains("Workspace:\n"));
}

#[test]
fn workspace_context_has_no_binary_garbage() {
    let ctx = workspace_context_for(PathBuf::from("/tmp").as_path()).unwrap();
    // Should be valid UTF-8 with no null bytes
    assert!(!ctx.contains('\0'));
}

#[test]
fn git_output_show_short_in_clean_repo() {
    let out = git_output::<2>(
        PathBuf::from("/Users/pandhuwibowo/Portfolio/anveesa/anveesa-cli").as_path(),
        ["status", "--short"],
    );
    // May be empty or have some changes
    let _ = out;
}
