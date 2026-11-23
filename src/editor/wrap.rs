//! Incremental soft-wrap index, chunked line tree, and shaped visual rows.

use std::borrow::Cow;
use std::ops::{Range, RangeInclusive};

use ropey::Rope;
#[cfg(test)]
use unicode_segmentation::UnicodeSegmentation as _;

use crate::geom::{Point, Size, point, size};
use crate::gfx::{Boundary, D2DRenderer, Font, ShapedLine, TextAlign, TextSystem};
#[cfg(test)]
use crate::gfx::{MAX_WRAP_INDENT, line_break_opportunities};

use super::buffer::{RopeExt, TextPoint};

const CHUNK_SIZE: usize = 256;

/// One visual sub-row within a logical buffer line: shaped for visible/cursor rows, windowed for
/// huge unwrapped lines (P-2), or placeholder otherwise.
pub(crate) enum VisualRow {
    Shaped(ShapedLine),
    Windowed {
        start_byte: usize,
        end_byte: usize,
        total_bytes: usize,
        start_x: f32,
        total_width: f32,
        line: ShapedLine,
    },
    Placeholder(usize),
}

impl From<ShapedLine> for VisualRow {
    fn from(line: ShapedLine) -> Self {
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

    pub(crate) fn paint(&self, origin: Point, line_height: f32, renderer: &mut D2DRenderer) {
        match self {
            Self::Shaped(line) => {
                line.paint(origin, line_height, TextAlign::Left, None, renderer);
            }
            Self::Windowed { start_x, line, .. } => {
                line.paint(
                    origin + point(*start_x, 0.0),
                    line_height,
                    TextAlign::Left,
                    None,
                    renderer,
                );
            }
            Self::Placeholder(_) => {}
        }
    }
}

/// Shaped visual sub-rows for a visible logical buffer line (or a visible sub-row slice on
/// huge wrapped lines, P-3).
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

    pub(crate) fn from_shaped(lines: Vec<ShapedLine>) -> Self {
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

    pub(crate) fn paint(&self, origin: Point, line_height: f32, renderer: &mut D2DRenderer) {
        for (ix, row) in self.rows.iter().enumerate() {
            let abs_sub = self.first_sub_row + ix;
            row.paint(
                origin + point(self.indent_for(abs_sub), abs_sub as f32 * line_height),
                line_height,
                renderer,
            );
        }
    }
}

/// Visual coordinate inside soft-wrapped text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DisplayPos {
    pub(crate) row: usize,
    pub(crate) sub_row: usize,
    pub(crate) col: usize,
}

impl DisplayPos {
    #[inline]
    pub(crate) fn new(row: usize, sub_row: usize, col: usize) -> Self {
        Self { row, sub_row, col }
    }
}

/// Inline single-range storage avoiding per-line heap allocations for non-wrapping lines (P-4).
#[derive(Debug, Clone)]
pub(crate) enum LineRanges {
    Single(usize),
    Wrapped(Box<[Range<usize>]>),
}

impl LineRanges {
    #[inline]
    pub(crate) fn len(&self) -> usize {
        match self {
            Self::Single(_) => 1,
            Self::Wrapped(s) => s.len(),
        }
    }

    #[inline]
    pub(crate) fn get(&self, index: usize) -> Option<Range<usize>> {
        match self {
            Self::Single(byte_len) => (index == 0).then_some(0..*byte_len),
            Self::Wrapped(s) => s.get(index).cloned(),
        }
    }

    #[inline]
    pub(crate) fn last(&self) -> Range<usize> {
        match self {
            Self::Single(byte_len) => 0..*byte_len,
            Self::Wrapped(s) => s.last().cloned().unwrap_or(0..0),
        }
    }
}

/// Soft-wrap ranges for a single logical buffer line.
#[derive(Debug, Clone)]
pub(crate) struct WrappedLine {
    pub(crate) byte_len: usize,
    pub(crate) indent_spaces: u32,
    pub(crate) ranges: LineRanges,
}

