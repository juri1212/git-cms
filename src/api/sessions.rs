use crate::{
    auth::{CurrentUser, UserCredential},
    error::AppError,
    sessions::{Session, SessionMetadata},
    AppState,
};
use axum::{
    extract::{Path, State},
    Json,
};
use chrono::Utc;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;
#[derive(Deserialize)]
pub struct Create {
    title: String,
    base_branch: Option<String>,
}
#[derive(Deserialize)]
pub struct Write {
    content: String,
    expected_commit: Option<String>,
    message: String,
}
#[derive(Deserialize)]
pub struct Move {
    from: String,
    to: String,
    message: String,
}
fn parse(id: &str) -> Result<Uuid, AppError> {
    Uuid::parse_str(id)
        .map_err(|_| AppError::bad_request("invalid_session_id", "session ID must be a UUID"))
}
async fn find(state: &AppState, user: &UserCredential, id: Uuid) -> Result<Session, AppError> {
    let session = state
        .github
        .sessions(&user.token)
        .await?
        .into_iter()
        .find(|s| s.session_id == id)
        .ok_or_else(|| {
            AppError::new(
                axum::http::StatusCode::NOT_FOUND,
                "session_not_found",
                "CMS session was not found",
            )
        })?;
    let expected = format!(
        "{}{}/{id}",
        state.config.repository.branch_prefix, session.created_by
    );
    if session.branch != expected {
        return Err(AppError::new(
            axum::http::StatusCode::FORBIDDEN,
            "invalid_session_branch",
            "session branch does not match CMS convention",
        ));
    }
    if session.created_by != user.login {
        return Err(AppError::new(
            axum::http::StatusCode::FORBIDDEN,
            "session_not_owned",
            "only the session creator may edit this session",
        ));
    }
    Ok(session)
}
pub async fn create(
    State(state): State<Arc<AppState>>,
    CurrentUser(user): CurrentUser,
    Json(request): Json<Create>,
) -> Result<Json<serde_json::Value>, AppError> {
    if request.title.trim().is_empty() {
        return Err(AppError::bad_request(
            "invalid_title",
            "session title cannot be empty",
        ));
    }
    let base = request
        .base_branch
        .unwrap_or_else(|| state.config.repository.default_branch.clone());
    if base.starts_with('-') || base.contains("..") || base.contains(' ') {
        return Err(AppError::bad_request("invalid_ref", "invalid base branch"));
    }
    state.github.verify_repo_access(&user.token).await?;
    let id = Uuid::new_v4();
    let branch = format!(
        "{}{}/{id}",
        state.config.repository.branch_prefix, user.login
    );
    state
        .repo
        .create_branch(&base, &branch, &user.token)
        .await?;
    let metadata = SessionMetadata {
        session_id: id,
        created_by: user.login.clone(),
        base_branch: base.clone(),
        created_at: Utc::now(),
    };
    let (number, url) = state
        .github
        .create_pr(&user.token, &request.title, &branch, &base, &metadata)
        .await?;
    Ok(Json(
        json!({"session_id":id,"branch":branch,"base_branch":base,"pull_request":{"number":number,"url":url,"draft":true}}),
    ))
}
pub async fn list(
    State(state): State<Arc<AppState>>,
    CurrentUser(user): CurrentUser,
) -> Result<Json<serde_json::Value>, AppError> {
    Ok(Json(
        json!({"sessions":state.github.sessions(&user.token).await?}),
    ))
}
pub async fn get(
    State(state): State<Arc<AppState>>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
) -> Result<Json<Session>, AppError> {
    Ok(Json(find(&state, &user, parse(&id)?).await?))
}
pub async fn write(
    State(state): State<Arc<AppState>>,
    CurrentUser(user): CurrentUser,
    Path((id, path)): Path<(String, String)>,
    Json(request): Json<Write>,
) -> Result<Json<serde_json::Value>, AppError> {
    let id = parse(&id)?;
    let session = find(&state, &user, id).await?;
    state.github.verify_repo_access(&user.token).await?;
    let commit = state
        .repo
        .write(
            &session.branch,
            &path,
            &request.content,
            request.expected_commit.as_deref(),
            &request.message,
            &user,
        )
        .await?;
    Ok(Json(
        json!({"path":path,"branch":session.branch,"commit":commit,"pull_request_number":session.pull_request_number}),
    ))
}
pub async fn delete(
    State(state): State<Arc<AppState>>,
    CurrentUser(user): CurrentUser,
    Path((id, path)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, AppError> {
    let session = find(&state, &user, parse(&id)?).await?;
    state.github.verify_repo_access(&user.token).await?;
    let commit = state.repo.delete(&session.branch, &path, &user).await?;
    Ok(Json(json!({"deleted":true,"commit":commit})))
}
pub async fn move_file(
    State(state): State<Arc<AppState>>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    Json(request): Json<Move>,
) -> Result<Json<serde_json::Value>, AppError> {
    let session = find(&state, &user, parse(&id)?).await?;
    state.github.verify_repo_access(&user.token).await?;
    let commit = state
        .repo
        .move_file(
            &session.branch,
            &request.from,
            &request.to,
            &request.message,
            &user,
        )
        .await?;
    Ok(Json(
        json!({"from":request.from,"to":request.to,"commit":commit}),
    ))
}
pub async fn ready(
    State(state): State<Arc<AppState>>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let session = find(&state, &user, parse(&id)?).await?;
    state.github.verify_repo_access(&user.token).await?;
    state
        .github
        .ready(&user.token, session.pull_request_number)
        .await?;
    Ok(Json(
        json!({"pull_request_number":session.pull_request_number,"draft":false}),
    ))
}
