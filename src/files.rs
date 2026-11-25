use std::collections::{HashSet, VecDeque};
use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use crate::document::{
    self, DocPath, Encoding, Format, LoadError, PathKey, SaveError, default_directory,
    display_name, file_name,
};
use crate::text::Rope;
use crate::platform::{
    OpenDialogOptions, SaveChoice, SaveDialogOptions, SaveDialogResult, Waker, WindowHandle,
    WindowId, panic_message, run_open_dialog, run_save_dialog,
};
use crate::types::{APP_NAME, Error, SharedStr};
use crate::ui::TabId;

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

    /// What choosing button `button` (or, with `None`, dismissing the prompt) answers.
    pub(crate) fn answer(&self, button: Option<usize>) -> PromptAnswer {
        button
            .and_then(|ix| self.choices.get(ix))
            .map_or(self.cancel, |&(_, answer)| answer)
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
    Saved { format: Format },
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
    pub(crate) path: DocPath,
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
        current: Option<DocPath>,
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
    pub(crate) path: Option<DocPath>,
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
    /// The window's handle, to own the file dialogs it asked for.
    fn window_handle(&self, window: WindowId) -> Option<WindowHandle>;
}

enum Goal {
    Save(Option<TabId>),
    SaveAs(Option<TabId>),
    SaveAll(VecDeque<TabId>),
    CloseTab(TabId),
    CloseWindow(VecDeque<TabId>),
}

enum Awaiting {
    Idle,
    DiscardPrompt {
        prompt: PromptId,
        tab: TabId,
    },
    SaveDialog {
        tab: TabId,
        format: Format,
    },
    QueuedWrite {
        tab: TabId,
        path: DocPath,
        format: Format,
    },
    Write(SaveJob),
    LossyPrompt { prompt: PromptId, job: SaveJob },
}

enum NextAction {
    Save { tab: TabId, activate: bool },
    SaveAs(TabId),
    ConfirmDiscard { tab: TabId, activate: bool },
    RemoveTab(TabId),
    CloseWindow(WindowId),
    Done,
}

struct Workflow {
    id: WorkflowId,
    origin: WindowId,
    goal: Goal,
    save_next: Option<TabId>,
    approved: Vec<(TabId, u64)>,
    awaiting: Awaiting,
}

impl Workflow {
    fn new(origin: WindowId, goal: Goal) -> Self {
        Self {
            id: WorkflowId::next(),
            origin,
            goal,
            save_next: None,
            approved: Vec::new(),
            awaiting: Awaiting::Idle,
        }
    }

    fn approve_discard(&mut self, tab: TabId, revision: u64) {
        self.approved.retain(|(id, _)| *id != tab);
        self.approved.push((tab, revision));
    }

    fn is_discard_approved(&self, tab: TabId, revision: u64) -> bool {
        self.approved
            .iter()
            .any(|&(id, rev)| id == tab && rev == revision)
    }

    fn next_step(&mut self, host: &impl DocHost) -> NextAction {
        if let Some(tab) = self.save_next.take() {
            return NextAction::Save { tab, activate: false };
        }
        match &mut self.goal {
            Goal::Save(slot) => slot
                .take()
                .map_or(NextAction::Done, |tab| NextAction::Save { tab, activate: false }),
            Goal::SaveAs(slot) => slot.take().map_or(NextAction::Done, NextAction::SaveAs),
            Goal::SaveAll(queue) => queue
                .pop_front()
                .map_or(NextAction::Done, |tab| NextAction::Save { tab, activate: true }),
            Goal::CloseTab(tab) => {
                let tab = *tab;
                let Some(info) = host.tab_info(tab) else {
                    return NextAction::Done;
                };
                if info.dirty && !self.is_discard_approved(tab, info.revision) {
                    NextAction::ConfirmDiscard { tab, activate: false }
                } else {
                    NextAction::RemoveTab(tab)
                }
            }
            Goal::CloseWindow(pending) => {
                let approved = &self.approved;
                let unapproved = |id: TabId, rev: u64| {
                    !approved.iter().any(|&(aid, arev)| aid == id && arev == rev)
                };
                loop {
                    if let Some(tab) = pending.pop_front() {
                        if let Some(info) = host.tab_info(tab)
                            && info.dirty
                            && unapproved(tab, info.revision)
                        {
                            return NextAction::ConfirmDiscard { tab, activate: true };
                        }
                        continue;
                    }
                    let remaining: VecDeque<TabId> = host
                        .window_dirty_tabs(self.origin)
                        .into_iter()
                        .filter(|&id| host.tab_info(id).is_some_and(|t| unapproved(id, t.revision)))
                        .collect();
                    if remaining.is_empty() {
                        return NextAction::CloseWindow(self.origin);
                    }
                    *pending = remaining;
                }
            }
        }
    }
}

#[derive(Default)]
pub(crate) struct FileOps {
    flows: Vec<Workflow>,
    writing: HashSet<TabId>,
    loading: HashSet<PathKey>,
    open_dialogs: HashSet<WindowId>,
    pub(crate) writes: WriteTracker,
}

