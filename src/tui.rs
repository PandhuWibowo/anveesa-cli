use std::{path::PathBuf, time::Duration};

use anyhow::{Context, Result};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph, Wrap},
};
use tokio::sync::{mpsc, oneshot};

use crate::{
    cli::AskOptions,
    config::AppConfig,
    provider::{
        ApprovalDecision, ApprovalPolicy, ChatMessage, ChatRole, ImageAttachment, PromptRequest,
        StreamEvent, ToolConfirmPreview, TurnResult, Usage,
    },
};

// ── Public event type sent from render_stream → TUI ──────────────────────────

pub enum TuiEvent {
    Token(String),
    Status(String),
    ToolCall(String),
    ToolDone { summary: String, ok: bool, elapsed_ms: u128 },
    FileOp { verb: String, path: String, added: usize, removed: usize },
    Confirm { summary: String, reply: oneshot::Sender<ApprovalDecision> },
    Usage(Usage),
    PlanSet(Vec<String>),
    PlanTaskDone(usize),
}

// ── Message types stored in conversation ─────────────────────────────────────

#[derive(Debug)]
enum Msg {
    User { text: String },
    Assistant { text: String },
    Tool { icon: &'static str, text: String, ok: bool },
    System { text: String },
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

// ── App state ─────────────────────────────────────────────────────────────────

pub struct App {
    // conversation
    messages: Vec<Msg>,
    streaming_buf: String,
    tool_status: String,
    plan_tasks: Vec<String>,
    plan_done: Vec<bool>,

    // input
    input: String,
    input_cursor: usize,
    input_history: Vec<String>,
    hist_idx: Option<usize>,
    hist_saved: String,
    pending_image: Option<ImageAttachment>,
    images_available: bool,

    // display
    scroll: usize,
    auto_scroll: bool,
    total_lines: usize,

    // status
    provider: String,
    model: String,
    usage: Usage,
    cwd: String,

    // mode
    mode: Mode,
    confirm: Option<PendingConfirm>,

    // session
    history: Vec<ChatMessage>,
    session_path: Option<PathBuf>,
    pub last_saved_at: u64,

    // provider config
    pub config: AppConfig,
    pub options: AskOptions,
    pub workspace_context: Option<String>,
    pub policy: ApprovalPolicy,

    // channels
    stream_rx: mpsc::UnboundedReceiver<TuiEvent>,
    stream_tx_proto: Option<mpsc::UnboundedSender<TuiEvent>>,
    key_rx: mpsc::UnboundedReceiver<crossterm::event::Event>,

    quit: bool,
    spinner_frame: usize,
}

impl App {
    pub fn new(
        provider: String,
        model: String,
        cwd: String,
        messages: Vec<ChatMessage>,
        images_available: bool,
        session_path: Option<PathBuf>,
        last_saved_at: u64,
        input_history: Vec<String>,
        config: AppConfig,
        options: AskOptions,
        workspace_context: Option<String>,
        policy: ApprovalPolicy,
        key_rx: mpsc::UnboundedReceiver<crossterm::event::Event>,
    ) -> Self {
        let (stream_tx, stream_rx) = mpsc::unbounded_channel();
        let msgs: Vec<Msg> = messages
            .iter()
            .map(|m| match m.role {
                ChatRole::User => Msg::User { text: m.content.clone() },
                ChatRole::Assistant => Msg::Assistant { text: m.content.clone() },
            })
            .collect();

        Self {
            messages: msgs,
            streaming_buf: String::new(),
            tool_status: String::new(),
            plan_tasks: vec![],
            plan_done: vec![],

            input: String::new(),
            input_cursor: 0,
            input_history,
            hist_idx: None,
            hist_saved: String::new(),
            pending_image: None,
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

            history: messages,
            session_path,
            last_saved_at,

            config,
            options,
            workspace_context,
            policy,

            stream_rx,
            stream_tx_proto: Some(stream_tx),
            key_rx,

            quit: false,
            spinner_frame: 0,
        }
    }

    pub fn take_stream_sender(&mut self) -> Option<mpsc::UnboundedSender<TuiEvent>> {
        self.stream_tx_proto.take()
    }
}

// ── Main TUI loop ─────────────────────────────────────────────────────────────

pub async fn run(mut app: App) -> Result<Vec<ChatMessage>> {
    let mut terminal = ratatui::init();
    terminal.clear()?;
    let result = event_loop(&mut terminal, &mut app).await;
    ratatui::restore();
    result
}

async fn event_loop(
    terminal: &mut DefaultTerminal,
    app: &mut App,
) -> Result<Vec<ChatMessage>> {
    loop {
        terminal.draw(|f| render(f, app))?;

        if app.quit {
            break;
        }

        tokio::select! {
            Some(ev) = app.key_rx.recv() => {
                handle_key_event(app, ev).await?;
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

// ── Key handling ──────────────────────────────────────────────────────────────

async fn handle_key_event(
    app: &mut App,
    event: crossterm::event::Event,
) -> Result<()> {
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

    let Event::Key(KeyEvent { code, modifiers, .. }) = event else {
        return Ok(());
    };

    // Confirmation mode: only y/n/Enter/Esc
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

    // Streaming mode: only Ctrl+C
    if app.mode == Mode::Streaming {
        // Allow scrolling during stream
        match code {
            KeyCode::PageUp => { app.auto_scroll = false; app.scroll = app.scroll.saturating_sub(10); }
            KeyCode::PageDown => { app.scroll = app.scroll.saturating_add(10); if app.scroll >= app.total_lines { app.auto_scroll = true; } }
            KeyCode::Up if modifiers.contains(KeyModifiers::ALT) => { app.auto_scroll = false; app.scroll = app.scroll.saturating_sub(1); }
            KeyCode::Down if modifiers.contains(KeyModifiers::ALT) => { app.scroll = app.scroll.saturating_add(1); }
            _ => {}
        }
        return Ok(());
    }

    match code {
        KeyCode::Enter => {
            let text = app.input.trim().to_string();
            if text.is_empty() {
                return Ok(());
            }
            submit_prompt(app, text).await?;
        }

        KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
            if app.input.is_empty() {
                app.quit = true;
            } else {
                app.input.clear();
                app.input_cursor = 0;
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

        KeyCode::Char('v') if modifiers.contains(KeyModifiers::CONTROL) && app.images_available => {
            if let Some(img) = crate::grab_clipboard_image() {
                app.pending_image = Some(img);
            }
        }

        KeyCode::Backspace => {
            if app.input_cursor > 0 {
                let ch_len = prev_char_len(&app.input, app.input_cursor);
                let start = app.input_cursor - ch_len;
                app.input.drain(start..app.input_cursor);
                app.input_cursor = start;
                app.hist_idx = None;
            }
        }

        KeyCode::Delete => {
            if app.input_cursor < app.input.len() {
                let ch_len = next_char_len(&app.input, app.input_cursor);
                app.input.drain(app.input_cursor..app.input_cursor + ch_len);
                app.hist_idx = None;
            }
        }

        KeyCode::Left => move_cursor_left(&mut app.input, &mut app.input_cursor),
        KeyCode::Right => move_cursor_right(&mut app.input, &mut app.input_cursor),
        KeyCode::Home => app.input_cursor = 0,
        KeyCode::End => app.input_cursor = app.input.len(),

        KeyCode::Up => {
            if app.hist_idx.is_none() && !app.input_history.is_empty() {
                app.hist_saved = app.input.clone();
                app.hist_idx = Some(app.input_history.len() - 1);
                let text = app.input_history[app.input_history.len() - 1].clone();
                app.input = text;
                app.input_cursor = app.input.len();
            } else if let Some(i) = app.hist_idx {
                if i > 0 {
                    app.hist_idx = Some(i - 1);
                    let text = app.input_history[i - 1].clone();
                    app.input = text;
                    app.input_cursor = app.input.len();
                }
            }
        }

        KeyCode::Down => {
            if let Some(i) = app.hist_idx {
                if i + 1 < app.input_history.len() {
                    app.hist_idx = Some(i + 1);
                    let text = app.input_history[i + 1].clone();
                    app.input = text;
                    app.input_cursor = app.input.len();
                } else {
                    app.hist_idx = None;
                    app.input = std::mem::take(&mut app.hist_saved);
                    app.input_cursor = app.input.len();
                }
            }
        }

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

        KeyCode::Char(c) => {
            let s = c.to_string();
            app.input.insert_str(app.input_cursor, &s);
            app.input_cursor += s.len();
            app.hist_idx = None;
        }

        _ => {}
    }

    // Handle slash commands typed into input
    handle_slash_command(app);

    Ok(())
}

fn handle_slash_command(app: &mut App) {
    let trimmed = app.input.trim();
    match trimmed {
        "/exit" | "/quit" | ":q" => {
            app.quit = true;
        }
        "/clear" => {
            app.messages.clear();
            app.history.clear();
            app.streaming_buf.clear();
            app.usage = Usage::default();
            app.pending_image = None;
            if let Some(path) = &app.session_path {
                let _ = std::fs::remove_file(path);
            }
            app.input.clear();
            app.input_cursor = 0;
        }
        s if s.starts_with("/export") => {
            let arg = s.strip_prefix("/export").unwrap().trim();
            let path = if arg.is_empty() {
                std::path::PathBuf::from(format!("anveesa-export-{}.md", crate::unix_now()))
            } else {
                std::path::PathBuf::from(arg)
            };
            let _ = crate::export_conversation(&path, &app.history);
            app.messages.push(Msg::System {
                text: format!("Exported to {}", path.display()),
            });
            app.input.clear();
            app.input_cursor = 0;
        }
        _ => {}
    }
}

async fn submit_prompt(app: &mut App, text: String) -> Result<()> {
    // Save to input history
    if app.input_history.last().map(|s| s.as_str()) != Some(&text) {
        app.input_history.push(text.clone());
    }
    app.hist_idx = None;

    app.messages.push(Msg::User {
        text: text.clone(),
    });
    app.input.clear();
    app.input_cursor = 0;
    app.auto_scroll = true;
    app.mode = Mode::Streaming;
    app.tool_status = "Thinking".to_string();
    app.spinner_frame = 0;

    let image = app.pending_image.take();
    let provider_name = app
        .config
        .provider_name(app.options.provider.as_deref())
        .context("unknown provider")?
        .to_string();

    let (tx, rx) = mpsc::unbounded_channel::<StreamEvent>();

    // Clone what we need for the spawned task
    let config = app.config.clone();
    let options = app.options.clone();
    let history = app.history.clone();
    let workspace_context = app.workspace_context.clone();
    let policy = app.policy;
    let tui_tx = app.stream_tx_proto.clone();

    tokio::spawn(async move {
        let request = PromptRequest {
            prompt: text,
            model: options.model.clone(),
            system: options.system.clone(),
            workspace_context: workspace_context.map(|s| s.to_string()),
            history,
            image,
        };

        let result = crate::provider::ask(&config, &provider_name, request, policy, &tx).await;
        drop(tx);

        if let Some(tui_tx) = tui_tx {
            match result {
                Ok(turn) => {
                    let _ = tui_tx.send(TuiEvent::Usage(turn.usage.unwrap_or_default()));
                }
                Err(e) => {
                    // Error will be communicated via the stream events already sent
                    let _ = tui_tx.send(TuiEvent::Status(format!("Error: {e:#}")));
                }
            }
        }
    });

    // Relay StreamEvents → TuiEvents
    if let Some(tui_tx) = &app.stream_tx_proto {
        let tui_tx = tui_tx.clone();
        tokio::spawn(async move {
            let mut rx = rx;
            while let Some(ev) = rx.recv().await {
                match ev {
                    StreamEvent::Token(t) => { let _ = tui_tx.send(TuiEvent::Token(t)); }
                    StreamEvent::Status { message } => { let _ = tui_tx.send(TuiEvent::Status(message)); }
                    StreamEvent::ToolCall { summary } => { let _ = tui_tx.send(TuiEvent::ToolCall(summary)); }
                    StreamEvent::ToolResult { summary, ok, elapsed_ms, .. } => {
                        let _ = tui_tx.send(TuiEvent::ToolDone { summary, ok, elapsed_ms });
                    }
                    StreamEvent::FileOp { verb, path, added, removed, .. } => {
                        let _ = tui_tx.send(TuiEvent::FileOp { verb, path, added, removed });
                    }
                    StreamEvent::Confirm { preview, reply } => {
                        let summary = match &preview {
                            ToolConfirmPreview::FileOp { verb, path, added, removed, .. } =>
                                format!("{verb} {path}  +{added} -{removed}"),
                            ToolConfirmPreview::CreateDir { path } =>
                                format!("create dir {path}"),
                            ToolConfirmPreview::Generic { summary } =>
                                summary.clone(),
                        };
                        let _ = tui_tx.send(TuiEvent::Confirm { summary, reply });
                    }
                    StreamEvent::Usage(u) => { let _ = tui_tx.send(TuiEvent::Usage(u)); }
                    StreamEvent::PlanSet { tasks } => { let _ = tui_tx.send(TuiEvent::PlanSet(tasks)); }
                    StreamEvent::PlanTaskDone { index } => { let _ = tui_tx.send(TuiEvent::PlanTaskDone(index)); }
                }
            }
        });
    }

    Ok(())
}

async fn handle_stream_event(app: &mut App, ev: TuiEvent) {
    match ev {
        TuiEvent::Token(text) => {
            app.streaming_buf.push_str(&text);
            app.auto_scroll = true;
        }
        TuiEvent::Status(msg) => {
            app.tool_status = msg;
        }
        TuiEvent::ToolCall(summary) => {
            if !app.streaming_buf.is_empty() {
                let text = std::mem::take(&mut app.streaming_buf);
                app.messages.push(Msg::Assistant { text });
            }
            app.messages.push(Msg::Tool {
                icon: "⚙",
                text: summary,
                ok: true,
            });
            app.tool_status = "Running tool".to_string();
        }
        TuiEvent::ToolDone { summary, ok, .. } => {
            // Update the last tool message to reflect result
            if let Some(Msg::Tool { text, ok: tool_ok, .. }) = app.messages.last_mut() {
                *text = summary;
                *tool_ok = ok;
            }
            app.tool_status = "Thinking".to_string();
        }
        TuiEvent::FileOp { verb, path, added, removed } => {
            if !app.streaming_buf.is_empty() {
                let text = std::mem::take(&mut app.streaming_buf);
                app.messages.push(Msg::Assistant { text });
            }
            app.messages.push(Msg::Tool {
                icon: "📄",
                text: format!("{verb} {path}  \x1b[32m+{added}\x1b[0m \x1b[31m-{removed}\x1b[0m"),
                ok: true,
            });
        }
        TuiEvent::Confirm { summary, reply } => {
            if !app.streaming_buf.is_empty() {
                let text = std::mem::take(&mut app.streaming_buf);
                app.messages.push(Msg::Assistant { text });
            }
            app.confirm = Some(PendingConfirm { summary, reply });
            app.mode = Mode::Confirming;
        }
        TuiEvent::Usage(u) => {
            app.usage.prompt_tokens += u.prompt_tokens;
            app.usage.completion_tokens += u.completion_tokens;
            app.usage.total_tokens += u.total_tokens;
            app.usage.cache_read_tokens += u.cache_read_tokens;
            app.usage.cache_write_tokens += u.cache_write_tokens;

            // Streaming finished — commit the buffered text
            if !app.streaming_buf.is_empty() {
                let text = std::mem::take(&mut app.streaming_buf);
                let prompt = app.messages.iter().rev()
                    .find_map(|m| if let Msg::User { text } = m { Some(text.clone()) } else { None })
                    .unwrap_or_default();
                let assistant_text = text.clone();
                app.history.push(ChatMessage::user(prompt));
                app.history.push(ChatMessage::assistant(assistant_text.clone()));
                app.messages.push(Msg::Assistant { text });
                // Save session
                if let Some(path) = &app.session_path {
                    if let Ok(cwd) = std::env::current_dir() {
                        let _ = crate::save_interactive_session_pub(
                            path, &cwd, &app.provider,
                            &app.options, &app.history,
                        );
                        app.last_saved_at = crate::unix_now();
                    }
                }
            }
            app.mode = Mode::Input;
            app.tool_status.clear();
        }
        TuiEvent::PlanSet(tasks) => {
            app.plan_done = vec![false; tasks.len()];
            app.plan_tasks = tasks;
        }
        TuiEvent::PlanTaskDone(i) => {
            if i < app.plan_done.len() {
                app.plan_done[i] = true;
            }
        }
    }
}

// ── Rendering ─────────────────────────────────────────────────────────────────

fn render(frame: &mut Frame, app: &mut App) {
    let area = frame.area();

    let input_height = (app.input.len() / area.width.max(1) as usize + 1).clamp(1, 5) as u16 + 2;

    let chunks = Layout::vertical([
        Constraint::Length(1),          // header
        Constraint::Min(3),             // messages
        Constraint::Length(input_height), // input box
        Constraint::Length(1),          // status bar
    ])
    .split(area);

    render_header(frame, chunks[0], app);
    render_messages(frame, chunks[1], app);
    render_input(frame, chunks[2], app);
    render_status(frame, chunks[3], app);
}

fn render_header(frame: &mut Frame, area: Rect, app: &App) {
    let version = env!("CARGO_PKG_VERSION");
    let left = format!(" anveesa v{version}");
    let right = format!("{} · {}  ", app.provider, app.model);
    let gap = (area.width as usize)
        .saturating_sub(left.chars().count() + right.chars().count());
    let title = format!("{left}{}{right}", " ".repeat(gap));
    let p = Paragraph::new(title).style(
        Style::default()
            .fg(Color::Black)
            .bg(Color::Rgb(97, 175, 239)),
    );
    frame.render_widget(p, area);
}

fn render_messages(frame: &mut Frame, area: Rect, app: &mut App) {
    let width = area.width.saturating_sub(4) as usize;

    let mut lines: Vec<Line<'static>> = vec![Line::from("")];

    for msg in &app.messages {
        match msg {
            Msg::User { text } => {
                lines.push(Line::from(vec![
                    Span::styled("  ● You", Style::default().fg(Color::Rgb(97, 175, 239)).add_modifier(Modifier::BOLD)),
                ]));
                for l in wrap_text(text, width) {
                    lines.push(Line::from(format!("    {l}")));
                }
                lines.push(Line::from(""));
            }
            Msg::Assistant { text } => {
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("  ● {}", app.model),
                        Style::default().fg(Color::Rgb(152, 195, 121)).add_modifier(Modifier::BOLD),
                    ),
                ]));
                for l in format_assistant_lines(text, width) {
                    lines.push(l);
                }
                lines.push(Line::from(""));
            }
            Msg::Tool { icon, text, ok } => {
                let color = if *ok { Color::Rgb(229, 192, 123) } else { Color::Rgb(224, 108, 117) };
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("  {icon} {text}"),
                        Style::default().fg(color),
                    ),
                ]));
                lines.push(Line::from(""));
            }
            Msg::System { text } => {
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("  ─ {text}"),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]));
                lines.push(Line::from(""));
            }
        }
    }

    // Streaming in-progress
    if !app.streaming_buf.is_empty() || app.mode == Mode::Streaming {
        lines.push(Line::from(vec![
            Span::styled(
                format!("  ● {}", app.model),
                Style::default().fg(Color::Rgb(152, 195, 121)).add_modifier(Modifier::BOLD),
            ),
        ]));
        if !app.streaming_buf.is_empty() {
            for l in format_assistant_lines(&app.streaming_buf, width) {
                lines.push(l);
            }
        } else if app.mode == Mode::Streaming {
            let dots = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
            let dot = dots[app.spinner_frame % dots.len()];
            let status = if app.tool_status.is_empty() { "Thinking" } else { &app.tool_status };
            lines.push(Line::from(vec![
                Span::styled(
                    format!("    {dot} {status}"),
                    Style::default().fg(Color::DarkGray),
                ),
            ]));
        }
        lines.push(Line::from(""));
    }

    let total = lines.len();
    app.total_lines = total;

    let visible = area.height as usize;
    let scroll = if app.auto_scroll || app.scroll == usize::MAX {
        total.saturating_sub(visible)
    } else {
        app.scroll.min(total.saturating_sub(visible))
    };
    app.scroll = scroll;

    let text = Text::from(lines);
    let p = Paragraph::new(text)
        .scroll((scroll as u16, 0));
    frame.render_widget(p, area);
}

