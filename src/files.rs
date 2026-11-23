use super::*;
use crate::tabs::TabId;
use crate::ui::{SaveChoice, SavePathOptions};
use document::{Encoding, Format, LoadError, default_directory, display_name, file_name};
use std::collections::VecDeque;
use std::io;

const UNTITLED_FILE_NAME: &str = "Untitled.txt";
const SAVE_AS_TITLE: &str = "Save as";
const TEXT_FILE_TYPE: (&str, &str) = ("Text documents (*.txt)", "*.txt");
const ALL_FILE_TYPE: (&str, &str) = ("All files", "*.*");
const TEXT_EXTENSION: &str = "txt";
const ENCODING_LABEL: &str = "Encoding:";
const LOSSY_ANSI_WARNING: &str = "This file contains characters in Unicode format which will be lost if you save this file as an ANSI encoded text file. To keep the Unicode information, click Cancel below and then select one of the Unicode options from the Encoding drop down list. Continue?";

#[derive(Clone, Debug, PartialEq, Eq)]
enum WorkflowStep {
    SaveTab { id: TabId, activate: bool },
    SaveTabAs(TabId),
    ConfirmDiscard { id: TabId, activate: bool },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WorkflowFinish {
    None,
    RemoveTab(TabId),
    CloseWindow,
}

/// Explicit state machine driving multi-step file save, save-as, lossy-ANSI confirmation,
/// save-all, and dirty-tab close workflows without async tasks or oneshot channels.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct FileWorkflow {
    pending: VecDeque<WorkflowStep>,
    on_finish: WorkflowFinish,
}

impl FileWorkflow {
    fn single(step: WorkflowStep, on_finish: WorkflowFinish) -> Self {
        let mut pending = VecDeque::with_capacity(1);
        pending.push_back(step);
        Self { pending, on_finish }
    }

    fn batch(steps: impl IntoIterator<Item = WorkflowStep>, on_finish: WorkflowFinish) -> Self {
        Self {
            pending: steps.into_iter().collect(),
            on_finish,
        }
    }

    fn next_step(&mut self) -> Option<WorkflowStep> {
        self.pending.pop_front()
    }

    fn prepend(&mut self, step: WorkflowStep) {
        self.pending.push_front(step);
    }
}

impl Jot {
    fn show_error(
        &mut self,
        title: impl Into<SharedString>,
        message: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.prompt(title, message, &["OK"], window, cx, |_, _, _, _| {});
    }

    fn show_save_error(
        &mut self,
        path: &Path,
        error: &io::Error,
        window: &mut Window,
        cx: &mut App,
    ) {
        let title = format!("Can't save {}", display_name(Some(path)));
        self.show_error(title, error.to_string(), window, cx);
    }

    pub(crate) fn refresh_title(&self, window: &mut Window) {
        window.set_window_title(&format!("{} - {APP_NAME}", self.tab_name(self.active)));
    }

    fn is_blank(&self, ix: usize) -> bool {
        let tab = &self.tabs[ix];
        tab.data.path.is_none() && tab.data.editor.text().len() == 0
    }

    pub(crate) fn open(&mut self, window: &mut Window, cx: &mut App) {
        let window_id = window.window_id();
        cx.prompt_for_paths(
            PathPromptOptions {
                directories: false,
                multiple: true,
                prompt: None,
            },
            move |result, cx| {
                let Ok(Some(paths)) = result else {
                    return;
                };
                cx.update_jot_window(window_id, |this, window, cx| {
                    this.open_paths(paths, window, cx);
                });
            },
        );
    }

    pub(crate) fn open_dropped(
        &mut self,
        paths: &ExternalPaths,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.open_paths(paths.paths().to_vec(), window, cx);
    }

    pub(crate) fn find_open_tab(&self, path: &Path) -> Option<usize> {
        self.tabs.iter().position(|tab| {
            tab.data
                .path
                .as_deref()
                .is_some_and(|existing| document::same_path(existing, path))
        })
    }

    pub(crate) fn open_paths(
        &mut self,
        paths: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let mut to_load: Vec<PathBuf> = Vec::new();
        let mut last_existing = None;
        for raw in paths {
            let path = std::path::absolute(&raw).unwrap_or(raw);
            if let Some(ix) = self.find_open_tab(&path) {
                last_existing = Some(ix);
                continue;
            }
            if !to_load
                .iter()
                .any(|queued| document::same_path(queued, &path))
            {
                to_load.push(path);
            }
        }
        if to_load.is_empty() {
            if let Some(ix) = last_existing {
                self.activate_tab(ix, window, cx);
            }
            return;
        }
        let window_id = window.window_id();
        cx.spawn_bg_in(
            window_id,
            move || {
                to_load
                    .into_iter()
                    .map(|path| {
                        let loaded = document::load(&path);
                        (path, loaded)
                    })
                    .collect::<Vec<_>>()
            },
            move |results, window, cx| {
                cx.with_jot(window_id, |this, cx| {
                    for (path, result) in results {
                        if let Some(ix) = this.find_open_tab(&path) {
                            this.activate_tab(ix, window, cx);
                            continue;
                        }
                        match result {
                            Ok(loaded) => {
                                let tab = Self::create_tab(Some((path, loaded)), window, cx);
                                if this.is_blank(this.active) {
                                    this.replace_tab(this.active, tab, window, cx);
                                } else {
                                    this.push_tab(tab, window, cx);
                                }
                            }
                            Err(error) => {
                                let detail: SharedString = match error {
                                    LoadError::Undecodable => {
                                        "Jot can't decode the text in this file.".into()
                                    }
                                    LoadError::Io(error) => error.to_string().into(),
                                };
                                let title = format!("Can't open {}", display_name(Some(&path)));
                                this.show_error(title, detail, window, cx);
                            }
                        }
                    }
                });
            },
        );
    }

