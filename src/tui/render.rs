use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};

use super::format::{content_hash, format_assistant_lines, wrap_text};
use super::{App, Mode, Msg};

pub(super) fn render(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let input_lines = app.kbd.input.lines().count().max(1);
    let input_height = (input_lines as u16).clamp(1, 5) + 2;

    let status_height = if app.mode == Mode::Confirming {
        let diff_rows = app
            .confirm
            .as_ref()
            .map(|c| c.diff.len().min(20) as u16)
            .unwrap_or(0);
        1 + diff_rows
    } else {
        1
    };

    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(input_height),
        Constraint::Length(status_height),
    ])
    .split(area);

    render_header(frame, chunks[0], app);
    render_messages(frame, chunks[1], app);
    render_input(frame, chunks[2], app);
    render_status(frame, chunks[3], app);
}

pub(super) fn set_mouse_capture(enabled: bool) {
    if enabled {
        let _ = crossterm::execute!(std::io::stdout(), EnableMouseCapture);
    } else {
        let _ = crossterm::execute!(std::io::stdout(), DisableMouseCapture);
    }
}

pub(super) fn write_to_clipboard(text: &str) -> bool {
    // macOS
    if cfg!(target_os = "macos")
        && let Ok(mut child) = std::process::Command::new("pbcopy")
            .stdin(std::process::Stdio::piped())
            .spawn()
    {
        use std::io::Write;
        if let Some(stdin) = child.stdin.as_mut() {
            let _ = stdin.write_all(text.as_bytes());
        }
        return child.wait().map(|s| s.success()).unwrap_or(false);
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

pub(super) fn send_desktop_notification(title: &str, body: &str) {
    #[cfg(target_os = "macos")]
    {
        let script = format!(
            "display notification \"{}\" with title \"{}\"",
            body.replace('"', "'"),
            title.replace('"', "'")
        );
        let _ = std::process::Command::new("osascript")
            .args(["-e", &script])
            .spawn();
    }
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("notify-send")
            .args([title, body])
            .spawn();
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (title, body);
    }
}

/// Returns (input_$/M, output_$/M, cache_read_$/M, cache_write_$/M).
fn model_pricing(model: &str) -> (f64, f64, f64, f64) {
    let m = model.to_lowercase();
    if m.contains("claude") {
        if m.contains("opus") {
            (15.0, 75.0, 1.5, 18.75)
        } else if m.contains("sonnet") {
            (3.0, 15.0, 0.3, 3.75)
        } else if m.contains("haiku") {
            if m.contains("3-5") || m.contains("3.5") {
                (0.25, 1.25, 0.03, 0.30)
            } else {
                (0.80, 4.0, 0.08, 1.0)
            }
        } else {
            (3.0, 15.0, 0.3, 3.75)
        }
    } else if m.contains("gpt-4o-mini") {
        (0.15, 0.60, 0.075, 0.0)
    } else if m.contains("gpt-4o") {
        (2.50, 10.0, 1.25, 0.0)
    } else if m.contains("gpt-4-turbo") || m.contains("gpt-4-1106") {
        (10.0, 30.0, 0.0, 0.0)
    } else if m.contains("gpt-3.5") {
        (0.50, 1.50, 0.0, 0.0)
    } else if m.contains("gemini-1.5-flash") {
        (0.075, 0.30, 0.0, 0.0)
    } else if m.contains("gemini") {
        (1.25, 5.0, 0.0, 0.0)
    } else {
        (1.0, 3.0, 0.0, 0.0)
    }
}

fn context_window_tokens(model: &str) -> usize {
    let m = model.to_lowercase();
    if m.contains("gemini") {
        1_000_000
    } else if m.contains("claude") {
        200_000
    } else if m.contains("gpt-4") {
        128_000
    } else if m.contains("gpt-3.5") {
        16_000
    } else {
        128_000
    }
}

fn render_header(frame: &mut Frame, area: Rect, app: &App) {
    let version = env!("CARGO_PKG_VERSION");

    // Token info
    let token_str = if app.mode == Mode::Streaming && !app.live.streaming_buf.is_empty() {
        let live = app.live.streaming_buf.len() / 4;
        format!(" → {live}t")
    } else if app.usage.total_tokens > 0 {
        format!(
            " {}↓ {}↑",
            app.usage.prompt_tokens, app.usage.completion_tokens
        )
    } else {
        String::new()
    };

    // Context usage bar
    let ctx_tokens: usize = app
        .conv
        .history
        .iter()
        .map(|m| m.content.len() / 4 + 4)
        .sum::<usize>()
        + 2000;
    let ctx_max = context_window_tokens(&app.model);
    let pct = (ctx_tokens * 100 / ctx_max.max(1)).min(100);
    let bar_len = 8usize;
    let filled = (pct * bar_len / 100).min(bar_len);
    let bar = format!(
        "[{}{}] {}k",
        "█".repeat(filled),
        "░".repeat(bar_len - filled),
        ctx_tokens / 1000
    );
    let bar_color = if pct > 80 {
        Color::Rgb(224, 108, 117)
    } else if pct > 50 {
        Color::Rgb(229, 192, 123)
    } else {
        Color::Rgb(152, 195, 121)
    };

    let cost_str = if app.session_cost_usd > 0.0 {
        if app.session_cost_usd < 0.001 {
            " <$0.001".to_string()
        } else if app.session_cost_usd < 1.0 {
            format!(" ~${:.3}", app.session_cost_usd)
        } else {
            format!(" ~${:.2}", app.session_cost_usd)
        }
    } else {
        String::new()
    };

    // Cache hit rate
    let cache_str = if app.usage.prompt_tokens > 0 && app.usage.cache_read_tokens > 0 {
        let rate = app.usage.cache_read_tokens * 100 / app.usage.prompt_tokens.max(1);
        format!(" ⚡{rate}%")
    } else {
        String::new()
    };

    // Show actual model used when routing switched to fast_model
    let model_display = match &app.last_model_used {
        Some(m) if m != &app.model => {
            let short: String = m
                .split(['/', '-'])
                .next_back()
                .unwrap_or(m)
                .chars()
                .take(12)
                .collect();
            format!(" {} · {}→{short}", app.provider, app.model)
        }
        _ => format!(" {} · {} ", app.provider, app.model),
    };

    let left = format!(" anveesa v{version}{token_str}{cost_str}{cache_str}");
    let mid = format!("  {bar}  ");
    let right = model_display;
    let gap = (area.width as usize)
        .saturating_sub(left.chars().count() + mid.chars().count() + right.chars().count());

    let line = ratatui::text::Line::from(vec![
        Span::styled(left, Style::default().fg(Color::Rgb(20, 20, 30))),
        Span::styled(mid, Style::default().fg(bar_color)),
        Span::styled(" ".repeat(gap), Style::default()),
        Span::styled(right, Style::default().fg(Color::Rgb(20, 20, 30))),
    ]);
    frame.render_widget(
        Paragraph::new(line).style(Style::default().bg(Color::Rgb(97, 175, 239))),
        area,
    );
}

fn render_messages(frame: &mut Frame, area: Rect, app: &mut App) {
    let width = area.width.saturating_sub(4) as usize;
    let visible = area.height as usize;
    let msg_count = app.view.messages.len();

    let mut cache = std::mem::take(&mut app.view.render_cache);
    if cache.len() < msg_count {
        cache.resize_with(msg_count, || None);
    }
    if cache.len() > msg_count {
        cache.truncate(msg_count); // messages shrank (/clear)
    }

    // Pass 1: refresh stale cache entries (only changed messages re-format)
    // and compute line offsets from cached lengths — no per-frame clones.
    let mut msg_offsets: Vec<usize> = Vec::with_capacity(msg_count);
    let mut running = 1usize; // leading blank line
    for (i, msg) in app.view.messages.iter().enumerate() {
        let focused = app.view.msg_focus == Some(i);
        let h = message_hash(msg, width, focused, &app.model);
        let fresh = cache[i].as_ref().is_some_and(|(eh, _)| *eh == h);
        if !fresh {
            cache[i] = Some((h, format_message(msg, width, focused, &app.model)));
        }
        msg_offsets.push(running);
        running += cache[i].as_ref().map(|(_, l)| l.len()).unwrap_or(0);
    }

    // Dynamic tail — pending tool spinners, live streaming text, padding.
    // Small and rebuilt every frame.
    let mut tail: Vec<Line<'static>> = Vec::new();
    if !app.live.pending_tools.is_empty() {
        let dots = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        let dot = dots[app.spinner_frame % dots.len()];
        for tool in &app.live.pending_tools {
            let elapsed = tool.started_at.elapsed().as_secs_f32();
            let elapsed_str = if elapsed < 0.5 {
                String::new()
            } else {
                format!(" ({:.1}s)", elapsed)
            };
            tail.push(Line::from(vec![Span::styled(
                format!("  {dot} {}{}", tool.summary, elapsed_str),
                Style::default().fg(Color::Rgb(180, 140, 60)),
            )]));
        }
        tail.push(Line::from(""));
    }

    // In-progress streaming — assistant message being built token by token
    if !app.live.streaming_buf.is_empty()
        || (app.mode == Mode::Streaming && app.live.pending_tools.is_empty())
    {
        tail.push(assistant_header(&app.model));
        if !app.live.streaming_buf.is_empty() {
            for l in format_assistant_lines(&app.live.streaming_buf, width) {
                tail.push(l);
            }
        } else {
            let dots = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
            let dot = dots[app.spinner_frame % dots.len()];
            let elapsed = app
                .live
                .streaming_started_at
                .map(|t| t.elapsed().as_secs_f32())
                .unwrap_or(0.0);
            let elapsed_str = if elapsed < 0.5 {
                String::new()
            } else {
                format!(" ({:.1}s)", elapsed)
            };
            let status = if app.live.tool_status.is_empty() {
                "Thinking"
            } else {
                app.live.tool_status.as_str()
            };
            tail.push(Line::from(Span::styled(
                format!("    {dot} {status}{elapsed_str}"),
                Style::default().fg(Color::Rgb(180, 140, 60)),
            )));
        }
        tail.push(Line::from(""));
    }

    // Bottom padding so the last lines are never cut off by the viewport
    for _ in 0..3 {
        tail.push(Line::from(""));
    }

    let total = running + tail.len();
    app.view.total_lines = total;
    app.view.viewport_height = visible;
    app.view.msg_line_offsets = msg_offsets;

    let scroll = if app.view.auto_scroll || app.view.scroll == usize::MAX {
        // Auto-scroll: use logical line count, not visual-row estimate.
        total.saturating_sub(visible)
    } else {
        app.view.scroll.min(total.saturating_sub(1))
    };
    app.view.scroll = scroll;
    let end = scroll + visible;

    // Materialize ONLY the visible window — clone at most `visible` lines,
    // regardless of transcript length.
    let mut widget_lines: Vec<Line<'static>> = Vec::with_capacity(visible.min(total));
    if scroll == 0 && visible > 0 {
        widget_lines.push(Line::from("")); // the leading blank line
    }
    let mut pos = 1usize;
    for entry in cache.iter().take(msg_count) {
        let Some((_, ml)) = entry else { continue };
        let len = ml.len();
        if pos + len <= scroll {
            pos += len;
            continue;
        }
        if pos >= end {
            break;
        }
        let start = scroll.saturating_sub(pos);
        let stop = len.min(end - pos);
        widget_lines.extend(ml[start..stop].iter().cloned());
        pos += len;
    }
    for (j, l) in tail.iter().enumerate() {
        let p = running + j;
        if p >= scroll && p < end {
            widget_lines.push(l.clone());
        }
    }

    app.view.render_cache = cache;

    // "↓ N lines" badge on the bottom viewport row when scrolled away.
    // Computed from the actual line count below the viewport — a counter
    // bumped per token both inflated wildly and showed when nothing was
    // actually below the fold.
    let below = total.saturating_sub(scroll + visible);
    if !app.view.auto_scroll && below > 0 && visible > 0 {
        let badge = format!(" ↓ {below} lines ");
        if widget_lines.len() >= visible {
            widget_lines.truncate(visible - 1);
        }
        widget_lines.push(Line::from(Span::styled(
            badge,
            Style::default()
                .fg(Color::Black)
                .bg(Color::Rgb(97, 175, 239))
                .add_modifier(Modifier::BOLD),
        )));
    }

    frame.render_widget(Paragraph::new(widget_lines), area);
}

/// Cache key for a message's formatted lines — covers everything that can
/// change the output: content, render width, focus, collapse state, model.
fn message_hash(msg: &Msg, width: usize, focused: bool, model: &str) -> u64 {
    let key = match msg {
        Msg::User { text } => format!("u:{width}:{text}"),
        Msg::Assistant { text } => format!("a:{width}:{model}:{text}"),
        Msg::Tool {
            done,
            ok,
            text,
            elapsed_ms,
        } => format!("t:{done}:{ok}:{elapsed_ms:?}:{text}"),
        Msg::FileOp {
            verb,
            path,
            added,
            removed,
            diff,
            collapsed,
        } => {
            // diff contents are immutable; a length fingerprint is enough.
            let fp: usize = diff.iter().map(|(a, l)| l.len() + *a as usize).sum();
            format!(
                "f:{width}:{focused}:{collapsed}:{verb}:{path}:{added}:{removed}:{}:{fp}",
                diff.len()
            )
        }
        Msg::Thinking { text, collapsed } => {
            format!("k:{width}:{focused}:{collapsed}:{text}")
        }
        Msg::Error(e) => format!("e:{e}"),
        Msg::System(s) => format!("s:{s}"),
        Msg::Separator => format!("sep:{width}"),
    };
    content_hash(&key)
}

/// Format one committed message into its display lines. Pure — the result is
/// cached under `message_hash` and reused until the message changes.
fn format_message(msg: &Msg, width: usize, focused: bool, model: &str) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    match msg {
        Msg::User { text } => {
            lines.push(user_header());
            for l in wrap_text(text, width) {
                lines.push(Line::from(format!("    {l}")));
            }
            lines.push(Line::from(""));
        }
        Msg::Assistant { text } => {
            lines.push(assistant_header(model));
            for l in format_assistant_lines(text, width) {
                lines.push(l);
            }
            lines.push(Line::from(""));
        }
        Msg::Tool {
            done,
            ok,
            text,
            elapsed_ms,
        } => {
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
        Msg::FileOp {
            verb,
            path,
            added,
            removed,
            diff,
            collapsed,
        } => {
            let focus_icon = if focused { "►" } else { " " };
            let header_bg = if focused {
                Color::Rgb(25, 25, 50)
            } else {
                Color::Reset
            };
            let toggle_hint = if *collapsed {
                format!("  [▶ {} lines]", diff.len())
            } else if diff.len() > 8 {
                "  [▼ collapse]".to_string()
            } else {
                String::new()
            };
            lines.push(Line::from(vec![
                Span::styled(
                    format!(" {focus_icon}📄 "),
                    Style::default().fg(Color::Rgb(229, 192, 123)).bg(header_bg),
                ),
                Span::styled(
                    format!("{verb} "),
                    Style::default().fg(Color::White).bg(header_bg),
                ),
                Span::styled(
                    path.clone(),
                    Style::default().fg(Color::Rgb(97, 175, 239)).bg(header_bg),
                ),
                Span::styled(
                    format!("  +{added}"),
                    Style::default().fg(Color::Rgb(152, 195, 121)).bg(header_bg),
                ),
                Span::styled(
                    format!(" -{removed}"),
                    Style::default().fg(Color::Rgb(224, 108, 117)).bg(header_bg),
                ),
                Span::styled(
                    toggle_hint,
                    Style::default().fg(Color::Rgb(80, 80, 100)).bg(header_bg),
                ),
            ]));
            if !collapsed {
                for (is_add, line) in diff.iter().take(40) {
                    let (prefix, color) = if *is_add {
                        ("  + ", Color::Rgb(152, 195, 121))
                    } else {
                        ("  - ", Color::Rgb(224, 108, 117))
                    };
                    let bg = if *is_add {
                        Color::Rgb(20, 35, 20)
                    } else {
                        Color::Rgb(35, 20, 20)
                    };
                    lines.push(Line::from(Span::styled(
                        format!(
                            "{prefix}{}",
                            &line
                                .trim_end()
                                .chars()
                                .take(width.saturating_sub(6))
                                .collect::<String>()
                        ),
                        Style::default().fg(color).bg(bg),
                    )));
                }
                if diff.len() > 40 {
                    lines.push(Line::from(Span::styled(
                        format!("  … {} more lines", diff.len() - 40),
                        Style::default().fg(Color::DarkGray),
                    )));
                }
            }
            lines.push(Line::from(""));
        }
        Msg::Thinking { text, collapsed } => {
            let focus_icon = if focused { "►" } else { " " };
            let header_bg = if focused {
                Color::Rgb(25, 25, 50)
            } else {
                Color::Reset
            };
            let word_count = text.split_whitespace().count();
            if *collapsed {
                lines.push(Line::from(vec![
                    Span::styled(
                        format!(" {focus_icon}🤔 "),
                        Style::default().fg(Color::Rgb(180, 140, 60)).bg(header_bg),
                    ),
                    Span::styled(
                        format!("thinking  [{word_count} words]"),
                        Style::default().fg(Color::DarkGray).bg(header_bg),
                    ),
                ]));
            } else {
                lines.push(Line::from(vec![
                    Span::styled(
                        format!(" {focus_icon}🤔 "),
                        Style::default().fg(Color::Rgb(180, 140, 60)).bg(header_bg),
                    ),
                    Span::styled(
                        "thinking [▼ collapse]",
                        Style::default().fg(Color::Rgb(80, 80, 100)).bg(header_bg),
                    ),
                ]));
                for line in text.lines().take(50) {
                    let w = width.saturating_sub(6);
                    let trunc: String = line.chars().take(w).collect();
                    lines.push(Line::from(Span::styled(
                        format!("    {trunc}"),
                        Style::default()
                            .fg(Color::Rgb(130, 110, 60))
                            .bg(Color::Rgb(22, 20, 12)),
                    )));
                }
                if text.lines().count() > 50 {
                    lines.push(Line::from(Span::styled(
                        "    …",
                        Style::default().fg(Color::DarkGray),
                    )));
                }
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
            let line_width = width.saturating_sub(2);
            lines.push(Line::from(Span::styled(
                format!("  {}", "─".repeat(line_width.min(60))),
                Style::default().fg(Color::Rgb(45, 45, 65)),
            )));
            lines.push(Line::from(""));
        }
    }
    lines
}

fn user_header() -> Line<'static> {
    Line::from(Span::styled(
        "  ● You",
        Style::default()
            .fg(Color::Rgb(97, 175, 239))
            .add_modifier(Modifier::BOLD),
    ))
}

fn assistant_header(model: &str) -> Line<'static> {
    Line::from(Span::styled(
        format!("  ● {model}"),
        Style::default()
            .fg(Color::Rgb(152, 195, 121))
            .add_modifier(Modifier::BOLD),
    ))
}

