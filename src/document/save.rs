use std::ffi::OsString;
use std::fs;
use std::io::{self, BufWriter, Write};
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use ropey::Rope;

use crate::platform::{
    HIDDEN_FILE_ATTRIBUTE, ReplaceFileError, file_hard_link_count, replace_file,
};

use super::encode::stream_encode;
use super::{Format, IO_CHUNK, SaveError};

const TEMP_ATTEMPTS: usize = 100;
const REPLACE_RETRIES: usize = 4;
const REPLACE_RETRY_DELAY: Duration = Duration::from_millis(25);
const MAX_SIBLING_STEM_CHARS: usize = 200;
const TEMP_SUFFIX: &str = ".jot-save";
pub(super) const BACKUP_SUFFIX: &str = ".jot-backup";

fn write_in_place(target: &Path, rope: &Rope, format: Format) -> io::Result<()> {
    let file = fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(target)?;
    let mut writer = BufWriter::with_capacity(IO_CHUNK, file);
    stream_encode(rope, format, &mut writer)?;
    writer.flush()?;
    let file = writer.into_inner().map_err(|e| e.into_error())?;
    file.sync_all()
}

pub(crate) fn write_rope(path: &Path, rope: &Rope, format: Format) -> Result<(), SaveError> {
    let target = resolve(path)?;
    let existing = match fs::metadata(&target) {
        Ok(metadata) if metadata.permissions().readonly() => return Err(SaveError::ReadOnly),
        Ok(_) => true,
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => return Err(error.into()),
    };

    if existing && file_hard_link_count(&target).unwrap_or(1) > 1 {
        return write_in_place(&target, rope, format).map_err(SaveError::from);
    }

    let (temp, file) = match create_temp(&target) {
        Ok(pair) => pair,
        Err(err) if existing && err.kind() == io::ErrorKind::PermissionDenied => {
            return write_in_place(&target, rope, format).map_err(SaveError::from);
        }
        Err(err) => return Err(err.into()),
    };

    let written = {
        let mut writer = BufWriter::with_capacity(IO_CHUNK, file);
        stream_encode(rope, format, &mut writer)
            .and_then(|()| writer.flush())
            .and_then(|()| writer.into_inner().map_err(|e| e.into_error()))
            .and_then(|file| file.sync_all())
    };
    if let Err(error) = written {
        fs::remove_file(&temp).ok();
        return Err(error.into());
    }
    if existing {
        replace(path, &target, &temp, rope, format)
    } else {
        fs::rename(&temp, &target)
            .inspect_err(|_| {
                fs::remove_file(&temp).ok();
            })
            .map_err(SaveError::from)
    }
}

fn resolve(path: &Path) -> io::Result<PathBuf> {
    match fs::canonicalize(path) {
        Ok(resolved) => Ok(resolved),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let parent = path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            let resolved_parent = fs::canonicalize(parent)
                .or_else(|_| std::path::absolute(parent))?;
            Ok(resolved_parent.join(path.file_name().unwrap_or_default()))
        }
        Err(_) => std::path::absolute(path),
    }
}

fn replace(
    path: &Path,
    target: &Path,
    temp: &Path,
    rope: &Rope,
    format: Format,
) -> Result<(), SaveError> {
    let mut last_err = None;
    for attempt in 0..REPLACE_RETRIES {
        let backup = free_sibling(target, BACKUP_SUFFIX)?;
        match replace_file(target, temp, &backup) {
            Ok(()) => {
                fs::remove_file(&backup).ok();
                return Ok(());
            }
            Err(ReplaceFileError::SharingOrAccess(error)) => {
                fs::remove_file(&backup).ok();
                last_err = Some(error);
                if attempt + 1 < REPLACE_RETRIES {
                    thread::sleep(REPLACE_RETRY_DELAY);
                }
            }
            Err(ReplaceFileError::UnableToMoveReplacement(source))
                if fs::rename(&backup, target).is_err() =>
            {
                let shown =
                    |sibling: &Path| path.with_file_name(sibling.file_name().unwrap_or_default());
                return Err(SaveError::Kept {
                    source,
                    backup: shown(&backup),
                    temp: shown(temp),
                });
            }
            Err(
                ReplaceFileError::UnableToMoveReplacement(error) | ReplaceFileError::Other(error),
            ) => {
                fs::remove_file(temp).ok();
                return Err(error.into());
            }
        }
    }
    fs::remove_file(temp).ok();
    write_in_place(target, rope, format)
        .map_err(|_| SaveError::Io(last_err.unwrap_or_else(|| io::Error::other("replace failed"))))
}

fn free_sibling(path: &Path, suffix: &str) -> io::Result<PathBuf> {
    (0..TEMP_ATTEMPTS)
        .map(|attempt| sibling(path, attempt, suffix))
        .find(|candidate| fs::symlink_metadata(candidate).is_err())
        .ok_or_else(|| io::Error::from(io::ErrorKind::AlreadyExists))
}

fn create_temp(path: &Path) -> io::Result<(PathBuf, fs::File)> {
    for attempt in 0..TEMP_ATTEMPTS {
        let temp = sibling(path, attempt, TEMP_SUFFIX);
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .attributes(HIDDEN_FILE_ATTRIBUTE)
            .open(&temp)
        {
            Ok(file) => return Ok((temp, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::from(io::ErrorKind::AlreadyExists))
}

pub(super) fn sibling(path: &Path, attempt: usize, suffix: &str) -> PathBuf {
    let base = path
        .file_name()
        .map(|n| n.to_string_lossy())
        .unwrap_or_default();
    let truncated: String = base.chars().take(MAX_SIBLING_STEM_CHARS).collect();
    let mut name = OsString::from(".");
    name.push(truncated);
    if attempt > 0 {
        name.push(format!(".{attempt}"));
    }
    name.push(suffix);
    path.with_file_name(name)
}
