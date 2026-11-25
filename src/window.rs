//! A main window: its tabs, menus, find bar and dialogs, the input that reaches it, and its frame.
//!
//! The app forwards each window event to `on_event`, which also decides whether the window needs
//! painting. What spans windows (files, tab drags between windows, new windows) leaves as an
//! `Effect`; the app calls back in through the tab and file methods below.

use std::time::Instant;

use crate::command::{AppCommand, Command, app_shortcut, edit_shortcut};
use crate::dialog::{DialogKey, DialogKind, Dialogs};
use crate::document::{self, DocPath, Format};
use crate::editor::{EditCommand, Editor, EditorPaintParams, EditorPaintStyle};
use crate::files::{FileRequest, Prompt};
use crate::find::{FindBar, FindKey, FindOutcome};
use crate::geom::{Edges, Point, Rect, ScreenPoint, Size, point, size, transparent_black};
use crate::gfx::{TextStyle, TextSystem, WindowSurface, fill};
use crate::layout::AvailSize;
use crate::menu::{FocusRequest, MENU_BAR_HEIGHT, MenuOutcome, MenuState, Menus};
use crate::platform::{
    KeyEvent, Modifiers, MouseButton, WindowAppearance, WindowControlArea, WindowEvent,
    WindowHandle, WindowId, clipboard_has_text, format_local_time_date,
};
use crate::shell::{Cx, Effect, Shell};
use crate::tabs::{self, Removal, StripFrame, TAB_HEIGHT, TabData, TabSet};
use crate::text::{Rope, RopeExt as _};
use crate::theme::{BODY_LINE, BODY_SIZE, FIELD_CARET, ICON_FONT, Palette, SMALL_FONT, TEXT_FONT};
use crate::tooltip::{KeyTip, TooltipProbe};
use crate::types::APP_NAME;
use crate::ui::{
    AppFrame, AppNode, AppUi, Brushes, CaptionHit, CustomDraw, DialogHit, EditorSlot, Focus, HitId,
    MenuHit, OverlayAnchor, TabHit, TabId, box_, custom, row, text,
};
use crate::widgets::icon_button;

pub(crate) const WINDOW_WIDTH: f32 = 2213.;
pub(crate) const WINDOW_HEIGHT: f32 = 1107.;
pub(crate) const WINDOW_MAX_SCREEN_SHARE: f32 = 0.9;

const EDITOR_FONT: &str = "Consolas";
const EDITOR_FONT_SIZE: f32 = 11. * 96. / 72.;
const EDITOR_LINE_HEIGHT: f32 = 20.;
pub(crate) const EDITOR_PADDINGS: Edges<f32> = Edges {
    top: 11.,
    right: 12.,
    bottom: 11.,
    left: 18.,
};
const CARET_HEIGHT: f32 = 18.;
const ZOOM_STEP: f32 = 0.1;
const ZOOM_MIN: f32 = 0.1;
const ZOOM_MAX: f32 = 5.;

/// What input goes to first: an open dialog, an open menu, the menu bar, or the editors.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum InputMode {
    Dialog,
    Popup,
    MenuBar,
    Normal,
}

pub(crate) struct MainWindow {
    pub(crate) id: WindowId,
    pub(crate) hwnd: WindowHandle,
    pub(crate) win: WindowSurface,
    pub(crate) ui: AppUi,
    pub(crate) tabs: TabSet,
    pub(crate) menus: Menus,
    pub(crate) find: FindBar,
    pub(crate) dialogs: Dialogs,
    focus: Focus,
    /// The window needs painting.
    pub(crate) dirty: bool,
    has_painted: bool,
    /// The window lost its last tab or finished closing; the app drops it.
    pub(crate) pending_close: bool,
    /// The tab a middle press went down on (a middle click closes it).
    middle_pressed_tab: Option<TabId>,
    pub(crate) min_size: Option<Size>,
}

fn new_tab_data(loaded: Option<document::Loaded>, shell: &Shell) -> TabData {
    TabData::create(loaded, shell.view.word_wrap, EDITOR_PADDINGS)
}

impl MainWindow {
    pub(crate) fn new(win: WindowSurface, initial: Option<TabData>, shell: &Shell) -> Self {
        let tabs = TabSet::new(initial.unwrap_or_else(|| new_tab_data(None, shell)));
        let mut window = Self {
            id: win.window.id,
            hwnd: win.window.hwnd(),
            win,
            ui: AppUi::default(),
            focus: Focus::Editor(tabs.active_slot()),
            tabs,
            menus: Menus::default(),
            find: FindBar::new(),
            dialogs: Dialogs::new(),
            dirty: true,
            has_painted: false,
            pending_close: false,
            middle_pressed_tab: None,
            min_size: None,
        };
        window.activate_tab(0, shell);
        window
    }

    pub(crate) fn mode(&self) -> InputMode {
        if self.dialogs.is_open() {
            InputMode::Dialog
        } else if self.menus.is_open() {
            InputMode::Popup
        } else if self.focus == Focus::MenuBar {
            InputMode::MenuBar
        } else {
            InputMode::Normal
        }
    }

    #[cfg(test)]
    pub(crate) fn focus(&self) -> Focus {
        self.focus
    }

    /// Acts on whatever is due at `now` (caret blinks, scrollbar fades, submenu and notice
    /// delays, held scroll arrows), marking the window dirty if anything changed.
    pub(crate) fn tick(&mut self, now: Instant) {
        let mut changed = false;
        self.tabs.for_each_editor_mut(|ed| changed |= ed.tick(now));
        for ed in self.fields_mut() {
            changed |= ed.tick(now);
        }
        changed |= self.menus.tick(now);
        changed |= self.find.tick(now);
        changed |= self.tabs.tick(now);
        self.dirty |= changed;
    }

    /// When the window next needs a `tick`.
    pub(crate) fn deadline(&self) -> Option<Instant> {
        let tabs = self.tabs.iter().map(|tab| &tab.data.editor);
        let editors = tabs.chain(self.fields()).filter_map(Editor::deadline);
        editors
            .chain(self.tabs.deadline())
            .chain(self.menus.deadline())
            .chain(self.find.deadline())
            .min()
    }

    pub(crate) fn apply_theme(&mut self, appearance: WindowAppearance) {
        self.win.window.set_appearance(appearance);
        self.dirty = true;
    }

    pub(crate) fn set_word_wrap(&mut self, shell: &Shell) {
        let word_wrap = shell.view.word_wrap;
        self.tabs
            .for_each_editor_mut(|ed| ed.set_soft_wrap(word_wrap, &shell.text_system));
        self.dirty = true;
    }

    // ---- Overlays the app draws for the window

    fn overlay_anchor(&self) -> OverlayAnchor {
        OverlayAnchor {
            window: self.id,
            origin: self.hwnd.client_origin(),
            scale: self.win.window.scale_factor(),
        }
    }

    pub(crate) fn key_tips_active(&self) -> bool {
        self.menus.access_active(self.focus)
    }

