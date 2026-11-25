//! The main windows. A `MainWindow` is one window: its tabs, menus, find bar and dialogs, the
//! input that reaches it, and its frame. `Windows` is the list of them, which opens, places and
//! sweeps them and lets file work see their tabs (`WindowsHost`); `TabDrag` moves a tab along
//! its strip, out into a window of its own, and into another window.
//!
//! The app forwards each window event to `on_event`, which also decides whether the window needs
//! painting. What spans windows (files, tab drags between windows, new windows) leaves as an
//! `Effect`; the app calls back in through the tab and file methods below.

use std::ops::{Deref, DerefMut};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::command::{AppCommand, Command, app_shortcut, edit_shortcut};
use crate::dialog::{DialogKey, DialogKind, Dialogs};
use crate::document::{self, DocPath, Format, PathKey};
use crate::editor::{EditCommand, Editor, EditorPaintParams, EditorPaintStyle};
use crate::files::{DocHost, FileOps, FileRequest, Prompt, PromptId, TabSnapshot};
use crate::find::{FindBar, FindKey, FindOutcome};
use crate::geom::{Edges, Point, Rect, ScreenPoint, Size, point, size, transparent_black};
use crate::gfx::{TextStyle, TextSystem, WindowSurface, fill};
use crate::layout::AvailSize;
use crate::menu::{FocusRequest, MENU_BAR_HEIGHT, MenuOutcome, MenuState, Menus};
use crate::platform::{
    KeyEvent, MainWindowConfig, Modifiers, MouseButton, NativeWindow, WindowControlArea,
    WindowEvent, WindowHandle, WindowId, WindowPlacement, WindowsDisplay, clipboard_has_text,
    cursor_screen_pos, debug_log, format_local_time_date, primary_mouse_button_down,
    release_mouse_capture, top_visible_window_at,
};
use crate::shell::{Cx, Effect, Shell};
use crate::tabs::{self, Removal, StripFrame, TAB_HEIGHT, TabData, TabSet};
use crate::text::{Rope, RopeExt as _};
use crate::theme::{BODY_SIZE, FIELD_CARET, FIELD_LINE, ICON_FONT, Palette, SMALL_FONT, TEXT_FONT};
use crate::tooltip::{KeyTip, OverlayAnchor, TooltipProbe};
use crate::types::APP_NAME;
use crate::ui::{
    AppFrame, AppNode, AppUi, Brushes, CaptionHit, CustomDraw, DialogHit, EditorSlot, Focus, HitId,
    MenuHit, TabHit, TabId, box_, custom, row, text,
};
use crate::widgets::icon_button;

// A new window's size, at most this share of the screen.
const WINDOW_WIDTH: f32 = 2213.;
const WINDOW_HEIGHT: f32 = 1107.;
const WINDOW_MAX_SCREEN_SHARE: f32 = 0.9;

// The document's text, and how far it zooms.
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

// The chrome: title bar, menu bar and status bar.
const HEADER_WIDTH: f32 = 5.;
const STRIP_TOP: f32 = 10.;
const CAPTION_BUTTON_WIDTH: f32 = 46.;
const CAPTION_WIDTH: f32 = 3. * CAPTION_BUTTON_WIDTH;
/// Room kept beside the strip to drag the window by.
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

// ---- 1. The window --------------------------------------------------------------------------

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
        let documents = self.tabs.iter().map(|tab| &tab.data.editor);
        let editors = documents.chain(self.fields()).filter_map(Editor::deadline);
        editors
            .chain(self.tabs.deadline())
            .chain(self.menus.deadline())
            .chain(self.find.deadline())
            .min()
    }

    pub(crate) fn apply_theme(&mut self, dark: bool) {
        self.win.window.set_dark(dark);
        self.dirty = true;
    }

    pub(crate) fn set_word_wrap(&mut self, shell: &Shell) {
        let wrap = shell.view.word_wrap;
        self.tabs.for_each_editor_mut(|ed| ed.set_soft_wrap(wrap));
        self.dirty = true;
    }

    /// Where the app's overlays for this window (key tips, tooltips) are placed from.
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
            .key_tips(self.focus, &self.ui.frame, &shell.text_system);
        (self.overlay_anchor(), tips)
    }

    pub(crate) fn tooltip_probe(&self) -> TooltipProbe<'_> {
        TooltipProbe {
            anchor: self.overlay_anchor(),
            pointer: self.ui.pointer,
            frame: &self.ui.frame,
        }
    }
}

// ---- 2. Input -------------------------------------------------------------------------------

