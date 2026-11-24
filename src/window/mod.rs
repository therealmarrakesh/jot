mod chrome;
mod commands;
mod input;
mod paint;
mod tab_set;

use std::time::Duration;

use crate::APP_NAME;
use crate::app::{AppEvent, Cx, Effect, ViewSettings};
use crate::dialog::{DialogKind, Dialogs, Prompt};
use crate::document::{self, DocPath, Format, PathKey};
use crate::editor::{Clipboard, Editor, EditorTimer, EditorTimers, RopeExt as _};
use crate::find::{self, FindBar, FindOutcome};
use crate::geom::{Edges, Point};
use crate::gfx::{Surface, TextSystem};
use crate::ids::{EditorSlot, Focus, HitId, UiState};
use crate::keytips::KeyTip;
use crate::menu::{FocusRequest, MenuBar, MenuOutcome, MenuState, SubmenuTimer};
use crate::platform::{Jobs, NativeWindow, SystemSettings, TimerHandle, WindowId, clipboard};
use crate::tabs::{TabData, TabId};
use crate::tooltip::TooltipProbe;

use chrome::MinSizeCache;
use tab_set::{RemoveOutcome, TakeOutcome};

pub(crate) use tab_set::TabSet;

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

pub(super) struct SystemClipboard;

impl Clipboard for SystemClipboard {
    fn read(&mut self) -> Option<String> {
        clipboard::read_clipboard_text()
    }

    fn write(&mut self, text: &str) -> bool {
        clipboard::write_clipboard_text(text)
    }
}

pub(crate) struct WindowEditorTimers<'a> {
    jobs: &'a Jobs<AppEvent>,
    window: WindowId,
    slot: EditorSlot,
    blink_timing: (Option<Duration>, Option<Duration>),
}

impl<'a> WindowEditorTimers<'a> {
    pub(crate) fn new(
        jobs: &'a Jobs<AppEvent>,
        window: WindowId,
        slot: EditorSlot,
        blink_timing: (Option<Duration>, Option<Duration>),
    ) -> Self {
        Self {
            jobs,
            window,
            slot,
            blink_timing,
        }
    }
}

impl EditorTimers for WindowEditorTimers<'_> {
    fn after(&mut self, delay: Duration, kind: EditorTimer) -> TimerHandle {
        self.jobs.after(
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
        self.jobs.every(delay, interval, move || AppEvent::EditorTimer {
            window,
            slot,
            kind,
        })
    }

    fn blink_timing(&self) -> (Option<Duration>, Option<Duration>) {
        self.blink_timing
    }
}

pub(crate) struct MainWindow {
    // Declared before native so swapchain/DComp target drop before DestroyWindow.
    pub(crate) surface: Surface,
    pub(crate) native: NativeWindow,
    pub(crate) ui: UiState,
    pub(crate) tabs: TabSet,
    pub(crate) menu_bar: MenuBar,
    pub(crate) find: FindBar,
    pub(crate) dialogs: Dialogs,
    pub(crate) focus: Focus,
    pub(crate) dirty: bool,
    pub(crate) has_painted: bool,
    pub(crate) pending_close: bool,
    blink_timing: (Option<Duration>, Option<Duration>),
    menu_submenu_timer: TimerHandle,
    middle_pressed_tab: Option<TabId>,
    min_size_cache: Option<MinSizeCache>,
}

impl MainWindow {
    pub(crate) fn new(
        native: NativeWindow,
        surface: Surface,
        initial: Option<TabData>,
        word_wrap: bool,
        settings: SystemSettings,
        jobs: &Jobs<AppEvent>,
    ) -> Self {
        let data = initial.unwrap_or_else(|| TabData::create(None, word_wrap, EDITOR_PADDINGS));
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
            blink_timing: (settings.caret_blink_interval, settings.caret_blink_timeout),
            menu_submenu_timer: TimerHandle::default(),
            middle_pressed_tab: None,
            min_size_cache: None,
        };
        win.activate_tab(0, jobs);
        win
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

    pub(crate) fn set_blink_timing(
        &mut self,
        interval: Option<Duration>,
        timeout: Option<Duration>,
    ) {
        self.blink_timing = (interval, timeout);
    }

