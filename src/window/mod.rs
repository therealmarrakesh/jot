mod chrome;
mod commands;
mod dialogs;
mod find;
mod input;
mod menu;
mod paint;
mod tabs;

use crate::dialog::Dialogs;
use crate::editor::{EditCommand, Editor};
use crate::find::FindBar;
use crate::geom::{Edges, Point, ScreenPoint, Size, point};
use crate::gfx::WindowSurface;
use crate::ids::{AppUi, CaptionHit, EditorSlot, Focus, HitId, TabHit, TabId};
use crate::keytips::KeyTip;
use crate::menu::Menus;
use crate::platform::{WindowAppearance, WindowHandle, WindowId, clipboard};
use crate::shell::{Shell, SlotTimers, WindowTimer};
use crate::tabs::{self as tab_strip, TabData, TabSet};
use crate::tooltip::TooltipProbe;
use crate::ui::OverlayAnchor;

use chrome::{CAPTION_WIDTH, HEADER_WIDTH, STRIP_TOP};

pub(crate) const WINDOW_WIDTH: f32 = 2213.;
pub(crate) const WINDOW_HEIGHT: f32 = 1107.;
pub(crate) const WINDOW_MAX_SCREEN_SHARE: f32 = 0.9;

pub(super) const EDITOR_FONT_SIZE: f32 = 11. * 96. / 72.;
pub(super) const EDITOR_LINE_HEIGHT: f32 = 20.;
const EDITOR_PADDING_LEFT: f32 = 18.;
const EDITOR_PADDING_RIGHT: f32 = 12.;
const EDITOR_PADDING_TOP: f32 = 11.;
const EDITOR_PADDING_BOTTOM: f32 = 11.;
pub(crate) const EDITOR_PADDINGS: Edges<f32> = Edges {
    top: EDITOR_PADDING_TOP,
    right: EDITOR_PADDING_RIGHT,
    bottom: EDITOR_PADDING_BOTTOM,
    left: EDITOR_PADDING_LEFT,
};
const CARET_HEIGHT: f32 = 18.;
pub(super) const ZOOM_STEP: f32 = 0.1;
pub(super) const ZOOM_MIN: f32 = 0.1;
pub(super) const ZOOM_MAX: f32 = 5.;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum InputMode {
    Dialog,
    Popup,
    MenuBar,
    Normal,
}

pub(crate) struct MainWindow {
    pub(super) win: WindowSurface,
    pub(super) ui: AppUi,
    pub(super) tabs: TabSet,
    pub(super) menus: Menus,
    pub(super) find: FindBar,
    pub(super) dialogs: Dialogs,
    pub(super) focus: Focus,
    pub(super) dirty: bool,
    has_painted: bool,
    pub(super) pending_close: bool,
    middle_pressed_tab: Option<TabId>,
    min_size: Option<Size>,
}

impl MainWindow {
    pub(crate) fn new(win: WindowSurface, initial: Option<TabData>, shell: &Shell) -> Self {
        let data =
            initial.unwrap_or_else(|| TabData::create(None, shell.view.word_wrap, EDITOR_PADDINGS));
        let tabs = TabSet::new(data);
        let active_id = tabs.active_id();

        let mut window = Self {
            win,
            ui: AppUi::default(),
            tabs,
            menus: Menus::default(),
            find: FindBar::new(),
            dialogs: Dialogs::new(),
            focus: Focus::Editor(EditorSlot::Tab(active_id)),
            dirty: true,
            has_painted: false,
            pending_close: false,
            middle_pressed_tab: None,
            min_size: None,
        };
        window.activate_tab(0, shell);
        window
    }

    #[inline]
    pub(crate) fn is_dirty(&self) -> bool {
        self.dirty
    }

    #[inline]
    pub(crate) fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    pub(crate) fn on_paint_requested(&mut self) {
        if !self.has_painted {
            self.dirty = true;
        }
    }

    #[inline]
    pub(crate) fn is_pending_close(&self) -> bool {
        self.pending_close
    }

