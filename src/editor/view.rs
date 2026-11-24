use std::borrow::Cow;
use std::ops::Range;
use std::time::{Duration, Instant};

use ropey::Rope;

use crate::color::Rgba;
use crate::geom::{Axis, Edges, Point, Rect, Size, point, size};
use crate::gfx::{D2DRenderer, Font, TextStyle, TextSystem, fill};
use crate::ids::{Node, UiState};
use crate::platform::{Modifiers, MouseButton, TimerHandle};
use crate::scrollbar::ScrollbarViewport;
use crate::theme::Palette;
use crate::types::SharedStr;

use super::buffer::RopeExt as _;
use super::wrap::{LineRanges, RowLayout, VisualRow, can_wrap_with_advances, horizontal_slice};
use super::{DocumentView, Editor, EditorTimer, EditorTimers, MoveDirection, ViewKind};

const BOTTOM_MARGIN_ROWS: usize = 3;
pub(super) const RIGHT_MARGIN: f32 = 10.0;
const HUGE_UNWRAPPED_LINE_BYTES: usize = 4096;
const HORIZONTAL_WINDOW_PAD: f32 = 256.0;

#[derive(Clone, Debug)]
pub(crate) struct EditorPaintStyle {
    pub(crate) font: Font,
    pub(crate) font_size: f32,
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

#[derive(Copy, Clone, Debug, Default)]
pub(crate) struct EditorPaintOutcome {
    pub(crate) needs_redraw: bool,
}

#[derive(Copy, Clone, Debug)]
pub(super) struct ScrollAnchor {
    line: usize,
    byte_in_line: usize,
    frac_px: f64,
}

pub(crate) struct BlinkState {
    active: bool,
    visible: bool,
    paused: bool,
    started_at: Option<Instant>,
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
            active: false,
            visible: false,
            paused: false,
            started_at: None,
            timer: TimerHandle::default(),
        }
    }

    #[inline]
    pub(crate) fn visible(&self) -> bool {
        self.paused || self.visible
    }

    pub(crate) fn stop(&mut self) {
        self.active = false;
        self.paused = false;
        self.visible = false;
        self.started_at = None;
        self.timer.cancel();
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
            self.timer.cancel();
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
            self.timer.cancel();
            return;
        };
        if let Some(timeout) = timeout
            && self
                .started_at
                .is_some_and(|start| start.elapsed() >= timeout)
        {
            self.visible = true;
            self.timer.cancel();
            return;
        }
        self.visible = !self.visible;
        self.timer = timers.after(interval, EditorTimer::Blink);
    }
}

