use std::borrow::Cow;
use std::ops::{Range, RangeInclusive};

use ropey::Rope;

use crate::color::Rgba;
use crate::geom::{Point, Size, point, size};
use crate::gfx::{AdvanceTable, Canvas, ShapedRow, TextStyle, TextSystem};

use super::buffer::{RopeExt, TextPoint};
use super::model::LineChange;

const CHUNK_SIZE: usize = 256;
const MAX_WRAP_INDENT: u32 = 32;

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

#[derive(Debug, Clone)]
pub(crate) struct WrappedLine {
    pub(crate) byte_len: usize,
    pub(crate) hanging_indent: f32,
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
    line_starts: Vec<usize>,
    row_starts: Vec<usize>,
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
            line_starts: Vec::new(),
            row_starts: Vec::new(),
            total: ChunkSummary::default(),
        };
        tree.recompute_total();
        tree
    }

    fn locate_line(&self, line: usize) -> (usize, usize) {
        if line >= self.total.buffer_lines {
            return (self.chunks.len(), 0);
        }
        let ix = self.line_starts.partition_point(|&start| start <= line) - 1;
        (ix, line - self.line_starts[ix])
    }

    fn get_by_line(&self, line: usize) -> (usize, Option<&WrappedLine>) {
        if line >= self.total.buffer_lines {
            return (self.total.visual_rows, None);
        }
        let ix = self.line_starts.partition_point(|&start| start <= line) - 1;
        let local = line - self.line_starts[ix];
        let mut row_acc = self.row_starts[ix];
        for item in &self.chunks[ix].items[..local] {
            row_acc += item.row_count();
        }
        (row_acc, Some(&self.chunks[ix].items[local]))
    }

    fn get_by_visual_row(&self, visual_row: usize) -> Option<(usize, usize, &WrappedLine)> {
        if visual_row >= self.total.visual_rows {
            return None;
        }
        let ix = self.row_starts.partition_point(|&start| start <= visual_row) - 1;
        let line_acc = self.line_starts[ix];
        let mut row_acc = self.row_starts[ix];
        for (local, item) in self.chunks[ix].items.iter().enumerate() {
            let next = row_acc + item.row_count();
            if visual_row < next {
                return Some((line_acc + local, row_acc, item));
            }
            row_acc = next;
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
        self.line_starts.clear();
        self.row_starts.clear();
        self.line_starts.reserve(self.chunks.len());
        self.row_starts.reserve(self.chunks.len());
        let mut total = ChunkSummary::default();
        for chunk in &self.chunks {
            self.line_starts.push(total.buffer_lines);
            self.row_starts.push(total.visual_rows);
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

pub(crate) fn can_wrap_with_advances(text: &str) -> bool {
    if is_printable_ascii_or_tab(text.as_bytes()) {
        return true;
    }
    text.chars().all(|c| {
        c == '\t' || CHAR_WIDTH_RANGES.iter().any(|r| r.contains(&(c as u32)))
    })
}

fn hanging_indent(line: &str, wrap_width: f32, advances: &mut AdvanceTable) -> f32 {
    let mut chars: u32 = 0;
    let mut width = 0.0f32;
    for ch in line.chars() {
        if (ch == ' ' || ch == '\t') && chars < MAX_WRAP_INDENT {
            width += advances.advance(ch, width);
            chars += 1;
        } else {
            break;
        }
    }
    if width >= wrap_width * 0.5 {
        0.0
    } else {
        width
    }
}

fn wrap_line(
    line: &str,
    wrap_width: f32,
    advances: &mut AdvanceTable,
    ts: &TextSystem,
    out: &mut Vec<usize>,
) -> f32 {
    out.clear();
    if line.is_empty() || !wrap_width.is_finite() || wrap_width <= 0.0 {
        return 0.0;
    }
    let indent_px = hanging_indent(line, wrap_width, advances);
    if !can_wrap_with_advances(line) {
        let cont_width = (wrap_width - indent_px).max(1.0);
        advances.dwrite_breaks(line, wrap_width, cont_width, out);
        return if out.is_empty() { 0.0 } else { indent_px };
    }

    let break_offsets = ts.break_opportunities(line);
    let mut row_start = 0usize;
    let mut row_width = 0.0f32;
    let mut last_break_ix = 0usize;
    let mut break_cursor = 0usize;

    for (ix, ch) in line.char_indices() {
        let char_end = ix + ch.len_utf8();
        while break_cursor < break_offsets.len() && break_offsets[break_cursor] <= ix {
            break_cursor += 1;
        }
        let w = advances.advance(ch, row_width);
        if row_width + w > wrap_width && ix > row_start {
            let split_at = if last_break_ix > row_start {
                last_break_ix
            } else {
                ix
            };
            out.push(split_at);
            row_start = split_at;
            row_width = indent_px;
            for c in line[split_at..char_end].chars() {
                row_width += advances.advance(c, row_width);
            }
            last_break_ix = split_at;
        } else {
            row_width += w;
        }
        if break_cursor < break_offsets.len() && break_offsets[break_cursor] == char_end {
            last_break_ix = char_end;
            break_cursor += 1;
        }
    }
    if out.is_empty() { 0.0 } else { indent_px }
}

pub(crate) fn horizontal_slice(
    line: &str,
    min_x: f32,
    max_x: f32,
    advances: &mut AdvanceTable,
) -> (usize, usize, f32, f32) {
    let mut x = 0.0f32;
    let mut start_byte = 0usize;
    let mut start_x = 0.0f32;
    let mut found_start = false;
    let mut end_byte = line.len();
    let mut found_end = false;

    for (ix, ch) in line.char_indices() {
        let adv = advances.advance(ch, x);
        if !found_start && x + adv >= min_x {
            start_byte = ix;
            start_x = x;
            found_start = true;
        }
        x += adv;
        if found_start && !found_end && x >= max_x {
            end_byte = ix + ch.len_utf8();
            found_end = true;
        }
    }
    if !found_start {
        start_byte = line.len();
        start_x = x;
        end_byte = line.len();
    }
    (start_byte, end_byte, start_x, x)
}

pub(crate) struct WrapIndex {
    style: TextStyle,
    wrap_width: Option<f32>,
    chunks: LineChunks,
    initialized: bool,
}

impl WrapIndex {
    pub(crate) fn new(style: TextStyle, wrap_width: Option<f32>) -> Self {
        Self {
            style,
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

    pub(crate) fn style(&self) -> &TextStyle {
        &self.style
    }

    pub(crate) fn wrap_width(&self) -> Option<f32> {
        self.wrap_width
    }

    pub(crate) fn set_wrap_width(&mut self, text: &Rope, wrap_width: Option<f32>, ts: &TextSystem) {
        if self.wrap_width == wrap_width {
            return;
        }
        self.wrap_width = wrap_width;
        if self.initialized {
            self.rebuild_all(text, ts);
        }
    }

    pub(crate) fn sync_layout(
        &mut self,
        text: &Rope,
        style: &TextStyle,
        wrap_width: Option<f32>,
        ts: &TextSystem,
    ) {
        let style_changed = self.style != *style;
        let width_changed = self.wrap_width != wrap_width;
        if style_changed {
            self.style = style.clone();
        }
        self.wrap_width = wrap_width;

        if !self.initialized {
            self.initialized = true;
            self.rebuild_all(text, ts);
            return;
        }
        if wrap_width.is_none() && !width_changed {
            return;
        }
        if style_changed || width_changed {
            self.rebuild_all(text, ts);
        }
    }

    pub(crate) fn apply_line_change(
        &mut self,
        updated_text: &Rope,
        change: &LineChange,
        ts: &TextSystem,
    ) {
        if !self.initialized {
            return;
        }
        match change {
            LineChange::Full => self.rebuild_all(updated_text, ts),
            LineChange::Splice {
                old_lines,
                new_lines,
            } => self.apply_line_splice(updated_text, old_lines.clone(), new_lines.clone(), ts),
        }
    }

    fn apply_line_splice(
        &mut self,
        updated_text: &Rope,
        old_lines: Range<usize>,
        new_lines: Range<usize>,
        ts: &TextSystem,
    ) {
        let Some(wrap_width) = self.wrap_width else {
            self.splice_line_range(
                updated_text,
                old_lines,
                new_lines,
                0,
                &mut |_, _, _| 0.0,
            );
            return;
        };

        let style = self.style.clone();
        ts.with_advances(&style, |advances| {
            let max_ascii = advances.max_printable_ascii();
            let ascii_fit_len = if max_ascii > 0.0 {
                (wrap_width / max_ascii).floor().max(0.0) as usize
            } else {
                0
            };
            self.splice_line_range(
                updated_text,
                old_lines,
                new_lines,
                ascii_fit_len,
                &mut |line_str, width, out| wrap_line(line_str, width, advances, ts, out),
            );
        });
    }

    fn splice_line_range<F>(
        &mut self,
        updated_text: &Rope,
        old_lines: Range<usize>,
        new_lines: Range<usize>,
        ascii_fit_len: usize,
        wrap_fn: &mut F,
    ) where
        F: FnMut(&str, f32, &mut Vec<usize>) -> f32,
    {
        let existing_count = self.line_count();
        let old_start = old_lines.start.min(existing_count);
        let old_end = old_lines.end.clamp(old_start, existing_count);

        let total_rows = updated_text.lines_len();
        let new_start = new_lines.start.min(total_rows);
        let new_end = new_lines.end.clamp(new_start, total_rows);

        let mut replacement = Vec::with_capacity(new_end - new_start);
        let mut breaks = Vec::new();
        let wrap_width = self.wrap_width;
        let mut iter = updated_text.lines_at(new_start, ropey::LineType::LF);

        for _ in new_start..new_end {
            let slice = match iter.next() {
                Some(raw) if raw.len() > 0 && raw.byte(raw.len() - 1) == b'\n' => {
                    raw.slice(..raw.len() - 1)
                }
                Some(raw) => raw,
                None => updated_text.slice(0..0),
            };
            let byte_len = slice.len();
            let mut indent_px = 0.0;
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
                    indent_px = wrap_fn(&line_str, width, &mut breaks);
                    if breaks.is_empty() {
                        LineRanges::Single(byte_len)
                    } else {
                        let mut v = Vec::with_capacity(breaks.len() + 1);
                        let mut cursor = 0;
                        for &ix in &breaks {
                            v.push(cursor..ix);
                            cursor = ix;
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
                hanging_indent: indent_px,
                ranges,
            });
        }

        self.chunks.splice(old_start..old_end, replacement);
    }

    pub(crate) fn rebuild_all(&mut self, text: &Rope, ts: &TextSystem) {
        let old_count = self.line_count();
        let new_count = text.lines_len();
        self.apply_line_splice(text, 0..old_count, 0..new_count, ts);
    }

    pub(crate) fn offset_to_display(
        &self,
        text: &Rope,
        offset: usize,
        end_affinity: bool,
    ) -> DisplayPos {
        let row = text.offset_to_point(offset).row;
        let start = text.line_start_offset(row);
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

    pub(crate) fn display_to_offset(&self, text: &Rope, pos: DisplayPos) -> usize {
        let Some((line_ix, first_row, line)) = self.chunks.get_by_visual_row(pos.row) else {
            return text.len();
        };
        let line_start = text.line_start_offset(line_ix);
        let sub_row = pos.row.saturating_sub(first_row);
        match line.ranges.get(sub_row) {
            Some(range) => line_start + (range.start + pos.col).min(range.end),
            None => line_start + line.byte_len,
        }
    }

    pub(crate) fn display_to_point(&self, text: &Rope, pos: DisplayPos) -> TextPoint {
        text.offset_to_point(self.display_to_offset(text, pos))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_style() -> TextStyle {
        TextStyle::regular("Arial", 14.)
    }

    #[test]
    fn incremental_splice_longest_row_and_ascii_fast_fit() {
        let mut index = WrapIndex::new(test_style(), Some(10.));
        let mut text = Rope::from("aa\nbbbb\nc");
        let mut fake = |line: &str, _: f32, out: &mut Vec<usize>| -> f32 {
            if line.len() > 2 {
                out.push(2);
            }
            0.0
        };
        index.splice_line_range(&text, 0..0, 0..text.lines_len(), 0, &mut fake);
        assert_eq!(index.line_count(), 3);
        assert_eq!(index.visual_row_count(), 4);
        assert_eq!(index.longest_line(), 1);
        assert_eq!(index.first_visual_row(2), 3);
        assert_eq!(index.visual_row_to_line(2), 1);

        let range = text.line_start_offset(1)..text.line_end_offset(1);
        text.replace(range, "dd\neeeeee");
        index.splice_line_range(&text, 1..2, 1..3, 0, &mut fake);
        assert_eq!(index.line_count(), 4);
        assert_eq!(index.visual_row_count(), 5);
        assert_eq!(index.longest_line(), 2);

        let all = 0..text.len();
        text.replace(all, "");
        index.splice_line_range(&text, 0..4, 0..1, 0, &mut fake);
        assert_eq!(index.line_count(), 1);
        assert_eq!(index.visual_row_count(), 1);
        assert_eq!(index.line(0).unwrap().ranges.get(0), Some(0..0));
        assert_eq!(index.line(0).unwrap().ranges.len(), 1);

        let mut fast = WrapIndex::new(test_style(), Some(100.));
        let sample = Rope::from("short ascii\nlonger ascii line\n日本語\n");
        let mut called = Vec::new();
        fast.splice_line_range(&sample, 0..0, 0..sample.lines_len(), 12, &mut |line, _, _| {
            called.push(line.to_string());
            0.0
        });
        assert_eq!(called, vec!["longer ascii line", "日本語"]);
    }

    #[test]
    fn multi_chunk_prefix_sums_match_across_splices() {
        let mut lines = Vec::new();
        for i in 0..700 {
            if i % 3 == 0 {
                lines.push("aaaa");
            } else {
                lines.push("bb");
            }
        }
        let mut text = Rope::from(lines.join("\n"));
        let mut index = WrapIndex::new(test_style(), Some(10.));
        let mut fake = |line: &str, _: f32, out: &mut Vec<usize>| -> f32 {
            if line.len() > 2 {
                out.push(2);
            }
            0.0
        };
        index.splice_line_range(&text, 0..0, 0..text.lines_len(), 0, &mut fake);
        assert!(index.chunks.chunks.len() >= 3);

        // Splice across a chunk boundary (lines 250..265 replaced by 3 wrapped lines).
        let start = text.line_start_offset(250);
        let end = text.line_end_offset(264);
        text.replace(start..end, "cccc\ndddd\neeee");
        index.splice_line_range(&text, 250..265, 250..253, 0, &mut fake);

        let mut expected_row = 0usize;
        for line_ix in 0..index.line_count() {
            assert_eq!(index.first_visual_row(line_ix), expected_row);
            let rows = index.line(line_ix).unwrap().row_count();
            for sub in 0..rows {
                assert_eq!(
                    index.visual_row_to_line_and_sub_row(expected_row + sub),
                    (line_ix, sub)
                );
            }
            expected_row += rows;
        }
        assert_eq!(index.visual_row_count(), expected_row);
    }

    #[test]
    fn wrap_boundary_affinity_and_hanging_indent_geometry() {
        let text = Rope::from("first line\nthis one wraps");
        let mut index = WrapIndex::new(test_style(), None);
        index.chunks = LineChunks::from_vec(vec![
            WrappedLine {
                byte_len: 10,
                hanging_indent: 0.0,
                ranges: LineRanges::Single(10),
            },
            WrappedLine {
                byte_len: 14,
                hanging_indent: 0.0,
                ranges: LineRanges::Wrapped(vec![0..9, 9..14].into_boxed_slice()),
            },
        ]);
        assert_eq!(index.offset_to_display(&text, 20, true), DisplayPos::new(1, 0, 9));
        assert_eq!(index.offset_to_display(&text, 20, false), DisplayPos::new(2, 1, 0));
        assert_eq!(index.display_to_offset(&text, DisplayPos::new(2, 1, 3)), 23);

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
