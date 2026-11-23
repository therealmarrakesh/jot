use std::borrow::Cow;
use std::ops::{Range, RangeInclusive};

use crate::ui::{
    App, Boundary, Font, FontRun, MAX_WRAP_INDENT, Pixels, Point, ShapedLine, Size, TextAlign,
    Window, point, px, size,
};
use ropey::Rope;
use unicode_segmentation::UnicodeSegmentation as _;

use super::{Point as TreeSitterPoint, RopeExt, layout::LastLayout};

/// A single visual sub-row within a wrapped buffer line.
pub(crate) enum InputLine {
    Shaped(ShapedLine),
    Unshaped(usize),
}

impl From<ShapedLine> for InputLine {
    fn from(line: ShapedLine) -> Self {
        Self::Shaped(line)
    }
}

impl InputLine {
    pub(crate) fn unshaped(len: usize) -> Self {
        Self::Unshaped(len)
    }

    #[inline]
    pub(crate) fn len(&self) -> usize {
        match self {
            Self::Shaped(line) => line.len,
            Self::Unshaped(len) => *len,
        }
    }

    #[inline]
    pub(crate) fn width(&self) -> Pixels {
        match self {
            Self::Shaped(line) => line.width,
            Self::Unshaped(_) => px(0.),
        }
    }

    #[inline]
    pub(crate) fn is_shaped(&self) -> bool {
        matches!(self, Self::Shaped(_))
    }

    pub(crate) fn x_for_index(&self, ix: usize) -> Pixels {
        match self {
            Self::Shaped(line) => line.x_for_index(ix),
            Self::Unshaped(_) => px(0.),
        }
    }

    pub(crate) fn closest_index_for_x(&self, x: Pixels) -> usize {
        match self {
            Self::Shaped(line) => line.closest_index_for_x(x),
            Self::Unshaped(_) => 0,
        }
    }

    pub(crate) fn paint(
        &self,
        pos: Point<Pixels>,
        height: Pixels,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Self::Shaped(line) = self {
            let _ = line.paint(pos, height, TextAlign::Left, None, window, cx);
        }
    }
}

const PRINTABLE_ASCII: RangeInclusive<u32> = 0x20..=0x7e;
const LATIN_EXTENDED_AND_SPACING_MODIFIERS: RangeInclusive<u32> = 0xa0..=0x2ff;
const GREEK_AND_CYRILLIC_LETTERS: RangeInclusive<u32> = 0x370..=0x482;
const CYRILLIC_AFTER_COMBINING_MARKS: RangeInclusive<u32> = 0x48a..=0x52f;
const LATIN_AND_GREEK_EXTENDED_ADDITIONAL: RangeInclusive<u32> = 0x1e00..=0x1fff;
const DASHES_QUOTES_AND_BULLETS: RangeInclusive<u32> = 0x2010..=0x2027;
const VISIBLE_GENERAL_PUNCTUATION: RangeInclusive<u32> = 0x2030..=0x205e;
const SCRIPTS_AND_CURRENCY: RangeInclusive<u32> = 0x2070..=0x20cf;
const LETTERLIKE_ARROWS_MATH_AND_SYMBOLS: RangeInclusive<u32> = 0x2100..=0x2bff;
const CJK_PUNCTUATION_BEFORE_TONE_MARKS: RangeInclusive<u32> = 0x3000..=0x3029;
const CJK_PUNCTUATION_AND_HIRAGANA: RangeInclusive<u32> = 0x3030..=0x3098;
const KANA_AFTER_VOICING_MARKS: RangeInclusive<u32> = 0x309b..=0x30ff;
const CJK_IDEOGRAPHS_EXTENSION_A: RangeInclusive<u32> = 0x3400..=0x4dbf;
const CJK_UNIFIED_IDEOGRAPHS: RangeInclusive<u32> = 0x4e00..=0x9fff;
const HANGUL_SYLLABLES: RangeInclusive<u32> = 0xac00..=0xd7a3;
const CJK_COMPATIBILITY_IDEOGRAPHS: RangeInclusive<u32> = 0xf900..=0xfaff;
const HALFWIDTH_AND_FULLWIDTH_FORMS: RangeInclusive<u32> = 0xff01..=0xffee;

const UNSHAPED_RANGES: [RangeInclusive<u32>; 17] = [
    PRINTABLE_ASCII,
    LATIN_EXTENDED_AND_SPACING_MODIFIERS,
    GREEK_AND_CYRILLIC_LETTERS,
    CYRILLIC_AFTER_COMBINING_MARKS,
    LATIN_AND_GREEK_EXTENDED_ADDITIONAL,
    DASHES_QUOTES_AND_BULLETS,
    VISIBLE_GENERAL_PUNCTUATION,
    SCRIPTS_AND_CURRENCY,
    LETTERLIKE_ARROWS_MATH_AND_SYMBOLS,
    CJK_PUNCTUATION_BEFORE_TONE_MARKS,
    CJK_PUNCTUATION_AND_HIRAGANA,
    KANA_AFTER_VOICING_MARKS,
    CJK_IDEOGRAPHS_EXTENSION_A,
    CJK_UNIFIED_IDEOGRAPHS,
    HANGUL_SYLLABLES,
    CJK_COMPATIBILITY_IDEOGRAPHS,
    HALFWIDTH_AND_FULLWIDTH_FORMS,
];

fn is_printable_ascii(bytes: &[u8]) -> bool {
    bytes.iter().all(|&b| (0x20..=0x7e).contains(&b))
}

fn wraps_by_char_widths(text: &str) -> bool {
    if is_printable_ascii(text.as_bytes()) {
        return true;
    }
    text.chars()
        .all(|c| UNSHAPED_RANGES.iter().any(|range| range.contains(&(c as u32))))
}