fn render_input(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(Color::Rgb(60, 60, 80)));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let label = if app.pending_image.is_some() { "  [📎] ❯ " } else { "  ❯ " };
    let label_width = label.chars().count();
    let display = format!("{label}{}", app.input);

    let p = Paragraph::new(display.clone())
        .style(Style::default().fg(Color::White))
        .wrap(Wrap { trim: false });
    frame.render_widget(p, inner);

    // Position cursor
    let cursor_char = label_width + app.input[..app.input_cursor].chars().count();
    let cursor_col = cursor_char % inner.width.max(1) as usize;
    let cursor_row = cursor_char / inner.width.max(1) as usize;
    frame.set_cursor_position((
        inner.x + cursor_col as u16,
        inner.y + cursor_row as u16,
    ));
}

fn render_status(frame: &mut Frame, area: Rect, app: &App) {
    let mode_str = match app.mode {
        Mode::Confirming => {
            let summary = app.confirm.as_ref().map(|c| c.summary.as_str()).unwrap_or("?");
            format!(" ⚠  Allow: {summary}  [y]es  [a]ll  [n]o ")
        }
        _ => {
            let cwd = &app.cwd;
            let tokens = if app.usage.total_tokens > 0 {
                format!("{}↓ {}↑  ", app.usage.prompt_tokens, app.usage.completion_tokens)
            } else {
                String::new()
            };
            let hints = "PageUp/Dn scroll  /help";
            let left = format!(" {tokens}{cwd}");
            let right = format!("{hints} ");
            let gap = (area.width as usize)
                .saturating_sub(left.chars().count() + right.chars().count());
            format!("{left}{}{right}", " ".repeat(gap))
        }
    };

    let style = if app.mode == Mode::Confirming {
        Style::default().fg(Color::Black).bg(Color::Rgb(229, 192, 123))
    } else {
        Style::default().fg(Color::DarkGray).bg(Color::Rgb(30, 30, 46))
    };

    frame.render_widget(Paragraph::new(mode_str).style(style), area);
}