impl FileOps {
    /// Starts `io`, the file work requests and events asked for, on worker threads; it reports
    /// back through `events`.
    pub(crate) fn spawn(&self, io: Vec<FileIo>, host: &impl DocHost, events: &FileEvents) {
        for cmd in io {
            let owner = match &cmd {
                FileIo::OpenDialog { window, .. } | FileIo::SaveDialog { owner: window, .. } => {
                    host.window_handle(*window)
                }
                FileIo::LoadFiles { .. } | FileIo::WriteFile { .. } => None,
            };
            spawn_file_io(cmd, owner, &self.writes, events);
        }
    }

    pub(crate) fn has_active_workflow_for(&self, window: WindowId) -> bool {
        self.flows.iter().any(|f| f.origin == window)
    }

    pub(crate) fn on_window_closed(&mut self, window: WindowId, cancelled_prompts: &[PromptId]) {
        self.open_dialogs.remove(&window);
        let mut freed_tabs = Vec::new();
        self.flows.retain(|f| {
            let prompt_cancelled = match &f.awaiting {
                Awaiting::DiscardPrompt { prompt, .. } | Awaiting::LossyPrompt { prompt, .. } => {
                    cancelled_prompts.contains(prompt)
                }
                _ => false,
            };
            let drop_flow = prompt_cancelled
                || (f.origin == window && !matches!(f.awaiting, Awaiting::Write(_)));
            if drop_flow
                && let Awaiting::LossyPrompt { job, .. } = &f.awaiting
            {
                freed_tabs.push(job.tab);
            }
            !drop_flow
        });
        for tab in freed_tabs {
            self.writing.remove(&tab);
        }
    }

    pub(crate) fn request(
        &mut self,
        req: FileRequest,
        host: &mut impl DocHost,
        io: &mut Vec<FileIo>,
    ) {
        match req {
            FileRequest::OpenDialog { window } => {
                if self.open_dialogs.insert(window) {
                    io.push(FileIo::OpenDialog {
                        window,
                        current: host.active_tab_path(window),
                    });
                }
            }
            FileRequest::OpenPaths { window, paths } => {
                self.open_paths(window, paths, host, io);
            }
            FileRequest::Save { window, tab } => {
                let Some(info) = host.tab_info(tab) else {
                    return;
                };
                if info.path.is_some() && !info.dirty && !info.format.lossy_decode {
                    return;
                }
                self.start_workflow(Workflow::new(window, Goal::Save(Some(tab))), host, io);
            }
            FileRequest::SaveAs { window, tab } => {
                self.start_workflow(Workflow::new(window, Goal::SaveAs(Some(tab))), host, io);
            }
            FileRequest::SaveAll { window } => {
                let dirty = host.window_dirty_tabs(window);
                if dirty.is_empty() {
                    return;
                }
                self.start_workflow(Workflow::new(window, Goal::SaveAll(dirty.into())), host, io);
            }
            FileRequest::CloseTab { window, tab } => {
                let Some(info) = host.tab_info(tab) else {
                    return;
                };
                if !info.dirty {
                    self.remove_tab_and_prune(tab, host);
                    return;
                }
                host.activate_tab(info.window, tab, false);
                self.start_workflow(Workflow::new(window, Goal::CloseTab(tab)), host, io);
            }
            FileRequest::CloseWindow { window } => {
                let dirty = host.window_dirty_tabs(window);
                if dirty.is_empty() {
                    host.mark_window_close(window);
                    return;
                }
                if self
                    .flows
                    .iter()
                    .any(|f| f.origin == window && matches!(f.goal, Goal::CloseWindow(_)))
                {
                    return;
                }
                self.start_workflow(
                    Workflow::new(window, Goal::CloseWindow(dirty.into())),
                    host,
                    io,
                );
            }
        }
    }

    pub(crate) fn on_prompt(
        &mut self,
        prompt_id: PromptId,
        answer: PromptAnswer,
        host: &mut impl DocHost,
        io: &mut Vec<FileIo>,
    ) {
        let Some(ix) = self.flows.iter().position(|f| match &f.awaiting {
            Awaiting::DiscardPrompt { prompt, .. } | Awaiting::LossyPrompt { prompt, .. } => {
                *prompt == prompt_id
            }
            _ => false,
        }) else {
            return;
        };
        let wf_id = self.flows[ix].id;
        let awaiting = std::mem::replace(&mut self.flows[ix].awaiting, Awaiting::Idle);
        match awaiting {
            Awaiting::DiscardPrompt { tab, .. } => match answer {
                PromptAnswer::Save => {
                    self.flows[ix].save_next = Some(tab);
                    self.advance(wf_id, host, io);
                }
                PromptAnswer::Discard => {
                    if let Some(info) = host.tab_info(tab) {
                        self.flows[ix].approve_discard(tab, info.revision);
                    }
                    self.advance(wf_id, host, io);
                }
                PromptAnswer::Continue | PromptAnswer::Cancel => {
                    self.flows.remove(ix);
                }
            },
            Awaiting::LossyPrompt { job, .. } => match answer {
                PromptAnswer::Continue => {
                    self.flows[ix].awaiting = Awaiting::Write(job.clone());
                    io.push(FileIo::WriteFile {
                        wf: wf_id,
                        job,
                        force_lossy: true,
                    });
                }
                PromptAnswer::Cancel | PromptAnswer::Save | PromptAnswer::Discard => {
                    let tab = job.tab;
                    self.writing.remove(&tab);
                    self.begin_save_as(wf_id, tab, host, io);
                    self.wake_queued_write(tab, host, io);
                }
            },
            _ => {}
        }
    }

