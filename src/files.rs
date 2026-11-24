use std::collections::VecDeque;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::APP_NAME;
use crate::app::AppEvent;
use crate::document::{
    self, DocPath, Encoding, Format, LoadError, PathKey, default_directory, display_name, file_name,
};
use crate::editor::Rope;
use crate::platform::{
    Jobs, OpenDialogOptions, SaveChoice, SaveDialogOptions, SaveDialogResult, WindowHwnd, WindowId,
    run_open_dialog, run_save_dialog,
};
use crate::tabs::TabId;
use crate::types::{Error, SharedStr};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct WorkflowGen(pub(crate) u64);

impl WorkflowGen {
    pub(crate) fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

fn write_flight_tracker() -> &'static (Mutex<usize>, Condvar) {
    static TRACKER: OnceLock<(Mutex<usize>, Condvar)> = OnceLock::new();
    TRACKER.get_or_init(|| (Mutex::new(0), Condvar::new()))
}

struct WriteFlightGuard;

impl WriteFlightGuard {
    fn enter() -> Self {
        let (lock, _) = write_flight_tracker();
        let mut count = lock.lock().unwrap_or_else(|e| e.into_inner());
        *count += 1;
        Self
    }
}

impl Drop for WriteFlightGuard {
    fn drop(&mut self) {
        let (lock, cvar) = write_flight_tracker();
        let mut count = lock.lock().unwrap_or_else(|e| e.into_inner());
        *count = count.saturating_sub(1);
        if *count == 0 {
            cvar.notify_all();
        }
    }
}

