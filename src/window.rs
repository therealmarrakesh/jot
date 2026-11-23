//! Main editor window: custom Mica Alt titlebar + caption buttons, tab strip, menu bar row,
//! multi-tab editor host, status bar, dynamic min-size tracking, and direct top-down input routing.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::APP_NAME;
use crate::app::{AppEvent, ViewSettings};
use crate::color::{rgb, rgba, transparent_black};
use crate::command::Command;
use crate::dialog::{DialogKind, Dialogs};
use crate::document::{self, Format};
use crate::editor::{
    Editor, EditorPaintStyle, EditorTimer, EditorTimers, Position, RopeExt as _, key_to_command,
};
use crate::files::{
    self, FilePrompt, FileWorkflow, UNTITLED_FILE_NAME, WorkflowFinish, WorkflowStep,
};
use crate::find::FindBar;
use crate::geom::{Axis, Edges, Point, Rect, Size, point, size};
use crate::gfx::{TextSystem, fill, font};
use crate::keytips::KeyTip;
use crate::menu::{MenuBar, MenuKind, MenuOutcome};
use crate::platform::{
    CursorStyle, Jobs, Key, KeyEvent, Modifiers, MouseButton, NativeWindow, TimerHandle,
    WindowControlArea, WindowId, format_local_time_date,
};
use crate::tabs::{
    self, CAPTION_BUTTON_WIDTH, SCROLL_REPEAT_DELAY, SCROLL_REPEAT_INTERVAL, SCROLL_STEP,
    STRIP_TOP, TAB_HEIGHT, Tab, TabData, TabId, TabStrip, WHEEL_SHARE,
};
use crate::theme::Palette;
use crate::types::Result;
use crate::ui::controls::{
    BODY_LINE, BODY_SIZE, CONTROL_RADIUS, ICON_FONT, IconButtonStyle, SMALL_FONT, TEXT_FONT,
    icon_button,
};
use crate::ui::{
    AvailSize, Brushes, CustomDraw, EditorSlot, Frame, HitId, Node, UiState, box_, col, custom,
    row, text,
};

pub(crate) const WINDOW_WIDTH: f32 = 2213.;
pub(crate) const WINDOW_HEIGHT: f32 = 1107.;
pub(crate) const WINDOW_MAX_SCREEN_SHARE: f32 = 0.9;

const CLOSE_HOVER: u32 = 0xc42b1cff;
const CLOSE_PRESSED: u32 = 0xc42b1ce6;
const CLOSE_GLYPH_HOVER: u32 = 0xffffffff;
const CLOSE_GLYPH_PRESSED: u32 = 0xffffffb3;

const SEPARATOR_HEIGHT: f32 = 1.;
pub(crate) const MENU_BAR_HEIGHT: f32 = 32.;
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

/// Explicit per-window focus target (replaces `FocusHandle` / `FocusNode` graph).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Focus {
    Editor(EditorSlot),
    MenuBar,
    DialogButton(usize),
}

/// Cross-window or app-level action returned cleanly from `MainWindow` methods to `App`.
pub(crate) enum WindowAction {
    NewWindow,
    Exit,
    ToggleStatusBar,
    ToggleTheme,
    ToggleWordWrap,
    StartTabDrag {
        tab: TabId,
        index: usize,
        grab: Point,
        position: Point,
        alone: bool,
    },
    CancelTabDrag,
    ForgetTabDrag(TabId),
}

/// Result of painting one frame of a `MainWindow`.
pub(crate) struct FrameResult {
    pub(crate) needs_vsync: bool,
    pub(crate) device_lost: bool,
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
    status: Option<StatusMetrics>,
    zoom_pct: u32,
    menu_width: f32,
    scale: f32,
}

#[derive(Clone, Copy)]
struct MinSizeCache {
    key: MinSizeKey,
    size: Size,
}

/// Lightweight `EditorTimers` adapter routing timer callbacks into typed `AppEvent`s.
pub(crate) struct WindowEditorTimers<'a> {
    jobs: &'a Jobs<AppEvent>,
    window: WindowId,
    slot: EditorSlot,
}

impl<'a> WindowEditorTimers<'a> {
    pub(crate) fn new(jobs: &'a Jobs<AppEvent>, window: WindowId, slot: EditorSlot) -> Self {
        Self { jobs, window, slot }
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
}

/// Concrete main application window owning its `NativeWindow`, tabs, UI scene, and workflows.
pub(crate) struct MainWindow {
    pub(crate) native: NativeWindow,
    pub(crate) ui: UiState,
    pub(crate) tabs: Vec<Tab>,
    pub(crate) active: usize,
    pub(crate) strip: TabStrip,
    pub(crate) menu_bar: MenuBar,
    pub(crate) find: FindBar,
    pub(crate) dialogs: Dialogs,
    pub(crate) focus: Focus,
    pub(crate) file_workflow: Option<FileWorkflow>,
    pub(crate) dirty: bool,
    pub(crate) pending_close: bool,
    pub(crate) pointer_hovered: bool,
    pub(crate) last_mouse_pos: Point,
    middle_pressed_tab: Option<TabId>,
    min_size_cache: Option<MinSizeCache>,
}

impl MainWindow {
    pub(crate) fn new(
        native: NativeWindow,
        initial: Option<TabData>,
        word_wrap: bool,
        jobs: &Jobs<AppEvent>,
    ) -> Self {
        let mut strip = TabStrip::new();
        let data = initial.unwrap_or_else(|| TabData::create(None, word_wrap, EDITOR_PADDINGS));
        let mut tab = Tab::adopt(data);
        tab.slot = strip.next_slot();
        let active_id = tab.data.id;

        let mut win = Self {
            native,
            ui: UiState::default(),
            tabs: vec![tab],
            active: 0,
            strip,
            menu_bar: MenuBar::default(),
            find: FindBar::new(),
            dialogs: Dialogs::new(),
            focus: Focus::Editor(EditorSlot::Tab(active_id.0)),
            file_workflow: None,
            dirty: true,
            pending_close: false,
            pointer_hovered: false,
            last_mouse_pos: Point::ZERO,
            middle_pressed_tab: None,
            min_size_cache: None,
        };
        win.activate_tab(0, jobs);
        win
    }

