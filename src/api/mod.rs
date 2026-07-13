pub mod auth;
pub mod files;
pub mod health;
pub mod sessions;
use crate::AppState;
use axum::{
    routing::{get, post, put},
    Router,
};
use std::sync::Arc;
pub fn router(state: AppState) -> Router {
    let state = Arc::new(state);
    Router::new()
        .route("/healthz", get(health::health))
        .route("/auth/github/start", get(auth::start))
        .route("/auth/github/callback", get(auth::callback))
        .route("/api/me", get(auth::me))
        .route("/api/files", get(files::list))
        .route("/api/files/*path", get(files::read))
        .route("/api/sessions", get(sessions::list).post(sessions::create))
        .route("/api/sessions/:session_id", get(sessions::get))
        .route(
            "/api/sessions/:session_id/files/*path",
            put(sessions::write).delete(sessions::delete),
        )
        .route("/api/sessions/:session_id/moves", post(sessions::move_file))
        .route("/api/sessions/:session_id/ready", post(sessions::ready))
        .with_state(state)
}