impl MainWindow {
    /// Handles input to the window; for keys, whether it was used.
    pub(crate) fn on_event(&mut self, event: WindowEvent, cx: &mut Cx<'_>) -> bool {
        // Presses, keys, text and activation change what shows; pointer motion, the wheel, key
        // releases and moving the window repaint only if they change something.
        let repaint = matches!(
            event,
            WindowEvent::Resized
                | WindowEvent::Activated(_)
                | WindowEvent::MouseDown { .. }
                | WindowEvent::MouseUp { .. }
                | WindowEvent::KeyDown(_)
                | WindowEvent::TextInput(_)
                | WindowEvent::ImeComposition { .. }
        );
        let mut handled = false;
        match event {
            WindowEvent::Resized | WindowEvent::Moved => self.cancel_menu(cx.shell),
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
            WindowEvent::KeyUp(ev) if self.mode() != InputMode::Dialog => {
                let out = self.menus.on_key_up(ev, self.focus);
                if out.handled {
                    handled = self.apply_menu_outcome(out, cx);
                } else {
                    self.dirty |= out.dirty;
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
            WindowEvent::KeyUp(_)
            | WindowEvent::FilesDropped(_)
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
        let strip = self.ui.frame.rect_of(HitId::Tab(TabHit::Strip));
        strip.is_some_and(|r| r.contains(pos))
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
        if self.mode() != InputMode::Dialog {
            let (tabs, shell) = (&self.tabs, cx.shell);
            let state = || menu_state(tabs, shell);
            let delay = shell.settings.menu_show_delay;
            let out = self.menus.on_hover_change(top, state, delay);
            // Focus that follows the pointer no longer shows as from the keyboard.
            self.ui.last_input_was_keyboard &= !out.followed_pointer;
            self.apply_menu_outcome(out, cx);
        }
        if !self.pointer_in_strip(pos) {
            self.dirty |= self.tabs.thaw_if_frozen();
        }
        self.dirty |= self.tabs.active().editor.hover_scrollbar(pos, &self.ui);
        let dragging = pressed == Some(MouseButton::Left);
        if let Some(notify) = self.tabs.active_mut().editor.drag_scrollbar(pos, dragging) {
            self.dirty |= notify;
        } else {
            // Only the editor with the keyboard drag-selects.
            let ed = self.focused_slot().and_then(|slot| self.editor_mut(slot));
            let moved = ed.is_some_and(|ed| ed.on_mouse_move(pos, dragging));
            self.dirty |= moved;
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
        if button == MouseButton::Left {
            match chain.first().copied() {
                Some(HitId::ScrollTrack(slot, axis))
                    if slot == self.tabs.active_slot()
                        && self.tabs.active_mut().editor.press_scrollbar(axis, pos) =>
                {
                    return;
                }
                Some(
                    HitId::FieldClear(_)
                    | HitId::Dialog(_)
                    | HitId::Find(_)
                    | HitId::Tab(TabHit::Close(_)),
                ) => return,
                // A press on a tab shows it and may start dragging it.
                Some(HitId::Tab(TabHit::Item(id))) => {
                    if let Some(index) = self.tabs.index_of(id) {
                        self.activate_tab(index, cx.shell);
                        let tab = self.ui.frame.rect_of(HitId::Tab(TabHit::Item(id)));
                        let left = tab.map_or(pos.x, |r| r.origin.x);
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
        if button == MouseButton::Middle {
            if let Some(tab) = self.middle_pressed_tab.take()
                && over == Some(tab)
            {
                self.request_close(tab, cx);
            }
            return;
        }
        // A press that opened a menu, released on one of its rows, runs the row.
        let menu_release =
            (button == MouseButton::Left && clicked.is_none() && self.mode() == InputMode::Popup)
                .then_some(top)
                .flatten()
                .filter(|hit| matches!(hit, HitId::Menu(MenuHit::Row { .. })));
        self.tabs.active_mut().editor.release_scrollbar(pos);
        if let Some(ed) = self.focused_field() {
            ed.on_mouse_up(button);
        }
        if let Some(anchor) = self.tabs.active_mut().editor.on_mouse_up(button) {
            let state = menu_state(&self.tabs, cx.shell);
            let out = self.menus.open_context_menu(anchor, state);
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
        let chain = self.ui.frame.hit_chain(pos);
        if chain.contains(&HitId::Tab(TabHit::List)) && self.tabs.wheel(lines) {
            self.dirty = true;
        } else if modifiers.ctrl && chain.contains(&HitId::EditorArea) {
            if lines.y != 0. {
                self.set_zoom(self.zoom() + ZOOM_STEP.copysign(lines.y));
            }
        } else {
            let ed = &mut self.tabs.active_mut().editor;
            self.dirty |= ed.on_scroll_wheel(lines, page, modifiers);
        }
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
        self.win.window.set_cursor(self.ui.frame.cursor_at(pos));
    }
}

/// The tab, if any, among the hits under the pointer.
fn tab_at(chain: &[HitId]) -> Option<TabId> {
    chain.iter().find_map(|hit| match hit {
        HitId::Tab(TabHit::Item(id)) => Some(*id),
        _ => None,
    })
}

// ---- 3. Commands, find, dialogs and menus ---------------------------------------------------

impl MainWindow {
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
                let ed = &mut self.tabs.active_mut().editor;
                ed.replace(&stamp, &cx.shell.text_system);
            }
            AppCommand::GoTo => {
                let (line, _) = self.tabs.active().editor.model().caret_line_col();
                self.show_go_to(Some(line), cx.shell);
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
        let ed = self.editor_mut(slot);
        ed.is_some_and(|ed| ed.handle_command(cmd, &shell.text_system))
    }

    fn has_text(&self) -> bool {
        self.tabs.active().editor.model().text().len() > 0
    }

    fn zoom(&self) -> f32 {
        self.tabs.active().zoom
    }

    fn set_zoom(&mut self, zoom: f32) {
        let stepped = (zoom / ZOOM_STEP).round() * ZOOM_STEP;
        self.tabs.active_mut().zoom = stepped.clamp(ZOOM_MIN, ZOOM_MAX);
        self.dirty = true;
    }

    fn line_height(&self) -> f32 {
        (EDITOR_LINE_HEIGHT * self.zoom()).round()
    }

    fn editor_min_height(&self) -> f32 {
        EDITOR_PADDINGS.top + EDITOR_PADDINGS.bottom + self.line_height()
    }

    /// Opens the find bar (with replace), seeded with the selection.
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
        } else if !self.find.is_open() {
            self.focus_active_editor(shell);
        }
    }

    pub(crate) fn show_prompt(&mut self, prompt: Prompt, shell: &Shell) {
        if let Some(next) = self.dialogs.enqueue(DialogKind::Prompt(prompt)) {
            self.cancel_menu(shell);
            self.set_focus(next, shell);
            self.dirty = true;
        }
    }

    /// Shows the go-to dialog, its field holding `line` (or what was typed before).
    fn show_go_to(&mut self, line: Option<usize>, shell: &Shell) {
        self.cancel_menu(shell);
        let next = self.dialogs.show_go_to(line, &shell.text_system);
        self.set_focus(next, shell);
    }

    /// Closes the open dialog, showing the next one queued, or returning to the document.
    fn close_dialog(&mut self, shell: &Shell) {
        if let Some(next) = self.dialogs.dismiss() {
            self.cancel_menu(shell);
            self.set_focus(next, shell);
        } else {
            self.focus_active_editor(shell);
        }
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
                    None => {
                        self.cancel_menu(cx.shell);
                        let next = self.dialogs.show_now(DialogKind::LineOutOfRange);
                        self.set_focus(next, cx.shell);
                    }
                }
            }
            DialogKind::GoTo => self.close_dialog(cx.shell),
            DialogKind::LineOutOfRange => self.show_go_to(None, cx.shell),
            DialogKind::Prompt(prompt) => {
                self.close_dialog(cx.shell);
                if let Some((id, _)) = prompt.reply {
                    let answer = prompt.answer(button);
                    cx.emit(Effect::PromptResponse { id, answer });
                }
            }
        }
    }

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
        match out.command {
            Some(Command::App(cmd)) => {
                self.dispatch_app_command(cmd, cx);
            }
            Some(Command::Edit(cmd)) => {
                self.run_edit_command(self.tabs.active_slot(), cmd, cx.shell);
            }
            None => return out.handled,
        }
        true
    }
}

/// What the menus show as enabled or checked.
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

// ---- 4. Tabs, and the calls the app makes for files and tab drags ---------------------------

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

    pub(crate) fn remove_tab(&mut self, ix: usize, shell: &Shell) {
        let pointer = self.ui.pointer;
        let hovering = pointer.is_some_and(|p| self.pointer_in_strip(p.pos));
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

    /// After a tab left: the window closes if it was the last, else the next tab shows (and
    /// takes focus, unless it's in the find bar and the active tab stayed).
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
    fn restore_tab(&mut self, data: TabData, ix: usize, shell: &Shell) {
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

    /// Whether a tab dragged from another window may land here.
    fn accepts_tab_drop(&self) -> bool {
        !self.dialogs.is_open() && !self.pending_close && self.hwnd.is_enabled()
    }

    /// Whether `pt` is over the tab strip, give or take `magnetism` above and below.
    fn strip_hit(&self, pt: ScreenPoint, magnetism: f32) -> bool {
        let pos = self.hwnd.to_client(pt);
        let frame = &self.ui.frame;
        let right = frame.rect_of(HitId::Caption(CaptionHit::Min)).map_or_else(
            || self.win.window.logical_size().width - CAPTION_WIDTH,
            |r| r.left(),
        );
        let fallback = (
            HEADER_WIDTH + tabs::LIST_PAD,
            STRIP_TOP,
            STRIP_TOP + TAB_HEIGHT,
        );
        let list = frame.rect_of(HitId::Tab(TabHit::List));
        let (left, top, bottom) = list.map_or(fallback, |r| (r.left(), r.top(), r.bottom()));
        (left..right).contains(&pos.x) && (top - magnetism..bottom + magnetism).contains(&pos.y)
    }

    /// Moves the window so a tab grabbed at `grab`, as the strip's first, sits under `pt`.
    fn place_under(&self, pt: ScreenPoint, grab: Point) {
        let offset = point(HEADER_WIDTH + tabs::LIST_PAD + grab.x, STRIP_TOP + grab.y);
        let (at, frame) = (self.hwnd.to_screen(offset), self.hwnd.window_rect());
        self.hwnd
            .move_noactivate(pt.x - (at.x - frame.left), pt.y - (at.y - frame.top));
    }
}

// ---- 5. Editors and focus -------------------------------------------------------------------

/// The editor at `slot`: a tab's document, a find field, or the go-to field. Free functions, so
/// a frame can borrow the editors apart from the rest of the window.
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
        let (find, dialogs) = (&mut self.find, &mut self.dialogs);
        [&mut find.query, &mut find.replacement, &mut dialogs.line]
    }

    fn focused_slot(&self) -> Option<EditorSlot> {
        match self.focus {
            Focus::Editor(slot) => Some(slot),
            Focus::MenuBar | Focus::DialogButton(_) => None,
        }
    }

    /// The focused editor when it isn't the document's (a find or go-to field).
    fn focused_field(&mut self) -> Option<&mut Editor> {
        let active = self.tabs.active_slot();
        let slot = self.focused_slot().filter(|&slot| slot != active)?;
        self.editor_mut(slot)
    }

    /// Keeps the focused caret steady while keys go to it.
    fn pause_blink(&mut self) {
        if let Some(ed) = self.focused_slot().and_then(|slot| self.editor_mut(slot)) {
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

// ---- 6. Drawing -----------------------------------------------------------------------------

impl MainWindow {
    /// Lays out and paints the window; whether another frame should follow on the next vsync.
    pub(crate) fn render_frame(&mut self, now: Instant, shell: &Shell) -> bool {
        let (p, view, ts) = (shell.palette, shell.view, &shell.text_system);
        self.dirty = false;
        let window = &self.win.window;
        let (logical, scale, window_active) = (
            window.logical_size(),
            window.scale_factor(),
            window.is_active(),
        );

        self.ui.begin_frame(now, shell.settings.reduce_motion);
        let strip_column =
            logical.width - HEADER_WIDTH - CAPTION_WIDTH - DRAG_RESERVE - tabs::STRIP_END_RESERVE;
        let strip = self.tabs.sample(strip_column, &mut self.ui);
        self.menus.sample(&mut self.ui);
        let title_bar = self.title_bar(&strip, window_active, p);
        let menu_bar = self.menu_bar_row(p);
        let find = self.find.render(self.focused_slot(), p, &mut self.ui);
        let editor = self.editor_area(find, p);
        let status = view.status_bar.then(|| self.status_bar(p));
        let popup = self.menus.render_popup(p, &mut self.ui);
        let dialog = self.dialogs.render(self.focus, p, &mut self.ui);
        let chrome = box_()
            .size_full()
            .child(title_bar)
            .child(menu_bar)
            .child(editor)
            .when_some(status, AppNode::child);
        let root = box_()
            .size_full()
            .child(chrome)
            .when_some(popup, AppNode::child)
            .when_some(dialog, AppNode::child);
        let available = AvailSize::definite(logical.width, logical.height);
        let frame = AppFrame::layout(root, available, scale, ts);
        self.win
            .window
            .set_control_areas(frame.control_areas.clone());
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
            line_height: FIELD_LINE,
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
                (
                    &document,
                    focused || self.menus.is_open() || self.find.is_open(),
                )
            } else {
                (&field, focused)
            };
            let params = EditorPaintParams {
                style,
                palette: p,
                window_active,
                show_selection,
                scale,
                caret_width_px: shell.settings.caret_width_px,
                now,
            };
            let ed = slot_editor_mut(&mut self.tabs, &mut self.find, &mut self.dialogs, slot);
            if let Some(ed) = ed {
                editor_needs_redraw |= ed.prepare(rect, params, &mut self.ui, ts);
            }
        }

        let (tabs, find, dialogs) = (&self.tabs, &self.find, &self.dialogs);
        let needs_retry = self.win.draw(p.window_clear, |canvas| {
            frame.paint(canvas, &mut |token, rect, canvas| match token {
                CustomDraw::Editor(slot) => {
                    if let Some(ed) = slot_editor(tabs, find, dialogs, slot) {
                        ed.paint(p, canvas);
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
                        canvas.paint_quad(fill(line, p.stroke));
                    }
                }
            });
        });

        // The IME's caret follows the focused editor's, and the system caret shows unless key
        // tips cover the window.
        let ime_caret = self
            .focused_slot()
            .and_then(|slot| slot_editor(&self.tabs, &self.find, &self.dialogs, slot))
            .and_then(Editor::ime_caret_bounds);
        let show_caret = window_active && !self.key_tips_active();
        self.win.window.set_text_caret(ime_caret, show_caret);
        self.has_painted = true;
        let ui_needs_vsync = self.ui.finish_frame(frame);
        if let Some(ptr) = self.ui.pointer {
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
        let menu = frame.rect_of(HitId::Menu(MenuHit::Bar));
        let menu_row = MENU_BAR_LEFT
            + menu.map_or(0., |r| r.size.width)
            + THEME_BUTTON_WIDTH
            + THEME_BUTTON_RIGHT;
        let width = status_w.max(menu_row).max(TITLE_BAR_MIN_WIDTH);
        let chrome_top = frame.rect_of(HitId::EditorArea).map_or(0., |r| r.top());
        let next = size(width, chrome_top + self.editor_min_height() + status_h);
        let moved = |prev: Size| {
            (prev.width - next.width).abs() > 0.5 || (prev.height - next.height).abs() > 0.5
        };
        if self.min_size.is_none_or(moved) {
            self.min_size = Some(next);
            self.win.window.set_min_size(Some(next));
        }
    }

    /// The tab strip, with room to drag the window by around it, and the caption buttons.
    fn title_bar(&mut self, strip: &StripFrame, window_active: bool, p: Palette) -> AppNode {
        let drag = || box_().window_area(WindowControlArea::Drag);
        let strip = self.tabs.render(strip, p, &mut self.ui);
        let maximized = self.hwnd.is_maximized();
        let captions = caption_buttons(window_active, maximized, p, &mut self.ui);
        row()
            .flex_none()
            .h(STRIP_TOP + TAB_HEIGHT)
            .child(drag().flex_none().w(HEADER_WIDTH).h_full())
            .child(box_().flex_none().child(drag().h(STRIP_TOP)).child(strip))
            .child(drag().flex_1().h_full())
            .child(captions)
    }

    /// The menu bar, with the theme switch at its right.
    fn menu_bar_row(&mut self, p: Palette) -> AppNode {
        let menu = self.menus.render_bar(self.focus, p, &mut self.ui);
        let glyph = if p.dark { SUN_GLYPH } else { MOON_GLYPH };
        let theme = icon_button(
            p,
            HitId::ThemeToggle,
            "Switch theme",
            (glyph, THEME_GLYPH_SIZE),
            size(THEME_BUTTON_WIDTH, MENU_BAR_HEIGHT),
            true,
            &mut self.ui,
        );
        let bar = row()
            .h(MENU_BAR_HEIGHT)
            .pl(MENU_BAR_LEFT)
            .pr(THEME_BUTTON_RIGHT)
            .bg(p.layer)
            .child(menu)
            .child(box_().flex_1())
            .child(theme);
        box_().flex_none().child(bar).child(separator(p))
    }

    /// The active document, with the find bar over it.
    fn editor_area(&self, find: Option<AppNode>, p: Palette) -> AppNode {
        let view = self.tabs.active().editor.view(self.line_height());
        box_()
            .hit(HitId::EditorArea)
            .flex_1()
            .min_h(self.editor_min_height())
            .bg(p.editor)
            .child(box_().size_full().overflow_hidden().child(view))
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
        let spacer = custom(CustomDraw::StatusSpacer)
            .flex_1()
            .min_w(STATUS_DIVIDER_WIDTH)
            .h(STATUS_DIVIDER_HEIGHT);
        let bar = row()
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
            .child(spacer)
            .text_child(format!("{zoom_pct}%"))
            .child(status_divider(p))
            .text_child(tab.format.line_ending_label())
            .child(status_divider(p))
            .text_child(tab.format.encoding.label());
        box_().flex_none().child(separator(p)).child(bar)
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

/// "1,234 characters", or "5 of 1,234 characters" with some selected.
fn character_count(selected: usize, total: usize) -> String {
    let grouped = |n: usize| {
        let digits = n.to_string();
        let mut out = String::with_capacity(digits.len() + digits.len() / 3);
        for (i, c) in digits.chars().enumerate() {
            if i > 0 && (digits.len() - i).is_multiple_of(3) {
                out.push(',');
            }
            out.push(c);
        }
        out
    };
    let noun = if total == 1 {
        "character"
    } else {
        "characters"
    };
    if selected > 0 {
        format!("{} of {} {noun}", grouped(selected), grouped(total))
    } else {
        format!("{} {noun}", grouped(total))
    }
}

/// The minimize, maximize (or restore) and close buttons, then room to drag the window by. Each
/// lights up under the pointer, Close in red.
fn caption_buttons(active: bool, maximized: bool, p: Palette, ui: &mut AppUi) -> AppNode {
    let max_glyph = if maximized { "\u{E923}" } else { "\u{E922}" };
    let buttons = [
        (CaptionHit::Min, WindowControlArea::Min, "\u{E921}"),
        (CaptionHit::Max, WindowControlArea::Max, max_glyph),
        (CaptionHit::Close, WindowControlArea::Close, "\u{E8BB}"),
    ]
    .map(|(caption, area, glyph)| {
        let (hover_bg, pressed_bg, hover_fg, pressed_fg) = if caption == CaptionHit::Close {
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
        let hit = HitId::Caption(caption);
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
        let minimize = caption == CaptionHit::Min;
        let glyph = box_()
            .when(minimize, |this| this.left(1.).top(1.))
            .child(text(glyph));
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
            .child(glyph)
    });
    box_()
        .flex_none()
        .h_full()
        .child(row().children(buttons))
        .child(box_().flex_1().window_area(WindowControlArea::Drag))
}

// ---- 7. The window list ---------------------------------------------------------------------

/// How new windows open: shown, focused, and on the display the cursor is on.
#[derive(Copy, Clone, Debug)]
pub(crate) struct WindowLaunch {
    pub(crate) show: bool,
    pub(crate) focus: bool,
    use_cursor_display: bool,
}

impl Default for WindowLaunch {
    fn default() -> Self {
        Self {
            show: true,
            focus: true,
            use_cursor_display: true,
        }
    }
}

impl WindowLaunch {
    #[cfg(test)]
    pub(crate) const HIDDEN: Self = Self {
        show: false,
        focus: false,
        use_cursor_display: false,
    };
}

/// A window that couldn't open, with the tab it was to hold.
pub(crate) struct OpenWindowError {
    pub(crate) tab: Option<TabData>,
    pub(crate) message: String,
}

impl std::fmt::Debug for OpenWindowError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenWindowError")
            .field("has_tab", &self.tab.is_some())
            .field("message", &self.message)
            .finish()
    }
}

/// The app's windows, oldest first, and the one last active.
pub(crate) struct Windows {
    list: Vec<MainWindow>,
    active: Option<WindowId>,
    /// How many new windows have cascaded from the centre.
    cascade_step: u32,
    launch: WindowLaunch,
}

impl Deref for Windows {
    type Target = [MainWindow];

    fn deref(&self) -> &[MainWindow] {
        &self.list
    }
}

impl DerefMut for Windows {
    fn deref_mut(&mut self) -> &mut [MainWindow] {
        &mut self.list
    }
}

impl Windows {
    pub(crate) fn new(launch: WindowLaunch) -> Self {
        Self {
            list: Vec::new(),
            active: None,
            cascade_step: 0,
            launch,
        }
    }

    pub(crate) fn set_active(&mut self, id: WindowId) {
        self.active = Some(id);
    }

    pub(crate) fn ix(&self, id: WindowId) -> Option<usize> {
        self.list.iter().position(|w| w.id == id)
    }

    pub(crate) fn get(&self, id: WindowId) -> Option<&MainWindow> {
        self.list.iter().find(|w| w.id == id)
    }

    pub(crate) fn get_mut(&mut self, id: WindowId) -> Option<&mut MainWindow> {
        self.list.iter_mut().find(|w| w.id == id)
    }

    pub(crate) fn hwnd(&self, id: WindowId) -> Option<WindowHandle> {
        self.get(id).map(|w| w.hwnd)
    }

    pub(crate) fn ids(&self) -> Vec<WindowId> {
        self.list.iter().map(|w| w.id).collect()
    }

    /// The window holding tab `tab`, and where in its tabs: as indices.
    pub(crate) fn owner_of(&self, tab: TabId) -> Option<(usize, usize)> {
        let mut found = self.list.iter().enumerate();
        found.find_map(|(wix, w)| w.tabs.index_of(tab).map(|tix| (wix, tix)))
    }

    pub(crate) fn find_by_key(&self, key: &PathKey) -> Option<(WindowId, TabId)> {
        self.list.iter().find_map(|w| {
            let tix = w.tabs.find_by_key(key)?;
            Some((w.id, w.tabs[tix].data.id))
        })
    }

    /// `preferred` if it's still open, else the active window, else the newest.
    pub(crate) fn fallback(&self, preferred: Option<WindowId>) -> Option<WindowId> {
        let open = |id: Option<WindowId>| id.filter(|&id| self.ix(id).is_some());
        open(preferred)
            .or_else(|| open(self.active))
            .or_else(|| self.list.last().map(|w| w.id))
    }

    pub(crate) fn mark_dirty(&mut self, id: WindowId) {
        if let Some(w) = self.get_mut(id) {
            w.dirty = true;
        }
    }

    pub(crate) fn mark_all_dirty(&mut self) {
        for w in &mut self.list {
            w.dirty = true;
        }
    }

    pub(crate) fn apply_theme(&mut self, dark: bool) {
        for w in &mut self.list {
            w.apply_theme(dark);
        }
    }

    pub(crate) fn set_word_wrap(&mut self, shell: &Shell) {
        for w in &mut self.list {
            w.set_word_wrap(shell);
        }
    }

    pub(crate) fn any_dirty(&self) -> bool {
        self.list.iter().any(|w| w.tabs.has_dirty())
    }

    /// Drops the windows that closed: their ids, with the prompts they still had open.
    pub(crate) fn sweep_closed(&mut self) -> Vec<(WindowId, Vec<PromptId>)> {
        let closing = self.list.iter_mut().filter(|w| w.pending_close);
        let closed: Vec<_> = closing
            .map(|w| (w.id, w.dialogs.drain_prompt_ids()))
            .collect();
        if closed.is_empty() {
            return closed;
        }
        self.list.retain(|w| !w.pending_close);
        if self.active.is_some_and(|id| self.ix(id).is_none()) {
            self.active = self.list.last().map(|w| w.id);
        }
        closed
    }

    /// Opens a window holding `initial` (or a new tab), `window_size` or the default size,
    /// centred on the display `target_point` (else the active window's, or the cursor's) is
    /// on; a new window of the default size cascades from the last.
    pub(crate) fn open_at(
        &mut self,
        initial: Option<TabData>,
        window_size: Option<Size>,
        target_point: Option<ScreenPoint>,
        shell: &Shell,
    ) -> Result<(WindowId, WindowHandle), Box<OpenWindowError>> {
        let active_origin = self
            .active
            .and_then(|id| self.get(id))
            .map(|w| w.hwnd.client_origin());
        let on = target_point
            .or(active_origin)
            .or_else(|| self.launch.use_cursor_display.then(cursor_screen_pos));
        let display = WindowsDisplay::from_point(on.unwrap_or_default());
        let vis = display.visible_bounds();
        let target = window_size.unwrap_or_else(|| {
            size(
                WINDOW_WIDTH.min(vis.size.width * WINDOW_MAX_SCREEN_SHARE),
                WINDOW_HEIGHT.min(vis.size.height * WINDOW_MAX_SCREEN_SHARE),
            )
        });
        // Centred in the work area (pinned to its top left if larger).
        let centre = |span: f32, of: f32| ((span - of) * 0.5).max(0.0);
        let mut origin = vis.origin
            + point(
                centre(vis.size.width, target.width),
                centre(vis.size.height, target.height),
            );
        if target_point.is_none() && !self.list.is_empty() && window_size.is_none() {
            let step = (self.cascade_step % 8) as f32 * 28.0;
            self.cascade_step = self.cascade_step.wrapping_add(1);
            let max_x = (vis.right() - target.width).max(vis.left());
            let max_y = (vis.bottom() - target.height).max(vis.top());
            origin = point(
                (origin.x + step).clamp(vis.left(), max_x),
                (origin.y + step).clamp(vis.top(), max_y),
            );
        }
        let config = MainWindowConfig {
            bounds: Rect::new(origin, target),
            scale: display.scale_factor(),
            focus: self.launch.focus,
            show: self.launch.show,
        };
        let created = NativeWindow::create_main(&config, shell.dark())
            .and_then(|native| WindowSurface::new(shell.gpu.clone(), native));
        let surface = match created {
            Ok(surface) => surface,
            Err(err) => {
                let message = err.to_string();
                debug_log(&format!("window/surface create failed: {message}"));
                return Err(Box::new(OpenWindowError {
                    tab: initial,
                    message,
                }));
            }
        };
        let (id, hwnd) = (surface.window.id, surface.window.hwnd());
        self.list.push(MainWindow::new(surface, initial, shell));
        self.active = Some(id);
        Ok((id, hwnd))
    }

    pub(crate) fn host<'a>(&'a mut self, shell: &'a Shell) -> WindowsHost<'a> {
        WindowsHost {
            windows: self,
            shell,
        }
    }
}

/// The windows as file work sees them: tabs to save, load into, close and ask about.
pub(crate) struct WindowsHost<'a> {
    windows: &'a mut Windows,
    shell: &'a Shell,
}

impl WindowsHost<'_> {
    /// The window holding tab `tab`.
    fn owner(&mut self, tab: TabId) -> Option<&mut MainWindow> {
        let (wix, _) = self.windows.owner_of(tab)?;
        Some(&mut self.windows[wix])
    }
}

impl DocHost for WindowsHost<'_> {
    fn tab_info(&self, tab: TabId) -> Option<TabSnapshot> {
        let (wix, tix) = self.windows.owner_of(tab)?;
        let w = &self.windows[wix];
        let data = &w.tabs[tix].data;
        Some(TabSnapshot {
            window: w.id,
            name: w.tabs.tab_name(tix),
            path: data.doc_path.clone(),
            format: data.format,
            dirty: data.is_dirty(),
            revision: data.editor.model().revision(),
        })
    }

    fn window_dirty_tabs(&self, window: WindowId) -> Vec<TabId> {
        let w = self.windows.get(window);
        w.map(|w| w.tabs.dirty_tabs()).unwrap_or_default()
    }

    fn active_tab_path(&self, window: WindowId) -> Option<PathBuf> {
        self.windows
            .get(window)?
            .tabs
            .active()
            .path()
            .map(PathBuf::from)
    }

    fn fallback_window(&self, preferred: WindowId) -> Option<WindowId> {
        self.windows.fallback(Some(preferred))
    }

    fn find_tab_by_key(&self, key: &PathKey) -> Option<(WindowId, TabId)> {
        self.windows.find_by_key(key)
    }

    fn activate_tab(&mut self, window: WindowId, tab: TabId, bring_to_front: bool) {
        let Some(w) = self.windows.get_mut(window) else {
            return;
        };
        if let Some(tix) = w.tabs.index_of(tab) {
            w.activate_tab(tix, self.shell);
            if bring_to_front {
                w.hwnd.bring_to_front();
            }
        }
    }

    fn begin_save(&mut self, tab: TabId) -> Option<(u64, Rope)> {
        self.owner(tab)?
            .tabs
            .editor_mut(tab)
            .map(Editor::begin_save)
    }

    fn mark_saved(&mut self, tab: TabId, snapshot: u64, doc_path: DocPath, format: Format) {
        if let Some(w) = self.owner(tab)
            && w.tabs.mark_saved(tab, snapshot, doc_path, format)
        {
            w.refresh_title();
            w.dirty = true;
        }
    }

    fn adopt_loaded(&mut self, window: WindowId, loaded: document::Loaded) {
        if let Some(w) = self.windows.get_mut(window) {
            w.adopt_loaded_file(loaded, self.shell);
        }
    }

    fn remove_tab(&mut self, tab: TabId) {
        if let Some((wix, tix)) = self.windows.owner_of(tab) {
            self.windows[wix].remove_tab(tix, self.shell);
        }
    }

    fn mark_window_close(&mut self, window: WindowId) {
        if let Some(w) = self.windows.get_mut(window) {
            w.pending_close = true;
        }
    }

    fn window_handle(&self, window: WindowId) -> Option<WindowHandle> {
        self.windows.hwnd(window)
    }

    fn show_prompt(&mut self, window: WindowId, prompt: Prompt) {
        if let Some(w) = self.windows.get_mut(window) {
            w.show_prompt(prompt, self.shell);
        }
    }
}

// ---- 8. Tab drags ---------------------------------------------------------------------------

/// How far the pointer moves before a pressed tab starts dragging.
const DRAG_THRESHOLD: f32 = 10.;
/// How far above or below a strip a dragged tab still counts as over it.
const DETACH_MAGNETISM: f32 = 15.;
/// How often a window in Windows' move loop is checked on, and how long the loop may take to
/// start.
const MOVE_POLL: Duration = Duration::from_millis(16);
const MOVE_LOOP_START_TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Clone, Copy)]
enum Phase {
    /// Pressed, not yet moved far enough to drag.
    Pressed,
    /// Dragged along the owner's strip.
    Dragging,
    /// In a window of its own, handed to Windows' move loop: waiting for it to start, then
    /// moving (and, over another window's strip, ending the loop to attach there).
    Entering {
        since: Instant,
    },
    Moving {
        attaching: bool,
    },
}

