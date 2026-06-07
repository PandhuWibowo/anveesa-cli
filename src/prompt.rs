use std::io::{self, Read, Write};

use anyhow::{Context, Result};

use crate::{
    image::{grab_clipboard_image, read_clipboard_text},
    provider::ImageAttachment,
};

#[cfg(unix)]
use libc;

pub enum PromptRead {
    Line(String, Option<ImageAttachment>),
    Interrupted,
    Eof,
}

pub struct PromptSegment {
    pub full: String,
    pub display: String,
    pub hidden: bool,
}

#[derive(Default)]
pub struct PromptBuffer {
    pub full: String,
    pub display: String,
    pub segments: Vec<PromptSegment>,
    /// Byte offset into `full` — where the next insertion goes.
    pub cursor: usize,
}

impl PromptBuffer {
    pub fn is_empty(&self) -> bool {
        self.full.is_empty()
    }

    /// Char offset in `display` that corresponds to the current cursor position in `full`.
    /// Used to position the terminal cursor after a redraw.
    pub fn display_cursor_char(&self) -> usize {
        let mut full_pos = 0usize;
        let mut disp_chars = 0usize;
        for seg in &self.segments {
            let seg_len = seg.full.len();
            let next_pos = full_pos + seg_len;
            if self.cursor <= next_pos {
                let offset = self.cursor - full_pos;
                return if seg.hidden {
                    // Hidden spans are atomic: cursor snaps to end of placeholder.
                    disp_chars + seg.display.chars().count()
                } else {
                    disp_chars + seg.full[..offset].chars().count()
                };
            }
            full_pos = next_pos;
            disp_chars += seg.display.chars().count();
        }
        disp_chars
    }

    pub fn push_text(&mut self, text: &str) {
        // Find the segment containing the cursor and insert there.
        let mut pos = 0usize;
        for seg in self.segments.iter_mut() {
            let seg_len = seg.full.len();
            if !seg.hidden && self.cursor >= pos && self.cursor <= pos + seg_len {
                let offset = self.cursor - pos;
                seg.full.insert_str(offset, text);
                seg.display.insert_str(offset, text);
                self.cursor += text.len();
                self.rebuild_flat();
                return;
            }
            pos += seg_len;
        }
        // Cursor is at end or after a hidden segment — append to last visible segment.
        if let Some(seg) = self.segments.last_mut().filter(|s| !s.hidden) {
            seg.full.push_str(text);
            seg.display.push_str(text);
        } else {
            self.segments.push(PromptSegment {
                full: text.to_string(),
                display: text.to_string(),
                hidden: false,
            });
        }
        self.cursor += text.len();
        self.rebuild_flat();
    }

    pub fn push_hidden_paste(&mut self, text: String, display: String) {
        self.full.push_str(&text);
        self.display.push_str(&display);
        self.cursor = self.full.len();
        self.segments.push(PromptSegment {
            full: text,
            display,
            hidden: true,
        });
    }

    /// Delete the character immediately before the cursor.
    /// Deletes the entire span atomically if the cursor is just past a hidden span.
    pub fn delete_before_cursor(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let mut pos = 0usize;
        let mut remove_idx: Option<usize> = None;
        for (i, seg) in self.segments.iter_mut().enumerate() {
            let seg_len = seg.full.len();
            let next_pos = pos + seg_len;
            if seg.hidden && next_pos == self.cursor {
                // cursor is right after a hidden span — delete the whole span
                self.cursor -= seg_len;
                remove_idx = Some(i);
                break;
            }
            if !seg.hidden && self.cursor > pos && self.cursor <= next_pos {
                let offset = self.cursor - pos;
                if let Some(ch) = seg.full[..offset].chars().next_back() {
                    let ch_len = ch.len_utf8();
                    seg.full.drain((offset - ch_len)..offset);
                    seg.display.drain((offset - ch_len)..offset);
                    self.cursor -= ch_len;
                    if seg.full.is_empty() {
                        remove_idx = Some(i);
                    }
                }
                break;
            }
            pos = next_pos;
        }
        if let Some(i) = remove_idx {
            self.segments.remove(i);
        }
        self.rebuild_flat();
    }

    pub fn move_left(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let mut pos = 0usize;
        for seg in &self.segments {
            let next_pos = pos + seg.full.len();
            if seg.hidden && next_pos == self.cursor {
                self.cursor = pos;
                return;
            }
            if !seg.hidden && self.cursor > pos && self.cursor <= next_pos {
                let offset = self.cursor - pos;
                if let Some(ch) = seg.full[..offset].chars().next_back() {
                    self.cursor -= ch.len_utf8();
                }
                return;
            }
            pos = next_pos;
        }
    }

