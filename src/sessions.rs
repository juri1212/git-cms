use crate::error::AppError;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
#[derive(Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    pub session_id: Uuid,
    pub created_by: String,
    pub base_branch: String,
    pub created_at: DateTime<Utc>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Session {
    pub session_id: Uuid,
    pub title: String,
    pub branch: String,
    pub base_branch: String,
    pub created_by: String,
    pub pull_request_number: u64,
    pub pull_request_url: String,
    pub draft: bool,
}
pub fn metadata_block(m: &SessionMetadata) -> String {
    format!(
        "<!-- cms-session\n{}\n-->",
        serde_json::to_string_pretty(m).expect("metadata serializes")
    )
}
pub fn parse_metadata(body: &str) -> Result<SessionMetadata, AppError> {
    let start = body.find("<!-- cms-session\n").ok_or_else(|| {
        AppError::new(
            axum::http::StatusCode::NOT_FOUND,
            "session_not_found",
            "pull request is not a CMS session",
        )
    })? + "<!-- cms-session\n".len();
    let end = body[start..].find("\n-->").ok_or_else(|| {
        AppError::bad_request("invalid_session_metadata", "metadata block is incomplete")
    })? + start;
    serde_json::from_str(&body[start..end])
        .map_err(|_| AppError::bad_request("invalid_session_metadata", "metadata block is invalid"))
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_round_trips() {
        let metadata = SessionMetadata {
            session_id: Uuid::new_v4(),
            created_by: "juri1212".into(),
            base_branch: "main".into(),
            created_at: Utc::now(),
        };
        assert_eq!(
            parse_metadata(&metadata_block(&metadata))
                .unwrap()
                .session_id,
            metadata.session_id
        );
    }

    #[test]
    fn rejects_missing_or_malformed_metadata() {
        assert!(parse_metadata("normal PR body").is_err());
        assert!(parse_metadata("<!-- cms-session\nnot-json\n-->").is_err());
    }
}
