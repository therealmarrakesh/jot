use std::time::{Duration, Instant};

use crate::APP_NAME;
use crate::app::{AppEvent, Cx, Effect};
use crate::color::transparent_black;
use crate::command::{AppCommand, Command, app_shortcut, edit_shortcut};
use crate::dialog::{DialogKind, Dialogs, Prompt};
use crate::document::{self, DocPath, Format, PathKey};
use crate::editor::{
    Clipboard, Editor, EditorPaintParams, EditorPaintStyle, EditorTimer, EditorTimers, RopeExt as _,
};
use crate::files::FileRequest;
use crate::find::{self, FindBar, FindOutcome};
use crate::geom::{Edges, Point, Rect, Size, point, size};
use crate::gfx::{Surface, TextSystem, fill, font};
use crate::ids::{CustomDraw, EditorSlot, Frame, HitId, Node, UiState};
use crate::keytips::KeyTip;
use crate::menu::{
    Entry, FocusRequest, MENU_BAR_HEIGHT, MenuBar, MenuOutcome, MenuState, SubmenuTimer,
};
use crate::platform::{
    Jobs, Key, KeyEvent, Modifiers, MouseButton, NativeWindow, SystemSettings, TimerHandle,
    WindowControlArea, WindowId, clipboard, format_local_time_date,
};

struct SystemClipboard;

impl Clipboard for SystemClipboard {
    fn read(&mut self) -> Option<String> {
        clipboard::read_clipboard_text()
    }

    fn write(&mut self, text: &str) -> bool {
        clipboard::write_clipboard_text(text)
    }
}
use crate::tabs::{
    self, CAPTION_BUTTON_WIDTH, SCROLL_REPEAT_DELAY, SCROLL_REPEAT_INTERVAL, SCROLL_STEP,
    STRIP_TOP, TAB_HEIGHT, Tab, TabData, TabId, TabStrip, WHEEL_SHARE,
};
use crate::theme::Palette;
use crate::tooltip::TooltipProbe;
use crate::ui::controls::{
    BODY_LINE, BODY_SIZE, CONTROL_RADIUS, ICON_FONT, IconButtonStyle, SMALL_FONT, TEXT_FONT,
    icon_button,
};
use crate::ui::{AvailSize, Brushes, box_, col, custom, row, text};

pub(crate) const WINDOW_WIDTH: f32 = 2213.;
pub(crate) const WINDOW_HEIGHT: f32 = 1107.;
pub(crate) const WINDOW_MAX_SCREEN_SHARE: f32 = 0.9;

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

