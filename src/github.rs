use crate::{
    config::Config,
    error::AppError,
    sessions::{metadata_block, parse_metadata, Session, SessionMetadata},
};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone)]
pub struct GithubClient {
    config: Arc<Config>,
    http: Client,
    api_base: String,
    oauth_base: String,
}
#[derive(Clone, Deserialize)]
pub struct GithubUser {
    pub id: u64,
    pub login: String,
}
#[derive(Deserialize)]
struct TokenResponse {
    token: String,
}
#[derive(Deserialize)]
struct OAuthResponse {
    access_token: String,
}
#[derive(Deserialize)]
struct Pull {
    number: u64,
    html_url: String,
    title: String,
    body: Option<String>,
    head: Head,
    base: Base,
    draft: bool,
}
#[derive(Deserialize)]
struct Head {
    #[serde(rename = "ref")]
    branch: String,
}
#[derive(Deserialize)]
struct Base {
    #[serde(rename = "ref")]
    branch: String,
}
impl GithubClient {
    pub fn new(config: Arc<Config>) -> Result<Self, AppError> {
        Ok(Self {
            config,
            http: Client::builder()
                .user_agent("git-cms/0.1")
                .build()
                .map_err(|e| AppError::internal(e.to_string()))?,
            api_base: "https://api.github.com".into(),
            oauth_base: "https://github.com".into(),
        })
    }
    fn api_url(&self, path: &str) -> String {
        format!("{}{}", self.api_base, path)
    }
    fn oauth_url(&self, path: &str) -> String {
        format!("{}{}", self.oauth_base, path)
    }
    fn app_jwt(&self) -> Result<String, AppError> {
        #[derive(Serialize)]
        struct C {
            iat: usize,
            exp: usize,
            iss: String,
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as usize;
        let key = self
            .config
            .secret(&self.config.github.private_key_pem_env)?
            .replace("\\n", "\n");
        let id = self.config.secret(&self.config.github.app_id_env)?;
        jsonwebtoken::encode(
            &Header::new(Algorithm::RS256),
            &C {
                iat: now.saturating_sub(60),
                exp: now + 540,
                iss: id,
            },
            &EncodingKey::from_rsa_pem(key.as_bytes())
                .map_err(|e| AppError::internal(e.to_string()))?,
        )
        .map_err(|e| AppError::internal(e.to_string()))
    }
    pub async fn installation_token(&self) -> Result<String, AppError> {
        let jwt = self.app_jwt()?;
        let id = self
            .config
            .secret(&self.config.github.installation_id_env)?;
        let r = self
            .http
            .post(self.api_url(&format!("/app/installations/{id}/access_tokens")))
            .header("Accept", "application/vnd.github+json")
            .bearer_auth(jwt)
            .send()
            .await
            .map_err(github_failure)?;
        if !r.status().is_success() {
            return Err(github_status(r).await);
        }
        Ok(r.json::<TokenResponse>()
            .await
            .map_err(github_failure)?
            .token)
    }
    pub fn authorization_url(&self, state: &str) -> Result<String, AppError> {
        let id = self.config.secret(&self.config.github.client_id_env)?;
        let callback = format!(
            "{}/auth/github/callback",
            self.config.server.public_base_url.trim_end_matches('/')
        );
        let mut url = url::Url::parse(&self.oauth_url("/login/oauth/authorize"))
            .map_err(|e| AppError::internal(e.to_string()))?;
        url.query_pairs_mut()
            .append_pair("client_id", &id)
            .append_pair("state", state)
            .append_pair("redirect_uri", &callback);
        Ok(url.to_string())
    }
    pub async fn exchange_code(&self, code: &str) -> Result<String, AppError> {
        let id = self.config.secret(&self.config.github.client_id_env)?;
        let secret = self.config.secret(&self.config.github.client_secret_env)?;
        let callback = format!(
            "{}/auth/github/callback",
            self.config.server.public_base_url.trim_end_matches('/')
        );
        let r = self.http.post(self.oauth_url("/login/oauth/access_token")).header("Accept", "application/json").json(&serde_json::json!({"client_id": id, "client_secret": secret, "code": code, "redirect_uri": callback})).send().await.map_err(github_failure)?;
        if !r.status().is_success() {
            return Err(github_status(r).await);
        }
        Ok(r.json::<OAuthResponse>()
            .await
            .map_err(github_failure)?
            .access_token)
    }
    pub async fn current_user(&self, token: &str) -> Result<GithubUser, AppError> {
        let r = self
            .http
            .get(self.api_url("/user"))
            .bearer_auth(token)
            .header("Accept", "application/vnd.github+json")
            .send()
            .await
            .map_err(github_failure)?;
        if !r.status().is_success() {
            return Err(github_status(r).await);
        }
        r.json().await.map_err(github_failure)
    }
    pub async fn verify_repo_access(&self, token: &str) -> Result<(), AppError> {
        let url = self.api_url(&format!("/repos/{}", self.config.repo_slug()));
        let r = self
            .http
            .get(url)
            .bearer_auth(token)
            .header("Accept", "application/vnd.github+json")
            .send()
            .await
            .map_err(github_failure)?;
        if r.status() == StatusCode::NOT_FOUND || r.status() == StatusCode::FORBIDDEN {
            return Err(AppError::new(
                axum::http::StatusCode::FORBIDDEN,
                "repository_access_denied",
                "GitHub user cannot access the configured repository",
            ));
        }
        if !r.status().is_success() {
            return Err(github_status(r).await);
        }
        Ok(())
    }
    pub async fn allowed(&self, token: &str, user: &GithubUser) -> Result<bool, AppError> {
        if self.config.access.allowed_users.is_empty() && self.config.access.allowed_orgs.is_empty()
        {
            return Ok(true);
        }
        if self
            .config
            .access
            .allowed_users
            .iter()
            .any(|x| x.eq_ignore_ascii_case(&user.login))
        {
            return Ok(true);
        }
        if self.config.access.allowed_orgs.is_empty() {
            return Ok(false);
        }
        #[derive(Deserialize)]
        struct Org {
            login: String,
        }
        let r = self
            .http
            .get(self.api_url("/user/orgs"))
            .bearer_auth(token)
            .header("Accept", "application/vnd.github+json")
            .send()
            .await
            .map_err(github_failure)?;
        if !r.status().is_success() {
            return Err(github_status(r).await);
        }
        let orgs: Vec<Org> = r.json().await.map_err(github_failure)?;
        Ok(orgs.iter().any(|o| {
            self.config
                .access
                .allowed_orgs
                .iter()
                .any(|a| a.eq_ignore_ascii_case(&o.login))
        }))
    }
    pub async fn create_pr(
        &self,
        token: &str,
        title: &str,
        branch: &str,
        base: &str,
        meta: &SessionMetadata,
    ) -> Result<(u64, String), AppError> {
        let url = self.api_url(&format!("/repos/{}/pulls", self.config.repo_slug()));
        let r = self.http.post(url).bearer_auth(token).header("Accept", "application/vnd.github+json").json(&serde_json::json!({"title": format!("CMS draft: {title}"), "head": branch, "base": base, "body": metadata_block(meta), "draft": true})).send().await.map_err(github_failure)?;
        if !r.status().is_success() {
            return Err(github_status(r).await);
        }
        let p: Pull = r.json().await.map_err(github_failure)?;
        self.labels(token, p.number).await?;
        Ok((p.number, p.html_url))
    }
    async fn labels(&self, token: &str, number: u64) -> Result<(), AppError> {
        for (name, color) in [("cms", "0e8a16"), ("cms-session", "1d76db")] {
            let create = self
                .http
                .post(self.api_url(&format!("/repos/{}/labels", self.config.repo_slug())))
                .bearer_auth(token)
                .header("Accept", "application/vnd.github+json")
                .json(&serde_json::json!({"name":name,"color":color}))
                .send()
                .await
                .map_err(github_failure)?;
            if !create.status().is_success() && create.status() != StatusCode::UNPROCESSABLE_ENTITY
            {
                return Err(github_status(create).await);
            }
        }
        let url = self.api_url(&format!(
            "/repos/{}/issues/{number}/labels",
            self.config.repo_slug()
        ));
        let r = self
            .http
            .post(url)
            .bearer_auth(token)
            .header("Accept", "application/vnd.github+json")
            .json(&serde_json::json!({"labels":["cms", "cms-session"]}))
            .send()
            .await
            .map_err(github_failure)?;
        if r.status().is_success() {
            Ok(())
        } else {
            Err(github_status(r).await)
        }
    }
    pub async fn sessions(&self, token: &str) -> Result<Vec<Session>, AppError> {
        let url = self.api_url(&format!(
            "/repos/{}/pulls?state=open&per_page=100",
            self.config.repo_slug()
        ));
        let r = self
            .http
            .get(url)
            .bearer_auth(token)
            .header("Accept", "application/vnd.github+json")
            .send()
            .await
            .map_err(github_failure)?;
        if !r.status().is_success() {
            return Err(github_status(r).await);
        }
        let pulls: Vec<Pull> = r.json().await.map_err(github_failure)?;
        Ok(pulls
            .into_iter()
            .filter_map(|p| {
                let m = parse_metadata(p.body.as_deref().unwrap_or("")).ok()?;
                Some(Session {
                    session_id: m.session_id,
                    title: p
                        .title
                        .strip_prefix("CMS draft: ")
                        .unwrap_or(&p.title)
                        .to_string(),
                    branch: p.head.branch,
                    base_branch: p.base.branch,
                    created_by: m.created_by,
                    pull_request_number: p.number,
                    pull_request_url: p.html_url,
                    draft: p.draft,
                })
            })
            .collect())
    }
    pub async fn ready(&self, token: &str, number: u64) -> Result<(), AppError> {
        let url = self.api_url(&format!(
            "/repos/{}/pulls/{number}/ready_for_review",
            self.config.repo_slug()
        ));
        let r = self
            .http
            .post(url)
            .bearer_auth(token)
            .header("Accept", "application/vnd.github+json")
            .send()
            .await
            .map_err(github_failure)?;
        if r.status().is_success() {
            Ok(())
        } else {
            Err(github_status(r).await)
        }
    }
}
fn github_failure(e: reqwest::Error) -> AppError {
    AppError::new(
        axum::http::StatusCode::BAD_GATEWAY,
        "github_failure",
        e.to_string(),
    )
}
async fn github_status(r: reqwest::Response) -> AppError {
    let status = r.status();
    let text = r.text().await.unwrap_or_default();
    AppError::new(
        axum::http::StatusCode::BAD_GATEWAY,
        "github_failure",
        format!("GitHub returned {status}: {text}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Access, Content, Github, Repository, Server};
    use std::path::PathBuf;
    use uuid::Uuid;
    use wiremock::{
        matchers::{method, path},
        Mock, MockServer, ResponseTemplate,
    };

    fn config() -> Arc<Config> {
        Arc::new(Config {
            server: Server {
                bind: "127.0.0.1:0".into(),
                public_base_url: "http://cms.test".into(),
                frontend_callback_url: "http://frontend.test/callback".into(),
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
                app_id_env: "GH_APP_ID_TEST".into(),
                client_id_env: "GH_CLIENT_ID_TEST".into(),
                client_secret_env: "GH_CLIENT_SECRET_TEST".into(),
                private_key_pem_env: "GH_PRIVATE_KEY_TEST".into(),
                installation_id_env: "GH_INSTALLATION_ID_TEST".into(),
                webhook_secret_env: "GH_WEBHOOK_SECRET_TEST".into(),
            },
            access: Access::default(),
        })
    }

    fn client(config: Arc<Config>, server: &MockServer) -> GithubClient {
        GithubClient {
            config,
            http: Client::new(),
            api_base: server.uri(),
            oauth_base: server.uri(),
        }
    }

    #[tokio::test]
    async fn exchanges_oauth_code_and_verifies_repository_access() {
        std::env::set_var("GH_CLIENT_ID_TEST", "client-id");
        std::env::set_var("GH_CLIENT_SECRET_TEST", "client-secret");
        let server = MockServer::start().await;
        let github = client(config(), &server);
        Mock::given(method("POST"))
            .and(path("/login/oauth/access_token"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"access_token":"user-token"})),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/user"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"id":42,"login":"juri1212"})),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/owner/repo"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"id":1})))
            .mount(&server)
            .await;

        assert_eq!(github.exchange_code("code").await.unwrap(), "user-token");
        let user = github.current_user("user-token").await.unwrap();
        assert_eq!(user.login, "juri1212");
        github.verify_repo_access("user-token").await.unwrap();
    }

    #[tokio::test]
    async fn parses_cms_sessions_and_marks_pr_ready() {
        let server = MockServer::start().await;
        let github = client(config(), &server);
        let session_id = Uuid::new_v4();
        let body = format!("<!-- cms-session\n{{\"session_id\":\"{session_id}\",\"created_by\":\"juri1212\",\"base_branch\":\"main\",\"created_at\":\"2026-07-13T12:00:00Z\"}}\n-->");
        Mock::given(method("GET")).and(path("/repos/owner/repo/pulls")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([{"number":12,"html_url":"https://github.com/owner/repo/pull/12","title":"CMS draft: Homepage","body":body,"head":{"ref":"cms/juri1212/session"},"base":{"ref":"main"},"draft":true}]))).mount(&server).await;
        Mock::given(method("POST"))
            .and(path("/repos/owner/repo/pulls/12/ready_for_review"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let sessions = github.sessions("user-token").await.unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, session_id);
        assert_eq!(sessions[0].title, "Homepage");
        github.ready("user-token", 12).await.unwrap();
    }
}
