//! Clean-room editing, cursor motion, selection, clipboard, indentation, undo/redo, and IME
//! operations on `Editor`.

use std::borrow::Cow;
use std::ops::Range;

use ropey::Rope;

use crate::ui::{App, Bounds, Command, Pixels, UTF16Selection, Window, point, px};

use super::buffer::{
    Bias, CaretSelection, Position, RopeExt as _, line_range_at, next_word_end, prev_word_start,
    word_range_at,
};
use super::history::{EditKind, EditRecord};
use super::{Editor, EditorHost, MoveDirection};

#[derive(Clone, Copy, PartialEq, Eq)]
enum IndentOp {
    Indent,
    Outdent,
}

impl Editor {
    // ---- Command dispatch ----

    pub(crate) fn handle_command(
        &mut self,
        command: Command,
        host: &EditorHost,
        cx: &mut App,
    ) -> bool {
        match command {
            Command::Backspace => return self.backspace(host, cx),
            Command::Delete => self.delete_forward(host, cx),
            Command::DeletePrevWord => self.delete_prev_word(host, cx),
            Command::DeleteNextWord => self.delete_next_word(host, cx),
            Command::Enter => return self.enter(host, cx),
            Command::Escape => return self.escape(),
            Command::Paste => self.paste(host, cx),
            Command::Cut => self.cut(host, cx),
            Command::Copy => self.copy(cx),
            Command::Undo => self.undo(host, cx),
            Command::Redo => self.redo(host, cx),
            Command::IndentInline if self.multi_line => {
                return self.apply_indent(IndentOp::Indent, false, host, cx);
            }
            Command::OutdentInline if self.multi_line => {
                return self.apply_indent(IndentOp::Outdent, false, host, cx);
            }
            Command::Indent if self.multi_line => {
                return self.apply_indent(IndentOp::Indent, true, host, cx);
            }
            Command::Outdent if self.multi_line => {
                return self.apply_indent(IndentOp::Outdent, true, host, cx);
            }
            Command::MoveLeft => return self.move_left(host, cx),
            Command::MoveRight => return self.move_right(host, cx),
            Command::SelectLeft => self.select_left(host, cx),
            Command::SelectRight => self.select_right(host, cx),
            Command::MoveUp if self.multi_line => self.move_vertical(-1, true, host, cx),
            Command::MoveDown if self.multi_line => self.move_vertical(1, true, host, cx),
            Command::SelectUp if self.multi_line => self.select_vertical(-1, host, cx),
            Command::SelectDown if self.multi_line => self.select_vertical(1, host, cx),
            Command::MovePageUp if self.multi_line => self.move_page(-1, host, cx),
            Command::MovePageDown if self.multi_line => self.move_page(1, host, cx),
            Command::SelectAll => self.select_all(),
            Command::SelectToStartOfLine => {
                let target = self.line_start_for(self.selection.head());
                self.select_keyboard_to(target, false, false, host, cx);
            }
            Command::SelectToEndOfLine => {
                let target = self.line_end_for(self.selection.head());
                self.select_keyboard_to(target, true, false, host, cx);
            }
            Command::SelectPrevWord => {
                let target = self.prev_word_at(self.selection.head());
                self.select_keyboard_to(target, false, false, host, cx);
            }
            Command::SelectNextWord => {
                let target = self.next_word_at(self.selection.head());
                self.select_keyboard_to(target, false, false, host, cx);
            }
            Command::MoveHome => {
                let target = self.line_start_for(self.selection.head());
                let anchor = self.preferred_x_for(target, false);
                self.place_cursor(target, anchor, false, Some(MoveDirection::Up), host, cx);
            }
            Command::MoveEnd => {
                let target = self.line_end_for(self.selection.head());
                let anchor = self.preferred_x_for(target, false);
                self.place_cursor(target, anchor, true, Some(MoveDirection::Down), host, cx);
            }
            Command::MoveToStart => self.move_to(0, None, Some(host), cx),
            Command::MoveToEnd => self.move_to(self.text.len(), None, Some(host), cx),
            Command::MovePrevWord => {
                let target = self.prev_word_at(self.selection.head());
                let anchor = self.preferred_x_for(target, false);
                self.place_cursor(target, anchor, false, None, host, cx);
            }
            Command::MoveNextWord => {
                let target = self.next_word_at(self.selection.head());
                let anchor = self.preferred_x_for(target, false);
                self.place_cursor(target, anchor, false, None, host, cx);
            }
            Command::SelectToStart => self.select_keyboard_to(0, false, false, host, cx),
            Command::SelectToEnd => {
                let end = self.text.len();
                self.select_keyboard_to(end, false, false, host, cx);
            }
            _ => return false,
        }
        true
    }