    pub(crate) fn key_tip_request(&self, shell: &Shell) -> (OverlayAnchor, Vec<KeyTip>) {
        let tips = self
            .menus
            .key_tips(self.focus, self.ui.frame(), &shell.text_system);
        (self.overlay_anchor(), tips)
    }

    pub(crate) fn tooltip_probe(&self) -> TooltipProbe<'_> {
        TooltipProbe {
            anchor: self.overlay_anchor(),
            pointer: self.ui.pointer(),
            frame: self.ui.frame(),
        }
    }
}

// ---- Input

impl MainWindow {
    /// Handles input to the window; for keys, whether it was used.
    pub(crate) fn on_event(&mut self, event: WindowEvent, cx: &mut Cx<'_>) -> bool {
        // Presses, keys, text and activation change what shows; pointer motion, the wheel, key
        // releases and moving the window repaint only if they change something.
        let repaint = matches!(
            event,
            WindowEvent::Resized { .. }
                | WindowEvent::Activated(_)
                | WindowEvent::MouseDown { .. }
                | WindowEvent::MouseUp { .. }
                | WindowEvent::KeyDown(_)
                | WindowEvent::TextInput(_)
                | WindowEvent::ImeComposition { .. }
        );
        let mut handled = false;
        match event {
            WindowEvent::Resized {
                physical_width,
                physical_height,
                scale,
            } => {
                self.win
                    .surface
                    .resize(physical_width, physical_height, scale);
                self.cancel_menu(cx.shell);
            }
            WindowEvent::Moved => self.cancel_menu(cx.shell),
            WindowEvent::Activated(active) => self.on_activated(active, cx.shell),
            WindowEvent::PaintRequested => self.dirty |= !self.has_painted,
            WindowEvent::MouseMove {
                position,
                pressed_button,
            } => self.on_mouse_move(position, pressed_button, cx),
            WindowEvent::MouseLeave => {
                self.dirty |= self.ui.on_mouse_leave();
                self.tabs.release_arrow();
                self.dirty |= self.tabs.thaw_if_frozen();
            }
            WindowEvent::MouseDown {
                button,
                position,
                modifiers,
                click_count,
            } => self.on_mouse_down(button, position, modifiers, click_count, cx),
            WindowEvent::MouseUp { button, position } => self.on_mouse_up(button, position, cx),
            WindowEvent::ScrollWheel {
                position,
                delta_lines,
                page_scroll,
                modifiers,
            } => self.on_scroll_wheel(position, delta_lines, page_scroll, modifiers),
            WindowEvent::KeyDown(ev) => handled = self.on_key_down(ev, cx),
            WindowEvent::KeyUp(ev) => {
                if self.mode() != InputMode::Dialog {
                    let out = self.menus.on_key_up(ev, self.focus);
                    if out.handled {
                        handled = self.apply_menu_outcome(out, cx);
                    } else {
                        self.dirty |= out.dirty;
                    }
                }
            }
            WindowEvent::CaptureLost => {
                self.middle_pressed_tab = None;
                self.tabs.release_arrow();
                self.dirty |= self.ui.on_capture_lost();
            }
            WindowEvent::TextInput(text) => {
                if let Some(ed) = self.text_input_editor() {
                    ed.handle_text_input(&text, &cx.shell.text_system);
                }
            }
            WindowEvent::ImeComposition { text, cursor } => {
                if let Some(ed) = self.text_input_editor() {
                    ed.handle_ime_composition(&text, cursor, &cx.shell.text_system);
                }
            }
            WindowEvent::FilesDropped(_)
            | WindowEvent::SizeMoveChanged(_)
            | WindowEvent::CloseRequested => {}
        }
        self.dirty |= repaint;
        handled
    }

    fn on_activated(&mut self, active: bool, shell: &Shell) {
        if active {
            if let Some(ed) = self.focused_slot().and_then(|slot| self.editor_mut(slot)) {
                ed.focus(&shell.settings);
            }
            return;
        }
        self.cancel_menu(shell);
        match self.focused_slot() {
            Some(slot) => self.blur(slot),
            None => self.hwnd.complete_ime_composition(),
        }
        self.win.window.set_text_caret(None, false);
    }

    fn pointer_in_strip(&self, pos: Point) -> bool {
        self.ui
            .frame()
            .rect_of(HitId::Tab(TabHit::Strip))
            .is_some_and(|r| r.contains(pos))
    }

    fn on_mouse_move(&mut self, pos: Point, pressed: Option<MouseButton>, cx: &mut Cx<'_>) {
        self.dirty |= self.ui.on_mouse_move(pos, pressed.is_some());
        let top = self.ui.top_hit();
        if !matches!(
            top,
            Some(HitId::Tab(TabHit::ScrollLeft | TabHit::ScrollRight))
        ) {
            self.tabs.release_arrow();
        }
        let mode = self.mode();
        if mode != InputMode::Dialog {
            let (tabs, shell) = (&self.tabs, cx.shell);
            let delay = shell.settings.menu_show_delay;
            let out =
                self.menus
                    .on_hover_change(top, || menu_state(tabs, shell), &mut self.ui, delay);
            self.apply_menu_outcome(out, cx);
        }
        if !self.pointer_in_strip(pos) {
            self.dirty |= self.tabs.thaw_if_frozen();
        }

        self.dirty |= self.tabs.active().editor.hover_scrollbar(pos, &self.ui);
        let dragging = pressed == Some(MouseButton::Left);
        if let Some(notify) = self.tabs.active_mut().editor.drag_scrollbar(pos, dragging) {
            self.dirty |= notify;
            self.sync_cursor(pos);
            return;
        }
        // Only the editor with the keyboard drag-selects.
        if self
            .focused_slot()
            .and_then(|slot| self.editor_mut(slot))
            .is_some_and(|ed| ed.on_mouse_move(pos, dragging))
        {
            self.dirty = true;
        }
        self.sync_cursor(pos);
    }