    pub(crate) fn on_event(
        &mut self,
        event: FileEvent,
        host: &mut impl DocHost,
        io: &mut Vec<FileIo>,
    ) {
        match event {
            FileEvent::OpenDialogDone { window, result } => {
                self.open_dialogs.remove(&window);
                let Some(target) = host.fallback_window(window) else {
                    return;
                };
                match result {
                    Ok(Some(paths)) => self.open_paths(target, paths, host, io),
                    Ok(None) => {}
                    Err(err) => {
                        host.show_prompt(
                            target,
                            dialog_error_prompt("Can't open file", err.to_string()),
                        );
                    }
                }
            }
            FileEvent::FilesLoaded { window, results } => {
                for (doc_path, res) in results {
                    self.loading.remove(doc_path.key());
                    if let Some((win, tab)) = host.find_tab_by_key(doc_path.key()) {
                        host.activate_tab(win, tab, true);
                        continue;
                    }
                    if let Some(target) = host.fallback_window(window) {
                        match res {
                            Ok(loaded) => host.adopt_loaded(target, loaded),
                            Err(err) => {
                                host.show_prompt(
                                    target,
                                    load_error_prompt(doc_path.as_path(), err),
                                );
                            }
                        }
                    }
                }
            }
            FileEvent::SaveDialogDone { wf, result } => {
                let Some(ix) = self.flow_ix(wf) else {
                    return;
                };
                let origin = self.flows[ix].origin;
                let Awaiting::SaveDialog { tab, format } =
                    std::mem::replace(&mut self.flows[ix].awaiting, Awaiting::Idle)
                else {
                    return;
                };
                match resolve_save_dialog(format, result) {
                    Ok(Some((path, next_format))) => {
                        let path = DocPath::new(path);
                        let conflict = host
                            .find_tab_by_key(path.key())
                            .is_some_and(|(_, existing)| existing != tab);
                        if conflict {
                            let prompt = already_open_save_prompt(path.as_path());
                            self.show_prompt_for(tab, origin, prompt, host);
                            self.remove_flow(wf);
                        } else {
                            self.begin_write(wf, tab, path, next_format, host, io);
                        }
                    }
                    Ok(None) => self.remove_flow(wf),
                    Err(msg) => {
                        self.show_prompt_for(
                            tab,
                            origin,
                            dialog_error_prompt("Can't save file", msg),
                            host,
                        );
                        self.remove_flow(wf);
                    }
                }
            }
            FileEvent::WriteDone { wf, outcome } => {
                let Some(ix) = self.flow_ix(wf) else {
                    return;
                };
                let origin = self.flows[ix].origin;
                let Awaiting::Write(job) =
                    std::mem::replace(&mut self.flows[ix].awaiting, Awaiting::Idle)
                else {
                    return;
                };
                match outcome {
                    WriteOutcome::Saved { format } => {
                        let tab = job.tab;
                        self.writing.remove(&tab);
                        host.mark_saved(tab, job.snapshot, job.path, format);
                        self.advance(wf, host, io);
                        self.wake_queued_write(tab, host, io);
                    }
                    WriteOutcome::NeedsLossyConfirm => {
                        let Some(target) = self.prompt_window(job.tab, origin, host) else {
                            let tab = job.tab;
                            self.writing.remove(&tab);
                            self.remove_flow(wf);
                            self.wake_queued_write(tab, host, io);
                            return;
                        };
                        let prompt_id = PromptId::next();
                        let prompt = lossy_ansi_prompt(prompt_id, job.tab, job.path.as_path());
                        self.flows[ix].awaiting = Awaiting::LossyPrompt {
                            prompt: prompt_id,
                            job,
                        };
                        host.show_prompt(target, prompt);
                    }
                    WriteOutcome::Error(err) => {
                        let tab = job.tab;
                        self.writing.remove(&tab);
                        let prompt = save_error_prompt(job.path.as_path(), &err);
                        self.show_prompt_for(tab, origin, prompt, host);
                        self.remove_flow(wf);
                        self.wake_queued_write(tab, host, io);
                    }
                }
            }
        }
    }

    fn open_paths(
        &mut self,
        window: WindowId,
        paths: Vec<PathBuf>,
        host: &mut impl DocHost,
        io: &mut Vec<FileIo>,
    ) {
        if paths.is_empty() {
            return;
        }
        let mut to_load: Vec<DocPath> = Vec::new();
        let mut last_existing = None;
        for raw in paths {
            let doc_path = DocPath::new(raw);
            if let Some(found) = host.find_tab_by_key(doc_path.key()) {
                last_existing = Some(found);
                continue;
            }
            if self.loading.contains(doc_path.key())
                || to_load.iter().any(|q| q.key() == doc_path.key())
            {
                continue;
            }
            self.loading.insert(doc_path.key().clone());
            to_load.push(doc_path);
        }
        if to_load.is_empty() {
            if let Some((win, tab)) = last_existing {
                host.activate_tab(win, tab, true);
            }
            return;
        }
        io.push(FileIo::LoadFiles {
            window,
            paths: to_load,
        });
    }

