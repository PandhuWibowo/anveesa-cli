//! Tests for src/image.rs

use crate::image::{image_fingerprint, image_mime_for_path, parse_attach_command, unquote_path};
use crate::provider::ImageAttachment;
use std::path::Path;

// ── image_fingerprint ────────────────────────────────────────────────

#[test]
fn image_fingerprint_basic() {
    let img = ImageAttachment {
        mime: "image/png".into(),
        data: "abc123".into(),
    };
    let fp = image_fingerprint(&img);
    assert_eq!(fp, "6:abc123");
}

#[test]
fn image_fingerprint_long_data() {
    let data = "x".repeat(200);
    let img = ImageAttachment {
        mime: "image/png".into(),
        data,
    };
    let fp = image_fingerprint(&img);
    assert!(fp.starts_with("200:"));
    let parts: Vec<&str> = fp.splitn(2, ':').collect();
    assert_eq!(parts[1].len(), 64);
    assert!(fp.starts_with("200:"));
}

#[test]
fn image_fingerprint_short_data() {
    let img = ImageAttachment {
        mime: "image/png".into(),
        data: "ab".into(),
    };
    let fp = image_fingerprint(&img);
    assert_eq!(fp, "2:ab");
}

#[test]
fn image_fingerprint_empty_data() {
    let img = ImageAttachment {
        mime: "image/png".into(),
        data: "".into(),
    };
    let fp = image_fingerprint(&img);
    assert_eq!(fp, "0:");
}

#[test]
fn image_fingerprint_unicode_data() {
    let img = ImageAttachment {
        mime: "image/png".into(),
        data: "😀🎉".into(),
    };
    let fp = image_fingerprint(&img);
    assert!(fp.starts_with("8:")); // 😀🎉 = 8 bytes
}

#[test]
fn image_fingerprint_different_mime_same_data() {
    let d: String = "samebase64data".into();
    let img1 = ImageAttachment {
        mime: "image/png".into(),
        data: d.clone(),
    };
    let img2 = ImageAttachment {
        mime: "image/jpeg".into(),
        data: d,
    };
    let fp1 = image_fingerprint(&img1);
    let fp2 = image_fingerprint(&img2);
    assert_eq!(fp1, fp2); // fingerprint doesn't include mime
}

#[test]
fn image_fingerprint_prefix_is_64_chars() {
    let data = "a".repeat(100);
    let img = ImageAttachment {
        mime: "image/png".into(),
        data,
    };
    let fp = image_fingerprint(&img);
    let parts: Vec<&str> = fp.splitn(2, ':').collect();
    assert_eq!(parts[1].len(), 64);
}

#[test]
fn image_fingerprint_64_bytes_exact() {
    let data = "a".repeat(64);
    let img = ImageAttachment {
        mime: "image/png".into(),
        data,
    };
    let fp = image_fingerprint(&img);
    let parts: Vec<&str> = fp.splitn(2, ':').collect();
    assert_eq!(parts[1].len(), 64);
}

// ── parse_attach_command ─────────────────────────────────────────────

#[test]
fn parse_attach_command_slash_attach() {
    assert_eq!(parse_attach_command("/attach"), Some(None));
}

#[test]
fn parse_attach_command_slash_image() {
    assert_eq!(parse_attach_command("/image"), Some(None));
}

#[test]
fn parse_attach_command_slash_img() {
    assert_eq!(parse_attach_command("/img"), Some(None));
}

#[test]
fn parse_attach_command_with_path() {
    assert_eq!(
        parse_attach_command("/attach photo.png"),
        Some(Some("photo.png".into()))
    );
}

#[test]
fn parse_attach_command_with_quoted_path() {
    assert_eq!(
        parse_attach_command("/attach \"/path/to/image.png\""),
        Some(Some("/path/to/image.png".into()))
    );
}

#[test]
fn parse_attach_command_single_quoted() {
    assert_eq!(
        parse_attach_command("/image '/path/to/file.jpg'"),
        Some(Some("/path/to/file.jpg".into()))
    );
}

#[test]
fn parse_attach_command_not_command() {
    assert!(parse_attach_command("hello world").is_none());
}

#[test]
fn parse_attach_command_partial_match() {
    assert!(parse_attach_command("/attachments").is_none());
}

#[test]
fn parse_attach_command_no_space() {
    assert!(parse_attach_command("/attachphoto.png").is_none());
}

#[test]
fn parse_attach_command_tabs() {
    assert_eq!(
        parse_attach_command("/attach\tphoto.png"),
        Some(Some("photo.png".into()))
    );
}

#[test]
fn parse_attach_command_multiple_spaces() {
    assert_eq!(
        parse_attach_command("/attach   photo.png"),
        Some(Some("photo.png".into()))
    );
}

#[test]
fn parse_attach_command_path_only() {
    assert_eq!(parse_attach_command("/attach"), Some(None));
}

#[test]
fn parse_attach_command_empty_after_space() {
    assert_eq!(parse_attach_command("/attach "), Some(None));
}

// ── unquote_path ─────────────────────────────────────────────────────

#[test]
fn unquote_path_no_quotes() {
    assert_eq!(unquote_path("photo.png"), "photo.png");
}

