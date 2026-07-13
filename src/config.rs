use crate::error::AppError;
use serde::Deserialize;
use std::{env, fs, path::PathBuf};

#[derive(Clone, Deserialize)]
pub struct Config {
    pub server: Server,
    pub repository: Repository,
    pub content: Content,
    pub github: Github,
    #[serde(default)]
    pub access: Access,
}
#[derive(Clone, Deserialize)]
pub struct Server {
    pub bind: String,
    pub public_base_url: String,
    pub frontend_callback_url: String,
}
#[derive(Clone, Deserialize)]
pub struct Repository {
    pub owner: String,
    pub name: String,
    pub default_branch: String,
    pub local_path: PathBuf,
    #[serde(default = "prefix")]
    pub branch_prefix: String,
    pub commit_author_name: String,
    pub commit_author_email: String,
}
#[derive(Clone, Deserialize)]
pub struct Content {
    pub roots: Vec<String>,
    pub max_file_bytes: u64,
    pub editable_extensions: Vec<String>,
}
#[derive(Clone, Deserialize)]
pub struct Github {
    pub app_id_env: String,
    pub client_id_env: String,
    pub client_secret_env: String,
    pub private_key_pem_env: String,
    pub installation_id_env: String,
    pub webhook_secret_env: String,
}
#[derive(Clone, Default, Deserialize)]
pub struct Access {
    #[serde(default)]
    pub allowed_users: Vec<String>,
    #[serde(default)]
    pub allowed_orgs: Vec<String>,
}
fn prefix() -> String {
    "cms/".into()
}
impl Config {
    pub fn load() -> Result<Self, AppError> {
        let path = env::var("CMS_CONFIG").unwrap_or_else(|_| "cms.toml".into());
        let data = fs::read_to_string(&path)
            .map_err(|e| AppError::internal(format!("cannot read {path}: {e}")))?;
        let c: Self = toml::from_str(&data)
            .map_err(|e| AppError::internal(format!("invalid config: {e}")))?;
        if c.content.roots.is_empty()
            || c.repository.owner.is_empty()
            || c.repository.name.is_empty()
        {
            return Err(AppError::bad_request(
                "invalid_config",
                "repository and content roots are required",
            ));
        }
        Ok(c)
    }
    pub fn secret(&self, name: &str) -> Result<String, AppError> {
        env::var(name).map_err(|_| {
            AppError::internal(format!("missing required environment variable {name}"))
        })
    }
    pub fn repo_slug(&self) -> String {
        format!("{}/{}", self.repository.owner, self.repository.name)
    }
}
