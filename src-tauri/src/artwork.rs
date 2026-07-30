//! Disk-cached artwork served over a custom `artwork://` scheme.
//!
//! Apple hands out templated URLs like `https://…/{w}x{h}bb.jpg`. The UI asks for
//! `artwork://localhost/<id>?w=300`; we resolve the template from SQLite, fetch
//! once, and keep the bytes on disk. Scrolling a library then costs no network
//! at all, which is most of why this app feels quicker than Apple's.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use tauri::{AppHandle, Manager, UriSchemeContext, UriSchemeResponder};

use crate::AppState;

pub const SCHEME: &str = "artwork";

fn cache_dir(app_data: Option<PathBuf>) -> Option<PathBuf> {
    if let Ok(p) = std::env::var("CAPSULE_ARTWORK_CACHE_PATH") {
        if !p.trim().is_empty() {
            return Some(PathBuf::from(p));
        }
    }
    Some(app_data?.join("artwork"))
}

/// Navidrome cover references are stored as `subsonic:<coverArtId>` rather than
/// a URL, because fetching one needs auth and an authenticated URL in SQLite
/// would break the promise that the database is safe to paste into a bug report.
pub fn is_subsonic_ref(template: &str) -> bool {
    template.starts_with(crate::subsonic::ARTWORK_PREFIX)
}

pub fn subsonic_id(template: &str) -> Option<&str> {
    template.strip_prefix(crate::subsonic::ARTWORK_PREFIX)
}

pub fn resolve_template(template: &str, size: u32) -> String {
    // Subsonic refs are not URLs and must not be mangled into one; the signed
    // URL is built at fetch time, where credentials are available.
    if is_subsonic_ref(template) {
        return template.to_string();
    }
    if template.contains("{w}") || template.contains("{h}") {
        return template
            .replace("{w}", &size.to_string())
            .replace("{h}", &size.to_string())
            .replace("{f}", "jpg")
            .replace("{c}", "bb");
    }
    rewrite_literal_size(template, size).unwrap_or_else(|| template.to_string())
}

fn rewrite_literal_size(url: &str, size: u32) -> Option<String> {
    let (path, query) = match url.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (url, None),
    };
    let slash = path.rfind('/')?;
    let (dir, file) = path.split_at(slash + 1);

    let bytes = file.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if !bytes[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] != b'x' {
            continue;
        }
        let x = i;
        i += 1;
        let h_start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i == h_start {
            continue;
        }
        let _ = x;
        let replaced = format!("{dir}{}{size}x{size}{}", &file[..start], &file[i..]);
        return Some(match query {
            Some(q) => format!("{replaced}?{q}"),
            None => replaced,
        });
    }
    None
}

fn cache_key(url: &str) -> String {
    let stable = match url.split_once('?') {
        None => url.to_string(),
        Some((path, query)) => {
            let kept: Vec<&str> = query
                .split('&')
                .filter(|p| !p.to_ascii_lowercase().starts_with("x-amz-"))
                .collect();
            if kept.is_empty() {
                path.to_string()
            } else {
                format!("{path}?{}", kept.join("&"))
            }
        }
    };
    let mut h = Sha256::new();
    h.update(stable.as_bytes());
    format!("{:x}", h.finalize())
}

fn content_type(url: &str) -> &'static str {
    let lower = url.to_ascii_lowercase();
    if lower.contains(".png") {
        "image/png"
    } else if lower.contains(".webp") {
        "image/webp"
    } else {
        "image/jpeg"
    }
}

pub fn parse_request(uri: &str) -> Option<(String, u32)> {
    let parsed = url::Url::parse(uri).ok()?;
    let id = parsed
        .path_segments()
        .and_then(|mut s| s.next_back())
        .filter(|s| !s.is_empty())
        .map(|s| urlencoding::decode(s).map(|c| c.into_owned()).unwrap_or_else(|_| s.to_string()))?;
    let raw = parsed
        .query_pairs()
        .find(|(k, _)| k == "w")
        .and_then(|(_, v)| v.parse::<u32>().ok())
        .unwrap_or(300);
    let size = if raw == 0 { 0 } else { raw.clamp(32, crate::db::MAX_ARTWORK) };
    Some((id, size))
}

pub fn handle(ctx: UriSchemeContext<'_, tauri::Wry>, req: http::Request<Vec<u8>>, responder: UriSchemeResponder) {
    let app = ctx.app_handle().clone();
    let uri = req.uri().to_string();

    tauri::async_runtime::spawn(async move {
        let response = serve(&app, &uri).await.unwrap_or_else(|e| {
            tracing::debug!(%uri, error = %e, "artwork miss");
            http::Response::builder()
                .status(404)
                .header("Content-Type", "text/plain")
                .body(Vec::new())
                .expect("static response builds")
        });
        responder.respond(response);
    });
}