    fn start_workflow(
        &mut self,
        wf: Workflow,
        host: &mut impl DocHost,
        io: &mut Vec<FileIo>,
    ) {
        let id = wf.id;
        self.flows.push(wf);
        self.advance(id, host, io);
    }

    fn flow_ix(&self, id: WorkflowId) -> Option<usize> {
        self.flows.iter().position(|f| f.id == id)
    }

    fn remove_flow(&mut self, id: WorkflowId) {
        if let Some(ix) = self.flow_ix(id) {
            self.flows.remove(ix);
        }
    }

    fn remove_tab_and_prune(&mut self, tab: TabId, host: &mut impl DocHost) {
        host.remove_tab(tab);
        self.flows.retain(|f| match &f.awaiting {
            Awaiting::DiscardPrompt { tab: t, .. }
            | Awaiting::LossyPrompt {
                job: SaveJob { tab: t, .. },
                ..
            } => *t != tab,
            _ => true,
        });
    }

    fn wake_queued_write(&mut self, tab: TabId, host: &mut impl DocHost, io: &mut Vec<FileIo>) {
        if self.writing.contains(&tab) {
            return;
        }
        let Some(ix) = self.flows.iter().position(|f| {
            matches!(&f.awaiting, Awaiting::QueuedWrite { tab: t, .. } if *t == tab)
        }) else {
            return;
        };
        let wf = self.flows[ix].id;
        let Awaiting::QueuedWrite { tab, path, format } =
            std::mem::replace(&mut self.flows[ix].awaiting, Awaiting::Idle)
        else {
            unreachable!();
        };
        self.begin_write(wf, tab, path, format, host, io);
    }

    fn prompt_window(&self, tab: TabId, origin: WindowId, host: &impl DocHost) -> Option<WindowId> {
        host.tab_info(tab)
            .map(|t| t.window)
            .or_else(|| host.fallback_window(origin))
    }

    fn show_prompt_for(
        &self,
        tab: TabId,
        origin: WindowId,
        prompt: Prompt,
        host: &mut impl DocHost,
    ) {
        if let Some(win) = self.prompt_window(tab, origin, host) {
            host.show_prompt(win, prompt);
        }
    }

    fn advance(&mut self, wf: WorkflowId, host: &mut impl DocHost, io: &mut Vec<FileIo>) {
        let Some(ix) = self.flow_ix(wf) else {
            return;
        };
        match self.flows[ix].next_step(host) {
            NextAction::Done => {
                self.flows.remove(ix);
            }
            NextAction::RemoveTab(tab) => {
                self.flows.remove(ix);
                self.remove_tab_and_prune(tab, host);
            }
            NextAction::CloseWindow(window) => {
                self.flows.remove(ix);
                host.mark_window_close(window);
            }
            NextAction::Save { tab, activate } => {
                let Some(info) = host.tab_info(tab) else {
                    self.flows.remove(ix);
                    return;
                };
                if activate {
                    host.activate_tab(info.window, tab, false);
                }
                match info.path {
                    Some(path) => self.begin_write(wf, tab, path, info.format, host, io),
                    None => self.begin_save_as(wf, tab, host, io),
                }
            }
            NextAction::SaveAs(tab) => {
                self.begin_save_as(wf, tab, host, io);
            }
            NextAction::ConfirmDiscard { tab, activate } => {
                let Some(info) = host.tab_info(tab) else {
                    self.advance(wf, host, io);
                    return;
                };
                if activate {
                    host.activate_tab(info.window, tab, false);
                }
                let prompt_id = PromptId::next();
                let prompt = discard_prompt(prompt_id, tab, &info.name);
                if let Some(ix) = self.flow_ix(wf) {
                    self.flows[ix].awaiting = Awaiting::DiscardPrompt {
                        prompt: prompt_id,
                        tab,
                    };
                }
                host.show_prompt(info.window, prompt);
            }
        }
    }

    fn begin_save_as(
        &mut self,
        wf: WorkflowId,
        tab: TabId,
        host: &impl DocHost,
        io: &mut Vec<FileIo>,
    ) {
        let Some(info) = host.tab_info(tab) else {
            self.remove_flow(wf);
            return;
        };
        if let Some(ix) = self.flow_ix(wf) {
            self.flows[ix].awaiting = Awaiting::SaveDialog {
                tab,
                format: info.format,
            };
        }
        io.push(FileIo::SaveDialog {
            wf,
            owner: info.window,
            current: info.path,
            format: info.format,
        });
    }

    fn begin_write(
        &mut self,
        wf: WorkflowId,
        tab: TabId,
        path: DocPath,
        format: Format,
        host: &mut impl DocHost,
        io: &mut Vec<FileIo>,
    ) {
        if self.writing.contains(&tab) {
            if let Some(ix) = self.flow_ix(wf) {
                self.flows[ix].awaiting = Awaiting::QueuedWrite { tab, path, format };
            }
            return;
        }
        let Some((snapshot, text)) = host.begin_save(tab) else {
            self.remove_flow(wf);
            return;
        };
        let job = SaveJob {
            tab,
            snapshot,
            text,
            path,
            format,
        };
        self.writing.insert(tab);
        if let Some(ix) = self.flow_ix(wf) {
            self.flows[ix].awaiting = Awaiting::Write(job.clone());
        }
        io.push(FileIo::WriteFile {
            wf,
            job,
            force_lossy: false,
        });
    }
}

