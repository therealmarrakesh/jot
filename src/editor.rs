use std::ops::Range;
use std::time::{Duration, Instant};

use crate::geom::{Axis, Edges, Point, Rect, Rgba, Size, point, size};
use crate::gfx::{Canvas, TextStyle, TextSystem, fill};
use crate::lines::{FrameLayout, LineIndex, Viewport};
use crate::platform::{CursorStyle, Modifiers, MouseButton, SystemSettings};
use crate::text::{
    Bias, EditPolicy, LineChange, Rope, RopeExt as _, TextModel, ValidateFn,
    next_grapheme_boundary, next_word_end, prev_grapheme_boundary, prev_word_start,
};
use crate::theme::Palette;
use crate::types::{SharedStr, take_due};
use crate::ui::{
    AnimKey, AppNode, AppUi, CustomDraw, Easing, EditorSlot, HitId, box_, custom, row,
};

const RIGHT_MARGIN: f32 = 10.0;

#[derive(Clone, Copy, Debug)]
pub(crate) struct DocOptions {
    pub(crate) soft_wrap: bool,
    pub(crate) paddings: Edges<f32>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct FieldOptions {
    pub(crate) placeholder: SharedStr,
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
    lines: LineIndex,
    soft_wrap: bool,
    last_line_height: f32,
    paddings: Edges<f32>,
    scrollbar: ScrollbarState,
    auto_scroll: AutoScroll,
    pending_context_menu: Option<(Point, usize)>,
}

impl DocumentView {
    fn new(opts: DocOptions) -> Self {
        Self {
            lines: LineIndex::new(TextStyle::default(), None),
            soft_wrap: opts.soft_wrap,
            last_line_height: 20.0,
            paddings: opts.paddings,
            scrollbar: ScrollbarState::default(),
            auto_scroll: AutoScroll::default(),
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
    frame: Option<Frame>,
    placeholder: SharedStr,
}

impl Editor {
    pub(crate) fn document(rope: Rope, opts: DocOptions) -> Self {
        let model = TextModel::new(EditPolicy {
            multi_line: true,
            validate: None,
        });
        let doc = Some(Box::new(DocumentView::new(opts)));
        Self::new(model.with_rope(rope), doc, SharedStr::default())
    }

    pub(crate) fn field(opts: FieldOptions) -> Self {
        let model = TextModel::new(EditPolicy {
            multi_line: false,
            validate: opts.validate,
        });
        Self::new(model, None, opts.placeholder)
    }

    fn new(model: TextModel, doc: Option<Box<DocumentView>>, placeholder: SharedStr) -> Self {
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
            placeholder,
        }
    }

    pub(crate) fn model(&self) -> &TextModel {
        &self.model
    }

    fn fresh_frame(&self) -> Option<&Frame> {
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
            doc.lines.apply_line_change(self.model.text(), &lines, ts);
        }
        let aff = self.model.selection().end_affinity;
        self.request_reveal(self.model.cursor(), aff, None, true);
    }

