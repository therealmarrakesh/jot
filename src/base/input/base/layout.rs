use std::{ops::Range, rc::Rc};

use gpui::{Bounds, Pixels};

use super::display_map::LineLayout;

#[derive(Clone)]
pub(super) struct LastLayout {
    pub(super) visible_range: Range<usize>,
    pub(super) visible_buffer_lines: Vec<usize>,
    pub(super) visible_line_byte_offsets: Vec<usize>,
    pub(super) visible_top: Pixels,
    pub(super) visible_range_offset: Range<usize>,
    pub(super) lines: Rc<Vec<LineLayout>>,
    pub(super) line_height: Pixels,
    pub(super) wrap_width: Option<Pixels>,
    /// Width of one space in the editor font, used to paint selected newlines.
    pub(super) space_width: Pixels,
    pub(super) cursor_bounds: Option<Bounds<Pixels>>,
}

impl LastLayout {
    pub(crate) fn line(&self, row: usize) -> Option<&LineLayout> {
        let pos = self.visible_buffer_lines.binary_search(&row).ok()?;
        self.lines.get(pos)
    }
}
