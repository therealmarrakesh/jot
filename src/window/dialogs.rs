use crate::dialog::DialogKind;
use crate::editor::RopeExt as _;
use crate::files::{Prompt, PromptId};
use crate::ids::{EditorSlot, Focus};
use crate::shell::{Cx, Effect, Shell};

use super::MainWindow;

impl MainWindow {
    pub(crate) fn show_prompt(&mut self, prompt: Prompt, shell: &Shell) {
        if let Some(next_focus) = self.dialogs.enqueue(DialogKind::Prompt(prompt)) {
            self.cancel_menu(shell);
            self.set_focus(next_focus, shell);
            self.dirty = true;
        }
    }

    fn show_dialog_now(&mut self, kind: DialogKind, shell: &Shell) {
        self.cancel_menu(shell);
        let next_focus = self.dialogs.show_now(kind);
        self.set_focus(next_focus, shell);
        self.dirty = true;
    }

    fn close_dialogs(&mut self, shell: &Shell) {
        if let Some(next_focus) = self.dialogs.dismiss() {
            self.cancel_menu(shell);
            self.set_focus(next_focus, shell);
        } else {
            self.focus_active_editor(shell);
        }
        self.dirty = true;
    }

    pub(crate) fn open_go_to(&mut self, shell: &Shell) {
        let m = self.active_editor().model();
        let row = m.text().offset_to_point(m.cursor()).row;
        self.show_go_to(Some(row + 1), shell);
    }

    fn show_go_to(&mut self, line: Option<usize>, shell: &Shell) {
        self.show_dialog_now(DialogKind::GoTo, shell);
        if let Some(line) = line {
            self.dialogs
                .line
                .set_value(&line.to_string(), &shell.text_system);
        }
        self.dialogs.line.select_all();
        self.set_focus(Focus::Editor(EditorSlot::GoToLine), shell);
        self.dirty = true;
    }

    fn submit_go_to(&mut self, shell: &Shell) {
        let value = self.dialogs.line.model().value();
        let lines = self.active_editor().model().text().lines_len();
        match value.parse::<usize>() {
            Ok(line) if (1..=lines).contains(&line) => {
                self.close_dialogs(shell);
                self.active_editor_mut().go_to_line(line - 1);
                self.dirty = true;
            }
            _ => {
                self.show_dialog_now(DialogKind::LineOutOfRange, shell);
            }
        }
    }

    pub(crate) fn drain_cancelled_prompts(&mut self) -> Vec<PromptId> {
        self.dialogs.drain_prompt_ids()
    }

    pub(super) fn finish_dialog(&mut self, button: Option<usize>, cx: &mut Cx<'_>) {
        let Some(kind) = self.dialogs.take_active() else {
            return;
        };
        match kind {
            DialogKind::GoTo if button == Some(0) => self.submit_go_to(cx.shell),
            DialogKind::GoTo => self.close_dialogs(cx.shell),
            DialogKind::LineOutOfRange => self.show_go_to(None, cx.shell),
            DialogKind::Prompt(prompt) => {
                self.close_dialogs(cx.shell);
                if let Some((id, _)) = prompt.reply {
                    let answer = button
                        .and_then(|ix| prompt.choices.get(ix))
                        .map_or(prompt.cancel, |&(_, a)| a);
                    cx.emit(Effect::PromptResponse { id, answer });
                }
            }
        }
    }
}
