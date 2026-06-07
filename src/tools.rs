use std::{
    collections::{HashMap, VecDeque},
    fs,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Mutex, OnceLock},
    time::{Duration, SystemTime},
};

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use serde_json::{Value, json};

// ── File read cache ───────────────────────────────────────────────────────────
// Keyed by (absolute path, mtime) → content. Lives for the process lifetime.
static FILE_CACHE: OnceLock<Mutex<HashMap<(PathBuf, SystemTime), String>>> = OnceLock::new();

fn file_cache() -> &'static Mutex<HashMap<(PathBuf, SystemTime), String>> {
    FILE_CACHE.get_or_init(Default::default)
}

const MAX_DIR_ENTRIES: usize = 120;
const MAX_SEARCH_RESULTS: usize = 80;
const MAX_VISITED_PATHS: usize = 5_000;
const MAX_DEPTH: usize = 8;
const MAX_READ_LINES: usize = 200;
const MAX_TEXT_BYTES: u64 = 1_000_000;
const MAX_COMMAND_OUTPUT: usize = 20_000;
const DEFAULT_COMMAND_TIMEOUT_SECS: u64 = 60;
const MAX_COMMAND_TIMEOUT_SECS: u64 = 300;

/// System guidance describing the available tools to the model.
pub fn guidance(include_write: bool) -> String {
    let mut text = String::from(
        "You can use Anveesa tools to inspect the workspace: list directories, find files by name, \
search text, read capped file snippets, fetch URLs, run git commands, and do a basic public web lookup. \
Prefer tools over guessing. \
If you need to inspect, read, list, search, fetch, or check something, call the relevant tool immediately; \
do not end a response by saying you will inspect something later.",
    );
    if include_write {
        text.push_str(
            " You may also modify the workspace with create_dir, write_file, edit_file, and run_command. \
These actions can require the user to approve them, so explain what you intend to do.",
        );
    }
    text.push_str(
        " For any multi-step task, start by calling set_plan with a list of the steps you will take. \
After each step completes, call complete_task with the zero-based index of that step. \
Do not describe your plan in prose — use set_plan instead.",
    );
    text.push_str(
        " CRITICAL — avoid redundant tool calls: All previous tool results are in your context. \
Do NOT re-read or re-list files and directories you have already inspected in this conversation. \
Before calling read_file or list_dir, check your conversation history first. \
Only call tools for information you do not yet have.",
    );
    text.push_str(
        " If a tool call fails or a command times out, do NOT retry it automatically. \
Stop immediately, report the exact error to the user, and wait for their input.",
    );
    text.push_str(" Never request or expose secrets such as API keys, SSH keys, or .env files.");
    text.push_str(
        " Use save_note/read_notes to persist important facts, decisions, or learnings \
beyond this conversation — notes survive across sessions.",
    );
    text
}

/// Whether a tool modifies the system and must pass the approval policy.
pub fn is_write_tool(name: &str) -> bool {
    matches!(
        name,
        "create_dir"
            | "write_file"
            | "edit_file"
            | "run_command"
            | "delete_file"
            | "move_file"
            | "copy_file"
            | "patch_file"
            | "git_commit"
            | "git_stash"
            | "git_branch"
            | "save_note"
            | "delete_note"
    )
}

/// Whether a tool name belongs to an MCP server (prefix mcp__).
pub fn is_mcp_tool(name: &str) -> bool {
    name.starts_with("mcp__")
}

/// A short, human-readable summary of a tool call for confirmation prompts.
pub fn describe_call(name: &str, arguments: &str) -> String {
    let args: Value = serde_json::from_str(arguments).unwrap_or(Value::Null);
    let field = |key: &str| args.get(key).and_then(Value::as_str).unwrap_or("");
    match name {
        "list_dir" => format!("list directory {}", field("path").if_empty(".")),
        "find_files" => format!(
            "find files matching `{}` under {}",
            field("query"),
            field("root").if_empty(".")
        ),
        "search_text" => format!(
            "search text `{}` under {}",
            field("query"),
            field("root").if_empty(".")
        ),
        "read_file" => format!("read file {}", field("path")),
        "web_search" => format!("web search `{}`", field("query")),
        "fetch_url" => format!("fetch {}", field("url")),
        "screenshot_url" => format!("screenshot {}", field("url")),
        "git_status" => "git status".to_string(),
        "git_diff" => {
            let path = field("path");
            if path.is_empty() {
                "git diff".to_string()
            } else {
                format!("git diff {path}")
            }
        }
        "git_log" => "git log".to_string(),
        "git_blame" => format!("git blame {}", field("path")),
        "git_show" => format!("git show {}", field("ref").if_empty("HEAD")),
        "git_stash" => format!("git stash {}", field("action").if_empty("list")),
        "git_branch" => {
            if !field("create").is_empty() {
                format!("git branch -b {}", field("create"))
            } else if !field("checkout").is_empty() {
                format!("git checkout {}", field("checkout"))
            } else if !field("delete").is_empty() {
                format!("git branch -d {}", field("delete"))
            } else {
                "git branch".to_string()
            }
        }
        "git_commit" => format!("git commit {}", field("message")),
        "patch_file" => format!("patch file {}", field("path")),
        "delete_file" => format!("delete {}", field("path")),
        "save_note" => format!("save note `{}`", field("key")),
        "read_notes" => format!(
            "read notes{}",
            if field("key").is_empty() {
                String::new()
            } else {
                format!(" `{}`", field("key"))
            }
        ),
        "search_notes" => format!("search notes `{}`", field("query")),
        "delete_note" => format!("delete note `{}`", field("key")),
        "move_file" => format!("move {} → {}", field("from"), field("to")),
        "copy_file" => format!("copy {} → {}", field("from"), field("to")),
        "create_dir" => format!("create directory {}", field("path")),
        "write_file" => format!("write file {}", field("path")),
        "edit_file" => format!("edit file {}", field("path")),
        "run_command" => format!("run command `{}`", field("command")),
        _ => format!("{name} {}", truncate(arguments, 80)),
    }
}

