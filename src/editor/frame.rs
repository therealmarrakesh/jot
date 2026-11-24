use std::ops::Range;

use ropey::Rope;

use crate::color::Rgba;
use crate::geom::{Point, Rect, Size, point, size};
use crate::gfx::{Canvas, ShapedRow, TextStyle, fill};
use crate::theme::Palette;

use super::buffer::RopeExt as _;

#[derive(Clone, Debug)]
pub(crate) struct EditorPaintStyle {
    pub(crate) text: TextStyle,
    pub(crate) line_height: f32,
    pub(crate) color: Rgba,
}

#[derive(Copy, Clone)]
pub(crate) struct EditorPaintParams<'a> {
    pub(crate) style: &'a EditorPaintStyle,
    pub(crate) palette: Palette,
    pub(crate) window_active: bool,
    pub(crate) focused: bool,
    pub(crate) show_selection: bool,
    pub(crate) scale: f32,
    pub(crate) caret_width_px: u32,
}

pub(crate) enum VisualRow {
    Shaped(ShapedRow),
    Windowed {
        start_byte: usize,
        end_byte: usize,
        total_bytes: usize,
        start_x: f32,
        total_width: f32,
        line: ShapedRow,
    },
    Placeholder(usize),
}

impl From<ShapedRow> for VisualRow {
    fn from(line: ShapedRow) -> Self {
        Self::Shaped(line)
    }
}

impl VisualRow {
    #[inline]
    pub(crate) fn byte_len(&self) -> usize {
        match self {
            Self::Shaped(line) => line.len,
            Self::Windowed { total_bytes, .. } => *total_bytes,
            Self::Placeholder(len) => *len,
        }
    }

    #[inline]
    pub(crate) fn width(&self) -> f32 {
        match self {
            Self::Shaped(line) => line.width,
            Self::Windowed { total_width, .. } => *total_width,
            Self::Placeholder(_) => 0.0,
        }
    }

    #[inline]
    pub(crate) fn is_shaped(&self) -> bool {
        !matches!(self, Self::Placeholder(_))
    }

    pub(crate) fn x_for_index(&self, ix: usize) -> f32 {
        match self {
            Self::Shaped(line) => line.x_for_index(ix),
            Self::Windowed {
                start_byte,
                end_byte,
                start_x,
                total_width,
                line,
                ..
            } => {
                if ix <= *start_byte {
                    *start_x
                } else if ix >= *end_byte {
                    (*start_x + line.width).min(*total_width)
                } else {
                    *start_x + line.x_for_index(ix - *start_byte)
                }
            }
            Self::Placeholder(_) => 0.0,
        }
    }

    pub(crate) fn selection_spans(&self, start: usize, end: usize) -> Vec<(f32, f32)> {
        if start >= end {
            return Vec::new();
        }
        match self {
            Self::Shaped(line) => line.selection_spans(start, end),
            Self::Windowed {
                start_byte,
                end_byte,
                start_x,
                line,
                ..
            } => {
                let s = start.max(*start_byte);
                let e = end.min(*end_byte);
                if s >= e {
                    return Vec::new();
                }
                line.selection_spans(s - *start_byte, e - *start_byte)
                    .into_iter()
                    .map(|(l, r)| (l + *start_x, r + *start_x))
                    .collect()
            }
            Self::Placeholder(_) => Vec::new(),
        }
    }

    pub(crate) fn closest_index_for_x(&self, x: f32) -> usize {
        match self {
            Self::Shaped(line) => line.closest_index_for_x(x),
            Self::Windowed {
                start_byte,
                start_x,
                line,
                ..
            } => *start_byte + line.closest_index_for_x((x - *start_x).max(0.0)),
            Self::Placeholder(_) => 0,
        }
    }