    // ---- Public programmatic edits & selection ----

    pub(crate) fn set_value(&mut self, value: &str, cx: &mut App) {
        self.history.set_ignoring(true);
        let full_utf16 = 0..self.text.len_utf16();
        self.history.set_next_kind(EditKind::Atomic);
        self.replace_text_in_range(Some(full_utf16), value, None, cx);
        self.history.set_ignoring(false);

        if self.multi_line {
            self.selection.clear();
        } else {
            let end = self.text.len();
            self.set_cursor_to(end);
        }
        self.scroll_offset = point(px(0.), px(0.));
        if !self.multi_line {
            self.pending_scroll = Some(point(px(0.), px(0.)));
        }
        self.history.clear();
    }

    pub(crate) fn replace(&mut self, text: &str, cx: &mut App) {
        self.history.set_next_kind(EditKind::Atomic);
        self.replace_text_in_range(None, text, None, cx);
        let end = self.selection.end;
        self.set_cursor_to(end);
    }

    pub(crate) fn replace_ranges(&mut self, edits: &[(Range<usize>, String)], cx: &mut App) {
        self.history.set_next_kind(EditKind::Atomic);
        self.apply_range_edits(edits, cx);
    }

    pub(crate) fn select_all(&mut self) {
        self.history.break_coalescing();
        self.selection.start = 0;
        self.selection.end = self.text.len();
    }

    pub(crate) fn set_selected_range(&mut self, range: Range<usize>) {
        let end_bias = if range.start == range.end {
            Bias::Left
        } else {
            Bias::Right
        };
        let start = self.text.clip_offset(range.start, Bias::Left);
        let end = self.text.clip_offset(range.end, end_bias);

        self.history.break_coalescing();
        let start = self.cursor_boundary(start, Bias::Left);
        self.line_end_affinity = false;
        self.set_cursor_to(start);
        self.word_anchor = None;
        self.select_to_with_affinity(end, false);
        self.scroll_to(self.cursor(), None);
        self.update_preferred_x();
    }

    pub(crate) fn set_cursor_position(&mut self, position: Position) {
        let offset = self.text.position_to_offset(&position);
        self.history.break_coalescing();
        let offset = self.cursor_boundary(offset, Bias::Left);
        self.line_end_affinity = false;
        self.set_cursor_to(offset);
        self.scroll_to(offset, None);
        self.update_preferred_x();
    }

    // ---- Character / word / line boundary helpers ----

    pub(super) fn cursor_boundary(&self, offset: usize, bias: Bias) -> usize {
        let clamped = self.text.clip_offset(offset, bias);
        if clamped > 0
            && self.text.char_at(clamped - 1) == Some('\r')
            && self.text.char_at(clamped) == Some('\n')
        {
            match bias {
                Bias::Left => clamped - 1,
                Bias::Right => clamped + 1,
            }
        } else {
            clamped
        }
    }

    fn prev_boundary(&self, offset: usize) -> usize {
        self.cursor_boundary(offset.saturating_sub(1), Bias::Left)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.cursor_boundary(offset.saturating_add(1), Bias::Right)
    }

    fn prev_word_at(&self, offset: usize) -> usize {
        let aligned = self
            .text
            .offset_utf16_to_offset(self.text.offset_to_offset_utf16(offset));
        prev_word_start(&self.text, aligned)
    }

