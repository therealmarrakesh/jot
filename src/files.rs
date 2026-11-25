//! Opening, saving and closing documents.
//!
//! The app hands `FileOps` its file requests (open, save, save as, close a tab or a window), the
//! answers to the prompts it raised, and the events of the file work it started. `FileOps` runs
//! each request as a workflow: it asks to save or discard dirty tabs, shows the save dialog,
//! writes one tab at a time, and warns before a lossy save. It sees the tabs through the app's
//! `DocHost`, and asks for slow work (dialogs, reading and writing files) as `FileIo`, which
//! `FileOps::spawn` runs on worker threads that report back through `FileEvents`.

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
use crate::platform::{
    OpenDialogOptions, SaveChoice, SaveDialogOptions, SaveDialogResult, Waker, WindowHandle,
    WindowId, panic_message, run_open_dialog, run_save_dialog,
};
use crate::text::Rope;
use crate::types::{APP_NAME, Error, SharedStr};
use crate::ui::TabId;

// ---- 1. What the app asks, and what it's asked ----------------------------------------------

crate::types::id_type!(PromptId);
crate::types::id_type!(WorkflowId);

pub(crate) enum FileRequest {
    OpenDialog {
        window: WindowId,
    },
    OpenPaths {
        window: WindowId,
        paths: Vec<PathBuf>,
    },
    Save {
        window: WindowId,
        tab: TabId,
    },
    SaveAs {
        window: WindowId,
        tab: TabId,
    },
    SaveAll {
        window: WindowId,
    },
    CloseTab {
        window: WindowId,
        tab: TabId,
    },
    CloseWindow {
        window: WindowId,
    },
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum PromptAnswer {
    Save,
    Discard,
    Continue,
    Cancel,
}

/// A message box to show: its buttons and what each answers, what dismissing it answers, and,
/// for a prompt a workflow waits on, its id and the tab it's about.
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

/// What a workflow needs to know about a tab.
pub(crate) struct TabSnapshot {
    pub(crate) window: WindowId,
    pub(crate) name: String,
    pub(crate) path: Option<DocPath>,
    pub(crate) format: Format,
    pub(crate) dirty: bool,
    pub(crate) revision: u64,
}

/// The app's tabs and windows, as file work sees and changes them.
pub(crate) trait DocHost {
    fn tab_info(&self, tab: TabId) -> Option<TabSnapshot>;
    fn window_dirty_tabs(&self, window: WindowId) -> Vec<TabId>;
    fn active_tab_path(&self, window: WindowId) -> Option<PathBuf>;
    /// `preferred` if it's still open, else another window to show things in.
    fn fallback_window(&self, preferred: WindowId) -> Option<WindowId>;
    fn find_tab_by_key(&self, key: &PathKey) -> Option<(WindowId, TabId)>;
    fn activate_tab(&mut self, window: WindowId, tab: TabId, bring_to_front: bool);
    /// The tab's text to write, and the revision it is.
    fn begin_save(&mut self, tab: TabId) -> Option<(u64, Rope)>;
    fn mark_saved(&mut self, tab: TabId, snapshot: u64, doc_path: DocPath, format: Format);
    fn adopt_loaded(&mut self, window: WindowId, loaded: document::Loaded);
    fn remove_tab(&mut self, tab: TabId);
    fn mark_window_close(&mut self, window: WindowId);
    fn show_prompt(&mut self, window: WindowId, prompt: Prompt);
    /// The window's handle, to own the file dialogs it asked for.
    fn window_handle(&self, window: WindowId) -> Option<WindowHandle>;
}

// ---- 2. Workflows ---------------------------------------------------------------------------

/// What a workflow was started to do.
enum Goal {
    Save(Option<TabId>),
    SaveAs(Option<TabId>),
    SaveAll(VecDeque<TabId>),
    CloseTab(TabId),
    /// The window's dirty tabs still to ask about.
    CloseWindow(VecDeque<TabId>),
}

/// What a workflow is waiting on.
enum Awaiting {
    DiscardPrompt {
        prompt: PromptId,
        tab: TabId,
    },
    SaveDialog {
        tab: TabId,
        format: Format,
    },
    /// Another write of the tab is under way.
    QueuedWrite {
        tab: TabId,
        path: DocPath,
        format: Format,
    },
    Write(SaveJob),
    LossyPrompt {
        prompt: PromptId,
        job: SaveJob,
    },
}

/// What a workflow does next.
enum Step {
    Save {
        tab: TabId,
        activate: bool,
    },
    SaveAs(TabId),
    /// Ask whether to save `tab` (named `name`, in `window`), showing it first if `activate`.
    ConfirmDiscard {
        tab: TabId,
        window: WindowId,
        name: String,
        activate: bool,
    },
    RemoveTab(TabId),
    CloseWindow(WindowId),
    Done,
}

/// One request being carried out. A workflow is taken out of `FileOps::flows` to work on, and
/// goes back in when it waits again; one that's done is dropped.
struct Workflow {
    id: WorkflowId,
    origin: WindowId,
    goal: Goal,
    /// A tab to save before going on (the user chose Save when asked).
    save_next: Option<TabId>,
    /// Tabs the user chose not to save, at the revision they chose it for.
    approved: Vec<(TabId, u64)>,
    awaiting: Option<Awaiting>,
}

impl Workflow {
    fn new(origin: WindowId, goal: Goal) -> Self {
        Self {
            id: WorkflowId::next(),
            origin,
            goal,
            save_next: None,
            approved: Vec::new(),
            awaiting: None,
        }
    }

