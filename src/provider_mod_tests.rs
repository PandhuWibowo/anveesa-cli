//! Tests for provider/mod.rs types

use crate::provider::*;

// ============ ChatMessage tests ============

#[test]
fn chat_message_user() {
    let m = ChatMessage::user("hello".to_string());
    assert_eq!(m.role, ChatRole::User);
    assert_eq!(m.content, "hello");
}

#[test]
fn chat_message_assistant() {
    let m = ChatMessage::assistant("hi there".to_string());
    assert_eq!(m.role, ChatRole::Assistant);
    assert_eq!(m.content, "hi there");
}

#[test]
fn chat_message_empty() {
    let m = ChatMessage::user(String::new());
    assert!(m.content.is_empty());
}

#[test]
fn chat_message_unicode() {
    let m = ChatMessage::user("こんにちは 🌍".to_string());
    assert_eq!(m.content, "こんにちは 🌍");
}

#[test]
fn chat_message_multiline() {
    let content = "line1\nline2\nline3".to_string();
    let m = ChatMessage::user(content.clone());
    assert_eq!(m.content, content);
}

#[test]
fn chat_message_clone() {
    let m = ChatMessage::user("test".to_string());
    let m2 = m.clone();
    assert_eq!(m, m2);
}

#[test]
fn chat_message_debug() {
    let m = ChatMessage::user("test".to_string());
    let debug = format!("{:?}", m);
    assert!(debug.contains("ChatMessage"));
}

#[test]
fn chat_message_serialize_user() {
    let m = ChatMessage::user("hello".to_string());
    let s = serde_json::to_string(&m).unwrap();
    assert!(s.contains("User"));
    assert!(s.contains("hello"));
}

#[test]
fn chat_message_serialize_assistant() {
    let m = ChatMessage::assistant("hi".to_string());
    let s = serde_json::to_string(&m).unwrap();
    assert!(s.contains("Assistant"));
}

