use crate::command::{AppCommand, app_shortcut, edit_shortcut};
use crate::dialog::DialogKey;
use crate::files::FileRequest;
use crate::find::FindKey;
use crate::geom::{Point, point};
use crate::ids::{EditorSlot, Focus, HitId};
use crate::menu::Entry;
use crate::platform::{KeyEvent, Modifiers, MouseButton};
use crate::shell::{AppEvent, Cx, Effect};
use crate::tabs::{SCROLL_REPEAT_DELAY, SCROLL_REPEAT_INTERVAL, SCROLL_STEP, WHEEL_SHARE};

use super::chrome::STRIP_TOP;
use super::{InputMode, MainWindow, ZOOM_STEP};

impl MainWindow {
    pub(crate) fn on_mouse_move(
        &mut self,
        pos: Point,
        pressed_button: Option<MouseButton>,
        cx: &mut Cx<'_>,
    ) {
        self.dirty |= self.ui.on_mouse_move(pos, pressed_button.is_some());

        let top = self.ui.top_hit();
        if !matches!(top, Some(HitId::TabScrollLeft | HitId::TabScrollRight)) {
            self.tabs.stop_scroll_repeat();
        }
        let mode = self.mode();
        if mode != InputMode::Dialog {
            let state = self.menu_state(cx.shell);
            let menu_out = self
                .menu_bar
                .on_hover_change(top, state, cx.ts(), &mut self.ui);
            let _ = self.apply_menu_outcome(menu_out, cx);
        }

        if self.tabs.is_frozen()
            && !self
                .ui
                .frame()
                .rect_of(HitId::TabStrip)
                .is_some_and(|r| r.contains(pos))
        {
            self.dirty |= self.tabs.thaw_if_frozen();
        }

        let dragging_left = pressed_button == Some(MouseButton::Left);
        let active_slot = self.active_slot();
        if let Some(axis) = self.active_editor().scrollbar_drag_axis() {
            let track_rect = self.ui.frame().rect_of(HitId::ScrollTrack(active_slot, axis));
            if let Some(Some(notify)) = self.with_editor(active_slot, cx.shell, |ed, timers| {
                ed.on_scrollbar_mouse_move(pos, dragging_left, track_rect, timers)
            }) {
                self.dirty |= notify;
                self.sync_cursor(pos);
                return;
            }
        }
        if let Some(slot) = self.focused_editor_slot()
            && slot != active_slot
            && self
                .with_editor(slot, cx.shell, |ed, timers| {
                    ed.on_mouse_move(pos, dragging_left, true, timers)
                })
                .unwrap_or(false)
        {
            self.dirty = true;
        }
        let active_focused = self.focus == Focus::Editor(active_slot);
        if matches!(mode, InputMode::Normal | InputMode::MenuBar)
            || self.active_editor().is_selecting()
        {
            self.dirty |= self
                .with_editor(active_slot, cx.shell, |ed, timers| {
                    ed.on_mouse_move(pos, dragging_left, active_focused, timers)
                })
                .unwrap_or(false);
        }

        self.sync_cursor(pos);
    }

    pub(crate) fn on_mouse_leave(&mut self) {
        self.dirty |= self.ui.on_mouse_leave();
        self.tabs.stop_scroll_repeat();
        if self.tabs.is_frozen() {
            self.dirty |= self.tabs.thaw_if_frozen();
        }
    }

    pub(crate) fn on_capture_lost(&mut self) {
        self.middle_pressed_tab = None;
        self.tabs.stop_scroll_repeat();
        self.dirty |= self.ui.on_capture_lost();
    }