/// A tab being dragged: from `source`, now in `owner` (they differ once it has moved windows).
pub(crate) struct TabDrag {
    pub(crate) owner: WindowId,
    pub(crate) source: WindowId,
    tab: TabId,
    /// Where the tab was held (from its top left), and where the press was, along the strip and
    /// on the screen.
    grab: Point,
    start: f32,
    press: ScreenPoint,
    phase: Phase,
    /// Where the tab was in its source, and the source window's placement, to put things back
    /// on a cancel.
    source_index: usize,
    source_placement: Option<WindowPlacement>,
    /// While Windows moves the dragged tab's window: when to check on the move next.
    pub(crate) poll_at: Option<Instant>,
}

impl TabDrag {
    /// A press on tab `tab` (at `index`, held `grab` in) at `position` in window `window_id`.
    pub(crate) fn start(
        window_id: WindowId,
        tab: TabId,
        index: usize,
        grab: Point,
        position: Point,
        windows: &mut Windows,
    ) -> Option<Self> {
        let win = windows.get_mut(window_id)?;
        win.dirty = true;
        Some(Self {
            owner: window_id,
            source: window_id,
            tab,
            grab,
            start: position.x,
            press: win.hwnd.to_screen(position),
            phase: Phase::Pressed,
            source_index: index,
            source_placement: None,
            poll_at: None,
        })
    }

