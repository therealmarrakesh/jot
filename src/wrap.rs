//! Document line layout: where each line soft-wraps, the index from lines to visual rows, shaped
//! rows for the visible lines, hit-testing and painting them.

use std::borrow::Cow;
use std::ops::Range;

use crate::geom::{Point, Rect, Rgba, Size, point, size};
use crate::gfx::{Canvas, FontFace, ShapedRow, TextStyle, TextSystem};
use crate::text::{LineChange, Rope, RopeExt as _, TextPoint};
use crate::types::SharedStr;

const CHUNK_SIZE: usize = 256;
const MAX_WRAP_INDENT: u32 = 32;

/// Where the document sits on screen: the viewport and how far its content is scrolled
/// (negative offsets move the content left and up).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Viewport {
    pub(crate) size: Size,
    pub(crate) scroll_x: f32,
    pub(crate) scroll_y: f64,
}

/// Unwrapped lines longer than this are shaped only around the viewport.
const HUGE_UNWRAPPED_LINE_BYTES: usize = 4096;
/// How far past the viewport such a line is shaped on each side.
const HORIZONTAL_WINDOW_PAD: f32 = 256.0;

/// The lines a frame shows (plus one past, for partly visible rows), the first one's top
/// relative to the viewport, and the visual rows worth shaping.
struct VisibleSlice {
    lines: Range<usize>,
    top_rel: f32,
    shape_rows: Range<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DisplayPos {
    pub(crate) row: usize,
    pub(crate) sub_row: usize,
    pub(crate) col: usize,
}

impl DisplayPos {
    pub(crate) fn new(row: usize, sub_row: usize, col: usize) -> Self {
        Self { row, sub_row, col }
    }
}

#[derive(Debug, Clone)]
enum LineRanges {
    Single(usize),
    Wrapped(Box<[Range<usize>]>),
}

impl LineRanges {
    fn len(&self) -> usize {
        match self {
            Self::Single(_) => 1,
            Self::Wrapped(s) => s.len(),
        }
    }

    #[inline]
    fn get(&self, index: usize) -> Option<Range<usize>> {
        match self {
            Self::Single(byte_len) => (index == 0).then_some(0..*byte_len),
            Self::Wrapped(s) => s.get(index).cloned(),
        }
    }

