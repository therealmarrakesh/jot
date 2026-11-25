//! Document layout. An index records where every line of the document wraps, so rows can be
//! counted and found without laying the document out; each frame then shapes only the lines on
//! screen, and positions, hit-tests and paints them.
//!
//! Wrapping is our own greedy fill from cached character advances, at the line-break
//! opportunities DirectWrite reports: wrapping a whole document with DirectWrite itself is an
//! order of magnitude slower. Text whose characters shape together is left to DirectWrite.

use std::borrow::Cow;
use std::ops::Range;

use ropey::LineType;

use crate::geom::{Point, Rect, Rgba, Size, point};
use crate::gfx::{
    AdvanceTable, Canvas, FontFace, ShapedRow, TextStyle, TextSystem, has_simple_advances,
};
use crate::text::{LineChange, Rope, RopeExt as _};
use crate::types::SharedStr;

/// Leading spaces and tabs, up to this many, indent a line's wrapped rows.
const MAX_INDENT_CHARS: usize = 32;
/// Unwrapped lines longer than this are measured from advances and shaped only near the viewport.
const HUGE_LINE_BYTES: usize = 4096;
/// How far past the viewport such a line is shaped on each side.
const WINDOW_PAD: f32 = 256.0;
/// Lines with more rows than this shape only the rows near the viewport.
const MANY_ROWS: usize = 64;
/// Lines per index chunk.
const CHUNK_LINES: usize = 256;

/// Where `line` breaks into rows `width` wide, and how far its wrapped rows indent (0 when it
/// doesn't wrap).
fn wrap(face: &FontFace, line: &str, width: f32) -> (Box<[usize]>, f32) {
    if line.is_empty() || !width.is_finite() || width <= 0.0 {
        return (Box::default(), 0.0);
    }
    let mut breaks = Vec::new();
    let indent = face.with_advances(|adv| {
        let indent = hanging_indent(adv, line, width * 0.5);
        if has_simple_advances(line) {
            fill(
                adv,
                line,
                width,
                indent,
                &face.break_opportunities(line),
                &mut breaks,
            );
        } else {
            face.wrap_breaks(line, width, (width - indent).max(1.0), &mut breaks);
        }
        indent
    });
    let indent = if breaks.is_empty() { 0.0 } else { indent };
    (breaks.into_boxed_slice(), indent)
}

/// The width of `line`'s leading spaces and tabs, or 0 if that's `max` or more.
fn hanging_indent(adv: &mut AdvanceTable, line: &str, max: f32) -> f32 {
    let mut width = 0.0;
    for ch in line
        .chars()
        .take(MAX_INDENT_CHARS)
        .take_while(|&c| c == ' ' || c == '\t')
    {
        width += adv.advance(ch, width);
    }
    if width >= max { 0.0 } else { width }
}

/// Greedy wrapping: fills each row up to `width`, breaking at the last opportunity that fits
/// (or mid-word when none does). Wrapped rows start `indent` in.
fn fill(
    adv: &mut AdvanceTable,
    line: &str,
    width: f32,
    indent: f32,
    opportunities: &[usize],
    breaks: &mut Vec<usize>,
) {
    let (mut row_start, mut row_width, mut last_break, mut next) = (0, 0.0f32, 0, 0);
    for (ix, ch) in line.char_indices() {
        let end = ix + ch.len_utf8();
        while next < opportunities.len() && opportunities[next] <= ix {
            next += 1;
        }
        let w = adv.advance(ch, row_width);
        if row_width + w > width && ix > row_start {
            let split = if last_break > row_start {
                last_break
            } else {
                ix
            };
            breaks.push(split);
            row_start = split;
            row_width = indent;
            for c in line[split..end].chars() {
                row_width += adv.advance(c, row_width);
            }
            last_break = split;
        } else {
            row_width += w;
        }
        if next < opportunities.len() && opportunities[next] == end {
            last_break = end;
            next += 1;
        }
    }
}

/// The bytes of `line` between `min_x` and `max_x`, where they start, and the whole line's
/// width: `(start, end, start_x, width)`.
fn window(adv: &mut AdvanceTable, line: &str, min_x: f32, max_x: f32) -> (usize, usize, f32, f32) {
    let (mut x, mut start, mut end) = (0.0f32, None, None);
    for (ix, ch) in line.char_indices() {
        let a = adv.advance(ch, x);
        if start.is_none() && x + a >= min_x {
            start = Some((ix, x));
        }
        x += a;
        if start.is_some() && end.is_none() && x >= max_x {
            end = Some(ix + ch.len_utf8());
        }
    }
    match start {
        Some((start, start_x)) => (start, end.unwrap_or(line.len()), start_x, x),
        None => (line.len(), line.len(), x, x),
    }
}