fn resolve_save_dialog(
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

const UNTITLED_FILE_NAME: &str = "Untitled.txt";
const SAVE_AS_TITLE: &str = "Save as";
const TEXT_FILE_TYPE: (&str, &str) = ("Text documents (*.txt)", "*.txt");
const ALL_FILE_TYPE: (&str, &str) = ("All files", "*.*");
const TEXT_EXTENSION: &str = "txt";
const ENCODING_LABEL: &str = "Encoding:";

#[derive(Clone, Default)]
pub(crate) struct WriteTracker(Arc<(Mutex<usize>, Condvar)>);

impl WriteTracker {
    pub(crate) fn enter(&self) -> WriteFlightGuard {
        let mut count = self.0.0.lock().unwrap_or_else(|e| e.into_inner());
        *count += 1;
        WriteFlightGuard(self.0.clone())
    }

    pub(crate) fn in_flight(&self) -> usize {
        *self.0.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(crate) fn wait(&self, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        let mut count = self.0.0.lock().unwrap_or_else(|e| e.into_inner());
        while *count > 0 {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            let (guard, _) = self
                .0
                .1
                .wait_timeout(count, remaining)
                .unwrap_or_else(|e| e.into_inner());
            count = guard;
        }
    }
}

pub(crate) struct WriteFlightGuard(Arc<(Mutex<usize>, Condvar)>);

impl Drop for WriteFlightGuard {
    fn drop(&mut self) {
        let mut count = self.0.0.lock().unwrap_or_else(|e| e.into_inner());
        *count = count.saturating_sub(1);
        if *count == 0 {
            self.0.1.notify_all();
        }
    }
}

/// Where file work finishes: worker threads queue their events here, and each one wakes the loop.
#[derive(Clone)]
pub(crate) struct FileEvents(Arc<Mutex<VecDeque<FileEvent>>>, Waker);

impl FileEvents {
    pub(crate) fn new(waker: Waker) -> Self {
        Self(Arc::default(), waker)
    }

    fn send(&self, event: FileEvent) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push_back(event);
        self.1.wake();
    }

    /// The next finished piece of file work, if any.
    pub(crate) fn next(&self) -> Option<FileEvent> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop_front()
    }

    /// Runs `work` on a thread named `name` and sends `finish`'s event for its result: the
    /// panic message if it panicked, or the error if the thread couldn't start.
    fn spawn<T: Send + 'static>(
        &self,
        name: &str,
        work: impl FnOnce() -> T + Send + 'static,
        finish: impl FnOnce(Result<T, String>) -> FileEvent + Send + 'static,
    ) {
        let finish = Arc::new(Mutex::new(Some(finish)));
        let (events, finished) = (self.clone(), finish.clone());
        let started = thread::Builder::new().name(name.into()).spawn(move || {
            let outcome = catch_unwind(AssertUnwindSafe(work)).map_err(|p| panic_message(&*p));
            if let Some(finish) = finished
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take()
            {
                events.send(finish(outcome));
            }
        });
        if let Err(err) = started
            && let Some(finish) = finish.lock().unwrap_or_else(PoisonError::into_inner).take()
        {
            self.send(finish(Err(format!("Failed to start worker thread: {err}"))));
        }
    }
}

