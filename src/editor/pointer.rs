use crate::ui::{
    Context, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollDelta,
    ScrollWheelEvent, Window, point, px,
};

use super::auto_scroll::AutoScroll;
use super::element::{RIGHT_MARGIN, clamp_horizontal_scroll_offset};
use super::movement::MoveDirection;
use super::{EditorState, RopeExt as _};

const DEFAULT_WHEEL_SCROLL: u32 = 3;
const WHEEL_STEP_FRACTION: f32 = 0.15;
const MIN_WHEEL_STEP: f32 = 48.;

fn wheel_scroll_amount(horizontal: bool) -> u32 {
    crate::win32::wheel_scroll_amount(horizontal).unwrap_or(DEFAULT_WHEEL_SCROLL)
}

impl EditorState {
    pub(crate) fn handle_right_click_menu(
        &mut self,
        position: Point<Pixels>,
        offset: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.selection.contains(offset) {
            self.move_to(offset, None, cx);
        }

        if let Some(handler) = self.context_menu_handler.clone() {
            let input = cx.weak_entity();
            window.defer(cx, move |window, cx| {
                if input.upgrade().is_none() {
                    return;
                }
                let previous_focus = window.focused(cx);
                handler(position, window, cx);
                input
                    .update(cx, |this, cx| {
                        if window.focused(cx) == previous_focus
                            && !this.focus_handle.is_focused(window)
                            && this.has_selection_focus(window, cx)
                        {
                            return;
                        }
                        let menu_focus = window.focused(cx).filter(|focus| {
                            Some(focus) != previous_focus.as_ref() && *focus != this.focus_handle
                        });
                        this.set_selection_focus(menu_focus, cx);
                    })
                    .ok();
            });
        }
    }

    pub(super) fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if window.default_prevented() {
            return;
        }
        self.undo_manager.break_transaction_coalescing();

        if let Some(ime_marked_range) = &self.ime_marked_range
            && ime_marked_range.is_empty()
        {
            self.ime_marked_range = None;
        }

        self.selecting = true;
        let (offset, line_end_affinity) = self.resolve_mouse_position(event.position);

        if event.button == MouseButton::Left && event.click_count >= 3 {
            self.select_line(offset, window, cx);
            return;
        }

        if event.button == MouseButton::Left && event.click_count == 2 {
            self.select_word(offset, window, cx);
            return;
        }

        if event.button == MouseButton::Right {
            if !self.selection.contains(offset) {
                self.move_to(offset, None, cx);
            }
            self.pending_context_menu = Some((event.position, offset));
            return;
        }

