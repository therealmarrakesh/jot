mod font_fallbacks;
mod font_features;
mod line;
mod line_layout;
mod line_wrapper;

pub(crate) use font_fallbacks::*;
pub(crate) use font_features::*;
pub(crate) use line::*;
pub(crate) use line_layout::*;
pub(crate) use line_wrapper::*;

use crate::ui::anyhow::anyhow;
use crate::ui::collections::FxHashMap;
use crate::win32::DirectWriteTextSystem;
use crate::ui::smallvec::{SmallVec, smallvec};
use crate::ui::sync::{Mutex, RwLock};
use crate::ui::{
    Bounds, DevicePixels, Hsla, Pixels, Point, Result, SharedString, Size, UnderlineStyle, px,
};
use core::fmt;
use std::{
    cmp,
    fmt::{Debug, Display, Formatter},
    hash::{Hash, Hasher},
    ops::{Deref, DerefMut},
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

// Leave enough room below the underline for its stroke while keeping it below the baseline.
const UNDERLINE_DESCENT_OFFSET_FACTOR: f32 = 0.618;

/// Returns the vertical offset used to paint an underline within a line.
pub(crate) fn underline_y_offset(line_height: Pixels, ascent: Pixels, descent: Pixels) -> Pixels {
    let padding_top = (line_height - ascent - descent) / 2.;
    padding_top + ascent + descent * UNDERLINE_DESCENT_OFFSET_FACTOR
}

/// The GPUI text rendering subsystem.
pub(crate) struct TextSystem {
    platform_text_system: Arc<DirectWriteTextSystem>,
    font_ids_by_font: RwLock<FxHashMap<Font, Result<FontId>>>,
    font_metrics: RwLock<FxHashMap<FontId, FontMetrics>>,
    raster_bounds: RwLock<FxHashMap<RenderGlyphParams, Bounds<DevicePixels>>>,
    wrapper_pool: Mutex<FxHashMap<FontIdWithSize, Vec<LineWrapper>>>,
    font_runs_pool: Mutex<Vec<Vec<FontRun>>>,
    fallback_font_stack: SmallVec<[Font; 2]>,
}

impl TextSystem {
    /// Create a new TextSystem with the given DirectWrite text system.
    pub(crate) fn new(platform_text_system: Arc<DirectWriteTextSystem>) -> Self {
        TextSystem {
            platform_text_system,
            font_metrics: RwLock::default(),
            raster_bounds: RwLock::default(),
            font_ids_by_font: RwLock::default(),
            wrapper_pool: Mutex::default(),
            font_runs_pool: Mutex::default(),
            fallback_font_stack: smallvec![font("Segoe UI"), font("Arial")],
        }
    }

    /// Get the FontId for the configure font family and style.
    fn font_id(&self, font: &Font) -> Result<FontId> {
        fn clone_font_id_result(font_id: &Result<FontId>) -> Result<FontId> {
            match font_id {
                Ok(font_id) => Ok(*font_id),
                Err(err) => Err(anyhow!("{err}")),
            }
        }

        let font_id = self
            .font_ids_by_font
            .read()
            .get(font)
            .map(clone_font_id_result);
        if let Some(font_id) = font_id {
            font_id
        } else {
            let font_id = self.platform_text_system.font_id(font);
            self.font_ids_by_font
                .write()
                .insert(font.clone(), clone_font_id_result(&font_id));
            font_id
        }
    }

    /// Resolves the specified font, falling back to the default font stack if
    /// the font fails to load.
    ///
    /// # Panics
    ///
    /// Panics if the font and none of the fallbacks can be resolved.
    pub(crate) fn resolve_font(&self, font: &Font) -> FontId {
        if let Ok(font_id) = self.font_id(font) {
            return font_id;
        }
        for fallback in &self.fallback_font_stack {
            if let Ok(font_id) = self.font_id(fallback) {
                return font_id;
            }
        }

        panic!(
            "failed to resolve font '{}' or any of the fallbacks: {}",
            font.family,
            self.fallback_font_stack
                .iter()
                .map(|fallback| fallback.family.as_ref())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    /// Get the bounding box for the given font and font size.
    /// A font's bounding box is the smallest rectangle that could enclose all glyphs
    /// in the font. superimposed over one another.
    pub(crate) fn bounding_box(&self, font_id: FontId, font_size: Pixels) -> Bounds<Pixels> {
        self.read_metrics(font_id, |metrics| metrics.bounding_box(font_size))
    }

    /// Returns the shaped layout width of for the given character, in the given font and size.
    pub(crate) fn layout_width(&self, font_id: FontId, font_size: Pixels, ch: char) -> Pixels {
        let mut buffer = [0; 4];
        let buffer = ch.encode_utf8(&mut buffer);
        self.platform_text_system
            .layout_line(
                buffer,
                font_size,
                &[FontRun {
                    len: buffer.len(),
                    font_id,
                }],
            )
            .width
    }

    fn read_metrics<T>(&self, font_id: FontId, read: impl FnOnce(&FontMetrics) -> T) -> T {
        if let Some(metrics) = self.font_metrics.read().get(&font_id) {
            return read(metrics);
        }
        let mut lock = self.font_metrics.write();
        let metrics = lock
            .entry(font_id)
            .or_insert_with(|| self.platform_text_system.font_metrics(font_id));
        read(metrics)
    }

    /// Returns a handle to a line wrapper, for the given font and font size.
    pub(crate) fn line_wrapper(self: &Arc<Self>, font: Font, font_size: Pixels) -> LineWrapperHandle {
        let lock = &mut self.wrapper_pool.lock();
        let font_id = self.resolve_font(&font);
        let wrappers = lock
            .entry(FontIdWithSize { font_id, font_size })
            .or_default();
        let wrapper = wrappers
            .pop()
            .unwrap_or_else(|| LineWrapper::new(font_id, font_size, self.clone()));

        LineWrapperHandle {
            wrapper: Some(wrapper),
            text_system: self.clone(),
        }
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

/// The GPUI text layout subsystem.
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

    /// Shape the given line, at the given font_size, for painting to the screen.
    /// Subsets of the line can be styled independently with the `runs` parameter.
    ///
    /// Note that this method can only shape a single line of text. It will panic
    /// if the text contains newlines. If you need to shape multiple lines of text,
    /// use [`Self::shape_text`] instead.
    pub(crate) fn shape_line(
        &self,
        text: SharedString,
        font_size: Pixels,
        runs: &[TextRun],
        force_width: Option<Pixels>,
    ) -> ShapedLine {
        debug_assert!(
            text.find('\n').is_none(),
            "text argument should not contain newlines"
        );

        let mut decoration_runs = SmallVec::<[DecorationRun; 32]>::new();
        for run in runs {
            if let Some(last_run) = decoration_runs.last_mut()
                && last_run.color == run.color
                && last_run.underline == run.underline
                && last_run.background_color == run.background_color
            {
                last_run.len += run.len as u32;
                continue;
            }
            decoration_runs.push(DecorationRun {
                len: run.len as u32,
                color: run.color,
                background_color: run.background_color,
                underline: run.underline,
            });
        }

        let layout = self.layout_line(&text, font_size, runs, force_width);

        ShapedLine {
            layout,
            decoration_runs,
        }
    }

    /// Shape a multi line string of text, at the given font_size, for painting to the screen.
    /// Subsets of the text can be styled independently with the `runs` parameter.
    /// If `wrap_width` is provided, the line breaks will be adjusted to fit within the given width.
    pub(crate) fn shape_text(
        &self,
        text: SharedString,
        font_size: Pixels,
        runs: &[TextRun],
        wrap_width: Option<Pixels>,
        line_clamp: Option<usize>,
    ) -> Result<SmallVec<[WrappedLine; 1]>> {
        let mut runs = runs.iter().filter(|run| run.len > 0).cloned().peekable();
        let mut font_runs = self.font_runs_pool.lock().pop().unwrap_or_default();

        let mut lines = SmallVec::new();
        let max_wrap_lines = line_clamp;
        let mut wrapped_lines = 0;

        let mut process_line = |line_text: SharedString, line_start, line_end| {
            font_runs.clear();

            let mut decoration_runs = <Vec<DecorationRun>>::with_capacity(32);
            let mut run_start = line_start;
            while run_start < line_end {
                let Some(run) = runs.peek_mut() else {
                    crate::ui::log::warn!("`TextRun`s do not cover the entire to be shaped text");
                    break;
                };

                let run_len_within_line = cmp::min(line_end - run_start, run.len);

                let decoration_changed = if let Some(last_run) = decoration_runs.last_mut()
                    && last_run.color == run.color
                    && last_run.underline == run.underline
                    && last_run.background_color == run.background_color
                {
                    last_run.len += run_len_within_line as u32;
                    false
                } else {
                    decoration_runs.push(DecorationRun {
                        len: run_len_within_line as u32,
                        color: run.color,
                        background_color: run.background_color,
                        underline: run.underline,
                    });
                    true
                };

                let font_id = self.resolve_font(&run.font);
                if let Some(font_run) = font_runs.last_mut()
                    && font_id == font_run.font_id
                    && !decoration_changed
                {
                    font_run.len += run_len_within_line;
                } else {
                    font_runs.push(FontRun {
                        len: run_len_within_line,
                        font_id,
                    });
                }

                // Preserve the remainder of the run for the next line
                run.len -= run_len_within_line;
                if run.len == 0 {
                    runs.next();
                }
                run_start += run_len_within_line;
            }

            let layout = self.line_layout_cache.layout_wrapped_line(
                &line_text,
                font_size,
                &font_runs,
                wrap_width,
                max_wrap_lines.map(|max| max.saturating_sub(wrapped_lines)),
            );
            wrapped_lines += layout.wrap_boundaries.len();

            lines.push(WrappedLine {
                layout,
                decoration_runs,
            });

            // Skip `\n` character.
            if let Some(run) = runs.peek_mut() {
                run.len -= 1;
                if run.len == 0 {
                    runs.next();
                }
            }
        };

        let mut split_lines = text.split('\n');

        // Special case single lines to prevent allocating a sharedstring
        if let Some(first_line) = split_lines.next()
            && let Some(second_line) = split_lines.next()
        {
            let mut line_start = 0;
            process_line(
                SharedString::new(first_line),
                line_start,
                line_start + first_line.len(),
            );
            line_start += first_line.len() + '\n'.len_utf8();
            process_line(
                SharedString::new(second_line),
                line_start,
                line_start + second_line.len(),
            );
            for line_text in split_lines {
                line_start += line_text.len() + '\n'.len_utf8();
                process_line(
                    SharedString::new(line_text),
                    line_start,
                    line_start + line_text.len(),
                );
            }
        } else {
            let end = text.len();
            process_line(text, 0, end);
        }

        self.font_runs_pool.lock().push(font_runs);

        Ok(lines)
    }

    pub(crate) fn finish_frame(&self) {
        self.line_layout_cache.finish_frame()
    }

    /// Layout the given line of text, at the given font_size.
    /// Subsets of the line can be styled independently with the `runs` parameter.
    /// Generally, you should prefer to use [`Self::shape_line`] instead, which
    /// can be painted directly.
    pub(crate) fn layout_line(
        &self,
        text: &str,
        font_size: Pixels,
        runs: &[TextRun],
        force_width: Option<Pixels>,
    ) -> Arc<LineLayout> {
        let mut last_run = None::<&TextRun>;
        let mut font_runs = self.font_runs_pool.lock().pop().unwrap_or_default();
        font_runs.clear();

        for run in runs.iter() {
            let decoration_changed = if let Some(last_run) = last_run
                && last_run.color == run.color
                && last_run.underline == run.underline
            // we do not consider differing background color relevant, as it does not affect glyphs
            // && last_run.background_color == run.background_color
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

        let layout = self.line_layout_cache.layout_line(
            SharedString::new(text),
            font_size,
            &font_runs,
            force_width,
        );

        self.font_runs_pool.lock().push(font_runs);

        layout
    }

    /// Returns the shaped layout width of for the given character, in the given font and size.
    pub(crate) fn layout_width(&self, font_id: FontId, font_size: Pixels, ch: char) -> Pixels {
        let mut buffer = [0; 4];
        let buffer: &_ = ch.encode_utf8(&mut buffer);
        self.line_layout_cache
            .layout_line(
                buffer,
                font_size,
                &[FontRun {
                    len: buffer.len(),
                    font_id,
                }],
                None,
            )
            .width
    }
}

#[derive(Hash, Eq, PartialEq)]
struct FontIdWithSize {
    font_id: FontId,
    font_size: Pixels,
}

/// A handle into the text system, which can be used to compute the wrapped layout of text
pub(crate) struct LineWrapperHandle {
    wrapper: Option<LineWrapper>,
    text_system: Arc<TextSystem>,
}

impl Drop for LineWrapperHandle {
    fn drop(&mut self) {
        let mut state = self.text_system.wrapper_pool.lock();
        let wrapper = self.wrapper.take().unwrap();
        state
            .get_mut(&FontIdWithSize {
                font_id: wrapper.font_id,
                font_size: wrapper.font_size,
            })
            .unwrap()
            .push(wrapper);
    }
}

impl Deref for LineWrapperHandle {
    type Target = LineWrapper;

    fn deref(&self) -> &Self::Target {
        self.wrapper.as_ref().unwrap()
    }
}

impl DerefMut for LineWrapperHandle {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.wrapper.as_mut().unwrap()
    }
}

/// The degree of blackness or stroke thickness of a font. This value ranges from 100.0 to 900.0,
/// with 400.0 as normal.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub(crate) struct FontWeight(pub(crate) f32);

impl std::ops::Add for FontWeight {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self(self.0 + rhs.0)
    }
}

impl std::ops::Sub for FontWeight {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self(self.0 - rhs.0)
    }
}

impl std::str::FromStr for FontWeight {
    type Err = std::num::ParseFloatError;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        s.parse::<f32>().map(Self)
    }
}

impl Display for FontWeight {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<f32> for FontWeight {
    fn from(weight: f32) -> Self {
        FontWeight(weight)
    }
}

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
    /// A face that is neither italic not obliqued.
    #[default]
    Normal,
    /// A form that is generally cursive in nature.
    Italic,
    /// A typically-sloped version of the regular face.
    Oblique,
}

impl Display for FontStyle {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        Debug::fmt(self, f)
    }
}

