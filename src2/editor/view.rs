//! Direct editor view layout, DirectWrite shaping, caret/selection/IME painting, pointer
//! hit-testing, blink timer, drag auto-scroll, and integrated scrollbar coordination.

use std::borrow::Cow;
use std::ops::Range;
use std::rc::Rc;
use std::time::{Duration, Instant};

use ropey::Rope;

use crate::color::{Rgba, rgb, rgba};
use crate::geom::{Edges, Point, Rect, Size, point, size};
use crate::gfx::{D2DRenderer, Font, TextRun, TextSystem, UnderlineStyle, fill};
use crate::platform::{Modifiers, MouseButton, TimerHandle, caret_blink_interval, os};
use crate::scrollbar::ScrollbarViewport;
use crate::theme::Palette;
use crate::types::SharedStr;

use super::buffer::RopeExt as _;
use super::wrap::{RowLayout, VisualRow};
use super::{Editor, EditorTimer, EditorTimers, MoveDirection};

const BOTTOM_MARGIN_ROWS: usize = 3;
const CARET_WIDTH: f32 = 1.0;
pub(super) const RIGHT_MARGIN: f32 = 10.0;
const DEFAULT_WHEEL_LINES: u32 = 3;
const WHEEL_VIEWPORT_SHARE: f32 = 0.15;
const MIN_WHEEL_STEP: f32 = 48.0;

#[derive(Clone, Debug)]
pub(crate) struct EditorPaintStyle {
    pub(crate) font: Font,
    pub(crate) font_size: f32,
    pub(crate) line_height: f32,
    pub(crate) color: Rgba,
}

#[derive(Copy, Clone, Debug, Default)]
pub(crate) struct EditorPaintOutcome {
    pub(crate) needs_redraw: bool,
    pub(crate) needs_vsync: bool,
}

#[derive(Copy, Clone, Debug, Default)]
pub(crate) struct EditorMouseOutcome {
    pub(crate) dirty: bool,
    pub(crate) open_context_menu: Option<Point>,
}

// ---------------------------------------------------------------------------
// Owned caret blink state
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

    pub(crate) fn start(&mut self, timers: &mut dyn EditorTimers) {
        self.visible = false;
        let epoch = self.epoch;
        self.step(epoch, timers);
    }

    pub(crate) fn pause(&mut self, timers: &mut dyn EditorTimers) {
        if self.epoch == 0 {
            return;
        }
        self.paused = true;
        self.visible = true;
        let epoch = self.bump_epoch();
        let Some(delay) = caret_blink_interval() else {
            self.timer.cancel();
            return;
        };
        self.timer = timers.after(delay, EditorTimer::Blink(epoch));
    }

    pub(crate) fn on_timer(&mut self, epoch: usize, timers: &mut dyn EditorTimers) -> bool {
        if epoch != self.epoch {
            return false;
        }
        if self.paused {
            self.paused = false;
        }
        self.step(epoch, timers);
        true
    }

    fn step(&mut self, epoch: usize, timers: &mut dyn EditorTimers) {
        if epoch != self.epoch {
            return;
        }
        if self.paused {
            self.visible = true;
            return;
        }
        let Some(interval) = caret_blink_interval() else {
            self.visible = true;
            self.timer.cancel();
            return;
        };
        self.visible = !self.visible;
        let next_epoch = self.bump_epoch();
        self.timer = timers.after(interval, EditorTimer::Blink(next_epoch));
    }
}

// ---------------------------------------------------------------------------
// Owned drag auto-scroll state
// ---------------------------------------------------------------------------

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

    #[inline]
    pub(crate) fn is_active(&self) -> bool {
        self.delta.is_some()
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

// ---------------------------------------------------------------------------
// Layout snapshots & scroll helpers
// ---------------------------------------------------------------------------

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
    pub(super) visible_top: f32,
    pub(super) visible_byte_range: Range<usize>,
    pub(super) rows: Rc<Vec<RowLayout>>,
    pub(super) line_height: f32,
    pub(super) wrap_width: Option<f32>,
    pub(super) space_width: f32,
    pub(super) caret_bounds: Option<Rect>,
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
    pub(super) synced_offset: Point,
}

