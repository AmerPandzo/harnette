//! Images the human attaches to a ticket, a refine message or a note. They are saved next to the
//! task doc (`.ai-tasks/<slug>.assets/`) so later agents can open them with Read, and the run they
//! were attached to gets them as image blocks.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};

use crate::tasks;

/// Larger files are refused; screenshots are far below this.
pub const MAX_BYTES: usize = 20 * 1024 * 1024;
/// The API takes at most 5 MB of base64 per image block; bigger images are left for the agent to
/// Read, which resizes them.
const INLINE_BYTES: usize = 3_700_000;

fn ext(mime: &str) -> Option<&'static str> {
    match mime {
        "image/png" => Some("png"),
        "image/jpeg" | "image/jpg" => Some("jpg"),
        "image/gif" => Some("gif"),
        "image/webp" => Some("webp"),
        _ => None,
    }
}

pub fn mime_of(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

/// `.ai-tasks/<slug>.assets`, relative to the repo.
pub fn assets_dir(slug: &str) -> String {
    format!(".ai-tasks/{slug}.assets")
}

/// A path `save` could have returned: inside some task's assets folder, no `..`.
pub fn is_asset(rel: &str) -> bool {
    let mut parts = rel.split('/');
    parts.next() == Some(".ai-tasks")
        && parts.next().is_some_and(|d| d.ends_with(".assets") && tasks::valid_slug(d.trim_end_matches(".assets")))
        && parts.next().is_some_and(|f| !f.is_empty() && !f.starts_with('.'))
        && parts.next().is_none()
        && mime_of(Path::new(rel)).is_some()
}

/// An attached image as a `data:` URL, for previews.
pub fn data_url(repo: &Path, rel: &str) -> anyhow::Result<String> {
    if !is_asset(rel) {
        bail!("not an attached image: {rel}");
    }
    let (mime, data) = read_file(&repo.join(rel))?;
    Ok(format!("data:{mime};base64,{data}"))
}

/// Save one image; returns its repo-relative path.
pub fn save(repo: &Path, slug: &str, mime: &str, bytes: &[u8]) -> anyhow::Result<String> {
    if !tasks::valid_slug(slug) {
        bail!("invalid slug {slug:?}: use letters, digits, '.', '_' or '-'");
    }
    let Some(ext) = ext(mime) else { bail!("only PNG, JPEG, GIF and WebP images can be attached (got {mime})") };
    if bytes.len() > MAX_BYTES {
        bail!("the image is {} MB; the limit is {} MB", bytes.len() / 1_000_000, MAX_BYTES / 1_000_000);
    }
    let dir = assets_dir(slug);
    std::fs::create_dir_all(repo.join(&dir)).with_context(|| format!("could not create {dir}"))?;
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let rel = (0..)
        .map(|n| if n == 0 { format!("{dir}/image-{stamp}.{ext}") } else { format!("{dir}/image-{stamp}-{n}.{ext}") })
        .find(|rel| !repo.join(rel).exists())
        .expect("an unused name");
    std::fs::write(repo.join(&rel), bytes).with_context(|| format!("could not write {rel}"))?;
    Ok(rel)
}

pub fn save_base64(repo: &Path, slug: &str, mime: &str, data: &str) -> anyhow::Result<String> {
    let bytes = STANDARD.decode(data.trim()).context("the image data is not valid base64")?;
    save(repo, slug, mime, &bytes)
}

/// A local image file (dropped on the window) as `(mime, base64)`.
pub fn read_file(path: &Path) -> anyhow::Result<(String, String)> {
    let mime = mime_of(path).context("only PNG, JPEG, GIF and WebP images can be attached")?;
    let bytes = std::fs::read(path).with_context(|| format!("could not read {}", path.display()))?;
    if bytes.len() > MAX_BYTES {
        bail!("the image is {} MB; the limit is {} MB", bytes.len() / 1_000_000, MAX_BYTES / 1_000_000);
    }
    Ok((mime.to_string(), STANDARD.encode(bytes)))
}

/// Text for the agent that names the attached files, so it can link them from the task doc.
pub fn listing(images: &[String]) -> String {
    if images.is_empty() {
        return String::new();
    }
    let files: Vec<String> = images.iter().map(|p| format!("- `{p}`")).collect();
    format!(
        "\n\nThe human attached {} image{} (shown above when small enough; open any with Read):\n{}\n\
         When an image matters for the task, link it from the task doc with a relative link such as \
         `![screenshot]({})`, so the implementer and reviewer can open it too.",
        images.len(),
        if images.len() == 1 { "" } else { "s" },
        files.join("\n"),
        images[0].trim_start_matches(".ai-tasks/"),
    )
}

/// A user message's content: plain text, or image blocks followed by the text.
pub fn content(repo: &Path, text: &str, images: &[String]) -> Value {
    if images.is_empty() {
        return Value::String(text.to_string());
    }
    let mut blocks: Vec<Value> = images
        .iter()
        .filter_map(|rel| {
            let path = repo.join(rel);
            let mime = mime_of(&path)?;
            let bytes = std::fs::read(&path).ok()?;
            (bytes.len() <= INLINE_BYTES)
                .then(|| json!({ "type": "image", "source": { "type": "base64", "media_type": mime, "data": STANDARD.encode(bytes) } }))
        })
        .collect();
    blocks.push(json!({ "type": "text", "text": format!("{text}{}", listing(images)) }));
    Value::Array(blocks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_and_attaches_images() {
        let dir = std::env::temp_dir().join(format!("kormilant-attach-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let a = save(&dir, "T-1", "image/png", b"\x89PNG fake").unwrap();
        let b = save_base64(&dir, "T-1", "image/jpeg", &STANDARD.encode(b"jpeg")).unwrap();
        assert!(a.starts_with(".ai-tasks/T-1.assets/image-") && a.ends_with(".png"));
        assert!(b.ends_with(".jpg") && a != b);
        assert!(save(&dir, "T-1", "application/pdf", b"x").is_err());
        assert!(save(&dir, "../x", "image/png", b"x").is_err());

        assert!(is_asset(&a) && is_asset(&b));
        assert!(!is_asset(".ai-tasks/T-1.assets/../../x.png") && !is_asset(".ai-tasks/../x.assets/a.png") && !is_asset("src/a.png"));
        assert!(!is_asset(".ai-tasks/T-1.assets/a.txt"));
        assert!(data_url(&dir, &a).unwrap().starts_with("data:image/png;base64,"));

        assert_eq!(content(&dir, "hi", &[]), json!("hi"));
        let c = content(&dir, "hi", &[a.clone(), b]);
        let blocks = c.as_array().unwrap();
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0]["source"]["media_type"], "image/png");
        assert_eq!(blocks[0]["source"]["data"], STANDARD.encode(b"\x89PNG fake"));
        let text = blocks[2]["text"].as_str().unwrap();
        assert!(text.starts_with("hi") && text.contains(&a) && text.contains("2 images"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
