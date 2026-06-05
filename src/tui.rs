use std::{path::PathBuf, time::{Duration, Instant}};

use anyhow::{Context, Result};
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyModifiers,
    MouseEvent, MouseEventKind,
};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};
use tokio::sync::{mpsc, oneshot};

use crate::{
    cli::AskOptions,
    config::AppConfig,
    provider::{
        ApprovalDecision, ApprovalPolicy, ChatMessage, ChatRole, ImageAttachment, PromptRequest,
        StreamEvent, ToolConfirmPreview, Usage,
    },
};

// ── Public stream event type ──────────────────────────────────────────────────

pub enum TuiEvent {
    Token(String),
    Status(String),
    ToolCall(String),
    ToolDone { summary: String, ok: bool },
    // diff: Vec<(is_add, line)>
    FileOp { verb: String, path: String, added: usize, removed: usize, diff: Vec<(bool, String)> },
    Confirm { summary: String, reply: oneshot::Sender<ApprovalDecision> },
    Usage(Usage),
    Error(String),
    PlanSet(Vec<String>),
    PlanTaskDone(usize),
}

// ── Display message types ─────────────────────────────────────────────────────

#[derive(Debug)]
enum Msg {
    User { text: String },
    Assistant { text: String },
    Tool { done: bool, ok: bool, text: String, elapsed_ms: Option<u128> },
    FileOp { verb: String, path: String, added: usize, removed: usize, diff: Vec<(bool, String)> },
    Error(String),
    System(String),
    Separator, // thin line between turns — "AI is done, your turn"
}

#[derive(Debug)]
struct PendingTool {
    summary: String,
}

#[derive(Debug)]
struct PendingConfirm {
    summary: String,
    reply: oneshot::Sender<ApprovalDecision>,
}

#[derive(Debug, PartialEq)]
enum Mode {
    Input,
    Streaming,
    Confirming,
}

// ── Application state ─────────────────────────────────────────────────────────

pub struct App {
    // conversation display
    messages: Vec<Msg>,
    streaming_buf: String,
    accumulated_response: String,
    pending_tool: Option<PendingTool>, // currently-running tool (not yet committed)
    tool_status: String,
    plan_tasks: Vec<String>,
    plan_done: Vec<bool>,

    // pending turn tracking
    pending_prompt: String,
    streaming_started_at: Option<Instant>,
    tool_started_at: Option<Instant>,
    unread_count: usize,
    seen_paths: std::collections::BTreeSet<String>,
    // undo stack: (path, old_content) — None content means file didn't exist before
    undo_stack: Vec<(String, Option<String>)>,

    // input
    input: String,
    input_cursor: usize,
    input_history: Vec<String>,
    hist_idx: Option<usize>,
    hist_saved: String,
    pending_image: Option<ImageAttachment>,
    last_image_fp: Option<String>,
    images_available: bool,

    // scroll
    scroll: usize,
    auto_scroll: bool,
    total_lines: usize,

    // status info
    provider: String,
    model: String,
    usage: Usage,
    cwd: String,

    // mode
    mode: Mode,
    confirm: Option<PendingConfirm>,
    mouse_capture: bool, // when false, terminal native text selection works

    // history & session
    history: Vec<ChatMessage>,
    session_path: Option<PathBuf>,
    pub last_saved_at: u64,

    // request params
    pub config: AppConfig,
    pub options: AskOptions,
    pub workspace_context: Option<String>,
    pub policy: ApprovalPolicy,
    pub mcp: Option<std::sync::Arc<crate::mcp::McpManager>>,

    // channels
    stream_rx: mpsc::UnboundedReceiver<TuiEvent>,
    stream_tx: mpsc::UnboundedSender<TuiEvent>,
    key_rx: mpsc::UnboundedReceiver<Event>,

    quit: bool,
    spinner_frame: usize,
}

impl App {
    pub fn new(
        provider: String,
        model: String,
        cwd: String,
        history: Vec<ChatMessage>,
        images_available: bool,
        session_path: Option<PathBuf>,
        last_saved_at: u64,
        input_history: Vec<String>,
        config: AppConfig,
        options: AskOptions,
        workspace_context: Option<String>,
        policy: ApprovalPolicy,
        key_rx: mpsc::UnboundedReceiver<Event>,
        mcp: Option<std::sync::Arc<crate::mcp::McpManager>>,
    ) -> Self {
        let (stream_tx, stream_rx) = mpsc::unbounded_channel();
        let messages = history
            .iter()
            .map(|m| match m.role {
                ChatRole::User => Msg::User { text: m.content.clone() },
                ChatRole::Assistant => Msg::Assistant { text: m.content.clone() },
            })
            .collect();

        Self {
            messages,
            streaming_buf: String::new(),
            accumulated_response: String::new(),
            pending_tool: None,
            tool_status: String::new(),
            plan_tasks: vec![],
            plan_done: vec![],
            pending_prompt: String::new(),
            streaming_started_at: None,
            tool_started_at: None,
            unread_count: 0,
            seen_paths: std::collections::BTreeSet::new(),
            undo_stack: Vec::new(),

            input: String::new(),
            input_cursor: 0,
            input_history,
            hist_idx: None,
            hist_saved: String::new(),
            pending_image: None,
            last_image_fp: None,
            images_available,

            scroll: usize::MAX,
            auto_scroll: true,
            total_lines: 0,

            provider,
            model,
            usage: Usage::default(),
            cwd,

            mode: Mode::Input,
            confirm: None,
            mouse_capture: true,

            history,
            session_path,
            last_saved_at,

            config,
            options,
            workspace_context,
            policy,
            mcp,

            stream_rx,
            stream_tx,
            key_rx,

            quit: false,
            spinner_frame: 0,
        }
    }
}

// ── Main TUI loop ─────────────────────────────────────────────────────────────

pub async fn run(mut app: App) -> Result<Vec<ChatMessage>> {
    crossterm::execute!(std::io::stdout(), EnableMouseCapture)?;
    let mut terminal = ratatui::init();
    terminal.clear()?;
    let result = event_loop(&mut terminal, &mut app).await;
    ratatui::restore();
    // Always release mouse capture on exit so the terminal works normally.
    crossterm::execute!(std::io::stdout(), DisableMouseCapture)?;
    result
}

fn set_mouse_capture(enabled: bool) {
    if enabled {
        let _ = crossterm::execute!(std::io::stdout(), EnableMouseCapture);
    } else {
        let _ = crossterm::execute!(std::io::stdout(), DisableMouseCapture);
    }
}


