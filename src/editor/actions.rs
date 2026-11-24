use std::ops::Range;

use crate::geom::{Rect, point};
use crate::gfx::TextSystem;

use super::buffer::{
    Bias, RopeExt as _, next_grapheme_boundary, next_word_end, prev_grapheme_boundary,
    prev_word_start,
};
use super::{Clipboard, EditCommand, Editor, EditorTimers, Motion, MoveDirection};

impl Editor {
    pub(crate) fn handle_command(
        &mut self,
        command: EditCommand,
        clipboard: &mut dyn Clipboard,
        ts: &TextSystem,
        timers: &mut dyn EditorTimers,
    ) -> bool {
        match command {
            EditCommand::Backspace => {
                self.model.commit_composition();
                if self.model.selection().is_empty() && self.cursor() == 0 {
                    return false;
                }
                self.pause_blink(timers);
                let fx = self.model.delete_backward();
                self.apply_effects(fx, ts);
            }
            EditCommand::Delete => {
                self.pause_blink(timers);
                let fx = self.model.delete_forward();
                self.apply_effects(fx, ts);
            }
            EditCommand::DeletePrevWord => {
                self.pause_blink(timers);
                let fx = self.model.delete_prev_word();
                self.apply_effects(fx, ts);
            }
            EditCommand::DeleteNextWord => {
                self.pause_blink(timers);
                let fx = self.model.delete_next_word();
                self.apply_effects(fx, ts);
            }
            EditCommand::Enter => {
                let Some(fx) = self.model.enter() else {
                    return false;
                };
                self.pause_blink(timers);
                self.apply_effects(fx, ts);
            }
            EditCommand::Escape => {
                let Some(fx) = self.model.cancel_composition() else {
                    return false;
                };
                self.apply_effects(fx, ts);
            }
            EditCommand::Copy => {
                if !self.model.selection().is_empty() {
                    clipboard.write(&self.selected_text().to_string());
                }
            }
            EditCommand::Cut => {
                self.model.commit_composition();
                if !self.model.selection().is_empty()
                    && clipboard.write(&self.selected_text().to_string())
                {
                    self.pause_blink(timers);
                    let fx = self.model.replace_selection("");
                    self.apply_effects(fx, ts);
                }
            }
            EditCommand::Paste => {
                self.model.commit_composition();
                if let Some(raw) = clipboard.read().filter(|s| !s.is_empty()) {
                    let text = if self.model.is_multi_line() {
                        raw.as_str()
                    } else {
                        raw.split(['\r', '\n']).next().unwrap_or("")
                    };
                    if !text.is_empty() {
                        self.pause_blink(timers);
                        let fx = self.model.replace_selection(text);
                        self.apply_effects(fx, ts);
                    }
                }
            }
            EditCommand::Undo => {
                if let Some(fx) = self.model.undo() {
                    self.pause_blink(timers);
                    self.apply_effects(fx, ts);
                }
            }
            EditCommand::Redo => {
                if let Some(fx) = self.model.redo() {
                    self.pause_blink(timers);
                    self.apply_effects(fx, ts);
                }
            }
            EditCommand::IndentInline | EditCommand::Indent => {
                let block = matches!(command, EditCommand::Indent);
                let Some(fx) = self.model.indent(block) else {
                    return false;
                };
                self.pause_blink(timers);
                self.apply_effects(fx, ts);
            }
            EditCommand::OutdentInline | EditCommand::Outdent => {
                let Some(fx) = self.model.outdent() else {
                    return false;
                };
                self.pause_blink(timers);
                self.apply_effects(fx, ts);
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
        self.pause_blink(timers);
        let fx = self.model.insert_text(text);
        self.apply_effects(fx, ts);
    }

    pub(crate) fn handle_ime_composition(&mut self, text: &str, cursor_utf16: usize, ts: &TextSystem) {
        if text.is_empty() && self.model.composition().is_none() {
            return;
        }
        let fx = self.model.set_composition(text, cursor_utf16);
        self.apply_effects(fx, ts);
    }

    pub(crate) fn ime_caret_bounds(&self) -> Option<Rect> {
        let bounds = self.content_bounds?;
        self.bounds_for_range(self.selected_range(), bounds)
            .or(self.caret_bounds)
    }

    pub(crate) fn set_value(&mut self, value: &str, ts: &TextSystem) {
        self.reset_pointer_selection_state();
        let fx = self.model.set_value(value);
        self.apply_effects(fx, ts);
        self.scroll_x = 0.0;
        self.scroll_y = 0.0;
        self.reveal = None;
    }

    pub(crate) fn replace(&mut self, text: &str, ts: &TextSystem) {
        let fx = self.model.replace_selection(text);
        self.apply_effects(fx, ts);
    }

    pub(crate) fn replace_all(&mut self, ranges: &[Range<usize>], replacement: &str, ts: &TextSystem) {
        let fx = self.model.replace_all(ranges, replacement);
        self.apply_effects(fx, ts);
    }

    pub(crate) fn select_all(&mut self) {
        self.model.select_all();
    }

    pub(crate) fn set_selected_range(&mut self, range: Range<usize>) {
        self.word_anchor = None;
        self.model.set_selected_range(range);
        self.request_reveal(self.cursor(), false, None, true);
        self.update_preferred_x();
    }

    pub(crate) fn go_to_line(&mut self, line: usize) {
        self.model.go_to_line(line);
        self.request_reveal(self.cursor(), false, None, true);
        self.update_preferred_x();
    }

    fn visual_line_start_for(&self, offset: usize, end_affinity: bool) -> usize {
        if !self.model.is_multi_line() {
            return 0;
        }
        let pt = self.model.text().offset_to_point(offset);
        let line_start = self.model.text().line_start_offset(pt.row);
        if self.soft_wrap {
            let disp = self.wrap.offset_to_display(offset, end_affinity);
            if let Some((range, _)) = self.wrap.visual_row_range(pt.row, disp.sub_row) {
                return line_start + range.start;
            }
        }
        line_start
    }

    fn visual_line_end_for(&self, offset: usize, end_affinity: bool) -> (usize, bool) {
        if !self.model.is_multi_line() {
            return (self.model.text().len(), false);
        }
        let pt = self.model.text().offset_to_point(offset);
        let line_start = self.model.text().line_start_offset(pt.row);
        if self.soft_wrap {
            let disp = self.wrap.offset_to_display(offset, end_affinity);
            if let Some((range, soft_end)) = self.wrap.visual_row_range(pt.row, disp.sub_row) {
                return (line_start + range.end, soft_end);
            }
        }
        (self.model.text().line_end_offset(pt.row), false)
    }

    pub(super) fn preferred_x_for(
        &self,
        offset: usize,
        end_affinity: bool,
    ) -> Option<(f32, usize)> {
        let layout = self.layout.as_ref()?;
        let pt = self.model.text().offset_to_point(offset);
        let row_layout = layout.row_layout(pt.row)?;
        let pos = row_layout.position_for_index(pt.column, layout.line_height, end_affinity)?;
        Some((pos.x, pt.column))
    }

    pub(super) fn update_preferred_x(&mut self) {
        let goal = self.preferred_x_for(self.cursor(), self.model.selection().end_affinity);
        self.model.selection_mut().goal = goal;
    }

    pub(super) fn move_to_with_affinity(
        &mut self,
        offset: usize,
        direction: Option<MoveDirection>,
        end_affinity: bool,
        with_margin: bool,
        timers: Option<&mut dyn EditorTimers>,
    ) {
        self.model.place_caret(offset, end_affinity, None);
        self.request_reveal(self.cursor(), end_affinity, direction, with_margin);
        if let Some(timers) = timers {
            self.pause_blink(timers);
        }
        self.update_preferred_x();
    }

    fn page_rows(&self) -> isize {
        let lh = self
            .layout
            .as_ref()
            .map_or(self.last_line_height, |l| l.line_height)
            .max(1.0);
        ((self.input_bounds.size.height / lh).floor() as isize).max(1)
    }

    fn vertical_target(
        &self,
        offset: usize,
        goal: Option<(f32, usize)>,
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
        let goal = goal.or_else(|| self.preferred_x_for(offset, end_affinity));
        let mut target_affinity = false;

        if let Some((pref_x, byte_col)) = goal {
            let mut next_disp = self.wrap.offset_to_display(target_offset, false);
            next_disp.col = 0;
            let next_pt = self.wrap.display_to_point(next_disp);
            let line_start = self.model.text().line_start_offset(next_pt.row);
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
                let max_len = self.model.text().slice_line(next_pt.row).len();
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
        if is_vertical && !self.model.is_multi_line() {
            return false;
        }
        self.model.commit_composition();

        let sel = self.model.selection();
        let len = self.model.text().len();
        if !extend {
            if matches!(motion, Motion::Left) && sel.is_empty() && self.cursor() == 0 {
                return false;
            }
            if matches!(motion, Motion::Right) && sel.is_empty() && self.cursor() == len {
                return false;
            }
        }

        let (target, target_aff, goal, direction, keep_col) = match motion {
            Motion::Left => {
                let t = if !extend && !sel.is_empty() {
                    sel.start()
                } else {
                    prev_grapheme_boundary(self.model.text(), sel.head)
                };
                (t, false, self.preferred_x_for(t, false), None, false)
            }
            Motion::Right => {
                let t = if !extend && !sel.is_empty() {
                    sel.end()
                } else {
                    next_grapheme_boundary(self.model.text(), sel.head)
                };
                (t, false, self.preferred_x_for(t, false), None, false)
            }
            Motion::PrevWord => {
                let t = prev_word_start(self.model.text(), sel.head);
                (t, false, self.preferred_x_for(t, false), None, false)
            }
            Motion::NextWord => {
                let t = next_word_end(self.model.text(), sel.head);
                (t, false, self.preferred_x_for(t, false), None, false)
            }
            Motion::LineStart => {
                let t = self.visual_line_start_for(sel.head, sel.end_affinity);
                (t, false, self.preferred_x_for(t, false), Some(MoveDirection::Up), false)
            }
            Motion::LineEnd => {
                let (t, aff) = self.visual_line_end_for(sel.head, sel.end_affinity);
                (t, aff, self.preferred_x_for(t, aff), Some(MoveDirection::Down), false)
            }
            Motion::DocStart => (0, false, None, None, false),
            Motion::DocEnd => (len, false, None, None, false),
            Motion::Up | Motion::Down | Motion::PageUp | Motion::PageDown => {
                let delta = match motion {
                    Motion::Up => -1,
                    Motion::Down => 1,
                    Motion::PageUp => -self.page_rows(),
                    Motion::PageDown => self.page_rows(),
                    _ => unreachable!(),
                };
                let dir = if delta < 0 {
                    MoveDirection::Up
                } else {
                    MoveDirection::Down
                };
                if extend {
                    let mut target =
                        self.vertical_target(sel.head, sel.goal, sel.end_affinity, delta);
                    if self.layout.is_some() {
                        let before_row =
                            self.wrap.offset_to_display(sel.head, sel.end_affinity).row;
                        let after_row = self.wrap.offset_to_display(target.0, target.1).row;
                        if before_row == after_row {
                            target = (if delta < 0 { 0 } else { len }, false);
                        }
                    }
                    (target.0, target.1, sel.goal, Some(dir), true)
                } else {
                    let collapse = matches!(motion, Motion::Up | Motion::Down);
                    let (base, anchor, aff) = if sel.is_empty() || !collapse {
                        (sel.head, sel.goal, sel.end_affinity)
                    } else if delta < 0 {
                        (sel.start(), self.preferred_x_for(sel.start(), false), false)
                    } else {
                        let a = sel.end() == self.cursor() && sel.end_affinity;
                        (sel.end(), self.preferred_x_for(sel.end(), a), a)
                    };
                    let anchor = anchor.or_else(|| self.preferred_x_for(base, aff));
                    let (t, t_aff) = self.vertical_target(base, anchor, aff, delta);
                    (t, t_aff, anchor, Some(dir), true)
                }
            }
        };

        if extend {
            let prev_head = sel.head;
            let prev_aff = sel.end_affinity;
            self.pause_blink(timers);
            self.model.break_coalescing();
            self.model.extend_selection(target, target_aff, None);
            if !keep_col {
                self.update_preferred_x();
            } else if self.model.selection().goal.is_none() {
                self.model.selection_mut().goal = self.preferred_x_for(prev_head, prev_aff);
            }
            self.request_reveal(self.cursor(), target_aff, None, true);
        } else {
            self.model.place_caret(target, target_aff, goal);
            if matches!(motion, Motion::DocStart | Motion::DocEnd) {
                self.update_preferred_x();
            }
            self.request_reveal(self.cursor(), target_aff, direction, true);
            self.pause_blink(timers);
        }
        true
    }

    pub(super) fn select_to_with_affinity(&mut self, offset: usize, end_affinity: bool) {
        self.model
            .extend_selection(offset, end_affinity, self.word_anchor.clone());
        if self.model.selection().is_empty() {
            self.update_preferred_x();
        }
    }

    pub(super) fn select_word_at(&mut self, offset: usize) {
        if let Some(range) = self.model.select_word_at(offset) {
            self.word_anchor = Some(range);
        }
    }

    pub(super) fn select_line_at(&mut self, offset: usize) {
        self.model.select_line_at(offset);
        self.word_anchor = None;
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
