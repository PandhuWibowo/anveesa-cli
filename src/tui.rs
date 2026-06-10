mod commands;
mod format;
mod input;
mod render;
mod stream;

use std::{
    collections::BTreeSet,
    path::PathBuf,
    time::{Duration, Instant},
};

use anyhow::Result;
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyModifiers, MouseEvent,
    MouseEventKind,
};
use ratatui::DefaultTerminal;
use tokio::sync::{mpsc, oneshot};

use crate::{
    cli::AskOptions,
    config::AppConfig,
    provider::{ApprovalDecision, ApprovalPolicy, ChatMessage, ChatRole, ImageAttachment, Usage},
};

use self::commands::handle_slash_command;
use self::format::{
    delete_word_before, move_cursor_left, move_cursor_right, next_char_len, prev_char_len,
};
use self::input::{tab_complete, update_search};
use self::render::{render, set_mouse_capture};
use self::stream::{handle_stream_event, submit_prompt};

// ── Public stream event type ──────────────────────────────────────────────────

pub enum TuiEvent {
    Token(String),
    Thinking(String),
    Status(String),
    ToolCall(String),
    ToolDone {
        summary: String,
        ok: bool,
        elapsed_ms: u128,
    },
    FileOp {
        verb: String,
        path: String,
        added: usize,
        removed: usize,
        diff: Vec<(bool, String)>,
    },
    Confirm {
        summary: String,
        diff: Vec<(bool, String)>,
        reply: oneshot::Sender<ApprovalDecision>,
    },
    Usage(Usage),
    ModelUsed(String),
    SystemMsg(String),
    Error(String),
    PlanSet(Vec<String>),
    PlanTaskDone(usize),
    SetInput(String),
}

// ── Display message types ─────────────────────────────────────────────────────

#[derive(Debug)]
enum Msg {
    User {
        text: String,
    },
    Assistant {
        text: String,
    },
    Tool {
        done: bool,
        ok: bool,
        text: String,
        elapsed_ms: Option<u128>,
    },
    FileOp {
        verb: String,
        path: String,
        added: usize,
        removed: usize,
        diff: Vec<(bool, String)>,
        collapsed: bool,
    },
    Thinking {
        text: String,
        collapsed: bool,
    },
    Error(String),
    System(String),
    Separator, // thin line between turns — "AI is done, your turn"
}

#[derive(Debug)]
struct PendingTool {
    summary: String,
    started_at: Instant,
}

#[derive(Debug)]
struct PendingConfirm {
    summary: String,
    diff: Vec<(bool, String)>,
    reply: oneshot::Sender<ApprovalDecision>,
}

#[derive(Debug, PartialEq)]
enum Mode {
    Input,
    Streaming,
    Confirming,
    Search,
}

// ── Focused sub-structs ───────────────────────────────────────────────────────

struct InputState {
    input: String,
    input_cursor: usize,
    input_history: Vec<String>,
    hist_idx: Option<usize>,
    hist_saved: String,
    pending_images: Vec<ImageAttachment>,
    last_image_fp: Option<String>,
    tab_state: Option<(String, Vec<String>, usize)>,
}

struct StreamState {
    streaming_buf: String,
    accumulated_response: String,
    // Tools currently executing — several can run concurrently in agent mode.
    pending_tools: Vec<PendingTool>,
    tool_status: String,
    plan_tasks: Vec<String>,
    plan_done: Vec<bool>,
    pending_prompt: String,
    streaming_started_at: Option<Instant>,
    unread_count: usize,
    thinking_buf: String,
}

pub(crate) struct ConvState {
    history: Vec<ChatMessage>,
    session_path: Option<PathBuf>,
    pub last_saved_at: u64,
    seen_paths: BTreeSet<String>,
    undo_stack: Vec<(String, Option<String>)>,
}

