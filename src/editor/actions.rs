use std::borrow::Cow;
use std::ops::Range;

use ropey::Rope;

use crate::command::Command;
use crate::geom::{Rect, point};
use crate::gfx::TextSystem;
use crate::platform::clipboard;

use super::buffer::{
    Bias, CaretSelection, RopeExt as _, line_range_at, next_grapheme_boundary,
    next_word_end, prev_grapheme_boundary, prev_word_start, snap_grapheme_boundary, word_range_at,
};
use super::history::{EditKind, EditRecord, ReplayPayload};
use super::{Composition, Editor, EditorTimers, MoveDirection};

#[derive(Clone, Copy, PartialEq, Eq)]
enum IndentOp {
    Indent,
    Outdent,
}

impl Editor {
    pub(crate) fn handle_command(
        &mut self,
        command: Command,
        ts: &TextSystem,
        timers: &mut dyn EditorTimers,
    ) -> bool {
        match command {
            Command::Backspace => return self.backspace(ts, timers),
            Command::Delete => self.delete_forward(ts, timers),
            Command::DeletePrevWord => self.delete_prev_word(ts, timers),
            Command::DeleteNextWord => self.delete_next_word(ts, timers),
            Command::Enter => return self.enter(ts, timers),
            Command::Escape => return self.escape(ts),
            Command::Paste => self.paste(ts, timers),
            Command::Cut => self.cut(ts, timers),
            Command::Copy => self.copy(),
            Command::Undo => self.undo(ts, timers),
            Command::Redo => self.redo(ts, timers),
            Command::IndentInline if self.multi_line => {
                return self.apply_indent(IndentOp::Indent, false, ts, timers);
            }
            Command::OutdentInline if self.multi_line => {
                return self.apply_indent(IndentOp::Outdent, false, ts, timers);
            }
            Command::Indent if self.multi_line => {
                return self.apply_indent(IndentOp::Indent, true, ts, timers);
            }
            Command::Outdent if self.multi_line => {
                return self.apply_indent(IndentOp::Outdent, true, ts, timers);
            }
            Command::MoveLeft => return self.move_left(timers),
            Command::MoveRight => return self.move_right(timers),
            Command::SelectLeft => self.select_left(timers),
            Command::SelectRight => self.select_right(timers),
            Command::MoveUp if self.multi_line => self.move_vertical(-1, true, timers),
            Command::MoveDown if self.multi_line => self.move_vertical(1, true, timers),
            Command::SelectUp if self.multi_line => self.select_vertical(-1, timers),
            Command::SelectDown if self.multi_line => self.select_vertical(1, timers),
            Command::MovePageUp if self.multi_line => self.move_page(-1, timers),
            Command::MovePageDown if self.multi_line => self.move_page(1, timers),
            Command::SelectPageUp if self.multi_line => self.select_page(-1, timers),
            Command::SelectPageDown if self.multi_line => self.select_page(1, timers),
            Command::SelectAll => self.select_all(),
            Command::SelectToStartOfLine => {
                let target = self.visual_line_start_for(self.selection.head(), self.line_end_affinity);
                self.select_keyboard_to(target, false, false, timers);
            }
            Command::SelectToEndOfLine => {
                let (target, aff) =
                    self.visual_line_end_for(self.selection.head(), self.line_end_affinity);
                self.select_keyboard_to(target, aff, false, timers);
            }
            Command::SelectPrevWord => {
                let target = self.prev_word_at(self.selection.head());
                self.select_keyboard_to(target, false, false, timers);
            }
            Command::SelectNextWord => {
                let target = self.next_word_at(self.selection.head());
                self.select_keyboard_to(target, false, false, timers);
            }
            Command::MoveHome => {
                let target = self.visual_line_start_for(self.selection.head(), self.line_end_affinity);
                let anchor = self.preferred_x_for(target, false);
                self.place_cursor(target, anchor, false, Some(MoveDirection::Up), timers);
            }
            Command::MoveEnd => {
                let (target, aff) =
                    self.visual_line_end_for(self.selection.head(), self.line_end_affinity);
                let anchor = self.preferred_x_for(target, aff);
                self.place_cursor(target, anchor, aff, Some(MoveDirection::Down), timers);
            }
            Command::MoveToStart => self.move_to(0, None, Some(timers)),
            Command::MoveToEnd => self.move_to(self.text.len(), None, Some(timers)),
            Command::MovePrevWord => {
                let target = self.prev_word_at(self.selection.head());
                let anchor = self.preferred_x_for(target, false);
                self.place_cursor(target, anchor, false, None, timers);
            }
            Command::MoveNextWord => {
                let target = self.next_word_at(self.selection.head());
                let anchor = self.preferred_x_for(target, false);
                self.place_cursor(target, anchor, false, None, timers);
            }
            Command::SelectToStart => self.select_keyboard_to(0, false, false, timers),
            Command::SelectToEnd => {
                let end = self.text.len();
                self.select_keyboard_to(end, false, false, timers);
            }
            _ => return false,
        }
        true
    }

