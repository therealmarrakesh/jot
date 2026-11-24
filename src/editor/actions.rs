use std::ops::Range;

use crate::geom::{Rect, point};
use crate::gfx::TextSystem;

use super::buffer::{
    Bias, RopeExt as _, next_grapheme_boundary, next_word_end, prev_grapheme_boundary,
    prev_word_start,
};
use super::model::LineChange;
use super::{DocumentView, EditCommand, Editor, EditorTimers, Motion, MoveDirection};

impl Editor {
    fn commit_edit(
        &mut self,
        change: Option<LineChange>,
        ts: &TextSystem,
        timers: &mut dyn EditorTimers,
    ) {
        self.pause_blink(timers);
        self.apply_change(change, ts);
    }

    pub(crate) fn selected_string(&self) -> Option<String> {
        (!self.model.selection().is_empty()).then(|| self.model.selected_text().to_string())
    }

    pub(crate) fn prepare_cut(&mut self) -> Option<String> {
        self.model.commit_composition();
        self.selected_string()
    }

    pub(crate) fn delete_selection(&mut self, ts: &TextSystem, timers: &mut dyn EditorTimers) {
        let change = self.model.replace_selection("");
        self.commit_edit(change, ts, timers);
    }

    pub(crate) fn paste(&mut self, raw: &str, ts: &TextSystem, timers: &mut dyn EditorTimers) {
        let text = if self.model.is_multi_line() {
            raw
        } else {
            raw.split(['\r', '\n']).next().unwrap_or("")
        };
        if !text.is_empty() {
            let change = self.model.replace_selection(text);
            self.commit_edit(change, ts, timers);
        }
    }

    pub(crate) fn handle_command(
        &mut self,
        command: EditCommand,
        ts: &TextSystem,
        timers: &mut dyn EditorTimers,
    ) -> bool {
        match command {
            EditCommand::Backspace => {
                let Some(change) = self.model.delete_backward() else {
                    return false;
                };
                self.commit_edit(Some(change), ts, timers);
            }
            EditCommand::Delete => {
                let change = self.model.delete_forward();
                self.commit_edit(change, ts, timers);
            }
            EditCommand::DeletePrevWord => {
                let change = self.model.delete_prev_word();
                self.commit_edit(change, ts, timers);
            }
            EditCommand::DeleteNextWord => {
                let change = self.model.delete_next_word();
                self.commit_edit(change, ts, timers);
            }
            EditCommand::Enter => {
                let Some(change) = self.model.enter() else {
                    return false;
                };
                self.commit_edit(Some(change), ts, timers);
            }
            EditCommand::Escape => {
                let Some(change) = self.model.cancel_composition() else {
                    return false;
                };
                self.apply_change(Some(change), ts);
            }
            EditCommand::Copy | EditCommand::Cut | EditCommand::Paste => return false,
            EditCommand::Undo => {
                if let Some(change) = self.model.undo() {
                    self.commit_edit(Some(change), ts, timers);
                }
            }
            EditCommand::Redo => {
                if let Some(change) = self.model.redo() {
                    self.commit_edit(Some(change), ts, timers);
                }
            }
            EditCommand::IndentInline | EditCommand::Indent => {
                let Some(change) = self.model.indent(matches!(command, EditCommand::Indent)) else {
                    return false;
                };
                self.commit_edit(change, ts, timers);
            }
            EditCommand::Outdent => {
                let Some(change) = self.model.outdent() else {
                    return false;
                };
                self.commit_edit(change, ts, timers);
            }
            EditCommand::SelectAll => self.select_all(),
            EditCommand::Move { motion, extend } => {
                return self.move_caret(motion, extend, timers);
            }
        }
        true
    }

    pub(crate) fn handle_text_input(
        &mut self,
        text: &str,
        ts: &TextSystem,
        timers: &mut dyn EditorTimers,
    ) {
        let change = self.model.insert_text(text);
        self.commit_edit(change, ts, timers);
    }

    pub(crate) fn handle_ime_composition(&mut self, text: &str, cursor_utf16: usize, ts: &TextSystem) {
        if text.is_empty() && self.model.composition().is_none() {
            return;
        }
        let change = self.model.set_composition(text, cursor_utf16);
        self.apply_change(change, ts);
    }

    pub(crate) fn ime_caret_bounds(&self) -> Option<Rect> {
        let frame = self.fresh_frame()?;
        frame
            .bounds_for_range(self.model.text(), self.model.selected_range())
            .or(Some(frame.caret_rect))
    }

    pub(crate) fn set_value(&mut self, value: &str, ts: &TextSystem) {
        self.reset_pointer_selection_state();
        let change = self.model.set_value(value);
        self.apply_change(change, ts);
        self.scroll.x = 0.0;
        self.scroll.y = 0.0;
        self.reveal = None;
    }

    pub(crate) fn replace(&mut self, text: &str, ts: &TextSystem) {
        let change = self.model.replace_selection(text);
        self.apply_change(change, ts);
    }

