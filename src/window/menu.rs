use crate::ids::Focus;
use crate::menu::{FocusRequest, MenuOutcome, MenuState};
use crate::platform::clipboard;
use crate::shell::{Cx, Shell};
use crate::tabs::TabSet;

use super::MainWindow;

impl MainWindow {
    pub(crate) fn key_tips_active(&self) -> bool {
        self.menus.access_active(self.focus)
    }

    pub(crate) fn cancel_menu(&mut self, shell: &Shell) {
        let out = self.menus.cancel(self.focus);
        self.dirty |= out.dirty;
        self.apply_focus_request(out.focus, shell);
    }

    fn apply_focus_request(&mut self, req: FocusRequest, shell: &Shell) {
        match req {
            FocusRequest::Keep => {}
            FocusRequest::MenuBar => self.set_focus(Focus::MenuBar, shell),
            FocusRequest::RestoreEditor => {
                self.set_focus(Focus::Editor(self.active_slot()), shell)
            }
            FocusRequest::Editor(slot) => self.set_focus(Focus::Editor(slot), shell),
        }
    }

    pub(super) fn apply_menu_outcome(&mut self, out: MenuOutcome, cx: &mut Cx<'_>) -> bool {
        self.dirty |= out.dirty;
        self.apply_focus_request(out.focus, cx.shell);
        if let Some(cmd) = out.command {
            self.dispatch_command(cmd, cx);
            return true;
        }
        out.handled
    }
}

pub(super) fn menu_state(tabs: &TabSet, shell: &Shell) -> MenuState {
    let m = tabs.active().editor.model();
    MenuState {
        can_undo: m.can_undo(),
        can_redo: m.can_redo(),
        has_selection: !m.selected_range().is_empty(),
        has_text: m.text().len() > 0,
        clipboard_has_text: clipboard::clipboard_has_text(),
        status_bar: shell.view.status_bar,
        word_wrap: shell.view.word_wrap,
    }
}