    fn step(&mut self, host: &impl DocHost) -> Step {
        if let Some(tab) = self.save_next.take() {
            return Step::Save {
                tab,
                activate: false,
            };
        }
        match &mut self.goal {
            Goal::Save(tab) => tab.take().map_or(Step::Done, |tab| Step::Save {
                tab,
                activate: false,
            }),
            Goal::SaveAs(tab) => tab.take().map_or(Step::Done, Step::SaveAs),
            Goal::SaveAll(tabs) => tabs.pop_front().map_or(Step::Done, |tab| Step::Save {
                tab,
                activate: true,
            }),
            Goal::CloseTab(tab) => {
                let tab = *tab;
                match host.tab_info(tab) {
                    None => Step::Done,
                    Some(t) if t.dirty && !self.approved.contains(&(tab, t.revision)) => {
                        Step::ConfirmDiscard {
                            tab,
                            window: t.window,
                            name: t.name,
                            activate: false,
                        }
                    }
                    Some(_) => Step::RemoveTab(tab),
                }
            }
            // Asks about each dirty tab not yet discarded at its revision; once none is left,
            // checks again (one may have been edited meanwhile) before closing.
            Goal::CloseWindow(pending) => loop {
                let unapproved =
                    |tab: TabId, revision: u64| !self.approved.contains(&(tab, revision));
                if let Some(tab) = pending.pop_front() {
                    if let Some(t) = host.tab_info(tab)
                        && t.dirty
                        && unapproved(tab, t.revision)
                    {
                        return Step::ConfirmDiscard {
                            tab,
                            window: t.window,
                            name: t.name,
                            activate: true,
                        };
                    }
                    continue;
                }
                let remaining: VecDeque<TabId> = host
                    .window_dirty_tabs(self.origin)
                    .into_iter()
                    .filter(|&tab| {
                        host.tab_info(tab)
                            .is_some_and(|t| unapproved(tab, t.revision))
                    })
                    .collect();
                if remaining.is_empty() {
                    return Step::CloseWindow(self.origin);
                }
                *pending = remaining;
            },
        }
    }
}

/// The file workflows under way, and what they're doing.
#[derive(Default)]
pub(crate) struct FileOps {
    flows: Vec<Workflow>,
    /// Tabs being written (a second write of one waits for the first).
    writing: HashSet<TabId>,
    /// Paths being read.
    loading: HashSet<PathKey>,
    /// Windows with an open dialog showing.
    open_dialogs: HashSet<WindowId>,
    pub(crate) writes: WriteTracker,
}

impl FileOps {
    /// Starts `io`, the file work requests and events asked for, on worker threads; it reports
    /// back through `events`.
    pub(crate) fn spawn(&self, io: Vec<FileIo>, host: &impl DocHost, events: &FileEvents) {
        for io in io {
            let owner = match &io {
                FileIo::OpenDialog { window, .. } | FileIo::SaveDialog { owner: window, .. } => {
                    host.window_handle(*window)
                }
                FileIo::LoadFiles { .. } | FileIo::WriteFile { .. } => None,
            };
            io.start(owner, &self.writes, events);
        }
    }