    pub(crate) fn paint(
        &self,
        origin: Point,
        line_height: f32,
        color: Rgba,
        underline: Option<Range<usize>>,
        canvas: &mut Canvas,
    ) {
        match self {
            Self::Shaped(line) => {
                canvas.draw_row(line, origin, line_height, color, underline);
            }
            Self::Windowed {
                start_byte,
                end_byte,
                start_x,
                line,
                ..
            } => {
                let ul = underline.and_then(|r| {
                    let s = r.start.max(*start_byte);
                    let e = r.end.min(*end_byte);
                    (s < e).then_some((s - *start_byte)..(e - *start_byte))
                });
                canvas.draw_row(line, origin + point(*start_x, 0.0), line_height, color, ul);
            }
            Self::Placeholder(_) => {}
        }
    }
}

pub(crate) struct RowLayout {
    pub(crate) first_sub_row: usize,
    pub(crate) leading_bytes: usize,
    pub(crate) total_sub_rows: usize,
    pub(crate) rows: Vec<VisualRow>,
    pub(crate) hanging_indent: f32,
    max_width: f32,
}

impl RowLayout {
    pub(crate) fn from_slice(
        rows: Vec<VisualRow>,
        first_sub_row: usize,
        leading_bytes: usize,
        total_sub_rows: usize,
        hanging_indent: f32,
    ) -> Self {
        let max_width = rows.iter().map(VisualRow::width).fold(0.0f32, f32::max);
        Self {
            first_sub_row,
            leading_bytes,
            total_sub_rows: total_sub_rows.max(rows.len()),
            rows,
            hanging_indent,
            max_width,
        }
    }

    pub(crate) fn from_shaped(lines: Vec<ShapedRow>) -> Self {
        let total = lines.len();
        let rows: Vec<VisualRow> = lines.into_iter().map(VisualRow::from).collect();
        Self::from_slice(rows, 0, 0, total, 0.0)
    }

    #[inline]
    pub(crate) fn indent_for(&self, abs_sub_row: usize) -> f32 {
        if abs_sub_row == 0 {
            0.0
        } else {
            self.hanging_indent
        }
    }

    pub(crate) fn position_for_index(
        &self,
        local_offset: usize,
        line_height: f32,
        end_affinity: bool,
    ) -> Option<Point> {
        let mut cursor = self.leading_bytes;
        let mut y = self.first_sub_row as f32 * line_height;
        for (ix, row) in self.rows.iter().enumerate() {
            let abs_sub = self.first_sub_row + ix;
            let is_last = abs_sub + 1 == self.total_sub_rows;
            let len = row.byte_len();
            let hit = if len == 0 {
                local_offset == cursor
            } else if is_last || end_affinity {
                local_offset >= cursor && local_offset <= cursor + len
            } else {
                local_offset >= cursor && local_offset < cursor + len
            };
            if hit {
                let x =
                    row.x_for_index(local_offset.saturating_sub(cursor)) + self.indent_for(abs_sub);
                return Some(point(x, y));
            }
            cursor += if is_last { len + 1 } else { len };
            y += line_height;
        }
        None
    }

    pub(crate) fn closest_index_for_x(&self, x: f32) -> usize {
        let mut cursor = self.leading_bytes;
        for (ix, row) in self.rows.iter().enumerate() {
            let abs_sub = self.first_sub_row + ix;
            let indent = self.indent_for(abs_sub);
            if x <= indent + row.width() || ix + 1 == self.rows.len() {
                return cursor + row.closest_index_for_x(x - indent);
            }
            cursor += row.byte_len();
        }
        cursor
    }

    pub(crate) fn closest_index_for_x_in_last_row(&self, x: f32) -> usize {
        let Some((ix, row)) = self.rows.iter().enumerate().next_back() else {
            return self.leading_bytes;
        };
        let abs_sub = self.first_sub_row + ix;
        let base = self.leading_bytes
            + self.rows[..ix]
                .iter()
                .map(VisualRow::byte_len)
                .sum::<usize>();
        let indent = self.indent_for(abs_sub);
        base + row.closest_index_for_x(x - indent)
    }

