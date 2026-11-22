mod line;

pub(crate) use line::*;

use crate::ui::collections::FxHashMap;
use crate::ui::smallvec::SmallVec;
use crate::ui::sync::{Mutex, RwLock};
use crate::ui::{
    Bounds, DevicePixels, Hsla, Pixels, Point, Result, SharedString, Size, UnderlineStyle, px,
};
use crate::win32::DirectWriteTextSystem;
use std::{
    borrow::Borrow,
    hash::{Hash, Hasher},
    sync::Arc,
};

/// An opaque identifier for a specific font.
#[derive(Hash, PartialEq, Eq, Clone, Copy, Debug)]
#[repr(C)]
pub(crate) struct FontId(pub(crate) usize);

/// Number of subpixel glyph variants along the X axis.
pub(crate) const SUBPIXEL_VARIANTS_X: u8 = 4;

/// Number of subpixel glyph variants along the Y axis.
pub(crate) const SUBPIXEL_VARIANTS_Y: u8 = 1;

/// Maximum leading-space indent preserved on soft-wrapped continuation lines.
pub(crate) const MAX_WRAP_INDENT: u32 = 256;

// Leave enough room below the underline for its stroke while keeping it below the baseline.
const UNDERLINE_DESCENT_OFFSET_FACTOR: f32 = 0.618;

/// Returns the vertical offset used to paint an underline within a line.
pub(crate) fn underline_y_offset(line_height: Pixels, ascent: Pixels, descent: Pixels) -> Pixels {
    let padding_top = (line_height - ascent - descent) / 2.;
    padding_top + ascent + descent * UNDERLINE_DESCENT_OFFSET_FACTOR
}

/// A soft-wrap break boundary within a logical line.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct Boundary {
    /// Byte index in the line where the next visual row begins.
    pub(crate) ix: usize,
    /// Leading space count reserved on continuation rows.
    pub(crate) next_indent: u32,
}

/// Characters treated as part of an unbroken word during fast line wrapping.
pub(crate) fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(
            c,
            '\u{00C0}'..='\u{00FF}'
                | '\u{0100}'..='\u{017F}'
                | '\u{0180}'..='\u{024F}'
                | '\u{0400}'..='\u{04FF}'
                | '\u{1E00}'..='\u{1EFF}'
                | '\u{0300}'..='\u{036F}'
                | '\u{0980}'..='\u{09FF}'
                | '-'
                | '_'
                | '.'
                | '\''
                | '’'
                | '‘'
                | '$'
                | '%'
                | '@'
                | '#'
                | '^'
                | '~'
                | ','
                | '='
                | ':'
                | ';'
                | '!'
                | ')'
                | ']'
                | '}'
                | '"'
                | '”'
                | '»'
                | '…'
                | '⋯'
                | '\u{202F}'
                | '\u{00A0}'
                | '\u{2011}'
        )
}

/// Returns whether a line can cleanly wrap immediately before `ch`.
#[inline]
pub(crate) fn is_break_opportunity(prev_ch: char, ch: char, has_non_space: bool) -> bool {
    has_non_space && ch != ' ' && (!is_word_char(ch) || prev_ch == ' ')
}

/// Per-(font, size) character width table powering fast editor wrapping and space metrics.
pub(crate) struct CharWidths {
    font_id: FontId,
    font_size: Pixels,
    ascii: [Option<Pixels>; 128],
    max_printable_ascii: Pixels,
    other: FxHashMap<char, Pixels>,
}

impl CharWidths {
    fn new(font_id: FontId, font_size: Pixels, dw: &DirectWriteTextSystem) -> Self {
        let mut ascii = [None; 128];
        let mut max_printable_ascii = px(0.);
        for b in 0x20u8..=0x7e {
            let width = measure_char_width(dw, font_id, font_size, char::from(b));
            ascii[b as usize] = Some(width);
            max_printable_ascii = max_printable_ascii.max(width);
        }
        Self {
            font_id,
            font_size,
            ascii,
            max_printable_ascii,
            other: FxHashMap::default(),
        }
    }

    #[inline]
    pub(crate) fn max_printable_ascii(&self) -> Pixels {
        self.max_printable_ascii
    }