    pub(crate) fn commit_composition(&mut self) {
        if let Some(comp) = self.composition.take() {
            let end = comp.end.min(self.text.len());
            self.set_cursor_to(end);
            self.history.end_group();
            self.debug_assert_invariants();
        }
    }

    pub(crate) fn handle_text_input(
        &mut self,
        text: &str,
        ts: &TextSystem,
        timers: &mut dyn EditorTimers,
    ) {
        self.replace_text_in_range(None, text, None, ts, Some(timers));
    }

    pub(crate) fn handle_ime_composition(&mut self, text: &str, cursor_utf16: usize, ts: &TextSystem) {
        if text.is_empty() && self.composition.is_none() {
            return;
        }
        if text.is_empty() {
            self.replace_and_mark_text_in_range("", None, ts);
        } else {
            let byte_pos = utf16_offset_to_byte(text, cursor_utf16);
            self.replace_and_mark_text_in_range(text, Some(byte_pos..byte_pos), ts);
        }
    }

    pub(crate) fn ime_caret_bounds(&self) -> Option<Rect> {
        let bounds = self.content_bounds?;
        self.bounds_for_range(self.selected_range(), bounds)
            .or(self.caret_bounds)
    }

    pub(crate) fn set_value(&mut self, value: &str, ts: &TextSystem) {
        self.commit_composition();
        self.reset_pointer_selection_state();
        self.history.set_ignoring(true);
        let full = 0..self.text.len();
        self.replace_text_in_range(Some(full), value, Some(EditKind::Atomic), ts, None);
        self.history.set_ignoring(false);

        if self.multi_line {
            self.selection.clear();
        } else {
            let end = self.text.len();
            self.set_cursor_to(end);
        }
        self.scroll_x = 0.0;
        self.scroll_y = 0.0;
        self.reveal = None;
        self.history.clear();
        self.debug_assert_invariants();
    }

    pub(crate) fn replace(&mut self, text: &str, ts: &TextSystem) {
        self.commit_composition();
        self.replace_text_in_range(None, text, Some(EditKind::Atomic), ts, None);
        let end = self.selection.end;
        self.set_cursor_to(end);
        self.debug_assert_invariants();
    }

    pub(crate) fn replace_all_bulk(&mut self, new_rope: Rope, cursor: usize, ts: &TextSystem) {
        self.commit_composition();
        self.reset_pointer_selection_state();
        let before = self.text.clone();
        let sel_before = self.selection;

        self.text = new_rope;
        self.revision = self.revision.wrapping_add(1);
        self.wrap.rebuild_all(&self.text, ts);

        let target = self.cursor_boundary(cursor.min(self.text.len()), Bias::Left);
        self.line_end_affinity = false;
        self.set_cursor_to(target);
        self.history
            .record_snapshot(before, self.text.clone(), sel_before, self.selection);
        self.request_reveal(target, false, None, true);
        self.update_preferred_x();
        self.debug_assert_invariants();
    }

    pub(crate) fn select_all(&mut self) {
        self.commit_composition();
        self.history.break_coalescing();
        self.selection.start = 0;
        self.selection.end = self.text.len();
        self.selection.reversed = false;
        self.selection.preferred_x = None;
        self.line_end_affinity = false;
        self.debug_assert_invariants();
    }

    pub(crate) fn set_selected_range(&mut self, range: Range<usize>) {
        self.commit_composition();
        let end_bias = if range.start == range.end {
            Bias::Left
        } else {
            Bias::Right
        };
        let start = self.cursor_boundary(range.start, Bias::Left);
        let end = self.cursor_boundary(range.end, end_bias);

        self.history.break_coalescing();
        self.line_end_affinity = false;
        self.set_cursor_to(start);
        self.word_anchor = None;
        self.select_to_with_affinity(end, false);
        self.request_reveal(self.cursor(), false, None, true);
        self.update_preferred_x();
        self.debug_assert_invariants();
    }

    pub(crate) fn go_to_line(&mut self, line: usize) {
        self.commit_composition();
        let offset = self.text.line_start_offset(line);
        self.history.break_coalescing();
        let offset = self.cursor_boundary(offset, Bias::Left);
        self.line_end_affinity = false;
        self.set_cursor_to(offset);
        self.request_reveal(offset, false, None, true);
        self.update_preferred_x();
        self.debug_assert_invariants();
    }

    pub(super) fn cursor_boundary(&self, offset: usize, bias: Bias) -> usize {
        snap_grapheme_boundary(&self.text, offset, bias)
    }

    fn prev_boundary(&self, offset: usize) -> usize {
        prev_grapheme_boundary(&self.text, offset)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        next_grapheme_boundary(&self.text, offset)
    }

    fn prev_word_at(&self, offset: usize) -> usize {
        prev_word_start(&self.text, offset)
    }

