mod chrome;
mod commands;
mod input;
mod paint;

use std::time::Duration;

use crate::APP_NAME;
use crate::dialog::{DialogKind, Dialogs};
use crate::document::{self, DocPath, Format};
use crate::editor::{EditCommand, Editor, EditorTimer, EditorTimers, Rope, RopeExt as _};
use crate::files::Prompt;
use crate::find::{self, FindBar, FindOutcome};
use crate::geom::{Edges, Point, ScreenPoint, Size, point};
use crate::gfx::Surface;
use crate::ids::{EditorSlot, Focus, HitId, TabId, UiState};
use crate::keytips::KeyTip;
use crate::menu::{FocusRequest, MenuBar, MenuOutcome, MenuState, SubmenuTimer};
use crate::platform::{
    NativeWindow, TimerHandle, WindowAppearance, WindowHwnd, WindowId, clipboard,
};
use crate::shell::{AppEvent, Cx, Effect, Shell};
use crate::tabs::{self, Removal, TabData, TabSet};
use crate::tooltip::TooltipProbe;

use chrome::{CAPTION_WIDTH, HEADER_WIDTH, MinSizeCache, STRIP_TOP};

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
const ZOOM_MIN: f32 = 0.1;
const ZOOM_MAX: f32 = 5.;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum InputMode {
    Dialog,
    Popup,
    MenuBar,
    Normal,
}

pub(crate) struct WindowEditorTimers<'a> {
    shell: &'a Shell,
    window: WindowId,
    slot: EditorSlot,
}

impl<'a> WindowEditorTimers<'a> {
    pub(crate) fn new(shell: &'a Shell, window: WindowId, slot: EditorSlot) -> Self {
        Self {
            shell,
            window,
            slot,
        }
    }
}

impl EditorTimers for WindowEditorTimers<'_> {
    fn after(&mut self, delay: Duration, kind: EditorTimer) -> TimerHandle {
        self.shell.jobs.after(
            delay,
            AppEvent::EditorTimer {
                window: self.window,
                slot: self.slot,
                kind,
            },
        )
    }

    fn every(&mut self, delay: Duration, interval: Duration, kind: EditorTimer) -> TimerHandle {
        let window = self.window;
        let slot = self.slot;
        self.shell
            .jobs
            .every(delay, interval, move || AppEvent::EditorTimer {
                window,
                slot,
                kind,
            })
    }

    fn blink_timing(&self) -> (Option<Duration>, Option<Duration>) {
        (
            self.shell.settings.caret_blink_interval,
            self.shell.settings.caret_blink_timeout,
        )
    }
}

pub(crate) struct MainWindow {
    // Declared before native so swapchain/DComp target drop before DestroyWindow.
    pub(super) surface: Surface,
    pub(super) native: NativeWindow,
    pub(super) ui: UiState,
    pub(super) tabs: TabSet,
    pub(super) menu_bar: MenuBar,
    pub(super) find: FindBar,
    pub(super) dialogs: Dialogs,
    pub(super) focus: Focus,
    dirty: bool,
    has_painted: bool,
    pending_close: bool,
    menu_submenu_timer: TimerHandle,
    middle_pressed_tab: Option<TabId>,
    min_size_cache: Option<MinSizeCache>,
}