fn write_to_clipboard(text: &str) -> bool {
    // macOS
    if cfg!(target_os = "macos") {
        if let Ok(mut child) = std::process::Command::new("pbcopy").stdin(std::process::Stdio::piped()).spawn() {
            use std::io::Write;
            if let Some(stdin) = child.stdin.as_mut() {
                let _ = stdin.write_all(text.as_bytes());
            }
            return child.wait().map(|s| s.success()).unwrap_or(false);
        }
    }
    // Linux — try wl-copy (Wayland) then xclip (X11) then xsel
    for cmd in &[
        ("wl-copy", vec![]),
        ("xclip", vec!["-selection", "clipboard"]),
        ("xsel", vec!["--clipboard", "--input"]),
    ] {
        if let Ok(mut child) = std::process::Command::new(cmd.0)
            .args(&cmd.1)
            .stdin(std::process::Stdio::piped())
            .spawn()
        {
            use std::io::Write;
            if let Some(stdin) = child.stdin.as_mut() {
                let _ = stdin.write_all(text.as_bytes());
            }
            if child.wait().map(|s| s.success()).unwrap_or(false) {
                return true;
            }
        }
    }
    false
}

async fn event_loop(terminal: &mut DefaultTerminal, app: &mut App) -> Result<Vec<ChatMessage>> {
    loop {
        terminal.draw(|f| render(f, app))?;
        if app.quit {
            break;
        }
        tokio::select! {
            Some(ev) = app.key_rx.recv() => {
                handle_event(app, ev).await?;
            }
            Some(tui_ev) = app.stream_rx.recv() => {
                handle_stream_event(app, tui_ev).await;
            }
            _ = tokio::time::sleep(Duration::from_millis(80)) => {
                if app.mode == Mode::Streaming {
                    app.spinner_frame = app.spinner_frame.wrapping_add(1);
                }
            }
        }
    }
    Ok(app.history.clone())
}

// ── Event handling ────────────────────────────────────────────────────────────

async fn handle_event(app: &mut App, event: Event) -> Result<()> {
    match event {
        Event::Mouse(MouseEvent { kind, .. }) => handle_mouse(app, kind),
        Event::Key(key) => handle_key(app, key).await?,
        // Cmd+V / terminal paste — insert text, or attach image if paste is empty
        Event::Paste(text) => {
            if app.mode != Mode::Input { return Ok(()); }
            if text.trim().is_empty() {
                // Empty paste = user pasted an image (terminal can't forward it as text)
                // Try to grab it directly from the clipboard
                if app.images_available {
                    if let Some(img) = crate::grab_clipboard_image() {
                        app.pending_image = Some(img);
                        app.last_image_fp = None;
                        return Ok(());
                    }
                }
            } else {
                let normalized = text.replace('\r', "\n");
                app.input.insert_str(app.input_cursor, &normalized);
                app.input_cursor += normalized.len();
                app.hist_idx = None;
            }
        }
        Event::Resize(_, _) => {}
        _ => {}
    }
    Ok(())
}

fn handle_mouse(app: &mut App, kind: MouseEventKind) {
    match kind {
        MouseEventKind::ScrollUp => {
            app.auto_scroll = false;
            app.scroll = app.scroll.saturating_sub(3);
        }
        MouseEventKind::ScrollDown => {
            app.scroll = app.scroll.saturating_add(3);
            if app.scroll >= app.total_lines {
                app.auto_scroll = true;
                app.unread_count = 0;
            }
        }
        _ => {}
    }
}

async fn handle_key(app: &mut App, KeyEvent { code, modifiers, .. }: KeyEvent) -> Result<()> {
    // ── Confirming mode: y/a/n only ───────────────────────────────────────────
    if app.mode == Mode::Confirming {
        if let Some(confirm) = app.confirm.take() {
            let decision = match code {
                KeyCode::Char('y') | KeyCode::Enter => ApprovalDecision::AllowOnce,
                KeyCode::Char('a') => ApprovalDecision::AllowForTurn,
                _ => ApprovalDecision::Deny,
            };
            let _ = confirm.reply.send(decision);
            app.mode = Mode::Streaming;
        }
        return Ok(());
    }

    // ── Streaming mode: scroll only ───────────────────────────────────────────
    if app.mode == Mode::Streaming {
        match code {
            KeyCode::PageUp => {
                app.auto_scroll = false;
                app.scroll = app.scroll.saturating_sub(10);
            }
            KeyCode::PageDown => {
                app.scroll = app.scroll.saturating_add(10);
                if app.scroll >= app.total_lines {
                    app.auto_scroll = true;
                }
            }
            _ => {}
        }
        return Ok(());
    }

    // ── Input mode ────────────────────────────────────────────────────────────
    match code {
        // Submit (Enter) or newline (Shift+Enter)
        KeyCode::Enter if modifiers.contains(KeyModifiers::SHIFT) => {
            app.input.insert(app.input_cursor, '\n');
            app.input_cursor += 1;
            app.hist_idx = None;
        }
        KeyCode::Enter => {
            let text = app.input.trim().to_string();
            if text.is_empty() {
                return Ok(());
            }
            if !handle_slash_command(app, &text) {
                submit_prompt(app, text).await?;
            }
        }

        // Ctrl shortcuts
        KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
            if app.input.is_empty() {
                app.quit = true;
            } else {
                app.input.clear();
                app.input_cursor = 0;
                app.hist_idx = None;
            }
        }
        KeyCode::Char('d') if modifiers.contains(KeyModifiers::CONTROL) && app.input.is_empty() => {
            app.quit = true;
        }
        KeyCode::Char('u') if modifiers.contains(KeyModifiers::CONTROL) => {
            app.input.drain(..app.input_cursor);
            app.input_cursor = 0;
            app.hist_idx = None;
        }
        KeyCode::Char('w') if modifiers.contains(KeyModifiers::CONTROL) => {
            delete_word_before(&mut app.input, &mut app.input_cursor);
            app.hist_idx = None;
        }
        // Ctrl+V — universal paste: image first, then clipboard text
        KeyCode::Char('v') if modifiers.contains(KeyModifiers::CONTROL) => {
            if app.images_available {
                if let Some(img) = crate::grab_clipboard_image() {
                    app.pending_image = Some(img);
                    app.last_image_fp = None;
                    return Ok(());
                }
            }
            // No image — fall back to clipboard text
            if let Some(text) = crate::read_clipboard_text() {
                if !text.is_empty() {
                    let normalized = text.replace('\r', "\n");
                    app.input.insert_str(app.input_cursor, &normalized);
                    app.input_cursor += normalized.len();
                    app.hist_idx = None;
                }
            }
        }

        // Ctrl+M — toggle mouse capture (scroll mode ↔ select mode)
        KeyCode::Char('m') if modifiers.contains(KeyModifiers::CONTROL) => {
            app.mouse_capture = !app.mouse_capture;
            set_mouse_capture(app.mouse_capture);
        }

        // Editing
        KeyCode::Backspace => {
            if app.input_cursor > 0 {
                let len = prev_char_len(&app.input, app.input_cursor);
                let start = app.input_cursor - len;
                app.input.drain(start..app.input_cursor);
                app.input_cursor = start;
                app.hist_idx = None;
            }
        }
        KeyCode::Delete => {
            if app.input_cursor < app.input.len() {
                let len = next_char_len(&app.input, app.input_cursor);
                app.input.drain(app.input_cursor..app.input_cursor + len);
                app.hist_idx = None;
            }
        }

        // Cursor movement
        KeyCode::Left => move_cursor_left(&app.input.clone(), &mut app.input_cursor),
        KeyCode::Right => move_cursor_right(&app.input.clone(), &mut app.input_cursor),
        KeyCode::Home => app.input_cursor = 0,
        KeyCode::End => app.input_cursor = app.input.len(),

        // History navigation
        KeyCode::Up => {
            if !app.input_history.is_empty() {
                let new_idx = match app.hist_idx {
                    None => {
                        app.hist_saved = app.input.clone();
                        app.input_history.len() - 1
                    }
                    Some(0) => 0,
                    Some(i) => i - 1,
                };
                app.hist_idx = Some(new_idx);
                app.input = app.input_history[new_idx].clone();
                app.input_cursor = app.input.len();
            }
        }
        KeyCode::Down => {
            match app.hist_idx {
                None => {}
                Some(i) if i + 1 >= app.input_history.len() => {
                    app.hist_idx = None;
                    app.input = std::mem::take(&mut app.hist_saved);
                    app.input_cursor = app.input.len();
                }
                Some(i) => {
                    app.hist_idx = Some(i + 1);
                    app.input = app.input_history[i + 1].clone();
                    app.input_cursor = app.input.len();
                }
            }
        }

        // Scroll
        KeyCode::PageUp => {
            app.auto_scroll = false;
            app.scroll = app.scroll.saturating_sub(10);
        }
        KeyCode::PageDown => {
            app.scroll = app.scroll.saturating_add(10);
            if app.scroll >= app.total_lines {
                app.auto_scroll = true;
            }
        }

        // j/k vim-style scroll when input is empty
        KeyCode::Char('j') if app.input.is_empty() => {
            app.scroll = app.scroll.saturating_add(3);
            if app.scroll >= app.total_lines { app.auto_scroll = true; app.unread_count = 0; }
            else { app.auto_scroll = false; }
        }
        KeyCode::Char('k') if app.input.is_empty() => {
            app.auto_scroll = false;
            app.scroll = app.scroll.saturating_sub(3);
        }

        // Printable characters
        KeyCode::Char(c) => {
            let s = c.to_string();
            app.input.insert_str(app.input_cursor, &s);
            app.input_cursor += s.len();
            app.hist_idx = None;
        }

        _ => {}
    }
    Ok(())
}