/// `text`'s width on one unwrapped line.
fn unwrapped_width(face: &FontFace, text: &str) -> f32 {
    if text.is_empty() {
        0.0
    } else if text.len() > HUGE_LINE_BYTES && has_simple_advances(text) {
        face.with_advances(|adv| window(adv, text, 0.0, 0.0).3)
    } else {
        face.measure(text)
    }
}

/// One document line: its length, where it breaks into rows, and how far wrapped rows indent.
struct Line {
    len: usize,
    breaks: Box<[usize]>,
    indent: f32,
}

impl Line {
    fn rows(&self) -> usize {
        self.breaks.len() + 1
    }

    /// The bytes of row `row` within the line.
    fn row(&self, row: usize) -> Option<Range<usize>> {
        let start = if row == 0 {
            0
        } else {
            *self.breaks.get(row - 1)?
        };
        Some(start..self.breaks.get(row).copied().unwrap_or(self.len))
    }
}

/// Consecutive lines, with their total rows and their (first) longest line.
struct Chunk {
    lines: Vec<Line>,
    rows: usize,
    /// The longest line's place in the chunk, and its length.
    longest: (usize, usize),
}

impl Chunk {
    fn new(lines: Vec<Line>) -> Self {
        let rows = lines.iter().map(Line::rows).sum();
        let longest = lines.iter().enumerate().fold((0, 0), |best, (i, line)| {
            if line.len > best.1 {
                (i, line.len)
            } else {
                best
            }
        });
        Self {
            lines,
            rows,
            longest,
        }
    }
}

/// A visual position: the visual row, the row within its line, and the byte column in that row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DisplayPos {
    pub(crate) row: usize,
    pub(crate) sub_row: usize,
    pub(crate) col: usize,
}

/// Where the document sits on screen: the viewport and how far its content is scrolled
/// (negative offsets move the content left and up).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Viewport {
    pub(crate) size: Size,
    pub(crate) scroll_x: f32,
    pub(crate) scroll_y: f64,
}

/// Every line of the document, wrapped (when there's a wrap width), in chunks with running
/// totals so lines and visual rows can be found quickly in documents of any size.
pub(crate) struct LineIndex {
    style: TextStyle,
    wrap_width: Option<f32>,
    chunks: Vec<Chunk>,
    /// The first line and first row of each chunk.
    starts: Vec<(usize, usize)>,
    lines: usize,
    rows: usize,
    longest: usize,
    synced: bool,
    /// Unwrapped width of the longest line, until the text or the layout changes.
    longest_width: Option<f32>,
}

impl LineIndex {
    pub(crate) fn new(style: TextStyle, wrap_width: Option<f32>) -> Self {
        Self {
            style,
            wrap_width,
            chunks: Vec::new(),
            starts: Vec::new(),
            lines: 0,
            rows: 0,
            longest: 0,
            synced: false,
            longest_width: None,
        }
    }

    pub(crate) fn line_count(&self) -> usize {
        self.lines
    }

    pub(crate) fn visual_row_count(&self) -> usize {
        self.rows
    }

    pub(crate) fn style(&self) -> &TextStyle {
        &self.style
    }

    pub(crate) fn wrap_width(&self) -> Option<f32> {
        self.wrap_width
    }

    /// Line `line` and the visual row it starts on (the row count past the last line).
    fn line(&self, line: usize) -> (usize, Option<&Line>) {
        let (c, i) = self.chunk_of(line);
        let Some(chunk) = self.chunks.get(c) else {
            return (self.rows, None);
        };
        let before: usize = chunk.lines[..i].iter().map(Line::rows).sum();
        (self.starts[c].1 + before, Some(&chunk.lines[i]))
    }

    /// The line holding visual row `row`: its index, the row it starts on, and the line.
    fn line_at_row(&self, row: usize) -> Option<(usize, usize, &Line)> {
        if row >= self.rows {
            return None;
        }
        let c = self.starts.partition_point(|&(_, r)| r <= row) - 1;
        let (mut index, mut first) = self.starts[c];
        for line in &self.chunks[c].lines {
            if row < first + line.rows() {
                return Some((index, first, line));
            }
            index += 1;
            first += line.rows();
        }
        None
    }

    /// The chunk holding line `line` and the line's place in it (the chunk count past the end).
    fn chunk_of(&self, line: usize) -> (usize, usize) {
        if line >= self.lines {
            return (self.chunks.len(), 0);
        }
        let c = self.starts.partition_point(|&(l, _)| l <= line) - 1;
        (c, line - self.starts[c].0)
    }