impl MainWindow {
    pub(crate) fn new(
        native: NativeWindow,
        surface: Surface,
        initial: Option<TabData>,
        shell: &Shell,
    ) -> Self {
        let data =
            initial.unwrap_or_else(|| TabData::create(None, shell.view.word_wrap, EDITOR_PADDINGS));
        let tabs = TabSet::new(data);
        let active_id = tabs.active_id();

        let mut win = Self {
            surface,
            native,
            ui: UiState::default(),
            tabs,
            menu_bar: MenuBar::default(),
            find: FindBar::new(),
            dialogs: Dialogs::new(),
            focus: Focus::Editor(EditorSlot::Tab(active_id)),
            dirty: true,
            has_painted: false,
            pending_close: false,
            menu_submenu_timer: TimerHandle::default(),
            middle_pressed_tab: None,
            min_size_cache: None,
        };
        win.activate_tab(0, shell);
        win
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
        } else if self.menu_bar.is_open() {
            InputMode::Popup
        } else if self.focus == Focus::MenuBar {
            InputMode::MenuBar
        } else {
            InputMode::Normal
        }
    }

    #[inline]
    pub(crate) fn id(&self) -> WindowId {
        self.native.id
    }

    #[inline]
    pub(crate) fn hwnd(&self) -> WindowHwnd {
        self.native.hwnd()
    }

    #[inline]
    pub(crate) fn scale_factor(&self) -> f32 {
        self.native.scale_factor()
    }

    pub(crate) fn restored_logical_size(&self) -> Size {
        if self.native.is_maximized() {
            self.native
                .hwnd()
                .restored_logical_size()
                .unwrap_or_else(|| self.native.logical_size())
        } else {
            self.native.logical_size()
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
        self.surface.resize(physical_width, physical_height, scale);
        self.cancel_menu(shell);
    }

    pub(crate) fn apply_theme(&mut self, appearance: WindowAppearance) {
        self.native.set_appearance(appearance);
        self.dirty = true;
    }

    pub(crate) fn set_block_shutdown(&self, reason: Option<&'static str>) {
        self.native.set_block_shutdown(reason);
    }

    pub(crate) fn accepts_tab_drop(&self) -> bool {
        !self.dialogs.is_open() && !self.pending_close && self.native.hwnd().is_enabled()
    }

    pub(crate) fn screen_to_local(&self, pt: ScreenPoint) -> Point {
        let hwnd = self.native.hwnd();
        let origin = hwnd.client_origin();
        let scale = hwnd.dpi_scale();
        point(
            (pt.x - origin.x) as f32 / scale,
            (pt.y - origin.y) as f32 / scale,
        )
    }

    pub(crate) fn local_to_screen(&self, pos: Point) -> ScreenPoint {
        let hwnd = self.native.hwnd();
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
            .rect_of(HitId::CaptionMin)
            .map_or_else(|| self.native.logical_size().width - CAPTION_WIDTH, |r| r.left());
        let (left, top, bottom) = self.ui.frame().rect_of(HitId::TabList).map_or(
            (
                HEADER_WIDTH + tabs::LIST_PAD,
                STRIP_TOP,
                STRIP_TOP + tabs::TAB_HEIGHT,
            ),
            |r| (r.left(), r.top(), r.bottom()),
        );
        (left..right).contains(&pos.x) && (top - magnetism..bottom + magnetism).contains(&pos.y)
    }

    pub(crate) fn strip_grab_offset(&self, grab: Point) -> Point {
        point(HEADER_WIDTH + tabs::LIST_PAD + grab.x, STRIP_TOP + grab.y)
    }

    pub(crate) fn key_tip_request(&self, shell: &Shell) -> (ScreenPoint, f32, Vec<KeyTip>) {
        (
            self.native.hwnd().client_origin(),
            self.native.scale_factor(),
            self.menu_bar
                .key_tips(self.focus, self.ui.frame(), &shell.text_system),
        )
    }

    pub(crate) fn begin_tab_save(&mut self, tab: TabId) -> Option<(u64, Rope)> {
        let ed = self.tabs.editor_mut(tab)?;
        let snapshot = ed.begin_save();
        let text = ed.model().text().clone();
        Some((snapshot, text))
    }

    pub(crate) fn tooltip_probe(&self) -> TooltipProbe<'_> {
        TooltipProbe {
            window: self.id(),
            pointer: self.ui.pointer(),
            origin: self.native.hwnd().client_origin(),
            scale: self.native.scale_factor(),
            frame: self.ui.frame(),
        }
    }

    pub(super) fn sync_cursor(&self, pos: Point) {
        self.native.set_cursor(self.ui.frame().cursor_at(pos));
    }

    #[cfg(test)]
    pub(crate) fn min_size_cached(&self) -> Option<Size> {
        self.min_size_cache.map(|c| c.size)
    }

    #[cfg(test)]
    pub(crate) fn frame(&self) -> &crate::ids::Frame {
        self.ui.frame()
    }

    #[cfg(test)]
    pub(crate) fn ui_state(&self) -> &UiState {
        &self.ui
    }

    #[cfg(test)]
    pub(crate) fn focus(&self) -> Focus {
        self.focus
    }

    #[cfg(test)]
    pub(crate) fn is_menu_open(&self) -> bool {
        self.menu_bar.is_open()
    }

    #[cfg(test)]
    pub(crate) fn open_menu_kind(&self) -> Option<crate::ids::MenuKind> {
        self.menu_bar.open_kind()
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
    pub(crate) fn is_active(&self) -> bool {
        self.native.is_active()
    }

    #[cfg(test)]
    pub(crate) fn in_size_move(&self) -> bool {
        self.native.in_size_move()
    }

    #[cfg(test)]
    pub(crate) fn surface_is_ready(&self) -> bool {
        self.surface.is_ready()
    }

    #[cfg(test)]
    pub(crate) fn mark_device_lost_for_test(&mut self) {
        self.surface.mark_device_lost_for_test();
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
            self.native.hwnd().complete_ime_composition();
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
        f: impl FnOnce(&mut Editor, &mut WindowEditorTimers<'_>) -> R,
    ) -> Option<R> {
        let wid = self.id();
        let ed = self.editor_by_slot_mut(slot)?;
        let mut timers = WindowEditorTimers::new(shell, wid, slot);
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

    pub(crate) fn refresh_title(&self) {
        self.native.set_title(&format!(
            "{} - {APP_NAME}",
            self.tabs.tab_name(self.tabs.active_index())
        ));
    }

    pub(crate) fn zoom(&self) -> f32 {
        self.tabs.active().zoom
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
            self.native.hwnd().complete_ime_composition();
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
                .rect_of(HitId::TabStrip)
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

    pub(crate) fn on_tab_scroll_repeat(&mut self, step: f32) {
        if !self.scroll_tabs(step) {
            self.tabs.stop_scroll_repeat();
        }
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

    pub(super) fn press_dialog_button(&mut self, ix: usize, cx: &mut Cx<'_>) {
        let Some(kind) = self.dialogs.take_active() else {
            return;
        };
        match kind {
            DialogKind::GoTo if ix == 0 => self.submit_go_to(cx.shell),
            DialogKind::GoTo => self.close_dialogs(cx.shell),
            DialogKind::LineOutOfRange => self.show_go_to(None, cx.shell),
            DialogKind::Prompt(prompt) => {
                self.close_dialogs(cx.shell);
                if let Some(id) = prompt.id {
                    cx.emit(Effect::PromptResponse { id, button: ix });
                }
            }
        }
    }

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

    fn apply_find_outcome(&mut self, out: FindOutcome, shell: &Shell) {
        if out.arm_notice {
            let window = self.id();
            self.find.set_notice_timer(
                shell
                    .jobs
                    .after(find::NOTICE_DURATION, AppEvent::FindNoticeDismiss { window }),
            );
        }
    }

    pub(super) fn find_step(&mut self, forward: bool, shell: &Shell) {
        let out = self.find.step(&mut self.tabs.active_mut().editor, forward);
        self.apply_find_outcome(out, shell);
        if !out.searched {
            self.open_find(false, shell);
            return;
        }
        if !self.find.is_open() {
            self.focus_active_editor(shell);
        }
        self.dirty = true;
    }

    pub(super) fn replace_next(&mut self, shell: &Shell) {
        let out = self
            .find
            .replace_next(&mut self.tabs.active_mut().editor, &shell.text_system);
        self.apply_find_outcome(out, shell);
        self.dirty = true;
    }

    pub(super) fn replace_all(&mut self, shell: &Shell) {
        let out = self
            .find
            .replace_all(&mut self.tabs.active_mut().editor, &shell.text_system);
        self.apply_find_outcome(out, shell);
        if out.replaced {
            self.refocus_find(shell);
        } else {
            self.dirty = true;
        }
    }

    pub(crate) fn on_find_notice_dismiss(&mut self) {
        if self.find.on_notice_dismiss() {
            self.dirty = true;
        }
    }

    pub(crate) fn key_tips_active(&self) -> bool {
        self.menu_bar.access_active(self.focus)
    }

    pub(super) fn menu_state(&self, shell: &Shell) -> MenuState {
        let m = self.active_editor().model();
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

    pub(crate) fn cancel_menu(&mut self, shell: &Shell) {
        let out = self.menu_bar.cancel(self.focus);
        self.dirty |= out.dirty;
        if matches!(out.submenu_timer, SubmenuTimer::Cancel) {
            self.menu_submenu_timer.cancel();
        }
        self.apply_focus_request(out.focus, shell);
    }

    pub(crate) fn on_menu_submenu_timer(&mut self, target: Option<usize>, shell: &Shell) {
        if self.menu_bar.on_submenu_timer(target, &shell.text_system) {
            self.dirty = true;
        }
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
        match out.submenu_timer {
            SubmenuTimer::Keep => {}
            SubmenuTimer::Cancel => self.menu_submenu_timer.cancel(),
            SubmenuTimer::Arm(target) => {
                let delay = Duration::from_millis(u64::from(cx.shell.settings.menu_show_delay_ms));
                let window = self.id();
                self.menu_submenu_timer = cx
                    .jobs()
                    .after(delay, AppEvent::MenuSubmenuTimer { window, target });
            }
        }
        self.apply_focus_request(out.focus, cx.shell);
        if let Some(cmd) = out.command {
            self.dispatch_command(cmd, cx);
            return true;
        }
        out.handled
    }

    pub(crate) fn on_activated(&mut self, active: bool, shell: &Shell) {
        if !active {
            self.native.hwnd().complete_ime_composition();
            if let Some(slot) = self.focused_editor_slot()
                && let Some(ed) = self.editor_by_slot_mut(slot)
            {
                ed.on_blur();
            }
            self.cancel_menu(shell);
            self.stop_all_editor_blinks();
            self.native.set_text_caret(None, false);
        } else if let Some(slot) = self.focused_editor_slot() {
            self.start_editor_blink(slot, shell);
        }
    }

    pub(crate) fn on_editor_timer(&mut self, slot: EditorSlot, kind: EditorTimer, shell: &Shell) {
        self.dirty |= self
            .with_editor(slot, shell, |ed, timers| ed.on_timer(kind, timers))
            .unwrap_or(false);
    }
}