/// A styled run of text, for use in [`crate::ui::TextLayout`].
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub(crate) struct TextRun {
    /// A number of utf8 bytes
    pub(crate) len: usize,
    /// The font to use for this run.
    pub(crate) font: Font,
    /// The color
    pub(crate) color: Hsla,
    /// The background color (if any)
    pub(crate) background_color: Option<Hsla>,
    /// The underline style (if any)
    pub(crate) underline: Option<UnderlineStyle>,
}

/// An identifier for a specific glyph, as returned by [`WindowTextSystem::layout_line`].
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
#[repr(C)]
pub(crate) struct GlyphId(pub(crate) u32);

/// Parameters for rendering a glyph, used as cache keys for raster bounds.
///
/// This struct identifies a specific glyph rendering configuration including
/// font, size, subpixel positioning, and scale factor. It's used to look up
/// cached raster bounds and sprite atlas entries.
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
    /// The font family name.
    ///
    /// The special name ".SystemUIFont" is used to identify the system UI font, which varies based on platform.
    pub(crate) family: SharedString,

    /// The font features to use.
    pub(crate) features: FontFeatures,

    /// The fallbacks fonts to use.
    pub(crate) fallbacks: Option<FontFallbacks>,

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

/// Get a [`Font`] for a given name.
pub(crate) fn font(family: impl Into<SharedString>) -> Font {
    Font {
        family: family.into(),
        features: FontFeatures::default(),
        weight: FontWeight::default(),
        style: FontStyle::default(),
        fallbacks: None,
    }
}

/// A struct for storing font metrics.
/// It is used to define the measurements of a typeface.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FontMetrics {
    /// The number of font units that make up the "em square",
    /// a scalable grid for determining the size of a typeface.
    pub(crate) units_per_em: u32,

    /// The outer limits of the area that the font covers.
    /// Corresponds to the xMin / xMax / yMin / yMax values in the OpenType `head` table
    pub(crate) bounding_box: Bounds<f32>,
}

impl FontMetrics {
    /// Returns the outer limits of the area that the font covers in pixels.
    pub(crate) fn bounding_box(&self, font_size: Pixels) -> Bounds<Pixels> {
        (self.bounding_box / self.units_per_em as f32 * font_size.0).map(px)
    }
}

/// Maps well-known virtual font names to their concrete equivalents.
pub(crate) fn font_name_with_fallbacks_shared<'a>(
    name: &'a SharedString,
    system: &'a SharedString,
) -> &'a SharedString {
    match name.as_str() {
        ".SystemUIFont" => system,
        _ => name,
    }
}