    pub(crate) fn is_dragging(&self) -> bool {
        matches!(self.phase, Phase::Dragging)
    }

    fn is_in_move_loop(&self) -> bool {
        matches!(self.phase, Phase::Entering { .. } | Phase::Moving { .. })
    }

    /// Whether the dragged tab is still where the drag thinks it is.
    pub(crate) fn is_alive(&self, windows: &Windows) -> bool {
        let owner = windows.get(self.owner);
        owner.is_some_and(|w| w.tabs.index_of(self.tab).is_some())
    }

    /// The owner entered or left Windows' move loop; whether the drag is over.
    pub(crate) fn on_size_move_changed(
        &mut self,
        window_id: WindowId,
        entering: bool,
        windows: &mut Windows,
        file_ops: &FileOps,
        shell: &Shell,
    ) -> bool {
        if self.owner != window_id {
            return false;
        }
        if !entering {
            return self.is_in_move_loop() && self.finish_move_loop(windows, file_ops, shell);
        }
        if matches!(self.phase, Phase::Entering { .. }) {
            self.phase = Phase::Moving { attaching: false };
        }
        false
    }

    /// The pointer moved during the drag: the tab follows it along the strip, into another
    /// window's strip, or out into a window of its own. Whether the drag is over.
    pub(crate) fn on_mouse_move(
        &mut self,
        pressed_left: bool,
        windows: &mut Windows,
        file_ops: &FileOps,
        shell: &Shell,
    ) -> bool {
        if !pressed_left {
            return self.on_release(windows);
        }
        let Some(scale) = windows.get(self.owner).map(|w| w.win.window.scale_factor()) else {
            return true;
        };
        let pt = cursor_screen_pos();
        match self.phase {
            Phase::Pressed => {
                let distance = ((pt.x - self.press.x) as f32).hypot((pt.y - self.press.y) as f32);
                if distance / scale <= DRAG_THRESHOLD {
                    return false;
                }
                self.phase = Phase::Dragging;
            }
            Phase::Dragging => {}
            Phase::Entering { .. } | Phase::Moving { .. } => return false,
        }
        // A lone tab takes its window with it.
        let (alone, in_own_strip) = windows.get(self.owner).map_or((true, false), |w| {
            (w.tabs.len() == 1, w.strip_hit(pt, DETACH_MAGNETISM))
        });
        if !alone && in_own_strip {
            if let Some(win) = windows.get_mut(self.owner) {
                let x = win.hwnd.to_client(pt).x;
                let start = self.start - HEADER_WIDTH;
                win.tabs
                    .drag(self.tab, x - HEADER_WIDTH, self.grab.x, start);
                win.dirty = true;
            }
        } else if !alone && let Some(target) = strip_window(windows, file_ops, pt, None) {
            self.attach(target, pt, windows, shell);
        } else {
            self.detach(alone, pt, windows, shell);
        }
        windows.mark_dirty(self.owner);
        false
    }