#[derive(Default)]
pub(crate) struct AutoScroll {
    delta: Option<f32>,
    timer: TimerHandle,
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
        self.timer.cancel();
        self.last_drag_pos = None;
    }

    pub(crate) fn update(&mut self, delta: Option<f32>, timers: &mut dyn EditorTimers) {
        let was_active = self.delta.is_some();
        self.delta = delta;
        if delta.is_none() {
            self.timer.cancel();
            return;
        }
        if !was_active {
            let interval = Duration::from_millis(16);
            self.timer = timers.every(interval, interval, EditorTimer::AutoScroll);
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct LongestLineCache {
    revision: u64,
    row: usize,
    len: usize,
    font: Font,
    font_size: f32,
    wrap_width: Option<f32>,
}

pub(super) struct FrameLayout {
    pub(super) input_bounds: Rect,
    pub(super) content_bounds: Rect,
    pub(super) visible_lines: Vec<usize>,
    pub(super) visible_byte_offsets: Vec<usize>,
    pub(super) visible_line_tops: Vec<f32>,
    pub(super) visible_byte_range: Range<usize>,
    pub(super) rows: Vec<RowLayout>,
    pub(super) line_height: f32,
    pub(super) wrap_width: Option<f32>,
    pub(super) space_width: f32,
    pub(super) text_color: Rgba,
    pub(super) caret_rect: Rect,
    pub(super) show_caret: bool,
    pub(super) selection_rects: Vec<Rect>,
    pub(super) marked_range: Option<Range<usize>>,
    pub(super) last_cursor: usize,
}

impl FrameLayout {
    pub(super) fn row_layout(&self, buffer_row: usize) -> Option<&RowLayout> {
        let ix = self.visible_lines.binary_search(&buffer_row).ok()?;
        self.rows.get(ix)
    }

    pub(super) fn position_for_offset(
        &self,
        text: &Rope,
        offset: usize,
        end_affinity: bool,
    ) -> Option<Point> {
        let row = text.offset_to_point(offset).row;
        let vi = self.visible_lines.binary_search(&row).ok()?;
        let local = offset.saturating_sub(self.visible_byte_offsets[vi]);
        let pos = self.rows[vi].position_for_index(local, self.line_height, end_affinity)?;
        Some(point(pos.x, self.visible_line_tops[vi] + pos.y))
    }

    pub(super) fn bounds_for_range(&self, text: &Rope, range: Range<usize>) -> Option<Rect> {
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

    pub(super) fn hit_test(&self, pos: Point, text_len: usize, multi_line: bool) -> (usize, bool) {
        if text_len == 0 {
            return (0, false);
        }
        let lh = self.line_height;
        let rel = pos - self.content_bounds.origin;

        for (vi, row_layout) in self.rows.iter().enumerate() {
            let base = self.visible_byte_offsets[vi];
            let local_pos = rel - point(0.0, self.visible_line_tops[vi]);

            if !multi_line {
                let col = row_layout.closest_index_for_x(local_pos.x);
                return ((base + col).min(text_len), false);
            }
            if let Some((col, aff)) = row_layout.closest_index_for_position(local_pos, lh) {
                return ((base + col).min(text_len), aff);
            } else if local_pos.y < 0.0 {
                return (base.min(text_len), false);
            }
        }

        if let Some((row, base)) = self.rows.last().zip(self.visible_byte_offsets.last()) {
            let col = row.closest_index_for_x_in_last_row(rel.x);
            return ((base + col).min(text_len), false);
        }
        (text_len, false)
    }

    pub(super) fn paint(&self, p: Palette, renderer: &mut D2DRenderer) {
        renderer.push_clip(self.input_bounds);
        for &rect in &self.selection_rects {
            renderer.paint_quad(fill(rect, p.selection));
        }

        let origin = self.content_bounds.origin;
        for (vi, row_layout) in self.rows.iter().enumerate() {
            let line_base = self.visible_byte_offsets[vi];
            let line_marked = self.marked_range.as_ref().and_then(|m| {
                (m.end > line_base).then(|| {
                    m.start.saturating_sub(line_base)..m.end.saturating_sub(line_base)
                })
            });
            row_layout.paint(
                point(origin.x, origin.y + self.visible_line_tops[vi]),
                self.line_height,
                self.text_color,
                line_marked,
                renderer,
            );
        }

        if self.show_caret {
            renderer.paint_quad(fill(self.caret_rect, p.caret));
        }
        renderer.pop_clip();
    }

    fn range_rects(&self, range: &Range<usize>) -> Vec<Rect> {
        let range = range.start.max(self.visible_byte_range.start)
            ..range.end.min(self.visible_byte_range.end);
        if range.is_empty() {
            return Vec::new();
        }
        let origin = self.content_bounds.origin;
        let mut out = Vec::new();
        for (vi, row_layout) in self.rows.iter().enumerate() {
            let line_base = self.visible_byte_offsets[vi];
            let line_y = self.visible_line_tops[vi];
            let mut off = line_base + row_layout.leading_bytes;
            for (i, row) in row_layout.rows.iter().enumerate() {
                let abs_sub = row_layout.first_sub_row + i;
                let row_y = line_y + abs_sub as f32 * self.line_height;
                let end = off + row.byte_len();
                if !row.is_shaped() {
                    off = end;
                    continue;
                }
                let s = range.start.max(off);
                let e = range.end.min(end);
                let is_last = abs_sub + 1 == row_layout.total_sub_rows;
                let includes_nl = is_last && range.start <= end && range.end > end;
                let indent = row_layout.indent_for(abs_sub);
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

fn scrollbar_viewport(
    input_size: Size,
    content_size: Size,
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
        horizontal_left: paddings.left,
        horizontal: !soft_wrap,
    }
}

fn clamp_horizontal_scroll(offset_x: f32, content_width: f32, viewport_width: f32) -> f32 {
    let min_x = (viewport_width - content_width).min(0.0);
    offset_x.clamp(min_x, 0.0)
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

#[inline]
fn snap_point(pt: Point, scale: f32) -> Point {
    point(
        (pt.x * scale).round() / scale,
        (pt.y * scale).round() / scale,
    )
}

impl DocumentView {
    pub(super) fn capture_scroll_anchor(&self, line_height: f32) -> Option<ScrollAnchor> {
        if self.wrap.visual_row_count() == 0 || line_height <= 0.0 {
            return None;
        }
        let lh = f64::from(line_height);
        let top = (-self.scroll_y).max(0.0);
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

    pub(super) fn restore_scroll_anchor(
        &mut self,
        text: &Rope,
        anchor: ScrollAnchor,
        line_height: f32,
    ) {
        if self.wrap.line_count() == 0 || line_height <= 0.0 {
            return;
        }
        let line = anchor.line.min(self.wrap.line_count().saturating_sub(1));
        let line_start = text.line_start_offset(line);
        let line_len = text.line_len(line);
        let target = line_start + anchor.byte_in_line.min(line_len);
        let disp = self.wrap.offset_to_display(text, target, false);
        let new_top = disp.row as f64 * f64::from(line_height) + anchor.frac_px;
        self.scroll_y = -new_top.max(0.0);
    }

    fn visible_slice(
        &self,
        line_height: f32,
        viewport_height: f32,
    ) -> (Range<usize>, Vec<usize>, f32, Range<usize>) {
        let total_rows = self.wrap.visual_row_count();
        let total_lines = self.wrap.line_count();
        if total_rows == 0 || total_lines == 0 {
            return (0..0, Vec::new(), 0.0, 0..0);
        }
        let lh = f64::from(line_height);
        let top = (-self.scroll_y).max(0.0);
        let bottom = top + f64::from(viewport_height);
        let first_row = ((top / lh).floor() as usize).min(total_rows - 1);
        let last_row = ((bottom / lh).ceil() as usize).min(total_rows - 1);
        let shape_rows = first_row.saturating_sub(2)..(last_row + 3).min(total_rows);

        let start_line = self.wrap.visual_row_to_line(first_row);
        let end_line = self.wrap.visual_row_to_line(last_row);
        let first_vis_row = self.wrap.first_visual_row(start_line);
        let visible_top_rel = (first_vis_row as f64 * lh + self.scroll_y) as f32;
        let line_range = start_line..(end_line + 2).min(total_lines);
        let lines: Vec<usize> = line_range.clone().collect();
        (line_range, lines, visible_top_rel, shape_rows)
    }
}

impl Editor {
    pub(super) fn clamp_and_set_scroll_f64(&mut self, mut x: f32, y: f64) -> bool {
        let y_changed = match &mut self.view {
            ViewKind::Document(doc) => {
                let min_y =
                    (-doc.content_height_f64 + f64::from(self.input_bounds.size.height)).min(0.0);
                let next_y = y.clamp(min_y, 0.0);
                let changed = doc.scroll_y != next_y;
                doc.scroll_y = next_y;
                changed
            }
            ViewKind::Field => false,
        };
        x = clamp_horizontal_scroll(x, self.content_width, self.input_bounds.size.width);
        let x_changed = self.scroll_x != x;
        self.scroll_x = x;
        x_changed || y_changed
    }

    pub(super) fn hit_test_point(&self, pos: Point) -> (usize, bool) {
        let Some(frame) = self.frame.as_ref() else {
            return (0, false);
        };
        frame.hit_test(pos, self.model.text().len(), self.model.is_multi_line())
    }

    pub(crate) fn on_scrollbar_mouse_down(
        &mut self,
        axis: Axis,
        hit_thumb: bool,
        position: Point,
        track_rect: Rect,
    ) -> bool {
        let offset = self.scroll_offset();
        let next = match &mut self.view {
            ViewKind::Document(doc) => {
                doc.scrollbar
                    .on_mouse_down(axis, hit_thumb, position, track_rect, offset)
            }
            ViewKind::Field => None,
        };
        if let Some(next) = next {
            self.reveal = None;
            self.clamp_and_set_scroll_f64(next.x, f64::from(next.y));
            true
        } else {
            false
        }
    }

    pub(crate) fn on_scrollbar_mouse_move(
        &mut self,
        position: Point,
        dragging_left: bool,
        track_rect: Option<Rect>,
        timers: &mut dyn EditorTimers,
    ) -> Option<bool> {
        let scroll = self.scroll_offset();
        let ViewKind::Document(doc) = &mut self.view else {
            return None;
        };
        let (next, notify) = doc.scrollbar.on_mouse_move(
            position,
            dragging_left,
            track_rect,
            scroll,
            |delay| timers.after(delay, EditorTimer::ScrollbarDrag),
        )?;
        if let Some(next) = next {
            self.reveal = None;
            self.clamp_and_set_scroll_f64(next.x, f64::from(next.y));
        }
        Some(notify)
    }

    pub(crate) fn on_scrollbar_mouse_up(
        &mut self,
        position: Point,
        track_rect: Option<Rect>,
    ) -> bool {
        let scroll = self.scroll_offset();
        let next = match &mut self.view {
            ViewKind::Document(doc) => doc.scrollbar.on_mouse_up(position, track_rect, scroll),
            ViewKind::Field => None,
        };
        if let Some(next) = next {
            self.reveal = None;
            self.clamp_and_set_scroll_f64(next.x, f64::from(next.y));
            true
        } else {
            false
        }
    }

    pub(crate) fn render_scrollbars(
        &mut self,
        line_height: f32,
        p: Palette,
        now: Instant,
        ui: &mut UiState,
        timers: &mut dyn EditorTimers,
    ) -> Vec<Node> {
        let input_size = self.input_bounds.size;
        if input_size.width <= 0. || input_size.height <= 0. {
            return Vec::new();
        }
        let scroll = self.scroll_offset();
        let content_width = self.content_width;
        let ViewKind::Document(doc) = &mut self.view else {
            return Vec::new();
        };
        let total_rows = doc.wrap.visual_row_count().max(1);
        let content_h = (total_rows as f64 * f64::from(line_height.max(1.0)))
            .max(f64::from(input_size.height)) as f32;
        let vp = scrollbar_viewport(
            input_size,
            size(content_width, content_h),
            doc.paddings,
            doc.soft_wrap,
        );
        doc.scrollbar.render(vp, scroll, p, now, ui, |delay| {
            timers.after(delay, EditorTimer::ScrollbarIdle)
        })
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
            if let ViewKind::Document(doc) = &mut self.view {
                doc.pending_context_menu = Some((position, offset));
            }
            return;
        }
        if modifiers.shift {
            self.select_to_with_affinity(offset, aff);
            self.request_reveal(self.cursor(), aff, None, false);
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
            && self.frame.is_some()
        {
            let (offset, aff) = self.hit_test_point(position);
            self.select_to_with_affinity(offset, aff);
            if let ViewKind::Document(doc) = &mut self.view {
                doc.auto_scroll.last_drag_pos = Some(position);
                let delta = AutoScroll::compute_delta(position.y, self.input_bounds);
                doc.auto_scroll.update(delta.map(|d| -d), timers);
            }
            self.request_reveal(self.cursor(), aff, None, false);
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
            && let ViewKind::Document(doc) = &mut self.view
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
        let cur_y = match &self.view {
            ViewKind::Document(doc) => doc.scroll_y,
            ViewKind::Field => 0.0,
        };
        let moved = self.clamp_and_set_scroll_f64(self.scroll_x + dx, cur_y + dy);
        if moved {
            let scroll = self.scroll_offset();
            if let ViewKind::Document(doc) = &mut self.view {
                doc.scrollbar.on_wheel(scroll, Instant::now());
            }
        }
        moved
    }

    pub(crate) fn on_timer(&mut self, kind: EditorTimer, timers: &mut dyn EditorTimers) -> bool {
        match kind {
            EditorTimer::Blink => self.blink.on_timer(timers),
            EditorTimer::AutoScroll => {
                let (delta, drag_pos, cur_y) = match &mut self.view {
                    ViewKind::Document(doc) => match doc.auto_scroll.delta {
                        Some(d) => (d, doc.auto_scroll.last_drag_pos, doc.scroll_y),
                        None => {
                            doc.auto_scroll.stop();
                            return false;
                        }
                    },
                    ViewKind::Field => return false,
                };
                self.reveal = None;
                self.clamp_and_set_scroll_f64(self.scroll_x, cur_y + f64::from(delta));
                if let Some(pos) = drag_pos {
                    let (offset, aff) = self.hit_test_point(pos);
                    self.select_to_with_affinity(offset, aff);
                }
                true
            }
            EditorTimer::ScrollbarIdle => match &mut self.view {
                ViewKind::Document(doc) => {
                    doc.scrollbar.on_idle_timer_fired();
                    true
                }
                ViewKind::Field => false,
            },
            EditorTimer::ScrollbarDrag => match &mut self.view {
                ViewKind::Document(doc) => doc.scrollbar.on_drag_timer_fired(Instant::now()),
                ViewKind::Field => false,
            },
        }
    }

    pub(crate) fn prepare(
        &mut self,
        input_bounds: Rect,
        params: EditorPaintParams<'_>,
        ts: &TextSystem,
    ) -> EditorPaintOutcome {
        let prev_size = self.input_bounds.size;
        self.input_bounds = input_bounds;
        if matches!(self.view, ViewKind::Field) {
            self.prepare_field(input_bounds, params, ts);
            return EditorPaintOutcome::default();
        }
        self.prepare_document(input_bounds, prev_size, params, ts)
    }

    pub(crate) fn paint(&self, p: Palette, renderer: &mut D2DRenderer) {
        if let Some(frame) = self.frame.as_ref() {
            frame.paint(p, renderer);
        }
    }

    fn prepare_field(&mut self, input_bounds: Rect, params: EditorPaintParams<'_>, ts: &TextSystem) {
        let EditorPaintParams {
            style,
            palette: p,
            window_active,
            focused,
            show_selection,
            scale,
            caret_width_px,
            ..
        } = params;
        let text_style = TextStyle::new(style.font.clone(), style.font_size);
        let line_height = style.line_height.max(1.0);
        let space_width = ts.measure(" ", &text_style);
        let is_empty = self.model.text().len() == 0;
        let (display_str, text_color): (SharedStr, Rgba) = if is_empty {
            (self.placeholder.clone(), p.text_secondary)
        } else {
            (self.model.value().into(), style.color)
        };
        let shaped = ts.shape_row(display_str, &text_style);
        let row_layout = RowLayout::from_shaped(vec![shaped]);
        let head = self.cursor();
        let head_pt = row_layout
            .position_for_index(head, line_height, false)
            .unwrap_or(Point::ZERO);
        let max_line_w = row_layout.size(line_height).width.max(head_pt.x);
        let content_width = if max_line_w + RIGHT_MARGIN > input_bounds.size.width {
            max_line_w + RIGHT_MARGIN
        } else {
            max_line_w
        };
        self.content_width = content_width;

        if self.reveal.take().is_some() {
            let w = input_bounds.size.width;
            if self.scroll_x + head_pt.x > w - RIGHT_MARGIN {
                self.scroll_x = w - RIGHT_MARGIN - head_pt.x;
            } else if self.scroll_x + head_pt.x < 0.0 {
                self.scroll_x = -head_pt.x;
            }
        }
        self.scroll_x =
            clamp_horizontal_scroll(self.scroll_x, content_width, input_bounds.size.width);

        let content_bounds = Rect::new(
            point(input_bounds.origin.x + self.scroll_x, input_bounds.origin.y),
            input_bounds.size,
        );
        let caret_rect = self.compute_caret_rect(
            content_bounds.origin,
            head_pt,
            line_height,
            scale,
            caret_width_px,
        );

        let show_caret = focused
            && window_active
            && self.blink.visible()
            && !self.caret_hidden
            && (self.model.selection().is_empty() || self.model.composition().is_some());

        let mut frame = FrameLayout {
            input_bounds,
            content_bounds,
            visible_lines: vec![0],
            visible_byte_offsets: vec![0],
            visible_line_tops: vec![0.0],
            visible_byte_range: 0..self.model.text().len(),
            rows: vec![row_layout],
            line_height,
            wrap_width: None,
            space_width,
            text_color,
            caret_rect,
            show_caret,
            selection_rects: Vec::new(),
            marked_range: (!is_empty).then(|| self.model.composition_range()).flatten(),
            last_cursor: head,
        };
        if window_active
            && show_selection
            && self.model.composition().is_none()
            && !self.model.selection().is_empty()
        {
            frame.selection_rects = frame.range_rects(&self.model.selected_range());
        }
        self.frame = Some(frame);
    }

    fn prepare_document(
        &mut self,
        input_bounds: Rect,
        prev_size: Size,
        params: EditorPaintParams<'_>,
        ts: &TextSystem,
    ) -> EditorPaintOutcome {
        let EditorPaintParams {
            style,
            palette: p,
            window_active,
            focused,
            show_selection,
            scale,
            caret_width_px,
        } = params;
        let font = &style.font;
        let font_size = style.font_size;
        let text_style = TextStyle::new(font.clone(), font_size);
        let line_height = style.line_height.max(1.0);
        let space_width = ts.measure(" ", &text_style);
        let text = self.model.text();
        let reveal_req = self.reveal.take();
        let prev_content_width = self.content_width;

        let ViewKind::Document(doc) = &mut self.view else {
            unreachable!()
        };
        let prev_content_height = doc.content_height_f64;
        let prev_scroll_y = doc.scroll_y;
        let soft_wrap = doc.soft_wrap;
        let wrap_width = soft_wrap.then_some((input_bounds.size.width - RIGHT_MARGIN).max(1.0));

        let reflowing = doc.wrap.font() != font
            || doc.wrap.font_size() != font_size
            || doc.wrap.wrap_width() != wrap_width
            || (doc.last_line_height - line_height).abs() > 0.01;
        let anchor = if reflowing && reveal_req.is_none() {
            doc.capture_scroll_anchor(doc.last_line_height)
        } else {
            None
        };

        doc.wrap
            .sync_layout(text, font.clone(), font_size, wrap_width, ts);
        doc.last_line_height = line_height;

        if let Some(anchor) = anchor {
            doc.restore_scroll_anchor(text, anchor, line_height);
        }

        let total_rows = doc.wrap.visual_row_count().max(1);
        let content_height_f64 = (total_rows as f64 * f64::from(line_height))
            .max(f64::from(input_bounds.size.height));
        doc.content_height_f64 = content_height_f64;

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
            let prev_y = doc.scroll_y;
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
            doc.scroll_y = next_y;
        }
        let min_y_f64 = (-content_height_f64 + f64::from(input_bounds.size.height)).min(0.0);
        doc.scroll_y = doc.scroll_y.clamp(min_y_f64, 0.0);

        let placeholder_rope = Rope::from(self.placeholder.as_str());
        let is_empty = text.len() == 0;
        let (display_text, text_color) = if is_empty {
            (&placeholder_rope, p.text_secondary)
        } else {
            (text, style.color)
        };

        let (vis_range, visible_lines, visible_top_rel, shape_rows) =
            doc.visible_slice(line_height, input_bounds.size.height);
        let start_off = text.line_start_offset(vis_range.start);
        let end_off = text.line_end_offset(vis_range.end.saturating_sub(1));
        let visible_byte_offsets: Vec<usize> = visible_lines
            .iter()
            .map(|&l| text.line_start_offset(l))
            .collect();

        let head = self.model.cursor();
        let end_affinity = self.model.selection().end_affinity;
        let (rows, shaped_windowed) = doc.shape_lines(
            ShapeContext {
                is_empty,
                display_text,
                visible_lines: &visible_lines,
                visible_byte_offsets: &visible_byte_offsets,
                shape_rows: &shape_rows,
                text_style: &text_style,
                scroll_x: self.scroll_x,
                viewport_width: input_bounds.size.width,
                caret: head,
                end_affinity,
            },
            ts,
        );

        let mut visible_line_tops = Vec::with_capacity(rows.len());
        let mut y = visible_top_rel;
        for row_layout in &rows {
            visible_line_tops.push(y);
            y += row_layout.size(line_height).height;
        }

        let mut max_line_w = wrap_width.unwrap_or(0.0);
        if !soft_wrap {
            let longest_row = doc.wrap.longest_line();
            let slice = text.slice_line(longest_row);
            let key = LongestLineCache {
                revision: self.model.revision(),
                row: longest_row,
                len: slice.len(),
                font: font.clone(),
                font_size,
                wrap_width,
            };
            max_line_w = match &doc.longest_line_cache {
                Some((cached, w)) if *cached == key => *w,
                _ => {
                    let s: Cow<str> = slice.into();
                    let w = if s.len() > HUGE_UNWRAPPED_LINE_BYTES && can_wrap_with_advances(&s) {
                        ts.with_advances(&text_style, |adv| {
                            horizontal_slice(&s, 0.0, 0.0, adv).3
                        })
                    } else {
                        ts.measure(&s, &text_style)
                    };
                    doc.longest_line_cache = Some((key, w));
                    w
                }
            };
            for row_layout in &rows {
                max_line_w = max_line_w.max(row_layout.size(line_height).width);
            }
        }

        let head_pt = {
            let row = text.offset_to_point(head).row;
            if let Ok(vi) = visible_lines.binary_search(&row) {
                let local = head.saturating_sub(visible_byte_offsets[vi]);
                rows[vi]
                    .position_for_index(local, line_height, end_affinity)
                    .map(|pt| point(pt.x, visible_line_tops[vi] + pt.y))
            } else {
                None
            }
            .unwrap_or_else(|| {
                let disp = doc.wrap.offset_to_display(text, head, end_affinity);
                point(
                    0.0,
                    (disp.row as f64 * f64::from(line_height) + doc.scroll_y) as f32,
                )
            })
        };
        if !soft_wrap {
            max_line_w = max_line_w.max(head_pt.x);
        }

        let content_width = if max_line_w + RIGHT_MARGIN > input_bounds.size.width {
            max_line_w + RIGHT_MARGIN
        } else {
            max_line_w
        };
        let content_size = size(content_width, content_height_f64 as f32);
        self.content_width = content_width;

        let prev_scroll_x = self.scroll_x;
        if reveal_req.is_some() && !soft_wrap {
            let w = input_bounds.size.width;
            if self.scroll_x + head_pt.x > w - RIGHT_MARGIN {
                self.scroll_x = w - RIGHT_MARGIN - head_pt.x;
            } else if self.scroll_x + head_pt.x < 0.0 {
                self.scroll_x = -head_pt.x;
            }
        }
        self.scroll_x = if soft_wrap {
            0.0
        } else {
            clamp_horizontal_scroll(self.scroll_x, content_size.width, input_bounds.size.width)
        };

        // Vertical scroll is baked into visible_line_tops in f64 so D2D coordinates stay exact on huge files.
        let content_bounds = Rect::new(
            point(input_bounds.origin.x + self.scroll_x, input_bounds.origin.y),
            input_bounds.size,
        );

        let scrollable = content_height_f64 > f64::from(input_bounds.size.height)
            || (!soft_wrap && content_width > input_bounds.size.width);
        let scrollbar_metrics_changed = scrollable
            && (prev_size != input_bounds.size
                || (prev_content_width - content_width).abs() > 0.5
                || (prev_content_height - content_height_f64).abs() > 0.5
                || (prev_scroll_y - doc.scroll_y).abs() > 0.5
                || (prev_scroll_x - self.scroll_x).abs() > 0.5);

        let caret_rect = self.compute_caret_rect(
            content_bounds.origin,
            head_pt,
            line_height,
            scale,
            caret_width_px,
        );
        let show_caret = focused
            && window_active
            && self.blink.visible()
            && !self.caret_hidden
            && (self.model.selection().is_empty() || self.model.composition().is_some());

        let mut frame = FrameLayout {
            input_bounds,
            content_bounds,
            visible_lines,
            visible_byte_offsets,
            visible_line_tops,
            visible_byte_range: start_off..end_off,
            rows,
            line_height,
            wrap_width,
            space_width,
            text_color,
            caret_rect,
            show_caret,
            selection_rects: Vec::new(),
            marked_range: (!is_empty).then(|| self.model.composition_range()).flatten(),
            last_cursor: head,
        };
        if window_active
            && show_selection
            && self.model.composition().is_none()
            && !self.model.selection().is_empty()
        {
            frame.selection_rects = frame.range_rects(&self.model.selected_range());
        }
        self.frame = Some(frame);

        let needs_redraw = (shaped_windowed && (self.scroll_x - prev_scroll_x).abs() > 1.0)
            || scrollbar_metrics_changed;
        EditorPaintOutcome { needs_redraw }
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

struct ShapeContext<'a> {
    is_empty: bool,
    display_text: &'a Rope,
    visible_lines: &'a [usize],
    visible_byte_offsets: &'a [usize],
    shape_rows: &'a Range<usize>,
    text_style: &'a TextStyle,
    scroll_x: f32,
    viewport_width: f32,
    caret: usize,
    end_affinity: bool,
}

impl DocumentView {
    fn shape_lines(&self, cx: ShapeContext<'_>, ts: &TextSystem) -> (Vec<RowLayout>, bool) {
        let ShapeContext {
            is_empty,
            display_text,
            visible_lines,
            visible_byte_offsets,
            shape_rows,
            text_style,
            scroll_x,
            viewport_width,
            caret,
            end_affinity,
        } = cx;
        if is_empty {
            let text = display_text.to_string();
            let shaped = text
                .split('\n')
                .map(|part| ts.shape_row(part.to_string().into(), text_style))
                .collect();
            return (vec![RowLayout::from_shaped(shaped)], false);
        }

        let mut result = Vec::with_capacity(visible_lines.len());
        let mut shaped_windowed = false;

        for (vi, &buf_line) in visible_lines.iter().enumerate() {
            let line_slice = display_text.slice_line(buf_line);
            let line_len = line_slice.len();
            let line_byte_start = visible_byte_offsets[vi];
            let wrapped = self.wrap.line(buf_line).expect("wrapped line exists");
            let first_row_of_line = self.wrap.first_visual_row(buf_line);
            let total_sub_rows = wrapped.row_count();

            if !self.soft_wrap && total_sub_rows == 1 && line_len > HUGE_UNWRAPPED_LINE_BYTES {
                let line_str: Cow<str> = line_slice.into();
                if can_wrap_with_advances(&line_str) {
                    let min_x = (-scroll_x - HORIZONTAL_WINDOW_PAD).max(0.0);
                    let max_x = (-scroll_x + viewport_width + HORIZONTAL_WINDOW_PAD).max(min_x);
                    let (mut start_b, mut end_b, start_x, total_w) =
                        ts.with_advances(text_style, |adv| {
                            horizontal_slice(&line_str, min_x, max_x, adv)
                        });
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
                    result.push(RowLayout::from_slice(vec![vrow], 0, 0, 1, 0.0));
                    continue;
                }
            }

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
                    let disp = self
                        .wrap
                        .offset_to_display(display_text, caret, end_affinity);
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

            result.push(RowLayout::from_slice(
                rows,
                slice_start,
                leading_bytes,
                total_sub_rows,
                wrapped.hanging_indent,
            ));
        }
        (result, shaped_windowed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::rect;

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
}