    fn next_word_at(&self, offset: usize) -> usize {
        next_word_end(&self.text, offset)
    }

    fn visual_line_start_for(&self, offset: usize, end_affinity: bool) -> usize {
        if !self.multi_line {
            return 0;
        }
        let pt = self.text.offset_to_point(offset);
        let line_start = self.text.line_start_offset(pt.row);
        if self.soft_wrap {
            let disp = self.wrap.offset_to_display(offset, end_affinity);
            if let Some((range, _)) = self.wrap.visual_row_range(pt.row, disp.sub_row) {
                return line_start + range.start;
            }
        }
        line_start
    }

    fn visual_line_end_for(&self, offset: usize, end_affinity: bool) -> (usize, bool) {
        if !self.multi_line {
            return (self.text.len(), false);
        }
        let pt = self.text.offset_to_point(offset);
        let line_start = self.text.line_start_offset(pt.row);
        if self.soft_wrap {
            let disp = self.wrap.offset_to_display(offset, end_affinity);
            if let Some((range, soft_end)) = self.wrap.visual_row_range(pt.row, disp.sub_row) {
                return (line_start + range.end, soft_end);
            }
        }
        (self.text.line_end_offset(pt.row), false)
    }

    pub(super) fn set_cursor_to(&mut self, offset: usize) {
        let clamped = offset.min(self.text.len());
        self.selection.start = clamped;
        self.selection.end = clamped;
        self.selection.reversed = false;
    }

    pub(super) fn preferred_x_for(
        &self,
        offset: usize,
        end_affinity: bool,
    ) -> Option<(f32, usize)> {
        let layout = self.layout.as_ref()?;
        let pt = self.text.offset_to_point(offset);
        let row_layout = layout.row_layout(pt.row)?;
        let pos = row_layout.position_for_index(pt.column, layout.line_height, end_affinity)?;
        Some((pos.x, pt.column))
    }

    pub(super) fn update_preferred_x(&mut self) {
        self.selection.preferred_x = self.preferred_x_for(self.cursor(), self.line_end_affinity);
    }

    pub(super) fn move_to(
        &mut self,
        offset: usize,
        direction: Option<MoveDirection>,
        timers: Option<&mut dyn EditorTimers>,
    ) {
        self.move_to_with_affinity(offset, direction, false, true, timers);
    }

    pub(super) fn move_to_with_affinity(
        &mut self,
        offset: usize,
        direction: Option<MoveDirection>,
        end_affinity: bool,
        with_margin: bool,
        timers: Option<&mut dyn EditorTimers>,
    ) {
        self.commit_composition();
        self.history.break_coalescing();
        let offset = self.cursor_boundary(offset, Bias::Left);
        self.line_end_affinity = end_affinity;
        self.set_cursor_to(offset);
        self.request_reveal(offset, end_affinity, direction, with_margin);
        if let Some(timers) = timers {
            self.pause_blink(timers);
        }
        self.update_preferred_x();
        self.debug_assert_invariants();
    }

    fn place_cursor(
        &mut self,
        offset: usize,
        anchor: Option<(f32, usize)>,
        end_affinity: bool,
        direction: Option<MoveDirection>,
        timers: &mut dyn EditorTimers,
    ) {
        self.commit_composition();
        self.history.break_coalescing();
        let offset = self.cursor_boundary(offset, Bias::Left);
        self.selection.place_at(offset, anchor);
        self.line_end_affinity = end_affinity;
        self.request_reveal(self.cursor(), end_affinity, direction, true);
        self.pause_blink(timers);
        self.debug_assert_invariants();
    }

    fn move_left(&mut self, timers: &mut dyn EditorTimers) -> bool {
        self.commit_composition();
        if self.selection.is_empty() && self.cursor() == 0 {
            return false;
        }
        let target = if self.selection.is_empty() {
            self.prev_boundary(self.selection.head())
        } else {
            self.selection.start
        };
        let anchor = self.preferred_x_for(target, false);
        self.place_cursor(target, anchor, false, None, timers);
        true
    }

    fn move_right(&mut self, timers: &mut dyn EditorTimers) -> bool {
        self.commit_composition();
        if self.selection.is_empty() && self.cursor() == self.text.len() {
            return false;
        }
        let target = if self.selection.is_empty() {
            self.next_boundary(self.selection.head())
        } else {
            self.selection.end
        };
        let anchor = self.preferred_x_for(target, false);
        self.place_cursor(target, anchor, false, None, timers);
        true
    }

    fn select_left(&mut self, timers: &mut dyn EditorTimers) {
        let target = self.prev_boundary(self.selection.head());
        self.select_keyboard_to(target, false, false, timers);
    }

    fn select_right(&mut self, timers: &mut dyn EditorTimers) {
        let target = self.next_boundary(self.selection.head());
        self.select_keyboard_to(target, false, false, timers);
    }

