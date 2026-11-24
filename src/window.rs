use std::time::Instant;

use crate::command::{AppCommand, Command, app_shortcut, edit_shortcut};
use crate::dialog::{DialogKey, DialogKind, Dialogs};
use crate::document::{self, DocPath, Format};
use crate::editor::{EditCommand, Editor, EditorPaintParams, EditorPaintStyle};
use crate::text::{Rope, RopeExt as _};
use crate::files::{FileRequest, Prompt, PromptId};
use crate::find::{FindBar, FindKey, FindOutcome};
use crate::geom::{Edges, Point, Rect, ScreenPoint, Size, point, size, transparent_black};
use crate::gfx::{TextStyle, TextSystem, WindowSurface, fill};
use crate::layout::AvailSize;
use crate::menu::{FocusRequest, MENU_BAR_HEIGHT, MenuOutcome, MenuState, Menus, RowPath};
use crate::platform::{
    KeyEvent, Modifiers, MouseButton, WindowAppearance, WindowControlArea, WindowHandle, WindowId,
    clipboard_has_text, format_local_time_date, read_clipboard_text, write_clipboard_text,
};
use crate::shell::{Cx, Effect, Shell, SlotTimers, WindowTimer};
use crate::tabs::{
    self, self as tab_strip, Removal, SCROLL_REPEAT_DELAY, SCROLL_REPEAT_INTERVAL, SCROLL_STEP,
    StripFrame, TAB_HEIGHT, TabData, TabSet, WHEEL_SHARE,
};
use crate::theme::{BODY_LINE, BODY_SIZE, ICON_FONT, Palette, SMALL_FONT, TEXT_FONT};
use crate::tooltip::{KeyTip, TooltipProbe};
use crate::types::APP_NAME;
use crate::ui::{
    AppFrame, AppNode, AppUi, Brushes, CaptionHit, CustomDraw, DialogHit, EditorSlot,
    FindHit, Focus, HitId, MenuHit, OverlayAnchor, TabHit, TabId, box_, custom, row, text,
};
use crate::widgets::icon_button;

pub(crate) const WINDOW_WIDTH: f32 = 2213.;
pub(crate) const WINDOW_HEIGHT: f32 = 1107.;
pub(crate) const WINDOW_MAX_SCREEN_SHARE: f32 = 0.9;

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
pub(crate) enum InputMode {
    Dialog,
    Popup,
    MenuBar,
    Normal,
}

pub(crate) struct MainWindow {
    pub(crate) win: WindowSurface,
    pub(crate) ui: AppUi,
    tabs: TabSet,
    pub(crate) menus: Menus,
    pub(crate) find: FindBar,
    pub(crate) dialogs: Dialogs,
    focus: Focus,
    pub(crate) dirty: bool,
    has_painted: bool,
    pub(crate) pending_close: bool,
    middle_pressed_tab: Option<TabId>,
    pub(crate) min_size: Option<Size>,
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