    #[inline]
    pub(crate) fn space_width(&self) -> Pixels {
        self.ascii[b' ' as usize].unwrap_or(px(0.))
    }

    #[inline]
    fn width_for_char(&mut self, c: char, dw: &DirectWriteTextSystem) -> Pixels {
        if (c as u32) < 128 {
            let slot = &mut self.ascii[c as usize];
            *slot.get_or_insert_with(|| measure_char_width(dw, self.font_id, self.font_size, c))
        } else {
            *self
                .other
                .entry(c)
                .or_insert_with(|| measure_char_width(dw, self.font_id, self.font_size, c))
        }
    }

    /// Append soft-wrap boundaries for `text` at `wrap_width`, preserving leading spaces.
    pub(crate) fn wrap_line_into(
        &mut self,
        text: &str,
        wrap_width: Pixels,
        dw: &DirectWriteTextSystem,
        out: &mut Vec<Boundary>,
    ) {
        let mut width = px(0.);
        let mut first_non_whitespace_ix = None;
        let mut base_indent = None;
        let mut last_candidate_ix = 0;
        let mut last_candidate_width = px(0.);
        let mut last_wrap_ix = 0;
        let mut prev_c = '\0';
        let space_width = self.space_width();

        for (ix, c) in text.char_indices() {
            if c == '\n' {
                continue;
            }

            if is_break_opportunity(prev_c, c, first_non_whitespace_ix.is_some()) {
                last_candidate_ix = ix;
                last_candidate_width = width;
            }

            if c != ' ' && first_non_whitespace_ix.is_none() {
                first_non_whitespace_ix = Some(ix);
            }

            let item_width = self.width_for_char(c, dw);
            width += item_width;
            if width > wrap_width && ix > last_wrap_ix {
                let wrap_at_candidate =
                    last_candidate_ix > 0 && width - last_candidate_width <= wrap_width;

                if base_indent.is_none()
                    && let Some(first_non_whitespace_ix) = first_non_whitespace_ix
                {
                    base_indent = Some(
                        MAX_WRAP_INDENT
                            .min(first_non_whitespace_ix.saturating_sub(last_wrap_ix) as u32),
                    );
                }

                let next_indent = base_indent.unwrap_or(0);

                if wrap_at_candidate {
                    last_wrap_ix = last_candidate_ix;
                    width -= last_candidate_width;
                } else {
                    last_wrap_ix = ix;
                    width = item_width;
                }
                last_candidate_ix = 0;
                width += space_width * next_indent as f32;
                out.push(Boundary {
                    ix: last_wrap_ix,
                    next_indent,
                });
            }

            prev_c = c;
        }
    }
}

#[inline]
fn measure_char_width(
    dw: &DirectWriteTextSystem,
    font_id: FontId,
    font_size: Pixels,
    ch: char,
) -> Pixels {
    let mut buffer = [0; 4];
    let text = ch.encode_utf8(&mut buffer);
    dw.layout_line(
        text,
        font_size,
        &[FontRun {
            len: text.len(),
            font_id,
        }],
    )
    .width
}

/// The unified DirectWrite-backed text rendering and wrapping subsystem.
pub(crate) struct TextSystem {
    platform_text_system: Arc<DirectWriteTextSystem>,
    font_metrics: RwLock<FxHashMap<FontId, FontMetrics>>,
    raster_bounds: RwLock<FxHashMap<RenderGlyphParams, Bounds<DevicePixels>>>,
    char_widths: Mutex<FxHashMap<FontIdWithSize, CharWidths>>,
    fallback_font_stack: [Font; 2],
}

impl TextSystem {
    /// Create a new TextSystem with the given DirectWrite text system.
    pub(crate) fn new(platform_text_system: Arc<DirectWriteTextSystem>) -> Self {
        Self {
            platform_text_system,
            font_metrics: RwLock::default(),
            raster_bounds: RwLock::default(),
            char_widths: Mutex::default(),
            fallback_font_stack: [font("Segoe UI"), font("Arial")],
        }
    }