#[test]
fn chat_message_deserialize_user() {
    let m: ChatMessage = serde_json::from_str(r#"{"role":"User","content":"hello"}"#).unwrap();
    assert_eq!(m.role, ChatRole::User);
    assert_eq!(m.content, "hello");
}

#[test]
fn chat_message_deserialize_assistant() {
    let m: ChatMessage = serde_json::from_str(r#"{"role":"Assistant","content":"hi"}"#).unwrap();
    assert_eq!(m.role, ChatRole::Assistant);
}

#[test]
fn chat_message_roundtrip() {
    let original = ChatMessage::user("roundtrip test".to_string());
    let s = serde_json::to_string(&original).unwrap();
    let restored: ChatMessage = serde_json::from_str(&s).unwrap();
    assert_eq!(original, restored);
}

#[test]
fn chat_message_partial_eq_same() {
    let a = ChatMessage::user("test".to_string());
    let b = ChatMessage::user("test".to_string());
    assert_eq!(a, b);
}

#[test]
fn chat_message_partial_eq_different_role() {
    let a = ChatMessage::user("test".to_string());
    let b = ChatMessage::assistant("test".to_string());
    assert_ne!(a, b);
}

#[test]
fn chat_message_partial_eq_different_content() {
    let a = ChatMessage::user("test1".to_string());
    let b = ChatMessage::user("test2".to_string());
    assert_ne!(a, b);
}

#[test]
fn chat_message_long_content() {
    let long = "a".repeat(100_000);
    let m = ChatMessage::user(long.clone());
    assert_eq!(m.content.len(), 100_000);
}

#[test]
fn chat_message_special_chars() {
    let m = ChatMessage::user(r#"hello "world" \n tab"#.to_string());
    assert!(m.content.contains('"'));
}

#[test]
fn chat_message_null_byte() {
    let m = ChatMessage::user("\0".to_string());
    assert_eq!(m.content, "\0");
}

// ============ ChatRole tests ============

#[test]
fn chat_role_user_eq() {
    assert_eq!(ChatRole::User, ChatRole::User);
}

#[test]
fn chat_role_assistant_eq() {
    assert_eq!(ChatRole::Assistant, ChatRole::Assistant);
}

#[test]
fn chat_role_user_ne_assistant() {
    assert_ne!(ChatRole::User, ChatRole::Assistant);
}

#[test]
fn chat_role_clone() {
    let role = ChatRole::User;
    assert_eq!(role.clone(), ChatRole::User);
}

#[test]
fn chat_role_serialize() {
    let s = serde_json::to_string(&ChatRole::User).unwrap();
    assert_eq!(s, "\"User\"");
}

#[test]
fn chat_role_deserialize() {
    let r: ChatRole = serde_json::from_str("\"User\"").unwrap();
    assert_eq!(r, ChatRole::User);
}

#[test]
fn chat_role_debug() {
    let d = format!("{:?}", ChatRole::User);
    assert!(d.contains("User"));
}

#[test]
fn chat_role_all_variants() {
    let roles = [ChatRole::User, ChatRole::Assistant];
    assert_eq!(roles.len(), 2);
}

// ============ Usage tests ============

#[test]
fn usage_default() {
    let u = Usage::default();
    assert_eq!(u.prompt_tokens, 0);
    assert_eq!(u.completion_tokens, 0);
    assert_eq!(u.total_tokens, 0);
    assert_eq!(u.cache_read_tokens, 0);
    assert_eq!(u.cache_write_tokens, 0);
}

#[test]
fn usage_custom() {
    let u = Usage {
        prompt_tokens: 100,
        completion_tokens: 50,
        total_tokens: 150,
        cache_read_tokens: 10,
        cache_write_tokens: 5,
    };
    assert_eq!(u.prompt_tokens, 100);
    assert_eq!(u.total_tokens, 150);
}

#[test]
fn usage_clone() {
    let u = Usage {
        prompt_tokens: 42,
        ..Default::default()
    };
    let u2 = u;
    assert_eq!(u2.prompt_tokens, 42);
}

#[test]
fn usage_debug() {
    let u = Usage::default();
    let d = format!("{:?}", u);
    assert!(d.contains("Usage"));
}

#[test]
fn usage_max_values() {
    let u = Usage {
        prompt_tokens: u64::MAX,
        completion_tokens: u64::MAX,
        total_tokens: u64::MAX,
        cache_read_tokens: u64::MAX,
        cache_write_tokens: u64::MAX,
    };
    assert_eq!(u.prompt_tokens, u64::MAX);
}

#[test]
fn usage_only_prompt() {
    let u = Usage {
        prompt_tokens: 100,
        ..Default::default()
    };
    assert_eq!(u.completion_tokens, 0);
}

#[test]
fn usage_only_completion() {
    let u = Usage {
        completion_tokens: 50,
        ..Default::default()
    };
    assert_eq!(u.prompt_tokens, 0);
}

#[test]
fn usage_cache_read_only() {
    let u = Usage {
        cache_read_tokens: 10,
        ..Default::default()
    };
    assert_eq!(u.cache_read_tokens, 10);
}

#[test]
fn usage_cache_write_only() {
    let u = Usage {
        cache_write_tokens: 5,
        ..Default::default()
    };
    assert_eq!(u.cache_write_tokens, 5);
}

#[test]
fn usage_copy() {
    let u = Usage {
        prompt_tokens: 1,
        ..Default::default()
    };
    let u2 = u;
    assert_eq!(u2.prompt_tokens, 1);
}

// ============ ApprovalPolicy tests ============

#[test]
fn approval_policy_deny_no_write_tools() {
    assert!(!ApprovalPolicy::Deny.allows_write_tools());
}

#[test]
fn approval_policy_prompt_allows_write_tools() {
    assert!(ApprovalPolicy::Prompt.allows_write_tools());
}

#[test]
fn approval_policy_allow_allows_write_tools() {
    assert!(ApprovalPolicy::Allow.allows_write_tools());
}

#[test]
fn approval_policy_deny_eq() {
    assert_eq!(ApprovalPolicy::Deny, ApprovalPolicy::Deny);
}

#[test]
fn approval_policy_prompt_eq() {
    assert_eq!(ApprovalPolicy::Prompt, ApprovalPolicy::Prompt);
}

#[test]
fn approval_policy_allow_eq() {
    assert_eq!(ApprovalPolicy::Allow, ApprovalPolicy::Allow);
}

#[test]
fn approval_policy_ne() {
    assert_ne!(ApprovalPolicy::Deny, ApprovalPolicy::Prompt);
    assert_ne!(ApprovalPolicy::Deny, ApprovalPolicy::Allow);
    assert_ne!(ApprovalPolicy::Prompt, ApprovalPolicy::Allow);
}

#[test]
fn approval_policy_clone() {
    assert_eq!(ApprovalPolicy::Deny.clone(), ApprovalPolicy::Deny);
    assert_eq!(ApprovalPolicy::Prompt.clone(), ApprovalPolicy::Prompt);
    assert_eq!(ApprovalPolicy::Allow.clone(), ApprovalPolicy::Allow);
}

#[test]
fn approval_policy_debug() {
    assert!(format!("{:?}", ApprovalPolicy::Deny).contains("Deny"));
}

#[test]
fn approval_policy_copy() {
    let p = ApprovalPolicy::Allow;
    let p2 = p;
    let p3 = p;
    assert_eq!(p2, ApprovalPolicy::Allow);
    assert_eq!(p3, ApprovalPolicy::Allow);
}

#[test]
fn approval_policy_all_variants() {
    let policies = [
        ApprovalPolicy::Deny,
        ApprovalPolicy::Prompt,
        ApprovalPolicy::Allow,
    ];
    assert_eq!(policies.len(), 3);
}

// ============ ApprovalDecision tests ============

#[test]
fn approval_decision_deny_eq() {
    assert_eq!(ApprovalDecision::Deny, ApprovalDecision::Deny);
}

#[test]
fn approval_decision_allow_once_eq() {
    assert_eq!(ApprovalDecision::AllowOnce, ApprovalDecision::AllowOnce);
}

#[test]
fn approval_decision_allow_for_turn_eq() {
    assert_eq!(
        ApprovalDecision::AllowForTurn,
        ApprovalDecision::AllowForTurn
    );
}

#[test]
fn approval_decision_ne() {
    assert_ne!(ApprovalDecision::Deny, ApprovalDecision::AllowOnce);
    assert_ne!(ApprovalDecision::Deny, ApprovalDecision::AllowForTurn);
    assert_ne!(ApprovalDecision::AllowOnce, ApprovalDecision::AllowForTurn);
}

#[test]
fn approval_decision_clone() {
    assert_eq!(
        ApprovalDecision::AllowOnce.clone(),
        ApprovalDecision::AllowOnce
    );
}

#[test]
fn approval_decision_copy() {
    let d = ApprovalDecision::AllowForTurn;
    let d2 = d;
    assert_eq!(d2, ApprovalDecision::AllowForTurn);
}

#[test]
fn approval_decision_debug() {
    assert!(format!("{:?}", ApprovalDecision::Deny).contains("Deny"));
}

#[test]
fn approval_decision_all_variants() {
    let decisions = [
        ApprovalDecision::Deny,
        ApprovalDecision::AllowOnce,
        ApprovalDecision::AllowForTurn,
    ];
    assert_eq!(decisions.len(), 3);
}

// ============ DiffKind tests ============

#[test]
fn diff_kind_add_eq() {
    assert_eq!(DiffKind::Add, DiffKind::Add);
}

#[test]
fn diff_kind_remove_eq() {
    assert_eq!(DiffKind::Remove, DiffKind::Remove);
}

#[test]
fn diff_kind_ne() {
    assert_ne!(DiffKind::Add, DiffKind::Remove);
}

#[test]
fn diff_kind_debug_add() {
    assert!(format!("{:?}", DiffKind::Add).contains("Add"));
}

#[test]
fn diff_kind_debug_remove() {
    assert!(format!("{:?}", DiffKind::Remove).contains("Remove"));
}

// ============ DiffLine tests ============

#[test]
fn diff_line_add() {
    let d = DiffLine {
        kind: DiffKind::Add,
        line_no: 1,
        text: "new line".to_string(),
    };
    assert_eq!(d.line_no, 1);
    assert!(format!("{:?}", d).contains("Add"));
}

#[test]
fn diff_line_remove() {
    let d = DiffLine {
        kind: DiffKind::Remove,
        line_no: 5,
        text: "old line".to_string(),
    };
    assert_eq!(d.line_no, 5);
}

#[test]
fn diff_line_empty_text() {
    let d = DiffLine {
        kind: DiffKind::Add,
        line_no: 0,
        text: String::new(),
    };
    assert!(d.text.is_empty());
}

#[test]
fn diff_line_large_line_no() {
    let d = DiffLine {
        kind: DiffKind::Add,
        line_no: 999999,
        text: "end of file".to_string(),
    };
    assert_eq!(d.line_no, 999999);
}

#[test]
fn diff_line_unicode() {
    let d = DiffLine {
        kind: DiffKind::Add,
        line_no: 1,
        text: "日本語コード".to_string(),
    };
    assert_eq!(d.text, "日本語コード");
}

#[test]
fn diff_line_multiline_text() {
    let d = DiffLine {
        kind: DiffKind::Add,
        line_no: 1,
        text: "line1\nline2".to_string(),
    };
    assert!(d.text.contains('\n'));
}

#[test]
fn diff_line_debug() {
    let d = DiffLine {
        kind: DiffKind::Add,
        line_no: 42,
        text: "test".to_string(),
    };
    let debug = format!("{:?}", d);
    assert!(debug.contains("DiffLine"));
}

// ============ ToolConfirmPreview tests ============

#[test]
fn tool_confirm_preview_file_op() {
    let p = ToolConfirmPreview::FileOp {
        verb: "write".to_string(),
        path: "/tmp/test.rs".to_string(),
        added: 10,
        removed: 5,
        diff: vec![],
        truncated: false,
    };
    match p {
        ToolConfirmPreview::FileOp { added, removed, .. } => {
            assert_eq!(added, 10);
            assert_eq!(removed, 5);
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn tool_confirm_preview_file_op_truncated() {
    let p = ToolConfirmPreview::FileOp {
        verb: "write".to_string(),
        path: "large_file.rs".to_string(),
        added: 1000,
        removed: 0,
        diff: vec![],
        truncated: true,
    };
    match p {
        ToolConfirmPreview::FileOp { truncated, .. } => assert!(truncated),
        _ => panic!("wrong variant"),
    }
}

#[test]
fn tool_confirm_preview_create_dir() {
    let p = ToolConfirmPreview::CreateDir {
        path: "/tmp/newdir".to_string(),
    };
    match p {
        ToolConfirmPreview::CreateDir { path } => assert_eq!(path, "/tmp/newdir"),
        _ => panic!("wrong variant"),
    }
}

#[test]
fn tool_confirm_preview_generic() {
    let p = ToolConfirmPreview::Generic {
        summary: "run cargo test".to_string(),
    };
    match p {
        ToolConfirmPreview::Generic { summary } => assert_eq!(summary, "run cargo test"),
        _ => panic!("wrong variant"),
    }
}

#[test]
fn tool_confirm_preview_empty_path() {
    let p = ToolConfirmPreview::CreateDir {
        path: String::new(),
    };
    match p {
        ToolConfirmPreview::CreateDir { path } => assert!(path.is_empty()),
        _ => panic!("wrong variant"),
    }
}

// ============ StreamEvent tests ============

#[test]
fn stream_event_status() {
    let e = StreamEvent::Status {
        message: "thinking...".to_string(),
    };
    match e {
        StreamEvent::Status { message } => assert_eq!(message, "thinking..."),
        _ => panic!("wrong variant"),
    }
}

#[test]
fn stream_event_token() {
    let e = StreamEvent::Token("hello".to_string());
    match e {
        StreamEvent::Token(t) => assert_eq!(t, "hello"),
        _ => panic!("wrong variant"),
    }
}

#[test]
fn stream_event_token_empty() {
    let e = StreamEvent::Token(String::new());
    match e {
        StreamEvent::Token(t) => assert!(t.is_empty()),
        _ => panic!("wrong variant"),
    }
}

#[test]
fn stream_event_thinking() {
    let e = StreamEvent::Thinking("Let me think...".to_string());
    match e {
        StreamEvent::Thinking(t) => assert_eq!(t, "Let me think..."),
        _ => panic!("wrong variant"),
    }
}

#[test]
fn stream_event_usage() {
    let usage = Usage {
        prompt_tokens: 100,
        completion_tokens: 50,
        ..Default::default()
    };
    let e = StreamEvent::Usage(usage);
    match e {
        StreamEvent::Usage(u) => {
            assert_eq!(u.prompt_tokens, 100);
            assert_eq!(u.completion_tokens, 50);
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn stream_event_tool_call() {
    let e = StreamEvent::ToolCall {
        summary: "read_file(src/main.rs)".to_string(),
    };
    match e {
        StreamEvent::ToolCall { summary } => assert_eq!(summary, "read_file(src/main.rs)"),
        _ => panic!("wrong variant"),
    }
}

#[test]
fn stream_event_tool_result_success() {
    let e = StreamEvent::ToolResult {
        summary: "write_file".to_string(),
        ok: true,
        elapsed_ms: 100,
        error: None,
    };
    match e {
        StreamEvent::ToolResult { ok, elapsed_ms, .. } => {
            assert!(ok);
            assert_eq!(elapsed_ms, 100);
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn stream_event_tool_result_failure() {
    let e = StreamEvent::ToolResult {
        summary: "exec".to_string(),
        ok: false,
        elapsed_ms: 50,
        error: Some("permission denied".to_string()),
    };
    match e {
        StreamEvent::ToolResult { ok, error, .. } => {
            assert!(!ok);
            assert_eq!(error.as_deref(), Some("permission denied"));
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn stream_event_confirm() {
    let (tx, _rx) = tokio::sync::oneshot::channel();
    let preview = ToolConfirmPreview::Generic {
        summary: "test".to_string(),
    };
    let e = StreamEvent::Confirm { preview, reply: tx };
    match e {
        StreamEvent::Confirm { preview, .. } => match preview {
            ToolConfirmPreview::Generic { summary } => assert_eq!(summary, "test"),
            _ => panic!("wrong preview variant"),
        },
        _ => panic!("wrong variant"),
    }
}

#[test]
fn stream_event_file_op() {
    let preview = vec![DiffLine {
        kind: DiffKind::Add,
        line_no: 1,
        text: "line1".to_string(),
    }];
    let e = StreamEvent::FileOp {
        verb: "write".to_string(),
        path: "test.rs".to_string(),
        added: 1,
        removed: 0,
        preview,
        truncated: false,
    };
    match e {
        StreamEvent::FileOp {
            verb, path, added, ..
        } => {
            assert_eq!(verb, "write");
            assert_eq!(path, "test.rs");
            assert_eq!(added, 1);
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn stream_event_plan_set() {
    let tasks = vec!["step 1".to_string(), "step 2".to_string()];
    let e = StreamEvent::PlanSet { tasks };
    match e {
        StreamEvent::PlanSet { tasks } => assert_eq!(tasks.len(), 2),
        _ => panic!("wrong variant"),
    }
}

#[test]
fn stream_event_plan_set_empty() {
    let e = StreamEvent::PlanSet { tasks: vec![] };
    match e {
        StreamEvent::PlanSet { tasks } => assert!(tasks.is_empty()),
        _ => panic!("wrong variant"),
    }
}

#[test]
fn stream_event_plan_task_done() {
    let e = StreamEvent::PlanTaskDone { index: 0 };
    match e {
        StreamEvent::PlanTaskDone { index } => assert_eq!(index, 0),
        _ => panic!("wrong variant"),
    }
}

#[test]
fn stream_event_plan_task_done_last() {
    let e = StreamEvent::PlanTaskDone { index: 99 };
    match e {
        StreamEvent::PlanTaskDone { index } => assert_eq!(index, 99),
        _ => panic!("wrong variant"),
    }
}

#[test]
fn stream_event_debug_status() {
    let e = StreamEvent::Status {
        message: "test".to_string(),
    };
    let debug = format!("{:?}", e);
    assert!(debug.contains("Status"));
}

#[test]
fn stream_event_debug_token() {
    let e = StreamEvent::Token("test".to_string());
    let debug = format!("{:?}", e);
    assert!(debug.contains("Token"));
}

// ============ TurnResult tests ============

#[test]
fn turn_result_default() {
    let r = TurnResult::default();
    assert!(r.text.is_empty());
    assert!(r.model_used.is_none());
    assert!(r.usage.is_none());
}

#[test]
fn turn_result_custom() {
    let r = TurnResult {
        text: "answer".to_string(),
        model_used: Some("gpt-4".to_string()),
        usage: Some(Usage {
            prompt_tokens: 10,
            completion_tokens: 5,
            ..Default::default()
        }),
    };
    assert_eq!(r.text, "answer");
    assert_eq!(r.model_used.as_deref(), Some("gpt-4"));
    assert!(r.usage.is_some());
}

#[test]
fn turn_result_clone() {
    let r = TurnResult {
        text: "hello".to_string(),
        ..Default::default()
    };
    let r2 = r.clone();
    assert_eq!(r2.text, "hello");
}

#[test]
fn turn_result_debug() {
    let r = TurnResult {
        text: "test".to_string(),
        ..Default::default()
    };
    let debug = format!("{:?}", r);
    assert!(debug.contains("TurnResult"));
}

#[test]
fn turn_result_empty_text_with_model() {
    let r = TurnResult {
        text: String::new(),
        model_used: Some("claude".to_string()),
        usage: None,
    };
    assert!(r.text.is_empty());
    assert_eq!(r.model_used.as_deref(), Some("claude"));
}

#[test]
fn turn_result_with_full_usage() {
    let usage = Usage {
        prompt_tokens: 100,
        completion_tokens: 200,
        total_tokens: 300,
        cache_read_tokens: 50,
        cache_write_tokens: 10,
    };
    let r = TurnResult {
        text: "result".to_string(),
        model_used: None,
        usage: Some(usage),
    };
    let u = r.usage.unwrap();
    assert_eq!(u.total_tokens, 300);
    assert_eq!(u.cache_read_tokens, 50);
}

// ============ ImageAttachment tests ============

#[test]
fn image_attachment_png() {
    let img = ImageAttachment {
        mime: "image/png".to_string(),
        data: "base64data".to_string(),
    };
    assert_eq!(img.mime, "image/png");
}

#[test]
fn image_attachment_jpeg() {
    let img = ImageAttachment {
        mime: "image/jpeg".to_string(),
        data: "data".to_string(),
    };
    assert_eq!(img.mime, "image/jpeg");
}

#[test]
fn image_attachment_clone() {
    let img = ImageAttachment {
        mime: "image/png".to_string(),
        data: "abc".to_string(),
    };
    let img2 = img.clone();
    assert_eq!(img2.data, "abc");
}

#[test]
fn image_attachment_debug() {
    let img = ImageAttachment {
        mime: "image/png".to_string(),
        data: "test".to_string(),
    };
    let debug = format!("{:?}", img);
    assert!(debug.contains("ImageAttachment"));
}

// ============ PromptRequest tests ============

#[test]
fn prompt_request_basic() {
    let req = PromptRequest {
        prompt: "hello".to_string(),
        model: None,
        system: None,
        workspace_context: None,
        history: vec![],
        images: vec![],
        mcp: None,
    };
    assert_eq!(req.prompt, "hello");
    assert!(req.model.is_none());
}

#[test]
fn prompt_request_with_model() {
    let req = PromptRequest {
        prompt: "test".to_string(),
        model: Some("gpt-4".to_string()),
        system: Some("be helpful".to_string()),
        workspace_context: Some("context".to_string()),
        history: vec![],
        images: vec![],
        mcp: None,
    };
    assert_eq!(req.model.as_deref(), Some("gpt-4"));
    assert_eq!(req.system.as_deref(), Some("be helpful"));
}

#[test]
fn prompt_request_with_history() {
    let history = vec![
        ChatMessage::user("q1".to_string()),
        ChatMessage::assistant("a1".to_string()),
    ];
    let req = PromptRequest {
        prompt: "q2".to_string(),
        model: None,
        system: None,
        workspace_context: None,
        history,
        images: vec![],
        mcp: None,
    };
    assert_eq!(req.history.len(), 2);
}

#[test]
fn prompt_request_with_images() {
    let images = vec![ImageAttachment {
        mime: "image/png".to_string(),
        data: "data".to_string(),
    }];
    let req = PromptRequest {
        prompt: "what's in this image?".to_string(),
        model: None,
        system: None,
        workspace_context: None,
        history: vec![],
        images,
        mcp: None,
    };
    assert_eq!(req.images.len(), 1);
}

#[test]
fn prompt_request_clone() {
    let req = PromptRequest {
        prompt: "test".to_string(),
        model: Some("model".to_string()),
        system: None,
        workspace_context: None,
        history: vec![],
        images: vec![],
        mcp: None,
    };
    let req2 = req.clone();
    assert_eq!(req2.prompt, "test");
}

#[test]
fn prompt_request_debug() {
    let req = PromptRequest {
        prompt: "test".to_string(),
        model: None,
        system: None,
        workspace_context: None,
        history: vec![],
        images: vec![],
        mcp: None,
    };
    let debug = format!("{:?}", req);
    assert!(debug.contains("PromptRequest"));
}