    fn next_word_at(&self, offset: usize) -> usize {
        let aligned = self
            .text
            .offset_utf16_to_offset(self.text.offset_to_offset_utf16(offset));
        next_word_end(&self.text, aligned)
    }

    fn line_start_for(&self, offset: usize) -> usize {
        if !self.multi_line {
            return 0;
        }
        let row = self.text.offset_to_point(offset).row;
        self.text.line_start_offset(row)
    }

    fn line_end_for(&self, offset: usize) -> usize {
        if !self.multi_line {
            return self.text.len();
        }
        let row = self.text.offset_to_point(offset).row;
        self.text.line_end_offset(row)
    }

    pub(super) fn set_cursor_to(&mut self, offset: usize) {
        self.selection.start = offset;
        self.selection.end = offset;
        self.selection.reversed = false;
    }

    // ---- Movement & selection ----

    pub(super) fn preferred_x_for(
        &self,
        offset: usize,
        end_affinity: bool,
    ) -> Option<(Pixels, usize)> {
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
        host: Option<&EditorHost>,
        cx: &mut App,
    ) {
        self.move_to_with_affinity(offset, direction, false, host, cx);
    }

    pub(super) fn move_to_with_affinity(
        &mut self,
        offset: usize,
        direction: Option<MoveDirection>,
        end_affinity: bool,
        host: Option<&EditorHost>,
        cx: &mut App,
    ) {
        self.history.break_coalescing();
        let offset = self.cursor_boundary(offset, Bias::Left);
        self.line_end_affinity = end_affinity;
        self.set_cursor_to(offset);
        self.scroll_to(offset, direction);
        if let Some(host) = host {
            self.pause_blink(host, cx);
        }
        self.update_preferred_x();
    }

    fn place_cursor(
        &mut self,
        offset: usize,
        anchor: Option<(Pixels, usize)>,
        end_affinity: bool,
        direction: Option<MoveDirection>,
        host: &EditorHost,
        cx: &mut App,
    ) {
        self.history.break_coalescing();
        let offset = self.cursor_boundary(offset, Bias::Left);
        self.selection.place_at(offset, anchor);
        self.line_end_affinity = end_affinity;
        self.scroll_to(self.cursor(), direction);
        self.pause_blink(host, cx);
    }

    fn move_left(&mut self, host: &EditorHost, cx: &mut App) -> bool {
        if self.selection.is_empty() && self.cursor() == 0 {
            return false;
        }
        let target = if self.selection.is_empty() {
            self.prev_boundary(self.selection.head())
        } else {
            self.selection.start
        };
        let anchor = self.preferred_x_for(target, false);
        self.place_cursor(target, anchor, false, None, host, cx);
        true
    }

    fn move_right(&mut self, host: &EditorHost, cx: &mut App) -> bool {
        if self.selection.is_empty() && self.cursor() == self.text.len() {
            return false;
        }
        let target = if self.selection.is_empty() {
            self.next_boundary(self.selection.head())
        } else {
            self.selection.end
        };
        let anchor = self.preferred_x_for(target, false);
        self.place_cursor(target, anchor, false, None, host, cx);
        true
    }

    fn select_left(&mut self, host: &EditorHost, cx: &mut App) {
        let target = self.prev_boundary(self.selection.head());
        self.select_keyboard_to(target, false, false, host, cx);
    }

    fn select_right(&mut self, host: &EditorHost, cx: &mut App) {
        let target = self.next_boundary(self.selection.head());
        self.select_keyboard_to(target, false, false, host, cx);
    }