pub fn cache_path(dir: &Path, template: &str, size: u32) -> PathBuf {
    dir.join(format!("{}-{}.img", cache_key(template), size))
}

/// The active Navidrome client, when that source is live.
///
/// Passed into [`ensure_cached`] rather than read inside it so the fetch path
/// stays independent of Tauri state and remains testable.
pub fn signer(app: &AppHandle) -> Option<std::sync::Arc<crate::subsonic::Client>> {
    app.state::<AppState>().navidrome.lock().expect("navidrome mutex").clone()
}

pub async fn ensure_cached(
    dir: &Path,
    template: &str,
    size: u32,
    signer: Option<&crate::subsonic::Client>,
) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    // Keyed on the stored template, never on the signed URL: a fresh salt per
    // request must not invalidate every cached cover.
    let path = cache_path(dir, template, size);
    if let Ok(bytes) = tokio::fs::read(&path).await {
        return Ok(bytes);
    }

    let url = match subsonic_id(template) {
        Some(id) => {
            let client = signer.ok_or("subsonic artwork requested with no active client")?;
            client.signed_url(
                "getCoverArt",
                &[("id", id.to_string()), ("size", size.to_string())],
            )
        }
        None => resolve_template(template, size),
    };
    // The error is rebuilt without the URL on purpose: a Subsonic cover URL
    // carries the account's signed token, and this failure gets logged.
    let resp = reqwest::get(&url).await.map_err(|e| {
        let what = if e.is_timeout() { "timed out" } else { "could not be fetched" };
        format!("artwork {what}")
    })?;
    if !resp.status().is_success() {
        return Err(format!("cdn {}", resp.status()).into());
    }
    let bytes = resp.bytes().await?.to_vec();

    if let Err(e) = tokio::fs::create_dir_all(dir).await {
        tracing::debug!(error = %e, "artwork cache dir");
    } else if let Err(e) = tokio::fs::write(&path, &bytes).await {
        tracing::debug!(error = %e, "artwork cache write");
    }
    Ok(bytes)
}

pub async fn prefetch(app: tauri::AppHandle, size: u32) {
    let Some(dir) = cache_dir(app.path().app_data_dir().ok()) else {
        return;
    };

    let art: Vec<crate::db::Artwork> = {
        let state = app.state::<AppState>();
        let db = state.db.lock().expect("db mutex");
        match db.all_artwork() {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(error = %e, "artwork prefetch query failed");
                return;
            }
        }
    };

    // Resolved once: Subsonic covers need a signed URL, and the client is
    // stable for the life of the prefetch.
    let nd = signer(&app);

    let total = art.len();
    let mut ok = 0usize;
    let mut bytes = 0usize;
    let mut unresizable = 0usize;
    for a in art {
        let want = a.clamp(size);
        match ensure_cached(&dir, &a.template, want, nd.as_deref()).await {
            Ok(b) => {
                ok += 1;
                bytes += b.len();
                if b.len() > 40_000 {
                    let resolved = resolve_template(&a.template, want);
                    if resolved == a.template {
                        unresizable += 1;
                        tracing::debug!(
                            kb = b.len() / 1024,
                            "artwork has no resizable size in its url; stored full-size"
                        );
                    } else {
                        tracing::warn!(
                            kb = b.len() / 1024,
                            requested = want,
                            url = %resolved,
                            "artwork resized but still large"
                        );
                    }
                }
            }
            Err(e) => tracing::debug!(error = %e, "artwork prefetch miss"),
        }
    }
    tracing::info!(
        cached = ok,
        total,
        size,
        kb = bytes / 1024,
        unresizable,
        "artwork prefetch done"
    );
}

async fn serve(
    app: &tauri::AppHandle,
    uri: &str,
) -> Result<http::Response<Vec<u8>>, Box<dyn std::error::Error + Send + Sync>> {
    let (id, size) = parse_request(uri).ok_or("malformed artwork uri")?;

    let art = {
        let state = app.state::<AppState>();
        let db = state.db.lock().expect("db mutex");
        db.artwork_for(&id)?
    }
    .ok_or("no artwork for id")?;

    let want = if size == 0 { art.best_size() } else { art.clamp(size) };
    let dir = cache_dir(app.path().app_data_dir().ok()).ok_or("no cache dir")?;
    let bytes = ensure_cached(&dir, &art.template, want, signer(app).as_deref()).await?;
    Ok(ok_response(bytes, content_type(&resolve_template(&art.template, want))))
}

