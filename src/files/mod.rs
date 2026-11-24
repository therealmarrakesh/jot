mod io;
mod prompt;
mod workflow;

pub(crate) use io::spawn_file_io;
pub(crate) use workflow::FileOps;

use std::path::PathBuf;

use crate::document::{self, DocPath, Format, LoadError, PathKey, SaveError};
use crate::editor::Rope;
use crate::ids::TabId;
use crate::platform::{SaveDialogResult, WindowId};
use crate::types::SharedStr;

crate::types::id_type!(PromptId);
crate::types::id_type!(WorkflowId);

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum PromptAnswer {
    Save,
    Discard,
    Continue,
    Cancel,
}

#[derive(Clone, Debug)]
pub(crate) struct Prompt {
    pub(crate) title: SharedStr,
    pub(crate) message: SharedStr,
    pub(crate) choices: &'static [(&'static str, PromptAnswer)],
    pub(crate) cancel: PromptAnswer,
    pub(crate) reply: Option<(PromptId, TabId)>,
}

impl Prompt {
    pub(crate) fn tab(&self) -> Option<TabId> {
        self.reply.map(|(_, tab)| tab)
    }
}

pub(crate) enum FileRequest {
    OpenDialog { window: WindowId },
    OpenPaths { window: WindowId, paths: Vec<PathBuf> },
    Save { window: WindowId, tab: TabId },
    SaveAs { window: WindowId, tab: TabId },
    SaveAll { window: WindowId },
    CloseTab { window: WindowId, tab: TabId },
    CloseWindow { window: WindowId },
}

pub(crate) enum WriteOutcome {
    Saved { doc_path: DocPath, format: Format },
    NeedsLossyConfirm,
    Error(SaveError),
}

pub(crate) enum FileEvent {
    OpenDialogDone {
        window: WindowId,
        result: crate::types::Result<Option<Vec<PathBuf>>>,
    },
    FilesLoaded {
        window: WindowId,
        results: Vec<(DocPath, Result<document::Loaded, LoadError>)>,
    },
    SaveDialogDone {
        wf: WorkflowId,
        result: SaveDialogResult,
    },
    WriteDone {
        wf: WorkflowId,
        outcome: WriteOutcome,
    },
}

#[derive(Clone)]
pub(crate) struct SaveJob {
    pub(crate) tab: TabId,
    pub(crate) snapshot: u64,
    pub(crate) text: Rope,
    pub(crate) path: PathBuf,
    pub(crate) format: Format,
}

pub(crate) enum FileIo {
    OpenDialog {
        window: WindowId,
        current: Option<PathBuf>,
    },
    LoadFiles {
        window: WindowId,
        paths: Vec<DocPath>,
    },
    SaveDialog {
        wf: WorkflowId,
        owner: WindowId,
        current: Option<PathBuf>,
        format: Format,
    },
    WriteFile {
        wf: WorkflowId,
        job: SaveJob,
        force_lossy: bool,
    },
}

pub(crate) struct TabSnapshot {
    pub(crate) window: WindowId,
    pub(crate) name: String,
    pub(crate) path: Option<PathBuf>,
    pub(crate) format: Format,
    pub(crate) dirty: bool,
    pub(crate) revision: u64,
}

pub(crate) trait DocHost {
    fn tab_info(&self, tab: TabId) -> Option<TabSnapshot>;
    fn window_dirty_tabs(&self, window: WindowId) -> Vec<TabId>;
    fn active_tab_path(&self, window: WindowId) -> Option<PathBuf>;
    fn fallback_window(&self, preferred: WindowId) -> Option<WindowId>;
    fn find_tab_by_key(&self, key: &PathKey) -> Option<(WindowId, TabId)>;
    fn activate_tab(&mut self, window: WindowId, tab: TabId, bring_to_front: bool);
    fn begin_save(&mut self, tab: TabId) -> Option<(u64, Rope)>;
    fn mark_saved(&mut self, tab: TabId, snapshot: u64, doc_path: DocPath, format: Format);
    fn adopt_loaded(&mut self, window: WindowId, loaded: document::Loaded);
    fn remove_tab(&mut self, tab: TabId);
    fn mark_window_close(&mut self, window: WindowId);
    fn show_prompt(&mut self, window: WindowId, prompt: Prompt);
}