fn render_input(frame: &mut Frame, area: Rect, app: &App) {
    // Border color reflects mode: ready=green, streaming=yellow, confirming=orange
    let border_color = match app.mode {
        Mode::Input | Mode::Search => Color::Rgb(152, 195, 121), // green — "your turn"
        Mode::Streaming => Color::Rgb(229, 192, 123),            // yellow — "thinking"
        Mode::Confirming => Color::Rgb(224, 108, 117),           // red — "needs decision"
    };
    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(border_color));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if app.mode != Mode::Input && app.mode != Mode::Search {
        // Don't show cursor or text while AI is working
        return;
    }

    if app.kbd.input.is_empty() && app.kbd.pending_images.is_empty() {
        frame.render_widget(
            Paragraph::new("  ❯ Ask anything…  (↑/↓ history · ⌘V paste image)")
                .style(Style::default().fg(Color::Rgb(60, 60, 80))),
            inner,
        );
        frame.set_cursor_position((inner.x + 4, inner.y));
        return;
    }

    let label = match app.kbd.pending_images.len() {
        0 => "  ❯ ".to_string(),
        1 => "  [📎] ❯ ".to_string(),
        n => format!("  [📎 ×{n}] ❯ "),
    };
    let label_w = label.chars().count();
    let display = format!("{label}{}", app.kbd.input);

    frame.render_widget(
        Paragraph::new(display)
            .style(Style::default().fg(Color::White))
            .wrap(Wrap { trim: false }),
        inner,
    );

    let cursor_chars = label_w + app.kbd.input[..app.kbd.input_cursor].chars().count();
    let w = inner.width.max(1) as usize;
    frame.set_cursor_position((
        inner.x + (cursor_chars % w) as u16,
        inner.y + (cursor_chars / w) as u16,
    ));
}

