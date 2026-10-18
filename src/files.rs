use super::*;
use document::{Encoding, Format, LoadError, default_directory, display_name};
use gpui_windows::{SaveChoice, SavePathOptions};

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
        title: String,
        message: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        drop(self.prompt(title, message, &["OK"], window, cx));
    }

    pub(crate) fn tab_name(&self, ix: usize) -> String {
        display_name(self.tabs[ix].path.as_deref())
    }

    pub(crate) fn refresh_title(&self, window: &mut Window) {
        window.set_window_title(&format!("{} - Jot", self.tab_name(self.active)));
    }

    pub(crate) fn tab_index(&self, id: EntityId) -> Option<usize> {
        self.tabs
            .iter()
            .position(|tab| tab.editor.entity_id() == id)
    }

    fn is_blank(&self, ix: usize, cx: &App) -> bool {
        let tab = &self.tabs[ix];
        tab.path.is_none() && !tab.dirty && tab.editor.read(cx).text().len() == 0
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
                for path in paths {
                    this.open_path(path, window, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn open_path(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self
            .tabs
            .iter()
            .position(|tab| tab.path.as_deref() == Some(path.as_path()))
        {
            self.activate_tab(ix, window, cx);
            return;
        }
        match document::load(&path) {
            Ok(loaded) => {
                let mut tab = Self::create_tab(Some((path, loaded)), window, cx);
                if self.is_blank(self.active, cx) {
                    tab.slot = self.tabs[self.active].slot;
                    self.tabs[self.active] = tab;
                    self.activate_tab(self.active, window, cx);
                } else {
                    self.push_tab(tab, window, cx);
                }
                cx.notify();
            }
            Err(error) => {
                let detail = match error {
                    LoadError::Undecodable => "Jot can't decode the text in this file.".to_string(),
                    LoadError::Io(error) => error.to_string(),
                };
                let title = format!("Can't open {}", display_name(Some(&path)));
                self.show_error(title, detail, window, cx);
            }
        }
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
            for id in dirty {
                let Ok(save) = this.update_in(cx, |this, window, cx| {
                    if let Some(ix) = this.tab_index(id) {
                        this.activate_tab(ix, window, cx);
                    }
                    this.save_tab(id, window, cx)
                }) else {
                    return;
                };
                if !save.await {
                    return;
                }
            }
        })
        .detach();
    }

    fn dirty_tabs(&self) -> Vec<EntityId> {
        self.tabs
            .iter()
            .filter(|tab| tab.dirty)
            .map(|tab| tab.editor.entity_id())
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
        match self.tabs[ix].path.clone() {
            Some(path) => {
                let format = self.tabs[ix].format;
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
        let current = self.tabs[ix].path.clone();
        let format = self.tabs[ix].format;
        let suggested = current
            .as_deref()
            .and_then(Path::file_name)
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| UNTITLED_FILE_NAME.into());
        let target = gpui_windows::prompt_for_save_path(
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
        let tab = &self.tabs[ix];
        let id = tab.editor.entity_id();
        let text = tab.editor.read(cx).text().to_string();
        let encoded = document::encode(&text, format);
        if !encoded.lossy {
            return Task::ready(self.commit_tab(ix, path, format, &encoded.bytes, window, cx));
        }
        let answer = self.prompt(
            "Jot",
            format!(
                "{}
{LOSSY_ANSI_WARNING}",
                path.display()
            ),
            &["OK", "Cancel"],
            window,
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await != Ok(0) {
                let Ok(save_as) =
                    this.update_in(cx, |this, window, cx| this.save_tab_as(id, window, cx))
                else {
                    return false;
                };
                return save_as.await;
            }
            this.update_in(cx, |this, window, cx| match this.tab_index(id) {
                Some(ix) => this.commit_tab(ix, path, format, &encoded.bytes, window, cx),
                None => false,
            })
            .unwrap_or(false)
        })
    }

    fn commit_tab(
        &mut self,
        ix: usize,
        path: PathBuf,
        format: Format,
        bytes: &[u8],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        match document::write(&path, bytes) {
            Ok(()) => {
                let tab = &mut self.tabs[ix];
                tab.path = Some(path);
                tab.format = format;
                tab.dirty = false;
                self.refresh_title(window);
                cx.notify();
                true
            }
            Err(error) => {
                let title = format!("Can't save {}", display_name(Some(&path)));
                self.show_error(title, error.to_string(), window, cx);
                false
            }
        }
    }

    pub(crate) fn close_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if !self.tabs[ix].dirty {
            self.remove_tab(ix, window, cx);
            return;
        }
        let id = self.tabs[ix].editor.entity_id();
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
        let name = self.tabs[ix]
            .path
            .as_deref()
            .map(|path| display_name(Some(path)))
            .unwrap_or_else(|| UNTITLED_FILE_NAME.into());
        let answer = self.prompt(
            "Jot",
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
        if dirty.is_empty() || self.close_confirmed {
            return true;
        }
        cx.spawn_in(window, async move |this, cx| {
            for id in dirty {
                let Ok(confirm) = this.update_in(cx, |this, window, cx| {
                    if let Some(ix) = this.tab_index(id) {
                        this.activate_tab(ix, window, cx);
                    }
                    this.confirm_discard(id, window, cx)
                }) else {
                    return;
                };
                if !confirm.await {
                    return;
                }
            }
            this.update_in(cx, |this, window, _| {
                this.close_confirmed = true;
                window.remove_window();
            })
            .ok();
        })
        .detach();
        false
    }
}