    pub fn move_right(&mut self) {
        if self.cursor >= self.full.len() {
            return;
        }
        let mut pos = 0usize;
        for seg in &self.segments {
            let seg_len = seg.full.len();
            if seg.hidden && pos == self.cursor {
                self.cursor += seg_len;
                return;
            }
            if !seg.hidden && self.cursor >= pos && self.cursor < pos + seg_len {
                let offset = self.cursor - pos;
                if let Some(ch) = seg.full[offset..].chars().next() {
                    self.cursor += ch.len_utf8();
                }
                return;
            }
            pos += seg_len;
        }
    }

    pub fn move_home(&mut self) {
        self.cursor = 0;
    }

    pub fn move_end(&mut self) {
        self.cursor = self.full.len();
    }

    /// Ctrl+U / Cmd+Delete — erase the entire line.
    pub fn clear_all(&mut self) {
        self.full.clear();
        self.display.clear();
        self.segments.clear();
        self.cursor = 0;
    }

    /// Ctrl+W / Option+Delete — erase the last word before the cursor.
    pub fn pop_word(&mut self) {
        while self.cursor > 0 && self.full[..self.cursor].ends_with(' ') {
            self.delete_before_cursor();
        }
        while self.cursor > 0 && !self.full[..self.cursor].ends_with(' ') {
            self.delete_before_cursor();
        }
    }

    pub fn rebuild_flat(&mut self) {
        self.full = self.segments.iter().map(|s| s.full.as_str()).collect();
        self.display = self.segments.iter().map(|s| s.display.as_str()).collect();
    }
}

#[cfg(unix)]
struct RawPromptMode {
    fd: i32,
    saved: libc::termios,
}

#[cfg(unix)]
impl RawPromptMode {
    fn enter() -> Result<Self> {
        let fd = libc::STDIN_FILENO;
        let mut saved = std::mem::MaybeUninit::<libc::termios>::uninit();
        if unsafe { libc::tcgetattr(fd, saved.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error()).context("failed to read terminal mode");
        }

        let saved = unsafe { saved.assume_init() };
        let mut raw = saved;
        raw.c_iflag &= !(libc::BRKINT | libc::ICRNL | libc::INPCK | libc::ISTRIP | libc::IXON);
        raw.c_oflag &= !libc::OPOST;
        raw.c_cflag |= libc::CS8;
        raw.c_lflag &= !(libc::ECHO | libc::ICANON | libc::IEXTEN | libc::ISIG);
        raw.c_cc[libc::VMIN] = 1;
        raw.c_cc[libc::VTIME] = 0;

        if unsafe { libc::tcsetattr(fd, libc::TCSAFLUSH, &raw) } != 0 {
            return Err(io::Error::last_os_error()).context("failed to set terminal raw mode");
        }

        print!("\x1b[?2004h");
        let _ = io::stdout().flush();

        Ok(Self { fd, saved })
    }
}

#[cfg(unix)]
impl Drop for RawPromptMode {
    fn drop(&mut self) {
        print!("\x1b[?2004l");
        let _ = io::stdout().flush();

        unsafe {
            libc::tcsetattr(self.fd, libc::TCSAFLUSH, &self.saved);
        }
    }
}

#[cfg(not(unix))]
struct RawPromptMode;

#[cfg(not(unix))]
impl RawPromptMode {
    fn enter() -> Result<Self> {
        Ok(Self)
    }
}

/// After a redraw (which leaves the terminal cursor at end of display), move it
/// back to the buffer's logical cursor position.
fn position_prompt_cursor(display: &str, cursor_char: usize) -> io::Result<()> {
    let back = display.chars().count().saturating_sub(cursor_char);
    if back > 0 {
        print!("\x1b[{}D", back);
        io::stdout().flush()?;
    }
    Ok(())
}