    pub(crate) fn save(&mut self, window: &mut Window, cx: &mut App) {
        let id = self.active_tab_id();
        self.start_file_workflow(
            FileWorkflow::single(
                WorkflowStep::SaveTab {
                    id,
                    activate: false,
                },
                WorkflowFinish::None,
            ),
            window,
            cx,
        );
    }

    pub(crate) fn save_as(&mut self, window: &mut Window, cx: &mut App) {
        let id = self.active_tab_id();
        self.start_file_workflow(
            FileWorkflow::single(WorkflowStep::SaveTabAs(id), WorkflowFinish::None),
            window,
            cx,
        );
    }

    pub(crate) fn save_all(&mut self, window: &mut Window, cx: &mut App) {
        let dirty = self.dirty_tabs();
        if dirty.is_empty() {
            return;
        }
        self.start_file_workflow(
            FileWorkflow::batch(
                dirty
                    .into_iter()
                    .map(|id| WorkflowStep::SaveTab { id, activate: true }),
                WorkflowFinish::None,
            ),
            window,
            cx,
        );
    }

    fn dirty_tabs(&self) -> Vec<TabId> {
        self.tabs
            .iter()
            .filter(|tab| tab.data.is_dirty())
            .map(|tab| tab.data.id)
            .collect()
    }

    pub(crate) fn close_tab(&mut self, ix: usize, window: &mut Window, cx: &mut App) {
        if !self.tabs[ix].data.is_dirty() {
            self.remove_tab(ix, window, cx);
            return;
        }
        let id = self.tabs[ix].data.id;
        self.activate_tab(ix, window, cx);
        self.start_file_workflow(
            FileWorkflow::single(
                WorkflowStep::ConfirmDiscard {
                    id,
                    activate: false,
                },
                WorkflowFinish::RemoveTab(id),
            ),
            window,
            cx,
        );
    }

    pub(crate) fn request_close_window(&mut self, window: &mut Window, cx: &mut App) -> bool {
        let dirty = self.dirty_tabs();
        if dirty.is_empty() {
            return true;
        }
        self.start_file_workflow(
            FileWorkflow::batch(
                dirty
                    .into_iter()
                    .map(|id| WorkflowStep::ConfirmDiscard { id, activate: true }),
                WorkflowFinish::CloseWindow,
            ),
            window,
            cx,
        );
        false
    }

    fn start_file_workflow(&mut self, workflow: FileWorkflow, window: &mut Window, cx: &mut App) {
        self.file_workflow = Some(workflow);
        self.advance_file_workflow(window, cx);
    }

    fn abort_file_workflow(&mut self) {
        self.file_workflow = None;
    }

    fn advance_file_workflow(&mut self, window: &mut Window, cx: &mut App) {
        loop {
            let next = self
                .file_workflow
                .as_mut()
                .and_then(FileWorkflow::next_step);
            let Some(step) = next else {
                if let Some(workflow) = self.file_workflow.take() {
                    match workflow.on_finish {
                        WorkflowFinish::None => {}
                        WorkflowFinish::RemoveTab(id) => {
                            if let Some(ix) = self.tab_index(id) {
                                self.remove_tab(ix, window, cx);
                            }
                        }
                        WorkflowFinish::CloseWindow => window.remove_window(),
                    }
                }
                return;
            };

            match step {
                WorkflowStep::SaveTab { id, activate } => {
                    let Some(ix) = self.tab_index(id) else {
                        self.abort_file_workflow();
                        return;
                    };
                    if activate {
                        self.activate_tab(ix, window, cx);
                    }
                    match self.tabs[ix].data.path.clone() {
                        Some(path) => {
                            let format = self.tabs[ix].data.format;
                            self.begin_write_tab(ix, path, format, window, cx);
                        }
                        None => self.begin_save_tab_as(id, window, cx),
                    }
                    return;
                }
                WorkflowStep::SaveTabAs(id) => {
                    self.begin_save_tab_as(id, window, cx);
                    return;
                }
                WorkflowStep::ConfirmDiscard { id, activate } => {
                    let Some(ix) = self.tab_index(id) else {
                        continue;
                    };
                    if activate {
                        self.activate_tab(ix, window, cx);
                    }
                    let name = self.tabs[ix].data.path.as_deref().map_or_else(
                        || UNTITLED_FILE_NAME.into(),
                        |path| display_name(Some(path)),
                    );
                    self.prompt(
                        APP_NAME,
                        format!("Do you want to save changes to {name}?"),
                        &["Save", "Don't save", "Cancel"],
                        window,
                        cx,
                        move |this, answer, window, cx| match answer {
                            0 => {
                                if let Some(wf) = this.file_workflow.as_mut() {
                                    wf.prepend(WorkflowStep::SaveTab {
                                        id,
                                        activate: false,
                                    });
                                }
                                this.advance_file_workflow(window, cx);
                            }
                            1 => this.advance_file_workflow(window, cx),
                            _ => this.abort_file_workflow(),
                        },
                    );
                    return;
                }
            }
        }
    }