trait EmptyStrExt {
    fn if_empty(self, fallback: &'static str) -> Self;
}

impl<'a> EmptyStrExt for &'a str {
    fn if_empty(self, fallback: &'static str) -> Self {
        if self.is_empty() { fallback } else { self }
    }
}

pub fn definitions(include_write: bool) -> Vec<Value> {
    let mut definitions = vec![
        json!({
            "type": "function",
            "function": {
                "name": "set_plan",
                "description": "Display a numbered task checklist of what you will do. Call this once at the start of any multi-step task before taking any action.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "steps": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Ordered list of task descriptions."
                        }
                    },
                    "required": ["steps"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "complete_task",
                "description": "Mark a step in the current plan as completed. Call this immediately after finishing each step.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "index": {
                            "type": "integer",
                            "description": "Zero-based index of the completed step."
                        }
                    },
                    "required": ["index"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "list_dir",
                "description": "List files and directories at a path. Use this to inspect the current workspace or nearby folders.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Directory path. Relative paths resolve from the terminal cwd." }
                    }
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "find_files",
                "description": "Search file and directory names recursively under a root path. Can search outside the project when given an absolute path.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "root": { "type": "string", "description": "Root directory. Defaults to the terminal cwd." },
                        "query": { "type": "string", "description": "Case-insensitive filename substring to find." }
                    },
                    "required": ["query"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "search_text",
                "description": "Search text content recursively under a root path. Skips large, binary, generated, and sensitive files.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "root": { "type": "string", "description": "Root directory. Defaults to the terminal cwd." },
                        "query": { "type": "string", "description": "Case-insensitive text to search for." }
                    },
                    "required": ["query"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "read_file",
                "description": "Read a capped range from a text file. Sensitive files such as secrets and SSH keys are blocked.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "File path. Relative paths resolve from the terminal cwd." },
                        "start_line": { "type": "integer", "minimum": 1, "description": "1-based line to start from." },
                        "max_lines": { "type": "integer", "minimum": 1, "maximum": 200, "description": "Maximum lines to return." }
                    },
                    "required": ["path"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "web_search",
                "description": "Do a basic web lookup for public information outside the local project.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Search query." }
                    },
                    "required": ["query"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "fetch_url",
                "description": "Fetch a URL. mode=\"text\" (default): returns plain text with HTML tags stripped. mode=\"raw\": returns the full HTML source unchanged. mode=\"deep\": returns HTML source PLUS the full content of every linked CSS file (and JS bundles if include_js=true) in one call — use this when you need to inspect design tokens, Tailwind classes, color variables, font imports, or component structure without multiple round-trips.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "url": { "type": "string", "description": "URL to fetch." },
                        "mode": { "type": "string", "description": "\"text\" (default, strips HTML), \"raw\" (full HTML source), \"deep\" (HTML source + fetch all linked CSS assets, and JS if include_js=true)." },
                        "max_chars": { "type": "integer", "description": "Max chars per resource (default 40000 for text, 60000 for raw/deep HTML, 30000 per asset)." },
                        "include_js": { "type": "boolean", "description": "deep mode only — also fetch linked JS bundles (default false; bundles can be large)." }
                    },
                    "required": ["url"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "screenshot_url",
                "description": "Take a full-page or viewport screenshot of a URL using a headless browser (Playwright). Returns the saved file path and a note. Use when you need to visually inspect a web page, compare UI designs, or verify a running app.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "url": { "type": "string", "description": "URL to screenshot." },
                        "output": { "type": "string", "description": "File path to save the PNG (default: /tmp/anveesa-screenshot-<timestamp>.png)." },
                        "width": { "type": "integer", "description": "Viewport width in pixels (default 1440)." },
                        "height": { "type": "integer", "description": "Viewport height in pixels (default 900)." },
                        "full_page": { "type": "boolean", "description": "Capture the full scrollable page (default false)." }
                    },
                    "required": ["url"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "git_status",
                "description": "Show the current git branch, staged/unstaged changes, and untracked files.",
                "parameters": { "type": "object", "properties": {} }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "git_diff",
                "description": "Show the git diff. Optionally limit to staged changes or a specific file path.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "staged": { "type": "boolean", "description": "Show staged diff (git diff --staged). Default false." },
                        "path":   { "type": "string",  "description": "Limit diff to this file or directory." },
                        "ref":    { "type": "string",  "description": "Diff against this ref (e.g. HEAD~1, main)." }
                    }
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "git_log",
                "description": "Show recent git commit history.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "n":    { "type": "integer", "description": "Number of commits (default 20, max 100)." },
                        "path": { "type": "string",  "description": "Limit to commits touching this path." }
                    }
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "git_blame",
                "description": "Show who last modified each line of a file (git blame).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path":       { "type": "string",  "description": "File path to blame." },
                        "start_line": { "type": "integer", "description": "First line (1-based)." },
                        "end_line":   { "type": "integer", "description": "Last line (1-based)." }
                    },
                    "required": ["path"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "git_show",
                "description": "Show the contents or diff of a specific commit or object.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "ref":  { "type": "string", "description": "Commit ref (e.g. HEAD, abc123, HEAD~2). Default HEAD." },
                        "path": { "type": "string", "description": "Limit output to this file." }
                    }
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "read_notes",
                "description": "Read your persistent notes. Omit key to list all notes with previews.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "key": { "type": "string", "description": "Note key to read. Omit to list all." }
                    }
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "search_notes",
                "description": "Search text across all saved notes.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Text to search for." }
                    },
                    "required": ["query"]
                }
            }
        }),
    ];

    if include_write {
        definitions.extend([
            json!({
                "type": "function",
                "function": {
                    "name": "save_note",
                    "description": "Save a persistent note that survives across sessions. Use to remember facts, decisions, learnings, or preferences.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "key":     { "type": "string",  "description": "Short identifier (e.g. 'project-decisions', 'bug-fixes')." },
                            "content": { "type": "string",  "description": "Markdown content to save." },
                            "append":  { "type": "boolean", "description": "Append to existing note instead of replacing. Default false." }
                        },
                        "required": ["key", "content"]
                    }
                }
            }),
            json!({
                "type": "function",
                "function": {
                    "name": "delete_note",
                    "description": "Delete a saved note.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "key": { "type": "string", "description": "Note key to delete." }
                        },
                        "required": ["key"]
                    }
                }
            }),
            json!({
                "type": "function",
                "function": {
                    "name": "create_dir",
                    "description": "Create a directory, including parent directories as needed. Use this for requests to make folders.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string", "description": "Directory path. Relative paths resolve from the terminal cwd." }
                        },
                        "required": ["path"]
                    }
                }
            }),
            json!({
                "type": "function",
                "function": {
                    "name": "write_file",
                    "description": "Create or overwrite a text file with the given content. Parent directories are created as needed.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string", "description": "File path. Relative paths resolve from the terminal cwd." },
                            "content": { "type": "string", "description": "Full file content to write." }
                        },
                        "required": ["path", "content"]
                    }
                }
            }),
            json!({
                "type": "function",
                "function": {
                    "name": "edit_file",
                    "description": "Replace a single, unique occurrence of old_string with new_string in an existing text file.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string", "description": "File path. Relative paths resolve from the terminal cwd." },
                            "old_string": { "type": "string", "description": "Exact text to replace. Must appear exactly once." },
                            "new_string": { "type": "string", "description": "Replacement text." }
                        },
                        "required": ["path", "old_string", "new_string"]
                    }
                }
            }),
            json!({
                "type": "function",
                "function": {
                    "name": "run_command",
                    "description": "Run a shell command in the terminal cwd and return its output. Use for builds, tests, and tasks not covered by other tools.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "command": { "type": "string", "description": "Shell command line to execute." },
                            "timeout_secs": { "type": "integer", "minimum": 1, "maximum": 300, "description": "Optional timeout in seconds (default 60)." }
                        },
                        "required": ["command"]
                    }
                }
            }),
            json!({
                "type": "function",
                "function": {
                    "name": "patch_file",
                    "description": "Apply multiple targeted replacements to a file in one call. Each patch must match exactly once. Prefer this over multiple edit_file calls when editing the same file.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string", "description": "File path." },
                            "patches": {
                                "type": "array",
                                "description": "Ordered list of replacements to apply sequentially.",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "old_string": { "type": "string", "description": "Unique text to replace." },
                                        "new_string": { "type": "string", "description": "Replacement text." }
                                    },
                                    "required": ["old_string", "new_string"]
                                }
                            }
                        },
                        "required": ["path", "patches"]
                    }
                }
            }),
            json!({
                "type": "function",
                "function": {
                    "name": "delete_file",
                    "description": "Delete a file or empty directory. Use with care — this is irreversible.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string", "description": "Path to delete." }
                        },
                        "required": ["path"]
                    }
                }
            }),
            json!({
                "type": "function",
                "function": {
                    "name": "move_file",
                    "description": "Move or rename a file or directory.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "from": { "type": "string", "description": "Source path." },
                            "to":   { "type": "string", "description": "Destination path." }
                        },
                        "required": ["from", "to"]
                    }
                }
            }),
            json!({
                "type": "function",
                "function": {
                    "name": "copy_file",
                    "description": "Copy a file to a new location. Parent directories are created as needed.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "from": { "type": "string", "description": "Source file path." },
                            "to":   { "type": "string", "description": "Destination path." }
                        },
                        "required": ["from", "to"]
                    }
                }
            }),
            json!({
                "type": "function",
                "function": {
                    "name": "git_stash",
                    "description": "Save or restore git stash. action: push|pop|list|drop.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "action":  { "type": "string",  "description": "push, pop, list, or drop." },
                            "message": { "type": "string",  "description": "Stash message (only for push)." }
                        }
                    }
                }
            }),
            json!({
                "type": "function",
                "function": {
                    "name": "git_branch",
                    "description": "List, create, checkout, or delete git branches.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "create":   { "type": "string", "description": "Create and switch to a new branch with this name." },
                            "checkout": { "type": "string", "description": "Switch to an existing branch." },
                            "delete":   { "type": "string", "description": "Delete a branch." }
                        }
                    }
                }
            }),
            json!({
                "type": "function",
                "function": {
                    "name": "git_commit",
                    "description": "Create a git commit with the given message. Optionally stage all changes first.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "message": { "type": "string",  "description": "Commit message." },
                            "add_all": { "type": "boolean", "description": "Run git add -A before committing." }
                        },
                        "required": ["message"]
                    }
                }
            }),
        ]);
    }

    definitions
}

pub async fn run(name: &str, arguments: &str) -> String {
    let result = match name {
        "list_dir" => list_dir(arguments).await,
        "find_files" => find_files(arguments).await,
        "search_text" => search_text(arguments).await,
        "read_file" => read_file(arguments).await,
        "web_search" => web_search(arguments).await,
        "fetch_url" => fetch_url(arguments).await,
        "git_status" => git_status(arguments).await,
        "git_diff" => git_diff(arguments).await,
        "git_log" => git_log(arguments).await,
        "git_blame" => git_blame(arguments).await,
        "git_show" => git_show(arguments).await,
        "git_stash" => git_stash(arguments).await,
        "git_branch" => git_branch(arguments).await,
        "git_commit" => git_commit(arguments).await,
        "patch_file" => patch_file(arguments).await,
        "delete_file" => delete_file(arguments).await,
        "save_note" => save_note(arguments).await,
        "read_notes" => read_notes(arguments).await,
        "search_notes" => search_notes(arguments).await,
        "delete_note" => delete_note(arguments).await,
        "move_file" => move_file(arguments).await,
        "copy_file" => copy_file(arguments).await,
        "create_dir" => create_dir(arguments).await,
        "write_file" => write_file(arguments).await,
        "edit_file" => edit_file(arguments).await,
        "run_command" => run_command(arguments).await,
        "screenshot_url" => screenshot_url(arguments).await,
        _ => Err(anyhow!("unknown tool '{name}'")),
    };

    match result {
        Ok(value) => value.to_string(),
        Err(error) => json!({
            "ok": false,
            "error": error.to_string()
        })
        .to_string(),
    }
}