    fn on_mouse_down(
        &mut self,
        button: MouseButton,
        pos: Point,
        modifiers: Modifiers,
        click_count: usize,
        cx: &mut Cx<'_>,
    ) {
        let chain = self.ui.on_mouse_down(pos, button).to_vec();
        let mode = self.mode();
        let state = menu_state(&self.tabs, cx.shell);
        let out = self.menus.on_mouse_down(&chain, button, self.focus, state);
        if self.apply_menu_outcome(out, cx) {
            return;
        }
        if button == MouseButton::Middle && mode != InputMode::Dialog {
            self.middle_pressed_tab = tab_at(&chain);
            return;
        }
        let top = chain.first().copied();
        if button == MouseButton::Left
            && let Some(HitId::ScrollTrack(slot, axis)) = top
            && slot == self.tabs.active_slot()
            && self.tabs.active_mut().editor.press_scrollbar(axis, pos)
        {
            return;
        }
        if button == MouseButton::Left {
            match top {
                Some(
                    HitId::FieldClear(_)
                    | HitId::Dialog(_)
                    | HitId::Find(_)
                    | HitId::Tab(TabHit::Close(_)),
                ) => return,
                Some(HitId::Tab(TabHit::Item(id))) => {
                    if let Some(index) = self.tabs.index_of(id) {
                        self.activate_tab(index, cx.shell);
                        let left = self
                            .ui
                            .frame()
                            .rect_of(HitId::Tab(TabHit::Item(id)))
                            .map_or(pos.x, |r| r.origin.x);
                        cx.emit(Effect::StartTabDrag {
                            window: self.id,
                            tab: id,
                            index,
                            grab: point(pos.x - left, pos.y - STRIP_TOP),
                            position: pos,
                        });
                    }
                    return;
                }
                Some(HitId::Tab(hit @ (TabHit::ScrollLeft | TabHit::ScrollRight))) => {
                    self.tabs
                        .press_arrow(hit == TabHit::ScrollRight, Instant::now());
                    return;
                }
                _ => {}
            }
        }
        let editor = chain.iter().find_map(|hit| match hit {
            HitId::Editor(slot) => Some(*slot),
            _ => None,
        });
        if let Some(slot) = editor {
            self.hwnd.complete_ime_composition();
            self.set_focus(Focus::Editor(slot), cx.shell);
            if let Some(ed) = self.editor_mut(slot) {
                ed.on_mouse_down(button, pos, modifiers, click_count);
            }
        }
    }

    fn on_mouse_up(&mut self, button: MouseButton, pos: Point, cx: &mut Cx<'_>) {
        let (clicked, chain) = self.ui.on_mouse_up(pos, button);
        let top = chain.first().copied();
        let over = tab_at(chain);
        self.tabs.release_arrow();
        // A press that opened a menu, released on one of its rows, runs the row.
        let menu_release =
            (button == MouseButton::Left && clicked.is_none() && self.mode() == InputMode::Popup)
                .then_some(top)
                .flatten()
                .filter(|hit| matches!(hit, HitId::Menu(MenuHit::Row { .. })));
        if button == MouseButton::Middle {
            if let Some(tab) = self.middle_pressed_tab.take()
                && over == Some(tab)
            {
                self.request_close(tab, cx);
            }
            return;
        }
        self.tabs.active_mut().editor.release_scrollbar(pos);
        if let Some(ed) = self.focused_field() {
            ed.on_mouse_up(button);
        }
        if let Some(anchor) = self.tabs.active_mut().editor.on_mouse_up(button) {
            let out = self
                .menus
                .open_context_menu(anchor, menu_state(&self.tabs, cx.shell));
            self.apply_menu_outcome(out, cx);
            return;
        }
        match clicked.or(menu_release) {
            Some(HitId::Dialog(DialogHit::Button(ix))) => self.finish_dialog(Some(ix), cx),
            Some(HitId::FieldClear(slot)) => {
                if let Some(ed) = self.editor_mut(slot) {
                    ed.set_value("", &cx.shell.text_system);
                }
                self.set_focus(Focus::Editor(slot), cx.shell);
            }
            Some(HitId::Find(hit)) => {
                if let Some(cmd) = hit.command() {
                    self.dispatch_app_command(cmd, cx);
                }
            }
            Some(HitId::Menu(hit)) => {
                let out = self.menus.on_click(hit);
                self.apply_menu_outcome(out, cx);
            }
            Some(HitId::Tab(TabHit::Close(tab))) => self.request_close(tab, cx),
            Some(HitId::Tab(TabHit::New)) => {
                self.dispatch_app_command(AppCommand::NewTab, cx);
            }
            Some(HitId::ThemeToggle) => cx.emit(Effect::ToggleTheme),
            _ => {}
        }
    }

    /// Asks the app to close tab `tab` (it may prompt to save first), if it's still here.
    fn request_close(&self, tab: TabId, cx: &mut Cx<'_>) {
        if self.tabs.index_of(tab).is_some() {
            let window = self.id;
            cx.emit(Effect::File(FileRequest::CloseTab { window, tab }));
        }
    }

    fn on_scroll_wheel(&mut self, pos: Point, lines: Point, page: bool, modifiers: Modifiers) {
        if matches!(self.mode(), InputMode::Dialog | InputMode::Popup) {
            return;
        }
        let chain = self.ui.frame().hit_chain(pos);
        if chain.contains(&HitId::Tab(TabHit::List)) && self.tabs.wheel(lines) {
            self.dirty = true;
            return;
        }
        if modifiers.ctrl && chain.contains(&HitId::EditorArea) {
            if lines.y != 0. {
                self.set_zoom(self.zoom() + ZOOM_STEP.copysign(lines.y));
            }
            return;
        }
        let ed = &mut self.tabs.active_mut().editor;
        self.dirty |= ed.on_scroll_wheel(lines, page, modifiers);
    }

    fn on_key_down(&mut self, ev: KeyEvent, cx: &mut Cx<'_>) -> bool {
        self.ui.last_input_was_keyboard = true;
        if self.mode() == InputMode::Dialog {
            self.pause_blink();
            return match self.dialogs.on_key(ev, self.focus) {
                DialogKey::PassThrough => false,
                DialogKey::Focus(next) => {
                    self.set_focus(next, cx.shell);
                    true
                }
                DialogKey::PressButton(ix) => {
                    self.finish_dialog(Some(ix), cx);
                    true
                }
                DialogKey::Cancel => {
                    self.finish_dialog(None, cx);
                    true
                }
                DialogKey::EditGoToLine => edit_shortcut(ev)
                    .is_some_and(|cmd| self.run_edit_command(EditorSlot::GoToLine, cmd, cx.shell)),
                DialogKey::Consumed => true,
            };
        }

        let was_open = self.menus.is_open();
        let state = menu_state(&self.tabs, cx.shell);
        let out = self.menus.on_key_down(ev, self.focus, state);
        let handled = self.apply_menu_outcome(out, cx);
        if handled || was_open {
            return handled;
        }

        self.pause_blink();
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
        if let Some(slot) = self.focused_slot()
            && let Some(cmd) = edit_shortcut(ev)
            && self.run_edit_command(slot, cmd, cx.shell)
        {
            return true;
        }
        app_shortcut(ev).is_some_and(|cmd| self.dispatch_app_command(cmd, cx))
    }

    /// The editor typing goes to: the focused one, unless a menu has the keyboard (or a dialog
    /// does, other than in its go-to field).
    fn text_input_editor(&mut self) -> Option<&mut Editor> {
        let slot = match self.mode() {
            InputMode::Dialog => {
                (self.focus == Focus::Editor(EditorSlot::GoToLine)).then_some(EditorSlot::GoToLine)
            }
            InputMode::Popup | InputMode::MenuBar => None,
            InputMode::Normal => self.focused_slot(),
        };
        self.editor_mut(slot?)
    }

    fn sync_cursor(&self, pos: Point) {
        self.win.window.set_cursor(self.ui.frame().cursor_at(pos));
    }
}

// ---- Commands

impl MainWindow {
    fn dispatch_command(&mut self, cmd: Command, cx: &mut Cx<'_>) -> bool {
        match cmd {
            Command::App(cmd) => self.dispatch_app_command(cmd, cx),
            Command::Edit(cmd) => self.run_edit_command(self.tabs.active_slot(), cmd, cx.shell),
        }
    }

