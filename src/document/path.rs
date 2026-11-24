use std::fs;
use std::path::{Path, PathBuf};

use crate::platform::documents_dir;

use super::UNTITLED;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct PathKey(String);

impl PathKey {
    pub(crate) fn new(path: &Path) -> Self {
        let resolved = fs::canonicalize(path)
            .or_else(|_| std::path::absolute(path))
            .unwrap_or_else(|_| path.to_path_buf());
        Self(normalize_path_key(&resolved))
    }
}

#[derive(Clone, Debug)]
pub(crate) struct DocPath {
    path: PathBuf,
    key: PathKey,
}

impl DocPath {
    pub(crate) fn new(path: PathBuf) -> Self {
        let path = std::path::absolute(&path).unwrap_or(path);
        let key = PathKey::new(&path);
        Self { path, key }
    }

    #[inline]
    pub(crate) fn as_path(&self) -> &Path {
        &self.path
    }

    #[inline]
    pub(crate) fn key(&self) -> &PathKey {
        &self.key
    }
}

pub(super) fn normalize_path_key(path: &Path) -> String {
    let raw = path.to_string_lossy();
    let stripped = raw.strip_prefix(r"\\?\").unwrap_or(&raw);
    stripped.replace('/', "\\").to_lowercase()
}

pub(crate) fn file_name(path: Option<&Path>) -> Option<String> {
    path.and_then(Path::file_name)
        .map(|name| name.to_string_lossy().into_owned())
}

pub(crate) fn display_name(path: Option<&Path>) -> String {
    file_name(path).unwrap_or_else(|| UNTITLED.into())
}

pub(crate) fn default_directory(path: Option<&Path>) -> PathBuf {
    path.and_then(Path::parent)
        .map(Path::to_path_buf)
        .or_else(documents_dir)
        .unwrap_or_else(|| PathBuf::from("."))
}