        if event.modifiers.shift {
            self.select_to_with_affinity(offset, line_end_affinity, cx);
        } else {
            self.move_to_with_affinity(offset, None, line_end_affinity, cx);
        }
    }

    pub(super) fn on_mouse_up(
        &mut self,
        event: &MouseUpEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.button == MouseButton::Right
            && let Some((position, offset)) = self.pending_context_menu.take()
        {
            self.handle_right_click_menu(position, offset, window, cx);
        }
        if self.selection.is_empty() {
            self.selection.reversed = false;
        }
        self.selecting = false;
        self.selected_word_range = None;
        self.auto_scroll.stop();
    }

    pub(super) fn on_scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.modifiers.control {
            return;
        }
        let line_height = self
            .last_layout
            .as_ref()
            .map(|layout| layout.line_height)
            .unwrap_or(window.line_height());
        let delta = match event.delta {
            ScrollDelta::Lines(lines) if self.viewport_wheel_step => {
                let chars_per_notch = wheel_scroll_amount(true).max(1) as f32;
                let lines_per_notch = wheel_scroll_amount(false).max(1) as f32;
                let size = self.input_bounds.size;
                let step = |notches: f32, extent: Pixels| {
                    let extent = extent.as_f32();
                    let notch = (extent * WHEEL_STEP_FRACTION).round().max(MIN_WHEEL_STEP);
                    px((notches * notch)
                        .round()
                        .clamp(-extent.floor(), extent.floor()))
                };
                point(
                    step(lines.x / chars_per_notch, size.width),
                    step(lines.y / lines_per_notch, size.height),
                )
            }
            _ => event.delta.pixel_delta(line_height),
        };

        let old_offset = self.scroll_handle.offset();
        self.update_scroll_offset(Some(old_offset + delta), cx);

        if self.scroll_handle.offset() != old_offset {
            cx.stop_propagation();
        }
    }

    pub(super) fn update_scroll_offset(
        &mut self,
        offset: Option<Point<Pixels>>,
        cx: &mut Context<Self>,
    ) {
        let mut offset = offset.unwrap_or(self.scroll_handle.offset());
        let safe_y_range =
            (-self.scroll_size.height + self.input_bounds.size.height).min(px(0.0))..px(0.);

        offset.y = if self.is_single_line() {
            px(0.)
        } else {
            offset.y.clamp(safe_y_range.start, safe_y_range.end)
        };
        offset.x = clamp_horizontal_scroll_offset(
            offset.x,
            self.scroll_size.width,
            self.input_bounds.size.width,
        );
        if self.scroll_handle.offset() != offset {
            self.scroll_handle.set_offset(offset);
            cx.notify();
        }
    }

    pub(crate) fn scroll_to(
        &mut self,
        offset: usize,
        direction: Option<MoveDirection>,
        cx: &mut Context<Self>,
    ) {
        let Some(last_layout) = self.last_layout.as_ref() else {
            return;
        };
        let Some(bounds) = self.last_bounds.as_ref() else {
            return;
        };

        let mut scroll_offset = self.scroll_handle.offset();
        let was_offset = scroll_offset;
        let line_height = last_layout.line_height;

        let point = self.text.offset_to_point(offset);
        let row = point.row;
        let display_pos = self.text_wrapper.point_to_display_point(point);
        let row_offset_y = line_height * display_pos.row;

        if let Ok(vi) = last_layout.visible_buffer_lines.binary_search(&row) {
            let line = &last_layout.lines[vi];
            let local_offset = offset.saturating_sub(last_layout.visible_line_byte_offsets[vi]);
            if let Some(pos) = line.position_for_index(local_offset, last_layout, false) {
                let bounds_width = bounds.size.width;
                let col_offset_x = pos.x;
                if col_offset_x - RIGHT_MARGIN < -scroll_offset.x {
                    scroll_offset.x = -col_offset_x + RIGHT_MARGIN;
                } else if col_offset_x + RIGHT_MARGIN > -scroll_offset.x + bounds_width {
                    scroll_offset.x = -(col_offset_x - bounds_width + RIGHT_MARGIN);
                }
            }
        }

        let edge_height = line_height;
        if row_offset_y - edge_height + line_height < -scroll_offset.y {
            scroll_offset.y = -row_offset_y + edge_height - line_height;
        } else if row_offset_y + edge_height > -scroll_offset.y + bounds.size.height {
            scroll_offset.y = -(row_offset_y - bounds.size.height + edge_height);
        }

        if direction == Some(MoveDirection::Up) {
            scroll_offset.y = scroll_offset.y.max(was_offset.y);
        } else if direction == Some(MoveDirection::Down) {
            scroll_offset.y = scroll_offset.y.min(was_offset.y);
        }

        let safe_y_min = (-self.scroll_size.height + self.input_bounds.size.height).min(px(0.));
        scroll_offset.x = scroll_offset.x.min(px(0.));
        scroll_offset.y = scroll_offset.y.clamp(safe_y_min, px(0.));
        self.deferred_scroll_offset = Some(scroll_offset);
        cx.notify();
    }

    pub(super) fn resolve_mouse_position(&self, position: Point<Pixels>) -> (usize, bool) {
        if self.text.len() == 0 {
            return (0, false);
        }
        let (Some(bounds), Some(last_layout)) =
            (self.last_bounds.as_ref(), self.last_layout.as_ref())
        else {
            return (0, false);
        };

        let line_height = last_layout.line_height;
        let inner_position = position - bounds.origin;
        let mut y_offset = last_layout.visible_top;

        for (vi, line_layout) in last_layout.lines.iter().enumerate() {
            let line_start_offset = last_layout.visible_line_byte_offsets[vi];
            let pos = inner_position - point(px(0.), y_offset);

            if self.is_single_line() {
                let local_index = line_layout.closest_index_for_x(pos.x);
                return (self.resolve_index(line_start_offset + local_index), false);
            }

            if let Some((local_index, line_end_affinity)) =
                line_layout.closest_index_for_position(pos, last_layout)
            {
                return (
                    self.resolve_index(line_start_offset + local_index),
                    line_end_affinity,
                );
            } else if pos.y < px(0.) {
                return (self.resolve_index(line_start_offset), false);
            }

            y_offset += line_layout.size(line_height).height;
        }

        let last_offset = last_layout
            .lines
            .last()
            .zip(last_layout.visible_line_byte_offsets.last())
            .map(|(line_layout, line_start)| self.resolve_index(line_start + line_layout.len()))
            .unwrap_or(self.text.len());
        (last_offset, false)
    }

    fn resolve_index(&self, index: usize) -> usize {
        index.min(self.text.len())
    }

    pub(super) fn on_drag_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.text.len() == 0
            || self.last_layout.is_none()
            || !self.focus_handle.is_focused(window)
            || !self.selecting
        {
            return;
        }

        self.auto_scroll.last_drag_position = Some(event.position);
        let (offset, line_end_affinity) = self.resolve_mouse_position(event.position);
        self.select_to_with_affinity(offset, line_end_affinity, cx);

        if self.is_multi_line() {
            let delta = AutoScroll::compute_delta(event.position.y, self.input_bounds);
            let scroll_delta = delta.map(|d| -d);
            self.auto_scroll.set(scroll_delta, cx, |delta, state, cx| {
                let current = state.scroll_handle.offset();
                state.update_scroll_offset(Some(point(current.x, current.y + delta)), cx);
                if let Some(pos) = state.auto_scroll.last_drag_position {
                    let (offset, line_end_affinity) = state.resolve_mouse_position(pos);
                    state.select_to_with_affinity(offset, line_end_affinity, cx);
                }
            });
        }
    }
}
