//! Clean-room DirectWrite text shaping, line layout, hit-testing, and generational UI-label cache.

mod wrap;

pub(crate) use wrap::{Boundary, CharWidths, MAX_WRAP_INDENT, line_break_opportunities};

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE,
    DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT, DWRITE_HIT_TEST_METRICS, DWRITE_LINE_METRICS,
    DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_METRICS,
    DWRITE_TEXT_RANGE, DWRITE_TRIMMING, DWRITE_TRIMMING_GRANULARITY_CHARACTER,
    DWRITE_WORD_WRAPPING_NO_WRAP, DWRITE_WORD_WRAPPING_WRAP, DWriteCreateFactory, IDWriteFactory5,
    IDWriteTextFormat, IDWriteTextLayout,
};
use windows::core::{BOOL, HSTRING};

use crate::ui::app::App;
use crate::ui::color::Rgba;
use crate::ui::geometry::{Bounds, Pixels, Point, Size, point, px, size};
use crate::ui::graphics::d2d_renderer::fill;
use crate::ui::types::{SharedString, UiResult};
use crate::ui::window::Window;

const MAX_LAYOUT_CACHE: usize = 512;

#[derive(Copy, Clone, Debug, PartialEq, PartialOrd)]
pub(crate) struct FontWeight(pub(crate) f32);

impl Default for FontWeight {
    fn default() -> Self {
        Self::NORMAL
    }
}

impl Eq for FontWeight {}

impl Hash for FontWeight {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.to_bits().hash(state);
    }
}

