use std::{fs, path::Path, process::Command as ProcessCommand};

use anyhow::{Context, Result};

use crate::config::config_path;

pub fn workspace_context() -> Result<String> {
    let cwd = std::env::current_dir().context("failed to resolve current directory")?;
    workspace_context_for(&cwd)
}

pub fn workspace_context_for(cwd: &Path) -> Result<String> {
    let mut context = String::new();

    context.push_str("You are running inside the user's terminal through the Anveesa CLI.\n");
    context.push_str("Use this workspace context when answering questions about where you are, what project this is, or what files are nearby.\n");
    context.push_str(
        "Do not claim you lack terminal location context when the answer is available below.\n\n",
    );
    context.push_str("Workspace:\n");
    context.push_str(&format!("- cwd: {}\n", cwd.display()));
    if let Some(parent) = cwd.parent() {
        context.push_str(&format!("- parent: {}\n", parent.display()));
    }

    // .anveesa.md — project-level instructions (highest priority context)
    let project_md_paths = [cwd.join(".anveesa.md"), cwd.join("ANVEESA.md")];
    for md_path in &project_md_paths {
        if let Ok(content) = fs::read_to_string(md_path) {
            if !content.trim().is_empty() {
                context.push_str("\nProject instructions (.anveesa.md):\n");
                let capped: String = content.chars().take(8_000).collect();
                context.push_str(&capped);
                context.push('\n');
            }
            break;
        }
    }

    // README — auto-inject up to 3 000 chars for project overview
    for readme in &["README.md", "readme.md", "Readme.md"] {
        if let Ok(content) = fs::read_to_string(cwd.join(readme)) {
            if !content.trim().is_empty() {
                context.push_str("\nProject README (first 3000 chars):\n");
                let capped: String = content.chars().take(3_000).collect();
                context.push_str(&capped);
                context.push('\n');
            }
            break;
        }
    }

    if let Some(git_root) = git_output(cwd, ["rev-parse", "--show-toplevel"]) {
        // Also check git root for .anveesa.md if different from cwd
        let git_root_path = std::path::Path::new(&git_root);
        if git_root_path != cwd {
            for md_path in &[
                git_root_path.join(".anveesa.md"),
                git_root_path.join("ANVEESA.md"),
            ] {
                if let Ok(content) = fs::read_to_string(md_path) {
                    if !content.trim().is_empty() {
                        context.push_str("\nProject instructions (from git root):\n");
                        let capped: String = content.chars().take(8_000).collect();
                        context.push_str(&capped);
                        context.push('\n');
                    }
                    break;
                }
            }
        }
        context.push_str(&format!("- git_root: {git_root}\n"));
        if let Some(branch) = git_output(cwd, ["branch", "--show-current"])
            && !branch.is_empty()
        {
            context.push_str(&format!("- git_branch: {branch}\n"));
        }
        if let Some(status) = git_output(cwd, ["status", "--short"]) {
            if status.is_empty() {
                context.push_str("- git_status: clean\n");
            } else {
                context.push_str("- git_status:\n");
                for line in status.lines().take(20) {
                    context.push_str(&format!("  {line}\n"));
                }
            }
        }
        // Recent commits give the model useful project history context
        if let Some(log) = git_output(cwd, ["log", "--oneline", "--decorate", "-8"])
            && !log.is_empty()
        {
            context.push_str("- recent_commits:\n");
            for line in log.lines() {
                context.push_str(&format!("  {line}\n"));
            }
        }
        // Repo map — all tracked source files grouped by directory
        if let Some(files) = git_output(cwd, ["ls-files", "--cached"]) {
            const SOURCE_EXTS: &[&str] = &[
                ".rs", ".ts", ".tsx", ".js", ".jsx", ".py", ".go", ".java", ".kt", ".swift", ".c",
                ".cpp", ".h", ".hpp", ".cs", ".rb", ".php", ".vue", ".svelte", ".toml", ".yaml",
                ".yml", ".json",
            ];
            let tracked: Vec<&str> = files
                .lines()
                .filter(|f| SOURCE_EXTS.iter().any(|ext| f.ends_with(ext)))
                .take(250)
                .collect();
            if !tracked.is_empty() {
                context.push_str("- repo_files:\n");
                for f in &tracked {
                    context.push_str(&format!("  {f}\n"));
                }
            }
        }
    } else {
        context.push_str("- git: not inside a git repository\n");
    }

    // Available notes
    let notes_dir = config_path()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("notes")));
    if let Some(dir) = notes_dir.filter(|d| d.exists()) {
        let note_keys: Vec<String> = fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let path = e.path();
                if path.extension()?.to_str()? == "md" {
                    path.file_stem()?.to_str().map(str::to_string)
                } else {
                    None
                }
            })
            .collect();
        if !note_keys.is_empty() {
            context.push_str(&format!("- saved_notes: {}\n", note_keys.join(", ")));
        }
    }

    // Project metadata from package.json / Cargo.toml
    if let Ok(raw) = fs::read_to_string(cwd.join("package.json")) {
        if let Ok(pkg) = serde_json::from_str::<serde_json::Value>(&raw) {
            if let Some(name) = pkg["name"].as_str() {
                context.push_str(&format!("- project_name: {name}\n"));
            }
            if let Some(ver) = pkg["version"].as_str() {
                context.push_str(&format!("- project_version: {ver}\n"));
            }
            if let Some(desc) = pkg["description"].as_str() {
                context.push_str(&format!("- project_description: {desc}\n"));
            }
        }
    } else if let Ok(raw) = fs::read_to_string(cwd.join("Cargo.toml")) {
        for line in raw.lines().take(15) {
            if line.starts_with("name")
                || line.starts_with("version")
                || line.starts_with("description")
            {
                context.push_str(&format!("- cargo_{}\n", line.trim()));
            }
        }
    }

    let entries = directory_entries(cwd)?;
    if entries.is_empty() {
        context.push_str("- directory_entries: empty\n");
    } else {
        context.push_str("- directory_entries:\n");
        for entry in entries {
            context.push_str(&format!("  {entry}\n"));
        }
    }

    Ok(context)
}

pub fn directory_entries(cwd: &Path) -> Result<Vec<String>> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(cwd).with_context(|| format!("failed to read {}", cwd.display()))? {
        let entry = entry?;
        let path = entry.path();
        let file_name = entry.file_name().to_string_lossy().into_owned();
        if file_name == ".git" {
            continue;
        }

        let kind = if path.is_dir() {
            "dir"
        } else if path.is_file() {
            "file"
        } else {
            "other"
        };
        entries.push(format!("{file_name}/ ({kind})").replace("/ (file)", " (file)"));
    }

    entries.sort();
    entries.truncate(40);
    Ok(entries)
}

pub fn git_output<const N: usize>(cwd: &Path, args: [&str; N]) -> Option<String> {
    let output = ProcessCommand::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}