    fn begin_save_tab_as(&mut self, id: TabId, window: &mut Window, cx: &mut App) {
        let Some(ix) = self.tab_index(id) else {
            self.abort_file_workflow();
            return;
        };
        let current = self.tabs[ix].data.path.clone();
        let format = self.tabs[ix].data.format;
        let suggested = file_name(current.as_deref()).unwrap_or_else(|| UNTITLED_FILE_NAME.into());
        let window_id = window.window_id();
        cx.prompt_for_save_path(
            self.hwnd,
            SavePathOptions {
                title: Some(SAVE_AS_TITLE.into()),
                directory: default_directory(current.as_deref()),
                file_name: Some(suggested),
                file_types: [TEXT_FILE_TYPE, ALL_FILE_TYPE]
                    .map(|(name, spec)| (name.into(), spec.into()))
                    .into(),
                default_extension: Some(TEXT_EXTENSION.into()),
                choice: Some(SaveChoice {
                    label: ENCODING_LABEL.into(),
                    items: Encoding::SAVE_ORDER
                        .map(|encoding| encoding.label().into())
                        .into(),
                    selected: Encoding::SAVE_ORDER
                        .iter()
                        .position(|&encoding| encoding == format.encoding)
                        .unwrap_or_default(),
                }),
            },
            move |result, cx| {
                cx.update_jot_window(window_id, |this, window, cx| {
                    let Ok(Some((path, choice))) = result else {
                        this.abort_file_workflow();
                        return;
                    };
                    let format = Format {
                        encoding: choice
                            .and_then(|ix| Encoding::SAVE_ORDER.get(ix).copied())
                            .unwrap_or(format.encoding),
                        ..format
                    };
                    match this.tab_index(id) {
                        Some(ix) => this.begin_write_tab(ix, path, format, window, cx),
                        None => this.abort_file_workflow(),
                    }
                });
            },
        );
    }

    fn begin_write_tab(
        &mut self,
        ix: usize,
        path: PathBuf,
        format: Format,
        window: &mut Window,
        cx: &mut App,
    ) {
        let id = self.tabs[ix].data.id;
        let snapshot = self.tabs[ix].data.editor.begin_save();
        let text = self.tabs[ix].data.editor.text().clone();
        let path = std::path::absolute(&path).unwrap_or(path);
        let check_text = text.clone();
        let window_id = window.window_id();
        cx.spawn_bg_in(
            window_id,
            move || document::is_lossy(&check_text, format),
            move |lossy_res, window, cx| {
                cx.with_jot(window_id, |this, cx| match lossy_res {
                    Err(error) => {
                        this.show_save_error(&path, &error, window, cx);
                        this.abort_file_workflow();
                    }
                    Ok(true) => {
                        let message = format!("{}\n{LOSSY_ANSI_WARNING}", path.display());
                        this.prompt(
                            APP_NAME,
                            message,
                            &["OK", "Cancel"],
                            window,
                            cx,
                            move |this, answer, window, cx| match answer {
                                0 => this.finish_write_tab(
                                    id, snapshot, text, path, format, window, cx,
                                ),
                                1 => this.begin_save_tab_as(id, window, cx),
                                _ => this.abort_file_workflow(),
                            },
                        );
                    }
                    Ok(false) => {
                        this.finish_write_tab(id, snapshot, text, path, format, window, cx);
                    }
                });
            },
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn finish_write_tab(
        &mut self,
        id: TabId,
        snapshot: u64,
        text: Rope,
        path: PathBuf,
        format: Format,
        window: &mut Window,
        cx: &mut App,
    ) {
        let write_path = path.clone();
        let window_id = window.window_id();
        cx.spawn_bg_in(
            window_id,
            move || document::write_rope(&write_path, &text, format),
            move |result, window, cx| {
                cx.with_jot(window_id, |this, cx| match result {
                    Ok(()) => {
                        let Some(ix) = this.tab_index(id) else {
                            this.abort_file_workflow();
                            return;
                        };
                        let tab = &mut this.tabs[ix];
                        tab.data.editor.mark_saved_version(snapshot);
                        tab.data.path = Some(path);
                        tab.data.format = format;
                        this.refresh_title(window);
                        this.notify(cx);
                        this.advance_file_workflow(window, cx);
                    }
                    Err(error) => {
                        this.show_save_error(&path, &error, window, cx);
                        this.abort_file_workflow();
                    }
                });
            },
        );
    }
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
        // User chooses "Save" on tab 1 -> SaveTab(id1) runs before confirming tab 2.
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