    fn last(&self) -> Range<usize> {
        match self {
            Self::Single(byte_len) => 0..*byte_len,
            Self::Wrapped(s) => s.last().cloned().unwrap_or(0..0),
        }
    }
}

#[derive(Debug, Clone)]
struct WrappedLine {
    byte_len: usize,
    hanging_indent: f32,
    ranges: LineRanges,
}

impl WrappedLine {
    fn row_count(&self) -> usize {
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
    fn single(line: &WrappedLine) -> Self {
        Self {
            buffer_lines: 1,
            visual_rows: line.row_count(),
            max_line_bytes: line.byte_len,
            longest_line: 0,
        }
    }

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
        let ix = self
            .row_starts
            .partition_point(|&start| start <= visual_row)
            - 1;
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
            merged.extend(
                self.chunks[start_chunk].items[..start_local]
                    .iter()
                    .cloned(),
            );
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

fn is_printable_ascii(bytes: &[u8]) -> bool {
    bytes.iter().all(|&b| (0x20..=0x7e).contains(&b))
}

fn wrap_line(line: &str, wrap_width: f32, face: &FontFace, out: &mut Vec<usize>) -> f32 {
    out.clear();
    if line.is_empty() || !wrap_width.is_finite() || wrap_width <= 0.0 {
        return 0.0;
    }
    let indent_px = face.hanging_indent(line, MAX_WRAP_INDENT, wrap_width * 0.5);
    face.line_breaks(line, wrap_width, indent_px, out);
    if out.is_empty() { 0.0 } else { indent_px }
}

pub(crate) struct WrapIndex {
    style: TextStyle,
    wrap_width: Option<f32>,
    chunks: LineChunks,
    initialized: bool,
    /// Unwrapped width of the longest line, until the text or the layout changes.
    longest_width: Option<f32>,
}

impl WrapIndex {
    /// Lays out the lines visible in `view`, shaping the rows on screen and the caret's row.
    /// Also returns whether a long unwrapped line was shaped only around the viewport, so a
    /// horizontal scroll needs a new layout.
    pub(crate) fn layout_visible(
        &self,
        text: &Rope,
        view: Viewport,
        caret: usize,
        end_affinity: bool,
        line_height: f32,
        ts: &TextSystem,
    ) -> (FrameLayout, bool) {
        let vis = self.visible_slice(view.scroll_y, line_height, view.size.height);
        let first_line = vis.lines.start;
        let start_off = text.line_start_offset(vis.lines.start);
        let end_off = text.line_end_offset(vis.lines.end.saturating_sub(1));
        let face = ts.face(&self.style);
        let mut lines = Vec::with_capacity(vis.lines.len());
        let mut windowed = false;
        let mut y = vis.top_rel;

        for buf_line in vis.lines.clone() {
            let line_slice = text.slice_line(buf_line);
            let line_len = line_slice.len();
            let line_byte_start = text.line_start_offset(buf_line);
            let wrapped = self.line(buf_line).expect("wrapped line exists");
            let total_sub_rows = wrapped.row_count();

            let layout = if self.wrap_width.is_none()
                && total_sub_rows == 1
                && line_len > HUGE_UNWRAPPED_LINE_BYTES
            {
                let line_str: Cow<str> = line_slice.into();
                let min_x = (-view.scroll_x - HORIZONTAL_WINDOW_PAD).max(0.0);
                let max_x = (-view.scroll_x + view.size.width + HORIZONTAL_WINDOW_PAD).max(min_x);
                if let Some((mut start_b, mut end_b, start_x, total_w)) =
                    face.horizontal_window(&line_str, min_x, max_x)
                {
                    if caret >= line_byte_start && caret <= line_byte_start + line_len {
                        let local_caret = caret - line_byte_start;
                        if local_caret < start_b {
                            start_b = local_caret;
                        } else if local_caret > end_b {
                            end_b = local_caret;
                        }
                    }
                    let slice_str: SharedStr = line_str[start_b..end_b].to_string().into();
                    let shaped = ts.shape_row(slice_str, &self.style);
                    let vrow = VisualRow::Windowed {
                        start_byte: start_b,
                        end_byte: end_b,
                        total_bytes: line_len,
                        start_x,
                        total_width: total_w,
                        line: shaped,
                    };
                    windowed = true;
                    RowLayout::from_slice(vec![vrow], 0, 0, 1, 0.0)
                } else {
                    self.shape_line(text, buf_line, &vis.shape_rows, caret, end_affinity, ts)
                }
            } else {
                self.shape_line(text, buf_line, &vis.shape_rows, caret, end_affinity, ts)
            };

            let h = layout.size(line_height).height;
            lines.push(VisibleLine {
                byte_offset: line_byte_start,
                top: y,
                layout,
            });
            y += h;
        }
        let layout = FrameLayout {
            first_line,
            lines,
            visible_byte_range: start_off..end_off,
            line_height,
            space_width: face.space_width(),
        };
        (layout, windowed)
    }

    /// Shapes line `buf_line`'s rows in `shape_rows` and the caret's row; the rest stay
    /// placeholders. Lines of many rows keep only the shaped stretch.
    fn shape_line(
        &self,
        text: &Rope,
        buf_line: usize,
        shape_rows: &Range<usize>,
        caret: usize,
        end_affinity: bool,
        ts: &TextSystem,
    ) -> RowLayout {
        let line_slice = text.slice_line(buf_line);
        let line_len = line_slice.len();
        let line_byte_start = text.line_start_offset(buf_line);
        let wrapped = self.line(buf_line).expect("wrapped line exists");
        let first_row_of_line = self.first_visual_row(buf_line);
        let total_sub_rows = wrapped.row_count();

        let (slice_start, slice_end) = if total_sub_rows > 64 {
            let vis_lo = shape_rows
                .start
                .saturating_sub(first_row_of_line)
                .min(total_sub_rows);
            let vis_hi = shape_rows
                .end
                .saturating_sub(first_row_of_line)
                .min(total_sub_rows);
            let mut lo = vis_lo;
            let mut hi = vis_hi.max(lo.saturating_add(1)).min(total_sub_rows);
            if caret >= line_byte_start && caret <= line_byte_start + line_len {
                let disp = self.offset_to_display(text, caret, end_affinity);
                if disp.sub_row < lo {
                    lo = disp.sub_row;
                }
                if disp.sub_row >= hi {
                    hi = (disp.sub_row + 1).min(total_sub_rows);
                }
            }
            (lo, hi)
        } else {
            (0, total_sub_rows)
        };

        let leading_bytes = wrapped.ranges.get(slice_start).map_or(0, |r| r.start);
        let mut rows = Vec::with_capacity(slice_end.saturating_sub(slice_start));
        for sub_ix in slice_start..slice_end {
            let range = match &wrapped.ranges {
                LineRanges::Single(len) => 0..*len,
                LineRanges::Wrapped(slice) => slice[sub_ix].clone(),
            };
            let visual_row = first_row_of_line + sub_ix;
            let sub_start = line_byte_start + range.start;
            let sub_end = line_byte_start + range.end;
            let has_caret = caret >= sub_start && caret <= sub_end;
            let must_shape = shape_rows.contains(&visual_row) || has_caret;
            if must_shape {
                let sub_text: SharedStr = line_slice.slice(range).to_string().into();
                let shaped = ts.shape_row(sub_text, &self.style);
                rows.push(VisualRow::from(shaped));
            } else {
                rows.push(VisualRow::Placeholder(range.len()));
            }
        }

        RowLayout::from_slice(
            rows,
            slice_start,
            leading_bytes,
            total_sub_rows,
            wrapped.hanging_indent,
        )
    }

    fn visible_slice(&self, scroll_y: f64, line_height: f32, viewport_height: f32) -> VisibleSlice {
        let total_rows = self.visual_row_count();
        let total_lines = self.line_count();
        if total_rows == 0 || total_lines == 0 {
            return VisibleSlice {
                lines: 0..0,
                top_rel: 0.0,
                shape_rows: 0..0,
            };
        }
        let lh = f64::from(line_height);
        let top = (-scroll_y).max(0.0);
        let bottom = top + f64::from(viewport_height);
        let first_row = ((top / lh).floor() as usize).min(total_rows - 1);
        let last_row = ((bottom / lh).ceil() as usize).min(total_rows - 1);
        let shape_rows = first_row.saturating_sub(2)..(last_row + 3).min(total_rows);

        let start_line = self.visual_row_to_line(first_row);
        let end_line = self.visual_row_to_line(last_row);
        let first_vis_row = self.first_visual_row(start_line);
        let top_rel = (first_vis_row as f64 * lh + scroll_y) as f32;
        VisibleSlice {
            lines: start_line..(end_line + 2).min(total_lines),
            top_rel,
            shape_rows,
        }
    }

    /// The unwrapped width of the longest line, measured once per change to the text or layout.
    pub(crate) fn longest_line_width(&mut self, text: &Rope, ts: &TextSystem) -> f32 {
        if let Some(width) = self.longest_width {
            return width;
        }
        let line: Cow<str> = text.slice_line(self.longest_line()).into();
        let width = ts.face(&self.style).unwrapped_width(&line);
        self.longest_width = Some(width);
        width
    }

    pub(crate) fn new(style: TextStyle, wrap_width: Option<f32>) -> Self {
        Self {
            style,
            wrap_width,
            chunks: LineChunks::default(),
            initialized: false,
            longest_width: None,
        }
    }

    pub(crate) fn visual_row_count(&self) -> usize {
        self.chunks.total.visual_rows
    }

    pub(crate) fn line_count(&self) -> usize {
        self.chunks.total.buffer_lines
    }

    fn longest_line(&self) -> usize {
        self.chunks.total.longest_line
    }

    fn line(&self, row: usize) -> Option<&WrappedLine> {
        self.chunks.get_by_line(row).1
    }

    fn first_visual_row(&self, line: usize) -> usize {
        self.chunks.get_by_line(line).0
    }

    fn visual_row_to_line(&self, visual_row: usize) -> usize {
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

    pub(crate) fn visual_row_range(
        &self,
        line_ix: usize,
        sub_row: usize,
    ) -> Option<(Range<usize>, bool)> {
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
        self.longest_width = None;
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
        if style_changed || width_changed || !self.initialized {
            self.longest_width = None;
        }
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
        self.longest_width = None;
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
            self.splice_line_range(updated_text, old_lines, new_lines, 0, &mut |_, _, _| 0.0);
            return;
        };

        let face = ts.face(&self.style);
        let max_ascii = face.max_printable_ascii();
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
            &mut |line_str, width, out| wrap_line(line_str, width, &face, out),
        );
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

    fn rebuild_all(&mut self, text: &Rope, ts: &TextSystem) {
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
            LineRanges::Single(byte_len) => DisplayPos::new(first_row, 0, local.min(*byte_len)),
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

enum VisualRow {
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
    fn byte_len(&self) -> usize {
        match self {
            Self::Shaped(line) => line.len,
            Self::Windowed { total_bytes, .. } => *total_bytes,
            Self::Placeholder(len) => *len,
        }
    }

    fn width(&self) -> f32 {
        match self {
            Self::Shaped(line) => line.width,
            Self::Windowed { total_width, .. } => *total_width,
            Self::Placeholder(_) => 0.0,
        }
    }

    fn is_shaped(&self) -> bool {
        !matches!(self, Self::Placeholder(_))
    }

    fn x_for_index(&self, ix: usize) -> f32 {
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

    fn selection_spans(&self, start: usize, end: usize) -> Vec<(f32, f32)> {
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

    fn closest_index_for_x(&self, x: f32) -> usize {
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

    fn paint(
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
    first_sub_row: usize,
    leading_bytes: usize,
    total_sub_rows: usize,
    rows: Vec<VisualRow>,
    hanging_indent: f32,
    max_width: f32,
}

impl RowLayout {
    fn from_slice(
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

    fn from_shaped(lines: Vec<ShapedRow>) -> Self {
        let total = lines.len();
        let rows: Vec<VisualRow> = lines.into_iter().map(VisualRow::from).collect();
        Self::from_slice(rows, 0, 0, total, 0.0)
    }

    fn indent_for(&self, abs_sub_row: usize) -> f32 {
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

    fn closest_index_for_x(&self, x: f32) -> usize {
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

    fn closest_index_for_x_in_last_row(&self, x: f32) -> usize {
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

    fn size(&self, line_height: f32) -> Size {
        let width = self
            .rows
            .iter()
            .enumerate()
            .map(|(ix, row)| row.width() + self.indent_for(self.first_sub_row + ix))
            .fold(self.max_width, f32::max);
        size(width, self.total_sub_rows as f32 * line_height)
    }

    fn paint(
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

/// One visible line: where it starts in the text, its top in content coordinates, its rows.
struct VisibleLine {
    byte_offset: usize,
    top: f32,
    layout: RowLayout,
}

/// The lines a frame shows, shaped and stacked in content coordinates.
pub(crate) struct FrameLayout {
    first_line: usize,
    lines: Vec<VisibleLine>,
    visible_byte_range: Range<usize>,
    line_height: f32,
    space_width: f32,
}

impl FrameLayout {
    /// One row of shaped text, as a single-line field shows it: `text_len` bytes of the field's
    /// text, or none while the row shows a placeholder.
    pub(crate) fn single_row(
        row: ShapedRow,
        text_len: usize,
        line_height: f32,
        space_width: f32,
    ) -> Self {
        let line = VisibleLine {
            byte_offset: 0,
            top: 0.0,
            layout: RowLayout::from_shaped(vec![row]),
        };
        Self {
            first_line: 0,
            lines: vec![line],
            visible_byte_range: 0..text_len,
            line_height,
            space_width,
        }
    }

    /// The widest laid-out line.
    pub(crate) fn width(&self) -> f32 {
        let widths = self
            .lines
            .iter()
            .map(|l| l.layout.size(self.line_height).width);
        widths.fold(0.0, f32::max)
    }

    pub(crate) fn line_height(&self) -> f32 {
        self.line_height
    }

    pub(crate) fn space_width(&self) -> f32 {
        self.space_width
    }

    pub(crate) fn row_layout(&self, buffer_row: usize) -> Option<&RowLayout> {
        let ix = buffer_row.checked_sub(self.first_line)?;
        self.lines.get(ix).map(|l| &l.layout)
    }

    pub(crate) fn position_for_offset(
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

    /// The box from `range`'s start to its end (or around `fallback` if the start isn't laid
    /// out), with the content placed at `origin`.
    pub(crate) fn bounds_for_range(
        &self,
        text: &Rope,
        range: Range<usize>,
        fallback: usize,
        origin: Point,
    ) -> Option<Rect> {
        let start_pos = self
            .position_for_offset(text, range.start, false)
            .or_else(|| self.position_for_offset(text, fallback, false))?;
        let end_pos = self
            .position_for_offset(text, range.end, false)
            .unwrap_or(start_pos);
        Some(Rect::from_corners(
            origin + start_pos,
            origin + point(end_pos.x, start_pos.y + self.line_height),
        ))
    }

    /// The text offset (and end affinity) nearest `rel`, a point in content coordinates.
    pub(crate) fn hit_test(&self, rel: Point, text_len: usize, multi_line: bool) -> (usize, bool) {
        if self.lines.is_empty() {
            return (0, false);
        }
        let lh = self.line_height;

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

    /// Paints the text with the content placed at `origin`, underlining the `marked` (IME) range.
    pub(crate) fn paint(
        &self,
        origin: Point,
        color: Rgba,
        marked: Option<&Range<usize>>,
        canvas: &mut Canvas,
    ) {
        for line in &self.lines {
            let line_base = line.byte_offset;
            let line_marked = marked.and_then(|m| {
                (m.end > line_base)
                    .then(|| m.start.saturating_sub(line_base)..m.end.saturating_sub(line_base))
            });
            line.layout.paint(
                point(origin.x, origin.y + line.top),
                self.line_height,
                color,
                line_marked,
                canvas,
            );
        }
    }

    /// Selection boxes for `range` over the laid-out rows, with the content placed at `origin`.
    pub(crate) fn range_rects(&self, range: &Range<usize>, origin: Point) -> Vec<Rect> {
        let range = range.start.max(self.visible_byte_range.start)
            ..range.end.min(self.visible_byte_range.end);
        if range.is_empty() {
            return Vec::new();
        }
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

    fn test_style() -> TextStyle {
        TextStyle::regular("Arial", 14.)
    }

    /// A long unwrapped line is shaped only around the viewport, but always across all of it.
    #[test]
    fn huge_unwrapped_lines_are_shaped_across_the_viewport() {
        let ts = TextSystem::new().unwrap();
        let text = Rope::from(format!("short\n{}\nend", "x".repeat(20_000)).as_str());
        let mut wrap = WrapIndex::new(test_style(), None);
        wrap.sync_layout(&text, &test_style(), None, &ts);
        for scroll_x in [0.0, -5_000.0, -40_000.0] {
            let size = size(300., 100.);
            let view = Viewport {
                size,
                scroll_x,
                scroll_y: 0.0,
            };
            let (layout, windowed) = wrap.layout_visible(&text, view, 0, false, 20., &ts);
            assert!(windowed);
            let VisualRow::Windowed { start_x, line, .. } = &layout.lines[1].layout.rows[0] else {
                panic!("the long line isn't windowed");
            };
            let (left, right) = (*start_x, start_x + line.width);
            assert!(
                left <= -scroll_x && right >= -scroll_x + size.width,
                "{left}..{right} at {scroll_x}"
            );
        }
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
        fast.splice_line_range(
            &sample,
            0..0,
            0..sample.lines_len(),
            12,
            &mut |line, _, _| {
                called.push(line.to_string());
                0.0
            },
        );
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
        assert_eq!(
            index.offset_to_display(&text, 20, true),
            DisplayPos::new(1, 0, 9)
        );
        assert_eq!(
            index.offset_to_display(&text, 20, false),
            DisplayPos::new(2, 1, 0)
        );
        assert_eq!(index.display_to_offset(&text, DisplayPos::new(2, 1, 3)), 23);
    }

    #[test]
    fn row_layout_hanging_indent_and_end_affinity_geometry() {
        let mut s1 = ShapedRow::default();
        s1.len = 10;
        let mut s2 = ShapedRow::default();
        s2.len = 5;
        let layout =
            RowLayout::from_slice(vec![VisualRow::from(s1), VisualRow::from(s2)], 0, 0, 2, 20.);
        let (ix, affinity) = layout
            .closest_index_for_position(point(999., 5.), 20.)
            .unwrap();
        assert_eq!(ix, 10);
        assert!(affinity);
        assert_eq!(
            layout.position_for_index(10, 20., true).map(|p| p.y),
            Some(0.)
        );
        assert_eq!(
            layout.position_for_index(10, 20., false),
            Some(point(20., 20.))
        );
    }
}
