use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;

use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT, DWRITE_HIT_TEST_METRICS, DWRITE_LINE_METRICS,
    DWRITE_LINE_SPACING_METHOD_UNIFORM, DWRITE_TEXT_METRICS, DWRITE_TRIMMING,
    DWRITE_TRIMMING_GRANULARITY_CHARACTER, DWRITE_WORD_WRAPPING_NO_WRAP, DWRITE_WORD_WRAPPING_WRAP,
    DWriteCreateFactory, IDWriteFactory5, IDWriteTextAnalyzer, IDWriteTextFormat,
    IDWriteTextLayout,
};
use windows::core::{BOOL, HSTRING};

use crate::color::Rgba;
use crate::geom::{Point, Rect, Size, point, size};
use crate::types::{Result, SharedStr};

use super::FrameCache;
use super::d2d::{Canvas, fill};
use super::line_breaks::{AdvanceTable, can_wrap_with_advances};

const MAX_LAYOUT_CACHE: usize = 512;
const MAX_SHAPED_CACHE_BYTES: usize = 4 * 1024 * 1024;
const MAX_CACHEABLE_LINE_BYTES: usize = 16 * 1024;
const ROW_CACHE_KEEP_FRAMES: u64 = 16;
const HUGE_LINE_BYTES: usize = 4096;
pub(super) const TAB_STOP_COLUMNS: f32 = 8.0;

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) enum FontWeight {
    #[default]
    Normal,
    Semibold,
}