    #[inline]
    pub(crate) fn request_close(&mut self) {
        self.pending_close = true;
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

    #[inline]
    pub(crate) fn id(&self) -> WindowId {
        self.win.window.id
    }

    #[inline]
    pub(crate) fn hwnd(&self) -> WindowHandle {
        self.win.window.hwnd()
    }

    #[inline]
    pub(crate) fn scale_factor(&self) -> f32 {
        self.win.window.scale_factor()
    }

    pub(crate) fn restored_logical_size(&self) -> Size {
        if self.win.window.is_maximized() {
            self.win
                .window
                .hwnd()
                .restored_logical_size()
                .unwrap_or_else(|| self.win.window.logical_size())
        } else {
            self.win.window.logical_size()
        }
    }

    #[inline]
    pub(crate) fn tabs(&self) -> &TabSet {
        &self.tabs
    }

    #[inline]
    pub(crate) fn has_pointer(&self) -> bool {
        self.ui.pointer().is_some()
    }

    pub(crate) fn on_resized(
        &mut self,
        physical_width: u32,
        physical_height: u32,
        scale: f32,
        shell: &Shell,
    ) {
        self.win.surface.resize(physical_width, physical_height, scale);
        self.cancel_menu(shell);
    }

    pub(crate) fn apply_theme(&mut self, appearance: WindowAppearance) {
        self.win.window.set_appearance(appearance);
        self.dirty = true;
    }

    pub(crate) fn accepts_tab_drop(&self) -> bool {
        !self.dialogs.is_open() && !self.pending_close && self.win.window.hwnd().is_enabled()
    }

    pub(crate) fn screen_to_local(&self, pt: ScreenPoint) -> Point {
        let hwnd = self.win.window.hwnd();
        let origin = hwnd.client_origin();
        let scale = hwnd.dpi_scale();
        point(
            (pt.x - origin.x) as f32 / scale,
            (pt.y - origin.y) as f32 / scale,
        )
    }

    pub(crate) fn local_to_screen(&self, pos: Point) -> ScreenPoint {
        let hwnd = self.win.window.hwnd();
        let origin = hwnd.client_origin();
        let scale = hwnd.dpi_scale();
        ScreenPoint {
            x: origin.x + (pos.x * scale).round() as i32,
            y: origin.y + (pos.y * scale).round() as i32,
        }
    }

    pub(crate) fn strip_hit(&self, pt: ScreenPoint, magnetism: f32) -> bool {
        let pos = self.screen_to_local(pt);
        let right = self
            .ui
            .frame()
            .rect_of(HitId::Caption(CaptionHit::Min))
            .map_or_else(
                || self.win.window.logical_size().width - CAPTION_WIDTH,
                |r| r.left(),
            );
        let (left, top, bottom) = self.ui.frame().rect_of(HitId::Tab(TabHit::List)).map_or(
            (
                HEADER_WIDTH + tab_strip::LIST_PAD,
                STRIP_TOP,
                STRIP_TOP + tab_strip::TAB_HEIGHT,
            ),
            |r| (r.left(), r.top(), r.bottom()),
        );
        (left..right).contains(&pos.x) && (top - magnetism..bottom + magnetism).contains(&pos.y)
    }

    pub(crate) fn strip_grab_offset(&self, grab: Point) -> Point {
        point(HEADER_WIDTH + tab_strip::LIST_PAD + grab.x, STRIP_TOP + grab.y)
    }

    fn overlay_anchor(&self) -> OverlayAnchor {
        OverlayAnchor {
            window: self.id(),
            origin: self.win.window.hwnd().client_origin(),
            scale: self.win.window.scale_factor(),
        }
    }

    pub(crate) fn key_tip_request(&self, shell: &Shell) -> (OverlayAnchor, Vec<KeyTip>) {
        (
            self.overlay_anchor(),
            self.menus
                .key_tips(self.focus, self.ui.frame(), &shell.text_system),
        )
    }

    pub(crate) fn tooltip_probe(&self) -> TooltipProbe<'_> {
        TooltipProbe {
            anchor: self.overlay_anchor(),
            pointer: self.ui.pointer(),
            frame: self.ui.frame(),
        }
    }