pub fn read_prompt_line(
    label: &str,
    width: usize,
    paste_count: &mut usize,
    images_available: bool,
    input_history: &[String],
) -> Result<PromptRead> {
    let _raw_mode = RawPromptMode::enter()?;
    let mut input = io::stdin().lock();
    let mut buffer = PromptBuffer::default();
    let mut display_rows = 1usize;
    let mut ctrl_v_image: Option<ImageAttachment> = None;

    // History navigation state.
    let mut hist_idx: Option<usize> = None; // None = current live input
    let mut saved_input = String::new(); // stash live input when navigating into history

    // Compose the visible prompt label, optionally prefixed with an image indicator.
    let effective_label = |img: &Option<ImageAttachment>| -> String {
        if img.is_some() {
            format!("\x1b[2m[📎]\x1b[0m {label}")
        } else {
            label.to_string()
        }
    };

    // Redraw the line and position the cursor, returning the new row count.
    macro_rules! redraw {
        () => {{
            let lbl = effective_label(&ctrl_v_image);
            let rows = redraw_prompt_line(&lbl, &buffer.display, display_rows, width)?;
            let _ = position_prompt_cursor(&buffer.display, buffer.display_cursor_char());
            rows
        }};
    }

    print!("{}", effective_label(&ctrl_v_image));
    io::stdout().flush().context("failed to write prompt")?;

    loop {
        let mut byte = [0u8; 1];
        input
            .read_exact(&mut byte)
            .context("failed to read prompt input")?;

        match byte[0] {
            b'\r' | b'\n' => {
                println!();
                return Ok(PromptRead::Line(buffer.full, ctrl_v_image));
            }
            3 => {
                println!("^C");
                return Ok(PromptRead::Interrupted);
            }
            4 if buffer.is_empty() => return Ok(PromptRead::Eof),
            8 | 127 => {
                // Backspace
                buffer.delete_before_cursor();
                display_rows = redraw!();
            }
            21 => {
                // Ctrl+U / Cmd+Delete — erase entire line
                buffer.clear_all();
                display_rows = redraw!();
            }
            22 => {
                // Ctrl+V — universal paste: image first, then clipboard text
                if images_available && let Some(img) = grab_clipboard_image() {
                    ctrl_v_image = Some(img);
                    display_rows = redraw!();
                    continue;
                }
                // Fall back to clipboard text via pbpaste / xclip
                if let Some(text) = read_clipboard_text()
                    && !text.is_empty()
                {
                    buffer.push_text(&text.replace('\r', "\n"));
                    display_rows = redraw!();
                }
            }
            23 => {
                // Ctrl+W / Option+Delete — erase last word
                buffer.pop_word();
                display_rows = redraw!();
            }
            0x1b => {
                let sequence = read_escape_sequence(&mut input)?;
                match sequence.as_slice() {
                    b"[200~" => {
                        // Bracketed paste
                        let paste = normalize_pasted_text(read_bracketed_paste(&mut input)?);
                        push_paste(&mut buffer, paste, paste_count);
                        display_rows = redraw!();
                    }
                    b"[A" => {
                        // Up arrow — previous history entry
                        if input_history.is_empty() {
                            continue;
                        }
                        let new_idx = match hist_idx {
                            None => {
                                saved_input = buffer.full.clone();
                                input_history.len() - 1
                            }
                            Some(0) => 0,
                            Some(i) => i - 1,
                        };
                        hist_idx = Some(new_idx);
                        buffer = PromptBuffer::default();
                        buffer.push_text(&input_history[new_idx].clone());
                        display_rows = redraw!();
                    }
                    b"[B" => {
                        // Down arrow — next history entry / back to live input
                        match hist_idx {
                            None => {}
                            Some(i) if i + 1 >= input_history.len() => {
                                hist_idx = None;
                                let text = std::mem::take(&mut saved_input);
                                buffer = PromptBuffer::default();
                                buffer.push_text(&text);
                                display_rows = redraw!();
                            }
                            Some(i) => {
                                hist_idx = Some(i + 1);
                                buffer = PromptBuffer::default();
                                buffer.push_text(&input_history[i + 1].clone());
                                display_rows = redraw!();
                            }
                        }
                    }
                    b"[C" => {
                        // Right arrow
                        buffer.move_right();
                        let _ =
                            position_prompt_cursor(&buffer.display, buffer.display_cursor_char());
                    }
                    b"[D" => {
                        // Left arrow
                        buffer.move_left();
                        let _ =
                            position_prompt_cursor(&buffer.display, buffer.display_cursor_char());
                    }
                    b"[H" | b"[1~" => {
                        // Home
                        buffer.move_home();
                        let _ = position_prompt_cursor(&buffer.display, 0);
                    }
                    b"[F" | b"[4~" => {
                        // End
                        buffer.move_end();
                        let _ =
                            position_prompt_cursor(&buffer.display, buffer.display_cursor_char());
                    }
                    _ => {}
                }
            }
            byte if byte >= 0x20 && byte != 0x7f => {
                if let Some(ch) = read_utf8_char(byte, &mut input)? {
                    buffer.push_text(ch.encode_utf8(&mut [0; 4]));
                    display_rows = redraw!();
                }
            }
            _ => {}
        }
    }
}