async fn list_dir(arguments: &str) -> Result<Value> {
    let args: PathArgs = parse_args(arguments)?;
    let path = resolve_path(args.path.as_deref().unwrap_or("."))?;
    if !path.is_dir() {
        bail!("{} is not a directory", path.display());
    }

    let mut entries = Vec::new();
    let mut truncated = false;
    for entry in
        fs::read_dir(&path).with_context(|| format!("failed to read {}", path.display()))?
    {
        let entry = entry?;
        let entry_path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if should_skip_name(&name) {
            continue;
        }

        entries.push(json!({
            "name": name,
            "path": entry_path.display().to_string(),
            "kind": path_kind(&entry_path),
        }));
        if entries.len() >= MAX_DIR_ENTRIES {
            truncated = true;
            break;
        }
    }

    Ok(json!({
        "ok": true,
        "path": path.display().to_string(),
        "entries": entries,
        "truncated": truncated,
    }))
}

async fn find_files(arguments: &str) -> Result<Value> {
    let args: SearchArgs = parse_args(arguments)?;
    let query = normalized_query(&args.query)?;
    let root = resolve_path(args.root.as_deref().unwrap_or("."))?;
    if !root.is_dir() {
        bail!("{} is not a directory", root.display());
    }

    let mut results = Vec::new();
    let mut truncated = false;
    walk_paths(&root, |path| {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            return Ok(true);
        };
        if name.to_lowercase().contains(&query) {
            results.push(json!({
                "path": path.display().to_string(),
                "kind": path_kind(path),
            }));
            if results.len() >= MAX_SEARCH_RESULTS {
                truncated = true;
                return Ok(false);
            }
        }
        Ok(true)
    })?;

    Ok(json!({
        "ok": true,
        "root": root.display().to_string(),
        "query": args.query,
        "results": results,
        "truncated": truncated,
    }))
}

async fn search_text(arguments: &str) -> Result<Value> {
    let args: SearchArgs = parse_args(arguments)?;
    let query = normalized_query(&args.query)?;
    let root = resolve_path(args.root.as_deref().unwrap_or("."))?;
    if !root.is_dir() {
        bail!("{} is not a directory", root.display());
    }

    let mut results = Vec::new();
    let mut truncated = false;
    walk_paths(&root, |path| {
        if !path.is_file() || is_sensitive_path(path) || !is_small_text_candidate(path) {
            return Ok(true);
        }

        let Ok(content) = fs::read_to_string(path) else {
            return Ok(true);
        };
        for (i, line) in content.lines().enumerate() {
            if line.to_lowercase().contains(&query) {
                results.push(json!({
                    "path": path.display().to_string(),
                    "line": i + 1,
                    "preview": truncate(line.trim(), 240),
                }));
                if results.len() >= MAX_SEARCH_RESULTS {
                    truncated = true;
                    return Ok(false);
                }
            }
        }

        Ok(true)
    })?;

    Ok(json!({
        "ok": true,
        "root": root.display().to_string(),
        "query": args.query,
        "results": results,
        "truncated": truncated,
    }))
}

async fn read_file(arguments: &str) -> Result<Value> {
    let args: ReadFileArgs = parse_args(arguments)?;
    let path = resolve_path(&args.path)?;
    if !path.is_file() {
        bail!("{} is not a file", path.display());
    }
    if is_sensitive_path(&path) {
        bail!("refusing to read sensitive-looking file {}", path.display());
    }
    if !is_small_text_candidate(&path) {
        bail!("file is too large or not a safe text candidate");
    }

    let start_line = args.start_line.unwrap_or(1).max(1);
    let max_lines = args.max_lines.unwrap_or(120).clamp(1, MAX_READ_LINES);

    // Smart cache: if the file hasn't changed since we last read it, use cached content
    let mtime = fs::metadata(&path).and_then(|m| m.modified()).ok();
    let content = if let Some(mtime) = mtime {
        let cache_key = (path.clone(), mtime);
        let cached = file_cache()
            .lock()
            .ok()
            .and_then(|c| c.get(&cache_key).cloned());
        if let Some(c) = cached {
            c
        } else {
            let c = fs::read_to_string(&path)
                .with_context(|| format!("failed to read {}", path.display()))?;
            if let Ok(mut cache) = file_cache().lock() {
                // Evict old entry for this path if any
                cache.retain(|(p, _), _| p != &path);
                cache.insert(cache_key, c.clone());
            }
            c
        }
    } else {
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?
    };
    let lines = content
        .lines()
        .enumerate()
        .skip(start_line - 1)
        .take(max_lines)
        .map(|(index, line)| {
            json!({
                "line": index + 1,
                "text": line
            })
        })
        .collect::<Vec<_>>();

    Ok(json!({
        "ok": true,
        "path": path.display().to_string(),
        "start_line": start_line,
        "lines": lines
    }))
}

fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .user_agent(concat!("anveesa-cli/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("failed to build HTTP client")
    })
}

async fn web_search(arguments: &str) -> Result<Value> {
    let args: WebSearchArgs = parse_args(arguments)?;
    let query = args.query.trim();
    if query.is_empty() {
        bail!("query is empty");
    }

    // 1. Brave Search API (best quality, free tier available)
    if let Ok(key) = std::env::var("BRAVE_SEARCH_API_KEY") {
        if let Ok(results) = search_brave(query, &key).await {
            if !results.is_empty() {
                return Ok(
                    json!({ "ok": true, "query": query, "source": "brave", "results": results }),
                );
            }
        }
    }

    // 2. Serper.dev (Google results via API)
    if let Ok(key) = std::env::var("SERPER_API_KEY") {
        if let Ok(results) = search_serper(query, &key).await {
            if !results.is_empty() {
                return Ok(
                    json!({ "ok": true, "query": query, "source": "serper", "results": results }),
                );
            }
        }
    }

    // 3. DuckDuckGo instant-answer API (no key needed, limited results)
    let api_url = format!(
        "https://api.duckduckgo.com/?q={}&format=json&no_html=1&skip_disambig=1",
        percent_encode(query)
    );
    let mut results = Vec::new();
    if let Ok(resp) = http_client().get(&api_url).send().await {
        if let Ok(response) = resp.json::<Value>().await {
            if let Some(abstract_text) = response.get("AbstractText").and_then(Value::as_str)
                && !abstract_text.is_empty()
            {
                results.push(json!({
                    "title": response.get("Heading").and_then(Value::as_str).unwrap_or(""),
                    "snippet": abstract_text,
                    "url": response.get("AbstractURL").and_then(Value::as_str).unwrap_or("")
                }));
            }
            collect_related_topics(response.get("RelatedTopics"), &mut results);
        }
    }

    // 4. DuckDuckGo lite HTML fallback
    if results.is_empty() {
        let lite_url = format!(
            "https://lite.duckduckgo.com/lite/?q={}",
            percent_encode(query)
        );
        if let Ok(resp) = http_client()
            .get(&lite_url)
            .header("Accept-Language", "en-US,en;q=0.9")
            .header(
                "User-Agent",
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36",
            )
            .send()
            .await
        {
            if let Ok(body) = resp.text().await {
                results = scrape_ddg_lite(&body, 8);
            }
        }
    }

    results.truncate(10);
    let source = if results.is_empty() {
        "none"
    } else {
        "duckduckgo"
    };
    Ok(json!({ "ok": true, "query": query, "source": source, "results": results }))
}