    pub(crate) fn on_mouse_down(
        &mut self,
        button: MouseButton,
        pos: Point,
        modifiers: Modifiers,
        click_count: usize,
        cx: &mut Cx<'_>,
    ) {
        let menu_out = self.menu_bar.pointer_down(self.focus);
        let _ = self.apply_menu_outcome(menu_out, cx);
        let chain = self.ui.on_mouse_down(pos, button);
        let top = chain.first().copied();
        let in_menu_ui = chain.iter().any(|h| {
            matches!(
                h,
                HitId::MenuPanel | HitId::SubmenuPanel | HitId::MenuTrigger(_)
            )
        });
        let tab_hit = chain.iter().find_map(|hit| match hit {
            HitId::Tab(id) => Some(*id),
            _ => None,
        });
        let editor_hit = chain.iter().find_map(|hit| match hit {
            HitId::Editor(slot) => Some(*slot),
            _ => None,
        });

        let mode = self.mode();
        if mode == InputMode::Popup && !in_menu_ui {
            let out = self.menu_bar.close();
            let _ = self.apply_menu_outcome(out, cx);
        }

        if button == MouseButton::Middle && mode != InputMode::Dialog {
            self.middle_pressed_tab = tab_hit;
            return;
        }

        if button == MouseButton::Left {
            match top {
                Some(
                    HitId::FieldClear(_)
                    | HitId::DialogButton(_)
                    | HitId::DialogBackdrop
                    | HitId::DialogCard
                    | HitId::FindCard
                    | HitId::FindToggleReplace
                    | HitId::FindNext
                    | HitId::FindPrev
                    | HitId::FindOptions
                    | HitId::FindClose
                    | HitId::FindReplace
                    | HitId::FindReplaceAll
                    | HitId::MenuPanel
                    | HitId::MenuItem { .. }
                    | HitId::MenuCommand { .. }
                    | HitId::SubmenuPanel
                    | HitId::SubmenuItem { .. }
                    | HitId::TabClose(_),
                ) => return,
                Some(HitId::MenuTrigger(kind)) => {
                    let state = self.menu_state(cx.shell);
                    let out = self.menu_bar.toggle_trigger(kind, state, cx.ts());
                    let _ = self.apply_menu_outcome(out, cx);
                    return;
                }
                Some(HitId::Tab(id)) => {
                    if let Some(ix) = self.tabs.index_of(id) {
                        self.activate_tab(ix, cx.shell);
                        let left = self
                            .ui
                            .frame()
                            .rect_of(HitId::Tab(id))
                            .map_or(pos.x, |r| r.origin.x);
                        let grab = point(pos.x - left, pos.y - STRIP_TOP);
                        cx.emit(Effect::StartTabDrag {
                            window: self.id(),
                            tab: id,
                            index: ix,
                            grab,
                            position: pos,
                        });
                    }
                    return;
                }
                Some(HitId::TabScrollLeft) => {
                    if self.scroll_tabs(-SCROLL_STEP) {
                        let wid = self.id();
                        self.tabs.set_scroll_repeat(cx.jobs().every(
                            SCROLL_REPEAT_DELAY,
                            SCROLL_REPEAT_INTERVAL,
                            move || AppEvent::TabScrollRepeat {
                                window: wid,
                                step: -SCROLL_STEP,
                            },
                        ));
                    }
                    return;
                }
                Some(HitId::TabScrollRight) => {
                    if self.scroll_tabs(SCROLL_STEP) {
                        let wid = self.id();
                        self.tabs.set_scroll_repeat(cx.jobs().every(
                            SCROLL_REPEAT_DELAY,
                            SCROLL_REPEAT_INTERVAL,
                            move || AppEvent::TabScrollRepeat {
                                window: wid,
                                step: SCROLL_STEP,
                            },
                        ));
                    }
                    return;
                }
                Some(HitId::ScrollThumb(slot, axis))
                    if slot == self.active_slot()
                        && self.active_editor().scrollbar_interactive(axis) =>
                {
                    if let Some(track) = self.ui.frame().rect_of(HitId::ScrollTrack(slot, axis)) {
                        self.active_editor_mut()
                            .on_scrollbar_mouse_down(axis, true, pos, track);
                    }
                    return;
                }
                Some(HitId::ScrollTrack(slot, axis))
                    if slot == self.active_slot()
                        && self.active_editor().scrollbar_interactive(axis) =>
                {
                    if let Some(track) = self.ui.frame().rect_of(HitId::ScrollTrack(slot, axis)) {
                        self.active_editor_mut()
                            .on_scrollbar_mouse_down(axis, false, pos, track);
                    }
                    return;
                }
                _ => {}
            }
        } else if in_menu_ui {
            return;
        }

        if let Some(slot) = editor_hit {
            self.native.hwnd().complete_ime_composition();
            self.set_focus(Focus::Editor(slot), cx.shell);
            self.with_editor(slot, cx.shell, |ed, timers| {
                ed.on_mouse_down(button, pos, modifiers, click_count, timers);
            });
        }
    }

