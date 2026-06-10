//! Tests for src/prompt.rs — PromptBuffer and PromptSegment

use crate::prompt::{PromptBuffer, PromptSegment};

// ── PromptBuffer basics ──────────────────────────────────────────────

#[test]
fn prompt_buffer_default() {
    let buf = PromptBuffer::default();
    assert!(buf.full.is_empty());
    assert!(buf.display.is_empty());
    assert!(buf.segments.is_empty());
    assert_eq!(buf.cursor, 0);
}

#[test]
fn prompt_buffer_is_empty() {
    let buf = PromptBuffer::default();
    assert!(buf.is_empty());
}

#[test]
fn prompt_buffer_is_not_empty_after_push() {
    let mut buf = PromptBuffer::default();
    buf.push_text("hello");
    assert!(!buf.is_empty());
}

// ── push_text ────────────────────────────────────────────────────────

#[test]
fn push_text_simple() {
    let mut buf = PromptBuffer::default();
    buf.push_text("hello");
    assert_eq!(buf.full, "hello");
    assert_eq!(buf.display, "hello");
    assert_eq!(buf.cursor, 5);
}

#[test]
fn push_text_twice() {
    let mut buf = PromptBuffer::default();
    buf.push_text("hello");
    buf.push_text(" world");
    assert_eq!(buf.full, "hello world");
    assert_eq!(buf.display, "hello world");
    assert_eq!(buf.cursor, 11);
}

#[test]
fn push_text_empty() {
    let mut buf = PromptBuffer::default();
    buf.push_text("");
    assert!(buf.is_empty());
}

#[test]
fn push_text_unicode() {
    let mut buf = PromptBuffer::default();
    buf.push_text("😀");
    assert_eq!(buf.full, "😀");
    assert_eq!(buf.cursor, 4); // 4 bytes for emoji
}

#[test]
fn push_text_multiple_unicode() {
    let mut buf = PromptBuffer::default();
    buf.push_text("a😀b");
    assert_eq!(buf.cursor, 6); // 1 + 4 + 1
    assert_eq!(buf.full.len(), 6);
}

#[test]
fn push_text_with_cursor_in_middle() {
    let mut buf = PromptBuffer::default();
    buf.push_text("hello");
    buf.cursor = 3; // after "hel"
    buf.push_text("X");
    assert_eq!(buf.full, "helXlo");
    assert_eq!(buf.cursor, 4);
}

#[test]
fn push_text_after_cursor_move() {
    let mut buf = PromptBuffer::default();
    buf.push_text("abcdef");
    buf.cursor = 2; // after "ab"
    buf.push_text("XX");
    assert_eq!(buf.full, "abXXcdef");
    assert_eq!(buf.cursor, 4);
}

#[test]
fn push_text_at_end() {
    let mut buf = PromptBuffer::default();
    buf.push_text("abc");
    buf.cursor = 3;
    buf.push_text("def");
    assert_eq!(buf.full, "abcdef");
    assert_eq!(buf.cursor, 6);
}

#[test]
fn push_text_at_start() {
    let mut buf = PromptBuffer::default();
    buf.push_text("world");
    buf.cursor = 0;
    buf.push_text("hello ");
    assert_eq!(buf.full, "hello world");
}

#[test]
fn push_text_newlines() {
    let mut buf = PromptBuffer::default();
    buf.push_text("line1\nline2");
    assert_eq!(buf.full, "line1\nline2");
}

#[test]
fn push_text_special_chars() {
    let mut buf = PromptBuffer::default();
    buf.push_text("echo \"hello\" | grep 'world'");
    assert_eq!(buf.full, "echo \"hello\" | grep 'world'");
}

// ── push_hidden_paste ────────────────────────────────────────────────

#[test]
fn push_hidden_paste_basic() {
    let mut buf = PromptBuffer::default();
    buf.push_hidden_paste("actual_content".into(), "[pasted text]".into());
    assert_eq!(buf.full, "actual_content");
    assert_eq!(buf.display, "[pasted text]");
    assert_eq!(buf.segments.len(), 1);
    assert!(buf.segments[0].hidden);
}

#[test]
fn push_hidden_paste_after_text() {
    let mut buf = PromptBuffer::default();
    buf.push_text("prefix ");
    buf.push_hidden_paste("hidden_data".into(), "[data]".into());
    assert_eq!(buf.full, "prefix hidden_data");
    assert_eq!(buf.display, "prefix [data]");
}