    pub(crate) fn on_paint_requested(&mut self) {
        if !self.has_painted {
            self.dirty = true;
        }
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

    pub(crate) fn id(&self) -> WindowId {
        self.win.window.id
    }

    pub(crate) fn hwnd(&self) -> WindowHandle {
        self.win.window.hwnd()
    }

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

    pub(crate) fn tabs(&self) -> &TabSet {
        &self.tabs
    }

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

    fn sync_cursor(&self, pos: Point) {
        self.win.window.set_cursor(self.ui.frame().cursor_at(pos));
    }

    #[cfg(test)]
    pub(crate) fn focus(&self) -> Focus {
        self.focus
    }

    pub(crate) fn active_tab_id(&self) -> TabId {
        self.tabs.active_id()
    }

    pub(crate) fn active_editor(&self) -> &Editor {
        &self.tabs.active().editor
    }

    pub(crate) fn active_editor_mut(&mut self) -> &mut Editor {
        &mut self.tabs.active_mut().editor
    }

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

    fn run_edit_command(
        &mut self,
        slot: EditorSlot,
        cmd: EditCommand,
        shell: &Shell,
    ) -> bool {
        match cmd {
            EditCommand::Copy => {
                if let Some(text) = self.editor_by_slot(slot).and_then(Editor::selected_string) {
                    write_clipboard_text(&text);
                }
                true
            }
            EditCommand::Cut => {
                let text = self
                    .editor_by_slot_mut(slot)
                    .and_then(Editor::prepare_cut);
                if let Some(text) = text
                    && write_clipboard_text(&text)
                {
                    self.with_editor(slot, shell, |ed, timers| {
                        ed.delete_selection(&shell.text_system, timers);
                    });
                }
                true
            }
            EditCommand::Paste => {
                if let Some(raw) = read_clipboard_text().filter(|s| !s.is_empty()) {
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

    fn with_editor<R>(
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

    fn line_height(&self) -> f32 {
        (EDITOR_LINE_HEIGHT * self.zoom()).round()
    }

    fn caret_height(&self) -> f32 {
        (CARET_HEIGHT * self.zoom()).round()
    }

    fn editor_min_height(&self) -> f32 {
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
                    self.tabs.repeat = None;
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

impl MainWindow {
    pub(crate) fn begin_tab_save(&mut self, tab: TabId) -> Option<(u64, Rope)> {
        let ed = self.tabs.editor_mut(tab)?;
        let snapshot = ed.begin_save();
        let text = ed.model().text().clone();
        Some((snapshot, text))
    }

    pub(crate) fn refresh_title(&self) {
        self.win.window.set_title(&format!(
            "{} - {APP_NAME}",
            self.tabs.tab_name(self.tabs.active_index())
        ));
    }

    pub(crate) fn zoom(&self) -> f32 {
        self.tabs.active().zoom
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
            self.win.window.hwnd().complete_ime_composition();
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
                .rect_of(HitId::Tab(TabHit::Strip))
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

    fn scroll_tabs(&mut self, delta: f32) -> bool {
        let moved = self.tabs.scroll_by(delta);
        self.dirty = true;
        moved
    }

    fn step_tab(&mut self, forward: bool, shell: &Shell) {
        let n = self.tabs.len();
        if n > 1 {
            let next = (self.tabs.active_index() + if forward { 1 } else { n - 1 }) % n;
            self.activate_tab(next, shell);
        }
    }

    fn select_tab(&mut self, ix: usize, shell: &Shell) {
        if ix >= self.tabs.len() || ix == self.tabs.active_index() {
            return;
        }
        self.activate_tab(ix, shell);
    }

    fn select_last_tab(&mut self, shell: &Shell) {
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
}

impl MainWindow {
    pub(crate) fn dispatch_command(&mut self, cmd: Command, cx: &mut Cx<'_>) -> bool {
        match cmd {
            Command::App(app_cmd) => self.dispatch_app_command(app_cmd, cx),
            Command::Edit(edit_cmd) => {
                self.run_edit_command(self.active_slot(), edit_cmd, cx.shell)
            }
        }
    }

    fn dispatch_app_command(&mut self, cmd: AppCommand, cx: &mut Cx<'_>) -> bool {
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
                    let state = menu_state(&self.tabs, cx.shell);
                    let out = self.menus.open_context_menu(anchor, state);
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
            AppCommand::NextTab => self.step_tab(true, cx.shell),
            AppCommand::PrevTab => self.step_tab(false, cx.shell),
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
            AppCommand::Find | AppCommand::Replace if self.has_text() => {
                self.open_find(cmd == AppCommand::Replace, cx.shell);
            }
            AppCommand::FindNext | AppCommand::FindPrevious if self.has_text() => {
                self.find_step(cmd == AppCommand::FindNext, cx.shell);
            }
            AppCommand::Find
            | AppCommand::Replace
            | AppCommand::FindNext
            | AppCommand::FindPrevious => {}
            AppCommand::ToggleMatchCase => {
                self.find.match_case = !self.find.match_case;
                self.refocus_find(cx.shell);
            }
            AppCommand::ToggleWrapAround => {
                self.find.wrap_around = !self.find.wrap_around;
                self.refocus_find(cx.shell);
            }
            AppCommand::ReplaceNext => self.replace_next(cx.shell),
        }
        true
    }
}

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

    fn finish_dialog(&mut self, button: Option<usize>, cx: &mut Cx<'_>) {
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

impl MainWindow {
    fn open_find(&mut self, replace: bool, shell: &Shell) {
        let selected = self.active_editor().model().selected_text().to_string();
        self.find.open(replace, &selected, &shell.text_system);
        self.set_focus(Focus::Editor(EditorSlot::FindQuery), shell);
        self.dirty = true;
    }

    fn close_find(&mut self, shell: &Shell) {
        if self.find.close() {
            self.focus_active_editor(shell);
            self.dirty = true;
        }
    }

    fn refocus_find(&mut self, shell: &Shell) {
        if self.find.is_open() {
            let slot =
                if self.find.replace && self.focus == Focus::Editor(EditorSlot::FindReplace) {
                    EditorSlot::FindReplace
                } else {
                    EditorSlot::FindQuery
                };
            self.set_focus(Focus::Editor(slot), shell);
        }
        self.dirty = true;
    }

    fn find_step(&mut self, forward: bool, shell: &Shell) {
        let timers = shell.window_timers(self.id());
        let out = self
            .find
            .step(&mut self.tabs.active_mut().editor, forward, timers);
        if matches!(out, FindOutcome::EmptyQuery) {
            self.open_find(false, shell);
            return;
        }
        if !self.find.is_open() {
            self.focus_active_editor(shell);
        }
        self.dirty = true;
    }

    fn replace_next(&mut self, shell: &Shell) {
        let timers = shell.window_timers(self.id());
        self.find.replace_next(
            &mut self.tabs.active_mut().editor,
            &shell.text_system,
            timers,
        );
        self.dirty = true;
    }

    fn replace_all(&mut self, shell: &Shell) {
        let timers = shell.window_timers(self.id());
        let out = self.find.replace_all(
            &mut self.tabs.active_mut().editor,
            &shell.text_system,
            timers,
        );
        if matches!(out, FindOutcome::Replaced(_)) {
            self.refocus_find(shell);
        } else {
            self.dirty = true;
        }
    }
}

impl MainWindow {
    pub(crate) fn key_tips_active(&self) -> bool {
        self.menus.access_active(self.focus)
    }

    pub(crate) fn cancel_menu(&mut self, shell: &Shell) {
        let out = self.menus.cancel(self.focus);
        self.dirty |= out.dirty;
        self.apply_focus_request(out.focus, shell);
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
        has_selection: !m.selected_range().is_empty(),
        has_text: m.text().len() > 0,
        clipboard_has_text: clipboard_has_text(),
        status_bar: shell.view.status_bar,
        word_wrap: shell.view.word_wrap,
    }
}

impl MainWindow {
    pub(crate) fn on_mouse_move(
        &mut self,
        pos: Point,
        pressed_button: Option<MouseButton>,
        cx: &mut Cx<'_>,
    ) {
        self.dirty |= self.ui.on_mouse_move(pos, pressed_button.is_some());

        let top = self.ui.top_hit();
        if !matches!(top, Some(HitId::Tab(TabHit::ScrollLeft | TabHit::ScrollRight))) {
            self.tabs.repeat = None;
        }
        let mode = self.mode();
        if mode != InputMode::Dialog {
            let timers = cx.timers(self.id());
            let tabs = &self.tabs;
            let shell = cx.shell;
            let menu_out =
                self.menus
                    .on_hover_change(top, || menu_state(tabs, shell), &mut self.ui, timers);
            let _ = self.apply_menu_outcome(menu_out, cx);
        }

        if self.tabs.is_frozen()
            && !self
                .ui
                .frame()
                .rect_of(HitId::Tab(TabHit::Strip))
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
        self.tabs.repeat = None;
        if self.tabs.is_frozen() {
            self.dirty |= self.tabs.thaw_if_frozen();
        }
    }

    pub(crate) fn on_capture_lost(&mut self) {
        self.middle_pressed_tab = None;
        self.tabs.repeat = None;
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
        let menu_out = self.menus.pointer_down(self.focus);
        let _ = self.apply_menu_outcome(menu_out, cx);
        let chain = self.ui.on_mouse_down(pos, button);
        let top = chain.first().copied();
        let in_menu_ui = chain.iter().any(|h| {
            matches!(
                h,
                HitId::Menu(MenuHit::Panel | MenuHit::SubmenuPanel | MenuHit::Trigger(_))
            )
        });
        let tab_hit = chain.iter().find_map(|hit| match hit {
            HitId::Tab(TabHit::Item(id)) => Some(*id),
            _ => None,
        });
        let editor_hit = chain.iter().find_map(|hit| match hit {
            HitId::Editor(slot) => Some(*slot),
            _ => None,
        });

        let mode = self.mode();
        if mode == InputMode::Popup && !in_menu_ui {
            let out = self.menus.close();
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
                    | HitId::Dialog(_)
                    | HitId::Find(_)
                    | HitId::Tab(TabHit::Close(_)),
                ) => return,
                Some(HitId::Menu(m)) if m.is_popup() => return,
                Some(HitId::Menu(MenuHit::Trigger(kind))) => {
                    let state = menu_state(&self.tabs, cx.shell);
                    let out = self.menus.toggle_trigger(kind, state);
                    let _ = self.apply_menu_outcome(out, cx);
                    return;
                }
                Some(HitId::Tab(TabHit::Item(id))) => {
                    if let Some(ix) = self.tabs.index_of(id) {
                        self.activate_tab(ix, cx.shell);
                        let left = self
                            .ui
                            .frame()
                            .rect_of(HitId::Tab(TabHit::Item(id)))
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
                Some(HitId::Tab(hit @ (TabHit::ScrollLeft | TabHit::ScrollRight))) => {
                    let step = if hit == TabHit::ScrollLeft { -SCROLL_STEP } else { SCROLL_STEP };
                    if self.scroll_tabs(step) {
                        self.tabs.repeat = Some(cx.timers(self.id()).every(
                            SCROLL_REPEAT_DELAY,
                            SCROLL_REPEAT_INTERVAL,
                            WindowTimer::TabScrollRepeat(step),
                        ));
                    }
                    return;
                }
                Some(HitId::ScrollThumb(slot, axis) | HitId::ScrollTrack(slot, axis))
                    if slot == self.active_slot()
                        && self.active_editor().scrollbar_interactive(axis) =>
                {
                    let thumb = matches!(top, Some(HitId::ScrollThumb(..)));
                    if let Some(track) = self.ui.frame().rect_of(HitId::ScrollTrack(slot, axis)) {
                        self.active_editor_mut()
                            .on_scrollbar_mouse_down(axis, thumb, pos, track);
                    }
                    return;
                }
                _ => {}
            }
        } else if in_menu_ui {
            return;
        }

        if let Some(slot) = editor_hit {
            self.win.window.hwnd().complete_ime_composition();
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
            HitId::Tab(TabHit::Item(id)) => Some(*id),
            _ => None,
        });
        let menu_release = if button == MouseButton::Left
            && clicked.is_none()
            && self.mode() == InputMode::Popup
        {
            match top {
                hit @ Some(HitId::Menu(
                    MenuHit::Item { .. } | MenuHit::SubmenuItem { .. } | MenuHit::Command { .. },
                )) => hit,
                _ => None,
            }
        } else {
            None
        };
        let activated = clicked.or(menu_release);
        self.tabs.repeat = None;

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
            let state = menu_state(&self.tabs, cx.shell);
            let out = self.menus.open_context_menu(anchor, state);
            let _ = self.apply_menu_outcome(out, cx);
            return;
        }

        match activated {
            Some(HitId::Dialog(DialogHit::Button(ix))) => self.finish_dialog(Some(ix), cx),
            Some(HitId::FieldClear(slot)) => {
                if let Some(ed) = self.editor_by_slot_mut(slot) {
                    ed.set_value("", cx.ts());
                }
                self.set_focus(Focus::Editor(slot), cx.shell);
            }
            Some(HitId::Find(FindHit::ToggleReplace)) => {
                self.find.replace = !self.find.replace;
                self.refocus_find(cx.shell);
            }
            Some(HitId::Find(FindHit::Next)) => self.find_step(true, cx.shell),
            Some(HitId::Find(FindHit::Prev)) => self.find_step(false, cx.shell),
            Some(HitId::Find(FindHit::Options)) => {
                let out = self
                    .menus
                    .open_find_options(self.find.match_case, self.find.wrap_around);
                let _ = self.apply_menu_outcome(out, cx);
            }
            Some(HitId::Find(FindHit::Close)) => self.close_find(cx.shell),
            Some(HitId::Find(FindHit::Replace)) => self.replace_next(cx.shell),
            Some(HitId::Find(FindHit::ReplaceAll)) => self.replace_all(cx.shell),
            Some(HitId::Menu(MenuHit::Item { ix, .. })) => {
                let out = self.menus.activate(RowPath::Root(ix), false);
                let _ = self.apply_menu_outcome(out, cx);
            }
            Some(HitId::Menu(MenuHit::SubmenuItem { ix: sub_ix, .. })) => {
                let out = self.menus.activate(RowPath::Sub(sub_ix), false);
                let _ = self.apply_menu_outcome(out, cx);
            }
            Some(HitId::Menu(MenuHit::Command { ix, .. })) => {
                let out = self.menus.click_command(ix);
                let _ = self.apply_menu_outcome(out, cx);
            }
            Some(HitId::Tab(TabHit::Close(id))) => {
                if self.tabs.index_of(id).is_some() {
                    cx.emit(Effect::File(FileRequest::CloseTab {
                        window: self.id(),
                        tab: id,
                    }));
                }
            }
            Some(HitId::Tab(TabHit::New)) => self.add_tab(cx.shell),
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
        if self.tabs.overflow() && chain.contains(&HitId::Tab(TabHit::List)) {
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
        self.ui.last_input_was_keyboard = true;

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

        let was_popup = self.menus.is_open();
        let tabs = &self.tabs;
        let shell = cx.shell;
        let out = self
            .menus
            .on_key_down(ev, self.focus, || menu_state(tabs, shell));
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
        let menu_out = self.menus.on_key_up(ev, self.focus);
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

const HEADER_WIDTH: f32 = 5.;
const STRIP_TOP: f32 = 10.;
const CAPTION_BUTTON_WIDTH: f32 = 46.;
const CAPTION_WIDTH: f32 = 3. * CAPTION_BUTTON_WIDTH;
const DRAG_RESERVE: f32 = 43.;
const TITLE_BAR_MIN_WIDTH: f32 =
    HEADER_WIDTH + CAPTION_WIDTH + DRAG_RESERVE + tabs::MIN_STRIP_WIDTH;

fn tab_strip_column(viewport_width: f32) -> f32 {
    viewport_width - HEADER_WIDTH - CAPTION_WIDTH - DRAG_RESERVE - tabs::STRIP_END_RESERVE
}

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

#[derive(Clone, Copy, PartialEq, Eq)]
struct StatusMetrics {
    line: usize,
    column: usize,
    selected: usize,
    total: usize,
    zoom_pct: u32,
    format: Format,
}

impl MainWindow {
    fn estimated_editor_rect(&self, logical: Size, show_status_bar: bool) -> Rect {
        let top_chrome = STRIP_TOP + TAB_HEIGHT + MENU_BAR_HEIGHT + SEPARATOR_HEIGHT;
        let bottom_chrome = if show_status_bar {
            SEPARATOR_HEIGHT + STATUS_BAR_HEIGHT
        } else {
            0.
        };
        let area_h = (logical.height - top_chrome - bottom_chrome).max(self.editor_min_height());
        Rect::new(
            point(EDITOR_PADDINGS.left, top_chrome + EDITOR_PADDINGS.top),
            size(
                (logical.width - EDITOR_PADDINGS.left - EDITOR_PADDINGS.right).max(0.),
                (area_h - EDITOR_PADDINGS.top - EDITOR_PADDINGS.bottom).max(0.),
            ),
        )
    }

    fn status_metrics(&self) -> StatusMetrics {
        let model = self.active_editor().model();
        let text = model.text();
        let (line, column) = model.caret_line_col();
        let selection = model.selected_range();
        let selected = text
            .byte_to_char_idx(selection.end)
            .saturating_sub(text.byte_to_char_idx(selection.start));
        StatusMetrics {
            line,
            column,
            selected,
            total: text.len_chars(),
            zoom_pct: (self.zoom() * 100.).round() as u32,
            format: self.tabs.active().format,
        }
    }

    fn update_min_size(
        &mut self,
        logical: Size,
        metrics: Option<StatusMetrics>,
        frame: &AppFrame,
        p: Palette,
        ts: &TextSystem,
    ) {
        let status_w = match metrics {
            Some(m) => {
                let spacer_w = frame
                    .custom_rects()
                    .find_map(|(tok, r)| (tok == CustomDraw::StatusSpacer).then_some(r.size.width));
                match spacer_w {
                    Some(w) if w > STATUS_DIVIDER_WIDTH + 0.5 => {
                        logical.width - (w - STATUS_DIVIDER_WIDTH)
                    }
                    _ => AppFrame::measure(
                        self.status_bar(m, p),
                        AvailSize::MIN,
                        self.win.window.scale_factor(),
                        ts,
                    )
                    .width,
                }
            }
            None => 0.0,
        };
        let status_h = if metrics.is_some() {
            SEPARATOR_HEIGHT + STATUS_BAR_HEIGHT
        } else {
            0.0
        };
        let menu_width = frame
            .rect_of(HitId::Menu(MenuHit::Bar))
            .map_or(0.0, |r| r.size.width);
        let menu_row = MENU_BAR_LEFT + menu_width + THEME_BUTTON_WIDTH + THEME_BUTTON_RIGHT;
        let width = status_w.max(menu_row).max(TITLE_BAR_MIN_WIDTH);
        let chrome_top = frame.rect_of(HitId::EditorArea).map_or(0., |r| r.top());
        let height = chrome_top + self.editor_min_height() + status_h;
        let next = size(width, height);
        if self.min_size.is_none_or(|prev| {
            (prev.width - next.width).abs() > 0.5 || (prev.height - next.height).abs() > 0.5
        }) {
            self.min_size = Some(next);
            self.win.window.set_min_size(Some(next));
        }
    }

    fn title_bar(
        &mut self,
        strip: &StripFrame,
        window_active: bool,
        maximized: bool,
        p: Palette,
    ) -> AppNode {
        let strip_node = self.tabs.render(strip, p, &mut self.ui);
        let captions = caption_buttons(window_active, maximized, p, &mut self.ui);
        row()
            .flex_none()
            .h(STRIP_TOP + TAB_HEIGHT)
            .child(
                box_()
                    .flex_none()
                    .w(HEADER_WIDTH)
                    .h_full()
                    .window_area(WindowControlArea::Drag),
            )
            .child(
                box_()
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

    fn menu_bar_row(&mut self, p: Palette) -> AppNode {
        let menu = self.menus.render_bar(self.focus, p, &mut self.ui);
        let theme_btn = icon_button(
            p,
            HitId::ThemeToggle,
            "Switch theme",
            (if p.dark { SUN_GLYPH } else { MOON_GLYPH }, THEME_GLYPH_SIZE),
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

    fn editor_area(
        &mut self,
        find: Option<AppNode>,
        now: Instant,
        shell: &Shell,
    ) -> AppNode {
        let p = shell.palette;
        let slot = self.active_slot();
        let lh = self.line_height();
        let wid = self.id();
        let mut timers = shell.window_timers(wid).for_editor(slot);
        let scrollbars = self
            .tabs
            .active_mut()
            .editor
            .render_scrollbars(slot, p, now, &mut self.ui, &mut timers);
        let editor_view = self.tabs.active().editor.view_with_scrollbars(
            HitId::Editor(slot),
            CustomDraw::Editor(slot),
            lh,
            scrollbars,
        );
        box_()
            .hit(HitId::EditorArea)
            .flex_1()
            .min_h(self.editor_min_height())
            .bg(p.editor)
            .child(
                box_()
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

    #[cfg(test)]
    pub(crate) fn status_bar_for_test(&self, p: Palette) -> AppNode {
        self.status_bar(self.status_metrics(), p)
    }

    fn status_bar(&self, metrics: StatusMetrics, p: Palette) -> AppNode {
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

fn separator(p: Palette) -> AppNode {
    box_()
        .flex_none()
        .h(SEPARATOR_HEIGHT)
        .bg(p.stroke_on_layer)
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

#[derive(Clone, Copy)]
enum Caption {
    Minimize,
    Maximize,
    Restore,
    Close,
}

impl Caption {
    fn hit(self) -> HitId {
        HitId::Caption(match self {
            Self::Minimize => CaptionHit::Min,
            Self::Maximize | Self::Restore => CaptionHit::Max,
            Self::Close => CaptionHit::Close,
        })
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

    fn render(self, active: bool, p: Palette, ui: &mut AppUi) -> AppNode {
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
                    .when(matches!(self, Self::Minimize), |this| {
                        this.left(1.).top(1.)
                    })
                    .child(text(self.glyph())),
            )
    }
}

fn caption_buttons(active: bool, maximized: bool, p: Palette, ui: &mut AppUi) -> AppNode {
    let maximize = if maximized {
        Caption::Restore
    } else {
        Caption::Maximize
    };
    box_()
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

impl MainWindow {
    pub(crate) fn render_frame(&mut self, now: Instant, shell: &Shell) -> bool {
        let p = shell.palette;
        let view = shell.view;
        let reduce_motion = shell.settings.reduce_motion;
        let caret_width_px = shell.settings.caret_width_px;
        let ts = &shell.text_system;
        self.dirty = false;
        let logical = self.win.window.logical_size();
        let scale = self.win.window.scale_factor();
        let window_active = self.win.window.is_active();
        let maximized = self.win.window.is_maximized();
        let status_metrics = view.status_bar.then(|| self.status_metrics());

        self.ui.begin_frame(now, reduce_motion);
        let strip = self
            .tabs
            .sample(tab_strip_column(logical.width), &mut self.ui);
        self.menus.sample(&mut self.ui);

        let editor_style = EditorPaintStyle {
            text: TextStyle::regular("Consolas", EDITOR_FONT_SIZE * self.zoom()),
            line_height: self.line_height(),
            color: p.text_primary,
        };
        let field_style = EditorPaintStyle {
            text: TextStyle::regular(TEXT_FONT, BODY_SIZE),
            line_height: BODY_LINE + 2.,
            color: p.text_primary,
        };
        let caret_height = self.caret_height();
        let active_slot = self.active_slot();
        let editor_focused = self.focus == Focus::Editor(active_slot);
        let show_selection = editor_focused || self.menus.is_open() || self.find.is_open();
        let caret_hidden = self.key_tips_active();

        // Prepare the active document before building scrollbars so thumb metrics never lag a frame.
        let estimated_editor_rect = self.estimated_editor_rect(logical, view.status_bar);
        let active_params = EditorPaintParams {
            style: &editor_style,
            palette: p,
            window_active,
            focused: editor_focused,
            show_selection,
            scale,
            caret_width_px,
        };
        self.active_editor_mut().caret_height = caret_height;
        self.active_editor_mut().caret_hidden = caret_hidden;
        let mut editor_needs_redraw =
            self.active_editor_mut()
                .prepare(estimated_editor_rect, active_params, ts);

        let title_bar = self.title_bar(&strip, window_active, maximized, p);
        let menu_bar = self.menu_bar_row(p);
        let find_node = self.find.render(self.focused_editor_slot(), p, &mut self.ui);
        let editor_node = self.editor_area(find_node, now, shell);
        let status_node = status_metrics.map(|m| self.status_bar(m, p));
        let menu_popup = self.menus.render_popup(p, &mut self.ui);
        let dialog_node = self.dialogs.render(self.focus, p, &mut self.ui);

        let chrome = box_()
            .size_full()
            .child(title_bar)
            .child(menu_bar)
            .child(editor_node)
            .when_some(status_node, |this, s| this.child(s));
        let root = box_()
            .size_full()
            .child(chrome)
            .when_some(menu_popup, |this, m| this.child(m))
            .when_some(dialog_node, |this, d| this.child(d));

        let frame = AppFrame::layout(
            root,
            AvailSize::definite(logical.width, logical.height),
            scale,
            ts,
        );
        self.win.window.set_control_areas(frame.control_areas());
        self.update_min_size(logical, status_metrics, &frame, p, ts);

        let editor_rects: Vec<(EditorSlot, Rect)> = frame
            .custom_rects()
            .filter_map(|(token, rect)| match token {
                CustomDraw::Editor(slot) => Some((slot, rect)),
                _ => None,
            })
            .collect();
        let focus = self.focus;
        for (slot, rect) in editor_rects {
            if slot == active_slot {
                if rect != estimated_editor_rect {
                    editor_needs_redraw |=
                        self.active_editor_mut().prepare(rect, active_params, ts);
                    let size_diff = (rect.size.width - estimated_editor_rect.size.width).abs() > 1.0
                        || (rect.size.height - estimated_editor_rect.size.height).abs() > 1.0;
                    editor_needs_redraw |= size_diff;
                }
                continue;
            }
            let focused = focus == Focus::Editor(slot);
            if let Some(ed) = self.editor_by_slot_mut(slot) {
                editor_needs_redraw |= ed.prepare(
                    rect,
                    EditorPaintParams {
                        style: &field_style,
                        palette: p,
                        window_active,
                        focused,
                        show_selection: focused,
                        scale,
                        caret_width_px,
                    },
                    ts,
                );
            }
        }

        let tabs = &self.tabs;
        let find = &self.find;
        let dialogs = &self.dialogs;
        let needs_retry = self.win.surface.draw(p.window_clear, |renderer| {
            frame.paint(renderer, &mut |token, rect, renderer| match token {
                CustomDraw::Editor(slot) => {
                    let ed = match slot {
                        EditorSlot::Tab(id) => tabs.editor(id),
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
}