impl ScrollbarSnapshot {
    fn compute(
        input_bounds: Rect,
        content_size: Size,
        synced_offset: Point,
        paddings: Edges<f32>,
        soft_wrap: bool,
    ) -> Self {
        Self {
            viewport: ScrollbarViewport {
                bounds: Rect::new(
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

fn clamp_horizontal_scroll(offset_x: f32, content_width: f32, viewport_width: f32) -> f32 {
    let min_x = (viewport_width - content_width).min(0.0);
    offset_x.clamp(min_x, 0.0)
}

fn caret_vertical_margin(viewport_height: f32, line_height: f32) -> f32 {
    let rows = (viewport_height / line_height) as usize;
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
    pub(super) fn position_for_offset(&self, offset: usize) -> Option<Point> {
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

    pub(super) fn clamp_and_set_scroll(&mut self, mut offset: Point) -> bool {
        let min_y = (-self.content_size.height + self.input_bounds.size.height).min(0.0);
        offset.y = if self.multi_line {
            offset.y.clamp(min_y, 0.0)
        } else {
            0.0
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
        let row_y = lh * disp.row as f32;

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

        let min_y = (-self.content_size.height + self.input_bounds.size.height).min(0.0);
        next.x = next.x.min(0.0);
        next.y = next.y.clamp(min_y, 0.0);
        self.pending_scroll = Some(next);
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
        let mut y = layout.visible_top;

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

        let end = layout
            .rows
            .last()
            .zip(layout.visible_byte_offsets.last())
            .map(|(row, base)| (base + row.byte_len()).min(self.text.len()))
            .unwrap_or(self.text.len());
        (end, false)
    }

    // ---- Direct pointer, wheel, and timer events ----

    pub(crate) fn on_mouse_down(
        &mut self,
        button: MouseButton,
        position: Point,
        modifiers: Modifiers,
        click_count: usize,
        timers: &mut dyn EditorTimers,
    ) -> EditorMouseOutcome {
        if button == MouseButton::Left
            && self.multi_line
            && let Some(next) = self.scrollbar.on_mouse_down(position, self.scroll_offset)
        {
            self.clamp_and_set_scroll(next);
            return EditorMouseOutcome {
                dirty: true,
                open_context_menu: None,
            };
        }

        self.history.break_coalescing();
        if self.ime_marked.is_some_and(|m| m.is_empty()) {
            self.ime_marked = None;
        }
        self.selecting = true;
        let (offset, aff) = self.hit_test_point(position);

        if button == MouseButton::Left && click_count >= 3 {
            self.select_line_at(offset);
            return EditorMouseOutcome {
                dirty: true,
                open_context_menu: None,
            };
        }
        if button == MouseButton::Left && click_count == 2 {
            self.select_word_at(offset);
            return EditorMouseOutcome {
                dirty: true,
                open_context_menu: None,
            };
        }
        if button == MouseButton::Right {
            if !self.selection.contains(offset) {
                self.move_to(offset, None, Some(timers));
            }
            self.pending_context_menu = Some((position, offset));
            return EditorMouseOutcome {
                dirty: true,
                open_context_menu: None,
            };
        }
        if modifiers.shift {
            self.select_to_with_affinity(offset, aff);
        } else {
            self.move_to_with_affinity(offset, None, aff, Some(timers));
        }
        EditorMouseOutcome {
            dirty: true,
            open_context_menu: None,
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
                self.scroll_offset,
                |delay| timers.after(delay, EditorTimer::ScrollbarDrag),
            );
            if let Some(next) = outcome.new_offset {
                self.clamp_and_set_scroll(next);
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
            dirty = true;
        }
        dirty
    }

    pub(crate) fn on_mouse_up(
        &mut self,
        button: MouseButton,
        position: Point,
        timers: &mut dyn EditorTimers,
    ) -> EditorMouseOutcome {
        let mut dirty = false;
        let mut open_context_menu = None;

        if self.multi_line
            && let Some(next) = self.scrollbar.on_mouse_up(position, self.scroll_offset)
        {
            self.clamp_and_set_scroll(next);
            dirty = true;
        }

        if button == MouseButton::Right
            && let Some((pos, offset)) = self.pending_context_menu.take()
            && self.multi_line
        {
            if !self.selection.contains(offset) {
                self.move_to(offset, None, Some(timers));
            }
            open_context_menu = Some(pos);
            dirty = true;
        }
        if self.selection.is_empty() {
            self.selection.reversed = false;
        }
        if self.selecting || self.auto_scroll.is_active() {
            dirty = true;
        }
        self.selecting = false;
        self.word_anchor = None;
        self.auto_scroll.stop();
        EditorMouseOutcome {
            dirty,
            open_context_menu,
        }
    }

    pub(crate) fn on_mouse_leave(&mut self) -> bool {
        self.multi_line && self.scrollbar.on_mouse_leave()
    }

    pub(crate) fn on_scroll_wheel(
        &mut self,
        delta_lines: Point,
        modifiers: Modifiers,
        default_line_height: f32,
    ) -> bool {
        if modifiers.ctrl {
            return false;
        }
        let lh = self
            .layout
            .as_ref()
            .map_or(default_line_height, |l| l.line_height);
        let delta = if self.viewport_wheel_step {
            let h_chars = os::wheel_scroll_amount(true)
                .unwrap_or(DEFAULT_WHEEL_LINES)
                .max(1) as f32;
            let v_lines = os::wheel_scroll_amount(false)
                .unwrap_or(DEFAULT_WHEEL_LINES)
                .max(1) as f32;
            let vp = self.input_bounds.size;
            let step = |notches: f32, extent: f32| {
                let notch = (extent * WHEEL_VIEWPORT_SHARE).round().max(MIN_WHEEL_STEP);
                (notches * notch).round().clamp(-extent.floor(), extent.floor())
            };
            point(
                step(delta_lines.x / h_chars, vp.width),
                step(delta_lines.y / v_lines, vp.height),
            )
        } else {
            point(delta_lines.x * lh, delta_lines.y * lh)
        };

        let moved = self.clamp_and_set_scroll(self.scroll_offset + delta);
        if moved {
            self.scrollbar.on_wheel(self.scroll_offset, Instant::now());
        }
        moved
    }

    pub(crate) fn on_timer(&mut self, kind: EditorTimer, timers: &mut dyn EditorTimers) -> bool {
        match kind {
            EditorTimer::Blink(epoch) => self.blink.on_timer(epoch, timers),
            EditorTimer::AutoScroll => {
                let Some(delta) = self.auto_scroll.delta else {
                    self.auto_scroll.stop();
                    return false;
                };
                let next = point(self.scroll_offset.x, self.scroll_offset.y + delta);
                self.clamp_and_set_scroll(next);
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

    // ---- Direct layout + Direct2D painting (zero CustomWidget / EditorHost) ----

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn paint(
        &mut self,
        input_bounds: Rect,
        style: &EditorPaintStyle,
        p: Palette,
        window_active: bool,
        focused: bool,
        show_selection: bool,
        scale: f32,
        reduce_motion: bool,
        now: Instant,
        ts: &TextSystem,
        renderer: &mut D2DRenderer,
        timers: &mut dyn EditorTimers,
    ) -> EditorPaintOutcome {
        let font = &style.font;
        let font_size = style.font_size;
        let line_height = style.line_height;
        let space_width = ts.space_width(font, font_size);

        let wrap_width =
            (self.multi_line && self.soft_wrap).then_some(input_bounds.size.width - RIGHT_MARGIN);
        self.wrap
            .sync_layout(&self.text, font.clone(), font_size, wrap_width, ts);

        let placeholder_rope = Rope::from(self.placeholder.as_str());
        let is_empty = self.text.len() == 0;
        let (display_text, text_color) = if is_empty {
            (&placeholder_rope, rgba(p.text_secondary))
        } else {
            (&self.text, style.color)
        };

        let (vis_range, visible_lines, visible_top, shape_rows) =
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
            color: text_color,
            underline: None,
        };
        let ime_underline = Some(UnderlineStyle {
            thickness: 1.0,
            color: Some(text_color),
        });
        let marked = self.ime_marked.map(|m| m.start..m.end);
        let runs = split_ime_underline(base_run, 0..display_text.len(), marked, ime_underline);
        let rows = self.shape_visible_lines(display_text, &layout, &shape_rows, font_size, &runs, ts);
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
                    let w = ts.measure_line_width(&s, &key.font, font_size);
                    self.longest_line_cache = Some((key, w));
                    w
                }
            };
        }

        let total_rows = self.wrap.visual_row_count();
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
            self.layout_caret_and_scroll(&layout, &mut scrolled_bounds, content_size);
        layout.caret_bounds = Some(caret_bounds);

        let safe_y_min = (-content_size.height + input_bounds.size.height).min(0.0);
        let clamped_y = if self.multi_line {
            synced_scroll.y.clamp(safe_y_min, 0.0)
        } else {
            0.0
        };
        let caret_h = if self.caret_height > 0.0 {
            self.caret_height
        } else {
            caret_bounds.size.height
        };
        let caret_origin = snap_point(
            point(caret_bounds.origin.x, caret_bounds.origin.y + clamped_y),
            scale,
        );
        let caret_rect = Rect::new(caret_origin, size(CARET_WIDTH, caret_h));
        let selection_rects = if window_active && show_selection {
            self.selection_rects(&layout, &scrolled_bounds)
        } else {
            Vec::new()
        };

        // Paint selection, text rows, and caret clipped to `input_bounds`.
        renderer.push_clip(input_bounds);
        if !selection_rects.is_empty() {
            let sel_color = p.selection();
            for rect in selection_rects {
                renderer.paint_quad(fill(rect, sel_color));
            }
        }

        let origin = scrolled_bounds.origin;
        let mut y = layout.visible_top;
        for row_layout in layout.rows.iter() {
            row_layout.paint(point(origin.x, origin.y + y), line_height, renderer);
            y += row_layout.size(line_height).height;
        }

        let show_caret = focused && window_active && self.blink.visible() && !self.caret_hidden;
        if show_caret && self.selection.is_empty() {
            renderer.paint_quad(fill(caret_rect, rgb(p.caret)));
        }
        renderer.pop_clip();

        // Commit state & prepare overlay scrollbar snapshot.
        let geom_changed = self.content_bounds != Some(scrolled_bounds)
            || self.input_bounds != input_bounds
            || self.content_size != content_size
            || self.layout.as_ref().is_none_or(|prev| {
                prev.caret_bounds != layout.caret_bounds || prev.line_height != layout.line_height
            });
        let width_changed = self.input_bounds.size.width != input_bounds.size.width;
        let selection = self.selection;

        self.caret_bounds = Some(caret_rect);
        self.layout = Some(layout);
        self.content_bounds = Some(scrolled_bounds);
        self.last_cursor = Some(self.cursor());
        self.input_bounds = input_bounds;
        let mut needs_redraw = geom_changed;
        if width_changed && let Some(l) = self.layout.as_ref() {
            let w = self.soft_wrap.then_some(l.wrap_width).flatten();
            self.wrap.set_wrap_width(w, ts);
            needs_redraw = true;
        }
        self.last_selection = Some(selection);
        self.content_size = content_size;
        needs_redraw |= self.clamp_and_set_scroll(synced_scroll);
        self.pending_scroll = None;

        let mut needs_vsync = false;
        if self.multi_line {
            let snap = ScrollbarSnapshot::compute(
                input_bounds,
                content_size,
                self.scroll_offset,
                self.paddings,
                self.soft_wrap,
            );
            self.scrollbar_snapshot = Some(snap);
            needs_vsync = self.scrollbar.prepare(
                snap.viewport,
                self.scroll_offset,
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

    // ---- Internal shaping & geometry helpers ----

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
        let scroll_y = self.pending_scroll.map_or(self.scroll_offset.y, |p| p.y);
        let top = (-scroll_y).max(0.0);
        let bottom = top + viewport_height;
        let first_row = ((top / line_height).floor() as usize).min(total_rows - 1);
        let last_row = ((bottom / line_height).ceil() as usize).min(total_rows - 1);
        let shape_rows = first_row.saturating_sub(2)..(last_row + 3).min(total_rows);

        let start_line = self.wrap.visual_row_to_line(first_row);
        let end_line = self.wrap.visual_row_to_line(last_row);
        let visible_top = line_height * self.wrap.first_visual_row(start_line) as f32;
        let line_range = start_line..(end_line + 2).min(total_lines);
        let lines: Vec<usize> = line_range.clone().collect();
        (line_range, lines, visible_top, shape_rows)
    }

    fn shape_visible_lines(
        &self,
        display_text: &Rope,
        layout: &ViewLayout,
        shape_rows: &Range<usize>,
        font_size: f32,
        runs: &[TextRun],
        ts: &TextSystem,
    ) -> Vec<RowLayout> {
        if !self.multi_line {
            let text: SharedStr = display_text.to_string().into();
            let aligned = align_runs(&text, runs);
            let line_runs = aligned.as_deref().unwrap_or(runs);
            let shaped = ts.shape_line(text, font_size, line_runs);
            return vec![RowLayout::from_shaped(vec![shaped])];
        }

        if self.text.len() == 0 {
            let text = display_text.to_string();
            let mut shaped = Vec::new();
            let mut off = 0;
            for part in text.split('\n') {
                let line_runs = slice_runs(runs, off, &(0..part.len()));
                shaped.push(ts.shape_line(part.to_string().into(), font_size, &line_runs));
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
                    let sub_text: SharedStr = line_slice.slice(range.clone()).to_string().into();
                    let sub_runs = align_runs(&sub_text, &sub_runs).unwrap_or(sub_runs);
                    let shaped = ts.shape_line(sub_text, font_size, &sub_runs);
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
                0.0
            };
            result.push(RowLayout::from_rows(rows, hanging_indent));
            run_offset += line_len + 1;
        }
        result
    }

    fn layout_caret_and_scroll(
        &self,
        layout: &ViewLayout,
        bounds: &mut Rect,
        content_size: Size,
    ) -> (Rect, Point) {
        let lh = layout.line_height;
        let mut scroll = self.scroll_offset;
        let v_margin = caret_vertical_margin(bounds.size.height, lh);

        let pos_for = |row: usize, offset: usize, aff: bool| -> Point {
            let origin = point(0.0, lh * self.wrap.first_visual_row(row) as f32);
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
            } else if scroll.x + head_pt.x < 0.0 {
                scroll.x = -head_pt.x;
            }

            if !auto_scrolling {
                if scroll.y + head_pt.y > bounds.size.height - v_margin {
                    scroll.y = bounds.size.height - v_margin - head_pt.y;
                } else if scroll.y + head_pt.y < v_margin {
                    scroll.y = (v_margin - head_pt.y).min(0.0);
                }
            }

            if self.selection.reversed {
                if scroll.x + start_pt.x < 0.0 {
                    scroll.x = -start_pt.x;
                }
                if !auto_scrolling && scroll.y + start_pt.y < 0.0 {
                    scroll.y = -start_pt.y;
                }
            } else {
                if scroll.x + end_pt.x <= 0.0 {
                    scroll.x = -end_pt.x;
                }
                if !auto_scrolling && scroll.y + end_pt.y <= 0.0 {
                    scroll.y = -end_pt.y;
                }
            }
        }

        if let Some(pending) = self.pending_scroll {
            scroll = pending;
        }
        scroll.x = clamp_horizontal_scroll(scroll.x, content_size.width, bounds.size.width);

        let caret_h = 0.85 * lh;
        let caret_bounds = Rect::new(
            point(
                bounds.left() + head_pt.x + scroll.x,
                bounds.top() + head_pt.y + (lh - caret_h) * 0.5,
            ),
            size(CARET_WIDTH, caret_h),
        );
        bounds.origin += scroll;
        (caret_bounds, scroll)
    }

    fn selection_rects(&self, layout: &ViewLayout, bounds: &Rect) -> Vec<Rect> {
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
        let Some(rects) = range_rects(&vis, layout) else {
            return Vec::new();
        };
        let origin = bounds.origin;
        rects
            .into_iter()
            .map(|r| r.translate(origin))
            .collect()
    }
}

fn range_rects(range: &Range<usize>, layout: &ViewLayout) -> Option<Vec<Rect>> {
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
                out.push(Rect::from_corners(
                    point(left, y),
                    point(right, y + layout.line_height),
                ));
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
        assert!(!scroll.is_active());
        assert_eq!(scroll.last_drag_pos, None);
    }
}