// Returns true if the command was consumed (don't send to AI).
fn handle_slash_command(app: &mut App, text: &str) -> bool {
    match text {
        "/exit" | "/quit" | ":q" => {
            app.quit = true;
            true
        }
        "/clear" => {
            app.messages.clear();
            app.history.clear();
            app.streaming_buf.clear();
            app.accumulated_response.clear();
            app.usage = Usage::default();
            app.pending_image = None;
            app.seen_paths.clear();
            app.undo_stack.clear();
            app.input.clear();
            app.input_cursor = 0;
            if let Some(path) = &app.session_path {
                let _ = std::fs::remove_file(path);
            }
            true
        }
        "/help" => {
            app.messages.push(Msg::System(
                "Commands:\n\
                 /clear        reset conversation\n\
                 /undo         restore last file changed by AI\n\
                 /compact      drop old turns to free context\n\
                 /copy         copy last response to clipboard\n\
                 /export [path] save conversation as markdown\n\
                 /model [name] · /provider [name] · /status · /exit\n\
                 \n\
                 Keys: ↑/↓ history  ←/→ cursor  Home/End  Shift+Enter newline\n\
                 j/k scroll (when input empty)  PageUp/Dn scroll\n\
                 Ctrl+V paste (image or text)  Ctrl+M scroll/select mode\n\
                 Ctrl+W delete-word  Ctrl+U clear line\n\
                 \n\
                 Search: set BRAVE_SEARCH_API_KEY or SERPER_API_KEY for better results".into(),
            ));
            app.input.clear();
            app.input_cursor = 0;
            true
        }
        "/status" => {
            let u = &app.usage;
            app.messages.push(Msg::System(format!(
                "provider: {}  model: {}  turns: {}  tokens: {}↓ {}↑ {} total",
                app.provider,
                app.model,
                app.history.len() / 2,
                u.prompt_tokens,
                u.completion_tokens,
                u.total_tokens,
            )));
            app.input.clear();
            app.input_cursor = 0;
            true
        }
        "/copy" => {
            let last = app.messages.iter().rev().find_map(|m| {
                if let Msg::Assistant { text } = m { Some(text.clone()) } else { None }
            });
            match last {
                Some(text) => {
                    if write_to_clipboard(&text) {
                        app.messages.push(Msg::System("Last response copied to clipboard.".into()));
                    } else {
                        app.messages.push(Msg::Error("Could not write to clipboard (pbcopy/xclip/wl-copy not found).".into()));
                    }
                }
                None => app.messages.push(Msg::System("No assistant response to copy yet.".into())),
            }
            app.input.clear();
            app.input_cursor = 0;
            true
        }
        "/undo" => {
            match app.undo_stack.pop() {
                None => app.messages.push(Msg::System("Nothing to undo.".into())),
                Some((path, Some(old_content))) => {
                    match std::fs::write(&path, &old_content) {
                        Ok(()) => app.messages.push(Msg::System(format!("Restored {path}"))),
                        Err(e) => app.messages.push(Msg::Error(format!("Undo failed: {e}"))),
                    }
                }
                Some((path, None)) => {
                    // File was newly created — delete it
                    match std::fs::remove_file(&path) {
                        Ok(()) => app.messages.push(Msg::System(format!("Deleted {path} (undo create)"))),
                        Err(e) => app.messages.push(Msg::Error(format!("Undo failed: {e}"))),
                    }
                }
            }
            app.input.clear();
            app.input_cursor = 0;
            true
        }
        "/compact" => {
            // Keep only the last 10 turns, drop older history to free context
            let keep = 10usize;
            let total_turns = app.history.len() / 2;
            if total_turns <= keep {
                app.messages.push(Msg::System(format!(
                    "Conversation has {total_turns} turn(s) — nothing to compact yet (threshold: {keep})."
                )));
            } else {
                let drop_turns = total_turns - keep;
                let drop_msgs = drop_turns * 2;
                app.history.drain(..drop_msgs);
                // Also remove older messages from the display (keep separators and last N turns)
                let msg_count = app.messages.len();
                if msg_count > keep * 3 {
                    app.messages.drain(..(msg_count - keep * 3));
                }
                app.seen_paths.clear(); // refresh seen paths for the new context window
                app.messages.insert(0, Msg::System(format!(
                    "Context compacted: dropped {drop_turns} older turn(s), keeping the last {keep}. \
                     Use /clear to start fresh."
                )));
                app.messages.push(Msg::Separator);
            }
            app.input.clear();
            app.input_cursor = 0;
            true
        }
        s if s.starts_with("/export") => {
            let arg = s.strip_prefix("/export").unwrap().trim();
            let path = if arg.is_empty() {
                std::path::PathBuf::from(format!("anveesa-export-{}.md", crate::unix_now()))
            } else {
                std::path::PathBuf::from(arg)
            };
            match crate::export_conversation(&path, &app.history) {
                Ok(()) => app.messages.push(Msg::System(format!("Exported → {}", path.display()))),
                Err(e) => app.messages.push(Msg::Error(format!("export failed: {e:#}"))),
            }
            app.input.clear();
            app.input_cursor = 0;
            true
        }
        s if s.starts_with("/model") => {
            let arg = s.strip_prefix("/model").unwrap().trim();
            if arg.is_empty() {
                let current = app.model.clone();
                app.messages.push(Msg::System(format!("current model: {current}")));
            } else {
                app.model = arg.to_string();
                app.options.model = Some(arg.to_string());
                app.messages.push(Msg::System(format!("switched to model: {arg}")));
            }
            app.input.clear();
            app.input_cursor = 0;
            true
        }
        s if s.starts_with("/provider") => {
            let arg = s.strip_prefix("/provider").unwrap().trim();
            if arg.is_empty() {
                let current = app.provider.clone();
                app.messages.push(Msg::System(format!("current provider: {current}")));
            } else {
                // Validate provider exists
                if app.config.providers.contains_key(arg) {
                    app.provider = arg.to_string();
                    app.options.provider = Some(arg.to_string());
                    // Update model to provider default
                    if let Some(m) = app.config.providers.get(arg)
                        .and_then(|p| p.default_model())
                    {
                        app.model = m.to_string();
                        app.options.model = Some(m.to_string());
                    }
                    app.messages.push(Msg::System(format!("switched to provider: {arg}")));
                } else {
                    app.messages.push(Msg::Error(format!("unknown provider '{arg}'")));
                }
            }
            app.input.clear();
            app.input_cursor = 0;
            true
        }
        _ => false,
    }
}

