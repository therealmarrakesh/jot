use super::*;
use document::{LoadError, default_directory, display_name};

const UNTITLED_FILE_NAME: &str = "Untitled.txt";

fn show_error(message: &str, detail: Option<&str>, window: &mut Window, cx: &mut App) {
    let answer = window.prompt(PromptLevel::Critical, message, detail, &["OK"], cx);
    cx.foreground_executor()
        .spawn(async move {
            answer.await.ok();
        })
        .detach();
}

impl Jot {
    pub(crate) fn tab_name(&self, ix: usize) -> String {
        display_name(self.tabs[ix].path.as_deref())
    }

    pub(crate) fn refresh_title(&self, window: &mut Window) {
        window.set_window_title(&format!("{} - Jot", self.tab_name(self.active)));
    }

    fn tab_index(&self, id: EntityId) -> Option<usize> {
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
                let tab = Self::create_tab(Some((path, loaded)), window, cx);
                if self.is_blank(self.active, cx) {
                    self.tabs[self.active] = tab;
                    self.activate_tab(self.active, window, cx);
                } else {
                    self.tabs.push(tab);
                    self.activate_tab(self.tabs.len() - 1, window, cx);
                }
                cx.notify();
            }
            Err(error) => {
                let detail = match error {
                    LoadError::NotUtf8 => "Jot only opens UTF-8 text files.".to_string(),
                    LoadError::Io(error) => error.to_string(),
                };
                let message = format!("Can't open {}", display_name(Some(&path)));
                show_error(&message, Some(&detail), window, cx);
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
            Some(path) => Task::ready(self.write_tab(ix, path, window, cx)),
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
        let suggested = current
            .as_deref()
            .and_then(Path::file_name)
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| UNTITLED_FILE_NAME.into());
        let target =
            cx.prompt_for_new_path(&default_directory(current.as_deref()), Some(&suggested));
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(path))) = target.await else {
                return false;
            };
            this.update_in(cx, |this, window, cx| match this.tab_index(id) {
                Some(ix) => this.write_tab(ix, path, window, cx),
                None => false,
            })
            .unwrap_or(false)
        })
    }

    fn write_tab(
        &mut self,
        ix: usize,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let tab = &self.tabs[ix];
        let text = tab.editor.read(cx).text().to_string();
        match document::save(&path, &text, tab.format) {
            Ok(()) => {
                let tab = &mut self.tabs[ix];
                tab.path = Some(path);
                tab.dirty = false;
                self.refresh_title(window);
                cx.notify();
                true
            }
            Err(error) => {
                let message = format!("Can't save {}", display_name(Some(&path)));
                show_error(&message, Some(&error.to_string()), window, cx);
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
        let message = format!("Do you want to save changes to {}?", self.tab_name(ix));
        let answer = window.prompt(
            PromptLevel::Warning,
            &message,
            None,
            &["Save", "Don't save", "Cancel"],
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