// ── Text formatting ───────────────────────────────────────────────────────────

fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![text.to_string()];
    }
    let mut out = Vec::new();
    for line in text.lines() {
        if line.is_empty() {
            out.push(String::new());
            continue;
        }
        let mut current = String::new();
        let mut col = 0usize;
        for word in line.split_whitespace() {
            let wlen = word.chars().count();
            if col > 0 && col + 1 + wlen > width {
                out.push(current.clone());
                current.clear();
                col = 0;
            }
            if col > 0 {
                current.push(' ');
                col += 1;
            }
            current.push_str(word);
            col += wlen;
        }
        if !current.is_empty() || line.starts_with(' ') {
            out.push(current);
        }
    }
    out
}

fn format_assistant_lines(text: &str, width: usize) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut in_code = false;
    let mut code_lang = String::new();

    for raw_line in text.lines() {
        if raw_line.starts_with("```") {
            if in_code {
                in_code = false;
                code_lang.clear();
                out.push(Line::from(Span::styled(
                    "    └─────────────────────".to_string(),
                    Style::default().fg(Color::Rgb(50, 50, 70)),
                )));
            } else {
                in_code = true;
                code_lang = raw_line[3..].trim().to_string();
                let lang_display = if code_lang.is_empty() {
                    String::new()
                } else {
                    format!(" {code_lang} ")
                };
                out.push(Line::from(Span::styled(
                    format!("    ┌─{lang_display}"),
                    Style::default().fg(Color::Rgb(50, 50, 70)),
                )));
            }
            continue;
        }

        if in_code {
            let highlighted = highlight_code_line(raw_line, &code_lang);
            out.push(highlighted);
        } else {
            // Prose line — basic inline markdown
            let lines = if width > 0 && raw_line.chars().count() + 4 > width {
                wrap_text(raw_line, width.saturating_sub(4))
            } else {
                vec![raw_line.to_string()]
            };
            for l in lines {
                out.push(format_prose_line(&l));
            }
        }
    }

    out
}