    /// Replaces lines `old` (clamped to the index) with `new`.
    fn splice(&mut self, old: Range<usize>, new: impl IntoIterator<Item = Line>) {
        let start = old.start.min(self.lines);
        let end = old.end.clamp(start, self.lines);
        let (first, last) = (self.chunk_of(start).0, self.chunk_of(end).0);
        let base = self.starts.get(first).map_or(self.lines, |&(line, _)| line);
        let replaced = first..(last + 1).min(self.chunks.len());
        let taken = self.chunks[replaced.clone()].iter_mut();
        let mut lines: Vec<Line> = taken
            .flat_map(|chunk| std::mem::take(&mut chunk.lines))
            .collect();
        lines.splice(start - base..end - base, new);
        let mut cut = Vec::with_capacity(lines.len().div_ceil(CHUNK_LINES));
        let mut lines = lines.into_iter();
        while lines.len() > 0 {
            cut.push(Chunk::new(lines.by_ref().take(CHUNK_LINES).collect()));
        }
        self.chunks.splice(replaced, cut);
        // Merge small neighbours around the edit so repeated edits don't fragment the index.
        let mut i = first.saturating_sub(1);
        while i + 1 < self.chunks.len() && i <= first + 2 {
            if self.chunks[i].lines.len() + self.chunks[i + 1].lines.len() <= CHUNK_LINES {
                let next = self.chunks.remove(i + 1);
                let mut merged = std::mem::take(&mut self.chunks[i].lines);
                merged.extend(next.lines);
                self.chunks[i] = Chunk::new(merged);
            } else {
                i += 1;
            }
        }
        self.starts.clear();
        let (mut lines, mut rows, mut longest) = (0, 0, (0, 0));
        for chunk in &self.chunks {
            self.starts.push((lines, rows));
            if chunk.longest.1 > longest.1 {
                longest = (lines + chunk.longest.0, chunk.longest.1);
            }
            lines += chunk.lines.len();
            rows += chunk.rows;
        }
        (self.lines, self.rows, self.longest) = (lines, rows, longest.0);
    }

    /// Wraps lines `new` of `text` in place of index lines `old`.
    fn rewrap(&mut self, text: &Rope, old: Range<usize>, new: Range<usize>, ts: &TextSystem) {
        let total = text.lines_len();
        let start = new.start.min(total);
        let new = start..new.end.clamp(start, total);
        let (face, wrap_width) = (ts.face(&self.style), self.wrap_width);
        // Printable ASCII lines this short fit the width without measuring.
        let fits = wrap_width.map_or(0, |width| {
            (width / face.with_advances(|adv| adv.max_ascii)) as usize
        });
        let mut source = text.lines_at(new.start, LineType::LF);
        let lines = new.map(|_| {
            let line = match source.next() {
                Some(raw) if raw.len() > 0 && raw.byte(raw.len() - 1) == b'\n' => {
                    raw.slice(..raw.len() - 1)
                }
                Some(raw) => raw,
                None => text.slice(0..0),
            };
            let len = line.len();
            let printable = |c: &str| c.bytes().all(|b| (0x20..=0x7e).contains(&b));
            let short = fits > 0 && len <= fits && line.chunks().all(printable);
            let (breaks, indent) = match wrap_width {
                Some(width) if len > 0 && !short => wrap(&face, &Cow::from(line), width),
                _ => (Box::default(), 0.0),
            };
            Line {
                len,
                breaks,
                indent,
            }
        });
        self.splice(old, lines);
    }

    fn rebuild(&mut self, text: &Rope, ts: &TextSystem) {
        self.rewrap(text, 0..self.lines, 0..text.lines_len(), ts);
    }

    /// Brings the index up to date with `style` and `wrap_width`, wrapping the whole text the
    /// first time or when either changes what wraps.
    pub(crate) fn sync_layout(
        &mut self,
        text: &Rope,
        style: &TextStyle,
        wrap_width: Option<f32>,
        ts: &TextSystem,
    ) {
        let style_changed = self.style != *style;
        let width_changed = self.wrap_width != wrap_width;
        if style_changed || width_changed || !self.synced {
            self.longest_width = None;
        }
        if style_changed {
            self.style = style.clone();
        }
        self.wrap_width = wrap_width;
        if !self.synced || width_changed || (style_changed && wrap_width.is_some()) {
            self.synced = true;
            self.rebuild(text, ts);
        }
    }

    /// Re-wraps the lines an edit changed.
    pub(crate) fn apply_line_change(&mut self, text: &Rope, change: &LineChange, ts: &TextSystem) {
        self.longest_width = None;
        if !self.synced {
            return;
        }
        match change {
            LineChange::Full => self.rebuild(text, ts),
            LineChange::Splice {
                old_lines,
                new_lines,
            } => {
                self.rewrap(text, old_lines.clone(), new_lines.clone(), ts);
            }
        }
    }

