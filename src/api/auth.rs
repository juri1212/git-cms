use crate::{
    auth::{self, CurrentUser, UserCredential},
    error::AppError,
    AppState,
};
use axum::{
    extract::{Query, State},
    response::Redirect,
    Json,
};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;
#[derive(Deserialize)]
pub struct Callback {
    code: String,
    state: String,
}
pub async fn start(State(state): State<Arc<AppState>>) -> Result<Redirect, AppError> {
    let nonce = Uuid::new_v4().to_string();
    state.oauth_states.insert(nonce.clone(), ());
    Ok(Redirect::temporary(
        &state.github.authorization_url(&nonce)?,
    ))
}
pub async fn callback(
    State(state): State<Arc<AppState>>,
    Query(q): Query<Callback>,
) -> Result<Redirect, AppError> {
    if state.oauth_states.remove(&q.state).is_none() {
        return Err(AppError::bad_request(
            "invalid_oauth_state",
            "OAuth state is invalid or expired",
        ));
    }
    let token = state.github.exchange_code(&q.code).await?;
    let gh_user = state.github.current_user(&token).await?;
    if !state.github.allowed(&token, &gh_user).await? {
        return Err(AppError::new(
            axum::http::StatusCode::FORBIDDEN,
            "access_denied",
            "user is not allowed by CMS policy",
        ));
    }
    state.github.verify_repo_access(&token).await?;
    let credential = UserCredential {
        token,
        login: gh_user.login,
        github_user_id: gh_user.id,
    };
    let jwt = auth::issue(&state.config, &credential)?;
    state
        .credentials
        .insert(credential.github_user_id.to_string(), credential);
    let mut target = url::Url::parse(&state.config.server.frontend_callback_url)
        .map_err(|_| AppError::internal("frontend_callback_url is invalid"))?;
    target.query_pairs_mut().append_pair("token", &jwt);
    Ok(Redirect::temporary(target.as_str()))
}
pub async fn me(
    State(state): State<Arc<AppState>>,
    CurrentUser(user): CurrentUser,
) -> Json<serde_json::Value> {
    Json(
        json!({"login":user.login, "github_user_id":user.github_user_id, "repository": state.config.repo_slug()}),
    )
}