    /// Runs `cmd`; whether it applied (none do while a dialog is open).
    fn dispatch_app_command(&mut self, cmd: AppCommand, cx: &mut Cx<'_>) -> bool {
        if self.dialogs.is_open() {
            return false;
        }
        let (window, tab) = (self.id, self.tabs.active_id());
        let file = |request| Effect::File(request);
        match cmd {
            AppCommand::Cancel => return self.close_find(cx.shell),
            AppCommand::ShowContextMenu => {
                if self.focus != Focus::Editor(self.tabs.active_slot()) {
                    return false;
                }
                if let Some(caret) = self.tabs.active().editor.caret_bounds() {
                    let anchor = point(caret.right(), caret.bottom());
                    let state = menu_state(&self.tabs, cx.shell);
                    let out = self.menus.open_context_menu(anchor, state);
                    self.apply_menu_outcome(out, cx);
                }
            }
            AppCommand::NewWindow => cx.emit(Effect::NewWindow),
            AppCommand::Exit => cx.emit(Effect::Exit),
            AppCommand::ToggleStatusBar => cx.emit(Effect::ToggleStatusBar),
            AppCommand::ToggleWordWrap => cx.emit(Effect::ToggleWordWrap),
            AppCommand::ZoomIn => self.set_zoom(self.zoom() + ZOOM_STEP),
            AppCommand::ZoomOut => self.set_zoom(self.zoom() - ZOOM_STEP),
            AppCommand::ResetZoom => self.set_zoom(1.),
            AppCommand::CloseWindow => cx.emit(file(FileRequest::CloseWindow { window })),
            AppCommand::CloseTab => cx.emit(file(FileRequest::CloseTab { window, tab })),
            AppCommand::Open => cx.emit(file(FileRequest::OpenDialog { window })),
            AppCommand::Save => cx.emit(file(FileRequest::Save { window, tab })),
            AppCommand::SaveAs => cx.emit(file(FileRequest::SaveAs { window, tab })),
            AppCommand::SaveAll => cx.emit(file(FileRequest::SaveAll { window })),
            AppCommand::NewTab => {
                let ix = self.tabs.push(new_tab_data(None, cx.shell));
                self.activate_tab(ix, cx.shell);
            }
            AppCommand::NextTab | AppCommand::PrevTab => {
                let (n, at) = (self.tabs.len(), self.tabs.active_index());
                let step = if cmd == AppCommand::NextTab { 1 } else { n - 1 };
                self.go_to_tab((at + step) % n, cx.shell);
            }
            AppCommand::SelectTab(ix) => self.go_to_tab(ix, cx.shell),
            AppCommand::SelectLastTab => self.go_to_tab(self.tabs.len() - 1, cx.shell),
            AppCommand::TimeDate => {
                let stamp = format_local_time_date();
                self.tabs
                    .active_mut()
                    .editor
                    .replace(&stamp, &cx.shell.text_system);
            }
            AppCommand::GoTo => {
                let m = self.tabs.active().editor.model();
                let row = m.text().offset_to_point(m.cursor()).row;
                self.show_go_to(Some(row + 1), cx.shell);
            }
            AppCommand::Find | AppCommand::Replace => {
                if self.has_text() {
                    self.open_find(cmd == AppCommand::Replace, cx.shell);
                }
            }
            AppCommand::FindNext | AppCommand::FindPrevious => {
                if self.has_text() {
                    self.find_step(cmd == AppCommand::FindNext, cx.shell);
                }
            }
            AppCommand::ReplaceNext => {
                let ed = &mut self.tabs.active_mut().editor;
                self.find.replace_next(ed, &cx.shell.text_system);
            }
            AppCommand::ReplaceAll => {
                let ed = &mut self.tabs.active_mut().editor;
                let out = self.find.replace_all(ed, &cx.shell.text_system);
                if matches!(out, FindOutcome::Replaced(_)) {
                    self.refocus_find(cx.shell);
                }
            }
            AppCommand::ToggleReplace
            | AppCommand::ToggleMatchCase
            | AppCommand::ToggleWrapAround => {
                self.find.toggle(cmd);
                self.refocus_find(cx.shell);
            }
            AppCommand::FindOptions => {
                let (case, wrap) = (self.find.match_case, self.find.wrap_around);
                let out = self.menus.open_find_options(case, wrap);
                self.apply_menu_outcome(out, cx);
            }
        }
        true
    }

    /// Runs an editing command in the editor at `slot`; whether it applied.
    fn run_edit_command(&mut self, slot: EditorSlot, cmd: EditCommand, shell: &Shell) -> bool {
        self.editor_mut(slot)
            .is_some_and(|ed| ed.handle_command(cmd, &shell.text_system))
    }

    fn has_text(&self) -> bool {
        self.tabs.active().editor.model().text().len() > 0
    }

    fn zoom(&self) -> f32 {
        self.tabs.active().zoom
    }

    fn set_zoom(&mut self, zoom: f32) {
        self.tabs.active_mut().zoom =
            ((zoom / ZOOM_STEP).round() * ZOOM_STEP).clamp(ZOOM_MIN, ZOOM_MAX);
        self.dirty = true;
    }

    fn line_height(&self) -> f32 {
        (EDITOR_LINE_HEIGHT * self.zoom()).round()
    }

    fn editor_min_height(&self) -> f32 {
        EDITOR_PADDINGS.top + EDITOR_PADDINGS.bottom + self.line_height()
    }

    // ---- Find

    fn open_find(&mut self, replace: bool, shell: &Shell) {
        let model = self.tabs.active().editor.model();
        let selected = model.text().slice(model.selection().range()).to_string();
        self.find.open(replace, &selected, &shell.text_system);
        self.set_focus(Focus::Editor(EditorSlot::FindQuery), shell);
    }

    /// Closes the find bar, if it's open; whether it was.
    fn close_find(&mut self, shell: &Shell) -> bool {
        if !self.find.is_open() {
            return false;
        }
        if self.find.close() {
            self.focus_active_editor(shell);
        }
        true
    }

    /// Puts focus back in the find bar after one of its options changed.
    fn refocus_find(&mut self, shell: &Shell) {
        if self.find.is_open() {
            let slot = self.find.field_for(self.focus);
            self.set_focus(Focus::Editor(slot), shell);
        }
    }

    fn find_step(&mut self, forward: bool, shell: &Shell) {
        let out = self.find.step(&mut self.tabs.active_mut().editor, forward);
        if matches!(out, FindOutcome::EmptyQuery) {
            self.open_find(false, shell);
            return;
        }
        if !self.find.is_open() {
            self.focus_active_editor(shell);
        }
    }

    // ---- Dialogs

    pub(crate) fn show_prompt(&mut self, prompt: Prompt, shell: &Shell) {
        if let Some(next) = self.dialogs.enqueue(DialogKind::Prompt(prompt)) {
            self.cancel_menu(shell);
            self.set_focus(next, shell);
            self.dirty = true;
        }
    }