    #[inline]
    pub(crate) fn id(&self) -> WindowId {
        self.native.id
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

    pub(crate) fn tab_index(&self, id: TabId) -> Option<usize> {
        self.tabs.iter().position(|t| t.data.id == id)
    }

    pub(crate) fn tab_name(&self, ix: usize) -> String {
        document::display_name(self.tabs[ix].data.path.as_deref())
    }

    pub(crate) fn editor_by_slot(&self, slot: EditorSlot) -> Option<&Editor> {
        match slot {
            EditorSlot::Tab(raw) => self.tab_index(TabId(raw)).map(|ix| &self.tabs[ix].data.editor),
            EditorSlot::FindQuery => Some(&self.find.query),
            EditorSlot::FindReplace => Some(&self.find.replacement),
            EditorSlot::GoToLine => Some(&self.dialogs.line),
        }
    }

    pub(crate) fn editor_by_slot_mut(&mut self, slot: EditorSlot) -> Option<&mut Editor> {
        match slot {
            EditorSlot::Tab(raw) => self
                .tab_index(TabId(raw))
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

    pub(crate) fn ime_caret_bounds(&self) -> Option<(Rect, f32)> {
        let slot = self.focused_editor_slot()?;
        let rect = self.editor_by_slot(slot)?.ime_caret_bounds()?;
        Some((rect, self.native.scale_factor))
    }

    pub(crate) fn set_focus(&mut self, next: Focus, jobs: &Jobs<AppEvent>) {
        if self.focus == next {
            if let Focus::Editor(slot) = next {
                self.start_editor_blink(slot, jobs);
            }
            return;
        }
        if let Focus::Editor(old_slot) = self.focus
            && let Some(ed) = self.editor_by_slot_mut(old_slot)
        {
            ed.on_blur();
        }
        self.focus = next;
        if let Focus::Editor(new_slot) = next {
            self.start_editor_blink(new_slot, jobs);
        }
        self.dirty = true;
    }

    fn start_editor_blink(&mut self, slot: EditorSlot, jobs: &Jobs<AppEvent>) {
        let wid = self.id();
        if let Some(ed) = self.editor_by_slot_mut(slot) {
            let mut timers = WindowEditorTimers::new(jobs, wid, slot);
            ed.start_blink(&mut timers);
        }
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
        let slot = EditorSlot::Tab(self.active_tab_id().0);
        self.set_focus(Focus::Editor(slot), jobs);
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

    // ---- Tab operations ----

    pub(crate) fn activate_tab(&mut self, ix: usize, jobs: &Jobs<AppEvent>) {
        self.active = ix;
        self.strip.reveal_tab(&self.tabs, ix);
        self.find.clear_notice();
        if self.find.is_open() && !self.has_text() {
            self.find.close();
        }
        self.focus_active_editor(jobs);
        self.refresh_title();
        self.dirty = true;
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

    pub(crate) fn replace_tab(&mut self, ix: usize, mut tab: Tab, jobs: &Jobs<AppEvent>) -> TabId {
        tab.slot = self.tabs[ix].slot;
        let replaced = std::mem::replace(&mut self.tabs[ix], tab);
        self.strip.depart(replaced.data.id);
        self.activate_tab(ix, jobs);
        replaced.data.id
    }

    pub(crate) fn remove_tab(&mut self, ix: usize, jobs: &Jobs<AppEvent>) -> Option<TabId> {
        if self.tabs.len() == 1 {
            self.pending_close = true;
            return None;
        }
        let width = self.strip.width;
        let name: crate::types::SharedStr = self.tab_name(ix).into();
        let tab = self.tabs.remove(ix);
        let removed_id = tab.data.id;
        self.strip.depart(removed_id);
        if ix < self.tabs.len()
            && self
                .ui
                .frame
                .rect_of(HitId::TabStrip)
                .is_some_and(|r| r.contains(self.last_mouse_pos))
        {
            self.strip.freeze(width);
        }
        self.strip.push_ghost(name, tab.slot, ix, width);
        self.settle_after_removal(ix, jobs);
        Some(removed_id)
    }

    fn settle_after_removal(&mut self, ix: usize, jobs: &Jobs<AppEvent>) {
        if ix < self.active || self.active == self.tabs.len() {
            self.active -= 1;
        }
        self.strip.layout(self.tabs.len());
        self.activate_tab(self.active, jobs);
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

    // ---- File workflows ----

    pub(crate) fn find_open_tab(&self, path: &Path) -> Option<usize> {
        self.tabs.iter().position(|tab| {
            tab.data
                .path
                .as_deref()
                .is_some_and(|existing| document::same_path(existing, path))
        })
    }

    fn is_blank(&self, ix: usize) -> bool {
        let tab = &self.tabs[ix];
        tab.data.path.is_none() && tab.data.editor.text().len() == 0
    }

    fn dirty_tabs(&self) -> Vec<TabId> {
        self.tabs
            .iter()
            .filter(|tab| tab.data.is_dirty())
            .map(|tab| tab.data.id)
            .collect()
    }

    pub(crate) fn open_paths(&mut self, paths: Vec<PathBuf>, jobs: &Jobs<AppEvent>) {
        let mut to_load: Vec<PathBuf> = Vec::new();
        let mut last_existing = None;
        for raw in paths {
            let path = std::path::absolute(&raw).unwrap_or(raw);
            if let Some(ix) = self.find_open_tab(&path) {
                last_existing = Some(ix);
                continue;
            }
            if !to_load
                .iter()
                .any(|queued| document::same_path(queued, &path))
            {
                to_load.push(path);
            }
        }
        if to_load.is_empty() {
            if let Some(ix) = last_existing {
                self.activate_tab(ix, jobs);
            }
            return;
        }
        files::spawn_load_files(self.id(), to_load, jobs);
    }

    pub(crate) fn on_files_loaded(
        &mut self,
        results: Vec<(PathBuf, std::result::Result<document::Loaded, document::LoadError>)>,
        word_wrap: bool,
        jobs: &Jobs<AppEvent>,
    ) -> Vec<TabId> {
        let mut forgotten = Vec::new();
        for (path, result) in results {
            if let Some(ix) = self.find_open_tab(&path) {
                self.activate_tab(ix, jobs);
                continue;
            }
            match result {
                Ok(loaded) => {
                    let tab = Tab::adopt(TabData::create(
                        Some((path, loaded)),
                        word_wrap,
                        EDITOR_PADDINGS,
                    ));
                    if self.is_blank(self.active) {
                        forgotten.push(self.replace_tab(self.active, tab, jobs));
                    } else {
                        self.push_tab(tab, jobs);
                    }
                }
                Err(err) => {
                    self.show_file_prompt(files::load_error_prompt(&path, err), jobs);
                }
            }
        }
        forgotten
    }

    pub(crate) fn save(&mut self, jobs: &Jobs<AppEvent>) {
        let id = self.active_tab_id();
        self.start_file_workflow(
            FileWorkflow::single(
                WorkflowStep::SaveTab {
                    id,
                    activate: false,
                },
                WorkflowFinish::None,
            ),
            jobs,
        );
    }

    pub(crate) fn save_as(&mut self, jobs: &Jobs<AppEvent>) {
        let id = self.active_tab_id();
        self.start_file_workflow(
            FileWorkflow::single(WorkflowStep::SaveTabAs(id), WorkflowFinish::None),
            jobs,
        );
    }

    pub(crate) fn save_all(&mut self, jobs: &Jobs<AppEvent>) {
        let dirty = self.dirty_tabs();
        if dirty.is_empty() {
            return;
        }
        self.start_file_workflow(
            FileWorkflow::batch(
                dirty
                    .into_iter()
                    .map(|id| WorkflowStep::SaveTab { id, activate: true }),
                WorkflowFinish::None,
            ),
            jobs,
        );
    }

    pub(crate) fn close_tab(&mut self, ix: usize, jobs: &Jobs<AppEvent>) -> Option<TabId> {
        if !self.tabs[ix].data.is_dirty() {
            return self.remove_tab(ix, jobs);
        }
        let id = self.tabs[ix].data.id;
        self.activate_tab(ix, jobs);
        self.start_file_workflow(
            FileWorkflow::single(
                WorkflowStep::ConfirmDiscard {
                    id,
                    activate: false,
                },
                WorkflowFinish::RemoveTab(id),
            ),
            jobs,
        );
        None
    }

    pub(crate) fn request_close_window(&mut self, jobs: &Jobs<AppEvent>) -> bool {
        let dirty = self.dirty_tabs();
        if dirty.is_empty() {
            self.pending_close = true;
            return true;
        }
        self.start_file_workflow(
            FileWorkflow::batch(
                dirty
                    .into_iter()
                    .map(|id| WorkflowStep::ConfirmDiscard { id, activate: true }),
                WorkflowFinish::CloseWindow,
            ),
            jobs,
        );
        false
    }

    fn start_file_workflow(&mut self, workflow: FileWorkflow, jobs: &Jobs<AppEvent>) {
        self.file_workflow = Some(workflow);
        self.advance_file_workflow(jobs);
    }

    pub(crate) fn abort_file_workflow(&mut self) {
        self.file_workflow = None;
    }

    pub(crate) fn advance_file_workflow(&mut self, jobs: &Jobs<AppEvent>) -> Option<TabId> {
        loop {
            let next = self
                .file_workflow
                .as_mut()
                .and_then(FileWorkflow::next_step);
            let Some(step) = next else {
                if let Some(workflow) = self.file_workflow.take() {
                    match workflow.on_finish {
                        WorkflowFinish::None => {}
                        WorkflowFinish::RemoveTab(id) => {
                            if let Some(ix) = self.tab_index(id) {
                                return self.remove_tab(ix, jobs);
                            }
                        }
                        WorkflowFinish::CloseWindow => {
                            self.pending_close = true;
                        }
                    }
                }
                return None;
            };

            match step {
                WorkflowStep::SaveTab { id, activate } => {
                    let Some(ix) = self.tab_index(id) else {
                        self.abort_file_workflow();
                        return None;
                    };
                    if activate {
                        self.activate_tab(ix, jobs);
                    }
                    match self.tabs[ix].data.path.clone() {
                        Some(path) => {
                            let format = self.tabs[ix].data.format;
                            self.begin_write_tab(ix, path, format, jobs);
                        }
                        None => self.begin_save_tab_as(id, jobs),
                    }
                    return None;
                }
                WorkflowStep::SaveTabAs(id) => {
                    self.begin_save_tab_as(id, jobs);
                    return None;
                }
                WorkflowStep::ConfirmDiscard { id, activate } => {
                    let Some(ix) = self.tab_index(id) else {
                        continue;
                    };
                    if activate {
                        self.activate_tab(ix, jobs);
                    }
                    let name = self.tabs[ix].data.path.as_deref().map_or_else(
                        || UNTITLED_FILE_NAME.into(),
                        |path| document::display_name(Some(path)),
                    );
                    self.show_file_prompt(FilePrompt::ConfirmDiscard { tab: id, name }, jobs);
                    return None;
                }
            }
        }
    }

    fn begin_save_tab_as(&mut self, id: TabId, jobs: &Jobs<AppEvent>) {
        let Some(ix) = self.tab_index(id) else {
            self.abort_file_workflow();
            return;
        };
        let current = self.tabs[ix].data.path.clone();
        let format = self.tabs[ix].data.format;
        files::spawn_save_dialog(self.id(), self.native.hwnd(), id, current, format, jobs);
    }

    fn begin_write_tab(&mut self, ix: usize, path: PathBuf, format: Format, jobs: &Jobs<AppEvent>) {
        let id = self.tabs[ix].data.id;
        let snapshot = self.tabs[ix].data.editor.begin_save();
        let text = self.tabs[ix].data.editor.text().clone();
        files::spawn_lossy_check(self.id(), id, snapshot, text, path, format, jobs);
    }

    pub(crate) fn on_save_dialog_done(
        &mut self,
        tab: TabId,
        format: Format,
        result: crate::platform::SaveDialogResult,
        jobs: &Jobs<AppEvent>,
    ) {
        let Some((path, next_format)) = files::resolve_save_dialog(format, result) else {
            self.abort_file_workflow();
            return;
        };
        match self.tab_index(tab) {
            Some(ix) => self.begin_write_tab(ix, path, next_format, jobs),
            None => self.abort_file_workflow(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn on_lossy_check_done(
        &mut self,
        tab: TabId,
        snapshot: u64,
        text: crate::editor::Rope,
        path: PathBuf,
        format: Format,
        result: std::io::Result<bool>,
        jobs: &Jobs<AppEvent>,
    ) {
        match result {
            Err(err) => {
                self.show_file_prompt(files::save_error_prompt(&path, &err), jobs);
                self.abort_file_workflow();
            }
            Ok(true) => {
                self.show_file_prompt(
                    FilePrompt::ConfirmLossyAnsi {
                        tab,
                        snapshot,
                        text,
                        path,
                        format,
                    },
                    jobs,
                );
            }
            Ok(false) => {
                files::spawn_write_file(self.id(), tab, snapshot, text, path, format, jobs);
            }
        }
    }

    pub(crate) fn on_file_saved(
        &mut self,
        tab: TabId,
        snapshot: u64,
        path: PathBuf,
        format: Format,
        result: std::io::Result<()>,
        jobs: &Jobs<AppEvent>,
    ) -> Option<TabId> {
        match result {
            Ok(()) => {
                let Some(ix) = self.tab_index(tab) else {
                    self.abort_file_workflow();
                    return None;
                };
                let t = &mut self.tabs[ix];
                t.data.editor.mark_saved_version(snapshot);
                t.data.path = Some(path);
                t.data.format = format;
                self.refresh_title();
                self.dirty = true;
                self.advance_file_workflow(jobs)
            }
            Err(err) => {
                self.show_file_prompt(files::save_error_prompt(&path, &err), jobs);
                self.abort_file_workflow();
                None
            }
        }
    }

    // ---- Dialogs & Find helpers ----

    fn show_dialog(&mut self, kind: DialogKind, jobs: &Jobs<AppEvent>) {
        self.cancel_menu(jobs);
        let next_focus = self.dialogs.show(kind);
        self.set_focus(next_focus, jobs);
        self.dirty = true;
    }

    fn show_file_prompt(&mut self, prompt: FilePrompt, jobs: &Jobs<AppEvent>) {
        self.show_dialog(DialogKind::from_file_prompt(prompt), jobs);
    }

    fn close_dialogs(&mut self, jobs: &Jobs<AppEvent>) {
        self.dialogs.hide();
        self.focus_active_editor(jobs);
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
        self.show_dialog(DialogKind::GoTo, jobs);
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
                self.active_editor_mut()
                    .set_cursor_position(Position::new((line - 1) as u32, 0));
                self.dirty = true;
            }
            _ => {
                self.show_dialog(DialogKind::LineOutOfRange, jobs);
            }
        }
    }

    fn press_dialog_button(
        &mut self,
        ix: usize,
        ts: &TextSystem,
        jobs: &Jobs<AppEvent>,
    ) -> Option<TabId> {
        let kind = self.dialogs.take_active()?;
        match kind {
            DialogKind::GoTo if ix == 0 => {
                self.submit_go_to(jobs);
                None
            }
            DialogKind::GoTo => {
                self.close_dialogs(jobs);
                None
            }
            DialogKind::LineOutOfRange => {
                self.show_go_to(None, ts, jobs);
                None
            }
            DialogKind::Error { .. } => {
                self.close_dialogs(jobs);
                None
            }
            DialogKind::ConfirmDiscard { tab, .. } => {
                self.close_dialogs(jobs);
                match ix {
                    0 => {
                        if let Some(wf) = self.file_workflow.as_mut() {
                            wf.prepend(WorkflowStep::SaveTab {
                                id: tab,
                                activate: false,
                            });
                        }
                        self.advance_file_workflow(jobs)
                    }
                    1 => self.advance_file_workflow(jobs),
                    _ => {
                        self.abort_file_workflow();
                        None
                    }
                }
            }
            DialogKind::ConfirmLossyAnsi {
                tab,
                snapshot,
                text,
                path,
                format,
            } => {
                self.close_dialogs(jobs);
                match ix {
                    0 => {
                        files::spawn_write_file(self.id(), tab, snapshot, text, path, format, jobs);
                        None
                    }
                    1 => {
                        self.begin_save_tab_as(tab, jobs);
                        None
                    }
                    _ => {
                        self.abort_file_workflow();
                        None
                    }
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

    fn find_step(&mut self, forward: bool, ts: &TextSystem, jobs: &Jobs<AppEvent>) {
        let wid = self.id();
        let searched = self
            .find
            .step(&mut self.tabs[self.active].data.editor, forward, wid, jobs);
        if !searched {
            self.open_find(false, ts, jobs);
            return;
        }
        if !self.find.is_open() {
            self.focus_active_editor(jobs);
        }
        self.dirty = true;
    }

    fn replace_next(&mut self, ts: &TextSystem, jobs: &Jobs<AppEvent>) {
        let wid = self.id();
        self.find
            .replace_next(&mut self.tabs[self.active].data.editor, wid, ts, jobs);
        self.dirty = true;
    }

    fn replace_all(&mut self, ts: &TextSystem, jobs: &Jobs<AppEvent>) {
        let wid = self.id();
        if self
            .find
            .replace_all(&mut self.tabs[self.active].data.editor, wid, ts, jobs)
        {
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

    // ---- Menu helpers ----

    pub(crate) fn key_tips(&self, ts: &TextSystem) -> Vec<KeyTip> {
        self.menu_bar.key_tips(&self.ui.frame, ts)
    }

    pub(crate) fn cancel_menu(&mut self, jobs: &Jobs<AppEvent>) {
        let out = self.menu_bar.cancel();
        if out.dirty {
            self.dirty = true;
        }
        if let Some(f) = out.focus {
            self.set_focus(f, jobs);
        } else if out.restore_editor_focus {
            let slot = EditorSlot::Tab(self.active_tab_id().0);
            self.set_focus(Focus::Editor(slot), jobs);
        }
    }

    pub(crate) fn on_menu_submenu_timer(&mut self, target: Option<usize>, ts: &TextSystem) {
        if self.menu_bar.on_submenu_timer(target, ts) {
            self.dirty = true;
        }
    }

    fn apply_menu_outcome(
        &mut self,
        out: MenuOutcome,
        view: ViewSettings,
        tab_dragging: bool,
        ts: &TextSystem,
        jobs: &Jobs<AppEvent>,
    ) -> (bool, Option<WindowAction>) {
        if out.dirty {
            self.dirty = true;
        }
        if let Some(f) = out.focus {
            self.set_focus(f, jobs);
        } else if out.restore_editor_focus {
            let slot = EditorSlot::Tab(self.active_tab_id().0);
            self.set_focus(Focus::Editor(slot), jobs);
        }
        if let Some(cmd) = out.command {
            let (_, action) = self.dispatch_command(cmd, view, tab_dragging, ts, jobs);
            return (true, action);
        }
        (out.handled, None)
    }

    // ---- Events ----

    pub(crate) fn on_activated(&mut self, active: bool, jobs: &Jobs<AppEvent>) {
        self.native.active = active;
        if !active {
            self.cancel_menu(jobs);
            self.stop_all_editor_blinks();
        } else if let Some(slot) = self.focused_editor_slot() {
            self.start_editor_blink(slot, jobs);
        }
        self.dirty = true;
    }

    pub(crate) fn on_mouse_move(
        &mut self,
        pos: Point,
        pressed_button: Option<MouseButton>,
        view: ViewSettings,
        ts: &TextSystem,
        jobs: &Jobs<AppEvent>,
    ) {
        self.pointer_hovered = true;
        self.last_mouse_pos = pos;
        let hover_changed = self.ui.update_hover(pos);
        if hover_changed {
            self.dirty = true;
        }

        let top = self.ui.top_hit();
        let wid = self.id();
        if !self.dialogs.is_open() {
            let menu_out = self.menu_bar.on_hover_change(
                wid,
                top,
                &self.tabs[self.active].data.editor,
                view,
                ts,
                jobs,
            );
            let _ = self.apply_menu_outcome(menu_out, view, false, ts, jobs);
        }

        if self.strip.is_frozen()
            && !self
                .ui
                .frame
                .rect_of(HitId::TabStrip)
                .is_some_and(|r| r.contains(pos))
            && self.strip.thaw(self.tabs.len())
        {
            self.dirty = true;
        }

        let dragging_left = pressed_button == Some(MouseButton::Left);
        let active_slot = EditorSlot::Tab(self.active_tab_id().0);
        if let Some(slot) = self.focused_editor_slot()
            && slot != active_slot
        {
            let mut timers = WindowEditorTimers::new(jobs, wid, slot);
            if let Some(ed) = self.editor_by_slot_mut(slot)
                && ed.on_mouse_move(pos, dragging_left, true, &mut timers)
            {
                self.dirty = true;
            }
        }
        let active_focused = self.focus == Focus::Editor(active_slot);
        if (!self.menu_bar.is_open() && !self.dialogs.is_open())
            || self.active_editor().is_selecting()
            || self.active_editor().is_scrollbar_dragging()
        {
            let mut timers = WindowEditorTimers::new(jobs, wid, active_slot);
            if self
                .active_editor_mut()
                .on_mouse_move(pos, dragging_left, active_focused, &mut timers)
            {
                self.dirty = true;
            }
        }

        let cursor = if !self.menu_bar.is_open()
            && !self.dialogs.is_open()
            && self.active_editor().hits_scrollbar_track(pos)
        {
            CursorStyle::Arrow
        } else {
            self.ui.frame.cursor_at(pos)
        };
        self.native.set_cursor(cursor);
    }

    pub(crate) fn on_mouse_leave(&mut self) {
        self.pointer_hovered = false;
        if self.ui.clear_hover() {
            self.dirty = true;
        }
        self.strip.stop_repeat();
        if self.active_editor_mut().on_mouse_leave() {
            self.dirty = true;
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn on_mouse_down(
        &mut self,
        button: MouseButton,
        pos: Point,
        modifiers: Modifiers,
        click_count: usize,
        view: ViewSettings,
        ts: &TextSystem,
        jobs: &Jobs<AppEvent>,
    ) -> Option<WindowAction> {
        self.pointer_hovered = true;
        self.last_mouse_pos = pos;
        if self.menu_bar.pointer_down() {
            self.dirty = true;
        }
        let top = self.ui.on_mouse_down(pos, button);
        let chain = self.ui.frame.hit_chain(pos);
        self.dirty = true;

        let in_menu_ui = chain.iter().any(|h| {
            matches!(
                h,
                HitId::MenuPanel | HitId::SubmenuPanel | HitId::MenuTrigger(_)
            )
        });
        if self.menu_bar.is_open() && !in_menu_ui {
            let out = self.menu_bar.close();
            let _ = self.apply_menu_outcome(out, view, false, ts, jobs);
        }

        if button == MouseButton::Middle && !self.dialogs.is_open() {
            self.middle_pressed_tab = chain.iter().find_map(|hit| match hit {
                HitId::Tab(raw) => Some(TabId(*raw)),
                _ => None,
            });
            return None;
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
                    | HitId::FindReplaceAll,
                ) => return None,
                Some(HitId::MenuTrigger(ix)) => {
                    if let Some(kind) = MenuKind::from_index(ix) {
                        let out = self.menu_bar.toggle_trigger(
                            kind,
                            &self.tabs[self.active].data.editor,
                            view,
                            ts,
                        );
                        let _ = self.apply_menu_outcome(out, view, false, ts, jobs);
                    }
                    return None;
                }
                Some(
                    HitId::MenuPanel
                    | HitId::MenuItem(_)
                    | HitId::MenuCommand(_)
                    | HitId::SubmenuPanel
                    | HitId::SubmenuItem(_),
                ) => return None,
                Some(HitId::TabClose(_)) => return None,
                Some(HitId::Tab(raw)) => {
                    let id = TabId(raw);
                    if let Some(ix) = self.tab_index(id) {
                        self.activate_tab(ix, jobs);
                        let left = self
                            .ui
                            .frame
                            .rect_of(HitId::Tab(raw))
                            .map_or(pos.x, |r| r.origin.x);
                        let grab = point(pos.x - left, pos.y - STRIP_TOP);
                        return Some(WindowAction::StartTabDrag {
                            tab: id,
                            index: ix,
                            grab,
                            position: pos,
                            alone: self.tabs.len() == 1,
                        });
                    }
                    return None;
                }
                Some(HitId::TabScrollLeft) => {
                    if self.scroll_tabs(-SCROLL_STEP) {
                        let wid = self.id();
                        self.strip.repeat = jobs.every(
                            SCROLL_REPEAT_DELAY,
                            SCROLL_REPEAT_INTERVAL,
                            move || AppEvent::TabScrollRepeat {
                                window: wid,
                                step: -SCROLL_STEP,
                            },
                        );
                    }
                    return None;
                }
                Some(HitId::TabScrollRight) => {
                    if self.scroll_tabs(SCROLL_STEP) {
                        let wid = self.id();
                        self.strip.repeat = jobs.every(
                            SCROLL_REPEAT_DELAY,
                            SCROLL_REPEAT_INTERVAL,
                            move || AppEvent::TabScrollRepeat {
                                window: wid,
                                step: SCROLL_STEP,
                            },
                        );
                    }
                    return None;
                }
                _ => {}
            }
        } else if in_menu_ui {
            return None;
        }

        if let Some(slot) = chain.iter().find_map(|hit| match hit {
            HitId::Editor(slot) => Some(*slot),
            _ => None,
        }) {
            self.set_focus(Focus::Editor(slot), jobs);
            let wid = self.id();
            if let Some(ed) = self.editor_by_slot_mut(slot) {
                let mut timers = WindowEditorTimers::new(jobs, wid, slot);
                let out = ed.on_mouse_down(button, pos, modifiers, click_count, &mut timers);
                if out.dirty {
                    self.dirty = true;
                }
            }
        }
        None
    }

    pub(crate) fn on_mouse_up(
        &mut self,
        button: MouseButton,
        pos: Point,
        view: ViewSettings,
        ts: &TextSystem,
        jobs: &Jobs<AppEvent>,
    ) -> Option<WindowAction> {
        self.last_mouse_pos = pos;
        let clicked = self.ui.on_mouse_up(pos, button);
        self.strip.stop_repeat();
        self.dirty = true;

        if button == MouseButton::Middle {
            let pressed = self.middle_pressed_tab.take();
            let over = self.ui.frame.hit_chain(pos).into_iter().find_map(|hit| {
                match hit {
                    HitId::Tab(raw) => Some(TabId(raw)),
                    _ => None,
                }
            });
            if let Some(id) = pressed
                && pressed == over
                && let Some(ix) = self.tab_index(id)
            {
                return self
                    .close_tab(ix, jobs)
                    .map(WindowAction::ForgetTabDrag);
            }
            return None;
        }

        let wid = self.id();
        let active_slot = EditorSlot::Tab(self.active_tab_id().0);
        if let Some(slot) = self.focused_editor_slot()
            && slot != active_slot
        {
            let mut timers = WindowEditorTimers::new(jobs, wid, slot);
            if let Some(ed) = self.editor_by_slot_mut(slot)
                && ed.on_mouse_up(button, pos, &mut timers).dirty
            {
                self.dirty = true;
            }
        }
        let open_ctx = {
            let mut timers = WindowEditorTimers::new(jobs, wid, active_slot);
            let out = self.active_editor_mut().on_mouse_up(button, pos, &mut timers);
            if out.dirty {
                self.dirty = true;
            }
            out.open_context_menu
        };
        if let Some(anchor) = open_ctx {
            let out = self.menu_bar.open_context_menu(
                anchor,
                &self.tabs[self.active].data.editor,
                false,
                ts,
            );
            let (_, action) = self.apply_menu_outcome(out, view, false, ts, jobs);
            return action;
        }

        match clicked {
            Some(HitId::DialogButton(ix)) => {
                return self
                    .press_dialog_button(ix, ts, jobs)
                    .map(WindowAction::ForgetTabDrag);
            }
            Some(HitId::FieldClear(slot)) => {
                if let Some(ed) = self.editor_by_slot_mut(slot) {
                    ed.set_value("", ts);
                }
                self.set_focus(Focus::Editor(slot), jobs);
                self.dirty = true;
            }
            Some(HitId::FindToggleReplace) => {
                self.find.replace = !self.find.replace;
                self.refocus_find(jobs);
            }
            Some(HitId::FindNext) => {
                self.find_step(true, ts, jobs);
            }
            Some(HitId::FindPrev) => {
                self.find_step(false, ts, jobs);
            }
            Some(HitId::FindOptions) => {
                if let Some(target) = self.ui.frame.rect_of(HitId::FindOptions) {
                    let entries = self.find.options_entries();
                    let out = self.menu_bar.open_flyout_above(
                        entries,
                        target,
                        Focus::Editor(EditorSlot::FindQuery),
                        ts,
                    );
                    let (_, action) = self.apply_menu_outcome(out, view, false, ts, jobs);
                    return action;
                }
            }
            Some(HitId::FindClose) => self.close_find(jobs),
            Some(HitId::FindReplace) => self.replace_next(ts, jobs),
            Some(HitId::FindReplaceAll) => self.replace_all(ts, jobs),
            Some(HitId::MenuItem(ix)) => {
                let out = self.menu_bar.click_item((ix, None), ts);
                let (_, action) = self.apply_menu_outcome(out, view, false, ts, jobs);
                return action;
            }
            Some(HitId::SubmenuItem(sub_ix)) => {
                let out = self.menu_bar.click_item((0, Some(sub_ix)), ts);
                let (_, action) = self.apply_menu_outcome(out, view, false, ts, jobs);
                return action;
            }
            Some(HitId::MenuCommand(ix)) => {
                let out = self.menu_bar.click_command(ix);
                let (_, action) = self.apply_menu_outcome(out, view, false, ts, jobs);
                return action;
            }
            Some(HitId::TabClose(raw)) => {
                if let Some(ix) = self.tab_index(TabId(raw)) {
                    return self
                        .close_tab(ix, jobs)
                        .map(WindowAction::ForgetTabDrag);
                }
            }
            Some(HitId::NewTab) => self.add_tab(view.word_wrap, jobs),
            Some(HitId::ThemeToggle) => return Some(WindowAction::ToggleTheme),
            _ => {}
        }
        None
    }

    pub(crate) fn on_scroll_wheel(
        &mut self,
        pos: Point,
        delta_lines: Point,
        modifiers: Modifiers,
    ) {
        if self.dialogs.is_open() {
            return;
        }
        let chain = self.ui.frame.hit_chain(pos);
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
            .on_scroll_wheel(delta_lines, modifiers, lh)
        {
            self.dirty = true;
        }
    }

    pub(crate) fn on_key_down(
        &mut self,
        ev: KeyEvent,
        view: ViewSettings,
        tab_dragging: bool,
        ts: &TextSystem,
        jobs: &Jobs<AppEvent>,
    ) -> (bool, Option<WindowAction>) {
        self.ui.last_input_was_keyboard = true;

        if self.dialogs.is_open() {
            if let Some(slot) = self.focused_editor_slot() {
                let wid = self.id();
                if let Some(ed) = self.editor_by_slot_mut(slot) {
                    let mut timers = WindowEditorTimers::new(jobs, wid, slot);
                    ed.pause_blink(&mut timers);
                    self.dirty = true;
                }
            }
            let m = ev.modifiers;
            match (m.ctrl, m.alt, m.shift, m.win, ev.key) {
                (false, false, false, false, Key::Enter) => {
                    let btn = match self.focus {
                        Focus::DialogButton(ix) => ix,
                        _ => 0,
                    };
                    let forgotten = self.press_dialog_button(btn, ts, jobs);
                    return (true, forgotten.map(WindowAction::ForgetTabDrag));
                }
                (false, false, false, false, Key::Space) => {
                    if let Focus::DialogButton(ix) = self.focus {
                        let forgotten = self.press_dialog_button(ix, ts, jobs);
                        return (true, forgotten.map(WindowAction::ForgetTabDrag));
                    }
                }
                (false, false, false, false, Key::Escape) => {
                    let last = self.dialogs.active_button_count().saturating_sub(1);
                    let forgotten = self.press_dialog_button(last, ts, jobs);
                    return (true, forgotten.map(WindowAction::ForgetTabDrag));
                }
                (false, false, false, false, Key::Tab) => {
                    if let Some(next) = self.dialogs.next_focus_step(self.focus, 1) {
                        self.set_focus(next, jobs);
                    }
                    return (true, None);
                }
                (false, false, true, false, Key::Tab) => {
                    if let Some(next) = self.dialogs.next_focus_step(self.focus, -1) {
                        self.set_focus(next, jobs);
                    }
                    return (true, None);
                }
                _ => {}
            }
            if self.focus == Focus::Editor(EditorSlot::GoToLine) {
                if let Some(cmd) = key_to_command(ev) {
                    let wid = self.id();
                    let mut timers = WindowEditorTimers::new(jobs, wid, EditorSlot::GoToLine);
                    if self.dialogs.line.handle_command(cmd, ts, &mut timers) {
                        self.dirty = true;
                        return (true, None);
                    }
                }
                return (false, None);
            }
            return (true, None);
        }

        let menu_out = self.menu_bar.on_key_down(
            ev,
            self.focus,
            &self.tabs[self.active].data.editor,
            view,
            ts,
        );
        if menu_out.handled {
            return self.apply_menu_outcome(menu_out, view, tab_dragging, ts, jobs);
        } else if menu_out.dirty {
            self.dirty = true;
        }

        if let Some(slot) = self.focused_editor_slot() {
            let wid = self.id();
            if let Some(ed) = self.editor_by_slot_mut(slot) {
                let mut timers = WindowEditorTimers::new(jobs, wid, slot);
                ed.pause_blink(&mut timers);
                self.dirty = true;
            }
        }

        // Single-line Find / Replace Enter shortcuts.
        if ev.key == Key::Enter && !ev.modifiers.ctrl && !ev.modifiers.alt && !ev.modifiers.win {
            match (self.focus, ev.modifiers.shift) {
                (Focus::Editor(EditorSlot::FindQuery), false) => {
                    let (_, action) =
                        self.dispatch_command(Command::FindNext, view, tab_dragging, ts, jobs);
                    return (true, action);
                }
                (Focus::Editor(EditorSlot::FindQuery), true) => {
                    let (_, action) =
                        self.dispatch_command(Command::FindPrevious, view, tab_dragging, ts, jobs);
                    return (true, action);
                }
                (Focus::Editor(EditorSlot::FindReplace), false) => {
                    let (_, action) =
                        self.dispatch_command(Command::ReplaceNext, view, tab_dragging, ts, jobs);
                    return (true, action);
                }
                _ => {}
            }
        }

        if let Some(slot) = self.focused_editor_slot()
            && let Some(cmd) = key_to_command(ev)
        {
            let wid = self.id();
            if let Some(ed) = self.editor_by_slot_mut(slot) {
                let mut timers = WindowEditorTimers::new(jobs, wid, slot);
                if ed.handle_command(cmd, ts, &mut timers) {
                    self.dirty = true;
                    return (true, None);
                }
            }
        }

        if let Some(cmd) = window_shortcut(ev) {
            let (handled, action) = self.dispatch_command(cmd, view, tab_dragging, ts, jobs);
            if handled {
                return (true, action);
            }
        }

        (false, None)
    }

    pub(crate) fn on_text_input(&mut self, text: &str, ts: &TextSystem, jobs: &Jobs<AppEvent>) {
        let Some(slot) = self.focused_editor_slot() else {
            return;
        };
        let wid = self.id();
        if let Some(ed) = self.editor_by_slot_mut(slot) {
            let mut timers = WindowEditorTimers::new(jobs, wid, slot);
            ed.handle_text_input(text, ts, &mut timers);
            self.dirty = true;
        }
    }

    pub(crate) fn on_ime_composition(&mut self, text: &str, cursor: usize, ts: &TextSystem) {
        let Some(slot) = self.focused_editor_slot() else {
            return;
        };
        if let Some(ed) = self.editor_by_slot_mut(slot) {
            ed.handle_ime_composition(text, cursor, ts);
            self.dirty = true;
        }
    }

    pub(crate) fn on_editor_timer(
        &mut self,
        slot: EditorSlot,
        kind: EditorTimer,
        jobs: &Jobs<AppEvent>,
    ) {
        let wid = self.id();
        if let Some(ed) = self.editor_by_slot_mut(slot) {
            let mut timers = WindowEditorTimers::new(jobs, wid, slot);
            if ed.on_timer(kind, &mut timers) {
                self.dirty = true;
            }
        }
    }

    pub(crate) fn dispatch_command(
        &mut self,
        cmd: Command,
        view: ViewSettings,
        tab_dragging: bool,
        ts: &TextSystem,
        jobs: &Jobs<AppEvent>,
    ) -> (bool, Option<WindowAction>) {
        match cmd {
            Command::Cancel => {
                if tab_dragging {
                    return (true, Some(WindowAction::CancelTabDrag));
                }
                if self.find.is_open() {
                    self.close_find(jobs);
                    return (true, None);
                }
                return (false, None);
            }
            Command::ShowContextMenu => {
                let active_slot = EditorSlot::Tab(self.active_tab_id().0);
                if self.focus != Focus::Editor(active_slot) {
                    return (false, None);
                }
                if let Some(caret) = self.active_editor().caret_bounds() {
                    let anchor = point(caret.right(), caret.bottom());
                    let out = self.menu_bar.open_context_menu(
                        anchor,
                        &self.tabs[self.active].data.editor,
                        true,
                        ts,
                    );
                    let _ = self.apply_menu_outcome(out, view, tab_dragging, ts, jobs);
                }
                return (true, None);
            }
            Command::NewWindow => return (true, Some(WindowAction::NewWindow)),
            Command::Exit => return (true, Some(WindowAction::Exit)),
            Command::ToggleStatusBar => return (true, Some(WindowAction::ToggleStatusBar)),
            Command::ToggleWordWrap => return (true, Some(WindowAction::ToggleWordWrap)),
            Command::ZoomIn => self.set_zoom(self.zoom() + ZOOM_STEP),
            Command::ZoomOut => self.set_zoom(self.zoom() - ZOOM_STEP),
            Command::ResetZoom => self.set_zoom(1.),
            Command::CloseWindow => {
                self.request_close_window(jobs);
            }
            Command::NewTab => self.add_tab(view.word_wrap, jobs),
            Command::CloseTab => {
                let forgotten = self.close_tab(self.active, jobs);
                return (true, forgotten.map(WindowAction::ForgetTabDrag));
            }
            Command::NextTab => self.next_tab(jobs),
            Command::PrevTab => self.prev_tab(jobs),
            Command::SelectTab(ix) => self.select_tab(ix, jobs),
            Command::SelectLastTab => self.select_last_tab(jobs),
            Command::Open => files::spawn_open_dialog(self.id(), jobs),
            Command::Save => self.save(jobs),
            Command::SaveAs => self.save_as(jobs),
            Command::SaveAll => self.save_all(jobs),
            Command::TimeDate => {
                let stamp = format_local_time_date();
                self.active_editor_mut().replace(&stamp, ts);
                self.dirty = true;
            }
            Command::GoTo => self.open_go_to(ts, jobs),
            Command::Find => {
                if self.has_text() {
                    self.open_find(false, ts, jobs);
                }
            }
            Command::Replace => {
                if self.has_text() {
                    self.open_find(true, ts, jobs);
                }
            }
            Command::FindNext => {
                if self.has_text() {
                    self.find_step(true, ts, jobs);
                }
            }
            Command::FindPrevious => {
                if self.has_text() {
                    self.find_step(false, ts, jobs);
                }
            }
            Command::ToggleMatchCase => {
                self.find.match_case = !self.find.match_case;
                self.refocus_find(jobs);
            }
            Command::ToggleWrapAround => {
                self.find.wrap_around = !self.find.wrap_around;
                self.refocus_find(jobs);
            }
            Command::ReplaceNext => self.replace_next(ts, jobs),
            other => {
                let wid = self.id();
                let active_slot = EditorSlot::Tab(self.active_tab_id().0);
                let mut timers = WindowEditorTimers::new(jobs, wid, active_slot);
                if self.active_editor_mut().handle_command(other, ts, &mut timers) {
                    self.dirty = true;
                    return (true, None);
                }
                return (false, None);
            }
        }
        (true, None)
    }

    // ---- Rendering & dynamic min-size ----

    fn status_metrics(&self) -> StatusMetrics {
        let editor = self.active_editor();
        let text = editor.text();
        let cursor = editor.cursor();
        let pt = text.offset_to_point(cursor);
        let column = text.byte_to_char_idx(cursor) - text.byte_to_char_idx(cursor - pt.column);
        let selection = editor.selected_range();
        let selected =
            text.byte_to_char_idx(selection.end) - text.byte_to_char_idx(selection.start);
        StatusMetrics {
            line: pt.row + 1,
            column: column + 1,
            selected,
            total: text.len_chars(),
            zoom_pct: (self.zoom() * 100.).round() as u32,
            format: self.tabs[self.active].data.format,
        }
    }

    fn update_min_size(
        &mut self,
        metrics: Option<StatusMetrics>,
        frame: &Frame,
        p: Palette,
        ts: &TextSystem,
    ) {
        let zoom_pct = metrics.map_or((self.zoom() * 100.).round() as u32, |m| m.zoom_pct);
        let scale = self.native.scale_factor;
        let menu_width = frame
            .rect_of(HitId::MenuBar)
            .map_or(0.0, |r| r.size.width);
        let key = MinSizeKey {
            status: metrics,
            zoom_pct,
            menu_width,
            scale,
        };
        if self
            .min_size_cache
            .is_some_and(|cached| cached.key == key)
        {
            return;
        }
        let status_sz = metrics.map(|m| Frame::measure_min_size(self.status_bar(m, p), scale, ts));
        let menu_row = MENU_BAR_LEFT + menu_width + THEME_BUTTON_WIDTH + THEME_BUTTON_RIGHT;
        let width = status_sz
            .map_or(0., |s| s.width)
            .max(menu_row)
            .max(tabs::TITLE_BAR_MIN_WIDTH);
        let rows = tabs::STRIP_TOP
            + tabs::TAB_HEIGHT
            + MENU_BAR_HEIGHT
            + SEPARATOR_HEIGHT
            + self.editor_min_height();
        let height = rows + status_sz.map_or(0., |s| s.height);
        let next = size(width, height);
        let changed = self
            .min_size_cache
            .is_none_or(|cached| cached.size != next);
        self.min_size_cache = Some(MinSizeCache { key, size: next });
        if changed {
            self.native.set_min_size(Some(next));
        }
    }

    pub(crate) fn render_frame(
        &mut self,
        p: Palette,
        view: ViewSettings,
        reduce_motion: bool,
        now: Instant,
        ts: &TextSystem,
        jobs: &Jobs<AppEvent>,
    ) -> Result<FrameResult> {
        self.dirty = false;
        let logical = self.native.logical_size;
        let scale = self.native.scale_factor;
        let window_active = self.native.active;
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
        let editor_node = self.editor_area(find_node, p);
        let status_node = status_metrics.map(|m| self.status_bar(m, p));
        let menu_popup = self.menu_bar.render_popup(logical, p, ts, &mut self.ui);
        let dialog_node = self.dialogs.render(self.focus, p, ts, &mut self.ui);

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
        self.update_min_size(status_metrics, &frame, p, ts);

        let editor_style = EditorPaintStyle {
            font: font("Consolas"),
            font_size: EDITOR_FONT_SIZE * self.zoom(),
            line_height: self.line_height(),
            color: rgba(p.text_primary),
        };
        let find_scale = self.find.scale();
        let find_field_style = EditorPaintStyle {
            font: font(TEXT_FONT),
            font_size: BODY_SIZE * find_scale,
            line_height: (BODY_LINE + 2.) * find_scale,
            color: rgba(p.text_primary),
        };
        let dialog_scale = self.dialogs.active_scale();
        let dialog_field_style = EditorPaintStyle {
            font: font(TEXT_FONT),
            font_size: BODY_SIZE * dialog_scale,
            line_height: (BODY_LINE + 2.) * dialog_scale,
            color: rgba(p.text_primary),
        };
        let caret_height = self.caret_height();
        let active_id = self.active_tab_id();
        let active_slot = EditorSlot::Tab(active_id.0);
        let editor_focused = self.focus == Focus::Editor(active_slot);
        let show_selection = editor_focused || self.menu_bar.is_open() || self.find.is_open();
        let caret_hidden = self.menu_bar.access_active();
        let wid = self.id();
        let mut editor_needs_redraw = false;
        let mut editor_needs_vsync = false;

        self.active_editor_mut().set_caret_height(caret_height);
        self.active_editor_mut().set_caret_hidden(caret_hidden);

        self.native.renderer.begin_frame(transparent_black());
        frame.paint(&mut self.native.renderer, &mut |token, rect, renderer| {
            match token {
                CustomDraw::Editor(slot) => match slot {
                    EditorSlot::Tab(raw) => {
                        if let Some(ix) = self.tabs.iter().position(|t| t.data.id == TabId(raw)) {
                            let mut timers = WindowEditorTimers::new(jobs, wid, slot);
                            let outcome = self.tabs[ix].data.editor.paint(
                                rect,
                                &editor_style,
                                p,
                                window_active,
                                editor_focused,
                                show_selection,
                                scale,
                                reduce_motion,
                                now,
                                ts,
                                renderer,
                                &mut timers,
                            );
                            editor_needs_redraw |= outcome.needs_redraw;
                            editor_needs_vsync |= outcome.needs_vsync;
                        }
                    }
                    EditorSlot::FindQuery => {
                        let focused = self.focus == Focus::Editor(slot);
                        let mut timers = WindowEditorTimers::new(jobs, wid, slot);
                        let outcome = self.find.query.paint(
                            rect,
                            &find_field_style,
                            p,
                            window_active,
                            focused,
                            focused,
                            scale,
                            reduce_motion,
                            now,
                            ts,
                            renderer,
                            &mut timers,
                        );
                        editor_needs_redraw |= outcome.needs_redraw;
                        editor_needs_vsync |= outcome.needs_vsync;
                    }
                    EditorSlot::FindReplace => {
                        let focused = self.focus == Focus::Editor(slot);
                        let mut timers = WindowEditorTimers::new(jobs, wid, slot);
                        let outcome = self.find.replacement.paint(
                            rect,
                            &find_field_style,
                            p,
                            window_active,
                            focused,
                            focused,
                            scale,
                            reduce_motion,
                            now,
                            ts,
                            renderer,
                            &mut timers,
                        );
                        editor_needs_redraw |= outcome.needs_redraw;
                        editor_needs_vsync |= outcome.needs_vsync;
                    }
                    EditorSlot::GoToLine => {
                        let focused = self.focus == Focus::Editor(slot);
                        let mut timers = WindowEditorTimers::new(jobs, wid, slot);
                        let outcome = self.dialogs.line.paint(
                            rect,
                            &dialog_field_style,
                            p,
                            window_active,
                            focused,
                            focused,
                            scale,
                            reduce_motion,
                            now,
                            ts,
                            renderer,
                            &mut timers,
                        );
                        editor_needs_redraw |= outcome.needs_redraw;
                        editor_needs_vsync |= outcome.needs_vsync;
                    }
                },
                CustomDraw::Scrollbar(Axis::Vertical) => {
                    self.tabs[self.active].data.editor.paint_scrollbar(renderer);
                }
                CustomDraw::Scrollbar(Axis::Horizontal) => {}
                CustomDraw::StatusSpacer => {
                    let width = rect.size.width;
                    if width < STATUS_DIVIDER_WIDTH + 0.5 {
                        let x = rect.origin.x + ((width - STATUS_DIVIDER_WIDTH) / 2.).round();
                        let line = Rect::new(
                            point(x, rect.origin.y),
                            size(STATUS_DIVIDER_WIDTH, rect.size.height),
                        );
                        renderer.paint_quad(fill(line, rgba(p.stroke)));
                    }
                }
            }
        });
        let device_lost = self.native.renderer.end_frame()?;

        let ui_animating = self.ui.finish_frame(frame);
        let needs_vsync = ui_animating || editor_needs_vsync || editor_needs_redraw;
        if needs_vsync {
            self.dirty = true;
        }
        Ok(FrameResult {
            needs_vsync,
            device_lost,
        })
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
        let menu = self.menu_bar.render_bar(self.focus, p, &mut self.ui);
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
                    .bg(rgba(p.layer))
                    .child(menu)
                    .child(box_().flex_1())
                    .child(theme_btn),
            )
            .child(separator(p))
    }

    fn editor_area(&self, find: Option<Node>, p: Palette) -> Node {
        let slot = EditorSlot::Tab(self.active_tab_id().0);
        let lh = self.line_height();
        let editor_view = self.active_editor().view(slot, lh);
        box_()
            .hit(HitId::EditorArea)
            .relative()
            .flex_1()
            .min_h(self.editor_min_height())
            .bg(rgb(p.editor))
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
                            .text_color(rgba(p.text_primary))
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
                .bg(rgba(p.layer))
                .font_family(SMALL_FONT)
                .text_size(12.)
                .text_color(rgba(p.text_secondary))
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
                .text_child(metrics.format.line_ending.label())
                .child(status_divider(p))
                .text_child(metrics.format.encoding.label()),
        )
    }
}

fn separator(p: Palette) -> Node {
    box_()
        .flex_none()
        .h(SEPARATOR_HEIGHT)
        .bg(rgba(p.stroke_on_layer))
}

fn status_divider(p: Palette) -> Node {
    box_()
        .flex_none()
        .w(STATUS_DIVIDER_WIDTH)
        .h(STATUS_DIVIDER_HEIGHT)
        .bg(rgba(p.stroke))
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
                CLOSE_HOVER,
                CLOSE_PRESSED,
                CLOSE_GLYPH_HOVER,
                CLOSE_GLYPH_PRESSED,
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
                hover: rgba(hover_bg),
                pressed: rgba(pressed_bg),
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
            .text_color(rgba(fg))
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

fn window_shortcut(ev: KeyEvent) -> Option<Command> {
    let m = ev.modifiers;
    match (m.ctrl, m.alt, m.shift, m.win, ev.key) {
        (false, false, false, false, Key::Escape) => Some(Command::Cancel),
        (true, false, false, false, Key::Char('n')) => Some(Command::NewTab),
        (true, false, true, false, Key::Char('n')) => Some(Command::NewWindow),
        (true, false, false, false, Key::Char('o')) => Some(Command::Open),
        (true, false, false, false, Key::Char('s')) => Some(Command::Save),
        (true, false, true, false, Key::Char('s')) => Some(Command::SaveAs),
        (true, true, false, false, Key::Char('s')) => Some(Command::SaveAll),
        (true, false, false, false, Key::Char('w')) => Some(Command::CloseTab),
        (true, false, true, false, Key::Char('w')) => Some(Command::CloseWindow),
        (true, false, false, false, Key::Tab | Key::PageDown) => Some(Command::NextTab),
        (true, false, true, false, Key::Tab) | (true, false, false, false, Key::PageUp) => {
            Some(Command::PrevTab)
        }
        (true, false, false, false, Key::Char('1')) => Some(Command::SelectTab(0)),
        (true, false, false, false, Key::Char('2')) => Some(Command::SelectTab(1)),
        (true, false, false, false, Key::Char('3')) => Some(Command::SelectTab(2)),
        (true, false, false, false, Key::Char('4')) => Some(Command::SelectTab(3)),
        (true, false, false, false, Key::Char('5')) => Some(Command::SelectTab(4)),
        (true, false, false, false, Key::Char('6')) => Some(Command::SelectTab(5)),
        (true, false, false, false, Key::Char('7')) => Some(Command::SelectTab(6)),
        (true, false, false, false, Key::Char('8')) => Some(Command::SelectTab(7)),
        (true, false, false, false, Key::Char('9')) => Some(Command::SelectLastTab),
        (true, false, false, false, Key::Char('f')) => Some(Command::Find),
        (false, false, false, false, Key::F(3)) => Some(Command::FindNext),
        (false, false, true, false, Key::F(3)) => Some(Command::FindPrevious),
        (true, false, false, false, Key::Char('h')) => Some(Command::Replace),
        (true, false, false, false, Key::Char('g')) => Some(Command::GoTo),
        (false, false, false, false, Key::F(5)) => Some(Command::TimeDate),
        (false, false, true, false, Key::F(10)) | (false, false, false, false, Key::Menu) => {
            Some(Command::ShowContextMenu)
        }
        (true, false, false | true, false, Key::Char('=' | '+')) => Some(Command::ZoomIn),
        (true, false, false, false, Key::Char('-')) => Some(Command::ZoomOut),
        (true, false, false, false, Key::Char('0')) => Some(Command::ResetZoom),
        _ => None,
    }
}