    /// The button came up: a dragged tab settles where it is, and a window it moved to comes
    /// to the front. Whether the drag is over (not while Windows is moving the window).
    pub(crate) fn on_release(&mut self, windows: &mut Windows) -> bool {
        if self.is_in_move_loop() {
            return false;
        }
        if self.is_dragging() {
            if let Some(win) = windows.get_mut(self.owner) {
                win.dirty |= win.tabs.release_drag();
            }
            if self.owner != self.source
                && let Some(hwnd) = windows.hwnd(self.owner)
            {
                hwnd.set_foreground();
            }
        }
        true
    }

    pub(crate) fn on_capture_lost(&mut self, window_id: WindowId, windows: &mut Windows) -> bool {
        self.owner == window_id && self.is_dragging() && self.on_release(windows)
    }

    /// Puts the tab back where it came from: in its slot, its source window back in place, or
    /// (with the source gone) in a window of its own.
    pub(crate) fn cancel(&mut self, windows: &mut Windows, shell: &Shell) {
        self.poll_at = None;
        release_mouse_capture();
        if matches!(self.phase, Phase::Pressed) {
            return;
        }
        let (tab, index) = (self.tab, self.source_index);
        if self.owner == self.source {
            if self.is_dragging() {
                if let Some(win) = windows.get_mut(self.source) {
                    win.return_tab(tab, index, shell);
                }
            } else if let Some(placement) = self.source_placement
                && let Some(hwnd) = windows.hwnd(self.source)
            {
                hwnd.restore_placement(placement);
            }
            return;
        }
        let owner = windows.get_mut(self.owner);
        let Some(data) = owner.and_then(|w| w.take_tab(tab, shell)) else {
            return;
        };
        if let Some(src) = windows.get_mut(self.source) {
            src.restore_tab(data, index, shell);
            src.hwnd.set_foreground();
            return;
        }
        match windows.open_at(Some(data), None, None, shell) {
            Ok((_, hwnd)) => {
                if let Some(placement) = self.source_placement {
                    hwnd.restore_placement(placement);
                }
            }
            // No window for it: it goes at the end of the last one.
            Err(err) => {
                if let Some(data) = err.tab
                    && let Some(win) = windows.last_mut()
                {
                    win.restore_tab(data, win.tabs.len(), shell);
                }
            }
        }
    }