async fn search_brave(query: &str, api_key: &str) -> Result<Vec<Value>> {
    let url = format!(
        "https://api.search.brave.com/res/v1/web/search?q={}&count=10&search_lang=en",
        percent_encode(query)
    );
    let resp = http_client()
        .get(&url)
        .header("Accept", "application/json")
        .header("Accept-Encoding", "gzip")
        .header("X-Subscription-Token", api_key)
        .send()
        .await
        .context("Brave Search request failed")?;

    if !resp.status().is_success() {
        bail!("Brave Search HTTP {}", resp.status());
    }
    let body: Value = resp
        .json()
        .await
        .context("failed to parse Brave response")?;
    let results = body["web"]["results"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    Ok(results
        .into_iter()
        .filter_map(|r| {
            let title = r["title"].as_str()?;
            let url = r["url"].as_str()?;
            let snip = r["description"].as_str().unwrap_or("");
            Some(json!({ "title": title, "snippet": snip, "url": url }))
        })
        .collect())
}

async fn search_serper(query: &str, api_key: &str) -> Result<Vec<Value>> {
    let resp = http_client()
        .post("https://google.serper.dev/search")
        .header("X-API-KEY", api_key)
        .header("Content-Type", "application/json")
        .json(&json!({ "q": query, "num": 10 }))
        .send()
        .await
        .context("Serper request failed")?;

    if !resp.status().is_success() {
        bail!("Serper HTTP {}", resp.status());
    }
    let body: Value = resp
        .json()
        .await
        .context("failed to parse Serper response")?;
    let results = body["organic"].as_array().cloned().unwrap_or_default();
    Ok(results
        .into_iter()
        .filter_map(|r| {
            let title = r["title"].as_str()?;
            let url = r["link"].as_str()?;
            let snip = r["snippet"].as_str().unwrap_or("");
            Some(json!({ "title": title, "snippet": snip, "url": url }))
        })
        .collect())
}

/// Scrape DuckDuckGo lite (text-only) results page.
fn scrape_ddg_lite(html: &str, max: usize) -> Vec<Value> {
    let mut results = Vec::new();
    let mut pos = 0;
    while results.len() < max {
        // DDG lite uses <a class="result-link"> for result links
        let Some(a_pos) = html[pos..].find("class=\"result-link\"") else {
            break;
        };
        let block = pos + a_pos;

        let url = extract_attr(&html[block..block.min(html.len()).min(block + 300)], "href")
            .map(|u| clean_ddg_url(u))
            .unwrap_or_default();
        let title = extract_tag_text(&html[block..block.min(html.len()).min(block + 300)], "a")
            .unwrap_or_default();

        // Snippet is in the next table cell after the result
        let snip_window_end = (block + 800).min(html.len());
        let snippet = html[block..snip_window_end]
            .find("result-snippet")
            .and_then(|s| extract_tag_text(&html[block + s..snip_window_end], "td"))
            .unwrap_or_default();

        if !title.is_empty() && !url.is_empty() {
            results.push(json!({ "title": title, "snippet": snippet, "url": url }));
        }
        pos = block + 10;
    }
    results
}

fn tag_attr(tag: &str, attr: &str) -> Option<String> {
    let dq = format!("{attr}=\"");
    let sq = format!("{attr}='");
    if let Some(s) = tag.find(&dq) {
        let start = s + dq.len();
        tag[start..]
            .find('"')
            .map(|e| tag[start..start + e].to_string())
    } else if let Some(s) = tag.find(&sq) {
        let start = s + sq.len();
        tag[start..]
            .find('\'')
            .map(|e| tag[start..start + e].to_string())
    } else {
        None
    }
}

fn url_origin(url: &str) -> String {
    let skip = if url.starts_with("https://") {
        8
    } else if url.starts_with("http://") {
        7
    } else {
        return String::new();
    };
    let scheme = &url[..skip - 3];
    let host = url[skip..].split('/').next().unwrap_or("");
    format!("{scheme}://{host}")
}

fn url_base_path(url: &str) -> String {
    let skip = if url.starts_with("https://") {
        8
    } else if url.starts_with("http://") {
        7
    } else {
        return "/".to_string();
    };
    let rest = &url[skip..];
    let path = rest
        .split_once('/')
        .map(|(_, p)| format!("/{p}"))
        .unwrap_or_default();
    path.rfind('/')
        .map(|i| path[..i + 1].to_string())
        .unwrap_or_else(|| "/".to_string())
}

fn resolve_asset_url(href: &str, origin: &str, base_path: &str) -> Option<String> {
    let h = href.trim();
    if h.is_empty() {
        return None;
    }
    if h.starts_with("http://") || h.starts_with("https://") {
        Some(h.to_string())
    } else if h.starts_with("//") {
        let scheme = if origin.starts_with("https") {
            "https"
        } else {
            "http"
        };
        Some(format!("{scheme}:{h}"))
    } else if h.starts_with('/') {
        if origin.is_empty() {
            None
        } else {
            Some(format!("{origin}{h}"))
        }
    } else if !origin.is_empty() {
        Some(format!("{origin}{base_path}{h}"))
    } else {
        None
    }
}

fn extract_asset_urls(html: &str, base_url: &str, include_js: bool) -> Vec<String> {
    let origin = url_origin(base_url);
    let base_path = url_base_path(base_url);
    let mut urls: Vec<String> = Vec::new();
    let mut pos = 0;

    while pos < html.len() {
        let Some(lt) = html[pos..].find('<') else {
            break;
        };
        let abs = pos + lt;
        let Some(gt) = html[abs..].find('>') else {
            break;
        };
        let tag = &html[abs..abs + gt + 1];
        let tag_lo = tag.to_lowercase();
        pos = abs + gt + 1;

        let href = if tag_lo.starts_with("<link") {
            let rel = tag_attr(&tag_lo, "rel").unwrap_or_default();
            let as_ = tag_attr(&tag_lo, "as").unwrap_or_default();
            if rel == "stylesheet" || (rel == "preload" && as_ == "style") {
                tag_attr(tag, "href").or_else(|| tag_attr(&tag_lo, "href"))
            } else {
                None
            }
        } else if include_js && tag_lo.starts_with("<script") {
            tag_attr(tag, "src").or_else(|| tag_attr(&tag_lo, "src"))
        } else {
            None
        };

        if let Some(h) = href {
            if let Some(resolved) = resolve_asset_url(&h, &origin, &base_path) {
                if !urls.contains(&resolved) {
                    urls.push(resolved);
                }
            }
        }
    }
    urls
}

fn extract_attr<'a>(html: &'a str, attr: &str) -> Option<&'a str> {
    let key = format!("{attr}=\"");
    let start = html.find(&key)? + key.len();
    let end = html[start..].find('"')? + start;
    Some(&html[start..end])
}

fn extract_tag_text(html: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}");
    let start = html.find(&open)?;
    let inner_start = html[start..].find('>')? + start + 1;
    let close = format!("</{tag}>");
    let end = html[inner_start..].find(&close)? + inner_start;
    let raw = &html[inner_start..end];
    let text = html_to_text(raw);
    if text.trim().is_empty() {
        None
    } else {
        Some(text.trim().to_string())
    }
}

fn clean_ddg_url(raw: &str) -> String {
    // DDG wraps URLs in redirect: //duckduckgo.com/l/?uddg=https%3A%2F%2F...
    if let Some(i) = raw.find("uddg=") {
        let encoded = &raw[i + 5..];
        let decoded = encoded
            .replace("%3A", ":")
            .replace("%2F", "/")
            .replace("%3F", "?")
            .replace("%3D", "=")
            .replace("%26", "&");
        decoded.split('&').next().unwrap_or(&decoded).to_string()
    } else if raw.starts_with("//") {
        format!("https:{raw}")
    } else {
        raw.to_string()
    }
}

// ── fetch_url ─────────────────────────────────────────────────────────────────

async fn fetch_url(arguments: &str) -> Result<Value> {
    #[derive(Deserialize)]
    struct Args {
        url: String,
        #[serde(default)]
        max_chars: Option<usize>,
        #[serde(default)]
        mode: Option<String>,
        #[serde(default)]
        include_js: Option<bool>,
    }
    let args: Args = parse_args(arguments)?;
    let url = args.url.trim().to_string();
    if url.is_empty() {
        bail!("url is required");
    }
    let mode = args.mode.as_deref().unwrap_or("text").to_string();

    let response = http_client()
        .get(&url)
        .send()
        .await
        .with_context(|| format!("failed to fetch {url}"))?;

    let status = response.status();
    if !status.is_success() {
        bail!("HTTP {status}");
    }

    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("text/plain")
        .to_string();

    let body = response
        .text()
        .await
        .context("failed to read response body")?;

    match mode.as_str() {
        "raw" => {
            let max = args.max_chars.unwrap_or(80_000);
            let char_count = body.chars().count();
            let truncated = char_count > max;
            let html: String = body.chars().take(max).collect();
            Ok(json!({
                "ok": true,
                "url": url,
                "content_type": content_type,
                "html": html,
                "char_count": char_count,
                "truncated": truncated,
            }))
        }
        "deep" => {
            const ASSET_MAX: usize = 30_000;
            const MAX_ASSETS: usize = 10;
            let html_max = args.max_chars.unwrap_or(60_000);
            let include_js = args.include_js.unwrap_or(false);

            let asset_urls: Vec<String> = extract_asset_urls(&body, &url, include_js)
                .into_iter()
                .take(MAX_ASSETS)
                .collect();

            let mut handles = Vec::new();
            for asset_url in asset_urls {
                handles.push(tokio::spawn(async move {
                    let Ok(resp) = http_client().get(&asset_url).send().await else {
                        return None;
                    };
                    if !resp.status().is_success() {
                        return None;
                    }
                    let ct = resp
                        .headers()
                        .get("content-type")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("")
                        .to_string();
                    let Ok(content) = resp.text().await else {
                        return None;
                    };
                    let kind = if ct.contains("css") || asset_url.ends_with(".css") {
                        "css"
                    } else if ct.contains("javascript") || asset_url.contains(".js") {
                        "js"
                    } else {
                        "other"
                    };
                    let char_count = content.chars().count();
                    let truncated = char_count > ASSET_MAX;
                    let trimmed: String = content.chars().take(ASSET_MAX).collect();
                    Some(json!({
                        "url": asset_url,
                        "type": kind,
                        "char_count": char_count,
                        "truncated": truncated,
                        "content": trimmed,
                    }))
                }));
            }

            let mut assets: Vec<Value> = Vec::new();
            for h in handles {
                if let Ok(Some(a)) = h.await {
                    assets.push(a);
                }
            }

            let html_chars = body.chars().count();
            let html_truncated = html_chars > html_max;
            let html: String = body.chars().take(html_max).collect();

            Ok(json!({
                "ok": true,
                "url": url,
                "html": html,
                "html_chars": html_chars,
                "html_truncated": html_truncated,
                "assets": assets,
            }))
        }
        _ => {
            // "text" mode — current behaviour
            let max = args.max_chars.unwrap_or(40_000);
            let text = if content_type.contains("html") || content_type.contains("xml") {
                html_to_text(&body)
            } else {
                body
            };
            let char_count = text.chars().count();
            let truncated = char_count > max;
            let text: String = text.chars().take(max).collect();
            Ok(json!({
                "ok": true,
                "url": url,
                "content_type": content_type,
                "text": text,
                "truncated": truncated,
            }))
        }
    }
}