fn ok_response(bytes: Vec<u8>, mime: &str) -> http::Response<Vec<u8>> {
    http::Response::builder()
        .status(200)
        .header("Content-Type", mime)
        .header("Access-Control-Allow-Origin", "*")
        .header("Cache-Control", "public, max-age=31536000, immutable")
        .body(bytes)
        .expect("response builds")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_every_placeholder_apple_uses() {
        let t = "https://is1.mzstatic.com/image/thumb/x/{w}x{h}{c}.{f}";
        let out = resolve_template(t, 300);
        assert_eq!(out, "https://is1.mzstatic.com/image/thumb/x/300x300bb.jpg");
        assert!(!out.contains('{'), "unresolved placeholder: {out}");
    }

    #[test]
    fn template_without_any_size_is_left_alone() {
        let t = "https://example.com/cover.png";
        assert_eq!(resolve_template(t, 300), t);
    }

    #[test]
    fn subsonic_refs_are_recognised_and_parsed() {
        assert!(is_subsonic_ref("subsonic:al-1"));
        assert!(!is_subsonic_ref("https://example.com/{w}x{h}bb.jpg"));
        assert_eq!(subsonic_id("subsonic:al-1"), Some("al-1"));
        assert_eq!(subsonic_id("https://example.com/a.jpg"), None);
    }

    #[test]
    fn resolve_template_leaves_subsonic_refs_alone() {
        assert_eq!(resolve_template("subsonic:al-1", 300), "subsonic:al-1");
    }

    #[test]
    fn cache_key_is_stable_for_a_subsonic_ref() {
        let dir = Path::new(r"C:\cache");
        assert_eq!(cache_path(dir, "subsonic:al-1", 300), cache_path(dir, "subsonic:al-1", 300));
        assert_ne!(cache_path(dir, "subsonic:al-1", 300), cache_path(dir, "subsonic:al-2", 300));
    }

    #[tokio::test]
    async fn subsonic_artwork_without_a_client_errors_rather_than_fetching() {
        let dir = std::env::temp_dir().join(format!("saint-art-nosign-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let res = ensure_cached(&dir, "subsonic:al-1", 56, None).await;
        assert!(res.is_err(), "must not attempt a fetch with no way to sign it");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn generated_playlist_cover_gets_its_literal_size_rewritten() {
        let t = "https://is1-ssl.mzstatic.com/image/thumb/gen/1200x1200AM.PDCXS11.jpg\
                 ?c1=9DB3F2&signature=6a9f5e93&t=abc%3D%3D&vkey=1";
        let out = resolve_template(t, 56);
        assert!(out.contains("/gen/56x56AM.PDCXS11.jpg"), "{out}");
        assert!(out.contains("signature=6a9f5e93"), "{out}");
        assert!(out.contains("c1=9DB3F2"), "{out}");
        assert!(!out.contains("1200x1200"), "{out}");
    }

    #[test]
    fn digits_in_the_query_string_are_never_rewritten() {
        let t = "https://cdn/image/gen/600x600A.jpg?signature=12x34&t=9x9";
        let out = resolve_template(t, 56);
        assert!(out.contains("/gen/56x56A.jpg"), "{out}");
        assert!(out.contains("signature=12x34"), "{out}");
        assert!(out.contains("t=9x9"), "{out}");
    }

    #[test]
    fn directory_names_with_digits_are_not_mistaken_for_a_size() {
        let t = "https://is1.mzstatic.com/image/thumb/Music116/v4/ab/cd/file.jpg/300x300bb.jpg";
        let out = resolve_template(t, 56);
        assert!(out.ends_with("/56x56bb.jpg"), "{out}");
        assert!(out.contains("Music116/v4"), "{out}");
    }

    #[test]
    fn placeholder_form_still_takes_precedence() {
        let t = "https://is1.mzstatic.com/image/thumb/x/{w}x{h}bb.{f}";
        assert_eq!(resolve_template(t, 300), "https://is1.mzstatic.com/image/thumb/x/300x300bb.jpg");
    }

    #[test]
    fn a_lone_number_without_an_x_is_not_a_size() {
        let t = "https://cdn/image/gen/1200AM.jpg";
        assert_eq!(resolve_template(t, 56), t);
    }

    #[test]
    fn parses_id_and_size() {
        let (id, size) = parse_request("artwork://localhost/l.abc123?w=600").unwrap();
        assert_eq!(id, "l.abc123");
        assert_eq!(size, 600);
    }

    #[test]
    fn size_defaults_and_clamps() {
        assert_eq!(parse_request("artwork://localhost/x").unwrap().1, 300);
        assert_eq!(parse_request("artwork://localhost/x?w=99999").unwrap().1, 3000);
        assert_eq!(parse_request("artwork://localhost/x?w=1").unwrap().1, 32);
        assert_eq!(parse_request("artwork://localhost/x?w=abc").unwrap().1, 300);
    }

    #[test]
    fn zero_is_the_sentinel_for_full_size() {
        assert_eq!(parse_request("artwork://localhost/x?w=0").unwrap().1, 0);
    }

    #[test]
    fn requests_never_exceed_the_source_resolution() {
        use crate::db::Artwork;
        let small = Artwork { template: "t".into(), max_side: Some(600) };
        assert_eq!(small.clamp(3000), 600);
        assert_eq!(small.best_size(), 600);
        assert_eq!(small.clamp(56), 56);
    }

    #[test]
    fn unknown_dimensions_fall_back_to_apples_ceiling() {
        use crate::db::{Artwork, MAX_ARTWORK};
        let unknown = Artwork { template: "t".into(), max_side: None };
        assert_eq!(unknown.best_size(), MAX_ARTWORK);
        assert_eq!(unknown.clamp(56), 56);
    }

    #[test]
    fn oversized_source_is_still_capped_at_apples_ceiling() {
        use crate::db::{Artwork, MAX_ARTWORK};
        let huge = Artwork { template: "t".into(), max_side: Some(10_000) };
        assert_eq!(huge.best_size(), MAX_ARTWORK);
    }

    #[test]
    fn handles_the_http_rewrite_tauri_uses_on_windows() {
        let (id, size) = parse_request("http://artwork.localhost/i.999?w=64").unwrap();
        assert_eq!(id, "i.999");
        assert_eq!(size, 64);
    }

    #[test]
    fn percent_encoded_ids_round_trip() {
        let (id, _) = parse_request("artwork://localhost/l.a%20b?w=300").unwrap();
        assert_eq!(id, "l.a b");
    }

    #[test]
    fn empty_path_is_rejected_rather_than_served() {
        assert!(parse_request("artwork://localhost/").is_none());
    }

    #[tokio::test]
    async fn cache_hit_returns_bytes_without_network() {
        let dir = std::env::temp_dir().join(format!("saint-art-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let template = "https://never-fetched.invalid/{w}x{h}.jpg";
        let path = cache_path(&dir, template, 56);
        std::fs::write(&path, b"cached-bytes").unwrap();

        let got = ensure_cached(&dir, template, 56, None).await.unwrap();
        assert_eq!(got, b"cached-bytes");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn unreachable_host_errors_rather_than_panicking() {
        let dir = std::env::temp_dir().join(format!("saint-art-fail-{}", std::process::id()));
        let res = ensure_cached(&dir, "https://nonexistent.invalid/{w}x{h}.jpg", 56, None).await;
        assert!(res.is_err(), "a dead CDN must surface as an error");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cache_path_separates_sizes_of_the_same_art() {
        let dir = Path::new("C:/tmp");
        let t = "https://a/{w}x{h}.jpg";
        assert_ne!(cache_path(dir, t, 56), cache_path(dir, t, 300));
    }

    #[test]
    fn cache_key_is_stable_and_distinct() {
        assert_eq!(cache_key("https://a/{w}x{h}.jpg"), cache_key("https://a/{w}x{h}.jpg"));
        assert_ne!(cache_key("https://a/1.jpg"), cache_key("https://a/2.jpg"));
    }

    #[test]
    fn presigned_urls_key_the_same_despite_a_new_signature() {
        let base = "https://store-033.blobstore.apple.com/x/y/image";
        let a = format!("{base}?X-Amz-Date=20260726T153726Z&X-Amz-Signature=aaa&X-Amz-Expires=86400");
        let b = format!("{base}?X-Amz-Date=20260727T090000Z&X-Amz-Signature=bbb&X-Amz-Expires=86400");
        assert_eq!(cache_key(&a), cache_key(&b));
        assert_eq!(cache_key(&a), cache_key(base));
    }

    #[test]
    fn generated_covers_sharing_a_path_do_not_collide() {
        let one = "https://cdn/image/gen/1200x1200AM.jpg?c1=9DB3F2&t=aaa";
        let two = "https://cdn/image/gen/1200x1200AM.jpg?c1=F29D9D&t=bbb";
        assert_ne!(cache_key(one), cache_key(two));
    }

    #[test]
    fn presigned_case_variants_are_still_stripped() {
        let a = "https://s/x/image?X-Amz-Signature=aaa";
        let b = "https://s/x/image?x-amz-signature=bbb";
        assert_eq!(cache_key(a), cache_key(b));
    }

    #[test]
    fn content_type_follows_extension() {
        assert_eq!(content_type("https://a/x.png"), "image/png");
        assert_eq!(content_type("https://a/x.webp"), "image/webp");
        assert_eq!(content_type("https://a/x.jpg"), "image/jpeg");
    }

    #[test]
    fn env_override_wins_for_cache_dir() {
        std::env::set_var("CAPSULE_ARTWORK_CACHE_PATH", r"C:\tmp\art");
        let d = cache_dir(Some(PathBuf::from(r"C:\appdata")));
        std::env::remove_var("CAPSULE_ARTWORK_CACHE_PATH");
        assert_eq!(d, Some(PathBuf::from(r"C:\tmp\art")));
    }
}