fn format_prose_line(line: &str) -> Line<'static> {
    if line.is_empty() {
        return Line::from("");
    }

    // Headings
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

    // List items
    let (prefix, rest) = if line.starts_with("- ") || line.starts_with("* ") {
        ("    • ", &line[2..])
    } else if line.len() > 2 && line.chars().next().map_or(false, |c| c.is_ascii_digit()) && &line[1..3] == ". " {
        ("    ", line)
    } else {
        ("    ", line)
    };

    // Parse inline spans (bold, italic, code)
    let spans = parse_inline_spans(&format!("{prefix}{rest}"));
    Line::from(spans)
}

fn parse_inline_spans(text: &str) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut chars = text.chars().peekable();
    let mut buf = String::new();

    while let Some(c) = chars.next() {
        if c == '`' {
            // Inline code
            if !buf.is_empty() {
                spans.push(Span::raw(buf.clone()));
                buf.clear();
            }
            let mut code = String::new();
            for ch in chars.by_ref() {
                if ch == '`' { break; }
                code.push(ch);
            }
            spans.push(Span::styled(code, Style::default().fg(Color::Rgb(229, 192, 123)).bg(Color::Rgb(40, 40, 55))));
        } else if c == '*' && chars.peek() == Some(&'*') {
            // Bold
            chars.next();
            if !buf.is_empty() {
                spans.push(Span::raw(buf.clone()));
                buf.clear();
            }
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
            // Italic
            if !buf.is_empty() {
                spans.push(Span::raw(buf.clone()));
                buf.clear();
            }
            let mut italic = String::new();
            for ch in chars.by_ref() {
                if ch == '*' { break; }
                italic.push(ch);
            }
            spans.push(Span::styled(italic, Style::default().add_modifier(Modifier::ITALIC)));
        } else {
            buf.push(c);
        }
    }
    if !buf.is_empty() {
        spans.push(Span::raw(buf));
    }
    spans
}

