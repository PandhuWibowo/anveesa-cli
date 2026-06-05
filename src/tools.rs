use std::{
    collections::VecDeque,
    fs,
    path::{Path, PathBuf},
    process::Stdio,
    sync::OnceLock,
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use serde_json::{Value, json};

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
    text
}

/// Whether a tool modifies the system and must pass the approval policy.
pub fn is_write_tool(name: &str) -> bool {
    matches!(
        name,
        "create_dir" | "write_file" | "edit_file" | "run_command"
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
        "fetch_url"  => format!("fetch {}", field("url")),
        "git_status" => "git status".to_string(),
        "git_diff"   => {
            let path = field("path");
            if path.is_empty() { "git diff".to_string() } else { format!("git diff {path}") }
        }
        "git_log"    => "git log".to_string(),
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
                "description": "Fetch the content of a URL and return it as plain text. Strips HTML tags automatically.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "url": { "type": "string", "description": "URL to fetch." },
                        "max_chars": { "type": "integer", "description": "Max characters to return (default 40000)." }
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
                        "n":    { "type": "integer", "description": "Number of commits to show (default 20, max 100)." },
                        "path": { "type": "string",  "description": "Limit log to commits touching this path." }
                    }
                }
            }
        }),
    ];

    if include_write {
        definitions.extend([
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
                    "description": "Run a shell command in the terminal cwd and return its output. Use for builds, tests, git, and similar tasks.",
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
        ]);
    }

    definitions
}

pub async fn run(name: &str, arguments: &str) -> String {
    let result = match name {
        "list_dir"   => list_dir(arguments).await,
        "find_files" => find_files(arguments).await,
        "search_text" => search_text(arguments).await,
        "read_file"  => read_file(arguments).await,
        "web_search" => web_search(arguments).await,
        "fetch_url"  => fetch_url(arguments).await,
        "git_status" => git_status(arguments).await,
        "git_diff"   => git_diff(arguments).await,
        "git_log"    => git_log(arguments).await,
        "create_dir" => create_dir(arguments).await,
        "write_file" => write_file(arguments).await,
        "edit_file"  => edit_file(arguments).await,
        "run_command" => run_command(arguments).await,
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
    let content =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
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

    let url = format!(
        "https://api.duckduckgo.com/?q={}&format=json&no_html=1&skip_disambig=1",
        percent_encode(query)
    );
    let response: Value = http_client()
        .get(&url)
        .send()
        .await
        .context("web search request failed")?
        .json()
        .await
        .context("failed to parse web search response")?;

    let mut results = Vec::new();
    if let Some(abstract_text) = response.get("AbstractText").and_then(Value::as_str)
        && !abstract_text.is_empty()
    {
        results.push(json!({
            "title": response.get("Heading").and_then(Value::as_str).unwrap_or("DuckDuckGo"),
            "snippet": abstract_text,
            "url": response.get("AbstractURL").and_then(Value::as_str).unwrap_or("")
        }));
    }
    collect_related_topics(response.get("RelatedTopics"), &mut results);
    results.truncate(8);

    Ok(json!({
        "ok": true,
        "query": query,
        "results": results
    }))
}

// ── fetch_url ─────────────────────────────────────────────────────────────────

async fn fetch_url(arguments: &str) -> Result<Value> {
    #[derive(Deserialize)]
    struct Args {
        url: String,
        #[serde(default)]
        max_chars: Option<usize>,
    }
    let args: Args = parse_args(arguments)?;
    let url = args.url.trim();
    if url.is_empty() { bail!("url is required"); }
    let max_chars = args.max_chars.unwrap_or(40_000);

    let response = http_client()
        .get(url)
        .send()
        .await
        .with_context(|| format!("failed to fetch {url}"))?;

    let status = response.status();
    if !status.is_success() { bail!("HTTP {status}"); }

    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("text/plain")
        .to_string();

    let body = response.text().await.context("failed to read response body")?;
    let text = if content_type.contains("html") || content_type.contains("xml") {
        html_to_text(&body)
    } else {
        body
    };

    let char_count = text.chars().count();
    let truncated = char_count > max_chars;
    let text: String = text.chars().take(max_chars).collect();

    Ok(json!({
        "ok": true,
        "url": url,
        "content_type": content_type,
        "text": text,
        "truncated": truncated,
    }))
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
            let name = raw.trim_start_matches('/').split_whitespace().next().unwrap_or("");
            if matches!(name, "script" | "style") && !raw.starts_with('/') {
                skip_close = Some(name.to_string());
            }
            if matches!(name, "p"|"div"|"h1"|"h2"|"h3"|"h4"|"h5"|"h6"|"br"|"li"|"tr"|"section"|"article") {
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
        .replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">")
        .replace("&quot;", "\"").replace("&#39;", "'").replace("&nbsp;", " ")
        .replace("&#x27;", "'").replace("&#x2F;", "/");

    // Collapse blank lines
    let mut result = String::new();
    let mut blank = 0usize;
    for line in out.lines() {
        let t = line.trim();
        if t.is_empty() {
            blank += 1;
            if blank <= 1 { result.push('\n'); }
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
        #[serde(default)] staged: bool,
        #[serde(default)] path: Option<String>,
        #[serde(rename = "ref", default)] refspec: Option<String>,
    }
    let args: Args = serde_json::from_str(arguments).unwrap_or_default();
    let mut cmd = tokio::process::Command::new("git");
    cmd.arg("diff").kill_on_drop(true);
    if args.staged { cmd.arg("--staged"); }
    if let Some(r) = &args.refspec { cmd.arg(r); }
    if let Some(p) = &args.path { cmd.arg("--").arg(p); }
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
        #[serde(default = "default_n")] n: usize,
        #[serde(default)] path: Option<String>,
    }
    fn default_n() -> usize { 20 }
    let args: Args = serde_json::from_str(arguments).unwrap_or(Args { n: 20, path: None });
    let n = args.n.clamp(1, 100);
    let mut cmd = tokio::process::Command::new("git");
    cmd.args(["log", "--oneline", "--decorate", &format!("-{n}")]).kill_on_drop(true);
    if let Some(p) = &args.path { cmd.arg("--").arg(p); }
    let out = cmd.output().await.context("failed to run git log")?;
    Ok(json!({
        "ok": out.status.success(),
        "log": String::from_utf8_lossy(&out.stdout).trim().to_string(),
        "error": if !out.status.success() { Some(String::from_utf8_lossy(&out.stderr).trim().to_string()) } else { None },
    }))
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
        assert!(!is_sensitive_path(Path::new("/proj/docs/secret_rotation.md")));
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
