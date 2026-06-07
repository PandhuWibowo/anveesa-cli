//! MCP (Model Context Protocol) client — connects to external tool servers
//! over JSON-RPC / stdio, discovers their tools, and routes calls to them.

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Child,
    sync::Mutex,
};

use crate::config::McpServerConfig;

// ── Tool definition exposed to the rest of anveesa ───────────────────────────

#[derive(Debug, Clone)]
pub struct McpTool {
    /// Namespaced as mcp__{server}__{original_name}
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub server: String,
    pub original_name: String,
}

impl McpTool {
    /// Convert to an OpenAI-compatible function definition.
    pub fn to_definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": self.name,
                "description": format!("[MCP:{}] {}", self.server, self.description),
                "parameters": self.input_schema,
            }
        })
    }
}

// ── Single MCP server connection ──────────────────────────────────────────────

struct McpServer {
    name: String,
    stdin: Mutex<tokio::process::ChildStdin>,
    stdout: Mutex<BufReader<tokio::process::ChildStdout>>,
    next_id: Mutex<u64>,
    _child: Child,
}

impl McpServer {
    async fn connect(name: &str, cfg: &McpServerConfig) -> Result<(Self, Vec<McpTool>)> {
        let mut child = tokio::process::Command::new(&cfg.command)
            .args(&cfg.args)
            .envs(&cfg.env)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("failed to start MCP server '{name}' ({})", cfg.command))?;

        let stdin = child.stdin.take().context("MCP server has no stdin")?;
        let stdout = BufReader::new(child.stdout.take().context("MCP server has no stdout")?);

        let server = Self {
            name: name.to_string(),
            stdin: Mutex::new(stdin),
            stdout: Mutex::new(stdout),
            next_id: Mutex::new(1),
            _child: child,
        };

        server.initialize().await?;
        let tools = server.list_tools().await?;
        Ok((server, tools))
    }

    async fn send_msg(&self, msg: Value) -> Result<()> {
        let line = serde_json::to_string(&msg)? + "\n";
        let mut stdin = self.stdin.lock().await;
        stdin.write_all(line.as_bytes()).await?;
        stdin.flush().await?;
        Ok(())
    }

    async fn recv_msg(&self) -> Result<Value> {
        let mut stdout = self.stdout.lock().await;
        let mut line = String::new();
        stdout
            .read_line(&mut line)
            .await
            .context("MCP server closed")?;
        if line.is_empty() {
            bail!("MCP server closed connection");
        }
        Ok(serde_json::from_str(line.trim())?)
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value> {
        let id = {
            let mut n = self.next_id.lock().await;
            let v = *n;
            *n += 1;
            v
        };
        self.send_msg(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
            .await?;

        // Wait for our response with a timeout
        let timeout = tokio::time::Duration::from_secs(30);
        let result = tokio::time::timeout(timeout, async {
            loop {
                let resp = self.recv_msg().await?;
                if resp.get("id").and_then(|v| v.as_u64()) == Some(id) {
                    if let Some(err) = resp.get("error") {
                        anyhow::bail!("MCP error from '{}': {}", self.name, err);
                    }
                    return Ok(resp["result"].clone());
                }
                // Drop unmatched messages (notifications, other ids)
            }
        })
        .await
        .context(format!(
            "MCP request to '{}' timed out after 30s",
            self.name
        ))??;
        Ok(result)
    }

    async fn notify(&self, method: &str, params: Value) -> Result<()> {
        self.send_msg(json!({ "jsonrpc": "2.0", "method": method, "params": params }))
            .await
    }

    async fn initialize(&self) -> Result<()> {
        self.request(
            "initialize",
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "anveesa", "version": env!("CARGO_PKG_VERSION") }
            }),
        )
        .await?;
        self.notify("notifications/initialized", json!({})).await?;
        Ok(())
    }

    async fn list_tools(&self) -> Result<Vec<McpTool>> {
        let result = self.request("tools/list", json!({})).await?;
        let raw = result["tools"].as_array().cloned().unwrap_or_default();
        Ok(raw
            .into_iter()
            .filter_map(|t| {
                let original_name = t["name"].as_str()?.to_string();
                let description = t["description"].as_str().unwrap_or("").to_string();
                let input_schema = t
                    .get("inputSchema")
                    .cloned()
                    .unwrap_or(json!({"type":"object","properties":{}}));
                let safe_server = self.name.replace('-', "_").replace('.', "_");
                Some(McpTool {
                    name: format!("mcp__{safe_server}__{original_name}"),
                    description,
                    input_schema,
                    server: self.name.clone(),
                    original_name,
                })
            })
            .collect())
    }

    async fn call_tool(&self, original_name: &str, arguments: Value) -> Result<String> {
        let result = self
            .request(
                "tools/call",
                json!({
                    "name": original_name,
                    "arguments": arguments,
                }),
            )
            .await?;

        // MCP returns content as an array of typed blocks
        let content = result["content"].as_array().cloned().unwrap_or_default();
        let text = content
            .iter()
            .filter_map(|c| match c["type"].as_str() {
                Some("text") => c["text"].as_str().map(str::to_string),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

        let is_error = result["isError"].as_bool().unwrap_or(false);
        Ok(json!({ "ok": !is_error, "result": text }).to_string())
    }
}

// ── McpManager — holds all connected servers ──────────────────────────────────

pub struct McpManager {
    servers: Vec<(McpServer, Vec<McpTool>)>,
}

impl std::fmt::Debug for McpManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<&str> = self.servers.iter().map(|(s, _)| s.name.as_str()).collect();
        write!(f, "McpManager {{ servers: {:?} }}", names)
    }
}

impl McpManager {
    /// Connect to all configured MCP servers. Errors for individual servers are
    /// logged and skipped so one broken server doesn't block startup.
    pub async fn connect(configs: &BTreeMap<String, McpServerConfig>) -> Self {
        let mut servers = Vec::new();
        for (name, cfg) in configs {
            match McpServer::connect(name, cfg).await {
                Ok(pair) => {
                    eprintln!(
                        "\x1b[2m  MCP: connected to '{name}' ({} tools)\x1b[0m",
                        pair.1.len()
                    );
                    servers.push(pair);
                }
                Err(e) => {
                    eprintln!("\x1b[33m  MCP: failed to connect to '{name}': {e:#}\x1b[0m");
                }
            }
        }
        Self { servers }
    }

    /// All tool definitions from all connected servers.
    pub fn tool_definitions(&self) -> Vec<Value> {
        self.servers
            .iter()
            .flat_map(|(_, tools)| tools.iter().map(|t| t.to_definition()))
            .collect()
    }

    /// All tool names from all connected servers.
    pub fn tool_names(&self) -> Vec<String> {
        self.servers
            .iter()
            .flat_map(|(_, tools)| tools.iter().map(|t| t.name.clone()))
            .collect()
    }

    /// Dispatch a call to the appropriate MCP server.
    pub async fn call(&self, tool_name: &str, arguments: &str) -> Option<String> {
        let args: Value = serde_json::from_str(arguments).unwrap_or(json!({}));
        for (server, tools) in &self.servers {
            if let Some(tool) = tools.iter().find(|t| t.name == tool_name) {
                return Some(match server.call_tool(&tool.original_name, args).await {
                    Ok(r) => r,
                    Err(e) => json!({ "ok": false, "error": e.to_string() }).to_string(),
                });
            }
        }
        None
    }

    pub fn is_empty(&self) -> bool {
        self.servers.is_empty()
    }
}