    pub(crate) fn closest_index_for_position(
        &self,
        pos: Point,
        line_height: f32,
    ) -> Option<(usize, bool)> {
        let mut offset = self.leading_bytes;
        let mut top = self.first_sub_row as f32 * line_height;
        for (ix, row) in self.rows.iter().enumerate() {
            let abs_sub = self.first_sub_row + ix;
            let bottom = top + line_height;
            if pos.y >= top && pos.y < bottom {
                let x = pos.x - self.indent_for(abs_sub);
                let col = row.closest_index_for_x(x);
                let end_affinity = abs_sub + 1 < self.total_sub_rows && col == row.byte_len();
                return Some((offset + col, end_affinity));
            }
            offset += row.byte_len();
            top = bottom;
        }
        None
    }

    pub(crate) fn size(&self, line_height: f32) -> Size {
        let width = self
            .rows
            .iter()
            .enumerate()
            .map(|(ix, row)| row.width() + self.indent_for(self.first_sub_row + ix))
            .fold(self.max_width, f32::max);
        size(width, self.total_sub_rows as f32 * line_height)
    }

    pub(crate) fn paint(
        &self,
        origin: Point,
        line_height: f32,
        color: Rgba,
        underline: Option<Range<usize>>,
        canvas: &mut Canvas,
    ) {
        let mut cursor = self.leading_bytes;
        for (ix, row) in self.rows.iter().enumerate() {
            let abs_sub = self.first_sub_row + ix;
            let len = row.byte_len();
            let sub_ul = underline.as_ref().and_then(|r| {
                let s = r.start.max(cursor);
                let e = r.end.min(cursor + len);
                (s < e).then_some((s - cursor)..(e - cursor))
            });
            row.paint(
                origin + point(self.indent_for(abs_sub), abs_sub as f32 * line_height),
                line_height,
                color,
                sub_ul,
                canvas,
            );
            cursor += len;
        }
    }
}

pub(super) struct VisibleLine {
    pub(super) byte_offset: usize,
    pub(super) top: f32,
    pub(super) layout: RowLayout,
}

pub(super) struct FrameLayout {
    pub(super) revision: u64,
    pub(super) scroll_x: f32,
    pub(super) scroll_y: f64,
    pub(super) input_bounds: Rect,
    pub(super) content_bounds: Rect,
    pub(super) first_line: usize,
    pub(super) lines: Vec<VisibleLine>,
    pub(super) visible_byte_range: Range<usize>,
    pub(super) line_height: f32,
    pub(super) wrap_width: Option<f32>,
    pub(super) space_width: f32,
    pub(super) text_color: Rgba,
    pub(super) caret_rect: Rect,
    pub(super) show_caret: bool,
    pub(super) selection_rects: Vec<Rect>,
    pub(super) marked_range: Option<Range<usize>>,
    pub(super) last_cursor: usize,
}

impl FrameLayout {
    pub(super) fn row_layout(&self, buffer_row: usize) -> Option<&RowLayout> {
        let ix = buffer_row.checked_sub(self.first_line)?;
        self.lines.get(ix).map(|l| &l.layout)
    }

    pub(super) fn position_for_offset(
        &self,
        text: &Rope,
        offset: usize,
        end_affinity: bool,
    ) -> Option<Point> {
        let row = text.offset_to_point(offset).row;
        let line = self.lines.get(row.checked_sub(self.first_line)?)?;
        let local = offset.saturating_sub(line.byte_offset);
        let pos = line
            .layout
            .position_for_index(local, self.line_height, end_affinity)?;
        Some(point(pos.x, line.top + pos.y))
    }

    pub(super) fn bounds_for_range(&self, text: &Rope, range: Range<usize>) -> Option<Rect> {
        let start_pos = self
            .position_for_offset(text, range.start, false)
            .or_else(|| self.position_for_offset(text, self.last_cursor, false))?;
        let end_pos = self
            .position_for_offset(text, range.end, false)
            .unwrap_or(start_pos);
        let origin = self.content_bounds.origin;
        Some(Rect::from_corners(
            origin + start_pos,
            origin + point(end_pos.x, start_pos.y + self.line_height),
        ))
    }

