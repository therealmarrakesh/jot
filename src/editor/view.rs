use std::borrow::Cow;
use std::ops::Range;
use std::time::{Duration, Instant};

use ropey::Rope;

use crate::color::Rgba;
use crate::geom::{Axis, Edges, Point, Rect, Size, point, size};
use crate::gfx::{Canvas, TextStyle, TextSystem, fill};
use crate::ids::{EditorSlot, Node, UiState};
use crate::platform::{Modifiers, MouseButton, TimerHandle};
use crate::theme::Palette;
use crate::types::SharedStr;

use super::buffer::RopeExt as _;
use super::scrollbar::ScrollbarViewport;
use super::wrap::{LineRanges, RowLayout, VisualRow, can_wrap_with_advances, horizontal_slice};
use super::{DocumentView, Editor, EditorTimer, EditorTimers, MoveDirection};

const BOTTOM_MARGIN_ROWS: usize = 3;
pub(super) const RIGHT_MARGIN: f32 = 10.0;
const HUGE_UNWRAPPED_LINE_BYTES: usize = 4096;
const HORIZONTAL_WINDOW_PAD: f32 = 256.0;

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
    timer: Option<TimerHandle>,
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
            timer: None,
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

#[derive(Clone, Debug, PartialEq)]
pub(super) struct LongestLineCache {
    revision: u64,
    row: usize,
    len: usize,
    style: TextStyle,
    wrap_width: Option<f32>,
}

pub(super) struct VisibleLine {
    pub(super) byte_offset: usize,
    pub(super) top: f32,
    pub(super) layout: RowLayout,
}

pub(super) struct FrameLayout {
    pub(super) revision: u64,
    pub(super) scroll_x: f32,
    pub(super) scroll_y: f64,
    pub(super) input_bounds: Rect,
    pub(super) content_bounds: Rect,
    pub(super) first_line: usize,
    pub(super) lines: Vec<VisibleLine>,
    pub(super) visible_byte_range: Range<usize>,
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
        let ix = buffer_row.checked_sub(self.first_line)?;
        self.lines.get(ix).map(|l| &l.layout)
    }

    pub(super) fn position_for_offset(
        &self,
        text: &Rope,
        offset: usize,
        end_affinity: bool,
    ) -> Option<Point> {
        let row = text.offset_to_point(offset).row;
        let line = self.lines.get(row.checked_sub(self.first_line)?)?;
        let local = offset.saturating_sub(line.byte_offset);
        let pos = line.layout.position_for_index(local, self.line_height, end_affinity)?;
        Some(point(pos.x, line.top + pos.y))
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

    pub(super) fn hit_test(
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

    pub(super) fn paint(&self, p: Palette, canvas: &mut Canvas) {
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

#[inline]
fn snap_point(pt: Point, scale: f32) -> Point {
    point(
        (pt.x * scale).round() / scale,
        (pt.y * scale).round() / scale,
    )
}

struct VisibleSlice {
    lines: Range<usize>,
    top_rel: f32,
    shape_rows: Range<usize>,
}

impl DocumentView {
    pub(super) fn capture_scroll_anchor(&self, scroll_y: f64, line_height: f32) -> Option<ScrollAnchor> {
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

    pub(super) fn restore_scroll_anchor(
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
    pub(super) fn clamp_and_set_scroll_f64(&mut self, mut x: f32, y: f64) -> bool {
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

    pub(super) fn hit_test_point(&self, pos: Point) -> (usize, bool) {
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
        ui: &mut UiState,
        timers: &mut dyn EditorTimers,
    ) -> Vec<Node> {
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
        let space_width = ts.measure(" ", text_style);
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
        let space_width = ts.measure(" ", text_style);
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
                    let w = measure_unwrapped_line(&s, text_style, ts);
                    doc.longest_line_cache = Some((key, w));
                    w
                }
            };
            if reveal_req.is_some() {
                let pt = text.offset_to_point(head);
                let line = text.slice_line(pt.row);
                let prefix: Cow<str> = line.slice(..pt.column.min(line.len())).into();
                let caret_x = measure_unwrapped_line(&prefix, text_style, ts);
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
                    if can_wrap_with_advances(&line_str) {
                        let min_x = (-self.scroll.x - HORIZONTAL_WINDOW_PAD).max(0.0);
                        let max_x =
                            (-self.scroll.x + viewport_width + HORIZONTAL_WINDOW_PAD).max(min_x);
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

fn content_width_with_margin(max_line_w: f32, viewport_width: f32) -> f32 {
    if max_line_w + RIGHT_MARGIN > viewport_width {
        max_line_w + RIGHT_MARGIN
    } else {
        max_line_w
    }
}

fn measure_unwrapped_line(s: &str, text_style: &TextStyle, ts: &TextSystem) -> f32 {
    if s.is_empty() {
        return 0.0;
    }
    if s.len() > HUGE_UNWRAPPED_LINE_BYTES && can_wrap_with_advances(s) {
        ts.with_advances(text_style, |adv| horizontal_slice(s, 0.0, 0.0, adv).3)
    } else {
        ts.measure(s, text_style)
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
