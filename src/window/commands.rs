use crate::command::{AppCommand, Command};
use crate::files::FileRequest;
use crate::geom::point;
use crate::ids::Focus;
use crate::platform::format_local_time_date;
use crate::shell::{Cx, Effect};

use super::{MainWindow, ZOOM_STEP};

impl MainWindow {
    pub(crate) fn dispatch_command(&mut self, cmd: Command, cx: &mut Cx<'_>) -> bool {
        match cmd {
            Command::App(app_cmd) => self.dispatch_app_command(app_cmd, cx),
            Command::Edit(edit_cmd) => {
                self.run_edit_command(self.active_slot(), edit_cmd, cx.shell)
            }
        }
    }

    pub(super) fn dispatch_app_command(&mut self, cmd: AppCommand, cx: &mut Cx<'_>) -> bool {
        if self.dialogs.is_open() {
            return false;
        }
        match cmd {
            AppCommand::Cancel => {
                if self.find.is_open() {
                    self.close_find(cx.shell);
                    return true;
                }
                return false;
            }
            AppCommand::ShowContextMenu => {
                if self.focus != Focus::Editor(self.active_slot()) {
                    return false;
                }
                if let Some(caret) = self.active_editor().caret_bounds() {
                    let anchor = point(caret.right(), caret.bottom());
                    let state = self.menu_state(cx.shell);
                    let out = self.menu_bar.open_context_menu(anchor, state, cx.ts());
                    let _ = self.apply_menu_outcome(out, cx);
                }
                return true;
            }
            AppCommand::NewWindow => cx.emit(Effect::NewWindow),
            AppCommand::Exit => cx.emit(Effect::Exit),
            AppCommand::ToggleStatusBar => cx.emit(Effect::ToggleStatusBar),
            AppCommand::ToggleWordWrap => cx.emit(Effect::ToggleWordWrap),
            AppCommand::ZoomIn => self.set_zoom(self.zoom() + ZOOM_STEP),
            AppCommand::ZoomOut => self.set_zoom(self.zoom() - ZOOM_STEP),
            AppCommand::ResetZoom => self.set_zoom(1.),
            AppCommand::CloseWindow => {
                cx.emit(Effect::File(FileRequest::CloseWindow { window: self.id() }));
            }
            AppCommand::NewTab => self.add_tab(cx.shell),
            AppCommand::CloseTab => {
                cx.emit(Effect::File(FileRequest::CloseTab {
                    window: self.id(),
                    tab: self.active_tab_id(),
                }));
            }
            AppCommand::NextTab => self.next_tab(cx.shell),
            AppCommand::PrevTab => self.prev_tab(cx.shell),
            AppCommand::SelectTab(ix) => self.select_tab(ix, cx.shell),
            AppCommand::SelectLastTab => self.select_last_tab(cx.shell),
            AppCommand::Open => {
                cx.emit(Effect::File(FileRequest::OpenDialog { window: self.id() }));
            }
            AppCommand::Save => {
                cx.emit(Effect::File(FileRequest::Save {
                    window: self.id(),
                    tab: self.active_tab_id(),
                }));
            }
            AppCommand::SaveAs => {
                cx.emit(Effect::File(FileRequest::SaveAs {
                    window: self.id(),
                    tab: self.active_tab_id(),
                }));
            }
            AppCommand::SaveAll => {
                cx.emit(Effect::File(FileRequest::SaveAll { window: self.id() }));
            }
            AppCommand::TimeDate => {
                let stamp = format_local_time_date();
                self.active_editor_mut().replace(&stamp, cx.ts());
            }
            AppCommand::GoTo => self.open_go_to(cx.shell),
            AppCommand::Find => {
                if self.has_text() {
                    self.open_find(false, cx.shell);
                }
            }
            AppCommand::Replace => {
                if self.has_text() {
                    self.open_find(true, cx.shell);
                }
            }
            AppCommand::FindNext => {
                if self.has_text() {
                    self.find_step(true, cx.shell);
                }
            }
            AppCommand::FindPrevious => {
                if self.has_text() {
                    self.find_step(false, cx.shell);
                }
            }
            AppCommand::ToggleMatchCase => {
                self.find.toggle_match_case();
                self.refocus_find(cx.shell);
            }
            AppCommand::ToggleWrapAround => {
                self.find.toggle_wrap_around();
                self.refocus_find(cx.shell);
            }
            AppCommand::ReplaceNext => self.replace_next(cx.shell),
        }
        true
    }
}