fn highlight_code_line(line: &str, _lang: &str) -> Line<'static> {
    static KEYWORDS: &[&str] = &[
        "fn", "let", "mut", "const", "struct", "enum", "impl", "trait", "use", "pub",
        "mod", "return", "if", "else", "for", "while", "loop", "match", "async", "await",
        "self", "Self", "true", "false", "Some", "None", "Ok", "Err", "type", "where",
        "def", "class", "import", "from", "pass", "with", "as", "in", "not", "and", "or",
        "var", "let", "const", "function", "new", "this", "typeof", "instanceof",
        "int", "str", "bool", "float", "None", "True", "False",
    ];

    let mut spans = Vec::new();
    let indent = "      ";
    spans.push(Span::styled(
        indent.to_string(),
        Style::default().bg(Color::Rgb(28, 28, 40)),
    ));

    // Tokenize the line simply
    let mut chars = line.chars().peekable();
    let mut buf = String::new();
    let base_style = Style::default().fg(Color::Rgb(171, 178, 191)).bg(Color::Rgb(28, 28, 40));

    let flush_buf = |buf: &mut String, spans: &mut Vec<Span<'static>>| {
        if !buf.is_empty() {
            let s = buf.clone();
            let style = if KEYWORDS.contains(&s.as_str()) {
                Style::default().fg(Color::Rgb(198, 120, 221)).bg(Color::Rgb(28, 28, 40))
            } else {
                Style::default().fg(Color::Rgb(171, 178, 191)).bg(Color::Rgb(28, 28, 40))
            };
            spans.push(Span::styled(s, style));
            buf.clear();
        }
    };

    let mut in_string = false;
    let mut string_char = '"';
    while let Some(c) = chars.next() {
        if in_string {
            buf.push(c);
            if c == string_char && !buf.ends_with("\\\"") {
                let s = buf.clone();
                spans.push(Span::styled(s, Style::default().fg(Color::Rgb(152, 195, 121)).bg(Color::Rgb(28, 28, 40))));
                buf.clear();
                in_string = false;
            }
            continue;
        }

        // Line comment
        if c == '/' && chars.peek() == Some(&'/') {
            flush_buf(&mut buf, &mut spans);
            let rest: String = std::iter::once(c).chain(chars.by_ref()).collect();
            spans.push(Span::styled(rest, Style::default().fg(Color::Rgb(92, 99, 112)).bg(Color::Rgb(28, 28, 40))));
            break;
        }
        if c == '#' {
            flush_buf(&mut buf, &mut spans);
            let rest: String = std::iter::once(c).chain(chars.by_ref()).collect();
            spans.push(Span::styled(rest, Style::default().fg(Color::Rgb(92, 99, 112)).bg(Color::Rgb(28, 28, 40))));
            break;
        }

        // String start
        if c == '"' || c == '\'' {
            flush_buf(&mut buf, &mut spans);
            in_string = true;
            string_char = c;
            buf.push(c);
            continue;
        }

        // Numbers
        if c.is_ascii_digit() && buf.is_empty() {
            flush_buf(&mut buf, &mut spans);
            let mut num = String::new();
            num.push(c);
            while let Some(&n) = chars.peek() {
                if n.is_ascii_alphanumeric() || n == '.' || n == '_' {
                    num.push(n);
                    chars.next();
                } else {
                    break;
                }
            }
            spans.push(Span::styled(num, Style::default().fg(Color::Rgb(209, 154, 102)).bg(Color::Rgb(28, 28, 40))));
            continue;
        }

        // Word boundary
        if c.is_alphanumeric() || c == '_' {
            buf.push(c);
        } else {
            flush_buf(&mut buf, &mut spans);
            spans.push(Span::styled(c.to_string(), base_style));
        }
    }
    flush_buf(&mut buf, &mut spans);

    // Pad to fill the line visually
    let total_content_len: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    if total_content_len < 80 {
        spans.push(Span::styled(
            " ".repeat(80 - total_content_len),
            Style::default().bg(Color::Rgb(28, 28, 40)),
        ));
    }

    Line::from(spans)
}

// ── Cursor / string helpers ───────────────────────────────────────────────────

fn prev_char_len(s: &str, pos: usize) -> usize {
    s[..pos].chars().next_back().map(|c| c.len_utf8()).unwrap_or(0)
}

fn next_char_len(s: &str, pos: usize) -> usize {
    s[pos..].chars().next().map(|c| c.len_utf8()).unwrap_or(0)
}

fn move_cursor_left(s: &str, pos: &mut usize) {
    let len = prev_char_len(s, *pos);
    *pos = pos.saturating_sub(len);
}

fn move_cursor_right(s: &str, pos: &mut usize) {
    let len = next_char_len(s, *pos);
    *pos = (*pos + len).min(s.len());
}

fn delete_word_before(s: &mut String, pos: &mut usize) {
    while *pos > 0 && s[..*pos].ends_with(' ') {
        let len = prev_char_len(s, *pos);
        let start = *pos - len;
        s.drain(start..*pos);
        *pos = start;
    }
    while *pos > 0 && !s[..*pos].ends_with(' ') {
        let len = prev_char_len(s, *pos);
        let start = *pos - len;
        s.drain(start..*pos);
        *pos = start;
    }
}