/// Choose Unicode line-break opportunities using the same shaped widths as
/// painting. Oversized words fall back to complete graphemes, never UTF-8 bytes.
fn measured_wrap_boundaries(
    text: &str,
    width: Pixels,
    preserve_indent: bool,
    mut measure: impl FnMut(&str) -> Pixels,
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
        px(0.)
    };
    let ends: Vec<usize> = text
        .grapheme_indices(true)
        .map(|(ix, grapheme)| ix + grapheme.len())
        .collect();
    let opportunities: Vec<usize> = crate::win32::line_break_opportunities(text)
        .into_iter()
        .filter(|ix| ends.binary_search(ix).is_ok())
        .collect();
    let mut first = 0;
    let mut start = 0;
    while first < ends.len() {
        let available = if start == 0 {
            width
        } else {
            width - indent_width
        };
        // Find the fitting prefix locally. Exponential probing avoids shaping
        // the entire remaining logical line for every visual row of a long paste.
        let remaining = ends.len() - first;
        let mut low = 0;
        let mut high = 1;
        while measure(&text[start..ends[first + high - 1]]) <= available {
            low = high;
            if high == remaining {
                break;
            }
            high = (high * 2).min(remaining);
        }
        while low + 1 < high {
            let mid = (low + high) / 2;
            if measure(&text[start..ends[first + mid - 1]]) <= available {
                low = mid;
            } else {
                high = mid;
            }
        }
        // An indivisible grapheme wider than the viewport still consumes a row.
        let fitting_end = ends[first + low.max(1) - 1];
        if fitting_end == text.len() {
            break;
        }
        let candidate = opportunities.partition_point(|&ix| ix <= fitting_end);
        let end = candidate
            .checked_sub(1)
            .map(|ix| opportunities[ix])
            .filter(|&ix| ix > start && (start != 0 || ix > indent))
            .unwrap_or(fitting_end);
        out.push(Boundary {
            ix: end,
            next_indent: indent as u32,
        });
        first = ends.partition_point(|&ix| ix <= end);
        start = end;
    }
}

/// A buffer line with its soft-wrapped sub-line byte ranges.
#[derive(Debug, Clone)]
pub(crate) struct LineItem {
    /// Byte length of the line, excluding trailing `\n`.
    len: usize,
    /// Number of leading spaces reserved as continuation indent.
    pub(crate) indent: u32,
    /// Relative byte ranges (`0..len`) of each visual row (including the first).
    pub(crate) wrapped_lines: Vec<Range<usize>>,
}

impl LineItem {
    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    #[inline]
    pub(crate) fn lines_len(&self) -> usize {
        self.wrapped_lines.len()
    }
}

/// Summary of a chunk or full [`LineTree`].
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct LineSummary {
    buffer_rows: usize,
    wrap_rows: usize,
    max_line_len: usize,
    longest_row: usize,
}

impl LineSummary {
    #[inline]
    fn from_item(item: &LineItem) -> Self {
        Self {
            buffer_rows: 1,
            wrap_rows: item.lines_len(),
            max_line_len: item.len(),
            longest_row: 0,
        }
    }

    #[inline]
    fn add(&mut self, other: &Self) {
        if other.max_line_len > self.max_line_len {
            self.longest_row = self.buffer_rows + other.longest_row;
            self.max_line_len = other.max_line_len;
        }
        self.buffer_rows += other.buffer_rows;
        self.wrap_rows += other.wrap_rows;
    }

    fn of_slice(items: &[LineItem]) -> Self {
        let mut summary = Self::default();
        for item in items {
            summary.add(&Self::from_item(item));
        }
        summary
    }
}

const LINE_CHUNK_TARGET: usize = 256;

#[derive(Debug, Clone)]
struct LineChunk {
    summary: LineSummary,
    items: Vec<LineItem>,
}

impl LineChunk {
    fn new(items: Vec<LineItem>) -> Self {
        let summary = LineSummary::of_slice(&items);
        Self { summary, items }
    }
}

/// Chunked line table maintaining incremental buffer-row, wrap-row, and longest-line summaries.
#[derive(Debug, Clone, Default)]
pub(crate) struct LineTree {
    chunks: Vec<LineChunk>,
    total: LineSummary,
}

impl LineTree {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn from_vec(items: Vec<LineItem>) -> Self {
        let mut chunks = Vec::with_capacity(items.len().div_ceil(LINE_CHUNK_TARGET));
        let mut iter = items.into_iter();
        while iter.len() > 0 {
            let chunk: Vec<LineItem> = iter.by_ref().take(LINE_CHUNK_TARGET).collect();
            chunks.push(LineChunk::new(chunk));
        }
        let mut tree = Self {
            chunks,
            total: LineSummary::default(),
        };
        tree.recompute_total();
        tree
    }

    #[inline]
    pub(crate) fn is_empty(&self) -> bool {
        self.total.buffer_rows == 0
    }

    #[inline]
    pub(crate) fn summary(&self) -> LineSummary {
        self.total
    }

    #[cfg(test)]
    pub(crate) fn iter(&self) -> impl Iterator<Item = &LineItem> {
        self.chunks.iter().flat_map(|chunk| chunk.items.iter())
    }

    fn locate_buffer_row(&self, row: usize) -> (usize, usize) {
        let mut offset = 0;
        for (chunk_ix, chunk) in self.chunks.iter().enumerate() {
            if row < offset + chunk.summary.buffer_rows {
                return (chunk_ix, row - offset);
            }
            offset += chunk.summary.buffer_rows;
        }
        (self.chunks.len(), 0)
    }

    /// Returns `(first_wrap_row, Some(item))` if `row` is in bounds, or `(total_wrap_rows, None)`.
    pub(crate) fn get_by_buffer_row(&self, row: usize) -> (usize, Option<&LineItem>) {
        let mut buffer_offset = 0;
        let mut wrap_offset = 0;
        for chunk in &self.chunks {
            if row < buffer_offset + chunk.summary.buffer_rows {
                let local = row - buffer_offset;
                for item in &chunk.items[..local] {
                    wrap_offset += item.lines_len();
                }
                return (wrap_offset, Some(&chunk.items[local]));
            }
            buffer_offset += chunk.summary.buffer_rows;
            wrap_offset += chunk.summary.wrap_rows;
        }
        (self.total.wrap_rows, None)
    }

    /// Returns `Some((buffer_row, first_wrap_row, item))` for `wrap_row`, or `None` if out of range.
    pub(crate) fn get_by_wrap_row(&self, wrap_row: usize) -> Option<(usize, usize, &LineItem)> {
        let mut buffer_offset = 0;
        let mut wrap_offset = 0;
        for chunk in &self.chunks {
            if wrap_row < wrap_offset + chunk.summary.wrap_rows {
                for (local, item) in chunk.items.iter().enumerate() {
                    let next_wrap = wrap_offset + item.lines_len();
                    if wrap_row < next_wrap {
                        return Some((buffer_offset + local, wrap_offset, item));
                    }
                    wrap_offset = next_wrap;
                }
            }
            buffer_offset += chunk.summary.buffer_rows;
            wrap_offset += chunk.summary.wrap_rows;
        }
        None
    }