    fn show_dialog_now(&mut self, kind: DialogKind, shell: &Shell) {
        self.cancel_menu(shell);
        let next = self.dialogs.show_now(kind);
        self.set_focus(next, shell);
    }

    fn close_dialog(&mut self, shell: &Shell) {
        if let Some(next) = self.dialogs.dismiss() {
            self.cancel_menu(shell);
            self.set_focus(next, shell);
        } else {
            self.focus_active_editor(shell);
        }
    }

    /// Shows the go-to dialog, its field holding `line` (or what was typed before).
    fn show_go_to(&mut self, line: Option<usize>, shell: &Shell) {
        self.cancel_menu(shell);
        let next = self.dialogs.show_go_to(line, &shell.text_system);
        self.set_focus(next, shell);
    }

    /// The open dialog's button `button` (or, with `None`, Escape) was chosen.
    fn finish_dialog(&mut self, button: Option<usize>, cx: &mut Cx<'_>) {
        let Some(kind) = self.dialogs.take_active() else {
            return;
        };
        match kind {
            DialogKind::GoTo if button == Some(0) => {
                let lines = self.tabs.active().editor.model().text().lines_len();
                match self.dialogs.go_to_target(lines) {
                    Some(line) => {
                        self.close_dialog(cx.shell);
                        self.tabs.active_mut().editor.go_to_line(line);
                    }
                    None => self.show_dialog_now(DialogKind::LineOutOfRange, cx.shell),
                }
            }
            DialogKind::GoTo => self.close_dialog(cx.shell),
            DialogKind::LineOutOfRange => self.show_go_to(None, cx.shell),
            DialogKind::Prompt(prompt) => {
                self.close_dialog(cx.shell);
                if let Some((id, _)) = prompt.reply {
                    let answer = button
                        .and_then(|ix| prompt.choices.get(ix))
                        .map_or(prompt.cancel, |&(_, a)| a);
                    cx.emit(Effect::PromptResponse { id, answer });
                }
            }
        }
    }

    // ---- Menus

    fn cancel_menu(&mut self, shell: &Shell) {
        let out = self.menus.cancel(self.focus);
        self.dirty |= out.dirty;
        self.apply_focus_request(out.focus, shell);
    }

    fn apply_focus_request(&mut self, req: FocusRequest, shell: &Shell) {
        match req {
            FocusRequest::Keep => {}
            FocusRequest::MenuBar => self.set_focus(Focus::MenuBar, shell),
            FocusRequest::RestoreEditor => self.focus_active_editor(shell),
            FocusRequest::Editor(slot) => self.set_focus(Focus::Editor(slot), shell),
        }
    }

    /// Acts on what the menus made of some input; whether they used it.
    fn apply_menu_outcome(&mut self, out: MenuOutcome, cx: &mut Cx<'_>) -> bool {
        self.dirty |= out.dirty;
        self.apply_focus_request(out.focus, cx.shell);
        if let Some(cmd) = out.command {
            self.dispatch_command(cmd, cx);
            return true;
        }
        out.handled
    }
}

fn menu_state(tabs: &TabSet, shell: &Shell) -> MenuState {
    let m = tabs.active().editor.model();
    MenuState {
        can_undo: m.can_undo(),
        can_redo: m.can_redo(),
        has_selection: !m.selection().range().is_empty(),
        has_text: m.text().len() > 0,
        clipboard_has_text: clipboard_has_text(),
        status_bar: shell.view.status_bar,
        word_wrap: shell.view.word_wrap,
    }
}

// ---- Tabs, and the calls the app makes for files and tab drags

impl MainWindow {
    fn refresh_title(&self) {
        let name = self.tabs.tab_name(self.tabs.active_index());
        self.win.window.set_title(&format!("{name} - {APP_NAME}"));
    }

    /// Shows tab `ix` without moving focus into it.
    fn select_tab(&mut self, ix: usize) {
        if self.tabs.active_index() != ix {
            self.blur(self.tabs.active_slot());
        }
        self.tabs.select(ix);
        self.find.clear_notice();
        if self.find.is_open() && !self.has_text() {
            self.find.close();
        }
        self.refresh_title();
        self.dirty = true;
    }

    /// Shows tab `ix` and, unless a dialog or menu has the keyboard, focuses it.
    pub(crate) fn activate_tab(&mut self, ix: usize, shell: &Shell) {
        self.select_tab(ix);
        if !matches!(self.mode(), InputMode::Dialog | InputMode::Popup) {
            self.focus_active_editor(shell);
        }
    }

    /// Activates tab `ix`, if there is one and it isn't the active one.
    fn go_to_tab(&mut self, ix: usize, shell: &Shell) {
        if ix < self.tabs.len() && ix != self.tabs.active_index() {
            self.activate_tab(ix, shell);
        }
    }

    /// Shows a loaded file: in the active tab if that's blank, else in a new one.
    pub(crate) fn adopt_loaded_file(&mut self, loaded: document::Loaded, shell: &Shell) {
        let data = new_tab_data(Some(loaded), shell);
        let active = self.tabs.active_index();
        let ix = if self.tabs.is_blank(active) {
            self.tabs.replace(active, data);
            active
        } else {
            self.tabs.push(data)
        };
        self.activate_tab(ix, shell);
    }

    pub(crate) fn begin_tab_save(&mut self, tab: TabId) -> Option<(u64, Rope)> {
        let ed = self.tabs.editor_mut(tab)?;
        Some((ed.begin_save(), ed.model().text().clone()))
    }

    pub(crate) fn mark_tab_saved(
        &mut self,
        tab: TabId,
        snapshot: u64,
        path: DocPath,
        format: Format,
    ) {
        if self.tabs.mark_saved(tab, snapshot, path, format) {
            self.refresh_title();
            self.dirty = true;
        }
    }

    pub(crate) fn remove_tab(&mut self, ix: usize, shell: &Shell) {
        let hovering = self
            .ui
            .pointer()
            .is_some_and(|p| self.pointer_in_strip(p.pos));
        let removal = self.tabs.remove(ix, hovering);
        if let Removal::Removed { id, .. } = removal {
            self.dialogs.close_for_tab(id);
        }
        self.after_removal(removal, shell);
    }

    /// Takes tab `id` out of the window (to move it to another).
    pub(crate) fn take_tab(&mut self, id: TabId, shell: &Shell) -> Option<TabData> {
        let (data, removal) = self.tabs.take(id)?;
        self.after_removal(removal, shell);
        Some(data)
    }

    fn after_removal(&mut self, removal: Removal, shell: &Shell) {
        let Removal::Removed {
            closed_active,
            next_active,
            ..
        } = removal
        else {
            self.pending_close = true;
            return;
        };
        let in_find = matches!(
            self.focus,
            Focus::Editor(EditorSlot::FindQuery | EditorSlot::FindReplace)
        );
        if closed_active || !in_find {
            self.activate_tab(next_active, shell);
        } else {
            self.select_tab(next_active);
        }
    }