    fn vertical_target(
        &self,
        offset: usize,
        anchor: Option<(Pixels, usize)>,
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
                    point(pref_x, next_disp.sub_row * layout.line_height),
                    layout.line_height,
                ) {
                    target_offset = line_start + local_ix;
                    target_affinity = affinity;
                }
            } else {
                let max_len = self.text.slice_line(next_pt.row).len();
                target_offset = line_start + byte_col.min(max_len);
            }
        }
        (target_offset, target_affinity)
    }

    fn move_vertical(
        &mut self,
        delta_rows: isize,
        collapse: bool,
        host: &EditorHost,
        cx: &mut App,
    ) {
        if !self.multi_line {
            return;
        }
        self.pause_blink(host, cx);
        let direction = if delta_rows < 0 {
            MoveDirection::Up
        } else {
            MoveDirection::Down
        };
        let sel = self.selection;
        let (base_offset, anchor, affinity) = if sel.is_empty() || !collapse {
            (sel.head(), sel.preferred_x, self.line_end_affinity)
        } else if delta_rows < 0 {
            let e = self.prev_boundary(sel.start.saturating_sub(1));
            (e, self.preferred_x_for(e, false), false)
        } else {
            let e = self.next_boundary(sel.end.saturating_sub(1));
            let aff = e == self.cursor() && self.line_end_affinity;
            (e, self.preferred_x_for(e, aff), aff)
        };
        let anchor = anchor.or_else(|| self.preferred_x_for(base_offset, affinity));
        let (target, target_affinity) =
            self.vertical_target(base_offset, anchor, affinity, delta_rows);
        self.place_cursor(target, anchor, target_affinity, Some(direction), host, cx);
    }

    fn select_vertical(&mut self, delta_rows: isize, host: &EditorHost, cx: &mut App) {
        if !self.multi_line {
            return;
        }
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
        self.select_keyboard_to(target.0, target.1, true, host, cx);
    }

    fn move_page(&mut self, sign: isize, host: &EditorHost, cx: &mut App) {
        let Some(layout) = self.layout.as_ref() else {
            return;
        };
        let rows = (self.input_bounds.size.height / layout.line_height) as isize;
        self.move_vertical(sign * rows, false, host, cx);
    }

    pub(super) fn select_to_with_affinity(&mut self, offset: usize, end_affinity: bool) {
        self.line_end_affinity = end_affinity;
        let clamped = self.cursor_boundary(offset, Bias::Left);
        self.selection.extend_to(clamped, self.word_anchor);
        if self.selection.is_empty() {
            self.update_preferred_x();
        }
    }

    fn select_keyboard_to(
        &mut self,
        offset: usize,
        end_affinity: bool,
        keep_column: bool,
        host: &EditorHost,
        cx: &mut App,
    ) {
        self.pause_blink(host, cx);
        self.history.break_coalescing();

        let prev_head = self.selection.head();
        let prev_aff = self.line_end_affinity;
        let clamped = self.cursor_boundary(offset, Bias::Left);
        self.selection.extend_to(clamped, None);
        self.line_end_affinity = end_affinity;

        if !keep_column {
            self.selection.preferred_x = self.preferred_x_for(self.selection.head(), end_affinity);
        } else if self.selection.preferred_x.is_none() {
            self.selection.preferred_x = self.preferred_x_for(prev_head, prev_aff);
        }
        self.scroll_to(self.cursor(), None);
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
    }

    pub(super) fn select_line_at(&mut self, offset: usize) {
        let range = line_range_at(&self.text, offset);
        self.history.break_coalescing();
        self.selection.start = range.start;
        self.selection.end = range.end;
        self.selection.reversed = false;
        self.word_anchor = None;
    }

    // ---- Deleting, clipboard, enter, escape ----

    fn delete_with(
        &mut self,
        collapsed_kind: EditKind,
        collapsed_span: impl FnOnce(&Self, usize) -> Range<usize>,
        host: &EditorHost,
        cx: &mut App,
    ) {
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
        self.history.set_next_kind(kind);

        self.replace_text_in_range(None, "", Some(host), cx);
        self.history.record_selection(before, self.selection);
        self.history.end_group();
        self.pause_blink(host, cx);
    }

    fn backspace(&mut self, host: &EditorHost, cx: &mut App) -> bool {
        if self.selection.is_empty() && self.cursor() == 0 {
            return false;
        }
        self.delete_with(
            EditKind::Backspace,
            |ed, off| ed.prev_boundary(off)..off,
            host,
            cx,
        );
        true
    }

    fn delete_forward(&mut self, host: &EditorHost, cx: &mut App) {
        self.delete_with(
            EditKind::DeleteForward,
            |ed, off| off..ed.next_boundary(off),
            host,
            cx,
        );
    }

    fn delete_prev_word(&mut self, host: &EditorHost, cx: &mut App) {
        self.delete_with(
            EditKind::Atomic,
            |ed, off| ed.prev_word_at(off)..off,
            host,
            cx,
        );
    }

    fn delete_next_word(&mut self, host: &EditorHost, cx: &mut App) {
        self.delete_with(
            EditKind::Atomic,
            |ed, off| off..ed.next_word_at(off),
            host,
            cx,
        );
    }

    fn enter(&mut self, host: &EditorHost, cx: &mut App) -> bool {
        if self.multi_line {
            self.replace_text_in_range(None, "\n", Some(host), cx);
            self.pause_blink(host, cx);
            true
        } else {
            self.history.break_coalescing();
            false
        }
    }

    fn escape(&mut self) -> bool {
        if self.ime_marked.is_some() {
            self.ime_marked = None;
            self.history.end_group();
        }
        false
    }

    fn copy(&self, cx: &App) {
        if !self.selection.is_empty() {
            let text = self.text.slice(self.selection).to_string();
            cx.write_clipboard_text(&text);
        }
    }

    fn cut(&mut self, host: &EditorHost, cx: &mut App) {
        if self.selection.is_empty() {
            return;
        }
        let text = self.text.slice(self.selection).to_string();
        cx.write_clipboard_text(&text);
        self.history.set_next_kind(EditKind::Atomic);
        self.replace_text_in_range(None, "", Some(host), cx);
    }

    fn paste(&mut self, host: &EditorHost, cx: &mut App) {
        let Some(mut text) = cx.read_clipboard_text().filter(|s| !s.is_empty()) else {
            return;
        };
        if text.contains('\r') {
            text = text.replace("\r\n", "\n").replace('\r', "\n");
        }
        if !self.multi_line {
            text = text.replace('\n', "");
        }
        self.history.set_next_kind(EditKind::Atomic);
        self.replace_text_in_range(None, &text, Some(host), cx);
        self.scroll_to(self.cursor(), None);
    }

    // ---- Indentation ----

    fn apply_indent(
        &mut self,
        op: IndentOp,
        block: bool,
        _host: &EditorHost,
        cx: &mut App,
    ) -> bool {
        if !self.multi_line {
            return false;
        }
        let indent_str = self.tab_size.as_str();
        let indent_len = indent_str.len();
        let indent_chars = indent_str.chars().count();
        let before = self.selection;

        let (edits, next_sel) = if !before.is_empty() || block {
            let start_row = self.text.offset_to_point(before.start).row;
            let end_row = self.text.offset_to_point(before.end).row;
            let mut edits = Vec::new();
            for row in start_row..=end_row {
                let start = self.text.line_start_offset(row);
                match op {
                    IndentOp::Indent => edits.push((start..start, indent_str.to_string())),
                    IndentOp::Outdent => {
                        if self
                            .text
                            .slice(start..)
                            .chars()
                            .take(indent_chars)
                            .eq(indent_str.chars())
                        {
                            edits.push((start..start + indent_len, String::new()));
                        }
                    }
                }
            }
            let remap = |offset: usize| match op {
                IndentOp::Indent => {
                    offset + edits.partition_point(|(r, _)| r.start <= offset) * indent_len
                }
                IndentOp::Outdent => {
                    let prior = edits.partition_point(|(r, _)| r.end <= offset);
                    let partial = edits
                        .get(prior)
                        .map_or(0, |(r, _)| offset.saturating_sub(r.start));
                    offset - prior * indent_len - partial
                }
            };
            let mut next = before;
            next.start = remap(before.start);
            next.end = remap(before.end);
            next.preferred_x = None;
            (edits, next)
        } else {
            let head = before.head();
            match op {
                IndentOp::Indent => (
                    vec![(head..head, indent_str.to_string())],
                    CaretSelection::new(head + indent_len, head + indent_len),
                ),
                IndentOp::Outdent => {
                    let row = self.text.offset_to_point(head).row;
                    let start = self.text.line_start_offset(row);
                    if self
                        .text
                        .slice(start..)
                        .chars()
                        .take(indent_chars)
                        .eq(indent_str.chars())
                    {
                        let removed = (start + indent_len).min(head) - start.min(head);
                        let next = head - removed;
                        (
                            vec![(start..start + indent_len, String::new())],
                            CaretSelection::new(next, next),
                        )
                    } else {
                        (Vec::new(), before)
                    }
                }
            }
        };

        if edits.is_empty() {
            return true;
        }
        self.history.begin_group();
        self.apply_range_edits(&edits, cx);
        self.selection = next_sel;
        self.history.record_selection(before, next_sel);
        self.history.end_group();
        self.scroll_to(self.cursor(), None);
        true
    }

    // ---- Undo / Redo ----

    fn undo(&mut self, host: &EditorHost, cx: &mut App) {
        self.replay_history(true, host, cx);
    }

    fn redo(&mut self, host: &EditorHost, cx: &mut App) {
        self.replay_history(false, host, cx);
    }

    fn replay_history(&mut self, is_undo: bool, host: &EditorHost, cx: &mut App) {
        let replay = if is_undo {
            self.history.undo()
        } else {
            self.history.redo()
        };
        let Some(replay) = replay else {
            return;
        };
        self.history.set_ignoring(true);
        for edit in &replay.edits {
            let (range, text) = if is_undo {
                (Range::from(edit.new_range), edit.old_text.as_str())
            } else {
                (Range::from(edit.old_range), edit.new_text.as_str())
            };
            let range_utf16 = self.range_to_utf16(&range);
            self.replace_text_in_range(Some(range_utf16), text, Some(host), cx);
        }
        if let Some(mut sel) = replay.selection {
            let len = self.text.len();
            sel.start = sel.start.min(len);
            sel.end = sel.end.min(len);
            self.selection = sel;
        }
        self.history.set_ignoring(false);
    }

    // ---- History & range-edit core ----

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

    fn apply_range_edits(&mut self, edits: &[(Range<usize>, String)], cx: &mut App) {
        if edits.is_empty() {
            return;
        }
        let mut descending: Vec<(Range<usize>, &str)> = edits
            .iter()
            .map(|(r, s)| (r.clone(), s.as_str()))
            .collect();
        descending.sort_by_key(|e| std::cmp::Reverse(e.0.start));

        let requested = self.history.take_next_kind();
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
            self.wrap.apply_edit(&self.text, range, new_text.len(), cx);
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
        if recorded {
            self.history.record_selection(sel_before, self.selection);
        }
        self.ime_marked = None;
        self.update_preferred_x();
    }

    fn normalize_single_line<'a>(&self, text: &'a str) -> Cow<'a, str> {
        if !self.multi_line && text.contains(['\n', '\r']) {
            Cow::Owned(text.replace(['\n', '\r'], ""))
        } else {
            Cow::Borrowed(text)
        }
    }

    fn accepts_text(&self, candidate: &str) -> bool {
        candidate.is_empty() || self.validate.as_ref().is_none_or(|f| f(candidate))
    }

    // ---- IME / Text input protocol ----

    pub(crate) fn selected_utf16_range(&self) -> UTF16Selection {
        UTF16Selection {
            range: self.range_to_utf16(&self.selected_range()),
        }
    }

    pub(crate) fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        host: Option<&EditorHost>,
        cx: &mut App,
    ) {
        let requested = self.history.take_next_kind();
        let selection_before = self.selection;
        let ends_composition = self.ime_marked.is_some();

        if let Some(host) = host {
            self.pause_blink(host, cx);
        }

        let normalized = self.normalize_single_line(new_text);
        let new_text: &str = &normalized;

        let range = range_utf16
            .as_ref()
            .map(|r| self.range_from_utf16(r))
            .or_else(|| {
                self.ime_marked.map(|m| {
                    let utf16 = self.range_to_utf16(&(m.start..m.end));
                    self.range_from_utf16(&utf16)
                })
            })
            .unwrap_or_else(|| self.selected_range());

        if self.multi_line {
            if let Some(kind) = requested {
                self.history.set_next_kind(kind);
            }
            self.apply_range_edits(&[(range, new_text.to_string())], cx);
            if ends_composition {
                self.history.end_group();
            }
            return;
        }

        let before_text = self.text.clone();
        self.text.replace(range.clone(), new_text);
        let new_offset = (range.start + new_text.len()).min(self.text.len());

        let pending = self.text.to_string();
        if !self.accepts_text(&pending) && self.accepts_text(&before_text.to_string()) {
            self.text = before_text;
            return;
        }

        self.push_history(
            &before_text,
            &range,
            new_text,
            requested,
            selection_before,
            None,
        );
        self.wrap.apply_edit(&self.text, &range, new_text.len(), cx);
        self.set_cursor_to(new_offset);
        self.ime_marked = None;
        if ends_composition {
            self.history
                .record_selection(selection_before, self.selection);
            self.history.end_group();
        }
        self.update_preferred_x();
    }

    pub(crate) fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_utf16: Option<Range<usize>>,
        cx: &mut App,
    ) {
        let requested = self.history.take_next_kind();
        let selection_before = self.selection;

        let starts_composition = self.ime_marked.is_none();
        if starts_composition {
            self.history.break_coalescing();
            self.history.begin_group();
        }

        let normalized = self.normalize_single_line(new_text);
        let new_text: &str = &normalized;

        let range = range_utf16
            .as_ref()
            .map(|r| self.range_from_utf16(r))
            .or_else(|| {
                self.ime_marked.map(|m| {
                    let utf16 = self.range_to_utf16(&(m.start..m.end));
                    self.range_from_utf16(&utf16)
                })
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

        self.wrap.apply_edit(&self.text, &range, new_text.len(), cx);

        if new_text.is_empty() {
            self.set_cursor_to(range.start);
            self.ime_marked = None;
        } else {
            self.ime_marked = Some((range.start..range.start + new_text.len()).into());
            let sel = new_selected_utf16
                .as_ref()
                .map(|r| {
                    let rope = Rope::from(new_text);
                    range.start + rope.offset_utf16_to_offset(r.start)
                        ..range.start + rope.offset_utf16_to_offset(r.end)
                })
                .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());
            self.selection.start = sel.start;
            self.selection.end = sel.end;
        }

        if self.push_history(
            &before_text,
            &range,
            new_text,
            requested,
            selection_before,
            Some(self.selection),
        ) {
            self.history
                .record_selection(selection_before, self.selection);
        }
        if new_text.is_empty() {
            self.history.end_group();
        }
    }

    pub(crate) fn bounds_for_utf16_range(
        &self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &Window,
    ) -> Option<Bounds<Pixels>> {
        let layout = self.layout.as_ref()?;
        let line_height = layout.line_height;
        let range = self.range_from_utf16(&range_utf16);

        let mut start_pos = None;
        let mut end_pos = None;
        let mut y = layout.visible_top;

        for (vi, row_layout) in layout.rows.iter().enumerate() {
            if start_pos.is_some() && end_pos.is_some() {
                break;
            }
            let base = layout.visible_byte_offsets[vi];
            if start_pos.is_none()
                && let Some(p) = row_layout.position_for_index(
                    range.start.saturating_sub(base),
                    line_height,
                    false,
                )
            {
                start_pos = Some(p + point(px(0.), y));
            }
            if end_pos.is_none()
                && let Some(p) = row_layout.position_for_index(
                    range.end.saturating_sub(base),
                    line_height,
                    false,
                )
            {
                end_pos = Some(p + point(px(0.), y));
            }
            y += row_layout.size(line_height).height;
        }

        let start_pos = start_pos.or_else(|| {
            let offset = self.last_cursor.unwrap_or_else(|| self.cursor());
            self.position_for_offset(offset)
        })?;
        let mut end_pos = end_pos.unwrap_or(start_pos);
        end_pos.y = start_pos.y;

        Some(Bounds::from_corners(
            bounds.origin + start_pos,
            bounds.origin + point(end_pos.x, end_pos.y + line_height),
        ))
    }
}