async fn submit_prompt(app: &mut App, text: String) -> Result<()> {
    // Save to input history
    if app.input_history.last().map(|s| s.as_str()) != Some(&text) {
        app.input_history.push(text.clone());
    }
    app.hist_idx = None;
    app.pending_prompt = text.clone();
    app.accumulated_response.clear();

    // Auto-attach clipboard image if nothing was explicitly Ctrl+V'd
    let image = app.pending_image.take().or_else(|| {
        if !app.images_available { return None; }
        let img = crate::grab_clipboard_image()?;
        let fp = crate::image_fingerprint(&img);
        if app.last_image_fp.as_deref() == Some(&fp) {
            return None; // same image as last time
        }
        app.last_image_fp = Some(fp);
        Some(img)
    });

    app.messages.push(Msg::User { text: text.clone() });
    app.input.clear();
    app.input_cursor = 0;
    app.auto_scroll = true;
    app.mode = Mode::Streaming;
    app.tool_status = "Thinking".to_string();
    app.spinner_frame = 0;

    let provider_name = app
        .config
        .provider_name(app.options.provider.as_deref())
        .context("unknown provider")?
        .to_string();

    let (stream_tx_inner, stream_rx_inner) = mpsc::unbounded_channel::<StreamEvent>();

    // Clone everything needed for the spawned tasks
    let config = app.config.clone();
    let options = app.options.clone();
    let history = app.history.clone();
    // Augment workspace context with already-seen paths so the model doesn't re-scan them
    let workspace_context = augmented_workspace_context(
        app.workspace_context.as_deref(),
        &app.seen_paths,
    );
    let policy = app.policy;
    let mcp_arc = app.mcp.clone();
    let tui_tx = app.stream_tx.clone();
    let tui_tx2 = app.stream_tx.clone();

    // Task 1: call the provider
    tokio::spawn(async move {
        let request = PromptRequest {
            prompt: text,
            model: options.model.clone(),
            system: options.system.clone(),
            workspace_context,
            history,
            image,
            mcp: mcp_arc,
        };
        let result = crate::provider::ask(&config, &provider_name, request, policy, &stream_tx_inner).await;
        drop(stream_tx_inner);
        match result {
            Ok(turn) => {
                let _ = tui_tx.send(TuiEvent::Usage(turn.usage.unwrap_or_default()));
            }
            Err(e) => {
                let _ = tui_tx.send(TuiEvent::Error(format!("{e:#}")));
            }
        }
    });

    // Task 2: relay StreamEvents → TuiEvents
    tokio::spawn(async move {
        let mut rx = stream_rx_inner;
        while let Some(ev) = rx.recv().await {
            let tui_ev = match ev {
                StreamEvent::Token(t) => TuiEvent::Token(t),
                StreamEvent::Status { message } => TuiEvent::Status(message),
                StreamEvent::ToolCall { summary } => TuiEvent::ToolCall(summary),
                StreamEvent::ToolResult { summary, ok, .. } => TuiEvent::ToolDone { summary, ok },
                StreamEvent::FileOp { verb, path, added, removed, preview, .. } => {
                    let diff = preview.into_iter().map(|dl| {
                        let is_add = matches!(dl.kind, crate::provider::DiffKind::Add);
                        (is_add, dl.text)
                    }).collect();
                    TuiEvent::FileOp { verb, path, added, removed, diff }
                }
                StreamEvent::Confirm { preview, reply } => {
                    let summary = match &preview {
                        ToolConfirmPreview::FileOp { verb, path, added, removed, .. } =>
                            format!("{verb} {path}  +{added} -{removed}"),
                        ToolConfirmPreview::CreateDir { path } => format!("mkdir {path}"),
                        ToolConfirmPreview::Generic { summary } => summary.clone(),
                    };
                    TuiEvent::Confirm { summary, reply }
                }
                StreamEvent::Usage(u) => TuiEvent::Usage(u),
                StreamEvent::PlanSet { tasks } => TuiEvent::PlanSet(tasks),
                StreamEvent::PlanTaskDone { index } => TuiEvent::PlanTaskDone(index),
            };
            if tui_tx2.send(tui_ev).is_err() { break; }
        }
    });

    Ok(())
}

