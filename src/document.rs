use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const UTF8_BOM: &[u8] = b"\xef\xbb\xbf";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LineEnding {
    #[default]
    Crlf,
    Lf,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Format {
    pub line_ending: LineEnding,
    pub bom: bool,
}

pub struct Loaded {
    pub text: String,
    pub format: Format,
}

pub enum LoadError {
    Io(io::Error),
    NotUtf8,
}

pub fn load(path: &Path) -> Result<Loaded, LoadError> {
    let bytes = fs::read(path).map_err(LoadError::Io)?;
    let (bom, body) = match bytes.strip_prefix(UTF8_BOM) {
        Some(body) => (true, body.to_vec()),
        None => (false, bytes),
    };
    let text = String::from_utf8(body).map_err(|_| LoadError::NotUtf8)?;
    let (text, line_ending) = if text.contains("\r\n") {
        (text.replace("\r\n", "\n"), LineEnding::Crlf)
    } else if text.contains('\n') {
        (text, LineEnding::Lf)
    } else {
        (text, LineEnding::Crlf)
    };
    Ok(Loaded {
        text,
        format: Format { line_ending, bom },
    })
}

pub fn save(path: &Path, text: &str, format: Format) -> io::Result<()> {
    let body = match format.line_ending {
        LineEnding::Crlf => text.replace('\n', "\r\n"),
        LineEnding::Lf => text.to_string(),
    };
    let mut bytes = Vec::with_capacity(body.len() + UTF8_BOM.len());
    if format.bom {
        bytes.extend_from_slice(UTF8_BOM);
    }
    bytes.extend_from_slice(body.as_bytes());

    let temp = temp_path(path);
    fs::write(&temp, &bytes)?;
    fs::rename(&temp, path).inspect_err(|_| {
        fs::remove_file(&temp).ok();
    })
}

pub fn display_name(path: Option<&Path>) -> String {
    path.and_then(Path::file_name)
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Untitled".into())
}

pub fn default_directory(path: Option<&Path>) -> PathBuf {
    path.and_then(Path::parent)
        .map(Path::to_path_buf)
        .or_else(|| {
            std::env::var_os("USERPROFILE").map(|home| PathBuf::from(home).join("Documents"))
        })
        .unwrap_or_else(|| PathBuf::from("."))
}

fn temp_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    path.with_file_name(format!(".{name}.jot-save"))
}