fn html_to_text(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    let mut tag_buf = String::new();
    let mut skip_close: Option<String> = None;

    for c in html.chars() {
        if let Some(ref close) = skip_close {
            tag_buf.push(c);
            if tag_buf.to_lowercase().ends_with(&format!("</{}>", close)) {
                skip_close = None;
                tag_buf.clear();
            }
            continue;
        }
        if c == '<' {
            in_tag = true;
            tag_buf.clear();
        } else if c == '>' {
            in_tag = false;
            let raw = tag_buf.trim().to_lowercase();
            let name = raw
                .trim_start_matches('/')
                .split_whitespace()
                .next()
                .unwrap_or("");
            if matches!(name, "script" | "style") && !raw.starts_with('/') {
                skip_close = Some(name.to_string());
            }
            if matches!(
                name,
                "p" | "div"
                    | "h1"
                    | "h2"
                    | "h3"
                    | "h4"
                    | "h5"
                    | "h6"
                    | "br"
                    | "li"
                    | "tr"
                    | "section"
                    | "article"
            ) {
                out.push('\n');
            }
            tag_buf.clear();
        } else if in_tag {
            tag_buf.push(c);
        } else {
            out.push(c);
        }
    }

    let out = out
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
        .replace("&#x27;", "'")
        .replace("&#x2F;", "/");

    // Collapse blank lines
    let mut result = String::new();
    let mut blank = 0usize;
    for line in out.lines() {
        let t = line.trim();
        if t.is_empty() {
            blank += 1;
            if blank <= 1 {
                result.push('\n');
            }
        } else {
            blank = 0;
            result.push_str(t);
            result.push('\n');
        }
    }
    result.trim().to_string()
}

// ── git tools ─────────────────────────────────────────────────────────────────

async fn git_status(_arguments: &str) -> Result<Value> {
    let out = tokio::process::Command::new("git")
        .args(["status", "-sb"])
        .kill_on_drop(true)
        .output()
        .await
        .context("failed to run git")?;
    Ok(json!({
        "ok": out.status.success(),
        "output": String::from_utf8_lossy(&out.stdout).trim().to_string(),
        "error": if !out.status.success() { Some(String::from_utf8_lossy(&out.stderr).trim().to_string()) } else { None },
    }))
}

async fn git_diff(arguments: &str) -> Result<Value> {
    #[derive(Deserialize, Default)]
    struct Args {
        #[serde(default)]
        staged: bool,
        #[serde(default)]
        path: Option<String>,
        #[serde(rename = "ref", default)]
        refspec: Option<String>,
    }
    let args: Args = serde_json::from_str(arguments).unwrap_or_default();
    let mut cmd = tokio::process::Command::new("git");
    cmd.arg("diff").kill_on_drop(true);
    if args.staged {
        cmd.arg("--staged");
    }
    if let Some(r) = &args.refspec {
        cmd.arg(r);
    }
    if let Some(p) = &args.path {
        cmd.arg("--").arg(p);
    }
    let out = cmd.output().await.context("failed to run git diff")?;
    let diff = String::from_utf8_lossy(&out.stdout).to_string();
    let truncated = diff.len() > 30_000;
    Ok(json!({
        "ok": true,
        "diff": if truncated { &diff[..30_000] } else { &diff },
        "truncated": truncated,
    }))
}

async fn git_log(arguments: &str) -> Result<Value> {
    #[derive(Deserialize)]
    struct Args {
        #[serde(default = "default_n")]
        n: usize,
        #[serde(default)]
        path: Option<String>,
    }
    fn default_n() -> usize {
        20
    }
    let args: Args = serde_json::from_str(arguments).unwrap_or(Args { n: 20, path: None });
    let n = args.n.clamp(1, 100);
    let mut cmd = tokio::process::Command::new("git");
    cmd.args(["log", "--oneline", "--decorate", &format!("-{n}")])
        .kill_on_drop(true);
    if let Some(p) = &args.path {
        cmd.arg("--").arg(p);
    }
    let out = cmd.output().await.context("failed to run git log")?;
    Ok(json!({
        "ok": out.status.success(),
        "log": String::from_utf8_lossy(&out.stdout).trim().to_string(),
        "error": if !out.status.success() { Some(String::from_utf8_lossy(&out.stderr).trim().to_string()) } else { None },
    }))
}

async fn git_blame(arguments: &str) -> Result<Value> {
    #[derive(Deserialize)]
    struct Args {
        path: String,
        #[serde(default)]
        start_line: Option<usize>,
        #[serde(default)]
        end_line: Option<usize>,
    }
    let args: Args = parse_args(arguments)?;
    let mut cmd = tokio::process::Command::new("git");
    cmd.args(["blame", "-s"]).kill_on_drop(true);
    if let (Some(s), Some(e)) = (args.start_line, args.end_line) {
        cmd.arg(format!("-L{s},{e}"));
    } else if let Some(s) = args.start_line {
        cmd.arg(format!("-L{s},+50"));
    }
    cmd.arg(&args.path);
    let out = cmd.output().await.context("failed to run git blame")?;
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let truncated = text.len() > 20_000;
    Ok(json!({
        "ok": out.status.success(),
        "blame": if truncated { &text[..20_000] } else { &text },
        "truncated": truncated,
        "error": if !out.status.success() { Some(String::from_utf8_lossy(&out.stderr).trim().to_string()) } else { None },
    }))
}

async fn git_show(arguments: &str) -> Result<Value> {
    #[derive(Deserialize, Default)]
    struct Args {
        #[serde(rename = "ref", default)]
        refspec: Option<String>,
        #[serde(default)]
        path: Option<String>,
    }
    let args: Args = serde_json::from_str(arguments).unwrap_or_default();
    let mut cmd = tokio::process::Command::new("git");
    cmd.arg("show").kill_on_drop(true);
    cmd.arg(args.refspec.as_deref().unwrap_or("HEAD"));
    if let Some(p) = &args.path {
        cmd.arg("--").arg(p);
    }
    let out = cmd.output().await.context("failed to run git show")?;
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let truncated = text.len() > 20_000;
    Ok(json!({
        "ok": out.status.success(),
        "output": if truncated { &text[..20_000] } else { &text },
        "truncated": truncated,
    }))
}

async fn git_stash(arguments: &str) -> Result<Value> {
    #[derive(Deserialize, Default)]
    struct Args {
        #[serde(default)]
        action: Option<String>,
        #[serde(default)]
        message: Option<String>,
    }
    let args: Args = serde_json::from_str(arguments).unwrap_or_default();
    let action = args.action.as_deref().unwrap_or("list");
    let mut cmd = tokio::process::Command::new("git");
    cmd.arg("stash").kill_on_drop(true);
    match action {
        "push" => {
            cmd.arg("push");
            if let Some(m) = &args.message {
                cmd.arg("-m").arg(m);
            }
        }
        "pop" => {
            cmd.arg("pop");
        }
        "drop" => {
            cmd.arg("drop");
        }
        _ => {
            cmd.arg("list");
        }
    }
    let out = cmd.output().await.context("failed to run git stash")?;
    Ok(json!({
        "ok": out.status.success(),
        "output": String::from_utf8_lossy(&out.stdout).trim().to_string(),
        "error": if !out.status.success() { Some(String::from_utf8_lossy(&out.stderr).trim().to_string()) } else { None },
    }))
}

async fn git_branch(arguments: &str) -> Result<Value> {
    #[derive(Deserialize, Default)]
    struct Args {
        #[serde(default)]
        create: Option<String>,
        #[serde(default)]
        checkout: Option<String>,
        #[serde(default)]
        delete: Option<String>,
    }
    let args: Args = serde_json::from_str(arguments).unwrap_or_default();
    let (git_args, key, val): (Vec<&str>, &str, &str) = if let Some(name) = &args.create {
        (vec!["checkout", "-b", name], "created", name)
    } else if let Some(name) = &args.checkout {
        (vec!["checkout", name], "checked_out", name)
    } else if let Some(name) = &args.delete {
        (vec!["branch", "-d", name], "deleted", name)
    } else {
        let out = tokio::process::Command::new("git")
            .args(["branch", "-a"])
            .kill_on_drop(true)
            .output()
            .await
            .context("failed to run git branch")?;
        return Ok(
            json!({ "ok": out.status.success(), "branches": String::from_utf8_lossy(&out.stdout).trim().to_string() }),
        );
    };
    let out = tokio::process::Command::new("git")
        .args(&git_args)
        .kill_on_drop(true)
        .output()
        .await
        .context("failed to run git branch")?;
    Ok(json!({
        "ok": out.status.success(),
        key: val,
        "error": if !out.status.success() { Some(String::from_utf8_lossy(&out.stderr).trim().to_string()) } else { None },
    }))
}

