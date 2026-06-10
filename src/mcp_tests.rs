//! Tests for src/mcp.rs — McpTool types and serialization.
//! McpManager / McpServer require running processes — tested via config integration.

use crate::mcp::McpTool;
use serde_json::json;

// ── McpTool construction ─────────────────────────────────────────────

#[test]
fn mcp_tool_construct() {
    let tool = McpTool {
        name: "mcp__server__greet".into(),
        description: "Say hello".into(),
        input_schema: json!({"type": "object", "properties": {"name": {"type": "string"}}}),
        server: "server".into(),
        original_name: "greet".into(),
    };
    assert_eq!(tool.name, "mcp__server__greet");
    assert_eq!(tool.original_name, "greet");
    assert_eq!(tool.server, "server");
}

#[test]
fn mcp_tool_server_with_dots() {
    let tool = McpTool {
        name: "mcp__my.server__foo".into(),
        description: "".into(),
        input_schema: json!({"type": "object"}),
        server: "my.server".into(),
        original_name: "foo".into(),
    };
    // Name stored as-is
    assert_eq!(tool.name, "mcp__my.server__foo");
    assert_eq!(tool.server, "my.server");
}

#[test]
fn mcp_tool_server_with_dashes() {
    let tool = McpTool {
        name: "mcp__my-server__bar".into(),
        description: "".into(),
        input_schema: json!({"type": "object"}),
        server: "my-server".into(),
        original_name: "bar".into(),
    };
    // Name stored as-is
    assert_eq!(tool.name, "mcp__my-server__bar");
    assert_eq!(tool.original_name, "bar");
}

#[test]
fn mcp_tool_empty_description() {
    let tool = McpTool {
        name: "mcp__s__x".into(),
        description: "".into(),
        input_schema: json!({"type": "object", "properties": {}}),
        server: "s".into(),
        original_name: "x".into(),
    };
    assert_eq!(tool.description, "");
}

#[test]
fn mcp_tool_clone() {
    let tool = McpTool {
        name: "mcp__s__g".into(),
        description: "greet".into(),
        input_schema: json!({"type": "object"}),
        server: "s".into(),
        original_name: "g".into(),
    };
    let t2 = tool.clone();
    assert_eq!(tool.name, t2.name);
    assert_eq!(tool.description, t2.description);
}

#[test]
fn mcp_tool_debug() {
    let tool = McpTool {
        name: "mcp__s__g".into(),
        description: "greet".into(),
        input_schema: json!({"type": "object"}),
        server: "s".into(),
        original_name: "g".into(),
    };
    let s = format!("{tool:?}");
    assert!(s.contains("McpTool"));
}

// ── to_definition() ──────────────────────────────────────────────────

#[test]
fn mcp_to_definition_basic() {
    let tool = McpTool {
        name: "mcp__s__greet".into(),
        description: "Say hello".into(),
        input_schema: json!({"type": "object", "properties": {"name": {"type": "string"}}}),
        server: "s".into(),
        original_name: "greet".into(),
    };
    let def = tool.to_definition();
    assert_eq!(def["type"], "function");
    assert_eq!(def["function"]["name"], "mcp__s__greet");
    assert!(
        def["function"]["description"]
            .as_str()
            .unwrap()
            .contains("[MCP:s]")
    );
    assert_eq!(def["function"]["parameters"]["type"], "object");
}

#[test]
fn mcp_to_definition_long_description() {
    let tool = McpTool {
        name: "mcp__s__do".into(),
        description: "A very long description that goes on and on".into(),
        input_schema: json!({"type": "object"}),
        server: "s".into(),
        original_name: "do".into(),
    };
    let def = tool.to_definition();
    let desc = def["function"]["description"].as_str().unwrap();
    assert!(desc.starts_with("[MCP:s] A very long"));
}

#[test]
fn mcp_to_definition_empty_schema() {
    let tool = McpTool {
        name: "mcp__s__noargs".into(),
        description: "No args".into(),
        input_schema: json!({"type": "object", "properties": {}}),
        server: "s".into(),
        original_name: "noargs".into(),
    };
    let def = tool.to_definition();
    assert_eq!(def["function"]["parameters"]["properties"], json!({}));
}