    pub(crate) fn on_mouse_up(&mut self, button: MouseButton, pos: Point, cx: &mut Cx<'_>) {
        let (clicked, chain) = self.ui.on_mouse_up(pos, button);
        let top = chain.first().copied();
        let tab_over = chain.iter().find_map(|hit| match hit {
            HitId::Tab(id) => Some(*id),
            _ => None,
        });
        let menu_release = if button == MouseButton::Left
            && clicked.is_none()
            && self.mode() == InputMode::Popup
        {
            match top {
                hit @ Some(
                    HitId::MenuItem { .. }
                    | HitId::SubmenuItem { .. }
                    | HitId::MenuCommand { .. },
                ) => hit,
                _ => None,
            }
        } else {
            None
        };
        let activated = clicked.or(menu_release);
        self.tabs.stop_scroll_repeat();

        if button == MouseButton::Middle {
            let pressed = self.middle_pressed_tab.take();
            if let Some(id) = pressed
                && pressed == tab_over
                && self.tabs.index_of(id).is_some()
            {
                cx.emit(Effect::File(FileRequest::CloseTab {
                    window: self.id(),
                    tab: id,
                }));
            }
            return;
        }

        let active_slot = self.active_slot();
        if let Some(axis) = self.active_editor().scrollbar_drag_axis() {
            let track_rect = self.ui.frame().rect_of(HitId::ScrollTrack(active_slot, axis));
            self.active_editor_mut()
                .on_scrollbar_mouse_up(pos, track_rect);
        }
        if let Some(slot) = self.focused_editor_slot()
            && slot != active_slot
        {
            self.with_editor(slot, cx.shell, |ed, timers| {
                ed.on_mouse_up(button, timers);
            });
        }
        let open_ctx = self
            .with_editor(active_slot, cx.shell, |ed, timers| {
                ed.on_mouse_up(button, timers)
            })
            .flatten();
        if let Some(anchor) = open_ctx {
            let state = self.menu_state(cx.shell);
            let out = self.menu_bar.open_context_menu(anchor, state, cx.ts());
            let _ = self.apply_menu_outcome(out, cx);
            return;
        }

        match activated {
            Some(HitId::DialogButton(ix)) => self.press_dialog_button(ix, cx),
            Some(HitId::FieldClear(slot)) => {
                if let Some(ed) = self.editor_by_slot_mut(slot) {
                    ed.set_value("", cx.ts());
                }
                self.set_focus(Focus::Editor(slot), cx.shell);
            }
            Some(HitId::FindToggleReplace) => {
                self.find.toggle_replace();
                self.refocus_find(cx.shell);
            }
            Some(HitId::FindNext) => self.find_step(true, cx.shell),
            Some(HitId::FindPrev) => self.find_step(false, cx.shell),
            Some(HitId::FindOptions) => {
                let entries = vec![
                    Entry::toggle(
                        "Match case",
                        self.find.match_case(),
                        AppCommand::ToggleMatchCase,
                    ),
                    Entry::toggle(
                        "Wrap around",
                        self.find.wrap_around(),
                        AppCommand::ToggleWrapAround,
                    ),
                ];
                let out = self.menu_bar.open_flyout(
                    entries,
                    HitId::FindOptions,
                    EditorSlot::FindQuery,
                    cx.ts(),
                );
                let _ = self.apply_menu_outcome(out, cx);
            }
            Some(HitId::FindClose) => self.close_find(cx.shell),
            Some(HitId::FindReplace) => self.replace_next(cx.shell),
            Some(HitId::FindReplaceAll) => self.replace_all(cx.shell),
            Some(HitId::MenuItem { ix, .. }) => {
                let out = self.menu_bar.activate((ix, None), false, cx.ts());
                let _ = self.apply_menu_outcome(out, cx);
            }
            Some(HitId::SubmenuItem { ix: sub_ix, .. }) => {
                let out = self.menu_bar.activate((0, Some(sub_ix)), false, cx.ts());
                let _ = self.apply_menu_outcome(out, cx);
            }
            Some(HitId::MenuCommand { ix, .. }) => {
                let out = self.menu_bar.click_command(ix);
                let _ = self.apply_menu_outcome(out, cx);
            }
            Some(HitId::TabClose(id)) => {
                if self.tabs.index_of(id).is_some() {
                    cx.emit(Effect::File(FileRequest::CloseTab {
                        window: self.id(),
                        tab: id,
                    }));
                }
            }
            Some(HitId::NewTab) => self.add_tab(cx.shell),
            Some(HitId::ThemeToggle) => cx.emit(Effect::ToggleTheme),
            _ => {}
        }
    }