    pub(crate) fn splice(&mut self, range: Range<usize>, replacement: Vec<LineItem>) {
        if self.is_empty() {
            *self = Self::from_vec(replacement);
            return;
        }
        let start_row = range.start.min(self.total.buffer_rows);
        let end_row = range.end.clamp(start_row, self.total.buffer_rows);
        let (start_chunk, start_local) = self.locate_buffer_row(start_row);
        let (end_chunk, end_local) = self.locate_buffer_row(end_row);

        let replace_end = if end_chunk < self.chunks.len() {
            end_chunk + 1
        } else {
            self.chunks.len()
        };

        let prefix_len = if start_chunk < self.chunks.len() {
            start_local
        } else {
            0
        };
        let suffix_len = if end_chunk < self.chunks.len() {
            self.chunks[end_chunk].items.len() - end_local
        } else {
            0
        };

        let mut merged = Vec::with_capacity(prefix_len + replacement.len() + suffix_len);
        if start_chunk < self.chunks.len() {
            merged.extend(self.chunks[start_chunk].items[..start_local].iter().cloned());
        }
        merged.extend(replacement);
        if end_chunk < self.chunks.len() {
            merged.extend(self.chunks[end_chunk].items[end_local..].iter().cloned());
        }

        let mut new_chunks = Vec::with_capacity(merged.len().div_ceil(LINE_CHUNK_TARGET));
        let mut iter = merged.into_iter();
        while iter.len() > 0 {
            let chunk: Vec<LineItem> = iter.by_ref().take(LINE_CHUNK_TARGET).collect();
            new_chunks.push(LineChunk::new(chunk));
        }

        self.chunks.splice(start_chunk..replace_end, new_chunks);
        self.recompute_total();
    }

    fn recompute_total(&mut self) {
        let mut total = LineSummary::default();
        for chunk in &self.chunks {
            total.add(&chunk.summary);
        }
        self.total = total;
    }
}

/// Incremental soft-wrap index mapping buffer lines to visual rows.
pub(crate) struct TextWrapper {
    text: Rope,
    font: Font,
    font_size: Pixels,
    /// `None` disables soft-wrapping.
    wrap_width: Option<Pixels>,
    pub(crate) lines: LineTree,
    initialized: bool,
}

impl TextWrapper {
    pub(crate) fn new(font: Font, font_size: Pixels, wrap_width: Option<Pixels>) -> Self {
        Self {
            text: Rope::new(),
            font,
            font_size,
            wrap_width,
            lines: LineTree::new(),
            initialized: false,
        }
    }

    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.lines.summary().wrap_rows
    }

    #[inline]
    pub(crate) fn lines_count(&self) -> usize {
        self.lines.summary().buffer_rows
    }

    #[inline]
    pub(crate) fn longest_row(&self) -> usize {
        self.lines.summary().longest_row
    }

    #[inline]
    pub(crate) fn line(&self, row: usize) -> Option<&LineItem> {
        self.lines.get_by_buffer_row(row).1
    }

    #[cfg(test)]
    #[inline]
    pub(crate) fn iter_lines(&self) -> impl Iterator<Item = &LineItem> {
        self.lines.iter()
    }

    /// First wrap row of buffer line `row`, or total wrap rows if out of range.
    pub(crate) fn buffer_line_to_first_wrap_row(&self, row: usize) -> usize {
        self.lines.get_by_buffer_row(row).0
    }

    #[cfg(test)]
    pub(crate) fn buffer_line_to_wrap_row_range(&self, row: usize) -> Range<usize> {
        let (start, item) = self.lines.get_by_buffer_row(row);
        let len = item.map(|l| l.lines_len()).unwrap_or(0);
        start..start + len
    }

    /// Buffer line containing `wrap_row`, clamped to the last line.
    pub(crate) fn wrap_row_to_buffer_line(&self, wrap_row: usize) -> usize {
        match self.lines.get_by_wrap_row(wrap_row) {
            Some((buffer_row, _, _)) => buffer_row,
            None => self.lines_count().saturating_sub(1),
        }
    }

    pub(crate) fn set_wrap_width(&mut self, wrap_width: Option<Pixels>, cx: &mut App) {
        if wrap_width == self.wrap_width {
            return;
        }
        self.wrap_width = wrap_width;
        self.update_all(&self.text.clone(), cx);
    }

    pub(crate) fn prepare_layout(
        &mut self,
        text: &Rope,
        font: Font,
        font_size: Pixels,
        wrap_width: Option<Pixels>,
        cx: &mut App,
    ) -> bool {
        let font_changed = self.font != font || self.font_size != font_size;
        let wrap_changed = self.wrap_width != wrap_width;
        self.font = font;
        self.font_size = font_size;
        self.wrap_width = wrap_width;

        if !self.initialized {
            self.initialized = true;
            self.update_all(text, cx);
            return true;
        }
        if wrap_width.is_none() && !wrap_changed {
            return false;
        }
        if font_changed || wrap_changed {
            self.update_all(text, cx);
            return true;
        }
        false
    }

    /// Incrementally re-wrap only the buffer lines touched by replacing `range` with `new_text`.
    pub(crate) fn update(
        &mut self,
        changed_text: &Rope,
        range: &Range<usize>,
        new_text: &Rope,
        cx: &mut App,
    ) {
        let Some(wrap_width) = self.wrap_width else {
            self.update_inner(changed_text, range, new_text, 0, &mut |_, _, _| {});
            return;
        };

        let text_system = cx.text_system().clone();
        let font = self.font.clone();
        let font_size = self.font_size;
        text_system.with_char_widths(&font, font_size, |font_id, char_widths, dw| {
            let max_ascii_width = char_widths.max_printable_ascii();
            let ascii_fit_len = if max_ascii_width > px(0.) {
                (wrap_width / max_ascii_width).floor().max(0.) as usize
            } else {
                0
            };
            self.update_inner(
                changed_text,
                range,
                new_text,
                ascii_fit_len,
                &mut |line_str, wrap_width, out| {
                    if wraps_by_char_widths(line_str) {
                        char_widths.wrap_line_into(line_str, wrap_width, dw, out);
                    } else {
                        measured_wrap_boundaries(
                            line_str,
                            wrap_width,
                            true,
                            |sub| {
                                if sub.is_empty() {
                                    return px(0.);
                                }
                                dw.layout_line(
                                    sub,
                                    font_size,
                                    &[FontRun {
                                        len: sub.len(),
                                        font_id,
                                    }],
                                )
                                .width
                            },
                            out,
                        );
                    }
                },
            );
        });
    }

    #[cfg(test)]
    pub(crate) fn update_for_bench<F>(
        &mut self,
        changed_text: &Rope,
        range: &Range<usize>,
        new_text: &Rope,
        ascii_fit_len: usize,
        wrap_line: &mut F,
    ) where
        F: FnMut(&str, Pixels, &mut Vec<Boundary>),
    {
        self.update_inner(changed_text, range, new_text, ascii_fit_len, wrap_line);
    }

    fn update_inner<F>(
        &mut self,
        changed_text: &Rope,
        range: &Range<usize>,
        new_text: &Rope,
        ascii_fit_len: usize,
        wrap_line: &mut F,
    ) where
        F: FnMut(&str, Pixels, &mut Vec<Boundary>),
    {
        let buffer_line_count = self.lines_count();
        let start_row = self
            .text
            .offset_to_point(range.start)
            .row
            .min(buffer_line_count.saturating_sub(1));
        let end_row = self
            .text
            .offset_to_point(range.end)
            .row
            .min(buffer_line_count.saturating_sub(1));

        let new_start_row = changed_text.offset_to_point(range.start).row;
        let new_end_row = changed_text
            .offset_to_point(range.start + new_text.len())
            .row;

        let line_count = new_end_row.saturating_sub(new_start_row) + 1;
        let mut new_lines = Vec::with_capacity(line_count);
        let mut boundaries = Vec::new();
        let wrap_width = self.wrap_width;
        let total_rows = changed_text.lines_len();
        let mut lines_iter =
            changed_text.lines_at(new_start_row.min(total_rows), ropey::LineType::LF);

        for _row in new_start_row..=new_end_row {
            let line = if let Some(raw) = lines_iter.next() {
                let raw_len = raw.len();
                if raw_len > 0 && raw.byte(raw_len - 1) == b'\n' {
                    raw.slice(..raw_len - 1)
                } else {
                    raw
                }
            } else {
                changed_text.slice(0..0)
            };
            let line_len = line.len();
            let mut wrapped_lines = Vec::<Range<usize>>::new();
            let mut prev_boundary_ix = 0;
            let mut indent_chars = 0;

            if let Some(wrap_width) = wrap_width
                && line_len > 0
            {
                let fits_fast = ascii_fit_len > 0
                    && line_len <= ascii_fit_len
                    && line.chunks().all(|chunk| is_printable_ascii(chunk.as_bytes()));
                if !fits_fast {
                    let line_str: Cow<str> = line.into();
                    boundaries.clear();
                    wrap_line(&line_str, wrap_width, &mut boundaries);
                    for &boundary in &boundaries {
                        wrapped_lines.push(prev_boundary_ix..boundary.ix);
                        prev_boundary_ix = boundary.ix;
                        indent_chars = boundary.next_indent;
                    }
                }
            }

            if prev_boundary_ix < line_len || prev_boundary_ix == 0 {
                wrapped_lines.push(prev_boundary_ix..line_len);
            }

            new_lines.push(LineItem {
                len: line_len,
                indent: indent_chars,
                wrapped_lines,
            });
        }

        self.lines.splice(start_row..end_row + 1, new_lines);
        self.text = changed_text.clone();
    }

    fn update_all(&mut self, text: &Rope, cx: &mut App) {
        self.update(text, &(0..text.len()), text, cx);
    }

    /// Return display point (with soft wrap) for a byte offset.
    pub(crate) fn offset_to_display_point(&self, offset: usize) -> WrapDisplayPoint {
        self.offset_to_display_point_with_affinity(offset, false)
    }

    /// Like [`Self::offset_to_display_point`], honouring caret line-end affinity at wrap boundaries.
    pub(crate) fn offset_to_display_point_with_affinity(
        &self,
        offset: usize,
        line_end_affinity: bool,
    ) -> WrapDisplayPoint {
        let row = self.text.offset_to_point(offset).row;
        let start = self.text.line_start_offset(row);

        let (wrapped_row, Some(line)) = self.lines.get_by_buffer_row(row) else {
            return WrapDisplayPoint::new(self.len(), 0, 0);
        };

        let local_offset = offset.saturating_sub(start);
        for (ix, range) in line.wrapped_lines.iter().enumerate() {
            let matches =
                range.contains(&local_offset) || (line_end_affinity && local_offset == range.end);
            if matches {
                return WrapDisplayPoint::new(
                    wrapped_row + ix,
                    ix,
                    local_offset.saturating_sub(range.start),
                );
            }
        }

        let last_range = line.wrapped_lines.last().unwrap_or(&(0..0));
        let ix = line.lines_len().saturating_sub(1);
        WrapDisplayPoint::new(wrapped_row + ix, ix, last_range.len())
    }

    /// Return byte offset for a display point.
    pub(crate) fn display_point_to_offset(&self, point: WrapDisplayPoint) -> usize {
        let Some((row, wrapped_row, line)) = self.lines.get_by_wrap_row(point.row) else {
            return self.text.len();
        };

        let line_start = self.text.line_start_offset(row);
        let local_row = point.row.saturating_sub(wrapped_row);
        if let Some(range) = line.wrapped_lines.get(local_row) {
            line_start + (range.start + point.column).min(range.end)
        } else {
            line_start + line.len()
        }
    }

    pub(crate) fn display_point_to_point(&self, point: WrapDisplayPoint) -> TreeSitterPoint {
        let offset = self.display_point_to_offset(point);
        self.text.offset_to_point(offset)
    }

    pub(crate) fn point_to_display_point(&self, point: TreeSitterPoint) -> WrapDisplayPoint {
        let offset = self.text.point_to_offset(point);
        self.offset_to_display_point(offset)
    }
}