    /// Resolves the specified font, falling back to the default font stack if
    /// the font fails to load.
    pub(crate) fn resolve_font(&self, font: &Font) -> FontId {
        if let Some(font_id) = self.platform_text_system.font_id(font) {
            return font_id;
        }
        for fallback in &self.fallback_font_stack {
            if let Some(font_id) = self.platform_text_system.font_id(fallback) {
                return font_id;
            }
        }

        panic!(
            "failed to resolve font '{}' or any of the fallbacks: {}, {}",
            font.family, self.fallback_font_stack[0].family, self.fallback_font_stack[1].family
        );
    }

    /// Get the bounding box for the given font and font size.
    pub(crate) fn bounding_box(&self, font_id: FontId, font_size: Pixels) -> Bounds<Pixels> {
        if let Some(metrics) = self.font_metrics.read().get(&font_id) {
            return metrics.bounding_box(font_size);
        }
        let mut lock = self.font_metrics.write();
        let metrics = lock
            .entry(font_id)
            .or_insert_with(|| self.platform_text_system.font_metrics(font_id));
        metrics.bounding_box(font_size)
    }

    /// Cached width of an ASCII space in the given font and size.
    pub(crate) fn space_width(&self, font: &Font, font_size: Pixels) -> Pixels {
        self.with_char_widths(font, font_size, |_, widths, _| widths.space_width())
    }

    /// Shape and measure the pixel width of a single line directly without polluting frame caches.
    pub(crate) fn measure_line_width(&self, text: &str, font: &Font, font_size: Pixels) -> Pixels {
        if text.is_empty() {
            return px(0.);
        }
        let font_id = self.resolve_font(font);
        self.platform_text_system
            .layout_line(
                text,
                font_size,
                &[FontRun {
                    len: text.len(),
                    font_id,
                }],
            )
            .width
    }

    /// Run `f` with mutable access to the cached [`CharWidths`] for `(font, font_size)`.
    pub(crate) fn with_char_widths<R>(
        &self,
        font: &Font,
        font_size: Pixels,
        f: impl FnOnce(FontId, &mut CharWidths, &DirectWriteTextSystem) -> R,
    ) -> R {
        let font_id = self.resolve_font(font);
        let mut map = self.char_widths.lock();
        let widths = map
            .entry(FontIdWithSize { font_id, font_size })
            .or_insert_with(|| CharWidths::new(font_id, font_size, &self.platform_text_system));
        f(font_id, widths, &self.platform_text_system)
    }

    /// Get the rasterized size and location of a specific, rendered glyph.
    pub(crate) fn raster_bounds(&self, params: &RenderGlyphParams) -> Result<Bounds<DevicePixels>> {
        if let Some(bounds) = self.raster_bounds.read().get(params) {
            return Ok(*bounds);
        }
        let bounds = self.platform_text_system.glyph_raster_bounds(params)?;
        self.raster_bounds.write().insert(params.clone(), bounds);
        Ok(bounds)
    }

    pub(crate) fn rasterize_glyph(
        &self,
        params: &RenderGlyphParams,
    ) -> Result<(Size<DevicePixels>, Vec<u8>)> {
        let raster_bounds = self.raster_bounds(params)?;
        self.platform_text_system
            .rasterize_glyph(params, raster_bounds)
    }
}

/// Per-window text shaping facade pairing [`TextSystem`] with a two-frame [`LineLayoutCache`].
pub(crate) struct WindowTextSystem {
    line_layout_cache: LineLayoutCache,
    text_system: Arc<TextSystem>,
}

impl std::ops::Deref for WindowTextSystem {
    type Target = Arc<TextSystem>;
    fn deref(&self) -> &Self::Target {
        &self.text_system
    }
}

impl WindowTextSystem {
    /// Create a new WindowTextSystem with the given TextSystem.
    pub(crate) fn new(text_system: Arc<TextSystem>) -> Self {
        Self {
            line_layout_cache: LineLayoutCache::new(text_system.platform_text_system.clone()),
            text_system,
        }
    }

    /// Shape a single line of text (without newlines) for painting to the screen.
    pub(crate) fn shape_line(
        &self,
        text: SharedString,
        font_size: Pixels,
        runs: &[TextRun],
    ) -> ShapedLine {
        debug_assert!(
            text.find('\n').is_none(),
            "text argument should not contain newlines"
        );

        let mut decoration_runs = SmallVec::<[DecorationRun; 1]>::new();
        for run in runs {
            if let Some(last_run) = decoration_runs.last_mut()
                && last_run.color == run.color
                && last_run.underline == run.underline
            {
                last_run.len += run.len as u32;
                continue;
            }
            decoration_runs.push(DecorationRun {
                len: run.len as u32,
                color: run.color,
                underline: run.underline,
            });
        }

        let layout = self.layout_line(&text, font_size, runs);
        ShapedLine {
            layout,
            decoration_runs,
        }
    }

