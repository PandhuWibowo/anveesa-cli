use std::{fs, path::Path};

use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};

use crate::provider::ImageAttachment;

/// Cheap fingerprint for deduplication: length + first 64 base64 chars.
pub fn image_fingerprint(img: &ImageAttachment) -> String {
    let prefix: String = img.data.chars().take(64).collect();
    format!("{}:{}", img.data.len(), prefix)
}

pub fn parse_attach_command(prompt: &str) -> Option<Option<String>> {
    for command in ["/attach", "/image", "/img"] {
        if prompt == command {
            return Some(None);
        }
        if let Some(rest) = prompt.strip_prefix(command)
            && rest.chars().next().is_some_and(char::is_whitespace)
        {
            let path = unquote_path(rest.trim());
            if !path.is_empty() {
                return Some(Some(path.to_string()));
            }
            return Some(None);
        }
    }
    None
}

pub fn unquote_path(path: &str) -> &str {
    let trimmed = path.trim();
    if trimmed.len() >= 2 {
        let bytes = trimmed.as_bytes();
        if (bytes[0] == b'"' && bytes[trimmed.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[trimmed.len() - 1] == b'\'')
        {
            return &trimmed[1..trimmed.len() - 1];
        }
    }
    trimmed
}

pub fn attach_image(path: Option<&str>) -> Result<ImageAttachment> {
    match path {
        Some(path) => load_image_file(Path::new(path)),
        None => read_clipboard_image().context(
            "no image found in clipboard — copy an image first, or for broader format support: brew install pngpaste",
        ),
    }
}

pub fn load_image_file(path: &Path) -> Result<ImageAttachment> {
    const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;

    let metadata =
        fs::metadata(path).with_context(|| format!("failed to read {}", path.display()))?;
    if !metadata.is_file() {
        bail!("{} is not a file", path.display());
    }
    if metadata.len() > MAX_IMAGE_BYTES {
        bail!(
            "{} is too large for an image attachment ({} MB max)",
            path.display(),
            MAX_IMAGE_BYTES / 1024 / 1024
        );
    }

    let mime = image_mime_for_path(path)
        .with_context(|| format!("unsupported image type for {}", path.display()))?;
    let bytes = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    if bytes.is_empty() {
        bail!("{} is empty", path.display());
    }

    Ok(ImageAttachment {
        mime: mime.to_string(),
        data: BASE64.encode(&bytes),
    })
}

pub fn image_mime_for_path(path: &Path) -> Option<&'static str> {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .as_deref()
    {
        Some("png") => Some("image/png"),
        Some("jpg") | Some("jpeg") => Some("image/jpeg"),
        Some("webp") => Some("image/webp"),
        Some("gif") => Some("image/gif"),
        _ => None,
    }
}

/// Try to grab an image from the system clipboard and return it base64-encoded.
/// Only supported on macOS; returns None on other platforms or when no image is present.
#[cfg(target_os = "macos")]
pub fn grab_clipboard_image() -> Option<ImageAttachment> {
    read_clipboard_image().ok()
}

/// Try to grab an image from the system clipboard and return it base64-encoded.
#[cfg(target_os = "macos")]
fn read_clipboard_image() -> Result<ImageAttachment> {
    // pngpaste handles all modern macOS clipboard formats (install: brew install pngpaste)
    if let Ok(bytes) = read_clipboard_via_pngpaste() {
        return Ok(ImageAttachment {
            mime: "image/png".to_string(),
            data: BASE64.encode(&bytes),
        });
    }

    // JXA via NSPasteboard: catches public.png (browsers, web apps)
    if let Ok(bytes) = read_clipboard_via_jxa("public.png") {
        return Ok(ImageAttachment {
            mime: "image/png".to_string(),
            data: BASE64.encode(&bytes),
        });
    }

    // JXA via NSPasteboard: catches public.tiff (screenshots, Preview, most macOS apps)
    if let Ok(tiff) = read_clipboard_via_jxa("public.tiff") {
        let png = convert_tiff_to_png(&tiff)?;
        return Ok(ImageAttachment {
            mime: "image/png".to_string(),
            data: BASE64.encode(&png),
        });
    }

    // Legacy AppleScript class-code fallback
    if let Ok(bytes) = read_clipboard_class_bytes("PNGf", "png") {
        return Ok(ImageAttachment {
            mime: "image/png".to_string(),
            data: BASE64.encode(&bytes),
        });
    }
    if let Ok(bytes) = read_clipboard_class_bytes("JPEG", "jpg") {
        return Ok(ImageAttachment {
            mime: "image/jpeg".to_string(),
            data: BASE64.encode(&bytes),
        });
    }
    if let Ok(tiff) = read_clipboard_class_bytes("TIFF", "tiff") {
        let png = convert_tiff_to_png(&tiff)?;
        return Ok(ImageAttachment {
            mime: "image/png".to_string(),
            data: BASE64.encode(&png),
        });
    }

    bail!("no image found in clipboard — copy an image first, or use: /attach path/to/image.png")
}

/// Read clipboard image using pngpaste (brew install pngpaste) — most reliable option.
#[cfg(target_os = "macos")]
fn read_clipboard_via_pngpaste() -> Result<Vec<u8>> {
    let tmp = std::env::temp_dir().join(format!("anveesa_pp_{}.png", std::process::id()));
    let status = std::process::Command::new("pngpaste")
        .arg(&tmp)
        .status()
        .context("pngpaste not available")?;
    if !status.success() {
        let _ = fs::remove_file(&tmp);
        bail!("pngpaste: no image in clipboard");
    }
    let bytes = fs::read(&tmp)?;
    let _ = fs::remove_file(&tmp);
    if bytes.len() < 8 {
        bail!("empty image from pngpaste");
    }
    Ok(bytes)
}

