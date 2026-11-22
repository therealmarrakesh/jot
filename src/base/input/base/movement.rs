use gpui::{Context, Pixels, Point, Window};

use crate::base::input::rope_ext::Bias;
use crate::base::input::{
    EditorState, MoveDown, MoveEnd, MoveHome, MoveLeft, MovePageDown, MovePageUp, MoveRight,
    MoveToEnd, MoveToNextWord, MoveToPreviousWord, MoveToStart, MoveUp, RopeExt as _,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum MoveDirection {
    Up,
    Down,
}

impl EditorState {
    /// Compute the column anchor for `offset`.
    pub(super) fn preferred_column_for(&self, offset: usize) -> Option<(Pixels, usize)> {
        self.preferred_column_for_with_affinity(offset, false)
    }

    /// Like [`Self::preferred_column_for`], resolving soft-wrap boundaries with affinity.
    pub(super) fn preferred_column_for_with_affinity(
        &self,
        offset: usize,
        line_end_affinity: bool,
    ) -> Option<(Pixels, usize)> {
        let last_layout = self.last_layout.as_ref()?;
        let point = self.text.offset_to_point(offset);
        let line = last_layout.line(point.row)?;
        let pos = line.position_for_index(point.column, last_layout, line_end_affinity)?;
        Some((pos.x, point.column))
    }

    pub(super) fn update_preferred_column(&mut self) {
        let anchor =
            self.preferred_column_for_with_affinity(self.cursor(), self.cursor_line_end_affinity);
        self.selection.column_anchor = anchor;
    }

    /// Move the cursor to the given UTF-8 byte offset.
    pub(crate) fn move_to(
        &mut self,
        offset: usize,
        direction: Option<MoveDirection>,
        cx: &mut Context<Self>,
    ) {
        self.move_to_with_affinity(offset, direction, false, cx);
    }

    /// Like [`Self::move_to`], carrying the caret's soft-wrap line-end affinity.
    pub(crate) fn move_to_with_affinity(
        &mut self,
        offset: usize,
        direction: Option<MoveDirection>,
        line_end_affinity: bool,
        cx: &mut Context<Self>,
    ) {
        self.undo_manager.break_transaction_coalescing();
        let offset = self.cursor_boundary(offset, Bias::Left);
        self.cursor_line_end_affinity = line_end_affinity;
        self.set_cursor_to(offset);
        self.scroll_to(offset, direction, cx);
        self.pause_blink_cursor(cx);
        self.update_preferred_column();
        cx.notify();
    }

    /// Compute the target offset and line-end affinity when moving vertically by `move_lines`.
    pub(super) fn vertical_target(
        &self,
        offset: usize,
        column_anchor: Option<(Pixels, usize)>,
        line_end_affinity: bool,
        move_lines: isize,
    ) -> (usize, bool) {
        let Some(last_layout) = &self.last_layout else {
            return (offset, line_end_affinity);
        };

        let mut display_point = self
            .text_wrapper
            .offset_to_display_point_with_affinity(offset, line_end_affinity);

        let max_row = self.text_wrapper.len().saturating_sub(1);
        display_point.row = display_point
            .row
            .saturating_add_signed(move_lines)
            .min(max_row);
        display_point.column = 0;
        let mut new_offset = self.text_wrapper.display_point_to_offset(display_point);

        let column_anchor = column_anchor
            .or_else(|| self.preferred_column_for_with_affinity(offset, line_end_affinity));
        let mut new_affinity = false;
        if let Some((preferred_x, column)) = column_anchor {
            let mut next_display_point = self.text_wrapper.offset_to_display_point(new_offset);
            next_display_point.column = 0;
            let next_point = self.text_wrapper.display_point_to_point(next_display_point);
            let line_start_offset = self.text.line_start_offset(next_point.row);

            if let Some(line) = last_layout.line(next_point.row) {
                if let Some((x, line_end_affinity)) = line.closest_index_for_position(
                    Point {
                        x: preferred_x,
                        y: next_display_point.local_row * last_layout.line_height,
                    },
                    last_layout,
                ) {
                    new_offset = line_start_offset + x;
                    new_affinity = line_end_affinity;
                }
            } else {
                let max_line_len = self.text.slice_line(next_point.row).len();
                new_offset = line_start_offset + column.min(max_line_len);
            }
        }

        (new_offset, new_affinity)
    }

    /// Extend to the document edge when there is no further visual row.
    pub(super) fn vertical_selection_target(
        &self,
        offset: usize,
        column_anchor: Option<(Pixels, usize)>,
        line_end_affinity: bool,
        move_lines: isize,
    ) -> (usize, bool) {
        let target = self.vertical_target(offset, column_anchor, line_end_affinity, move_lines);
        if self.last_layout.is_some() {
            let row = |offset, affinity| {
                self.text_wrapper
                    .offset_to_display_point_with_affinity(offset, affinity)
                    .row
            };
            if row(offset, line_end_affinity) == row(target.0, target.1) {
                return (if move_lines < 0 { 0 } else { self.text.len() }, false);
            }
        }
        target
    }

    fn place_cursor(
        &mut self,
        offset: usize,
        anchor: Option<(Pixels, usize)>,
        line_end_affinity: bool,
        direction: Option<MoveDirection>,
        cx: &mut Context<Self>,
    ) {
        self.undo_manager.break_transaction_coalescing();
        let offset = self.cursor_boundary(offset, Bias::Left);
        self.selection.place_at(offset, anchor);
        self.cursor_line_end_affinity = line_end_affinity;
        self.scroll_to(self.cursor(), direction, cx);
        self.pause_blink_cursor(cx);
        cx.notify();
    }

    fn move_vertical(&mut self, move_lines: isize, collapse: bool, cx: &mut Context<Self>) {
        if self.is_single_line() {
            return;
        }
        self.pause_blink_cursor(cx);

        let direction = if move_lines < 0 {
            MoveDirection::Up
        } else {
            MoveDirection::Down
        };

        let sel = self.selection;
        let (effective, anchor, affinity) = if sel.is_empty() || !collapse {
            (
                sel.cursor_offset(),
                sel.column_anchor,
                self.cursor_line_end_affinity,
            )
        } else if move_lines < 0 {
            let e = self.previous_boundary(sel.start.saturating_sub(1));
            (e, self.preferred_column_for(e), false)
        } else {
            let e = self.next_boundary(sel.end.saturating_sub(1));
            let affinity = e == self.cursor() && self.cursor_line_end_affinity;
            (
                e,
                self.preferred_column_for_with_affinity(e, affinity),
                affinity,
            )
        };
        let anchor = anchor.or_else(|| self.preferred_column_for_with_affinity(effective, affinity));
        let (offset, affinity) = self.vertical_target(effective, anchor, affinity, move_lines);
        self.place_cursor(offset, anchor, affinity, Some(direction), cx);
    }

    pub(super) fn left(&mut self, _: &MoveLeft, _: &mut Window, cx: &mut Context<Self>) {
        if self.selection.is_empty() && self.cursor() == 0 {
            cx.propagate();
            return;
        }

        let offset = if self.selection.is_empty() {
            self.previous_boundary(self.selection.cursor_offset())
        } else {
            self.selection.start
        };
        let anchor = self.preferred_column_for(offset);
        self.place_cursor(offset, anchor, false, None, cx);
    }

    pub(super) fn right(&mut self, _: &MoveRight, _: &mut Window, cx: &mut Context<Self>) {
        if self.selection.is_empty() && self.cursor() == self.text.len() {
            cx.propagate();
            return;
        }

        let offset = if self.selection.is_empty() {
            self.next_boundary(self.selection.cursor_offset())
        } else {
            self.selection.end
        };
        let anchor = self.preferred_column_for(offset);
        self.place_cursor(offset, anchor, false, None, cx);
    }

    pub(super) fn up(&mut self, _: &MoveUp, _: &mut Window, cx: &mut Context<Self>) {
        self.move_vertical(-1, true, cx);
    }

    pub(super) fn down(&mut self, _: &MoveDown, _: &mut Window, cx: &mut Context<Self>) {
        self.move_vertical(1, true, cx);
    }

    pub(super) fn page_up(&mut self, _: &MovePageUp, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_single_line() {
            return;
        }
        let Some(last_layout) = &self.last_layout else {
            return;
        };
        let display_lines = (self.input_bounds.size.height / last_layout.line_height) as isize;
        self.move_vertical(-display_lines, false, cx);
    }

    pub(super) fn page_down(&mut self, _: &MovePageDown, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_single_line() {
            return;
        }
        let Some(last_layout) = &self.last_layout else {
            return;
        };
        let display_lines = (self.input_bounds.size.height / last_layout.line_height) as isize;
        self.move_vertical(display_lines, false, cx);
    }

    pub(super) fn home(&mut self, _: &MoveHome, _: &mut Window, cx: &mut Context<Self>) {
        let offset = self.start_of_line_at(self.selection.cursor_offset());
        let anchor = self.preferred_column_for(offset);
        self.place_cursor(offset, anchor, false, Some(MoveDirection::Up), cx);
    }

    pub(super) fn end(&mut self, _: &MoveEnd, _: &mut Window, cx: &mut Context<Self>) {
        let offset = self.end_of_line_at(self.selection.cursor_offset());
        let anchor = self.preferred_column_for(offset);
        self.place_cursor(offset, anchor, true, Some(MoveDirection::Down), cx);
    }

    pub(super) fn move_to_start(
        &mut self,
        _: &MoveToStart,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_to(0, None, cx);
    }

    pub(super) fn move_to_end(&mut self, _: &MoveToEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.text.len(), None, cx);
    }

    pub(super) fn move_to_previous_word(
        &mut self,
        _: &MoveToPreviousWord,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let offset = self.previous_start_of_word_at(self.selection.cursor_offset());
        let anchor = self.preferred_column_for(offset);
        self.place_cursor(offset, anchor, false, None, cx);
    }

    pub(super) fn move_to_next_word(
        &mut self,
        _: &MoveToNextWord,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let offset = self.next_end_of_word_at(self.selection.cursor_offset());
        let anchor = self.preferred_column_for(offset);
        self.place_cursor(offset, anchor, false, None, cx);
    }
}