    fn vertical_target(
        &self,
        offset: usize,
        anchor: Option<(f32, usize)>,
        end_affinity: bool,
        delta_rows: isize,
    ) -> (usize, bool) {
        let Some(layout) = self.layout.as_ref() else {
            return (offset, end_affinity);
        };
        let mut display = self.wrap.offset_to_display(offset, end_affinity);
        let max_row = self.wrap.visual_row_count().saturating_sub(1);
        display.row = display.row.saturating_add_signed(delta_rows).min(max_row);
        display.col = 0;

        let mut target_offset = self.wrap.display_to_offset(display);
        let anchor = anchor.or_else(|| self.preferred_x_for(offset, end_affinity));
        let mut target_affinity = false;

        if let Some((pref_x, byte_col)) = anchor {
            let mut next_disp = self.wrap.offset_to_display(target_offset, false);
            next_disp.col = 0;
            let next_pt = self.wrap.display_to_point(next_disp);
            let line_start = self.text.line_start_offset(next_pt.row);
            if let Some(row_layout) = layout.row_layout(next_pt.row) {
                if let Some((local_ix, affinity)) = row_layout.closest_index_for_position(
                    point(pref_x, next_disp.sub_row as f32 * layout.line_height),
                    layout.line_height,
                ) {
                    target_offset = line_start + local_ix;
                    target_affinity = affinity;
                }
            } else if let Some((sub_range, soft_end)) =
                self.wrap.visual_row_range(next_pt.row, next_disp.sub_row)
            {
                let max_col = sub_range.len();
                let col = byte_col.min(max_col);
                target_offset = line_start + sub_range.start + col;
                target_affinity = soft_end && col == max_col;
            } else {
                let max_len = self.text.slice_line(next_pt.row).len();
                target_offset = line_start + byte_col.min(max_len);
            }
        }
        (self.cursor_boundary(target_offset, Bias::Left), target_affinity)
    }

    fn move_vertical(
        &mut self,
        delta_rows: isize,
        collapse: bool,
        timers: &mut dyn EditorTimers,
    ) {
        if !self.multi_line {
            return;
        }
        self.commit_composition();
        let direction = if delta_rows < 0 {
            MoveDirection::Up
        } else {
            MoveDirection::Down
        };
        let sel = self.selection;
        let (base_offset, anchor, affinity) = if sel.is_empty() || !collapse {
            (sel.head(), sel.preferred_x, self.line_end_affinity)
        } else if delta_rows < 0 {
            (sel.start, self.preferred_x_for(sel.start, false), false)
        } else {
            let aff = sel.end == self.cursor() && self.line_end_affinity;
            (sel.end, self.preferred_x_for(sel.end, aff), aff)
        };
        let anchor = anchor.or_else(|| self.preferred_x_for(base_offset, affinity));
        let (target, target_affinity) =
            self.vertical_target(base_offset, anchor, affinity, delta_rows);
        self.place_cursor(target, anchor, target_affinity, Some(direction), timers);
    }

    fn select_vertical(&mut self, delta_rows: isize, timers: &mut dyn EditorTimers) {
        if !self.multi_line {
            return;
        }
        self.commit_composition();
        let head = self.selection.head();
        let mut target = self.vertical_target(
            head,
            self.selection.preferred_x,
            self.line_end_affinity,
            delta_rows,
        );
        if self.layout.is_some() {
            let before_row = self.wrap.offset_to_display(head, self.line_end_affinity).row;
            let after_row = self.wrap.offset_to_display(target.0, target.1).row;
            if before_row == after_row {
                target = (if delta_rows < 0 { 0 } else { self.text.len() }, false);
            }
        }
        self.select_keyboard_to(target.0, target.1, true, timers);
    }

    fn page_rows(&self) -> isize {
        let lh = self
            .layout
            .as_ref()
            .map_or(self.last_line_height, |l| l.line_height)
            .max(1.0);
        ((self.input_bounds.size.height / lh).floor() as isize).max(1)
    }

    fn move_page(&mut self, sign: isize, timers: &mut dyn EditorTimers) {
        let rows = self.page_rows();
        self.move_vertical(sign * rows, false, timers);
    }

    fn select_page(&mut self, sign: isize, timers: &mut dyn EditorTimers) {
        let rows = self.page_rows();
        self.select_vertical(sign * rows, timers);
    }

    pub(super) fn select_to_with_affinity(&mut self, offset: usize, end_affinity: bool) {
        self.line_end_affinity = end_affinity;
        let clamped = self.cursor_boundary(offset, Bias::Left);
        self.selection
            .extend_to(clamped, self.word_anchor, self.text.len());
        if self.selection.is_empty() {
            self.update_preferred_x();
        }
        self.debug_assert_invariants();
    }