    /// The unwrapped width of the longest line, measured once per change to the text or layout.
    pub(crate) fn longest_line_width(&mut self, text: &Rope, ts: &TextSystem) -> f32 {
        if let Some(width) = self.longest_width {
            return width;
        }
        let line: Cow<str> = text.slice_line(self.longest).into();
        let width = unwrapped_width(&ts.face(&self.style), &line);
        self.longest_width = Some(width);
        width
    }

    /// How far into its unwrapped line `offset` is.
    pub(crate) fn unwrapped_x(&self, text: &Rope, offset: usize, ts: &TextSystem) -> f32 {
        let pt = text.offset_to_point(offset);
        let line = text.slice_line(pt.row);
        let before: Cow<str> = line.slice(..pt.column.min(line.len())).into();
        unwrapped_width(&ts.face(&self.style), &before)
    }

    pub(crate) fn visual_row_to_line_and_sub_row(&self, row: usize) -> (usize, usize) {
        self.line_at_row(row)
            .map_or((self.lines.saturating_sub(1), 0), |(line, first, _)| {
                (line, row - first)
            })
    }

    /// The bytes of row `sub_row` of line `line`, and whether the line wraps after it.
    pub(crate) fn visual_row_range(
        &self,
        line: usize,
        sub_row: usize,
    ) -> Option<(Range<usize>, bool)> {
        let line = self.line(line).1?;
        Some((line.row(sub_row)?, sub_row + 1 < line.rows()))
    }

    /// Where `offset` is drawn; at a wrap, `end_affinity` keeps it on the end of the earlier row.
    pub(crate) fn offset_to_display(
        &self,
        text: &Rope,
        offset: usize,
        end_affinity: bool,
    ) -> DisplayPos {
        let line_ix = text.offset_to_point(offset).row;
        let (first_row, Some(line)) = self.line(line_ix) else {
            return DisplayPos {
                row: self.rows,
                sub_row: 0,
                col: 0,
            };
        };
        let local = offset.saturating_sub(text.line_start_offset(line_ix));
        for sub_row in 0..line.rows() {
            let range = line.row(sub_row).expect("row of the line");
            if range.contains(&local) || (end_affinity && local == range.end) {
                return DisplayPos {
                    row: first_row + sub_row,
                    sub_row,
                    col: local - range.start,
                };
            }
        }
        let sub_row = line.rows() - 1;
        let last = line.row(sub_row).expect("row of the line");
        DisplayPos {
            row: first_row + sub_row,
            sub_row,
            col: last.len(),
        }
    }

    pub(crate) fn display_to_offset(&self, text: &Rope, pos: DisplayPos) -> usize {
        let Some((line_ix, first_row, line)) = self.line_at_row(pos.row) else {
            return text.len();
        };
        let start = text.line_start_offset(line_ix);
        match line.row(pos.row - first_row) {
            Some(range) => start + (range.start + pos.col).min(range.end),
            None => start + line.len,
        }
    }