    /// Checks on Windows' move loop: over another window's strip, the loop is ended to attach
    /// there; once it's over (or never started), the drag finishes. Whether the drag is over.
    pub(crate) fn on_poll(
        &mut self,
        windows: &mut Windows,
        file_ops: &FileOps,
        shell: &Shell,
    ) -> bool {
        self.poll_at = Some(Instant::now() + MOVE_POLL);
        let Some(hwnd) = windows.hwnd(self.owner) else {
            return true;
        };
        let attaching = match self.phase {
            Phase::Entering { .. } => false,
            Phase::Moving { attaching } => attaching,
            Phase::Pressed | Phase::Dragging => return false,
        };
        if hwnd.in_move_loop() {
            let over = strip_window(windows, file_ops, cursor_screen_pos(), Some(self.owner));
            let attaching = attaching || over.is_some();
            if attaching && !matches!(self.phase, Phase::Moving { attaching: true }) {
                hwnd.cancel_mode();
            }
            self.phase = Phase::Moving { attaching };
            return false;
        }
        if let Phase::Entering { since } = self.phase
            && since.elapsed() < MOVE_LOOP_START_TIMEOUT
        {
            return false;
        }
        self.finish_move_loop(windows, file_ops, shell)
    }

    /// Windows' move loop ended: over a strip the tab attaches there; still held after an
    /// attach that missed, the window moves on; otherwise the drag ends (cancelled if the
    /// loop ended with the button still down). Whether the drag is over.
    fn finish_move_loop(
        &mut self,
        windows: &mut Windows,
        file_ops: &FileOps,
        shell: &Shell,
    ) -> bool {
        self.poll_at = None;
        let Some(hwnd) = windows.hwnd(self.owner) else {
            return true;
        };
        let attaching = matches!(self.phase, Phase::Moving { attaching: true });
        let pt = cursor_screen_pos();
        let pressed = primary_mouse_button_down();
        if let Some(target) = strip_window(windows, file_ops, pt, Some(self.owner)) {
            self.attach(target, pt, windows, shell);
            return !pressed && self.on_release(windows);
        }
        if pressed && attaching {
            self.run_move_loop(pt, windows);
            windows.mark_dirty(self.owner);
            false
        } else if pressed {
            self.cancel(windows, shell);
            true
        } else {
            windows.mark_dirty(self.owner);
            hwnd.set_foreground();
            true
        }
    }