async fn handle_stream_event(app: &mut App, ev: TuiEvent) {
    match ev {
        TuiEvent::Token(text) => {
            if app.streaming_started_at.is_none() {
                app.streaming_started_at = Some(Instant::now());
            }
            app.streaming_buf.push_str(&text);
            if app.auto_scroll {
                app.scroll = usize::MAX;
            } else {
                app.unread_count += 1;
            }
        }
        TuiEvent::Status(msg) => {
            app.tool_status = msg;
        }
        TuiEvent::ToolCall(summary) => {
            flush_streaming_buf(app);
            commit_pending_tool(app, true);
            app.pending_tool = Some(PendingTool { summary: summary.clone() });
            app.tool_started_at = Some(Instant::now());
            app.tool_status = summary;
        }
        TuiEvent::ToolDone { summary, ok } => {
            let elapsed_ms = app.tool_started_at.take().map(|t| t.elapsed().as_millis());
            // Record the inspected path so we can tell the model what it already knows
            record_seen_path(&mut app.seen_paths, &summary);
            app.pending_tool = Some(PendingTool { summary });
            commit_pending_tool_timed(app, ok, elapsed_ms);
            app.tool_status = "Thinking".to_string();
        }
        TuiEvent::FileOp { verb, path, added, removed, diff } => {
            flush_streaming_buf(app);
            commit_pending_tool(app, true);
            // Snapshot for /undo (read current content before the write is reflected in messages)
            let old_content = std::fs::read_to_string(&path).ok();
            if app.undo_stack.len() >= 20 { app.undo_stack.remove(0); }
            app.undo_stack.push((path.clone(), old_content));
            app.messages.push(Msg::FileOp { verb, path, added, removed, diff });
        }
        TuiEvent::Confirm { summary, reply } => {
            flush_streaming_buf(app);
            commit_pending_tool(app, true);
            app.confirm = Some(PendingConfirm { summary, reply });
            app.mode = Mode::Confirming;
        }
        TuiEvent::Usage(u) => {
            app.usage.prompt_tokens += u.prompt_tokens;
            app.usage.completion_tokens += u.completion_tokens;
            app.usage.total_tokens += u.total_tokens;
            app.usage.cache_read_tokens += u.cache_read_tokens;
            app.usage.cache_write_tokens += u.cache_write_tokens;
            finish_turn(app);
        }
        TuiEvent::Error(msg) => {
            flush_streaming_buf(app);
            app.messages.push(Msg::Error(msg));
            app.mode = Mode::Input;
            app.tool_status.clear();
        }
        TuiEvent::PlanSet(tasks) => {
            app.plan_done = vec![false; tasks.len()];
            app.plan_tasks = tasks;
        }
        TuiEvent::PlanTaskDone(i) => {
            if i < app.plan_done.len() { app.plan_done[i] = true; }
        }
    }
}

/// Extract a path from a tool call summary string and record it as "already seen".
fn record_seen_path(seen: &mut std::collections::BTreeSet<String>, summary: &str) {
    // Summaries look like "read file src/foo.ts" or "list directory src/bar"
    // or "git status", "web search `...`" — only record file/dir paths
    for prefix in &["read file ", "list directory "] {
        if let Some(path) = summary.strip_prefix(prefix) {
            let path = path.trim().to_string();
            if !path.is_empty() {
                seen.insert(path);
            }
            return;
        }
    }
}

/// Build an augmented workspace context that includes already-seen paths.
fn augmented_workspace_context(
    base: Option<&str>,
    seen: &std::collections::BTreeSet<String>,
) -> Option<String> {
    if seen.is_empty() {
        return base.map(str::to_string);
    }
    let seen_note = format!(
        "\nAlready inspected this session (do NOT re-read these):\n{}",
        seen.iter().map(|p| format!("  - {p}")).collect::<Vec<_>>().join("\n")
    );
    Some(match base {
        Some(b) => format!("{b}{seen_note}"),
        None => seen_note,
    })
}

/// Flush streaming_buf to messages and accumulated_response.
fn flush_streaming_buf(app: &mut App) {
    if !app.streaming_buf.is_empty() {
        let text = std::mem::take(&mut app.streaming_buf);
        app.accumulated_response.push_str(&text);
        app.messages.push(Msg::Assistant { text });
    }
}

/// Commit a pending tool call to the message history with its final status.
fn commit_pending_tool(app: &mut App, ok: bool) {
    let elapsed = app.tool_started_at.take().map(|t| t.elapsed().as_millis());
    commit_pending_tool_timed(app, ok, elapsed);
}

fn commit_pending_tool_timed(app: &mut App, ok: bool, elapsed_ms: Option<u128>) {
    if let Some(tool) = app.pending_tool.take() {
        app.messages.push(Msg::Tool { done: true, ok, text: tool.summary, elapsed_ms });
    }
}

/// Commit the completed turn to history and save session.
fn finish_turn(app: &mut App) {
    commit_pending_tool(app, true);
    flush_streaming_buf(app);
    let response = std::mem::take(&mut app.accumulated_response);
    if !response.is_empty() {
        let prompt = std::mem::take(&mut app.pending_prompt);
        app.history.push(ChatMessage::user(prompt));
        app.history.push(ChatMessage::assistant(response));
        if let Some(path) = &app.session_path {
            if let Ok(cwd) = std::env::current_dir() {
                let _ = crate::save_interactive_session_pub(
                    path, &cwd, &app.provider, &app.options, &app.history,
                );
                app.last_saved_at = crate::unix_now();
            }
        }
    }
    app.mode = Mode::Input;
    app.tool_status.clear();
    app.streaming_started_at = None;
    app.tool_started_at = None;
    // Add a separator so the user sees clearly the AI is done
    if !app.history.is_empty() {
        app.messages.push(Msg::Separator);
    }
    // Auto-compact when history exceeds ~40K estimated tokens (1 char ≈ 0.25 tokens)
    auto_compact_if_needed(app);
}

fn auto_compact_if_needed(app: &mut App) {
    const TOKEN_THRESHOLD: usize = 40_000;
    let estimated: usize = app.history.iter().map(|m| m.content.len() / 4).sum();
    if estimated < TOKEN_THRESHOLD || app.history.len() < 8 {
        return;
    }
    // Drop oldest quarter of turns (keep at least 4 turns)
    let total_turns = app.history.len() / 2;
    let drop_turns = (total_turns / 4).max(1).min(total_turns.saturating_sub(4));
    let drop_msgs = drop_turns * 2;
    app.history.drain(..drop_msgs);
    app.seen_paths.clear();
    app.messages.push(Msg::System(format!(
        "Auto-compacted: dropped {drop_turns} older turn(s) (~{estimated}K est. tokens). Use /compact for manual control."
    )));
}

// ── Rendering ─────────────────────────────────────────────────────────────────

fn render(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let input_lines = app.input.lines().count().max(1);
    let input_height = (input_lines as u16).clamp(1, 5) + 2;

    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(input_height),
        Constraint::Length(1),
    ])
    .split(area);

    render_header(frame, chunks[0], app);
    render_messages(frame, chunks[1], app);
    render_input(frame, chunks[2], app);
    render_status(frame, chunks[3], app);
}