impl FontWeight {
    pub(crate) const NORMAL: Self = Self(400.0);
    pub(crate) const SEMIBOLD: Self = Self(600.0);
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) enum FontStyle {
    #[default]
    Normal,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Font {
    pub(crate) family: SharedString,
    pub(crate) weight: FontWeight,
    pub(crate) style: FontStyle,
}

impl Default for Font {
    fn default() -> Self {
        Self {
            family: SharedString::new_static("Segoe UI Variable Text"),
            weight: FontWeight::NORMAL,
            style: FontStyle::Normal,
        }
    }
}

pub(crate) fn font(family: impl Into<SharedString>) -> Font {
    Font {
        family: family.into(),
        ..Default::default()
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct FontId(pub(crate) usize);

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct FontRun {
    pub(crate) len: usize,
    pub(crate) font_id: FontId,
}

#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub(crate) struct UnderlineStyle {
    pub(crate) thickness: Pixels,
    pub(crate) color: Option<Rgba>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TextRun {
    pub(crate) len: usize,
    pub(crate) font: Font,
    pub(crate) color: Rgba,
    pub(crate) underline: Option<UnderlineStyle>,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) enum TextAlign {
    #[default]
    Left,
    Center,
}

#[derive(Copy, Clone, Debug)]
struct UnderlineSpan {
    start_utf8: usize,
    end_utf8: usize,
    thickness: Pixels,
    color: Rgba,
}

/// DirectWrite-backed shaped single-line text ready for hit-testing and Direct2D painting.
#[derive(Clone)]
pub(crate) struct ShapedLine {
    pub(crate) len: usize,
    pub(crate) width: Pixels,
    pub(crate) ascent: Pixels,
    pub(crate) descent: Pixels,
    pub(crate) text: SharedString,
    layout: Option<IDWriteTextLayout>,
    default_color: Rgba,
    run_colors: Vec<(u32, u32, Rgba)>,
    underlines: Vec<UnderlineSpan>,
    is_ascii: bool,
}

impl Default for ShapedLine {
    fn default() -> Self {
        Self {
            len: 0,
            width: Pixels::ZERO,
            ascent: px(11.0),
            descent: px(3.0),
            text: SharedString::default(),
            layout: None,
            default_color: Rgba::new(0.0, 0.0, 0.0, 1.0),
            run_colors: Vec::new(),
            underlines: Vec::new(),
            is_ascii: true,
        }
    }
}

impl ShapedLine {
    #[cfg(test)]
    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    #[cfg(test)]
    pub(crate) fn with_len(mut self, len: usize) -> Self {
        self.len = len;
        self
    }

    #[cfg(test)]
    #[inline]
    pub(crate) fn width(&self) -> Pixels {
        self.width
    }

    pub(crate) fn x_for_index(&self, index: usize) -> Pixels {
        let index = index.min(self.len);
        if index == 0 {
            return Pixels::ZERO;
        }
        if index >= self.len {
            return self.width;
        }
        let Some(layout) = &self.layout else {
            return Pixels::ZERO;
        };
        let utf16_pos = utf8_to_utf16_offset(self.text.as_str(), index, self.is_ascii);
        let mut x = 0.0f32;
        let mut y = 0.0f32;
        let mut metrics = DWRITE_HIT_TEST_METRICS::default();
        if unsafe { layout.HitTestTextPosition(utf16_pos, false, &mut x, &mut y, &mut metrics) }
            .is_ok()
        {
            px(x)
        } else {
            self.width
        }
    }

    pub(crate) fn closest_index_for_x(&self, x: Pixels) -> usize {
        if self.len == 0 || x <= Pixels::ZERO {
            return 0;
        }
        if x >= self.width {
            return self.len;
        }
        let Some(layout) = &self.layout else {
            return 0;
        };
        let mut is_trailing = BOOL(0);
        let mut is_inside = BOOL(0);
        let mut metrics = DWRITE_HIT_TEST_METRICS::default();
        if unsafe {
            layout.HitTestPoint(x.as_f32(), 0.0, &mut is_trailing, &mut is_inside, &mut metrics)
        }
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

    pub(crate) fn paint(
        &self,
        origin: Point<Pixels>,
        line_height: Pixels,
        align: TextAlign,
        align_width: Option<Pixels>,
        window: &mut Window,
        _cx: &mut App,
    ) -> UiResult<()> {
        let Some(layout) = &self.layout else {
            return Ok(());
        };
        let align_offset_x = match (align, align_width) {
            (TextAlign::Center, Some(w)) => (w - self.width) * 0.5,
            _ => Pixels::ZERO,
        };
        let glyph_height = self.ascent + self.descent;
        let top_padding = ((line_height - glyph_height) * 0.5).max(Pixels::ZERO);
        let draw_origin = point(origin.x + align_offset_x, origin.y + top_padding);

        window.paint_dwrite_layout(draw_origin, layout, self.default_color, &self.run_colors);

        for ul in &self.underlines {
            let x0 = self.x_for_index(ul.start_utf8);
            let x1 = self.x_for_index(ul.end_utf8);
            if x1 > x0 {
                let y = draw_origin.y + self.ascent + px(2.0);
                let rect = Bounds::new(
                    point(draw_origin.x + x0, y),
                    size(x1 - x0, ul.thickness.max(px(1.0))),
                );
                window.paint_quad(fill(rect, ul.color));
            }
        }
        Ok(())
    }
}

/// Lightweight single-line measurement returned by `layout_line`.
#[derive(Clone, Debug, Default)]
pub(crate) struct LineLayout {
    pub(crate) width: Pixels,
}

/// Immutable multi-line wrapped DirectWrite layout (used by tooltips, dialogs, and `TextLeaf`).
#[derive(Clone)]
pub(crate) struct WrappedLine {
    pub(crate) width: Pixels,
    pub(crate) line_count: usize,
    layout_box_width: Option<Pixels>,
    wrap_breaks: Vec<usize>,
    pub(crate) ascent: Pixels,
    pub(crate) descent: Pixels,
    layout: Option<IDWriteTextLayout>,
    run_colors: Vec<(u32, u32, Rgba)>,
}

impl WrappedLine {
    #[inline]
    pub(crate) fn width(&self) -> Pixels {
        self.width
    }

    #[inline]
    pub(crate) fn wrap_boundaries(&self) -> &[usize] {
        &self.wrap_breaks
    }

    pub(crate) fn size(&self, line_height: Pixels) -> Size<Pixels> {
        size(self.width, line_height * self.line_count.max(1))
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn paint_with_color(
        &self,
        origin: Point<Pixels>,
        line_height: Pixels,
        align: TextAlign,
        align_width: Option<Pixels>,
        color: Rgba,
        window: &mut Window,
        _cx: &mut App,
    ) -> UiResult<()> {
        let Some(layout) = &self.layout else {
            return Ok(());
        };
        // Never mutate `IDWriteTextLayout` COM state at paint time (layouts are shared in the
        // two-frame cache). Centering shifts the draw origin by the box delta.
        let align_offset_x = match (align, align_width) {
            (TextAlign::Center, Some(w)) => {
                let ref_w = self.layout_box_width.unwrap_or(self.width);
                (w - ref_w) * 0.5
            }
            _ => Pixels::ZERO,
        };
        let glyph_height = self.ascent + self.descent;
        let top_padding = ((line_height - glyph_height) * 0.5).max(Pixels::ZERO);
        window.paint_dwrite_layout(
            point(origin.x + align_offset_x, origin.y + top_padding),
            layout,
            color,
            &self.run_colors,
        );
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct FormatKey {
    family: SharedString,
    weight: FontWeight,
    style: FontStyle,
    size_hundredths: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ShapedCacheKey {
    text: SharedString,
    font_size_hundredths: u32,
    family: SharedString,
    weight: FontWeight,
    style: FontStyle,
    color_bits: [u32; 4],
    has_underline: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct WrappedCacheKey {
    text: SharedString,
    font_size_hundredths: u32,
    family: SharedString,
    weight: FontWeight,
    style: FontStyle,
    wrap_width_tenths: Option<u32>,
    trim_width_tenths: Option<u32>,
    line_clamp: Option<usize>,
    align: TextAlign,
}

/// Low-level DirectWrite helper passed into `with_char_widths` callbacks.
pub(crate) struct DirectWriteCore {
    factory: IDWriteFactory5,
    fonts: Vec<Font>,
    font_ids: HashMap<Font, FontId>,
    formats: RefCell<HashMap<FormatKey, IDWriteTextFormat>>,
    space_widths: RefCell<HashMap<FormatKey, Pixels>>,
}

impl DirectWriteCore {
    fn new() -> Self {
        let factory: IDWriteFactory5 =
            unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED) }
                .expect("failed to create DirectWrite factory");
        Self {
            factory,
            fonts: Vec::new(),
            font_ids: HashMap::new(),
            formats: RefCell::new(HashMap::new()),
            space_widths: RefCell::new(HashMap::new()),
        }
    }

    fn font_id(&mut self, font: &Font) -> FontId {
        if let Some(&id) = self.font_ids.get(font) {
            return id;
        }
        let id = FontId(self.fonts.len());
        self.fonts.push(font.clone());
        self.font_ids.insert(font.clone(), id);
        id
    }

    fn font_by_id(&self, id: FontId) -> Font {
        self.fonts.get(id.0).cloned().unwrap_or_default()
    }

    pub(super) fn text_format(&self, font: &Font, font_size: Pixels) -> IDWriteTextFormat {
        let key = FormatKey {
            family: font.family.clone(),
            weight: font.weight,
            style: font.style,
            size_hundredths: (font_size.as_f32() * 100.0).round().max(1.0) as u32,
        };
        if let Some(existing) = self.formats.borrow().get(&key) {
            return existing.clone();
        }
        let family = HSTRING::from(font.family.as_str());
        let locale = HSTRING::from("en-us");
        let dw_weight = DWRITE_FONT_WEIGHT(font.weight.0.round() as i32);
        let dw_style: DWRITE_FONT_STYLE = match font.style {
            FontStyle::Normal => DWRITE_FONT_STYLE_NORMAL,
        };
        let format = unsafe {
            self.factory.CreateTextFormat(
                &family,
                None,
                dw_weight,
                dw_style,
                DWRITE_FONT_STRETCH_NORMAL,
                font_size.as_f32().max(1.0),
                &locale,
            )
        }
        .expect("CreateTextFormat failed");
        let _ = unsafe { format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP) };
        self.formats.borrow_mut().insert(key, format.clone());
        format
    }

    pub(super) fn measure_utf16(&self, utf16: &[u16], format: &IDWriteTextFormat) -> Pixels {
        if utf16.is_empty() {
            return Pixels::ZERO;
        }
        let Ok(layout) =
            (unsafe { self.factory.CreateTextLayout(utf16, format, f32::MAX, f32::MAX) })
        else {
            return Pixels::ZERO;
        };
        let mut metrics = DWRITE_TEXT_METRICS::default();
        if unsafe { layout.GetMetrics(&mut metrics) }.is_ok() {
            px(metrics.widthIncludingTrailingWhitespace)
        } else {
            Pixels::ZERO
        }
    }

    pub(crate) fn layout_line(
        &self,
        text: &str,
        font_size: Pixels,
        runs: &[FontRun],
    ) -> LineLayout {
        let base_font = runs
            .first()
            .map(|r| self.font_by_id(r.font_id))
            .unwrap_or_default();
        if text.is_empty() {
            return LineLayout {
                width: Pixels::ZERO,
            };
        }
        let format = self.text_format(&base_font, font_size);
        let utf16: Vec<u16> = text.encode_utf16().collect();
        let Ok(layout) =
            (unsafe { self.factory.CreateTextLayout(&utf16, &format, f32::MAX, f32::MAX) })
        else {
            return LineLayout {
                width: Pixels::ZERO,
            };
        };
        let (width, _, _) = extract_single_line_metrics(&layout, font_size);
        LineLayout { width }
    }
}

/// High-level text system owned by `App` (`window.text_system()` / `cx.text_system()`).
pub(crate) struct TextSystem {
    state: RefCell<TextSystemState>,
}

struct TextSystemState {
    dw: DirectWriteCore,
    char_widths: HashMap<(Font, u32), CharWidths>,
    shaped_cache: HashMap<ShapedCacheKey, ShapedLine>,
    prev_wrapped_cache: HashMap<WrappedCacheKey, Vec<WrappedLine>>,
    curr_wrapped_cache: HashMap<WrappedCacheKey, Vec<WrappedLine>>,
}

impl TextSystem {
    pub(crate) fn new() -> Self {
        Self {
            state: RefCell::new(TextSystemState {
                dw: DirectWriteCore::new(),
                char_widths: HashMap::new(),
                shaped_cache: HashMap::new(),
                prev_wrapped_cache: HashMap::new(),
                curr_wrapped_cache: HashMap::new(),
            }),
        }
    }

    /// Rotates the two-frame generational cache for UI-label DirectWrite layouts.
    pub(crate) fn finish_frame(&self) {
        let mut state = self.state.borrow_mut();
        state.prev_wrapped_cache = std::mem::take(&mut state.curr_wrapped_cache);
    }

    pub(crate) fn space_width(&self, font: &Font, font_size: Pixels) -> Pixels {
        let state = self.state.borrow();
        let key = FormatKey {
            family: font.family.clone(),
            weight: font.weight,
            style: font.style,
            size_hundredths: (font_size.as_f32() * 100.0).round().max(1.0) as u32,
        };
        if let Some(&w) = state.dw.space_widths.borrow().get(&key) {
            return w;
        }
        let format = state.dw.text_format(font, font_size);
        let w = state.dw.measure_utf16(&[b' ' as u16], &format);
        state.dw.space_widths.borrow_mut().insert(key, w);
        w
    }

    pub(crate) fn measure_line_width(&self, text: &str, font: &Font, font_size: Pixels) -> Pixels {
        if text.is_empty() {
            return Pixels::ZERO;
        }
        let state = self.state.borrow();
        let format = state.dw.text_format(font, font_size);
        let utf16: Vec<u16> = text.encode_utf16().collect();
        state.dw.measure_utf16(&utf16, &format)
    }

    pub(crate) fn layout_line(
        &self,
        text: &str,
        font_size: Pixels,
        runs: &[TextRun],
    ) -> LineLayout {
        let mut state = self.state.borrow_mut();
        let font_runs: Vec<FontRun> = runs
            .iter()
            .map(|r| FontRun {
                len: r.len,
                font_id: state.dw.font_id(&r.font),
            })
            .collect();
        state.dw.layout_line(text, font_size, &font_runs)
    }

    pub(crate) fn shape_line(
        &self,
        text: SharedString,
        font_size: Pixels,
        runs: &[TextRun],
    ) -> ShapedLine {
        let base_run = runs.first();
        let base_font = base_run.map(|r| r.font.clone()).unwrap_or_default();
        let default_color = base_run.map_or(Rgba::new(0.0, 0.0, 0.0, 1.0), |r| r.color);

        if text.is_empty() {
            return ShapedLine {
                len: 0,
                width: Pixels::ZERO,
                ascent: font_size * 0.8,
                descent: font_size * 0.2,
                text,
                layout: None,
                default_color,
                run_colors: Vec::new(),
                underlines: Vec::new(),
                is_ascii: true,
            };
        }

        let single_uniform_run = runs.len() <= 1;
        let cache_key = single_uniform_run.then(|| ShapedCacheKey {
            text: text.clone(),
            font_size_hundredths: (font_size.as_f32() * 100.0).round() as u32,
            family: base_font.family.clone(),
            weight: base_font.weight,
            style: base_font.style,
            color_bits: [
                default_color.r.to_bits(),
                default_color.g.to_bits(),
                default_color.b.to_bits(),
                default_color.a.to_bits(),
            ],
            has_underline: base_run.is_some_and(|r| r.underline.is_some()),
        });

        let mut state = self.state.borrow_mut();
        if let Some(key) = &cache_key
            && let Some(cached) = state.shaped_cache.get(key)
        {
            return cached.clone();
        }

        let format = state.dw.text_format(&base_font, font_size);
        let utf16: Vec<u16> = text.as_str().encode_utf16().collect();
        let is_ascii = text.as_str().is_ascii();
        let layout = unsafe {
            state
                .dw
                .factory
                .CreateTextLayout(&utf16, &format, f32::MAX, f32::MAX)
        }
        .ok();

        let mut run_colors = Vec::new();
        let mut underlines = Vec::new();

        if let Some(layout) = &layout {
            let mut byte_pos = 0usize;
            let mut utf16_pos = 0u32;
            for run in runs {
                let end_byte = (byte_pos + run.len).min(text.len());
                let slice = &text.as_str()[byte_pos..end_byte];
                let utf16_len = if is_ascii {
                    slice.len() as u32
                } else {
                    slice.encode_utf16().count() as u32
                };
                if utf16_len > 0 {
                    let range = DWRITE_TEXT_RANGE {
                        startPosition: utf16_pos,
                        length: utf16_len,
                    };
                    if run.font.family != base_font.family {
                        let fam = HSTRING::from(run.font.family.as_str());
                        let _ = unsafe { layout.SetFontFamilyName(&fam, range) };
                    }
                    if run.font.weight != base_font.weight {
                        let _ = unsafe {
                            layout.SetFontWeight(
                                DWRITE_FONT_WEIGHT(run.font.weight.0.round() as i32),
                                range,
                            )
                        };
                    }
                    if run.font.style != base_font.style {
                        let st = match run.font.style {
                            FontStyle::Normal => DWRITE_FONT_STYLE_NORMAL,
                        };
                        let _ = unsafe { layout.SetFontStyle(st, range) };
                    }
                    if run.color != default_color {
                        run_colors.push((utf16_pos, utf16_len, run.color));
                    }
                    if let Some(ul) = run.underline {
                        underlines.push(UnderlineSpan {
                            start_utf8: byte_pos,
                            end_utf8: end_byte,
                            thickness: ul.thickness,
                            color: ul.color.unwrap_or(run.color),
                        });
                    }
                }
                byte_pos = end_byte;
                utf16_pos = utf16_pos.saturating_add(utf16_len);
            }
        }

        let (width, ascent, descent) = layout
            .as_ref()
            .map_or((Pixels::ZERO, font_size * 0.8, font_size * 0.2), |l| {
                extract_single_line_metrics(l, font_size)
            });

        let shaped = ShapedLine {
            len: text.len(),
            width,
            ascent,
            descent,
            text,
            layout,
            default_color,
            run_colors,
            underlines,
            is_ascii,
        };

        if let Some(key) = cache_key {
            if state.shaped_cache.len() >= MAX_LAYOUT_CACHE {
                state.shaped_cache.clear();
            }
            state.shaped_cache.insert(key, shaped.clone());
        }
        shaped
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn shape_text(
        &self,
        text: SharedString,
        font_size: Pixels,
        font: &Font,
        color: Rgba,
        underline: Option<UnderlineStyle>,
        wrap_width: Option<Pixels>,
        line_clamp: Option<usize>,
    ) -> Vec<WrappedLine> {
        self.shape_text_trimmed(
            text,
            font_size,
            font,
            color,
            underline,
            wrap_width,
            line_clamp,
            None,
            TextAlign::Left,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn shape_text_trimmed(
        &self,
        text: SharedString,
        font_size: Pixels,
        font: &Font,
        _color: Rgba,
        _underline: Option<UnderlineStyle>,
        wrap_width: Option<Pixels>,
        line_clamp: Option<usize>,
        trim_width: Option<Pixels>,
        align: TextAlign,
    ) -> Vec<WrappedLine> {
        let key = WrappedCacheKey {
            text: text.clone(),
            font_size_hundredths: (font_size.as_f32() * 100.0).round().max(1.0) as u32,
            family: font.family.clone(),
            weight: font.weight,
            style: font.style,
            wrap_width_tenths: wrap_width.map(|w| (w.as_f32().max(1.0) * 10.0).round() as u32),
            trim_width_tenths: trim_width.map(|w| (w.as_f32().max(1.0) * 10.0).round() as u32),
            line_clamp,
            align,
        };

        let mut state = self.state.borrow_mut();
        if let Some(cached) = state.curr_wrapped_cache.get(&key) {
            return cached.clone();
        }
        if let Some(cached) = state.prev_wrapped_cache.remove(&key) {
            state.curr_wrapped_cache.insert(key, cached.clone());
            return cached;
        }

        let format = state.dw.text_format(font, font_size);
        let ellipsis_sign = if trim_width.is_some() {
            unsafe { state.dw.factory.CreateEllipsisTrimmingSign(&format) }.ok()
        } else {
            None
        };
        let mut out = Vec::new();
        let mut remaining_lines = line_clamp;

        for segment in text.as_str().split('\n') {
            if remaining_lines == Some(0) {
                break;
            }
            let utf16: Vec<u16> = segment.encode_utf16().collect();
            let box_w = wrap_width.or(trim_width).map(|w| w.max(px(1.0)));
            let max_w = box_w.map_or(f32::MAX, Pixels::as_f32);
            let layout = unsafe {
                state
                    .dw
                    .factory
                    .CreateTextLayout(&utf16, &format, max_w, f32::MAX)
            }
            .ok();
            if let Some(layout) = &layout {
                let _ = unsafe {
                    layout.SetWordWrapping(if wrap_width.is_some() {
                        DWRITE_WORD_WRAPPING_WRAP
                    } else {
                        DWRITE_WORD_WRAPPING_NO_WRAP
                    })
                };
                if box_w.is_some() {
                    let dw_align = match align {
                        TextAlign::Left => DWRITE_TEXT_ALIGNMENT_LEADING,
                        TextAlign::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
                    };
                    let _ = unsafe { layout.SetTextAlignment(dw_align) };
                }
                if let Some(sign) = &ellipsis_sign {
                    let trimming = DWRITE_TRIMMING {
                        granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                        delimiter: 0,
                        delimiterCount: 0,
                    };
                    let _ = unsafe { layout.SetTrimming(&trimming, sign) };
                }
            }
            let (width, raw_lines, ascent, descent) = layout.as_ref().map_or(
                (Pixels::ZERO, 1, font_size * 0.8, font_size * 0.2),
                |l| {
                    let mut m = DWRITE_TEXT_METRICS::default();
                    let _ = unsafe { l.GetMetrics(&mut m) };
                    let (_, asc, desc) = extract_single_line_metrics(l, font_size);
                    (
                        px(m.widthIncludingTrailingWhitespace),
                        m.lineCount.max(1) as usize,
                        asc,
                        desc,
                    )
                },
            );
            let line_count = match remaining_lines {
                Some(cap) => {
                    let clamped = raw_lines.min(cap);
                    remaining_lines = Some(cap.saturating_sub(clamped));
                    if clamped < raw_lines
                        && let Some(layout) = &layout
                    {
                        let _ = unsafe {
                            layout.SetMaxHeight((ascent + descent).as_f32() * clamped as f32)
                        };
                    }
                    clamped
                }
                None => raw_lines,
            };
            let wrap_breaks = vec![0usize; line_count.saturating_sub(1)];
            out.push(WrappedLine {
                width,
                line_count,
                layout_box_width: box_w,
                wrap_breaks,
                ascent,
                descent,
                layout,
                run_colors: Vec::new(),
            });
        }

        if state.curr_wrapped_cache.len() < MAX_LAYOUT_CACHE {
            state.curr_wrapped_cache.insert(key, out.clone());
        }
        out
    }

    pub(crate) fn with_char_widths<R>(
        &self,
        font: &Font,
        font_size: Pixels,
        f: impl FnOnce(FontId, &mut CharWidths, &DirectWriteCore) -> R,
    ) -> R {
        let mut state = self.state.borrow_mut();
        let font_id = state.dw.font_id(font);
        let key = (font.clone(), (font_size.as_f32() * 100.0).round() as u32);
        let TextSystemState {
            dw, char_widths, ..
        } = &mut *state;
        let widths = char_widths
            .entry(key)
            .or_insert_with(|| CharWidths::new(font.clone(), font_size, dw));
        f(font_id, widths, dw)
    }

    #[cfg(test)]
    pub(crate) fn wrapped_cache_counts(&self) -> (usize, usize) {
        let state = self.state.borrow();
        (state.curr_wrapped_cache.len(), state.prev_wrapped_cache.len())
    }
}

fn extract_single_line_metrics(
    layout: &IDWriteTextLayout,
    font_size: Pixels,
) -> (Pixels, Pixels, Pixels) {
    let mut metrics = DWRITE_TEXT_METRICS::default();
    let _ = unsafe { layout.GetMetrics(&mut metrics) };
    let mut line_metrics = [DWRITE_LINE_METRICS::default(); 2];
    let mut count = 0u32;
    let (ascent, descent) =
        if unsafe { layout.GetLineMetrics(Some(&mut line_metrics), &mut count) }.is_ok()
            && count > 0
        {
            let lm = line_metrics[0];
            (px(lm.baseline), px((lm.height - lm.baseline).max(0.0)))
        } else {
            (font_size * 0.8, font_size * 0.2)
        };
    (px(metrics.widthIncludingTrailingWhitespace), ascent, descent)
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
        let ts = TextSystem::new();
        let shaped = ts.shape_line(
            SharedString::new_static("Hello 🎉 world"),
            px(14.0),
            &[TextRun {
                len: "Hello 🎉 world".len(),
                font: font("Segoe UI Variable Text"),
                color: Rgba::new(0.0, 0.0, 0.0, 1.0),
                underline: None,
            }],
        );
        assert!(shaped.width() > Pixels::ZERO);
        let mid_x = shaped.x_for_index(6);
        assert!(mid_x > Pixels::ZERO && mid_x < shaped.width());
        let hit = shaped.closest_index_for_x(mid_x);
        assert!(hit <= shaped.len());

        let breaks = line_break_opportunities("hello world");
        assert!(breaks.contains(&6));
        assert!(line_break_opportunities("foo\u{00A0}bar").is_empty());
        assert_eq!(line_break_opportunities("foo-bar"), vec![4]);
    }

    #[test]
    fn wrapped_text_two_frame_cache_promotes_and_evicts_and_isolates_alignment() {
        use windows::core::Interface;

        let ts = TextSystem::new();
        let f = font("Segoe UI Variable Small");
        let black = Rgba::new(0.0, 0.0, 0.0, 1.0);

        let a = ts.shape_text_trimmed(
            SharedString::new_static("Ln 1, Col 1"),
            px(12.0),
            &f,
            black,
            None,
            None,
            None,
            None,
            TextAlign::Left,
        );
        let b = ts.shape_text_trimmed(
            SharedString::new_static("Ln 1, Col 1"),
            px(12.0),
            &f,
            black,
            None,
            None,
            None,
            None,
            TextAlign::Center,
        );
        // Distinct TextAlign produces distinct cached COM layouts.
        assert_ne!(
            a[0].layout.as_ref().unwrap().as_raw(),
            b[0].layout.as_ref().unwrap().as_raw()
        );
        assert_eq!(ts.wrapped_cache_counts(), (2, 0));

        // Rotate frame: entries move to prev. Re-querying `a` promotes it and reuses the COM object.
        ts.finish_frame();
        assert_eq!(ts.wrapped_cache_counts(), (0, 2));
        let a2 = ts.shape_text_trimmed(
            SharedString::new_static("Ln 1, Col 1"),
            px(12.0),
            &f,
            black,
            None,
            None,
            None,
            None,
            TextAlign::Left,
        );
        assert_eq!(
            a[0].layout.as_ref().unwrap().as_raw(),
            a2[0].layout.as_ref().unwrap().as_raw()
        );
        assert_eq!(ts.wrapped_cache_counts(), (1, 1));

        // Next frame rotation evicts untouched `b` and keeps `a` in prev.
        ts.finish_frame();
        assert_eq!(ts.wrapped_cache_counts(), (0, 1));
        // One more unused frame evicts `a` completely.
        ts.finish_frame();
        assert_eq!(ts.wrapped_cache_counts(), (0, 0));
    }
}