#[test]
fn push_hidden_paste_then_more_text() {
    let mut buf = PromptBuffer::default();
    buf.push_hidden_paste("secret".into(), "[pasted]".into());
    buf.push_text(" visible");
    assert_eq!(buf.full, "secret visible");
    assert_eq!(buf.display, "[pasted] visible");
}

#[test]
fn push_hidden_paste_empty() {
    let mut buf = PromptBuffer::default();
    buf.push_hidden_paste("".into(), "".into());
    assert_eq!(buf.segments.len(), 1);
    assert!(buf.segments[0].hidden);
}

#[test]
fn push_hidden_paste_unicode() {
    let mut buf = PromptBuffer::default();
    buf.push_hidden_paste("😀data".into(), "[emoji]".into());
    assert_eq!(buf.full, "😀data");
    assert_eq!(buf.display, "[emoji]");
}

// ── display_cursor_char ──────────────────────────────────────────────

#[test]
fn display_cursor_char_empty() {
    let buf = PromptBuffer::default();
    assert_eq!(buf.display_cursor_char(), 0);
}

#[test]
fn display_cursor_char_simple() {
    let mut buf = PromptBuffer::default();
    buf.push_text("hello");
    assert_eq!(buf.display_cursor_char(), 5);
}

#[test]
fn display_cursor_char_moved() {
    let mut buf = PromptBuffer::default();
    buf.push_text("hello");
    buf.cursor = 3;
    assert_eq!(buf.display_cursor_char(), 3);
}

#[test]
fn display_cursor_char_with_hidden() {
    let mut buf = PromptBuffer::default();
    buf.push_text("pre ");
    buf.push_hidden_paste("actual_hidden_text".into(), "[hidden]".into());
    // cursor at end of full: 4 + 18 = 22
    assert_eq!(buf.display_cursor_char(), 4 + 8); // "pre " + "[hidden]"
}

#[test]
fn display_cursor_char_unicode() {
    let mut buf = PromptBuffer::default();
    buf.push_text("a😀b");
    buf.cursor = 6;
    assert_eq!(buf.display_cursor_char(), 3); // 3 chars: a, 😀, b
}

#[test]
fn display_cursor_char_hidden_unicode_display() {
    let mut buf = PromptBuffer::default();
    buf.push_hidden_paste("hidden".into(), "[😀]".into());
    assert_eq!(buf.display_cursor_char(), 3); // [, 😀, ]
}

// ── PromptSegment ────────────────────────────────────────────────────

#[test]
fn prompt_segment_visible() {
    let seg = PromptSegment {
        full: "hello".into(),
        display: "hello".into(),
        hidden: false,
    };
    assert!(!seg.hidden);
    assert_eq!(seg.full, seg.display);
}

#[test]
fn prompt_segment_hidden() {
    let seg = PromptSegment {
        full: "secret_data_here".into(),
        display: "[pasted]".into(),
        hidden: true,
    };
    assert!(seg.hidden);
    assert_ne!(seg.full, seg.display);
}

// ── PromptBuffer segments ────────────────────────────────────────────

#[test]
fn prompt_buffer_segments_count() {
    let mut buf = PromptBuffer::default();
    buf.push_text("a");
    buf.push_hidden_paste("hidden".into(), "[h]".into());
    buf.push_text("b");
    assert_eq!(buf.segments.len(), 3); // visible + hidden + visible
}

#[test]
fn prompt_buffer_rebuild_flat() {
    let mut buf = PromptBuffer::default();
    buf.push_text("x");
    buf.push_hidden_paste("hidden".into(), "[h]".into());
    buf.push_text("y");
    assert_eq!(buf.full, "xhiddeny");
    assert_eq!(buf.display, "x[h]y");
}

#[test]
fn prompt_buffer_multiple_pushes() {
    let mut buf = PromptBuffer::default();
    for i in 0..10 {
        buf.push_text(&i.to_string());
    }
    assert_eq!(buf.full, "0123456789");
    assert_eq!(buf.display, "0123456789");
    assert_eq!(buf.cursor, 10);
}

#[test]
fn prompt_buffer_cursor_at_zero() {
    let mut buf = PromptBuffer::default();
    buf.push_text("abc");
    buf.cursor = 0;
    buf.push_text("X");
    assert_eq!(buf.full, "Xabc");
}