/// A display point within soft-wrapped text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WrapDisplayPoint {
    /// 0-based soft-wrapped visual row index.
    pub(crate) row: usize,
    /// 0-based sub-row index within the logical buffer line.
    pub(crate) local_row: usize,
    /// 0-based UTF-8 byte column within the visual row.
    pub(crate) column: usize,
}

impl WrapDisplayPoint {
    pub(crate) fn new(row: usize, local_row: usize, column: usize) -> Self {
        Self {
            row,
            local_row,
            column,
        }
    }
}

/// Shaped visual rows for a single visible buffer line.
pub(crate) struct BufferLineLayout {
    len: usize,
    pub(crate) wrapped_lines: Vec<InputLine>,
    pub(crate) wrap_indent: Pixels,
    longest_width: Pixels,
}

impl BufferLineLayout {
    pub(crate) fn new() -> Self {
        Self {
            len: 0,
            longest_width: px(0.),
            wrapped_lines: Vec::new(),
            wrap_indent: px(0.),
        }
    }

    pub(crate) fn wrap_indent(mut self, wrap_indent: Pixels) -> Self {
        self.wrap_indent = wrap_indent;
        self
    }

    #[inline]
    fn line_indent(&self, line_index: usize) -> Pixels {
        if line_index == 0 {
            px(0.)
        } else {
            self.wrap_indent
        }
    }

    pub(crate) fn lines(mut self, wrapped_lines: Vec<ShapedLine>) -> Self {
        self.set_wrapped_lines(wrapped_lines);
        self
    }

    pub(crate) fn set_wrapped_lines(&mut self, wrapped_lines: Vec<ShapedLine>) {
        self.len = wrapped_lines.iter().map(|l| l.len).sum();
        self.longest_width = wrapped_lines
            .iter()
            .map(|l| l.width)
            .max()
            .unwrap_or_default();
        self.wrapped_lines = wrapped_lines.into_iter().map(InputLine::from).collect();
    }

