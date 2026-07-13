mod api;
mod auth;
mod config;
mod content;
mod error;
mod git_repo;
mod github;
mod sessions;

use axum::{
    http::{header, Method},
    Router,
};
use dashmap::DashMap;
use std::sync::Arc;
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tracing_subscriber::EnvFilter;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<config::Config>,
    pub github: github::GithubClient,
    pub repo: git_repo::GitRepository,
    pub credentials: Arc<DashMap<String, auth::UserCredential>>,
    pub oauth_states: Arc<DashMap<String, ()>>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();
    let config = Arc::new(config::Config::load()?);
    let github = github::GithubClient::new(config.clone())?;
    let repo = git_repo::GitRepository::new(config.clone(), github.clone()).await?;
    let state = AppState {
        config: config.clone(),
        github,
        repo,
        credentials: Arc::new(DashMap::new()),
        oauth_states: Arc::new(DashMap::new()),
    };
    let frontend_origin = url::Url::parse(&config.server.frontend_callback_url)?
        .origin()
        .ascii_serialization()
        .parse::<axum::http::HeaderValue>()?;
    let cors = CorsLayer::new()
        .allow_origin(frontend_origin)
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE]);
    let app: Router = api::router(state)
        .layer(cors)
        .layer(TraceLayer::new_for_http());
    let listener = tokio::net::TcpListener::bind(&config.server.bind).await?;
    tracing::info!(bind = %config.server.bind, "git-cms listening");
    axum::serve(listener, app).await?;
    Ok(())
}
