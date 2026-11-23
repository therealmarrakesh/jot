use crate::ui::{Context, EntityInputHandler as _, SharedString, Window};
use ropey::Rope;
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

use super::rope_ext::Bias;
use super::{
    EditorState, RopeExt as _, change::Change, cursor::CursorSelection, undo_manager::EditIntent,
};

const WORD_SCAN_WINDOW_BYTES: usize = 256;

impl EditorState {
    pub(super) fn replace_text(
        &mut self,
        text: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text: SharedString = text.into();
        self.undo_manager.set_pending_intent(EditIntent::Atomic);
        let range = 0..self.text.chars().map(|c| c.len_utf16()).sum();
        self.replace_text_in_range(Some(range), &text, window, cx);
    }

    pub(super) fn select_left(&mut self, cx: &mut Context<Self>) {
        let offset = self.previous_boundary(self.selection.cursor_offset());
        self.select_keyboard_to(offset, false, false, cx);
    }

    pub(super) fn select_right(&mut self, cx: &mut Context<Self>) {
        let offset = self.next_boundary(self.selection.cursor_offset());
        self.select_keyboard_to(offset, false, false, cx);
    }

    pub(super) fn select_up(&mut self, cx: &mut Context<Self>) {
        if self.is_single_line() {
            return;
        }
        let (offset, affinity) = self.vertical_selection_target(
            self.selection.cursor_offset(),
            self.selection.column_anchor,
            self.cursor_line_end_affinity,
            -1,
        );
        self.select_keyboard_to(offset, affinity, true, cx);
    }

    pub(super) fn select_down(&mut self, cx: &mut Context<Self>) {
        if self.is_single_line() {
            return;
        }
        let (offset, affinity) = self.vertical_selection_target(
            self.selection.cursor_offset(),
            self.selection.column_anchor,
            self.cursor_line_end_affinity,
            1,
        );
        self.select_keyboard_to(offset, affinity, true, cx);
    }

    pub(super) fn select_to_start(&mut self, cx: &mut Context<Self>) {
        self.select_keyboard_to(0, false, false, cx);
    }

    pub(super) fn select_to_end(&mut self, cx: &mut Context<Self>) {
        let end = self.text.len();
        self.select_keyboard_to(end, false, false, cx);
    }

    pub(super) fn select_to_start_of_line(&mut self, cx: &mut Context<Self>) {
        let offset = self.start_of_line_at(self.selection.cursor_offset());
        self.select_keyboard_to(offset, false, false, cx);
    }

    pub(super) fn select_to_end_of_line(&mut self, cx: &mut Context<Self>) {
        let offset = self.end_of_line_at(self.selection.cursor_offset());
        self.select_keyboard_to(offset, true, false, cx);
    }

    pub(super) fn select_to_previous_word(&mut self, cx: &mut Context<Self>) {
        let offset = self.previous_start_of_word_at(self.selection.cursor_offset());
        self.select_keyboard_to(offset, false, false, cx);
    }

    pub(super) fn select_to_next_word(&mut self, cx: &mut Context<Self>) {
        let offset = self.next_end_of_word_at(self.selection.cursor_offset());
        self.select_keyboard_to(offset, false, false, cx);
    }

    pub(super) fn previous_start_of_word_at(&self, offset: usize) -> usize {
        let offset = self.offset_from_utf16(self.offset_to_utf16(offset));
        previous_word_start(&self.text, offset)
    }

    pub(super) fn next_end_of_word_at(&self, offset: usize) -> usize {
        let offset = self.offset_from_utf16(self.offset_to_utf16(offset));
        next_word_end(&self.text, offset)
    }

    pub(super) fn start_of_line_at(&self, offset: usize) -> usize {
        if self.is_single_line() {
            return 0;
        }
        let row = self.text.offset_to_point(offset).row;
        self.text.line_start_offset(row)
    }

    pub(super) fn end_of_line_at(&self, offset: usize) -> usize {
        if self.is_single_line() {
            return self.text.len();
        }
        let row = self.text.offset_to_point(offset).row;
        self.text.line_end_offset(row)
    }

