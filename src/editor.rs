use std::borrow::Cow;
use std::ops::Range;
use std::time::{Duration, Instant};

use ropey::{LineType, RopeBuilder, RopeSlice};
use unicode_segmentation::{GraphemeCursor, GraphemeIncomplete, UnicodeSegmentation};

use crate::geom::{Axis, Edges, Point, Rect, Rgba, Size, point, size};
use crate::gfx::{Canvas, FontFace, ShapedRow, TextStyle, TextSystem, fill};
use crate::platform::{CursorStyle, Modifiers, MouseButton, TimerHandle};
use crate::theme::Palette;
use crate::types::SharedStr;
use crate::ui::{AnimKey, AppNode, AppUi, Easing, EditorSlot, HitId, Node, box_, custom, row};

pub(crate) use ropey::Rope;

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
    caret_height: f32,
    caret_hidden: bool,
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

    #[inline]
    pub(crate) fn model(&self) -> &TextModel {
        &self.model
    }

    #[inline]
    fn fresh_frame(&self) -> Option<&FrameLayout> {
        self.frame
            .as_ref()
            .filter(|f| f.revision == self.model.revision())
    }

    #[inline]
    pub(crate) fn caret_bounds(&self) -> Option<Rect> {
        self.fresh_frame().map(|f| f.caret_rect)
    }

    pub(crate) fn set_caret_height(&mut self, height: f32) {
        self.caret_height = height;
    }

    pub(crate) fn set_caret_hidden(&mut self, hidden: bool) {
        self.caret_hidden = hidden;
    }

    #[cfg(test)]
    #[inline]
    pub(crate) fn soft_wrap_enabled(&self) -> bool {
        self.doc.as_deref().is_some_and(|d| d.soft_wrap)
    }

    #[inline]
    pub(crate) fn scroll_offset(&self) -> Point {
        point(self.scroll.x, self.scroll.y as f32)
    }

    #[inline]
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

    #[inline]
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

    #[inline]
    pub(crate) fn is_selecting(&self) -> bool {
        self.selecting
    }

    #[inline]
    pub(crate) fn scrollbar_drag_axis(&self) -> Option<Axis> {
        self.doc.as_deref().and_then(|d| d.scrollbar.drag_axis())
    }

    #[inline]
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
    #[inline]
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
        (!self.model.selection().is_empty()).then(|| self.model.selected_text().to_string())
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
            EditCommand::IndentInline | EditCommand::Indent => {
                let Some(change) = self.model.indent(matches!(command, EditCommand::Indent)) else {
                    return false;
                };
                self.commit_edit(change, ts, timers);
            }
            EditCommand::Outdent => {
                let Some(change) = self.model.outdent() else {
                    return false;
                };
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
            .bounds_for_range(self.model.text(), self.model.selected_range())
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

#[inline]
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
            .then(|| self.model.composition_range())
            .flatten();
        if params.window_active
            && params.show_selection
            && self.model.composition().is_none()
            && !self.model.selection().is_empty()
        {
            frame.selection_rects = frame.range_rects(&self.model.selected_range());
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
            (self.model.value().into(), style.color)
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

const WORD_SCAN_WINDOW_BYTES: usize = 256;
const GRAPHEME_SCAN_WINDOW_BYTES: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Bias {
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct TextPoint {
    pub(crate) row: usize,
    pub(crate) column: usize,
}

impl TextPoint {
    #[inline]
    pub(crate) fn new(row: usize, column: usize) -> Self {
        Self { row, column }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Default)]
pub(crate) struct Selection {
    pub(crate) anchor: usize,
    pub(crate) head: usize,
    pub(crate) end_affinity: bool,
}

impl Selection {
    #[inline]
    pub(crate) fn caret(offset: usize) -> Self {
        Self {
            anchor: offset,
            head: offset,
            end_affinity: false,
        }
    }

    #[inline]
    pub(crate) fn from_range(range: Range<usize>) -> Self {
        Self {
            anchor: range.start,
            head: range.end,
            end_affinity: false,
        }
    }

    #[inline]
    pub(crate) fn start(&self) -> usize {
        self.anchor.min(self.head)
    }

    #[inline]
    pub(crate) fn end(&self) -> usize {
        self.anchor.max(self.head)
    }

    #[inline]
    pub(crate) fn range(&self) -> Range<usize> {
        self.start()..self.end()
    }

    #[inline]
    pub(crate) fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    #[inline]
    pub(crate) fn contains(&self, offset: usize) -> bool {
        offset >= self.start() && offset < self.end()
    }

    #[inline]
    pub(crate) fn place(&mut self, offset: usize, end_affinity: bool) {
        self.anchor = offset;
        self.head = offset;
        self.end_affinity = end_affinity;
    }

    pub(crate) fn extend_to(
        &mut self,
        offset: usize,
        end_affinity: bool,
        anchor_word: Option<Range<usize>>,
        max_len: usize,
    ) {
        let offset = offset.min(max_len);
        self.anchor = self.anchor.min(max_len);
        self.head = offset;
        self.end_affinity = end_affinity;
        if let Some(word) = anchor_word {
            let ws = word.start.min(max_len);
            let we = word.end.min(max_len);
            if offset < ws {
                self.anchor = we;
                self.head = offset;
            } else if offset > we {
                self.anchor = ws;
                self.head = offset;
            } else {
                self.anchor = ws;
                self.head = we;
            }
        }
    }
}

pub(crate) trait RopeExt {
    fn slice_line(&self, row: usize) -> RopeSlice<'_>;
    fn line_len(&self, row: usize) -> usize;
    fn line_start_offset(&self, row: usize) -> usize;
    fn line_end_offset(&self, row: usize) -> usize;
    fn lines_len(&self) -> usize;
    fn char_at(&self, offset: usize) -> Option<char>;
    fn offset_to_point(&self, offset: usize) -> TextPoint;
    fn point_to_offset(&self, point: TextPoint) -> usize;
    fn clip_offset(&self, offset: usize, bias: Bias) -> usize;
    fn replace(&mut self, range: Range<usize>, text: &str);
}

impl RopeExt for Rope {
    fn slice_line(&self, row: usize) -> RopeSlice<'_> {
        if row >= self.lines_len() {
            return self.slice(0..0);
        }
        let line = self.line(row, LineType::LF);
        if line.len() > 0 && line.byte(line.len() - 1) == b'\n' {
            line.slice(..line.len() - 1)
        } else {
            line
        }
    }

    fn line_len(&self, row: usize) -> usize {
        self.slice_line(row).len()
    }

    fn line_start_offset(&self, row: usize) -> usize {
        self.point_to_offset(TextPoint::new(row, 0))
    }

    fn line_end_offset(&self, row: usize) -> usize {
        if row >= self.lines_len() {
            return self.len();
        }
        self.line_start_offset(row) + self.line_len(row)
    }

    fn lines_len(&self) -> usize {
        self.len_lines(LineType::LF)
    }

    fn char_at(&self, offset: usize) -> Option<char> {
        (offset < self.len())
            .then(|| self.get_char(offset).ok())
            .flatten()
    }

    fn offset_to_point(&self, offset: usize) -> TextPoint {
        let clamped = self.clip_offset(offset, Bias::Left);
        let row = self.byte_to_line_idx(clamped, LineType::LF);
        let start = self.line_to_byte_idx(row, LineType::LF);
        TextPoint::new(row, clamped.saturating_sub(start))
    }

    fn point_to_offset(&self, point: TextPoint) -> usize {
        if point.row >= self.lines_len() {
            return self.len();
        }
        self.line_to_byte_idx(point.row, LineType::LF) + point.column
    }

    fn clip_offset(&self, offset: usize, bias: Bias) -> usize {
        if offset >= self.len() {
            return self.len();
        }
        if self.is_char_boundary(offset) {
            return offset;
        }
        match bias {
            Bias::Left => self.floor_char_boundary(offset),
            Bias::Right => self.ceil_char_boundary(offset),
        }
    }

    fn replace(&mut self, range: Range<usize>, text: &str) {
        let start = self.clip_offset(range.start, Bias::Left);
        let end = self.clip_offset(range.end, Bias::Right);
        self.remove(start..end);
        self.insert(start, text);
    }
}

pub(crate) fn line_range_at(text: &Rope, offset: usize) -> Range<usize> {
    let clamped = text.clip_offset(offset, Bias::Left);
    let row = text.offset_to_point(clamped).row;
    text.line_start_offset(row)..text.line_end_offset(row)
}

pub(crate) fn prev_grapheme_boundary(text: &Rope, offset: usize) -> usize {
    let len = text.len();
    let off = text.clip_offset(offset, Bias::Left);
    if off == 0 {
        return 0;
    }
    let prev_b = text.byte(off - 1);
    if (0x20..=0x7E).contains(&prev_b) && (off == 1 || text.byte(off - 2) < 0x80) {
        return off - 1;
    }
    let mut span = GRAPHEME_SCAN_WINDOW_BYTES;
    loop {
        let win_start = text.floor_char_boundary(off.saturating_sub(span));
        let win_end = text.ceil_char_boundary(off.saturating_add(32).min(len));
        let chunk = text.slice(win_start..win_end).to_string();
        let mut cursor = GraphemeCursor::new(off, len, true);
        match cursor.prev_boundary(&chunk, win_start) {
            Ok(Some(pos)) => return pos,
            Ok(None) => return 0,
            Err(GraphemeIncomplete::PrevChunk | GraphemeIncomplete::PreContext(_))
                if win_start > 0 =>
            {
                span = span.saturating_mul(2);
            }
            Err(_) => return text.floor_char_boundary(off.saturating_sub(1)),
        }
    }
}

pub(crate) fn next_grapheme_boundary(text: &Rope, offset: usize) -> usize {
    let len = text.len();
    let off = text.clip_offset(offset, Bias::Right);
    if off >= len {
        return len;
    }
    let b = text.byte(off);
    if (0x20..=0x7E).contains(&b) && (off + 1 == len || text.byte(off + 1) < 0x80) {
        return off + 1;
    }
    let mut span = GRAPHEME_SCAN_WINDOW_BYTES;
    loop {
        let win_start = text.floor_char_boundary(off.saturating_sub(span));
        let win_end = text.ceil_char_boundary(off.saturating_add(span).min(len));
        let chunk = text.slice(win_start..win_end).to_string();
        let mut cursor = GraphemeCursor::new(off, len, true);
        match cursor.next_boundary(&chunk, win_start) {
            Ok(Some(pos)) => return pos,
            Ok(None) => return len,
            Err(GraphemeIncomplete::NextChunk | GraphemeIncomplete::PreContext(_))
                if win_start > 0 || win_end < len =>
            {
                span = span.saturating_mul(2);
            }
            Err(_) => return text.ceil_char_boundary((off + 1).min(len)),
        }
    }
}

pub(crate) fn snap_grapheme_boundary(text: &Rope, offset: usize, bias: Bias) -> usize {
    let len = text.len();
    let off = text.clip_offset(offset, bias);
    if off == 0 || off >= len {
        return off;
    }
    let prev_b = text.byte(off - 1);
    let curr_b = text.byte(off);
    if prev_b < 0x80 && curr_b < 0x80 && !(prev_b == b'\r' && curr_b == b'\n') {
        return off;
    }
    let mut span = GRAPHEME_SCAN_WINDOW_BYTES;
    loop {
        let win_start = text.floor_char_boundary(off.saturating_sub(span));
        let win_end = text.ceil_char_boundary(off.saturating_add(span).min(len));
        let chunk = text.slice(win_start..win_end).to_string();
        let mut cursor = GraphemeCursor::new(off, len, true);
        match cursor.is_boundary(&chunk, win_start) {
            Ok(true) => return off,
            Ok(false) => {
                return match bias {
                    Bias::Left => prev_grapheme_boundary(text, off),
                    Bias::Right => next_grapheme_boundary(text, off),
                };
            }
            Err(GraphemeIncomplete::PreContext(_)) if win_start > 0 => {
                span = span.saturating_mul(2);
            }
            Err(_) => return off,
        }
    }
}

#[inline]
fn is_horizontal_ws(ch: char) -> bool {
    ch.is_whitespace() && !matches!(ch, '\n' | '\r')
}

pub(crate) fn word_range_at(text: &Rope, offset: usize) -> Option<Range<usize>> {
    let line = line_range_at(text, offset);
    if line.is_empty() {
        return None;
    }
    let mut pos = text.clip_offset(offset, Bias::Left);
    if pos >= line.end {
        pos = prev_grapheme_boundary(text, line.end);
    }
    if pos < line.start || pos >= line.end {
        return None;
    }

    let ch = text.char_at(pos)?;
    if is_horizontal_ws(ch) {
        let mut left = pos;
        for prev in text.chars_at(pos).reversed() {
            if left <= line.start || !is_horizontal_ws(prev) {
                break;
            }
            left -= prev.len_utf8();
        }
        let mut right = pos + ch.len_utf8();
        for next in text.chars_at(right) {
            if right >= line.end || !is_horizontal_ws(next) {
                break;
            }
            right += next.len_utf8();
        }
        return Some(left..right);
    }

    let mut span = WORD_SCAN_WINDOW_BYTES;
    loop {
        let win_start = text
            .floor_char_boundary(pos.saturating_sub(span))
            .max(line.start);
        let win_end = text
            .ceil_char_boundary(pos.saturating_add(span).min(line.end))
            .min(line.end);
        let chunk = text.slice(win_start..win_end).to_string();
        let rel_pos = pos - win_start;
        for (seg_start, seg) in UnicodeSegmentation::split_word_bound_indices(chunk.as_str()) {
            let seg_end = seg_start + seg.len();
            if rel_pos >= seg_start && rel_pos < seg_end {
                let touches_left = win_start > line.start && chunk[..seg_start].chars().count() < 4;
                let touches_right = win_end < line.end && chunk[seg_end..].chars().count() < 4;
                if !touches_left && !touches_right {
                    return Some((win_start + seg_start)..(win_start + seg_end));
                }
                break;
            }
        }
        if win_start == line.start && win_end == line.end {
            return Some(pos..(pos + ch.len_utf8()));
        }
        span = span.saturating_mul(2);
    }
}

pub(crate) fn prev_word_start(text: &Rope, offset: usize) -> usize {
    let offset = text.floor_char_boundary(offset.min(text.len()));
    if offset == 0 {
        return 0;
    }
    if text.byte(offset - 1) == b'\n' {
        return offset - 1;
    }
    let row = text.offset_to_point(offset).row;
    let line_start = text.line_start_offset(row);

    let mut end = offset;
    for ch in text.chars_at(offset).reversed() {
        if end <= line_start || !is_horizontal_ws(ch) {
            break;
        }
        end -= ch.len_utf8();
    }
    if end <= line_start {
        return line_start;
    }

    let mut span = WORD_SCAN_WINDOW_BYTES;
    loop {
        let win_start = text
            .floor_char_boundary(end.saturating_sub(span))
            .max(line_start);
        let chunk = text.slice(win_start..end).to_string();
        if let Some((rel, _)) = UnicodeSegmentation::split_word_bound_indices(chunk.as_str())
            .rfind(|(_, piece)| !piece.chars().all(is_horizontal_ws))
            && (win_start == line_start || (rel > 0 && chunk[..rel].chars().count() >= 4))
        {
            return win_start + rel;
        }
        if win_start == line_start {
            return line_start;
        }
        span = span.saturating_mul(2);
    }
}

pub(crate) fn next_word_end(text: &Rope, offset: usize) -> usize {
    let len = text.len();
    let offset = text.floor_char_boundary(offset.min(len));
    if offset >= len {
        return len;
    }
    if text.byte(offset) == b'\n' {
        return offset + 1;
    }
    let row = text.offset_to_point(offset).row;
    let line_end = text.line_end_offset(row);
    if offset >= line_end {
        return (line_end + 1).min(len);
    }

    let mut start = offset;
    if text.char_at(start).is_some_and(is_horizontal_ws) {
        for ch in text.chars_at(start) {
            if start >= line_end || !is_horizontal_ws(ch) {
                break;
            }
            start += ch.len_utf8();
        }
        return start.min(line_end);
    }

    let mut span = WORD_SCAN_WINDOW_BYTES;
    let word_end = loop {
        let win_end = text
            .ceil_char_boundary(start.saturating_add(span).min(line_end))
            .min(line_end);
        let chunk = text.slice(start..win_end).to_string();
        if let Some((rel, piece)) = UnicodeSegmentation::split_word_bound_indices(chunk.as_str())
            .find(|(_, piece)| !piece.chars().all(is_horizontal_ws))
        {
            let end = rel + piece.len();
            if win_end == line_end || chunk[end..].chars().count() >= 4 {
                break start + end;
            }
        }
        if win_end == line_end {
            break line_end;
        }
        span = span.saturating_mul(2);
    };

    let mut pos = word_end.min(line_end);
    for ch in text.chars_at(pos) {
        if pos >= line_end || !is_horizontal_ws(ch) {
            break;
        }
        pos += ch.len_utf8();
    }
    pos.min(line_end)
}

const BULK_REBUILD_THRESHOLD: usize = 64;
const SNAPSHOT_HISTORY_THRESHOLD: usize = 1024;

pub(crate) type ValidateFn = fn(&str) -> bool;

#[derive(Clone, Copy, Debug)]
pub(crate) struct EditPolicy {
    pub(crate) multi_line: bool,
    pub(crate) validate: Option<ValidateFn>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LineChange {
    Splice {
        old_lines: Range<usize>,
        new_lines: Range<usize>,
    },
    Full,
}

#[derive(Clone, Copy, Debug)]
enum SelectionAfter {
    AfterLastEdit,
    Remap,
    Explicit(Selection),
}

enum Record {
    Undoable(EditKind),
    CommitPreedit(Composition),
    Preedit {
        replaced_text: String,
        sel_before: Selection,
        cursor_byte: usize,
    },
    ResetHistory,
    Untracked,
}

enum EditSource {
    Changes(ChangeSet),
    ReplaceRope(Rope),
}

struct Edit {
    source: EditSource,
    selection: SelectionAfter,
    record: Record,
    validate: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Composition {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) cursor: usize,
    replaced_text: String,
    sel_before: Selection,
}

pub(crate) struct TextModel {
    text: Rope,
    policy: EditPolicy,
    selection: Selection,
    composition: Option<Composition>,
    history: History,
    revision: u64,
}

impl TextModel {
    pub(crate) fn new(policy: EditPolicy) -> Self {
        Self {
            text: Rope::new(),
            policy,
            selection: Selection::default(),
            composition: None,
            history: History::new(),
            revision: 0,
        }
    }

    pub(crate) fn with_rope(mut self, rope: Rope) -> Self {
        self.text = rope;
        self
    }

    #[inline]
    pub(crate) fn is_multi_line(&self) -> bool {
        self.policy.multi_line
    }

    #[inline]
    pub(crate) fn text(&self) -> &Rope {
        &self.text
    }

    #[inline]
    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }

    #[inline]
    pub(crate) fn selection(&self) -> Selection {
        self.selection
    }

    #[inline]
    pub(crate) fn composition(&self) -> Option<&Composition> {
        self.composition.as_ref()
    }

    pub(crate) fn composition_range(&self) -> Option<Range<usize>> {
        self.composition.as_ref().map(|c| c.start..c.end)
    }

    pub(crate) fn value(&self) -> String {
        self.text.to_string()
    }

    pub(crate) fn cursor(&self) -> usize {
        if let Some(comp) = &self.composition {
            return comp
                .start
                .saturating_add(comp.cursor)
                .clamp(comp.start, comp.end);
        }
        self.selection.head
    }

    pub(crate) fn selected_range(&self) -> Range<usize> {
        self.selection.range()
    }

    pub(crate) fn selected_text(&self) -> RopeSlice<'_> {
        self.text.slice(self.selection.range())
    }

    pub(crate) fn caret_line_col(&self) -> (usize, usize) {
        let offset = self.cursor();
        let pt = self.text.offset_to_point(offset);
        let line_start = self.text.line_start_offset(pt.row);
        let col = self
            .text
            .byte_to_char_idx(offset)
            .saturating_sub(self.text.byte_to_char_idx(line_start));
        (pt.row + 1, col + 1)
    }

    pub(crate) fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    pub(crate) fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    pub(crate) fn is_clean(&self) -> bool {
        self.history.is_clean() && self.composition.is_none()
    }

    pub(crate) fn begin_save(&mut self) -> u64 {
        self.commit_composition();
        self.history.begin_save()
    }

    pub(crate) fn mark_saved_version(&mut self, version: u64) {
        self.history.mark_saved(version);
    }

    pub(crate) fn break_coalescing(&mut self) {
        self.history.break_coalescing();
    }

    pub(crate) fn snap_boundary(&self, offset: usize, bias: Bias) -> usize {
        snap_grapheme_boundary(&self.text, offset, bias)
    }

    fn normalize<'a>(&self, text: &'a str) -> Cow<'a, str> {
        if !self.policy.multi_line {
            if text.contains(['\n', '\r']) {
                Cow::Owned(text.replace(['\n', '\r'], ""))
            } else {
                Cow::Borrowed(text)
            }
        } else if text.contains('\r') {
            Cow::Owned(text.replace("\r\n", "\n").replace('\r', "\n"))
        } else {
            Cow::Borrowed(text)
        }
    }

    fn accepts(&self, candidate: &str) -> bool {
        candidate.is_empty() || self.policy.validate.is_none_or(|f| f(candidate))
    }

    fn debug_assert_invariants(&self) {
        debug_assert!(self.selection.start() <= self.selection.end());
        debug_assert!(self.selection.end() <= self.text.len());
        debug_assert!(self.text.is_char_boundary(self.selection.anchor));
        debug_assert!(self.text.is_char_boundary(self.selection.head));
        if let Some(comp) = &self.composition {
            debug_assert!(comp.start <= comp.end && comp.end <= self.text.len());
            debug_assert!(self.text.is_char_boundary(comp.start));
            debug_assert!(self.text.is_char_boundary(comp.end));
        }
    }

    fn commit(&mut self, edit: Edit) -> Option<LineChange> {
        let sel_before = self.selection;
        let before_rope = self.text.clone();

        let (lines, applied_changes, next_selection) = match edit.source {
            EditSource::ReplaceRope(rope) => {
                self.text = rope;
                let sel = match edit.selection {
                    SelectionAfter::Explicit(sel) => sel,
                    SelectionAfter::AfterLastEdit | SelectionAfter::Remap => {
                        Selection::caret(self.text.len())
                    }
                };
                (LineChange::Full, None, sel)
            }
            EditSource::Changes(raw) => {
                if raw.is_empty() {
                    return None;
                }
                let changes = ChangeSet::new(
                    raw.changes
                        .into_iter()
                        .map(|c| {
                            let start = self.text.clip_offset(c.range.start, Bias::Left);
                            let end = self.text.clip_offset(c.range.end, Bias::Right);
                            Change {
                                range: start.min(end)..start.max(end),
                                text: self.normalize(&c.text).into_owned(),
                            }
                        })
                        .collect(),
                );
                let first_start = changes.changes[0].range.start;
                let last_end = changes.changes.last().unwrap().range.end;
                let old_start_line = self.text.offset_to_point(first_start).row;
                let old_end_line = self.text.offset_to_point(last_end).row;

                if changes.len() > BULK_REBUILD_THRESHOLD {
                    self.text = rebuild_rope(&self.text, &changes.changes);
                } else {
                    for c in changes.changes.iter().rev() {
                        self.text.replace(c.range.clone(), &c.text);
                    }
                }

                if edit.validate
                    && !self.policy.multi_line
                    && self.policy.validate.is_some()
                    && !self.accepts(&self.text.to_string())
                    && self.accepts(&before_rope.to_string())
                {
                    self.text = before_rope;
                    return None;
                }

                let mut shift: isize = 0;
                let mut last_new_end = first_start;
                for c in &changes.changes {
                    last_new_end = (c.range.start as isize + shift) as usize + c.text.len();
                    shift += c.text.len() as isize - (c.range.end - c.range.start) as isize;
                }
                let lines = if matches!(edit.record, Record::ResetHistory) {
                    LineChange::Full
                } else {
                    let new_start_line = self.text.offset_to_point(first_start).row;
                    let new_end_line = self.text.offset_to_point(last_new_end).row;
                    LineChange::Splice {
                        old_lines: old_start_line..old_end_line + 1,
                        new_lines: new_start_line..new_end_line + 1,
                    }
                };
                let sel = match edit.selection {
                    SelectionAfter::AfterLastEdit => Selection::caret(last_new_end),
                    SelectionAfter::Remap => Selection {
                        anchor: changes.remap_offset(sel_before.anchor),
                        head: changes.remap_offset(sel_before.head),
                        end_affinity: false,
                    },
                    SelectionAfter::Explicit(sel) => sel,
                };
                (lines, Some(changes), sel)
            }
        };

        self.revision = self.revision.wrapping_add(1);
        let mut snapped = next_selection;
        snapped.anchor = self.snap_boundary(snapped.anchor, Bias::Left);
        snapped.head = self.snap_boundary(snapped.head, Bias::Left);
        self.selection = snapped;

        match edit.record {
            Record::Preedit {
                replaced_text,
                sel_before,
                cursor_byte,
            } => {
                let change = &applied_changes.as_ref().unwrap().changes[0];
                let start = change.range.start;
                let end = start + change.text.len();
                let caret = self.snap_boundary(start + cursor_byte, Bias::Left);
                self.composition = Some(Composition {
                    start,
                    end,
                    cursor: caret.saturating_sub(start),
                    replaced_text,
                    sel_before,
                });
                self.selection = Selection::caret(caret);
            }
            Record::CommitPreedit(comp) => {
                self.composition = None;
                let change = &applied_changes.as_ref().unwrap().changes[0];
                let orig_end = comp.start + comp.replaced_text.len();
                let new_end = comp.start + change.text.len();
                self.history.push(Transaction {
                    kind: EditKind::Atomic,
                    payload: TxPayload::Changes {
                        undo: ChangeSet::single(comp.start..new_end, comp.replaced_text),
                        redo: ChangeSet::single(comp.start..orig_end, change.text.clone()),
                    },
                    selection_before: comp.sel_before,
                    selection_after: self.selection,
                });
            }
            Record::Undoable(kind) => {
                self.composition = None;
                if let Some(changes) = applied_changes {
                    let payload = if changes.len() > SNAPSHOT_HISTORY_THRESHOLD {
                        let delta_bytes: usize = changes
                            .changes
                            .iter()
                            .map(|c| (c.range.end - c.range.start) + c.text.len())
                            .sum();
                        TxPayload::Snapshot {
                            before: before_rope,
                            after: self.text.clone(),
                            delta_bytes,
                        }
                    } else {
                        let undo = invert_changes(&before_rope, &changes);
                        TxPayload::Changes {
                            undo,
                            redo: changes,
                        }
                    };
                    self.history.push(Transaction {
                        kind,
                        payload,
                        selection_before: sel_before,
                        selection_after: self.selection,
                    });
                }
            }
            Record::ResetHistory => {
                self.composition = None;
                self.history.clear();
            }
            Record::Untracked => {
                self.composition = None;
            }
        }

        self.debug_assert_invariants();
        Some(lines)
    }

    pub(crate) fn replace_all(
        &mut self,
        ranges: &[Range<usize>],
        replacement: &str,
    ) -> Option<LineChange> {
        self.commit_composition();
        if ranges.is_empty() {
            return None;
        }
        let replacement = self.normalize(replacement).into_owned();
        let changes = ChangeSet::new(
            ranges
                .iter()
                .map(|r| Change {
                    range: r.clone(),
                    text: replacement.clone(),
                })
                .collect(),
        );
        let next_cursor = changes.remap_offset(self.cursor());
        self.commit(Edit {
            source: EditSource::Changes(changes),
            selection: SelectionAfter::Explicit(Selection::caret(next_cursor)),
            record: Record::Undoable(EditKind::Atomic),
            validate: true,
        })
    }

    pub(crate) fn set_value(&mut self, value: &str) -> Option<LineChange> {
        self.composition = None;
        let normalized = self.normalize(value).into_owned();
        let target_offset = if self.policy.multi_line {
            0
        } else {
            normalized.len()
        };
        let full = 0..self.text.len();
        self.commit(Edit {
            source: EditSource::Changes(ChangeSet::single(full, normalized)),
            selection: SelectionAfter::Explicit(Selection::caret(target_offset)),
            record: Record::ResetHistory,
            validate: false,
        })
    }

    pub(crate) fn insert_text(&mut self, text: &str) -> Option<LineChange> {
        if let Some(comp) = self.composition.take() {
            let range = comp.start..comp.end;
            let res = self.commit(Edit {
                source: EditSource::Changes(ChangeSet::single(range, text)),
                selection: SelectionAfter::AfterLastEdit,
                record: Record::CommitPreedit(comp.clone()),
                validate: true,
            });
            if res.is_none() {
                self.composition = Some(comp);
            }
            return res;
        }

        let range = self.selected_range();
        let normalized = self.normalize(text).into_owned();
        let kind = if range.is_empty() && !normalized.is_empty() && !normalized.contains('\n') {
            EditKind::Insert
        } else {
            EditKind::Atomic
        };
        self.replace_range(range, normalized, kind)
    }

    pub(crate) fn replace_selection(&mut self, text: &str) -> Option<LineChange> {
        self.commit_composition();
        self.replace_range(self.selected_range(), text, EditKind::Atomic)
    }

    fn replace_range(
        &mut self,
        range: Range<usize>,
        text: impl Into<String>,
        kind: EditKind,
    ) -> Option<LineChange> {
        self.commit(Edit {
            source: EditSource::Changes(ChangeSet::single(range, text)),
            selection: SelectionAfter::AfterLastEdit,
            record: Record::Undoable(kind),
            validate: true,
        })
    }

    pub(crate) fn set_composition(&mut self, text: &str, cursor_utf16: usize) -> Option<LineChange> {
        if text.is_empty() {
            return self.cancel_composition();
        }
        let normalized = self.normalize(text).into_owned();
        if normalized.is_empty() {
            return self.cancel_composition();
        }
        let cursor_byte = utf16_offset_to_byte(&normalized, cursor_utf16);
        let (range, replaced_text, sel_before) = match &self.composition {
            Some(comp) => (comp.start..comp.end, comp.replaced_text.clone(), comp.sel_before),
            None => {
                let r = self.selected_range();
                let old = self.text.slice(r.clone()).to_string();
                (r, old, self.selection)
            }
        };
        self.commit(Edit {
            source: EditSource::Changes(ChangeSet::single(range, normalized)),
            selection: SelectionAfter::AfterLastEdit,
            record: Record::Preedit {
                replaced_text,
                sel_before,
                cursor_byte,
            },
            validate: true,
        })
    }

    pub(crate) fn commit_composition(&mut self) -> bool {
        let Some(comp) = self.composition.take() else {
            return false;
        };
        let committed = self.text.slice(comp.start..comp.end).to_string();
        self.selection = Selection::caret(self.snap_boundary(comp.end, Bias::Left));
        let orig_end = comp.start + comp.replaced_text.len();
        if committed != comp.replaced_text {
            self.history.push(Transaction {
                kind: EditKind::Atomic,
                payload: TxPayload::Changes {
                    undo: ChangeSet::single(comp.start..comp.end, comp.replaced_text),
                    redo: ChangeSet::single(comp.start..orig_end, committed),
                },
                selection_before: comp.sel_before,
                selection_after: self.selection,
            });
        }
        self.debug_assert_invariants();
        true
    }

    pub(crate) fn cancel_composition(&mut self) -> Option<LineChange> {
        let comp = self.composition.take()?;
        self.commit(Edit {
            source: EditSource::Changes(ChangeSet::single(comp.start..comp.end, comp.replaced_text)),
            selection: SelectionAfter::Explicit(comp.sel_before),
            record: Record::Untracked,
            validate: false,
        })
    }

    fn delete_with(
        &mut self,
        collapsed_kind: EditKind,
        collapsed_span: impl FnOnce(&Rope, usize) -> Range<usize>,
    ) -> Option<LineChange> {
        self.commit_composition();
        let (range, kind) = if self.selection.is_empty() {
            let span = collapsed_span(&self.text, self.selection.head);
            if span.start == span.end {
                return None;
            }
            (span.start.min(span.end)..span.start.max(span.end), collapsed_kind)
        } else {
            (self.selection.range(), EditKind::Atomic)
        };
        self.replace_range(range, "", kind)
    }

    pub(crate) fn delete_backward(&mut self) -> Option<LineChange> {
        self.delete_with(EditKind::Backspace, |t, off| {
            prev_grapheme_boundary(t, off)..off
        })
    }

    pub(crate) fn delete_forward(&mut self) -> Option<LineChange> {
        self.delete_with(EditKind::DeleteForward, |t, off| {
            off..next_grapheme_boundary(t, off)
        })
    }

    pub(crate) fn delete_prev_word(&mut self) -> Option<LineChange> {
        self.delete_with(EditKind::Atomic, |t, off| prev_word_start(t, off)..off)
    }

    pub(crate) fn delete_next_word(&mut self) -> Option<LineChange> {
        self.delete_with(EditKind::Atomic, |t, off| off..next_word_end(t, off))
    }

    pub(crate) fn enter(&mut self) -> Option<LineChange> {
        self.commit_composition();
        if !self.policy.multi_line {
            return None;
        }
        self.replace_range(self.selected_range(), "\n", EditKind::Atomic)
    }

    pub(crate) fn indent(&mut self, block: bool) -> Option<Option<LineChange>> {
        if !self.policy.multi_line {
            return None;
        }
        self.commit_composition();
        let before = self.selection;
        let spans_multiple_lines = !before.is_empty()
            && self.text.offset_to_point(before.start()).row
                != self.text.offset_to_point(before.end()).row;
        if !block && !spans_multiple_lines {
            return Some(self.replace_range(before.range(), "\t", EditKind::Atomic));
        }
        let (start_row, end_row) = self.selected_line_bounds(before);
        let changes = ChangeSet::new(
            (start_row..=end_row)
                .map(|row| {
                    let start = self.text.line_start_offset(row);
                    Change {
                        range: start..start,
                        text: "\t".into(),
                    }
                })
                .collect(),
        );
        Some(self.commit(Edit {
            source: EditSource::Changes(changes),
            selection: SelectionAfter::Remap,
            record: Record::Undoable(EditKind::Atomic),
            validate: true,
        }))
    }

    pub(crate) fn outdent(&mut self) -> Option<Option<LineChange>> {
        if !self.policy.multi_line {
            return None;
        }
        self.commit_composition();
        let (start_row, end_row) = self.selected_line_bounds(self.selection);
        let changes: Vec<Change> = (start_row..=end_row)
            .filter_map(|row| {
                let start = self.text.line_start_offset(row);
                let remove = self.leading_outdent_bytes(start);
                (remove > 0).then(|| Change {
                    range: start..start + remove,
                    text: String::new(),
                })
            })
            .collect();
        if changes.is_empty() {
            return Some(None);
        }
        Some(self.commit(Edit {
            source: EditSource::Changes(ChangeSet::new(changes)),
            selection: SelectionAfter::Remap,
            record: Record::Undoable(EditKind::Atomic),
            validate: true,
        }))
    }

    fn selected_line_bounds(&self, sel: Selection) -> (usize, usize) {
        let start_row = self.text.offset_to_point(sel.start()).row;
        let end_pt = self.text.offset_to_point(sel.end());
        let end_row = if !sel.is_empty() && end_pt.column == 0 && end_pt.row > start_row {
            end_pt.row - 1
        } else {
            end_pt.row
        };
        (start_row, end_row)
    }

    fn leading_outdent_bytes(&self, line_start: usize) -> usize {
        if self.text.char_at(line_start) == Some('\t') {
            return 1;
        }
        self.text
            .slice(line_start..)
            .chars()
            .take(8)
            .take_while(|&ch| ch == ' ')
            .count()
    }

    pub(crate) fn undo(&mut self) -> Option<LineChange> {
        self.replay(true)
    }

    pub(crate) fn redo(&mut self) -> Option<LineChange> {
        self.replay(false)
    }

    fn replay(&mut self, is_undo: bool) -> Option<LineChange> {
        self.commit_composition();
        let replay = self.history.replay(is_undo)?;
        let source = match replay.payload {
            ReplayPayload::Changes(changes) => EditSource::Changes(changes),
            ReplayPayload::Snapshot(rope) => EditSource::ReplaceRope(rope),
        };
        self.commit(Edit {
            source,
            selection: SelectionAfter::Explicit(replay.selection),
            record: Record::Untracked,
            validate: false,
        })
    }

    pub(crate) fn select_all(&mut self) {
        self.commit_composition();
        self.selection = Selection::from_range(0..self.text.len());
        self.debug_assert_invariants();
    }

    pub(crate) fn set_selected_range(&mut self, range: Range<usize>) {
        self.commit_composition();
        let end_bias = if range.start == range.end {
            Bias::Left
        } else {
            Bias::Right
        };
        let start = self.snap_boundary(range.start, Bias::Left);
        let end = self.snap_boundary(range.end, end_bias);
        self.selection = Selection::from_range(start..end);
        self.debug_assert_invariants();
    }

    pub(crate) fn go_to_line(&mut self, line: usize) {
        self.commit_composition();
        let offset = self.snap_boundary(self.text.line_start_offset(line), Bias::Left);
        self.selection = Selection::caret(offset);
        self.debug_assert_invariants();
    }

    pub(crate) fn place_caret(&mut self, offset: usize, end_affinity: bool) {
        self.commit_composition();
        let offset = self.snap_boundary(offset, Bias::Left);
        self.selection.place(offset, end_affinity);
        self.debug_assert_invariants();
    }

    pub(crate) fn extend_selection(
        &mut self,
        offset: usize,
        end_affinity: bool,
        anchor_word: Option<Range<usize>>,
    ) {
        let clamped = self.snap_boundary(offset, Bias::Left);
        self.selection
            .extend_to(clamped, end_affinity, anchor_word, self.text.len());
        self.debug_assert_invariants();
    }

    pub(crate) fn select_word_at(&mut self, offset: usize) -> Option<Range<usize>> {
        let range = word_range_at(&self.text, offset)?;
        self.selection = Selection::from_range(range.clone());
        self.debug_assert_invariants();
        Some(range)
    }

    pub(crate) fn select_line_at(&mut self, offset: usize) {
        let range = line_range_at(&self.text, offset);
        self.selection = Selection::from_range(range);
        self.debug_assert_invariants();
    }
}

