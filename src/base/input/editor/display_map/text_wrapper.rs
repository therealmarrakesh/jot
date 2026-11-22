use super::inline_line::InputLine;
use std::borrow::Cow;
use std::ops::{Range, RangeInclusive};

use gpui::{
    App, Font, LineFragment, Pixels, Point, ShapedLine, Size, TextAlign, Window, point, px, size,
};
use ropey::Rope;
use gpui::smallvec::SmallVec;
use unicode_segmentation::UnicodeSegmentation as _;

use crate::base::input::{Point as TreeSitterPoint, RopeExt, layout::LastLayout};

/// Controls how soft-wrapped continuation lines are indented.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WrappingIndent {
    /// Continuation lines start flush-left at the full editor width.
    None,
    /// Continuation lines keep the same indentation as the first line.
    #[default]
    Same,
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
    wrapping_indent: WrappingIndent,
    mut measure: impl FnMut(&str) -> Pixels,
) -> Vec<gpui::Boundary> {
    let indent = if wrapping_indent == WrappingIndent::Same {
        text.chars()
            .take_while(|&c| c == ' ')
            .count()
            .min(gpui::LineWrapper::MAX_INDENT as usize)
    } else {
        0
    };
    let indent_width = measure(&text[..indent]);
    let ends: Vec<usize> = text
        .grapheme_indices(true)
        .map(|(ix, grapheme)| ix + grapheme.len())
        .collect();
    let opportunities: Vec<usize> = unicode_linebreak::linebreaks(text)
        .map(|(ix, _)| ix)
        .filter(|ix| ends.binary_search(ix).is_ok())
        .collect();
    let mut result = Vec::new();
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
        result.push(gpui::Boundary {
            ix: end,
            next_indent: indent as u32,
        });
        first = ends.partition_point(|&ix| ix <= end);
        start = end;
    }
    result
}

/// A line with soft wrapped lines info.
#[derive(Debug, Clone)]
pub(crate) struct LineItem {
    /// The byte length of the line, without the end `\n`.
    len: usize,
    /// Number of leading characters of the line reserved as indentation for continuation wrapped
    /// lines, when [`WrappingIndent::Same`] is used.
    ///
    /// Zero when [`WrappingIndent::None`] is used or the line is not wrapped.
    pub(crate) indent: u32,
    /// The soft wrapped lines relative byte range (0..len) of this line (Include first line).
    ///
    /// Not contains the line end `\n`.
    pub(crate) wrapped_lines: SmallVec<[Range<usize>; 1]>,
}

impl LineItem {
    /// Get the bytes length of this line.
    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    /// Get number of soft wrapped lines of this line (include the first line).
    #[inline]
    pub(crate) fn lines_len(&self) -> usize {
        self.wrapped_lines.len()
    }
}