    pub(crate) fn has_active_workflow_for(&self, window: WindowId) -> bool {
        self.flows.iter().any(|f| f.origin == window)
    }

    /// `window` closed, taking its prompts with it: its workflows end, except a write under
    /// way, and so does any waiting on one of the prompts.
    pub(crate) fn on_window_closed(&mut self, window: WindowId, cancelled_prompts: &[PromptId]) {
        self.open_dialogs.remove(&window);
        let mut freed = Vec::new();
        self.flows.retain(|f| {
            let prompt_gone = match &f.awaiting {
                Some(
                    Awaiting::DiscardPrompt { prompt, .. } | Awaiting::LossyPrompt { prompt, .. },
                ) => cancelled_prompts.contains(prompt),
                _ => false,
            };
            let writing = matches!(f.awaiting, Some(Awaiting::Write(_)));
            let ends = prompt_gone || (f.origin == window && !writing);
            if ends && let Some(Awaiting::LossyPrompt { job, .. }) = &f.awaiting {
                freed.push(job.tab);
            }
            !ends
        });
        for tab in freed {
            self.writing.remove(&tab);
        }
    }

    pub(crate) fn request(
        &mut self,
        req: FileRequest,
        host: &mut impl DocHost,
        io: &mut Vec<FileIo>,
    ) {
        let goal = match req {
            FileRequest::OpenDialog { window } => {
                if self.open_dialogs.insert(window) {
                    let current = host.active_tab_path(window);
                    io.push(FileIo::OpenDialog { window, current });
                }
                return;
            }
            FileRequest::OpenPaths { window, paths } => {
                self.open_paths(window, paths, host, io);
                return;
            }
            FileRequest::Save { window, tab } => {
                // A saved file with nothing to write and nothing lost reading it stays as it is.
                match host.tab_info(tab) {
                    Some(t) if t.path.is_some() && !t.dirty && !t.format.lossy_decode => return,
                    Some(_) => (window, Goal::Save(Some(tab))),
                    None => return,
                }
            }
            FileRequest::SaveAs { window, tab } => (window, Goal::SaveAs(Some(tab))),
            FileRequest::SaveAll { window } => {
                let dirty = host.window_dirty_tabs(window);
                if dirty.is_empty() {
                    return;
                }
                (window, Goal::SaveAll(dirty.into()))
            }
            FileRequest::CloseTab { window, tab } => {
                let Some(info) = host.tab_info(tab) else {
                    return;
                };
                if !info.dirty {
                    self.remove_tab(tab, host);
                    return;
                }
                host.activate_tab(info.window, tab, false);
                (window, Goal::CloseTab(tab))
            }
            FileRequest::CloseWindow { window } => {
                let dirty = host.window_dirty_tabs(window);
                if dirty.is_empty() {
                    host.mark_window_close(window);
                    return;
                }
                let closing =
                    |f: &Workflow| f.origin == window && matches!(f.goal, Goal::CloseWindow(_));
                if self.flows.iter().any(closing) {
                    return;
                }
                (window, Goal::CloseWindow(dirty.into()))
            }
        };
        self.advance(Workflow::new(goal.0, goal.1), host, io);
    }