fn spawn_file_io(
    io: FileIo,
    owner: Option<WindowHandle>,
    tracker: &WriteTracker,
    events: &FileEvents,
) {
    let file_types = || {
        [TEXT_FILE_TYPE, ALL_FILE_TYPE]
            .map(|(name, spec)| (SharedStr::Static(name), SharedStr::Static(spec)))
            .into()
    };
    match io {
        FileIo::OpenDialog { window, current } => {
            let Some(owner) = owner else {
                events.send(FileEvent::OpenDialogDone {
                    window,
                    result: Ok(None),
                });
                return;
            };
            let options = OpenDialogOptions {
                directory: default_directory(current.as_deref()),
                file_types: file_types(),
            };
            events.spawn(
                "jot-open-dialog",
                move || run_open_dialog(owner, options),
                move |res| FileEvent::OpenDialogDone {
                    window,
                    result: res.unwrap_or_else(|msg| Err(Error::new(msg))),
                },
            );
        }
        FileIo::LoadFiles { window, paths } => {
            let fallback = paths.clone();
            events.spawn(
                "jot-load-files",
                move || {
                    paths
                        .into_iter()
                        .map(|doc_path| {
                            let loaded = document::load(&doc_path);
                            (doc_path, loaded)
                        })
                        .collect::<Vec<_>>()
                },
                move |res| {
                    let results = res.unwrap_or_else(|msg| {
                        fallback
                            .into_iter()
                            .map(|doc_path| {
                                (doc_path, Err(LoadError::Io(io::Error::other(msg.clone()))))
                            })
                            .collect()
                    });
                    FileEvent::FilesLoaded { window, results }
                },
            );
        }
        FileIo::SaveDialog {
            wf,
            current,
            format,
            ..
        } => {
            let Some(owner) = owner else {
                events.send(FileEvent::SaveDialogDone {
                    wf,
                    result: Ok(None),
                });
                return;
            };
            let current = current.as_ref().map(DocPath::as_path);
            let suggested = file_name(current).unwrap_or_else(|| UNTITLED_FILE_NAME.into());
            let options = SaveDialogOptions {
                title: SAVE_AS_TITLE.into(),
                directory: default_directory(current),
                file_name: suggested,
                file_types: file_types(),
                default_extension: TEXT_EXTENSION.into(),
                choice: SaveChoice {
                    label: ENCODING_LABEL.into(),
                    items: Encoding::SAVE_ORDER
                        .map(|encoding| SharedStr::Static(encoding.label()))
                        .into(),
                    selected: Encoding::SAVE_ORDER
                        .iter()
                        .position(|&encoding| encoding == format.encoding)
                        .unwrap_or_default(),
                },
            };
            events.spawn(
                "jot-save-dialog",
                move || run_save_dialog(owner, options),
                move |res| FileEvent::SaveDialogDone {
                    wf,
                    result: res.unwrap_or_else(|msg| Err(Error::new(msg))),
                },
            );
        }
        FileIo::WriteFile {
            wf,
            job,
            force_lossy,
        } => {
            let guard = tracker.enter();
            events.spawn(
                "jot-write-file",
                move || {
                    let _guard = guard;
                    if !force_lossy {
                        match document::is_lossy(&job.text, job.format) {
                            Ok(true) => return WriteOutcome::NeedsLossyConfirm,
                            Ok(false) => {}
                            Err(err) => return WriteOutcome::Error(err.into()),
                        }
                    }
                    match document::write_rope(job.path.as_path(), &job.text, job.format) {
                        Ok(()) => WriteOutcome::Saved {
                            format: job.format.written(),
                        },
                        Err(err) => WriteOutcome::Error(err),
                    }
                },
                move |res| {
                    let outcome = res.unwrap_or_else(|msg| {
                        WriteOutcome::Error(SaveError::Io(io::Error::other(msg)))
                    });
                    FileEvent::WriteDone { wf, outcome }
                },
            );
        }
    }
}

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