#[test]
fn mcp_to_definition_complex_schema() {
    let tool = McpTool {
        name: "mcp__s__complex".into(),
        description: "Complex params".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "a": {"type": "integer", "minimum": 0},
                "b": {"type": "string", "enum": ["x", "y"]},
                "c": {"type": "boolean"}
            },
            "required": ["a"]
        }),
        server: "s".into(),
        original_name: "complex".into(),
    };
    let def = tool.to_definition();
    assert_eq!(def["function"]["parameters"]["required"], json!(["a"]));
    assert_eq!(
        def["function"]["parameters"]["properties"]["a"]["minimum"],
        0
    );
}

#[test]
fn mcp_to_definition_unicode_in_name() {
    let tool = McpTool {
        name: "mcp__\u{4f1a}\u{5927}__test".into(),
        description: "Unicode server".into(),
        input_schema: json!({"type": "object"}),
        server: "\u{4f1a}\u{5927}".into(),
        original_name: "test".into(),
    };
    let def = tool.to_definition();
    assert_eq!(def["function"]["name"], "mcp__\u{4f1a}\u{5927}__test");
}

#[test]
fn mcp_to_definition_server_name_in_description() {
    let tool = McpTool {
        name: "mcp__file_system__read".into(),
        description: "Read a file".into(),
        input_schema: json!({"type": "object"}),
        server: "file_system".into(),
        original_name: "read".into(),
    };
    let def = tool.to_definition();
    let desc = def["function"]["description"].as_str().unwrap();
    assert!(desc.contains("[MCP:file_system]"));
}

// ── McpTool edge cases ───────────────────────────────────────────────

#[test]
fn mcp_tool_very_long_name() {
    let long = "a".repeat(500);
    let tool = McpTool {
        name: format!("mcp__s__{long}"),
        description: "Long name".into(),
        input_schema: json!({"type": "object"}),
        server: "s".into(),
        original_name: long.clone(),
    };
    assert_eq!(tool.name.len(), 8 + 500);
}

#[test]
fn mcp_tool_nested_schema() {
    let tool = McpTool {
        name: "mcp__s__nested".into(),
        description: "Nested params".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "outer": {
                    "type": "object",
                    "properties": {
                        "inner": {"type": "string"}
                    }
                }
            }
        }),
        server: "s".into(),
        original_name: "nested".into(),
    };
    let def = tool.to_definition();
    assert_eq!(
        def["function"]["parameters"]["properties"]["outer"]["properties"]["inner"]["type"],
        "string"
    );
}

#[test]
fn mcp_tool_special_chars_in_description() {
    let tool = McpTool {
        name: "mcp__s__special".into(),
        description: "Has \"quotes\" and \n newline".into(),
        input_schema: json!({"type": "object"}),
        server: "s".into(),
        original_name: "special".into(),
    };
    let def = tool.to_definition();
    let desc = def["function"]["description"].as_str().unwrap();
    assert!(desc.contains("quotes"));
}

#[test]
fn mcp_tool_empty_server_name() {
    let tool = McpTool {
        name: "mcp____empty".into(),
        description: "Empty server".into(),
        input_schema: json!({"type": "object"}),
        server: "".into(),
        original_name: "empty".into(),
    };
    let def = tool.to_definition();
    assert_eq!(def["function"]["name"], "mcp____empty");
}

#[test]
fn mcp_tool_special_chars_in_server() {
    let tool = McpTool {
        name: "mcp__my-server.v2__tool".into(),
        description: "Dotted server".into(),
        input_schema: json!({"type": "object"}),
        server: "my-server.v2".into(),
        original_name: "tool".into(),
    };
    // Name stored as-is
    assert_eq!(tool.name, "mcp__my-server.v2__tool");
    assert_eq!(tool.server, "my-server.v2");
}

#[test]
fn mcp_tool_schema_with_defaults() {
    let tool = McpTool {
        name: "mcp__s__defaults".into(),
        description: "Has defaults".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "count": {"type": "integer", "default": 10},
                "verbose": {"type": "boolean", "default": false}
            }
        }),
        server: "s".into(),
        original_name: "defaults".into(),
    };
    let def = tool.to_definition();
    assert_eq!(
        def["function"]["parameters"]["properties"]["count"]["default"],
        10
    );
    assert_eq!(
        def["function"]["parameters"]["properties"]["verbose"]["default"],
        false
    );
}
