/// DisplayMap: Public facade for Input soft-wrap display mapping.
use std::ops::Range;

use gpui::{App, Font, Pixels};
use ropey::Rope;

pub use super::text_wrapper::WrappingIndent;
use super::text_wrapper::{LineItem, WrapDisplayPoint};
use super::wrap_map::WrapMap;
use super::{BufferPoint, DisplayPoint, WrapPoint};
use crate::input::Point as TreeSitterPoint;

pub struct DisplayMap {
    wrap_map: WrapMap,
}

impl DisplayMap {
    pub fn new(font: Font, font_size: Pixels, wrap_width: Option<Pixels>) -> Self {
        Self {
            wrap_map: WrapMap::new(font, font_size, wrap_width),
        }
    }

    pub fn buffer_pos_to_display_pos(&self, pos: BufferPoint) -> DisplayPoint {
        let wrap_pos = self.wrap_map.buffer_pos_to_wrap_pos(pos);
        DisplayPoint::new(wrap_pos.row, wrap_pos.col)
    }

    pub fn display_pos_to_buffer_pos(&self, pos: DisplayPoint) -> BufferPoint {
        self.wrap_map
            .wrap_pos_to_buffer_pos(WrapPoint::new(pos.row, pos.col))
    }

    #[inline]
    pub fn display_row_count(&self) -> usize {
        self.wrap_map.wrap_row_count()
    }

    pub fn display_row_to_buffer_line(&self, display_row: usize) -> usize {
        self.wrap_map.wrap_row_to_buffer_line(display_row)
    }

    pub fn buffer_line_to_display_row_range(&self, line: usize) -> Option<Range<usize>> {
        let range = self.wrap_map.buffer_line_to_wrap_row_range(line);
        (!range.is_empty()).then_some(range)
    }

    pub fn buffer_line_to_display_row(&self, line: usize) -> usize {
        self.wrap_map.buffer_line_to_first_wrap_row(line)
    }

    pub fn on_text_changed(
        &mut self,
        changed_text: &Rope,
        range: &Range<usize>,
        new_text: &Rope,
        cx: &mut App,
    ) {
        self.wrap_map
            .on_text_changed(changed_text, range, new_text, cx);
    }

    pub(crate) fn set_inline_metrics(
        &mut self,
        metrics: std::rc::Rc<[(std::ops::Range<usize>, Pixels)]>,
        cx: &mut App,
    ) {
        self.wrap_map.set_inline_metrics(metrics, cx);
    }

    pub fn on_layout_changed(&mut self, wrap_width: Option<Pixels>, cx: &mut App) {
        self.wrap_map.on_layout_changed(wrap_width, cx);
    }

    pub fn set_wrapping_indent(&mut self, wrapping_indent: WrappingIndent, cx: &mut App) {
        self.wrap_map.set_wrapping_indent(wrapping_indent, cx);
    }

    pub fn set_font(&mut self, font: Font, font_size: Pixels, cx: &mut App) {
        self.wrap_map.set_font(font, font_size, cx);
    }

    pub fn prepare_layout(
        &mut self,
        text: &Rope,
        font: Font,
        font_size: Pixels,
        wrap_width: Option<Pixels>,
        wrapping_indent: WrappingIndent,
        cx: &mut App,
    ) -> bool {
        self.wrap_map
            .prepare_layout(text, font, font_size, wrap_width, wrapping_indent, cx)
    }

    pub fn ensure_text_prepared(&mut self, text: &Rope, cx: &mut App) {
        self.wrap_map.ensure_text_prepared(text, cx);
    }

    pub fn set_text(&mut self, text: &Rope, cx: &mut App) {
        self.wrap_map.set_text(text, cx);
    }

    #[inline]
    pub(crate) fn offset_to_wrap_display_point(&self, offset: usize) -> WrapDisplayPoint {
        self.wrap_map.wrapper().offset_to_display_point(offset)
    }

    #[inline]
    pub(crate) fn offset_to_wrap_display_point_with_affinity(
        &self,
        offset: usize,
        line_end_affinity: bool,
    ) -> WrapDisplayPoint {
        self.wrap_map
            .wrapper()
            .offset_to_display_point_with_affinity(offset, line_end_affinity)
    }

    #[inline]
    pub(crate) fn wrap_display_point_to_offset(&self, point: WrapDisplayPoint) -> usize {
        self.wrap_map.wrapper().display_point_to_offset(point)
    }

    #[inline]
    pub(crate) fn wrap_display_point_to_point(&self, point: WrapDisplayPoint) -> TreeSitterPoint {
        self.wrap_map.wrapper().display_point_to_point(point)
    }

    #[inline]
    pub fn wrap_row_to_display_row(&self, wrap_row: usize) -> Option<usize> {
        Some(wrap_row)
    }

    #[inline]
    pub fn nearest_visible_display_row(&self, wrap_row: usize) -> usize {
        wrap_row.min(self.display_row_count().saturating_sub(1))
    }

    #[inline]
    pub fn display_row_to_wrap_row(&self, display_row: usize) -> Option<usize> {
        Some(display_row)
    }

    pub(crate) fn display_row_column_to_offset(&self, display_row: usize, column: usize) -> usize {
        self.wrap_map
            .wrapper()
            .display_point_to_offset(WrapDisplayPoint::new(display_row, 0, column))
    }

    #[inline]
    pub(crate) fn longest_row(&self) -> usize {
        self.wrap_map.wrapper().longest_row()
    }

    #[inline]
    pub(crate) fn line(&self, row: usize) -> Option<&LineItem> {
        self.wrap_map.line(row)
    }

    #[inline]
    pub fn text(&self) -> &Rope {
        self.wrap_map.text()
    }

    #[inline]
    pub fn visible_wrap_row_count_for_buffer_line(&self, line: usize) -> usize {
        self.wrap_map.buffer_line_to_wrap_row_range(line).len()
    }

    #[inline]
    pub fn wrap_row_count(&self) -> usize {
        self.wrap_map.wrap_row_count()
    }

    #[inline]
    pub fn buffer_line_count(&self) -> usize {
        self.wrap_map.buffer_line_count()
    }
}
