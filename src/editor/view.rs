use std::borrow::Cow;
use std::ops::Range;
use std::rc::Rc;
use std::time::{Duration, Instant};

use ropey::Rope;

use crate::color::Rgba;
use crate::geom::{Edges, Point, Rect, Size, point, size};
use crate::gfx::{D2DRenderer, Font, TextStyle, TextSystem, fill};
use crate::platform::{Modifiers, MouseButton, TimerHandle, system_settings};
use crate::scrollbar::ScrollbarViewport;
use crate::theme::Palette;
use crate::types::SharedStr;

use super::buffer::RopeExt as _;
use super::wrap::{LineRanges, RowLayout, VisualRow, can_wrap_with_advances, horizontal_slice};
use super::{Editor, EditorTimer, EditorTimers, MoveDirection};

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
    pub(crate) reduce_motion: bool,
    pub(crate) now: Instant,
}

#[derive(Copy, Clone, Debug, Default)]
pub(crate) struct EditorPaintOutcome {
    pub(crate) needs_redraw: bool,
    pub(crate) needs_vsync: bool,
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
        let Some(delay) = system_settings().caret_blink_interval else {
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
        let settings = system_settings();
        let Some(interval) = settings.caret_blink_interval else {
            self.visible = true;
            self.timer.cancel();
            return;
        };
        if let Some(timeout) = settings.caret_blink_timeout
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

#[derive(Clone)]
pub(super) struct ViewLayout {
    pub(super) visible_lines: Vec<usize>,
    pub(super) visible_byte_offsets: Vec<usize>,
    pub(super) visible_top_rel: f32,
    pub(super) visible_byte_range: Range<usize>,
    pub(super) rows: Rc<Vec<RowLayout>>,
    pub(super) line_height: f32,
    pub(super) wrap_width: Option<f32>,
    pub(super) space_width: f32,
}

impl ViewLayout {
    pub(super) fn row_layout(&self, buffer_row: usize) -> Option<&RowLayout> {
        let ix = self.visible_lines.binary_search(&buffer_row).ok()?;
        self.rows.get(ix)
    }
}

fn scrollbar_viewport(
    input_bounds: Rect,
    content_size: Size,
    paddings: Edges<f32>,
    soft_wrap: bool,
) -> ScrollbarViewport {
    ScrollbarViewport {
        bounds: Rect::new(
            point(input_bounds.origin.x, input_bounds.origin.y - paddings.top),
            size(
                input_bounds.size.width + paddings.right,
                input_bounds.size.height + paddings.top + paddings.bottom,
            ),
        ),
        content_size: size(
            content_size.width + paddings.right,
            content_size.height + paddings.top + paddings.bottom,
        ),
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

impl Editor {
    pub(super) fn capture_scroll_anchor(&self, line_height: f32) -> Option<ScrollAnchor> {
        if !self.multi_line || self.wrap.visual_row_count() == 0 || line_height <= 0.0 {
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

    pub(super) fn restore_scroll_anchor(&mut self, anchor: ScrollAnchor, line_height: f32) {
        if self.wrap.line_count() == 0 || line_height <= 0.0 {
            return;
        }
        let line = anchor.line.min(self.wrap.line_count().saturating_sub(1));
        let line_start = self.text.line_start_offset(line);
        let line_len = self.text.line_len(line);
        let target = line_start + anchor.byte_in_line.min(line_len);
        let disp = self.wrap.offset_to_display(target, false);
        let new_top = disp.row as f64 * f64::from(line_height) + anchor.frac_px;
        self.scroll_y = -new_top.max(0.0);
    }

    pub(super) fn position_for_offset(&self, offset: usize) -> Option<Point> {
        let layout = self.layout.as_ref()?;
        let mut y = layout.visible_top_rel;
        for (vi, row_layout) in layout.rows.iter().enumerate() {
            let base = layout.visible_byte_offsets[vi];
            if offset < base {
                return None;
            }
            let local = offset - base;
            if let Some(pos) = row_layout.position_for_index(local, layout.line_height, false) {
                return Some(point(pos.x, pos.y + y));
            }
            y += row_layout.size(layout.line_height).height;
        }
        None
    }

    pub(super) fn clamp_and_set_scroll_f64(&mut self, mut x: f32, mut y: f64) -> bool {
        let min_y = (-self.content_height_f64 + f64::from(self.input_bounds.size.height)).min(0.0);
        y = if self.multi_line {
            y.clamp(min_y, 0.0)
        } else {
            0.0
        };
        x = clamp_horizontal_scroll(x, self.content_size.width, self.input_bounds.size.width);
        let changed = self.scroll_x != x || self.scroll_y != y;
        self.scroll_x = x;
        self.scroll_y = y;
        changed
    }

    pub(super) fn hit_test_point(&self, pos: Point) -> (usize, bool) {
        if self.text.len() == 0 {
            return (0, false);
        }
        let (Some(bounds), Some(layout)) = (self.content_bounds.as_ref(), self.layout.as_ref())
        else {
            return (0, false);
        };
        let lh = layout.line_height;
        let rel = pos - bounds.origin;
        let mut y = layout.visible_top_rel;

        for (vi, row_layout) in layout.rows.iter().enumerate() {
            let base = layout.visible_byte_offsets[vi];
            let local_pos = rel - point(0.0, y);

            if !self.multi_line {
                let col = row_layout.closest_index_for_x(local_pos.x);
                return ((base + col).min(self.text.len()), false);
            }
            if let Some((col, aff)) = row_layout.closest_index_for_position(local_pos, lh) {
                return ((base + col).min(self.text.len()), aff);
            } else if local_pos.y < 0.0 {
                return (base.min(self.text.len()), false);
            }
            y += row_layout.size(lh).height;
        }

        if let Some((row, base)) = layout.rows.last().zip(layout.visible_byte_offsets.last()) {
            let col = row.closest_index_for_x_in_last_row(rel.x);
            return ((base + col).min(self.text.len()), false);
        }
        (self.text.len(), false)
    }

    pub(crate) fn on_mouse_down(
        &mut self,
        button: MouseButton,
        position: Point,
        modifiers: Modifiers,
        click_count: usize,
        timers: &mut dyn EditorTimers,
    ) {
        if button == MouseButton::Left
            && self.multi_line
            && let Some(next) = self
                .scrollbar
                .on_mouse_down(position, self.scroll_offset())
        {
            self.reveal = None;
            self.clamp_and_set_scroll_f64(next.x, f64::from(next.y));
            return;
        }

        self.commit_composition();
        self.history.break_coalescing();
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
            if !self.selection.contains(offset) {
                self.move_to_with_affinity(offset, None, aff, false, Some(timers));
            }
            self.pending_context_menu = Some((position, offset));
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
        let mut dirty = false;
        if self.multi_line {
            let outcome = self.scrollbar.on_mouse_move(
                position,
                dragging_left,
                self.scroll_offset(),
                |delay| timers.after(delay, EditorTimer::ScrollbarDrag),
            );
            if let Some(next) = outcome.new_offset {
                self.reveal = None;
                self.clamp_and_set_scroll_f64(next.x, f64::from(next.y));
            }
            if outcome.consumed {
                return outcome.notify_now;
            }
            dirty |= outcome.notify_now;
        }

        if dragging_left && self.selecting && focused && self.text.len() > 0 && self.layout.is_some()
        {
            self.auto_scroll.last_drag_pos = Some(position);
            let (offset, aff) = self.hit_test_point(position);
            self.select_to_with_affinity(offset, aff);
            if self.multi_line {
                let delta = AutoScroll::compute_delta(position.y, self.input_bounds);
                self.auto_scroll.update(delta.map(|d| -d), timers);
            }
            self.request_reveal(self.cursor(), aff, None, false);
            dirty = true;
        }
        dirty
    }

    pub(crate) fn on_mouse_up(
        &mut self,
        button: MouseButton,
        position: Point,
        timers: &mut dyn EditorTimers,
    ) -> Option<Point> {
        let mut open_context_menu = None;

        if self.multi_line
            && let Some(next) = self
                .scrollbar
                .on_mouse_up(position, self.scroll_offset())
        {
            self.reveal = None;
            self.clamp_and_set_scroll_f64(next.x, f64::from(next.y));
        }

        if button == MouseButton::Right
            && let Some((pos, offset)) = self.pending_context_menu.take()
            && self.multi_line
        {
            if !self.selection.contains(offset) {
                self.move_to_with_affinity(offset, None, false, false, Some(timers));
            }
            open_context_menu = Some(pos);
        }
        if self.selection.is_empty() {
            self.selection.reversed = false;
        }
        self.reset_pointer_selection_state();
        open_context_menu
    }

    pub(crate) fn on_mouse_leave(&mut self) -> bool {
        self.multi_line && self.scrollbar.on_mouse_leave()
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
            .layout
            .as_ref()
            .map_or(default_line_height, |l| l.line_height)
            .max(1.0);
        let vp = self.input_bounds.size;
        let dx = if delta_lines.x != 0.0 {
            let space_w = self
                .layout
                .as_ref()
                .map_or(lh * 0.5, |l| l.space_width.max(1.0));
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
        let moved = self.clamp_and_set_scroll_f64(self.scroll_x + dx, self.scroll_y + dy);
        if moved {
            self.scrollbar
                .on_wheel(self.scroll_offset(), Instant::now());
        }
        moved
    }

    pub(crate) fn on_timer(&mut self, kind: EditorTimer, timers: &mut dyn EditorTimers) -> bool {
        match kind {
            EditorTimer::Blink => self.blink.on_timer(timers),
            EditorTimer::AutoScroll => {
                let Some(delta) = self.auto_scroll.delta else {
                    self.auto_scroll.stop();
                    return false;
                };
                self.reveal = None;
                self.clamp_and_set_scroll_f64(self.scroll_x, self.scroll_y + f64::from(delta));
                if let Some(pos) = self.auto_scroll.last_drag_pos {
                    let (offset, aff) = self.hit_test_point(pos);
                    self.select_to_with_affinity(offset, aff);
                }
                true
            }
            EditorTimer::ScrollbarIdle => {
                self.scrollbar.on_idle_timer_fired();
                true
            }
            EditorTimer::ScrollbarDrag => self.scrollbar.on_drag_timer_fired(Instant::now()),
        }
    }

    pub(crate) fn paint(
        &mut self,
        input_bounds: Rect,
        params: EditorPaintParams<'_>,
        ts: &TextSystem,
        renderer: &mut D2DRenderer,
        timers: &mut dyn EditorTimers,
    ) -> EditorPaintOutcome {
        let EditorPaintParams {
            style,
            palette: p,
            window_active,
            focused,
            show_selection,
            scale,
            reduce_motion,
            now,
        } = params;
        let font = &style.font;
        let font_size = style.font_size;
        let text_style = TextStyle::new(font.clone(), font_size);
        let line_height = style.line_height.max(1.0);
        let space_width = ts.measure(" ", &text_style);

        let wrap_width = (self.multi_line && self.soft_wrap)
            .then_some((input_bounds.size.width - RIGHT_MARGIN).max(1.0));

        let reflowing = self.wrap.font() != font
            || self.wrap.font_size() != font_size
            || self.wrap.wrap_width() != wrap_width
            || (self.last_line_height - line_height).abs() > 0.01;
        let anchor = if reflowing && self.reveal.is_none() {
            self.capture_scroll_anchor(self.last_line_height)
        } else {
            None
        };

        self.wrap
            .sync_layout(&self.text, font.clone(), font_size, wrap_width, ts);
        self.last_line_height = line_height;
        self.input_bounds = input_bounds;

        if let Some(anchor) = anchor {
            self.restore_scroll_anchor(anchor, line_height);
        }

        let total_rows = self.wrap.visual_row_count().max(1);
        let content_height_f64 = (total_rows as f64 * f64::from(line_height))
            .max(f64::from(input_bounds.size.height));
        self.content_height_f64 = content_height_f64;

        let reveal_req = self.reveal;
        if let Some(req) = reveal_req
            && self.multi_line
        {
            let target = req.offset.min(self.text.len());
            let disp = self.wrap.offset_to_display(target, req.end_affinity);
            let row_y = disp.row as f64 * f64::from(line_height);
            let vp_h = f64::from(input_bounds.size.height);
            let lh_f64 = f64::from(line_height);
            let v_margin = if req.with_margin {
                f64::from(caret_vertical_margin(input_bounds.size.height, line_height))
            } else {
                0.0
            };
            let prev_y = self.scroll_y;
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
            self.scroll_y = next_y;
        }
        let min_y_f64 = (-content_height_f64 + f64::from(input_bounds.size.height)).min(0.0);
        self.scroll_y = if self.multi_line {
            self.scroll_y.clamp(min_y_f64, 0.0)
        } else {
            0.0
        };

        let placeholder_rope = Rope::from(self.placeholder.as_str());
        let is_empty = self.text.len() == 0;
        let (display_text, text_color) = if is_empty {
            (&placeholder_rope, p.text_secondary)
        } else {
            (&self.text, style.color)
        };

        let (vis_range, visible_lines, visible_top_rel, shape_rows) =
            self.visible_slice(line_height, input_bounds.size.height);
        let start_off = self.text.line_start_offset(vis_range.start);
        let end_off = self.text.line_end_offset(vis_range.end.saturating_sub(1));
        let visible_byte_offsets: Vec<usize> = visible_lines
            .iter()
            .map(|&l| self.text.line_start_offset(l))
            .collect();

        let mut layout = ViewLayout {
            visible_lines,
            visible_byte_offsets,
            visible_top_rel,
            visible_byte_range: start_off..end_off,
            rows: Rc::new(Vec::new()),
            line_height,
            wrap_width,
            space_width,
        };

        let marked = (!is_empty)
            .then_some(self.composition)
            .flatten()
            .map(|c| {
                let len = display_text.len();
                c.start.min(len)..c.end.min(len)
            });
        let (rows, shaped_windowed) = self.shape_visible_lines(
            display_text,
            &layout,
            &shape_rows,
            &text_style,
            input_bounds.size.width,
            ts,
        );
        layout.rows = Rc::new(rows);

        let mut max_line_w = wrap_width.unwrap_or(0.0);
        if !self.multi_line || !self.soft_wrap {
            let longest_row = self.wrap.longest_line();
            let slice = self.text.slice_line(longest_row);
            let key = LongestLineCache {
                revision: self.revision,
                row: longest_row,
                len: slice.len(),
                font: font.clone(),
                font_size,
                wrap_width,
            };
            max_line_w = match &self.longest_line_cache {
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
                    self.longest_line_cache = Some((key, w));
                    w
                }
            };
            for row_layout in layout.rows.iter() {
                max_line_w = max_line_w.max(row_layout.size(line_height).width);
            }
        }

        let head = self.cursor();
        let head_pt = self.viewport_pos_for(&layout, head, self.line_end_affinity);
        if !self.multi_line || !self.soft_wrap {
            max_line_w = max_line_w.max(head_pt.x);
        }

        let content_width = if max_line_w + RIGHT_MARGIN > input_bounds.size.width {
            max_line_w + RIGHT_MARGIN
        } else {
            max_line_w
        };
        let content_size = size(content_width, content_height_f64 as f32);
        self.content_size = content_size;

        let prev_scroll_x = self.scroll_x;
        if reveal_req.is_some() && (!self.multi_line || !self.soft_wrap) {
            let w = input_bounds.size.width;
            if self.scroll_x + head_pt.x > w - RIGHT_MARGIN {
                self.scroll_x = w - RIGHT_MARGIN - head_pt.x;
            } else if self.scroll_x + head_pt.x < 0.0 {
                self.scroll_x = -head_pt.x;
            }
        }
        self.scroll_x = if self.multi_line && self.soft_wrap {
            0.0
        } else {
            clamp_horizontal_scroll(self.scroll_x, content_size.width, input_bounds.size.width)
        };
        self.reveal = None;

        // Vertical scroll is baked into visible_top_rel in f64 so D2D coordinates stay exact on huge files.
        let content_bounds = Rect::new(
            point(input_bounds.origin.x + self.scroll_x, input_bounds.origin.y),
            input_bounds.size,
        );

        let caret_h = if self.caret_height > 0.0 {
            self.caret_height
        } else {
            0.85 * line_height
        };
        let caret_origin = snap_point(
            point(
                content_bounds.origin.x + head_pt.x,
                content_bounds.origin.y + head_pt.y + (line_height - caret_h) * 0.5,
            ),
            scale,
        );
        let caret_w = (system_settings().caret_width_px.max(1) as f32 / scale).max(1.0 / scale);
        let caret_rect = Rect::new(caret_origin, size(caret_w, caret_h));

        let selection_rects = if window_active && show_selection {
            self.selection_rects(&layout, &content_bounds)
        } else {
            Vec::new()
        };

        renderer.push_clip(input_bounds);
        if !selection_rects.is_empty() {
            for rect in selection_rects {
                renderer.paint_quad(fill(rect, p.selection));
            }
        }

        let origin = content_bounds.origin;
        let mut y = layout.visible_top_rel;
        for (vi, row_layout) in layout.rows.iter().enumerate() {
            let line_base = layout.visible_byte_offsets[vi];
            let line_marked = marked.as_ref().and_then(|m| {
                (m.end > line_base).then(|| {
                    m.start.saturating_sub(line_base)..m.end.saturating_sub(line_base)
                })
            });
            row_layout.paint(
                point(origin.x, origin.y + y),
                line_height,
                text_color,
                line_marked,
                renderer,
            );
            y += row_layout.size(line_height).height;
        }

        let show_caret = focused && window_active && self.blink.visible() && !self.caret_hidden;
        if show_caret && (self.selection.is_empty() || self.composition.is_some()) {
            renderer.paint_quad(fill(caret_rect, p.caret));
        }
        renderer.pop_clip();

        self.caret_bounds = Some(caret_rect);
        self.layout = Some(layout);
        self.content_bounds = Some(content_bounds);
        self.last_cursor = Some(head);

        let needs_redraw = shaped_windowed && (self.scroll_x - prev_scroll_x).abs() > 1.0;

        let mut needs_vsync = false;
        if self.multi_line {
            let vp = scrollbar_viewport(input_bounds, content_size, self.paddings, self.soft_wrap);
            needs_vsync = self.scrollbar.prepare(
                vp,
                self.scroll_offset(),
                reduce_motion,
                p,
                now,
                |delay| timers.after(delay, EditorTimer::ScrollbarIdle),
            );
        }

        EditorPaintOutcome {
            needs_redraw,
            needs_vsync,
        }
    }

    pub(crate) fn paint_scrollbar(&self, renderer: &mut D2DRenderer) {
        if self.multi_line {
            self.scrollbar.paint(renderer);
        }
    }

    fn viewport_pos_for(&self, layout: &ViewLayout, offset: usize, aff: bool) -> Point {
        let lh = layout.line_height;
        let row = self.text.offset_to_point(offset).row;
        let line_top_rel =
            (self.wrap.first_visual_row(row) as f64 * f64::from(lh) + self.scroll_y) as f32;
        if let Ok(vi) = layout.visible_lines.binary_search(&row) {
            let local = offset.saturating_sub(layout.visible_byte_offsets[vi]);
            if let Some(p) = layout.rows[vi].position_for_index(local, lh, aff) {
                return point(p.x, line_top_rel + p.y);
            }
        }
        let disp = self.wrap.offset_to_display(offset, aff);
        let fallback_y = (disp.row as f64 * f64::from(lh) + self.scroll_y) as f32;
        point(0.0, fallback_y)
    }

    fn visible_slice(
        &self,
        line_height: f32,
        viewport_height: f32,
    ) -> (Range<usize>, Vec<usize>, f32, Range<usize>) {
        if !self.multi_line {
            return (0..1, vec![0], 0.0, 0..1);
        }
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

    fn shape_visible_lines(
        &self,
        display_text: &Rope,
        layout: &ViewLayout,
        shape_rows: &Range<usize>,
        text_style: &TextStyle,
        viewport_width: f32,
        ts: &TextSystem,
    ) -> (Vec<RowLayout>, bool) {
        if !self.multi_line {
            let text: SharedStr = display_text.to_string().into();
            let shaped = ts.shape_row(text, text_style);
            return (vec![RowLayout::from_shaped(vec![shaped])], false);
        }

        if self.text.len() == 0 {
            let text = display_text.to_string();
            let shaped = text
                .split('\n')
                .map(|part| ts.shape_row(part.to_string().into(), text_style))
                .collect();
            return (vec![RowLayout::from_shaped(shaped)], false);
        }

        let mut result = Vec::with_capacity(layout.visible_lines.len());
        let mut shaped_windowed = false;
        let caret = self.cursor();

        for (vi, &buf_line) in layout.visible_lines.iter().enumerate() {
            let line_slice = display_text.slice_line(buf_line);
            let line_len = line_slice.len();
            let line_byte_start = layout.visible_byte_offsets[vi];
            let wrapped = self.wrap.line(buf_line).expect("wrapped line exists");
            let first_row_of_line = self.wrap.first_visual_row(buf_line);
            let total_sub_rows = wrapped.row_count();

            if !self.soft_wrap
                && total_sub_rows == 1
                && line_len > HUGE_UNWRAPPED_LINE_BYTES
            {
                let line_str: Cow<str> = line_slice.into();
                if can_wrap_with_advances(&line_str) {
                    let min_x = (-self.scroll_x - HORIZONTAL_WINDOW_PAD).max(0.0);
                    let max_x = (-self.scroll_x + viewport_width + HORIZONTAL_WINDOW_PAD).max(min_x);
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
                    let disp = self.wrap.offset_to_display(caret, self.line_end_affinity);
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

            let leading_bytes = wrapped
                .ranges
                .get(slice_start)
                .map_or(0, |r| r.start);

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

    fn selection_rects(&self, layout: &ViewLayout, bounds: &Rect) -> Vec<Rect> {
        let mut sel = self.selection;
        if self.composition.is_some() {
            return Vec::new();
        }
        if sel.is_empty() {
            return Vec::new();
        }
        sel.start = sel.start.min(self.text.len());
        sel.end = sel.end.min(self.text.len());
        let (lo, hi) = (sel.start.min(sel.end), sel.start.max(sel.end));
        let vis = lo.max(layout.visible_byte_range.start)..hi.min(layout.visible_byte_range.end);
        if vis.is_empty() {
            return Vec::new();
        }
        let Some(rects) = range_rects(&vis, layout) else {
            return Vec::new();
        };
        let origin = bounds.origin;
        rects.into_iter().map(|r| r.translate(origin)).collect()
    }
}

fn range_rects(range: &Range<usize>, layout: &ViewLayout) -> Option<Vec<Rect>> {
    let range = range.start.max(layout.visible_byte_range.start)
        ..range.end.min(layout.visible_byte_range.end);
    if range.is_empty() {
        return None;
    }
    let mut line_y = layout.visible_top_rel;
    let mut out = Vec::new();
    for (&line_base, row_layout) in layout.visible_byte_offsets.iter().zip(layout.rows.iter()) {
        let mut off = line_base + row_layout.leading_bytes;
        for (i, row) in row_layout.rows.iter().enumerate() {
            let abs_sub = row_layout.first_sub_row + i;
            let row_y = line_y + abs_sub as f32 * layout.line_height;
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
                    out.push(Rect::from_corners(
                        point(indent + span_l, row_y),
                        point(indent + span_r, row_y + layout.line_height),
                    ));
                }
            }
            if includes_nl {
                let nl_left = indent + row.width();
                out.push(Rect::from_corners(
                    point(nl_left, row_y),
                    point(nl_left + layout.space_width, row_y + layout.line_height),
                ));
            }
            off = end;
        }
        line_y += row_layout.size(layout.line_height).height;
    }
    (!out.is_empty()).then_some(out)
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
        assert_eq!(top, -bottom);
        assert!(top < -12.);

        let mut scroll = AutoScroll {
            delta: Some(20.),
            last_drag_pos: Some(point(1., 2.)),
            ..Default::default()
        };
        scroll.stop();
        assert!(scroll.delta.is_none());
        assert_eq!(scroll.last_drag_pos, None);
    }
}