    pub(crate) fn on_prompt(
        &mut self,
        prompt: PromptId,
        answer: PromptAnswer,
        host: &mut impl DocHost,
        io: &mut Vec<FileIo>,
    ) {
        let asked = |f: &Workflow| match &f.awaiting {
            Some(
                Awaiting::DiscardPrompt { prompt: p, .. } | Awaiting::LossyPrompt { prompt: p, .. },
            ) => *p == prompt,
            _ => false,
        };
        let Some(ix) = self.flows.iter().position(asked) else {
            return;
        };
        let mut flow = self.flows.remove(ix);
        match (flow.awaiting.take(), answer) {
            (Some(Awaiting::DiscardPrompt { tab, .. }), PromptAnswer::Save) => {
                flow.save_next = Some(tab);
                self.advance(flow, host, io);
            }
            (Some(Awaiting::DiscardPrompt { tab, .. }), PromptAnswer::Discard) => {
                if let Some(info) = host.tab_info(tab) {
                    flow.approved.retain(|&(t, _)| t != tab);
                    flow.approved.push((tab, info.revision));
                }
                self.advance(flow, host, io);
            }
            (Some(Awaiting::LossyPrompt { job, .. }), PromptAnswer::Continue) => {
                let wf = flow.id;
                self.wait(flow, Awaiting::Write(job.clone()));
                io.push(FileIo::WriteFile {
                    wf,
                    job,
                    force_lossy: true,
                });
            }
            // Not saving lossily: choose another encoding in the save dialog.
            (Some(Awaiting::LossyPrompt { job, .. }), _) => {
                self.writing.remove(&job.tab);
                self.save_as(flow, job.tab, host, io);
                self.wake_queued_write(job.tab, host, io);
            }
            // Cancelled: the workflow ends.
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
                    Err(err) => host.show_prompt(
                        target,
                        dialog_error_prompt("Can't open file", err.to_string()),
                    ),
                }
            }
            FileEvent::FilesLoaded { window, results } => {
                for (doc_path, loaded) in results {
                    self.loading.remove(doc_path.key());
                    // Opened meanwhile (or already open): show that tab.
                    if let Some((win, tab)) = host.find_tab_by_key(doc_path.key()) {
                        host.activate_tab(win, tab, true);
                        continue;
                    }
                    let Some(target) = host.fallback_window(window) else {
                        continue;
                    };
                    match loaded {
                        Ok(loaded) => host.adopt_loaded(target, loaded),
                        Err(err) => {
                            host.show_prompt(target, load_error_prompt(doc_path.as_path(), err))
                        }
                    }
                }
            }
            FileEvent::SaveDialogDone { wf, result } => {
                let Some(mut flow) = self.take(wf, |a| matches!(a, Awaiting::SaveDialog { .. }))
                else {
                    return;
                };
                let Some(Awaiting::SaveDialog { tab, format }) = flow.awaiting.take() else {
                    unreachable!();
                };
                match result {
                    Ok(Some((path, encoding))) => {
                        let path = DocPath::new(path);
                        let format = Format {
                            encoding: encoding
                                .and_then(|ix| Encoding::SAVE_ORDER.get(ix).copied())
                                .unwrap_or(format.encoding),
                            lossy_decode: false,
                            ..format
                        };
                        let taken = host
                            .find_tab_by_key(path.key())
                            .is_some_and(|(_, other)| other != tab);
                        if taken {
                            self.show_prompt_for(
                                tab,
                                flow.origin,
                                already_open_save_prompt(path.as_path()),
                                host,
                            );
                        } else {
                            self.write(flow, tab, path, format, host, io);
                        }
                    }
                    Ok(None) => {}
                    Err(err) => {
                        let prompt = dialog_error_prompt("Can't save file", err.to_string());
                        self.show_prompt_for(tab, flow.origin, prompt, host);
                    }
                }
            }
            FileEvent::WriteDone { wf, outcome } => {
                let Some(mut flow) = self.take(wf, |a| matches!(a, Awaiting::Write(_))) else {
                    return;
                };
                let Some(Awaiting::Write(job)) = flow.awaiting.take() else {
                    unreachable!();
                };
                let tab = job.tab;
                match outcome {
                    WriteOutcome::Saved { format } => {
                        self.writing.remove(&tab);
                        host.mark_saved(tab, job.snapshot, job.path, format);
                        self.advance(flow, host, io);
                        self.wake_queued_write(tab, host, io);
                    }
                    WriteOutcome::NeedsLossyConfirm => {
                        match self.prompt_window(tab, flow.origin, host) {
                            Some(target) => {
                                let prompt = PromptId::next();
                                let message = lossy_ansi_prompt(prompt, tab, job.path.as_path());
                                self.wait(flow, Awaiting::LossyPrompt { prompt, job });
                                host.show_prompt(target, message);
                            }
                            None => {
                                self.writing.remove(&tab);
                                self.wake_queued_write(tab, host, io);
                            }
                        }
                    }
                    WriteOutcome::Error(err) => {
                        self.writing.remove(&tab);
                        let prompt = save_error_prompt(job.path.as_path(), &err);
                        self.show_prompt_for(tab, flow.origin, prompt, host);
                        self.wake_queued_write(tab, host, io);
                    }
                }
            }
        }
    }

    /// Opens `paths` in `window`: those open already are shown, the rest are read (once each).
    fn open_paths(
        &mut self,
        window: WindowId,
        paths: Vec<PathBuf>,
        host: &mut impl DocHost,
        io: &mut Vec<FileIo>,
    ) {
        let mut to_load: Vec<DocPath> = Vec::new();
        let mut open = None;
        for path in paths {
            let path = DocPath::new(path);
            if let Some(found) = host.find_tab_by_key(path.key()) {
                open = Some(found);
            } else if self.loading.insert(path.key().clone()) {
                to_load.push(path);
            }
        }
        if !to_load.is_empty() {
            io.push(FileIo::LoadFiles {
                window,
                paths: to_load,
            });
        } else if let Some((win, tab)) = open {
            host.activate_tab(win, tab, true);
        }
    }

    /// Takes workflow `id` out to work on, if it's waiting on what `waiting` matches.
    fn take(&mut self, id: WorkflowId, waiting: impl Fn(&Awaiting) -> bool) -> Option<Workflow> {
        let ix = self
            .flows
            .iter()
            .position(|f| f.id == id && f.awaiting.as_ref().is_some_and(&waiting))?;
        Some(self.flows.remove(ix))
    }

    /// Puts `flow` back, waiting on `awaiting`. Workflows are kept in the order they started, so
    /// writes queued for one tab go in the order they were asked for.
    fn wait(&mut self, mut flow: Workflow, awaiting: Awaiting) {
        flow.awaiting = Some(awaiting);
        let at = self.flows.partition_point(|f| f.id.0 < flow.id.0);
        self.flows.insert(at, flow);
    }

    /// Closes `tab`, and ends what was waiting on a prompt about it.
    fn remove_tab(&mut self, tab: TabId, host: &mut impl DocHost) {
        host.remove_tab(tab);
        self.flows.retain(|f| match &f.awaiting {
            Some(
                Awaiting::DiscardPrompt { tab: t, .. }
                | Awaiting::LossyPrompt {
                    job: SaveJob { tab: t, .. },
                    ..
                },
            ) => *t != tab,
            _ => true,
        });
    }

    /// Starts the write of `tab` that was waiting for its last one to finish.
    fn wake_queued_write(&mut self, tab: TabId, host: &mut impl DocHost, io: &mut Vec<FileIo>) {
        if self.writing.contains(&tab) {
            return;
        }
        let queued = |f: &Workflow| matches!(&f.awaiting, Some(Awaiting::QueuedWrite { tab: t, .. }) if *t == tab);
        let Some(ix) = self.flows.iter().position(queued) else {
            return;
        };
        let mut flow = self.flows.remove(ix);
        let Some(Awaiting::QueuedWrite { tab, path, format }) = flow.awaiting.take() else {
            unreachable!();
        };
        self.write(flow, tab, path, format, host, io);
    }

    /// The window to ask about `tab` in: its own, or the workflow's (or another) if it's gone.
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
        if let Some(window) = self.prompt_window(tab, origin, host) {
            host.show_prompt(window, prompt);
        }
    }

    /// Takes `flow`'s next step: it waits on what that needs, or it's done.
    fn advance(&mut self, mut flow: Workflow, host: &mut impl DocHost, io: &mut Vec<FileIo>) {
        match flow.step(host) {
            Step::Done => {}
            Step::RemoveTab(tab) => self.remove_tab(tab, host),
            Step::CloseWindow(window) => host.mark_window_close(window),
            Step::Save { tab, activate } => {
                let Some(info) = host.tab_info(tab) else {
                    return;
                };
                if activate {
                    host.activate_tab(info.window, tab, false);
                }
                match info.path {
                    Some(path) => self.write(flow, tab, path, info.format, host, io),
                    None => self.save_as(flow, tab, host, io),
                }
            }
            Step::SaveAs(tab) => self.save_as(flow, tab, host, io),
            Step::ConfirmDiscard {
                tab,
                window,
                name,
                activate,
            } => {
                if activate {
                    host.activate_tab(window, tab, false);
                }
                let prompt = PromptId::next();
                self.wait(flow, Awaiting::DiscardPrompt { prompt, tab });
                host.show_prompt(window, discard_prompt(prompt, tab, &name));
            }
        }
    }

    /// Shows the save dialog for `tab`.
    fn save_as(&mut self, flow: Workflow, tab: TabId, host: &impl DocHost, io: &mut Vec<FileIo>) {
        let Some(info) = host.tab_info(tab) else {
            return;
        };
        let wf = flow.id;
        self.wait(
            flow,
            Awaiting::SaveDialog {
                tab,
                format: info.format,
            },
        );
        io.push(FileIo::SaveDialog {
            wf,
            owner: info.window,
            current: info.path,
            format: info.format,
        });
    }

    /// Writes `tab` to `path` in `format`, or waits for the write of it under way.
    fn write(
        &mut self,
        flow: Workflow,
        tab: TabId,
        path: DocPath,
        format: Format,
        host: &mut impl DocHost,
        io: &mut Vec<FileIo>,
    ) {
        if self.writing.contains(&tab) {
            self.wait(flow, Awaiting::QueuedWrite { tab, path, format });
            return;
        }
        let Some((snapshot, text)) = host.begin_save(tab) else {
            return;
        };
        let job = SaveJob {
            tab,
            snapshot,
            text,
            path,
            format,
        };
        let wf = flow.id;
        self.writing.insert(tab);
        self.wait(flow, Awaiting::Write(job.clone()));
        io.push(FileIo::WriteFile {
            wf,
            job,
            force_lossy: false,
        });
    }
}