    /// The lines on screen (plus one past, for partly visible rows), the first one's top
    /// relative to the viewport, and the visual rows worth shaping.
    fn visible(
        &self,
        scroll_y: f64,
        line_height: f32,
        height: f32,
    ) -> (Range<usize>, f32, Range<usize>) {
        if self.rows == 0 || self.lines == 0 {
            return (0..0, 0.0, 0..0);
        }
        let lh = f64::from(line_height);
        let top = (-scroll_y).max(0.0);
        let first_row = ((top / lh).floor() as usize).min(self.rows - 1);
        let last_row = (((top + f64::from(height)) / lh).ceil() as usize).min(self.rows - 1);
        let shape_rows = first_row.saturating_sub(2)..(last_row + 3).min(self.rows);
        let first_line = self.visual_row_to_line_and_sub_row(first_row).0;
        let last_line = self.visual_row_to_line_and_sub_row(last_row).0;
        let top_rel = (self.line(first_line).0 as f64 * lh + scroll_y) as f32;
        (
            first_line..(last_line + 2).min(self.lines),
            top_rel,
            shape_rows,
        )
    }

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
        let face = ts.face(&self.style);
        let (visible, mut top, shape_rows) =
            self.visible(view.scroll_y, line_height, view.size.height);
        let first_line = visible.start;
        let start = text.line_start_offset(visible.start);
        let end = text.line_end_offset(visible.end.saturating_sub(1));
        let mut lines = Vec::with_capacity(visible.len());
        let mut windowed = false;
        for line_ix in visible {
            let slice = text.slice_line(line_ix);
            let offset = text.line_start_offset(line_ix);
            let (first_row, line) = self.line(line_ix);
            let line = line.expect("visible line is indexed");
            let has_caret = (offset..=offset + slice.len()).contains(&caret);
            let huge =
                self.wrap_width.is_none() && line.rows() == 1 && slice.len() > HUGE_LINE_BYTES;
            let whole = huge
                .then(|| Cow::<str>::from(slice))
                .filter(|s| has_simple_advances(s));
            let shown = if let Some(whole) = whole {
                // Shape only what's near the viewport, stretched to include the caret.
                let min_x = (-view.scroll_x - WINDOW_PAD).max(0.0);
                let max_x = (-view.scroll_x + view.size.width + WINDOW_PAD).max(min_x);
                let (mut from, mut to, x, width) =
                    face.with_advances(|adv| window(adv, &whole, min_x, max_x));
                if has_caret {
                    let caret = caret - offset;
                    if caret < from {
                        from = caret;
                    } else if caret > to {
                        to = caret;
                    }
                }
                windowed = true;
                let shaped = ts.shape_row(whole[from..to].to_string().into(), &self.style);
                let row = Row::Window {
                    text: shaped,
                    start: from,
                    x,
                    len: whole.len(),
                    width,
                };
                ShownLine::one_row(offset, top, row)
            } else {
                // A line of many rows keeps only the stretch near the viewport and the caret.
                let total = line.rows();
                let (mut lo, mut hi) = (0, total);
                if total > MANY_ROWS {
                    lo = shape_rows.start.saturating_sub(first_row).min(total);
                    hi = shape_rows
                        .end
                        .saturating_sub(first_row)
                        .min(total)
                        .max(lo + 1)
                        .min(total);
                    if has_caret {
                        let sub_row = self.offset_to_display(text, caret, end_affinity).sub_row;
                        lo = lo.min(sub_row);
                        hi = hi.max((sub_row + 1).min(total));
                    }
                }
                let rows = (lo..hi)
                    .map(|sub_row| {
                        let bytes = line.row(sub_row).expect("row of the line");
                        let on_screen = shape_rows.contains(&(first_row + sub_row));
                        if on_screen || (offset + bytes.start..=offset + bytes.end).contains(&caret)
                        {
                            Row::Shaped(
                                ts.shape_row(slice.slice(bytes).to_string().into(), &self.style),
                            )
                        } else {
                            Row::Unshaped(bytes.len())
                        }
                    })
                    .collect();
                let skipped = line.row(lo).map_or(0, |r| r.start);
                ShownLine {
                    offset,
                    top,
                    first_row: lo,
                    skipped,
                    total_rows: total,
                    indent: line.indent,
                    rows,
                }
            };
            top += shown.total_rows as f32 * line_height;
            lines.push(shown);
        }
        let layout = FrameLayout {
            first_line,
            lines,
            visible: start..end,
            line_height,
            space_width: face.space_width,
        };
        (layout, windowed)
    }
}

/// One visual row of a shown line.
enum Row {
    /// Shaped text for the whole row.
    Shaped(ShapedRow),
    /// A long unwrapped line shaped only near the viewport: `text` holds the line's bytes from
    /// `start` and is drawn from `x`; the whole line is `len` bytes and `width` wide.
    Window {
        text: ShapedRow,
        start: usize,
        x: f32,
        len: usize,
        width: f32,
    },
    /// A row away from the viewport in a line of many rows: only its length is kept.
    Unshaped(usize),
}

impl Row {
    fn len(&self) -> usize {
        match self {
            Self::Shaped(text) => text.len,
            Self::Window { len, .. } | Self::Unshaped(len) => *len,
        }
    }

    fn width(&self) -> f32 {
        match self {
            Self::Shaped(text) => text.width,
            Self::Window { width, .. } => *width,
            Self::Unshaped(_) => 0.0,
        }
    }

    fn x_for_index(&self, ix: usize) -> f32 {
        match self {
            Self::Shaped(text) => text.x_for_index(ix),
            Self::Window {
                text,
                start,
                x,
                width,
                ..
            } => {
                if ix <= *start {
                    *x
                } else if ix >= start + text.len {
                    (x + text.width).min(*width)
                } else {
                    x + text.x_for_index(ix - start)
                }
            }
            Self::Unshaped(_) => 0.0,
        }
    }

    fn index_at_x(&self, at: f32) -> usize {
        match self {
            Self::Shaped(text) => text.closest_index_for_x(at),
            Self::Window { text, start, x, .. } => {
                start + text.closest_index_for_x((at - x).max(0.0))
            }
            Self::Unshaped(_) => 0,
        }
    }