fn invert_changes(before: &Rope, forward: &ChangeSet) -> ChangeSet {
    let mut shift: isize = 0;
    let mut inverse = Vec::with_capacity(forward.len());
    for c in &forward.changes {
        let new_start = (c.range.start as isize + shift) as usize;
        let new_end = new_start + c.text.len();
        inverse.push(Change {
            range: new_start..new_end,
            text: before.slice(c.range.clone()).to_string(),
        });
        shift += c.text.len() as isize - (c.range.end - c.range.start) as isize;
    }
    ChangeSet::new(inverse)
}

fn rebuild_rope(text: &Rope, changes: &[Change]) -> Rope {
    let mut builder = RopeBuilder::new();
    let mut cursor = 0usize;
    for c in changes {
        if c.range.start > cursor {
            for chunk in text.slice(cursor..c.range.start).chunks() {
                builder.append(chunk);
            }
        }
        if !c.text.is_empty() {
            builder.append(&c.text);
        }
        cursor = c.range.end;
    }
    if cursor < text.len() {
        for chunk in text.slice(cursor..text.len()).chunks() {
            builder.append(chunk);
        }
    }
    builder.finish()
}

fn utf16_offset_to_byte(text: &str, target_utf16: usize) -> usize {
    let mut u16_pos = 0usize;
    for (byte_ix, ch) in text.char_indices() {
        if u16_pos >= target_utf16 {
            return byte_ix;
        }
        u16_pos += ch.len_utf16();
    }
    text.len()
}