#[test]
fn unquote_path_double_quotes() {
    assert_eq!(unquote_path("\"/path/to/photo.png\""), "/path/to/photo.png");
}

#[test]
fn unquote_path_single_quotes() {
    assert_eq!(unquote_path("'/path/to/photo.png'"), "/path/to/photo.png");
}

#[test]
fn unquote_path_mismatched_quotes() {
    assert_eq!(
        unquote_path("\"/path/to/photo.png'"),
        "\"/path/to/photo.png'"
    );
}

#[test]
fn unquote_path_leading_quote_only() {
    assert_eq!(unquote_path("\"path.png"), "\"path.png");
}

#[test]
fn unquote_path_trailing_quote_only() {
    assert_eq!(unquote_path("path.png\""), "path.png\"");
}

#[test]
fn unquote_path_spaces() {
    assert_eq!(unquote_path("  /path  "), "/path");
}

#[test]
fn unquote_path_empty() {
    assert_eq!(unquote_path(""), "");
}

#[test]
fn unquote_path_only_quotes() {
    assert_eq!(unquote_path("\"\""), "");
}

#[test]
fn unquote_path_single_only_quotes() {
    assert_eq!(unquote_path("''"), "");
}

#[test]
fn unquote_path_with_spaces_inside() {
    assert_eq!(unquote_path("\"my photo.png\""), "my photo.png");
}

#[test]
fn unquote_path_one_char() {
    assert_eq!(unquote_path("x"), "x");
}

#[test]
fn unquote_path_two_chars_same_quote() {
    assert_eq!(unquote_path("xx"), "xx");
}

#[test]
fn unquote_path_nested_quotes() {
    assert_eq!(unquote_path("\"a\"\"b\""), "a\"\"b"); // outer quotes removed
}

// ── image_mime_for_path ──────────────────────────────────────────────

#[test]
fn image_mime_png() {
    assert_eq!(
        image_mime_for_path(Path::new("photo.png")),
        Some("image/png")
    );
}

#[test]
fn image_mime_jpg() {
    assert_eq!(
        image_mime_for_path(Path::new("photo.jpg")),
        Some("image/jpeg")
    );
}

#[test]
fn image_mime_jpeg() {
    assert_eq!(
        image_mime_for_path(Path::new("photo.jpeg")),
        Some("image/jpeg")
    );
}

#[test]
fn image_mime_webp() {
    assert_eq!(
        image_mime_for_path(Path::new("photo.webp")),
        Some("image/webp")
    );
}

#[test]
fn image_mime_gif() {
    assert_eq!(
        image_mime_for_path(Path::new("photo.gif")),
        Some("image/gif")
    );
}

#[test]
fn image_mime_uppercase_ext() {
    assert_eq!(
        image_mime_for_path(Path::new("photo.PNG")),
        Some("image/png")
    );
}

#[test]
fn image_mime_mixed_case() {
    assert_eq!(
        image_mime_for_path(Path::new("photo.JpEg")),
        Some("image/jpeg")
    );
}

#[test]
fn image_mime_no_ext() {
    assert_eq!(image_mime_for_path(Path::new("photo")), None);
}

#[test]
fn image_mime_txt() {
    assert_eq!(image_mime_for_path(Path::new("photo.txt")), None);
}

#[test]
fn image_mime_pdf() {
    assert_eq!(image_mime_for_path(Path::new("photo.pdf")), None);
}

#[test]
fn image_mime_bmp() {
    assert_eq!(image_mime_for_path(Path::new("photo.bmp")), None);
}

#[test]
fn image_mime_tiff() {
    assert_eq!(image_mime_for_path(Path::new("photo.tiff")), None);
}

#[test]
fn image_mime_empty_path() {
    assert_eq!(image_mime_for_path(Path::new("")), None);
}

#[test]
fn image_mime_hidden_file() {
    assert_eq!(image_mime_for_path(Path::new(".DS_Store")), None);
}

#[test]
fn image_mime_nested_path() {
    assert_eq!(
        image_mime_for_path(Path::new("/Users/me/Desktop/photo.webp")),
        Some("image/webp")
    );
}

// ── Edge cases ───────────────────────────────────────────────────────

#[test]
fn parse_attach_command_with_path_and_spaces() {
    assert_eq!(
        parse_attach_command("/attach /path/to/my file.png"),
        Some(Some("/path/to/my file.png".into()))
    );
}

#[test]
fn unquote_path_with_unicode() {
    assert_eq!(unquote_path("😀.png"), "😀.png");
}

#[test]
fn image_fingerprint_max_data() {
    let data = "A".repeat(10_000);
    let img = ImageAttachment {
        mime: "image/png".into(),
        data,
    };
    let fp = image_fingerprint(&img);
    let parts: Vec<&str> = fp.splitn(2, ':').collect();
    assert_eq!(parts[0], "10000");
    assert_eq!(parts[1].len(), 64);
}

#[test]
fn image_mime_for_path_with_dot_in_name() {
    assert_eq!(
        image_mime_for_path(Path::new("v2.0.png")),
        Some("image/png")
    );
}