// ---- 3. File work on worker threads ---------------------------------------------------------

/// A tab's text to write, as it was when the write began.
#[derive(Clone)]
pub(crate) struct SaveJob {
    pub(crate) tab: TabId,
    pub(crate) snapshot: u64,
    pub(crate) text: Rope,
    pub(crate) path: DocPath,
    pub(crate) format: Format,
}

/// Slow file work a workflow asks for.
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
    /// Writes the job, first checking it can be written without loss unless `force_lossy`.
    WriteFile {
        wf: WorkflowId,
        job: SaveJob,
        force_lossy: bool,
    },
}

pub(crate) enum WriteOutcome {
    Saved { format: Format },
    NeedsLossyConfirm,
    Error(SaveError),
}

/// File work finished.
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

const UNTITLED_FILE_NAME: &str = "Untitled.txt";
const SAVE_AS_TITLE: &str = "Save as";
const FILE_TYPES: [(&str, &str); 2] = [("Text documents (*.txt)", "*.txt"), ("All files", "*.*")];
const TEXT_EXTENSION: &str = "txt";
const ENCODING_LABEL: &str = "Encoding:";

impl FileIo {
    /// Runs the work on a worker thread, which sends its event to `events`. A dialog needs its
    /// window's `owner`; with none (the window has gone) it's cancelled.
    fn start(self, owner: Option<WindowHandle>, writes: &WriteTracker, events: &FileEvents) {
        let file_types = || {
            FILE_TYPES
                .map(|(name, spec)| (SharedStr::Static(name), SharedStr::Static(spec)))
                .into()
        };
        match self {
            FileIo::OpenDialog { window, current } => {
                let Some(owner) = owner else {
                    return events.send(FileEvent::OpenDialogDone {
                        window,
                        result: Ok(None),
                    });
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
                let asked = paths.clone();
                events.spawn(
                    "jot-load-files",
                    move || {
                        paths
                            .into_iter()
                            .map(|path| {
                                let loaded = document::load(&path);
                                (path, loaded)
                            })
                            .collect()
                    },
                    move |res| FileEvent::FilesLoaded {
                        window,
                        // A reader that panicked fails every file it was given.
                        results: res.unwrap_or_else(|msg| {
                            let failed =
                                |path| (path, Err(LoadError::Io(io::Error::other(msg.clone()))));
                            asked.into_iter().map(failed).collect()
                        }),
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
                    return events.send(FileEvent::SaveDialogDone {
                        wf,
                        result: Ok(None),
                    });
                };
                let current = current.as_ref().map(DocPath::as_path);
                let options = SaveDialogOptions {
                    title: SAVE_AS_TITLE.into(),
                    directory: default_directory(current),
                    file_name: file_name(current).unwrap_or_else(|| UNTITLED_FILE_NAME.into()),
                    file_types: file_types(),
                    default_extension: TEXT_EXTENSION.into(),
                    choice: SaveChoice {
                        label: ENCODING_LABEL.into(),
                        items: Encoding::SAVE_ORDER
                            .map(|e| SharedStr::Static(e.label()))
                            .into(),
                        selected: Encoding::SAVE_ORDER
                            .iter()
                            .position(|&e| e == format.encoding)
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
                let writing = writes.start();
                events.spawn(
                    "jot-write-file",
                    move || {
                        let _writing = writing;
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
                    move |res| FileEvent::WriteDone {
                        wf,
                        outcome: res.unwrap_or_else(|msg| {
                            WriteOutcome::Error(SaveError::Io(io::Error::other(msg)))
                        }),
                    },
                );
            }
        }
    }
}

/// How many writes are under way, so quitting or a session ending can wait for them.
#[derive(Clone, Default)]
pub(crate) struct WriteTracker(Arc<(Mutex<usize>, Condvar)>);

/// A write under way, until it's dropped.
struct Writing(WriteTracker);

impl WriteTracker {
    fn count(&self) -> std::sync::MutexGuard<'_, usize> {
        self.0.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn start(&self) -> Writing {
        *self.count() += 1;
        Writing(self.clone())
    }

    pub(crate) fn in_flight(&self) -> usize {
        *self.count()
    }

    /// Waits up to `timeout` for the writes under way to finish.
    pub(crate) fn wait(&self, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        let mut count = self.count();
        while *count > 0 {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            count = self
                .0
                .1
                .wait_timeout(count, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

impl Drop for Writing {
    fn drop(&mut self) {
        let mut count = self.0.count();
        *count = count.saturating_sub(1);
        if *count == 0 {
            self.0.0.1.notify_all();
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
        // Shared, so it's still here to report with if the thread never starts.
        let finish = Arc::new(Mutex::new(Some(finish)));
        let take = |f: &Mutex<Option<_>>| f.lock().unwrap_or_else(PoisonError::into_inner).take();
        let (events, finished) = (self.clone(), finish.clone());
        let started = thread::Builder::new().name(name.into()).spawn(move || {
            let outcome = catch_unwind(AssertUnwindSafe(work)).map_err(|p| panic_message(&*p));
            if let Some(finish) = take(&finished) {
                events.send(finish(outcome));
            }
        });
        if let Err(err) = started
            && let Some(finish) = take(&finish)
        {
            self.send(finish(Err(format!("Failed to start worker thread: {err}"))));
        }
    }
}

// ---- 4. What the prompts say ----------------------------------------------------------------

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

/// The error's message, without the "(os error N)" Windows adds.
fn friendly_io_error(error: &io::Error) -> String {
    let raw = error.to_string();
    if let Some(prefix) = raw.strip_suffix(')')
        && let Some((clean, code)) = prefix.rsplit_once(" (os error ")
        && code.chars().all(|c| c.is_ascii_digit())
    {
        return clean.trim_end().to_owned();
    }
    raw
}

/// A message with an OK button.
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
