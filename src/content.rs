use crate::{config::Content, error::AppError};
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde_json::Value;
use std::path::{Component, Path, PathBuf};
pub fn allowlist(config: &Content) -> Result<GlobSet, AppError> {
    let mut b = GlobSetBuilder::new();
    for root in &config.roots {
        b.add(Glob::new(root).map_err(|e| AppError::internal(e.to_string()))?);
    }
    b.build().map_err(|e| AppError::internal(e.to_string()))
}
pub fn normalize(path: &str) -> Result<PathBuf, AppError> {
    if path.is_empty() || path.contains('\0') {
        return Err(AppError::bad_request(
            "invalid_path",
            "path is empty or contains a null byte",
        ));
    }
    let p = Path::new(path);
    if p.is_absolute()
        || p.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(AppError::bad_request(
            "invalid_path",
            "path must be relative and cannot traverse directories",
        ));
    }
    Ok(p.to_path_buf())
}
pub fn validate(config: &Content, path: &str) -> Result<PathBuf, AppError> {
    let p = normalize(path)?;
    let allowed = allowlist(config)?;
    if !allowed.is_match(&p) {
        return Err(AppError::new(
            axum::http::StatusCode::FORBIDDEN,
            "path_not_allowed",
            "Path is outside configured CMS roots",
        ));
    }
    let ext = p.extension().and_then(|x| x.to_str()).unwrap_or("");
    if !config.editable_extensions.iter().any(|v| v == ext) {
        return Err(AppError::new(
            axum::http::StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_file",
            "file extension is not editable",
        ));
    }
    Ok(p)
}
pub fn ensure_text(bytes: &[u8], max: u64) -> Result<&str, AppError> {
    if bytes.len() as u64 > max {
        return Err(AppError::new(
            axum::http::StatusCode::PAYLOAD_TOO_LARGE,
            "file_too_large",
            "file exceeds configured size limit",
        ));
    }
    std::str::from_utf8(bytes).map_err(|_| {
        AppError::new(
            axum::http::StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "binary_file",
            "only UTF-8 text files are supported",
        )
    })
}
pub fn frontmatter(content: &str) -> Option<Value> {
    let body = content.strip_prefix("---\n")?;
    let end = body.find("\n---\n")?;
    serde_yaml::from_str::<Value>(&body[..end]).ok()
}
#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Content {
        Content {
            roots: vec!["content/**".into(), "README.md".into()],
            max_file_bytes: 5,
            editable_extensions: vec!["md".into(), "txt".into()],
        }
    }

    #[test]
    fn rejects_traversal() {
        assert!(normalize("../secret").is_err());
        assert!(normalize("/secret").is_err());
        assert!(normalize("content/../../secret").is_err());
    }

    #[test]
    fn only_allows_configured_roots_and_extensions() {
        assert!(validate(&config(), "content/home.md").is_ok());
        assert!(validate(&config(), "README.md").is_ok());
        assert!(validate(&config(), "private/key.md").is_err());
        assert!(validate(&config(), "content/image.png").is_err());
    }

    #[test]
    fn enforces_text_and_size_limits() {
        assert_eq!(ensure_text(b"hello", 5).unwrap(), "hello");
        assert!(ensure_text(b"longer", 5).is_err());
        assert!(ensure_text(&[0xff], 5).is_err());
    }

    #[test]
    fn extracts_yaml_frontmatter() {
        let value = frontmatter("---\ntitle: Home\ndraft: false\n---\n# Home\n").unwrap();
        assert_eq!(value["title"], "Home");
        assert_eq!(value["draft"], false);
    }
}