    fn delete_selection(
        &mut self,
        collapsed_intent: EditIntent,
        collapsed_target: impl FnOnce(&Self, usize) -> Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let before = self.selection;
        let intent = if before.is_empty() {
            collapsed_intent
        } else {
            EditIntent::Atomic
        };
        let range = if before.is_empty() {
            collapsed_target(self, before.cursor_offset())
        } else {
            before.start..before.end
        };
        let (start, end) = (range.start.min(range.end), range.start.max(range.end));

        self.undo_manager.begin_transaction_with(intent);
        self.undo_manager.record_selection(before, before);
        self.selection.start = start;
        self.selection.end = end;
        self.undo_manager.set_pending_intent(intent);

        self.replace_text_in_range(None, "", window, cx);
        self.undo_manager.record_selection(before, self.selection);
        self.undo_manager.commit_transaction();
        self.pause_blink_cursor(cx);
    }

    pub(super) fn backspace(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.selection.is_empty() && self.cursor() == 0 {
            return false;
        }

        self.delete_selection(
            EditIntent::Backspace,
            |s, offset| s.previous_boundary(offset)..offset,
            window,
            cx,
        );
        true
    }

    pub(super) fn delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.delete_selection(
            EditIntent::DeleteForward,
            |s, offset| offset..s.next_boundary(offset),
            window,
            cx,
        );
    }

    pub(super) fn delete_previous_word(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.delete_selection(
            EditIntent::Atomic,
            |s, offset| s.previous_start_of_word_at(offset)..offset,
            window,
            cx,
        );
    }

    pub(super) fn delete_next_word(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.delete_selection(
            EditIntent::Atomic,
            |s, offset| offset..s.next_end_of_word_at(offset),
            window,
            cx,
        );
    }

    pub(super) fn enter(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.is_multi_line() {
            self.replace_text_in_range(None, "\n", window, cx);
            self.pause_blink_cursor(cx);
            true
        } else {
            self.undo_manager.break_transaction_coalescing();
            false
        }
    }

    pub(super) fn escape(&mut self) -> bool {
        if self.ime_marked_range.is_some() {
            self.unmark_text();
        }
        false
    }

    pub(super) fn copy(&mut self, cx: &mut Context<Self>) {
        if !self.is_copyable() {
            return;
        }
        let text = self.text.slice(self.selection).to_string();
        cx.write_clipboard_text(&text);
    }

    pub(super) fn cut(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_copyable() {
            return;
        }
        let text = self.text.slice(self.selection).to_string();
        cx.write_clipboard_text(&text);
        self.undo_manager.set_pending_intent(EditIntent::Atomic);
        self.replace_text_in_range(None, "", window, cx);
    }

    pub(super) fn paste(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(mut new_text) = cx.read_clipboard_text().filter(|text| !text.is_empty()) {
            if new_text.contains('\r') {
                new_text = new_text.replace("\r\n", "\n").replace('\r', "\n");
            }
            if self.is_single_line() {
                new_text = new_text.replace('\n', "");
            }
            self.undo_manager.set_pending_intent(EditIntent::Atomic);
            self.replace_text_in_range(None, &new_text, window, cx);
            self.scroll_to(self.cursor(), None, cx);
        }
    }

    fn collapse_for_intent(
        intent: EditIntent,
        mut selection: CursorSelection,
        range: &Range<usize>,
    ) -> CursorSelection {
        match intent {
            EditIntent::Backspace => selection.place_at(range.end, None),
            EditIntent::DeleteForward => selection.place_at(range.start, None),
            EditIntent::Typing | EditIntent::Atomic => {}
        }
        selection
    }

    pub(super) fn push_history(
        &mut self,
        text: &Rope,
        range: &Range<usize>,
        new_text: &str,
        requested_intent: Option<EditIntent>,
        selection_before: CursorSelection,
        selection_after: Option<CursorSelection>,
    ) -> bool {
        self.document_revision = self.document_revision.wrapping_add(1);
        if self.undo_manager.is_ignoring() {
            return false;
        }

        let range =
            text.clip_offset(range.start, Bias::Left)..text.clip_offset(range.end, Bias::Right);
        let old_text = text.slice(range.clone()).to_string();
        let new_range = range.start..range.start + new_text.len();

        let intent = requested_intent.unwrap_or_else(|| {
            if range.is_empty()
                && old_text.is_empty()
                && !new_text.is_empty()
                && !new_text.contains(['\n', '\r'])
            {
                EditIntent::Typing
            } else {
                EditIntent::Atomic
            }
        });

        let selection_before = Self::collapse_for_intent(intent, selection_before, &range);
        let selection_after =
            selection_after.unwrap_or_else(|| (new_range.end..new_range.end).into());

        let open_transaction = self.undo_manager.has_open_transaction();
        let change = Change::new(range, &old_text, new_range, new_text);
        let recorded = self.undo_manager.record_transaction(change, intent);
        if recorded && !open_transaction {
            self.undo_manager
                .record_selection(selection_before, selection_after);
        }
        recorded
    }

    pub(super) fn undo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.replay_history(true, window, cx);
    }

    pub(super) fn redo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.replay_history(false, window, cx);
    }

    fn replay_history(&mut self, undo: bool, window: &mut Window, cx: &mut Context<Self>) {
        let replay = if undo {
            self.undo_manager.undo()
        } else {
            self.undo_manager.redo()
        };
        let Some(replay) = replay else {
            return;
        };
        self.undo_manager.set_ignoring(true);
        for change in &replay.changes {
            let (range, text) = if undo {
                (change.new_range.into(), &change.old_text)
            } else {
                (change.old_range.into(), &change.new_text)
            };
            let range_utf16 = self.range_to_utf16(&range);
            self.replace_text_in_range(Some(range_utf16), text, window, cx);
        }
        if let Some(mut sel) = replay.selection {
            let len = self.text.len();
            sel.start = sel.start.min(len);
            sel.end = sel.end.min(len);
            self.selection = sel;
        }
        self.undo_manager.set_ignoring(false);
        cx.notify();
    }

    fn extend_selection(
        sel: &mut CursorSelection,
        offset: usize,
        word_range: Option<CursorSelection>,
    ) {
        if sel.reversed {
            sel.start = offset;
        } else {
            sel.end = offset;
        }

        if sel.end < sel.start {
            sel.reversed = !sel.reversed;
            std::mem::swap(&mut sel.start, &mut sel.end);
        }

        if let Some(word_range) = word_range {
            if sel.start > word_range.start {
                sel.start = word_range.start;
            }
            if sel.end < word_range.end {
                sel.end = word_range.end;
            }
        }
    }

    pub(crate) fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.select_to_with_affinity(offset, false, cx);
    }

    pub(crate) fn select_to_with_affinity(
        &mut self,
        offset: usize,
        line_end_affinity: bool,
        cx: &mut Context<Self>,
    ) {
        self.cursor_line_end_affinity = line_end_affinity;
        let offset = self.cursor_boundary(offset, Bias::Left);
        let word_range = self.selected_word_range;
        Self::extend_selection(&mut self.selection, offset, word_range);

        if self.selection.is_empty() {
            self.update_preferred_column();
        }
        cx.notify();
    }

    fn select_keyboard_to(
        &mut self,
        offset: usize,
        affinity: bool,
        preserve_column: bool,
        cx: &mut Context<Self>,
    ) {
        self.pause_blink_cursor(cx);
        self.undo_manager.break_transaction_coalescing();

        let prev_cursor = self.selection.cursor_offset();
        let prev_affinity = self.cursor_line_end_affinity;
        let offset = self.cursor_boundary(offset, Bias::Left);
        Self::extend_selection(&mut self.selection, offset, None);
        self.cursor_line_end_affinity = affinity;

        if !preserve_column {
            self.selection.column_anchor =
                self.preferred_column_for_with_affinity(self.selection.cursor_offset(), affinity);
        } else if self.selection.column_anchor.is_none() {
            self.selection.column_anchor =
                self.preferred_column_for_with_affinity(prev_cursor, prev_affinity);
        }

        self.scroll_to(self.cursor(), None, cx);
        cx.notify();
    }

    pub(crate) fn replace_text_in_ranges(
        &mut self,
        edits: &[(Range<usize>, String)],
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if edits.is_empty() {
            return;
        }

        let mut sorted: Vec<(Range<usize>, &str)> = edits
            .iter()
            .map(|(range, text)| (range.clone(), text.as_str()))
            .collect();
        sorted.sort_by_key(|edit| std::cmp::Reverse(edit.0.start));

        #[cfg(debug_assertions)]
        for pair in sorted.windows(2) {
            debug_assert!(
                pair[1].0.end <= pair[0].0.start,
                "replace_text_in_ranges requires disjoint ranges"
            );
        }

        let requested_intent = self.undo_manager.take_pending_intent();
        let selection_before = Self::collapse_for_intent(
            requested_intent.unwrap_or(EditIntent::Atomic),
            self.selection,
            &(self.selection.start..self.selection.end),
        );
        let group = sorted.len() > 1;
        if group {
            self.undo_manager.begin_transaction();
        }

        let mut recorded = false;
        for (range, new_text) in &sorted {
            let old_text = self.text.clone();
            self.text.replace(range.clone(), new_text);

            recorded |= self.push_history(
                &old_text,
                range,
                new_text,
                requested_intent,
                self.selection,
                None,
            );

            self.text_wrapper
                .update(&self.text, range, &Rope::from(*new_text), cx);
        }

        if group {
            self.undo_manager.commit_transaction();
        }

        let mut ascending = sorted;
        ascending.sort_by_key(|edit| edit.0.start);
        let text_len = self.text.len();
        let mut delta: isize = 0;
        let mut last_offset = 0;
        for (range, new_text) in &ascending {
            last_offset = ((range.start as isize + delta) as usize + new_text.len()).min(text_len);
            delta += new_text.len() as isize - (range.end as isize - range.start as isize);
        }
        self.selection.place_at(last_offset, None);

        if recorded {
            self.undo_manager
                .record_selection(selection_before, self.selection);
        }

        self.ime_marked_range.take();
        self.update_preferred_column();
        cx.notify();
    }
}