    pub(crate) fn inline_lines(mut self, lines: Vec<InputLine>) -> Self {
        self.len = lines.iter().map(InputLine::len).sum();
        self.longest_width = lines
            .iter()
            .map(InputLine::width)
            .max()
            .unwrap_or_default();
        self.wrapped_lines = lines;
        self
    }

    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    pub(crate) fn position_for_index(
        &self,
        offset: usize,
        last_layout: &LastLayout,
        line_end_affinity: bool,
    ) -> Option<Point<Pixels>> {
        let mut acc_len = 0;
        let mut offset_y = px(0.);

        for (i, line) in self.wrapped_lines.iter().enumerate() {
            let is_last = i + 1 == self.wrapped_lines.len();
            let line_len = line.len();

            let matches = if line_len == 0 {
                offset == acc_len
            } else if is_last || line_end_affinity {
                offset >= acc_len && offset <= acc_len + line_len
            } else {
                offset >= acc_len && offset < acc_len + line_len
            };

            if matches {
                let x = line.x_for_index(offset.saturating_sub(acc_len)) + self.line_indent(i);
                return Some(point(x, offset_y));
            }

            acc_len += if is_last { line_len + 1 } else { line_len };
            offset_y += last_layout.line_height;
        }

        None
    }

    pub(crate) fn closest_index_for_x(&self, x: Pixels) -> usize {
        let mut acc_len = 0;

        for (i, line) in self.wrapped_lines.iter().enumerate() {
            let line_indent = self.line_indent(i);
            if x <= line_indent + line.width() {
                return acc_len + line.closest_index_for_x(x - line_indent);
            }
            acc_len += line.len();
        }

        acc_len
    }

    fn wrapped_line_at(
        &self,
        pos: Point<Pixels>,
        last_layout: &LastLayout,
    ) -> Option<(usize, usize, Pixels)> {
        let mut offset = 0;
        let mut line_top = px(0.);

        for (i, line) in self.wrapped_lines.iter().enumerate() {
            let line_bottom = line_top + last_layout.line_height;
            if pos.y >= line_top && pos.y < line_bottom {
                return Some((i, offset, pos.x - self.line_indent(i)));
            }

            offset += line.len();
            line_top = line_bottom;
        }

        None
    }

    pub(crate) fn closest_index_for_position(
        &self,
        pos: Point<Pixels>,
        last_layout: &LastLayout,
    ) -> Option<(usize, bool)> {
        let (i, offset, x) = self.wrapped_line_at(pos, last_layout)?;
        let line = &self.wrapped_lines[i];
        let ix = line.closest_index_for_x(x);
        let line_end_affinity = i + 1 < self.wrapped_lines.len() && ix == line.len();

        Some((offset + ix, line_end_affinity))
    }

    pub(crate) fn size(&self, line_height: Pixels) -> Size<Pixels> {
        let width = self
            .wrapped_lines
            .iter()
            .enumerate()
            .map(|(ix, line)| line.width() + self.line_indent(ix))
            .max()
            .unwrap_or(self.longest_width);
        size(width, self.wrapped_lines.len() * line_height)
    }

