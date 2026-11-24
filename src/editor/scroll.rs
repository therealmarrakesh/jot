use std::time::Instant;

use ropey::Rope;

use crate::geom::{Axis, Edges, Point, Rect, Size, size};
use crate::ids::{AppNode, AppUi, EditorSlot};
use crate::platform::Modifiers;
use crate::theme::Palette;

use super::buffer::RopeExt as _;
use super::scrollbar::ScrollbarViewport;
use super::{DocumentView, Editor, EditorTimer, EditorTimers, RIGHT_MARGIN};

const BOTTOM_MARGIN_ROWS: usize = 3;

#[derive(Copy, Clone, Debug)]
pub(super) struct ScrollAnchor {
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

pub(super) fn clamp_horizontal_scroll(offset_x: f32, content_width: f32, viewport_width: f32) -> f32 {
    let min_x = (viewport_width - content_width).min(0.0);
    offset_x.clamp(min_x, 0.0)
}

pub(super) fn reveal_horizontal_scroll(scroll_x: f32, caret_x: f32, viewport_width: f32) -> f32 {
    if scroll_x + caret_x > viewport_width - RIGHT_MARGIN {
        viewport_width - RIGHT_MARGIN - caret_x
    } else if scroll_x + caret_x < 0.0 {
        -caret_x
    } else {
        scroll_x
    }
}

pub(super) fn caret_vertical_margin(viewport_height: f32, line_height: f32) -> f32 {
    let rows = (viewport_height / line_height.max(1.0)) as usize;
    let raw = if rows < BOTTOM_MARGIN_ROWS * 8 {
        line_height
    } else {
        BOTTOM_MARGIN_ROWS as f32 * line_height
    };
    raw.min(rows as f32 * line_height * 0.5)
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