fn render_header(frame: &mut Frame, area: Rect, app: &App) {
    let version = env!("CARGO_PKG_VERSION");
    let token_str = if app.mode == Mode::Streaming && !app.streaming_buf.is_empty() {
        // Live estimate: chars / 4 ≈ tokens
        let live = app.streaming_buf.len() / 4;
        format!("  → {live}t")
    } else if app.usage.total_tokens > 0 {
        format!("  {}↓ {}↑", app.usage.prompt_tokens, app.usage.completion_tokens)
    } else {
        String::new()
    };
    let left = format!(" anveesa v{version}{token_str}");
    let right = format!(" {} · {} ", app.provider, app.model);
    let gap = (area.width as usize).saturating_sub(left.chars().count() + right.chars().count());
    let title = format!("{left}{}{right}", " ".repeat(gap));
    frame.render_widget(
        Paragraph::new(title).style(Style::default().fg(Color::Rgb(20, 20, 30)).bg(Color::Rgb(97, 175, 239))),
        area,
    );
}

fn render_messages(frame: &mut Frame, area: Rect, app: &mut App) {
    let width = area.width.saturating_sub(4) as usize;
    let mut lines: Vec<Line<'static>> = vec![Line::from("")];

    for msg in &app.messages {
        match msg {
            Msg::User { text } => {
                lines.push(user_header());
                for l in wrap_text(text, width) {
                    lines.push(Line::from(format!("    {l}")));
                }
                lines.push(Line::from(""));
            }
            Msg::Assistant { text } => {
                lines.push(assistant_header(&app.model));
                for l in format_assistant_lines(text, width) {
                    lines.push(l);
                }
                lines.push(Line::from(""));
            }
            Msg::Tool { done, ok, text, elapsed_ms } => {
                let (icon, color) = if !done {
                    ("⠋", Color::DarkGray)
                } else if *ok {
                    ("✓", Color::Rgb(152, 195, 121))
                } else {
                    ("✗", Color::Rgb(224, 108, 117))
                };
                let elapsed_str = match elapsed_ms {
                    Some(ms) if *ms < 1000 => format!("  {ms}ms"),
                    Some(ms) => format!("  {:.1}s", *ms as f64 / 1000.0),
                    None => String::new(),
                };
                lines.push(Line::from(vec![
                    Span::styled(format!("  {icon} {text}"), Style::default().fg(color)),
                    Span::styled(elapsed_str, Style::default().fg(Color::Rgb(80, 80, 100))),
                ]));
                lines.push(Line::from(""));
            }
            Msg::FileOp { verb, path, added, removed, diff } => {
                lines.push(Line::from(vec![
                    Span::styled("  📄 ", Style::default().fg(Color::Rgb(229, 192, 123))),
                    Span::styled(format!("{verb} "), Style::default().fg(Color::White)),
                    Span::styled(path.clone(), Style::default().fg(Color::Rgb(97, 175, 239))),
                    Span::styled(format!("  +{added}"), Style::default().fg(Color::Rgb(152, 195, 121))),
                    Span::styled(format!(" -{removed}"), Style::default().fg(Color::Rgb(224, 108, 117))),
                ]));
                // Show inline diff (up to 40 lines)
                for (is_add, line) in diff.iter().take(40) {
                    let (prefix, color) = if *is_add {
                        ("  + ", Color::Rgb(152, 195, 121))
                    } else {
                        ("  - ", Color::Rgb(224, 108, 117))
                    };
                    let bg = if *is_add { Color::Rgb(20, 35, 20) } else { Color::Rgb(35, 20, 20) };
                    lines.push(Line::from(Span::styled(
                        format!("{prefix}{}", &line.trim_end().chars().take(width.saturating_sub(6)).collect::<String>()),
                        Style::default().fg(color).bg(bg),
                    )));
                }
                if diff.len() > 40 {
                    lines.push(Line::from(Span::styled(
                        format!("  … {} more lines", diff.len() - 40),
                        Style::default().fg(Color::DarkGray),
                    )));
                }
                lines.push(Line::from(""));
            }
            Msg::Error(msg) => {
                lines.push(Line::from(Span::styled(
                    format!("  ✗ {msg}"),
                    Style::default().fg(Color::Rgb(224, 108, 117)),
                )));
                lines.push(Line::from(""));
            }
            Msg::System(msg) => {
                for l in msg.lines() {
                    lines.push(Line::from(Span::styled(
                        format!("  · {l}"),
                        Style::default().fg(Color::DarkGray),
                    )));
                }
                lines.push(Line::from(""));
            }
            Msg::Separator => {
                // Thin line between turns — signals "AI is done, your turn"
                let line_width = width.saturating_sub(2);
                lines.push(Line::from(Span::styled(
                    format!("  {}", "─".repeat(line_width.min(60))),
                    Style::default().fg(Color::Rgb(45, 45, 65)),
                )));
                lines.push(Line::from(""));
            }
        }
    }

    // Live pending tool (running, not yet committed) — animated with elapsed time
    if let Some(tool) = &app.pending_tool {
        let dots = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        let dot = dots[app.spinner_frame % dots.len()];
        let elapsed = app.tool_started_at
            .map(|t| t.elapsed().as_secs_f32())
            .unwrap_or(0.0);
        let elapsed_str = if elapsed < 0.5 { String::new() } else { format!(" ({:.1}s)", elapsed) };
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {dot} {}{}", tool.summary, elapsed_str),
                Style::default().fg(Color::Rgb(180, 140, 60)),
            ),
        ]));
        lines.push(Line::from(""));
    }

    // In-progress streaming — assistant message being built token by token
    if !app.streaming_buf.is_empty() || (app.mode == Mode::Streaming && app.pending_tool.is_none()) {
        lines.push(assistant_header(&app.model));
        if !app.streaming_buf.is_empty() {
            for l in format_assistant_lines(&app.streaming_buf, width) {
                lines.push(l);
            }
        } else {
            let dots = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
            let dot = dots[app.spinner_frame % dots.len()];
            let elapsed = app.streaming_started_at
                .map(|t| t.elapsed().as_secs_f32())
                .unwrap_or(0.0);
            let elapsed_str = if elapsed < 0.5 { String::new() } else { format!(" ({:.1}s)", elapsed) };
            let status = if app.tool_status.is_empty() { "Thinking" } else { app.tool_status.as_str() };
            lines.push(Line::from(Span::styled(
                format!("    {dot} {status}{elapsed_str}"),
                Style::default().fg(Color::Rgb(180, 140, 60)),
            )));
        }
        lines.push(Line::from(""));
    }

    // Add bottom padding so wrapped last lines are never cut off by viewport
    for _ in 0..3 { lines.push(Line::from("")); }

    // Estimate visual rows (accounting for line wrapping) for accurate auto-scroll
    let visual_rows: usize = if width == 0 { lines.len() } else {
        lines.iter().map(|l| {
            let chars: usize = l.spans.iter().map(|s| s.content.chars().count()).sum();
            if chars == 0 { 1 } else { chars.div_ceil(width) }
        }).sum()
    };

    let total = lines.len();
    app.total_lines = total;
    let visible = area.height as usize;
    let scroll = if app.auto_scroll || app.scroll == usize::MAX {
        // Use visual-row estimate to scroll accurately to the bottom
        visual_rows.saturating_sub(visible)
    } else {
        app.scroll.min(total.saturating_sub(1))
    };
    app.scroll = scroll;

    // "↓ unread" badge overlay when scrolled away
    let mut widget_lines = lines;
    if !app.auto_scroll && app.unread_count > 0 {
        let badge = format!(" ↓ {} new ", app.unread_count);
        widget_lines.push(Line::from(Span::styled(
            badge,
            Style::default()
                .fg(Color::Black)
                .bg(Color::Rgb(97, 175, 239))
                .add_modifier(Modifier::BOLD),
        )));
    }

    frame.render_widget(
        Paragraph::new(widget_lines).scroll((scroll as u16, 0)),
        area,
    );
}