    pub(crate) fn on_scroll_wheel(
        &mut self,
        pos: Point,
        delta_lines: Point,
        page_scroll: bool,
        modifiers: Modifiers,
    ) {
        if matches!(self.mode(), InputMode::Dialog | InputMode::Popup) {
            return;
        }
        let chain = self.ui.frame().hit_chain(pos);
        if self.tabs.overflow() && chain.contains(&HitId::TabList) {
            let viewport = self.tabs.scroll_viewport();
            let notches = if delta_lines.y != 0. {
                delta_lines.y
            } else {
                delta_lines.x
            };
            self.scroll_tabs(-notches * WHEEL_SHARE * viewport);
            return;
        }

        if modifiers.ctrl && chain.contains(&HitId::EditorArea) {
            if delta_lines.y > 0. {
                self.set_zoom(self.zoom() + ZOOM_STEP);
            } else if delta_lines.y < 0. {
                self.set_zoom(self.zoom() - ZOOM_STEP);
            }
            return;
        }

        let lh = self.line_height();
        if self
            .active_editor_mut()
            .on_scroll_wheel(delta_lines, page_scroll, modifiers, lh)
        {
            self.dirty = true;
        }
    }

    pub(crate) fn on_key_down(&mut self, ev: KeyEvent, cx: &mut Cx<'_>) -> bool {
        self.ui.mark_keyboard_input();

        if self.mode() == InputMode::Dialog {
            if let Some(slot) = self.focused_editor_slot() {
                self.with_editor(slot, cx.shell, |ed, timers| ed.pause_blink(timers));
            }
            return match self.dialogs.on_key(ev, self.focus) {
                DialogKey::PassThrough => false,
                DialogKey::Focus(next) => {
                    self.set_focus(next, cx.shell);
                    true
                }
                DialogKey::PressButton(ix) => {
                    self.press_dialog_button(ix, cx);
                    true
                }
                DialogKey::Cancel => {
                    self.cancel_dialog(cx);
                    true
                }
                DialogKey::EditGoToLine => edit_shortcut(ev)
                    .is_some_and(|cmd| self.run_edit_command(EditorSlot::GoToLine, cmd, cx.shell)),
                DialogKey::Consumed => true,
            };
        }

        let was_popup = self.menu_bar.is_open();
        let state = self.menu_state(cx.shell);
        let out = self.menu_bar.on_key_down(ev, self.focus, state, cx.ts());
        let handled = self.apply_menu_outcome(out, cx);
        if handled || was_popup {
            return handled;
        }

        if let Some(slot) = self.focused_editor_slot() {
            self.with_editor(slot, cx.shell, |ed, timers| ed.pause_blink(timers));
        }

        match self.find.on_key(ev, self.focus) {
            FindKey::Focus(next) => {
                self.set_focus(next, cx.shell);
                return true;
            }
            FindKey::Command(cmd) => {
                self.dispatch_app_command(cmd, cx);
                return true;
            }
            FindKey::PassThrough => {}
        }

        if let Some(slot) = self.focused_editor_slot()
            && let Some(cmd) = edit_shortcut(ev)
            && self.run_edit_command(slot, cmd, cx.shell)
        {
            return true;
        }

        if let Some(cmd) = app_shortcut(ev)
            && self.dispatch_app_command(cmd, cx)
        {
            return true;
        }

        false
    }

    pub(crate) fn on_key_up(&mut self, ev: KeyEvent, cx: &mut Cx<'_>) -> bool {
        if self.mode() == InputMode::Dialog {
            return false;
        }
        let menu_out = self.menu_bar.on_key_up(ev, self.focus);
        if menu_out.handled {
            return self.apply_menu_outcome(menu_out, cx);
        }
        self.dirty |= menu_out.dirty;
        false
    }

    fn text_input_slot(&self) -> Option<EditorSlot> {
        match self.mode() {
            InputMode::Dialog => (self.focus == Focus::Editor(EditorSlot::GoToLine))
                .then_some(EditorSlot::GoToLine),
            InputMode::Popup | InputMode::MenuBar => None,
            InputMode::Normal => self.focused_editor_slot(),
        }
    }

    pub(crate) fn on_text_input(&mut self, text: &str, cx: &mut Cx<'_>) {
        let Some(slot) = self.text_input_slot() else {
            return;
        };
        self.with_editor(slot, cx.shell, |ed, timers| {
            ed.handle_text_input(text, cx.ts(), timers);
        });
    }

    pub(crate) fn on_ime_composition(&mut self, text: &str, cursor: usize, cx: &mut Cx<'_>) {
        let Some(slot) = self.text_input_slot() else {
            return;
        };
        if let Some(ed) = self.editor_by_slot_mut(slot) {
            ed.handle_ime_composition(text, cursor, cx.ts());
        }
    }
}