    fn select_keyboard_to(
        &mut self,
        offset: usize,
        end_affinity: bool,
        keep_column: bool,
        timers: &mut dyn EditorTimers,
    ) {
        self.commit_composition();
        self.pause_blink(timers);
        self.history.break_coalescing();

        let prev_head = self.selection.head();
        let prev_aff = self.line_end_affinity;
        let clamped = self.cursor_boundary(offset, Bias::Left);
        self.selection.extend_to(clamped, None, self.text.len());
        self.line_end_affinity = end_affinity;

        if !keep_column {
            self.selection.preferred_x = self.preferred_x_for(self.selection.head(), end_affinity);
        } else if self.selection.preferred_x.is_none() {
            self.selection.preferred_x = self.preferred_x_for(prev_head, prev_aff);
        }
        self.request_reveal(self.cursor(), end_affinity, None, true);
        self.debug_assert_invariants();
    }

    pub(super) fn select_word_at(&mut self, offset: usize) {
        let Some(range) = word_range_at(&self.text, offset) else {
            return;
        };
        self.history.break_coalescing();
        self.selection.start = range.start;
        self.selection.end = range.end;
        self.selection.reversed = false;
        self.word_anchor = Some(self.selection);
        self.debug_assert_invariants();
    }

    pub(super) fn select_line_at(&mut self, offset: usize) {
        let range = line_range_at(&self.text, offset);
        self.history.break_coalescing();
        self.selection.start = range.start;
        self.selection.end = range.end;
        self.selection.reversed = false;
        self.word_anchor = None;
        self.debug_assert_invariants();
    }

    fn delete_with(
        &mut self,
        collapsed_kind: EditKind,
        collapsed_span: impl FnOnce(&Self, usize) -> Range<usize>,
        ts: &TextSystem,
        timers: &mut dyn EditorTimers,
    ) {
        self.commit_composition();
        let before = self.selection;
        let kind = if before.is_empty() {
            collapsed_kind
        } else {
            EditKind::Atomic
        };
        let span = if before.is_empty() {
            collapsed_span(self, before.head())
        } else {
            before.start..before.end
        };
        let (start, end) = (span.start.min(span.end), span.start.max(span.end));

        self.history.begin_group_with(kind);
        self.history.record_selection(before, before);
        self.selection.start = start;
        self.selection.end = end;

        self.replace_text_in_range(None, "", Some(kind), ts, Some(timers));
        self.history.record_selection(before, self.selection);
        self.history.end_group();
    }

    fn backspace(&mut self, ts: &TextSystem, timers: &mut dyn EditorTimers) -> bool {
        self.commit_composition();
        if self.selection.is_empty() && self.cursor() == 0 {
            return false;
        }
        self.delete_with(
            EditKind::Backspace,
            |ed, off| ed.prev_boundary(off)..off,
            ts,
            timers,
        );
        true
    }

    fn delete_forward(&mut self, ts: &TextSystem, timers: &mut dyn EditorTimers) {
        self.delete_with(
            EditKind::DeleteForward,
            |ed, off| off..ed.next_boundary(off),
            ts,
            timers,
        );
    }

    fn delete_prev_word(&mut self, ts: &TextSystem, timers: &mut dyn EditorTimers) {
        self.delete_with(
            EditKind::Atomic,
            |ed, off| ed.prev_word_at(off)..off,
            ts,
            timers,
        );
    }

    fn delete_next_word(&mut self, ts: &TextSystem, timers: &mut dyn EditorTimers) {
        self.delete_with(
            EditKind::Atomic,
            |ed, off| off..ed.next_word_at(off),
            ts,
            timers,
        );
    }

    fn enter(&mut self, ts: &TextSystem, timers: &mut dyn EditorTimers) -> bool {
        self.commit_composition();
        if self.multi_line {
            self.replace_text_in_range(None, "\n", Some(EditKind::Atomic), ts, Some(timers));
            true
        } else {
            self.history.break_coalescing();
            false
        }
    }

    fn escape(&mut self, ts: &TextSystem) -> bool {
        let Some(comp) = self.composition.take() else {
            return false;
        };
        let len = self.text.len();
        let range = comp.start.min(len)..comp.end.min(len);
        let before_text = self.text.clone();
        let sel_before = self.selection;
        self.reset_pointer_selection_state();
        self.text.replace(range.clone(), "");
        self.wrap.apply_edit(&self.text, &range, 0, ts);
        self.set_cursor_to(range.start);
        self.push_history(
            &before_text,
            &range,
            "",
            Some(EditKind::Atomic),
            sel_before,
            Some(self.selection),
        );
        self.history.end_group();
        self.request_reveal(self.cursor(), false, None, true);
        self.debug_assert_invariants();
        true
    }

    fn copy(&self) {
        if !self.selection.is_empty() {
            let text = self.selected_text().to_string();
            clipboard::write_clipboard_text(&text);
        }
    }

    fn cut(&mut self, ts: &TextSystem, timers: &mut dyn EditorTimers) {
        self.commit_composition();
        if self.selection.is_empty() {
            return;
        }
        let text = self.selected_text().to_string();
        if !clipboard::write_clipboard_text(&text) {
            return;
        }
        self.replace_text_in_range(None, "", Some(EditKind::Atomic), ts, Some(timers));
    }