    pub(crate) fn is_selecting(&self) -> bool {
        self.selecting
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
            doc.lines.set_wrap_width(text, width, ts);
            self.scroll.x = 0.0;
        } else {
            doc.lines.set_wrap_width(text, None, ts);
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

    pub(crate) fn start_blink(&mut self, settings: &SystemSettings) {
        self.blink
            .start((settings.caret_blink_interval, settings.caret_blink_timeout));
    }

    pub(crate) fn pause_blink(&mut self) {
        self.blink.pause();
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

    /// The editor's node: a field's single row, or a document's padded text with its
    /// `scrollbars` over it.
    pub(crate) fn view(
        &self,
        slot: EditorSlot,
        line_height: f32,
        scrollbars: Vec<AppNode>,
    ) -> AppNode {
        let (hit, editor_draw) = (HitId::Editor(slot), CustomDraw::Editor(slot));
        let cursor = CursorStyle::IBeam;
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
struct BlinkState {
    active: bool,
    visible: bool,
    paused: bool,
    started_at: Option<Instant>,
    /// The system's blink interval and how long blinking lasts, captured by `start`.
    timing: (Option<Duration>, Option<Duration>),
    next: Option<Instant>,
}

impl BlinkState {
    fn visible(&self) -> bool {
        self.paused || self.visible
    }

    fn stop(&mut self) {
        self.active = false;
        self.paused = false;
        self.visible = false;
        self.started_at = None;
        self.next = None;
    }

    fn start(&mut self, timing: (Option<Duration>, Option<Duration>)) {
        let now = Instant::now();
        self.timing = timing;
        self.active = true;
        self.paused = false;
        self.visible = false;
        self.started_at = Some(now);
        self.step(now);
    }

    /// Holds the caret visible for an interval (after typing or moving), then blinks again.
    fn pause(&mut self) {
        if !self.active && !self.visible {
            return;
        }
        let now = Instant::now();
        self.paused = true;
        self.visible = true;
        self.started_at = Some(now);
        self.next = self.timing.0.map(|interval| now + interval);
    }

    /// Blinks if it's due; whether the caret changed.
    fn tick(&mut self, now: Instant) -> bool {
        if !take_due(&mut self.next, now) || !self.active {
            return false;
        }
        self.paused = false;
        self.step(now);
        true
    }

    fn step(&mut self, now: Instant) {
        let (interval, timeout) = self.timing;
        let timed_out = timeout.is_some_and(|timeout| {
            self.started_at
                .is_some_and(|start| now.saturating_duration_since(start) >= timeout)
        });
        match interval {
            Some(interval) if !timed_out => {
                self.visible = !self.visible;
                self.next = Some(now + interval);
            }
            _ => {
                self.visible = true;
                self.next = None;
            }
        }
    }
}

#[derive(Default)]
struct AutoScroll {
    delta: Option<f32>,
    next: Option<Instant>,
    last_drag_pos: Option<Point>,
}

/// How often a drag-selection past the editor's edge scrolls.
const AUTO_SCROLL_INTERVAL: Duration = Duration::from_millis(16);

impl AutoScroll {
    fn compute_delta(y: f32, bounds: Rect) -> Option<f32> {
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

    fn stop(&mut self) {
        self.delta = None;
        self.next = None;
        self.last_drag_pos = None;
    }

    fn update(&mut self, delta: Option<f32>) {
        let was_active = self.delta.is_some();
        self.delta = delta;
        if delta.is_none() {
            self.next = None;
        } else if !was_active {
            self.next = Some(Instant::now() + AUTO_SCROLL_INTERVAL);
        }
    }
}

impl Editor {
    fn hit_test_point(&self, pos: Point) -> (usize, bool) {
        let Some(frame) = self.fresh_frame() else {
            return (0, false);
        };
        let rel = pos
            - frame.content_bounds.origin
            - point(
                self.scroll.x - frame.scroll_x,
                (self.scroll.y - frame.scroll_y) as f32,
            );
        frame
            .layout
            .hit_test(rel, self.model.text().len(), self.model.is_multi_line())
    }

    pub(crate) fn on_mouse_down(
        &mut self,
        button: MouseButton,
        position: Point,
        modifiers: Modifiers,
        click_count: usize,
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
                self.move_to_with_affinity(offset, aff);
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
            self.move_to_with_affinity(offset, aff);
        }
    }

    pub(crate) fn on_mouse_move(
        &mut self,
        position: Point,
        dragging_left: bool,
        focused: bool,
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
                doc.auto_scroll.update(delta.map(|d| -d));
            }
            self.request_reveal(self.model.cursor(), aff, None, false);
            return true;
        }
        false
    }

    pub(crate) fn on_mouse_up(&mut self, button: MouseButton) -> Option<Point> {
        let mut open_context_menu = None;
        if button == MouseButton::Right
            && let Some(doc) = &mut self.doc
            && let Some((pos, offset)) = doc.pending_context_menu.take()
        {
            if !self.model.selection().contains(offset) {
                self.move_to_with_affinity(offset, false);
            }
            open_context_menu = Some(pos);
        }
        self.reset_pointer_selection_state();
        open_context_menu
    }
}

impl Editor {
    fn commit_edit(&mut self, change: Option<LineChange>, ts: &TextSystem) {
        self.pause_blink();
        self.apply_change(change, ts);
    }

    pub(crate) fn selected_string(&self) -> Option<String> {
        (!self.model.selection().is_empty()).then(|| self.model.text().slice(self.model.selection().range()).to_string())
    }

    pub(crate) fn prepare_cut(&mut self) -> Option<String> {
        self.model.commit_composition();
        self.selected_string()
    }

    pub(crate) fn delete_selection(&mut self, ts: &TextSystem) {
        let change = self.model.replace_selection("");
        self.commit_edit(change, ts);
    }

    pub(crate) fn paste(&mut self, raw: &str, ts: &TextSystem) {
        let text = if self.model.is_multi_line() {
            raw
        } else {
            raw.split(['\r', '\n']).next().unwrap_or("")
        };
        if !text.is_empty() {
            let change = self.model.replace_selection(text);
            self.commit_edit(change, ts);
        }
    }

    pub(crate) fn handle_command(&mut self, command: EditCommand, ts: &TextSystem) -> bool {
        match command {
            EditCommand::Backspace => {
                let Some(change) = self.model.delete_backward() else {
                    return false;
                };
                self.commit_edit(Some(change), ts);
            }
            EditCommand::Delete => {
                let change = self.model.delete_forward();
                self.commit_edit(change, ts);
            }
            EditCommand::DeletePrevWord => {
                let change = self.model.delete_prev_word();
                self.commit_edit(change, ts);
            }
            EditCommand::DeleteNextWord => {
                let change = self.model.delete_next_word();
                self.commit_edit(change, ts);
            }
            EditCommand::Enter => {
                let Some(change) = self.model.enter() else {
                    return false;
                };
                self.commit_edit(Some(change), ts);
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
                    self.commit_edit(Some(change), ts);
                }
            }
            EditCommand::Redo => {
                if let Some(change) = self.model.redo() {
                    self.commit_edit(Some(change), ts);
                }
            }
            EditCommand::IndentInline | EditCommand::Indent | EditCommand::Outdent
                if !self.model.is_multi_line() =>
            {
                return false;
            }
            EditCommand::IndentInline | EditCommand::Indent => {
                let change = self.model.indent(matches!(command, EditCommand::Indent));
                self.commit_edit(change, ts);
            }
            EditCommand::Outdent => {
                let change = self.model.outdent();
                self.commit_edit(change, ts);
            }
            EditCommand::SelectAll => self.select_all(),
            EditCommand::Move { motion, extend } => {
                return self.move_caret(motion, extend);
            }
        }
        true
    }

    pub(crate) fn handle_text_input(&mut self, text: &str, ts: &TextSystem) {
        let change = self.model.insert_text(text);
        self.commit_edit(change, ts);
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
        let origin = frame.content_bounds.origin;
        let selection = self.model.selection().range();
        frame
            .layout
            .bounds_for_range(self.model.text(), selection, frame.last_cursor, origin)
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
            let disp = doc.lines.offset_to_display(text, offset, end_affinity);
            if let Some((range, _)) = doc.lines.visual_row_range(pt.row, disp.sub_row) {
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
            let disp = doc.lines.offset_to_display(text, offset, end_affinity);
            if let Some((range, soft_end)) = doc.lines.visual_row_range(pt.row, disp.sub_row) {
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
        let row_layout = frame.layout.line(pt.row)?;
        let line_height = frame.layout.line_height;
        let pos = row_layout.position_for_index(pt.column, line_height, end_affinity)?;
        Some((pos.x, pt.column))
    }

    fn move_to_with_affinity(&mut self, offset: usize, end_affinity: bool) {
        self.goal = None;
        self.model.place_caret(offset, end_affinity);
        self.request_reveal(self.model.cursor(), end_affinity, None, false);
        self.pause_blink();
    }

    fn page_rows(&self, doc: &DocumentView) -> isize {
        let lh = self
            .frame
            .as_ref()
            .map_or(doc.last_line_height, |f| f.layout.line_height)
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
        if doc.lines.visual_row_count() == 0 {
            return (offset, end_affinity);
        }
        let text = self.model.text();
        let mut display = doc.lines.offset_to_display(text, offset, end_affinity);
        let max_row = doc.lines.visual_row_count().saturating_sub(1);
        display.row = display.row.saturating_add_signed(delta_rows).min(max_row);
        display.col = 0;

        let mut target_offset = doc.lines.display_to_offset(text, display);
        let mut target_affinity = false;

        if let Some((pref_x, byte_col)) = goal {
            let mut next_disp = doc.lines.offset_to_display(text, target_offset, false);
            next_disp.col = 0;
            let next_pt = text.offset_to_point(doc.lines.display_to_offset(text, next_disp));
            let line_start = text.line_start_offset(next_pt.row);
            if let Some(frame) = self.fresh_frame()
                && let Some(row_layout) = frame.layout.line(next_pt.row)
            {
                let line_height = frame.layout.line_height;
                if let Some((local_ix, affinity)) = row_layout.closest_index_for_position(
                    point(pref_x, next_disp.sub_row as f32 * line_height),
                    line_height,
                ) {
                    target_offset = line_start + local_ix;
                    target_affinity = affinity;
                }
            } else if let Some((sub_range, soft_end)) =
                doc.lines.visual_row_range(next_pt.row, next_disp.sub_row)
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

    fn move_caret(&mut self, motion: Motion, extend: bool) -> bool {
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
                    if doc.lines.visual_row_count() > 0 {
                        let text = self.model.text();
                        let before_row = doc
                            .lines
                            .offset_to_display(text, sel.head, sel.end_affinity)
                            .row;
                        let after_row = doc.lines.offset_to_display(text, target.0, target.1).row;
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
            self.pause_blink();
            self.model.break_coalescing();
            self.model.extend_selection(target, target_aff, None);
            self.request_reveal(self.model.cursor(), target_aff, None, true);
        } else {
            self.model.place_caret(target, target_aff);
            self.request_reveal(self.model.cursor(), target_aff, direction, true);
            self.pause_blink();
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
        let Some(frame) = self.frame.as_ref() else {
            return;
        };
        canvas.with_clip(frame.input_bounds, |canvas| {
            for &rect in &frame.selection_rects {
                canvas.paint_quad(fill(rect, p.selection));
            }
            let origin = frame.content_bounds.origin;
            let marked = frame.marked_range.as_ref();
            frame.layout.paint(origin, frame.text_color, marked, canvas);
            if frame.show_caret {
                canvas.paint_quad(fill(frame.caret_rect, p.caret));
            }
        });
    }

    fn finish_frame(
        &mut self,
        layout: FrameLayout,
        wrap_width: Option<f32>,
        text_color: Rgba,
        head_pt: Point,
        is_empty_field: bool,
        params: EditorPaintParams<'_>,
    ) {
        // Vertical scroll is baked into each line's top in f64 so D2D coordinates stay exact on
        // huge files.
        let input_bounds = self.input_bounds;
        let content_bounds = Rect::new(
            point(input_bounds.origin.x + self.scroll.x, input_bounds.origin.y),
            input_bounds.size,
        );
        let caret_rect = caret_rect(
            content_bounds.origin + head_pt,
            layout.line_height,
            params.style.caret_height,
            params.scale,
            params.caret_width_px,
        );
        let show_caret = params.focused
            && params.window_active
            && self.blink.visible()
            && (self.model.selection().is_empty() || self.model.composition().is_some());
        let marked_range = (!is_empty_field)
            .then(|| self.model.composition().map(|c| c.start..c.end))
            .flatten();
        let selection_rects = if params.window_active
            && params.show_selection
            && self.model.composition().is_none()
            && !self.model.selection().is_empty()
        {
            layout.range_rects(&self.model.selection().range(), content_bounds.origin)
        } else {
            Vec::new()
        };
        self.frame = Some(Frame {
            layout,
            revision: self.model.revision(),
            scroll_x: self.scroll.x,
            scroll_y: self.scroll.y,
            input_bounds,
            content_bounds,
            wrap_width,
            text_color,
            caret_rect,
            show_caret,
            selection_rects,
            marked_range,
            last_cursor: self.model.cursor(),
        });
    }

    fn prepare_field(&mut self, input_bounds: Rect, params: EditorPaintParams<'_>, ts: &TextSystem) {
        let style = params.style;
        let text_style = &style.text;
        let line_height = style.line_height.max(1.0);
        let is_empty = self.model.text().len() == 0;
        let (display_str, text_color): (SharedStr, Rgba) = if is_empty {
            (self.placeholder.clone(), params.palette.text_secondary)
        } else {
            (self.model.text().to_string().into(), style.color)
        };
        let text_len = self.model.text().len();
        let layout = FrameLayout::single_row(display_str, text_len, text_style, line_height, ts);
        let head = self.model.cursor();
        let head_pt = layout
            .position_for_offset(self.model.text(), head, false)
            .unwrap_or(Point::ZERO);
        let max_line_w = layout.width().max(head_pt.x);
        self.scroll.content_w = content_width_with_margin(max_line_w, input_bounds.size.width);
        if self.reveal.take().is_some() {
            self.scroll.x =
                reveal_horizontal_scroll(self.scroll.x, head_pt.x, input_bounds.size.width);
        }
        self.scroll.x =
            clamp_horizontal_scroll(self.scroll.x, self.scroll.content_w, input_bounds.size.width);
        self.finish_frame(layout, None, text_color, head_pt, is_empty, params);
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
        let text = self.model.text();
        let reveal_req = self.reveal.take();
        let head = self.model.cursor();
        let end_affinity = self.model.selection().end_affinity;

        let doc = self.doc.as_deref_mut().expect("document view");
        let soft_wrap = doc.soft_wrap;
        let wrap_width = soft_wrap.then_some((input_bounds.size.width - RIGHT_MARGIN).max(1.0));

        let reflowing = doc.lines.style() != text_style
            || doc.lines.wrap_width() != wrap_width
            || (doc.last_line_height - line_height).abs() > 0.01;
        let anchor = if reflowing && reveal_req.is_none() {
            doc.capture_scroll_anchor(self.scroll.y, doc.last_line_height)
        } else {
            None
        };

        doc.lines.sync_layout(text, text_style, wrap_width, ts);
        doc.last_line_height = line_height;

        if let Some(anchor) = anchor {
            self.scroll.y = doc.restore_scroll_anchor(text, anchor, line_height);
        }

        let total_rows = doc.lines.visual_row_count().max(1);
        let content_h = (total_rows as f64 * f64::from(line_height))
            .max(f64::from(input_bounds.size.height));
        self.scroll.content_h = content_h;

        if let Some(req) = reveal_req {
            let target = req.offset.min(text.len());
            let disp = doc.lines.offset_to_display(text, target, req.end_affinity);
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
            max_line_w = doc.lines.longest_line_width(text, ts);
            if reveal_req.is_some() {
                let caret_x = doc.lines.unwrapped_x(text, head, ts);
                max_line_w = max_line_w.max(caret_x);
                self.scroll.x =
                    reveal_horizontal_scroll(self.scroll.x, caret_x, input_bounds.size.width);
            }
            let est_w = content_width_with_margin(max_line_w, input_bounds.size.width);
            self.scroll.x = clamp_horizontal_scroll(self.scroll.x, est_w, input_bounds.size.width);
        }

        let prev_scroll_x = self.scroll.x;
        let view = Viewport {
            size: input_bounds.size,
            scroll_x: self.scroll.x,
            scroll_y: self.scroll.y,
        };
        let (layout, shaped_windowed) =
            doc.lines
                .layout_visible(text, view, head, end_affinity, line_height, ts);
        if !soft_wrap {
            max_line_w = max_line_w.max(layout.width());
        }
        let head_pt = layout
            .position_for_offset(text, head, end_affinity)
            .unwrap_or_else(|| {
                let disp = doc.lines.offset_to_display(text, head, end_affinity);
                point(
                    0.0,
                    (disp.row as f64 * f64::from(line_height) + self.scroll.y) as f32,
                )
            });
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

        self.finish_frame(layout, wrap_width, style.color, head_pt, false, params);

        shaped_windowed && (self.scroll.x - prev_scroll_x).abs() > 1.0
    }

}

/// The caret at `at` (the top of its line), `height` tall and centred on the line.
fn caret_rect(at: Point, line_height: f32, height: f32, scale: f32, width_px: u32) -> Rect {
    let origin = snap_point(point(at.x, at.y + (line_height - height) * 0.5), scale);
    let width = (width_px.max(1) as f32 / scale).max(1.0 / scale);
    Rect::new(origin, size(width, height))
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
        if self.lines.visual_row_count() == 0 || line_height <= 0.0 {
            return None;
        }
        let lh = f64::from(line_height);
        let top = (-scroll_y).max(0.0);
        let top_row = (top / lh).floor() as usize;
        let frac_px = top - top_row as f64 * lh;
        let clamped_row = top_row.min(self.lines.visual_row_count().saturating_sub(1));
        let (line, sub_row) = self.lines.visual_row_to_line_and_sub_row(clamped_row);
        let byte_in_line = self
            .lines
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
        if self.lines.line_count() == 0 || line_height <= 0.0 {
            return 0.0;
        }
        let line = anchor.line.min(self.lines.line_count().saturating_sub(1));
        let line_start = text.line_start_offset(line);
        let line_len = text.line_len(line);
        let target = line_start + anchor.byte_in_line.min(line_len);
        let disp = self.lines.offset_to_display(text, target, false);
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

    /// Where the scrollbar's track starts along `axis`: it's laid out over the padding beside
    /// the text.
    fn track_origin(&self, doc: &DocumentView, axis: Axis) -> f32 {
        match axis {
            Axis::Vertical => self.input_bounds.top() - doc.paddings.top,
            Axis::Horizontal => self.input_bounds.left(),
        }
    }

    /// A press on the scrollbar's thumb (starting a drag) or track (paging); whether the
    /// scrollbar took it, which it doesn't while faded out.
    pub(crate) fn press_scrollbar(&mut self, axis: Axis, thumb: bool, pos: Point) -> bool {
        let scroll = self.scroll_offset();
        let Some(doc) = self.doc.as_deref() else {
            return false;
        };
        if !doc.scrollbar.is_interactive(axis) {
            return false;
        }
        let origin = self.track_origin(doc, axis);
        let doc = self.doc.as_deref_mut().expect("document view");
        let next = doc
            .scrollbar
            .on_mouse_down(axis, thumb, pos, origin, scroll);
        self.apply_scrollbar_scroll(next);
        true
    }

    /// While the thumb is dragged, follows the pointer: whether to repaint now (`None` when no
    /// drag is going on).
    pub(crate) fn drag_scrollbar(&mut self, pos: Point, pressed: bool) -> Option<bool> {
        let scroll = self.scroll_offset();
        let doc = self.doc.as_deref()?;
        let origin = self.track_origin(doc, doc.scrollbar.drag_axis()?);
        let doc = self.doc.as_deref_mut().expect("document view");
        let (next, notify) = doc.scrollbar.on_mouse_move(pos, pressed, origin, scroll)?;
        self.apply_scrollbar_scroll(next);
        Some(notify)
    }

    /// Ends a thumb drag at `pos`.
    pub(crate) fn release_scrollbar(&mut self, pos: Point) {
        let scroll = self.scroll_offset();
        let Some(doc) = self.doc.as_deref() else {
            return;
        };
        let Some(axis) = doc.scrollbar.drag_axis() else {
            return;
        };
        let origin = self.track_origin(doc, axis);
        let doc = self.doc.as_deref_mut().expect("document view");
        let next = doc.scrollbar.on_mouse_up(pos, origin, scroll);
        self.apply_scrollbar_scroll(next);
    }

    pub(crate) fn render_scrollbars(
        &mut self,
        slot: EditorSlot,
        p: Palette,
        now: Instant,
        ui: &mut AppUi,
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
        doc.scrollbar.render(slot, vp, p, now, ui)
    }

    pub(crate) fn on_scroll_wheel(
        &mut self,
        delta_lines: Point,
        page_scroll: bool,
        modifiers: Modifiers,
    ) -> bool {
        let Some(doc) = self.doc.as_deref().filter(|_| !modifiers.ctrl) else {
            return false;
        };
        let lh = doc.last_line_height.max(1.0);
        let vp = self.input_bounds.size;
        let dx = if delta_lines.x != 0.0 {
            let space_w = self
                .frame
                .as_ref()
                .map_or(lh * 0.5, |f| f.layout.space_width.max(1.0));
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

    /// Acts on whatever is due at `now`: blinking, auto-scrolling, scrollbar fading. Returns
    /// whether the editor needs repainting.
    pub(crate) fn tick(&mut self, now: Instant) -> bool {
        let mut changed = self.blink.tick(now);
        let Some(doc) = &mut self.doc else {
            return changed;
        };
        changed |= doc.scrollbar.tick(now);
        if take_due(&mut doc.auto_scroll.next, now) {
            let Some(delta) = doc.auto_scroll.delta else {
                doc.auto_scroll.stop();
                return changed;
            };
            doc.auto_scroll.next = Some(now + AUTO_SCROLL_INTERVAL);
            let drag_pos = doc.auto_scroll.last_drag_pos;
            self.reveal = None;
            self.clamp_and_set_scroll_f64(self.scroll.x, self.scroll.y + f64::from(delta));
            if let Some(pos) = drag_pos {
                let (offset, aff) = self.hit_test_point(pos);
                self.select_to_with_affinity(offset, aff);
            }
            changed = true;
        }
        changed
    }

    /// When the editor next needs a `tick`.
    pub(crate) fn deadline(&self) -> Option<Instant> {
        let doc = self.doc.as_deref();
        let auto_scroll = doc.and_then(|doc| doc.auto_scroll.next);
        let scrollbar = doc.and_then(|doc| doc.scrollbar.deadline());
        [self.blink.next, auto_scroll, scrollbar]
            .into_iter()
            .flatten()
            .min()
    }
}

#[derive(Clone, Debug)]
pub(crate) struct EditorPaintStyle {
    pub(crate) text: TextStyle,
    pub(crate) line_height: f32,
    pub(crate) color: Rgba,
    pub(crate) caret_height: f32,
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

/// The editor's last layout, with where it was placed and what was drawn over it.
struct Frame {
    layout: FrameLayout,
    revision: u64,
    scroll_x: f32,
    scroll_y: f64,
    input_bounds: Rect,
    content_bounds: Rect,
    wrap_width: Option<f32>,
    text_color: Rgba,
    caret_rect: Rect,
    show_caret: bool,
    selection_rects: Vec<Rect>,
    marked_range: Option<Range<usize>>,
    last_cursor: usize,
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
struct TrackMetrics {
    margin: f32,
    thumb_len: f32,
    travel: f32,
    max_scroll: f32,
}

impl TrackMetrics {
    fn compute(viewport_span: f32, content_span: f32, corner_reserve: f32) -> Self {
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

    fn thumb_local(self, scroll_offset: f32) -> f32 {
        let ratio = (-scroll_offset / self.max_scroll).clamp(0., 1.);
        self.margin + ratio * self.travel
    }

    fn apply_drag(
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
struct ScrollbarViewport {
    size: Size,
    content_size: Size,
    scroll_offset: Point,
    horizontal_left: f32,
    horizontal: bool,
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

struct ScrollbarState {
    drag: Option<Drag>,
    last_offset: Point,
    last_activity: Option<Instant>,
    was_hovered: bool,
    last_drag_notify: Instant,
    /// When a throttled drag next notifies, and when the bar next starts fading.
    drag_at: Option<Instant>,
    idle_at: Option<Instant>,
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
            drag_at: None,
            idle_at: None,
            axes: [None, None],
        }
    }

    fn drag_axis(&self) -> Option<Axis> {
        self.drag.map(|d| d.axis)
    }

    fn is_interactive(&self, axis: Axis) -> bool {
        self.axes[axis_ix(axis)].is_some_and(|a| a.interactive)
    }

    fn tick(&mut self, now: Instant) -> bool {
        let idle = take_due(&mut self.idle_at, now);
        let drag = take_due(&mut self.drag_at, now);
        if drag {
            self.last_drag_notify = now;
        }
        idle || drag
    }

    fn deadline(&self) -> Option<Instant> {
        self.idle_at.into_iter().chain(self.drag_at).min()
    }

    fn on_wheel(&mut self, offset: Point, now: Instant) -> bool {
        if offset == self.last_offset {
            return false;
        }
        self.last_offset = offset;
        self.last_activity = Some(now);
        self.idle_at = None;
        true
    }

    fn render(
        &mut self,
        slot: EditorSlot,
        vp: ScrollbarViewport,
        p: Palette,
        now: Instant,
        ui: &mut AppUi,
    ) -> Vec<AppNode> {
        let scroll_offset = vp.scroll_offset;
        if scroll_offset != self.last_offset {
            self.last_offset = scroll_offset;
            self.last_activity = Some(now);
            self.idle_at = None;
        }

        let is_axis_hovered = |axis: Axis, ui: &AppUi| {
            ui.is_hovered(HitId::ScrollTrack(slot, axis))
                || ui.is_hovered(HitId::ScrollThumb(slot, axis))
        };
        let hovered =
            is_axis_hovered(Axis::Vertical, ui) || is_axis_hovered(Axis::Horizontal, ui);
        if hovered || (self.was_hovered && !hovered) {
            self.last_activity = Some(now);
            self.idle_at = None;
        }
        self.was_hovered = hovered;

        let visible = hovered
            || self.drag.is_some()
            || self
                .last_activity
                .is_some_and(|t| now.saturating_duration_since(t) < IDLE_HOLD);

        if !hovered
            && self.drag.is_none()
            && self.idle_at.is_none()
            && let Some(last) = self.last_activity
            && now.saturating_duration_since(last) < IDLE_HOLD
        {
            self.idle_at = Some(last + IDLE_HOLD);
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

    fn on_mouse_down(
        &mut self,
        axis: Axis,
        hit_thumb: bool,
        position: Point,
        track_origin: f32,
        current_offset: Point,
    ) -> Option<Point> {
        let active = self.axes[axis_ix(axis)]?;
        if !active.interactive {
            return None;
        }
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

    fn on_mouse_move(
        &mut self,
        position: Point,
        dragging_primary: bool,
        track_origin: f32,
        current_offset: Point,
    ) -> Option<(Option<Point>, bool)> {
        let drag = self.drag.filter(|_| dragging_primary)?;
        let active = self.axes[axis_ix(drag.axis)]?;
        let now = Instant::now();
        let next = active
            .metrics
            .apply_drag(drag.axis, track_origin, position, drag.grab, current_offset);
        let moved = next != current_offset;
        if moved {
            let elapsed = now.saturating_duration_since(self.last_drag_notify);
            if elapsed >= DRAG_NOTIFY_INTERVAL {
                self.last_drag_notify = now;
                self.drag_at = None;
                return Some((Some(next), true));
            }
            if self.drag_at.is_none() {
                self.drag_at = Some(self.last_drag_notify + DRAG_NOTIFY_INTERVAL);
            }
        }
        Some((moved.then_some(next), false))
    }

    fn on_mouse_up(
        &mut self,
        position: Point,
        track_origin: f32,
        current_offset: Point,
    ) -> Option<Point> {
        let drag = self.drag.take()?;
        self.drag_at = None;
        self.last_activity = Some(Instant::now());
        let active = self.axes[axis_ix(drag.axis)]?;
        Some(active.metrics.apply_drag(
            drag.axis,
            track_origin,
            position,
            drag.grab,
            current_offset,
        ))
    }
}

#[cfg(test)]
mod tests {
    use crate::geom::{point, rect, size};
    use crate::ui::TabId;

    use super::*;

    /// Drawing and clicking agree: clicking the middle of the drawn caret lands on the caret,
    /// clicking inside a drawn selection box lands inside the selection, and the IME underline
    /// covers the composition. Runs over wrapped, scrolled and horizontally windowed lines.
    #[test]
    fn clicks_land_where_the_caret_and_selection_are_drawn() {
        let ts = TextSystem::new().unwrap();
        let palette = Palette::resolve(false, None, None);
        let style = EditorPaintStyle {
            text: TextStyle::regular("Arial", 14.),
            line_height: 20.,
            color: palette.text_primary,
            caret_height: 17.,
        };
        let params = EditorPaintParams {
            style: &style,
            palette,
            window_active: true,
            focused: true,
            show_selection: true,
            scale: 1.0,
            caret_width_px: 1,
        };
        let bounds = Rect::new(point(10., 20.), size(300., 120.));
        let words = "lorem ipsum dolor sit amet ".repeat(30);
        let text = format!("short line\n{words}\n\tindented {words}\n{}\nend", "x".repeat(6000));
        for soft_wrap in [true, false] {
            let paddings = Edges::ZERO;
            let opts = DocOptions { soft_wrap, paddings };
            let mut ed = Editor::document(Rope::from(text.as_str()), opts);
            for offset in (0..=text.len()).step_by(37) {
                ed.set_selected_range(offset..offset);
                ed.prepare(bounds, params, &ts);
                let caret = ed.caret_bounds().unwrap();
                assert!(bounds.contains(caret.center()), "caret {caret:?} off screen at {offset}");
                let (hit, _) = ed.hit_test_point(caret.center());
                assert_eq!(hit, offset, "caret at {offset}, wrap {soft_wrap}");
            }

            // A click between a wheel scroll and the next layout still lands on the text under it.
            ed.set_selected_range(1700..1700);
            ed.prepare(bounds, params, &ts);
            let caret = ed.caret_bounds().unwrap();
            let before = ed.scroll_offset();
            let wheel = if soft_wrap { point(0., 1.) } else { point(-1., 0.) };
            ed.on_scroll_wheel(wheel, false, Modifiers::default());
            let moved = ed.scroll_offset() - before;
            assert!(moved != Point::ZERO, "wheel {wheel:?} didn't scroll");
            assert_eq!(ed.hit_test_point(caret.center() + moved).0, 1700);


            let selection = 15..90;
            ed.set_selected_range(selection.clone());
            ed.prepare(bounds, params, &ts);
            let rects = ed.frame.as_ref().unwrap().selection_rects.clone();
            assert!(!rects.is_empty());
            for rect in rects {
                let (hit, _) = ed.hit_test_point(rect.center());
                assert!(selection.contains(&hit) || hit == selection.end, "box {rect:?} hit {hit}");
            }

            ed.set_selected_range(40..40);
            ed.handle_ime_composition("にほん", 2, &ts);
            ed.prepare(bounds, params, &ts);
            let composing = ed.model().composition().map(|c| c.start..c.end);
            assert_eq!(ed.frame.as_ref().unwrap().marked_range, composing);
            let caret = ed.caret_bounds().unwrap();
            assert_eq!(ed.hit_test_point(caret.center()).0, ed.model().cursor());
        }
    }

    #[test]
    fn editor_direct_typing_undo_redo_and_word_navigation() {
        let ts = TextSystem::new().unwrap();
        let mut ed = Editor::document(
            Rope::new(),
            DocOptions {
                soft_wrap: true,
                paddings: Edges::ZERO,
            },
        );

        ed.handle_text_input("hello", &ts);
        ed.handle_text_input(" world", &ts);
        assert_eq!(ed.model().text().to_string(), "hello world");
        assert!(!ed.model().is_clean());

        assert!(ed.handle_command(EditCommand::Undo, &ts));
        assert_eq!(ed.model().text().to_string(), "");
        assert!(ed.model().is_clean());

        assert!(ed.handle_command(EditCommand::Redo, &ts));
        assert_eq!(ed.model().text().to_string(), "hello world");

        assert!(ed.handle_command(
            EditCommand::Move {
                motion: Motion::PrevWord,
                extend: false,
            },
            &ts,
        ));
        assert_eq!(ed.model().cursor(), 6);
        assert!(ed.handle_command(
            EditCommand::Move {
                motion: Motion::NextWord,
                extend: true,
            },
            &ts,
        ));
        assert_eq!(ed.model().text().slice(ed.model().selection().range()).to_string(), "world");

        let copied = ed.selected_string();
        assert_eq!(copied.as_deref(), Some("world"));
        let cut = ed.prepare_cut();
        assert_eq!(cut.as_deref(), Some("world"));
        ed.delete_selection(&ts);
        assert_eq!(ed.model().text().to_string(), "hello ");
        ed.paste("world", &ts);
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
    fn caret_blinks_at_its_deadlines_pauses_and_stops() {
        let interval = Duration::from_millis(500);
        let mut blink = BlinkState::default();
        blink.start((Some(interval), Some(Duration::from_secs(5))));
        let due = blink.next.expect("blinking schedules the first toggle");
        let shown = blink.visible();
        assert!(!blink.tick(due - Duration::from_millis(1)));
        assert!(blink.tick(due));
        assert_ne!(blink.visible(), shown);
        assert_eq!(blink.next, Some(due + interval));

        blink.pause();
        assert!(blink.visible(), "typing holds the caret visible");
        let resume = blink.next.expect("a pause resumes after an interval");
        assert!(blink.tick(resume));
        assert!(!blink.paused);

        let late = blink.started_at.unwrap() + Duration::from_secs(6);
        blink.next = Some(late);
        assert!(blink.tick(late));
        assert!(
            blink.visible() && blink.next.is_none(),
            "blinking ends visible after the timeout"
        );

        blink.start((Some(interval), None));
        blink.stop();
        assert!(blink.next.is_none() && !blink.tick(Instant::now() + interval));
    }

    #[test]
    fn auto_scroll_repeats_while_dragging_past_the_edge() {
        let text = Rope::from("line\n".repeat(200).as_str());
        let mut ed = Editor::document(
            text,
            DocOptions {
                soft_wrap: false,
                paddings: Edges::ZERO,
            },
        );
        ed.doc
            .as_deref_mut()
            .unwrap()
            .auto_scroll
            .update(Some(30.0));
        let due = ed
            .deadline()
            .expect("dragging past the edge schedules scrolling");
        assert!(!ed.tick(due - Duration::from_millis(1)));
        assert!(ed.tick(due));
        assert_eq!(
            ed.deadline(),
            Some(due + AUTO_SCROLL_INTERVAL),
            "and it repeats"
        );
        ed.doc.as_deref_mut().unwrap().auto_scroll.update(None);
        assert_eq!(ed.deadline(), None, "leaving the edge stops it");
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
        let nodes = sb.render(slot, vp, p, t0, &mut ui);
        assert_eq!(nodes.len(), 1);
        assert!(sb.is_interactive(Axis::Vertical));
        assert_eq!(
            sb.deadline(),
            Some(t0 + IDLE_HOLD),
            "scrolling arms the fade"
        );
        assert!(ui.finish_frame(crate::ui::AppFrame::default()));

        ui.begin_frame(t0 + FADE_IN_DURATION, false);
        let _ = sb.render(slot, vp, p, t0 + FADE_IN_DURATION, &mut ui);
        assert!(!ui.finish_frame(crate::ui::AppFrame::default()));

        assert!(!sb.tick(t0 + IDLE_HOLD - Duration::from_millis(1)));
        assert!(sb.tick(t0 + IDLE_HOLD), "the fade comes due after the hold");
        assert_eq!(sb.deadline(), None);
        let after_idle = t0 + IDLE_HOLD + Duration::from_millis(10);
        ui.begin_frame(after_idle, true);
        let _ = sb.render(slot, vp, p, after_idle, &mut ui);
        assert!(!sb.is_interactive(Axis::Vertical));
    }
}