    /// Puts a tab back at `ix` (a cancelled drag's, or one a failed window couldn't take).
    pub(crate) fn restore_tab(&mut self, data: TabData, ix: usize, shell: &Shell) {
        let ix = self.tabs.restore(data, ix);
        self.activate_tab(ix, shell);
    }

    /// Moves a tab dragged within the strip back to `ix`.
    pub(crate) fn return_tab(&mut self, id: TabId, ix: usize, shell: &Shell) {
        if let Some(ix) = self.tabs.return_tab(id, ix) {
            self.activate_tab(ix, shell);
        }
        self.dirty = true;
    }

    /// Drops a tab dragged in from another window where the pointer is.
    pub(crate) fn insert_dragged_tab(
        &mut self,
        data: TabData,
        at: ScreenPoint,
        grab: f32,
        shell: &Shell,
    ) {
        let x = self.hwnd.to_client(at).x - HEADER_WIDTH;
        let ix = self.tabs.insert_dragged(data, x, grab);
        self.activate_tab(ix, shell);
    }

    /// Moves tab `id`, grabbed `grab` from its left after a press at `start`, to the pointer.
    pub(crate) fn drag_tab(&mut self, id: TabId, at: ScreenPoint, grab: f32, start: f32) {
        let x = self.hwnd.to_client(at).x;
        self.tabs
            .drag(id, x - HEADER_WIDTH, grab, start - HEADER_WIDTH);
        self.dirty = true;
    }

    pub(crate) fn release_tab(&mut self) {
        self.dirty |= self.tabs.release_drag();
    }

    /// Whether a tab dragged from another window may land here.
    pub(crate) fn accepts_tab_drop(&self) -> bool {
        !self.dialogs.is_open() && !self.pending_close && self.hwnd.is_enabled()
    }

    /// Whether `pt` is over the tab strip, give or take `magnetism` above and below.
    pub(crate) fn strip_hit(&self, pt: ScreenPoint, magnetism: f32) -> bool {
        let pos = self.hwnd.to_client(pt);
        let frame = self.ui.frame();
        let right = frame.rect_of(HitId::Caption(CaptionHit::Min)).map_or_else(
            || self.win.window.logical_size().width - CAPTION_WIDTH,
            |r| r.left(),
        );
        let (left, top, bottom) = frame.rect_of(HitId::Tab(TabHit::List)).map_or(
            (
                HEADER_WIDTH + tabs::LIST_PAD,
                STRIP_TOP,
                STRIP_TOP + TAB_HEIGHT,
            ),
            |r| (r.left(), r.top(), r.bottom()),
        );
        (left..right).contains(&pos.x) && (top - magnetism..bottom + magnetism).contains(&pos.y)
    }

    /// Moves the window so a tab grabbed at `grab`, as the strip's first, sits under `pt`.
    pub(crate) fn place_under(&self, pt: ScreenPoint, grab: Point) {
        let offset = point(HEADER_WIDTH + tabs::LIST_PAD + grab.x, STRIP_TOP + grab.y);
        let (at, frame) = (self.hwnd.to_screen(offset), self.hwnd.window_rect());
        self.hwnd
            .move_noactivate(pt.x - (at.x - frame.left), pt.y - (at.y - frame.top));
    }
}

// ---- Editors and focus

/// The tab, if any, among the hits under the pointer.
fn tab_at(chain: &[HitId]) -> Option<TabId> {
    chain.iter().find_map(|hit| match hit {
        HitId::Tab(TabHit::Item(id)) => Some(*id),
        _ => None,
    })
}

fn slot_editor<'a>(
    tabs: &'a TabSet,
    find: &'a FindBar,
    dialogs: &'a Dialogs,
    slot: EditorSlot,
) -> Option<&'a Editor> {
    match slot {
        EditorSlot::Tab(id) => tabs.editor(id),
        EditorSlot::FindQuery => Some(&find.query),
        EditorSlot::FindReplace => Some(&find.replacement),
        EditorSlot::GoToLine => Some(&dialogs.line),
    }
}

fn slot_editor_mut<'a>(
    tabs: &'a mut TabSet,
    find: &'a mut FindBar,
    dialogs: &'a mut Dialogs,
    slot: EditorSlot,
) -> Option<&'a mut Editor> {
    match slot {
        EditorSlot::Tab(id) => tabs.editor_mut(id),
        EditorSlot::FindQuery => Some(&mut find.query),
        EditorSlot::FindReplace => Some(&mut find.replacement),
        EditorSlot::GoToLine => Some(&mut dialogs.line),
    }
}

impl MainWindow {
    fn editor_mut(&mut self, slot: EditorSlot) -> Option<&mut Editor> {
        slot_editor_mut(&mut self.tabs, &mut self.find, &mut self.dialogs, slot)
    }

    /// The single-line fields: find's query and replacement, and the go-to line.
    fn fields(&self) -> [&Editor; 3] {
        [&self.find.query, &self.find.replacement, &self.dialogs.line]
    }

    fn fields_mut(&mut self) -> [&mut Editor; 3] {
        [
            &mut self.find.query,
            &mut self.find.replacement,
            &mut self.dialogs.line,
        ]
    }

    fn focused_slot(&self) -> Option<EditorSlot> {
        match self.focus {
            Focus::Editor(slot) => Some(slot),
            Focus::MenuBar | Focus::DialogButton(_) => None,
        }
    }

    /// The focused editor when it isn't the document's (a find or go-to field).
    fn focused_field(&mut self) -> Option<&mut Editor> {
        let slot = self
            .focused_slot()
            .filter(|&slot| slot != self.tabs.active_slot())?;
        self.editor_mut(slot)
    }

    /// Keeps the focused caret steady while keys go to it.
    fn pause_blink(&mut self) {
        if let Some(slot) = self.focused_slot()
            && let Some(ed) = self.editor_mut(slot)
        {
            ed.pause_blink();
        }
    }

    /// Finishes any IME composition and takes the editor at `slot` out of focus.
    fn blur(&mut self, slot: EditorSlot) {
        self.hwnd.complete_ime_composition();
        if let Some(ed) = self.editor_mut(slot) {
            ed.blur();
        }
    }

    fn set_focus(&mut self, next: Focus, shell: &Shell) {
        if self.focus != next {
            if let Some(old) = self.focused_slot() {
                self.blur(old);
            }
            self.focus = next;
            self.dirty = true;
        }
        if let Focus::Editor(slot) = next
            && let Some(ed) = self.editor_mut(slot)
        {
            ed.focus(&shell.settings);
        }
    }

    fn focus_active_editor(&mut self, shell: &Shell) {
        self.set_focus(Focus::Editor(self.tabs.active_slot()), shell);
    }
}

// ---- Drawing

const HEADER_WIDTH: f32 = 5.;
const STRIP_TOP: f32 = 10.;
const CAPTION_BUTTON_WIDTH: f32 = 46.;
const CAPTION_WIDTH: f32 = 3. * CAPTION_BUTTON_WIDTH;
const DRAG_RESERVE: f32 = 43.;
const TITLE_BAR_MIN_WIDTH: f32 =
    HEADER_WIDTH + CAPTION_WIDTH + DRAG_RESERVE + tabs::MIN_STRIP_WIDTH;