const EDITOR_FONT_SIZE: f32 = 11. * 96. / 72.;
const EDITOR_LINE_HEIGHT: f32 = 20.;
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
const ZOOM_STEP: f32 = 0.1;
const ZOOM_MIN: f32 = 0.1;
const ZOOM_MAX: f32 = 5.;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Focus {
    Editor(EditorSlot),
    MenuBar,
    DialogButton(usize),
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct StatusMetrics {
    line: usize,
    column: usize,
    selected: usize,
    total: usize,
    zoom_pct: u32,
    format: Format,
}

#[derive(Clone, Copy, PartialEq)]
struct MinSizeKey {
    status_bar: bool,
    zoom_pct: u32,
    format: Format,
    menu_width: f32,
    scale: f32,
}

#[derive(Clone, Copy)]
struct MinSizeCache {
    key: MinSizeKey,
    size: Size,
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
    pub(crate) tabs: Vec<Tab>,
    pub(crate) active: usize,
    pub(crate) strip: TabStrip,
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
        let mut strip = TabStrip::new();
        let data = initial.unwrap_or_else(|| TabData::create(None, word_wrap, EDITOR_PADDINGS));
        let mut tab = Tab::adopt(data);
        tab.slot = strip.next_slot();
        let active_id = tab.data.id;

        let mut win = Self {
            surface,
            native,
            ui: UiState::default(),
            tabs: vec![tab],
            active: 0,
            strip,
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

    fn sync_cursor(&self, pos: Point) {
        self.native.set_cursor(self.ui.frame().cursor_at(pos));
    }

    #[cfg(test)]
    pub(crate) fn min_size_cached(&self) -> Option<Size> {
        self.min_size_cache.map(|c| c.size)
    }

    #[inline]
    pub(crate) fn active_tab_id(&self) -> TabId {
        self.tabs[self.active].data.id
    }

    #[inline]
    pub(crate) fn active_editor(&self) -> &Editor {
        &self.tabs[self.active].data.editor
    }

    #[inline]
    pub(crate) fn active_editor_mut(&mut self) -> &mut Editor {
        &mut self.tabs[self.active].data.editor
    }

    #[inline]
    pub(crate) fn active_slot(&self) -> EditorSlot {
        EditorSlot::Tab(self.active_tab_id())
    }

    pub(crate) fn tab_index(&self, id: TabId) -> Option<usize> {
        self.tabs.iter().position(|t| t.data.id == id)
    }

    pub(crate) fn tab_name(&self, ix: usize) -> String {
        document::display_name(self.tabs[ix].data.path())
    }

    pub(crate) fn editor_by_slot(&self, slot: EditorSlot) -> Option<&Editor> {
        match slot {
            EditorSlot::Tab(id) => self.tab_index(id).map(|ix| &self.tabs[ix].data.editor),
            EditorSlot::FindQuery => Some(&self.find.query),
            EditorSlot::FindReplace => Some(&self.find.replacement),
            EditorSlot::GoToLine => Some(&self.dialogs.line),
        }
    }

    pub(crate) fn editor_by_slot_mut(&mut self, slot: EditorSlot) -> Option<&mut Editor> {
        match slot {
            EditorSlot::Tab(id) => self
                .tab_index(id)
                .map(|ix| &mut self.tabs[ix].data.editor),
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

    fn with_editor<R>(
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
        for tab in &mut self.tabs {
            tab.data.editor.stop_blink();
        }
        self.find.query.stop_blink();
        self.find.replacement.stop_blink();
        self.dialogs.line.stop_blink();
    }

    pub(crate) fn focus_active_editor(&mut self, jobs: &Jobs<AppEvent>) {
        self.set_focus(Focus::Editor(self.active_slot()), jobs);
    }

    pub(crate) fn refresh_title(&self) {
        self.native
            .set_title(&format!("{} - {APP_NAME}", self.tab_name(self.active)));
    }

    pub(crate) fn zoom(&self) -> f32 {
        self.tabs[self.active].data.zoom
    }

    fn line_height(&self) -> f32 {
        (EDITOR_LINE_HEIGHT * self.zoom()).round()
    }

    fn caret_height(&self) -> f32 {
        (CARET_HEIGHT * self.zoom()).round()
    }

    fn editor_min_height(&self) -> f32 {
        EDITOR_PADDING_TOP + EDITOR_PADDING_BOTTOM + self.line_height()
    }

    pub(crate) fn set_zoom(&mut self, zoom: f32) {
        self.tabs[self.active].data.zoom =
            ((zoom / ZOOM_STEP).round() * ZOOM_STEP).clamp(ZOOM_MIN, ZOOM_MAX);
        self.dirty = true;
    }

    pub(crate) fn set_word_wrap(&mut self, word_wrap: bool, ts: &TextSystem) {
        for tab in &mut self.tabs {
            tab.data.editor.set_soft_wrap(word_wrap, ts);
        }
        self.dirty = true;
    }

    pub(crate) fn select_tab_only(&mut self, ix: usize) {
        if self.active != ix {
            self.native.hwnd().complete_ime_composition();
            self.active_editor_mut().on_blur();
        }
        self.active = ix;
        self.strip.reveal_tab(&self.tabs, ix);
        self.find.clear_notice();
        if self.find.is_open() && !self.has_text() {
            self.find.close();
        }
        self.refresh_title();
        self.dirty = true;
    }

    pub(crate) fn activate_tab(&mut self, ix: usize, jobs: &Jobs<AppEvent>) {
        self.select_tab_only(ix);
        if !self.dialogs.is_open() && !self.menu_bar.is_open() {
            self.focus_active_editor(jobs);
        }
    }

    pub(crate) fn add_tab(&mut self, word_wrap: bool, jobs: &Jobs<AppEvent>) {
        let data = TabData::create(None, word_wrap, EDITOR_PADDINGS);
        self.push_tab(Tab::adopt(data), jobs);
    }

    pub(crate) fn push_tab(&mut self, mut tab: Tab, jobs: &Jobs<AppEvent>) {
        tab.slot = self.strip.next_slot();
        self.tabs.push(tab);
        self.strip.layout(self.tabs.len());
        self.activate_tab(self.tabs.len() - 1, jobs);
    }

    pub(crate) fn replace_tab(&mut self, ix: usize, mut tab: Tab, jobs: &Jobs<AppEvent>) {
        tab.slot = self.tabs[ix].slot;
        let replaced = std::mem::replace(&mut self.tabs[ix], tab);
        self.strip.depart(replaced.data.id);
        self.activate_tab(ix, jobs);
    }

    pub(crate) fn remove_tab(&mut self, ix: usize, jobs: &Jobs<AppEvent>) {
        if self.tabs.len() == 1 {
            self.pending_close = true;
            return;
        }
        let was_active = ix == self.active;
        let width = self.strip.width;
        let name: crate::types::SharedStr = self.tab_name(ix).into();
        let tab = self.tabs.remove(ix);
        let removed_id = tab.data.id;
        self.strip.depart(removed_id);
        self.dialogs.close_for_tab(removed_id);
        if ix < self.tabs.len()
            && self.ui.pointer().is_some_and(|p| {
                self.ui
                    .frame()
                    .rect_of(HitId::TabStrip)
                    .is_some_and(|r| r.contains(p.pos))
            })
        {
            self.strip.freeze(width);
        }
        self.strip.push_ghost(name, tab.slot, ix, width, was_active);
        self.settle_after_removal(ix, jobs);
    }

    fn settle_after_removal(&mut self, ix: usize, jobs: &Jobs<AppEvent>) {
        let closed_active = ix == self.active;
        if ix < self.active || self.active == self.tabs.len() {
            self.active -= 1;
        }
        self.strip.layout(self.tabs.len());
        let focus_in_find = matches!(
            self.focus,
            Focus::Editor(EditorSlot::FindQuery | EditorSlot::FindReplace)
        );
        if closed_active || !focus_in_find {
            self.activate_tab(self.active, jobs);
        } else {
            self.select_tab_only(self.active);
        }
    }

    pub(crate) fn drag_tab(&mut self, id: TabId, x: f32, grab: f32, start: f32) {
        let active_id = self.active_tab_id();
        if let Some(next_active) =
            self.strip
                .drag_tab(&mut self.tabs, active_id, id, x, grab, start)
        {
            self.active = next_active;
        }
        self.dirty = true;
    }

    pub(crate) fn release_tab(&mut self) {
        if self.strip.release_drag(self.tabs.len()) {
            self.dirty = true;
        }
    }

    pub(crate) fn take_tab(&mut self, id: TabId, jobs: &Jobs<AppEvent>) -> Option<TabData> {
        let ix = self.tab_index(id)?;
        let tab = self.tabs.remove(ix);
        self.strip.depart(id);
        if self.tabs.is_empty() {
            self.pending_close = true;
        } else {
            self.settle_after_removal(ix, jobs);
        }
        Some(tab.data)
    }

    pub(crate) fn insert_dragged_tab(
        &mut self,
        data: TabData,
        x: f32,
        grab: f32,
        jobs: &Jobs<AppEvent>,
    ) {
        let mut tab = Tab::adopt(data);
        tab.slot = self.strip.next_slot();
        let id = tab.data.id;
        let ix = self
            .strip
            .begin_inserted_drag(id, x, grab, self.tabs.len());
        self.tabs.insert(ix, tab);
        self.strip.layout(self.tabs.len());
        self.activate_tab(ix, jobs);
    }

    pub(crate) fn restore_tab(&mut self, data: TabData, ix: usize, jobs: &Jobs<AppEvent>) {
        let mut tab = Tab::adopt(data);
        let ix = ix.min(self.tabs.len());
        tab.slot = self.strip.next_slot();
        self.tabs.insert(ix, tab);
        self.strip.layout(self.tabs.len());
        self.activate_tab(ix, jobs);
    }

    pub(crate) fn return_tab(&mut self, id: TabId, ix: usize, jobs: &Jobs<AppEvent>) {
        self.strip.clear_drag();
        if let Some(from) = self.tab_index(id) {
            let tab = self.tabs.remove(from);
            let ix = ix.min(self.tabs.len());
            self.tabs.insert(ix, tab);
        }
        self.strip.layout(self.tabs.len());
        if let Some(ix) = self.tab_index(id) {
            self.activate_tab(ix, jobs);
        }
        self.dirty = true;
    }

    fn scroll_tabs(&mut self, delta: f32) -> bool {
        let moved = self.strip.scroll_by(delta);
        self.dirty = true;
        moved
    }

    pub(crate) fn on_tab_scroll_repeat(&mut self, step: f32) {
        if !self.scroll_tabs(step) {
            self.strip.stop_repeat();
        }
    }

    fn next_tab(&mut self, jobs: &Jobs<AppEvent>) {
        if self.dialogs.is_open() || self.tabs.len() <= 1 {
            return;
        }
        let next = (self.active + 1) % self.tabs.len();
        self.activate_tab(next, jobs);
    }

    fn prev_tab(&mut self, jobs: &Jobs<AppEvent>) {
        if self.dialogs.is_open() || self.tabs.len() <= 1 {
            return;
        }
        let prev = (self.active + self.tabs.len() - 1) % self.tabs.len();
        self.activate_tab(prev, jobs);
    }

    fn select_tab(&mut self, ix: usize, jobs: &Jobs<AppEvent>) {
        if self.dialogs.is_open() || ix >= self.tabs.len() || ix == self.active {
            return;
        }
        self.activate_tab(ix, jobs);
    }

    fn select_last_tab(&mut self, jobs: &Jobs<AppEvent>) {
        let last = self.tabs.len().saturating_sub(1);
        if self.dialogs.is_open() || last == self.active {
            return;
        }
        self.activate_tab(last, jobs);
    }

    pub(crate) fn find_open_tab_by_key(&self, key: &PathKey) -> Option<usize> {
        self.tabs
            .iter()
            .position(|tab| tab.data.path_key() == Some(key))
    }

    fn is_blank(&self, ix: usize) -> bool {
        let tab = &self.tabs[ix];
        tab.data.doc_path.is_none() && tab.data.editor.text().len() == 0
    }

    pub(crate) fn has_dirty_tabs(&self) -> bool {
        self.tabs.iter().any(|tab| tab.data.is_dirty())
    }

    pub(crate) fn dirty_tabs(&self) -> Vec<TabId> {
        self.tabs
            .iter()
            .filter(|tab| tab.data.is_dirty())
            .map(|tab| tab.data.id)
            .collect()
    }

    pub(crate) fn adopt_loaded_file(
        &mut self,
        loaded: document::Loaded,
        word_wrap: bool,
        jobs: &Jobs<AppEvent>,
    ) {
        let tab = Tab::adopt(TabData::create(Some(loaded), word_wrap, EDITOR_PADDINGS));
        if self.is_blank(self.active) {
            self.replace_tab(self.active, tab, jobs);
        } else {
            self.push_tab(tab, jobs);
        }
    }

    pub(crate) fn mark_tab_saved(
        &mut self,
        tab: TabId,
        snapshot: u64,
        doc_path: DocPath,
        format: Format,
    ) {
        let Some(ix) = self.tab_index(tab) else {
            return;
        };
        let t = &mut self.tabs[ix];
        t.data.editor.mark_saved_version(snapshot);
        t.data.doc_path = Some(doc_path);
        t.data.format = format;
        self.refresh_title();
        self.dirty = true;
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

    fn press_dialog_button(&mut self, ix: usize, cx: &mut Cx<'_>) {
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

    fn open_find(&mut self, replace: bool, ts: &TextSystem, jobs: &Jobs<AppEvent>) {
        let selected = self.active_editor().selected_text().to_string();
        self.find.open(replace, &selected, ts);
        self.set_focus(Focus::Editor(EditorSlot::FindQuery), jobs);
        self.dirty = true;
    }

    fn close_find(&mut self, jobs: &Jobs<AppEvent>) {
        if self.find.close() {
            self.focus_active_editor(jobs);
            self.dirty = true;
        }
    }

    fn refocus_find(&mut self, jobs: &Jobs<AppEvent>) {
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

    fn find_step(&mut self, forward: bool, ts: &TextSystem, jobs: &Jobs<AppEvent>) {
        let out = self
            .find
            .step(&mut self.tabs[self.active].data.editor, forward);
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

    fn replace_next(&mut self, ts: &TextSystem, jobs: &Jobs<AppEvent>) {
        let out = self
            .find
            .replace_next(&mut self.tabs[self.active].data.editor, ts);
        self.apply_find_outcome(out, jobs);
        self.dirty = true;
    }

    fn replace_all(&mut self, ts: &TextSystem, jobs: &Jobs<AppEvent>) {
        let out = self
            .find
            .replace_all(&mut self.tabs[self.active].data.editor, ts);
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

    fn menu_state(&self, view: crate::app::ViewSettings) -> MenuState {
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

    fn apply_menu_outcome(&mut self, out: MenuOutcome, cx: &mut Cx<'_>) -> bool {
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

    pub(crate) fn on_mouse_move(
        &mut self,
        pos: Point,
        pressed_button: Option<MouseButton>,
        cx: &mut Cx<'_>,
    ) {
        self.dirty |= self.ui.on_mouse_move(pos, pressed_button.is_some());

        let top = self.ui.top_hit();
        if !matches!(top, Some(HitId::TabScrollLeft | HitId::TabScrollRight)) {
            self.strip.stop_repeat();
        }
        if !self.dialogs.is_open() {
            let state = self.menu_state(cx.view);
            let menu_out = self.menu_bar.on_hover_change(top, state, cx.ts);
            let _ = self.apply_menu_outcome(menu_out, cx);
        }

        if self.strip.is_frozen()
            && !self
                .ui
                .frame()
                .rect_of(HitId::TabStrip)
                .is_some_and(|r| r.contains(pos))
        {
            self.dirty |= self.strip.thaw(self.tabs.len());
        }

        let dragging_left = pressed_button == Some(MouseButton::Left);
        let active_slot = self.active_slot();
        if let Some(axis) = self.active_editor().scrollbar_drag_axis() {
            let track_rect = self.ui.frame().rect_of(HitId::ScrollTrack(axis));
            if let Some(Some(notify)) = self.with_editor(active_slot, cx.jobs, |ed, timers| {
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
                .with_editor(slot, cx.jobs, |ed, timers| {
                    ed.on_mouse_move(pos, dragging_left, true, timers)
                })
                .unwrap_or(false)
        {
            self.dirty = true;
        }
        let active_focused = self.focus == Focus::Editor(active_slot);
        if (!self.menu_bar.is_open() && !self.dialogs.is_open())
            || self.active_editor().is_selecting()
        {
            self.dirty |= self
                .with_editor(active_slot, cx.jobs, |ed, timers| {
                    ed.on_mouse_move(pos, dragging_left, active_focused, timers)
                })
                .unwrap_or(false);
        }

        self.sync_cursor(pos);
    }

    pub(crate) fn on_mouse_leave(&mut self) {
        self.dirty |= self.ui.on_mouse_leave();
        self.strip.stop_repeat();
        if self.strip.is_frozen() {
            self.dirty |= self.strip.thaw(self.tabs.len());
        }
    }

    pub(crate) fn on_capture_lost(&mut self) {
        self.middle_pressed_tab = None;
        self.strip.stop_repeat();
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
        let menu_out = self.menu_bar.pointer_down();
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

        if self.menu_bar.is_open() && !in_menu_ui {
            let out = self.menu_bar.close();
            let _ = self.apply_menu_outcome(out, cx);
        }

        if button == MouseButton::Middle && !self.dialogs.is_open() {
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
                    let state = self.menu_state(cx.view);
                    let out = self.menu_bar.toggle_trigger(kind, state, cx.ts);
                    let _ = self.apply_menu_outcome(out, cx);
                    return;
                }
                Some(HitId::Tab(id)) => {
                    if let Some(ix) = self.tab_index(id) {
                        self.activate_tab(ix, cx.jobs);
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
                        self.strip.repeat = cx.jobs.every(
                            SCROLL_REPEAT_DELAY,
                            SCROLL_REPEAT_INTERVAL,
                            move || AppEvent::TabScrollRepeat {
                                window: wid,
                                step: -SCROLL_STEP,
                            },
                        );
                    }
                    return;
                }
                Some(HitId::TabScrollRight) => {
                    if self.scroll_tabs(SCROLL_STEP) {
                        let wid = self.id();
                        self.strip.repeat = cx.jobs.every(
                            SCROLL_REPEAT_DELAY,
                            SCROLL_REPEAT_INTERVAL,
                            move || AppEvent::TabScrollRepeat {
                                window: wid,
                                step: SCROLL_STEP,
                            },
                        );
                    }
                    return;
                }
                Some(HitId::ScrollThumb(axis)) if self.active_editor().scrollbar_interactive(axis) => {
                    if let Some(track) = self.ui.frame().rect_of(HitId::ScrollTrack(axis)) {
                        self.active_editor_mut()
                            .on_scrollbar_mouse_down(axis, true, pos, track);
                    }
                    return;
                }
                Some(HitId::ScrollTrack(axis)) if self.active_editor().scrollbar_interactive(axis) => {
                    if let Some(track) = self.ui.frame().rect_of(HitId::ScrollTrack(axis)) {
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
            self.set_focus(Focus::Editor(slot), cx.jobs);
            self.with_editor(slot, cx.jobs, |ed, timers| {
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
            && self.menu_bar.is_open()
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
        self.strip.stop_repeat();

        if button == MouseButton::Middle {
            let pressed = self.middle_pressed_tab.take();
            if let Some(id) = pressed
                && pressed == tab_over
                && self.tab_index(id).is_some()
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
            let track_rect = self.ui.frame().rect_of(HitId::ScrollTrack(axis));
            self.active_editor_mut()
                .on_scrollbar_mouse_up(pos, track_rect);
        }
        if let Some(slot) = self.focused_editor_slot()
            && slot != active_slot
        {
            self.with_editor(slot, cx.jobs, |ed, timers| {
                ed.on_mouse_up(button, timers);
            });
        }
        let open_ctx = self
            .with_editor(active_slot, cx.jobs, |ed, timers| {
                ed.on_mouse_up(button, timers)
            })
            .flatten();
        if let Some(anchor) = open_ctx {
            let state = self.menu_state(cx.view);
            let out = self.menu_bar.open_context_menu(anchor, state, false, cx.ts);
            let _ = self.apply_menu_outcome(out, cx);
            return;
        }

        match activated {
            Some(HitId::DialogButton(ix)) => self.press_dialog_button(ix, cx),
            Some(HitId::FieldClear(slot)) => {
                if let Some(ed) = self.editor_by_slot_mut(slot) {
                    ed.set_value("", cx.ts);
                }
                self.set_focus(Focus::Editor(slot), cx.jobs);
            }
            Some(HitId::FindToggleReplace) => {
                self.find.replace = !self.find.replace;
                self.refocus_find(cx.jobs);
            }
            Some(HitId::FindNext) => self.find_step(true, cx.ts, cx.jobs),
            Some(HitId::FindPrev) => self.find_step(false, cx.ts, cx.jobs),
            Some(HitId::FindOptions) => {
                let entries = vec![
                    Entry::toggle(
                        "Match case",
                        self.find.match_case,
                        AppCommand::ToggleMatchCase,
                    ),
                    Entry::toggle(
                        "Wrap around",
                        self.find.wrap_around,
                        AppCommand::ToggleWrapAround,
                    ),
                ];
                let out = self.menu_bar.open_flyout(
                    entries,
                    HitId::FindOptions,
                    EditorSlot::FindQuery,
                    cx.ts,
                );
                let _ = self.apply_menu_outcome(out, cx);
            }
            Some(HitId::FindClose) => self.close_find(cx.jobs),
            Some(HitId::FindReplace) => self.replace_next(cx.ts, cx.jobs),
            Some(HitId::FindReplaceAll) => self.replace_all(cx.ts, cx.jobs),
            Some(HitId::MenuItem { ix, .. }) => {
                let out = self.menu_bar.click_item((ix, None), cx.ts);
                let _ = self.apply_menu_outcome(out, cx);
            }
            Some(HitId::SubmenuItem { ix: sub_ix, .. }) => {
                let out = self.menu_bar.click_item((0, Some(sub_ix)), cx.ts);
                let _ = self.apply_menu_outcome(out, cx);
            }
            Some(HitId::MenuCommand { ix, .. }) => {
                let out = self.menu_bar.click_command(ix);
                let _ = self.apply_menu_outcome(out, cx);
            }
            Some(HitId::TabClose(id)) => {
                if self.tab_index(id).is_some() {
                    cx.emit(Effect::File(FileRequest::CloseTab {
                        window: self.id(),
                        tab: id,
                    }));
                }
            }
            Some(HitId::NewTab) => self.add_tab(cx.view.word_wrap, cx.jobs),
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
        if self.dialogs.is_open() {
            return;
        }
        let chain = self.ui.frame().hit_chain(pos);
        if self.strip.overflow && chain.contains(&HitId::TabList) {
            let viewport = self.strip.scroll_viewport();
            let notches = if delta_lines.y != 0. {
                delta_lines.y
            } else {
                delta_lines.x
            };
            self.scroll_tabs(-notches * WHEEL_SHARE * viewport);
            return;
        }

        if modifiers.ctrl && !self.menu_bar.is_open() && chain.contains(&HitId::EditorArea) {
            if delta_lines.y > 0. {
                self.set_zoom(self.zoom() + ZOOM_STEP);
            } else if delta_lines.y < 0. {
                self.set_zoom(self.zoom() - ZOOM_STEP);
            }
            return;
        }

        if self.menu_bar.is_open() {
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

        if self.dialogs.is_open() {
            let m = ev.modifiers;
            if m.alt && !m.ctrl && !m.win && matches!(ev.key, Key::F(4) | Key::Space) {
                return false;
            }
            if let Some(slot) = self.focused_editor_slot() {
                self.with_editor(slot, cx.jobs, |ed, timers| ed.pause_blink(timers));
            }
            match (m.ctrl, m.alt, m.shift, m.win, ev.key) {
                (false, false, false, false, Key::Enter) => {
                    let btn = match self.focus {
                        Focus::DialogButton(ix) => ix,
                        _ => 0,
                    };
                    self.press_dialog_button(btn, cx);
                    return true;
                }
                (false, false, false, false, Key::Space) => {
                    if let Focus::DialogButton(ix) = self.focus {
                        self.press_dialog_button(ix, cx);
                        return true;
                    }
                }
                (false, false, false, false, Key::Escape) => {
                    let last = self.dialogs.active_button_count().saturating_sub(1);
                    self.press_dialog_button(last, cx);
                    return true;
                }
                (false, false, false, false, Key::Tab) => {
                    if let Some(next) = self.dialogs.next_focus_step(self.focus, 1) {
                        self.set_focus(next, cx.jobs);
                    }
                    return true;
                }
                (false, false, true, false, Key::Tab) => {
                    if let Some(next) = self.dialogs.next_focus_step(self.focus, -1) {
                        self.set_focus(next, cx.jobs);
                    }
                    return true;
                }
                _ => {}
            }
            if self.focus == Focus::Editor(EditorSlot::GoToLine) {
                return edit_shortcut(ev)
                    .and_then(|cmd| {
                        self.with_editor(EditorSlot::GoToLine, cx.jobs, |ed, timers| {
                            ed.handle_command(cmd, &mut SystemClipboard, cx.ts, timers)
                        })
                    })
                    .unwrap_or(false);
            }
            return true;
        }

        let state = self.menu_state(cx.view);
        let menu_out =
            self.menu_bar
                .on_key_down(ev, self.focus == Focus::MenuBar, state, cx.ts);
        if menu_out.handled {
            return self.apply_menu_outcome(menu_out, cx);
        }

        if let Some(slot) = self.focused_editor_slot() {
            self.with_editor(slot, cx.jobs, |ed, timers| ed.pause_blink(timers));
        }

        if ev.key == Key::Tab
            && !ev.modifiers.ctrl
            && !ev.modifiers.alt
            && !ev.modifiers.win
            && self.find.replace
        {
            match self.focus {
                Focus::Editor(EditorSlot::FindQuery) => {
                    self.set_focus(Focus::Editor(EditorSlot::FindReplace), cx.jobs);
                    return true;
                }
                Focus::Editor(EditorSlot::FindReplace) => {
                    self.set_focus(Focus::Editor(EditorSlot::FindQuery), cx.jobs);
                    return true;
                }
                _ => {}
            }
        }
        if ev.key == Key::Enter && !ev.modifiers.ctrl && !ev.modifiers.alt && !ev.modifiers.win {
            match (self.focus, ev.modifiers.shift) {
                (Focus::Editor(EditorSlot::FindQuery), false) => {
                    self.dispatch_app_command(AppCommand::FindNext, cx);
                    return true;
                }
                (Focus::Editor(EditorSlot::FindQuery), true) => {
                    self.dispatch_app_command(AppCommand::FindPrevious, cx);
                    return true;
                }
                (Focus::Editor(EditorSlot::FindReplace), false) => {
                    self.dispatch_app_command(AppCommand::ReplaceNext, cx);
                    return true;
                }
                _ => {}
            }
        }

        if let Some(slot) = self.focused_editor_slot()
            && let Some(cmd) = edit_shortcut(ev)
            && self
                .with_editor(slot, cx.jobs, |ed, timers| {
                    ed.handle_command(cmd, &mut SystemClipboard, cx.ts, timers)
                })
                .unwrap_or(false)
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
        if self.dialogs.is_open() {
            return false;
        }
        let menu_out = self.menu_bar.on_key_up(ev);
        if menu_out.handled {
            return self.apply_menu_outcome(menu_out, cx);
        }
        self.dirty |= menu_out.dirty;
        false
    }

    pub(crate) fn on_text_input(&mut self, text: &str, cx: &mut Cx<'_>) {
        let Some(slot) = self.focused_editor_slot() else {
            return;
        };
        if self.dialogs.is_open() && slot != EditorSlot::GoToLine {
            return;
        }
        self.with_editor(slot, cx.jobs, |ed, timers| {
            ed.handle_text_input(text, cx.ts, timers);
        });
    }

    pub(crate) fn on_ime_composition(&mut self, text: &str, cursor: usize, cx: &mut Cx<'_>) {
        let Some(slot) = self.focused_editor_slot() else {
            return;
        };
        if self.dialogs.is_open() && slot != EditorSlot::GoToLine {
            return;
        }
        if let Some(ed) = self.editor_by_slot_mut(slot) {
            ed.handle_ime_composition(text, cursor, cx.ts);
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

    pub(crate) fn dispatch_command(&mut self, cmd: Command, cx: &mut Cx<'_>) -> bool {
        match cmd {
            Command::App(app_cmd) => self.dispatch_app_command(app_cmd, cx),
            Command::Edit(edit_cmd) => self
                .with_editor(self.active_slot(), cx.jobs, |ed, timers| {
                    ed.handle_command(edit_cmd, &mut SystemClipboard, cx.ts, timers)
                })
                .unwrap_or(false),
        }
    }

    fn dispatch_app_command(&mut self, cmd: AppCommand, cx: &mut Cx<'_>) -> bool {
        match cmd {
            AppCommand::Cancel => {
                if self.find.is_open() {
                    self.close_find(cx.jobs);
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
                    let state = self.menu_state(cx.view);
                    let out = self.menu_bar.open_context_menu(anchor, state, true, cx.ts);
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
            AppCommand::NewTab => self.add_tab(cx.view.word_wrap, cx.jobs),
            AppCommand::CloseTab => {
                cx.emit(Effect::File(FileRequest::CloseTab {
                    window: self.id(),
                    tab: self.active_tab_id(),
                }));
            }
            AppCommand::NextTab => self.next_tab(cx.jobs),
            AppCommand::PrevTab => self.prev_tab(cx.jobs),
            AppCommand::SelectTab(ix) => self.select_tab(ix, cx.jobs),
            AppCommand::SelectLastTab => self.select_last_tab(cx.jobs),
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
                self.active_editor_mut().replace(&stamp, cx.ts);
            }
            AppCommand::GoTo => self.open_go_to(cx.ts, cx.jobs),
            AppCommand::Find => {
                if self.has_text() {
                    self.open_find(false, cx.ts, cx.jobs);
                }
            }
            AppCommand::Replace => {
                if self.has_text() {
                    self.open_find(true, cx.ts, cx.jobs);
                }
            }
            AppCommand::FindNext => {
                if self.has_text() {
                    self.find_step(true, cx.ts, cx.jobs);
                }
            }
            AppCommand::FindPrevious => {
                if self.has_text() {
                    self.find_step(false, cx.ts, cx.jobs);
                }
            }
            AppCommand::ToggleMatchCase => {
                self.find.match_case = !self.find.match_case;
                self.refocus_find(cx.jobs);
            }
            AppCommand::ToggleWrapAround => {
                self.find.wrap_around = !self.find.wrap_around;
                self.refocus_find(cx.jobs);
            }
            AppCommand::ReplaceNext => self.replace_next(cx.ts, cx.jobs),
        }
        true
    }

    fn status_metrics(&self) -> StatusMetrics {
        let editor = self.active_editor();
        let text = editor.text();
        let (line, column) = editor.caret_line_col();
        let selection = editor.selected_range();
        let selected = text
            .byte_to_char_idx(selection.end)
            .saturating_sub(text.byte_to_char_idx(selection.start));
        StatusMetrics {
            line,
            column,
            selected,
            total: text.len_chars(),
            zoom_pct: (self.zoom() * 100.).round() as u32,
            format: self.tabs[self.active].data.format,
        }
    }

    fn update_min_size(
        &mut self,
        status_bar: bool,
        frame: &Frame,
        p: Palette,
        ts: &TextSystem,
    ) {
        let zoom_pct = (self.zoom() * 100.).round() as u32;
        let format = self.tabs[self.active].data.format;
        let scale = self.native.scale_factor();
        let menu_width = frame
            .rect_of(HitId::MenuBar)
            .map_or(0.0, |r| r.size.width);
        let key = MinSizeKey {
            status_bar,
            zoom_pct,
            format,
            menu_width,
            scale,
        };
        if self
            .min_size_cache
            .is_some_and(|cached| cached.key == key)
        {
            return;
        }
        let status_sz = status_bar.then(|| {
            let reference = StatusMetrics {
                line: 9999,
                column: 999,
                selected: 0,
                total: 999_999,
                zoom_pct,
                format,
            };
            Frame::measure_min_size(self.status_bar(reference, p), scale, ts)
        });
        let menu_row = MENU_BAR_LEFT + menu_width + THEME_BUTTON_WIDTH + THEME_BUTTON_RIGHT;
        let width = status_sz
            .map_or(0., |s| s.width)
            .max(menu_row)
            .max(tabs::TITLE_BAR_MIN_WIDTH);
        let chrome_top = frame.rect_of(HitId::EditorArea).map_or(0., |r| r.top());
        let height = chrome_top + self.editor_min_height() + status_sz.map_or(0., |s| s.height);
        let next = size(width, height);
        let changed = self
            .min_size_cache
            .is_none_or(|cached| cached.size != next);
        self.min_size_cache = Some(MinSizeCache { key, size: next });
        if changed {
            self.native.set_min_size(Some(next));
        }
    }

    pub(crate) fn render_frame(&mut self, now: Instant, cx: &Cx<'_>) -> bool {
        let p = cx.palette;
        let view = cx.view;
        let reduce_motion = cx.settings.reduce_motion;
        let caret_width_px = cx.settings.caret_width_px;
        let ts = cx.ts;
        let jobs = cx.jobs;
        self.dirty = false;
        let logical = self.native.logical_size();
        let scale = self.native.scale_factor();
        let window_active = self.native.is_active();
        let maximized = self.native.is_maximized();
        let status_metrics = view.status_bar.then(|| self.status_metrics());

        self.ui.begin_frame(now, reduce_motion);
        let strip = self
            .strip
            .sample(&self.tabs, self.active, logical.width, &mut self.ui);
        self.menu_bar.sample(&mut self.ui);

        let title_bar = self.title_bar(&strip, window_active, maximized, p);
        let menu_bar = self.menu_bar_row(p);
        let find_node = self
            .find
            .render(logical.width, self.focused_editor_slot(), p, &mut self.ui);
        let editor_node = self.editor_area(find_node, now, p, jobs);
        let status_node = status_metrics.map(|m| self.status_bar(m, p));
        let menu_popup = self.menu_bar.render_popup(p, ts, &mut self.ui);
        let dialog_node = self.dialogs.render(self.focus, p, &mut self.ui);

        let chrome = col()
            .size_full()
            .child(title_bar)
            .child(menu_bar)
            .child(editor_node)
            .when_some(status_node, |this, s| this.child(s));
        let root = box_()
            .relative()
            .size_full()
            .child(chrome)
            .when_some(menu_popup, |this, m| this.child(m))
            .when_some(dialog_node, |this, d| this.child(d));

        let frame = Frame::layout(
            root,
            AvailSize::definite(logical.width, logical.height),
            scale,
            ts,
        );
        self.native.set_control_areas(frame.control_areas());
        self.update_min_size(view.status_bar, &frame, p, ts);

        let editor_style = EditorPaintStyle {
            font: font("Consolas"),
            font_size: EDITOR_FONT_SIZE * self.zoom(),
            line_height: self.line_height(),
            color: p.text_primary,
        };
        let field_style = EditorPaintStyle {
            font: font(TEXT_FONT),
            font_size: BODY_SIZE,
            line_height: BODY_LINE + 2.,
            color: p.text_primary,
        };
        let caret_height = self.caret_height();
        let active_slot = self.active_slot();
        let editor_focused = self.focus == Focus::Editor(active_slot);
        let show_selection = editor_focused || self.menu_bar.is_open() || self.find.is_open();
        let caret_hidden = self.menu_bar.access_active();
        let mut editor_needs_redraw = false;

        self.active_editor_mut().set_caret_height(caret_height);
        self.active_editor_mut().set_caret_hidden(caret_hidden);

        let editor_rects: Vec<(EditorSlot, Rect)> = frame
            .custom_rects()
            .filter_map(|(token, rect)| match token {
                CustomDraw::Editor(slot) => Some((slot, rect)),
                _ => None,
            })
            .collect();
        let focus = self.focus;
        for (slot, rect) in editor_rects {
            let (style, focused, show_sel) = match slot {
                EditorSlot::Tab(_) => (&editor_style, editor_focused, show_selection),
                _ => {
                    let f = focus == Focus::Editor(slot);
                    (&field_style, f, f)
                }
            };
            if let Some(ed) = self.editor_by_slot_mut(slot) {
                let outcome = ed.prepare(
                    rect,
                    EditorPaintParams {
                        style,
                        palette: p,
                        window_active,
                        focused,
                        show_selection: show_sel,
                        scale,
                        caret_width_px,
                    },
                    ts,
                );
                editor_needs_redraw |= outcome.needs_redraw;
            }
        }

        let tabs = &self.tabs;
        let find = &self.find;
        let dialogs = &self.dialogs;
        let needs_retry = self.surface.draw(p.window_clear, |renderer| {
            frame.paint(renderer, &mut |token, rect, renderer| match token {
                CustomDraw::Editor(slot) => {
                    let ed = match slot {
                        EditorSlot::Tab(id) => tabs
                            .iter()
                            .find(|t| t.data.id == id)
                            .map(|t| &t.data.editor),
                        EditorSlot::FindQuery => Some(&find.query),
                        EditorSlot::FindReplace => Some(&find.replacement),
                        EditorSlot::GoToLine => Some(&dialogs.line),
                    };
                    if let Some(ed) = ed {
                        ed.paint(p, renderer);
                    }
                }
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

        let ime_caret = self
            .focused_editor_slot()
            .and_then(|slot| self.editor_by_slot(slot))
            .and_then(|ed| ed.ime_caret_bounds());
        self.native
            .set_text_caret(ime_caret, window_active && !caret_hidden);

        self.has_painted = true;
        let ui_needs_vsync = self.ui.finish_frame(frame);
        if let Some(ptr) = self.ui.pointer() {
            self.sync_cursor(ptr.pos);
        }
        let needs_vsync = ui_needs_vsync || editor_needs_redraw || needs_retry;
        if needs_vsync {
            self.dirty = true;
        }
        needs_vsync
    }

    fn title_bar(
        &mut self,
        strip: &tabs::StripFrame,
        window_active: bool,
        maximized: bool,
        p: Palette,
    ) -> Node {
        let strip_node = self
            .strip
            .render(&self.tabs, self.active, strip, p, &mut self.ui);
        let captions = caption_buttons(window_active, maximized, p, &mut self.ui);
        row()
            .flex_none()
            .h(STRIP_TOP + TAB_HEIGHT)
            .child(
                box_()
                    .flex_none()
                    .w(tabs::HEADER_WIDTH)
                    .h_full()
                    .window_area(WindowControlArea::Drag),
            )
            .child(
                col()
                    .flex_none()
                    .child(
                        box_()
                            .h(STRIP_TOP)
                            .window_area(WindowControlArea::Drag),
                    )
                    .child(strip_node),
            )
            .child(
                box_()
                    .flex_1()
                    .h_full()
                    .window_area(WindowControlArea::Drag),
            )
            .child(captions)
    }

    fn menu_bar_row(&mut self, p: Palette) -> Node {
        let menu = self
            .menu_bar
            .render_bar(self.focus == Focus::MenuBar, p, &mut self.ui);
        let theme_btn = icon_button(
            p,
            HitId::ThemeToggle,
            "Switch theme",
            if p.dark { SUN_GLYPH } else { MOON_GLYPH },
            IconButtonStyle {
                glyph_size: THEME_GLYPH_SIZE,
                width: THEME_BUTTON_WIDTH,
                height: MENU_BAR_HEIGHT,
                radius: CONTROL_RADIUS,
            },
            true,
            &mut self.ui,
        );
        col()
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

    fn editor_area(
        &mut self,
        find: Option<Node>,
        now: Instant,
        p: Palette,
        jobs: &Jobs<AppEvent>,
    ) -> Node {
        let slot = self.active_slot();
        let lh = self.line_height();
        let wid = self.id();
        let blink_timing = self.blink_timing;
        let mut timers = WindowEditorTimers::new(jobs, wid, slot, blink_timing);
        let scrollbars = self.tabs[self.active]
            .data
            .editor
            .render_scrollbars(lh, p, now, &mut self.ui, &mut timers);
        let editor_view = self.tabs[self.active].data.editor.view_with_scrollbars(
            HitId::Editor(slot),
            CustomDraw::Editor(slot),
            lh,
            scrollbars,
        );
        box_()
            .hit(HitId::EditorArea)
            .relative()
            .flex_1()
            .min_h(self.editor_min_height())
            .bg(p.editor)
            .child(
                box_()
                    .relative()
                    .size_full()
                    .overflow_hidden()
                    .child(
                        box_()
                            .size_full()
                            .font_family("Consolas")
                            .text_size(EDITOR_FONT_SIZE * self.zoom())
                            .line_height(lh)
                            .text_color(p.text_primary)
                            .child(editor_view),
                    ),
            )
            .when_some(find, |this, f| this.child(f))
    }

    fn status_bar(&self, metrics: StatusMetrics, p: Palette) -> Node {
        col().flex_none().child(separator(p)).child(
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
                .text_child(format!("Ln {}, Col {}", metrics.line, metrics.column))
                .child(status_divider(p))
                .text_child(character_count(metrics.selected, metrics.total))
                .child(
                    custom(CustomDraw::StatusSpacer)
                        .flex_1()
                        .min_w(STATUS_DIVIDER_WIDTH)
                        .h(STATUS_DIVIDER_HEIGHT),
                )
                .text_child(format!("{}%", metrics.zoom_pct))
                .child(status_divider(p))
                .text_child(metrics.format.line_ending_label())
                .child(status_divider(p))
                .text_child(metrics.format.encoding.label()),
        )
    }
}

fn separator(p: Palette) -> Node {
    box_()
        .flex_none()
        .h(SEPARATOR_HEIGHT)
        .bg(p.stroke_on_layer)
}

fn status_divider(p: Palette) -> Node {
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

#[derive(Clone, Copy)]
enum Caption {
    Minimize,
    Maximize,
    Restore,
    Close,
}

impl Caption {
    fn hit(self) -> HitId {
        match self {
            Self::Minimize => HitId::CaptionMin,
            Self::Maximize | Self::Restore => HitId::CaptionMax,
            Self::Close => HitId::CaptionClose,
        }
    }

    fn glyph(self) -> &'static str {
        match self {
            Self::Minimize => "\u{E921}",
            Self::Maximize => "\u{E922}",
            Self::Restore => "\u{E923}",
            Self::Close => "\u{E8BB}",
        }
    }

    fn area(self) -> WindowControlArea {
        match self {
            Self::Minimize => WindowControlArea::Min,
            Self::Maximize | Self::Restore => WindowControlArea::Max,
            Self::Close => WindowControlArea::Close,
        }
    }

    fn render(self, active: bool, p: Palette, ui: &mut UiState) -> Node {
        let hit = self.hit();
        let (hover_bg, pressed_bg, hover_fg, pressed_fg) = match self {
            Self::Close => (
                p.close_hover,
                p.close_pressed,
                p.close_glyph_hover,
                p.close_glyph_pressed,
            ),
            _ => (
                p.layer_hover,
                p.subtle_tertiary,
                p.caption_glyph,
                p.caption_glyph_pressed,
            ),
        };
        let rest_fg = if active {
            p.caption_glyph
        } else {
            p.caption_glyph_inactive
        };
        let bg = ui.brush(
            hit,
            Brushes {
                rest: transparent_black(),
                hover: hover_bg,
                pressed: pressed_bg,
            },
            true,
        );
        let fg = if ui.is_pressed(hit) {
            pressed_fg
        } else if ui.is_hovered(hit) {
            hover_fg
        } else {
            rest_fg
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
            .window_area(self.area())
            .child(
                box_()
                    .relative()
                    .when(matches!(self, Self::Minimize), |this| {
                        this.left(1.).top(1.)
                    })
                    .child(text(self.glyph())),
            )
    }
}

fn caption_buttons(active: bool, maximized: bool, p: Palette, ui: &mut UiState) -> Node {
    let maximize = if maximized {
        Caption::Restore
    } else {
        Caption::Maximize
    };
    col()
        .flex_none()
        .h_full()
        .child(
            row()
                .child(Caption::Minimize.render(active, p, ui))
                .child(maximize.render(active, p, ui))
                .child(Caption::Close.render(active, p, ui)),
        )
        .child(box_().flex_1().window_area(WindowControlArea::Drag))
}
