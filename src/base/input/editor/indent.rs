use crate::base::input::{
    EditorState, Indent, IndentInline, Outdent, OutdentInline, RopeExt, cursor::CursorSelection,
};
use gpui::{Context, SharedString, Window};
use ropey::RopeSlice;
use std::ops::Range;

#[derive(Debug, Copy, Clone)]
pub struct TabSize {
    pub tab_size: usize,
    pub hard_tabs: bool,
}

impl Default for TabSize {
    fn default() -> Self {
        Self {
            tab_size: 2,
            hard_tabs: false,
        }
    }
}

impl TabSize {
    pub(super) fn to_string(&self) -> SharedString {
        if self.hard_tabs {
            "\t".into()
        } else {
            " ".repeat(self.tab_size).into()
        }
    }

    /// Count the indent size of the line in spaces.
    pub fn indent_count(&self, line: &RopeSlice) -> usize {
        let mut count = 0;
        for ch in line.chars() {
            match ch {
                '\t' => count += self.tab_size,
                ' ' => count += 1,
                _ => break,
            }
        }
        count
    }
}

impl EditorState {
    pub(super) fn indent_inline(
        &mut self,
        _: &IndentInline,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_indent(IndentDirection::Indent, false, window, cx);
    }

    pub(super) fn indent_block(&mut self, _: &Indent, window: &mut Window, cx: &mut Context<Self>) {
        self.apply_indent(IndentDirection::Indent, true, window, cx);
    }

    pub(super) fn outdent_inline(
        &mut self,
        _: &OutdentInline,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_indent(IndentDirection::Outdent, false, window, cx);
    }

    pub(super) fn outdent_block(
        &mut self,
        _: &Outdent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_indent(IndentDirection::Outdent, true, window, cx);
    }

    fn apply_indent(
        &mut self,
        direction: IndentDirection,
        block: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.is_multi_line() {
            cx.propagate();
            return;
        }

        let tab_indent = self.tab_size.to_string();
        let tab_len = tab_indent.len();
        let before = self.selection;

        let (edits, new_selection) = if !before.is_empty() || block {
            self.compute_block_indent(direction, &tab_indent, tab_len)
        } else {
            self.compute_inline_indent(direction, &tab_indent, tab_len)
        };

        if edits.is_empty() {
            return;
        }

        self.undo_manager.begin_transaction();
        self.replace_text_in_ranges(&edits, window, cx);
        self.selection = new_selection;
        self.undo_manager.record_selection(before, new_selection);
        self.undo_manager.commit_transaction();

        self.scroll_to(self.cursor(), None, cx);
        cx.notify();
    }

    fn compute_block_indent(
        &self,
        direction: IndentDirection,
        tab_indent: &str,
        tab_len: usize,
    ) -> (Vec<(Range<usize>, String)>, CursorSelection) {
        let sel = self.selection;
        let start_row = self.text.offset_to_point(sel.start).row;
        let end_row = self.text.offset_to_point(sel.end).row;

        let mut edits = Vec::new();
        for row in start_row..=end_row {
            let line_start = self.text.line_start_offset(row);
            match direction {
                IndentDirection::Indent => {
                    edits.push((line_start..line_start, tab_indent.to_string()));
                }
                IndentDirection::Outdent => {
                    if self
                        .text
                        .slice(line_start..)
                        .chars()
                        .take(tab_indent.chars().count())
                        .eq(tab_indent.chars())
                    {
                        edits.push((line_start..line_start + tab_len, String::new()));
                    }
                }
            }
        }

        let map_offset = |offset: usize| match direction {
            IndentDirection::Indent => {
                offset + edits.partition_point(|(range, _)| range.start <= offset) * tab_len
            }
            IndentDirection::Outdent => {
                let preceding = edits.partition_point(|(range, _)| range.end <= offset);
                let partial = edits
                    .get(preceding)
                    .map_or(0, |(range, _)| offset.saturating_sub(range.start));
                offset - preceding * tab_len - partial
            }
        };

        let mut new_selection = sel;
        new_selection.start = map_offset(sel.start);
        new_selection.end = map_offset(sel.end);
        new_selection.column_anchor = None;
        (edits, new_selection)
    }

    fn compute_inline_indent(
        &self,
        direction: IndentDirection,
        tab_indent: &str,
        tab_len: usize,
    ) -> (Vec<(Range<usize>, String)>, CursorSelection) {
        let cursor = self.selection.cursor_offset();
        match direction {
            IndentDirection::Indent => (
                vec![(cursor..cursor, tab_indent.to_string())],
                CursorSelection::new(cursor + tab_len, cursor + tab_len),
            ),
            IndentDirection::Outdent => {
                let row = self.text.offset_to_point(cursor).row;
                let start = self.text.line_start_offset(row);
                if self
                    .text
                    .slice(start..)
                    .chars()
                    .take(tab_indent.chars().count())
                    .eq(tab_indent.chars())
                {
                    let removed = (start + tab_len).min(cursor) - start.min(cursor);
                    let next = cursor - removed;
                    (
                        vec![(start..start + tab_len, String::new())],
                        CursorSelection::new(next, next),
                    )
                } else {
                    (Vec::new(), self.selection)
                }
            }
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq)]
enum IndentDirection {
    Indent,
    Outdent,
}

#[cfg(test)]
mod tests {
    use ropey::RopeSlice;

    use super::TabSize;

    #[test]
    fn test_tab_size() {
        let tab = TabSize {
            tab_size: 2,
            hard_tabs: false,
        };
        assert_eq!(tab.to_string(), "  ");
        let tab = TabSize {
            tab_size: 4,
            hard_tabs: false,
        };
        assert_eq!(tab.to_string(), "    ");

        let tab = TabSize {
            tab_size: 2,
            hard_tabs: true,
        };
        assert_eq!(tab.to_string(), "\t");
        let tab = TabSize {
            tab_size: 4,
            hard_tabs: true,
        };
        assert_eq!(tab.to_string(), "\t");
    }

    #[test]
    fn test_tab_size_indent_count() {
        let tab = TabSize {
            tab_size: 4,
            hard_tabs: false,
        };
        assert_eq!(tab.indent_count(&RopeSlice::from("abc")), 0);
        assert_eq!(tab.indent_count(&RopeSlice::from("  abc")), 2);
        assert_eq!(tab.indent_count(&RopeSlice::from("    abc")), 4);
        assert_eq!(tab.indent_count(&RopeSlice::from("\tabc")), 4);
        assert_eq!(tab.indent_count(&RopeSlice::from("  \tabc")), 6);
        assert_eq!(tab.indent_count(&RopeSlice::from(" \t abc  ")), 6);
        assert_eq!(tab.indent_count(&RopeSlice::from("abc")), 0);
    }
}