    /// Shape a multi-line string of text with uniform style, optional wrapping, and line clamping.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn shape_text(
        &self,
        text: SharedString,
        font_size: Pixels,
        font: &Font,
        color: Hsla,
        underline: Option<UnderlineStyle>,
        wrap_width: Option<Pixels>,
        line_clamp: Option<usize>,
    ) -> SmallVec<[WrappedLine; 1]> {
        let font_id = self.resolve_font(font);
        let mut lines = SmallVec::new();
        let mut wrapped_lines = 0;

        let mut push_line = |line_text: SharedString| {
            let len = line_text.len();
            let single_run = [FontRun { len, font_id }];
            let font_runs = if len > 0 {
                single_run.as_slice()
            } else {
                &[]
            };
            let layout = self.line_layout_cache.layout_wrapped_line(
                line_text,
                font_size,
                font_runs,
                wrap_width,
                line_clamp.map(|max| max.saturating_sub(wrapped_lines)),
            );
            wrapped_lines += layout.wrap_boundaries.len();

            let mut decoration_runs = SmallVec::new();
            if len > 0 {
                decoration_runs.push(DecorationRun {
                    len: len as u32,
                    color,
                    underline,
                });
            }
            lines.push(WrappedLine {
                layout,
                decoration_runs,
            });
        };

        if !text.contains('\n') {
            push_line(text);
        } else {
            for line_text in text.split('\n') {
                push_line(SharedString::new(line_text));
            }
        }

        lines
    }

    pub(crate) fn finish_frame(&self) {
        self.line_layout_cache.finish_frame();
    }

    /// Layout a single line of text using the two-frame layout cache.
    pub(crate) fn layout_line(
        &self,
        text: &str,
        font_size: Pixels,
        runs: &[TextRun],
    ) -> Arc<LineLayout> {
        let mut last_run = None::<&TextRun>;
        let mut font_runs = SmallVec::<[FontRun; 4]>::new();

        for run in runs {
            let decoration_changed = if let Some(last_run) = last_run
                && last_run.color == run.color
                && last_run.underline == run.underline
            {
                false
            } else {
                last_run = Some(run);
                true
            };

            let font_id = self.resolve_font(&run.font);
            if let Some(font_run) = font_runs.last_mut()
                && font_id == font_run.font_id
                && !decoration_changed
            {
                font_run.len += run.len;
            } else {
                font_runs.push(FontRun {
                    len: run.len,
                    font_id,
                });
            }
        }

        self.line_layout_cache
            .layout_line(text, font_size, &font_runs)
    }
}

struct LineLayoutCache {
    previous_frame: Mutex<FrameCache>,
    current_frame: RwLock<FrameCache>,
    platform_text_system: Arc<DirectWriteTextSystem>,
}

#[derive(Default)]
struct FrameCache {
    lines: FxHashMap<Arc<CacheKey>, Arc<LineLayout>>,
    wrapped_lines: FxHashMap<Arc<CacheKey>, Arc<WrappedLineLayout>>,
}

impl LineLayoutCache {
    fn new(platform_text_system: Arc<DirectWriteTextSystem>) -> Self {
        Self {
            previous_frame: Mutex::default(),
            current_frame: RwLock::default(),
            platform_text_system,
        }
    }

    fn finish_frame(&self) {
        let mut curr_frame = self.current_frame.write();
        let mut prev_frame = self.previous_frame.lock();
        std::mem::swap(&mut *prev_frame, &mut *curr_frame);
        curr_frame.lines.clear();
        curr_frame.wrapped_lines.clear();
    }