    /// Selection spans for bytes `from..to` of the row.
    fn spans(&self, from: usize, to: usize) -> Vec<(f32, f32)> {
        match self {
            Self::Shaped(text) => text.selection_spans(from, to),
            Self::Window { text, start, x, .. } => {
                let (from, to) = (from.max(*start), to.min(start + text.len));
                if from >= to {
                    return Vec::new();
                }
                let spans = text.selection_spans(from - start, to - start);
                spans.into_iter().map(|(l, r)| (l + x, r + x)).collect()
            }
            Self::Unshaped(_) => Vec::new(),
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
            Self::Shaped(text) => canvas.draw_row(text, origin, line_height, color, underline),
            Self::Window { text, start, x, .. } => {
                let underline = underline.and_then(|r| {
                    let (from, to) = (r.start.max(*start), r.end.min(start + text.len));
                    (from < to).then(|| from - start..to - start)
                });
                canvas.draw_row(text, origin + point(*x, 0.0), line_height, color, underline);
            }
            Self::Unshaped(_) => {}
        }
    }
}

/// A line on screen: where it starts in the text, its top, and its rows from `first_row` on.
/// Rows before `first_row` (off screen in a line of many rows) are only counted, as `skipped`
/// bytes.
pub(crate) struct ShownLine {
    offset: usize,
    top: f32,
    first_row: usize,
    skipped: usize,
    total_rows: usize,
    indent: f32,
    rows: Vec<Row>,
}

impl ShownLine {
    /// A line shown as one row.
    fn one_row(offset: usize, top: f32, row: Row) -> Self {
        Self {
            offset,
            top,
            first_row: 0,
            skipped: 0,
            total_rows: 1,
            indent: 0.0,
            rows: vec![row],
        }
    }

    /// Each row with its row number in the line and where its bytes start in the line.
    fn each_row(&self) -> impl Iterator<Item = (usize, usize, &Row)> {
        let starts = self.rows.iter().scan(self.skipped, |start, row| {
            let at = *start;
            *start += row.len();
            Some(at)
        });
        (self.first_row..)
            .zip(starts)
            .zip(&self.rows)
            .map(|((n, start), row)| (n, start, row))
    }

    fn indent(&self, row: usize) -> f32 {
        if row == 0 { 0.0 } else { self.indent }
    }

    fn width(&self) -> f32 {
        let widths = self
            .each_row()
            .map(|(n, _, row)| row.width() + self.indent(n));
        widths.fold(0.0, f32::max)
    }

    /// Where byte `local` of the line is drawn, relative to the line's top. At a wrap,
    /// `end_affinity` keeps it on the end of the earlier row.
    pub(crate) fn position_for_index(
        &self,
        local: usize,
        line_height: f32,
        end_affinity: bool,
    ) -> Option<Point> {
        self.each_row().find_map(|(n, start, row)| {
            let end = start + row.len();
            let hit = if row.len() == 0 {
                local == start
            } else if n + 1 == self.total_rows || end_affinity {
                (start..=end).contains(&local)
            } else {
                (start..end).contains(&local)
            };
            hit.then(|| {
                point(
                    row.x_for_index(local - start) + self.indent(n),
                    n as f32 * line_height,
                )
            })
        })
    }

    /// The byte of the line nearest `pos` (relative to the line's top), and whether it's the
    /// end of a wrapped row; `None` unless `pos` is level with a shown row.
    pub(crate) fn closest_index_for_position(
        &self,
        pos: Point,
        line_height: f32,
    ) -> Option<(usize, bool)> {
        self.each_row().find_map(|(n, start, row)| {
            let top = n as f32 * line_height;
            (pos.y >= top && pos.y < top + line_height).then(|| {
                let col = row.index_at_x(pos.x - self.indent(n));
                (start + col, n + 1 < self.total_rows && col == row.len())
            })
        })
    }

    /// The byte of the line nearest `x` in its last shown row.
    fn index_in_last_row(&self, x: f32) -> usize {
        let Some((n, start, row)) = self.each_row().last() else {
            return self.skipped;
        };
        start + row.index_at_x(x - self.indent(n))
    }
}

/// The lines a frame shows, shaped and stacked in content coordinates.
pub(crate) struct FrameLayout {
    first_line: usize,
    lines: Vec<ShownLine>,
    /// The bytes of the shown lines.
    visible: Range<usize>,
    pub(crate) line_height: f32,
    pub(crate) space_width: f32,
}

impl FrameLayout {
    /// A single-line field's row: `shown` (its text, or a placeholder while it's empty, so
    /// `text_len` is 0).
    pub(crate) fn single_row(
        shown: SharedStr,
        text_len: usize,
        style: &TextStyle,
        line_height: f32,
        ts: &TextSystem,
    ) -> Self {
        let row = Row::Shaped(ts.shape_row(shown, style));
        Self {
            first_line: 0,
            lines: vec![ShownLine::one_row(0, 0.0, row)],
            visible: 0..text_len,
            line_height,
            space_width: ts.face(style).space_width,
        }
    }

    /// The widest shown line.
    pub(crate) fn width(&self) -> f32 {
        self.lines.iter().map(ShownLine::width).fold(0.0, f32::max)
    }