    fn paste(&mut self, ts: &TextSystem, timers: &mut dyn EditorTimers) {
        self.commit_composition();
        let Some(raw) = clipboard::read_clipboard_text().filter(|s| !s.is_empty()) else {
            return;
        };
        let text = if self.multi_line {
            raw.as_str()
        } else {
            raw.split(['\r', '\n']).next().unwrap_or("")
        };
        if text.is_empty() {
            return;
        }
        self.replace_text_in_range(None, text, Some(EditKind::Atomic), ts, Some(timers));
    }

    fn apply_indent(
        &mut self,
        op: IndentOp,
        block: bool,
        ts: &TextSystem,
        timers: &mut dyn EditorTimers,
    ) -> bool {
        if !self.multi_line {
            return false;
        }
        self.commit_composition();
        self.pause_blink(timers);
        let indent_str = "\t";
        let before = self.selection;

        let spans_multiple_lines = !before.is_empty()
            && self.text.offset_to_point(before.start).row
                != self.text.offset_to_point(before.end).row;
        if !block && op == IndentOp::Indent && !spans_multiple_lines {
            self.replace_text_in_range(
                None,
                indent_str,
                Some(EditKind::Atomic),
                ts,
                None,
            );
            return true;
        }

        let start_row = self.text.offset_to_point(before.start).row;
        let end_pt = self.text.offset_to_point(before.end);
        let end_row = if !before.is_empty() && end_pt.column == 0 && end_pt.row > start_row {
            end_pt.row - 1
        } else {
            end_pt.row
        };

        let mut edits: Vec<(Range<usize>, String)> = Vec::new();
        for row in start_row..=end_row {
            let line_start = self.text.line_start_offset(row);
            match op {
                IndentOp::Indent => edits.push((line_start..line_start, indent_str.to_string())),
                IndentOp::Outdent => {
                    let remove_bytes = self.leading_outdent_bytes(line_start);
                    if remove_bytes > 0 {
                        edits.push((line_start..line_start + remove_bytes, String::new()));
                    }
                }
            }
        }

        if edits.is_empty() {
            return true;
        }

        let remap = |offset: usize| -> usize {
            let mut delta: isize = 0;
            for (r, new_s) in &edits {
                if r.end <= offset {
                    delta += new_s.len() as isize - (r.end - r.start) as isize;
                } else if r.start < offset {
                    let removed = offset - r.start;
                    delta -= removed as isize;
                    break;
                } else {
                    break;
                }
            }
            (offset as isize + delta).max(0) as usize
        };
        let mut next_sel = before;
        next_sel.start = remap(before.start);
        next_sel.end = remap(before.end);
        next_sel.preferred_x = None;

        self.history.begin_group();
        self.apply_range_edits(&edits, Some(EditKind::Atomic), ts);
        let len = self.text.len();
        next_sel.start = next_sel.start.min(len);
        next_sel.end = next_sel.end.min(len);
        self.selection = next_sel;
        self.history.record_selection(before, next_sel);
        self.history.end_group();
        self.request_reveal(self.cursor(), false, None, true);
        self.debug_assert_invariants();
        true
    }

    fn leading_outdent_bytes(&self, line_start: usize) -> usize {
        if self.text.char_at(line_start) == Some('\t') {
            return 1;
        }
        let mut spaces = 0usize;
        for ch in self.text.slice(line_start..).chars().take(8) {
            if ch == ' ' {
                spaces += 1;
            } else {
                break;
            }
        }
        spaces
    }

    fn undo(&mut self, ts: &TextSystem, timers: &mut dyn EditorTimers) {
        self.replay_history(true, ts, timers);
    }

    fn redo(&mut self, ts: &TextSystem, timers: &mut dyn EditorTimers) {
        self.replay_history(false, ts, timers);
    }

    fn replay_history(&mut self, is_undo: bool, ts: &TextSystem, timers: &mut dyn EditorTimers) {
        self.commit_composition();
        let replay = if is_undo {
            self.history.undo()
        } else {
            self.history.redo()
        };
        let Some(replay) = replay else {
            return;
        };
        self.reset_pointer_selection_state();
        self.pause_blink(timers);
        self.history.set_ignoring(true);
        match replay.payload {
            ReplayPayload::Edits(edits) => {
                for edit in &edits {
                    let (range, text) = if is_undo {
                        (Range::from(edit.new_range), edit.old_text.as_str())
                    } else {
                        (Range::from(edit.old_range), edit.new_text.as_str())
                    };
                    self.replace_text_in_range(
                        Some(range),
                        text,
                        Some(EditKind::Atomic),
                        ts,
                        None,
                    );
                }
            }
            ReplayPayload::Snapshot(rope) => {
                self.text = rope;
                self.revision = self.revision.wrapping_add(1);
                self.wrap.rebuild_all(&self.text, ts);
            }
        }
        if let Some(mut sel) = replay.selection {
            let len = self.text.len();
            sel.start = self.cursor_boundary(sel.start.min(len), Bias::Left);
            sel.end = self.cursor_boundary(sel.end.min(len), Bias::Right);
            self.selection = sel;
        }
        self.history.set_ignoring(false);
        self.request_reveal(self.cursor(), false, None, true);
        self.update_preferred_x();
        self.debug_assert_invariants();
    }