    fn layout_wrapped_line<Text>(
        &self,
        text: Text,
        font_size: Pixels,
        runs: &[FontRun],
        wrap_width: Option<Pixels>,
        max_lines: Option<usize>,
    ) -> Arc<WrappedLineLayout>
    where
        Text: AsRef<str>,
        SharedString: From<Text>,
    {
        let key = &CacheKeyRef {
            text: text.as_ref(),
            font_size,
            runs,
            wrap_width,
        } as &dyn AsCacheKeyRef;

        {
            let current_frame = self.current_frame.read();
            if let Some(layout) = current_frame.wrapped_lines.get(key) {
                return layout.clone();
            }
        }

        let previous_frame_entry = self.previous_frame.lock().wrapped_lines.remove_entry(key);
        if let Some((key, layout)) = previous_frame_entry {
            self.current_frame
                .write()
                .wrapped_lines
                .insert(key, layout.clone());
            layout
        } else {
            let text = SharedString::from(text);
            let unwrapped_layout = self.layout_line::<&SharedString>(&text, font_size, runs);
            let wrap_boundaries = if let Some(wrap_width) = wrap_width {
                unwrapped_layout.compute_wrap_boundaries(text.as_ref(), wrap_width, max_lines)
            } else {
                SmallVec::new()
            };
            let layout = Arc::new(WrappedLineLayout {
                unwrapped_layout,
                wrap_boundaries,
                wrap_width,
            });
            let key = Arc::new(CacheKey {
                text,
                font_size,
                runs: SmallVec::from(runs),
                wrap_width,
            });

            self.current_frame
                .write()
                .wrapped_lines
                .insert(key, layout.clone());
            layout
        }
    }

    fn layout_line<Text>(
        &self,
        text: Text,
        font_size: Pixels,
        runs: &[FontRun],
    ) -> Arc<LineLayout>
    where
        Text: AsRef<str>,
        SharedString: From<Text>,
    {
        let key = &CacheKeyRef {
            text: text.as_ref(),
            font_size,
            runs,
            wrap_width: None,
        } as &dyn AsCacheKeyRef;

        {
            let current_frame = self.current_frame.read();
            if let Some(layout) = current_frame.lines.get(key) {
                return layout.clone();
            }
        }

        let mut current_frame = self.current_frame.write();
        if let Some(layout) = current_frame.lines.get(key) {
            return layout.clone();
        }
        if let Some((key, layout)) = self.previous_frame.lock().lines.remove_entry(key) {
            current_frame.lines.insert(key, layout.clone());
            layout
        } else {
            let text = SharedString::from(text);
            let layout = self
                .platform_text_system
                .layout_line(&text, font_size, runs);
            let key = Arc::new(CacheKey {
                text,
                font_size,
                runs: SmallVec::from(runs),
                wrap_width: None,
            });
            let layout = Arc::new(layout);
            current_frame.lines.insert(key, layout.clone());
            layout
        }
    }
}

/// A run of text with a single font.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub(crate) struct FontRun {
    pub(crate) len: usize,
    pub(crate) font_id: FontId,
}

trait AsCacheKeyRef {
    fn as_cache_key_ref(&self) -> CacheKeyRef<'_>;
}

#[derive(Clone, Debug, Eq)]
struct CacheKey {
    text: SharedString,
    font_size: Pixels,
    runs: SmallVec<[FontRun; 1]>,
    wrap_width: Option<Pixels>,
}

#[derive(Copy, Clone, PartialEq, Eq, Hash)]
struct CacheKeyRef<'a> {
    text: &'a str,
    font_size: Pixels,
    runs: &'a [FontRun],
    wrap_width: Option<Pixels>,
}

impl PartialEq for dyn AsCacheKeyRef + '_ {
    fn eq(&self, other: &dyn AsCacheKeyRef) -> bool {
        self.as_cache_key_ref() == other.as_cache_key_ref()
    }
}

impl Eq for dyn AsCacheKeyRef + '_ {}

impl Hash for dyn AsCacheKeyRef + '_ {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_cache_key_ref().hash(state);
    }
}

impl AsCacheKeyRef for CacheKey {
    fn as_cache_key_ref(&self) -> CacheKeyRef<'_> {
        CacheKeyRef {
            text: &self.text,
            font_size: self.font_size,
            runs: self.runs.as_slice(),
            wrap_width: self.wrap_width,
        }
    }
}