const MAX_HISTORY_ENTRIES: usize = 1000;
const MAX_CHANGES_PER_ENTRY: usize = 1000;
const MAX_HISTORY_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Change {
    pub(crate) range: Range<usize>,
    pub(crate) text: String,
}

impl Change {
    fn approx_bytes(&self) -> usize {
        self.text.len() + std::mem::size_of::<Self>()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ChangeSet {
    changes: Vec<Change>,
}

impl ChangeSet {
    pub(crate) fn single(range: Range<usize>, text: impl Into<String>) -> Self {
        Self {
            changes: vec![Change {
                range,
                text: text.into(),
            }],
        }
    }

    pub(crate) fn new(mut changes: Vec<Change>) -> Self {
        changes.sort_by_key(|c| c.range.start);
        debug_assert!(
            changes.windows(2).all(|w| w[0].range.end <= w[1].range.start),
            "ChangeSet ranges must be non-overlapping"
        );
        Self { changes }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    pub(crate) fn len(&self) -> usize {
        self.changes.len()
    }

    fn approx_bytes(&self) -> usize {
        self.changes.iter().map(Change::approx_bytes).sum()
    }

    pub(crate) fn remap_offset(&self, offset: usize) -> usize {
        remap_offset(
            self.changes.iter().map(|c| (c.range.clone(), c.text.len())),
            offset,
        )
    }
}

fn remap_offset(
    changes: impl IntoIterator<Item = (Range<usize>, usize)>,
    offset: usize,
) -> usize {
    let mut out = offset;
    for (range, new_len) in changes {
        if range.end <= offset {
            out = out - (range.end - range.start) + new_len;
        } else if range.start < offset {
            let inside = offset - range.start;
            out = out - inside + inside.min(new_len);
            break;
        } else {
            break;
        }
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EditKind {
    Insert,
    Backspace,
    DeleteForward,
    Atomic,
}

#[derive(Debug)]
pub(crate) enum TxPayload {
    Changes { undo: ChangeSet, redo: ChangeSet },
    Snapshot { before: Rope, after: Rope, delta_bytes: usize },
}

impl TxPayload {
    fn approx_bytes(&self) -> usize {
        match self {
            Self::Changes { undo, redo } => undo.approx_bytes() + redo.approx_bytes(),
            Self::Snapshot { delta_bytes, .. } => (*delta_bytes).max(4096),
        }
    }

    fn is_noop(&self) -> bool {
        match self {
            Self::Changes { undo, redo } => undo.is_empty() || undo == redo,
            Self::Snapshot { .. } => false,
        }
    }
}

pub(crate) struct Transaction {
    pub(crate) kind: EditKind,
    pub(crate) payload: TxPayload,
    pub(crate) selection_before: Selection,
    pub(crate) selection_after: Selection,
}

#[derive(Debug)]
struct HistoryEntry {
    kind: EditKind,
    payload: TxPayload,
    merged_count: usize,
    approx_bytes: usize,
    selection_before: Selection,
    selection_after: Selection,
    version_before: u64,
    version_after: u64,
}

pub(crate) enum ReplayPayload {
    Changes(ChangeSet),
    Snapshot(Rope),
}

pub(crate) struct HistoryReplay {
    pub(crate) payload: ReplayPayload,
    pub(crate) selection: Selection,
}

#[derive(Debug)]
pub(crate) struct History {
    undo_stack: Vec<HistoryEntry>,
    redo_stack: Vec<HistoryEntry>,
    total_bytes: usize,
    coalesce_broken: bool,
    next_version: u64,
    current_version: u64,
    saved_version: u64,
}

impl History {
    pub(crate) fn new() -> Self {
        Self {
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            total_bytes: 0,
            coalesce_broken: false,
            next_version: 1,
            current_version: 0,
            saved_version: 0,
        }
    }

    pub(crate) fn break_coalescing(&mut self) {
        self.coalesce_broken = true;
    }

    fn clear_redo(&mut self) {
        for entry in self.redo_stack.drain(..) {
            self.total_bytes = self.total_bytes.saturating_sub(entry.approx_bytes);
        }
    }

    fn enforce_budget(&mut self) {
        while self.undo_stack.len() > MAX_HISTORY_ENTRIES
            || (self.undo_stack.len() > 1 && self.total_bytes > MAX_HISTORY_BYTES)
        {
            let evicted = self.undo_stack.remove(0);
            self.total_bytes = self.total_bytes.saturating_sub(evicted.approx_bytes);
        }
    }

    pub(crate) fn push(&mut self, tx: Transaction) {
        let Transaction {
            kind,
            payload,
            selection_before,
            selection_after,
        } = tx;

        if payload.is_noop() {
            self.coalesce_broken = true;
            return;
        }

        self.clear_redo();

        if !self.coalesce_broken
            && kind != EditKind::Atomic
            && let Some(prev) = self.undo_stack.last_mut()
            && prev.kind == kind
            && prev.merged_count < MAX_CHANGES_PER_ENTRY
            && prev.selection_after == selection_before
            && let (
                TxPayload::Changes {
                    undo: prev_undo,
                    redo: prev_redo,
                },
                TxPayload::Changes {
                    undo: curr_undo,
                    redo: curr_redo,
                },
            ) = (&mut prev.payload, &payload)
            && try_coalesce(kind, prev_undo, prev_redo, curr_undo, curr_redo)
        {
            let old_bytes = prev.approx_bytes;
            let version = self.next_version;
            self.next_version += 1;
            prev.version_after = version;
            self.current_version = version;
            prev.merged_count += 1;
            prev.selection_after = selection_after;
            let new_bytes = prev.payload.approx_bytes();
            prev.approx_bytes = new_bytes;
            self.total_bytes = self.total_bytes.saturating_sub(old_bytes) + new_bytes;
            self.enforce_budget();
            return;
        }

        let version_before = self.current_version;
        let version_after = self.next_version;
        self.next_version += 1;
        self.current_version = version_after;
        let merged_count = match &payload {
            TxPayload::Changes { redo, .. } => redo.len(),
            TxPayload::Snapshot { .. } => 1,
        };
        let approx_bytes = payload.approx_bytes();
        self.total_bytes += approx_bytes;
        self.undo_stack.push(HistoryEntry {
            kind,
            payload,
            merged_count,
            approx_bytes,
            selection_before,
            selection_after,
            version_before,
            version_after,
        });
        self.coalesce_broken = kind == EditKind::Atomic;
        self.enforce_budget();
    }

    pub(crate) fn clear(&mut self) {
        *self = Self::new();
    }

    pub(crate) fn begin_save(&mut self) -> u64 {
        self.coalesce_broken = true;
        self.current_version
    }

    pub(crate) fn mark_saved(&mut self, version: u64) {
        self.saved_version = version;
    }

    pub(crate) fn is_clean(&self) -> bool {
        self.saved_version == self.current_version
    }

    pub(crate) fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub(crate) fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    pub(crate) fn replay(&mut self, is_undo: bool) -> Option<HistoryReplay> {
        let (from, to) = if is_undo {
            (&mut self.undo_stack, &mut self.redo_stack)
        } else {
            (&mut self.redo_stack, &mut self.undo_stack)
        };
        let entry = from.pop()?;
        let (version, selection) = if is_undo {
            (entry.version_before, entry.selection_before)
        } else {
            (entry.version_after, entry.selection_after)
        };
        let payload = match (&entry.payload, is_undo) {
            (TxPayload::Changes { undo: c, .. }, true) | (TxPayload::Changes { redo: c, .. }, false) => {
                ReplayPayload::Changes(c.clone())
            }
            (TxPayload::Snapshot { before: r, .. }, true)
            | (TxPayload::Snapshot { after: r, .. }, false) => ReplayPayload::Snapshot(r.clone()),
        };
        to.push(entry);
        self.current_version = version;
        self.coalesce_broken = true;
        Some(HistoryReplay { payload, selection })
    }
}

fn try_coalesce(
    kind: EditKind,
    prev_undo: &mut ChangeSet,
    prev_redo: &mut ChangeSet,
    curr_undo: &ChangeSet,
    curr_redo: &ChangeSet,
) -> bool {
    let ([pu], [pr], [cu], [cr]) = (
        prev_undo.changes.as_mut_slice(),
        prev_redo.changes.as_mut_slice(),
        curr_undo.changes.as_slice(),
        curr_redo.changes.as_slice(),
    ) else {
        return false;
    };
    match kind {
        EditKind::Insert => {
            if !pr.range.is_empty()
                || pr.text.contains(['\n', '\r'])
                || cr.text.contains(['\n', '\r'])
                || cr.range.start < pu.range.start
                || cr.range.end != pu.range.end
            {
                return false;
            }
            let cut = cr.range.start - pr.range.start;
            if cut > pr.text.len() || !pr.text.is_char_boundary(cut) {
                return false;
            }
            pr.text.truncate(cut);
            pr.text.push_str(&cr.text);
            pu.range.end = pr.range.start + pr.text.len();
            true
        }
        EditKind::Backspace => {
            if !pr.text.is_empty() || !cr.text.is_empty() || cr.range.end != pr.range.start {
                return false;
            }
            pr.range.start = cr.range.start;
            pu.range = cr.range.start..cr.range.start;
            pu.text.insert_str(0, &cu.text);
            true
        }
        EditKind::DeleteForward => {
            if !pr.text.is_empty() || !cr.text.is_empty() || cr.range.start != pr.range.start {
                return false;
            }
            pr.range.end += cr.range.end - cr.range.start;
            pu.text.push_str(&cu.text);
            true
        }
        EditKind::Atomic => false,
    }
}

const CHUNK_SIZE: usize = 256;
const MAX_WRAP_INDENT: u32 = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DisplayPos {
    pub(crate) row: usize,
    pub(crate) sub_row: usize,
    pub(crate) col: usize,
}

impl DisplayPos {
    #[inline]
    pub(crate) fn new(row: usize, sub_row: usize, col: usize) -> Self {
        Self { row, sub_row, col }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum LineRanges {
    Single(usize),
    Wrapped(Box<[Range<usize>]>),
}

impl LineRanges {
    #[inline]
    pub(crate) fn len(&self) -> usize {
        match self {
            Self::Single(_) => 1,
            Self::Wrapped(s) => s.len(),
        }
    }

    #[inline]
    pub(crate) fn get(&self, index: usize) -> Option<Range<usize>> {
        match self {
            Self::Single(byte_len) => (index == 0).then_some(0..*byte_len),
            Self::Wrapped(s) => s.get(index).cloned(),
        }
    }

    #[inline]
    pub(crate) fn last(&self) -> Range<usize> {
        match self {
            Self::Single(byte_len) => 0..*byte_len,
            Self::Wrapped(s) => s.last().cloned().unwrap_or(0..0),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct WrappedLine {
    pub(crate) byte_len: usize,
    pub(crate) hanging_indent: f32,
    pub(crate) ranges: LineRanges,
}

impl WrappedLine {
    #[inline]
    pub(crate) fn row_count(&self) -> usize {
        self.ranges.len()
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct ChunkSummary {
    buffer_lines: usize,
    visual_rows: usize,
    max_line_bytes: usize,
    longest_line: usize,
}

impl ChunkSummary {
    #[inline]
    fn single(line: &WrappedLine) -> Self {
        Self {
            buffer_lines: 1,
            visual_rows: line.row_count(),
            max_line_bytes: line.byte_len,
            longest_line: 0,
        }
    }

    #[inline]
    fn combine(&mut self, other: &Self) {
        if other.max_line_bytes > self.max_line_bytes {
            self.longest_line = self.buffer_lines + other.longest_line;
            self.max_line_bytes = other.max_line_bytes;
        }
        self.buffer_lines += other.buffer_lines;
        self.visual_rows += other.visual_rows;
    }

    fn of_slice(lines: &[WrappedLine]) -> Self {
        let mut total = Self::default();
        for line in lines {
            total.combine(&Self::single(line));
        }
        total
    }
}

#[derive(Debug, Clone)]
struct LineChunk {
    summary: ChunkSummary,
    items: Vec<WrappedLine>,
}

impl LineChunk {
    fn new(items: Vec<WrappedLine>) -> Self {
        let summary = ChunkSummary::of_slice(&items);
        Self { summary, items }
    }
}

#[derive(Debug, Clone, Default)]
struct LineChunks {
    chunks: Vec<LineChunk>,
    line_starts: Vec<usize>,
    row_starts: Vec<usize>,
    total: ChunkSummary,
}

impl LineChunks {
    fn from_vec(items: Vec<WrappedLine>) -> Self {
        let mut chunks = Vec::with_capacity(items.len().div_ceil(CHUNK_SIZE));
        let mut iter = items.into_iter();
        while iter.len() > 0 {
            chunks.push(LineChunk::new(iter.by_ref().take(CHUNK_SIZE).collect()));
        }
        let mut tree = Self {
            chunks,
            line_starts: Vec::new(),
            row_starts: Vec::new(),
            total: ChunkSummary::default(),
        };
        tree.recompute_total();
        tree
    }

    fn locate_line(&self, line: usize) -> (usize, usize) {
        if line >= self.total.buffer_lines {
            return (self.chunks.len(), 0);
        }
        let ix = self.line_starts.partition_point(|&start| start <= line) - 1;
        (ix, line - self.line_starts[ix])
    }

    fn get_by_line(&self, line: usize) -> (usize, Option<&WrappedLine>) {
        if line >= self.total.buffer_lines {
            return (self.total.visual_rows, None);
        }
        let ix = self.line_starts.partition_point(|&start| start <= line) - 1;
        let local = line - self.line_starts[ix];
        let mut row_acc = self.row_starts[ix];
        for item in &self.chunks[ix].items[..local] {
            row_acc += item.row_count();
        }
        (row_acc, Some(&self.chunks[ix].items[local]))
    }

    fn get_by_visual_row(&self, visual_row: usize) -> Option<(usize, usize, &WrappedLine)> {
        if visual_row >= self.total.visual_rows {
            return None;
        }
        let ix = self.row_starts.partition_point(|&start| start <= visual_row) - 1;
        let line_acc = self.line_starts[ix];
        let mut row_acc = self.row_starts[ix];
        for (local, item) in self.chunks[ix].items.iter().enumerate() {
            let next = row_acc + item.row_count();
            if visual_row < next {
                return Some((line_acc + local, row_acc, item));
            }
            row_acc = next;
        }
        None
    }

    fn splice(&mut self, range: Range<usize>, replacement: Vec<WrappedLine>) {
        if self.total.buffer_lines == 0 {
            *self = Self::from_vec(replacement);
            return;
        }
        let start = range.start.min(self.total.buffer_lines);
        let end = range.end.clamp(start, self.total.buffer_lines);
        let (start_chunk, start_local) = self.locate_line(start);
        let (end_chunk, end_local) = self.locate_line(end);
        let replace_end = if end_chunk < self.chunks.len() {
            end_chunk + 1
        } else {
            self.chunks.len()
        };

        let prefix = if start_chunk < self.chunks.len() {
            start_local
        } else {
            0
        };
        let suffix = if end_chunk < self.chunks.len() {
            self.chunks[end_chunk].items.len() - end_local
        } else {
            0
        };

        let mut merged = Vec::with_capacity(prefix + replacement.len() + suffix);
        if start_chunk < self.chunks.len() {
            merged.extend(self.chunks[start_chunk].items[..start_local].iter().cloned());
        }
        merged.extend(replacement);
        if end_chunk < self.chunks.len() {
            merged.extend(self.chunks[end_chunk].items[end_local..].iter().cloned());
        }

        let mut rebuilt = Vec::with_capacity(merged.len().div_ceil(CHUNK_SIZE));
        let mut iter = merged.into_iter();
        while iter.len() > 0 {
            rebuilt.push(LineChunk::new(iter.by_ref().take(CHUNK_SIZE).collect()));
        }
        self.chunks.splice(start_chunk..replace_end, rebuilt);
        let mut idx = start_chunk.saturating_sub(1);
        while idx + 1 < self.chunks.len() && idx <= start_chunk + 2 {
            let combined = self.chunks[idx].items.len() + self.chunks[idx + 1].items.len();
            if combined <= CHUNK_SIZE {
                let next_chunk = self.chunks.remove(idx + 1);
                self.chunks[idx].items.extend(next_chunk.items);
                self.chunks[idx].summary = ChunkSummary::of_slice(&self.chunks[idx].items);
            } else {
                idx += 1;
            }
        }
        self.recompute_total();
    }

    fn recompute_total(&mut self) {
        self.line_starts.clear();
        self.row_starts.clear();
        self.line_starts.reserve(self.chunks.len());
        self.row_starts.reserve(self.chunks.len());
        let mut total = ChunkSummary::default();
        for chunk in &self.chunks {
            self.line_starts.push(total.buffer_lines);
            self.row_starts.push(total.visual_rows);
            total.combine(&chunk.summary);
        }
        self.total = total;
    }
}

#[inline]
fn is_printable_ascii(bytes: &[u8]) -> bool {
    bytes.iter().all(|&b| (0x20..=0x7e).contains(&b))
}

fn wrap_line(line: &str, wrap_width: f32, face: &FontFace, out: &mut Vec<usize>) -> f32 {
    out.clear();
    if line.is_empty() || !wrap_width.is_finite() || wrap_width <= 0.0 {
        return 0.0;
    }
    let indent_px = face.hanging_indent(line, MAX_WRAP_INDENT, wrap_width * 0.5);
    face.line_breaks(line, wrap_width, indent_px, out);
    if out.is_empty() { 0.0 } else { indent_px }
}

pub(crate) struct WrapIndex {
    style: TextStyle,
    wrap_width: Option<f32>,
    chunks: LineChunks,
    initialized: bool,
}

impl WrapIndex {
    pub(crate) fn new(style: TextStyle, wrap_width: Option<f32>) -> Self {
        Self {
            style,
            wrap_width,
            chunks: LineChunks::default(),
            initialized: false,
        }
    }

    #[inline]
    pub(crate) fn visual_row_count(&self) -> usize {
        self.chunks.total.visual_rows
    }

    #[inline]
    pub(crate) fn line_count(&self) -> usize {
        self.chunks.total.buffer_lines
    }

    #[inline]
    pub(crate) fn longest_line(&self) -> usize {
        self.chunks.total.longest_line
    }

    #[inline]
    pub(crate) fn line(&self, row: usize) -> Option<&WrappedLine> {
        self.chunks.get_by_line(row).1
    }

    pub(crate) fn first_visual_row(&self, line: usize) -> usize {
        self.chunks.get_by_line(line).0
    }

    pub(crate) fn visual_row_to_line(&self, visual_row: usize) -> usize {
        self.chunks
            .get_by_visual_row(visual_row)
            .map_or_else(|| self.line_count().saturating_sub(1), |(line, _, _)| line)
    }

    pub(crate) fn visual_row_to_line_and_sub_row(&self, visual_row: usize) -> (usize, usize) {
        self.chunks.get_by_visual_row(visual_row).map_or_else(
            || (self.line_count().saturating_sub(1), 0),
            |(line, first_row, _)| (line, visual_row.saturating_sub(first_row)),
        )
    }

    pub(crate) fn visual_row_range(&self, line_ix: usize, sub_row: usize) -> Option<(Range<usize>, bool)> {
        let line = self.line(line_ix)?;
        let range = line.ranges.get(sub_row)?;
        let soft_end = sub_row + 1 < line.row_count();
        Some((range, soft_end))
    }

    pub(crate) fn style(&self) -> &TextStyle {
        &self.style
    }

    pub(crate) fn wrap_width(&self) -> Option<f32> {
        self.wrap_width
    }

    pub(crate) fn set_wrap_width(&mut self, text: &Rope, wrap_width: Option<f32>, ts: &TextSystem) {
        if self.wrap_width == wrap_width {
            return;
        }
        self.wrap_width = wrap_width;
        if self.initialized {
            self.rebuild_all(text, ts);
        }
    }

    pub(crate) fn sync_layout(
        &mut self,
        text: &Rope,
        style: &TextStyle,
        wrap_width: Option<f32>,
        ts: &TextSystem,
    ) {
        let style_changed = self.style != *style;
        let width_changed = self.wrap_width != wrap_width;
        if style_changed {
            self.style = style.clone();
        }
        self.wrap_width = wrap_width;

        if !self.initialized {
            self.initialized = true;
            self.rebuild_all(text, ts);
            return;
        }
        if wrap_width.is_none() && !width_changed {
            return;
        }
        if style_changed || width_changed {
            self.rebuild_all(text, ts);
        }
    }

    pub(crate) fn apply_line_change(
        &mut self,
        updated_text: &Rope,
        change: &LineChange,
        ts: &TextSystem,
    ) {
        if !self.initialized {
            return;
        }
        match change {
            LineChange::Full => self.rebuild_all(updated_text, ts),
            LineChange::Splice {
                old_lines,
                new_lines,
            } => self.apply_line_splice(updated_text, old_lines.clone(), new_lines.clone(), ts),
        }
    }

    fn apply_line_splice(
        &mut self,
        updated_text: &Rope,
        old_lines: Range<usize>,
        new_lines: Range<usize>,
        ts: &TextSystem,
    ) {
        let Some(wrap_width) = self.wrap_width else {
            self.splice_line_range(
                updated_text,
                old_lines,
                new_lines,
                0,
                &mut |_, _, _| 0.0,
            );
            return;
        };

        let face = ts.face(&self.style);
        let max_ascii = face.max_printable_ascii();
        let ascii_fit_len = if max_ascii > 0.0 {
            (wrap_width / max_ascii).floor().max(0.0) as usize
        } else {
            0
        };
        self.splice_line_range(
            updated_text,
            old_lines,
            new_lines,
            ascii_fit_len,
            &mut |line_str, width, out| wrap_line(line_str, width, &face, out),
        );
    }

    fn splice_line_range<F>(
        &mut self,
        updated_text: &Rope,
        old_lines: Range<usize>,
        new_lines: Range<usize>,
        ascii_fit_len: usize,
        wrap_fn: &mut F,
    ) where
        F: FnMut(&str, f32, &mut Vec<usize>) -> f32,
    {
        let existing_count = self.line_count();
        let old_start = old_lines.start.min(existing_count);
        let old_end = old_lines.end.clamp(old_start, existing_count);

        let total_rows = updated_text.lines_len();
        let new_start = new_lines.start.min(total_rows);
        let new_end = new_lines.end.clamp(new_start, total_rows);

        let mut replacement = Vec::with_capacity(new_end - new_start);
        let mut breaks = Vec::new();
        let wrap_width = self.wrap_width;
        let mut iter = updated_text.lines_at(new_start, ropey::LineType::LF);

        for _ in new_start..new_end {
            let slice = match iter.next() {
                Some(raw) if raw.len() > 0 && raw.byte(raw.len() - 1) == b'\n' => {
                    raw.slice(..raw.len() - 1)
                }
                Some(raw) => raw,
                None => updated_text.slice(0..0),
            };
            let byte_len = slice.len();
            let mut indent_px = 0.0;
            let ranges = if let Some(width) = wrap_width
                && byte_len > 0
            {
                let fast_ascii = ascii_fit_len > 0
                    && byte_len <= ascii_fit_len
                    && slice.chunks().all(|c| is_printable_ascii(c.as_bytes()));
                if fast_ascii {
                    LineRanges::Single(byte_len)
                } else {
                    let line_str: Cow<str> = slice.into();
                    breaks.clear();
                    indent_px = wrap_fn(&line_str, width, &mut breaks);
                    if breaks.is_empty() {
                        LineRanges::Single(byte_len)
                    } else {
                        let mut v = Vec::with_capacity(breaks.len() + 1);
                        let mut cursor = 0;
                        for &ix in &breaks {
                            v.push(cursor..ix);
                            cursor = ix;
                        }
                        if cursor < byte_len {
                            v.push(cursor..byte_len);
                        }
                        LineRanges::Wrapped(v.into_boxed_slice())
                    }
                }
            } else {
                LineRanges::Single(byte_len)
            };

            replacement.push(WrappedLine {
                byte_len,
                hanging_indent: indent_px,
                ranges,
            });
        }

        self.chunks.splice(old_start..old_end, replacement);
    }

    pub(crate) fn rebuild_all(&mut self, text: &Rope, ts: &TextSystem) {
        let old_count = self.line_count();
        let new_count = text.lines_len();
        self.apply_line_splice(text, 0..old_count, 0..new_count, ts);
    }

    pub(crate) fn offset_to_display(
        &self,
        text: &Rope,
        offset: usize,
        end_affinity: bool,
    ) -> DisplayPos {
        let row = text.offset_to_point(offset).row;
        let start = text.line_start_offset(row);
        let (first_row, Some(line)) = self.chunks.get_by_line(row) else {
            return DisplayPos::new(self.visual_row_count(), 0, 0);
        };
        let local = offset.saturating_sub(start);
        match &line.ranges {
            LineRanges::Single(byte_len) => {
                DisplayPos::new(first_row, 0, local.min(*byte_len))
            }
            LineRanges::Wrapped(ranges) => {
                for (sub_row, range) in ranges.iter().enumerate() {
                    if range.contains(&local) || (end_affinity && local == range.end) {
                        return DisplayPos::new(
                            first_row + sub_row,
                            sub_row,
                            local.saturating_sub(range.start),
                        );
                    }
                }
                let last = line.ranges.last();
                let sub_row = line.row_count().saturating_sub(1);
                DisplayPos::new(first_row + sub_row, sub_row, last.len())
            }
        }
    }

    pub(crate) fn display_to_offset(&self, text: &Rope, pos: DisplayPos) -> usize {
        let Some((line_ix, first_row, line)) = self.chunks.get_by_visual_row(pos.row) else {
            return text.len();
        };
        let line_start = text.line_start_offset(line_ix);
        let sub_row = pos.row.saturating_sub(first_row);
        match line.ranges.get(sub_row) {
            Some(range) => line_start + (range.start + pos.col).min(range.end),
            None => line_start + line.byte_len,
        }
    }

    pub(crate) fn display_to_point(&self, text: &Rope, pos: DisplayPos) -> TextPoint {
        text.offset_to_point(self.display_to_offset(text, pos))
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

pub(crate) enum VisualRow {
    Shaped(ShapedRow),
    Windowed {
        start_byte: usize,
        end_byte: usize,
        total_bytes: usize,
        start_x: f32,
        total_width: f32,
        line: ShapedRow,
    },
    Placeholder(usize),
}

impl From<ShapedRow> for VisualRow {
    fn from(line: ShapedRow) -> Self {
        Self::Shaped(line)
    }
}

impl VisualRow {
    #[inline]
    pub(crate) fn byte_len(&self) -> usize {
        match self {
            Self::Shaped(line) => line.len,
            Self::Windowed { total_bytes, .. } => *total_bytes,
            Self::Placeholder(len) => *len,
        }
    }

    #[inline]
    pub(crate) fn width(&self) -> f32 {
        match self {
            Self::Shaped(line) => line.width,
            Self::Windowed { total_width, .. } => *total_width,
            Self::Placeholder(_) => 0.0,
        }
    }

    #[inline]
    pub(crate) fn is_shaped(&self) -> bool {
        !matches!(self, Self::Placeholder(_))
    }

    pub(crate) fn x_for_index(&self, ix: usize) -> f32 {
        match self {
            Self::Shaped(line) => line.x_for_index(ix),
            Self::Windowed {
                start_byte,
                end_byte,
                start_x,
                total_width,
                line,
                ..
            } => {
                if ix <= *start_byte {
                    *start_x
                } else if ix >= *end_byte {
                    (*start_x + line.width).min(*total_width)
                } else {
                    *start_x + line.x_for_index(ix - *start_byte)
                }
            }
            Self::Placeholder(_) => 0.0,
        }
    }

    pub(crate) fn selection_spans(&self, start: usize, end: usize) -> Vec<(f32, f32)> {
        if start >= end {
            return Vec::new();
        }
        match self {
            Self::Shaped(line) => line.selection_spans(start, end),
            Self::Windowed {
                start_byte,
                end_byte,
                start_x,
                line,
                ..
            } => {
                let s = start.max(*start_byte);
                let e = end.min(*end_byte);
                if s >= e {
                    return Vec::new();
                }
                line.selection_spans(s - *start_byte, e - *start_byte)
                    .into_iter()
                    .map(|(l, r)| (l + *start_x, r + *start_x))
                    .collect()
            }
            Self::Placeholder(_) => Vec::new(),
        }
    }

    pub(crate) fn closest_index_for_x(&self, x: f32) -> usize {
        match self {
            Self::Shaped(line) => line.closest_index_for_x(x),
            Self::Windowed {
                start_byte,
                start_x,
                line,
                ..
            } => *start_byte + line.closest_index_for_x((x - *start_x).max(0.0)),
            Self::Placeholder(_) => 0,
        }
    }

    pub(crate) fn paint(
        &self,
        origin: Point,
        line_height: f32,
        color: Rgba,
        underline: Option<Range<usize>>,
        canvas: &mut Canvas,
    ) {
        match self {
            Self::Shaped(line) => {
                canvas.draw_row(line, origin, line_height, color, underline);
            }
            Self::Windowed {
                start_byte,
                end_byte,
                start_x,
                line,
                ..
            } => {
                let ul = underline.and_then(|r| {
                    let s = r.start.max(*start_byte);
                    let e = r.end.min(*end_byte);
                    (s < e).then_some((s - *start_byte)..(e - *start_byte))
                });
                canvas.draw_row(line, origin + point(*start_x, 0.0), line_height, color, ul);
            }
            Self::Placeholder(_) => {}
        }
    }
}

pub(crate) struct RowLayout {
    pub(crate) first_sub_row: usize,
    pub(crate) leading_bytes: usize,
    pub(crate) total_sub_rows: usize,
    pub(crate) rows: Vec<VisualRow>,
    pub(crate) hanging_indent: f32,
    max_width: f32,
}

impl RowLayout {
    pub(crate) fn from_slice(
        rows: Vec<VisualRow>,
        first_sub_row: usize,
        leading_bytes: usize,
        total_sub_rows: usize,
        hanging_indent: f32,
    ) -> Self {
        let max_width = rows.iter().map(VisualRow::width).fold(0.0f32, f32::max);
        Self {
            first_sub_row,
            leading_bytes,
            total_sub_rows: total_sub_rows.max(rows.len()),
            rows,
            hanging_indent,
            max_width,
        }
    }

    pub(crate) fn from_shaped(lines: Vec<ShapedRow>) -> Self {
        let total = lines.len();
        let rows: Vec<VisualRow> = lines.into_iter().map(VisualRow::from).collect();
        Self::from_slice(rows, 0, 0, total, 0.0)
    }

    #[inline]
    pub(crate) fn indent_for(&self, abs_sub_row: usize) -> f32 {
        if abs_sub_row == 0 {
            0.0
        } else {
            self.hanging_indent
        }
    }

    pub(crate) fn position_for_index(
        &self,
        local_offset: usize,
        line_height: f32,
        end_affinity: bool,
    ) -> Option<Point> {
        let mut cursor = self.leading_bytes;
        let mut y = self.first_sub_row as f32 * line_height;
        for (ix, row) in self.rows.iter().enumerate() {
            let abs_sub = self.first_sub_row + ix;
            let is_last = abs_sub + 1 == self.total_sub_rows;
            let len = row.byte_len();
            let hit = if len == 0 {
                local_offset == cursor
            } else if is_last || end_affinity {
                local_offset >= cursor && local_offset <= cursor + len
            } else {
                local_offset >= cursor && local_offset < cursor + len
            };
            if hit {
                let x =
                    row.x_for_index(local_offset.saturating_sub(cursor)) + self.indent_for(abs_sub);
                return Some(point(x, y));
            }
            cursor += if is_last { len + 1 } else { len };
            y += line_height;
        }
        None
    }

    pub(crate) fn closest_index_for_x(&self, x: f32) -> usize {
        let mut cursor = self.leading_bytes;
        for (ix, row) in self.rows.iter().enumerate() {
            let abs_sub = self.first_sub_row + ix;
            let indent = self.indent_for(abs_sub);
            if x <= indent + row.width() || ix + 1 == self.rows.len() {
                return cursor + row.closest_index_for_x(x - indent);
            }
            cursor += row.byte_len();
        }
        cursor
    }

    pub(crate) fn closest_index_for_x_in_last_row(&self, x: f32) -> usize {
        let Some((ix, row)) = self.rows.iter().enumerate().next_back() else {
            return self.leading_bytes;
        };
        let abs_sub = self.first_sub_row + ix;
        let base = self.leading_bytes
            + self.rows[..ix]
                .iter()
                .map(VisualRow::byte_len)
                .sum::<usize>();
        let indent = self.indent_for(abs_sub);
        base + row.closest_index_for_x(x - indent)
    }

    pub(crate) fn closest_index_for_position(
        &self,
        pos: Point,
        line_height: f32,
    ) -> Option<(usize, bool)> {
        let mut offset = self.leading_bytes;
        let mut top = self.first_sub_row as f32 * line_height;
        for (ix, row) in self.rows.iter().enumerate() {
            let abs_sub = self.first_sub_row + ix;
            let bottom = top + line_height;
            if pos.y >= top && pos.y < bottom {
                let x = pos.x - self.indent_for(abs_sub);
                let col = row.closest_index_for_x(x);
                let end_affinity = abs_sub + 1 < self.total_sub_rows && col == row.byte_len();
                return Some((offset + col, end_affinity));
            }
            offset += row.byte_len();
            top = bottom;
        }
        None
    }

    pub(crate) fn size(&self, line_height: f32) -> Size {
        let width = self
            .rows
            .iter()
            .enumerate()
            .map(|(ix, row)| row.width() + self.indent_for(self.first_sub_row + ix))
            .fold(self.max_width, f32::max);
        size(width, self.total_sub_rows as f32 * line_height)
    }

    pub(crate) fn paint(
        &self,
        origin: Point,
        line_height: f32,
        color: Rgba,
        underline: Option<Range<usize>>,
        canvas: &mut Canvas,
    ) {
        let mut cursor = self.leading_bytes;
        for (ix, row) in self.rows.iter().enumerate() {
            let abs_sub = self.first_sub_row + ix;
            let len = row.byte_len();
            let sub_ul = underline.as_ref().and_then(|r| {
                let s = r.start.max(cursor);
                let e = r.end.min(cursor + len);
                (s < e).then_some((s - cursor)..(e - cursor))
            });
            row.paint(
                origin + point(self.indent_for(abs_sub), abs_sub as f32 * line_height),
                line_height,
                color,
                sub_ul,
                canvas,
            );
            cursor += len;
        }
    }
}

struct VisibleLine {
    byte_offset: usize,
    top: f32,
    layout: RowLayout,
}

struct FrameLayout {
    revision: u64,
    scroll_x: f32,
    scroll_y: f64,
    input_bounds: Rect,
    content_bounds: Rect,
    first_line: usize,
    lines: Vec<VisibleLine>,
    visible_byte_range: Range<usize>,
    line_height: f32,
    wrap_width: Option<f32>,
    space_width: f32,
    text_color: Rgba,
    caret_rect: Rect,
    show_caret: bool,
    selection_rects: Vec<Rect>,
    marked_range: Option<Range<usize>>,
    last_cursor: usize,
}

impl FrameLayout {
    fn row_layout(&self, buffer_row: usize) -> Option<&RowLayout> {
        let ix = buffer_row.checked_sub(self.first_line)?;
        self.lines.get(ix).map(|l| &l.layout)
    }

    fn position_for_offset(
        &self,
        text: &Rope,
        offset: usize,
        end_affinity: bool,
    ) -> Option<Point> {
        let row = text.offset_to_point(offset).row;
        let line = self.lines.get(row.checked_sub(self.first_line)?)?;
        let local = offset.saturating_sub(line.byte_offset);
        let pos = line
            .layout
            .position_for_index(local, self.line_height, end_affinity)?;
        Some(point(pos.x, line.top + pos.y))
    }

    fn bounds_for_range(&self, text: &Rope, range: Range<usize>) -> Option<Rect> {
        let start_pos = self
            .position_for_offset(text, range.start, false)
            .or_else(|| self.position_for_offset(text, self.last_cursor, false))?;
        let end_pos = self
            .position_for_offset(text, range.end, false)
            .unwrap_or(start_pos);
        let origin = self.content_bounds.origin;
        Some(Rect::from_corners(
            origin + start_pos,
            origin + point(end_pos.x, start_pos.y + self.line_height),
        ))
    }

    fn hit_test(
        &self,
        pos: Point,
        scroll_x: f32,
        scroll_y: f64,
        text_len: usize,
        multi_line: bool,
    ) -> (usize, bool) {
        if self.lines.is_empty() {
            return (0, false);
        }
        let lh = self.line_height;
        let rel = pos
            - self.content_bounds.origin
            - point(scroll_x - self.scroll_x, (scroll_y - self.scroll_y) as f32);

        for line in &self.lines {
            let local_pos = rel - point(0.0, line.top);
            if !multi_line {
                let col = line.layout.closest_index_for_x(local_pos.x);
                return ((line.byte_offset + col).min(text_len), false);
            }
            if let Some((col, aff)) = line.layout.closest_index_for_position(local_pos, lh) {
                return ((line.byte_offset + col).min(text_len), aff);
            } else if local_pos.y < 0.0 {
                return (line.byte_offset.min(text_len), false);
            }
        }

        if let Some(last) = self.lines.last() {
            let col = last.layout.closest_index_for_x_in_last_row(rel.x);
            return ((last.byte_offset + col).min(text_len), false);
        }
        (text_len, false)
    }

    fn paint(&self, p: Palette, canvas: &mut Canvas) {
        canvas.with_clip(self.input_bounds, |canvas| {
            for &rect in &self.selection_rects {
                canvas.paint_quad(fill(rect, p.selection));
            }

            let origin = self.content_bounds.origin;
            for line in &self.lines {
                let line_base = line.byte_offset;
                let line_marked = self.marked_range.as_ref().and_then(|m| {
                    (m.end > line_base).then(|| {
                        m.start.saturating_sub(line_base)..m.end.saturating_sub(line_base)
                    })
                });
                line.layout.paint(
                    point(origin.x, origin.y + line.top),
                    self.line_height,
                    self.text_color,
                    line_marked,
                    canvas,
                );
            }

            if self.show_caret {
                canvas.paint_quad(fill(self.caret_rect, p.caret));
            }
        });
    }

    fn range_rects(&self, range: &Range<usize>) -> Vec<Rect> {
        let range = range.start.max(self.visible_byte_range.start)
            ..range.end.min(self.visible_byte_range.end);
        if range.is_empty() {
            return Vec::new();
        }
        let origin = self.content_bounds.origin;
        let mut out = Vec::new();
        for line in &self.lines {
            let mut off = line.byte_offset + line.layout.leading_bytes;
            for (i, row) in line.layout.rows.iter().enumerate() {
                let abs_sub = line.layout.first_sub_row + i;
                let row_y = line.top + abs_sub as f32 * self.line_height;
                let end = off + row.byte_len();
                if !row.is_shaped() {
                    off = end;
                    continue;
                }
                let s = range.start.max(off);
                let e = range.end.min(end);
                let is_last = abs_sub + 1 == line.layout.total_sub_rows;
                let includes_nl = is_last && range.start <= end && range.end > end;
                let indent = line.layout.indent_for(abs_sub);
                if s < e {
                    for (span_l, span_r) in row.selection_spans(s - off, e - off) {
                        out.push(
                            Rect::from_corners(
                                point(indent + span_l, row_y),
                                point(indent + span_r, row_y + self.line_height),
                            )
                            .translate(origin),
                        );
                    }
                }
                if includes_nl {
                    let nl_left = indent + row.width();
                    out.push(
                        Rect::from_corners(
                            point(nl_left, row_y),
                            point(nl_left + self.space_width, row_y + self.line_height),
                        )
                        .translate(origin),
                    );
                }
                off = end;
            }
        }
        out
    }
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

#[inline]
fn axis_ix(axis: Axis) -> usize {
    match axis {
        Axis::Vertical => 0,
        Axis::Horizontal => 1,
    }
}

#[inline]
fn axis_val(p: Point, axis: Axis) -> f32 {
    match axis {
        Axis::Vertical => p.y,
        Axis::Horizontal => p.x,
    }
}

#[inline]
fn set_axis_val(p: &mut Point, axis: Axis, v: f32) {
    match axis {
        Axis::Vertical => p.y = v,
        Axis::Horizontal => p.x = v,
    }
}

#[inline]
fn axis_size(s: Size, axis: Axis) -> f32 {
    match axis {
        Axis::Vertical => s.height,
        Axis::Horizontal => s.width,
    }
}

#[inline]
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
        assert_eq!(ed.model().selected_text().to_string(), "world");

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
    fn selection_and_caret_basics() {
        let mut sel = Selection::from_range(5..10);
        assert_eq!(sel.range(), 5..10);
        assert!(!sel.is_empty());
        assert_eq!(sel.head, 10);

        sel.extend_to(2, false, None, 20);
        assert_eq!(sel.range(), 2..5);
        assert_eq!(sel.head, 2);

        sel.place(7, true);
        assert!(sel.is_empty());
        assert!(sel.end_affinity);

        sel.extend_to(3, false, Some(5..8), 20);
        assert_eq!(sel.range(), 3..8);
        assert_eq!(sel.head, 3);
        sel.extend_to(12, false, Some(5..8), 20);
        assert_eq!(sel.range(), 5..12);
        assert_eq!(sel.head, 12);
    }

    #[test]
    fn rope_line_and_utf16_helpers_handle_multibyte_and_crlf() {
        let rope = Rope::from("Hello\nWorld\r\nThis is a test 中文🎉\nRope");
        assert_eq!(rope.lines_len(), 4);
        assert_eq!(rope.slice_line(0).to_string(), "Hello");
        assert_eq!(rope.slice_line(1).to_string(), "World\r");
        assert_eq!(rope.slice_line(2).to_string(), "This is a test 中文🎉");
        assert_eq!(rope.slice_line(3).to_string(), "Rope");
        assert_eq!(rope.slice_line(6).to_string(), "");

        assert_eq!(rope.line_start_offset(0), 0);
        assert_eq!(rope.line_end_offset(0), 5);
        assert_eq!(rope.line_start_offset(1), 6);
        assert_eq!(rope.line_end_offset(1), 12);
        assert_eq!(rope.char_at(28), Some('中'));
        assert_eq!(rope.char_at(34), Some('🎉'));
        assert_eq!(rope.char_at(99), None);

        let sample = Rope::from("hello 中文🎉 test\nRope");
        assert_eq!(
            sample.offset_to_point("hello 中文🎉\nR".len()),
            TextPoint::new(0, "hello 中文🎉\nR".len())
        );

        let mut edit = Rope::from("中文");
        edit.replace(0..2, "New");
        assert_eq!(edit.to_string(), "New文");
        assert_eq!(sample.clip_offset(7, Bias::Left), 6);
        assert_eq!(sample.clip_offset(7, Bias::Right), 9);
    }

    #[test]
    fn word_and_line_ranges_match_expected_boundaries() {
        let rope = Rope::from(
            "test text:\nabcde 中文🎉 test\nhello[()]\ntest_connector ____\nRope\nrök\ngrande île\n",
        );
        let cases = [
            (0, 0, Some("test")),
            (0, 4, Some(" ")),
            (1, 0, Some("abcde")),
            (1, 6, Some("中")),
            (1, 13, Some("🎉")),
            (2, 5, Some("[")),
            (3, 5, Some("test_connector")),
            (3, 16, Some("____")),
            (5, 0, Some("rök")),
            (6, 8, Some("île")),
        ];
        for (line, col, expected) in cases {
            let offset = rope.line_start_offset(line) + col;
            let got = word_range_at(&rope, offset).map(|r| rope.slice(r).to_string());
            assert_eq!(got.as_deref(), expected, "line {line}, col {col}");
        }

        let lines = Rope::from("first line\nsecond line\nthird");
        assert_eq!(
            lines.slice(line_range_at(&lines, 5)).to_string(),
            "first line"
        );
        assert_eq!(
            lines
                .slice(line_range_at(&lines, lines.line_start_offset(2) + 1))
                .to_string(),
            "third"
        );
    }

    #[test]
    fn bounded_word_navigation_matches_full_scan_everywhere() {
        let long_word = "a".repeat(WORD_SCAN_WINDOW_BYTES + 64);
        let long_spaces = " ".repeat(WORD_SCAN_WINDOW_BYTES + 64);
        let sample = format!(
            "  hello, don't 1,234.56 foo_bar\n日本語🎉 test{long_spaces}{long_word}  tail"
        );
        let rope = Rope::from(sample.as_str());

        let nl = sample.find('\n').unwrap();
        assert_eq!(prev_word_start(&rope, nl + 1), nl);
        assert_eq!(next_word_end(&rope, nl), nl + 1);
        assert_eq!(next_word_end(&rope, nl - 3), nl);

        let long_start = sample.find(&long_word).unwrap();
        assert_eq!(
            word_range_at(&rope, long_start + 10),
            Some(long_start..long_start + long_word.len())
        );
        assert_eq!(
            prev_word_start(&rope, long_start + long_word.len()),
            long_start
        );
        assert_eq!(
            next_word_end(&rope, long_start),
            long_start + long_word.len() + 2
        );

        let emoji = Rope::from("a👍🏽b👨‍👩‍👧🇺🇸e\u{0301}z");
        let mut forward = vec![0];
        let mut cur = 0;
        while cur < emoji.len() {
            cur = next_grapheme_boundary(&emoji, cur);
            forward.push(cur);
        }
        assert_eq!(forward.len(), 8, "7 grapheme clusters + start");
        let mut backward = vec![emoji.len()];
        cur = emoji.len();
        while cur > 0 {
            cur = prev_grapheme_boundary(&emoji, cur);
            backward.push(cur);
        }
        backward.reverse();
        assert_eq!(forward, backward);
    }

    #[test]
    fn ime_preedit_does_not_pollute_history_and_commits_atomically() {
        let mut m = TextModel::new(EditPolicy {
            multi_line: true,
            validate: None,
        })
        .with_rope(Rope::from("hello world"));
        m.set_selected_range(6..11);

        m.set_composition("に", 1);
        assert_eq!(m.value(), "hello に");
        assert!(!m.can_undo());
        assert!(!m.is_clean());

        m.set_composition("にほん", 3);
        assert_eq!(m.value(), "hello にほん");
        assert!(!m.can_undo());

        // Cancelling restores original replaced text without adding an undo entry.
        m.cancel_composition();
        assert_eq!(m.value(), "hello world");
        assert_eq!(m.selected_range(), 6..11);
        assert!(!m.can_undo());
        assert!(m.is_clean());

        // Committing pushes one atomic undo entry restoring the replaced text.
        m.set_composition("に", 1);
        m.insert_text("日本");
        assert_eq!(m.value(), "hello 日本");
        assert!(m.can_undo());
        m.undo();
        assert_eq!(m.value(), "hello world");
        assert_eq!(m.selected_range(), 6..11);
        assert!(m.is_clean());
    }

    #[test]
    fn single_line_validation_and_indent_remapping_work() {
        let mut field = TextModel::new(EditPolicy {
            multi_line: false,
            validate: Some(|s| s.bytes().all(|b| b.is_ascii_digit())),
        });
        assert!(field.insert_text("123").is_some());
        assert!(field.insert_text("abc").is_none());
        assert_eq!(field.value(), "123");
        assert!(field.enter().is_none());

        let mut doc = TextModel::new(EditPolicy {
            multi_line: true,
            validate: None,
        })
        .with_rope(Rope::from("alpha\nbeta\ngamma"));
        doc.set_selected_range(2..9);
        doc.indent(true);
        assert_eq!(doc.value(), "\talpha\n\tbeta\ngamma");
        assert_eq!(doc.selected_range(), 3..11);
        doc.outdent();
        assert_eq!(doc.value(), "alpha\nbeta\ngamma");
        assert_eq!(doc.selected_range(), 2..9);
    }

    #[test]
    fn replace_all_remaps_cursor_and_round_trips_undo_redo() {
        let ranges = [0..3, 8..11];
        assert_eq!(remap_offset(ranges.iter().map(|r| (r.clone(), 1)), 0), 0);
        assert_eq!(remap_offset(ranges.iter().map(|r| (r.clone(), 1)), 2), 1);
        assert_eq!(remap_offset(ranges.iter().map(|r| (r.clone(), 0)), 2), 0);
        assert_eq!(remap_offset(ranges.iter().map(|r| (r.clone(), 5)), 2), 2);
        assert_eq!(remap_offset(ranges.iter().map(|r| (r.clone(), 1)), 3), 1);
        assert_eq!(remap_offset(ranges.iter().map(|r| (r.clone(), 1)), 5), 3);
        assert_eq!(remap_offset(ranges.iter().map(|r| (r.clone(), 1)), 10), 7);
        assert_eq!(remap_offset(ranges.iter().map(|r| (r.clone(), 0)), 10), 5);
        assert_eq!(remap_offset(ranges.iter().map(|r| (r.clone(), 1)), 11), 7);

        let mut doc = TextModel::new(EditPolicy {
            multi_line: true,
            validate: None,
        })
        .with_rope(Rope::from("aa bb cc aa dd aa"));
        doc.place_caret(10, false);
        doc.replace_all(&[0..2, 9..11, 15..17], "XYZ");
        assert_eq!(doc.value(), "XYZ bb cc XYZ dd XYZ");
        assert_eq!(doc.cursor(), 11);

        doc.undo();
        assert_eq!(doc.value(), "aa bb cc aa dd aa");
        assert_eq!(doc.cursor(), 10);
        doc.redo();
        assert_eq!(doc.value(), "XYZ bb cc XYZ dd XYZ");
        assert_eq!(doc.cursor(), 11);
    }

    fn insert_tx(offset: usize, text: &str, kind: EditKind) -> Transaction {
        Transaction {
            kind,
            payload: TxPayload::Changes {
                undo: ChangeSet::single(offset..offset + text.len(), ""),
                redo: ChangeSet::single(offset..offset, text),
            },
            selection_before: Selection::caret(offset),
            selection_after: Selection::caret(offset + text.len()),
        }
    }

    fn undo_changes(h: &mut History) -> ChangeSet {
        match h.replay(true).unwrap().payload {
            ReplayPayload::Changes(c) => c,
            ReplayPayload::Snapshot(_) => panic!("expected changes"),
        }
    }

    #[test]
    fn adjacent_typing_coalesces_and_splits_at_limit() {
        let mut h = History::new();
        h.push(insert_tx(0, "a", EditKind::Insert));
        h.push(insert_tx(1, "b", EditKind::Insert));
        assert_eq!(undo_changes(&mut h), ChangeSet::single(0..2, ""));
        assert!(h.replay(true).is_none());

        let mut h = History::new();
        for i in 0..1_100 {
            h.push(insert_tx(i, "a", EditKind::Insert));
        }
        assert_eq!(
            undo_changes(&mut h),
            ChangeSet::single(MAX_CHANGES_PER_ENTRY..MAX_CHANGES_PER_ENTRY + 100, "")
        );
        assert_eq!(
            undo_changes(&mut h),
            ChangeSet::single(0..MAX_CHANGES_PER_ENTRY, "")
        );
        assert!(h.replay(true).is_none());
    }

    #[test]
    fn multi_edit_atomic_isolation_and_stack_cap() {
        let mut h = History::new();
        h.push(Transaction {
            kind: EditKind::Atomic,
            payload: TxPayload::Changes {
                undo: ChangeSet::new(vec![
                    Change {
                        range: 0..1,
                        text: String::new(),
                    },
                    Change {
                        range: 2..3,
                        text: String::new(),
                    },
                    Change {
                        range: 4..5,
                        text: String::new(),
                    },
                ]),
                redo: ChangeSet::new(vec![
                    Change {
                        range: 0..0,
                        text: "a".into(),
                    },
                    Change {
                        range: 1..1,
                        text: "b".into(),
                    },
                    Change {
                        range: 2..2,
                        text: "c".into(),
                    },
                ]),
            },
            selection_before: Selection::caret(0),
            selection_after: Selection::caret(5),
        });
        assert_eq!(undo_changes(&mut h).len(), 3);

        h.push(insert_tx(0, "x", EditKind::Atomic));
        h.push(insert_tx(1, "y", EditKind::Insert));
        assert_eq!(undo_changes(&mut h).len(), 1);
        assert_eq!(undo_changes(&mut h).len(), 1);

        let mut capped = History::new();
        for i in 0..1_100 {
            capped.push(insert_tx(i, "a", EditKind::Atomic));
        }
        for _ in 0..MAX_HISTORY_ENTRIES {
            assert!(capped.replay(true).is_some());
        }
        assert!(capped.replay(true).is_none());
    }

    #[test]
    fn clean_state_tracks_undo_redo_and_save_boundaries() {
        let mut h = History::new();
        assert!(h.is_clean());

        h.push(insert_tx(0, "a", EditKind::Insert));
        assert!(!h.is_clean());
        assert!(h.replay(true).is_some());
        assert!(h.is_clean());
        assert!(h.can_redo());
        assert!(h.replay(false).is_some());
        assert!(!h.is_clean());

        let saved = h.begin_save();
        h.mark_saved(saved);
        assert!(h.is_clean());
        h.push(insert_tx(1, "b", EditKind::Insert));
        assert!(!h.is_clean());
        assert_eq!(undo_changes(&mut h), ChangeSet::single(1..2, ""));
        assert!(h.is_clean());

        let snapshot = h.begin_save();
        h.push(insert_tx(1, "c", EditKind::Insert));
        h.mark_saved(snapshot);
        assert!(!h.is_clean());
        assert!(h.replay(true).is_some());
        assert!(h.is_clean());

        assert!(h.replay(true).is_some());
        h.push(insert_tx(0, "z", EditKind::Insert));
        assert!(!h.is_clean());
        assert!(h.replay(true).is_some());
        assert!(!h.is_clean());
    }

    fn test_style() -> TextStyle {
        TextStyle::regular("Arial", 14.)
    }

    #[test]
    fn incremental_splice_longest_row_and_ascii_fast_fit() {
        let mut index = WrapIndex::new(test_style(), Some(10.));
        let mut text = Rope::from("aa\nbbbb\nc");
        let mut fake = |line: &str, _: f32, out: &mut Vec<usize>| -> f32 {
            if line.len() > 2 {
                out.push(2);
            }
            0.0
        };
        index.splice_line_range(&text, 0..0, 0..text.lines_len(), 0, &mut fake);
        assert_eq!(index.line_count(), 3);
        assert_eq!(index.visual_row_count(), 4);
        assert_eq!(index.longest_line(), 1);
        assert_eq!(index.first_visual_row(2), 3);
        assert_eq!(index.visual_row_to_line(2), 1);

        let range = text.line_start_offset(1)..text.line_end_offset(1);
        text.replace(range, "dd\neeeeee");
        index.splice_line_range(&text, 1..2, 1..3, 0, &mut fake);
        assert_eq!(index.line_count(), 4);
        assert_eq!(index.visual_row_count(), 5);
        assert_eq!(index.longest_line(), 2);

        let all = 0..text.len();
        text.replace(all, "");
        index.splice_line_range(&text, 0..4, 0..1, 0, &mut fake);
        assert_eq!(index.line_count(), 1);
        assert_eq!(index.visual_row_count(), 1);
        assert_eq!(index.line(0).unwrap().ranges.get(0), Some(0..0));
        assert_eq!(index.line(0).unwrap().ranges.len(), 1);

        let mut fast = WrapIndex::new(test_style(), Some(100.));
        let sample = Rope::from("short ascii\nlonger ascii line\n日本語\n");
        let mut called = Vec::new();
        fast.splice_line_range(&sample, 0..0, 0..sample.lines_len(), 12, &mut |line, _, _| {
            called.push(line.to_string());
            0.0
        });
        assert_eq!(called, vec!["longer ascii line", "日本語"]);
    }

    #[test]
    fn multi_chunk_prefix_sums_match_across_splices() {
        let mut lines = Vec::new();
        for i in 0..700 {
            if i % 3 == 0 {
                lines.push("aaaa");
            } else {
                lines.push("bb");
            }
        }
        let mut text = Rope::from(lines.join("\n"));
        let mut index = WrapIndex::new(test_style(), Some(10.));
        let mut fake = |line: &str, _: f32, out: &mut Vec<usize>| -> f32 {
            if line.len() > 2 {
                out.push(2);
            }
            0.0
        };
        index.splice_line_range(&text, 0..0, 0..text.lines_len(), 0, &mut fake);
        assert!(index.chunks.chunks.len() >= 3);

        // Splice across a chunk boundary (lines 250..265 replaced by 3 wrapped lines).
        let start = text.line_start_offset(250);
        let end = text.line_end_offset(264);
        text.replace(start..end, "cccc\ndddd\neeee");
        index.splice_line_range(&text, 250..265, 250..253, 0, &mut fake);

        let mut expected_row = 0usize;
        for line_ix in 0..index.line_count() {
            assert_eq!(index.first_visual_row(line_ix), expected_row);
            let rows = index.line(line_ix).unwrap().row_count();
            for sub in 0..rows {
                assert_eq!(
                    index.visual_row_to_line_and_sub_row(expected_row + sub),
                    (line_ix, sub)
                );
            }
            expected_row += rows;
        }
        assert_eq!(index.visual_row_count(), expected_row);
    }

    #[test]
    fn wrap_boundary_affinity_and_hanging_indent_geometry() {
        let text = Rope::from("first line\nthis one wraps");
        let mut index = WrapIndex::new(test_style(), None);
        index.chunks = LineChunks::from_vec(vec![
            WrappedLine {
                byte_len: 10,
                hanging_indent: 0.0,
                ranges: LineRanges::Single(10),
            },
            WrappedLine {
                byte_len: 14,
                hanging_indent: 0.0,
                ranges: LineRanges::Wrapped(vec![0..9, 9..14].into_boxed_slice()),
            },
        ]);
        assert_eq!(index.offset_to_display(&text, 20, true), DisplayPos::new(1, 0, 9));
        assert_eq!(index.offset_to_display(&text, 20, false), DisplayPos::new(2, 1, 0));
        assert_eq!(index.display_to_offset(&text, DisplayPos::new(2, 1, 3)), 23);
    }

    #[test]
    fn row_layout_hanging_indent_and_end_affinity_geometry() {
        let mut s1 = ShapedRow::default();
        s1.len = 10;
        let mut s2 = ShapedRow::default();
        s2.len = 5;
        let layout = RowLayout::from_slice(
            vec![VisualRow::from(s1), VisualRow::from(s2)],
            0,
            0,
            2,
            20.,
        );
        let (ix, affinity) = layout
            .closest_index_for_position(point(999., 5.), 20.)
            .unwrap();
        assert_eq!(ix, 10);
        assert!(affinity);
        assert_eq!(layout.position_for_index(10, 20., true).map(|p| p.y), Some(0.));
        assert_eq!(layout.position_for_index(10, 20., false), Some(point(20., 20.)));
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
