use std::borrow::Cow;
use std::ops::Range;
use std::time::{Duration, Instant};

use crate::geom::{Axis, Edges, Point, Rect, Rgba, Size, point, size};
use crate::gfx::{Canvas, TextStyle, TextSystem};
use crate::platform::{CursorStyle, Modifiers, MouseButton, TimerHandle};
use crate::text::{
    Bias, EditPolicy, LineChange, Rope, RopeExt as _, TextModel, ValidateFn,
    next_grapheme_boundary, next_word_end, prev_grapheme_boundary, prev_word_start,
};
use crate::theme::Palette;
use crate::types::SharedStr;
use crate::ui::{AnimKey, AppNode, AppUi, Easing, EditorSlot, HitId, Node, box_, custom, row};
use crate::wrap::{FrameLayout, LineRanges, RowLayout, VisibleLine, VisualRow, WrapIndex};

const RIGHT_MARGIN: f32 = 10.0;

#[derive(Clone, Copy, Debug)]
pub(crate) struct DocOptions {
    pub(crate) soft_wrap: bool,
    pub(crate) paddings: Edges<f32>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct FieldOptions {
    pub(crate) placeholder: SharedStr,
    pub(crate) caret_height: f32,
    pub(crate) validate: Option<ValidateFn>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum MoveDirection {
    Up,
    Down,
}

#[derive(Clone, Copy, Debug)]
struct RevealRequest {
    offset: usize,
    end_affinity: bool,
    direction: Option<MoveDirection>,
    with_margin: bool,
}

#[derive(Copy, Clone, Debug, Default)]
struct Scroll {
    x: f32,
    y: f64,
    content_w: f32,
    content_h: f64,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum EditorTimer {
    Blink,
    AutoScroll,
    ScrollbarIdle,
    ScrollbarDrag,
}

pub(crate) trait EditorTimers {
    fn after(&mut self, delay: Duration, kind: EditorTimer) -> Option<TimerHandle>;
    fn every(
        &mut self,
        delay: Duration,
        interval: Duration,
        kind: EditorTimer,
    ) -> Option<TimerHandle>;
    fn blink_timing(&self) -> (Option<Duration>, Option<Duration>);
}

#[cfg(test)]
pub(crate) struct NoopTimers;

#[cfg(test)]
impl EditorTimers for NoopTimers {
    fn after(&mut self, _delay: Duration, _kind: EditorTimer) -> Option<TimerHandle> {
        None
    }

    fn every(
        &mut self,
        _delay: Duration,
        _interval: Duration,
        _kind: EditorTimer,
    ) -> Option<TimerHandle> {
        None
    }

    fn blink_timing(&self) -> (Option<Duration>, Option<Duration>) {
        (None, None)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Motion {
    Left,
    Right,
    Up,
    Down,
    LineStart,
    LineEnd,
    PageUp,
    PageDown,
    DocStart,
    DocEnd,
    PrevWord,
    NextWord,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum EditCommand {
    Enter,
    Backspace,
    Delete,
    DeletePrevWord,
    DeleteNextWord,
    Indent,
    Outdent,
    IndentInline,
    Move { motion: Motion, extend: bool },
    SelectAll,
    Copy,
    Cut,
    Paste,
    Undo,
    Redo,
    Escape,
}

struct DocumentView {
    wrap: WrapIndex,
    soft_wrap: bool,
    last_line_height: f32,
    paddings: Edges<f32>,
    scrollbar: ScrollbarState,
    auto_scroll: AutoScroll,
    longest_line_cache: Option<(LongestLineCache, f32)>,
    pending_context_menu: Option<(Point, usize)>,
}

impl DocumentView {
    fn new(opts: DocOptions) -> Self {
        Self {
            wrap: WrapIndex::new(TextStyle::default(), None),
            soft_wrap: opts.soft_wrap,
            last_line_height: 20.0,
            paddings: opts.paddings,
            scrollbar: ScrollbarState::default(),
            auto_scroll: AutoScroll::default(),
            longest_line_cache: None,
            pending_context_menu: None,
        }
    }
}

pub(crate) struct Editor {
    model: TextModel,
    doc: Option<Box<DocumentView>>,
    blink: BlinkState,
    goal: Option<(f32, usize)>,
    word_anchor: Option<Range<usize>>,
    selecting: bool,
    scroll: Scroll,
    reveal: Option<RevealRequest>,
    input_bounds: Rect,
    frame: Option<FrameLayout>,
    pub(crate) caret_height: f32,
    pub(crate) caret_hidden: bool,
    placeholder: SharedStr,
}

impl Editor {
    pub(crate) fn document(rope: Rope, opts: DocOptions) -> Self {
        let model = TextModel::new(EditPolicy {
            multi_line: true,
            validate: None,
        });
        let doc = Some(Box::new(DocumentView::new(opts)));
        Self::new(model.with_rope(rope), doc, 0.0, SharedStr::default())
    }

    pub(crate) fn field(opts: FieldOptions) -> Self {
        let model = TextModel::new(EditPolicy {
            multi_line: false,
            validate: opts.validate,
        });
        Self::new(model, None, opts.caret_height, opts.placeholder)
    }

    fn new(
        model: TextModel,
        doc: Option<Box<DocumentView>>,
        caret_height: f32,
        placeholder: SharedStr,
    ) -> Self {
        Self {
            model,
            doc,
            blink: BlinkState::default(),
            goal: None,
            word_anchor: None,
            selecting: false,
            scroll: Scroll::default(),
            reveal: None,
            input_bounds: Rect::ZERO,
            frame: None,
            caret_height,
            caret_hidden: false,
            placeholder,
        }
    }

    pub(crate) fn model(&self) -> &TextModel {
        &self.model
    }

    fn fresh_frame(&self) -> Option<&FrameLayout> {
        self.frame
            .as_ref()
            .filter(|f| f.revision == self.model.revision())
    }

    pub(crate) fn caret_bounds(&self) -> Option<Rect> {
        self.fresh_frame().map(|f| f.caret_rect)
    }

    #[cfg(test)]
    pub(crate) fn soft_wrap_enabled(&self) -> bool {
        self.doc.as_deref().is_some_and(|d| d.soft_wrap)
    }

    pub(crate) fn scroll_offset(&self) -> Point {
        point(self.scroll.x, self.scroll.y as f32)
    }

    fn request_reveal(
        &mut self,
        offset: usize,
        end_affinity: bool,
        direction: Option<MoveDirection>,
        with_margin: bool,
    ) {
        self.reveal = Some(RevealRequest {
            offset,
            end_affinity,
            direction,
            with_margin,
        });
    }

    fn reset_pointer_selection_state(&mut self) {
        self.word_anchor = None;
        self.selecting = false;
        if let Some(doc) = &mut self.doc {
            doc.auto_scroll.stop();
        }
    }

    fn apply_change(&mut self, change: Option<LineChange>, ts: &TextSystem) {
        let Some(lines) = change else {
            return;
        };
        self.goal = None;
        self.reset_pointer_selection_state();
        if let Some(doc) = &mut self.doc {
            doc.wrap.apply_line_change(self.model.text(), &lines, ts);
        }
        let aff = self.model.selection().end_affinity;
        self.request_reveal(self.model.cursor(), aff, None, true);
    }

    pub(crate) fn is_selecting(&self) -> bool {
        self.selecting
    }

    pub(crate) fn scrollbar_drag_axis(&self) -> Option<Axis> {
        self.doc.as_deref().and_then(|d| d.scrollbar.drag_axis())
    }

    pub(crate) fn scrollbar_interactive(&self, axis: Axis) -> bool {
        self.doc
            .as_deref()
            .is_some_and(|d| d.scrollbar.is_interactive(axis))
    }

    pub(crate) fn set_soft_wrap(&mut self, wrap: bool, ts: &TextSystem) {
        let Some(doc) = &mut self.doc else {
            return;
        };
        if doc.soft_wrap == wrap {
            return;
        }
        let anchor = doc.capture_scroll_anchor(self.scroll.y, doc.last_line_height);
        doc.soft_wrap = wrap;
        let text = self.model.text();
        if wrap {
            let width = self
                .frame
                .as_ref()
                .and_then(|f| f.wrap_width)
                .or_else(|| {
                    (self.input_bounds.size.width > RIGHT_MARGIN)
                        .then_some(self.input_bounds.size.width - RIGHT_MARGIN)
                });
            doc.wrap.set_wrap_width(text, width, ts);
            self.scroll.x = 0.0;
        } else {
            doc.wrap.set_wrap_width(text, None, ts);
        }
        if let Some(anchor) = anchor {
            self.scroll.y = doc.restore_scroll_anchor(text, anchor, doc.last_line_height);
        }
    }

    pub(crate) fn begin_save(&mut self) -> u64 {
        self.model.begin_save()
    }

    pub(crate) fn mark_saved_version(&mut self, version: u64) {
        self.model.mark_saved_version(version);
    }

    pub(crate) fn start_blink(&mut self, timers: &mut dyn EditorTimers) {
        self.blink.start(timers);
    }

    pub(crate) fn pause_blink(&mut self, timers: &mut dyn EditorTimers) {
        self.blink.pause(timers);
    }

    pub(crate) fn stop_blink(&mut self) {
        self.blink.stop();
    }

    pub(crate) fn on_blur(&mut self) {
        self.model.commit_composition();
        self.model.break_coalescing();
        self.blink.stop();
        self.reset_pointer_selection_state();
    }

    pub(crate) fn view<H, C>(&self, hit: H, editor_draw: C, line_height: f32) -> Node<H, C> {
        self.view_with_scrollbars(hit, editor_draw, line_height, Vec::new())
    }

    pub(crate) fn view_with_scrollbars<H, C>(
        &self,
        hit: H,
        editor_draw: C,
        line_height: f32,
        scrollbars: Vec<Node<H, C>>,
    ) -> Node<H, C> {
        let cursor = crate::platform::CursorStyle::IBeam;
        if let Some(doc) = &self.doc {
            let text_leaf = custom(editor_draw)
                .w_full()
                .h_full()
                .flex_1()
                .min_h(2.0 * line_height);
            box_()
                .hit(hit)
                .cursor(cursor)
                .flex_1()
                .overflow_x_hidden()
                .h_full()
                .child(box_().size_full().padding(doc.paddings).child(text_leaf))
                .children(scrollbars)
        } else {
            row()
                .hit(hit)
                .cursor(cursor)
                .flex_1()
                .overflow_x_hidden()
                .min_h_full()
                .items_center()
                .child(custom(editor_draw).w_full().h(line_height))
        }
    }
}

#[derive(Default)]
pub(crate) struct BlinkState {
    active: bool,
    visible: bool,
    paused: bool,
    started_at: Option<Instant>,
    timer: Option<TimerHandle>,
}

impl BlinkState {
    pub(crate) fn visible(&self) -> bool {
        self.paused || self.visible
    }

    pub(crate) fn stop(&mut self) {
        self.active = false;
        self.paused = false;
        self.visible = false;
        self.started_at = None;
        self.timer = None;
    }

    pub(crate) fn start(&mut self, timers: &mut dyn EditorTimers) {
        self.active = true;
        self.paused = false;
        self.visible = false;
        self.started_at = Some(Instant::now());
        self.step(timers);
    }

    pub(crate) fn pause(&mut self, timers: &mut dyn EditorTimers) {
        if !self.active && !self.visible {
            return;
        }
        self.paused = true;
        self.visible = true;
        self.started_at = Some(Instant::now());
        let Some(delay) = timers.blink_timing().0 else {
            self.timer = None;
            return;
        };
        self.timer = timers.after(delay, EditorTimer::Blink);
    }

    pub(crate) fn on_timer(&mut self, timers: &mut dyn EditorTimers) -> bool {
        if !self.active {
            return false;
        }
        self.paused = false;
        self.step(timers);
        true
    }

    fn step(&mut self, timers: &mut dyn EditorTimers) {
        if self.paused {
            self.visible = true;
            return;
        }
        let (interval, timeout) = timers.blink_timing();
        let Some(interval) = interval else {
            self.visible = true;
            self.timer = None;
            return;
        };
        if let Some(timeout) = timeout
            && self
                .started_at
                .is_some_and(|start| start.elapsed() >= timeout)
        {
            self.visible = true;
            self.timer = None;
            return;
        }
        self.visible = !self.visible;
        self.timer = timers.after(interval, EditorTimer::Blink);
    }
}

#[derive(Default)]
pub(crate) struct AutoScroll {
    delta: Option<f32>,
    timer: Option<TimerHandle>,
    pub(crate) last_drag_pos: Option<Point>,
}

impl AutoScroll {
    pub(crate) fn compute_delta(y: f32, bounds: Rect) -> Option<f32> {
        const MIN_SPEED: f32 = 12.0;
        const MAX_SPEED: f32 = 64.0;
        const INNER_ZONE: f32 = 16.0;
        const OUTER_RAMP: f32 = 80.0;

        let bottom = bounds.bottom() - INNER_ZONE;
        let top = bounds.top() + INNER_ZONE;

        if y > bottom {
            let t = ((y - bottom) / (INNER_ZONE + OUTER_RAMP)).min(1.0);
            Some(MIN_SPEED + t * (MAX_SPEED - MIN_SPEED))
        } else if y < top {
            let t = ((top - y) / (INNER_ZONE + OUTER_RAMP)).min(1.0);
            Some(-(MIN_SPEED + t * (MAX_SPEED - MIN_SPEED)))
        } else {
            None
        }
    }

    pub(crate) fn stop(&mut self) {
        self.delta = None;
        self.timer = None;
        self.last_drag_pos = None;
    }

    pub(crate) fn update(&mut self, delta: Option<f32>, timers: &mut dyn EditorTimers) {
        let was_active = self.delta.is_some();
        self.delta = delta;
        if delta.is_none() {
            self.timer = None;
            return;
        }
        if !was_active {
            let interval = Duration::from_millis(16);
            self.timer = timers.every(interval, interval, EditorTimer::AutoScroll);
        }
    }
}

impl Editor {
    fn hit_test_point(&self, pos: Point) -> (usize, bool) {
        let Some(frame) = self.fresh_frame() else {
            return (0, false);
        };
        frame.hit_test(
            pos,
            self.scroll.x,
            self.scroll.y,
            self.model.text().len(),
            self.model.is_multi_line(),
        )
    }

    pub(crate) fn on_mouse_down(
        &mut self,
        button: MouseButton,
        position: Point,
        modifiers: Modifiers,
        click_count: usize,
        timers: &mut dyn EditorTimers,
    ) {
        self.model.commit_composition();
        self.model.break_coalescing();
        self.selecting = true;
        let (offset, aff) = self.hit_test_point(position);

        if button == MouseButton::Left && click_count >= 3 {
            self.select_line_at(offset);
            return;
        }
        if button == MouseButton::Left && click_count == 2 {
            self.select_word_at(offset);
            return;
        }
        if button == MouseButton::Right {
            if !self.model.selection().contains(offset) {
                self.move_to_with_affinity(offset, None, aff, false, Some(timers));
            }
            if let Some(doc) = &mut self.doc {
                doc.pending_context_menu = Some((position, offset));
            }
            return;
        }
        if modifiers.shift {
            self.select_to_with_affinity(offset, aff);
            self.request_reveal(self.model.cursor(), aff, None, false);
        } else {
            self.move_to_with_affinity(offset, None, aff, false, Some(timers));
        }
    }

    pub(crate) fn on_mouse_move(
        &mut self,
        position: Point,
        dragging_left: bool,
        focused: bool,
        timers: &mut dyn EditorTimers,
    ) -> bool {
        if dragging_left
            && self.selecting
            && focused
            && self.model.text().len() > 0
            && self.fresh_frame().is_some()
        {
            let (offset, aff) = self.hit_test_point(position);
            self.select_to_with_affinity(offset, aff);
            if let Some(doc) = &mut self.doc {
                doc.auto_scroll.last_drag_pos = Some(position);
                let delta = AutoScroll::compute_delta(position.y, self.input_bounds);
                doc.auto_scroll.update(delta.map(|d| -d), timers);
            }
            self.request_reveal(self.model.cursor(), aff, None, false);
            return true;
        }
        false
    }

    pub(crate) fn on_mouse_up(
        &mut self,
        button: MouseButton,
        timers: &mut dyn EditorTimers,
    ) -> Option<Point> {
        let mut open_context_menu = None;
        if button == MouseButton::Right
            && let Some(doc) = &mut self.doc
            && let Some((pos, offset)) = doc.pending_context_menu.take()
        {
            if !self.model.selection().contains(offset) {
                self.move_to_with_affinity(offset, None, false, false, Some(timers));
            }
            open_context_menu = Some(pos);
        }
        self.reset_pointer_selection_state();
        open_context_menu
    }
}

impl Editor {
    fn commit_edit(
        &mut self,
        change: Option<LineChange>,
        ts: &TextSystem,
        timers: &mut dyn EditorTimers,
    ) {
        self.pause_blink(timers);
        self.apply_change(change, ts);
    }

    pub(crate) fn selected_string(&self) -> Option<String> {
        (!self.model.selection().is_empty()).then(|| self.model.text().slice(self.model.selection().range()).to_string())
    }

    pub(crate) fn prepare_cut(&mut self) -> Option<String> {
        self.model.commit_composition();
        self.selected_string()
    }

    pub(crate) fn delete_selection(&mut self, ts: &TextSystem, timers: &mut dyn EditorTimers) {
        let change = self.model.replace_selection("");
        self.commit_edit(change, ts, timers);
    }

    pub(crate) fn paste(&mut self, raw: &str, ts: &TextSystem, timers: &mut dyn EditorTimers) {
        let text = if self.model.is_multi_line() {
            raw
        } else {
            raw.split(['\r', '\n']).next().unwrap_or("")
        };
        if !text.is_empty() {
            let change = self.model.replace_selection(text);
            self.commit_edit(change, ts, timers);
        }
    }

    pub(crate) fn handle_command(
        &mut self,
        command: EditCommand,
        ts: &TextSystem,
        timers: &mut dyn EditorTimers,
    ) -> bool {
        match command {
            EditCommand::Backspace => {
                let Some(change) = self.model.delete_backward() else {
                    return false;
                };
                self.commit_edit(Some(change), ts, timers);
            }
            EditCommand::Delete => {
                let change = self.model.delete_forward();
                self.commit_edit(change, ts, timers);
            }
            EditCommand::DeletePrevWord => {
                let change = self.model.delete_prev_word();
                self.commit_edit(change, ts, timers);
            }
            EditCommand::DeleteNextWord => {
                let change = self.model.delete_next_word();
                self.commit_edit(change, ts, timers);
            }
            EditCommand::Enter => {
                let Some(change) = self.model.enter() else {
                    return false;
                };
                self.commit_edit(Some(change), ts, timers);
            }
            EditCommand::Escape => {
                let Some(change) = self.model.cancel_composition() else {
                    return false;
                };
                self.apply_change(Some(change), ts);
            }
            EditCommand::Copy | EditCommand::Cut | EditCommand::Paste => return false,
            EditCommand::Undo => {
                if let Some(change) = self.model.undo() {
                    self.commit_edit(Some(change), ts, timers);
                }
            }
            EditCommand::Redo => {
                if let Some(change) = self.model.redo() {
                    self.commit_edit(Some(change), ts, timers);
                }
            }
            EditCommand::IndentInline | EditCommand::Indent | EditCommand::Outdent
                if !self.model.is_multi_line() =>
            {
                return false;
            }
            EditCommand::IndentInline | EditCommand::Indent => {
                let change = self.model.indent(matches!(command, EditCommand::Indent));
                self.commit_edit(change, ts, timers);
            }
            EditCommand::Outdent => {
                let change = self.model.outdent();
                self.commit_edit(change, ts, timers);
            }
            EditCommand::SelectAll => self.select_all(),
            EditCommand::Move { motion, extend } => {
                return self.move_caret(motion, extend, timers);
            }
        }
        true
    }

    pub(crate) fn handle_text_input(
        &mut self,
        text: &str,
        ts: &TextSystem,
        timers: &mut dyn EditorTimers,
    ) {
        let change = self.model.insert_text(text);
        self.commit_edit(change, ts, timers);
    }

    pub(crate) fn handle_ime_composition(&mut self, text: &str, cursor_utf16: usize, ts: &TextSystem) {
        if text.is_empty() && self.model.composition().is_none() {
            return;
        }
        let change = self.model.set_composition(text, cursor_utf16);
        self.apply_change(change, ts);
    }

    pub(crate) fn ime_caret_bounds(&self) -> Option<Rect> {
        let frame = self.fresh_frame()?;
        frame
            .bounds_for_range(self.model.text(), self.model.selection().range())
            .or(Some(frame.caret_rect))
    }

    pub(crate) fn set_value(&mut self, value: &str, ts: &TextSystem) {
        self.reset_pointer_selection_state();
        let change = self.model.set_value(value);
        self.apply_change(change, ts);
        self.scroll.x = 0.0;
        self.scroll.y = 0.0;
        self.reveal = None;
    }

    pub(crate) fn replace(&mut self, text: &str, ts: &TextSystem) {
        let change = self.model.replace_selection(text);
        self.apply_change(change, ts);
    }

    pub(crate) fn replace_all(&mut self, ranges: &[Range<usize>], replacement: &str, ts: &TextSystem) {
        let change = self.model.replace_all(ranges, replacement);
        self.apply_change(change, ts);
    }

    pub(crate) fn select_all(&mut self) {
        self.goal = None;
        self.model.select_all();
    }

    pub(crate) fn set_selected_range(&mut self, range: Range<usize>) {
        self.goal = None;
        self.word_anchor = None;
        self.model.set_selected_range(range);
        self.request_reveal(self.model.cursor(), false, None, true);
    }

    pub(crate) fn go_to_line(&mut self, line: usize) {
        self.goal = None;
        self.model.go_to_line(line);
        self.request_reveal(self.model.cursor(), false, None, true);
    }

    fn visual_line_start_for(&self, offset: usize, end_affinity: bool) -> usize {
        let Some(doc) = &self.doc else {
            return 0;
        };
        let text = self.model.text();
        let pt = text.offset_to_point(offset);
        let line_start = text.line_start_offset(pt.row);
        if doc.soft_wrap {
            let disp = doc.wrap.offset_to_display(text, offset, end_affinity);
            if let Some((range, _)) = doc.wrap.visual_row_range(pt.row, disp.sub_row) {
                return line_start + range.start;
            }
        }
        line_start
    }

    fn visual_line_end_for(&self, offset: usize, end_affinity: bool) -> (usize, bool) {
        let text = self.model.text();
        let Some(doc) = &self.doc else {
            return (text.len(), false);
        };
        let pt = text.offset_to_point(offset);
        let line_start = text.line_start_offset(pt.row);
        if doc.soft_wrap {
            let disp = doc.wrap.offset_to_display(text, offset, end_affinity);
            if let Some((range, soft_end)) = doc.wrap.visual_row_range(pt.row, disp.sub_row) {
                return (line_start + range.end, soft_end);
            }
        }
        (text.line_end_offset(pt.row), false)
    }

    fn preferred_x_for(
        &self,
        offset: usize,
        end_affinity: bool,
    ) -> Option<(f32, usize)> {
        let frame = self.fresh_frame()?;
        let text = self.model.text();
        let pt = text.offset_to_point(offset);
        let row_layout = frame.row_layout(pt.row)?;
        let pos = row_layout.position_for_index(pt.column, frame.line_height, end_affinity)?;
        Some((pos.x, pt.column))
    }

    fn move_to_with_affinity(
        &mut self,
        offset: usize,
        direction: Option<MoveDirection>,
        end_affinity: bool,
        with_margin: bool,
        timers: Option<&mut dyn EditorTimers>,
    ) {
        self.goal = None;
        self.model.place_caret(offset, end_affinity);
        self.request_reveal(self.model.cursor(), end_affinity, direction, with_margin);
        if let Some(timers) = timers {
            self.pause_blink(timers);
        }
    }

    fn page_rows(&self, doc: &DocumentView) -> isize {
        let lh = self
            .frame
            .as_ref()
            .map_or(doc.last_line_height, |f| f.line_height)
            .max(1.0);
        ((self.input_bounds.size.height / lh).floor() as isize).max(1)
    }

    fn vertical_target(
        &self,
        doc: &DocumentView,
        offset: usize,
        goal: Option<(f32, usize)>,
        end_affinity: bool,
        delta_rows: isize,
    ) -> (usize, bool) {
        if doc.wrap.visual_row_count() == 0 {
            return (offset, end_affinity);
        }
        let text = self.model.text();
        let mut display = doc.wrap.offset_to_display(text, offset, end_affinity);
        let max_row = doc.wrap.visual_row_count().saturating_sub(1);
        display.row = display.row.saturating_add_signed(delta_rows).min(max_row);
        display.col = 0;

        let mut target_offset = doc.wrap.display_to_offset(text, display);
        let mut target_affinity = false;

        if let Some((pref_x, byte_col)) = goal {
            let mut next_disp = doc.wrap.offset_to_display(text, target_offset, false);
            next_disp.col = 0;
            let next_pt = doc.wrap.display_to_point(text, next_disp);
            let line_start = text.line_start_offset(next_pt.row);
            if let Some(frame) = self.fresh_frame()
                && let Some(row_layout) = frame.row_layout(next_pt.row)
            {
                if let Some((local_ix, affinity)) = row_layout.closest_index_for_position(
                    point(pref_x, next_disp.sub_row as f32 * frame.line_height),
                    frame.line_height,
                ) {
                    target_offset = line_start + local_ix;
                    target_affinity = affinity;
                }
            } else if let Some((sub_range, soft_end)) =
                doc.wrap.visual_row_range(next_pt.row, next_disp.sub_row)
            {
                let max_col = sub_range.len();
                let col = byte_col.min(max_col);
                target_offset = line_start + sub_range.start + col;
                target_affinity = soft_end && col == max_col;
            } else {
                let max_len = text.slice_line(next_pt.row).len();
                target_offset = line_start + byte_col.min(max_len);
            }
        }
        (self.model.snap_boundary(target_offset, Bias::Left), target_affinity)
    }

    fn move_caret(
        &mut self,
        motion: Motion,
        extend: bool,
        timers: &mut dyn EditorTimers,
    ) -> bool {
        let is_vertical = matches!(
            motion,
            Motion::Up | Motion::Down | Motion::PageUp | Motion::PageDown
        );
        if is_vertical && self.doc.is_none() {
            return false;
        }
        self.model.commit_composition();

        let sel = self.model.selection();
        let len = self.model.text().len();
        if !extend {
            if matches!(motion, Motion::Left) && sel.is_empty() && self.model.cursor() == 0 {
                return false;
            }
            if matches!(motion, Motion::Right) && sel.is_empty() && self.model.cursor() == len {
                return false;
            }
        }

        let (target, target_aff, next_goal, direction) = match motion {
            Motion::Left => {
                let t = if !extend && !sel.is_empty() {
                    sel.start()
                } else {
                    prev_grapheme_boundary(self.model.text(), sel.head)
                };
                (t, false, None, None)
            }
            Motion::Right => {
                let t = if !extend && !sel.is_empty() {
                    sel.end()
                } else {
                    next_grapheme_boundary(self.model.text(), sel.head)
                };
                (t, false, None, None)
            }
            Motion::PrevWord => (prev_word_start(self.model.text(), sel.head), false, None, None),
            Motion::NextWord => (next_word_end(self.model.text(), sel.head), false, None, None),
            Motion::LineStart => (
                self.visual_line_start_for(sel.head, sel.end_affinity),
                false,
                None,
                Some(MoveDirection::Up),
            ),
            Motion::LineEnd => {
                let (t, aff) = self.visual_line_end_for(sel.head, sel.end_affinity);
                (t, aff, None, Some(MoveDirection::Down))
            }
            Motion::DocStart => (0, false, None, None),
            Motion::DocEnd => (len, false, None, None),
            Motion::Up | Motion::Down | Motion::PageUp | Motion::PageDown => {
                let Some(doc) = &self.doc else {
                    return false;
                };
                let delta = match motion {
                    Motion::Up => -1,
                    Motion::Down => 1,
                    Motion::PageUp => -self.page_rows(doc),
                    Motion::PageDown => self.page_rows(doc),
                    _ => unreachable!(),
                };
                let dir = if delta < 0 {
                    MoveDirection::Up
                } else {
                    MoveDirection::Down
                };
                if extend {
                    let goal = self
                        .goal
                        .or_else(|| self.preferred_x_for(sel.head, sel.end_affinity));
                    let mut target =
                        self.vertical_target(doc, sel.head, goal, sel.end_affinity, delta);
                    if doc.wrap.visual_row_count() > 0 {
                        let text = self.model.text();
                        let before_row = doc
                            .wrap
                            .offset_to_display(text, sel.head, sel.end_affinity)
                            .row;
                        let after_row = doc.wrap.offset_to_display(text, target.0, target.1).row;
                        if before_row == after_row {
                            target = (if delta < 0 { 0 } else { len }, false);
                        }
                    }
                    (target.0, target.1, goal, Some(dir))
                } else {
                    let collapse = matches!(motion, Motion::Up | Motion::Down);
                    let (base, goal, aff) = if sel.is_empty() || !collapse {
                        (
                            sel.head,
                            self.goal
                                .or_else(|| self.preferred_x_for(sel.head, sel.end_affinity)),
                            sel.end_affinity,
                        )
                    } else if delta < 0 {
                        (sel.start(), self.preferred_x_for(sel.start(), false), false)
                    } else {
                        let a = sel.end() == self.model.cursor() && sel.end_affinity;
                        (sel.end(), self.preferred_x_for(sel.end(), a), a)
                    };
                    let (t, t_aff) = self.vertical_target(doc, base, goal, aff, delta);
                    (t, t_aff, goal, Some(dir))
                }
            }
        };

        self.goal = next_goal;
        if extend {
            self.pause_blink(timers);
            self.model.break_coalescing();
            self.model.extend_selection(target, target_aff, None);
            self.request_reveal(self.model.cursor(), target_aff, None, true);
        } else {
            self.model.place_caret(target, target_aff);
            self.request_reveal(self.model.cursor(), target_aff, direction, true);
            self.pause_blink(timers);
        }
        true
    }

    fn select_to_with_affinity(&mut self, offset: usize, end_affinity: bool) {
        self.goal = None;
        self.model
            .extend_selection(offset, end_affinity, self.word_anchor.clone());
    }

    fn select_word_at(&mut self, offset: usize) {
        self.goal = None;
        if let Some(range) = self.model.select_word_at(offset) {
            self.word_anchor = Some(range);
        }
    }

    fn select_line_at(&mut self, offset: usize) {
        self.goal = None;
        self.model.select_line_at(offset);
        self.word_anchor = None;
    }
}

const HUGE_UNWRAPPED_LINE_BYTES: usize = 4096;
const HORIZONTAL_WINDOW_PAD: f32 = 256.0;

#[derive(Clone, Debug, PartialEq)]
struct LongestLineCache {
    revision: u64,
    row: usize,
    len: usize,
    style: TextStyle,
    wrap_width: Option<f32>,
}

struct VisibleSlice {
    lines: Range<usize>,
    top_rel: f32,
    shape_rows: Range<usize>,
}

fn snap_point(pt: Point, scale: f32) -> Point {
    point(
        (pt.x * scale).round() / scale,
        (pt.y * scale).round() / scale,
    )
}

fn content_width_with_margin(max_line_w: f32, viewport_width: f32) -> f32 {
    if max_line_w + RIGHT_MARGIN > viewport_width {
        max_line_w + RIGHT_MARGIN
    } else {
        max_line_w
    }
}

impl DocumentView {
    fn visible_slice(&self, scroll_y: f64, line_height: f32, viewport_height: f32) -> VisibleSlice {
        let total_rows = self.wrap.visual_row_count();
        let total_lines = self.wrap.line_count();
        if total_rows == 0 || total_lines == 0 {
            return VisibleSlice {
                lines: 0..0,
                top_rel: 0.0,
                shape_rows: 0..0,
            };
        }
        let lh = f64::from(line_height);
        let top = (-scroll_y).max(0.0);
        let bottom = top + f64::from(viewport_height);
        let first_row = ((top / lh).floor() as usize).min(total_rows - 1);
        let last_row = ((bottom / lh).ceil() as usize).min(total_rows - 1);
        let shape_rows = first_row.saturating_sub(2)..(last_row + 3).min(total_rows);

        let start_line = self.wrap.visual_row_to_line(first_row);
        let end_line = self.wrap.visual_row_to_line(last_row);
        let first_vis_row = self.wrap.first_visual_row(start_line);
        let top_rel = (first_vis_row as f64 * lh + scroll_y) as f32;
        VisibleSlice {
            lines: start_line..(end_line + 2).min(total_lines),
            top_rel,
            shape_rows,
        }
    }
}

impl Editor {
    pub(crate) fn prepare(
        &mut self,
        input_bounds: Rect,
        params: EditorPaintParams<'_>,
        ts: &TextSystem,
    ) -> bool {
        self.input_bounds = input_bounds;
        if self.doc.is_none() {
            self.prepare_field(input_bounds, params, ts);
            return false;
        }
        self.prepare_document(input_bounds, params, ts)
    }

    pub(crate) fn paint(&self, p: Palette, canvas: &mut Canvas) {
        if let Some(frame) = self.frame.as_ref() {
            frame.paint(p, canvas);
        }
    }

    fn finish_frame(
        &mut self,
        mut frame: FrameLayout,
        head_pt: Point,
        is_empty_field: bool,
        params: EditorPaintParams<'_>,
    ) {
        // Vertical scroll is baked into each VisibleLine::top in f64 so D2D coordinates stay exact on huge files.
        frame.scroll_x = self.scroll.x;
        frame.scroll_y = self.scroll.y;
        frame.content_bounds = Rect::new(
            point(
                frame.input_bounds.origin.x + self.scroll.x,
                frame.input_bounds.origin.y,
            ),
            frame.input_bounds.size,
        );
        frame.caret_rect = self.compute_caret_rect(
            frame.content_bounds.origin,
            head_pt,
            frame.line_height,
            params.scale,
            params.caret_width_px,
        );
        frame.show_caret = params.focused
            && params.window_active
            && self.blink.visible()
            && !self.caret_hidden
            && (self.model.selection().is_empty() || self.model.composition().is_some());
        frame.marked_range = (!is_empty_field)
            .then(|| self.model.composition().map(|c| c.start..c.end))
            .flatten();
        if params.window_active
            && params.show_selection
            && self.model.composition().is_none()
            && !self.model.selection().is_empty()
        {
            frame.selection_rects = frame.range_rects(&self.model.selection().range());
        }
        self.frame = Some(frame);
    }

    fn prepare_field(&mut self, input_bounds: Rect, params: EditorPaintParams<'_>, ts: &TextSystem) {
        let style = params.style;
        let text_style = &style.text;
        let line_height = style.line_height.max(1.0);
        let space_width = ts.face(text_style).space_width();
        let is_empty = self.model.text().len() == 0;
        let (display_str, text_color): (SharedStr, Rgba) = if is_empty {
            (self.placeholder.clone(), params.palette.text_secondary)
        } else {
            (self.model.text().to_string().into(), style.color)
        };
        let shaped = ts.shape_row(display_str, text_style);
        let row_layout = RowLayout::from_shaped(vec![shaped]);
        let head = self.model.cursor();
        let head_pt = row_layout
            .position_for_index(head, line_height, false)
            .unwrap_or(Point::ZERO);
        let max_line_w = row_layout.size(line_height).width.max(head_pt.x);
        self.scroll.content_w = content_width_with_margin(max_line_w, input_bounds.size.width);
        if self.reveal.take().is_some() {
            self.scroll.x =
                reveal_horizontal_scroll(self.scroll.x, head_pt.x, input_bounds.size.width);
        }
        self.scroll.x =
            clamp_horizontal_scroll(self.scroll.x, self.scroll.content_w, input_bounds.size.width);
        let text_len = self.model.text().len();
        let frame = FrameLayout {
            revision: self.model.revision(),
            scroll_x: 0.0,
            scroll_y: 0.0,
            input_bounds,
            content_bounds: Rect::ZERO,
            first_line: 0,
            lines: vec![VisibleLine {
                byte_offset: 0,
                top: 0.0,
                layout: row_layout,
            }],
            visible_byte_range: 0..text_len,
            line_height,
            wrap_width: None,
            space_width,
            text_color,
            caret_rect: Rect::ZERO,
            show_caret: false,
            selection_rects: Vec::new(),
            marked_range: None,
            last_cursor: head,
        };
        self.finish_frame(frame, head_pt, is_empty, params);
    }

    fn prepare_document(
        &mut self,
        input_bounds: Rect,
        params: EditorPaintParams<'_>,
        ts: &TextSystem,
    ) -> bool {
        let style = params.style;
        let text_style = &style.text;
        let line_height = style.line_height.max(1.0);
        let face = ts.face(text_style);
        let space_width = face.space_width();
        let text = self.model.text();
        let reveal_req = self.reveal.take();
        let head = self.model.cursor();
        let end_affinity = self.model.selection().end_affinity;

        let doc = self.doc.as_deref_mut().expect("document view");
        let soft_wrap = doc.soft_wrap;
        let wrap_width = soft_wrap.then_some((input_bounds.size.width - RIGHT_MARGIN).max(1.0));

        let reflowing = doc.wrap.style() != text_style
            || doc.wrap.wrap_width() != wrap_width
            || (doc.last_line_height - line_height).abs() > 0.01;
        let anchor = if reflowing && reveal_req.is_none() {
            doc.capture_scroll_anchor(self.scroll.y, doc.last_line_height)
        } else {
            None
        };

        doc.wrap.sync_layout(text, text_style, wrap_width, ts);
        doc.last_line_height = line_height;

        if let Some(anchor) = anchor {
            self.scroll.y = doc.restore_scroll_anchor(text, anchor, line_height);
        }

        let total_rows = doc.wrap.visual_row_count().max(1);
        let content_h = (total_rows as f64 * f64::from(line_height))
            .max(f64::from(input_bounds.size.height));
        self.scroll.content_h = content_h;

        if let Some(req) = reveal_req {
            let target = req.offset.min(text.len());
            let disp = doc.wrap.offset_to_display(text, target, req.end_affinity);
            let row_y = disp.row as f64 * f64::from(line_height);
            let vp_h = f64::from(input_bounds.size.height);
            let lh_f64 = f64::from(line_height);
            let v_margin = if req.with_margin {
                f64::from(caret_vertical_margin(input_bounds.size.height, line_height))
            } else {
                0.0
            };
            let prev_y = self.scroll.y;
            let mut next_y = prev_y;
            if row_y < -next_y + v_margin {
                next_y = (v_margin - row_y).min(0.0);
            } else if row_y + lh_f64 > -next_y + vp_h - v_margin {
                next_y = vp_h - v_margin - (row_y + lh_f64);
            }
            match req.direction {
                Some(MoveDirection::Up) => next_y = next_y.max(prev_y),
                Some(MoveDirection::Down) => next_y = next_y.min(prev_y),
                None => {}
            }
            self.scroll.y = next_y;
        }
        let min_y = (-content_h + f64::from(input_bounds.size.height)).min(0.0);
        self.scroll.y = self.scroll.y.clamp(min_y, 0.0);

        // Compute horizontal extent and resolve horizontal reveal before shaping windowed lines.
        let mut max_line_w = wrap_width.unwrap_or(0.0);
        if soft_wrap {
            self.scroll.x = 0.0;
        } else {
            let longest_row = doc.wrap.longest_line();
            let slice = text.slice_line(longest_row);
            let key = LongestLineCache {
                revision: self.model.revision(),
                row: longest_row,
                len: slice.len(),
                style: text_style.clone(),
                wrap_width,
            };
            max_line_w = match &doc.longest_line_cache {
                Some((cached, w)) if *cached == key => *w,
                _ => {
                    let s: Cow<str> = slice.into();
                    let w = face.unwrapped_width(&s);
                    doc.longest_line_cache = Some((key, w));
                    w
                }
            };
            if reveal_req.is_some() {
                let pt = text.offset_to_point(head);
                let line = text.slice_line(pt.row);
                let prefix: Cow<str> = line.slice(..pt.column.min(line.len())).into();
                let caret_x = face.unwrapped_width(&prefix);
                max_line_w = max_line_w.max(caret_x);
                self.scroll.x =
                    reveal_horizontal_scroll(self.scroll.x, caret_x, input_bounds.size.width);
            }
            let est_w = content_width_with_margin(max_line_w, input_bounds.size.width);
            self.scroll.x = clamp_horizontal_scroll(self.scroll.x, est_w, input_bounds.size.width);
        }

        let vis = doc.visible_slice(self.scroll.y, line_height, input_bounds.size.height);
        let first_line = vis.lines.start;
        let start_off = text.line_start_offset(vis.lines.start);
        let end_off = text.line_end_offset(vis.lines.end.saturating_sub(1));

        let prev_scroll_x = self.scroll.x;
        let (lines, shaped_windowed) =
            self.shape_visible_lines(vis, style, line_height, input_bounds.size.width, ts);

        if !soft_wrap {
            for line in &lines {
                max_line_w = max_line_w.max(line.layout.size(line_height).width);
            }
        }

        let doc = self.doc.as_deref().expect("document view");
        let text = self.model.text();
        let head_pt = {
            let row = text.offset_to_point(head).row;
            row.checked_sub(first_line)
                .and_then(|vi| lines.get(vi))
                .and_then(|line| {
                    let local = head.saturating_sub(line.byte_offset);
                    line.layout
                        .position_for_index(local, line_height, end_affinity)
                        .map(|pt| point(pt.x, line.top + pt.y))
                })
                .unwrap_or_else(|| {
                    let disp = doc.wrap.offset_to_display(text, head, end_affinity);
                    point(
                        0.0,
                        (disp.row as f64 * f64::from(line_height) + self.scroll.y) as f32,
                    )
                })
        };
        if !soft_wrap {
            max_line_w = max_line_w.max(head_pt.x);
        }

        self.scroll.content_w = content_width_with_margin(max_line_w, input_bounds.size.width);
        if reveal_req.is_some() && !soft_wrap {
            self.scroll.x =
                reveal_horizontal_scroll(self.scroll.x, head_pt.x, input_bounds.size.width);
        }
        self.scroll.x = if soft_wrap {
            0.0
        } else {
            clamp_horizontal_scroll(self.scroll.x, self.scroll.content_w, input_bounds.size.width)
        };

        let frame = FrameLayout {
            revision: self.model.revision(),
            scroll_x: 0.0,
            scroll_y: 0.0,
            input_bounds,
            content_bounds: Rect::ZERO,
            first_line,
            lines,
            visible_byte_range: start_off..end_off,
            line_height,
            wrap_width,
            space_width,
            text_color: style.color,
            caret_rect: Rect::ZERO,
            show_caret: false,
            selection_rects: Vec::new(),
            marked_range: None,
            last_cursor: head,
        };
        self.finish_frame(frame, head_pt, false, params);

        shaped_windowed && (self.scroll.x - prev_scroll_x).abs() > 1.0
    }

    fn shape_visible_lines(
        &self,
        vis: VisibleSlice,
        style: &EditorPaintStyle,
        line_height: f32,
        viewport_width: f32,
        ts: &TextSystem,
    ) -> (Vec<VisibleLine>, bool) {
        let doc = self.doc.as_deref().expect("document view");
        let text = self.model.text();
        let text_style = &style.text;
        let caret = self.model.cursor();
        let mut result = Vec::with_capacity(vis.lines.len());
        let mut shaped_windowed = false;
        let mut y = vis.top_rel;

        for buf_line in vis.lines {
            let line_slice = text.slice_line(buf_line);
            let line_len = line_slice.len();
            let line_byte_start = text.line_start_offset(buf_line);
            let wrapped = doc.wrap.line(buf_line).expect("wrapped line exists");
            let total_sub_rows = wrapped.row_count();

            let layout =
                if !doc.soft_wrap && total_sub_rows == 1 && line_len > HUGE_UNWRAPPED_LINE_BYTES {
                    let line_str: Cow<str> = line_slice.into();
                    let min_x = (-self.scroll.x - HORIZONTAL_WINDOW_PAD).max(0.0);
                    let max_x =
                        (-self.scroll.x + viewport_width + HORIZONTAL_WINDOW_PAD).max(min_x);
                    if let Some((mut start_b, mut end_b, start_x, total_w)) =
                        ts.face(text_style).horizontal_window(&line_str, min_x, max_x)
                    {
                        if caret >= line_byte_start && caret <= line_byte_start + line_len {
                            let local_caret = caret - line_byte_start;
                            if local_caret < start_b {
                                start_b = local_caret;
                            } else if local_caret > end_b {
                                end_b = local_caret;
                            }
                        }
                        let slice_str: SharedStr = line_str[start_b..end_b].to_string().into();
                        let shaped = ts.shape_row(slice_str, text_style);
                        let vrow = VisualRow::Windowed {
                            start_byte: start_b,
                            end_byte: end_b,
                            total_bytes: line_len,
                            start_x,
                            total_width: total_w,
                            line: shaped,
                        };
                        shaped_windowed = true;
                        RowLayout::from_slice(vec![vrow], 0, 0, 1, 0.0)
                    } else {
                        self.shape_wrapped_line(buf_line, &vis.shape_rows, text_style, ts)
                    }
                } else {
                    self.shape_wrapped_line(buf_line, &vis.shape_rows, text_style, ts)
                };

            let h = layout.size(line_height).height;
            result.push(VisibleLine {
                byte_offset: line_byte_start,
                top: y,
                layout,
            });
            y += h;
        }
        (result, shaped_windowed)
    }

    fn shape_wrapped_line(
        &self,
        buf_line: usize,
        shape_rows: &Range<usize>,
        text_style: &TextStyle,
        ts: &TextSystem,
    ) -> RowLayout {
        let doc = self.doc.as_deref().expect("document view");
        let text = self.model.text();
        let line_slice = text.slice_line(buf_line);
        let line_len = line_slice.len();
        let line_byte_start = text.line_start_offset(buf_line);
        let wrapped = doc.wrap.line(buf_line).expect("wrapped line exists");
        let first_row_of_line = doc.wrap.first_visual_row(buf_line);
        let total_sub_rows = wrapped.row_count();
        let caret = self.model.cursor();
        let end_affinity = self.model.selection().end_affinity;

        let (slice_start, slice_end) = if total_sub_rows > 64 {
            let vis_lo = shape_rows
                .start
                .saturating_sub(first_row_of_line)
                .min(total_sub_rows);
            let vis_hi = shape_rows
                .end
                .saturating_sub(first_row_of_line)
                .min(total_sub_rows);
            let mut lo = vis_lo;
            let mut hi = vis_hi.max(lo.saturating_add(1)).min(total_sub_rows);
            if caret >= line_byte_start && caret <= line_byte_start + line_len {
                let disp = doc.wrap.offset_to_display(text, caret, end_affinity);
                if disp.sub_row < lo {
                    lo = disp.sub_row;
                }
                if disp.sub_row >= hi {
                    hi = (disp.sub_row + 1).min(total_sub_rows);
                }
            }
            (lo, hi)
        } else {
            (0, total_sub_rows)
        };

        let leading_bytes = wrapped.ranges.get(slice_start).map_or(0, |r| r.start);
        let mut rows = Vec::with_capacity(slice_end.saturating_sub(slice_start));
        for sub_ix in slice_start..slice_end {
            let range = match &wrapped.ranges {
                LineRanges::Single(len) => 0..*len,
                LineRanges::Wrapped(slice) => slice[sub_ix].clone(),
            };
            let visual_row = first_row_of_line + sub_ix;
            let sub_start = line_byte_start + range.start;
            let sub_end = line_byte_start + range.end;
            let has_caret = caret >= sub_start && caret <= sub_end;
            let must_shape = shape_rows.contains(&visual_row) || has_caret;
            if must_shape {
                let sub_text: SharedStr = line_slice.slice(range).to_string().into();
                let shaped = ts.shape_row(sub_text, text_style);
                rows.push(VisualRow::from(shaped));
            } else {
                rows.push(VisualRow::Placeholder(range.len()));
            }
        }

        RowLayout::from_slice(
            rows,
            slice_start,
            leading_bytes,
            total_sub_rows,
            wrapped.hanging_indent,
        )
    }

    fn compute_caret_rect(
        &self,
        origin: Point,
        head_pt: Point,
        line_height: f32,
        scale: f32,
        caret_width_px: u32,
    ) -> Rect {
        let caret_h = if self.caret_height > 0.0 {
            self.caret_height
        } else {
            0.85 * line_height
        };
        let caret_origin = snap_point(
            point(
                origin.x + head_pt.x,
                origin.y + head_pt.y + (line_height - caret_h) * 0.5,
            ),
            scale,
        );
        let caret_w = (caret_width_px.max(1) as f32 / scale).max(1.0 / scale);
        Rect::new(caret_origin, size(caret_w, caret_h))
    }
}

const BOTTOM_MARGIN_ROWS: usize = 3;

#[derive(Copy, Clone, Debug)]
struct ScrollAnchor {
    line: usize,
    byte_in_line: usize,
    frac_px: f64,
}

fn scrollbar_viewport(
    input_size: Size,
    content_size: Size,
    scroll_offset: Point,
    paddings: Edges<f32>,
    soft_wrap: bool,
) -> ScrollbarViewport {
    ScrollbarViewport {
        size: size(
            input_size.width + paddings.right,
            input_size.height + paddings.top + paddings.bottom,
        ),
        content_size: size(
            content_size.width + paddings.right,
            content_size.height + paddings.top + paddings.bottom,
        ),
        scroll_offset,
        horizontal_left: paddings.left,
        horizontal: !soft_wrap,
    }
}

fn clamp_horizontal_scroll(offset_x: f32, content_width: f32, viewport_width: f32) -> f32 {
    let min_x = (viewport_width - content_width).min(0.0);
    offset_x.clamp(min_x, 0.0)
}

fn reveal_horizontal_scroll(scroll_x: f32, caret_x: f32, viewport_width: f32) -> f32 {
    if scroll_x + caret_x > viewport_width - RIGHT_MARGIN {
        viewport_width - RIGHT_MARGIN - caret_x
    } else if scroll_x + caret_x < 0.0 {
        -caret_x
    } else {
        scroll_x
    }
}

fn caret_vertical_margin(viewport_height: f32, line_height: f32) -> f32 {
    let rows = (viewport_height / line_height.max(1.0)) as usize;
    let raw = if rows < BOTTOM_MARGIN_ROWS * 8 {
        line_height
    } else {
        BOTTOM_MARGIN_ROWS as f32 * line_height
    };
    raw.min(rows as f32 * line_height * 0.5)
}

impl DocumentView {
    fn capture_scroll_anchor(&self, scroll_y: f64, line_height: f32) -> Option<ScrollAnchor> {
        if self.wrap.visual_row_count() == 0 || line_height <= 0.0 {
            return None;
        }
        let lh = f64::from(line_height);
        let top = (-scroll_y).max(0.0);
        let top_row = (top / lh).floor() as usize;
        let frac_px = top - top_row as f64 * lh;
        let clamped_row = top_row.min(self.wrap.visual_row_count().saturating_sub(1));
        let (line, sub_row) = self.wrap.visual_row_to_line_and_sub_row(clamped_row);
        let byte_in_line = self
            .wrap
            .visual_row_range(line, sub_row)
            .map_or(0, |(r, _)| r.start);
        Some(ScrollAnchor {
            line,
            byte_in_line,
            frac_px,
        })
    }

    fn restore_scroll_anchor(
        &self,
        text: &Rope,
        anchor: ScrollAnchor,
        line_height: f32,
    ) -> f64 {
        if self.wrap.line_count() == 0 || line_height <= 0.0 {
            return 0.0;
        }
        let line = anchor.line.min(self.wrap.line_count().saturating_sub(1));
        let line_start = text.line_start_offset(line);
        let line_len = text.line_len(line);
        let target = line_start + anchor.byte_in_line.min(line_len);
        let disp = self.wrap.offset_to_display(text, target, false);
        let new_top = disp.row as f64 * f64::from(line_height) + anchor.frac_px;
        -new_top.max(0.0)
    }
}

impl Editor {
    fn clamp_and_set_scroll_f64(&mut self, mut x: f32, y: f64) -> bool {
        let y_changed = if self.doc.is_some() {
            let min_y =
                (-self.scroll.content_h + f64::from(self.input_bounds.size.height)).min(0.0);
            let next_y = y.clamp(min_y, 0.0);
            let changed = self.scroll.y != next_y;
            self.scroll.y = next_y;
            changed
        } else {
            false
        };
        x = clamp_horizontal_scroll(x, self.scroll.content_w, self.input_bounds.size.width);
        let x_changed = self.scroll.x != x;
        self.scroll.x = x;
        x_changed || y_changed
    }

    fn apply_scrollbar_scroll(&mut self, next: Option<Point>) -> bool {
        let Some(next) = next else {
            return false;
        };
        self.reveal = None;
        self.clamp_and_set_scroll_f64(next.x, f64::from(next.y));
        true
    }

    pub(crate) fn on_scrollbar_mouse_down(
        &mut self,
        axis: Axis,
        hit_thumb: bool,
        position: Point,
        track_rect: Rect,
    ) -> bool {
        let offset = self.scroll_offset();
        let next = self
            .doc
            .as_deref_mut()
            .and_then(|d| d.scrollbar.on_mouse_down(axis, hit_thumb, position, track_rect, offset));
        self.apply_scrollbar_scroll(next)
    }

    pub(crate) fn on_scrollbar_mouse_move(
        &mut self,
        position: Point,
        dragging_left: bool,
        track_rect: Option<Rect>,
        timers: &mut dyn EditorTimers,
    ) -> Option<bool> {
        let scroll = self.scroll_offset();
        let doc = self.doc.as_deref_mut()?;
        let (next, notify) = doc.scrollbar.on_mouse_move(
            position,
            dragging_left,
            track_rect,
            scroll,
            |delay| timers.after(delay, EditorTimer::ScrollbarDrag),
        )?;
        self.apply_scrollbar_scroll(next);
        Some(notify)
    }

    pub(crate) fn on_scrollbar_mouse_up(
        &mut self,
        position: Point,
        track_rect: Option<Rect>,
    ) -> bool {
        let scroll = self.scroll_offset();
        let next = self
            .doc
            .as_deref_mut()
            .and_then(|d| d.scrollbar.on_mouse_up(position, track_rect, scroll));
        self.apply_scrollbar_scroll(next)
    }

    pub(crate) fn render_scrollbars(
        &mut self,
        slot: EditorSlot,
        p: Palette,
        now: Instant,
        ui: &mut AppUi,
        timers: &mut dyn EditorTimers,
    ) -> Vec<AppNode> {
        let input_size = self.input_bounds.size;
        if input_size.width <= 0. || input_size.height <= 0. {
            return Vec::new();
        }
        let scroll = self.scroll_offset();
        let content_size = size(self.scroll.content_w, self.scroll.content_h as f32);
        let Some(doc) = self.doc.as_deref_mut() else {
            return Vec::new();
        };
        let vp = scrollbar_viewport(input_size, content_size, scroll, doc.paddings, doc.soft_wrap);
        doc.scrollbar.render(slot, vp, p, now, ui, |delay| {
            timers.after(delay, EditorTimer::ScrollbarIdle)
        })
    }

    pub(crate) fn on_scroll_wheel(
        &mut self,
        delta_lines: Point,
        page_scroll: bool,
        modifiers: Modifiers,
        default_line_height: f32,
    ) -> bool {
        if modifiers.ctrl {
            return false;
        }
        let lh = self
            .frame
            .as_ref()
            .map_or(default_line_height, |f| f.line_height)
            .max(1.0);
        let vp = self.input_bounds.size;
        let dx = if delta_lines.x != 0.0 {
            let space_w = self
                .frame
                .as_ref()
                .map_or(lh * 0.5, |f| f.space_width.max(1.0));
            (delta_lines.x * space_w * 3.0)
                .round()
                .clamp(-vp.width.max(lh), vp.width.max(lh))
        } else {
            0.0
        };
        let dy = if page_scroll {
            f64::from(delta_lines.y) * f64::from((vp.height - lh).max(lh))
        } else {
            f64::from(delta_lines.y) * f64::from(lh)
        };

        self.reveal = None;
        let moved = self.clamp_and_set_scroll_f64(self.scroll.x + dx, self.scroll.y + dy);
        if moved {
            let scroll = self.scroll_offset();
            if let Some(doc) = &mut self.doc {
                doc.scrollbar.on_wheel(scroll, Instant::now());
            }
        }
        moved
    }

    pub(crate) fn on_timer(&mut self, kind: EditorTimer, timers: &mut dyn EditorTimers) -> bool {
        let Some(doc) = &mut self.doc else {
            return matches!(kind, EditorTimer::Blink) && self.blink.on_timer(timers);
        };
        match kind {
            EditorTimer::Blink => self.blink.on_timer(timers),
            EditorTimer::AutoScroll => {
                let Some(delta) = doc.auto_scroll.delta else {
                    doc.auto_scroll.stop();
                    return false;
                };
                let drag_pos = doc.auto_scroll.last_drag_pos;
                self.reveal = None;
                self.clamp_and_set_scroll_f64(self.scroll.x, self.scroll.y + f64::from(delta));
                if let Some(pos) = drag_pos {
                    let (offset, aff) = self.hit_test_point(pos);
                    self.select_to_with_affinity(offset, aff);
                }
                true
            }
            EditorTimer::ScrollbarIdle => {
                doc.scrollbar.on_idle_timer_fired();
                true
            }
            EditorTimer::ScrollbarDrag => doc.scrollbar.on_drag_timer_fired(Instant::now()),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct EditorPaintStyle {
    pub(crate) text: TextStyle,
    pub(crate) line_height: f32,
    pub(crate) color: Rgba,
}

#[derive(Copy, Clone)]
pub(crate) struct EditorPaintParams<'a> {
    pub(crate) style: &'a EditorPaintStyle,
    pub(crate) palette: Palette,
    pub(crate) window_active: bool,
    pub(crate) focused: bool,
    pub(crate) show_selection: bool,
    pub(crate) scale: f32,
    pub(crate) caret_width_px: u32,
}

const TRACK_THICKNESS: f32 = 16.;
const MIN_THUMB_LENGTH: f32 = 48.;
const THUMB_MARGIN: f32 = 3.;
const THUMB_REST_THICKNESS: f32 = 6.;
const THUMB_ACTIVE_THICKNESS: f32 = 8.;
const THUMB_REST_RADIUS: f32 = 3.;
const THUMB_ACTIVE_RADIUS: f32 = 4.;

const IDLE_HOLD: Duration = Duration::from_secs(2);
const FADE_IN_DURATION: Duration = Duration::from_millis(120);
const FADE_OUT_DURATION: Duration = Duration::from_millis(200);
const EXPAND_DURATION: Duration = Duration::from_millis(120);
const DRAG_NOTIFY_INTERVAL: Duration = Duration::from_micros(8_333);

fn axis_ix(axis: Axis) -> usize {
    match axis {
        Axis::Vertical => 0,
        Axis::Horizontal => 1,
    }
}

fn axis_val(p: Point, axis: Axis) -> f32 {
    match axis {
        Axis::Vertical => p.y,
        Axis::Horizontal => p.x,
    }
}

fn set_axis_val(p: &mut Point, axis: Axis, v: f32) {
    match axis {
        Axis::Vertical => p.y = v,
        Axis::Horizontal => p.x = v,
    }
}

fn axis_size(s: Size, axis: Axis) -> f32 {
    match axis {
        Axis::Vertical => s.height,
        Axis::Horizontal => s.width,
    }
}

fn clamp_radius(radius: f32, width: f32, height: f32) -> f32 {
    radius.min(width * 0.5).min(height * 0.5).max(0.0)
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct TrackMetrics {
    margin: f32,
    pub(crate) thumb_len: f32,
    travel: f32,
    max_scroll: f32,
}

impl TrackMetrics {
    pub(crate) fn compute(viewport_span: f32, content_span: f32, corner_reserve: f32) -> Self {
        let track_span = (viewport_span - corner_reserve).max(0.);
        let raw_thumb = (viewport_span / content_span * viewport_span)
            .max(MIN_THUMB_LENGTH)
            .min(track_span);
        let margin = THUMB_MARGIN.clamp(0., raw_thumb * 0.5);
        Self {
            margin,
            thumb_len: raw_thumb - margin * 2.,
            travel: track_span - raw_thumb,
            max_scroll: content_span - viewport_span,
        }
    }

    pub(crate) fn thumb_local(self, scroll_offset: f32) -> f32 {
        let ratio = (-scroll_offset / self.max_scroll).clamp(0., 1.);
        self.margin + ratio * self.travel
    }

    pub(crate) fn apply_drag(
        self,
        axis: Axis,
        track_origin: f32,
        pointer: Point,
        grab: f32,
        mut offset: Point,
    ) -> Point {
        if self.travel <= 0. {
            return offset;
        }
        let pos = axis_val(pointer, axis);
        let ratio = ((pos - track_origin - self.margin - grab) / self.travel).clamp(0., 1.);
        set_axis_val(&mut offset, axis, -self.max_scroll * ratio);
        offset
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ScrollbarViewport {
    pub(crate) size: Size,
    pub(crate) content_size: Size,
    pub(crate) scroll_offset: Point,
    pub(crate) horizontal_left: f32,
    pub(crate) horizontal: bool,
}

#[derive(Clone, Copy, Debug)]
struct Drag {
    axis: Axis,
    grab: f32,
}

#[derive(Clone, Copy, Debug)]
struct ActiveAxis {
    metrics: TrackMetrics,
    viewport_span: f32,
    thumb_local: f32,
    interactive: bool,
}

pub(crate) struct ScrollbarState {
    drag: Option<Drag>,
    last_offset: Point,
    last_activity: Option<Instant>,
    was_hovered: bool,
    last_drag_notify: Instant,
    drag_timer: Option<TimerHandle>,
    idle_timer: Option<TimerHandle>,
    axes: [Option<ActiveAxis>; 2],
}

impl Default for ScrollbarState {
    fn default() -> Self {
        Self::new_at(Instant::now())
    }
}

impl ScrollbarState {
    fn new_at(now: Instant) -> Self {
        Self {
            drag: None,
            last_offset: Point::ZERO,
            last_activity: None,
            was_hovered: false,
            last_drag_notify: now,
            drag_timer: None,
            idle_timer: None,
            axes: [None, None],
        }
    }

    pub(crate) fn drag_axis(&self) -> Option<Axis> {
        self.drag.map(|d| d.axis)
    }

    pub(crate) fn is_interactive(&self, axis: Axis) -> bool {
        self.axes[axis_ix(axis)].is_some_and(|a| a.interactive)
    }

    pub(crate) fn on_idle_timer_fired(&mut self) {
        self.idle_timer = None;
    }

    pub(crate) fn on_drag_timer_fired(&mut self, now: Instant) -> bool {
        if self.drag_timer.take().is_none() {
            return false;
        }
        self.last_drag_notify = now;
        true
    }

    pub(crate) fn on_wheel(&mut self, offset: Point, now: Instant) -> bool {
        if offset == self.last_offset {
            return false;
        }
        self.last_offset = offset;
        self.last_activity = Some(now);
        self.idle_timer = None;
        true
    }

    pub(crate) fn render(
        &mut self,
        slot: EditorSlot,
        vp: ScrollbarViewport,
        p: Palette,
        now: Instant,
        ui: &mut AppUi,
        schedule_idle: impl FnOnce(Duration) -> Option<TimerHandle>,
    ) -> Vec<AppNode> {
        let scroll_offset = vp.scroll_offset;
        if scroll_offset != self.last_offset {
            self.last_offset = scroll_offset;
            self.last_activity = Some(now);
            self.idle_timer = None;
        }

        let is_axis_hovered = |axis: Axis, ui: &AppUi| {
            ui.is_hovered(HitId::ScrollTrack(slot, axis))
                || ui.is_hovered(HitId::ScrollThumb(slot, axis))
        };
        let hovered =
            is_axis_hovered(Axis::Vertical, ui) || is_axis_hovered(Axis::Horizontal, ui);
        if hovered || (self.was_hovered && !hovered) {
            self.last_activity = Some(now);
            self.idle_timer = None;
        }
        self.was_hovered = hovered;

        let visible = hovered
            || self.drag.is_some()
            || self
                .last_activity
                .is_some_and(|t| now.saturating_duration_since(t) < IDLE_HOLD);

        if !hovered
            && self.drag.is_none()
            && self.idle_timer.is_none()
            && let Some(last) = self.last_activity
        {
            let elapsed = now.saturating_duration_since(last);
            if elapsed < IDLE_HOLD {
                self.idle_timer = schedule_idle(IDLE_HOLD - elapsed);
            }
        }

        let vertical_needed = vp.size.height > 0. && vp.content_size.height > vp.size.height;
        let horizontal_needed =
            vp.horizontal && vp.size.width > 0. && vp.content_size.width > vp.size.width;
        self.axes = [None, None];
        if !vertical_needed && !horizontal_needed {
            return Vec::new();
        }

        let (fade_dur, fade_ease) = if visible {
            (FADE_IN_DURATION, Easing::Linear)
        } else {
            (FADE_OUT_DURATION, Easing::EaseInCubic)
        };
        let opacity = ui
            .animate_f32_from(
                AnimKey::ScrollOpacity(slot),
                0.0,
                if visible { 1.0 } else { 0.0 },
                fade_dur,
                fade_ease,
            )
            .value;
        let interactive = visible || opacity > 0.0;

        let mut nodes = Vec::with_capacity(2);
        for (needed, axis) in [
            (horizontal_needed, Axis::Horizontal),
            (vertical_needed, Axis::Vertical),
        ] {
            if !needed {
                continue;
            }
            let viewport_span = axis_size(vp.size, axis);
            let content_span = axis_size(vp.content_size, axis);
            let corner_reserve = if axis == Axis::Horizontal && vertical_needed {
                TRACK_THICKNESS
            } else {
                0.
            };
            let metrics = TrackMetrics::compute(viewport_span, content_span, corner_reserve);
            let thumb_local = metrics.thumb_local(axis_val(scroll_offset, axis));
            self.axes[axis_ix(axis)] = Some(ActiveAxis {
                metrics,
                viewport_span,
                thumb_local,
                interactive,
            });

            let is_dragged = self.drag.is_some_and(|d| d.axis == axis);
            let is_thumb_hovered = interactive && ui.is_hovered(HitId::ScrollThumb(slot, axis));
            let (thumb_color, target_thickness, target_radius) = if is_dragged {
                (p.text_primary, THUMB_ACTIVE_THICKNESS, THUMB_ACTIVE_RADIUS)
            } else if is_thumb_hovered {
                (p.text_secondary, THUMB_ACTIVE_THICKNESS, THUMB_ACTIVE_RADIUS)
            } else {
                (p.text_tertiary, THUMB_REST_THICKNESS, THUMB_REST_RADIUS)
            };
            let thickness = ui
                .animate_f32_from(
                    AnimKey::ScrollThickness(slot, axis),
                    THUMB_REST_THICKNESS,
                    target_thickness,
                    EXPAND_DURATION,
                    Easing::EaseOutCubic,
                )
                .value;

            let thumb_node = (opacity > 0.0).then(|| {
                let (pill_w, pill_h) = match axis {
                    Axis::Vertical => (thickness, metrics.thumb_len),
                    Axis::Horizontal => (metrics.thumb_len, thickness),
                };
                let radius = clamp_radius(target_radius, pill_w, pill_h);
                let pill = match axis {
                    Axis::Vertical => box_()
                        .absolute()
                        .right(THUMB_MARGIN)
                        .top(0.)
                        .bottom(0.)
                        .w(thickness),
                    Axis::Horizontal => box_()
                        .absolute()
                        .bottom(THUMB_MARGIN)
                        .left(0.)
                        .right(0.)
                        .h(thickness),
                }
                .rounded(radius)
                .bg(thumb_color.opacity(opacity));

                let thumb = match axis {
                    Axis::Vertical => box_()
                        .absolute()
                        .left(0.)
                        .right(0.)
                        .top(thumb_local)
                        .h(metrics.thumb_len),
                    Axis::Horizontal => box_()
                        .absolute()
                        .top(0.)
                        .bottom(0.)
                        .left(thumb_local)
                        .w(metrics.thumb_len),
                };
                thumb
                    .hit(HitId::ScrollThumb(slot, axis))
                    .cursor(CursorStyle::Arrow)
                    .child(pill)
            });

            let track = match axis {
                Axis::Vertical => box_()
                    .absolute()
                    .right(0.)
                    .top(0.)
                    .bottom(0.)
                    .w(TRACK_THICKNESS),
                Axis::Horizontal => box_()
                    .absolute()
                    .left(vp.horizontal_left)
                    .right(corner_reserve)
                    .bottom(0.)
                    .h(TRACK_THICKNESS),
            };
            nodes.push(
                track
                    .hit(HitId::ScrollTrack(slot, axis))
                    .when(interactive, |n| n.cursor(CursorStyle::Arrow))
                    .when_some(thumb_node, |n, t| n.child(t)),
            );
        }
        nodes
    }

    pub(crate) fn on_mouse_down(
        &mut self,
        axis: Axis,
        hit_thumb: bool,
        position: Point,
        track_rect: Rect,
        current_offset: Point,
    ) -> Option<Point> {
        let active = self.axes[axis_ix(axis)]?;
        if !active.interactive {
            return None;
        }
        let track_origin = axis_val(track_rect.origin, axis);
        let pointer = axis_val(position, axis);
        let thumb_origin = track_origin + active.thumb_local;
        if hit_thumb {
            let grab = pointer - thumb_origin;
            self.drag = Some(Drag { axis, grab });
            Some(
                active
                    .metrics
                    .apply_drag(axis, track_origin, position, grab, current_offset),
            )
        } else {
            let page = (active.viewport_span - 24.0).max(24.0);
            let delta = if pointer < thumb_origin { page } else { -page };
            let mut next = current_offset;
            let val = axis_val(next, axis) + delta;
            set_axis_val(&mut next, axis, val);
            Some(next)
        }
    }

    pub(crate) fn on_mouse_move(
        &mut self,
        position: Point,
        dragging_primary: bool,
        track_rect: Option<Rect>,
        current_offset: Point,
        schedule_drag: impl FnOnce(Duration) -> Option<TimerHandle>,
    ) -> Option<(Option<Point>, bool)> {
        let drag = self.drag.filter(|_| dragging_primary)?;
        let active = self.axes[axis_ix(drag.axis)]?;
        let track_origin = axis_val(track_rect?.origin, drag.axis);
        let now = Instant::now();
        let next = active
            .metrics
            .apply_drag(drag.axis, track_origin, position, drag.grab, current_offset);
        let moved = next != current_offset;
        if moved {
            let elapsed = now.saturating_duration_since(self.last_drag_notify);
            if elapsed >= DRAG_NOTIFY_INTERVAL {
                self.last_drag_notify = now;
                self.drag_timer = None;
                return Some((Some(next), true));
            }
            if self.drag_timer.is_none() {
                self.drag_timer = schedule_drag(DRAG_NOTIFY_INTERVAL - elapsed);
            }
        }
        Some((moved.then_some(next), false))
    }

    pub(crate) fn on_mouse_up(
        &mut self,
        position: Point,
        track_rect: Option<Rect>,
        current_offset: Point,
    ) -> Option<Point> {
        let drag = self.drag.take()?;
        self.drag_timer = None;
        self.last_activity = Some(Instant::now());
        let active = self.axes[axis_ix(drag.axis)]?;
        let track_origin = axis_val(track_rect?.origin, drag.axis);
        Some(
            active
                .metrics
                .apply_drag(drag.axis, track_origin, position, drag.grab, current_offset),
        )
    }
}

#[cfg(test)]
mod tests {
    use crate::geom::{point, rect, size};
    use crate::ui::TabId;

    use super::*;

    #[test]
    fn editor_direct_typing_undo_redo_and_word_navigation() {
        let ts = TextSystem::new().unwrap();
        let mut timers = NoopTimers;
        let mut ed = Editor::document(
            Rope::new(),
            DocOptions {
                soft_wrap: true,
                paddings: Edges::ZERO,
            },
        );

        ed.handle_text_input("hello", &ts, &mut timers);
        ed.handle_text_input(" world", &ts, &mut timers);
        assert_eq!(ed.model().text().to_string(), "hello world");
        assert!(!ed.model().is_clean());

        assert!(ed.handle_command(EditCommand::Undo, &ts, &mut timers));
        assert_eq!(ed.model().text().to_string(), "");
        assert!(ed.model().is_clean());

        assert!(ed.handle_command(EditCommand::Redo, &ts, &mut timers));
        assert_eq!(ed.model().text().to_string(), "hello world");

        assert!(ed.handle_command(
            EditCommand::Move {
                motion: Motion::PrevWord,
                extend: false,
            },
            &ts,
            &mut timers,
        ));
        assert_eq!(ed.model().cursor(), 6);
        assert!(ed.handle_command(
            EditCommand::Move {
                motion: Motion::NextWord,
                extend: true,
            },
            &ts,
            &mut timers,
        ));
        assert_eq!(ed.model().text().slice(ed.model().selection().range()).to_string(), "world");

        let copied = ed.selected_string();
        assert_eq!(copied.as_deref(), Some("world"));
        let cut = ed.prepare_cut();
        assert_eq!(cut.as_deref(), Some("world"));
        ed.delete_selection(&ts, &mut timers);
        assert_eq!(ed.model().text().to_string(), "hello ");
        ed.paste("world", &ts, &mut timers);
        assert_eq!(ed.model().text().to_string(), "hello world");
    }

    #[test]
    fn auto_scroll_delta_has_dead_zone_and_symmetric_ramps() {
        let bounds = rect(0., 100., 200., 100.);
        assert_eq!(AutoScroll::compute_delta(150., bounds), None);

        let top = AutoScroll::compute_delta(80., bounds).unwrap();
        let bottom = AutoScroll::compute_delta(220., bounds).unwrap();
        assert!(top < 0. && bottom > 0.);
        assert!((top.abs() - bottom.abs()).abs() < 1e-3);

        let mut scroll = AutoScroll::default();
        scroll.stop();
        assert!(scroll.delta.is_none());
    }

    #[test]
    fn thumb_radius_is_limited_by_its_actual_bounds() {
        assert_eq!(clamp_radius(6., 8., 80.), 4.);
        assert_eq!(clamp_radius(6., 80., 6.), 3.);
        assert_eq!(clamp_radius(0., 8., 80.), 0.);
    }

    #[test]
    fn full_track_thumb_preserves_scroll_offset() {
        let metrics = TrackMetrics::compute(200., 250., 0.);
        assert!(metrics.travel > 0.);
        let dragged = metrics.apply_drag(Axis::Vertical, 0., point(0., 100.), 10., point(-5., 0.));
        assert_eq!(dragged.x, -5.);
        assert!(dragged.y < 0.);

        let zero_travel = TrackMetrics::compute(40., 100., 0.);
        assert_eq!(zero_travel.travel, 0.);
        let kept = zero_travel.apply_drag(Axis::Vertical, 0., point(0., 20.), 5., point(0., -12.));
        assert_eq!(kept.y, -12.);
    }

    #[test]
    fn scene_scrollbar_renders_fades_and_snaps_on_reduce_motion() {
        let t0 = Instant::now();
        let mut sb = ScrollbarState::new_at(t0);
        let mut ui = AppUi::default();
        let p = Palette::resolve(false, None, None);
        let slot = EditorSlot::Tab(TabId::next());
        let vp = ScrollbarViewport {
            size: size(400., 200.),
            content_size: size(400., 1000.),
            scroll_offset: point(0., -40.),
            horizontal_left: 0.,
            horizontal: false,
        };

        sb.on_wheel(point(0., -40.), t0);
        ui.begin_frame(t0, false);
        let nodes = sb.render(slot, vp, p, t0, &mut ui, |_| None);
        assert_eq!(nodes.len(), 1);
        assert!(sb.is_interactive(Axis::Vertical));
        assert!(ui.finish_frame(crate::ui::AppFrame::default()));

        ui.begin_frame(t0 + FADE_IN_DURATION, false);
        let _ = sb.render(
            slot,
            vp,
            p,
            t0 + FADE_IN_DURATION,
            &mut ui,
            |_| None,
        );
        assert!(!ui.finish_frame(crate::ui::AppFrame::default()));

        let after_idle = t0 + IDLE_HOLD + Duration::from_millis(10);
        ui.begin_frame(after_idle, true);
        let _ = sb.render(slot, vp, p, after_idle, &mut ui, |_| None);
        assert!(!sb.is_interactive(Axis::Vertical));
    }
}