impl PartialEq for CacheKey {
    fn eq(&self, other: &Self) -> bool {
        self.as_cache_key_ref().eq(&other.as_cache_key_ref())
    }
}

impl Hash for CacheKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_cache_key_ref().hash(state);
    }
}

impl<'a> Borrow<dyn AsCacheKeyRef + 'a> for Arc<CacheKey> {
    fn borrow(&self) -> &(dyn AsCacheKeyRef + 'a) {
        self.as_ref() as &dyn AsCacheKeyRef
    }
}

impl AsCacheKeyRef for CacheKeyRef<'_> {
    fn as_cache_key_ref(&self) -> CacheKeyRef<'_> {
        *self
    }
}

#[derive(Hash, Eq, PartialEq)]
struct FontIdWithSize {
    font_id: FontId,
    font_size: Pixels,
}

/// The degree of blackness or stroke thickness of a font (100.0..=900.0, 400.0 normal).
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub(crate) struct FontWeight(pub(crate) f32);

impl Default for FontWeight {
    #[inline]
    fn default() -> FontWeight {
        FontWeight::NORMAL
    }
}

impl Hash for FontWeight {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_u32(u32::from_be_bytes(self.0.to_be_bytes()));
    }
}

impl Eq for FontWeight {}

impl FontWeight {
    /// Normal (400).
    pub(crate) const NORMAL: FontWeight = FontWeight(400.0);
    /// Semibold weight (600).
    pub(crate) const SEMIBOLD: FontWeight = FontWeight(600.0);
}

/// Allows italic or oblique faces to be selected.
#[derive(Clone, Copy, Eq, PartialEq, Debug, Hash, Default)]
pub(crate) enum FontStyle {
    /// A face that is neither italic nor oblique.
    #[default]
    Normal,
    /// A form that is generally cursive in nature.
    Italic,
    /// A typically-sloped version of the regular face.
    Oblique,
}

/// A styled run of text.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub(crate) struct TextRun {
    /// Length in UTF-8 bytes.
    pub(crate) len: usize,
    /// The font to use for this run.
    pub(crate) font: Font,
    /// The text color.
    pub(crate) color: Hsla,
    /// The underline style (if any).
    pub(crate) underline: Option<UnderlineStyle>,
}

/// An identifier for a specific glyph, as returned by [`DirectWriteTextSystem::layout_line`].
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
#[repr(C)]
pub(crate) struct GlyphId(pub(crate) u32);

/// Parameters for rendering a glyph, used as cache keys for raster bounds and atlas entries.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RenderGlyphParams {
    pub(crate) font_id: FontId,
    pub(crate) glyph_id: GlyphId,
    pub(crate) font_size: Pixels,
    pub(crate) subpixel_variant: Point<u8>,
    pub(crate) scale_factor: f32,
    pub(crate) is_emoji: bool,
    pub(crate) dilation: u8,
}

impl Eq for RenderGlyphParams {}

impl Hash for RenderGlyphParams {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.font_id.0.hash(state);
        self.glyph_id.0.hash(state);
        self.font_size.0.to_bits().hash(state);
        self.subpixel_variant.hash(state);
        self.scale_factor.to_bits().hash(state);
        self.is_emoji.hash(state);
        self.dilation.hash(state);
    }
}

/// The configuration details for identifying a specific font.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub(crate) struct Font {
    /// The font family name (`.SystemUIFont` selects the Windows UI font).
    pub(crate) family: SharedString,
    /// The font weight.
    pub(crate) weight: FontWeight,
    /// The font style.
    pub(crate) style: FontStyle,
}

impl Default for Font {
    fn default() -> Self {
        font(".SystemUIFont")
    }
}

/// Get a [`Font`] for a given family name.
pub(crate) fn font(family: impl Into<SharedString>) -> Font {
    Font {
        family: family.into(),
        weight: FontWeight::default(),
        style: FontStyle::default(),
    }
}

/// Font metrics used to compute glyph bounding boxes.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FontMetrics {
    pub(crate) units_per_em: u32,
    pub(crate) bounding_box: Bounds<f32>,
}

impl FontMetrics {
    pub(crate) fn bounding_box(&self, font_size: Pixels) -> Bounds<Pixels> {
        (self.bounding_box / self.units_per_em as f32 * font_size.0).map(px)
    }
}