async fn git_commit(arguments: &str) -> Result<Value> {
    #[derive(Deserialize)]
    struct Args {
        message: String,
        #[serde(default)]
        add_all: bool,
    }
    let args: Args = parse_args(arguments)?;
    if args.message.trim().is_empty() {
        bail!("commit message is required");
    }
    if args.add_all {
        tokio::process::Command::new("git")
            .args(["add", "-A"])
            .kill_on_drop(true)
            .output()
            .await
            .context("failed to git add")?;
    }
    let out = tokio::process::Command::new("git")
        .args(["commit", "-m", &args.message])
        .kill_on_drop(true)
        .output()
        .await
        .context("failed to run git commit")?;
    Ok(json!({
        "ok": out.status.success(),
        "output": String::from_utf8_lossy(&out.stdout).trim().to_string(),
        "error": if !out.status.success() { Some(String::from_utf8_lossy(&out.stderr).trim().to_string()) } else { None },
    }))
}

// ── file management ───────────────────────────────────────────────────────────

async fn delete_file(arguments: &str) -> Result<Value> {
    let args: PathArgs = parse_args(arguments)?;
    let path = resolve_writable_path(&args.path.context("path is required")?)?;
    if is_sensitive_path(&path) {
        bail!("refusing to delete sensitive path {}", path.display());
    }
    if !path.exists() {
        bail!("{} does not exist", path.display());
    }
    let was_dir = path.is_dir();
    if was_dir {
        fs::remove_dir_all(&path)
            .with_context(|| format!("failed to delete {}", path.display()))?;
    } else {
        fs::remove_file(&path).with_context(|| format!("failed to delete {}", path.display()))?;
    }
    Ok(json!({ "ok": true, "path": path.display().to_string(), "was_dir": was_dir }))
}

async fn move_file(arguments: &str) -> Result<Value> {
    #[derive(Deserialize)]
    struct Args {
        from: String,
        to: String,
    }
    let args: Args = parse_args(arguments)?;
    let from = resolve_writable_path(&args.from)?;
    let to = resolve_writable_path(&args.to)?;
    if is_sensitive_path(&from) || is_sensitive_path(&to) {
        bail!("refusing to move sensitive path");
    }
    if !from.exists() {
        bail!("{} does not exist", from.display());
    }
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::rename(&from, &to)
        .with_context(|| format!("failed to move {} → {}", from.display(), to.display()))?;
    Ok(json!({ "ok": true, "from": from.display().to_string(), "to": to.display().to_string() }))
}

async fn copy_file(arguments: &str) -> Result<Value> {
    #[derive(Deserialize)]
    struct Args {
        from: String,
        to: String,
    }
    let args: Args = parse_args(arguments)?;
    let from_str = args.from.trim();
    let from = resolve_path(from_str)?;
    let to = resolve_writable_path(&args.to)?;
    if is_sensitive_path(&from) || is_sensitive_path(&to) {
        bail!("refusing to copy sensitive path");
    }
    if !from.is_file() {
        bail!("{} is not a file", from.display());
    }
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)?;
    }
    let bytes = fs::copy(&from, &to)
        .with_context(|| format!("failed to copy {} → {}", from.display(), to.display()))?;
    Ok(
        json!({ "ok": true, "from": from.display().to_string(), "to": to.display().to_string(), "bytes": bytes }),
    )
}

async fn create_dir(arguments: &str) -> Result<Value> {
    let args: CreateDirArgs = parse_args(arguments)?;
    let path = resolve_writable_path(&args.path)?;
    if is_sensitive_path(&path) {
        bail!(
            "refusing to create sensitive-looking directory {}",
            path.display()
        );
    }
    if path.exists() && !path.is_dir() {
        bail!("{} exists and is not a directory", path.display());
    }

    let existed = path.exists();
    fs::create_dir_all(&path).with_context(|| format!("failed to create {}", path.display()))?;

    Ok(json!({
        "ok": true,
        "path": path.display().to_string(),
        "created": !existed,
    }))
}

async fn write_file(arguments: &str) -> Result<Value> {
    let args: WriteFileArgs = parse_args(arguments)?;
    let path = resolve_writable_path(&args.path)?;
    if is_sensitive_path(&path) {
        bail!(
            "refusing to write sensitive-looking file {}",
            path.display()
        );
    }
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    let existed = path.exists();
    fs::write(&path, &args.content)
        .with_context(|| format!("failed to write {}", path.display()))?;

    Ok(json!({
        "ok": true,
        "path": path.display().to_string(),
        "created": !existed,
        "bytes_written": args.content.len(),
    }))
}

async fn patch_file(arguments: &str) -> Result<Value> {
    #[derive(Deserialize)]
    struct Hunk {
        old_string: String,
        new_string: String,
    }
    #[derive(Deserialize)]
    struct Args {
        path: String,
        patches: Vec<Hunk>,
    }

    let args: Args = parse_args(arguments)?;
    let path = resolve_writable_path(&args.path)?;
    if !path.is_file() {
        bail!("{} is not a file", path.display());
    }
    if is_sensitive_path(&path) {
        bail!("refusing to edit sensitive file");
    }
    if args.patches.is_empty() {
        bail!("patches array is empty");
    }

    let mut content =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;

    for (i, hunk) in args.patches.iter().enumerate() {
        if hunk.old_string.is_empty() {
            bail!("patch[{i}]: old_string must not be empty");
        }
        if hunk.old_string == hunk.new_string {
            bail!("patch[{i}]: old_string and new_string are identical");
        }
        let count = content.matches(&hunk.old_string).count();
        match count {
            0 => bail!("patch[{i}]: old_string not found in {}", path.display()),
            1 => {}
            n => bail!("patch[{i}]: old_string appears {n} times — make it unique"),
        }
        content = content.replacen(&hunk.old_string, &hunk.new_string, 1);
    }

    fs::write(&path, &content).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(
        json!({ "ok": true, "path": path.display().to_string(), "patches_applied": args.patches.len() }),
    )
}

async fn edit_file(arguments: &str) -> Result<Value> {
    let args: EditFileArgs = parse_args(arguments)?;
    let path = resolve_writable_path(&args.path)?;
    if !path.is_file() {
        bail!("{} is not a file", path.display());
    }
    if is_sensitive_path(&path) {
        bail!("refusing to edit sensitive-looking file {}", path.display());
    }
    if !is_small_text_candidate(&path) {
        bail!("file is too large to edit safely");
    }
    if args.old_string.is_empty() {
        bail!("old_string must not be empty");
    }
    if args.old_string == args.new_string {
        bail!("old_string and new_string are identical");
    }

    let content =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let occurrences = content.matches(&args.old_string).count();
    match occurrences {
        0 => bail!("old_string was not found in {}", path.display()),
        1 => {}
        n => bail!(
            "old_string appears {n} times in {}; make it unique",
            path.display()
        ),
    }

    let updated = content.replacen(&args.old_string, &args.new_string, 1);
    fs::write(&path, &updated).with_context(|| format!("failed to write {}", path.display()))?;

    Ok(json!({
        "ok": true,
        "path": path.display().to_string(),
        "replacements": 1,
    }))
}

// ── persistent memory tools ───────────────────────────────────────────────────

fn notes_dir() -> Result<PathBuf> {
    let dir = crate::config::config_path()?
        .parent()
        .context("no config dir")?
        .join("notes");
    fs::create_dir_all(&dir).context("failed to create notes dir")?;
    Ok(dir)
}

fn sanitize_key(key: &str) -> String {
    key.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect::<String>()
        .to_lowercase()
}

async fn save_note(arguments: &str) -> Result<Value> {
    #[derive(Deserialize)]
    struct Args {
        key: String,
        content: String,
        #[serde(default)]
        append: bool,
    }
    let args: Args = parse_args(arguments)?;
    if args.key.trim().is_empty() {
        bail!("key is required");
    }
    let path = notes_dir()?.join(sanitize_key(&args.key) + ".md");
    let content = if args.append && path.exists() {
        format!(
            "{}\n\n{}",
            fs::read_to_string(&path).unwrap_or_default().trim_end(),
            args.content
        )
    } else {
        args.content
    };
    fs::write(&path, &content)?;
    Ok(json!({ "ok": true, "key": args.key, "bytes": content.len() }))
}

async fn read_notes(arguments: &str) -> Result<Value> {
    #[derive(Deserialize, Default)]
    struct Args {
        #[serde(default)]
        key: Option<String>,
    }
    let args: Args = serde_json::from_str(arguments).unwrap_or_default();
    let dir = notes_dir()?;
    if let Some(key) = &args.key {
        let path = dir.join(sanitize_key(key) + ".md");
        if !path.exists() {
            bail!("note '{}' not found", key);
        }
        let content = fs::read_to_string(&path)?;
        return Ok(json!({ "ok": true, "key": key, "content": content }));
    }
    let mut notes = Vec::new();
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let key = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            let preview = fs::read_to_string(&path)
                .ok()
                .and_then(|c| c.lines().next().map(|l| l.trim().to_string()))
                .unwrap_or_default();
            let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            notes.push(json!({ "key": key, "preview": preview, "size_bytes": size }));
        }
    }
    notes.sort_by(|a, b| a["key"].as_str().cmp(&b["key"].as_str()));
    Ok(json!({ "ok": true, "count": notes.len(), "notes": notes }))
}