/// Read clipboard image via JXA + NSPasteboard using a modern UTI type.
/// This correctly handles images copied from browsers and web apps.
#[cfg(target_os = "macos")]
fn read_clipboard_via_jxa(pb_type: &str) -> Result<Vec<u8>> {
    let script = format!(
        "ObjC.import('AppKit'); \
         var d = $.NSPasteboard.generalPasteboard.dataForType('{pb_type}'); \
         d && d.length > 0 ? d.base64EncodedStringWithOptions(0).js : 'none'"
    );
    let out = std::process::Command::new("osascript")
        .arg("-l")
        .arg("JavaScript")
        .arg("-e")
        .arg(&script)
        .output()
        .context("osascript not available")?;
    let result = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !out.status.success() || result == "none" || result.is_empty() {
        bail!("no {pb_type} data in clipboard");
    }
    let clean: String = result.chars().filter(|c| !c.is_whitespace()).collect();
    BASE64
        .decode(clean.as_bytes())
        .context("failed to decode clipboard image data from JXA")
}

#[cfg(target_os = "macos")]
fn read_clipboard_class_bytes(class_code: &str, extension: &str) -> Result<Vec<u8>> {
    let tmp = std::env::temp_dir().join(format!(
        "anveesa_clip_{}_{}.{}",
        std::process::id(),
        class_code,
        extension
    ));
    let tmp_display = tmp.display();
    let script = format!(
        "try\n\
         set d to (the clipboard as \u{00AB}class {class_code}\u{00BB})\n\
         set f to open for access POSIX file \"{tmp_display}\" with write permission\n\
         write d to f\n\
         close access f\n\
         return \"ok\"\n\
         on error\n\
         return \"none\"\n\
         end try"
    );

    let out = std::process::Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .output()
        .context("failed to read macOS clipboard with osascript")?;

    if String::from_utf8_lossy(&out.stdout).trim() != "ok" {
        let _ = fs::remove_file(&tmp);
        bail!("clipboard does not contain {class_code} image data");
    }

    let bytes = fs::read(&tmp).with_context(|| format!("failed to read {tmp_display}"))?;
    let _ = fs::remove_file(&tmp);

    if bytes.len() < 8 {
        bail!("clipboard {class_code} image data is empty");
    }

    Ok(bytes)
}

#[cfg(target_os = "macos")]
fn convert_tiff_to_png(tiff: &[u8]) -> Result<Vec<u8>> {
    let base = std::env::temp_dir().join(format!("anveesa_clip_{}", std::process::id()));
    let tiff_path = base.with_extension("tiff");
    let png_path = base.with_extension("png");
    fs::write(&tiff_path, tiff).context("failed to write temporary TIFF clipboard image")?;

    let status = std::process::Command::new("sips")
        .arg("-s")
        .arg("format")
        .arg("png")
        .arg(&tiff_path)
        .arg("--out")
        .arg(&png_path)
        .status()
        .context("failed to convert clipboard TIFF to PNG with sips")?;

    let _ = fs::remove_file(&tiff_path);
    if !status.success() {
        let _ = fs::remove_file(&png_path);
        bail!("failed to convert clipboard TIFF image to PNG");
    }

    let bytes = fs::read(&png_path).context("failed to read converted clipboard PNG")?;
    let _ = fs::remove_file(&png_path);
    if bytes.len() < 8 {
        bail!("converted clipboard PNG is empty");
    }
    Ok(bytes)
}

#[cfg(not(target_os = "macos"))]
pub fn grab_clipboard_image() -> Option<ImageAttachment> {
    None
}

#[cfg(not(target_os = "macos"))]
fn read_clipboard_image() -> Result<ImageAttachment> {
    bail!("clipboard image attachment is only supported on macOS; use /attach path/to/image.png")
}

pub fn read_clipboard_text() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("pbpaste").output().ok()?;
        if out.status.success() {
            let text = String::from_utf8_lossy(&out.stdout).into_owned();
            if !text.is_empty() {
                return Some(text);
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    for (cmd, args) in &[
        ("wl-paste", vec!["--no-newline"]),
        ("xclip", vec!["-o", "-selection", "clipboard"]),
        ("xsel", vec!["--clipboard", "--output"]),
    ] {
        if let Ok(out) = std::process::Command::new(cmd).args(args).output() {
            if out.status.success() {
                let text = String::from_utf8_lossy(&out.stdout).into_owned();
                if !text.is_empty() {
                    return Some(text);
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_attach_commands() {
        assert_eq!(parse_attach_command("/attach"), Some(None));
        assert_eq!(
            parse_attach_command("/attach screenshot.png"),
            Some(Some("screenshot.png".into()))
        );
        assert_eq!(
            parse_attach_command("/attach \"folder/my image.jpg\""),
            Some(Some("folder/my image.jpg".into()))
        );
        assert_eq!(
            parse_attach_command("/img '/tmp/capture.webp'"),
            Some(Some("/tmp/capture.webp".into()))
        );
        assert_eq!(parse_attach_command("/attachment nope"), None);
    }

    #[test]
    fn detects_image_mime_from_path() {
        assert_eq!(image_mime_for_path(Path::new("a.png")), Some("image/png"));
        assert_eq!(image_mime_for_path(Path::new("a.JPEG")), Some("image/jpeg"));
        assert_eq!(image_mime_for_path(Path::new("a.webp")), Some("image/webp"));
        assert_eq!(image_mime_for_path(Path::new("a.txt")), None);
    }
}