    /// Text line `line`, if it's shown.
    pub(crate) fn line(&self, line: usize) -> Option<&ShownLine> {
        self.lines.get(line.checked_sub(self.first_line)?)
    }

    pub(crate) fn position_for_offset(
        &self,
        text: &Rope,
        offset: usize,
        end_affinity: bool,
    ) -> Option<Point> {
        let line = self.line(text.offset_to_point(offset).row)?;
        let local = offset.saturating_sub(line.offset);
        let pos = line.position_for_index(local, self.line_height, end_affinity)?;
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
        let start = self
            .position_for_offset(text, range.start, false)
            .or_else(|| self.position_for_offset(text, fallback, false))?;
        let end = self
            .position_for_offset(text, range.end, false)
            .unwrap_or(start);
        Some(Rect::from_corners(
            origin + start,
            origin + point(end.x, start.y + self.line_height),
        ))
    }

    /// The text offset (and end affinity) nearest `rel`, a point in content coordinates.
    /// Above a line means its start; below the last means its last row.
    pub(crate) fn hit_test(&self, rel: Point, text_len: usize, multi_line: bool) -> (usize, bool) {
        if multi_line {
            for line in &self.lines {
                let local = rel - point(0.0, line.top);
                if let Some((col, affinity)) =
                    line.closest_index_for_position(local, self.line_height)
                {
                    return ((line.offset + col).min(text_len), affinity);
                }
                if local.y < 0.0 {
                    return (line.offset.min(text_len), false);
                }
            }
        }
        self.lines.last().map_or((0, false), |line| {
            (
                (line.offset + line.index_in_last_row(rel.x)).min(text_len),
                false,
            )
        })
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
            let marked = marked.and_then(|m| {
                (m.end > line.offset)
                    .then(|| m.start.saturating_sub(line.offset)..m.end.saturating_sub(line.offset))
            });
            let line_origin = point(origin.x, origin.y + line.top);
            for (n, start, row) in line.each_row() {
                let underline = marked.as_ref().and_then(|m| {
                    let (from, to) = (m.start.max(start), m.end.min(start + row.len()));
                    (from < to).then(|| from - start..to - start)
                });
                let row_origin = line_origin + point(line.indent(n), n as f32 * self.line_height);
                row.paint(row_origin, self.line_height, color, underline, canvas);
            }
        }
    }

    /// Selection boxes for `range` over the shaped rows, with the content placed at `origin`.
    /// A selection running past a line's end also covers a space for the line break.
    pub(crate) fn range_rects(&self, range: &Range<usize>, origin: Point) -> Vec<Rect> {
        let range = range.start.max(self.visible.start)..range.end.min(self.visible.end);
        let mut out = Vec::new();
        if range.is_empty() {
            return out;
        }
        let lh = self.line_height;
        for line in &self.lines {
            for (n, start, row) in line.each_row() {
                if matches!(row, Row::Unshaped(_)) {
                    continue;
                }
                let (from, to) = (line.offset + start, line.offset + start + row.len());
                let y = line.top + n as f32 * lh;
                let indent = line.indent(n);
                let (s, e) = (range.start.max(from), range.end.min(to));
                if s < e {
                    for (l, r) in row.spans(s - from, e - from) {
                        let rect =
                            Rect::from_corners(point(indent + l, y), point(indent + r, y + lh));
                        out.push(rect.translate(origin));
                    }
                }
                if n + 1 == line.total_rows && range.start <= to && range.end > to {
                    let left = indent + row.width();
                    let rect =
                        Rect::from_corners(point(left, y), point(left + self.space_width, y + lh));
                    out.push(rect.translate(origin));
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn style() -> TextStyle {
        TextStyle::regular("Arial", 14.)
    }

    fn lines(spec: &[(usize, &[usize])]) -> Vec<Line> {
        let line = |&(len, breaks): &(usize, &[usize])| Line {
            len,
            breaks: breaks.into(),
            indent: 0.0,
        };
        spec.iter().map(line).collect()
    }

    #[test]
    fn splices_keep_line_and_row_counts_and_the_longest_line() {
        let mut index = LineIndex::new(style(), Some(10.));
        index.splice(0..0, lines(&[(2, &[]), (4, &[2]), (1, &[])]));
        assert_eq!(
            (index.line_count(), index.visual_row_count(), index.longest),
            (3, 4, 1)
        );
        assert_eq!(index.line(2).0, 3);
        assert_eq!(index.visual_row_to_line_and_sub_row(2), (1, 1));

        index.splice(1..2, lines(&[(2, &[]), (6, &[2, 4])]));
        assert_eq!(
            (index.line_count(), index.visual_row_count(), index.longest),
            (4, 6, 2)
        );

        index.splice(0..4, lines(&[(0, &[])]));
        assert_eq!((index.line_count(), index.visual_row_count()), (1, 1));
        assert_eq!(index.visual_row_range(0, 0), Some((0..0, false)));
    }

    #[test]
    fn rows_stay_consistent_across_chunk_boundaries() {
        let wrapped = |i: usize| {
            if i.is_multiple_of(3) {
                (4, &[2][..])
            } else {
                (2, &[][..])
            }
        };
        let mut index = LineIndex::new(style(), Some(10.));
        index.splice(0..0, lines(&(0..700).map(wrapped).collect::<Vec<_>>()));
        assert!(index.chunks.len() >= 3);
        index.splice(250..265, lines(&[(4, &[2]), (4, &[2]), (4, &[2])]));

        let mut row = 0;
        for line in 0..index.line_count() {
            let (first, wrapped) = index.line(line);
            assert_eq!(first, row);
            for sub_row in 0..wrapped.unwrap().rows() {
                assert_eq!(
                    index.visual_row_to_line_and_sub_row(row + sub_row),
                    (line, sub_row)
                );
            }
            row += wrapped.unwrap().rows();
        }
        assert_eq!(index.visual_row_count(), row);
    }

    #[test]
    fn positions_at_a_wrap_follow_end_affinity() {
        let text = Rope::from("first line\nthis one wraps");
        let mut index = LineIndex::new(style(), None);
        index.splice(0..0, lines(&[(10, &[]), (14, &[9])]));
        let at = |row, sub_row, col| DisplayPos { row, sub_row, col };
        assert_eq!(index.offset_to_display(&text, 20, true), at(1, 0, 9));
        assert_eq!(index.offset_to_display(&text, 20, false), at(2, 1, 0));
        assert_eq!(index.display_to_offset(&text, at(2, 1, 3)), 23);
    }

    #[test]
    fn wrapped_rows_indent_and_carets_keep_their_affinity() {
        let row = |len| {
            let mut text = ShapedRow::default();
            text.len = len;
            Row::Shaped(text)
        };
        let line = ShownLine {
            offset: 0,
            top: 0.0,
            first_row: 0,
            skipped: 0,
            total_rows: 2,
            indent: 20.,
            rows: vec![row(10), row(5)],
        };
        assert_eq!(
            line.closest_index_for_position(point(999., 5.), 20.),
            Some((10, true))
        );
        assert_eq!(
            line.position_for_index(10, 20., true).map(|p| p.y),
            Some(0.)
        );
        assert_eq!(
            line.position_for_index(10, 20., false),
            Some(point(20., 20.))
        );
    }

    #[test]
    fn leading_whitespace_indents_wrapped_rows() {
        let ts = TextSystem::new().unwrap();
        let face = ts.face(&style());
        let (breaks, indent) = wrap(&face, "    indented words that wrap onto more rows", 120.);
        assert!(!breaks.is_empty());
        assert!(indent > 0.0);
        let (breaks, indent) = wrap(&face, "short", 120.);
        assert!(breaks.is_empty() && indent == 0.0);
    }

    #[test]
    fn wraps_tabs_emoji_combining_marks_and_line_separators() {
        let ts = TextSystem::new().unwrap();
        let sample =
            "\tindented e\u{0301} 👨‍👩‍👧\u{2028}after_lsep more words to wrap cleanly across rows";
        let rope = Rope::from(sample);
        let style = TextStyle::regular("Consolas", 14.0);
        for (width, rows) in [(180.0, 2), (0.0, 1)] {
            let mut index = LineIndex::new(style.clone(), Some(width));
            index.sync_layout(&rope, &style, Some(width), &ts);
            assert!(index.visual_row_count() >= rows, "{width}");
        }
    }

    /// A long unwrapped line is shaped only around the viewport, but always across all of it.
    #[test]
    fn huge_unwrapped_lines_are_shaped_across_the_viewport() {
        let ts = TextSystem::new().unwrap();
        let text = Rope::from(format!("short\n{}\nend", "x".repeat(20_000)).as_str());
        let mut index = LineIndex::new(style(), None);
        index.sync_layout(&text, &style(), None, &ts);
        for scroll_x in [0.0, -5_000.0, -40_000.0] {
            let size = crate::geom::size(300., 100.);
            let view = Viewport {
                size,
                scroll_x,
                scroll_y: 0.0,
            };
            let (layout, windowed) = index.layout_visible(&text, view, 0, false, 20., &ts);
            assert!(windowed);
            let Row::Window { text, x, .. } = &layout.lines[1].rows[0] else {
                panic!("the long line isn't windowed");
            };
            let (left, right) = (*x, x + text.width);
            assert!(
                left <= -scroll_x && right >= -scroll_x + size.width,
                "{left}..{right} at {scroll_x}"
            );
        }
    }
}