    /// Moves the tab into window `target`'s strip, under the pointer at `pt`, and drags it on
    /// there.
    fn attach(&mut self, target: WindowId, pt: ScreenPoint, windows: &mut Windows, shell: &Shell) {
        let Some(target_hwnd) = windows.hwnd(target) else {
            return;
        };
        let prev_owner = self.owner;
        let owner = windows.get_mut(prev_owner);
        let Some(data) = owner.and_then(|w| w.take_tab(self.tab, shell)) else {
            return;
        };
        if let Some(win) = windows.get_mut(target) {
            let x = win.hwnd.to_client(pt).x - HEADER_WIDTH;
            let ix = win.tabs.insert_dragged(data, x, self.grab.x);
            win.activate_tab(ix, shell);
        }
        self.owner = target;
        self.phase = Phase::Dragging;
        self.poll_at = None;
        target_hwnd.set_capture();
        windows.mark_dirty(target);
        windows.mark_dirty(prev_owner);
    }

    /// Tears the tab out into a window of its own under the pointer (or, a lone tab, takes its
    /// window along), and hands that window to Windows' move loop.
    fn detach(&mut self, alone: bool, pt: ScreenPoint, windows: &mut Windows, shell: &Shell) {
        if alone {
            let Some(win) = windows.get_mut(self.owner) else {
                return;
            };
            let hwnd = win.hwnd;
            if self.owner == self.source && self.source_placement.is_none() {
                self.source_placement = hwnd.saved_placement();
            }
            if hwnd.is_maximized() {
                hwnd.restore();
            }
            win.dirty |= win.tabs.release_drag();
        } else {
            let from = self.owner;
            let taken = windows.get_mut(from).and_then(|w| {
                let size = w.win.window.restored_size();
                w.take_tab(self.tab, shell).map(|data| (data, size))
            });
            let Some((data, size)) = taken else {
                return;
            };
            match windows.open_at(Some(data), Some(size), Some(pt), shell) {
                Ok((window, _)) => {
                    windows.mark_dirty(from);
                    self.owner = window;
                }
                Err(err) => {
                    if let Some(data) = err.tab
                        && let Some(win) = windows.get_mut(from)
                    {
                        win.restore_tab(data, self.source_index, shell);
                    }
                    return;
                }
            }
        }
        self.run_move_loop(pt, windows);
    }