    pub(super) fn sync_cursor(&self, pos: Point) {
        self.win.window.set_cursor(self.ui.frame().cursor_at(pos));
    }

    #[cfg(test)]
    pub(crate) fn min_size(&self) -> Option<Size> {
        self.min_size
    }

    #[cfg(test)]
    pub(crate) fn frame(&self) -> &crate::ids::AppFrame {
        self.ui.frame()
    }

    #[cfg(test)]
    pub(crate) fn ui_state(&self) -> &AppUi {
        &self.ui
    }

    #[cfg(test)]
    pub(crate) fn focus(&self) -> Focus {
        self.focus
    }

    #[cfg(test)]
    pub(crate) fn menus(&self) -> &Menus {
        &self.menus
    }

    #[cfg(test)]
    pub(crate) fn is_find_open(&self) -> bool {
        self.find.is_open()
    }

    #[cfg(test)]
    pub(crate) fn find_match_case(&self) -> bool {
        self.find.match_case()
    }

    #[cfg(test)]
    pub(crate) fn is_dialog_open(&self) -> bool {
        self.dialogs.is_open()
    }

    #[cfg(test)]
    pub(crate) fn surface_is_ready(&self) -> bool {
        self.win.surface.is_ready()
    }

    #[cfg(test)]
    pub(crate) fn mark_device_lost_for_test(&mut self) {
        self.win.surface.mark_device_lost_for_test();
    }

    #[inline]
    pub(crate) fn active_tab_id(&self) -> TabId {
        self.tabs.active_id()
    }

    #[inline]
    pub(crate) fn active_editor(&self) -> &Editor {
        &self.tabs.active().editor
    }

    #[inline]
    pub(crate) fn active_editor_mut(&mut self) -> &mut Editor {
        &mut self.tabs.active_mut().editor
    }

    #[inline]
    pub(crate) fn active_slot(&self) -> EditorSlot {
        self.tabs.active_slot()
    }

    pub(crate) fn editor_by_slot(&self, slot: EditorSlot) -> Option<&Editor> {
        match slot {
            EditorSlot::Tab(id) => self.tabs.editor(id),
            EditorSlot::FindQuery => Some(&self.find.query),
            EditorSlot::FindReplace => Some(&self.find.replacement),
            EditorSlot::GoToLine => Some(&self.dialogs.line),
        }
    }

    pub(crate) fn editor_by_slot_mut(&mut self, slot: EditorSlot) -> Option<&mut Editor> {
        match slot {
            EditorSlot::Tab(id) => self.tabs.editor_mut(id),
            EditorSlot::FindQuery => Some(&mut self.find.query),
            EditorSlot::FindReplace => Some(&mut self.find.replacement),
            EditorSlot::GoToLine => Some(&mut self.dialogs.line),
        }
    }

    pub(crate) fn has_text(&self) -> bool {
        self.active_editor().model().text().len() > 0
    }

    pub(super) fn run_edit_command(
        &mut self,
        slot: EditorSlot,
        cmd: EditCommand,
        shell: &Shell,
    ) -> bool {
        match cmd {
            EditCommand::Copy => {
                if let Some(text) = self.editor_by_slot(slot).and_then(Editor::selected_string) {
                    clipboard::write_clipboard_text(&text);
                }
                true
            }
            EditCommand::Cut => {
                let text = self
                    .editor_by_slot_mut(slot)
                    .and_then(Editor::prepare_cut);
                if let Some(text) = text
                    && clipboard::write_clipboard_text(&text)
                {
                    self.with_editor(slot, shell, |ed, timers| {
                        ed.delete_selection(&shell.text_system, timers);
                    });
                }
                true
            }
            EditCommand::Paste => {
                if let Some(raw) = clipboard::read_clipboard_text().filter(|s| !s.is_empty()) {
                    self.with_editor(slot, shell, |ed, timers| {
                        ed.paste(&raw, &shell.text_system, timers);
                    });
                }
                true
            }
            other => self
                .with_editor(slot, shell, |ed, timers| {
                    ed.handle_command(other, &shell.text_system, timers)
                })
                .unwrap_or(false),
        }
    }