fn render_status(frame: &mut Frame, area: Rect, app: &App) {
    match app.mode {
        Mode::Confirming => {
            let summary = app
                .confirm
                .as_ref()
                .map(|c| c.summary.clone())
                .unwrap_or_default();
            let diff = app
                .confirm
                .as_ref()
                .map(|c| c.diff.clone())
                .unwrap_or_default();
            let w = area.width as usize;
            let mut lines: Vec<Line<'static>> = Vec::new();
            for (is_add, line_text) in diff.iter().take(20) {
                let (prefix, fg, bg) = if *is_add {
                    ("+ ", Color::Rgb(152, 195, 121), Color::Rgb(20, 35, 20))
                } else {
                    ("- ", Color::Rgb(224, 108, 117), Color::Rgb(35, 20, 20))
                };
                let truncated: String = line_text
                    .trim_end()
                    .chars()
                    .take(w.saturating_sub(3))
                    .collect();
                lines.push(Line::from(Span::styled(
                    format!(" {prefix}{truncated}"),
                    Style::default().fg(fg).bg(bg),
                )));
            }
            lines.push(Line::from(Span::styled(
                format!(
                    " ⚠  {summary}   [y] once  [a] all this turn  [s] always (save)  [n] deny "
                ),
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Rgb(224, 108, 117)),
            )));
            frame.render_widget(Paragraph::new(lines), area);
        }
        Mode::Streaming => {
            let dots = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
            let dot = dots[app.spinner_frame % dots.len()];
            let elapsed = app
                .live
                .streaming_started_at
                .map(|t| t.elapsed().as_secs_f32())
                .unwrap_or(0.0);
            let state = if !app.live.tool_status.is_empty() {
                format!("{dot} {}  ({:.1}s)", app.live.tool_status, elapsed)
            } else {
                format!("{dot} Thinking…  ({:.1}s)", elapsed)
            };
            let left = format!(" {state}");
            let right = format!(" {}  Ctrl+C cancel ", app.cwd);
            let gap =
                (area.width as usize).saturating_sub(left.chars().count() + right.chars().count());
            let text = format!("{left}{}{right}", " ".repeat(gap));
            frame.render_widget(
                Paragraph::new(text).style(
                    Style::default()
                        .fg(Color::Rgb(229, 192, 123))
                        .bg(Color::Rgb(30, 28, 20)),
                ),
                area,
            );
        }
        Mode::Search => {
            let n = app.view.search_results.len();
            let pos = if n == 0 {
                String::new()
            } else {
                format!("  {}/{n}", app.view.search_idx + 1)
            };
            let left = format!(" 🔍 {}{pos}", app.view.search_query);
            let right = " ↑↓ navigate  Esc close ";
            let gap =
                (area.width as usize).saturating_sub(left.chars().count() + right.chars().count());
            let text = format!("{left}{}{right}", " ".repeat(gap));
            frame.render_widget(
                Paragraph::new(text)
                    .style(Style::default().fg(Color::White).bg(Color::Rgb(30, 20, 50))),
                area,
            );
        }
        Mode::Input => {
            let mode_icon = if app.view.mouse_capture { "⊙" } else { "⊕" };
            let mode_label = if app.view.mouse_capture {
                "scroll"
            } else {
                "select"
            };
            let left = format!(" ● Ready  {}", app.cwd);
            let right = format!(" {mode_icon} {mode_label}  /help ");
            let gap =
                (area.width as usize).saturating_sub(left.chars().count() + right.chars().count());
            let text = format!("{left}{}{right}", " ".repeat(gap));
            frame.render_widget(
                Paragraph::new(text).style(
                    Style::default()
                        .fg(Color::Rgb(152, 195, 121))
                        .bg(Color::Rgb(20, 30, 20)),
                ),
                area,
            );
        }
    }
}