fn load_error_prompt(path: &Path, error: LoadError) -> Prompt {
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

fn save_error_prompt(path: &Path, error: &SaveError) -> Prompt {
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

fn dialog_error_prompt(title: &'static str, message: String) -> Prompt {
    info_prompt(SharedStr::Static(title), message.into())
}

fn already_open_save_prompt(path: &Path) -> Prompt {
    info_prompt(
        format!("Can't save {}", display_name(Some(path))).into(),
        SharedStr::Static(
            "This file is already open in another tab. Close that tab before saving over it.",
        ),
    )
}

fn discard_prompt(id: PromptId, tab: TabId, name: &str) -> Prompt {
    Prompt {
        title: APP_NAME.into(),
        message: format!("Do you want to save changes to {name}?").into(),
        choices: DISCARD_CHOICES,
        cancel: PromptAnswer::Cancel,
        reply: Some((id, tab)),
    }
}

fn lossy_ansi_prompt(id: PromptId, tab: TabId, path: &Path) -> Prompt {
    Prompt {
        title: APP_NAME.into(),
        message: format!("{}\n{LOSSY_ANSI_WARNING}", path.display()).into(),
        choices: LOSSY_CHOICES,
        cancel: PromptAnswer::Cancel,
        reply: Some((id, tab)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_results_arrive_and_a_panicking_worker_reports_an_error() {
        let platform = crate::platform::Platform::new(None).unwrap();
        let events = FileEvents::new(platform.waker());
        let window = WindowId::next();
        let finish = move |res: Result<crate::types::Result<Option<Vec<PathBuf>>>, String>| {
            FileEvent::OpenDialogDone {
                window,
                result: res.unwrap_or_else(|m| Err(Error::new(m))),
            }
        };
        events.spawn("test-ok", || Ok(None), finish);
        events.spawn("test-panic", || panic!("boom"), finish);
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut outcomes = Vec::new();
        while outcomes.len() < 2 && Instant::now() < deadline {
            match events.next() {
                Some(FileEvent::OpenDialogDone { result, .. }) => {
                    outcomes.push(result.map_err(|e| e.to_string()))
                }
                Some(_) => panic!("unexpected event"),
                None => thread::sleep(Duration::from_millis(5)),
            }
        }
        assert!(outcomes.iter().any(|o| matches!(o, Ok(None))));
        assert!(
            outcomes
                .iter()
                .any(|o| o.as_ref().is_err_and(|e| e.contains("boom")))
        );
    }

    struct FakeTab {
        id: TabId,
        window: WindowId,
        doc_path: Option<DocPath>,
        format: Format,
        text: String,
        saved_rev: u64,
        rev: u64,
    }

    impl FakeTab {
        fn new(window: WindowId, path: Option<&str>, text: &str, dirty: bool) -> Self {
            let doc_path = path.map(|p| DocPath::new(PathBuf::from(p)));
            let rev = if dirty { 1 } else { 0 };
            Self {
                id: TabId::next(),
                window,
                doc_path,
                format: Format::default(),
                text: text.into(),
                saved_rev: 0,
                rev,
            }
        }

        fn is_dirty(&self) -> bool {
            self.rev != self.saved_rev
        }

        fn edit(&mut self, extra: &str) {
            self.text.push_str(extra);
            self.rev += 1;
        }
    }

    #[derive(Default)]
    struct FakeHost {
        tabs: Vec<FakeTab>,
        windows: Vec<WindowId>,
        closed_windows: Vec<WindowId>,
        prompts: Vec<(WindowId, Prompt)>,
        activated: Vec<(WindowId, TabId, bool)>,
    }

    impl FakeHost {
        fn add_window(&mut self, id: WindowId) {
            self.windows.push(id);
        }

        fn add_tab(&mut self, tab: FakeTab) -> TabId {
            let id = tab.id;
            self.tabs.push(tab);
            id
        }

        fn tab_mut(&mut self, id: TabId) -> &mut FakeTab {
            self.tabs.iter_mut().find(|t| t.id == id).unwrap()
        }

        fn pop_prompt(&mut self) -> (WindowId, Prompt) {
            self.prompts.remove(0)
        }
    }

    impl DocHost for FakeHost {
        fn tab_info(&self, tab: TabId) -> Option<TabSnapshot> {
            let t = self.tabs.iter().find(|t| t.id == tab)?;
            Some(TabSnapshot {
                window: t.window,
                name: display_name(t.doc_path.as_ref().map(DocPath::as_path)),
                path: t.doc_path.clone(),
                format: t.format,
                dirty: t.is_dirty(),
                revision: t.rev,
            })
        }

        fn window_dirty_tabs(&self, window: WindowId) -> Vec<TabId> {
            self.tabs
                .iter()
                .filter(|t| t.window == window && t.is_dirty())
                .map(|t| t.id)
                .collect()
        }

        fn active_tab_path(&self, window: WindowId) -> Option<PathBuf> {
            self.tabs
                .iter()
                .find(|t| t.window == window)
                .and_then(|t| t.doc_path.as_ref().map(|p| p.as_path().to_path_buf()))
        }

        fn fallback_window(&self, preferred: WindowId) -> Option<WindowId> {
            if self.windows.contains(&preferred) {
                Some(preferred)
            } else {
                self.windows.last().copied()
            }
        }

        fn find_tab_by_key(&self, key: &PathKey) -> Option<(WindowId, TabId)> {
            self.tabs
                .iter()
                .find(|t| t.doc_path.as_ref().is_some_and(|p| p.key() == key))
                .map(|t| (t.window, t.id))
        }

        fn activate_tab(&mut self, window: WindowId, tab: TabId, bring_to_front: bool) {
            self.activated.push((window, tab, bring_to_front));
        }

        fn begin_save(&mut self, tab: TabId) -> Option<(u64, Rope)> {
            let t = self.tabs.iter().find(|t| t.id == tab)?;
            Some((t.rev, Rope::from(t.text.as_str())))
        }

        fn mark_saved(&mut self, tab: TabId, snapshot: u64, doc_path: DocPath, format: Format) {
            if let Some(t) = self.tabs.iter_mut().find(|t| t.id == tab) {
                t.saved_rev = snapshot;
                t.doc_path = Some(doc_path);
                t.format = format;
            }
        }

        fn adopt_loaded(&mut self, window: WindowId, loaded: document::Loaded) {
            self.tabs.push(FakeTab {
                id: TabId::next(),
                window,
                doc_path: Some(loaded.doc_path),
                format: loaded.format,
                text: loaded.text.to_string(),
                saved_rev: 0,
                rev: 0,
            });
        }

        fn remove_tab(&mut self, tab: TabId) {
            self.tabs.retain(|t| t.id != tab);
        }

        fn mark_window_close(&mut self, window: WindowId) {
            self.closed_windows.push(window);
            self.windows.retain(|&w| w != window);
        }

        fn show_prompt(&mut self, window: WindowId, prompt: Prompt) {
            self.prompts.push((window, prompt));
        }

        fn window_handle(&self, _: WindowId) -> Option<WindowHandle> {
            None
        }
    }

    #[test]
    fn close_window_saves_first_tab_reprompts_on_mid_write_edit_and_discards_second() {
        let mut ops = FileOps::default();
        let mut host = FakeHost::default();
        let mut io = Vec::new();
        let w1 = WindowId(1);
        host.add_window(w1);
        let t1 = host.add_tab(FakeTab::new(w1, Some(r"C:\a.txt"), "hello", true));
        let t2 = host.add_tab(FakeTab::new(w1, None, "unsaved", true));

        ops.request(FileRequest::CloseWindow { window: w1 }, &mut host, &mut io);
        let (_, p1) = host.pop_prompt();
        assert_eq!(p1.tab(), Some(t1));

        // User chooses Save on t1; then edits t1 while write is in flight.
        ops.on_prompt(p1.reply.unwrap().0, PromptAnswer::Save, &mut host, &mut io);
        let FileIo::WriteFile { wf, job, .. } = io.pop().unwrap() else {
            panic!("expected WriteFile");
        };
        assert_eq!(job.tab, t1);
        host.tab_mut(t1).edit(" more");

        // Stale workflow event must be ignored.
        ops.on_event(
            FileEvent::WriteDone {
                wf: WorkflowId::next(),
                outcome: WriteOutcome::Saved {
                    format: Format::default(),
                },
            },
            &mut host,
            &mut io,
        );
        assert!(host.prompts.is_empty());

        // Real write finishes -> t1 is still dirty, advances to t2 discard prompt.
        ops.on_event(
            FileEvent::WriteDone {
                wf,
                outcome: WriteOutcome::Saved { format: job.format },
            },
            &mut host,
            &mut io,
        );
        let (_, p2) = host.pop_prompt();
        assert_eq!(p2.tab(), Some(t2));

        // User chooses Don't save on t2 -> window must NOT close yet because t1 was edited mid-write.
        ops.on_prompt(
            p2.reply.unwrap().0,
            PromptAnswer::Discard,
            &mut host,
            &mut io,
        );
        assert!(host.closed_windows.is_empty());
        let (_, p3) = host.pop_prompt();
        assert_eq!(p3.tab(), Some(t1));

        // User chooses Don't save on re-prompted t1 -> window closes.
        ops.on_prompt(
            p3.reply.unwrap().0,
            PromptAnswer::Discard,
            &mut host,
            &mut io,
        );
        assert_eq!(host.closed_windows, vec![w1]);
        assert!(!ops.has_active_workflow_for(w1));
    }

    #[test]
    fn migrated_save_lossy_confirm_conflict_and_coalesced_resave() {
        let mut ops = FileOps::default();
        let mut host = FakeHost::default();
        let mut io = Vec::new();
        let (w1, w2) = (WindowId(1), WindowId(2));
        host.add_window(w1);
        host.add_window(w2);
        let t1 = host.add_tab(FakeTab::new(w1, Some(r"C:\doc.txt"), "v1", true));
        let t2 = host.add_tab(FakeTab::new(w2, Some(r"C:\other.txt"), "other", false));

        // Start save on w1, queue a second save mid-flight, then migrate tab to w2.
        ops.request(FileRequest::Save { window: w1, tab: t1 }, &mut host, &mut io);
        let FileIo::WriteFile {
            wf: wf1,
            job: job1,
            ..
        } = io.pop().unwrap()
        else {
            panic!("expected WriteFile");
        };
        host.tab_mut(t1).edit(" v2");
        ops.request(FileRequest::Save { window: w1, tab: t1 }, &mut host, &mut io);
        assert!(io.is_empty(), "second save must queue while first is in flight");
        host.tab_mut(t1).window = w2;

        // First save triggers lossy confirm -> prompt must appear on current owner w2!
        ops.on_event(
            FileEvent::WriteDone {
                wf: wf1,
                outcome: WriteOutcome::NeedsLossyConfirm,
            },
            &mut host,
            &mut io,
        );
        let (prompt_win, lossy_prompt) = host.pop_prompt();
        assert_eq!(prompt_win, w2);
        assert_eq!(lossy_prompt.tab(), Some(t1));

        // Confirm OK -> writes with force_lossy=true.
        ops.on_prompt(
            lossy_prompt.reply.unwrap().0,
            PromptAnswer::Continue,
            &mut host,
            &mut io,
        );
        let FileIo::WriteFile {
            wf: wf1_again,
            force_lossy,
            ..
        } = io.pop().unwrap()
        else {
            panic!("expected forced WriteFile");
        };
        assert_eq!(wf1_again, wf1);
        assert!(force_lossy);

        // First write completes -> immediately wakes queued second save with new snapshot.
        ops.on_event(
            FileEvent::WriteDone {
                wf: wf1,
                outcome: WriteOutcome::Saved {
                    format: job1.format,
                },
            },
            &mut host,
            &mut io,
        );
        let FileIo::WriteFile {
            wf: wf2,
            job: job2,
            ..
        } = io.pop().unwrap()
        else {
            panic!("expected queued WriteFile");
        };
        assert_eq!(job2.text.to_string(), "v1 v2");
        ops.on_event(
            FileEvent::WriteDone {
                wf: wf2,
                outcome: WriteOutcome::Saved {
                    format: job2.format,
                },
            },
            &mut host,
            &mut io,
        );
        assert!(!host.tab_info(t1).unwrap().dirty);

        // Save As over already-open tab t2 -> conflict error prompt, no write spawned.
        ops.request(FileRequest::SaveAs { window: w2, tab: t1 }, &mut host, &mut io);
        let FileIo::SaveDialog { wf: wf3, .. } = io.pop().unwrap() else {
            panic!("expected SaveDialog");
        };
        ops.on_event(
            FileEvent::SaveDialogDone {
                wf: wf3,
                result: Ok(Some((PathBuf::from(r"C:\other.txt"), None))),
            },
            &mut host,
            &mut io,
        );
        assert!(io.is_empty());
        let (conflict_win, conflict_prompt) = host.pop_prompt();
        assert_eq!(conflict_win, w2);
        assert!(conflict_prompt.reply.is_none());
        let _ = t2;
    }
}