    pub(super) fn hit_test(
        &self,
        pos: Point,
        scroll_x: f32,
        scroll_y: f64,
        text_len: usize,
        multi_line: bool,
    ) -> (usize, bool) {
        if self.lines.is_empty() {
            return (0, false);
        }
        let lh = self.line_height;
        let rel = pos
            - self.content_bounds.origin
            - point(scroll_x - self.scroll_x, (scroll_y - self.scroll_y) as f32);

        for line in &self.lines {
            let local_pos = rel - point(0.0, line.top);
            if !multi_line {
                let col = line.layout.closest_index_for_x(local_pos.x);
                return ((line.byte_offset + col).min(text_len), false);
            }
            if let Some((col, aff)) = line.layout.closest_index_for_position(local_pos, lh) {
                return ((line.byte_offset + col).min(text_len), aff);
            } else if local_pos.y < 0.0 {
                return (line.byte_offset.min(text_len), false);
            }
        }

        if let Some(last) = self.lines.last() {
            let col = last.layout.closest_index_for_x_in_last_row(rel.x);
            return ((last.byte_offset + col).min(text_len), false);
        }
        (text_len, false)
    }

    pub(super) fn paint(&self, p: Palette, canvas: &mut Canvas) {
        canvas.with_clip(self.input_bounds, |canvas| {
            for &rect in &self.selection_rects {
                canvas.paint_quad(fill(rect, p.selection));
            }

            let origin = self.content_bounds.origin;
            for line in &self.lines {
                let line_base = line.byte_offset;
                let line_marked = self.marked_range.as_ref().and_then(|m| {
                    (m.end > line_base).then(|| {
                        m.start.saturating_sub(line_base)..m.end.saturating_sub(line_base)
                    })
                });
                line.layout.paint(
                    point(origin.x, origin.y + line.top),
                    self.line_height,
                    self.text_color,
                    line_marked,
                    canvas,
                );
            }

            if self.show_caret {
                canvas.paint_quad(fill(self.caret_rect, p.caret));
            }
        });
    }

    pub(super) fn range_rects(&self, range: &Range<usize>) -> Vec<Rect> {
        let range = range.start.max(self.visible_byte_range.start)
            ..range.end.min(self.visible_byte_range.end);
        if range.is_empty() {
            return Vec::new();
        }
        let origin = self.content_bounds.origin;
        let mut out = Vec::new();
        for line in &self.lines {
            let mut off = line.byte_offset + line.layout.leading_bytes;
            for (i, row) in line.layout.rows.iter().enumerate() {
                let abs_sub = line.layout.first_sub_row + i;
                let row_y = line.top + abs_sub as f32 * self.line_height;
                let end = off + row.byte_len();
                if !row.is_shaped() {
                    off = end;
                    continue;
                }
                let s = range.start.max(off);
                let e = range.end.min(end);
                let is_last = abs_sub + 1 == line.layout.total_sub_rows;
                let includes_nl = is_last && range.start <= end && range.end > end;
                let indent = line.layout.indent_for(abs_sub);
                if s < e {
                    for (span_l, span_r) in row.selection_spans(s - off, e - off) {
                        out.push(
                            Rect::from_corners(
                                point(indent + span_l, row_y),
                                point(indent + span_r, row_y + self.line_height),
                            )
                            .translate(origin),
                        );
                    }
                }
                if includes_nl {
                    let nl_left = indent + row.width();
                    out.push(
                        Rect::from_corners(
                            point(nl_left, row_y),
                            point(nl_left + self.space_width, row_y + self.line_height),
                        )
                        .translate(origin),
                    );
                }
                off = end;
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_layout_hanging_indent_and_end_affinity_geometry() {
        let mut s1 = ShapedRow::default();
        s1.len = 10;
        let mut s2 = ShapedRow::default();
        s2.len = 5;
        let layout = RowLayout::from_slice(
            vec![VisualRow::from(s1), VisualRow::from(s2)],
            0,
            0,
            2,
            20.,
        );
        let (ix, affinity) = layout
            .closest_index_for_position(point(999., 5.), 20.)
            .unwrap();
        assert_eq!(ix, 10);
        assert!(affinity);
        assert_eq!(layout.position_for_index(10, 20., true).map(|p| p.y), Some(0.));
        assert_eq!(layout.position_for_index(10, 20., false), Some(point(20., 20.)));
    }
}
