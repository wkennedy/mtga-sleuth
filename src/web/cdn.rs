//! `/cdn/{*path}` — local-first asset proxy.
//!
//! Layout under `assets_dir` mirrors the URL:
//!
//! ```text
//! assets/
//!   symbols/W.svg
//!   cards/small/0000419b-...jpg
//!   cards/normal/...jpg
//!   cards/large/...jpg
//! ```
//!
//! When a requested file exists locally we stream it back with a long
//! `Cache-Control` (Scryfall content is content-addressed by UUID, so it's
//! immutable). Otherwise we 302-redirect to the equivalent Scryfall URL —
//! which means the UI works whether or not `scripts/download_assets.py` has
//! ever been run, and partial cache states "just work."

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};

use crate::state::AppState;

/// Reconstruct the upstream Scryfall URL from the local-cache path.
/// `cards/{size}/{uuid}.jpg` → `https://cards.scryfall.io/{size}/front/{a}/{b}/{uuid}.jpg`
/// `symbols/{slug}.svg`     → `https://svgs.scryfall.io/card-symbols/{slug}.svg`
///
/// Returns None for paths we don't recognise, so callers can 404 rather than
/// blindly redirect to nonsense.
fn upstream_url(path: &str) -> Option<String> {
    if let Some(rest) = path.strip_prefix("symbols/") {
        return Some(format!("https://svgs.scryfall.io/card-symbols/{rest}"));
    }
    if let Some(rest) = path.strip_prefix("cards/") {
        // rest = "small/0000419b-...jpg"
        let (size, file) = rest.split_once('/')?;
        if !matches!(size, "small" | "normal" | "large" | "png") {
            return None;
        }
        let uuid_stem = file.split('.').next()?;
        if uuid_stem.len() < 2 {
            return None;
        }
        let a = &uuid_stem[0..1];
        let b = &uuid_stem[1..2];
        return Some(format!("https://cards.scryfall.io/{size}/front/{a}/{b}/{file}"));
    }
    None
}

/// Reject paths that try to escape the assets dir. Wildcard captures can
/// contain `..` segments; reject them rather than canonicalising, which is
/// both faster and safer.
fn safe_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.split('/').any(|seg| seg == ".." || seg.is_empty())
}

pub async fn serve(
    State(state): State<Arc<AppState>>,
    Path(path): Path<String>,
) -> Response {
    if !safe_path(&path) {
        return (StatusCode::BAD_REQUEST, "invalid path").into_response();
    }

    let local = state.assets_dir.join(&path);
    if local.is_file() {
        match tokio::fs::read(&local).await {
            Ok(bytes) => {
                let mime = mime_guess::from_path(&local).first_or_octet_stream();
                let mut resp = (
                    [(header::CONTENT_TYPE, mime.as_ref())],
                    bytes,
                ).into_response();
                resp.headers_mut().insert(
                    header::CACHE_CONTROL,
                    HeaderValue::from_static("public, max-age=31536000, immutable"),
                );
                return resp;
            }
            Err(e) => {
                tracing::warn!(path = %local.display(), error = %e, "failed to read cached asset; falling through to upstream");
            }
        }
    }

    match upstream_url(&path) {
        Some(url) => Redirect::temporary(&url).into_response(),
        None => (StatusCode::NOT_FOUND, "unknown asset path").into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upstream_for_card_image() {
        assert_eq!(
            upstream_url("cards/small/0000419b-0bba-4488-8f7a-6194544ce91e.jpg").as_deref(),
            Some("https://cards.scryfall.io/small/front/0/0/0000419b-0bba-4488-8f7a-6194544ce91e.jpg"),
        );
        assert_eq!(
            upstream_url("cards/normal/cb53a29d-2de2-4874-a6f3-0fecbfa14cf2.jpg").as_deref(),
            Some("https://cards.scryfall.io/normal/front/c/b/cb53a29d-2de2-4874-a6f3-0fecbfa14cf2.jpg"),
        );
    }

    #[test]
    fn upstream_for_symbol() {
        assert_eq!(
            upstream_url("symbols/W.svg").as_deref(),
            Some("https://svgs.scryfall.io/card-symbols/W.svg"),
        );
        assert_eq!(
            upstream_url("symbols/2W.svg").as_deref(),
            Some("https://svgs.scryfall.io/card-symbols/2W.svg"),
        );
    }

    #[test]
    fn upstream_rejects_unknown_prefix() {
        assert_eq!(upstream_url("nope/foo.jpg"), None);
        assert_eq!(upstream_url("cards/wat/foo.jpg"), None);
    }

    #[test]
    fn safe_path_blocks_traversal() {
        assert!(safe_path("cards/small/abc.jpg"));
        assert!(!safe_path("../etc/passwd"));
        assert!(!safe_path("cards/../../etc"));
        assert!(!safe_path("/abs"));
        assert!(!safe_path(""));
    }
}