/// Summary of a chunk or full [`LineTree`].
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct LineSummary {
    /// Number of buffer lines.
    buffer_rows: usize,
    /// Number of wrap rows (sum of each line's `lines_len()`).
    wrap_rows: usize,
    /// Byte length of the longest line in this subtree.
    max_line_len: usize,
    /// Buffer row (relative to this subtree) of the first line achieving `max_line_len`.
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
        // Keep the leftmost row that achieves a strictly greater length.
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

/// Used to prepare the text with soft wrap to be get lines to displayed in the Editor.
///
/// After use lines to calculate the scroll size of the Editor.
pub(crate) struct TextWrapper {
    text: Rope,
    font: Font,
    font_size: Pixels,
    /// If is none, it means the text is not wrapped
    wrap_width: Option<Pixels>,
    wrapping_indent: WrappingIndent,
    /// The lines by split \n
    pub(crate) lines: LineTree,

    ascii_max_width: Option<Pixels>,
    _initialized: bool,
}

#[allow(unused)]
impl TextWrapper {
    pub(crate) fn new(font: Font, font_size: Pixels, wrap_width: Option<Pixels>) -> Self {
        Self {
            text: Rope::new(),
            font,
            font_size,
            wrap_width,
            wrapping_indent: WrappingIndent::default(),
            lines: LineTree::new(),
            ascii_max_width: None,
            _initialized: false,
        }
    }

    #[inline]
    pub(crate) fn set_default_text(&mut self, text: &Rope) {
        self.text = text.clone();
    }

    /// Get reference to the rope text.
    #[inline]
    pub(crate) fn text(&self) -> &Rope {
        &self.text
    }

    /// Get the total number of lines including wrapped lines.
    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.lines.summary().wrap_rows
    }

    /// Get the total number of buffer lines.
    #[inline]
    pub(crate) fn lines_count(&self) -> usize {
        self.lines.summary().buffer_rows
    }

    /// Get the 0-based row index of the longest line (by byte length).
    #[inline]
    pub(crate) fn longest_row(&self) -> usize {
        self.lines.summary().longest_row
    }

    /// Get the line item by buffer row index.
    #[inline]
    pub(crate) fn line(&self, row: usize) -> Option<&LineItem> {
        self.lines.get_by_buffer_row(row).1
    }

    /// Iterate buffer lines in order.
    #[inline]
    pub(crate) fn iter_lines(&self) -> impl Iterator<Item = &LineItem> {
        self.lines.iter()
    }

    /// First wrap row of buffer line `row`. Returns the total wrap row count if `row` is
    /// out of range.
    pub(crate) fn buffer_line_to_first_wrap_row(&self, row: usize) -> usize {
        self.lines.get_by_buffer_row(row).0
    }

    /// Wrap row range of buffer line `row`.
    pub(crate) fn buffer_line_to_wrap_row_range(&self, row: usize) -> Range<usize> {
        let (start, item) = self.lines.get_by_buffer_row(row);
        let len = item.map(|l| l.lines_len()).unwrap_or(0);
        start..start + len
    }

    /// Buffer line containing wrap row `wrap_row`, clamped to the last line.
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

    pub(crate) fn set_wrapping_indent(&mut self, wrapping_indent: WrappingIndent, cx: &mut App) {
        if wrapping_indent == self.wrapping_indent {
            return;
        }

        self.wrapping_indent = wrapping_indent;
        self.update_all(&self.text.clone(), cx);
    }

    pub(crate) fn set_font(&mut self, font: Font, font_size: Pixels, cx: &mut App) {
        if self.font.eq(&font) && self.font_size == font_size {
            return;
        }

        self.font = font;
        self.font_size = font_size;
        self.ascii_max_width = None;
        self.update_all(&self.text.clone(), cx);
    }

    pub(crate) fn prepare_layout(
        &mut self,
        text: &Rope,
        font: Font,
        font_size: Pixels,
        wrap_width: Option<Pixels>,
        wrapping_indent: WrappingIndent,
        cx: &mut App,
    ) -> bool {
        let font_changed = !self.font.eq(&font) || self.font_size != font_size;
        let wrap_changed = self.wrap_width != wrap_width;
        let indent_changed = self.wrapping_indent != wrapping_indent;
        if font_changed {
            self.ascii_max_width = None;
        }
        self.font = font;
        self.font_size = font_size;
        self.wrap_width = wrap_width;
        self.wrapping_indent = wrapping_indent;

        if !self._initialized {
            self._initialized = true;
            self.update_all(text, cx);
            return true;
        }
        // When wrapping is off before and after, font/indent changes cannot change
        // any line's wrap boundaries, so avoid rebuilding the SumTree.
        if wrap_width.is_none() && !wrap_changed {
            return false;
        }
        if font_changed || wrap_changed || indent_changed {
            self.update_all(text, cx);
            return true;
        }
        false
    }

    pub(crate) fn prepare_if_need(&mut self, text: &Rope, cx: &mut App) -> bool {
        if self._initialized {
            return false;
        }
        self._initialized = true;
        self.update_all(text, cx);
        true
    }

    /// Update the text wrapper and recalculate the wrapped lines.
    pub(crate) fn update(
        &mut self,
        changed_text: &Rope,
        range: &Range<usize>,
        new_text: &Rope,
        cx: &mut App,
    ) {
        let Some(wrap_width) = self.wrap_width else {
            self._update(changed_text, range, new_text, 0, &mut |_, _, _| Vec::new());
            return;
        };

        let text_system_ref = cx.text_system();
        let mut line_wrapper = text_system_ref.line_wrapper(self.font.clone(), self.font_size);
        let max_ascii_width = *self.ascii_max_width.get_or_insert_with(|| {
            let font_id = text_system_ref.resolve_font(&self.font);
            (0x20u8..=0x7e)
                .map(|b| text_system_ref.layout_width(font_id, self.font_size, char::from(b)))
                .fold(px(0.), Pixels::max)
        });
        let ascii_fit_len = if max_ascii_width > px(0.) {
            (wrap_width / max_ascii_width).floor().max(0.) as usize
        } else {
            0
        };
        let text_system = gpui::WindowTextSystem::new(text_system_ref.clone());
        let font = self.font.clone();
        let font_size = self.font_size;
        let wrapping_indent = self.wrapping_indent;
        self._update(
            changed_text,
            range,
            new_text,
            ascii_fit_len,
            &mut |line_str, wrap_width, _line_start| {
                if wraps_by_char_widths(line_str) {
                    let indent = match wrapping_indent {
                        WrappingIndent::Same => gpui::IndentAdjustment::SameIndent,
                        WrappingIndent::None => gpui::IndentAdjustment::NoIndent,
                    };
                    return line_wrapper
                        .wrap_line(&[LineFragment::text(line_str)], wrap_width, indent)
                        .collect();
                }
                measured_wrap_boundaries(line_str, wrap_width, wrapping_indent, |text| {
                    text_system
                        .layout_line(
                            text,
                            font_size,
                            &[gpui::TextRun {
                                len: text.len(),
                                font: font.clone(),
                                color: gpui::black(),
                                background_color: None,
                                underline: None,
                                strikethrough: None,
                            }],
                            None,
                        )
                        .width
                })
            },
        );
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
        F: FnMut(&str, Pixels, usize) -> Vec<gpui::Boundary>,
    {
        self._update(changed_text, range, new_text, ascii_fit_len, wrap_line);
    }

    fn _update<F>(
        &mut self,
        changed_text: &Rope,
        range: &Range<usize>,
        new_text: &Rope,
        ascii_fit_len: usize,
        wrap_line: &mut F,
    ) where
        F: FnMut(&str, Pixels, usize) -> Vec<gpui::Boundary>,
    {
        // Remove the old changed lines.
        let buffer_line_count = self.lines_count();
        let start_row = self.text.offset_to_point(range.start).row;
        let start_row = start_row.min(buffer_line_count.saturating_sub(1));
        let end_row = self.text.offset_to_point(range.end).row;
        let end_row = end_row.min(buffer_line_count.saturating_sub(1));

        // To add the new lines.
        let new_start_row = changed_text.offset_to_point(range.start).row;
        let new_end_row = changed_text
            .offset_to_point(range.start + new_text.len())
            .row;

        let line_count = new_end_row.saturating_sub(new_start_row) + 1;
        let mut new_lines = Vec::with_capacity(line_count);
        let wrap_width = self.wrap_width;
        let mut line_start = changed_text.line_start_offset(new_start_row);
        let total_rows = changed_text.lines_len();
        let mut lines_iter =
            changed_text.lines_at(new_start_row.min(total_rows), ropey::LineType::LF);

        // line does not contain `\n`.
        for _row in new_start_row..=new_end_row {
            let (raw_len, line) = if let Some(raw) = lines_iter.next() {
                let raw_len = raw.len();
                let trimmed = if raw_len > 0 && raw.byte(raw_len - 1) == b'\n' {
                    raw.slice(..raw_len - 1)
                } else {
                    raw
                };
                (raw_len, trimmed)
            } else {
                (0, changed_text.slice(0..0))
            };
            let line_len = line.len();
            let mut wrapped_lines = SmallVec::<[Range<usize>; 1]>::new();
            let mut prev_boundary_ix = 0;
            let mut indent_chars = 0;

            if let Some(wrap_width) = wrap_width
                && line_len > 0
            {
                let fits_fast = ascii_fit_len > 0
                    && line_len <= ascii_fit_len
                    && line.chunks().all(|chunk| is_printable_ascii(chunk.as_bytes()));
                if !fits_fast {
                    // Borrowed for lines within a single rope chunk.
                    let line_str: Cow<str> = line.into();
                    match self.wrapping_indent {
                        WrappingIndent::Same => {
                            for boundary in wrap_line(&line_str, wrap_width, line_start) {
                                wrapped_lines.push(prev_boundary_ix..boundary.ix);
                                prev_boundary_ix = boundary.ix;
                                indent_chars = boundary.next_indent;
                            }
                        }
                        WrappingIndent::None => {
                            let boundaries = wrap_line(&line_str, wrap_width, line_start);
                            if let Some(first) = boundaries.first() {
                                if first.next_indent > 0 {
                                    let first_ix = first.ix;
                                    wrapped_lines.push(prev_boundary_ix..first_ix);
                                    prev_boundary_ix = first_ix;

                                    for boundary in wrap_line(
                                        &line_str[first_ix..],
                                        wrap_width,
                                        line_start + first_ix,
                                    ) {
                                        let ix = first_ix + boundary.ix;
                                        wrapped_lines.push(prev_boundary_ix..ix);
                                        prev_boundary_ix = ix;
                                    }
                                } else {
                                    for boundary in boundaries {
                                        wrapped_lines.push(prev_boundary_ix..boundary.ix);
                                        prev_boundary_ix = boundary.ix;
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Rest of the line
            if prev_boundary_ix < line_len || prev_boundary_ix == 0 {
                wrapped_lines.push(prev_boundary_ix..line_len);
            }

            new_lines.push(LineItem {
                len: line_len,
                indent: indent_chars,
                wrapped_lines,
            });
            line_start += raw_len;
        }

        self.lines.splice(start_row..end_row + 1, new_lines);

        self.text = changed_text.clone();
    }

    /// Update the text wrapper and recalculate the wrapped lines.
    ///
    /// If the `text` is the same as the current text, do nothing.
    fn update_all(&mut self, text: &Rope, cx: &mut App) {
        self.update(text, &(0..text.len()), &text, cx);
    }

    /// Return display point (with soft wrap) from the given byte offset in the text.
    ///
    /// Panics if the `offset` is out of bounds.
    pub(crate) fn offset_to_display_point(&self, offset: usize) -> WrapDisplayPoint {
        self.offset_to_display_point_with_affinity(offset, false)
    }

    /// Like [`Self::offset_to_display_point`], but honours the caret's line-end affinity.
    ///
    /// A soft wrap boundary is one offset shared by two visual rows. Without affinity it always
    /// resolves to the start of the second row, which is wrong for a caret that is being drawn at
    /// the end of the first one -- vertical movement would then step from the row below the one
    /// the user can see.
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
            // With affinity the boundary offset closes the current row instead of opening the
            // next one, so the range is matched inclusively.
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

        // Otherwise return the eof of the line.
        let last_range = line.wrapped_lines.last().unwrap_or(&(0..0));
        let ix = line.lines_len().saturating_sub(1);
        return WrapDisplayPoint::new(wrapped_row + ix, ix, last_range.len());
    }

    /// Return byte offset in the text from the given display point (with soft wrap).
    ///
    /// Panics if the `point.row` is out of bounds.
    pub(crate) fn display_point_to_offset(&self, point: WrapDisplayPoint) -> usize {
        let Some((row, wrapped_row, line)) = self.lines.get_by_wrap_row(point.row) else {
            return self.text.len();
        };

        let line_start = self.text.line_start_offset(row);
        let local_row = point.row.saturating_sub(wrapped_row);
        if let Some(range) = line.wrapped_lines.get(local_row) {
            line_start + (range.start + point.column).min(range.end)
        } else {
            // If not found, return the end of the line.
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

/// A display point within the soft-wrapped text.
///
/// This represents a position in the text after soft-wrapping,
/// with an additional `local_row` field tracking the wrap line
/// within the original buffer line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WrapDisplayPoint {
    /// The 0-based soft wrapped row index in the text.
    pub row: usize,
    /// The 0-based row index in local line (include first line).
    ///
    /// This value only valid when return from [`TextWrapper::offset_to_display_point`], otherwise it will be ignored.
    pub local_row: usize,
    /// The 0-based column byte index in the display line (with soft wrap).
    pub column: usize,
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

/// The layout info of a line with soft wrapped lines.
pub(crate) struct LineLayout {
    /// Total bytes length of this line.
    len: usize,
    /// The soft wrapped lines of this line (Include the first line).
    pub(crate) wrapped_lines: SmallVec<[InputLine; 1]>,
    /// Extra left offset applied to continuation wrapped lines, used to reserve the first line's
    /// indentation when [`WrappingIndent::Same`] is used.
    pub(crate) wrap_indent: Pixels,
    pub(crate) longest_width: Pixels,
    /// Whether any run of this line carries a background color, so [`Self::paint_background`]
    /// can skip the glyph walk for the common case of a line without highlights.
    has_background: bool,
}

impl LineLayout {
    pub(crate) fn new() -> Self {
        Self {
            len: 0,
            longest_width: px(0.),
            wrapped_lines: SmallVec::new(),
            wrap_indent: px(0.),
            has_background: false,
        }
    }

    /// Record whether any run of this line carries a background color.
    pub(crate) fn with_background(mut self, has_background: bool) -> Self {
        self.has_background = has_background;
        self
    }

    /// Set the left offset reserved for continuation wrapped lines.
    pub(crate) fn wrap_indent(mut self, wrap_indent: Pixels) -> Self {
        self.wrap_indent = wrap_indent;
        self
    }

    /// The pixel indent applied to the given visual line, relative to the line's
    /// leading text. Only continuation lines (index > 0) are indented.
    #[inline]
    fn line_indent(&self, line_index: usize) -> Pixels {
        if line_index == 0 {
            px(0.)
        } else {
            self.wrap_indent
        }
    }

    pub(crate) fn lines(mut self, wrapped_lines: SmallVec<[ShapedLine; 1]>) -> Self {
        self.set_wrapped_lines(wrapped_lines);
        self
    }

    pub(crate) fn set_wrapped_lines(&mut self, wrapped_lines: SmallVec<[ShapedLine; 1]>) {
        self.len = wrapped_lines.iter().map(|l| l.len).sum();
        let width = wrapped_lines
            .iter()
            .map(|l| l.width)
            .max()
            .unwrap_or_default();
        self.longest_width = width;
        self.wrapped_lines = wrapped_lines.into_iter().map(InputLine::from).collect();
    }

    pub(crate) fn inline_lines(mut self, lines: SmallVec<[InputLine; 1]>) -> Self {
        self.len = lines.iter().map(|line| line.len).sum();
        self.longest_width = lines
            .iter()
            .map(|line| line.width)
            .max()
            .unwrap_or_default();
        self.wrapped_lines = lines;
        self
    }

    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    /// Get the position (x, y) for the given index in this line layout.
    ///
    /// - The `offset` is a local byte index in this line layout.
    /// - When `line_end_affinity` is true, an offset at a soft wrap boundary is placed at
    ///   the end of the current visual line rather than the start of the next one.
    /// - The return value is relative to the top-left corner of this line layout, start from (0, 0)
    pub(crate) fn position_for_index(
        &self,
        offset: usize,
        last_layout: &LastLayout,
        line_end_affinity: bool,
    ) -> Option<Point<Pixels>> {
        let mut acc_len = 0;
        let mut offset_y = px(0.);

        let x_offset = last_layout.alignment_offset(self.longest_width);

        for (i, line) in self.wrapped_lines.iter().enumerate() {
            let is_last = i + 1 == self.wrapped_lines.len();

            let matches = if line.len == 0 {
                // Empty visual lines still own their boundary offset.
                offset == acc_len
            } else if is_last || line_end_affinity {
                // Inclusive: cursor can sit at end of this visual line.
                offset >= acc_len && offset <= acc_len + line.len
            } else {
                // Exclusive: boundary offset belongs to the next visual line.
                offset >= acc_len && offset < acc_len + line.len
            };

            if matches {
                let x = line.x_for_index(offset.saturating_sub(acc_len))
                    + x_offset
                    + self.line_indent(i);
                return Some(point(x, offset_y));
            }

            // Always advance by actual line length. The last line gets +1 so the
            // cursor can be placed after the final character.
            acc_len += if is_last { line.len + 1 } else { line.len };
            offset_y += last_layout.line_height;
        }

        None
    }

    /// Get the closest index for the given x in this line layout.
    ///
    /// This ignores y, so it only makes sense for a layout that is known to occupy a single
    /// visual line. Wrapped layouts must use [`Self::closest_index_for_position`], which also
    /// reports the caret affinity that a wrap boundary needs.
    pub(crate) fn closest_index_for_x(&self, x: Pixels, last_layout: &LastLayout) -> usize {
        let mut acc_len = 0;
        let x_offset = last_layout.alignment_offset(self.longest_width);
        let x = x - x_offset;

        for (i, line) in self.wrapped_lines.iter().enumerate() {
            let line_indent = self.line_indent(i);
            if x <= line_indent + line.width {
                return acc_len + line.closest_index_for_x(x - line_indent);
            }
            acc_len += line.len;
        }

        acc_len
    }

    /// Resolve `pos` to the wrapped sub-line under it.
    ///
    /// Returns the sub-line index, the byte offset that sub-line starts at within this line
    /// layout, and `pos.x` translated into that sub-line's own coordinate space.
    fn wrapped_line_at(
        &self,
        pos: Point<Pixels>,
        last_layout: &LastLayout,
    ) -> Option<(usize, usize, Pixels)> {
        let mut offset = 0;
        let mut line_top = px(0.);
        let x_offset = last_layout.alignment_offset(self.longest_width);

        for (i, line) in self.wrapped_lines.iter().enumerate() {
            let line_bottom = line_top + last_layout.line_height;
            if pos.y >= line_top && pos.y < line_bottom {
                return Some((i, offset, pos.x - x_offset - self.line_indent(i)));
            }

            offset += line.len;
            line_top = line_bottom;
        }

        None
    }

    /// Get the index for the given position (x, y) in this line layout.
    ///
    /// The `pos` is relative to the top-left corner of this line layout, start from (0, 0).
    ///
    /// Returns a local byte index in this line layout (start from 0) together with the caret
    /// affinity to use for it: `true` when the index landed on the wrap boundary of a non-final
    /// sub-line. That boundary offset is shared by the end of one visual line and the start of
    /// the next, so the affinity is what tells [`Self::position_for_index`] which of the two the
    /// caret belongs to. Without it a click past the last glyph of a wrapped line would put a
    /// visible caret on the following line.
    pub(crate) fn closest_index_for_position(
        &self,
        pos: Point<Pixels>,
        last_layout: &LastLayout,
    ) -> Option<(usize, bool)> {
        let (i, offset, x) = self.wrapped_line_at(pos, last_layout)?;
        let line = &self.wrapped_lines[i];
        let ix = line.closest_index_for_x(x);
        let line_end_affinity = i + 1 < self.wrapped_lines.len() && ix == line.len;

        Some((offset + ix, line_end_affinity))
    }

    /// How many columns the given position sits past the end of the line under it.
    ///
    /// Past the end of a line there is no glyph to hit-test against, so a position out
    /// there resolves to the line end and loses how far right it really was. The extra
    /// distance is reported here in whole spaces, letting a columnar selection keep its
    /// width over a short row. Only the final visual line of a wrapped layout has that
    /// trailing space; a continuation line ends at a wrap boundary, where the next glyph
    /// merely lives on the following row.
    ///
    /// The `pos` is relative to the top-left corner of this line layout, start from (0, 0).
    pub(crate) fn columns_past_line_end(
        &self,
        pos: Point<Pixels>,
        last_layout: &LastLayout,
    ) -> usize {
        let Some((i, _, x)) = self.wrapped_line_at(pos, last_layout) else {
            return 0;
        };

        if i + 1 < self.wrapped_lines.len() || last_layout.space_width <= px(0.) {
            return 0;
        }

        let past_end = x - self.wrapped_lines[i].width;
        if past_end <= px(0.) {
            return 0;
        }

        (past_end / last_layout.space_width).round() as usize
    }

    pub(crate) fn index_for_position(
        &self,
        pos: Point<Pixels>,
        last_layout: &LastLayout,
    ) -> Option<usize> {
        let (i, offset, x) = self.wrapped_line_at(pos, last_layout)?;

        Some(offset + self.wrapped_lines[i].index_for_x(x)?)
    }

    pub(crate) fn size(&self, line_height: Pixels) -> Size<Pixels> {
        let width = self
            .wrapped_lines
            .iter()
            .enumerate()
            .map(|(ix, line)| line.width + self.line_indent(ix))
            .max()
            .unwrap_or(self.longest_width);
        size(width, self.wrapped_lines.len() * line_height)
    }

    /// Paint only the glyph background quads of this line.
    ///
    /// gpui's [`ShapedLine::paint`] does not draw backgrounds, so every line painted with
    /// [`Self::paint`] needs this called first, with the same origin and align width.
    pub(crate) fn paint_background(
        &self,
        pos: Point<Pixels>,
        line_height: Pixels,
        text_align: TextAlign,
        align_width: Option<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) {
        // Painting a background walks every glyph and pushes a scene layer, so skip the
        // whole pass for lines that have no background color to paint.
        if !self.has_background {
            return;
        }

        for (ix, line) in self.wrapped_lines.iter().enumerate() {
            _ = line.paint_background(
                pos + point(self.line_indent(ix), ix * line_height),
                line_height,
                text_align,
                align_width,
                window,
                cx,
            );
        }
    }

    pub(crate) fn paint(
        &self,
        pos: Point<Pixels>,
        line_height: Pixels,
        text_align: TextAlign,
        align_width: Option<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) {
        for (ix, line) in self.wrapped_lines.iter().enumerate() {
            _ = line.paint(
                pos + point(self.line_indent(ix), ix * line_height),
                line_height,
                text_align,
                align_width,
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

    use gpui::{Boundary, FontFeatures, FontStyle, FontWeight, px};

    #[test]
    fn measured_wrap_keeps_cjk_latin_boundary_stable_during_edits() {
        let measure = |text: &str| {
            px(text
                .chars()
                .map(|c| if c.is_ascii() { 1. } else { 2. })
                .sum())
        };
        let mut wrapper = TextWrapper::new(gpui::font("Arial"), px(14.), Some(px(6.)));
        let mut previous = Rope::new();
        for value in ["abcd的", "abcd的s", "abcd的ss", "abcd的s", "abcd的"] {
            let text = Rope::from(value);
            let start = previous.len().min(text.len());
            let inserted = Rope::from(text.slice(start..).to_string());
            wrapper._update(
                &text,
                &(start..previous.len()),
                &inserted,
                0,
                &mut |line, width, _| {
                    measured_wrap_boundaries(line, width, WrappingIndent::None, measure)
                },
            );
            let expected = if value.ends_with('s') {
                vec![0..7, 7..value.len()]
            } else {
                vec![0..7]
            };
            assert_eq!(wrapper.line(0).unwrap().wrapped_lines.as_slice(), expected);
            let cursor = wrapper.offset_to_display_point(value.len());
            assert_eq!(cursor.row, usize::from(value.ends_with('s')));
            previous = text;
        }
    }

    #[test]
    fn measured_wrap_preserves_words_graphemes_and_indentation() {
        let wrap = |text: &str, width, indent| {
            measured_wrap_boundaries(text, px(width), indent, |s| {
                px(s.graphemes(true).count() as f32)
            })
            .into_iter()
            .map(|b| b.ix)
            .collect::<Vec<_>>()
        };
        assert_eq!(wrap("hello world", 8., WrappingIndent::None), vec![6]);
        assert_eq!(wrap("a👩‍💻b", 2., WrappingIndent::None), vec!["a👩‍💻".len()]);
        assert_eq!(wrap("  abcdefgh", 5., WrappingIndent::Same), vec![5, 8]);
        assert_eq!(wrap("  abcdefgh", 5., WrappingIndent::None), vec![2, 7]);
        assert!(wrap("", 0., WrappingIndent::None).is_empty());
        assert_eq!(wrap("abc", 0., WrappingIndent::None), vec![1, 2]);
        // Closing punctuation stays with the preceding Chinese character.
        assert_eq!(wrap("你好，世界", 2., WrappingIndent::None), vec![3, 9]);
    }

    #[test]
    fn test_update() {
        let font = gpui::Font {
            family: "Arial".into(),
            weight: FontWeight::default(),
            style: FontStyle::Normal,
            features: FontFeatures::default(),
            fallbacks: None,
        };

        let mut wrapper = TextWrapper::new(font, px(14.), None);
        let mut text = Rope::from(
            "Hello, 世界!\r\nThis is second line.\nThis is third line.\n这里是第 4 行。",
        );

        fn fake_wrap_line(_line: &str, _wrap_width: Pixels, _: usize) -> Vec<Boundary> {
            vec![]
        }

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
                // +1 \n
                offset += line.len() + 1;
            }
            assert_eq!(actual_lines, expected_lines);
        }

        wrapper._update(&text, &(0..text.len()), &text, 0, &mut fake_wrap_line);
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

        // Add a new text to end
        let range = text.len()..text.len();
        let new_text = "New text";
        text.replace(range.clone(), new_text);
        wrapper._update(&text, &range, &Rope::from(new_text), 0, &mut fake_wrap_line);
        assert_eq!(
            text.to_string(),
            "Hello, 世界!\r\nThis is second line.\nThis is third line.\n这里是第 4 行。New text"
        );
        assert_eq!(wrapper.lines_count(), 4);
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

        // Replace first line `Hello` to `AAA`
        let range = 0..5;
        let new_text = "AAA";
        text.replace(range.clone(), new_text);
        wrapper._update(&text, &range, &Rope::from(new_text), 0, &mut fake_wrap_line);
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
        wrapper._update(&text, &range, &Rope::from(""), 0, &mut fake_wrap_line);
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

        // Replace the first 2 lines to "This is a new line."
        let range = text.line_start_offset(0)..text.line_end_offset(1) + 1;
        let new_text = "This is a new line.\nThis is new line 2.\n";
        text.replace(range.clone(), new_text);
        wrapper._update(&text, &range, &Rope::from(new_text), 0, &mut fake_wrap_line);
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
        wrapper._update(&text, &range, &Rope::from(new_text), 0, &mut fake_wrap_line);
        assert_eq!(
            text.to_string(),
            "This is a new line.\nThis is new line 2.\n这里是第 4 行。New text\nThis is a new line at the end."
        );
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
        wrapper._update(&text, &range, &Rope::from(new_text), 0, &mut fake_wrap_line);
        assert_eq!(
            text.to_string(),
            "This is a new line at the beginning.\nThis is a new line.\nThis is new line 2.\n这里是第 4 行。New text\nThis is a new line at the end."
        );
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

        // Remove all to at least one line in `lines`.
        let range = 0..text.len();
        let new_text = "";
        text.replace(range.clone(), new_text);
        wrapper._update(&text, &range, &Rope::from(new_text), 0, &mut fake_wrap_line);
        assert_eq!(text.to_string(), "");
        assert_eq!(wrapper.lines_count(), 1);
        assert_eq!(wrapper.line(0).unwrap().wrapped_lines.as_slice(), [0..0]);

        // Test update_all
        let range = 0..text.len();
        let new_text = "This is a full text.\nThis is a second line.";
        text.replace(range.clone(), new_text);
        wrapper._update(&text, &range, &text, 0, &mut fake_wrap_line);
        assert_eq!(
            text.to_string(),
            "This is a full text.\nThis is a second line."
        );
        assert_eq!(wrapper.lines_count(), 2);
    }

    fn test_font() -> gpui::Font {
        gpui::Font {
            family: "Arial".into(),
            weight: FontWeight::default(),
            style: FontStyle::Normal,
            features: FontFeatures::default(),
            fallbacks: None,
        }
    }

    /// The longest-row summary stays exact when the previously-longest line is shrunk.
    #[test]
    fn test_longest_row_after_shrink() {
        let mut wrapper = TextWrapper::new(test_font(), px(14.), None);
        let mut text = Rope::from("aa\nthis is the longest line\nbb");
        wrapper._update(&text, &(0..text.len()), &text, 0, &mut |_, _, _| vec![]);
        assert_eq!(wrapper.longest_row(), 1);

        // Shrink line 1 so line 2-equivalent isn't longest.
        // Make line 0 the longest now.
        let start = text.line_start_offset(0);
        let end = text.line_end_offset(0);
        let range = start..end;
        let new_text = "a very very long first line now";
        text.replace(range.clone(), new_text);
        wrapper._update(&text, &range, &Rope::from(new_text), 0, &mut |_, _, _| vec![]);
        assert_eq!(wrapper.longest_row(), 0);
    }

    /// Editing the last line and deleting everything must keep the tree consistent.
    #[test]
    fn test_edit_last_line_and_full_delete() {
        let mut wrapper = TextWrapper::new(test_font(), px(14.), None);
        let mut text = Rope::from("one\ntwo\nthree");
        wrapper._update(&text, &(0..text.len()), &text, 0, &mut |_, _, _| vec![]);
        assert_eq!(wrapper.lines_count(), 3);

        // Replace the last line only.
        let start = text.line_start_offset(2);
        let range = start..text.len();
        let new_text = "THREE EDITED";
        text.replace(range.clone(), new_text);
        wrapper._update(&text, &range, &Rope::from(new_text), 0, &mut |_, _, _| vec![]);
        assert_eq!(wrapper.lines_count(), 3);
        assert_eq!(wrapper.line(2).unwrap().len(), "THREE EDITED".len());

        // Delete everything.
        let range = 0..text.len();
        text.replace(range.clone(), "");
        wrapper._update(&text, &range, &Rope::from(""), 0, &mut |_, _, _| vec![]);
        assert_eq!(wrapper.lines_count(), 1);
        assert_eq!(wrapper.len(), 1);
        assert_eq!(wrapper.line(0).unwrap().wrapped_lines.as_slice(), [0..0]);
    }

    #[test]
    fn test_wrap_row_buffer_line_boundaries() {
        let mut wrapper = TextWrapper::new(test_font(), px(14.), None);
        wrapper.text = Rope::from("aa\nbbbb\nc");
        wrapper.lines = LineTree::from_vec(vec![
            LineItem {
                len: 2,
                indent: 0,
                wrapped_lines: gpui::smallvec::smallvec![0..2],
            },
            LineItem {
                len: 4,
                indent: 0,
                wrapped_lines: gpui::smallvec::smallvec![0..2, 2..4],
            },
            LineItem {
                len: 1,
                indent: 0,
                wrapped_lines: gpui::smallvec::smallvec![0..1],
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
        let mut fake_wrap_line = |line: &str, _wrap_width: Pixels, _: usize| {
            if line.len() > 2 {
                vec![Boundary {
                    ix: 2,
                    next_indent: 0,
                }]
            } else {
                vec![]
            }
        };

        wrapper._update(&text, &(0..text.len()), &text, 0, &mut fake_wrap_line);
        assert_eq!(wrapper.buffer_line_to_wrap_row_range(0), 0..1);
        assert_eq!(wrapper.buffer_line_to_wrap_row_range(1), 1..3);
        assert_eq!(wrapper.buffer_line_to_wrap_row_range(2), 3..4);

        let range = text.line_start_offset(1)..text.line_end_offset(1);
        let new_text = "dd\neeee";
        text.replace(range.clone(), new_text);
        wrapper._update(&text, &range, &Rope::from(new_text), 0, &mut fake_wrap_line);

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
        let mut line_layout = LineLayout::new();

        let line1 = ShapedLine::default().with_len(100);
        let line2 = ShapedLine::default().with_len(50);
        let wrapped_lines = gpui::smallvec::smallvec![line1, line2];
        line_layout.set_wrapped_lines(wrapped_lines);
        assert_eq!(line_layout.len(), 150);
        assert_eq!(line_layout.wrapped_lines.len(), 2);
    }

    /// A layout context whose only load-bearing field is the line height.
    fn test_last_layout(line_height: Pixels) -> LastLayout {
        LastLayout {
            visible_range: 0..1,
            visible_buffer_lines: vec![0],
            visible_line_byte_offsets: vec![0],
            visible_top: px(0.),
            visible_range_offset: 0..0,
            lines: Rc::new(vec![]),
            line_height,
            wrap_width: None,
            line_number_width: px(0.),
            space_width: px(0.),
            cursor_bounds: None,
            text_align: TextAlign::Left,
            content_width: px(0.),
        }
    }

    #[test]
    fn test_position_for_index_prefers_first_leading_empty_visual_line() {
        let mut line_layout = LineLayout::new();
        line_layout.set_wrapped_lines(gpui::smallvec::smallvec![
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
        // One buffer line wrapped into two visual rows, splitting at byte 10.
        let mut line_layout = LineLayout::new();
        line_layout.set_wrapped_lines(gpui::smallvec::smallvec![
            ShapedLine::default().with_len(10),
            ShapedLine::default().with_len(5),
        ]);
        let last_layout = test_last_layout(px(20.));

        // Clicking past the last glyph of the first row resolves to the wrap boundary, which is
        // also the first offset of the second row -- hence the affinity.
        let (ix, line_end_affinity) = line_layout
            .closest_index_for_position(point(px(999.), px(5.)), &last_layout)
            .unwrap();
        assert_eq!(ix, 10);
        assert!(line_end_affinity);

        // Carrying that affinity is what keeps the caret on the row that was clicked; dropping it
        // is the bug -- the caret shows up one row below the pointer.
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

        // The final row owns the end of the line outright, so there is nothing to disambiguate.
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
                wrapped_lines: gpui::smallvec::smallvec![0..10],
            },
            LineItem {
                len: Rope::from("this one wraps").len(),
                indent: 0,
                wrapped_lines: gpui::smallvec::smallvec![0..9, 9..14],
            },
        ]);

        // Offset 20 is the wrap boundary of the second buffer line: 11 (line start) + 9.
        // It is the last offset of wrap row 1 and the first of wrap row 2 at the same time, so
        // only the affinity can say which row a vertical move should step away from.
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

        // An offset that is not on a boundary is unaffected either way.
        for line_end_affinity in [false, true] {
            assert_eq!(
                wrapper.offset_to_display_point_with_affinity(23, line_end_affinity),
                WrapDisplayPoint::new(2, 1, 3)
            );
        }
    }

    #[test]
    fn test_offset_to_display_point() {
        let font = gpui::Font {
            family: "Arial".into(),
            weight: FontWeight::default(),
            style: FontStyle::Normal,
            features: FontFeatures::default(),
            fallbacks: None,
        };

        let mut wrapper = TextWrapper::new(font, px(14.), None);
        wrapper.text = Rope::from(
            "Hello, 世界!\r\nThis is second line.\nThis is third line.\n这里是第 4 行。",
        );
        wrapper.lines = LineTree::from_vec(vec![
            // range: 0..15
            LineItem {
                len: Rope::from("Hello, 世界!\r").len(),
                indent: 0,
                wrapped_lines: gpui::smallvec::smallvec![0..15],
            },
            // range: 16..36
            LineItem {
                len: Rope::from("This is second line.\n").len(),
                indent: 0,
                wrapped_lines: gpui::smallvec::smallvec![0..10, 10..20],
            },
            // range: 37..56
            LineItem {
                len: Rope::from("This is third line.\n").len(),
                indent: 0,
                wrapped_lines: gpui::smallvec::smallvec![0..9, 9..15, 15..20],
            },
            // range: 57..79
            LineItem {
                len: Rope::from("这里是第 4 行。").len(),
                indent: 0,
                wrapped_lines: gpui::smallvec::smallvec![0..22],
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
        wrapper.wrapping_indent = WrappingIndent::Same;
        let text = Rope::from("  abcdefghijklmnopqrstuv");
        let mut fake_wrap_line = |line: &str, _wrap_width: Pixels, _: usize| {
            if line.starts_with(' ') {
                vec![Boundary {
                    ix: 5,
                    next_indent: 2,
                }]
            } else {
                let mut boundaries = vec![];
                let mut i = 8;
                while i < line.len() {
                    boundaries.push(Boundary {
                        ix: i,
                        next_indent: 0,
                    });
                    i += 8;
                }
                boundaries
            }
        };

        wrapper._update(&text, &(0..text.len()), &text, 0, &mut fake_wrap_line);

        let line = wrapper.line(0).unwrap();
        assert_eq!(line.indent, 2);
        assert_eq!(line.wrapped_lines.as_slice(), [0..5, 5..24]);
    }

    #[test]
    fn test_wrapping_indent_none_continuation_lines_wrapped_at_full_width() {
        let mut wrapper = TextWrapper::new(test_font(), px(14.0), Some(px(10.)));
        wrapper.wrapping_indent = WrappingIndent::None;
        let text = Rope::from("  abcdefghijklmnopqrstuv");
        let mut fake_wrap_line = |line: &str, _wrap_width: Pixels, _: usize| {
            if line.starts_with(' ') {
                vec![Boundary {
                    ix: 5,
                    next_indent: 2,
                }]
            } else {
                let mut boundaries = vec![];
                let mut i = 8;
                while i < line.len() {
                    boundaries.push(Boundary {
                        ix: i,
                        next_indent: 0,
                    });
                    i += 8;
                }
                boundaries
            }
        };

        wrapper._update(&text, &(0..text.len()), &text, 0, &mut fake_wrap_line);

        let line = wrapper.line(0).unwrap();
        assert_eq!(line.indent, 0);
        assert_eq!(line.wrapped_lines.as_slice(), [0..5, 5..13, 13..21, 21..24]);
    }

    #[test]
    fn test_wrap_indent_offsets_continuation_lines() {
        let mut line_layout = LineLayout::new();
        line_layout.set_wrapped_lines(gpui::smallvec::smallvec![
            ShapedLine::default().with_len(5),
            ShapedLine::default().with_len(10),
        ]);

        line_layout = line_layout.wrap_indent(px(20.0));

        let last_layout = LastLayout {
            visible_range: 0..1,
            visible_buffer_lines: vec![0],
            visible_line_byte_offsets: vec![0],
            visible_top: px(0.),
            visible_range_offset: 0..0,
            lines: Rc::new(vec![]),
            line_height: px(20.0),
            wrap_width: Some(px(10.)),
            line_number_width: px(0.),
            space_width: px(0.),
            cursor_bounds: None,
            text_align: TextAlign::Left,
            content_width: px(0.),
        };

        assert_eq!(
            line_layout.position_for_index(0, &last_layout, false),
            Some(point(px(0.), px(0.))),
        );

        assert_eq!(
            line_layout.position_for_index(6, &last_layout, false),
            Some(point(px(20.), px(20.))),
        )
    }

    #[test]
    fn test_ascii_fast_fit_bypasses_wrap_line_for_short_printable_ascii() {
        let mut wrapper = TextWrapper::new(test_font(), px(14.), Some(px(100.)));
        let text = Rope::from("short ascii\nlonger ascii line\n日本語\n");
        let mut wrapped_calls = Vec::new();
        wrapper._update(
            &text,
            &(0..text.len()),
            &text,
            12,
            &mut |line, _, _| {
                wrapped_calls.push(line.to_string());
                vec![]
            },
        );
        assert_eq!(wrapper.lines_count(), 4);
        assert_eq!(wrapped_calls, vec!["longer ascii line", "日本語"]);
    }
}
