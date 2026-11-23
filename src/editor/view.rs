//! Clean-room editor view layout, DirectWrite shaping, caret/selection/IME painting, pointer
//! hit-testing, blink timer, drag auto-scroll, and integrated scrollbar widget.

use std::borrow::Cow;
use std::ops::Range;
use std::rc::Rc;
use std::time::{Duration, Instant};

use ropey::Rope;

use crate::scrollbar::{PreparedAxis, ScrollbarState, ScrollbarViewport};
use crate::theme::palette;
use crate::ui::{
    App, Bounds, ContentMask, Corners, CustomWidget, Edges, ElementId, Font, Half, HitboxBehavior,
    InputHandler, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels,
    Point, Position as CssPosition, ScrollDelta, ScrollWheelEvent, SharedString, Size, Style,
    TextRun, TimerHandle, UTF16Selection, UiNode, UnderlineStyle, Window, fill, point, px,
    relative, rgb, rgba, size,
};

use super::buffer::RopeExt as _;
use super::wrap::{RowLayout, VisualRow};
use super::{Editor, EditorHost, MoveDirection};

const BOTTOM_MARGIN_ROWS: usize = 3;
const CARET_WIDTH: Pixels = px(1.);
pub(super) const RIGHT_MARGIN: Pixels = px(10.);
const DEFAULT_WHEEL_LINES: u32 = 3;
const WHEEL_VIEWPORT_SHARE: f32 = 0.15;
const MIN_WHEEL_STEP: f32 = 48.;

// ---------------------------------------------------------------------------
// Owned caret blink state (zero Entity, drop-cancelling TimerHandle)
// ---------------------------------------------------------------------------

pub(crate) struct BlinkState {
    visible: bool,
    paused: bool,
    epoch: usize,
    timer: TimerHandle,
}

impl Default for BlinkState {
    fn default() -> Self {
        Self::new()
    }
}

impl BlinkState {
    pub(crate) fn new() -> Self {
        Self {
            visible: false,
            paused: false,
            epoch: 0,
            timer: TimerHandle::default(),
        }
    }

    #[inline]
    pub(crate) fn visible(&self) -> bool {
        self.paused || self.visible
    }

    pub(crate) fn stop(&mut self) {
        self.epoch = 0;
        self.paused = false;
        self.visible = false;
        self.timer.cancel();
    }

    fn bump_epoch(&mut self) -> usize {
        self.epoch += 1;
        self.epoch
    }

    pub(crate) fn start(&mut self, host: &EditorHost, cx: &App) {
        self.visible = false;
        let epoch = self.epoch;
        self.step(epoch, host, cx);
    }

    pub(crate) fn pause(&mut self, host: &EditorHost, cx: &App) {
        if self.epoch == 0 {
            return;
        }
        self.paused = true;
        self.visible = true;
        let epoch = self.bump_epoch();
        let Some(delay) = crate::win32::caret_blink_interval() else {
            self.timer.cancel();
            return;
        };
        let host = host.clone();
        self.timer = cx.after(delay, move |cx| {
            let h = host.clone();
            host.with_mut(cx, |ed, cx| {
                if ed.blink.epoch != epoch {
                    return;
                }
                ed.blink.paused = false;
                ed.blink.step(epoch, &h, cx);
            });
            host.notify(cx);
        });
    }

    fn step(&mut self, epoch: usize, host: &EditorHost, cx: &App) {
        if epoch != self.epoch {
            return;
        }
        if self.paused {
            self.visible = true;
            return;
        }
        let Some(interval) = crate::win32::caret_blink_interval() else {
            self.visible = true;
            self.timer.cancel();
            return;
        };
        self.visible = !self.visible;
        let next_epoch = self.bump_epoch();
        let host = host.clone();
        self.timer = cx.after(interval, move |cx| {
            let h = host.clone();
            host.with_mut(cx, |ed, cx| {
                ed.blink.step(next_epoch, &h, cx);
            });
            host.notify(cx);
        });
    }
}

// ---------------------------------------------------------------------------
// Owned drag auto-scroll state (zero Entity, drop-cancelling TimerHandle)
// ---------------------------------------------------------------------------

#[derive(Default)]
pub(crate) struct AutoScroll {
    delta: Option<Pixels>,
    timer: TimerHandle,
    pub(crate) last_drag_pos: Option<Point<Pixels>>,
}

impl AutoScroll {
    pub(crate) fn compute_delta(y: Pixels, bounds: Bounds<Pixels>) -> Option<Pixels> {
        const MIN_SPEED: f32 = 12.0;
        const MAX_SPEED: f32 = 64.0;
        const INNER_ZONE: f32 = 16.0;
        const OUTER_RAMP: f32 = 80.0;

        let bottom = bounds.bottom() - px(INNER_ZONE);
        let top = bounds.top() + px(INNER_ZONE);

        if y > bottom {
            let t = ((y - bottom) / px(INNER_ZONE + OUTER_RAMP)).min(1.0);
            Some(px(MIN_SPEED + t * (MAX_SPEED - MIN_SPEED)))
        } else if y < top {
            let t = ((top - y) / px(INNER_ZONE + OUTER_RAMP)).min(1.0);
            Some(px(-(MIN_SPEED + t * (MAX_SPEED - MIN_SPEED))))
        } else {
            None
        }
    }

    #[inline]
    pub(crate) fn is_active(&self) -> bool {
        self.delta.is_some()
    }

    pub(crate) fn stop(&mut self) {
        self.delta = None;
        self.timer.cancel();
        self.last_drag_pos = None;
    }

