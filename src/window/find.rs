use crate::find::FindOutcome;
use crate::ids::{EditorSlot, Focus};
use crate::shell::Shell;

use super::MainWindow;

impl MainWindow {
    pub(super) fn open_find(&mut self, replace: bool, shell: &Shell) {
        let selected = self.active_editor().model().selected_text().to_string();
        self.find.open(replace, &selected, &shell.text_system);
        self.set_focus(Focus::Editor(EditorSlot::FindQuery), shell);
        self.dirty = true;
    }

    pub(super) fn close_find(&mut self, shell: &Shell) {
        if self.find.close() {
            self.focus_active_editor(shell);
            self.dirty = true;
        }
    }

    pub(super) fn refocus_find(&mut self, shell: &Shell) {
        if self.find.is_open() {
            let slot =
                if self.find.is_replace() && self.focus == Focus::Editor(EditorSlot::FindReplace) {
                    EditorSlot::FindReplace
                } else {
                    EditorSlot::FindQuery
                };
            self.set_focus(Focus::Editor(slot), shell);
        }
        self.dirty = true;
    }

    pub(super) fn find_step(&mut self, forward: bool, shell: &Shell) {
        let timers = shell.window_timers(self.id());
        let out = self
            .find
            .step(&mut self.tabs.active_mut().editor, forward, timers);
        if matches!(out, FindOutcome::EmptyQuery) {
            self.open_find(false, shell);
            return;
        }
        if !self.find.is_open() {
            self.focus_active_editor(shell);
        }
        self.dirty = true;
    }

    pub(super) fn replace_next(&mut self, shell: &Shell) {
        let timers = shell.window_timers(self.id());
        self.find.replace_next(
            &mut self.tabs.active_mut().editor,
            &shell.text_system,
            timers,
        );
        self.dirty = true;
    }

    pub(super) fn replace_all(&mut self, shell: &Shell) {
        let timers = shell.window_timers(self.id());
        let out = self.find.replace_all(
            &mut self.tabs.active_mut().editor,
            &shell.text_system,
            timers,
        );
        if matches!(out, FindOutcome::Replaced(_)) {
            self.refocus_find(shell);
        } else {
            self.dirty = true;
        }
    }
}