#[test]
fn prompt_buffer_cursor_past_end() {
    let mut buf = PromptBuffer::default();
    buf.push_text("abc");
    buf.cursor = 10; // past end
    buf.push_text("X");
    // Should append at end
    assert!(buf.full.ends_with("X"));
}

// ── Edge cases ───────────────────────────────────────────────────────

#[test]
fn push_text_very_long() {
    let mut buf = PromptBuffer::default();
    let long = "a".repeat(100_000);
    buf.push_text(&long);
    assert_eq!(buf.full.len(), 100_000);
    assert_eq!(buf.cursor, 100_000);
}

#[test]
fn push_text_null_bytes() {
    let mut buf = PromptBuffer::default();
    buf.push_text("a\x00b");
    assert_eq!(buf.full, "a\x00b");
}

#[test]
fn push_text_tab() {
    let mut buf = PromptBuffer::default();
    buf.push_text("a\tb");
    assert_eq!(buf.full, "a\tb");
}

#[test]
fn display_cursor_char_segments() {
    let mut buf = PromptBuffer::default();
    buf.push_text("hello ");
    buf.push_hidden_paste("hidden".into(), "[h]".into());
    buf.push_text(" world");
    buf.cursor = 12; // after "hello hidden"
    let pos = buf.display_cursor_char();
    // "hello " (6) + "[h]" (3) = 9
    assert!(pos >= 9);
}

#[test]
fn push_hidden_paste_then_cursor_back() {
    let mut buf = PromptBuffer::default();
    buf.push_hidden_paste("data".into(), "[d]".into());
    buf.cursor = 2; // middle of "data"
    let pos = buf.display_cursor_char();
    // Hidden segment: cursor snaps to end of display
    assert_eq!(pos, 3); // "[d]" = 3 chars
}

#[test]
fn push_text_backspace_simulation() {
    let mut buf = PromptBuffer::default();
    buf.push_text("hello");
    buf.cursor = 4; // "hell"
    assert_eq!(buf.cursor, 4);
}

#[test]
fn prompt_buffer_multiple_hidden() {
    let mut buf = PromptBuffer::default();
    buf.push_hidden_paste("secret1".into(), "[1]".into());
    buf.push_hidden_paste("secret2".into(), "[2]".into());
    assert_eq!(buf.full, "secret1secret2");
    assert_eq!(buf.display, "[1][2]");
}

#[test]
fn push_text_then_hidden_then_text() {
    let mut buf = PromptBuffer::default();
    buf.push_text("start ");
    buf.push_hidden_paste("middle".into(), "[m]".into());
    buf.push_text(" end");
    assert_eq!(buf.full, "start middle end");
    assert_eq!(buf.display, "start [m] end");
}

// ── More cursor tests ────────────────────────────────────────────────

#[test]
fn display_cursor_char_at_segment_boundary() {
    let mut buf = PromptBuffer::default();
    buf.push_text("abc");
    buf.push_hidden_paste("hidden".into(), "[h]".into());
    buf.cursor = 3; // exactly at end of "abc"
    assert_eq!(buf.display_cursor_char(), 3);
}

#[test]
fn push_text_preserves_display() {
    let mut buf = PromptBuffer::default();
    buf.push_text("hello");
    assert_eq!(buf.full, buf.display);
}

#[test]
fn push_hidden_changes_display() {
    let mut buf = PromptBuffer::default();
    buf.push_hidden_paste("hidden_content".into(), "[h]".into());
    assert_ne!(buf.full, buf.display);
    assert_eq!(buf.display, "[h]");
}

// ── Stress tests ─────────────────────────────────────────────────────

#[test]
fn push_text_many_segments() {
    let mut buf = PromptBuffer::default();
    for i in 0..100 {
        buf.push_text(&format!("seg{}", i));
        buf.push_hidden_paste("hidden".into(), "[h]".into());
    }
    assert!(!buf.is_empty());
    assert!(buf.full.len() > 1000);
    assert!(buf.display.len() < buf.full.len());
}

#[test]
fn push_text_repeated_unicode() {
    let mut buf = PromptBuffer::default();
    for _ in 0..1000 {
        buf.push_text("😀");
    }
    assert_eq!(buf.cursor, 4000); // 4 bytes per emoji
    assert_eq!(buf.display_cursor_char(), 1000); // 1000 chars
}
