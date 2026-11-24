use std::io;
use std::path::Path;

use crate::APP_NAME;
use crate::document::{LoadError, SaveError, display_name};
use crate::ids::TabId;
use crate::types::SharedStr;

use super::{Prompt, PromptAnswer, PromptId};

const OK_CHOICES: &[(&str, PromptAnswer)] = &[("OK", PromptAnswer::Cancel)];
const DISCARD_CHOICES: &[(&str, PromptAnswer)] = &[
    ("Save", PromptAnswer::Save),
    ("Don't save", PromptAnswer::Discard),
    ("Cancel", PromptAnswer::Cancel),
];
const LOSSY_CHOICES: &[(&str, PromptAnswer)] = &[
    ("OK", PromptAnswer::Continue),
    ("Cancel", PromptAnswer::Cancel),
];
const LOSSY_ANSI_WARNING: &str = "This file contains characters in Unicode format which will be lost if you save this file as an ANSI encoded text file. To keep the Unicode information, click Cancel below and then select one of the Unicode options from the Encoding drop down list. Continue?";

fn friendly_io_error(error: &io::Error) -> String {
    let raw = error.to_string();
    if let Some(prefix) = raw.strip_suffix(')')
        && let Some((clean, suffix)) = prefix.rsplit_once(" (os error ")
        && suffix.chars().all(|c| c.is_ascii_digit())
    {
        return clean.trim_end().to_owned();
    }
    raw
}

fn info_prompt(title: SharedStr, message: SharedStr) -> Prompt {
    Prompt {
        title,
        message,
        choices: OK_CHOICES,
        cancel: PromptAnswer::Cancel,
        reply: None,
    }
}

pub(super) fn load_error_prompt(path: &Path, error: LoadError) -> Prompt {
    let detail: SharedStr = match error {
        LoadError::Undecodable => "Jot can't decode the text in this file.".into(),
        LoadError::TooLarge(bytes) => format!(
            "This file is too large to open ({} MB).",
            bytes / (1024 * 1024)
        )
        .into(),
        LoadError::Io(err) => friendly_io_error(&err).into(),
    };
    info_prompt(
        format!("Can't open {}", display_name(Some(path))).into(),
        detail,
    )
}

pub(super) fn save_error_prompt(path: &Path, error: &SaveError) -> Prompt {
    let detail: SharedStr = match error {
        SaveError::ReadOnly => "This file is read-only.".into(),
        SaveError::Kept {
            source,
            backup,
            temp,
        } => format!(
            "{}\nThe original file was kept as {}\nThe new text was saved as {}",
            friendly_io_error(source),
            backup.display(),
            temp.display(),
        )
        .into(),
        SaveError::Io(err) => friendly_io_error(err).into(),
    };
    info_prompt(
        format!("Can't save {}", display_name(Some(path))).into(),
        detail,
    )
}

pub(super) fn dialog_error_prompt(title: &'static str, message: String) -> Prompt {
    info_prompt(SharedStr::new_static(title), message.into())
}

pub(super) fn already_open_save_prompt(path: &Path) -> Prompt {
    info_prompt(
        format!("Can't save {}", display_name(Some(path))).into(),
        SharedStr::new_static(
            "This file is already open in another tab. Close that tab before saving over it.",
        ),
    )
}

pub(super) fn discard_prompt(id: PromptId, tab: TabId, name: &str) -> Prompt {
    Prompt {
        title: APP_NAME.into(),
        message: format!("Do you want to save changes to {name}?").into(),
        choices: DISCARD_CHOICES,
        cancel: PromptAnswer::Cancel,
        reply: Some((id, tab)),
    }
}

pub(super) fn lossy_ansi_prompt(id: PromptId, tab: TabId, path: &Path) -> Prompt {
    Prompt {
        title: APP_NAME.into(),
        message: format!("{}\n{LOSSY_ANSI_WARNING}", path.display()).into(),
        choices: LOSSY_CHOICES,
        cancel: PromptAnswer::Cancel,
        reply: Some((id, tab)),
    }
}