impl FontWeight {
    #[inline]
    fn to_dwrite(self) -> DWRITE_FONT_WEIGHT {
        match self {
            Self::Normal => DWRITE_FONT_WEIGHT(400),
            Self::Semibold => DWRITE_FONT_WEIGHT(600),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Font {
    pub(crate) family: SharedStr,
    pub(crate) weight: FontWeight,
}

impl Default for Font {
    fn default() -> Self {
        Self {
            family: SharedStr::new_static("Segoe UI Variable Text"),
            weight: FontWeight::Normal,
        }
    }
}

pub(crate) fn font(family: impl Into<SharedStr>) -> Font {
    Font {
        family: family.into(),
        weight: FontWeight::Normal,
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct TextStyle {
    pub(crate) font: Font,
    size_hundredths: u32,
}

impl Default for TextStyle {
    fn default() -> Self {
        Self::new(Font::default(), 14.0)
    }
}

impl TextStyle {
    #[inline]
    fn quantize_size(size: f32) -> u32 {
        let safe = if size.is_finite() {
            size.clamp(1.0, 4096.0)
        } else {
            14.0
        };
        (safe * 100.0).round().max(1.0) as u32
    }

    pub(crate) fn new(font: Font, size: f32) -> Self {
        Self {
            font,
            size_hundredths: Self::quantize_size(size),
        }
    }

    pub(crate) fn regular(family: impl Into<SharedStr>, size: f32) -> Self {
        Self::new(font(family), size)
    }

    #[inline]
    pub(crate) fn set_size(&mut self, size: f32) {
        self.size_hundredths = Self::quantize_size(size);
    }

    #[inline]
    pub(crate) fn size(&self) -> f32 {
        self.size_hundredths as f32 / 100.0
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct LabelLayout {
    pub(crate) line_height: f32,
    pub(crate) wrap_width: Option<f32>,
    pub(crate) line_clamp: Option<usize>,
}

#[derive(Clone)]
pub(crate) struct ShapedRow {
    pub(crate) len: usize,
    pub(crate) width: f32,
    ascent: f32,
    descent: f32,
    text: SharedStr,
    layout: Option<IDWriteTextLayout>,
    is_ascii: bool,
}

impl Default for ShapedRow {
    fn default() -> Self {
        Self {
            len: 0,
            width: 0.0,
            ascent: 11.0,
            descent: 3.0,
            text: SharedStr::default(),
            layout: None,
            is_ascii: true,
        }
    }
}

impl ShapedRow {
    pub(crate) fn x_for_index(&self, index: usize) -> f32 {
        let index = index.min(self.len);
        if index == 0 && self.is_ascii {
            return 0.0;
        }
        if index >= self.len && self.is_ascii {
            return self.width;
        }
        let Some(layout) = &self.layout else {
            return if index == 0 { 0.0 } else { self.width };
        };
        let utf16_pos = utf8_to_utf16_offset(self.text.as_str(), index, self.is_ascii);
        let mut x = 0.0f32;
        let mut y = 0.0f32;
        let mut metrics = DWRITE_HIT_TEST_METRICS::default();
        if unsafe { layout.HitTestTextPosition(utf16_pos, false, &mut x, &mut y, &mut metrics) }
            .is_ok()
        {
            x
        } else {
            self.width
        }
    }

    pub(crate) fn selection_spans(&self, start: usize, end: usize) -> Vec<(f32, f32)> {
        let start = start.min(self.len);
        let end = end.min(self.len);
        if start >= end {
            return Vec::new();
        }
        if self.is_ascii {
            let x0 = self.x_for_index(start);
            let x1 = self.x_for_index(end);
            return if x1 > x0 { vec![(x0, x1)] } else { Vec::new() };
        }
        let Some(layout) = &self.layout else {
            return Vec::new();
        };
        let u16_start = utf8_to_utf16_offset(self.text.as_str(), start, false);
        let u16_end = utf8_to_utf16_offset(self.text.as_str(), end, false);
        let u16_len = u16_end.saturating_sub(u16_start);
        if u16_len == 0 {
            return Vec::new();
        }
        let mut actual = 0u32;
        let mut buf = [DWRITE_HIT_TEST_METRICS::default(); 8];
        let res = unsafe {
            layout.HitTestTextRange(u16_start, u16_len, 0.0, 0.0, Some(&mut buf), &mut actual)
        };
        if res.is_ok() {
            return buf[..actual as usize]
                .iter()
                .filter(|m| m.width > 0.0)
                .map(|m| (m.left, m.left + m.width))
                .collect();
        }
        if actual > 0 {
            let mut heap = vec![DWRITE_HIT_TEST_METRICS::default(); actual as usize];
            if unsafe {
                layout.HitTestTextRange(u16_start, u16_len, 0.0, 0.0, Some(&mut heap), &mut actual)
            }
            .is_ok()
            {
                return heap[..actual as usize]
                    .iter()
                    .filter(|m| m.width > 0.0)
                    .map(|m| (m.left, m.left + m.width))
                    .collect();
            }
        }
        let x0 = self.x_for_index(start);
        let x1 = self.x_for_index(end);
        let (lo, hi) = if x0 <= x1 { (x0, x1) } else { (x1, x0) };
        if hi > lo { vec![(lo, hi)] } else { Vec::new() }
    }

    pub(crate) fn closest_index_for_x(&self, x: f32) -> usize {
        if self.len == 0 {
            return 0;
        }
        if self.is_ascii && x <= 0.0 {
            return 0;
        }
        if self.is_ascii && x >= self.width {
            return self.len;
        }
        let Some(layout) = &self.layout else {
            return 0;
        };
        let mut is_trailing = BOOL(0);
        let mut is_inside = BOOL(0);
        let mut metrics = DWRITE_HIT_TEST_METRICS::default();
        if unsafe { layout.HitTestPoint(x, 0.0, &mut is_trailing, &mut is_inside, &mut metrics) }
            .is_err()
        {
            return 0;
        }
        let utf16_pos = if is_trailing.as_bool() {
            metrics.textPosition.saturating_add(metrics.length)
        } else {
            metrics.textPosition
        };
        utf16_to_utf8_offset(self.text.as_str(), utf16_pos, self.is_ascii)
    }
}

#[derive(Clone)]
pub(crate) struct Label {
    pub(crate) width: f32,
    pub(crate) line_count: usize,
    line_height: f32,
    layout_box_width: Option<f32>,
    clamped: bool,
    layout: Option<IDWriteTextLayout>,
}

impl Label {
    pub(crate) fn size(&self) -> Size {
        size(self.width.ceil(), self.line_height * self.line_count as f32)
    }
}

impl Canvas {
    pub(crate) fn draw_row(
        &mut self,
        row: &ShapedRow,
        origin: Point,
        line_height: f32,
        color: Rgba,
        underline: Option<Range<usize>>,
    ) {
        let Some(layout) = &row.layout else {
            return;
        };
        let glyph_height = row.ascent + row.descent;
        let top_padding = ((line_height - glyph_height) * 0.5).max(0.0);
        let draw_origin = point(origin.x, origin.y + top_padding);

        self.draw_text_layout(draw_origin, layout, color);

        if let Some(range) = underline
            && range.start < range.end
        {
            let x0 = row.x_for_index(range.start);
            let x1 = row.x_for_index(range.end);
            let (lo, hi) = if x0 <= x1 { (x0, x1) } else { (x1, x0) };
            if hi > lo {
                let y = draw_origin.y + row.ascent + 2.0;
                self.paint_quad(fill(
                    Rect::new(point(draw_origin.x + lo, y), size(hi - lo, 1.0)),
                    color,
                ));
            }
        }
    }

    pub(crate) fn draw_label(
        &mut self,
        label: &Label,
        origin: Point,
        width: f32,
        color: Rgba,
    ) {
        let Some(layout) = &label.layout else {
            return;
        };
        if label.clamped {
            let clip_w = label
                .layout_box_width
                .map_or(width, |bw| width.max(bw))
                .max(label.width)
                + 2.0;
            self.with_clip(Rect::new(origin, size(clip_w, label.size().height)), |c| {
                c.draw_text_layout(origin, layout, color);
            });
        } else {
            self.draw_text_layout(origin, layout, color);
        }
    }
}

pub(crate) struct FontFace {
    factory: IDWriteFactory5,
    analyzer: Option<IDWriteTextAnalyzer>,
    format: IDWriteTextFormat,
    ascent: f32,
    descent: f32,
    space_width: f32,
    advances: RefCell<Option<AdvanceTable>>,
}

impl FontFace {
    #[inline]
    pub(crate) fn space_width(&self) -> f32 {
        self.space_width
    }

    fn with_advances<R>(&self, f: impl FnOnce(&mut AdvanceTable) -> R) -> R {
        let mut slot = self.advances.borrow_mut();
        let table = slot.get_or_insert_with(|| AdvanceTable::new(self.factory.clone(), self.format.clone()));
        f(table)
    }

    pub(crate) fn max_printable_ascii(&self) -> f32 {
        self.with_advances(|adv| adv.max_printable_ascii())
    }

    pub(crate) fn hanging_indent(&self, line: &str, max_chars: u32, max_width: f32) -> f32 {
        self.with_advances(|adv| {
            let mut chars = 0u32;
            let mut width = 0.0f32;
            for ch in line.chars() {
                if (ch == ' ' || ch == '\t') && chars < max_chars {
                    width += adv.advance(ch, width);
                    chars += 1;
                } else {
                    break;
                }
            }
            if width >= max_width { 0.0 } else { width }
        })
    }

    pub(crate) fn line_breaks(
        &self,
        line: &str,
        wrap_width: f32,
        indent_px: f32,
        out: &mut Vec<usize>,
    ) {
        out.clear();
        if line.is_empty() || !wrap_width.is_finite() || wrap_width <= 0.0 {
            return;
        }
        self.with_advances(|adv| {
            if can_wrap_with_advances(line) {
                adv.fast_breaks(line, wrap_width, indent_px, self.analyzer.as_ref(), out);
            } else {
                let cont_width = (wrap_width - indent_px).max(1.0);
                adv.dwrite_breaks(line, wrap_width, cont_width, out);
            }
        });
    }

    pub(crate) fn horizontal_window(
        &self,
        line: &str,
        min_x: f32,
        max_x: f32,
    ) -> Option<(usize, usize, f32, f32)> {
        if !can_wrap_with_advances(line) {
            return None;
        }
        Some(self.with_advances(|adv| adv.horizontal_slice(line, min_x, max_x)))
    }

    pub(crate) fn unwrapped_width(&self, line: &str) -> f32 {
        if line.is_empty() {
            return 0.0;
        }
        if line.len() > HUGE_LINE_BYTES && can_wrap_with_advances(line) {
            self.with_advances(|adv| adv.horizontal_slice(line, 0.0, 0.0).3)
        } else {
            let utf16 = encode_single_line_utf16(line);
            measure_utf16(&self.factory, &utf16, &self.format)
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct RowCacheKey {
    text: SharedStr,
    style: TextStyle,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct LabelCacheKey {
    text: SharedStr,
    style: TextStyle,
    line_height_tenths: u32,
    wrap_width_tenths: Option<u32>,
    line_clamp: Option<usize>,
}

pub(crate) struct TextSystem {
    factory: IDWriteFactory5,
    analyzer: Option<IDWriteTextAnalyzer>,
    state: RefCell<TextSystemState>,
}

struct TextSystemState {
    faces: HashMap<TextStyle, Rc<FontFace>>,
    rows: FrameCache<RowCacheKey, ShapedRow>,
    labels: FrameCache<LabelCacheKey, Label>,
}

impl TextSystem {
    pub(crate) fn new() -> Result<Self> {
        let factory: IDWriteFactory5 =
            unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)? };
        let analyzer = unsafe { factory.CreateTextAnalyzer() }.ok();
        Ok(Self {
            factory,
            analyzer,
            state: RefCell::new(TextSystemState {
                faces: HashMap::new(),
                rows: FrameCache::new(
                    MAX_LAYOUT_CACHE,
                    MAX_SHAPED_CACHE_BYTES,
                    ROW_CACHE_KEEP_FRAMES,
                ),
                labels: FrameCache::new(MAX_LAYOUT_CACHE, 0, 1),
            }),
        })
    }

    pub(crate) fn face(&self, style: &TextStyle) -> Rc<FontFace> {
        let mut state = self.state.borrow_mut();
        state
            .faces
            .entry(style.clone())
            .or_insert_with(|| Rc::new(create_font_face(&self.factory, &self.analyzer, style)))
            .clone()
    }

    pub(crate) fn finish_frame(&self) {
        let mut state = self.state.borrow_mut();
        state.labels.advance_frame();
        state.rows.advance_frame();
    }

    pub(crate) fn measure(&self, text: &str, style: &TextStyle) -> f32 {
        if text.is_empty() {
            return 0.0;
        }
        if text == " " {
            return self.face(style).space_width();
        }
        self.shape_label(
            SharedStr::from(text.to_owned()),
            style,
            LabelLayout {
                line_height: style.size(),
                wrap_width: None,
                line_clamp: None,
            },
        )
        .width
    }

    pub(crate) fn shape_row(&self, text: SharedStr, style: &TextStyle) -> ShapedRow {
        let face = self.face(style);
        if text.is_empty() {
            return ShapedRow {
                len: 0,
                width: 0.0,
                ascent: face.ascent,
                descent: face.descent,
                text,
                layout: None,
                is_ascii: true,
            };
        }

        let cache_key = (text.len() <= MAX_CACHEABLE_LINE_BYTES).then(|| RowCacheKey {
            text: text.clone(),
            style: style.clone(),
        });
        if let Some(key) = &cache_key
            && let Some(cached) = self.state.borrow_mut().rows.get(key)
        {
            return cached.clone();
        }

        let utf16 = encode_single_line_utf16(text.as_str());
        let is_ascii = text.as_str().is_ascii();
        let layout = unsafe {
            self.factory
                .CreateTextLayout(&utf16, &face.format, f32::MAX, f32::MAX)
        }
        .ok();

        if let Some(layout) = &layout {
            let _ = unsafe {
                layout.SetLineSpacing(
                    DWRITE_LINE_SPACING_METHOD_UNIFORM,
                    face.ascent + face.descent,
                    face.ascent,
                )
            };
        }

        let width = layout.as_ref().map_or(0.0, |l| {
            let mut metrics = DWRITE_TEXT_METRICS::default();
            if unsafe { l.GetMetrics(&mut metrics) }.is_ok() {
                metrics.widthIncludingTrailingWhitespace
            } else {
                0.0
            }
        });

        let shaped = ShapedRow {
            len: text.len(),
            width,
            ascent: face.ascent,
            descent: face.descent,
            text,
            layout,
            is_ascii,
        };

        if let Some(key) = cache_key {
            let weight = key.text.len().max(64);
            self.state.borrow_mut().rows.insert(key, shaped.clone(), weight);
        }
        shaped
    }

    pub(crate) fn shape_label(
        &self,
        text: SharedStr,
        style: &TextStyle,
        opts: LabelLayout,
    ) -> Label {
        let lh = opts.line_height.max(1.0);
        let box_w = opts.wrap_width.map(|w| w.max(1.0));
        let key = LabelCacheKey {
            text: text.clone(),
            style: style.clone(),
            line_height_tenths: (lh * 10.0).round() as u32,
            wrap_width_tenths: box_w.map(|w| (w * 10.0).round() as u32),
            line_clamp: opts.line_clamp,
        };

        if let Some(cached) = self.state.borrow_mut().labels.get(&key) {
            return cached.clone();
        }

        let face = self.face(style);
        let glyph_h = (face.ascent + face.descent).max(1.0);
        let utf16: Vec<u16> = text.as_str().encode_utf16().collect();
        let max_w = box_w.unwrap_or(f32::MAX);
        let layout = unsafe {
            self.factory
                .CreateTextLayout(&utf16, &face.format, max_w, f32::MAX)
        }
        .ok();

        if let Some(layout) = &layout {
            let top_pad = ((lh - glyph_h) * 0.5).max(0.0);
            unsafe {
                let _ = layout.SetWordWrapping(if box_w.is_some() {
                    DWRITE_WORD_WRAPPING_WRAP
                } else {
                    DWRITE_WORD_WRAPPING_NO_WRAP
                });
                let _ = layout.SetLineSpacing(
                    DWRITE_LINE_SPACING_METHOD_UNIFORM,
                    lh,
                    top_pad + face.ascent,
                );
            }
        }

        let (width, raw_lines) = layout.as_ref().map_or((0.0, 1), |l| {
            let mut m = DWRITE_TEXT_METRICS::default();
            let _ = unsafe { l.GetMetrics(&mut m) };
            (m.widthIncludingTrailingWhitespace, m.lineCount.max(1) as usize)
        });

        let (line_count, clamped) = match opts.line_clamp {
            Some(cap) if raw_lines > cap => {
                let clamped_lines = cap.max(1);
                if let Some(layout) = &layout {
                    unsafe {
                        let _ = layout.SetMaxHeight(lh * clamped_lines as f32);
                        if let Ok(sign) = self.factory.CreateEllipsisTrimmingSign(&face.format) {
                            let trimming = DWRITE_TRIMMING {
                                granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                                delimiter: 0,
                                delimiterCount: 0,
                            };
                            let _ = layout.SetTrimming(&trimming, &sign);
                        }
                    }
                }
                (clamped_lines, true)
            }
            _ => (raw_lines, false),
        };

        let label = Label {
            width,
            line_count,
            line_height: lh,
            layout_box_width: box_w,
            clamped,
            layout,
        };
        self.state.borrow_mut().labels.insert(key, label.clone(), 0);
        label
    }

    #[cfg(test)]
    pub(crate) fn label_cache_counts(&self) -> (usize, usize) {
        self.state.borrow().labels.age_counts()
    }
}

fn create_font_face(
    factory: &IDWriteFactory5,
    analyzer: &Option<IDWriteTextAnalyzer>,
    style: &TextStyle,
) -> FontFace {
    let safe_size = style.size();
    let family = HSTRING::from(style.font.family.as_str());
    let locale = HSTRING::from("");
    let dw_weight = style.font.weight.to_dwrite();
    let format = unsafe {
        factory
            .CreateTextFormat(
                &family,
                None,
                dw_weight,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                safe_size,
                &locale,
            )
            .or_else(|_| {
                let fallback = HSTRING::from("Segoe UI");
                factory.CreateTextFormat(
                    &fallback,
                    None,
                    DWRITE_FONT_WEIGHT(400),
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    safe_size,
                    &locale,
                )
            })
    }
    .expect("CreateTextFormat fallback failed");
    let _ = unsafe { format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP) };
    let space_width = measure_utf16(factory, &[b' ' as u16], &format);
    if space_width > 0.0 {
        let _ = unsafe { format.SetIncrementalTabStop(space_width * TAB_STOP_COLUMNS) };
    }
    let sample = [b'H' as u16, b'g' as u16];
    let (ascent, descent) =
        unsafe { factory.CreateTextLayout(&sample, &format, f32::MAX, f32::MAX) }
            .ok()
            .map_or((safe_size * 0.8, safe_size * 0.2), |l| {
                let mut line_metrics = [DWRITE_LINE_METRICS::default(); 2];
                let mut count = 0u32;
                if unsafe { l.GetLineMetrics(Some(&mut line_metrics), &mut count) }.is_ok()
                    && count > 0
                {
                    let lm = line_metrics[0];
                    (lm.baseline, (lm.height - lm.baseline).max(0.0))
                } else {
                    (safe_size * 0.8, safe_size * 0.2)
                }
            });
    FontFace {
        factory: factory.clone(),
        analyzer: analyzer.clone(),
        format,
        ascent,
        descent,
        space_width,
        advances: RefCell::new(None),
    }
}

pub(super) fn measure_utf16(
    factory: &IDWriteFactory5,
    utf16: &[u16],
    format: &IDWriteTextFormat,
) -> f32 {
    if utf16.is_empty() {
        return 0.0;
    }
    let Ok(layout) = (unsafe { factory.CreateTextLayout(utf16, format, f32::MAX, f32::MAX) })
    else {
        return 0.0;
    };
    let mut metrics = DWRITE_TEXT_METRICS::default();
    if unsafe { layout.GetMetrics(&mut metrics) }.is_ok() {
        metrics.widthIncludingTrailingWhitespace
    } else {
        0.0
    }
}

pub(super) fn encode_single_line_utf16(text: &str) -> Vec<u16> {
    let mut out = Vec::with_capacity(text.len());
    for ch in text.chars() {
        let mapped = match ch {
            '\u{000B}' | '\u{000C}' | '\u{0085}' | '\u{2028}' | '\u{2029}' => '\u{25A1}',
            other => other,
        };
        let mut buf = [0u16; 2];
        out.extend_from_slice(mapped.encode_utf16(&mut buf));
    }
    out
}

#[inline]
pub(crate) fn utf8_to_utf16_offset(text: &str, utf8_offset: usize, is_ascii: bool) -> u32 {
    if is_ascii {
        return utf8_offset.min(text.len()) as u32;
    }
    let clamped = text.floor_char_boundary(utf8_offset.min(text.len()));
    text[..clamped].encode_utf16().count() as u32
}

#[inline]
pub(crate) fn utf16_to_utf8_offset(text: &str, utf16_offset: u32, is_ascii: bool) -> usize {
    if is_ascii {
        return (utf16_offset as usize).min(text.len());
    }
    let mut u16_acc = 0u32;
    for (byte_ix, ch) in text.char_indices() {
        if u16_acc >= utf16_offset {
            return byte_ix;
        }
        u16_acc += ch.len_utf16() as u32;
    }
    text.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_utf16_roundtrip_handles_ascii_cjk_and_surrogate_emoji() {
        let sample = "hi 日本語 🎉 end";
        for (byte_ix, _) in sample.char_indices().chain([(sample.len(), '\0')]) {
            let u16_off = utf8_to_utf16_offset(sample, byte_ix, false);
            assert_eq!(utf16_to_utf8_offset(sample, u16_off, false), byte_ix);
        }
    }

    #[test]
    fn directwrite_line_breaks_and_shaping_work() {
        let ts = TextSystem::new().unwrap();
        let style = TextStyle::regular("Segoe UI Variable Text", 14.0);
        let shaped = ts.shape_row(SharedStr::new_static("Hello 🎉 world"), &style);
        assert!(shaped.width > 0.0);
        let mid_x = shaped.x_for_index(6);
        assert!(mid_x > 0.0 && mid_x < shaped.width);
        let hit = shaped.closest_index_for_x(mid_x);
        assert!(hit <= shaped.len);

        let analyzer = ts.analyzer.as_ref().expect("text analyzer");
        let breaks = super::super::line_breaks::analyze_break_opportunities(analyzer, "hello world");
        assert!(breaks.contains(&6));
        assert!(
            super::super::line_breaks::analyze_break_opportunities(analyzer, "foo\u{00A0}bar")
                .is_empty()
        );
        assert_eq!(
            super::super::line_breaks::analyze_break_opportunities(analyzer, "foo-bar"),
            vec![4]
        );
    }

    #[test]
    fn wrapped_text_two_frame_cache_promotes_and_evicts() {
        use windows::core::Interface;

        let ts = TextSystem::new().unwrap();
        let style = TextStyle::regular("Segoe UI Variable Small", 12.0);
        let single = LabelLayout {
            line_height: 16.0,
            wrap_width: None,
            line_clamp: None,
        };
        let wrapped = LabelLayout {
            wrap_width: Some(80.0),
            ..single
        };

        let a = ts.shape_label(SharedStr::new_static("Ln 1, Col 1"), &style, single);
        let b = ts.shape_label(SharedStr::new_static("Ln 1, Col 1"), &style, wrapped);
        assert_ne!(
            a.layout.as_ref().unwrap().as_raw(),
            b.layout.as_ref().unwrap().as_raw()
        );
        assert_eq!(ts.label_cache_counts(), (2, 0));

        ts.finish_frame();
        assert_eq!(ts.label_cache_counts(), (0, 2));
        let a2 = ts.shape_label(SharedStr::new_static("Ln 1, Col 1"), &style, single);
        assert_eq!(
            a.layout.as_ref().unwrap().as_raw(),
            a2.layout.as_ref().unwrap().as_raw()
        );
        assert_eq!(ts.label_cache_counts(), (1, 1));

        ts.finish_frame();
        assert_eq!(ts.label_cache_counts(), (0, 1));
        ts.finish_frame();
        assert_eq!(ts.label_cache_counts(), (0, 0));
    }
}
