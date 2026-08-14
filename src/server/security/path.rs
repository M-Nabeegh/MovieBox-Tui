use std::path::{Component, Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PathSecurityError {
    #[error("absolute paths are not allowed")]
    AbsolutePath,
    #[error("path traversal is not allowed")]
    Traversal,
    #[error("path contains control characters: {0}")]
    ControlCharacter(String),
    #[error("path escapes the media root: {0}")]
    OutsideRoot(PathBuf),
    #[error("path contains an unsupported component")]
    UnsupportedComponent,
    #[error("path error: {0}")]
    Io(#[from] std::io::Error),
}

pub fn contained_path(root: &Path, relative: &Path) -> Result<PathBuf, PathSecurityError> {
    if relative.is_absolute() {
        return Err(PathSecurityError::AbsolutePath);
    }

    let canonical_root = root.canonicalize()?;
    let mut candidate = canonical_root.clone();

    for component in relative.components() {
        match component {
            Component::CurDir => continue,
            Component::ParentDir => return Err(PathSecurityError::Traversal),
            Component::RootDir | Component::Prefix(_) => {
                return Err(PathSecurityError::AbsolutePath);
            }
            Component::Normal(value) => {
                let value = value.to_string_lossy();
                if value.chars().any(char::is_control) {
                    return Err(PathSecurityError::ControlCharacter(value.into_owned()));
                }
                candidate.push(value.as_ref());
                if candidate.exists() {
                    let canonical = candidate.canonicalize()?;
                    if !canonical.starts_with(&canonical_root) {
                        return Err(PathSecurityError::OutsideRoot(canonical));
                    }
                    candidate = canonical;
                }
            }
        }
    }

    if let Ok(canonical) = candidate.canonicalize() {
        if !canonical.starts_with(&canonical_root) {
            return Err(PathSecurityError::OutsideRoot(canonical));
        }
        return Ok(canonical);
    }

    if !candidate.starts_with(&canonical_root) {
        return Err(PathSecurityError::OutsideRoot(candidate));
    }

    if relative
        .components()
        .any(|component| matches!(component, Component::Prefix(_) | Component::RootDir))
    {
        return Err(PathSecurityError::UnsupportedComponent);
    }

    Ok(candidate)
}