    pub(crate) fn update(&mut self, delta: Option<Pixels>, host: &EditorHost, cx: &App) {
        let was_active = self.delta.is_some();
        self.delta = delta;
        if delta.is_none() {
            self.timer.cancel();
            return;
        }
        if !was_active {
            let host = host.clone();
            let interval = Duration::from_millis(16);
            self.timer = cx.every(interval, interval, move |cx| {
                let mut keep_running = false;
                host.with_mut(cx, |ed, _| {
                    let Some(delta) = ed.auto_scroll.delta else {
                        return;
                    };
                    keep_running = true;
                    let next = point(ed.scroll_offset.x, ed.scroll_offset.y + delta);
                    ed.clamp_and_set_scroll(next);
                    if let Some(pos) = ed.auto_scroll.last_drag_pos {
                        let (offset, aff) = ed.hit_test_point(pos);
                        ed.select_to_with_affinity(offset, aff);
                    }
                });
                if keep_running {
                    host.notify(cx);
                }
                keep_running
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Layout snapshots & scroll helpers
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub(super) struct LongestLineCache {
    revision: u64,
    row: usize,
    len: usize,
    font: Font,
    font_size: Pixels,
    wrap_width: Option<Pixels>,
}

#[derive(Clone)]
pub(super) struct ViewLayout {
    pub(super) visible_lines: Vec<usize>,
    pub(super) visible_byte_offsets: Vec<usize>,
    pub(super) visible_top: Pixels,
    pub(super) visible_byte_range: Range<usize>,
    pub(super) rows: Rc<Vec<RowLayout>>,
    pub(super) line_height: Pixels,
    pub(super) wrap_width: Option<Pixels>,
    pub(super) space_width: Pixels,
    pub(super) caret_bounds: Option<Bounds<Pixels>>,
}

impl ViewLayout {
    pub(super) fn row_layout(&self, buffer_row: usize) -> Option<&RowLayout> {
        let ix = self.visible_lines.binary_search(&buffer_row).ok()?;
        self.rows.get(ix)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ScrollbarSnapshot {
    pub(super) viewport: ScrollbarViewport,
    pub(super) synced_offset: Point<Pixels>,
}

impl ScrollbarSnapshot {
    fn compute(
        input_bounds: Bounds<Pixels>,
        content_size: Size<Pixels>,
        synced_offset: Point<Pixels>,
        paddings: Edges<Pixels>,
        soft_wrap: bool,
    ) -> Self {
        Self {
            viewport: ScrollbarViewport {
                bounds: Bounds::new(
                    point(input_bounds.origin.x, input_bounds.origin.y - paddings.top),
                    size(
                        input_bounds.size.width + paddings.right,
                        input_bounds.size.height + paddings.top + paddings.bottom,
                    ),
                ),
                content_size: size(
                    content_size.width + paddings.right + RIGHT_MARGIN,
                    content_size.height,
                ),
                horizontal: !soft_wrap,
            },
            synced_offset,
        }
    }
}

pub(super) fn clamp_horizontal_scroll(
    offset_x: Pixels,
    content_width: Pixels,
    viewport_width: Pixels,
) -> Pixels {
    let min_x = (viewport_width - content_width).min(px(0.));
    offset_x.clamp(min_x, px(0.))
}

fn caret_vertical_margin(viewport_height: Pixels, line_height: Pixels) -> Pixels {
    let rows = (viewport_height / line_height) as usize;
    let raw = if rows < BOTTOM_MARGIN_ROWS * 8 {
        line_height
    } else {
        BOTTOM_MARGIN_ROWS * line_height
    };
    raw.min((rows as f32 * line_height).half())
}

impl Editor {
    pub(super) fn position_for_offset(&self, offset: usize) -> Option<Point<Pixels>> {
        let layout = self.layout.as_ref()?;
        let mut y = layout.visible_top;
        for (vi, row_layout) in layout.rows.iter().enumerate() {
            let base = layout.visible_byte_offsets[vi];
            let local = offset.saturating_sub(base);
            if let Some(pos) = row_layout.position_for_index(local, layout.line_height, false) {
                return Some(point(pos.x, pos.y + y));
            }
            y += row_layout.size(layout.line_height).height;
        }
        None
    }

    pub(super) fn clamp_and_set_scroll(&mut self, mut offset: Point<Pixels>) -> bool {
        let min_y = (-self.content_size.height + self.input_bounds.size.height).min(px(0.));
        offset.y = if self.multi_line {
            offset.y.clamp(min_y, px(0.))
        } else {
            px(0.)
        };
        offset.x = clamp_horizontal_scroll(
            offset.x,
            self.content_size.width,
            self.input_bounds.size.width,
        );
        if self.scroll_offset != offset {
            self.scroll_offset = offset;
            true
        } else {
            false
        }
    }

    pub(super) fn scroll_to(&mut self, offset: usize, direction: Option<MoveDirection>) {
        let (Some(layout), Some(bounds)) = (self.layout.as_ref(), self.content_bounds.as_ref())
        else {
            return;
        };
        let mut next = self.scroll_offset;
        let prev = next;
        let lh = layout.line_height;

        let pt = self.text.offset_to_point(offset);
        let disp = self.wrap.point_to_display(pt);
        let row_y = lh * disp.row;

        if let Ok(vi) = layout.visible_lines.binary_search(&pt.row) {
            let local = offset.saturating_sub(layout.visible_byte_offsets[vi]);
            if let Some(pos) = layout.rows[vi].position_for_index(local, lh, false) {
                let w = bounds.size.width;
                if pos.x - RIGHT_MARGIN < -next.x {
                    next.x = -pos.x + RIGHT_MARGIN;
                } else if pos.x + RIGHT_MARGIN > -next.x + w {
                    next.x = -(pos.x - w + RIGHT_MARGIN);
                }
            }
        }

        if row_y < -next.y {
            next.y = -row_y;
        } else if row_y + lh > -next.y + bounds.size.height {
            next.y = -(row_y - bounds.size.height + lh);
        }

        match direction {
            Some(MoveDirection::Up) => next.y = next.y.max(prev.y),
            Some(MoveDirection::Down) => next.y = next.y.min(prev.y),
            None => {}
        }

        let min_y = (-self.content_size.height + self.input_bounds.size.height).min(px(0.));
        next.x = next.x.min(px(0.));
        next.y = next.y.clamp(min_y, px(0.));
        self.pending_scroll = Some(next);
    }

    pub(super) fn hit_test_point(&self, pos: Point<Pixels>) -> (usize, bool) {
        if self.text.len() == 0 {
            return (0, false);
        }
        let (Some(bounds), Some(layout)) = (self.content_bounds.as_ref(), self.layout.as_ref())
        else {
            return (0, false);
        };
        let lh = layout.line_height;
        let rel = pos - bounds.origin;
        let mut y = layout.visible_top;

        for (vi, row_layout) in layout.rows.iter().enumerate() {
            let base = layout.visible_byte_offsets[vi];
            let local_pos = rel - point(px(0.), y);

            if !self.multi_line {
                let col = row_layout.closest_index_for_x(local_pos.x);
                return ((base + col).min(self.text.len()), false);
            }
            if let Some((col, aff)) = row_layout.closest_index_for_position(local_pos, lh) {
                return ((base + col).min(self.text.len()), aff);
            } else if local_pos.y < px(0.) {
                return (base.min(self.text.len()), false);
            }
            y += row_layout.size(lh).height;
        }

        let end = layout
            .rows
            .last()
            .zip(layout.visible_byte_offsets.last())
            .map(|(row, base)| (base + row.byte_len()).min(self.text.len()))
            .unwrap_or(self.text.len());
        (end, false)
    }

    pub(super) fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        host: &EditorHost,
        window: &Window,
        cx: &mut App,
    ) {
        if window.default_prevented() {
            return;
        }
        self.history.break_coalescing();
        if self.ime_marked.is_some_and(|m| m.is_empty()) {
            self.ime_marked = None;
        }
        self.selecting = true;
        let (offset, aff) = self.hit_test_point(event.position);

        if event.button == MouseButton::Left && event.click_count >= 3 {
            self.select_line_at(offset);
            return;
        }
        if event.button == MouseButton::Left && event.click_count == 2 {
            self.select_word_at(offset);
            return;
        }
        if event.button == MouseButton::Right {
            if !self.selection.contains(offset) {
                self.move_to(offset, None, Some(host), cx);
            }
            self.pending_context_menu = Some((event.position, offset));
            return;
        }
        if event.modifiers.shift {
            self.select_to_with_affinity(offset, aff);
        } else {
            self.move_to_with_affinity(offset, None, aff, Some(host), cx);
        }
    }

    pub(super) fn on_mouse_up(
        &mut self,
        event: &MouseUpEvent,
        host: &EditorHost,
        window: &mut Window,
        cx: &mut App,
    ) {
        if event.button == MouseButton::Right
            && let Some((pos, offset)) = self.pending_context_menu.take()
            && self.multi_line
        {
            if !self.selection.contains(offset) {
                self.move_to(offset, None, Some(host), cx);
            }
            let h = host.clone();
            window.defer(cx, move |window, cx| {
                h.open_context_menu(pos, window, cx);
            });
        }
        if self.selection.is_empty() {
            self.selection.reversed = false;
        }
        self.selecting = false;
        self.word_anchor = None;
        self.auto_scroll.stop();
    }

    pub(super) fn on_drag_move(
        &mut self,
        event: &MouseMoveEvent,
        host: &EditorHost,
        window: &Window,
        cx: &mut App,
    ) {
        if self.text.len() == 0
            || self.layout.is_none()
            || !self.focus_handle.is_focused(window)
            || !self.selecting
        {
            return;
        }
        self.auto_scroll.last_drag_pos = Some(event.position);
        let (offset, aff) = self.hit_test_point(event.position);
        self.select_to_with_affinity(offset, aff);

        if self.multi_line {
            let delta = AutoScroll::compute_delta(event.position.y, self.input_bounds);
            self.auto_scroll.update(delta.map(|d| -d), host, cx);
        }
    }

    pub(super) fn on_scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        window: &Window,
        cx: &mut App,
    ) -> bool {
        if event.modifiers.control {
            return false;
        }
        let lh = self
            .layout
            .as_ref()
            .map_or_else(|| window.line_height(), |l| l.line_height);
        let delta = match event.delta {
            ScrollDelta::Lines(lines) if self.viewport_wheel_step => {
                let h_chars = crate::win32::wheel_scroll_amount(true)
                    .unwrap_or(DEFAULT_WHEEL_LINES)
                    .max(1) as f32;
                let v_lines = crate::win32::wheel_scroll_amount(false)
                    .unwrap_or(DEFAULT_WHEEL_LINES)
                    .max(1) as f32;
                let vp = self.input_bounds.size;
                let step = |notches: f32, extent: Pixels| {
                    let ext = extent.as_f32();
                    let notch = (ext * WHEEL_VIEWPORT_SHARE).round().max(MIN_WHEEL_STEP);
                    px((notches * notch).round().clamp(-ext.floor(), ext.floor()))
                };
                point(
                    step(lines.x / h_chars, vp.width),
                    step(lines.y / v_lines, vp.height),
                )
            }
            _ => event.delta.pixel_delta(lh),
        };

        let moved = self.clamp_and_set_scroll(self.scroll_offset + delta);
        if moved {
            self.scrollbar.on_wheel(self.scroll_offset, Instant::now());
            cx.stop_propagation();
        }
        moved
    }

    fn visible_slice(
        &self,
        line_height: Pixels,
        viewport_height: Pixels,
    ) -> (Range<usize>, Vec<usize>, Pixels, Range<usize>) {
        if !self.multi_line {
            return (0..1, vec![0], px(0.), 0..1);
        }
        let total_rows = self.wrap.visual_row_count();
        let total_lines = self.wrap.line_count();
        if total_rows == 0 || total_lines == 0 {
            return (0..0, Vec::new(), px(0.), 0..0);
        }
        let scroll_y = self
            .pending_scroll
            .map_or(self.scroll_offset.y, |p| p.y);
        let top = (-scroll_y).max(px(0.));
        let bottom = top + viewport_height;
        let lh = f32::from(line_height);
        let first_row = ((f32::from(top) / lh).floor() as usize).min(total_rows - 1);
        let last_row = ((f32::from(bottom) / lh).ceil() as usize).min(total_rows - 1);
        let shape_rows = first_row.saturating_sub(2)..(last_row + 3).min(total_rows);

        let start_line = self.wrap.visual_row_to_line(first_row);
        let end_line = self.wrap.visual_row_to_line(last_row);
        let visible_top = line_height * self.wrap.first_visual_row(start_line);
        let line_range = start_line..(end_line + 2).min(total_lines);
        let lines: Vec<usize> = line_range.clone().collect();
        (line_range, lines, visible_top, shape_rows)
    }

    fn shape_visible_lines(
        &self,
        display_text: &Rope,
        layout: &ViewLayout,
        shape_rows: &Range<usize>,
        font_size: Pixels,
        runs: &[TextRun],
        window: &mut Window,
    ) -> Vec<RowLayout> {
        if !self.multi_line {
            let text: SharedString = display_text.to_string().into();
            let aligned = align_runs(&text, runs);
            let line_runs = aligned.as_deref().unwrap_or(runs);
            let shaped = window.text_system().shape_line(text, font_size, line_runs);
            return vec![RowLayout::from_shaped(vec![shaped])];
        }

        if self.text.len() == 0 {
            let text = display_text.to_string();
            let mut shaped = Vec::new();
            let mut off = 0;
            for part in text.split('\n') {
                let line_runs = slice_runs(runs, off, &(0..part.len()));
                shaped.push(
                    window
                        .text_system()
                        .shape_line(part.to_string().into(), font_size, &line_runs),
                );
                off += part.len() + 1;
            }
            return vec![RowLayout::from_shaped(shaped)];
        }

        let mut result = Vec::with_capacity(layout.visible_lines.len());
        let mut run_offset = 0;
        let mut visual_row = layout
            .visible_lines
            .first()
            .map_or(0, |&line| self.wrap.first_visual_row(line));
        let caret = self.selection.head();

        for (vi, &buf_line) in layout.visible_lines.iter().enumerate() {
            let line_slice = display_text.slice_line(buf_line);
            let line_len = line_slice.len();
            let line_byte_start = layout.visible_byte_offsets[vi];
            let wrapped = self.wrap.line(buf_line).expect("wrapped line exists");

            let mut rows = Vec::with_capacity(wrapped.ranges.len());
            for (sub_ix, range) in wrapped.ranges.iter().enumerate() {
                let sub_start = line_byte_start + range.start;
                let sub_end = line_byte_start + range.end;
                let has_caret = caret >= sub_start && caret <= sub_end;
                let must_shape = shape_rows.contains(&visual_row)
                    || has_caret
                    || (sub_ix == 0 && wrapped.indent_spaces > 0);
                if must_shape {
                    let sub_runs = slice_runs(runs, run_offset, range);
                    let sub_text: SharedString = line_slice.slice(range.clone()).to_string().into();
                    let sub_runs = align_runs(&sub_text, &sub_runs).unwrap_or(sub_runs);
                    let shaped = window
                        .text_system()
                        .shape_line(sub_text, font_size, &sub_runs);
                    rows.push(VisualRow::from(shaped));
                } else {
                    rows.push(VisualRow::Placeholder(range.len()));
                }
                visual_row += 1;
            }

            let hanging_indent = if wrapped.indent_spaces > 0 && rows.len() > 1 {
                let indent_bytes = line_slice
                    .chars()
                    .take(wrapped.indent_spaces as usize)
                    .map(char::len_utf8)
                    .sum();
                rows[0].x_for_index(indent_bytes)
            } else {
                px(0.)
            };
            result.push(RowLayout::from_rows(rows, hanging_indent));
            run_offset += line_len + 1;
        }
        result
    }

    fn layout_caret_and_scroll(
        &self,
        layout: &ViewLayout,
        bounds: &mut Bounds<Pixels>,
        content_size: Size<Pixels>,
    ) -> (Bounds<Pixels>, Point<Pixels>) {
        let lh = layout.line_height;
        let mut scroll = self.scroll_offset;
        let v_margin = caret_vertical_margin(bounds.size.height, lh);

        let pos_for = |row: usize, offset: usize, aff: bool| -> Point<Pixels> {
            let origin = point(px(0.), lh * self.wrap.first_visual_row(row));
            if let Ok(vi) = layout.visible_lines.binary_search(&row) {
                let local = offset.saturating_sub(layout.visible_byte_offsets[vi]);
                if let Some(p) = layout.rows[vi].position_for_index(local, lh, aff) {
                    return origin + p;
                }
            }
            origin
        };

        let mut sel = self.selection;
        let mut head = self.selection.head();
        if let Some(marked) = self.ime_marked {
            sel = (marked.end..marked.end).into();
            head = marked.end;
        }
        let all_selected = sel.len() == self.text.len();

        let head_pt = pos_for(
            self.text.offset_to_point(head).row,
            head,
            self.line_end_affinity,
        );
        let start_pt = pos_for(self.text.offset_to_point(sel.start).row, sel.start, false);
        let end_pt = pos_for(self.text.offset_to_point(sel.end).row, sel.end, false);

        let sel_changed = self.last_selection != Some(sel);
        let auto_scrolling = self.auto_scroll.is_active();
        if sel_changed && !all_selected {
            if scroll.x + head_pt.x > bounds.size.width - RIGHT_MARGIN {
                scroll.x = bounds.size.width - RIGHT_MARGIN - head_pt.x;
            } else if scroll.x + head_pt.x < px(0.) {
                scroll.x = -head_pt.x;
            }

            if !auto_scrolling {
                if scroll.y + head_pt.y > bounds.size.height - v_margin {
                    scroll.y = bounds.size.height - v_margin - head_pt.y;
                } else if scroll.y + head_pt.y < v_margin {
                    scroll.y = (v_margin - head_pt.y).min(px(0.));
                }
            }

            if self.selection.reversed {
                if scroll.x + start_pt.x < px(0.) {
                    scroll.x = -start_pt.x;
                }
                if !auto_scrolling && scroll.y + start_pt.y < px(0.) {
                    scroll.y = -start_pt.y;
                }
            } else {
                if scroll.x + end_pt.x <= px(0.) {
                    scroll.x = -end_pt.x;
                }
                if !auto_scrolling && scroll.y + end_pt.y <= px(0.) {
                    scroll.y = -end_pt.y;
                }
            }
        }

        if let Some(pending) = self.pending_scroll {
            scroll = pending;
        }
        scroll.x = clamp_horizontal_scroll(scroll.x, content_size.width, bounds.size.width);

        let caret_h = 0.85 * lh;
        let caret_bounds = Bounds::new(
            point(
                bounds.left() + head_pt.x + scroll.x,
                bounds.top() + head_pt.y + ((lh - caret_h) / 2.),
            ),
            size(CARET_WIDTH, caret_h),
        );
        bounds.origin += scroll;
        (caret_bounds, scroll)
    }

    fn selection_rects(
        &self,
        layout: &ViewLayout,
        bounds: &Bounds<Pixels>,
        window: &Window,
        cx: &App,
    ) -> Vec<Bounds<Pixels>> {
        if !self.has_selection_focus(window, cx) {
            return Vec::new();
        }
        let mut sel = self.selection;
        if let Some(marked) = self.ime_marked
            && !marked.is_empty()
        {
            sel = (marked.end..marked.end).into();
        }
        if sel.is_empty() {
            return Vec::new();
        }
        let (lo, hi) = (sel.start.min(sel.end), sel.start.max(sel.end));
        let vis = lo.max(layout.visible_byte_range.start)..hi.min(layout.visible_byte_range.end);
        if vis.is_empty() {
            return Vec::new();
        }
        let Some(corners) = range_corners(&vis, layout) else {
            return Vec::new();
        };
        let origin = bounds.origin;
        corners
            .into_iter()
            .map(|c| Bounds::from_corners(origin + c.top_left, origin + c.bottom_right))
            .collect()
    }
}

fn range_corners(
    range: &Range<usize>,
    layout: &ViewLayout,
) -> Option<Vec<Corners<Point<Pixels>>>> {
    let range = range.start.max(layout.visible_byte_range.start)
        ..range.end.min(layout.visible_byte_range.end);
    if range.is_empty() {
        return None;
    }
    let mut y = layout.visible_top;
    let mut out = Vec::new();
    for (&line_base, row_layout) in layout.visible_byte_offsets.iter().zip(layout.rows.iter()) {
        let mut off = line_base;
        for (sub_ix, row) in row_layout.rows.iter().enumerate() {
            let end = off + row.byte_len();
            if !row.is_shaped() {
                off = end;
                y += layout.line_height;
                continue;
            }
            let s = range.start.max(off);
            let e = range.end.min(end);
            let is_last = sub_ix + 1 == row_layout.rows.len();
            let includes_nl = is_last && range.start <= end && range.end > end;
            if s < e || includes_nl {
                let indent = row_layout.indent_for(sub_ix);
                let left = indent + row.x_for_index(s.min(end) - off);
                let right = indent
                    + if includes_nl {
                        row.width() + layout.space_width
                    } else {
                        row.x_for_index(e - off)
                    };
                out.push(Corners {
                    top_left: point(left, y),
                    top_right: point(right, y),
                    bottom_left: point(left, y + layout.line_height),
                    bottom_right: point(right, y + layout.line_height),
                });
            }
            off = end;
            y += layout.line_height;
        }
    }
    (!out.is_empty()).then_some(out)
}

fn slice_runs(runs: &[TextRun], line_offset: usize, range: &Range<usize>) -> Vec<TextRun> {
    let mut out = Vec::new();
    let target = (line_offset + range.start)..(line_offset + range.end);
    let mut cursor = 0;
    for run in runs {
        let start = cursor;
        let end = cursor + run.len;
        if end <= target.start {
            cursor = end;
            continue;
        }
        if start >= target.end {
            break;
        }
        let s = target.start.max(start) - start;
        let e = target.end.min(end) - start;
        if e > s {
            out.push(TextRun {
                len: e - s,
                ..run.clone()
            });
        }
        cursor = end;
    }
    out
}

fn align_runs(text: &str, runs: &[TextRun]) -> Option<Vec<TextRun>> {
    let mut pos = 0;
    if runs.iter().all(|r| {
        pos += r.len;
        pos <= text.len() && text.is_char_boundary(pos)
    }) {
        return None;
    }
    let mut out = Vec::with_capacity(runs.len());
    let mut cursor = 0;
    let mut raw = 0;
    for run in runs {
        raw = (raw + run.len).min(text.len());
        let mut end = raw;
        while !text.is_char_boundary(end) {
            end += 1;
        }
        if end > cursor {
            out.push(TextRun {
                len: end - cursor,
                ..run.clone()
            });
            cursor = end;
        }
    }
    Some(out)
}

fn split_ime_underline(
    run: TextRun,
    total_range: Range<usize>,
    marked: Option<Range<usize>>,
    underline: Option<UnderlineStyle>,
) -> Vec<TextRun> {
    if run.len == 0 {
        return Vec::new();
    }
    let Some(marked) = marked else {
        return vec![run];
    };
    let s = total_range.start.max(marked.start);
    let e = total_range.end.min(marked.end);
    if s >= e {
        return vec![run];
    }
    [
        TextRun {
            len: s - total_range.start,
            ..run.clone()
        },
        TextRun {
            len: e - s,
            underline,
            ..run.clone()
        },
        TextRun {
            len: total_range.end - e,
            ..run
        },
    ]
    .into_iter()
    .filter(|r| r.len > 0)
    .collect()
}

// ---------------------------------------------------------------------------
// IME InputHandler bridging Window -> EditorHost -> Editor
// ---------------------------------------------------------------------------

struct EditorInputHandler {
    host: EditorHost,
    bounds: Bounds<Pixels>,
}

impl InputHandler for EditorInputHandler {
    fn selected_text_range(&mut self, _: &mut Window, cx: &mut App) -> Option<UTF16Selection> {
        self.host.with(cx, Editor::selected_utf16_range)
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut App,
    ) {
        let h = self.host.clone();
        self.host.with_mut(cx, |ed, cx| {
            ed.replace_text_in_range(range_utf16, new_text, Some(&h), cx);
        });
        self.host.notify(cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut App,
    ) {
        self.host.with_mut(cx, |ed, cx| {
            ed.replace_and_mark_text_in_range(
                range_utf16,
                new_text,
                new_selected_range_utf16,
                cx,
            );
        });
        self.host.notify(cx);
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Bounds<Pixels>> {
        let bounds = self.bounds;
        self.host
            .with(cx, |ed| ed.bounds_for_utf16_range(range_utf16, bounds, window))
            .flatten()
    }
}

// ---------------------------------------------------------------------------
// CustomWidget: EditorTextWidget
// ---------------------------------------------------------------------------

struct TextPrepaint {
    layout: ViewLayout,
    content_size: Size<Pixels>,
    caret_rect: Bounds<Pixels>,
    synced_scroll: Point<Pixels>,
    selection_rects: Vec<Bounds<Pixels>>,
    scrolled_bounds: Bounds<Pixels>,
}

pub(super) struct EditorTextWidget {
    host: EditorHost,
    placeholder: SharedString,
    prepaint: Option<TextPrepaint>,
}

impl EditorTextWidget {
    pub(super) fn new(host: EditorHost, placeholder: SharedString) -> Self {
        Self {
            host,
            placeholder,
            prepaint: None,
        }
    }
}

impl From<EditorTextWidget> for UiNode {
    fn from(w: EditorTextWidget) -> Self {
        UiNode::widget(w)
    }
}

impl CustomWidget for EditorTextWidget {
    fn request_layout(&mut self, window: &mut Window, cx: &mut App) -> LayoutId {
        let multi_line = self.host.with(cx, |ed| ed.multi_line).unwrap_or(false);
        let lh = window.line_height();
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        if multi_line {
            style.flex_grow = 1.0;
            style.size.height = relative(1.).into();
            style.min_size.height = (2 * lh).into();
        } else {
            style.size.height = lh.into();
        }
        window.request_layout(style, [], cx)
    }

    fn prepaint(&mut self, input_bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let style = window.text_style();
        let font = style.font();
        let font_size = style.font_size;
        let line_height = window.line_height();
        let space_width = window.text_system().space_width(&font, font_size);

        let wrap_width = self
            .host
            .with(cx, |ed| {
                (ed.multi_line && ed.soft_wrap).then_some(input_bounds.size.width - RIGHT_MARGIN)
            })
            .flatten();

        let font_for_wrap = font.clone();
        self.host.with_mut(cx, |ed, cx| {
            ed.wrap
                .sync_layout(&ed.text, font_for_wrap, font_size, wrap_width, cx);
        });

        let p = palette(cx);
        let placeholder_rope = Rope::from(self.placeholder.as_str());

        let prep = self.host.with(cx, |ed| {
            let is_empty = ed.text.len() == 0;
            let (display_text, color) = if is_empty {
                (&placeholder_rope, rgba(p.text_secondary))
            } else {
                (&ed.text, style.color)
            };

            let (vis_range, visible_lines, visible_top, shape_rows) =
                ed.visible_slice(line_height, input_bounds.size.height);
            let start_off = ed.text.line_start_offset(vis_range.start);
            let end_off = ed.text.line_end_offset(vis_range.end.saturating_sub(1));
            let visible_byte_offsets: Vec<usize> = visible_lines
                .iter()
                .map(|&l| ed.text.line_start_offset(l))
                .collect();

            let mut layout = ViewLayout {
                visible_lines,
                visible_byte_offsets,
                visible_top,
                visible_byte_range: start_off..end_off,
                rows: Rc::new(Vec::new()),
                line_height,
                wrap_width,
                space_width,
                caret_bounds: None,
            };

            let base_run = TextRun {
                len: display_text.len(),
                font: font.clone(),
                color,
                underline: None,
            };
            let ime_underline = Some(UnderlineStyle {
                thickness: px(1.),
                color: Some(color),
            });
            let marked = ed.ime_marked.map(|m| m.start..m.end);
            let runs = split_ime_underline(base_run, 0..display_text.len(), marked, ime_underline);
            let rows =
                ed.shape_visible_lines(display_text, &layout, &shape_rows, font_size, &runs, window);
            layout.rows = Rc::new(rows);

            let mut new_longest = None;
            let mut max_line_w = wrap_width.unwrap_or(px(0.));
            if !ed.multi_line || !ed.soft_wrap {
                let longest_row = ed.wrap.longest_line();
                let slice = ed.text.slice_line(longest_row);
                let key = LongestLineCache {
                    revision: ed.revision,
                    row: longest_row,
                    len: slice.len(),
                    font: font.clone(),
                    font_size,
                    wrap_width,
                };
                max_line_w = match &ed.longest_line_cache {
                    Some((cached, w)) if *cached == key => *w,
                    _ => {
                        let s: Cow<str> = slice.into();
                        let w = window
                            .text_system()
                            .measure_line_width(&s, &key.font, font_size);
                        new_longest = Some((key, w));
                        w
                    }
                };
            }

            let total_rows = ed.wrap.visual_row_count();
            let content_size = size(
                if max_line_w + RIGHT_MARGIN > input_bounds.size.width {
                    max_line_w + RIGHT_MARGIN
                } else {
                    max_line_w
                },
                (total_rows as f32 * line_height).max(input_bounds.size.height),
            );

            let mut scrolled_bounds = input_bounds;
            let (caret_bounds, synced_scroll) =
                ed.layout_caret_and_scroll(&layout, &mut scrolled_bounds, content_size);
            layout.caret_bounds = Some(caret_bounds);

            let safe_y_min = (-content_size.height + input_bounds.size.height).min(px(0.));
            let clamped_y = if ed.multi_line {
                synced_scroll.y.clamp(safe_y_min, px(0.))
            } else {
                px(0.)
            };
            let caret_h = if ed.caret_height > px(0.) {
                ed.caret_height
            } else {
                caret_bounds.size.height
            };
            let caret_origin = window.pixel_snap_point(point(
                caret_bounds.origin.x,
                caret_bounds.origin.y + clamped_y,
            ));
            let caret_rect = Bounds::new(caret_origin, size(CARET_WIDTH, caret_h));
            let selection_rects = ed.selection_rects(&layout, &scrolled_bounds, window, cx);
            let sb_snap = ScrollbarSnapshot::compute(
                input_bounds,
                content_size,
                synced_scroll,
                ed.paddings,
                ed.soft_wrap,
            );

            (
                TextPrepaint {
                    layout,
                    content_size,
                    caret_rect,
                    synced_scroll,
                    selection_rects,
                    scrolled_bounds,
                },
                new_longest,
                sb_snap,
            )
        });

        let Some((tp, new_longest, sb_snap)) = prep else {
            return;
        };

        self.host.with_mut(cx, |ed, _| {
            if let Some(entry) = new_longest {
                ed.longest_line_cache = Some(entry);
            }
            ed.caret_bounds = Some(tp.caret_rect);
            ed.scrollbar_snapshot = Some(sb_snap);
        });

        let _ = window.insert_hitbox(input_bounds, HitboxBehavior::Normal);
        self.prepaint = Some(tp);
    }

    fn paint(&mut self, input_bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let Some(tp) = self.prepaint.as_mut() else {
            return;
        };
        let Some((focus_handle, show_caret, selection, caret_hidden)) = self.host.with(cx, |ed| {
            (
                ed.focus_handle.clone(),
                ed.show_caret(window),
                ed.selection,
                ed.caret_hidden,
            )
        }) else {
            return;
        };

        let bounds = tp.scrolled_bounds;
        window.handle_input(
            &focus_handle,
            EditorInputHandler {
                host: self.host.clone(),
                bounds,
            },
        );

        let p = palette(cx);
        let lh = window.line_height();
        let origin = bounds.origin;

        if window.is_window_active() {
            let sel_color = p.selection();
            for rect in tp.selection_rects.drain(..) {
                window.paint_quad(fill(rect, sel_color));
            }
        }

        let mut y = tp.layout.visible_top;
        for row_layout in tp.layout.rows.iter() {
            row_layout.paint(point(origin.x, origin.y + y), lh, window, cx);
            y += row_layout.size(lh).height;
        }

        if show_caret && !caret_hidden && selection.is_empty() {
            let caret = tp.caret_rect;
            window.with_content_mask(
                Some(ContentMask {
                    bounds: input_bounds,
                }),
                |window| window.paint_quad(fill(caret, rgb(p.caret))),
            );
        }

        let mut needs_notify = false;
        self.host.with_mut(cx, |ed, cx| {
            let geom_changed = ed.content_bounds != Some(bounds)
                || ed.input_bounds != input_bounds
                || ed.content_size != tp.content_size
                || ed.layout.as_ref().is_none_or(|prev| {
                    prev.caret_bounds != tp.layout.caret_bounds
                        || prev.line_height != tp.layout.line_height
                });
            let width_changed = ed.input_bounds.size.width != input_bounds.size.width;
            ed.layout = Some(tp.layout.clone());
            ed.content_bounds = Some(bounds);
            ed.last_cursor = Some(ed.cursor());
            ed.input_bounds = input_bounds;
            if width_changed && let Some(layout) = ed.layout.as_ref() {
                let w = ed.soft_wrap.then_some(layout.wrap_width).flatten();
                ed.wrap.set_wrap_width(w, cx);
                needs_notify = true;
            }
            ed.last_selection = Some(selection);
            ed.content_size = tp.content_size;
            needs_notify |= ed.clamp_and_set_scroll(tp.synced_scroll);
            ed.pending_scroll = None;
            needs_notify |= geom_changed;
        });
        if needs_notify {
            self.host.notify(cx);
        }

        // Drag-select & mouse-up release listeners
        let host_move = self.host.clone();
        window.on_mouse_event(move |event: &MouseMoveEvent, _, window, cx| {
            if event.pressed_button == Some(MouseButton::Left) {
                let h = host_move.clone();
                let mut changed = false;
                host_move.with_mut(cx, |ed, cx| {
                    if ed.selecting {
                        ed.on_drag_move(event, &h, window, cx);
                        changed = true;
                    }
                });
                if changed {
                    host_move.notify(cx);
                }
            }
        });

        let host_up = self.host.clone();
        window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
            if !phase.bubble() {
                return;
            }
            host_up.with_mut(cx, |ed, _| {
                ed.auto_scroll.stop();
                ed.selecting = false;
            });
        });
    }
}

// ---------------------------------------------------------------------------
// CustomWidget: EditorScrollbarWidget (direct bridge to Editor.scrollbar)
// ---------------------------------------------------------------------------

struct ScrollbarPrepaint {
    viewport_bounds: Bounds<Pixels>,
    axes: Vec<PreparedAxis>,
}

pub(super) struct EditorScrollbarWidget {
    host: EditorHost,
    prepaint: Option<ScrollbarPrepaint>,
}

impl EditorScrollbarWidget {
    pub(super) fn new(host: EditorHost) -> Self {
        Self {
            host,
            prepaint: None,
        }
    }
}

impl From<EditorScrollbarWidget> for UiNode {
    fn from(w: EditorScrollbarWidget) -> Self {
        UiNode::widget(w)
    }
}

impl CustomWidget for EditorScrollbarWidget {
    fn id(&self) -> Option<ElementId> {
        Some("editor-scrollbar".into())
    }

    fn request_layout(&mut self, window: &mut Window, cx: &mut App) -> LayoutId {
        let style = Style {
            position: CssPosition::Absolute,
            size: Size {
                width: relative(1.).into(),
                height: relative(1.).into(),
            },
            ..Style::default()
        };
        window.request_layout(style, [], cx)
    }

    fn prepaint(&mut self, _: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let p = palette(cx);
        let reduce_motion = cx.reduce_motion();
        let host = self.host.clone();

        let prepared = self.host.with_mut(cx, |ed, cx| {
            let snap = ed.scrollbar_snapshot?;
            ed.clamp_and_set_scroll(snap.synced_offset);
            let offset = ed.scroll_offset;
            let axes = ed.scrollbar.prepare(
                snap.viewport,
                offset,
                reduce_motion,
                p,
                window,
                |delay| {
                    let h = host.clone();
                    cx.after(delay, move |cx| {
                        h.with_mut(cx, |ed, _| ed.scrollbar.on_idle_timer_fired());
                        h.notify(cx);
                    })
                },
            );
            Some(ScrollbarPrepaint {
                viewport_bounds: snap.viewport.bounds,
                axes,
            })
        });
        self.prepaint = prepared.flatten();
    }

    fn paint(&mut self, _: Bounds<Pixels>, window: &mut Window, _: &mut App) {
        let Some(sp) = self.prepaint.as_ref() else {
            return;
        };
        let viewport = sp.viewport_bounds;
        ScrollbarState::paint_axes(&sp.axes, viewport, window);

        for item in &sp.axes {
            let item = item.clone();

            // Wheel wakeup
            let host_wheel = self.host.clone();
            window.on_mouse_event(move |event: &ScrollWheelEvent, phase, _, cx| {
                if phase.bubble() && viewport.contains(&event.position) {
                    let mut woke = false;
                    host_wheel.with_mut(cx, |ed, _| {
                        woke = ed.scrollbar.on_wheel(ed.scroll_offset, Instant::now());
                    });
                    if woke {
                        host_wheel.notify(cx);
                    }
                }
            });

            // Track click & thumb grab (registered AFTER editor container -> wins Bubble!)
            if item.interactive {
                let host_down = self.host.clone();
                let down_item = item.clone();
                window.on_mouse_event(move |event: &MouseDownEvent, phase, _, cx| {
                    if phase.bubble() && down_item.track_bounds.contains(&event.position) {
                        cx.stop_propagation();
                        host_down.with_mut(cx, |ed, _| {
                            let next = ed.scrollbar.on_mouse_down(
                                &down_item,
                                event.position,
                                ed.scroll_offset,
                            );
                            ed.clamp_and_set_scroll(next);
                        });
                        host_down.notify(cx);
                    }
                });
            }

            // Hover & thumb drag
            let host_move = self.host.clone();
            let move_item = item.clone();
            window.on_mouse_event(move |event: &MouseMoveEvent, _, _, cx| {
                let h = host_move.clone();
                let outcome = host_move.with_mut(cx, |ed, cx| {
                    let out = ed.scrollbar.on_mouse_move(
                        &move_item,
                        event.position,
                        event.dragging(),
                        ed.scroll_offset,
                        |delay| {
                            let h2 = h.clone();
                            cx.after(delay, move |cx| {
                                let mut fire = false;
                                h2.with_mut(cx, |ed, _| {
                                    fire = ed.scrollbar.on_drag_timer_fired(Instant::now());
                                });
                                if fire {
                                    h2.notify(cx);
                                }
                            })
                        },
                    );
                    if let Some(next) = out.new_offset {
                        ed.clamp_and_set_scroll(next);
                    }
                    out
                });
                if let Some(out) = outcome {
                    if out.stop_propagation {
                        cx.stop_propagation();
                    }
                    if out.notify_now {
                        host_move.notify(cx);
                    }
                }
            });

            // End thumb drag
            let host_up = self.host.clone();
            let up_item = item;
            window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                if !phase.bubble() {
                    return;
                }
                let mut dragged = false;
                host_up.with_mut(cx, |ed, _| {
                    if let Some(next) =
                        ed.scrollbar
                            .on_mouse_up(&up_item, event.position, ed.scroll_offset)
                    {
                        ed.clamp_and_set_scroll(next);
                        dragged = true;
                    }
                });
                if dragged {
                    host_up.notify(cx);
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_scroll_delta_has_dead_zone_and_symmetric_ramps() {
        let bounds = Bounds::new(point(px(0.), px(100.)), size(px(200.), px(100.)));
        assert_eq!(AutoScroll::compute_delta(px(150.), bounds), None);

        let top = AutoScroll::compute_delta(px(80.), bounds).unwrap();
        let bottom = AutoScroll::compute_delta(px(220.), bounds).unwrap();
        assert_eq!(top, -bottom);
        assert!(top < px(-12.));

        let mut scroll = AutoScroll {
            delta: Some(px(20.)),
            last_drag_pos: Some(point(px(1.), px(2.))),
            ..Default::default()
        };
        scroll.stop();
        assert!(!scroll.is_active());
        assert_eq!(scroll.last_drag_pos, None);
    }
}
