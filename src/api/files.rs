use crate::{auth::CurrentUser, content, error::AppError, AppState};
use axum::{
    extract::{Path, Query, State},
    Json,
};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
#[derive(Deserialize)]
pub struct FileQuery {
    pub r#ref: Option<String>,
    pub path: Option<String>,
}
fn valid_ref(value: &str) -> Result<(), AppError> {
    if value.is_empty()
        || value.starts_with('-')
        || value.contains("..")
        || value.contains(' ')
        || value.contains('\0')
    {
        Err(AppError::bad_request(
            "invalid_ref",
            "invalid git reference",
        ))
    } else {
        Ok(())
    }
}
pub async fn list(
    State(state): State<Arc<AppState>>,
    CurrentUser(_): CurrentUser,
    Query(q): Query<FileQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let r = q
        .r#ref
        .unwrap_or_else(|| state.config.repository.default_branch.clone());
    valid_ref(&r)?;
    let path = q.path.unwrap_or_default();
    if !path.is_empty() {
        content::normalize(&path)?;
    }
    let entries = state
        .repo
        .list(&r, if path.is_empty() { None } else { Some(&path) })
        .await?;
    Ok(Json(json!({"ref":r,"path":path,"entries":entries})))
}
pub async fn read(
    State(state): State<Arc<AppState>>,
    CurrentUser(_): CurrentUser,
    Path(path): Path<String>,
    Query(q): Query<FileQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let r = q
        .r#ref
        .unwrap_or_else(|| state.config.repository.default_branch.clone());
    valid_ref(&r)?;
    let (value, commit) = state.repo.read(&r, &path).await?;
    Ok(Json(
        json!({"path":path,"ref":r,"commit":commit,"size":value.len(),"media_type":mime_guess::from_path(&path).first_raw().unwrap_or("text/plain"),"encoding":"utf-8","content":value,"frontmatter":content::frontmatter(&value)}),
    ))
}