    /// Puts the owner under the pointer and hands it to Windows' move loop.
    fn run_move_loop(&mut self, pt: ScreenPoint, windows: &Windows) {
        let Some(win) = windows.get(self.owner) else {
            return;
        };
        win.place_under(pt, self.grab);
        win.hwnd.begin_system_move(pt);
        self.phase = Phase::Entering {
            since: Instant::now(),
        };
        self.poll_at = Some(Instant::now() + MOVE_POLL);
    }

    #[cfg(test)]
    pub(crate) fn detach_for_test(&mut self, windows: &mut Windows, shell: &Shell) {
        self.phase = Phase::Dragging;
        let pt = ScreenPoint {
            x: self.press.x + 120,
            y: self.press.y + 200,
        };
        self.detach(false, pt, windows, shell);
        windows.mark_dirty(self.owner);
    }
}

/// The window whose tab strip is under `pt` (other than `exclude`), if it may take a tab now.
fn strip_window(
    windows: &Windows,
    file_ops: &FileOps,
    pt: ScreenPoint,
    exclude: Option<WindowId>,
) -> Option<WindowId> {
    let hwnd = top_visible_window_at(pt, exclude.and_then(|id| windows.hwnd(id)))?;
    let target = windows.iter().find(|w| w.hwnd == hwnd)?;
    let accepts = target.accepts_tab_drop()
        && !file_ops.has_active_workflow_for(target.id)
        && target.strip_hit(pt, DETACH_MAGNETISM);
    accepts.then_some(target.id)
}