async fn search_notes(arguments: &str) -> Result<Value> {
    #[derive(Deserialize)]
    struct Args {
        query: String,
    }
    let args: Args = parse_args(arguments)?;
    let query = args.query.to_lowercase();
    let dir = notes_dir()?;
    let mut results = Vec::new();
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let content = fs::read_to_string(&path).unwrap_or_default();
            let matching: Vec<&str> = content
                .lines()
                .filter(|l| l.to_lowercase().contains(&query))
                .take(3)
                .collect();
            if !matching.is_empty() {
                let key = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_string();
                results.push(json!({ "key": key, "matches": matching }));
            }
        }
    }
    Ok(json!({ "ok": true, "query": args.query, "results": results }))
}

async fn delete_note(arguments: &str) -> Result<Value> {
    #[derive(Deserialize)]
    struct Args {
        key: String,
    }
    let args: Args = parse_args(arguments)?;
    let path = notes_dir()?.join(sanitize_key(&args.key) + ".md");
    if !path.exists() {
        bail!("note '{}' not found", args.key);
    }
    fs::remove_file(&path)?;
    Ok(json!({ "ok": true, "key": args.key }))
}

// ── run_command (streaming variant for live output) ───────────────────────────

/// Streaming run_command: calls `on_line` with each output line as it arrives.
/// Returns the same JSON as `run_command` but streams progress via the callback.
pub async fn run_command_with_progress<F>(arguments: &str, mut on_line: F) -> String
where
    F: FnMut(String),
{
    match run_command_streaming_impl(arguments, &mut on_line).await {
        Ok(v) => v.to_string(),
        Err(e) => json!({ "ok": false, "error": e.to_string() }).to_string(),
    }
}

async fn run_command_streaming_impl<F>(arguments: &str, on_line: &mut F) -> Result<Value>
where
    F: FnMut(String),
{
    use tokio::io::AsyncBufReadExt;

    #[derive(Deserialize)]
    struct Args {
        command: String,
        #[serde(default)]
        timeout_secs: Option<u64>,
    }
    let args: Args = parse_args(arguments)?;
    if args.command.trim().is_empty() {
        bail!("command is empty");
    }

    let timeout_secs = args
        .timeout_secs
        .unwrap_or(DEFAULT_COMMAND_TIMEOUT_SECS)
        .min(MAX_COMMAND_TIMEOUT_SECS);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);

    // Merge stderr into stdout for unified live streaming
    let mut child = tokio::process::Command::new("sh")
        .args(["-c", &format!("({}) 2>&1", args.command)])
        .stdout(std::process::Stdio::piped())
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("failed to spawn command")?;

    let stdout = child.stdout.take().context("no stdout")?;
    let mut reader = tokio::io::BufReader::new(stdout).lines();
    let mut all_output = String::new();
    let mut line_count = 0usize;

    loop {
        tokio::select! {
            result = reader.next_line() => {
                match result? {
                    Some(line) => {
                        on_line(line.clone());
                        all_output.push_str(&line);
                        all_output.push('\n');
                        line_count += 1;
                        if all_output.len() > MAX_COMMAND_OUTPUT {
                            all_output.push_str("\n...[output truncated]");
                            break;
                        }
                    }
                    None => break,
                }
            }
            _ = tokio::time::sleep_until(deadline) => {
                child.kill().await.ok();
                bail!("command timed out after {timeout_secs}s ({line_count} lines output)");
            }
        }
    }

    let exit_code = child.wait().await?.code().unwrap_or(-1);
    Ok(json!({
        "ok": exit_code == 0,
        "exit_code": exit_code,
        "stdout": all_output,
        "stderr": "",
        "lines": line_count,
    }))
}

async fn run_command(arguments: &str) -> Result<Value> {
    let args: RunCommandArgs = parse_args(arguments)?;
    let command = args.command.trim();
    if command.is_empty() {
        bail!("command is empty");
    }
    let timeout = Duration::from_secs(
        args.timeout_secs
            .unwrap_or(DEFAULT_COMMAND_TIMEOUT_SECS)
            .clamp(1, MAX_COMMAND_TIMEOUT_SECS),
    );

    let child = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("failed to spawn command")?;

    let output = match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(result) => result.context("failed to run command")?,
        Err(_) => {
            bail!(
                "Command timed out after {}s. \
                Do NOT retry this command — report the timeout to the user and ask for guidance.",
                timeout.as_secs()
            );
        }
    };

    Ok(json!({
        "ok": output.status.success(),
        "exit_code": output.status.code(),
        "stdout": cap_output(&output.stdout),
        "stderr": cap_output(&output.stderr),
    }))
}

fn cap_output(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    if text.len() <= MAX_COMMAND_OUTPUT {
        return text.into_owned();
    }
    let mut clipped: String = text.chars().take(MAX_COMMAND_OUTPUT).collect();
    clipped.push_str("\n...[output truncated]");
    clipped
}

#[derive(Debug, Deserialize)]
struct PathArgs {
    path: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CreateDirArgs {
    path: String,
}

#[derive(Debug, Deserialize)]
struct WriteFileArgs {
    path: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct EditFileArgs {
    path: String,
    old_string: String,
    new_string: String,
}

#[derive(Debug, Deserialize)]
struct RunCommandArgs {
    command: String,
    timeout_secs: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct SearchArgs {
    root: Option<String>,
    query: String,
}

#[derive(Debug, Deserialize)]
struct ReadFileArgs {
    path: String,
    start_line: Option<usize>,
    max_lines: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct WebSearchArgs {
    query: String,
}

fn parse_args<T: for<'de> Deserialize<'de>>(arguments: &str) -> Result<T> {
    serde_json::from_str(arguments).with_context(|| format!("invalid tool arguments: {arguments}"))
}

fn resolve_path(path: &str) -> Result<PathBuf> {
    let path = Path::new(path);
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .context("failed to resolve current directory")?
            .join(path)
    };

    path.canonicalize()
        .with_context(|| format!("failed to resolve {}", path.display()))
}

/// Resolve a path that may not exist yet (for writes). Does not canonicalize the
/// final component, but anchors relative paths to the terminal cwd.
fn resolve_writable_path(path: &str) -> Result<PathBuf> {
    let path = Path::new(path);
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    Ok(std::env::current_dir()
        .context("failed to resolve current directory")?
        .join(path))
}

fn walk_paths<F>(root: &Path, mut visit: F) -> Result<()>
where
    F: FnMut(&Path) -> Result<bool>,
{
    let mut queue = VecDeque::from([(root.to_path_buf(), 0usize)]);
    let mut visited = 0usize;

    while let Some((path, depth)) = queue.pop_front() {
        if visited >= MAX_VISITED_PATHS {
            break;
        }
        visited += 1;

        if !visit(&path)? {
            break;
        }

        if depth >= MAX_DEPTH || !path.is_dir() {
            continue;
        }

        let Ok(entries) = fs::read_dir(&path) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if should_skip_name(&name) {
                continue;
            }
            queue.push_back((entry.path(), depth + 1));
        }
    }