    fn collapse_for_kind(
        kind: EditKind,
        mut sel: CaretSelection,
        range: &Range<usize>,
    ) -> CaretSelection {
        match kind {
            EditKind::Backspace => sel.place_at(range.end, None),
            EditKind::DeleteForward => sel.place_at(range.start, None),
            EditKind::Insert | EditKind::Atomic => {}
        }
        sel
    }

    fn push_history(
        &mut self,
        before_text: &Rope,
        range: &Range<usize>,
        inserted: &str,
        requested: Option<EditKind>,
        selection_before: CaretSelection,
        selection_after: Option<CaretSelection>,
    ) -> bool {
        self.revision = self.revision.wrapping_add(1);
        if self.history.is_ignoring() {
            return false;
        }
        let clipped = before_text.clip_offset(range.start, Bias::Left)
            ..before_text.clip_offset(range.end, Bias::Right);
        let old_text = before_text.slice(clipped.clone()).to_string();
        let new_range = clipped.start..clipped.start + inserted.len();

        let kind = requested.unwrap_or_else(|| {
            if clipped.is_empty()
                && old_text.is_empty()
                && !inserted.is_empty()
                && !inserted.contains(['\n', '\r'])
            {
                EditKind::Insert
            } else {
                EditKind::Atomic
            }
        });

        let sel_before = Self::collapse_for_kind(kind, selection_before, &clipped);
        let sel_after = selection_after.unwrap_or_else(|| (new_range.end..new_range.end).into());

        let in_group = self.history.has_open_group();
        let record = EditRecord::new(clipped, &old_text, new_range, inserted);
        let recorded = self.history.record_edit(record, kind);
        if recorded && !in_group {
            self.history.record_selection(sel_before, sel_after);
        }
        recorded
    }

    fn apply_range_edits(
        &mut self,
        edits: &[(Range<usize>, String)],
        requested: Option<EditKind>,
        ts: &TextSystem,
    ) {
        if edits.is_empty() {
            return;
        }
        self.reset_pointer_selection_state();

        let mut descending: Vec<(Range<usize>, Cow<'_, str>)> = edits
            .iter()
            .map(|(r, s)| (r.clone(), self.normalize_text(s.as_str())))
            .collect();
        descending.sort_by_key(|e| std::cmp::Reverse(e.0.start));

        let sel_before = Self::collapse_for_kind(
            requested.unwrap_or(EditKind::Atomic),
            self.selection,
            &(self.selection.start..self.selection.end),
        );
        let grouped = descending.len() > 1;
        if grouped {
            self.history.begin_group();
        }

        let mut recorded = false;
        for (range, new_text) in &descending {
            let before_text = self.text.clone();
            self.text.replace(range.clone(), new_text);
            recorded |= self.push_history(
                &before_text,
                range,
                new_text,
                requested,
                self.selection,
                None,
            );
            self.wrap.apply_edit(&self.text, range, new_text.len(), ts);
        }

        if grouped {
            self.history.end_group();
        }

        let mut ascending = descending;
        ascending.sort_by_key(|e| e.0.start);
        let len = self.text.len();
        let mut shift: isize = 0;
        let mut final_offset = 0;
        for (range, new_text) in &ascending {
            final_offset = ((range.start as isize + shift) as usize + new_text.len()).min(len);
            shift += new_text.len() as isize - (range.end as isize - range.start as isize);
        }
        self.selection.place_at(final_offset, None);
        self.line_end_affinity = false;
        if recorded {
            self.history.record_selection(sel_before, self.selection);
        }
        self.composition = None;
        self.request_reveal(self.cursor(), false, None, true);
        self.update_preferred_x();
        self.debug_assert_invariants();
    }

    fn normalize_text<'a>(&self, text: &'a str) -> Cow<'a, str> {
        if !self.multi_line {
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

    fn accepts_text(&self, candidate: &str) -> bool {
        candidate.is_empty() || self.validate.is_none_or(|f| f(candidate))
    }

    pub(crate) fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        new_text: &str,
        requested: Option<EditKind>,
        ts: &TextSystem,
        timers: Option<&mut dyn EditorTimers>,
    ) {
        let selection_before = self.selection;
        let active_comp = self.composition;
        let ends_composition = active_comp.is_some();

        if let Some(timers) = timers {
            self.pause_blink(timers);
        }
        self.reset_pointer_selection_state();

        let normalized = self.normalize_text(new_text);
        let new_text: &str = &normalized;

        let range = range
            .or_else(|| {
                active_comp.map(|c| {
                    let len = self.text.len();
                    c.start.min(len)..c.end.min(len)
                })
            })
            .unwrap_or_else(|| self.selected_range());

        if self.multi_line {
            self.apply_range_edits(&[(range, new_text.to_string())], requested, ts);
            if ends_composition {
                self.history.end_group();
            }
            return;
        }

        let before_text = self.text.clone();
        self.text.replace(range.clone(), new_text);
        let new_offset = (range.start + new_text.len()).min(self.text.len());

        if !self.history.is_ignoring() {
            let pending = self.text.to_string();
            if !self.accepts_text(&pending) && self.accepts_text(&before_text.to_string()) {
                self.text = before_text;
                return;
            }
        }

        self.push_history(
            &before_text,
            &range,
            new_text,
            requested,
            selection_before,
            None,
        );
        self.wrap.apply_edit(&self.text, &range, new_text.len(), ts);
        self.line_end_affinity = false;
        self.set_cursor_to(new_offset);
        self.composition = None;
        if ends_composition {
            self.history
                .record_selection(selection_before, self.selection);
            self.history.end_group();
        }
        self.request_reveal(self.cursor(), false, None, true);
        self.update_preferred_x();
        self.debug_assert_invariants();
    }

