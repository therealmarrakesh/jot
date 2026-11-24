use crate::APP_NAME;
use crate::document::{self, DocPath, Format};
use crate::editor::Rope;
use crate::ids::{EditorSlot, Focus, HitId, TabHit, TabId};
use crate::shell::Shell;
use crate::tabs::{Removal, TabData};

use super::chrome::HEADER_WIDTH;
use super::{EDITOR_PADDINGS, InputMode, MainWindow, ZOOM_MAX, ZOOM_MIN, ZOOM_STEP};

impl MainWindow {
    pub(crate) fn begin_tab_save(&mut self, tab: TabId) -> Option<(u64, Rope)> {
        let ed = self.tabs.editor_mut(tab)?;
        let snapshot = ed.begin_save();
        let text = ed.model().text().clone();
        Some((snapshot, text))
    }

    pub(crate) fn refresh_title(&self) {
        self.win.window.set_title(&format!(
            "{} - {APP_NAME}",
            self.tabs.tab_name(self.tabs.active_index())
        ));
    }

    pub(crate) fn zoom(&self) -> f32 {
        self.tabs.active().zoom
    }

    pub(crate) fn set_zoom(&mut self, zoom: f32) {
        self.tabs.active_mut().zoom =
            ((zoom / ZOOM_STEP).round() * ZOOM_STEP).clamp(ZOOM_MIN, ZOOM_MAX);
        self.dirty = true;
    }

    pub(crate) fn set_word_wrap(&mut self, shell: &Shell) {
        let word_wrap = shell.view.word_wrap;
        self.tabs
            .for_each_editor_mut(|ed| ed.set_soft_wrap(word_wrap, &shell.text_system));
        self.dirty = true;
    }

    pub(crate) fn select_tab_only(&mut self, ix: usize) {
        if self.tabs.active_index() != ix {
            self.win.window.hwnd().complete_ime_composition();
            self.active_editor_mut().on_blur();
        }
        self.tabs.select(ix);
        self.find.clear_notice();
        if self.find.is_open() && !self.has_text() {
            self.find.close();
        }
        self.refresh_title();
        self.dirty = true;
    }

    pub(crate) fn activate_tab(&mut self, ix: usize, shell: &Shell) {
        self.select_tab_only(ix);
        if !matches!(self.mode(), InputMode::Dialog | InputMode::Popup) {
            self.focus_active_editor(shell);
        }
    }

    pub(crate) fn add_tab(&mut self, shell: &Shell) {
        let data = TabData::create(None, shell.view.word_wrap, EDITOR_PADDINGS);
        self.push_tab(data, shell);
    }

    pub(crate) fn push_tab(&mut self, data: TabData, shell: &Shell) {
        let ix = self.tabs.push(data);
        self.activate_tab(ix, shell);
    }

    pub(crate) fn replace_tab(&mut self, ix: usize, data: TabData, shell: &Shell) {
        self.tabs.replace(ix, data);
        self.activate_tab(ix, shell);
    }

    pub(crate) fn remove_tab(&mut self, ix: usize, shell: &Shell) {
        let pointer_in_strip = self.ui.pointer().is_some_and(|p| {
            self.ui
                .frame()
                .rect_of(HitId::Tab(TabHit::Strip))
                .is_some_and(|r| r.contains(p.pos))
        });
        match self.tabs.remove(ix, pointer_in_strip) {
            Removal::LastTab => self.pending_close = true,
            Removal::Removed {
                id,
                closed_active,
                next_active,
            } => {
                self.dialogs.close_for_tab(id);
                self.finish_removal(closed_active, next_active, shell);
            }
        }
    }

    fn finish_removal(&mut self, closed_active: bool, next_active: usize, shell: &Shell) {
        let focus_in_find = matches!(
            self.focus,
            Focus::Editor(EditorSlot::FindQuery | EditorSlot::FindReplace)
        );
        if closed_active || !focus_in_find {
            self.activate_tab(next_active, shell);
        } else {
            self.select_tab_only(next_active);
        }
    }

    pub(crate) fn drag_tab(&mut self, id: TabId, x: f32, grab: f32, start: f32) {
        self.tabs.drag(id, x - HEADER_WIDTH, grab, start - HEADER_WIDTH);
        self.dirty = true;
    }

    pub(crate) fn release_tab(&mut self) {
        if self.tabs.release_drag() {
            self.dirty = true;
        }
    }

    pub(crate) fn take_tab(&mut self, id: TabId, shell: &Shell) -> Option<TabData> {
        let (data, outcome) = self.tabs.take(id)?;
        match outcome {
            Removal::LastTab => self.pending_close = true,
            Removal::Removed {
                closed_active,
                next_active,
                ..
            } => self.finish_removal(closed_active, next_active, shell),
        }
        Some(data)
    }

    pub(crate) fn insert_dragged_tab(&mut self, data: TabData, x: f32, grab: f32, shell: &Shell) {
        let ix = self.tabs.insert_dragged(data, x - HEADER_WIDTH, grab);
        self.activate_tab(ix, shell);
    }

    pub(crate) fn restore_tab(&mut self, data: TabData, ix: usize, shell: &Shell) {
        let ix = self.tabs.restore(data, ix);
        self.activate_tab(ix, shell);
    }

    pub(crate) fn return_tab(&mut self, id: TabId, ix: usize, shell: &Shell) {
        if let Some(ix) = self.tabs.return_tab(id, ix) {
            self.activate_tab(ix, shell);
        }
        self.dirty = true;
    }

    pub(super) fn scroll_tabs(&mut self, delta: f32) -> bool {
        let moved = self.tabs.scroll_by(delta);
        self.dirty = true;
        moved
    }

    pub(super) fn next_tab(&mut self, shell: &Shell) {
        if self.tabs.len() <= 1 {
            return;
        }
        let next = (self.tabs.active_index() + 1) % self.tabs.len();
        self.activate_tab(next, shell);
    }

    pub(super) fn prev_tab(&mut self, shell: &Shell) {
        if self.tabs.len() <= 1 {
            return;
        }
        let prev = (self.tabs.active_index() + self.tabs.len() - 1) % self.tabs.len();
        self.activate_tab(prev, shell);
    }

    pub(super) fn select_tab(&mut self, ix: usize, shell: &Shell) {
        if ix >= self.tabs.len() || ix == self.tabs.active_index() {
            return;
        }
        self.activate_tab(ix, shell);
    }

    pub(super) fn select_last_tab(&mut self, shell: &Shell) {
        let last = self.tabs.len().saturating_sub(1);
        if last == self.tabs.active_index() {
            return;
        }
        self.activate_tab(last, shell);
    }

    pub(crate) fn adopt_loaded_file(&mut self, loaded: document::Loaded, shell: &Shell) {
        let data = TabData::create(Some(loaded), shell.view.word_wrap, EDITOR_PADDINGS);
        let active = self.tabs.active_index();
        if self.tabs.is_blank(active) {
            self.replace_tab(active, data, shell);
        } else {
            self.push_tab(data, shell);
        }
    }

    pub(crate) fn mark_tab_saved(
        &mut self,
        tab: TabId,
        snapshot: u64,
        doc_path: DocPath,
        format: Format,
    ) {
        if self.tabs.mark_saved(tab, snapshot, doc_path, format) {
            self.refresh_title();
            self.dirty = true;
        }
    }
}