pub fn push_paste(buffer: &mut PromptBuffer, text: String, paste_count: &mut usize) {
    let line_count = pasted_line_count(&text);
    if should_collapse_paste(&text) {
        *paste_count += 1;
        buffer.push_hidden_paste(
            text,
            pasted_text_display_placeholder(*paste_count, line_count),
        );
    } else {
        buffer.push_text(&text);
    }
}

fn redraw_prompt_line(
    label: &str,
    display: &str,
    previous_rows: usize,
    width: usize,
) -> Result<usize> {
    if previous_rows > 1 {
        print!("\x1b[{}A", previous_rows - 1);
    }
    print!("\r\x1b[J{label}{display}");
    io::stdout().flush().context("failed to redraw prompt")?;
    Ok(input_screen_rows(display, width, 2))
}

fn read_escape_sequence(input: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut sequence = Vec::new();
    let mut byte = [0u8; 1];

    input.read_exact(&mut byte)?;
    sequence.push(byte[0]);

    if byte[0] == b'[' {
        loop {
            input.read_exact(&mut byte)?;
            sequence.push(byte[0]);
            if (0x40..=0x7e).contains(&byte[0]) {
                break;
            }
            if sequence.len() >= 16 {
                break;
            }
        }
    }

    Ok(sequence)
}

fn read_bracketed_paste(input: &mut impl Read) -> io::Result<String> {
    const END: &[u8] = b"\x1b[201~";

    let mut bytes = Vec::new();
    let mut byte = [0u8; 1];

    loop {
        input.read_exact(&mut byte)?;
        bytes.push(byte[0]);
        if bytes.ends_with(END) {
            let new_len = bytes.len() - END.len();
            bytes.truncate(new_len);
            return Ok(String::from_utf8_lossy(&bytes).into_owned());
        }
    }
}

fn read_utf8_char(first: u8, input: &mut impl Read) -> io::Result<Option<char>> {
    let expected_len = match first {
        0x00..=0x7f => 1,
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => return Ok(None),
    };

    let mut bytes = vec![first];
    if expected_len > 1 {
        let mut rest = vec![0u8; expected_len - 1];
        input.read_exact(&mut rest)?;
        bytes.extend(rest);
    }

    Ok(std::str::from_utf8(&bytes)
        .ok()
        .and_then(|text| text.chars().next()))
}

fn normalize_pasted_text(text: String) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

pub fn should_collapse_paste(text: &str) -> bool {
    pasted_line_count(text) > 3 || text.len() > 200
}

pub fn pasted_line_count(text: &str) -> usize {
    text.lines().count().max(1)
}

pub fn pasted_text_display_placeholder(paste_count: usize, line_count: usize) -> String {
    format!("[Pasted text #{paste_count} +{line_count} lines]")
}

pub fn input_screen_rows(
    input: &str,
    terminal_width: usize,
    first_row_prefix_width: usize,
) -> usize {
    let width = terminal_width.max(1);

    input
        .split('\n')
        .enumerate()
        .map(|(index, line)| {
            let prompt_prefix_width = if index == 0 {
                first_row_prefix_width
            } else {
                0
            };
            let columns = line.chars().count() + prompt_prefix_width;
            columns.div_ceil(width).max(1)
        })
        .sum::<usize>()
        .max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pasted_input_screen_rows_accounts_for_prompt_and_wrapping() {
        assert_eq!(input_screen_rows("hello", 80, 2), 1);
        assert_eq!(input_screen_rows("one\ntwo\nthree", 80, 2), 3);
        assert_eq!(input_screen_rows(&"x".repeat(78), 80, 2), 1);
        assert_eq!(input_screen_rows(&"x".repeat(79), 80, 2), 2);
        assert_eq!(input_screen_rows("", 80, 2), 1);
    }

    #[test]
    fn pasted_text_placeholder_does_not_look_like_a_prompt() {
        let placeholder = pasted_text_display_placeholder(2, 157);

        assert!(placeholder.contains("[Pasted text #2 +157 lines]"));
        assert!(!placeholder.contains("❯"));
    }

    #[test]
    fn prompt_buffer_hidden_paste_preserves_full_text() {
        let mut buffer = PromptBuffer::default();
        let mut paste_count = 0;
        let pasted = "warning: one\nwarning: two\nwarning: three\nwarning: four".to_string();

        buffer.push_text("please read this: ");
        push_paste(&mut buffer, pasted.clone(), &mut paste_count);

        assert_eq!(buffer.full, format!("please read this: {pasted}"));
        assert_eq!(
            buffer.display,
            "please read this: [Pasted text #1 +4 lines]"
        );
    }
}