    pub(crate) fn focused_editor_slot(&self) -> Option<EditorSlot> {
        match self.focus {
            Focus::Editor(slot) => Some(slot),
            Focus::MenuBar | Focus::DialogButton(_) => None,
        }
    }

    pub(crate) fn set_focus(&mut self, next: Focus, shell: &Shell) {
        if self.focus == next {
            if let Focus::Editor(slot) = next {
                self.start_editor_blink(slot, shell);
            }
            return;
        }
        if let Focus::Editor(old_slot) = self.focus {
            self.win.window.hwnd().complete_ime_composition();
            if let Some(ed) = self.editor_by_slot_mut(old_slot) {
                ed.on_blur();
            }
        }
        self.focus = next;
        if let Focus::Editor(new_slot) = next {
            self.start_editor_blink(new_slot, shell);
        }
        self.dirty = true;
    }

    pub(super) fn with_editor<R>(
        &mut self,
        slot: EditorSlot,
        shell: &Shell,
        f: impl FnOnce(&mut Editor, &mut SlotTimers<'_>) -> R,
    ) -> Option<R> {
        let wid = self.id();
        let ed = self.editor_by_slot_mut(slot)?;
        let mut timers = shell.window_timers(wid).for_editor(slot);
        Some(f(ed, &mut timers))
    }

    fn start_editor_blink(&mut self, slot: EditorSlot, shell: &Shell) {
        self.with_editor(slot, shell, |ed, timers| ed.start_blink(timers));
    }

    fn stop_all_editor_blinks(&mut self) {
        self.tabs.for_each_editor_mut(Editor::stop_blink);
        self.find.query.stop_blink();
        self.find.replacement.stop_blink();
        self.dialogs.line.stop_blink();
    }

    pub(crate) fn focus_active_editor(&mut self, shell: &Shell) {
        self.set_focus(Focus::Editor(self.active_slot()), shell);
    }

    pub(super) fn line_height(&self) -> f32 {
        (EDITOR_LINE_HEIGHT * self.zoom()).round()
    }

    pub(super) fn caret_height(&self) -> f32 {
        (CARET_HEIGHT * self.zoom()).round()
    }

    pub(super) fn editor_min_height(&self) -> f32 {
        EDITOR_PADDING_TOP + EDITOR_PADDING_BOTTOM + self.line_height()
    }

    pub(crate) fn on_activated(&mut self, active: bool, shell: &Shell) {
        if !active {
            self.win.window.hwnd().complete_ime_composition();
            if let Some(slot) = self.focused_editor_slot()
                && let Some(ed) = self.editor_by_slot_mut(slot)
            {
                ed.on_blur();
            }
            self.cancel_menu(shell);
            self.stop_all_editor_blinks();
            self.win.window.set_text_caret(None, false);
        } else if let Some(slot) = self.focused_editor_slot() {
            self.start_editor_blink(slot, shell);
        }
    }

    pub(crate) fn on_timer(&mut self, timer: WindowTimer, shell: &Shell) {
        match timer {
            WindowTimer::Editor { slot, kind } => {
                self.dirty |= self
                    .with_editor(slot, shell, |ed, timers| ed.on_timer(kind, timers))
                    .unwrap_or(false);
            }
            WindowTimer::TabScrollRepeat(step) => {
                if !self.scroll_tabs(step) {
                    self.tabs.stop_scroll_repeat();
                }
            }
            WindowTimer::Submenu(target) => {
                if self.menus.on_submenu_timer(target) {
                    self.dirty = true;
                }
            }
            WindowTimer::FindNotice => {
                if self.find.on_notice_dismiss() {
                    self.dirty = true;
                }
            }
        }
    }
}