pub(crate) fn in_flight_writes() -> usize {
    let (lock, _) = write_flight_tracker();
    *lock.lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn wait_for_in_flight_writes(timeout: Duration) {
    let (lock, cvar) = write_flight_tracker();
    let deadline = Instant::now() + timeout;
    let mut count = lock.lock().unwrap_or_else(|e| e.into_inner());
    while *count > 0 {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        let (guard, _) = cvar
            .wait_timeout(count, remaining)
            .unwrap_or_else(|e| e.into_inner());
        count = guard;
    }
}

pub(crate) const UNTITLED_FILE_NAME: &str = "Untitled.txt";
const SAVE_AS_TITLE: &str = "Save as";
const TEXT_FILE_TYPE: (&str, &str) = ("Text documents (*.txt)", "*.txt");
const ALL_FILE_TYPE: (&str, &str) = ("All files", "*.*");
const TEXT_EXTENSION: &str = "txt";
const ENCODING_LABEL: &str = "Encoding:";
pub(crate) const LOSSY_ANSI_WARNING: &str = "This file contains characters in Unicode format which will be lost if you save this file as an ANSI encoded text file. To keep the Unicode information, click Cancel below and then select one of the Unicode options from the Encoding drop down list. Continue?";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum WorkflowStep {
    SaveTab { id: TabId, activate: bool },
    SaveTabAs(TabId),
    ConfirmDiscard { id: TabId, activate: bool },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkflowFinish {
    None,
    RemoveTab(TabId),
    CloseWindow,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct FileWorkflow {
    pub(crate) wf_gen: WorkflowGen,
    pending: VecDeque<WorkflowStep>,
    approved_discards: Vec<(TabId, u64)>,
    pub(crate) on_finish: WorkflowFinish,
}

impl FileWorkflow {
    pub(crate) fn single(step: WorkflowStep, on_finish: WorkflowFinish) -> Self {
        let mut pending = VecDeque::with_capacity(1);
        pending.push_back(step);
        Self {
            wf_gen: WorkflowGen::next(),
            pending,
            approved_discards: Vec::new(),
            on_finish,
        }
    }

    pub(crate) fn batch(
        steps: impl IntoIterator<Item = WorkflowStep>,
        on_finish: WorkflowFinish,
    ) -> Self {
        Self {
            wf_gen: WorkflowGen::next(),
            pending: steps.into_iter().collect(),
            approved_discards: Vec::new(),
            on_finish,
        }
    }

    pub(crate) fn next_step(&mut self) -> Option<WorkflowStep> {
        self.pending.pop_front()
    }

    pub(crate) fn prepend(&mut self, step: WorkflowStep) {
        self.pending.push_front(step);
    }

    pub(crate) fn approve_discard(&mut self, tab: TabId, revision: u64) {
        self.approved_discards.retain(|(id, _)| *id != tab);
        self.approved_discards.push((tab, revision));
    }

    pub(crate) fn is_discard_approved(&self, tab: TabId, revision: u64) -> bool {
        self.approved_discards
            .iter()
            .any(|&(id, rev)| id == tab && rev == revision)
    }

    pub(crate) fn take_approved_discards(&mut self) -> Vec<(TabId, u64)> {
        std::mem::take(&mut self.approved_discards)
    }

    pub(crate) fn with_approved_discards(mut self, approved: Vec<(TabId, u64)>) -> Self {
        self.approved_discards = approved;
        self
    }
}

pub(crate) enum FilePrompt {
    Error {
        title: SharedStr,
        message: SharedStr,
    },
    ConfirmDiscard {
        tab: TabId,
        name: String,
    },
    ConfirmLossyAnsi {
        wf_gen: WorkflowGen,
        tab: TabId,
        snapshot: u64,
        text: Rope,
        path: PathBuf,
        format: Format,
    },
}

pub(crate) fn spawn_open_dialog(
    window: WindowId,
    hwnd: WindowHwnd,
    current: Option<PathBuf>,
    jobs: &Jobs<AppEvent>,
) {
    let owner_raw = hwnd.raw().0 as isize;
    let options = OpenDialogOptions {
        multiple: true,
        directory: Some(default_directory(current.as_deref())),
        file_types: [TEXT_FILE_TYPE, ALL_FILE_TYPE]
            .map(|(name, spec)| (SharedStr::new_static(name), SharedStr::new_static(spec)))
            .into(),
    };
    jobs.spawn(
        "jot-open-dialog",
        move || run_open_dialog(owner_raw, options),
        move |res| AppEvent::OpenDialogDone {
            window,
            result: res.unwrap_or_else(|msg| Err(Error::new(msg))),
        },
    );
}

pub(crate) fn spawn_load_files(
    window: WindowId,
    paths: Vec<DocPath>,
    jobs: &Jobs<AppEvent>,
) {
    let fallback_paths = paths.clone();
    jobs.spawn(
        "jot-load-files",
        move || {
            paths
                .into_iter()
                .map(|doc_path| {
                    let loaded = document::load(doc_path.as_path());
                    (doc_path, loaded)
                })
                .collect::<Vec<_>>()
        },
        move |res| {
            let results = res.unwrap_or_else(|msg| {
                fallback_paths
                    .into_iter()
                    .map(|doc_path| (doc_path, Err(LoadError::Io(io::Error::other(msg.clone())))))
                    .collect()
            });
            AppEvent::FilesLoaded { window, results }
        },
    );
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_save_dialog(
    window: WindowId,
    wf_gen: WorkflowGen,
    hwnd: WindowHwnd,
    tab: TabId,
    current: Option<PathBuf>,
    format: Format,
    jobs: &Jobs<AppEvent>,
) {
    let suggested = file_name(current.as_deref()).unwrap_or_else(|| UNTITLED_FILE_NAME.into());
    let options = SaveDialogOptions {
        title: Some(SAVE_AS_TITLE.into()),
        directory: default_directory(current.as_deref()),
        file_name: Some(suggested),
        file_types: [TEXT_FILE_TYPE, ALL_FILE_TYPE]
            .map(|(name, spec)| (SharedStr::new_static(name), SharedStr::new_static(spec)))
            .into(),
        default_extension: Some(TEXT_EXTENSION.into()),
        choice: Some(SaveChoice {
            label: ENCODING_LABEL.into(),
            items: Encoding::SAVE_ORDER
                .map(|encoding| SharedStr::new_static(encoding.label()))
                .into(),
            selected: Encoding::SAVE_ORDER
                .iter()
                .position(|&encoding| encoding == format.encoding)
                .unwrap_or_default(),
        }),
    };
    let owner_raw = hwnd.raw().0 as isize;
    jobs.spawn(
        "jot-save-dialog",
        move || run_save_dialog(owner_raw, options),
        move |res| AppEvent::SaveDialogDone {
            window,
            wf_gen,
            tab,
            format,
            result: res.unwrap_or_else(|msg| Err(Error::new(msg))),
        },
    );
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_lossy_check(
    window: WindowId,
    wf_gen: WorkflowGen,
    tab: TabId,
    snapshot: u64,
    text: Rope,
    path: PathBuf,
    format: Format,
    jobs: &Jobs<AppEvent>,
) {
    let path = std::path::absolute(&path).unwrap_or(path);
    let check_text = text.clone();
    jobs.spawn(
        "jot-lossy-check",
        move || document::is_lossy(&check_text, format),
        move |res| AppEvent::LossyCheckDone {
            window,
            wf_gen,
            tab,
            snapshot,
            text,
            path,
            format,
            result: res.unwrap_or_else(|msg| Err(io::Error::other(msg))),
        },
    );
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_write_file(
    window: WindowId,
    wf_gen: WorkflowGen,
    tab: TabId,
    snapshot: u64,
    text: Rope,
    path: PathBuf,
    format: Format,
    jobs: &Jobs<AppEvent>,
) {
    let guard = WriteFlightGuard::enter();
    let write_path = path.clone();
    jobs.spawn(
        "jot-write-file",
        move || {
            let _guard = guard;
            let result = document::write_rope(&write_path, &text, format);
            let key = if result.is_ok() {
                PathKey::canonical(&write_path)
            } else {
                PathKey::quick(&write_path)
            };
            (key, result)
        },
        move |res| {
            let (key, result) =
                res.unwrap_or_else(|msg| (PathKey::quick(&path), Err(io::Error::other(msg))));
            let saved_format = if result.is_ok() {
                Format {
                    mixed_line_endings: false,
                    lossy_decode: false,
                    ..format
                }
            } else {
                format
            };
            AppEvent::FileSaved {
                window,
                wf_gen,
                tab,
                snapshot,
                doc_path: DocPath { path, key },
                format: saved_format,
                result,
            }
        },
    );
}

pub(crate) fn resolve_save_dialog(
    format: Format,
    result: SaveDialogResult,
) -> std::result::Result<Option<(PathBuf, Format)>, String> {
    match result {
        Ok(Some((path, choice))) => {
            let next_format = Format {
                encoding: choice
                    .and_then(|ix| Encoding::SAVE_ORDER.get(ix).copied())
                    .unwrap_or(format.encoding),
                lossy_decode: false,
                ..format
            };
            Ok(Some((path, next_format)))
        }
        Ok(None) => Ok(None),
        Err(err) => Err(err.to_string()),
    }
}

pub(crate) fn friendly_io_error(error: &io::Error) -> String {
    let raw = error.to_string();
    if let Some(prefix) = raw.strip_suffix(')')
        && let Some((clean, suffix)) = prefix.rsplit_once(" (os error ")
        && suffix.chars().all(|c| c.is_ascii_digit())
    {
        return clean.trim_end().to_owned();
    }
    raw
}

pub(crate) fn load_error_prompt(path: &Path, error: LoadError) -> FilePrompt {
    let detail: SharedStr = match error {
        LoadError::Undecodable => "Jot can't decode the text in this file.".into(),
        LoadError::TooLarge(bytes) => format!(
            "This file is too large to open ({} MB).",
            bytes / (1024 * 1024)
        )
        .into(),
        LoadError::Io(err) => friendly_io_error(&err).into(),
    };
    FilePrompt::Error {
        title: format!("Can't open {}", display_name(Some(path))).into(),
        message: detail,
    }
}

pub(crate) fn save_error_prompt(path: &Path, error: &io::Error) -> FilePrompt {
    FilePrompt::Error {
        title: format!("Can't save {}", display_name(Some(path))).into(),
        message: friendly_io_error(error).into(),
    }
}

pub(crate) fn dialog_error_prompt(title: &'static str, message: String) -> FilePrompt {
    FilePrompt::Error {
        title: SharedStr::new_static(title),
        message: message.into(),
    }
}

pub(crate) fn already_open_save_prompt(path: &Path) -> FilePrompt {
    FilePrompt::Error {
        title: format!("Can't save {}", display_name(Some(path))).into(),
        message: SharedStr::new_static(
            "This file is already open in another tab. Close that tab before saving over it.",
        ),
    }
}

pub(crate) fn discard_prompt_text(name: &str) -> (SharedStr, SharedStr) {
    (
        APP_NAME.into(),
        format!("Do you want to save changes to {name}?").into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_workflow_queues_prepends_and_drains_steps_in_order() {
        let id1 = TabId::next();
        let id2 = TabId::next();
        let mut wf = FileWorkflow::batch(
            [
                WorkflowStep::ConfirmDiscard {
                    id: id1,
                    activate: true,
                },
                WorkflowStep::ConfirmDiscard {
                    id: id2,
                    activate: true,
                },
            ],
            WorkflowFinish::CloseWindow,
        );

        assert_eq!(
            wf.next_step(),
            Some(WorkflowStep::ConfirmDiscard {
                id: id1,
                activate: true,
            })
        );
        wf.prepend(WorkflowStep::SaveTab {
            id: id1,
            activate: false,
        });
        assert_eq!(
            wf.next_step(),
            Some(WorkflowStep::SaveTab {
                id: id1,
                activate: false,
            })
        );
        assert_eq!(
            wf.next_step(),
            Some(WorkflowStep::ConfirmDiscard {
                id: id2,
                activate: true,
            })
        );
        assert_eq!(wf.next_step(), None);
        assert_eq!(wf.on_finish, WorkflowFinish::CloseWindow);
    }
}
