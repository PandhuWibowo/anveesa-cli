use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

pub(super) fn wrap_text(text: &str, width: usize) -> Vec<String> {
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

pub(super) fn format_assistant_lines(text: &str, width: usize) -> Vec<Line<'static>> {
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

pub(super) fn prev_char_len(s: &str, pos: usize) -> usize {
    s[..pos].chars().next_back().map(|c| c.len_utf8()).unwrap_or(0)
}

pub(super) fn next_char_len(s: &str, pos: usize) -> usize {
    s[pos..].chars().next().map(|c| c.len_utf8()).unwrap_or(0)
}

pub(super) fn move_cursor_left(s: &str, pos: &mut usize) {
    *pos = pos.saturating_sub(prev_char_len(s, *pos));
}

pub(super) fn move_cursor_right(s: &str, pos: &mut usize) {
    *pos = (*pos + next_char_len(s, *pos)).min(s.len());
}

pub(super) fn delete_word_before(s: &mut String, pos: &mut usize) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_text_fits_width() {
        assert_eq!(wrap_text("hello world", 80), vec!["hello world"]);
    }

    #[test]
    fn wrap_text_splits_at_word_boundary() {
        assert_eq!(wrap_text("hello world", 6), vec!["hello", "world"]);
    }

    #[test]
    fn wrap_text_preserves_empty_lines() {
        assert_eq!(wrap_text("a\n\nb", 80), vec!["a", "", "b"]);
    }

    #[test]
    fn wrap_text_zero_width_passthrough() {
        assert_eq!(wrap_text("hello world", 0), vec!["hello world"]);
    }

    #[test]
    fn prev_next_char_len_ascii() {
        assert_eq!(prev_char_len("hello", 5), 1);
        assert_eq!(next_char_len("hello", 0), 1);
        assert_eq!(prev_char_len("hello", 1), 1);
        assert_eq!(next_char_len("hello", 4), 1);
    }

    #[test]
    fn prev_next_char_len_boundary() {
        assert_eq!(prev_char_len("hi", 0), 0);
        assert_eq!(next_char_len("hi", 2), 0);
    }
}