impl WrappedLine {
    #[inline]
    pub(crate) fn row_count(&self) -> usize {
        self.ranges.len()
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct ChunkSummary {
    buffer_lines: usize,
    visual_rows: usize,
    max_line_bytes: usize,
    longest_line: usize,
}

impl ChunkSummary {
    #[inline]
    fn single(line: &WrappedLine) -> Self {
        Self {
            buffer_lines: 1,
            visual_rows: line.row_count(),
            max_line_bytes: line.byte_len,
            longest_line: 0,
        }
    }

    #[inline]
    fn combine(&mut self, other: &Self) {
        if other.max_line_bytes > self.max_line_bytes {
            self.longest_line = self.buffer_lines + other.longest_line;
            self.max_line_bytes = other.max_line_bytes;
        }
        self.buffer_lines += other.buffer_lines;
        self.visual_rows += other.visual_rows;
    }

    fn of_slice(lines: &[WrappedLine]) -> Self {
        let mut total = Self::default();
        for line in lines {
            total.combine(&Self::single(line));
        }
        total
    }
}

#[derive(Debug, Clone)]
struct LineChunk {
    summary: ChunkSummary,
    items: Vec<WrappedLine>,
}

impl LineChunk {
    fn new(items: Vec<WrappedLine>) -> Self {
        let summary = ChunkSummary::of_slice(&items);
        Self { summary, items }
    }
}

#[derive(Debug, Clone, Default)]
struct LineChunks {
    chunks: Vec<LineChunk>,
    total: ChunkSummary,
}

impl LineChunks {
    fn from_vec(items: Vec<WrappedLine>) -> Self {
        let mut chunks = Vec::with_capacity(items.len().div_ceil(CHUNK_SIZE));
        let mut iter = items.into_iter();
        while iter.len() > 0 {
            chunks.push(LineChunk::new(iter.by_ref().take(CHUNK_SIZE).collect()));
        }
        let mut tree = Self {
            chunks,
            total: ChunkSummary::default(),
        };
        tree.recompute_total();
        tree
    }

    fn locate_line(&self, line: usize) -> (usize, usize) {
        let mut acc = 0;
        for (ix, chunk) in self.chunks.iter().enumerate() {
            if line < acc + chunk.summary.buffer_lines {
                return (ix, line - acc);
            }
            acc += chunk.summary.buffer_lines;
        }
        (self.chunks.len(), 0)
    }

    fn get_by_line(&self, line: usize) -> (usize, Option<&WrappedLine>) {
        let mut line_acc = 0;
        let mut row_acc = 0;
        for chunk in &self.chunks {
            if line < line_acc + chunk.summary.buffer_lines {
                let local = line - line_acc;
                for item in &chunk.items[..local] {
                    row_acc += item.row_count();
                }
                return (row_acc, Some(&chunk.items[local]));
            }
            line_acc += chunk.summary.buffer_lines;
            row_acc += chunk.summary.visual_rows;
        }
        (self.total.visual_rows, None)
    }

    fn get_by_visual_row(&self, visual_row: usize) -> Option<(usize, usize, &WrappedLine)> {
        let mut line_acc = 0;
        let mut row_acc = 0;
        for chunk in &self.chunks {
            if visual_row < row_acc + chunk.summary.visual_rows {
                for (local, item) in chunk.items.iter().enumerate() {
                    let next = row_acc + item.row_count();
                    if visual_row < next {
                        return Some((line_acc + local, row_acc, item));
                    }
                    row_acc = next;
                }
            }
            line_acc += chunk.summary.buffer_lines;
            row_acc += chunk.summary.visual_rows;
        }
        None
    }

    fn splice(&mut self, range: Range<usize>, replacement: Vec<WrappedLine>) {
        if self.total.buffer_lines == 0 {
            *self = Self::from_vec(replacement);
            return;
        }
        let start = range.start.min(self.total.buffer_lines);
        let end = range.end.clamp(start, self.total.buffer_lines);
        let (start_chunk, start_local) = self.locate_line(start);
        let (end_chunk, end_local) = self.locate_line(end);
        let replace_end = if end_chunk < self.chunks.len() {
            end_chunk + 1
        } else {
            self.chunks.len()
        };

        let prefix = if start_chunk < self.chunks.len() {
            start_local
        } else {
            0
        };
        let suffix = if end_chunk < self.chunks.len() {
            self.chunks[end_chunk].items.len() - end_local
        } else {
            0
        };

        let mut merged = Vec::with_capacity(prefix + replacement.len() + suffix);
        if start_chunk < self.chunks.len() {
            merged.extend(self.chunks[start_chunk].items[..start_local].iter().cloned());
        }
        merged.extend(replacement);
        if end_chunk < self.chunks.len() {
            merged.extend(self.chunks[end_chunk].items[end_local..].iter().cloned());
        }

        let mut rebuilt = Vec::with_capacity(merged.len().div_ceil(CHUNK_SIZE));
        let mut iter = merged.into_iter();
        while iter.len() > 0 {
            rebuilt.push(LineChunk::new(iter.by_ref().take(CHUNK_SIZE).collect()));
        }
        self.chunks.splice(start_chunk..replace_end, rebuilt);
        // Rebalance under-full neighboring chunks so repeated small edits never fragment (P-4).
        let mut idx = start_chunk.saturating_sub(1);
        while idx + 1 < self.chunks.len() && idx <= start_chunk + 2 {
            let combined = self.chunks[idx].items.len() + self.chunks[idx + 1].items.len();
            if combined <= CHUNK_SIZE {
                let next_chunk = self.chunks.remove(idx + 1);
                self.chunks[idx].items.extend(next_chunk.items);
                self.chunks[idx].summary = ChunkSummary::of_slice(&self.chunks[idx].items);
            } else {
                idx += 1;
            }
        }
        self.recompute_total();
    }

    fn recompute_total(&mut self) {
        let mut total = ChunkSummary::default();
        for chunk in &self.chunks {
            total.combine(&chunk.summary);
        }
        self.total = total;
    }
}

const CHAR_WIDTH_RANGES: [RangeInclusive<u32>; 17] = [
    0x0020..=0x007e,
    0x00a0..=0x02ff,
    0x0370..=0x0482,
    0x048a..=0x052f,
    0x1e00..=0x1fff,
    0x2010..=0x2027,
    0x2030..=0x205e,
    0x2070..=0x20cf,
    0x2100..=0x2bff,
    0x3000..=0x3029,
    0x3030..=0x3098,
    0x309b..=0x30ff,
    0x3400..=0x4dbf,
    0x4e00..=0x9fff,
    0xac00..=0xd7a3,
    0xf900..=0xfaff,
    0xff01..=0xffee,
];

#[inline]
fn is_printable_ascii(bytes: &[u8]) -> bool {
    bytes.iter().all(|&b| (0x20..=0x7e).contains(&b))
}

#[inline]
fn is_printable_ascii_or_tab(bytes: &[u8]) -> bool {
    bytes.iter().all(|&b| b == b'\t' || (0x20..=0x7e).contains(&b))
}

pub(crate) fn can_wrap_with_char_widths(text: &str) -> bool {
    if is_printable_ascii_or_tab(text.as_bytes()) {
        return true;
    }
    text.chars().all(|c| {
        c == '\t' || CHAR_WIDTH_RANGES.iter().any(|r| r.contains(&(c as u32)))
    })
}

#[cfg(test)]
fn measured_wrap_breaks(
    text: &str,
    width: f32,
    preserve_indent: bool,
    mut measure: impl FnMut(&str) -> f32,
    out: &mut Vec<Boundary>,
) {
    let indent = if preserve_indent {
        text.chars()
            .take_while(|&c| c == ' ')
            .count()
            .min(MAX_WRAP_INDENT as usize)
    } else {
        0
    };
    let indent_width = if indent > 0 {
        measure(&text[..indent])
    } else {
        0.0
    };
    let grapheme_ends: Vec<usize> = text
        .grapheme_indices(true)
        .map(|(ix, g)| ix + g.len())
        .collect();
    let opportunities: Vec<usize> = line_break_opportunities(text)
        .into_iter()
        .filter(|ix| grapheme_ends.binary_search(ix).is_ok())
        .collect();

    let mut first = 0;
    let mut start = 0;
    while first < grapheme_ends.len() {
        let available = if start == 0 {
            width
        } else {
            width - indent_width
        };
        let remaining = grapheme_ends.len() - first;
        let mut lo = 0;
        let mut hi = 1;
        while measure(&text[start..grapheme_ends[first + hi - 1]]) <= available {
            lo = hi;
            if hi == remaining {
                break;
            }
            hi = (hi * 2).min(remaining);
        }
        while lo + 1 < hi {
            let mid = (lo + hi) / 2;
            if measure(&text[start..grapheme_ends[first + mid - 1]]) <= available {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let fitting_end = grapheme_ends[first + lo.max(1) - 1];
        if fitting_end == text.len() {
            break;
        }
        let idx = opportunities.partition_point(|&ix| ix <= fitting_end);
        let chosen = idx
            .checked_sub(1)
            .map(|i| opportunities[i])
            .filter(|&ix| ix > start && (start != 0 || ix > indent))
            .unwrap_or(fitting_end);
        out.push(Boundary {
            ix: chosen,
            next_indent: indent as u32,
        });
        first = grapheme_ends.partition_point(|&ix| ix <= chosen);
        start = chosen;
    }
}

/// Incremental soft-wrap index mapping logical buffer lines to visual rows.
pub(crate) struct WrapIndex {
    text: Rope,
    font: Font,
    font_size: f32,
    wrap_width: Option<f32>,
    chunks: LineChunks,
    initialized: bool,
}

impl WrapIndex {
    pub(crate) fn new(font: Font, font_size: f32, wrap_width: Option<f32>) -> Self {
        Self {
            text: Rope::new(),
            font,
            font_size,
            wrap_width,
            chunks: LineChunks::default(),
            initialized: false,
        }
    }

    #[inline]
    pub(crate) fn visual_row_count(&self) -> usize {
        self.chunks.total.visual_rows
    }

    #[inline]
    pub(crate) fn line_count(&self) -> usize {
        self.chunks.total.buffer_lines
    }

    #[inline]
    pub(crate) fn longest_line(&self) -> usize {
        self.chunks.total.longest_line
    }

    #[inline]
    pub(crate) fn line(&self, row: usize) -> Option<&WrappedLine> {
        self.chunks.get_by_line(row).1
    }

    pub(crate) fn first_visual_row(&self, line: usize) -> usize {
        self.chunks.get_by_line(line).0
    }

    pub(crate) fn visual_row_to_line(&self, visual_row: usize) -> usize {
        self.chunks
            .get_by_visual_row(visual_row)
            .map_or_else(|| self.line_count().saturating_sub(1), |(line, _, _)| line)
    }

    pub(crate) fn visual_row_to_line_and_sub_row(&self, visual_row: usize) -> (usize, usize) {
        self.chunks.get_by_visual_row(visual_row).map_or_else(
            || (self.line_count().saturating_sub(1), 0),
            |(line, first_row, _)| (line, visual_row.saturating_sub(first_row)),
        )
    }

    pub(crate) fn visual_row_range(&self, line_ix: usize, sub_row: usize) -> Option<(Range<usize>, bool)> {
        let line = self.line(line_ix)?;
        let range = line.ranges.get(sub_row)?;
        let soft_end = sub_row + 1 < line.row_count();
        Some((range, soft_end))
    }

    pub(crate) fn font(&self) -> &Font {
        &self.font
    }

    pub(crate) fn font_size(&self) -> f32 {
        self.font_size
    }

    pub(crate) fn wrap_width(&self) -> Option<f32> {
        self.wrap_width
    }

    pub(crate) fn set_wrap_width(&mut self, wrap_width: Option<f32>, ts: &TextSystem) {
        if self.wrap_width == wrap_width {
            return;
        }
        self.wrap_width = wrap_width;
        let snapshot = self.text.clone();
        self.rebuild_all(&snapshot, ts);
    }

    pub(crate) fn sync_layout(
        &mut self,
        text: &Rope,
        font: Font,
        font_size: f32,
        wrap_width: Option<f32>,
        ts: &TextSystem,
    ) {
        let font_changed = self.font != font || self.font_size != font_size;
        let width_changed = self.wrap_width != wrap_width;
        self.font = font;
        self.font_size = font_size;
        self.wrap_width = wrap_width;

        if !self.initialized {
            self.initialized = true;
            self.rebuild_all(text, ts);
            return;
        }
        if wrap_width.is_none() && !width_changed {
            return;
        }
        if font_changed || width_changed {
            self.rebuild_all(text, ts);
        }
    }

    pub(crate) fn apply_edit(
        &mut self,
        updated_text: &Rope,
        old_range: &Range<usize>,
        inserted_bytes: usize,
        ts: &TextSystem,
    ) {
        let Some(wrap_width) = self.wrap_width else {
            self.splice_lines(
                updated_text,
                old_range,
                inserted_bytes,
                0,
                &mut |_, _, _| {},
            );
            return;
        };

        let font = self.font.clone();
        let font_size = self.font_size;
        ts.with_char_widths(&font, font_size, |char_widths, dw| {
            let max_ascii = char_widths.max_printable_ascii();
            let ascii_fit_len = if max_ascii > 0.0 {
                (wrap_width / max_ascii).floor().max(0.0) as usize
            } else {
                0
            };
            self.splice_lines(
                updated_text,
                old_range,
                inserted_bytes,
                ascii_fit_len,
                &mut |line_str, width, out| {
                    if can_wrap_with_char_widths(line_str) {
                        char_widths.wrap_line_into(line_str, width, dw, out);
                    } else {
                        // Single-pass DirectWrite layout wrapping for complex scripts/emoji (P-1).
                        dw.wrap_line_breaks(line_str, &font, font_size, width, true, out);
                    }
                },
            );
        });
    }

    fn splice_lines<F>(
        &mut self,
        updated_text: &Rope,
        old_range: &Range<usize>,
        inserted_bytes: usize,
        ascii_fit_len: usize,
        wrap_line: &mut F,
    ) where
        F: FnMut(&str, f32, &mut Vec<Boundary>),
    {
        let existing_count = self.line_count();
        let old_start = self
            .text
            .offset_to_point(old_range.start)
            .row
            .min(existing_count.saturating_sub(1));
        let old_end = self
            .text
            .offset_to_point(old_range.end)
            .row
            .min(existing_count.saturating_sub(1));

        let new_start = updated_text.offset_to_point(old_range.start).row;
        let new_end = updated_text
            .offset_to_point(old_range.start + inserted_bytes)
            .row;

        let mut replacement = Vec::with_capacity(new_end.saturating_sub(new_start) + 1);
        let mut breaks = Vec::new();
        let wrap_width = self.wrap_width;
        let total_rows = updated_text.lines_len();
        let mut iter = updated_text.lines_at(new_start.min(total_rows), ropey::LineType::LF);

        for _ in new_start..=new_end {
            let slice = match iter.next() {
                Some(raw) if raw.len() > 0 && raw.byte(raw.len() - 1) == b'\n' => {
                    raw.slice(..raw.len() - 1)
                }
                Some(raw) => raw,
                None => updated_text.slice(0..0),
            };
            let byte_len = slice.len();
            let mut indent_spaces = 0;
            let ranges = if let Some(width) = wrap_width
                && byte_len > 0
            {
                let fast_ascii = ascii_fit_len > 0
                    && byte_len <= ascii_fit_len
                    && slice.chunks().all(|c| is_printable_ascii(c.as_bytes()));
                if fast_ascii {
                    LineRanges::Single(byte_len)
                } else {
                    let line_str: Cow<str> = slice.into();
                    breaks.clear();
                    wrap_line(&line_str, width, &mut breaks);
                    if breaks.is_empty() {
                        LineRanges::Single(byte_len)
                    } else {
                        let mut v = Vec::with_capacity(breaks.len() + 1);
                        let mut cursor = 0;
                        for &b in &breaks {
                            v.push(cursor..b.ix);
                            cursor = b.ix;
                            indent_spaces = b.next_indent;
                        }
                        if cursor < byte_len {
                            v.push(cursor..byte_len);
                        }
                        LineRanges::Wrapped(v.into_boxed_slice())
                    }
                }
            } else {
                LineRanges::Single(byte_len)
            };

            replacement.push(WrappedLine {
                byte_len,
                indent_spaces,
                ranges,
            });
        }

        self.chunks.splice(old_start..old_end + 1, replacement);
        self.text = updated_text.clone();
    }

    pub(crate) fn rebuild_all(&mut self, text: &Rope, ts: &TextSystem) {
        self.apply_edit(text, &(0..self.text.len()), text.len(), ts);
    }

    pub(crate) fn offset_to_display(&self, offset: usize, end_affinity: bool) -> DisplayPos {
        let row = self.text.offset_to_point(offset).row;
        let start = self.text.line_start_offset(row);
        let (first_row, Some(line)) = self.chunks.get_by_line(row) else {
            return DisplayPos::new(self.visual_row_count(), 0, 0);
        };
        let local = offset.saturating_sub(start);
        match &line.ranges {
            LineRanges::Single(byte_len) => {
                DisplayPos::new(first_row, 0, local.min(*byte_len))
            }
            LineRanges::Wrapped(ranges) => {
                for (sub_row, range) in ranges.iter().enumerate() {
                    if range.contains(&local) || (end_affinity && local == range.end) {
                        return DisplayPos::new(
                            first_row + sub_row,
                            sub_row,
                            local.saturating_sub(range.start),
                        );
                    }
                }
                let last = line.ranges.last();
                let sub_row = line.row_count().saturating_sub(1);
                DisplayPos::new(first_row + sub_row, sub_row, last.len())
            }
        }
    }

    pub(crate) fn display_to_offset(&self, pos: DisplayPos) -> usize {
        let Some((line_ix, first_row, line)) = self.chunks.get_by_visual_row(pos.row) else {
            return self.text.len();
        };
        let line_start = self.text.line_start_offset(line_ix);
        let sub_row = pos.row.saturating_sub(first_row);
        match line.ranges.get(sub_row) {
            Some(range) => line_start + (range.start + pos.col).min(range.end),
            None => line_start + line.byte_len,
        }
    }

    pub(crate) fn display_to_point(&self, pos: DisplayPos) -> TextPoint {
        self.text.offset_to_point(self.display_to_offset(pos))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gfx::font;

    fn test_font() -> Font {
        font("Arial")
    }

    #[test]
    fn measured_wrap_preserves_words_graphemes_cjk_and_indentation() {
        let wrap = |text: &str, width, preserve_indent| {
            let mut out = Vec::new();
            measured_wrap_breaks(
                text,
                width,
                preserve_indent,
                |s| s.graphemes(true).count() as f32,
                &mut out,
            );
            out.into_iter().map(|b| b.ix).collect::<Vec<_>>()
        };

        assert_eq!(wrap("hello world", 8., false), vec![6]);
        assert_eq!(wrap("a👩‍💻b", 2., false), vec!["a👩‍💻".len()]);
        assert_eq!(wrap("  abcdefgh", 5., true), vec![5, 8]);
        assert_eq!(wrap("  abcdefgh", 5., false), vec![2, 7]);
        assert!(wrap("", 0., false).is_empty());
        assert_eq!(wrap("abc", 0., false), vec![1, 2]);
        assert_eq!(wrap("你好，世界", 2., false), vec![3, 9]);
        assert!(line_break_opportunities("foo\u{00A0}bar").is_empty());
        assert_eq!(line_break_opportunities("foo-bar"), vec![4]);
        assert_eq!(wrap("👨‍👩‍👧‍👦 word", 4., false), vec!["👨‍👩‍👧‍👦 ".len()]);
        assert_eq!(wrap("你好😀世界", 3., false), vec!["你好😀".len()]);
    }

    #[test]
    fn incremental_splice_longest_row_and_ascii_fast_fit() {
        let mut index = WrapIndex::new(test_font(), 14., Some(10.));
        let mut text = Rope::from("aa\nbbbb\nc");
        let mut fake = |line: &str, _: f32, out: &mut Vec<Boundary>| {
            if line.len() > 2 {
                out.push(Boundary {
                    ix: 2,
                    next_indent: 0,
                });
            }
        };
        index.splice_lines(&text, &(0..0), text.len(), 0, &mut fake);
        assert_eq!(index.line_count(), 3);
        assert_eq!(index.visual_row_count(), 4);
        assert_eq!(index.longest_line(), 1);
        assert_eq!(index.first_visual_row(2), 3);
        assert_eq!(index.visual_row_to_line(2), 1);

        let range = text.line_start_offset(1)..text.line_end_offset(1);
        let rep = "dd\neeeeee";
        text.replace(range.clone(), rep);
        index.splice_lines(&text, &range, rep.len(), 0, &mut fake);
        assert_eq!(index.line_count(), 4);
        assert_eq!(index.visual_row_count(), 5);
        assert_eq!(index.longest_line(), 2);

        let all = 0..text.len();
        text.replace(all.clone(), "");
        index.splice_lines(&text, &all, 0, 0, &mut fake);
        assert_eq!(index.line_count(), 1);
        assert_eq!(index.visual_row_count(), 1);
        assert_eq!(index.line(0).unwrap().ranges.get(0), Some(0..0));
        assert_eq!(index.line(0).unwrap().ranges.len(), 1);

        let mut fast = WrapIndex::new(test_font(), 14., Some(100.));
        let sample = Rope::from("short ascii\nlonger ascii line\n日本語\n");
        let mut called = Vec::new();
        fast.splice_lines(&sample, &(0..0), sample.len(), 12, &mut |line, _, _| {
            called.push(line.to_string());
        });
        assert_eq!(called, vec!["longer ascii line", "日本語"]);
    }

    #[test]
    fn wrap_boundary_affinity_and_hanging_indent_geometry() {
        let mut index = WrapIndex::new(test_font(), 14., None);
        index.text = Rope::from("first line\nthis one wraps");
        index.chunks = LineChunks::from_vec(vec![
            WrappedLine {
                byte_len: 10,
                indent_spaces: 0,
                ranges: LineRanges::Single(10),
            },
            WrappedLine {
                byte_len: 14,
                indent_spaces: 0,
                ranges: LineRanges::Wrapped(vec![0..9, 9..14].into_boxed_slice()),
            },
        ]);
        assert_eq!(index.offset_to_display(20, true), DisplayPos::new(1, 0, 9));
        assert_eq!(index.offset_to_display(20, false), DisplayPos::new(2, 1, 0));
        assert_eq!(index.display_to_offset(DisplayPos::new(2, 1, 3)), 23);

        let mut s1 = ShapedLine::default();
        s1.len = 10;
        let mut s2 = ShapedLine::default();
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