    pub(crate) fn replace_all(&mut self, ranges: &[Range<usize>], replacement: &str, ts: &TextSystem) {
        let change = self.model.replace_all(ranges, replacement);
        self.apply_change(change, ts);
    }

    pub(crate) fn select_all(&mut self) {
        self.goal = None;
        self.model.select_all();
    }

    pub(crate) fn set_selected_range(&mut self, range: Range<usize>) {
        self.goal = None;
        self.word_anchor = None;
        self.model.set_selected_range(range);
        self.request_reveal(self.model.cursor(), false, None, true);
    }

    pub(crate) fn go_to_line(&mut self, line: usize) {
        self.goal = None;
        self.model.go_to_line(line);
        self.request_reveal(self.model.cursor(), false, None, true);
    }

    fn visual_line_start_for(&self, offset: usize, end_affinity: bool) -> usize {
        let Some(doc) = &self.doc else {
            return 0;
        };
        let text = self.model.text();
        let pt = text.offset_to_point(offset);
        let line_start = text.line_start_offset(pt.row);
        if doc.soft_wrap {
            let disp = doc.wrap.offset_to_display(text, offset, end_affinity);
            if let Some((range, _)) = doc.wrap.visual_row_range(pt.row, disp.sub_row) {
                return line_start + range.start;
            }
        }
        line_start
    }

    fn visual_line_end_for(&self, offset: usize, end_affinity: bool) -> (usize, bool) {
        let text = self.model.text();
        let Some(doc) = &self.doc else {
            return (text.len(), false);
        };
        let pt = text.offset_to_point(offset);
        let line_start = text.line_start_offset(pt.row);
        if doc.soft_wrap {
            let disp = doc.wrap.offset_to_display(text, offset, end_affinity);
            if let Some((range, soft_end)) = doc.wrap.visual_row_range(pt.row, disp.sub_row) {
                return (line_start + range.end, soft_end);
            }
        }
        (text.line_end_offset(pt.row), false)
    }

    pub(super) fn preferred_x_for(
        &self,
        offset: usize,
        end_affinity: bool,
    ) -> Option<(f32, usize)> {
        let frame = self.fresh_frame()?;
        let text = self.model.text();
        let pt = text.offset_to_point(offset);
        let row_layout = frame.row_layout(pt.row)?;
        let pos = row_layout.position_for_index(pt.column, frame.line_height, end_affinity)?;
        Some((pos.x, pt.column))
    }

    pub(super) fn move_to_with_affinity(
        &mut self,
        offset: usize,
        direction: Option<MoveDirection>,
        end_affinity: bool,
        with_margin: bool,
        timers: Option<&mut dyn EditorTimers>,
    ) {
        self.goal = None;
        self.model.place_caret(offset, end_affinity);
        self.request_reveal(self.model.cursor(), end_affinity, direction, with_margin);
        if let Some(timers) = timers {
            self.pause_blink(timers);
        }
    }

    fn page_rows(&self, doc: &DocumentView) -> isize {
        let lh = self
            .frame
            .as_ref()
            .map_or(doc.last_line_height, |f| f.line_height)
            .max(1.0);
        ((self.input_bounds.size.height / lh).floor() as isize).max(1)
    }

    fn vertical_target(
        &self,
        doc: &DocumentView,
        offset: usize,
        goal: Option<(f32, usize)>,
        end_affinity: bool,
        delta_rows: isize,
    ) -> (usize, bool) {
        if doc.wrap.visual_row_count() == 0 {
            return (offset, end_affinity);
        }
        let text = self.model.text();
        let mut display = doc.wrap.offset_to_display(text, offset, end_affinity);
        let max_row = doc.wrap.visual_row_count().saturating_sub(1);
        display.row = display.row.saturating_add_signed(delta_rows).min(max_row);
        display.col = 0;

        let mut target_offset = doc.wrap.display_to_offset(text, display);
        let mut target_affinity = false;

        if let Some((pref_x, byte_col)) = goal {
            let mut next_disp = doc.wrap.offset_to_display(text, target_offset, false);
            next_disp.col = 0;
            let next_pt = doc.wrap.display_to_point(text, next_disp);
            let line_start = text.line_start_offset(next_pt.row);
            if let Some(frame) = self.fresh_frame()
                && let Some(row_layout) = frame.row_layout(next_pt.row)
            {
                if let Some((local_ix, affinity)) = row_layout.closest_index_for_position(
                    point(pref_x, next_disp.sub_row as f32 * frame.line_height),
                    frame.line_height,
                ) {
                    target_offset = line_start + local_ix;
                    target_affinity = affinity;
                }
            } else if let Some((sub_range, soft_end)) =
                doc.wrap.visual_row_range(next_pt.row, next_disp.sub_row)
            {
                let max_col = sub_range.len();
                let col = byte_col.min(max_col);
                target_offset = line_start + sub_range.start + col;
                target_affinity = soft_end && col == max_col;
            } else {
                let max_len = text.slice_line(next_pt.row).len();
                target_offset = line_start + byte_col.min(max_len);
            }
        }
        (self.model.snap_boundary(target_offset, Bias::Left), target_affinity)
    }