    #[inline]
    pub(crate) fn id(&self) -> WindowId {
        self.native.id
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
    pub(crate) fn min_size_cached(&self) -> Option<crate::geom::Size> {
        self.min_size_cache.map(|c| c.size)
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

    pub(crate) fn tab_index(&self, id: TabId) -> Option<usize> {
        self.tabs.index_of(id)
    }

    pub(crate) fn tab_name(&self, ix: usize) -> String {
        self.tabs.tab_name(ix)
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
        self.active_editor().text().len() > 0
    }

    pub(crate) fn focused_editor_slot(&self) -> Option<EditorSlot> {
        match self.focus {
            Focus::Editor(slot) => Some(slot),
            Focus::MenuBar | Focus::DialogButton(_) => None,
        }
    }

    pub(crate) fn set_focus(&mut self, next: Focus, jobs: &Jobs<AppEvent>) {
        if self.focus == next {
            if let Focus::Editor(slot) = next {
                self.start_editor_blink(slot, jobs);
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
            self.start_editor_blink(new_slot, jobs);
        }
        self.dirty = true;
    }

    pub(super) fn with_editor<R>(
        &mut self,
        slot: EditorSlot,
        jobs: &Jobs<AppEvent>,
        f: impl FnOnce(&mut Editor, &mut WindowEditorTimers<'_>) -> R,
    ) -> Option<R> {
        let wid = self.id();
        let blink_timing = self.blink_timing;
        let ed = self.editor_by_slot_mut(slot)?;
        let mut timers = WindowEditorTimers::new(jobs, wid, slot, blink_timing);
        Some(f(ed, &mut timers))
    }

    fn start_editor_blink(&mut self, slot: EditorSlot, jobs: &Jobs<AppEvent>) {
        self.with_editor(slot, jobs, |ed, timers| ed.start_blink(timers));
    }

    fn stop_all_editor_blinks(&mut self) {
        self.tabs.for_each_editor_mut(Editor::stop_blink);
        self.find.query.stop_blink();
        self.find.replacement.stop_blink();
        self.dialogs.line.stop_blink();
    }

    pub(crate) fn focus_active_editor(&mut self, jobs: &Jobs<AppEvent>) {
        self.set_focus(Focus::Editor(self.active_slot()), jobs);
    }

    pub(crate) fn refresh_title(&self) {
        self.native.set_title(&format!(
            "{} - {APP_NAME}",
            self.tab_name(self.tabs.active_index())
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

    pub(crate) fn set_word_wrap(&mut self, word_wrap: bool, ts: &TextSystem) {
        self.tabs
            .for_each_editor_mut(|ed| ed.set_soft_wrap(word_wrap, ts));
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

    pub(crate) fn activate_tab(&mut self, ix: usize, jobs: &Jobs<AppEvent>) {
        self.select_tab_only(ix);
        if !matches!(self.mode(), InputMode::Dialog | InputMode::Popup) {
            self.focus_active_editor(jobs);
        }
    }

    pub(crate) fn add_tab(&mut self, word_wrap: bool, jobs: &Jobs<AppEvent>) {
        let data = TabData::create(None, word_wrap, EDITOR_PADDINGS);
        self.push_tab(data, jobs);
    }

    pub(crate) fn push_tab(&mut self, data: TabData, jobs: &Jobs<AppEvent>) {
        let ix = self.tabs.push(data);
        self.activate_tab(ix, jobs);
    }

    pub(crate) fn replace_tab(&mut self, ix: usize, data: TabData, jobs: &Jobs<AppEvent>) {
        self.tabs.replace(ix, data);
        self.activate_tab(ix, jobs);
    }

    pub(crate) fn remove_tab(&mut self, ix: usize, jobs: &Jobs<AppEvent>) {
        let pointer_in_strip = self.ui.pointer().is_some_and(|p| {
            self.ui
                .frame()
                .rect_of(HitId::TabStrip)
                .is_some_and(|r| r.contains(p.pos))
        });
        match self.tabs.remove(ix, pointer_in_strip) {
            RemoveOutcome::LastTab => self.pending_close = true,
            RemoveOutcome::Removed {
                id,
                closed_active,
                next_active,
            } => {
                self.dialogs.close_for_tab(id);
                self.finish_removal(closed_active, next_active, jobs);
            }
        }
    }

    fn finish_removal(&mut self, closed_active: bool, next_active: usize, jobs: &Jobs<AppEvent>) {
        let focus_in_find = matches!(
            self.focus,
            Focus::Editor(EditorSlot::FindQuery | EditorSlot::FindReplace)
        );
        if closed_active || !focus_in_find {
            self.activate_tab(next_active, jobs);
        } else {
            self.select_tab_only(next_active);
        }
    }

    pub(crate) fn drag_tab(&mut self, id: TabId, x: f32, grab: f32, start: f32) {
        self.tabs.drag(id, x, grab, start);
        self.dirty = true;
    }

    pub(crate) fn release_tab(&mut self) {
        if self.tabs.release_drag() {
            self.dirty = true;
        }
    }

    pub(crate) fn take_tab(&mut self, id: TabId, jobs: &Jobs<AppEvent>) -> Option<TabData> {
        let (data, outcome) = self.tabs.take(id)?;
        match outcome {
            TakeOutcome::NowEmpty => self.pending_close = true,
            TakeOutcome::Settled {
                closed_active,
                next_active,
            } => self.finish_removal(closed_active, next_active, jobs),
        }
        Some(data)
    }

    pub(crate) fn insert_dragged_tab(
        &mut self,
        data: TabData,
        x: f32,
        grab: f32,
        jobs: &Jobs<AppEvent>,
    ) {
        let ix = self.tabs.insert_dragged(data, x, grab);
        self.activate_tab(ix, jobs);
    }

    pub(crate) fn restore_tab(&mut self, data: TabData, ix: usize, jobs: &Jobs<AppEvent>) {
        let ix = self.tabs.restore(data, ix);
        self.activate_tab(ix, jobs);
    }

    pub(crate) fn return_tab(&mut self, id: TabId, ix: usize, jobs: &Jobs<AppEvent>) {
        if let Some(ix) = self.tabs.return_tab(id, ix) {
            self.activate_tab(ix, jobs);
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

    pub(super) fn next_tab(&mut self, jobs: &Jobs<AppEvent>) {
        if self.dialogs.is_open() || self.tabs.len() <= 1 {
            return;
        }
        let next = (self.tabs.active_index() + 1) % self.tabs.len();
        self.activate_tab(next, jobs);
    }

    pub(super) fn prev_tab(&mut self, jobs: &Jobs<AppEvent>) {
        if self.dialogs.is_open() || self.tabs.len() <= 1 {
            return;
        }
        let prev = (self.tabs.active_index() + self.tabs.len() - 1) % self.tabs.len();
        self.activate_tab(prev, jobs);
    }

    pub(super) fn select_tab(&mut self, ix: usize, jobs: &Jobs<AppEvent>) {
        if self.dialogs.is_open() || ix >= self.tabs.len() || ix == self.tabs.active_index() {
            return;
        }
        self.activate_tab(ix, jobs);
    }

    pub(super) fn select_last_tab(&mut self, jobs: &Jobs<AppEvent>) {
        let last = self.tabs.len().saturating_sub(1);
        if self.dialogs.is_open() || last == self.tabs.active_index() {
            return;
        }
        self.activate_tab(last, jobs);
    }

    pub(crate) fn find_open_tab_by_key(&self, key: &PathKey) -> Option<usize> {
        self.tabs.find_by_key(key)
    }

    pub(crate) fn has_dirty_tabs(&self) -> bool {
        self.tabs.has_dirty()
    }

    pub(crate) fn dirty_tabs(&self) -> Vec<TabId> {
        self.tabs.dirty_tabs()
    }

    pub(crate) fn adopt_loaded_file(
        &mut self,
        loaded: document::Loaded,
        word_wrap: bool,
        jobs: &Jobs<AppEvent>,
    ) {
        let data = TabData::create(Some(loaded), word_wrap, EDITOR_PADDINGS);
        let active = self.tabs.active_index();
        if self.tabs.is_blank(active) {
            self.replace_tab(active, data, jobs);
        } else {
            self.push_tab(data, jobs);
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

    pub(crate) fn show_prompt(&mut self, prompt: Prompt, jobs: &Jobs<AppEvent>) {
        if let Some(next_focus) = self.dialogs.enqueue(DialogKind::Prompt(prompt)) {
            self.cancel_menu(jobs);
            self.set_focus(next_focus, jobs);
            self.dirty = true;
        }
    }

    fn show_dialog_now(&mut self, kind: DialogKind, jobs: &Jobs<AppEvent>) {
        self.cancel_menu(jobs);
        let next_focus = self.dialogs.show_now(kind);
        self.set_focus(next_focus, jobs);
        self.dirty = true;
    }

    fn close_dialogs(&mut self, jobs: &Jobs<AppEvent>) {
        if let Some(next_focus) = self.dialogs.dismiss() {
            self.cancel_menu(jobs);
            self.set_focus(next_focus, jobs);
        } else {
            self.focus_active_editor(jobs);
        }
        self.dirty = true;
    }

    pub(crate) fn open_go_to(&mut self, ts: &TextSystem, jobs: &Jobs<AppEvent>) {
        if self.dialogs.is_open() {
            return;
        }
        let ed = self.active_editor();
        let row = ed.text().offset_to_point(ed.cursor()).row;
        self.show_go_to(Some(row + 1), ts, jobs);
    }

    fn show_go_to(&mut self, line: Option<usize>, ts: &TextSystem, jobs: &Jobs<AppEvent>) {
        self.show_dialog_now(DialogKind::GoTo, jobs);
        if let Some(line) = line {
            self.dialogs.line.set_value(&line.to_string(), ts);
        }
        self.dialogs.line.select_all();
        self.set_focus(Focus::Editor(EditorSlot::GoToLine), jobs);
        self.dirty = true;
    }

    fn submit_go_to(&mut self, jobs: &Jobs<AppEvent>) {
        let value = self.dialogs.line.value();
        let lines = self.active_editor().text().lines_len();
        match value.parse::<usize>() {
            Ok(line) if (1..=lines).contains(&line) => {
                self.close_dialogs(jobs);
                self.active_editor_mut().go_to_line(line - 1);
                self.dirty = true;
            }
            _ => {
                self.show_dialog_now(DialogKind::LineOutOfRange, jobs);
            }
        }
    }

    pub(super) fn press_dialog_button(&mut self, ix: usize, cx: &mut Cx<'_>) {
        let Some(kind) = self.dialogs.take_active() else {
            return;
        };
        match kind {
            DialogKind::GoTo if ix == 0 => self.submit_go_to(cx.jobs),
            DialogKind::GoTo => self.close_dialogs(cx.jobs),
            DialogKind::LineOutOfRange => self.show_go_to(None, cx.ts, cx.jobs),
            DialogKind::Prompt(prompt) => {
                self.close_dialogs(cx.jobs);
                if let Some(id) = prompt.id {
                    cx.emit(Effect::PromptResponse { id, button: ix });
                }
            }
        }
    }

    pub(super) fn open_find(&mut self, replace: bool, ts: &TextSystem, jobs: &Jobs<AppEvent>) {
        let selected = self.active_editor().selected_text().to_string();
        self.find.open(replace, &selected, ts);
        self.set_focus(Focus::Editor(EditorSlot::FindQuery), jobs);
        self.dirty = true;
    }

    pub(super) fn close_find(&mut self, jobs: &Jobs<AppEvent>) {
        if self.find.close() {
            self.focus_active_editor(jobs);
            self.dirty = true;
        }
    }

    pub(super) fn refocus_find(&mut self, jobs: &Jobs<AppEvent>) {
        if self.find.is_open() {
            let slot = if self.find.replace && self.focus == Focus::Editor(EditorSlot::FindReplace)
            {
                EditorSlot::FindReplace
            } else {
                EditorSlot::FindQuery
            };
            self.set_focus(Focus::Editor(slot), jobs);
        }
        self.dirty = true;
    }

    fn apply_find_outcome(&mut self, out: FindOutcome, jobs: &Jobs<AppEvent>) {
        if out.arm_notice {
            let window = self.id();
            self.find.set_notice_timer(
                jobs.after(find::NOTICE_DURATION, AppEvent::FindNoticeDismiss { window }),
            );
        }
    }

    pub(super) fn find_step(&mut self, forward: bool, ts: &TextSystem, jobs: &Jobs<AppEvent>) {
        let out = self.find.step(&mut self.tabs.active_mut().editor, forward);
        self.apply_find_outcome(out, jobs);
        if !out.searched {
            self.open_find(false, ts, jobs);
            return;
        }
        if !self.find.is_open() {
            self.focus_active_editor(jobs);
        }
        self.dirty = true;
    }

    pub(super) fn replace_next(&mut self, ts: &TextSystem, jobs: &Jobs<AppEvent>) {
        let out = self
            .find
            .replace_next(&mut self.tabs.active_mut().editor, ts);
        self.apply_find_outcome(out, jobs);
        self.dirty = true;
    }

    pub(super) fn replace_all(&mut self, ts: &TextSystem, jobs: &Jobs<AppEvent>) {
        let out = self
            .find
            .replace_all(&mut self.tabs.active_mut().editor, ts);
        self.apply_find_outcome(out, jobs);
        if out.replaced {
            self.refocus_find(jobs);
        } else {
            self.dirty = true;
        }
    }

    pub(crate) fn on_find_notice_dismiss(&mut self) {
        if self.find.on_notice_dismiss() {
            self.dirty = true;
        }
    }

    pub(crate) fn key_tips(&self, ts: &TextSystem) -> Vec<KeyTip> {
        self.menu_bar.key_tips(self.ui.frame(), ts)
    }

    pub(super) fn menu_state(&self, view: ViewSettings) -> MenuState {
        let ed = self.active_editor();
        MenuState {
            can_undo: ed.can_undo(),
            can_redo: ed.can_redo(),
            has_selection: !ed.selected_range().is_empty(),
            has_text: ed.text().len() > 0,
            clipboard_has_text: clipboard::clipboard_has_text(),
            status_bar: view.status_bar,
            word_wrap: view.word_wrap,
        }
    }

    pub(crate) fn cancel_menu(&mut self, jobs: &Jobs<AppEvent>) {
        let out = self.menu_bar.cancel();
        self.dirty |= out.dirty;
        if matches!(out.submenu_timer, SubmenuTimer::Cancel) {
            self.menu_submenu_timer.cancel();
        }
        self.apply_focus_request(out.focus, jobs);
    }

    pub(crate) fn on_menu_submenu_timer(&mut self, target: Option<usize>, ts: &TextSystem) {
        if self.menu_bar.on_submenu_timer(target, ts) {
            self.dirty = true;
        }
    }

    fn apply_focus_request(&mut self, req: FocusRequest, jobs: &Jobs<AppEvent>) {
        match req {
            FocusRequest::Keep => {}
            FocusRequest::MenuBar => self.set_focus(Focus::MenuBar, jobs),
            FocusRequest::RestoreEditor => self.set_focus(Focus::Editor(self.active_slot()), jobs),
            FocusRequest::Editor(slot) => self.set_focus(Focus::Editor(slot), jobs),
        }
    }

    pub(super) fn apply_menu_outcome(&mut self, out: MenuOutcome, cx: &mut Cx<'_>) -> bool {
        self.dirty |= out.dirty;
        match out.submenu_timer {
            SubmenuTimer::Keep => {}
            SubmenuTimer::Cancel => self.menu_submenu_timer.cancel(),
            SubmenuTimer::Arm(target) => {
                let delay = Duration::from_millis(u64::from(cx.settings.menu_show_delay_ms));
                let window = self.id();
                self.menu_submenu_timer = cx
                    .jobs
                    .after(delay, AppEvent::MenuSubmenuTimer { window, target });
            }
        }
        self.apply_focus_request(out.focus, cx.jobs);
        if let Some(cmd) = out.command {
            self.dispatch_command(cmd, cx);
            return true;
        }
        out.handled
    }

    pub(crate) fn on_activated(&mut self, active: bool, jobs: &Jobs<AppEvent>) {
        if !active {
            self.native.hwnd().complete_ime_composition();
            if let Some(slot) = self.focused_editor_slot()
                && let Some(ed) = self.editor_by_slot_mut(slot)
            {
                ed.on_blur();
            }
            self.cancel_menu(jobs);
            self.stop_all_editor_blinks();
            self.native.set_text_caret(None, false);
        } else if let Some(slot) = self.focused_editor_slot() {
            self.start_editor_blink(slot, jobs);
        }
    }

    pub(crate) fn on_editor_timer(
        &mut self,
        slot: EditorSlot,
        kind: EditorTimer,
        jobs: &Jobs<AppEvent>,
    ) {
        self.dirty |= self
            .with_editor(slot, jobs, |ed, timers| ed.on_timer(kind, timers))
            .unwrap_or(false);
    }
}