struct ViewState {
    messages: Vec<Msg>,
    scroll: usize,
    auto_scroll: bool,
    total_lines: usize,
    msg_focus: Option<usize>,
    msg_line_offsets: Vec<usize>,
    search_query: String,
    search_results: Vec<usize>,
    search_idx: usize,
    search_scroll_saved: usize,
    mouse_capture: bool,
    /// Cached formatted lines per message index: Option<(content_hash, lines)>. Indexed by msg index.
    render_cache: Vec<Option<(u64, Vec<ratatui::text::Line<'static>>)>>,
    /// Length of streaming_buf last time we rendered (for skip-diff).
    render_cache_streaming_len: usize,
    /// Last time we drew a frame (for render throttling during streaming).
    last_render: Option<std::time::Instant>,
}

// ── Application state ─────────────────────────────────────────────────────────

pub struct App {
    // mode
    mode: Mode,
    confirm: Option<PendingConfirm>,

    // status info
    provider: String,
    model: String,
    last_model_used: Option<String>,
    usage: Usage,
    session_cost_usd: f64,
    cwd: String,
    images_available: bool,

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

    // focused sub-structs
    kbd: InputState,
    live: StreamState,
    pub(crate) conv: ConvState,
    view: ViewState,
}

impl App {
    #[allow(clippy::too_many_arguments)]
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
                ChatRole::User => Msg::User {
                    text: m.content.clone(),
                },
                ChatRole::Assistant => Msg::Assistant {
                    text: m.content.clone(),
                },
            })
            .collect();

        Self {
            mode: Mode::Input,
            confirm: None,

            provider,
            model,
            last_model_used: None,
            usage: Usage::default(),
            session_cost_usd: 0.0,
            cwd,
            images_available,

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

            kbd: InputState {
                input: String::new(),
                input_cursor: 0,
                input_history,
                hist_idx: None,
                hist_saved: String::new(),
                pending_images: Vec::new(),
                last_image_fp: None,
                tab_state: None,
            },

            live: StreamState {
                streaming_buf: String::new(),
                accumulated_response: String::new(),
                pending_tools: Vec::new(),
                tool_status: String::new(),
                plan_tasks: vec![],
                plan_done: vec![],
                pending_prompt: String::new(),
                streaming_started_at: None,
                unread_count: 0,
                thinking_buf: String::new(),
            },

            conv: ConvState {
                history,
                session_path,
                last_saved_at,
                seen_paths: BTreeSet::new(),
                undo_stack: Vec::new(),
            },

            view: ViewState {
                messages,
                scroll: usize::MAX,
                auto_scroll: true,
                total_lines: 0,
                msg_focus: None,
                msg_line_offsets: Vec::new(),
                search_query: String::new(),
                search_results: Vec::new(),
                search_idx: 0,
                search_scroll_saved: 0,
                mouse_capture: true,
                render_cache: Vec::new(),
                render_cache_streaming_len: 0,
                last_render: None,
            },
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

async fn event_loop(terminal: &mut DefaultTerminal, app: &mut App) -> Result<Vec<ChatMessage>> {
    loop {
        if app.quit {
            break;
        }
        // Throttle drawing during streaming (~20 fps cap) but NEVER skip event
        // processing — a skipped frame is repainted on the next 80 ms tick,
        // while a skipped event would back up the channel and freeze the UI.
        let needs_render = app.mode != Mode::Streaming
            || app.live.streaming_buf.len() != app.view.render_cache_streaming_len
            || !app.live.pending_tools.is_empty();
        let throttled = app.mode == Mode::Streaming
            && app
                .view
                .last_render
                .is_some_and(|t| t.elapsed() < Duration::from_millis(50));
        if needs_render && !throttled {
            terminal.draw(|f| render(f, app))?;
            app.view.last_render = Some(std::time::Instant::now());
            app.view.render_cache_streaming_len = app.live.streaming_buf.len();
        }
        tokio::select! {
            Some(ev) = app.key_rx.recv() => {
                handle_event(app, ev).await?;
            }
            Some(tui_ev) = app.stream_rx.recv() => {
                handle_stream_event(app, tui_ev).await;
                // Drain any burst of stream events (concurrent tools, fast
                // tokens) before the next draw so rendering can't fall behind.
                let mut drained = 0;
                while drained < 256 {
                    match app.stream_rx.try_recv() {
                        Ok(ev) => {
                            handle_stream_event(app, ev).await;
                            drained += 1;
                        }
                        Err(_) => break,
                    }
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(80)) => {
                if app.mode == Mode::Streaming {
                    app.spinner_frame = app.spinner_frame.wrapping_add(1);
                }
            }
        }
    }
    Ok(app.conv.history.clone())
}

// ── Event handling ────────────────────────────────────────────────────────────

async fn handle_event(app: &mut App, event: Event) -> Result<()> {
    match event {
        Event::Mouse(MouseEvent { kind, .. }) => handle_mouse(app, kind),
        Event::Key(key) => handle_key(app, key).await?,
        // Cmd+V / terminal paste — insert text, or attach image if paste is empty
        Event::Paste(text) => {
            if app.mode != Mode::Input {
                return Ok(());
            }
            if text.trim().is_empty() {
                if app.images_available
                    && let Some(img) = crate::image::grab_clipboard_image()
                {
                    app.kbd.pending_images.push(img);
                    app.kbd.last_image_fp = None;
                    return Ok(());
                }
            } else {
                let normalized = text.replace('\r', "\n");
                app.kbd.input.insert_str(app.kbd.input_cursor, &normalized);
                app.kbd.input_cursor += normalized.len();
                app.kbd.hist_idx = None;
                app.kbd.tab_state = None;
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
            app.view.auto_scroll = false;
            app.view.scroll = app.view.scroll.saturating_sub(3);
        }
        MouseEventKind::ScrollDown => {
            app.view.scroll = app.view.scroll.saturating_add(3);
            if app.view.scroll >= app.view.total_lines {
                app.view.auto_scroll = true;
                app.live.unread_count = 0;
            }
        }
        _ => {}
    }
}

async fn handle_key(
    app: &mut App,
    KeyEvent {
        code, modifiers, ..
    }: KeyEvent,
) -> Result<()> {
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
                app.view.auto_scroll = false;
                app.view.scroll = app.view.scroll.saturating_sub(10);
            }
            KeyCode::PageDown => {
                app.view.scroll = app.view.scroll.saturating_add(10);
                // Re-enable auto-scroll when reaching the bottom
                if app.view.scroll >= app.view.total_lines.saturating_sub(10) {
                    app.view.auto_scroll = true;
                    app.live.unread_count = 0;
                }
            }
            KeyCode::Char('j') | KeyCode::Down => {
                app.view.auto_scroll = false;
                app.view.scroll = app.view.scroll.saturating_add(1);
            }
            KeyCode::Char('k') | KeyCode::Up => {
                app.view.auto_scroll = false;
                app.view.scroll = app.view.scroll.saturating_sub(1);
            }
            _ => {}
        }
        return Ok(());
    }

    // ── Search mode ───────────────────────────────────────────────────────────
    if app.mode == Mode::Search {
        match code {
            KeyCode::Esc => {
                app.mode = Mode::Input;
                app.view.auto_scroll = false;
                app.view.scroll = app.view.search_scroll_saved;
                app.view.search_query.clear();
                app.view.search_results.clear();
            }
            KeyCode::Enter | KeyCode::Down | KeyCode::Char('n')
                if !app.view.search_results.is_empty() =>
            {
                app.view.search_idx = (app.view.search_idx + 1) % app.view.search_results.len();
                let idx = app.view.search_results[app.view.search_idx];
                if let Some(&off) = app.view.msg_line_offsets.get(idx) {
                    app.view.scroll = off.saturating_sub(2);
                }
            }
            KeyCode::Up | KeyCode::Char('p') if !app.view.search_results.is_empty() => {
                app.view.search_idx = app
                    .view
                    .search_idx
                    .checked_sub(1)
                    .unwrap_or(app.view.search_results.len() - 1);
                let idx = app.view.search_results[app.view.search_idx];
                if let Some(&off) = app.view.msg_line_offsets.get(idx) {
                    app.view.scroll = off.saturating_sub(2);
                }
            }
            KeyCode::Backspace => {
                app.view.search_query.pop();
                update_search(app);
            }
            KeyCode::Char(c) => {
                app.view.search_query.push(c);
                update_search(app);
            }
            _ => {}
        }
        return Ok(());
    }

    // ── Input mode ────────────────────────────────────────────────────────────
    match code {
        // Submit (Enter) or newline (Shift+Enter)
        KeyCode::Enter if modifiers.contains(KeyModifiers::SHIFT) => {
            app.kbd.input.insert(app.kbd.input_cursor, '\n');
            app.kbd.input_cursor += 1;
            app.kbd.hist_idx = None;
        }
        KeyCode::Tab => {
            tab_complete(app);
        }

        KeyCode::Char('[') if app.kbd.input.is_empty() => {
            let cur = app.view.msg_focus.unwrap_or(app.view.messages.len());
            let prev = app.view.messages[..cur]
                .iter()
                .rposition(|m| matches!(m, Msg::FileOp { .. } | Msg::Thinking { .. }));
            if let Some(idx) = prev {
                app.view.msg_focus = Some(idx);
                app.view.auto_scroll = false;
                if let Some(&off) = app.view.msg_line_offsets.get(idx) {
                    app.view.scroll = off.saturating_sub(2);
                }
            }
        }
        KeyCode::Char(']') if app.kbd.input.is_empty() => {
            let start = app.view.msg_focus.map(|i| i + 1).unwrap_or(0);
            let next = app.view.messages[start..]
                .iter()
                .position(|m| matches!(m, Msg::FileOp { .. } | Msg::Thinking { .. }))
                .map(|i| start + i);
            if let Some(idx) = next {
                app.view.msg_focus = Some(idx);
                app.view.auto_scroll = false;
                if let Some(&off) = app.view.msg_line_offsets.get(idx) {
                    app.view.scroll = off.saturating_sub(2);
                }
            }
        }
        KeyCode::Esc if app.view.msg_focus.is_some() => {
            app.view.msg_focus = None;
        }

        KeyCode::Enter => {
            // Toggle collapse on focused FileOp if one is selected
            if let Some(idx) = app.view.msg_focus {
                match app.view.messages.get_mut(idx) {
                    Some(Msg::FileOp { collapsed, .. }) | Some(Msg::Thinking { collapsed, .. }) => {
                        *collapsed = !*collapsed;
                        return Ok(());
                    }
                    _ => {}
                }
            }
            let text = app.kbd.input.trim().to_string();
            if text.is_empty() {
                return Ok(());
            }
            app.view.msg_focus = None;
            app.kbd.tab_state = None;
            if !handle_slash_command(app, &text) {
                submit_prompt(app, text).await?;
            }
        }

        // Ctrl shortcuts
        KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
            if app.kbd.input.is_empty() {
                app.quit = true;
            } else {
                app.kbd.input.clear();
                app.kbd.input_cursor = 0;
                app.kbd.hist_idx = None;
            }
        }
        KeyCode::Char('d')
            if modifiers.contains(KeyModifiers::CONTROL) && app.kbd.input.is_empty() =>
        {
            app.quit = true;
        }
        KeyCode::Char('u') if modifiers.contains(KeyModifiers::CONTROL) => {
            app.kbd.input.drain(..app.kbd.input_cursor);
            app.kbd.input_cursor = 0;
            app.kbd.hist_idx = None;
        }
        KeyCode::Char('w') if modifiers.contains(KeyModifiers::CONTROL) => {
            delete_word_before(&mut app.kbd.input, &mut app.kbd.input_cursor);
            app.kbd.hist_idx = None;
        }
        // Ctrl+V (all platforms) or Cmd+V (macOS) — image first, then text
        KeyCode::Char('v')
            if modifiers.contains(KeyModifiers::CONTROL)
                || (cfg!(target_os = "macos") && modifiers.contains(KeyModifiers::SUPER)) =>
        {
            if app.images_available
                && let Some(img) = crate::image::grab_clipboard_image()
            {
                app.kbd.pending_images.push(img);
                app.kbd.last_image_fp = None;
                return Ok(());
            }
            if let Some(text) = crate::image::read_clipboard_text()
                && !text.is_empty()
            {
                let normalized = text.replace('\r', "\n");
                app.kbd.input.insert_str(app.kbd.input_cursor, &normalized);
                app.kbd.input_cursor += normalized.len();
                app.kbd.hist_idx = None;
                app.kbd.tab_state = None;
            }
        }

        // Ctrl+R — activate conversation search
        KeyCode::Char('r') if modifiers.contains(KeyModifiers::CONTROL) => {
            app.view.search_scroll_saved = app.view.scroll;
            app.view.search_query.clear();
            app.view.search_results.clear();
            app.view.search_idx = 0;
            app.mode = Mode::Search;
        }

        // Ctrl+M — toggle mouse capture (scroll mode ↔ select mode)
        KeyCode::Char('m') if modifiers.contains(KeyModifiers::CONTROL) => {
            app.view.mouse_capture = !app.view.mouse_capture;
            set_mouse_capture(app.view.mouse_capture);
        }

        // Editing
        KeyCode::Backspace if app.kbd.input_cursor > 0 => {
            let len = prev_char_len(&app.kbd.input, app.kbd.input_cursor);
            let start = app.kbd.input_cursor - len;
            app.kbd.input.drain(start..app.kbd.input_cursor);
            app.kbd.input_cursor = start;
            app.kbd.hist_idx = None;
            app.kbd.tab_state = None;
        }
        KeyCode::Delete if app.kbd.input_cursor < app.kbd.input.len() => {
            let len = next_char_len(&app.kbd.input, app.kbd.input_cursor);
            app.kbd
                .input
                .drain(app.kbd.input_cursor..app.kbd.input_cursor + len);
            app.kbd.hist_idx = None;
            app.kbd.tab_state = None;
        }

        // Cursor movement
        KeyCode::Left => move_cursor_left(&app.kbd.input.clone(), &mut app.kbd.input_cursor),
        KeyCode::Right => move_cursor_right(&app.kbd.input.clone(), &mut app.kbd.input_cursor),
        KeyCode::Home => app.kbd.input_cursor = 0,
        KeyCode::End => app.kbd.input_cursor = app.kbd.input.len(),

        // History navigation
        KeyCode::Up if !app.kbd.input_history.is_empty() => {
            let new_idx = match app.kbd.hist_idx {
                None => {
                    app.kbd.hist_saved = app.kbd.input.clone();
                    app.kbd.input_history.len() - 1
                }
                Some(0) => 0,
                Some(i) => i - 1,
            };
            app.kbd.hist_idx = Some(new_idx);
            app.kbd.input = app.kbd.input_history[new_idx].clone();
            app.kbd.input_cursor = app.kbd.input.len();
        }
        KeyCode::Down => match app.kbd.hist_idx {
            None => {}
            Some(i) if i + 1 >= app.kbd.input_history.len() => {
                app.kbd.hist_idx = None;
                app.kbd.input = std::mem::take(&mut app.kbd.hist_saved);
                app.kbd.input_cursor = app.kbd.input.len();
            }
            Some(i) => {
                app.kbd.hist_idx = Some(i + 1);
                app.kbd.input = app.kbd.input_history[i + 1].clone();
                app.kbd.input_cursor = app.kbd.input.len();
            }
        },

        // Scroll
        KeyCode::PageUp => {
            app.view.auto_scroll = false;
            app.view.scroll = app.view.scroll.saturating_sub(10);
        }
        KeyCode::PageDown => {
            app.view.scroll = app.view.scroll.saturating_add(10);
            if app.view.scroll >= app.view.total_lines {
                app.view.auto_scroll = true;
            }
        }

        // j/k vim-style scroll when input is empty
        KeyCode::Char('j') if app.kbd.input.is_empty() => {
            app.view.scroll = app.view.scroll.saturating_add(3);
            if app.view.scroll >= app.view.total_lines {
                app.view.auto_scroll = true;
                app.live.unread_count = 0;
            } else {
                app.view.auto_scroll = false;
            }
        }
        KeyCode::Char('k') if app.kbd.input.is_empty() => {
            app.view.auto_scroll = false;
            app.view.scroll = app.view.scroll.saturating_sub(3);
        }

        // Printable characters
        KeyCode::Char(c) => {
            let s = c.to_string();
            app.kbd.input.insert_str(app.kbd.input_cursor, &s);
            app.kbd.input_cursor += s.len();
            app.kbd.hist_idx = None;
            app.view.msg_focus = None;
            app.kbd.tab_state = None;
        }

        _ => {}
    }
    Ok(())
}
