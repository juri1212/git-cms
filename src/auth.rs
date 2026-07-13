use crate::{error::AppError, AppState};
use axum::{
    extract::FromRequestParts,
    http::{header, request::Parts},
};
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone)]
pub struct UserCredential {
    pub token: String,
    pub login: String,
    pub github_user_id: u64,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub login: String,
    pub iat: usize,
    pub exp: usize,
    pub repo: String,
}
#[derive(Clone)]
pub struct CurrentUser(pub UserCredential);
pub fn issue(
    config: &crate::config::Config,
    credential: &UserCredential,
) -> Result<String, AppError> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as usize;
    let secret = config.secret("CMS_JWT_SECRET")?;
    jsonwebtoken::encode(
        &Header::default(),
        &Claims {
            sub: credential.github_user_id.to_string(),
            login: credential.login.clone(),
            iat: now,
            exp: now + 8 * 60 * 60,
            repo: config.repo_slug(),
        },
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .map_err(|e| AppError::internal(e.to_string()))
}
#[async_trait::async_trait]
impl FromRequestParts<Arc<AppState>> for CurrentUser {
    type Rejection = AppError;
    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let value = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| {
                AppError::new(
                    axum::http::StatusCode::UNAUTHORIZED,
                    "unauthorized",
                    "missing bearer token",
                )
            })?;
        let token = value.strip_prefix("Bearer ").ok_or_else(|| {
            AppError::new(
                axum::http::StatusCode::UNAUTHORIZED,
                "unauthorized",
                "invalid bearer token",
            )
        })?;
        let secret = state.config.secret("CMS_JWT_SECRET")?;
        let claims = jsonwebtoken::decode::<Claims>(
            token,
            &DecodingKey::from_secret(secret.as_bytes()),
            &Validation::default(),
        )
        .map_err(|_| {
            AppError::new(
                axum::http::StatusCode::UNAUTHORIZED,
                "unauthorized",
                "invalid or expired token",
            )
        })?
        .claims;
        if claims.repo != state.config.repo_slug() {
            return Err(AppError::new(
                axum::http::StatusCode::UNAUTHORIZED,
                "unauthorized",
                "token repository does not match",
            ));
        }
        state
            .credentials
            .get(&claims.sub)
            .map(|c| Self(c.clone()))
            .ok_or_else(|| {
                AppError::new(
                    axum::http::StatusCode::UNAUTHORIZED,
                    "reauthentication_required",
                    "GitHub session is no longer available",
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Access, Config, Content, Github, Repository, Server};
    use std::path::PathBuf;

    fn config() -> Config {
        Config {
            server: Server {
                bind: "127.0.0.1:0".into(),
                public_base_url: "http://localhost".into(),
                frontend_callback_url: "http://localhost/callback".into(),
            },
            repository: Repository {
                owner: "owner".into(),
                name: "repo".into(),
                default_branch: "main".into(),
                local_path: PathBuf::from("repo"),
                branch_prefix: "cms/".into(),
                commit_author_name: "CMS".into(),
                commit_author_email: "cms@example.com".into(),
            },
            content: Content {
                roots: vec!["content/**".into()],
                max_file_bytes: 100,
                editable_extensions: vec!["md".into()],
            },
            github: Github {
                app_id_env: "APP_ID".into(),
                client_id_env: "CLIENT_ID".into(),
                client_secret_env: "CLIENT_SECRET".into(),
                private_key_pem_env: "PRIVATE_KEY".into(),
                installation_id_env: "INSTALLATION_ID".into(),
                webhook_secret_env: "WEBHOOK_SECRET".into(),
            },
            access: Access::default(),
        }
    }

    #[test]
    fn issues_a_signed_repository_scoped_jwt() {
        std::env::set_var("CMS_JWT_SECRET", "test-secret");
        let credential = UserCredential {
            token: "github-token".into(),
            login: "juri1212".into(),
            github_user_id: 42,
        };
        let token = issue(&config(), &credential).unwrap();
        let claims = jsonwebtoken::decode::<Claims>(
            &token,
            &DecodingKey::from_secret(b"test-secret"),
            &Validation::default(),
        )
        .unwrap()
        .claims;
        assert_eq!(claims.sub, "42");
        assert_eq!(claims.login, "juri1212");
        assert_eq!(claims.repo, "owner/repo");
    }
}