const SEPARATOR_HEIGHT: f32 = 1.;
const MENU_BAR_LEFT: f32 = 4.;
const STATUS_BAR_HEIGHT: f32 = 31.;
const STATUS_DIVIDER_WIDTH: f32 = 1.;
const STATUS_DIVIDER_HEIGHT: f32 = 16.;
const THEME_BUTTON_WIDTH: f32 = 30.;
const THEME_BUTTON_RIGHT: f32 = 8.;
const THEME_GLYPH_SIZE: f32 = 16.;
const MOON_GLYPH: &str = "\u{E708}";
const SUN_GLYPH: &str = "\u{E706}";

impl MainWindow {
    /// Lays out and paints the window; whether another frame should follow on the next vsync.
    pub(crate) fn render_frame(&mut self, now: Instant, shell: &Shell) -> bool {
        let p = shell.palette;
        let view = shell.view;
        let caret_width_px = shell.settings.caret_width_px;
        let ts = &shell.text_system;
        self.dirty = false;
        let logical = self.win.window.logical_size();
        let scale = self.win.window.scale_factor();
        let window_active = self.win.window.is_active();

        self.ui.begin_frame(now, shell.settings.reduce_motion);
        let strip_column =
            logical.width - HEADER_WIDTH - CAPTION_WIDTH - DRAG_RESERVE - tabs::STRIP_END_RESERVE;
        let strip = self.tabs.sample(strip_column, &mut self.ui);
        self.menus.sample(&mut self.ui);

        let title_bar = self.title_bar(&strip, window_active, p);
        let menu_bar = self.menu_bar_row(p);
        let find_node = self.find.render(self.focused_slot(), p, &mut self.ui);
        let editor_node = self.editor_area(find_node, p);
        let status_node = view.status_bar.then(|| self.status_bar(p));
        let menu_popup = self.menus.render_popup(p, &mut self.ui);
        let dialog_node = self.dialogs.render(self.focus, p, &mut self.ui);

        let chrome = box_()
            .size_full()
            .child(title_bar)
            .child(menu_bar)
            .child(editor_node)
            .when_some(status_node, AppNode::child);
        let root = box_()
            .size_full()
            .child(chrome)
            .when_some(menu_popup, AppNode::child)
            .when_some(dialog_node, AppNode::child);
        let frame = AppFrame::layout(
            root,
            AvailSize::definite(logical.width, logical.height),
            scale,
            ts,
        );
        self.win.window.set_control_areas(frame.control_areas());
        self.update_min_size(view.status_bar, &frame, p, ts);

        // Every editor lays out where it landed: the document in the editor font, the fields in
        // the body font.
        let document = EditorPaintStyle {
            text: TextStyle::regular(EDITOR_FONT, EDITOR_FONT_SIZE * self.zoom()),
            line_height: self.line_height(),
            color: p.text_primary,
            caret_height: (CARET_HEIGHT * self.zoom()).round(),
        };
        let field = EditorPaintStyle {
            text: TextStyle::regular(TEXT_FONT, BODY_SIZE),
            line_height: BODY_LINE + 2.,
            color: p.text_primary,
            caret_height: FIELD_CARET,
        };
        let active_slot = self.tabs.active_slot();
        let mut editor_needs_redraw = false;
        for (token, rect) in frame.custom_rects() {
            let CustomDraw::Editor(slot) = token else {
                continue;
            };
            let focused = self.focus == Focus::Editor(slot);
            let (style, show_selection) = if slot == active_slot {
                let shown = focused || self.menus.is_open() || self.find.is_open();
                (&document, shown)
            } else {
                (&field, focused)
            };
            let params = EditorPaintParams {
                style,
                palette: p,
                window_active,
                show_selection,
                scale,
                caret_width_px,
                now,
            };
            let ed = slot_editor_mut(&mut self.tabs, &mut self.find, &mut self.dialogs, slot);
            if let Some(ed) = ed {
                editor_needs_redraw |= ed.prepare(rect, params, &mut self.ui, ts);
            }
        }

        let (tabs, find, dialogs) = (&self.tabs, &self.find, &self.dialogs);
        let needs_retry = self.win.surface.draw(p.window_clear, |renderer| {
            frame.paint(renderer, &mut |token, rect, renderer| match token {
                CustomDraw::Editor(slot) => {
                    if let Some(ed) = slot_editor(tabs, find, dialogs, slot) {
                        ed.paint(p, renderer);
                    }
                }
                // A spacer squeezed narrower than a divider draws one instead.
                CustomDraw::StatusSpacer => {
                    let width = rect.size.width;
                    if width < STATUS_DIVIDER_WIDTH + 0.5 {
                        let x = rect.origin.x + ((width - STATUS_DIVIDER_WIDTH) / 2.).round();
                        let line = Rect::new(
                            point(x, rect.origin.y),
                            size(STATUS_DIVIDER_WIDTH, rect.size.height),
                        );
                        renderer.paint_quad(fill(line, p.stroke));
                    }
                }
            });
        });

        let caret_hidden = self.key_tips_active();
        let ime_caret = self
            .focused_slot()
            .and_then(|slot| slot_editor(&self.tabs, &self.find, &self.dialogs, slot))
            .and_then(Editor::ime_caret_bounds);
        self.win
            .window
            .set_text_caret(ime_caret, window_active && !caret_hidden);

        self.has_painted = true;
        let ui_needs_vsync = self.ui.finish_frame(frame);
        if let Some(ptr) = self.ui.pointer() {
            self.sync_cursor(ptr.pos);
        }
        let needs_vsync = ui_needs_vsync || editor_needs_redraw || needs_retry;
        self.dirty |= needs_vsync;
        needs_vsync
    }

    /// Keeps the window from shrinking past its chrome, the status bar's text and one line.
    fn update_min_size(&mut self, status_bar: bool, frame: &AppFrame, p: Palette, ts: &TextSystem) {
        let scale = self.win.window.scale_factor();
        let (status_w, status_h) = if status_bar {
            let bar = AppFrame::measure(self.status_bar(p), AvailSize::MIN, scale, ts);
            (bar.width, SEPARATOR_HEIGHT + STATUS_BAR_HEIGHT)
        } else {
            (0., 0.)
        };
        let menu_width = frame
            .rect_of(HitId::Menu(MenuHit::Bar))
            .map_or(0., |r| r.size.width);
        let menu_row = MENU_BAR_LEFT + menu_width + THEME_BUTTON_WIDTH + THEME_BUTTON_RIGHT;
        let width = status_w.max(menu_row).max(TITLE_BAR_MIN_WIDTH);
        let chrome_top = frame.rect_of(HitId::EditorArea).map_or(0., |r| r.top());
        let next = size(width, chrome_top + self.editor_min_height() + status_h);
        if self.min_size.is_none_or(|prev| {
            (prev.width - next.width).abs() > 0.5 || (prev.height - next.height).abs() > 0.5
        }) {
            self.min_size = Some(next);
            self.win.window.set_min_size(Some(next));
        }
    }