    fn move_caret(
        &mut self,
        motion: Motion,
        extend: bool,
        timers: &mut dyn EditorTimers,
    ) -> bool {
        let is_vertical = matches!(
            motion,
            Motion::Up | Motion::Down | Motion::PageUp | Motion::PageDown
        );
        if is_vertical && self.doc.is_none() {
            return false;
        }
        self.model.commit_composition();

        let sel = self.model.selection();
        let len = self.model.text().len();
        if !extend {
            if matches!(motion, Motion::Left) && sel.is_empty() && self.model.cursor() == 0 {
                return false;
            }
            if matches!(motion, Motion::Right) && sel.is_empty() && self.model.cursor() == len {
                return false;
            }
        }

        let (target, target_aff, next_goal, direction) = match motion {
            Motion::Left => {
                let t = if !extend && !sel.is_empty() {
                    sel.start()
                } else {
                    prev_grapheme_boundary(self.model.text(), sel.head)
                };
                (t, false, None, None)
            }
            Motion::Right => {
                let t = if !extend && !sel.is_empty() {
                    sel.end()
                } else {
                    next_grapheme_boundary(self.model.text(), sel.head)
                };
                (t, false, None, None)
            }
            Motion::PrevWord => (prev_word_start(self.model.text(), sel.head), false, None, None),
            Motion::NextWord => (next_word_end(self.model.text(), sel.head), false, None, None),
            Motion::LineStart => (
                self.visual_line_start_for(sel.head, sel.end_affinity),
                false,
                None,
                Some(MoveDirection::Up),
            ),
            Motion::LineEnd => {
                let (t, aff) = self.visual_line_end_for(sel.head, sel.end_affinity);
                (t, aff, None, Some(MoveDirection::Down))
            }
            Motion::DocStart => (0, false, None, None),
            Motion::DocEnd => (len, false, None, None),
            Motion::Up | Motion::Down | Motion::PageUp | Motion::PageDown => {
                let Some(doc) = &self.doc else {
                    return false;
                };
                let delta = match motion {
                    Motion::Up => -1,
                    Motion::Down => 1,
                    Motion::PageUp => -self.page_rows(doc),
                    Motion::PageDown => self.page_rows(doc),
                    _ => unreachable!(),
                };
                let dir = if delta < 0 {
                    MoveDirection::Up
                } else {
                    MoveDirection::Down
                };
                if extend {
                    let goal = self
                        .goal
                        .or_else(|| self.preferred_x_for(sel.head, sel.end_affinity));
                    let mut target =
                        self.vertical_target(doc, sel.head, goal, sel.end_affinity, delta);
                    if doc.wrap.visual_row_count() > 0 {
                        let text = self.model.text();
                        let before_row = doc
                            .wrap
                            .offset_to_display(text, sel.head, sel.end_affinity)
                            .row;
                        let after_row = doc.wrap.offset_to_display(text, target.0, target.1).row;
                        if before_row == after_row {
                            target = (if delta < 0 { 0 } else { len }, false);
                        }
                    }
                    (target.0, target.1, goal, Some(dir))
                } else {
                    let collapse = matches!(motion, Motion::Up | Motion::Down);
                    let (base, goal, aff) = if sel.is_empty() || !collapse {
                        (
                            sel.head,
                            self.goal
                                .or_else(|| self.preferred_x_for(sel.head, sel.end_affinity)),
                            sel.end_affinity,
                        )
                    } else if delta < 0 {
                        (sel.start(), self.preferred_x_for(sel.start(), false), false)
                    } else {
                        let a = sel.end() == self.model.cursor() && sel.end_affinity;
                        (sel.end(), self.preferred_x_for(sel.end(), a), a)
                    };
                    let (t, t_aff) = self.vertical_target(doc, base, goal, aff, delta);
                    (t, t_aff, goal, Some(dir))
                }
            }
        };

        self.goal = next_goal;
        if extend {
            self.pause_blink(timers);
            self.model.break_coalescing();
            self.model.extend_selection(target, target_aff, None);
            self.request_reveal(self.model.cursor(), target_aff, None, true);
        } else {
            self.model.place_caret(target, target_aff);
            self.request_reveal(self.model.cursor(), target_aff, direction, true);
            self.pause_blink(timers);
        }
        true
    }

    pub(super) fn select_to_with_affinity(&mut self, offset: usize, end_affinity: bool) {
        self.goal = None;
        self.model
            .extend_selection(offset, end_affinity, self.word_anchor.clone());
    }

    pub(super) fn select_word_at(&mut self, offset: usize) {
        self.goal = None;
        if let Some(range) = self.model.select_word_at(offset) {
            self.word_anchor = Some(range);
        }
    }

    pub(super) fn select_line_at(&mut self, offset: usize) {
        self.goal = None;
        self.model.select_line_at(offset);
        self.word_anchor = None;
    }
}
