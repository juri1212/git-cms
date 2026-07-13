use crate::{auth::UserCredential, config::Config, content, error::AppError, github::GithubClient};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::{process::Command, sync::Mutex};
#[derive(Clone)]
pub struct GitRepository {
    config: Arc<Config>,
    github: GithubClient,
    lock: Arc<Mutex<()>>,
}
#[derive(serde::Serialize)]
pub struct FileEntry {
    pub path: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extension: Option<String>,
}
impl GitRepository {
    pub async fn new(config: Arc<Config>, github: GithubClient) -> Result<Self, AppError> {
        let r = Self {
            config,
            github,
            lock: Arc::new(Mutex::new(())),
        };
        r.initialize().await?;
        Ok(r)
    }
    fn path(&self) -> &Path {
        &self.config.repository.local_path
    }
    async fn git(&self, args: &[&str]) -> Result<Vec<u8>, AppError> {
        let output = Command::new("git")
            .arg("-C")
            .arg(self.path())
            .args(args)
            .output()
            .await?;
        if output.status.success() {
            Ok(output.stdout)
        } else {
            Err(AppError::internal(
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ))
        }
    }
    async fn initialize(&self) -> Result<(), AppError> {
        let parent = self
            .path()
            .parent()
            .ok_or_else(|| AppError::internal("repository local path must have a parent"))?;
        tokio::fs::create_dir_all(parent).await?;
        let _ = tokio::fs::create_dir_all(".cms/locks").await;
        if !self.path().join(".git").exists() {
            let token = self.github.installation_token().await?;
            let url = self.auth_url(&token);
            let output = Command::new("git")
                .arg("clone")
                .arg(&url)
                .arg(self.path())
                .output()
                .await?;
            if !output.status.success() {
                return Err(AppError::internal(format!(
                    "clone failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                )));
            }
            self.git(&["remote", "set-url", "origin", &self.public_url()])
                .await?;
        }
        let remote = String::from_utf8(self.git(&["config", "--get", "remote.origin.url"]).await?)
            .unwrap_or_default();
        if !remote.contains(&format!(
            "{}/{}",
            self.config.repository.owner, self.config.repository.name
        )) {
            return Err(AppError::internal(
                "existing repository origin does not match configured repository",
            ));
        }
        let token = self.github.installation_token().await?;
        self.fetch(&token).await?;
        let branch = self.config.repository.default_branch.clone();
        self.git(&["checkout", "-B", &branch, &format!("origin/{branch}")])
            .await?;
        Ok(())
    }
    fn public_url(&self) -> String {
        format!("https://github.com/{}.git", self.config.repo_slug())
    }
    fn auth_url(&self, token: &str) -> String {
        format!(
            "https://x-access-token:{token}@github.com/{}.git",
            self.config.repo_slug()
        )
    }
    async fn fetch(&self, token: &str) -> Result<(), AppError> {
        let url = self.auth_url(token);
        self.git(&["fetch", &url, "+refs/heads/*:refs/remotes/origin/*"])
            .await
            .map(|_| ())
    }
    pub async fn list(
        &self,
        reference: &str,
        requested: Option<&str>,
    ) -> Result<Vec<FileEntry>, AppError> {
        let _guard = self.lock.lock().await;
        let path = requested.unwrap_or("");
        if !path.is_empty() {
            content::normalize(path)?;
        }
        let spec = if path.is_empty() {
            reference.to_string()
        } else {
            format!("{reference}:{path}")
        };
        let out = self.git(&["ls-tree", "-l", &spec]).await.map_err(|_| {
            AppError::new(
                axum::http::StatusCode::NOT_FOUND,
                "path_not_found",
                "path or ref not found",
            )
        })?;
        let allowed = content::allowlist(&self.config.content)?;
        let base = Path::new(path);
        let mut entries = Vec::new();
        for line in String::from_utf8_lossy(&out).lines() {
            let Some((before, name)) = line.split_once('\t') else {
                continue;
            };
            let parts: Vec<_> = before.split_whitespace().collect();
            if parts.len() < 4 {
                continue;
            }
            let full = base.join(name);
            let kind = if parts[1] == "tree" {
                "directory"
            } else {
                "file"
            };
            if kind == "file" && !allowed.is_match(&full) {
                continue;
            }
            if kind == "directory" && !self.may_contain_allowed(&full, &allowed) {
                continue;
            }
            entries.push(FileEntry {
                path: full.to_string_lossy().into(),
                kind: kind.into(),
                size: if kind == "file" {
                    parts[3].parse().ok()
                } else {
                    None
                },
                extension: if kind == "file" {
                    full.extension()
                        .and_then(|v| v.to_str())
                        .map(str::to_string)
                } else {
                    None
                },
            });
        }
        Ok(entries)
    }
    fn may_contain_allowed(&self, p: &Path, allowed: &globset::GlobSet) -> bool {
        allowed.is_match(p)
            || self
                .config
                .content
                .roots
                .iter()
                .any(|r| r.starts_with(&format!("{}/", p.to_string_lossy())))
    }
    pub async fn read(&self, reference: &str, file: &str) -> Result<(String, String), AppError> {
        let path = content::validate(&self.config.content, file)?;
        let _guard = self.lock.lock().await;
        let bytes = self
            .git(&["show", &format!("{reference}:{}", path.to_string_lossy())])
            .await
            .map_err(|_| {
                AppError::new(
                    axum::http::StatusCode::NOT_FOUND,
                    "file_not_found",
                    "file or ref not found",
                )
            })?;
        let text = content::ensure_text(&bytes, self.config.content.max_file_bytes)?.to_owned();
        let commit = String::from_utf8(
            self.git(&[
                "log",
                "-1",
                "--format=%H",
                reference,
                "--",
                &path.to_string_lossy(),
            ])
            .await?,
        )
        .unwrap_or_default()
        .trim()
        .to_owned();
        Ok((text, commit))
    }
    pub async fn create_branch_with_initial_file(
        &self,
        base: &str,
        branch: &str,
        file: &str,
        value: &str,
        message: &str,
        user: &UserCredential,
    ) -> Result<String, AppError> {
        let path = content::validate(&self.config.content, file)?;
        if value.len() as u64 > self.config.content.max_file_bytes {
            return Err(AppError::new(
                axum::http::StatusCode::PAYLOAD_TOO_LARGE,
                "file_too_large",
                "file exceeds configured size limit",
            ));
        }
        let _g = self.lock.lock().await;
        self.fetch(&user.token).await?;
        self.git(&["checkout", "-B", branch, &format!("origin/{base}")])
            .await?;
        self.ensure_safe_target(&path, false).await?;
        let absolute = self.path().join(&path);
        if let Some(parent) = absolute.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(absolute, value).await?;
        self.commit_push(&path, message, branch, user).await
    }
    async fn checkout_session(&self, branch: &str, token: &str) -> Result<(), AppError> {
        self.fetch(token).await?;
        self.git(&["checkout", "-B", branch, &format!("origin/{branch}")])
            .await
            .map(|_| ())
    }
    async fn ensure_safe_target(&self, path: &Path, allow_file: bool) -> Result<(), AppError> {
        let root = std::fs::canonicalize(self.path()).map_err(AppError::from)?;
        let target = self.path().join(path);
        if let Ok(metadata) = std::fs::symlink_metadata(&target) {
            if metadata.file_type().is_symlink() && !allow_file {
                return Err(AppError::new(
                    axum::http::StatusCode::FORBIDDEN,
                    "symlink_not_allowed",
                    "CMS cannot write through a symlink",
                ));
            }
        }
        let mut parent = target
            .parent()
            .map(PathBuf::from)
            .ok_or_else(|| AppError::bad_request("invalid_path", "path has no parent"))?;
        while !parent.exists() {
            parent = parent.parent().map(PathBuf::from).ok_or_else(|| {
                AppError::bad_request("invalid_path", "path has no existing parent")
            })?;
        }
        let resolved = std::fs::canonicalize(parent).map_err(AppError::from)?;
        if !resolved.starts_with(root) {
            return Err(AppError::new(
                axum::http::StatusCode::FORBIDDEN,
                "symlink_not_allowed",
                "path resolves outside the repository",
            ));
        }
        Ok(())
    }
    async fn push(&self, branch: &str, token: &str) -> Result<(), AppError> {
        let url = self.auth_url(token);
        let out = Command::new("git")
            .arg("-C")
            .arg(self.path())
            .args(["push", &url, &format!("HEAD:refs/heads/{branch}")])
            .output()
            .await?;
        if out.status.success() {
            Ok(())
        } else {
            Err(AppError::new(
                axum::http::StatusCode::CONFLICT,
                "push_conflict",
                String::from_utf8_lossy(&out.stderr).trim().to_string(),
            ))
        }
    }
    pub async fn write(
        &self,
        branch: &str,
        file: &str,
        value: &str,
        expected: Option<&str>,
        message: &str,
        user: &UserCredential,
    ) -> Result<String, AppError> {
        let path = content::validate(&self.config.content, file)?;
        if value.len() as u64 > self.config.content.max_file_bytes {
            return Err(AppError::new(
                axum::http::StatusCode::PAYLOAD_TOO_LARGE,
                "file_too_large",
                "file exceeds configured size limit",
            ));
        }
        let _g = self.lock.lock().await;
        self.checkout_session(branch, &user.token).await?;
        self.ensure_safe_target(&path, false).await?;
        if let Some(expected) = expected {
            let actual = String::from_utf8(
                self.git(&["log", "-1", "--format=%H", "--", &path.to_string_lossy()])
                    .await?,
            )
            .unwrap_or_default()
            .trim()
            .to_string();
            if actual != expected {
                return Err(AppError::new(
                    axum::http::StatusCode::CONFLICT,
                    "stale_commit",
                    "file changed since expected_commit",
                ));
            }
        }
        let absolute = self.path().join(&path);
        if let Some(parent) = absolute.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&absolute, value).await?;
        self.commit_push(&path, message, branch, user).await
    }
    pub async fn delete(
        &self,
        branch: &str,
        file: &str,
        user: &UserCredential,
    ) -> Result<String, AppError> {
        let path = content::validate(&self.config.content, file)?;
        let _g = self.lock.lock().await;
        self.checkout_session(branch, &user.token).await?;
        self.ensure_safe_target(&path, false).await?;
        let absolute = self.path().join(&path);
        if !absolute.exists() {
            return Err(AppError::new(
                axum::http::StatusCode::NOT_FOUND,
                "file_not_found",
                "file not found",
            ));
        }
        tokio::fs::remove_file(absolute).await?;
        self.commit_push(&path, &format!("Delete {file}"), branch, user)
            .await
    }
    pub async fn move_file(
        &self,
        branch: &str,
        from: &str,
        to: &str,
        message: &str,
        user: &UserCredential,
    ) -> Result<String, AppError> {
        let from = content::validate(&self.config.content, from)?;
        let to = content::validate(&self.config.content, to)?;
        let _g = self.lock.lock().await;
        self.checkout_session(branch, &user.token).await?;
        self.ensure_safe_target(&from, false).await?;
        self.ensure_safe_target(&to, false).await?;
        let target = self.path().join(&to);
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::rename(self.path().join(&from), target)
            .await
            .map_err(|_| {
                AppError::new(
                    axum::http::StatusCode::NOT_FOUND,
                    "file_not_found",
                    "source file not found",
                )
            })?;
        self.git(&[
            "add",
            "-A",
            "--",
            &from.to_string_lossy(),
            &to.to_string_lossy(),
        ])
        .await?;
        self.commit(message, user).await?;
        let id = self.commit_id().await?;
        self.push(branch, &user.token).await?;
        Ok(id)
    }
    async fn commit_push(
        &self,
        path: &Path,
        message: &str,
        branch: &str,
        user: &UserCredential,
    ) -> Result<String, AppError> {
        self.git(&["add", "-A", "--", &path.to_string_lossy()])
            .await?;
        if !self.has_staged_changes().await? {
            return Err(AppError::bad_request(
                "no_changes",
                "content is unchanged; edit it before saving",
            ));
        }
        self.commit(message, user).await?;
        let id = self.commit_id().await?;
        self.push(branch, &user.token).await?;
        Ok(id)
    }
    async fn has_staged_changes(&self) -> Result<bool, AppError> {
        let output = Command::new("git")
            .arg("-C")
            .arg(self.path())
            .args(["diff", "--cached", "--quiet"])
            .output()
            .await?;
        match output.status.code() {
            Some(0) => Ok(false),
            Some(1) => Ok(true),
            _ => Err(AppError::internal(
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            )),
        }
    }
    async fn commit(&self, message: &str, user: &UserCredential) -> Result<(), AppError> {
        let email = format!(
            "{}+{}@users.noreply.github.com",
            user.github_user_id, user.login
        );
        let mut command = Command::new("git");
        command
            .arg("-C")
            .arg(self.path())
            .arg("-c")
            .arg(format!("user.name={}", user.login))
            .arg("-c")
            .arg(format!("user.email={email}"))
            .arg("commit");
        let output = command
            .args(["-m", message])
            .env("GIT_AUTHOR_NAME", &user.login)
            .env("GIT_AUTHOR_EMAIL", &email)
            .env("GIT_COMMITTER_NAME", &user.login)
            .env("GIT_COMMITTER_EMAIL", &email)
            .output()
            .await?;
        if output.status.success() {
            Ok(())
        } else {
            Err(AppError::internal(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ))
        }
    }
    async fn commit_id(&self) -> Result<String, AppError> {
        Ok(String::from_utf8(self.git(&["rev-parse", "HEAD"]).await?)
            .unwrap_or_default()
            .trim()
            .to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Access, Content, Github, Repository, Server};
    use std::{ffi::OsStr, process::Command as StdCommand};
    use tempfile::TempDir;

    fn run_git(dir: &Path, args: &[&str]) {
        let output = StdCommand::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn config(path: PathBuf) -> Arc<Config> {
        Arc::new(Config {
            server: Server {
                bind: "127.0.0.1:0".into(),
                public_base_url: "http://localhost".into(),
                frontend_callback_url: "http://localhost/callback".into(),
            },
            repository: Repository {
                owner: "owner".into(),
                name: "repo".into(),
                default_branch: "main".into(),
                local_path: path,
                branch_prefix: "cms/".into(),
                commit_author_name: "CMS".into(),
                commit_author_email: "cms@example.com".into(),
            },
            content: Content {
                roots: vec!["content/**".into()],
                max_file_bytes: 1024,
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
        })
    }

    fn create_seed_repo() -> TempDir {
        let temp = tempfile::tempdir().unwrap();
        run_git(temp.path(), &["init"]);
        run_git(temp.path(), &["config", "user.name", "Test User"]);
        run_git(temp.path(), &["config", "user.email", "test@example.com"]);
        std::fs::create_dir_all(temp.path().join("content")).unwrap();
        std::fs::write(temp.path().join("content/home.md"), "# Home\n").unwrap();
        std::fs::write(temp.path().join("private.md"), "secret\n").unwrap();
        run_git(temp.path(), &["add", "."]);
        run_git(temp.path(), &["commit", "-m", "seed"]);
        temp
    }

    #[tokio::test]
    async fn reads_and_lists_only_allowlisted_content_from_a_local_checkout() {
        let temp = create_seed_repo();
        let config = config(temp.path().to_path_buf());
        let github = GithubClient::new(config.clone()).unwrap();
        let repo = GitRepository {
            config,
            github,
            lock: Arc::new(Mutex::new(())),
        };

        let (content, commit) = repo.read("HEAD", "content/home.md").await.unwrap();
        assert_eq!(content, "# Home\n");
        assert_eq!(commit.len(), 40);

        let entries = repo.list("HEAD", None).await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, "content");
        assert_eq!(entries[0].kind, "directory");
    }

    #[tokio::test]
    async fn detects_staged_content_changes() {
        let temp = create_seed_repo();
        let config = config(temp.path().to_path_buf());
        let github = GithubClient::new(config.clone()).unwrap();
        let repo = GitRepository {
            config,
            github,
            lock: Arc::new(Mutex::new(())),
        };

        assert!(!repo.has_staged_changes().await.unwrap());
        std::fs::write(temp.path().join("content/home.md"), "updated\n").unwrap();
        repo.git(&["add", "content/home.md"]).await.unwrap();
        assert!(repo.has_staged_changes().await.unwrap());
    }

    #[test]
    fn local_git_rejects_a_non_fast_forward_session_push() {
        let temp = tempfile::tempdir().unwrap();
        let remote = temp.path().join("remote.git");
        let seed = temp.path().join("seed");
        let first = temp.path().join("first");
        let second = temp.path().join("second");

        StdCommand::new("git")
            .args([OsStr::new("init"), OsStr::new("--bare"), remote.as_os_str()])
            .output()
            .unwrap();
        std::fs::create_dir_all(&seed).unwrap();
        run_git(&seed, &["init"]);
        run_git(&seed, &["config", "user.name", "Test User"]);
        run_git(&seed, &["config", "user.email", "test@example.com"]);
        std::fs::create_dir_all(seed.join("content")).unwrap();
        std::fs::write(seed.join("content/page.md"), "initial\n").unwrap();
        run_git(&seed, &["add", "."]);
        run_git(&seed, &["commit", "-m", "seed"]);
        run_git(&seed, &["branch", "-M", "main"]);
        run_git(
            &seed,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        run_git(&seed, &["push", "-u", "origin", "main"]);

        for clone in [&first, &second] {
            let output = StdCommand::new("git")
                .args([OsStr::new("clone"), remote.as_os_str(), clone.as_os_str()])
                .output()
                .unwrap();
            assert!(output.status.success());
            run_git(clone, &["config", "user.name", "Test User"]);
            run_git(clone, &["config", "user.email", "test@example.com"]);
            run_git(
                clone,
                &["checkout", "-b", "cms/test/session", "origin/main"],
            );
        }

        std::fs::write(first.join("content/page.md"), "first\n").unwrap();
        run_git(&first, &["add", "."]);
        run_git(&first, &["commit", "-m", "first update"]);
        run_git(&first, &["push", "-u", "origin", "cms/test/session"]);

        run_git(&second, &["fetch", "origin"]);
        run_git(&second, &["reset", "--hard", "origin/cms/test/session"]);
        std::fs::rename(
            second.join("content/page.md"),
            second.join("content/renamed.md"),
        )
        .unwrap();
        run_git(&second, &["add", "-A"]);
        run_git(&second, &["commit", "-m", "rename page"]);
        run_git(&second, &["push", "origin", "cms/test/session"]);

        std::fs::write(first.join("content/page.md"), "stale write\n").unwrap();
        run_git(&first, &["add", "."]);
        run_git(&first, &["commit", "-m", "stale write"]);
        let rejected = StdCommand::new("git")
            .arg("-C")
            .arg(&first)
            .args(["push", "origin", "cms/test/session"])
            .output()
            .unwrap();
        assert!(!rejected.status.success());
    }
}