    Ok(())
}

fn should_skip_name(name: &str) -> bool {
    matches!(
        name,
        ".git"
            | ".next"
            | ".turbo"
            | ".cache"
            | ".venv"
            | "node_modules"
            | "target"
            | "dist"
            | "build"
            | "vendor"
            | "Library"
    )
}

fn path_kind(path: &Path) -> &'static str {
    if path.is_dir() {
        "dir"
    } else if path.is_file() {
        "file"
    } else {
        "other"
    }
}

fn is_small_text_candidate(path: &Path) -> bool {
    fs::metadata(path)
        .map(|metadata| metadata.len() <= MAX_TEXT_BYTES)
        .unwrap_or(false)
}

fn is_sensitive_path(path: &Path) -> bool {
    let lower = path.display().to_string().to_lowercase();
    // Credential directories
    lower.contains("/.ssh/")
        || lower.contains("/.aws/")
        || lower.contains("/.gnupg/")
        || lower.contains("/.kube/")
        || lower.contains("/.docker/")
        // Environment and secret files
        || lower.ends_with("/.env")
        || lower.contains("/.env.")
        // SSH private key filenames
        || lower.ends_with("/id_rsa")
        || lower.ends_with("/id_dsa")
        || lower.ends_with("/id_ed25519")
        || lower.ends_with("/id_ecdsa")
        // Cloud/tool credential files
        || lower.ends_with("/credentials")
        || lower.ends_with("/.netrc")
        || lower.ends_with("/.npmrc")
        || lower.ends_with("/.pypirc")
        || lower.ends_with("/.git-credentials")
        // System auth files
        || lower.ends_with("/etc/shadow")
        || lower.ends_with("/etc/passwd")
        // Targeted secret patterns (narrower than a broad "secret" substring)
        || lower.contains("secret_key")
        || lower.contains("secretkey")
        || lower.contains("/secrets.")
        || lower.contains("/secrets/")
        || lower.contains("private_key")
}

fn normalized_query(query: &str) -> Result<String> {
    let query = query.trim();
    if query.is_empty() {
        bail!("query is empty");
    }
    Ok(query.to_lowercase())
}

fn truncate(value: &str, max_chars: usize) -> String {
    let mut output = value.chars().take(max_chars).collect::<String>();
    if value.chars().count() > max_chars {
        output.push_str("...");
    }
    output
}

fn collect_related_topics(value: Option<&Value>, results: &mut Vec<Value>) {
    let Some(Value::Array(topics)) = value else {
        return;
    };

    for topic in topics {
        if let Some(nested) = topic.get("Topics") {
            collect_related_topics(Some(nested), results);
            continue;
        }

        let text = topic
            .get("Text")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if text.is_empty() {
            continue;
        }
        results.push(json!({
            "title": text.split(" - ").next().unwrap_or("Result"),
            "snippet": text,
            "url": topic.get("FirstURL").and_then(Value::as_str).unwrap_or("")
        }));
    }
}

fn percent_encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

// ── screenshot_url ─────────────────────────────────────────────────────────────

async fn screenshot_url(arguments: &str) -> Result<Value> {
    #[derive(Deserialize)]
    struct Args {
        url: String,
        #[serde(default)]
        output: Option<String>,
        #[serde(default)]
        width: Option<u32>,
        #[serde(default)]
        height: Option<u32>,
        #[serde(default)]
        full_page: Option<bool>,
    }
    let args: Args = parse_args(arguments)?;
    let url = args.url.trim().to_string();
    if url.is_empty() {
        bail!("url is required");
    }

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let output = args
        .output
        .unwrap_or_else(|| format!("/tmp/anveesa-screenshot-{ts}.png"));
    let width = args.width.unwrap_or(1440);
    let height = args.height.unwrap_or(900);
    let viewport = format!("{width},{height}");

    let mut cmd = tokio::process::Command::new("npx");
    cmd.args(["playwright", "screenshot", "--viewport-size", &viewport]);
    if args.full_page.unwrap_or(false) {
        cmd.arg("--full-page");
    }
    cmd.arg(&url).arg(&output);

    let result = cmd.output().await;
    match result {
        Ok(out) if out.status.success() => {
            let size_kb = tokio::fs::metadata(&output)
                .await
                .map(|m| m.len() / 1024)
                .unwrap_or(0);
            Ok(json!({
                "ok": true,
                "saved_to": output,
                "url": url,
                "width": width,
                "height": height,
                "size_kb": size_kb,
                "note": format!("Screenshot saved to {output}. Open it with: open {output}")
            }))
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            bail!("playwright failed (exit {}): {}", out.status, stderr.trim())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            bail!(
                "playwright not found — install with: npm install -g playwright && npx playwright install chromium"
            )
        }
        Err(e) => bail!("failed to run playwright: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_tools_only_when_permitted() {
        let read_only = definitions(false);
        assert!(!read_only.iter().any(|tool| tool_name(tool) == "write_file"));

        let with_writes = definitions(true);
        let names: Vec<&str> = with_writes.iter().map(tool_name).collect();
        assert!(names.contains(&"create_dir"));
        assert!(names.contains(&"write_file"));
        assert!(names.contains(&"edit_file"));
        assert!(names.contains(&"run_command"));
    }

    fn tool_name(tool: &Value) -> &str {
        tool["function"]["name"].as_str().unwrap_or_default()
    }

    #[test]
    fn classifies_write_tools() {
        assert!(is_write_tool("create_dir"));
        assert!(is_write_tool("write_file"));
        assert!(is_write_tool("run_command"));
        assert!(!is_write_tool("read_file"));
        assert!(!is_write_tool("web_search"));
    }

    #[test]
    fn describes_calls_for_confirmation() {
        assert_eq!(describe_call("list_dir", r#"{}"#), "list directory .");
        assert_eq!(
            describe_call("find_files", r#"{"query":"Cargo","root":"src"}"#),
            "find files matching `Cargo` under src"
        );
        assert_eq!(
            describe_call("search_text", r#"{"query":"TODO"}"#),
            "search text `TODO` under ."
        );
        assert_eq!(
            describe_call("read_file", r#"{"path":"README.md"}"#),
            "read file README.md"
        );
        assert_eq!(
            describe_call("web_search", r#"{"query":"rust termios"}"#),
            "web search `rust termios`"
        );
        assert_eq!(
            describe_call("create_dir", r#"{"path":"hello"}"#),
            "create directory hello"
        );
        assert_eq!(
            describe_call("write_file", r#"{"path":"a.txt","content":"x"}"#),
            "write file a.txt"
        );
        assert_eq!(
            describe_call("run_command", r#"{"command":"cargo test"}"#),
            "run command `cargo test`"
        );
    }

    #[test]
    fn guidance_mentions_writes_only_when_enabled() {
        assert!(!guidance(false).contains("write_file"));
        assert!(guidance(false).contains("call the relevant tool immediately"));
        assert!(guidance(true).contains("create_dir"));
        assert!(guidance(true).contains("write_file"));
    }

    #[test]
    fn flags_sensitive_paths() {
        // Original cases
        assert!(is_sensitive_path(Path::new("/home/u/.ssh/id_rsa")));
        assert!(is_sensitive_path(Path::new("/proj/.env")));
        // New credential directories
        assert!(is_sensitive_path(Path::new("/home/u/.kube/config")));
        assert!(is_sensitive_path(Path::new("/home/u/.docker/config.json")));
        assert!(is_sensitive_path(Path::new("/home/u/.git-credentials")));
        assert!(is_sensitive_path(Path::new("/home/u/.netrc")));
        assert!(is_sensitive_path(Path::new("/home/u/.npmrc")));
        // Targeted secret patterns
        assert!(is_sensitive_path(Path::new("/proj/config/secrets.yaml")));
        assert!(is_sensitive_path(Path::new("/proj/secrets/db.json")));
        assert!(is_sensitive_path(Path::new("/proj/config/secret_key.txt")));
        // Non-sensitive paths — including the false-positive the old "secret" check caused
        assert!(!is_sensitive_path(Path::new("/proj/src/main.rs")));
        assert!(!is_sensitive_path(Path::new("/proj/src/secret_manager.rs")));
        assert!(!is_sensitive_path(Path::new(
            "/proj/docs/secret_rotation.md"
        )));
    }

    #[test]
    fn truncates_long_values() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello", 3), "hel...");
    }

    #[test]
    fn percent_encodes_reserved_characters() {
        assert_eq!(percent_encode("a b"), "a%20b");
        assert_eq!(percent_encode("rust-lang"), "rust-lang");
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("anveesa_test_{tag}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn create_dir_creates_nested_directory() {
        let dir = temp_dir("mkdir");
        let path = dir.join("hello").join("world");
        let result = create_dir(&json!({ "path": path.to_str().unwrap() }).to_string())
            .await
            .unwrap();
        assert_eq!(result["ok"], json!(true));
        assert_eq!(result["created"], json!(true));
        assert!(path.is_dir());

        let result = create_dir(&json!({ "path": path.to_str().unwrap() }).to_string())
            .await
            .unwrap();
        assert_eq!(result["created"], json!(false));

        fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn write_then_edit_file() {
        let dir = temp_dir("write");
        let path = dir.join("note.txt");
        let path_str = path.to_str().unwrap();

        let result = write_file(&json!({ "path": path_str, "content": "alpha beta" }).to_string())
            .await
            .unwrap();
        assert_eq!(result["ok"], json!(true));
        assert_eq!(result["created"], json!(true));
        assert_eq!(fs::read_to_string(&path).unwrap(), "alpha beta");

        edit_file(
            &json!({ "path": path_str, "old_string": "beta", "new_string": "gamma" }).to_string(),
        )
        .await
        .unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "alpha gamma");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn edit_file_requires_unique_match() {
        let dir = temp_dir("unique");
        let path = dir.join("dup.txt");
        fs::write(&path, "x and x").unwrap();
        let path_str = path.to_str().unwrap();

        let duplicate = edit_file(
            &json!({ "path": path_str, "old_string": "x", "new_string": "y" }).to_string(),
        )
        .await;
        assert!(duplicate.is_err());

        let missing = edit_file(
            &json!({ "path": path_str, "old_string": "zzz", "new_string": "y" }).to_string(),
        )
        .await;
        assert!(missing.is_err());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn write_file_refuses_sensitive_paths() {
        let dir = temp_dir("sensitive");
        let path = dir.join(".env");
        let result = write_file(
            &json!({ "path": path.to_str().unwrap(), "content": "SECRET=1" }).to_string(),
        )
        .await;
        assert!(result.is_err());
        assert!(!path.exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn run_command_captures_output() {
        let result = run_command(&json!({ "command": "printf hello" }).to_string())
            .await
            .unwrap();
        assert_eq!(result["ok"], json!(true));
        assert_eq!(result["exit_code"], json!(0));
        assert_eq!(result["stdout"], json!("hello"));
    }

    #[tokio::test]
    async fn run_command_reports_failure() {
        let result = run_command(&json!({ "command": "exit 3" }).to_string())
            .await
            .unwrap();
        assert_eq!(result["ok"], json!(false));
        assert_eq!(result["exit_code"], json!(3));
    }
}

#[cfg(test)]
#[path = "tools_scenarios.rs"]
mod scenarios;
