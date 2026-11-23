//! Explicit multi-step file workflow state machine for Open, Save, Save As, Save All,
//! lossy-ANSI checks, and dirty-tab/window close prompts.

use std::collections::VecDeque;
use std::io;
use std::path::{Path, PathBuf};

use crate::APP_NAME;
use crate::app::AppEvent;
use crate::document::{self, Encoding, Format, LoadError, default_directory, display_name, file_name};
use crate::editor::Rope;
use crate::platform::{
    Jobs, OpenDialogOptions, SaveChoice, SaveDialogOptions, SaveDialogResult, WindowHwnd, WindowId,
    run_open_dialog, run_save_dialog,
};
use crate::tabs::TabId;
use crate::types::SharedStr;

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
    pending: VecDeque<WorkflowStep>,
    pub(crate) on_finish: WorkflowFinish,
}

impl FileWorkflow {
    pub(crate) fn single(step: WorkflowStep, on_finish: WorkflowFinish) -> Self {
        let mut pending = VecDeque::with_capacity(1);
        pending.push_back(step);
        Self { pending, on_finish }
    }

    pub(crate) fn batch(
        steps: impl IntoIterator<Item = WorkflowStep>,
        on_finish: WorkflowFinish,
    ) -> Self {
        Self {
            pending: steps.into_iter().collect(),
            on_finish,
        }
    }

    pub(crate) fn next_step(&mut self) -> Option<WorkflowStep> {
        self.pending.pop_front()
    }

    pub(crate) fn prepend(&mut self, step: WorkflowStep) {
        self.pending.push_front(step);
    }
}

/// Typed file prompt data passed to the modal dialog layer (zero boxed callbacks).
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
        tab: TabId,
        snapshot: u64,
        text: Rope,
        path: PathBuf,
        format: Format,
    },
}

pub(crate) fn spawn_open_dialog(window: WindowId, jobs: &Jobs<AppEvent>) {
    jobs.spawn_dedicated("jot-open-dialog", move || {
        let result = run_open_dialog(OpenDialogOptions {
            directories: false,
            multiple: true,
            prompt: None,
        });
        AppEvent::OpenDialogDone { window, result }
    });
}

pub(crate) fn spawn_load_files(window: WindowId, paths: Vec<PathBuf>, jobs: &Jobs<AppEvent>) {
    jobs.spawn_bg(move || {
        let results = paths
            .into_iter()
            .map(|path| {
                let loaded = document::load(&path);
                (path, loaded)
            })
            .collect();
        AppEvent::FilesLoaded { window, results }
    });
}

pub(crate) fn spawn_save_dialog(
    window: WindowId,
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
    jobs.spawn_dedicated("jot-save-dialog", move || {
        let result = run_save_dialog(owner_raw, options);
        AppEvent::SaveDialogDone {
            window,
            tab,
            format,
            result,
        }
    });
}

pub(crate) fn spawn_lossy_check(
    window: WindowId,
    tab: TabId,
    snapshot: u64,
    text: Rope,
    path: PathBuf,
    format: Format,
    jobs: &Jobs<AppEvent>,
) {
    let path = std::path::absolute(&path).unwrap_or(path);
    jobs.spawn_bg(move || {
        let result = document::is_lossy(&text, format);
        AppEvent::LossyCheckDone {
            window,
            tab,
            snapshot,
            text,
            path,
            format,
            result,
        }
    });
}

pub(crate) fn spawn_write_file(
    window: WindowId,
    tab: TabId,
    snapshot: u64,
    text: Rope,
    path: PathBuf,
    format: Format,
    jobs: &Jobs<AppEvent>,
) {
    jobs.spawn_bg(move || {
        let result = document::write_rope(&path, &text, format);
        AppEvent::FileSaved {
            window,
            tab,
            snapshot,
            path,
            format,
            result,
        }
    });
}

pub(crate) fn resolve_save_dialog(
    format: Format,
    result: SaveDialogResult,
) -> Option<(PathBuf, Format)> {
    let Ok(Some((path, choice))) = result else {
        return None;
    };
    let next_format = Format {
        encoding: choice
            .and_then(|ix| Encoding::SAVE_ORDER.get(ix).copied())
            .unwrap_or(format.encoding),
        ..format
    };
    Some((path, next_format))
}

pub(crate) fn load_error_prompt(path: &Path, error: LoadError) -> FilePrompt {
    let detail: SharedStr = match error {
        LoadError::Undecodable => "Jot can't decode the text in this file.".into(),
        LoadError::Io(err) => err.to_string().into(),
    };
    FilePrompt::Error {
        title: format!("Can't open {}", display_name(Some(path))).into(),
        message: detail,
    }
}

pub(crate) fn save_error_prompt(path: &Path, error: &io::Error) -> FilePrompt {
    FilePrompt::Error {
        title: format!("Can't save {}", display_name(Some(path))).into(),
        message: error.to_string().into(),
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
        let id1 = TabId(10);
        let id2 = TabId(20);
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