/// Find the start of the previous UAX #29 word before `offset` in O(word length) time.
fn previous_word_start(text: &Rope, offset: usize) -> usize {
    let offset = text.floor_char_boundary(offset.min(text.len()));
    if offset == 0 {
        return 0;
    }

    let mut word_end = offset;
    for ch in text.chars_at(offset).reversed() {
        if ch.is_whitespace() {
            word_end -= ch.len_utf8();
        } else {
            break;
        }
    }
    if word_end == 0 {
        return 0;
    }

    let mut window = WORD_SCAN_WINDOW_BYTES;
    loop {
        let win_start = text.floor_char_boundary(word_end.saturating_sub(window));
        let slice = text.slice(win_start..word_end).to_string();
        if let Some((i, _)) = UnicodeSegmentation::split_word_bound_indices(slice.as_str())
            .rfind(|(_, s)| !s.trim_start().is_empty())
            && (win_start == 0 || (i > 0 && slice[..i].chars().count() >= 4))
        {
            return win_start + i;
        }
        if win_start == 0 {
            return 0;
        }
        window = window.saturating_mul(2);
    }
}

/// Find the end of the next UAX #29 word after `offset` in O(word length) time.
fn next_word_end(text: &Rope, offset: usize) -> usize {
    let len = text.len();
    let offset = text.floor_char_boundary(offset.min(len));
    if offset >= len {
        return len;
    }

    let mut word_start = offset;
    for ch in text.chars_at(offset) {
        if ch.is_whitespace() {
            word_start += ch.len_utf8();
        } else {
            break;
        }
    }
    if word_start >= len {
        return len;
    }

    let mut window = WORD_SCAN_WINDOW_BYTES;
    loop {
        let win_end = text.ceil_char_boundary(word_start.saturating_add(window).min(len));
        let slice = text.slice(word_start..win_end).to_string();
        if let Some((i, s)) = UnicodeSegmentation::split_word_bound_indices(slice.as_str())
            .find(|(_, s)| !s.trim_start().is_empty())
        {
            let end = i + s.len();
            if win_end == len || slice[end..].chars().count() >= 4 {
                return word_start + end;
            }
        }
        if win_end == len {
            return len;
        }
        window = window.saturating_mul(2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reference_prev_word(text: &str, offset: usize) -> usize {
        UnicodeSegmentation::split_word_bound_indices(&text[..offset])
            .rfind(|(_, s)| !s.trim_start().is_empty())
            .map(|(i, _)| i)
            .unwrap_or(0)
    }

    fn reference_next_word(text: &str, offset: usize) -> usize {
        UnicodeSegmentation::split_word_bound_indices(&text[offset..])
            .find(|(_, s)| !s.trim_start().is_empty())
            .map(|(i, s)| offset + i + s.len())
            .unwrap_or(text.len())
    }

    #[test]
    fn bounded_word_navigation_matches_full_scan_everywhere() {
        let long_word = "a".repeat(WORD_SCAN_WINDOW_BYTES + 64);
        let long_spaces = " ".repeat(WORD_SCAN_WINDOW_BYTES + 64);
        let sample = format!(
            "  hello, don't 1,234.56 foo_bar\r\n日本語🎉 test{long_spaces}{long_word}  tail"
        );
        let rope = Rope::from(sample.as_str());

        for (offset, _) in sample.char_indices().chain([(sample.len(), '\0')]) {
            assert_eq!(
                previous_word_start(&rope, offset),
                reference_prev_word(&sample, offset),
                "prev mismatch at offset {offset}"
            );
            assert_eq!(
                next_word_end(&rope, offset),
                reference_next_word(&sample, offset),
                "next mismatch at offset {offset}"
            );
        }
    }
}