fn user_header() -> Line<'static> {
    Line::from(Span::styled(
        "  ● You",
        Style::default().fg(Color::Rgb(97, 175, 239)).add_modifier(Modifier::BOLD),
    ))
}

fn assistant_header(model: &str) -> Line<'static> {
    Line::from(Span::styled(
        format!("  ● {model}"),
        Style::default().fg(Color::Rgb(152, 195, 121)).add_modifier(Modifier::BOLD),
    ))
}

fn render_input(frame: &mut Frame, area: Rect, app: &App) {
    // Border color reflects mode: ready=green, streaming=yellow, confirming=orange
    let border_color = match app.mode {
        Mode::Input     => Color::Rgb(152, 195, 121), // green — "your turn"
        Mode::Streaming => Color::Rgb(229, 192, 123), // yellow — "thinking"
        Mode::Confirming=> Color::Rgb(224, 108, 117), // red — "needs decision"
    };
    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(border_color));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if app.mode != Mode::Input {
        // Don't show cursor or text while AI is working
        return;
    }

    if app.input.is_empty() && app.pending_image.is_none() {
        // Placeholder hint
        frame.render_widget(
            Paragraph::new("  ❯ Ask anything…  (↑/↓ history · Ctrl+V paste image)")
                .style(Style::default().fg(Color::Rgb(60, 60, 80))),
            inner,
        );
        frame.set_cursor_position((inner.x + 4, inner.y));
        return;
    }

    let label = if app.pending_image.is_some() { "  [📎] ❯ " } else { "  ❯ " };
    let label_w = label.chars().count();
    let display = format!("{label}{}", app.input);

    frame.render_widget(
        Paragraph::new(display).style(Style::default().fg(Color::White)).wrap(Wrap { trim: false }),
        inner,
    );

    let cursor_chars = label_w + app.input[..app.input_cursor].chars().count();
    let w = inner.width.max(1) as usize;
    frame.set_cursor_position((
        inner.x + (cursor_chars % w) as u16,
        inner.y + (cursor_chars / w) as u16,
    ));
}

fn render_status(frame: &mut Frame, area: Rect, app: &App) {
    match app.mode {
        Mode::Confirming => {
            let summary = app.confirm.as_ref().map(|c| c.summary.as_str()).unwrap_or("?");
            let text = format!(" ⚠  {summary}   [y] allow once   [a] allow all   [n] deny ");
            frame.render_widget(
                Paragraph::new(text)
                    .style(Style::default().fg(Color::Black).bg(Color::Rgb(224, 108, 117))),
                area,
            );
        }
        Mode::Streaming => {
            let dots = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
            let dot = dots[app.spinner_frame % dots.len()];
            let elapsed = app.streaming_started_at
                .map(|t| t.elapsed().as_secs_f32())
                .unwrap_or(0.0);
            let state = if !app.tool_status.is_empty() {
                format!("{dot} {}  ({:.1}s)", app.tool_status, elapsed)
            } else {
                format!("{dot} Thinking…  ({:.1}s)", elapsed)
            };
            let left = format!(" {state}");
            let right = format!(" {}  Ctrl+C cancel ", app.cwd);
            let gap = (area.width as usize).saturating_sub(left.chars().count() + right.chars().count());
            let text = format!("{left}{}{right}", " ".repeat(gap));
            frame.render_widget(
                Paragraph::new(text)
                    .style(Style::default().fg(Color::Rgb(229, 192, 123)).bg(Color::Rgb(30, 28, 20))),
                area,
            );
        }
        Mode::Input => {
            let mode_icon = if app.mouse_capture { "⊙" } else { "⊕" };
            let mode_label = if app.mouse_capture { "scroll" } else { "select" };
            let left = format!(" ● Ready  {}", app.cwd);
            let right = format!(" {mode_icon} {mode_label}  /help ");
            let gap = (area.width as usize).saturating_sub(left.chars().count() + right.chars().count());
            let text = format!("{left}{}{right}", " ".repeat(gap));
            frame.render_widget(
                Paragraph::new(text)
                    .style(Style::default().fg(Color::Rgb(152, 195, 121)).bg(Color::Rgb(20, 30, 20))),
                area,
            );
        }
    }
}

// ── Text formatting ───────────────────────────────────────────────────────────

fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 { return vec![text.to_string()]; }
    let mut out = Vec::new();
    for line in text.lines() {
        if line.is_empty() { out.push(String::new()); continue; }
        let mut current = String::new();
        let mut col = 0usize;
        for word in line.split_whitespace() {
            let wlen = word.chars().count();
            if col > 0 && col + 1 + wlen > width {
                out.push(current.clone());
                current.clear();
                col = 0;
            }
            if col > 0 { current.push(' '); col += 1; }
            current.push_str(word);
            col += wlen;
        }
        out.push(current);
    }
    out
}

fn format_assistant_lines(text: &str, width: usize) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    let mut in_code = false;
    let mut code_lang = String::new();

    for raw in text.lines() {
        if raw.starts_with("```") {
            if in_code {
                in_code = false;
                code_lang.clear();
                out.push(Line::from(Span::styled(
                    "    └──────────────────────".to_string(),
                    Style::default().fg(Color::Rgb(50, 50, 70)),
                )));
            } else {
                in_code = true;
                code_lang = raw[3..].trim().to_string();
                let lang = if code_lang.is_empty() { String::new() } else { format!(" {} ", code_lang) };
                out.push(Line::from(Span::styled(
                    format!("    ┌─{lang}"),
                    Style::default().fg(Color::Rgb(50, 50, 70)),
                )));
            }
            continue;
        }

        if in_code {
            out.push(highlight_code_line(raw, &code_lang));
        } else {
            let wrapped = if width > 4 && raw.chars().count() + 4 > width {
                wrap_text(raw, width.saturating_sub(4))
            } else {
                vec![raw.to_string()]
            };
            for l in wrapped {
                out.push(format_prose_line(&l));
            }
        }
    }
    out
}

