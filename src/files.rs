use super::*;
use document::{Encoding, Format, LoadError, default_directory, display_name, file_name};
use gpui::{SaveChoice, SavePathOptions};
use std::io;

const UNTITLED_FILE_NAME: &str = "Untitled.txt";
const SAVE_AS_TITLE: &str = "Save as";
const TEXT_FILE_TYPE: (&str, &str) = ("Text documents (*.txt)", "*.txt");
const ALL_FILE_TYPE: (&str, &str) = ("All files", "*.*");
const TEXT_EXTENSION: &str = "txt";
const ENCODING_LABEL: &str = "Encoding:";
const LOSSY_ANSI_WARNING: &str = "This file contains characters in Unicode format which will be lost if you save this file as an ANSI encoded text file. To keep the Unicode information, click Cancel below and then select one of the Unicode options from the Encoding drop down list. Continue?";

impl Jot {
    fn show_error(
        &mut self,
        title: impl Into<SharedString>,
        message: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        drop(self.prompt(title, message, &["OK"], window, cx));
    }

    fn show_save_error(
        &mut self,
        path: &Path,
        error: &io::Error,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let title = format!("Can't save {}", display_name(Some(path)));
        self.show_error(title, error.to_string(), window, cx);
    }

    pub(crate) fn refresh_title(&self, window: &mut Window) {
        window.set_window_title(&format!("{} - {APP_NAME}", self.tab_name(self.active)));
    }

    fn is_blank(&self, ix: usize, cx: &App) -> bool {
        let tab = &self.tabs[ix];
        tab.data.path.is_none() && tab.data.editor.read(cx).text().len() == 0
    }