    fn title_bar(&mut self, strip: &StripFrame, window_active: bool, p: Palette) -> AppNode {
        let drag = || box_().window_area(WindowControlArea::Drag);
        let strip_node = self.tabs.render(strip, p, &mut self.ui);
        let maximized = self.hwnd.is_maximized();
        let captions = caption_buttons(window_active, maximized, p, &mut self.ui);
        row()
            .flex_none()
            .h(STRIP_TOP + TAB_HEIGHT)
            .child(drag().flex_none().w(HEADER_WIDTH).h_full())
            .child(
                box_()
                    .flex_none()
                    .child(drag().h(STRIP_TOP))
                    .child(strip_node),
            )
            .child(drag().flex_1().h_full())
            .child(captions)
    }

    fn menu_bar_row(&mut self, p: Palette) -> AppNode {
        let menu = self.menus.render_bar(self.focus, p, &mut self.ui);
        let glyph = if p.dark { SUN_GLYPH } else { MOON_GLYPH };
        let theme_btn = icon_button(
            p,
            HitId::ThemeToggle,
            "Switch theme",
            (glyph, THEME_GLYPH_SIZE),
            size(THEME_BUTTON_WIDTH, MENU_BAR_HEIGHT),
            true,
            &mut self.ui,
        );
        box_()
            .flex_none()
            .child(
                row()
                    .h(MENU_BAR_HEIGHT)
                    .pl(MENU_BAR_LEFT)
                    .pr(THEME_BUTTON_RIGHT)
                    .bg(p.layer)
                    .child(menu)
                    .child(box_().flex_1())
                    .child(theme_btn),
            )
            .child(separator(p))
    }

    fn editor_area(&self, find: Option<AppNode>, p: Palette) -> AppNode {
        let line_height = self.line_height();
        let font_size = EDITOR_FONT_SIZE * self.zoom();
        let view = self.tabs.active().editor.view(line_height);
        box_()
            .hit(HitId::EditorArea)
            .flex_1()
            .min_h(self.editor_min_height())
            .bg(p.editor)
            .child(
                box_().size_full().overflow_hidden().child(
                    box_()
                        .size_full()
                        .font_family(EDITOR_FONT)
                        .text_size(font_size)
                        .line_height(line_height)
                        .text_color(p.text_primary)
                        .child(view),
                ),
            )
            .when_some(find, AppNode::child)
    }

    /// The status bar: where the caret is, how much is selected, the zoom and the file format.
    pub(crate) fn status_bar(&self, p: Palette) -> AppNode {
        let tab = self.tabs.active();
        let model = tab.editor.model();
        let text = model.text();
        let (line, column) = model.caret_line_col();
        let selection = model.selection().range();
        let selected = text
            .byte_to_char_idx(selection.end)
            .saturating_sub(text.byte_to_char_idx(selection.start));
        let zoom_pct = (tab.zoom * 100.).round() as u32;
        box_().flex_none().child(separator(p)).child(
            row()
                .items_center()
                .gap(24.)
                .h(STATUS_BAR_HEIGHT)
                .px(22.)
                .bg(p.layer)
                .font_family(SMALL_FONT)
                .text_size(12.)
                .text_color(p.text_secondary)
                .nowrap()
                .text_child(format!("Ln {line}, Col {column}"))
                .child(status_divider(p))
                .text_child(character_count(selected, text.len_chars()))
                .child(
                    custom(CustomDraw::StatusSpacer)
                        .flex_1()
                        .min_w(STATUS_DIVIDER_WIDTH)
                        .h(STATUS_DIVIDER_HEIGHT),
                )
                .text_child(format!("{zoom_pct}%"))
                .child(status_divider(p))
                .text_child(tab.format.line_ending_label())
                .child(status_divider(p))
                .text_child(tab.format.encoding.label()),
        )
    }
}

fn separator(p: Palette) -> AppNode {
    box_().flex_none().h(SEPARATOR_HEIGHT).bg(p.stroke_on_layer)
}

fn status_divider(p: Palette) -> AppNode {
    box_()
        .flex_none()
        .w(STATUS_DIVIDER_WIDTH)
        .h(STATUS_DIVIDER_HEIGHT)
        .bg(p.stroke)
}

fn group_thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn character_count(selected: usize, total: usize) -> String {
    let noun = if total == 1 {
        "character"
    } else {
        "characters"
    };
    if selected > 0 {
        format!(
            "{} of {} {noun}",
            group_thousands(selected),
            group_thousands(total)
        )
    } else {
        format!("{} {noun}", group_thousands(total))
    }
}

/// The minimize, maximize (or restore) and close buttons, then room to drag the window by.
fn caption_buttons(active: bool, maximized: bool, p: Palette, ui: &mut AppUi) -> AppNode {
    let max_glyph = if maximized { "\u{E923}" } else { "\u{E922}" };
    let buttons = [
        (CaptionHit::Min, WindowControlArea::Min, "\u{E921}"),
        (CaptionHit::Max, WindowControlArea::Max, max_glyph),
        (CaptionHit::Close, WindowControlArea::Close, "\u{E8BB}"),
    ]
    .map(|(hit, area, glyph)| caption_button(hit, area, glyph, active, p, ui));
    box_()
        .flex_none()
        .h_full()
        .child(row().children(buttons))
        .child(box_().flex_1().window_area(WindowControlArea::Drag))
}

/// A caption button; Close lights up red.
fn caption_button(
    hit: CaptionHit,
    area: WindowControlArea,
    glyph: &'static str,
    active: bool,
    p: Palette,
    ui: &mut AppUi,
) -> AppNode {
    let (hover_bg, pressed_bg, hover_fg, pressed_fg) = if hit == CaptionHit::Close {
        (
            p.close_hover,
            p.close_pressed,
            p.close_glyph_hover,
            p.close_glyph_pressed,
        )
    } else {
        (
            p.layer_hover,
            p.subtle_tertiary,
            p.caption_glyph,
            p.caption_glyph_pressed,
        )
    };
    let minimize = hit == CaptionHit::Min;
    let hit = HitId::Caption(hit);
    let brushes = Brushes {
        rest: transparent_black(),
        hover: hover_bg,
        pressed: pressed_bg,
    };
    let bg = ui.brush(hit, brushes, true);
    let fg = if ui.is_pressed(hit) {
        pressed_fg
    } else if ui.is_hovered(hit) {
        hover_fg
    } else if active {
        p.caption_glyph
    } else {
        p.caption_glyph_inactive
    };
    row()
        .hit(hit)
        .flex_none()
        .w(CAPTION_BUTTON_WIDTH)
        .h(30.)
        .items_center()
        .justify_center()
        .bg(bg)
        .font_family(ICON_FONT)
        .text_size(10.)
        .text_color(fg)
        .window_area(area)
        .child(
            box_()
                .when(minimize, |this| this.left(1.).top(1.))
                .child(text(glyph)),
        )
}