    pub(crate) fn replace_and_mark_text_in_range(
        &mut self,
        new_text: &str,
        new_selected: Option<Range<usize>>,
        ts: &TextSystem,
    ) {
        let selection_before = self.selection;
        let starts_composition = self.composition.is_none();
        if starts_composition {
            self.history.break_coalescing();
            self.history.begin_group();
        }
        self.reset_pointer_selection_state();

        let normalized = self.normalize_text(new_text);
        let new_text: &str = &normalized;

        let range = self
            .composition
            .map(|c| {
                let len = self.text.len();
                c.start.min(len)..c.end.min(len)
            })
            .unwrap_or_else(|| self.selected_range());

        let before_text = self.text.clone();
        self.text.replace(range.clone(), new_text);

        if !self.multi_line {
            let pending = self.text.to_string();
            if !self.accepts_text(&pending) && self.accepts_text(&before_text.to_string()) {
                self.text = before_text;
                if starts_composition {
                    self.history.end_group();
                }
                return;
            }
        }

        self.wrap.apply_edit(&self.text, &range, new_text.len(), ts);

        if new_text.is_empty() {
            self.set_cursor_to(range.start);
            self.composition = None;
        } else {
            let cursor = new_selected
                .as_ref()
                .map_or(new_text.len(), |r| r.end.min(new_text.len()));
            self.composition = Some(Composition {
                start: range.start,
                end: range.start + new_text.len(),
                cursor,
            });
            let sel = new_selected
                .as_ref()
                .map(|r| {
                    range.start + r.start.min(new_text.len())
                        ..range.start + r.end.min(new_text.len())
                })
                .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());
            self.selection.start = sel.start;
            self.selection.end = sel.end;
            self.selection.reversed = false;
        }

        if self.push_history(
            &before_text,
            &range,
            new_text,
            Some(EditKind::Atomic),
            selection_before,
            Some(self.selection),
        ) {
            self.history
                .record_selection(selection_before, self.selection);
        }
        if new_text.is_empty() {
            self.history.end_group();
        }
        self.request_reveal(self.cursor(), false, None, true);
        self.debug_assert_invariants();
    }

    fn bounds_for_range(&self, range: Range<usize>, bounds: Rect) -> Option<Rect> {
        let layout = self.layout.as_ref()?;
        let line_height = layout.line_height;

        let mut start_pos = None;
        let mut end_pos = None;
        let mut y = layout.visible_top_rel;

        for (vi, row_layout) in layout.rows.iter().enumerate() {
            if start_pos.is_some() && end_pos.is_some() {
                break;
            }
            let base = layout.visible_byte_offsets[vi];
            if start_pos.is_none()
                && range.start >= base
                && let Some(p) = row_layout.position_for_index(
                    range.start - base,
                    line_height,
                    false,
                )
            {
                start_pos = Some(p + point(0.0, y));
            }
            if end_pos.is_none()
                && range.end >= base
                && let Some(p) = row_layout.position_for_index(
                    range.end - base,
                    line_height,
                    false,
                )
            {
                end_pos = Some(p + point(0.0, y));
            }
            y += row_layout.size(line_height).height;
        }

        let start_pos = start_pos.or_else(|| {
            let offset = self.last_cursor.unwrap_or_else(|| self.cursor());
            self.position_for_offset(offset)
        })?;
        let mut end_pos = end_pos.unwrap_or(start_pos);
        end_pos.y = start_pos.y;

        Some(Rect::from_corners(
            bounds.origin + start_pos,
            bounds.origin + point(end_pos.x, end_pos.y + line_height),
        ))
    }
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