    pub(crate) fn open(&mut self, _: &Open, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: None,
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            this.update_in(cx, |this, window, cx| {
                this.open_paths(paths, window, cx);
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn open_dropped(
        &mut self,
        paths: &ExternalPaths,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_paths(paths.paths().to_vec(), window, cx);
    }

    fn find_open_tab(&self, path: &Path) -> Option<usize> {
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
        cx: &mut Context<Self>,
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
        cx.spawn_in(window, async move |this, cx| {
            let results = cx
                .background_executor()
                .spawn(async move {
                    to_load
                        .into_iter()
                        .map(|path| {
                            let loaded = document::load(&path);
                            (path, loaded)
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                for (path, result) in results {
                    if let Some(ix) = this.find_open_tab(&path) {
                        this.activate_tab(ix, window, cx);
                        continue;
                    }
                    match result {
                        Ok(loaded) => {
                            let tab = Self::create_tab(Some((path, loaded)), window, cx);
                            if this.is_blank(this.active, cx) {
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
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn save(&mut self, _: &Save, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.active_editor().entity_id();
        self.save_tab(id, window, cx).detach();
    }

    pub(crate) fn save_as(&mut self, _: &SaveAs, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.active_editor().entity_id();
        self.save_tab_as(id, window, cx).detach();
    }

    pub(crate) fn save_all(&mut self, _: &SaveAll, window: &mut Window, cx: &mut Context<Self>) {
        let dirty = self.dirty_tabs();
        cx.spawn_in(window, async move |this, cx| {
            visit_tabs(&this, dirty, Self::save_tab, cx).await;
        })
        .detach();
    }

    fn dirty_tabs(&self) -> Vec<EntityId> {
        self.tabs
            .iter()
            .filter(|tab| tab.data.dirty)
            .map(|tab| tab.data.editor.entity_id())
            .collect()
    }

    fn save_tab(
        &mut self,
        id: EntityId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<bool> {
        let Some(ix) = self.tab_index(id) else {
            return Task::ready(false);
        };
        match self.tabs[ix].data.path.clone() {
            Some(path) => {
                let format = self.tabs[ix].data.format;
                self.write_tab(ix, path, format, window, cx)
            }
            None => self.save_tab_as(id, window, cx),
        }
    }

    fn save_tab_as(
        &mut self,
        id: EntityId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<bool> {
        let Some(ix) = self.tab_index(id) else {
            return Task::ready(false);
        };
        let current = self.tabs[ix].data.path.clone();
        let format = self.tabs[ix].data.format;
        let suggested = file_name(current.as_deref()).unwrap_or_else(|| UNTITLED_FILE_NAME.into());
        let target = gpui::prompt_for_save_path(
            self.hwnd,
            cx.foreground_executor(),
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
        );
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some((path, choice)))) = target.await else {
                return false;
            };
            let format = Format {
                encoding: choice
                    .and_then(|ix| Encoding::SAVE_ORDER.get(ix).copied())
                    .unwrap_or(format.encoding),
                ..format
            };
            let Ok(write) = this.update_in(cx, |this, window, cx| match this.tab_index(id) {
                Some(ix) => this.write_tab(ix, path, format, window, cx),
                None => Task::ready(false),
            }) else {
                return false;
            };
            write.await
        })
    }

    fn write_tab(
        &mut self,
        ix: usize,
        path: PathBuf,
        format: Format,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<bool> {
        let editor = self.tabs[ix].data.editor.clone();
        let id = editor.entity_id();
        let snapshot = editor.update(cx, |editor, _| editor.begin_save());
        let text = editor.read(cx).text().clone();
        let path = std::path::absolute(&path).unwrap_or(path);
        cx.spawn_in(window, async move |this, cx| {
            let check_text = text.clone();
            let lossy = match cx
                .background_executor()
                .spawn(async move { document::is_lossy(&check_text, format) })
                .await
            {
                Ok(lossy) => lossy,
                Err(error) => {
                    this.update_in(cx, |this, window, cx| {
                        this.show_save_error(&path, &error, window, cx);
                    })
                    .ok();
                    return false;
                }
            };
            if lossy {
                let Ok(answer) = this.update_in(cx, |this, window, cx| {
                    this.prompt(
                        APP_NAME,
                        format!("{}\n{LOSSY_ANSI_WARNING}", path.display()),
                        &["OK", "Cancel"],
                        window,
                        cx,
                    )
                }) else {
                    return false;
                };
                match answer.await {
                    Ok(0) => {}
                    Ok(1) => {
                        let Ok(save_as) =
                            this.update_in(cx, |this, window, cx| this.save_tab_as(id, window, cx))
                        else {
                            return false;
                        };
                        return save_as.await;
                    }
                    _ => return false,
                }
            }
            let write_path = path.clone();
            let result = cx
                .background_executor()
                .spawn(async move { document::write_rope(&write_path, &text, format) })
                .await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(()) => {
                    let Some(ix) = this.tab_index(id) else {
                        return false;
                    };
                    let tab = &mut this.tabs[ix];
                    let clean = tab.data.editor.update(cx, |editor, _| {
                        editor.mark_saved_version(snapshot);
                        editor.is_clean()
                    });
                    tab.data.path = Some(path);
                    tab.data.format = format;
                    tab.data.dirty = !clean;
                    this.refresh_title(window);
                    cx.notify();
                    true
                }
                Err(error) => {
                    this.show_save_error(&path, &error, window, cx);
                    false
                }
            })
            .unwrap_or(false)
        })
    }

    pub(crate) fn close_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if !self.tabs[ix].data.dirty {
            self.remove_tab(ix, window, cx);
            return;
        }
        let id = self.tabs[ix].data.editor.entity_id();
        self.activate_tab(ix, window, cx);
        let confirm = self.confirm_discard(id, window, cx);
        cx.spawn_in(window, async move |this, cx| {
            if confirm.await {
                this.update_in(cx, |this, window, cx| {
                    if let Some(ix) = this.tab_index(id) {
                        this.remove_tab(ix, window, cx);
                    }
                })
                .ok();
            }
        })
        .detach();
    }

    fn confirm_discard(
        &mut self,
        id: EntityId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<bool> {
        let Some(ix) = self.tab_index(id) else {
            return Task::ready(true);
        };
        let name = self.tabs[ix].data.path.as_deref().map_or_else(
            || UNTITLED_FILE_NAME.into(),
            |path| display_name(Some(path)),
        );
        let answer = self.prompt(
            APP_NAME,
            format!("Do you want to save changes to {name}?"),
            &["Save", "Don't save", "Cancel"],
            window,
            cx,
        );
        cx.spawn_in(window, async move |this, cx| match answer.await {
            Ok(0) => {
                let Ok(save) = this.update_in(cx, |this, window, cx| this.save_tab(id, window, cx))
                else {
                    return false;
                };
                save.await
            }
            Ok(1) => true,
            _ => false,
        })
    }

    pub(crate) fn request_close_window(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let dirty = self.dirty_tabs();
        if dirty.is_empty() {
            return true;
        }
        cx.spawn_in(window, async move |this, cx| {
            if visit_tabs(&this, dirty, Self::confirm_discard, cx).await {
                this.update_in(cx, |_, window, _| window.remove_window())
                    .ok();
            }
        })
        .detach();
        false
    }
}

async fn visit_tabs(
    this: &WeakEntity<Jot>,
    ids: Vec<EntityId>,
    step: fn(&mut Jot, EntityId, &mut Window, &mut Context<Jot>) -> Task<bool>,
    cx: &mut AsyncWindowContext,
) -> bool {
    for id in ids {
        let Ok(task) = this.update_in(cx, |this, window, cx| {
            if let Some(ix) = this.tab_index(id) {
                this.activate_tab(ix, window, cx);
            }
            step(this, id, window, cx)
        }) else {
            return false;
        };
        if !task.await {
            return false;
        }
    }
    true
}