// model_pricing is used by the parent tui.rs for handle_stream_event
pub(super) fn get_model_pricing(model: &str) -> (f64, f64, f64, f64) {
    model_pricing(model)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(text: &str) -> Msg {
        Msg::User {
            text: text.to_string(),
        }
    }

    #[test]
    fn format_message_user_has_header_body_blank() {
        let lines = format_message(&user("hello"), 80, false, "m");
        assert_eq!(lines.len(), 3); // header + 1 wrapped line + trailing blank
    }

    #[test]
    fn hash_changes_with_width_and_content() {
        let m = user("hello");
        let h80 = message_hash(&m, 80, false, "m");
        let h40 = message_hash(&m, 40, false, "m");
        assert_ne!(h80, h40, "resize must invalidate cached wrapping");
        let h2 = message_hash(&user("hello!"), 80, false, "m");
        assert_ne!(h80, h2);
        // stable for identical inputs
        assert_eq!(h80, message_hash(&user("hello"), 80, false, "m"));
    }

    #[test]
    fn hash_changes_when_fileop_collapse_toggles() {
        let mk = |collapsed| Msg::FileOp {
            verb: "Update".into(),
            path: "a.rs".into(),
            added: 1,
            removed: 1,
            diff: vec![(true, "x".into()); 10],
            collapsed,
        };
        let open = message_hash(&mk(false), 80, false, "m");
        let closed = message_hash(&mk(true), 80, false, "m");
        assert_ne!(open, closed);
        // and the rendered length actually differs
        assert!(
            format_message(&mk(false), 80, false, "m").len()
                > format_message(&mk(true), 80, false, "m").len()
        );
    }

    #[test]
    fn hash_changes_with_focus() {
        let m = Msg::Thinking {
            text: "hmm".into(),
            collapsed: true,
        };
        assert_ne!(
            message_hash(&m, 80, true, "m"),
            message_hash(&m, 80, false, "m")
        );
    }
}
