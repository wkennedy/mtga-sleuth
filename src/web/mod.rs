use std::sync::Arc;

use anyhow::{Context, Result};
use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use rust_embed::RustEmbed;
use tower_http::trace::TraceLayer;

use crate::state::AppState;

pub mod api;
pub mod import;
mod sse;
pub mod wildcards;

#[derive(RustEmbed)]
#[folder = "web/static/"]
struct Assets;

pub async fn serve(bind: String, state: Arc<AppState>) -> Result<()> {
    let app = Router::new()
        .route("/api/health", get(api::health))
        .route("/api/live", get(api::live))
        .route("/api/decks", get(api::list_decks).post(api::create_deck))
        .route("/api/decks/analyze", post(wildcards::analyze_pasted))
        .route("/api/decks/{id}", get(api::get_deck))
        .route("/api/matches", get(api::list_matches))
        .route("/api/matches/{id}", get(api::get_match))
        .route("/api/collection", get(api::collection))
        .route("/api/collection/import", post(import::import))
        .route("/api/wallet", get(api::wallet))
        .route("/api/drafts", get(api::list_drafts))
        .route("/api/drafts/{id}", get(api::get_draft))
        .route("/api/cards/{id}", get(api::get_card))
        .route("/api/events", get(api::recent_events))
        .route("/api/sse", get(sse::stream))
        .route("/", get(serve_index))
        .fallback(static_handler)
        .with_state(state)
        .layer(TraceLayer::new_for_http());

    let listener = tokio::net::TcpListener::bind(&bind).await.with_context(|| format!("binding {bind}"))?;
    tracing::info!(addr = %bind, "web server listening; open http://{bind} in your browser");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn serve_index() -> Response {
    asset_response("index.html")
}

async fn static_handler(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if path.is_empty() {
        return asset_response("index.html");
    }
    asset_response(path)
}

fn asset_response(path: &str) -> Response {
    match Assets::get(path) {
        Some(content) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            ([(header::CONTENT_TYPE, mime.as_ref())], content.data.into_owned()).into_response()
        }
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}