fn format_prose_line(line: &str) -> Line<'static> {
    if line.is_empty() { return Line::from(""); }

    if line.starts_with("### ") {
        return Line::from(Span::styled(
            format!("    {}", &line[4..]),
            Style::default().fg(Color::Rgb(198, 160, 246)).add_modifier(Modifier::BOLD),
        ));
    }
    if line.starts_with("## ") {
        return Line::from(Span::styled(
            format!("    {}", &line[3..]),
            Style::default().fg(Color::Rgb(198, 160, 246)).add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        ));
    }
    if line.starts_with("# ") {
        return Line::from(Span::styled(
            format!("    {}", &line[2..]),
            Style::default().fg(Color::Rgb(198, 160, 246)).add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        ));
    }

    let (prefix, rest) = if let Some(s) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
        ("    • ", s)
    } else {
        ("    ", line)
    };

    Line::from(parse_inline(&format!("{prefix}{rest}")))
}

fn parse_inline(text: &str) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut chars = text.chars().peekable();
    let mut buf = String::new();

    while let Some(c) = chars.next() {
        if c == '`' {
            if !buf.is_empty() { spans.push(Span::raw(buf.clone())); buf.clear(); }
            let mut code = String::new();
            for ch in chars.by_ref() { if ch == '`' { break; } code.push(ch); }
            spans.push(Span::styled(code, Style::default().fg(Color::Rgb(229, 192, 123)).bg(Color::Rgb(40, 40, 55))));
        } else if c == '*' && chars.peek() == Some(&'*') {
            chars.next();
            if !buf.is_empty() { spans.push(Span::raw(buf.clone())); buf.clear(); }
            let mut bold = String::new();
            loop {
                match chars.next() {
                    Some('*') if chars.peek() == Some(&'*') => { chars.next(); break; }
                    Some(ch) => bold.push(ch),
                    None => break,
                }
            }
            spans.push(Span::styled(bold, Style::default().add_modifier(Modifier::BOLD)));
        } else if c == '*' {
            if !buf.is_empty() { spans.push(Span::raw(buf.clone())); buf.clear(); }
            let mut italic = String::new();
            for ch in chars.by_ref() { if ch == '*' { break; } italic.push(ch); }
            spans.push(Span::styled(italic, Style::default().add_modifier(Modifier::ITALIC)));
        } else {
            buf.push(c);
        }
    }
    if !buf.is_empty() { spans.push(Span::raw(buf)); }
    spans
}

fn highlight_code_line(line: &str, _lang: &str) -> Line<'static> {
    static KEYWORDS: &[&str] = &[
        "fn", "let", "mut", "const", "struct", "enum", "impl", "trait", "use", "pub",
        "mod", "return", "if", "else", "for", "while", "loop", "match", "async", "await",
        "self", "Self", "true", "false", "Some", "None", "Ok", "Err", "type", "where",
        "def", "class", "import", "from", "pass", "with", "as", "in", "not", "and", "or",
        "var", "function", "new", "this", "typeof", "instanceof", "yield", "break", "continue",
        "int", "str", "bool", "float", "None", "True", "False", "null", "undefined",
        "interface", "extends", "implements", "static", "final", "void", "package",
    ];

    let bg = Color::Rgb(28, 28, 40);
    let mut spans: Vec<Span<'static>> = vec![
        Span::styled("      ".to_string(), Style::default().bg(bg)),
    ];

    let mut chars = line.chars().peekable();
    let mut buf = String::new();
    let mut in_string = false;
    let mut string_char = '"';

    let flush = |buf: &mut String, spans: &mut Vec<Span<'static>>| {
        if buf.is_empty() { return; }
        let s = buf.clone();
        let style = if KEYWORDS.contains(&s.as_str()) {
            Style::default().fg(Color::Rgb(198, 120, 221)).bg(bg)
        } else {
            Style::default().fg(Color::Rgb(171, 178, 191)).bg(bg)
        };
        spans.push(Span::styled(s, style));
        buf.clear();
    };

    while let Some(c) = chars.next() {
        if in_string {
            buf.push(c);
            if c == string_char {
                let s = buf.clone();
                spans.push(Span::styled(s, Style::default().fg(Color::Rgb(152, 195, 121)).bg(bg)));
                buf.clear();
                in_string = false;
            }
            continue;
        }

        // Line comments
        if (c == '/' && chars.peek() == Some(&'/')) || c == '#' {
            flush(&mut buf, &mut spans);
            let rest: String = std::iter::once(c).chain(chars.by_ref()).collect();
            spans.push(Span::styled(rest, Style::default().fg(Color::Rgb(92, 99, 112)).bg(bg)));
            break;
        }

        // String start
        if c == '"' || c == '\'' {
            flush(&mut buf, &mut spans);
            in_string = true;
            string_char = c;
            buf.push(c);
            continue;
        }

        // Numbers
        if c.is_ascii_digit() && buf.is_empty() {
            flush(&mut buf, &mut spans);
            let mut num = c.to_string();
            while let Some(&n) = chars.peek() {
                if n.is_ascii_alphanumeric() || n == '.' || n == '_' { num.push(n); chars.next(); }
                else { break; }
            }
            spans.push(Span::styled(num, Style::default().fg(Color::Rgb(209, 154, 102)).bg(bg)));
            continue;
        }

        if c.is_alphanumeric() || c == '_' {
            buf.push(c);
        } else {
            flush(&mut buf, &mut spans);
            spans.push(Span::styled(c.to_string(), Style::default().fg(Color::Rgb(171, 178, 191)).bg(bg)));
        }
    }
    flush(&mut buf, &mut spans);

    // Fill remainder with bg color
    let content_len: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    if content_len < 84 {
        spans.push(Span::styled(" ".repeat(84 - content_len), Style::default().bg(bg)));
    }

    Line::from(spans)
}

// ── String/cursor helpers ─────────────────────────────────────────────────────

fn prev_char_len(s: &str, pos: usize) -> usize {
    s[..pos].chars().next_back().map(|c| c.len_utf8()).unwrap_or(0)
}

fn next_char_len(s: &str, pos: usize) -> usize {
    s[pos..].chars().next().map(|c| c.len_utf8()).unwrap_or(0)
}

fn move_cursor_left(s: &str, pos: &mut usize) {
    *pos = pos.saturating_sub(prev_char_len(s, *pos));
}

fn move_cursor_right(s: &str, pos: &mut usize) {
    *pos = (*pos + next_char_len(s, *pos)).min(s.len());
}

fn delete_word_before(s: &mut String, pos: &mut usize) {
    while *pos > 0 && s[..*pos].ends_with(|c: char| c == ' ' || c == '\n') {
        let len = prev_char_len(s, *pos);
        let start = *pos - len;
        s.drain(start..*pos);
        *pos = start;
    }
    while *pos > 0 && !s[..*pos].ends_with(|c: char| c == ' ' || c == '\n') {
        let len = prev_char_len(s, *pos);
        let start = *pos - len;
        s.drain(start..*pos);
        *pos = start;
    }
}
