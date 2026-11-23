use crate::ui::{Bounds, Context, EntityInputHandler, Pixels, UTF16Selection, Window, point, px};
use ropey::Rope;
use std::ops::Range;

use super::{EditorState, RopeExt as _};

impl EditorState {
    pub(super) fn unmark_text(&mut self) {
        self.ime_marked_range = None;
        self.undo_manager.commit_transaction();
    }
}

impl EntityInputHandler for EditorState {
    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range()),
        })
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let requested_intent = self.undo_manager.take_pending_intent();
        let selection_before = self.selection;
        let ends_composition = self.ime_marked_range.is_some();

        self.pause_blink_cursor(cx);

        let new_text = self.normalize_input(new_text);
        let new_text: &str = &new_text;

        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.ime_marked_range.map(|range| {
                let range = self.range_to_utf16(&(range.start..range.end));
                self.range_from_utf16(&range)
            }))
            .unwrap_or(self.selected_range());

        if self.is_multi_line() {
            if let Some(intent) = requested_intent {
                self.undo_manager.set_pending_intent(intent);
            }
            self.replace_text_in_ranges(&[(range, new_text.to_string())], window, cx);
            if ends_composition {
                self.undo_manager.commit_transaction();
            }
            return;
        }

        let old_text = self.text.clone();
        self.text.replace(range.clone(), new_text);
        let new_offset = (range.start + new_text.len()).min(self.text.len());

        let pending_text = self.text.to_string();
        if !self.is_valid_input(&pending_text, cx)
            && self.is_valid_input(&old_text.to_string(), cx)
        {
            self.text = old_text;
            return;
        }

        self.push_history(
            &old_text,
            &range,
            new_text,
            requested_intent,
            selection_before,
            None,
        );
        self.text_wrapper
            .update(&self.text, &range, &Rope::from(new_text), cx);

        self.set_cursor_to(new_offset);
        self.ime_marked_range.take();
        if ends_composition {
            self.undo_manager
                .record_selection(selection_before, self.selection);
            self.undo_manager.commit_transaction();
        }
        self.update_preferred_column();
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let requested_intent = self.undo_manager.take_pending_intent();
        let selection_before = self.selection;

        let starts_composition = self.ime_marked_range.is_none();
        if starts_composition {
            self.undo_manager.break_transaction_coalescing();
            self.undo_manager.begin_transaction();
        }

        let new_text = self.normalize_input(new_text);
        let new_text: &str = &new_text;

        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.ime_marked_range.map(|range| {
                let range = self.range_to_utf16(&(range.start..range.end));
                self.range_from_utf16(&range)
            }))
            .unwrap_or(self.selected_range());

        let old_text = self.text.clone();
        self.text.replace(range.clone(), new_text);

        if self.is_single_line() {
            let pending_text = self.text.to_string();
            if !self.is_valid_input(&pending_text, cx)
                && self.is_valid_input(&old_text.to_string(), cx)
            {
                self.text = old_text;
                if starts_composition {
                    self.undo_manager.commit_transaction();
                }
                return;
            }
        }

        self.text_wrapper
            .update(&self.text, &range, &Rope::from(new_text), cx);

        if new_text.is_empty() {
            self.set_cursor_to(range.start);
            self.ime_marked_range = None;
        } else {
            self.ime_marked_range = Some((range.start..range.start + new_text.len()).into());
            let new_range = new_selected_range_utf16
                .as_ref()
                .map(|range_utf16| {
                    let new_text = Rope::from(new_text);
                    range.start + new_text.offset_utf16_to_offset(range_utf16.start)
                        ..range.start + new_text.offset_utf16_to_offset(range_utf16.end)
                })
                .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());
            self.set_selection(new_range.start, new_range.end);
        }
        if self.push_history(
            &old_text,
            &range,
            new_text,
            requested_intent,
            selection_before,
            Some(self.selection),
        ) {
            self.undo_manager
                .record_selection(selection_before, self.selection);
        }
        if new_text.is_empty() {
            self.undo_manager.commit_transaction();
        }
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let last_layout = self.last_layout.as_ref()?;
        let line_height = last_layout.line_height;
        let range = self.range_from_utf16(&range_utf16);

        let mut start_origin = None;
        let mut end_origin = None;
        let mut y_offset = last_layout.visible_top;

        for (vi, line) in last_layout.lines.iter().enumerate() {
            if start_origin.is_some() && end_origin.is_some() {
                break;
            }
            let index_offset = last_layout.visible_line_byte_offsets[vi];
            if start_origin.is_none()
                && let Some(p) = line.position_for_index(
                    range.start.saturating_sub(index_offset),
                    last_layout,
                    false,
                )
            {
                start_origin = Some(p + point(px(0.), y_offset));
            }
            if end_origin.is_none()
                && let Some(p) = line.position_for_index(
                    range.end.saturating_sub(index_offset),
                    last_layout,
                    false,
                )
            {
                end_origin = Some(p + point(px(0.), y_offset));
            }
            y_offset += line.size(line_height).height;
        }

        let start_origin = start_origin.or_else(|| {
            let offset = self.last_cursor.or(Some(self.cursor()))?;
            self.line_and_position_for_offset(offset).2
        })?;
        let mut end_origin = end_origin.unwrap_or(start_origin);
        end_origin.y = start_origin.y;

        Some(Bounds::from_corners(
            bounds.origin + start_origin,
            bounds.origin + point(end_origin.x, end_origin.y + line_height),
        ))
    }
}