    pub(crate) fn paint(
        &self,
        pos: Point<Pixels>,
        line_height: Pixels,
        window: &mut Window,
        cx: &mut App,
    ) {
        for (ix, line) in self.wrapped_lines.iter().enumerate() {
            line.paint(
                pos + point(self.line_indent(ix), ix * line_height),
                line_height,
                window,
                cx,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;

    use crate::ui::{Boundary, font, px};

    fn test_font() -> Font {
        font("Arial")
    }

    #[test]
    fn measured_wrap_keeps_cjk_latin_boundary_stable_during_edits() {
        let measure = |text: &str| {
            px(text
                .chars()
                .map(|c| if c.is_ascii() { 1. } else { 2. })
                .sum())
        };
        let mut wrapper = TextWrapper::new(test_font(), px(14.), Some(px(6.)));
        let mut previous = Rope::new();
        for value in ["abcd的", "abcd的s", "abcd的ss", "abcd的s", "abcd的"] {
            let text = Rope::from(value);
            let start = previous.len().min(text.len());
            let inserted = Rope::from(text.slice(start..).to_string());
            wrapper.update_inner(
                &text,
                &(start..previous.len()),
                &inserted,
                0,
                &mut |line, width, out| {
                    measured_wrap_boundaries(line, width, false, measure, out);
                },
            );
            let expected: &[Range<usize>] = if value.ends_with('s') {
                &[0..7, 7..value.len()]
            } else {
                std::slice::from_ref(&(0..7))
            };
            assert_eq!(wrapper.line(0).unwrap().wrapped_lines.as_slice(), expected);
            let cursor = wrapper.offset_to_display_point(value.len());
            assert_eq!(cursor.row, usize::from(value.ends_with('s')));
            previous = text;
        }
    }

    #[test]
    fn measured_wrap_preserves_words_graphemes_and_indentation() {
        let wrap = |text: &str, width, preserve_indent| {
            let mut out = Vec::new();
            measured_wrap_boundaries(
                text,
                px(width),
                preserve_indent,
                |s| px(s.graphemes(true).count() as f32),
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
        // Closing punctuation stays with the preceding Chinese character.
        assert_eq!(wrap("你好，世界", 2., false), vec![3, 9]);
        // Non-breaking space glues words together; hyphen allows breaking after it.
        assert!(crate::win32::line_break_opportunities("foo\u{00A0}bar").is_empty());
        assert_eq!(crate::win32::line_break_opportunities("foo-bar"), vec![4]);
        // Astral-plane surrogate pairs (emoji) map cleanly to UTF-8 boundaries.
        assert_eq!(wrap("👨‍👩‍👧‍👦 word", 4., false), vec!["👨‍👩‍👧‍👦 ".len()]);
        assert_eq!(wrap("你好😀世界", 3., false), vec!["你好😀".len()]);
    }

    #[test]
    fn test_update() {
        let mut wrapper = TextWrapper::new(test_font(), px(14.), None);
        let mut text = Rope::from(
            "Hello, 世界!\r\nThis is second line.\nThis is third line.\n这里是第 4 行。",
        );

        fn fake_wrap_line(_line: &str, _wrap_width: Pixels, _out: &mut Vec<Boundary>) {}

        #[track_caller]
        fn assert_wrapper_lines(text: &Rope, wrapper: &TextWrapper, expected_lines: &[&[&str]]) {
            let mut actual_lines = vec![];
            let mut offset = 0;
            for line in wrapper.iter_lines() {
                actual_lines.push(
                    line.wrapped_lines
                        .iter()
                        .map(|range| text.slice(offset + range.start..offset + range.end))
                        .collect::<Vec<_>>(),
                );
                offset += line.len() + 1;
            }
            assert_eq!(actual_lines, expected_lines);
        }

        wrapper.update_inner(&text, &(0..text.len()), &text, 0, &mut fake_wrap_line);
        assert_eq!(wrapper.lines_count(), 4);
        assert_wrapper_lines(
            &text,
            &wrapper,
            &[
                &["Hello, 世界!\r"],
                &["This is second line."],
                &["This is third line."],
                &["这里是第 4 行。"],
            ],
        );

        // Add new text to end
        let range = text.len()..text.len();
        let new_text = "New text";
        text.replace(range.clone(), new_text);
        wrapper.update_inner(&text, &range, &Rope::from(new_text), 0, &mut fake_wrap_line);
        assert_eq!(
            text.to_string(),
            "Hello, 世界!\r\nThis is second line.\nThis is third line.\n这里是第 4 行。New text"
        );
        assert_eq!(wrapper.lines_count(), 4);
        assert_wrapper_lines(
            &text,
            &wrapper,
            &[
                &["Hello, 世界!\r"],
                &["This is second line."],
                &["This is third line."],
                &["这里是第 4 行。New text"],
            ],
        );

        // Replace first line `Hello` with `AAA`
        let range = 0..5;
        let new_text = "AAA";
        text.replace(range.clone(), new_text);
        wrapper.update_inner(&text, &range, &Rope::from(new_text), 0, &mut fake_wrap_line);
        assert_eq!(
            text.to_string(),
            "AAA, 世界!\r\nThis is second line.\nThis is third line.\n这里是第 4 行。New text"
        );
        assert_eq!(wrapper.lines_count(), 4);
        assert_wrapper_lines(
            &text,
            &wrapper,
            &[
                &["AAA, 世界!\r"],
                &["This is second line."],
                &["This is third line."],
                &["这里是第 4 行。New text"],
            ],
        );

        // Remove the second line
        let start_offset = text.line_start_offset(1);
        let end_offset = text.line_end_offset(1);
        let range = start_offset..end_offset + 1;
        text.replace(range.clone(), "");
        wrapper.update_inner(&text, &range, &Rope::from(""), 0, &mut fake_wrap_line);
        assert_eq!(
            text.to_string(),
            "AAA, 世界!\r\nThis is third line.\n这里是第 4 行。New text"
        );
        assert_eq!(wrapper.lines_count(), 3);
        assert_wrapper_lines(
            &text,
            &wrapper,
            &[
                &["AAA, 世界!\r"],
                &["This is third line."],
                &["这里是第 4 行。New text"],
            ],
        );

        // Replace the first 2 lines
        let range = text.line_start_offset(0)..text.line_end_offset(1) + 1;
        let new_text = "This is a new line.\nThis is new line 2.\n";
        text.replace(range.clone(), new_text);
        wrapper.update_inner(&text, &range, &Rope::from(new_text), 0, &mut fake_wrap_line);
        assert_eq!(
            text.to_string(),
            "This is a new line.\nThis is new line 2.\n这里是第 4 行。New text"
        );
        assert_eq!(wrapper.lines_count(), 3);
        assert_wrapper_lines(
            &text,
            &wrapper,
            &[
                &["This is a new line."],
                &["This is new line 2."],
                &["这里是第 4 行。New text"],
            ],
        );

        // Add a new line at the end
        let range = text.len()..text.len();
        let new_text = "\nThis is a new line at the end.";
        text.replace(range.clone(), new_text);
        wrapper.update_inner(&text, &range, &Rope::from(new_text), 0, &mut fake_wrap_line);
        assert_eq!(wrapper.lines_count(), 4);
        assert_wrapper_lines(
            &text,
            &wrapper,
            &[
                &["This is a new line."],
                &["This is new line 2."],
                &["这里是第 4 行。New text"],
                &["This is a new line at the end."],
            ],
        );

        // Add a new line at the beginning
        let range = 0..0;
        let new_text = "This is a new line at the beginning.\n";
        text.replace(range.clone(), new_text);
        wrapper.update_inner(&text, &range, &Rope::from(new_text), 0, &mut fake_wrap_line);
        assert_eq!(wrapper.lines_count(), 5);
        assert_wrapper_lines(
            &text,
            &wrapper,
            &[
                &["This is a new line at the beginning."],
                &["This is a new line."],
                &["This is new line 2."],
                &["这里是第 4 行。New text"],
                &["This is a new line at the end."],
            ],
        );

        // Remove all to leave at least one line in `lines`.
        let range = 0..text.len();
        text.replace(range.clone(), "");
        wrapper.update_inner(&text, &range, &Rope::from(""), 0, &mut fake_wrap_line);
        assert_eq!(text.to_string(), "");
        assert_eq!(wrapper.lines_count(), 1);
        assert_eq!(wrapper.line(0).unwrap().wrapped_lines.as_slice(), std::slice::from_ref(&(0..0)));

        // Test full update
        let range = 0..text.len();
        let new_text = "This is a full text.\nThis is a second line.";
        text.replace(range.clone(), new_text);
        wrapper.update_inner(&text, &range, &text, 0, &mut fake_wrap_line);
        assert_eq!(wrapper.lines_count(), 2);
    }

    #[test]
    fn test_longest_row_after_shrink() {
        let mut wrapper = TextWrapper::new(test_font(), px(14.), None);
        let mut text = Rope::from("aa\nthis is the longest line\nbb");
        wrapper.update_inner(&text, &(0..text.len()), &text, 0, &mut |_, _, _| {});
        assert_eq!(wrapper.longest_row(), 1);

        let start = text.line_start_offset(0);
        let end = text.line_end_offset(0);
        let range = start..end;
        let new_text = "a very very long first line now";
        text.replace(range.clone(), new_text);
        wrapper.update_inner(&text, &range, &Rope::from(new_text), 0, &mut |_, _, _| {});
        assert_eq!(wrapper.longest_row(), 0);
    }

    #[test]
    fn test_edit_last_line_and_full_delete() {
        let mut wrapper = TextWrapper::new(test_font(), px(14.), None);
        let mut text = Rope::from("one\ntwo\nthree");
        wrapper.update_inner(&text, &(0..text.len()), &text, 0, &mut |_, _, _| {});
        assert_eq!(wrapper.lines_count(), 3);

        let start = text.line_start_offset(2);
        let range = start..text.len();
        let new_text = "THREE EDITED";
        text.replace(range.clone(), new_text);
        wrapper.update_inner(&text, &range, &Rope::from(new_text), 0, &mut |_, _, _| {});
        assert_eq!(wrapper.lines_count(), 3);
        assert_eq!(wrapper.line(2).unwrap().len(), "THREE EDITED".len());

        let range = 0..text.len();
        text.replace(range.clone(), "");
        wrapper.update_inner(&text, &range, &Rope::from(""), 0, &mut |_, _, _| {});
        assert_eq!(wrapper.lines_count(), 1);
        assert_eq!(wrapper.len(), 1);
        assert_eq!(wrapper.line(0).unwrap().wrapped_lines.as_slice(), std::slice::from_ref(&(0..0)));
    }

    #[test]
    fn test_wrap_row_buffer_line_boundaries() {
        let mut wrapper = TextWrapper::new(test_font(), px(14.), None);
        wrapper.text = Rope::from("aa\nbbbb\nc");
        wrapper.lines = LineTree::from_vec(vec![
            LineItem {
                len: 2,
                indent: 0,
                wrapped_lines: std::iter::once(0..2).collect(),
            },
            LineItem {
                len: 4,
                indent: 0,
                wrapped_lines: vec![0..2, 2..4],
            },
            LineItem {
                len: 1,
                indent: 0,
                wrapped_lines: std::iter::once(0..1).collect(),
            },
        ]);

        assert_eq!(wrapper.lines_count(), 3);
        assert_eq!(wrapper.len(), 4);

        assert_eq!(wrapper.buffer_line_to_first_wrap_row(0), 0);
        assert_eq!(wrapper.buffer_line_to_first_wrap_row(1), 1);
        assert_eq!(wrapper.buffer_line_to_first_wrap_row(2), 3);
        assert_eq!(wrapper.buffer_line_to_first_wrap_row(3), 4);

        assert_eq!(wrapper.buffer_line_to_wrap_row_range(0), 0..1);
        assert_eq!(wrapper.buffer_line_to_wrap_row_range(1), 1..3);
        assert_eq!(wrapper.buffer_line_to_wrap_row_range(2), 3..4);
        assert_eq!(wrapper.buffer_line_to_wrap_row_range(3), 4..4);

        assert_eq!(wrapper.wrap_row_to_buffer_line(0), 0);
        assert_eq!(wrapper.wrap_row_to_buffer_line(1), 1);
        assert_eq!(wrapper.wrap_row_to_buffer_line(2), 1);
        assert_eq!(wrapper.wrap_row_to_buffer_line(3), 2);
        assert_eq!(wrapper.wrap_row_to_buffer_line(4), 2);
    }

    #[test]
    fn test_wrap_row_queries_after_incremental_splice() {
        let mut wrapper = TextWrapper::new(test_font(), px(14.), Some(px(10.)));
        let mut text = Rope::from("aa\nbbbb\nc");
        let mut fake_wrap_line = |line: &str, _wrap_width: Pixels, out: &mut Vec<Boundary>| {
            if line.len() > 2 {
                out.push(Boundary {
                    ix: 2,
                    next_indent: 0,
                });
            }
        };

        wrapper.update_inner(&text, &(0..text.len()), &text, 0, &mut fake_wrap_line);
        assert_eq!(wrapper.buffer_line_to_wrap_row_range(0), 0..1);
        assert_eq!(wrapper.buffer_line_to_wrap_row_range(1), 1..3);
        assert_eq!(wrapper.buffer_line_to_wrap_row_range(2), 3..4);

        let range = text.line_start_offset(1)..text.line_end_offset(1);
        let new_text = "dd\neeee";
        text.replace(range.clone(), new_text);
        wrapper.update_inner(&text, &range, &Rope::from(new_text), 0, &mut fake_wrap_line);

        assert_eq!(wrapper.lines_count(), 4);
        assert_eq!(wrapper.len(), 5);
        assert_eq!(wrapper.buffer_line_to_wrap_row_range(0), 0..1);
        assert_eq!(wrapper.buffer_line_to_wrap_row_range(1), 1..2);
        assert_eq!(wrapper.buffer_line_to_wrap_row_range(2), 2..4);
        assert_eq!(wrapper.buffer_line_to_wrap_row_range(3), 4..5);
        assert_eq!(wrapper.wrap_row_to_buffer_line(0), 0);
        assert_eq!(wrapper.wrap_row_to_buffer_line(1), 1);
        assert_eq!(wrapper.wrap_row_to_buffer_line(2), 2);
        assert_eq!(wrapper.wrap_row_to_buffer_line(3), 2);
        assert_eq!(wrapper.wrap_row_to_buffer_line(4), 3);
    }

    #[test]
    fn test_line_layout() {
        let mut line_layout = BufferLineLayout::new();
        let line1 = ShapedLine::default().with_len(100);
        let line2 = ShapedLine::default().with_len(50);
        line_layout.set_wrapped_lines(vec![line1, line2]);
        assert_eq!(line_layout.len(), 150);
        assert_eq!(line_layout.wrapped_lines.len(), 2);
    }

    fn test_last_layout(line_height: Pixels) -> LastLayout {
        LastLayout {
            visible_buffer_lines: vec![0],
            visible_line_byte_offsets: vec![0],
            visible_top: px(0.),
            visible_range_offset: 0..0,
            lines: Rc::new(vec![]),
            line_height,
            wrap_width: None,
            space_width: px(0.),
            cursor_bounds: None,
        }
    }

    #[test]
    fn test_position_for_index_prefers_first_leading_empty_visual_line() {
        let mut line_layout = BufferLineLayout::new();
        line_layout.set_wrapped_lines(vec![
            ShapedLine::default(),
            ShapedLine::default(),
            ShapedLine::default().with_len(3),
        ]);

        assert_eq!(
            line_layout.position_for_index(0, &test_last_layout(px(20.)), false),
            Some(point(px(0.), px(0.)))
        );
    }

    #[test]
    fn clicking_past_a_wrapped_row_keeps_the_caret_on_that_row() {
        let mut line_layout = BufferLineLayout::new();
        line_layout.set_wrapped_lines(vec![
            ShapedLine::default().with_len(10),
            ShapedLine::default().with_len(5),
        ]);
        let last_layout = test_last_layout(px(20.));

        let (ix, line_end_affinity) = line_layout
            .closest_index_for_position(point(px(999.), px(5.)), &last_layout)
            .unwrap();
        assert_eq!(ix, 10);
        assert!(line_end_affinity);

        assert_eq!(
            line_layout
                .position_for_index(ix, &last_layout, line_end_affinity)
                .map(|pos| pos.y),
            Some(px(0.))
        );
        assert_eq!(
            line_layout
                .position_for_index(ix, &last_layout, false)
                .map(|pos| pos.y),
            Some(px(20.))
        );

        let (ix, line_end_affinity) = line_layout
            .closest_index_for_position(point(px(999.), px(25.)), &last_layout)
            .unwrap();
        assert_eq!(ix, 15);
        assert!(!line_end_affinity);
    }

    #[test]
    fn a_wrap_boundary_offset_resolves_to_the_row_the_caret_is_drawn_on() {
        let mut wrapper = TextWrapper::new(test_font(), px(14.), None);
        wrapper.text = Rope::from("first line\nthis one wraps");
        wrapper.lines = LineTree::from_vec(vec![
            LineItem {
                len: Rope::from("first line").len(),
                indent: 0,
                wrapped_lines: std::iter::once(0..10).collect(),
            },
            LineItem {
                len: Rope::from("this one wraps").len(),
                indent: 0,
                wrapped_lines: vec![0..9, 9..14],
            },
        ]);

        assert_eq!(
            wrapper.offset_to_display_point_with_affinity(20, true),
            WrapDisplayPoint::new(1, 0, 9)
        );
        assert_eq!(
            wrapper.offset_to_display_point_with_affinity(20, false),
            WrapDisplayPoint::new(2, 1, 0)
        );
        assert_eq!(
            wrapper.offset_to_display_point(20),
            wrapper.offset_to_display_point_with_affinity(20, false)
        );

        for line_end_affinity in [false, true] {
            assert_eq!(
                wrapper.offset_to_display_point_with_affinity(23, line_end_affinity),
                WrapDisplayPoint::new(2, 1, 3)
            );
        }
    }

    #[test]
    fn test_offset_to_display_point() {
        let mut wrapper = TextWrapper::new(test_font(), px(14.), None);
        wrapper.text = Rope::from(
            "Hello, 世界!\r\nThis is second line.\nThis is third line.\n这里是第 4 行。",
        );
        wrapper.lines = LineTree::from_vec(vec![
            LineItem {
                len: Rope::from("Hello, 世界!\r").len(),
                indent: 0,
                wrapped_lines: std::iter::once(0..15).collect(),
            },
            LineItem {
                len: Rope::from("This is second line.\n").len(),
                indent: 0,
                wrapped_lines: vec![0..10, 10..20],
            },
            LineItem {
                len: Rope::from("This is third line.\n").len(),
                indent: 0,
                wrapped_lines: vec![0..9, 9..15, 15..20],
            },
            LineItem {
                len: Rope::from("这里是第 4 行。").len(),
                indent: 0,
                wrapped_lines: std::iter::once(0..22).collect(),
            },
        ]);

        assert_eq!(
            wrapper.offset_to_display_point(12),
            WrapDisplayPoint::new(0, 0, 12)
        );
        assert_eq!(
            wrapper.offset_to_display_point(15),
            WrapDisplayPoint::new(0, 0, 15)
        );
        assert_eq!(
            wrapper.offset_to_display_point(16),
            WrapDisplayPoint::new(1, 0, 0)
        );
        assert_eq!(
            wrapper.offset_to_display_point(21),
            WrapDisplayPoint::new(1, 0, 5)
        );
        assert_eq!(
            wrapper.offset_to_display_point(27),
            WrapDisplayPoint::new(2, 1, 1)
        );
        assert_eq!(
            wrapper.offset_to_display_point(37),
            WrapDisplayPoint::new(3, 0, 0)
        );
        assert_eq!(
            wrapper.offset_to_display_point(54),
            WrapDisplayPoint::new(5, 2, 2)
        );
        assert_eq!(
            wrapper.offset_to_display_point(59),
            WrapDisplayPoint::new(6, 0, 2)
        );

        assert_eq!(
            wrapper.display_point_to_offset(WrapDisplayPoint::new(6, 0, 2)),
            59
        );
        assert_eq!(
            wrapper.display_point_to_offset(WrapDisplayPoint::new(5, 2, 2)),
            54
        );
        assert_eq!(
            wrapper.display_point_to_offset(WrapDisplayPoint::new(3, 0, 0)),
            37
        );
        assert_eq!(
            wrapper.display_point_to_offset(WrapDisplayPoint::new(2, 1, 1)),
            27
        );
        assert_eq!(
            wrapper.display_point_to_offset(WrapDisplayPoint::new(1, 0, 5)),
            21
        );
        assert_eq!(
            wrapper.display_point_to_offset(WrapDisplayPoint::new(1, 0, 0)),
            16
        );
        assert_eq!(
            wrapper.display_point_to_offset(WrapDisplayPoint::new(0, 0, 15)),
            15
        );
    }

    #[test]
    fn test_wrapping_indent_same_keeps_indent_reserved() {
        let mut wrapper = TextWrapper::new(test_font(), px(14.0), Some(px(10.)));
        let text = Rope::from("  abcdefghijklmnopqrstuv");
        let mut fake_wrap_line = |line: &str, _wrap_width: Pixels, out: &mut Vec<Boundary>| {
            if line.starts_with(' ') {
                out.push(Boundary {
                    ix: 5,
                    next_indent: 2,
                });
            }
        };

        wrapper.update_inner(&text, &(0..text.len()), &text, 0, &mut fake_wrap_line);
        let line = wrapper.line(0).unwrap();
        assert_eq!(line.indent, 2);
        assert_eq!(line.wrapped_lines.as_slice(), [0..5, 5..24]);
    }

    #[test]
    fn test_wrap_indent_offsets_continuation_lines() {
        let mut line_layout = BufferLineLayout::new();
        line_layout.set_wrapped_lines(vec![
            ShapedLine::default().with_len(5),
            ShapedLine::default().with_len(10),
        ]);
        line_layout = line_layout.wrap_indent(px(20.0));

        let last_layout = LastLayout {
            visible_buffer_lines: vec![0],
            visible_line_byte_offsets: vec![0],
            visible_top: px(0.),
            visible_range_offset: 0..0,
            lines: Rc::new(vec![]),
            line_height: px(20.0),
            wrap_width: Some(px(10.)),
            space_width: px(0.),
            cursor_bounds: None,
        };

        assert_eq!(
            line_layout.position_for_index(0, &last_layout, false),
            Some(point(px(0.), px(0.))),
        );
        assert_eq!(
            line_layout.position_for_index(6, &last_layout, false),
            Some(point(px(20.), px(20.))),
        );
    }

    #[test]
    fn test_ascii_fast_fit_bypasses_wrap_line_for_short_printable_ascii() {
        let mut wrapper = TextWrapper::new(test_font(), px(14.), Some(px(100.)));
        let text = Rope::from("short ascii\nlonger ascii line\n日本語\n");
        let mut wrapped_calls = Vec::new();
        wrapper.update_inner(
            &text,
            &(0..text.len()),
            &text,
            12,
            &mut |line, _, _| {
                wrapped_calls.push(line.to_string());
            },
        );
        assert_eq!(wrapper.lines_count(), 4);
        assert_eq!(wrapped_calls, vec!["longer ascii line", "日本語"]);
    }
}
