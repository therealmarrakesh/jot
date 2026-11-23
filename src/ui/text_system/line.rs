use crate::ui::smallvec::SmallVec;
use crate::ui::{
    App, Bounds, FontId, GlyphId, Half, Pixels, Point, Result, Rgba, Size, TextAlign,
    UnderlineStyle, Window, black, point, px, size, underline_y_offset,
};
use std::sync::Arc;

use super::is_break_opportunity;

/// A laid out line of shaped glyphs.
#[derive(Default, Debug)]
pub(crate) struct LineLayout {
    pub(crate) font_size: Pixels,
    pub(crate) width: Pixels,
    pub(crate) ascent: Pixels,
    pub(crate) descent: Pixels,
    pub(crate) runs: Vec<ShapedRun>,
    pub(crate) len: usize,
}

/// A run of text that has been shaped with one font.
#[derive(Debug, Clone)]
pub(crate) struct ShapedRun {
    pub(crate) font_id: FontId,
    pub(crate) glyphs: Vec<ShapedGlyph>,
}

/// A single glyph, ready to paint.
#[derive(Clone, Debug)]
pub(crate) struct ShapedGlyph {
    pub(crate) id: GlyphId,
    pub(crate) position: Point<Pixels>,
    pub(crate) index: usize,
    pub(crate) is_emoji: bool,
}

impl LineLayout {
    /// Returns the character byte boundary closest to `x`.
    pub(crate) fn closest_index_for_x(&self, x: Pixels) -> usize {
        let mut prev_index = 0;
        let mut prev_x = px(0.);

        for run in &self.runs {
            for glyph in &run.glyphs {
                if glyph.position.x >= x {
                    if glyph.position.x - x < x - prev_x {
                        return glyph.index;
                    } else {
                        return prev_index;
                    }
                }
                prev_index = glyph.index;
                prev_x = glyph.position.x;
            }
        }

        if self.len == 1 {
            if x > self.width / 2. {
                return 1;
            } else {
                return 0;
            }
        }

        self.len
    }

    /// The x position of the character at the given UTF-8 byte index.
    pub(crate) fn x_for_index(&self, index: usize) -> Pixels {
        for run in &self.runs {
            for glyph in &run.glyphs {
                if glyph.index >= index {
                    return glyph.position.x;
                }
            }
        }
        self.width
    }

    pub(super) fn compute_wrap_boundaries(
        &self,
        text: &str,
        wrap_width: Pixels,
        max_lines: Option<usize>,
    ) -> SmallVec<[WrapBoundary; 1]> {
        let mut boundaries = SmallVec::new();
        let mut has_non_space = false;
        let mut last_candidate_ix = None;
        let mut last_candidate_x = px(0.);
        let mut last_boundary = WrapBoundary {
            run_ix: 0,
            glyph_ix: 0,
        };
        let mut last_boundary_x = px(0.);
        let mut prev_ch = '\0';
        let mut glyphs = self
            .runs
            .iter()
            .enumerate()
            .flat_map(|(run_ix, run)| {
                run.glyphs.iter().enumerate().map(move |(glyph_ix, glyph)| {
                    let character = text[glyph.index..].chars().next().unwrap();
                    (WrapBoundary { run_ix, glyph_ix }, character, glyph.position.x)
                })
            })
            .peekable();

        while let Some((boundary, ch, x)) = glyphs.next() {
            if ch == '\n' {
                continue;
            }

            if is_break_opportunity(prev_ch, ch, has_non_space) {
                last_candidate_ix = Some(boundary);
                last_candidate_x = x;
            }

            if ch != ' ' {
                has_non_space = true;
            }

            let next_x = glyphs.peek().map_or(self.width, |(_, _, x)| *x);
            if next_x - last_boundary_x > wrap_width && boundary > last_boundary {
                if let Some(max_lines) = max_lines
                    && boundaries.len() >= max_lines.saturating_sub(1)
                {
                    break;
                }

                if let Some(candidate) = last_candidate_ix.take() {
                    last_boundary = candidate;
                    last_boundary_x = last_candidate_x;
                } else {
                    last_boundary = boundary;
                    last_boundary_x = x;
                }
                boundaries.push(last_boundary);
            }
            prev_ch = ch;
        }

        boundaries
    }
}

/// A boundary at which a shaped line was wrapped.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct WrapBoundary {
    pub(crate) run_ix: usize,
    pub(crate) glyph_ix: usize,
}

/// A shaped line layout paired with soft-wrap boundaries.
#[derive(Default, Debug)]
pub(crate) struct WrappedLineLayout {
    pub(crate) unwrapped_layout: Arc<LineLayout>,
    pub(crate) wrap_boundaries: SmallVec<[WrapBoundary; 1]>,
    pub(crate) wrap_width: Option<Pixels>,
}

impl WrappedLineLayout {
    pub(crate) fn width(&self) -> Pixels {
        self.wrap_width
            .unwrap_or(Pixels::MAX)
            .min(self.unwrapped_layout.width)
    }

    pub(crate) fn size(&self, line_height: Pixels) -> Size<Pixels> {
        Size {
            width: self.width(),
            height: line_height * (self.wrap_boundaries.len() + 1),
        }
    }

    pub(crate) fn wrap_boundaries(&self) -> &[WrapBoundary] {
        &self.wrap_boundaries
    }
}

/// Color and underline decoration for a UTF-8 byte run of text.
#[derive(Debug, Clone)]
pub(crate) struct DecorationRun {
    pub(crate) len: u32,
    pub(crate) color: Rgba,
    pub(crate) underline: Option<UnderlineStyle>,
}

/// A line of text that has been shaped and decorated.
#[derive(Clone, Default, Debug)]
pub(crate) struct ShapedLine {
    pub(crate) layout: Arc<LineLayout>,
    pub(crate) decoration_runs: SmallVec<[DecorationRun; 1]>,
}

impl std::ops::Deref for ShapedLine {
    type Target = Arc<LineLayout>;
    fn deref(&self) -> &Self::Target {
        &self.layout
    }
}

impl std::ops::DerefMut for ShapedLine {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.layout
    }
}

impl ShapedLine {
    /// Override the len (used by text-wrapper unit tests).
    #[cfg(test)]
    pub(crate) fn with_len(mut self, len: usize) -> Self {
        let layout = self.layout.as_ref();
        self.layout = Arc::new(LineLayout {
            font_size: layout.font_size,
            width: layout.width,
            ascent: layout.ascent,
            descent: layout.descent,
            runs: layout.runs.clone(),
            len,
        });
        self
    }

    /// Paint the line of text to the window.
    pub(crate) fn paint(
        &self,
        origin: Point<Pixels>,
        line_height: Pixels,
        align: TextAlign,
        align_width: Option<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<()> {
        paint_line(
            origin,
            &self.layout,
            line_height,
            align,
            align_width,
            &self.decoration_runs,
            &[],
            window,
            cx,
        )
    }
}

/// A line of text that has been shaped, decorated, and wrapped by the text layout system.
#[derive(Default, Debug)]
pub(crate) struct WrappedLine {
    pub(crate) layout: Arc<WrappedLineLayout>,
    pub(crate) decoration_runs: SmallVec<[DecorationRun; 1]>,
}

impl std::ops::Deref for WrappedLine {
    type Target = Arc<WrappedLineLayout>;
    fn deref(&self) -> &Self::Target {
        &self.layout
    }
}

impl std::ops::DerefMut for WrappedLine {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.layout
    }
}

impl WrappedLine {
    /// Paint this wrapped line of text to the window.
    pub(crate) fn paint(
        &self,
        origin: Point<Pixels>,
        line_height: Pixels,
        align: TextAlign,
        bounds: Option<Bounds<Pixels>>,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<()> {
        let align_width = match bounds {
            Some(bounds) => Some(bounds.size.width),
            None => self.layout.wrap_width,
        };

        paint_line(
            origin,
            &self.layout.unwrapped_layout,
            line_height,
            align,
            align_width,
            &self.decoration_runs,
            &self.wrap_boundaries,
            window,
            cx,
        )
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_line(
    origin: Point<Pixels>,
    layout: &LineLayout,
    line_height: Pixels,
    align: TextAlign,
    align_width: Option<Pixels>,
    decoration_runs: &[DecorationRun],
    wrap_boundaries: &[WrapBoundary],
    window: &mut Window,
    cx: &mut App,
) -> Result<()> {
    let line_bounds = line_paint_bounds(
        origin,
        layout,
        line_height,
        align,
        align_width,
        wrap_boundaries,
    );
    window.paint_layer(line_bounds, |window| {
        let padding_top = (line_height - layout.ascent - layout.descent) / 2.;
        let baseline_offset = point(px(0.), padding_top + layout.ascent);
        let underline_y_offset = underline_y_offset(line_height, layout.ascent, layout.descent);
        let mut decoration_runs = decoration_runs.iter();
        let mut wraps = wrap_boundaries.iter().peekable();
        let mut run_end = 0;
        let mut color = black();
        let mut current_underline: Option<(Point<Pixels>, UnderlineStyle)> = None;
        let text_system = cx.text_system().clone();
        let mut glyph_origin = point(
            aligned_origin_x(
                origin,
                align_width.unwrap_or(layout.width),
                px(0.0),
                &align,
                layout,
                wraps.peek(),
            ),
            origin.y,
        );
        let mut prev_glyph_position = Point::default();
        let mut max_glyph_size = size(px(0.), px(0.));
        let mut first_glyph_x = origin.x;
        for (run_ix, run) in layout.runs.iter().enumerate() {
            max_glyph_size = text_system.bounding_box(run.font_id, layout.font_size).size;

            for (glyph_ix, glyph) in run.glyphs.iter().enumerate() {
                glyph_origin.x += glyph.position.x - prev_glyph_position.x;
                if glyph_ix == 0 && run_ix == 0 {
                    first_glyph_x = glyph_origin.x;
                }

                if wraps.peek() == Some(&&WrapBoundary { run_ix, glyph_ix }) {
                    wraps.next();
                    if let Some((underline_origin, underline_style)) = current_underline.as_mut() {
                        if glyph_origin.x == underline_origin.x {
                            underline_origin.x -= max_glyph_size.width.half();
                        }
                        window.paint_underline(
                            *underline_origin,
                            glyph_origin.x - underline_origin.x,
                            underline_style,
                        );
                        if glyph.index < run_end {
                            underline_origin.x = origin.x;
                            underline_origin.y += line_height;
                        } else {
                            current_underline = None;
                        }
                    }

                    glyph_origin.x = aligned_origin_x(
                        origin,
                        align_width.unwrap_or(layout.width),
                        glyph.position.x,
                        &align,
                        layout,
                        wraps.peek(),
                    );
                    glyph_origin.y += line_height;
                }
                prev_glyph_position = glyph.position;

                let mut finished_underline: Option<(Point<Pixels>, UnderlineStyle)> = None;
                if glyph.index >= run_end {
                    let mut style_run = decoration_runs.next();
                    while let Some(run) = style_run {
                        if glyph.index < run_end + (run.len as usize) {
                            break;
                        }
                        run_end += run.len as usize;
                        style_run = decoration_runs.next();
                    }

                    if let Some(style_run) = style_run {
                        if let Some((_, underline_style)) = &current_underline
                            && style_run.underline.as_ref() != Some(underline_style)
                        {
                            finished_underline = current_underline.take();
                        }
                        if let Some(run_underline) = style_run.underline.as_ref() {
                            current_underline.get_or_insert((
                                point(glyph_origin.x, glyph_origin.y + underline_y_offset),
                                UnderlineStyle {
                                    color: Some(run_underline.color.unwrap_or(style_run.color)),
                                    thickness: run_underline.thickness,
                                },
                            ));
                        }

                        run_end += style_run.len as usize;
                        color = style_run.color;
                    } else {
                        run_end = layout.len;
                        finished_underline = current_underline.take();
                    }
                }

                if let Some((mut underline_origin, underline_style)) = finished_underline {
                    if underline_origin.x == glyph_origin.x {
                        underline_origin.x -= max_glyph_size.width.half();
                    }
                    window.paint_underline(
                        underline_origin,
                        glyph_origin.x - underline_origin.x,
                        &underline_style,
                    );
                }

                let max_glyph_bounds = Bounds {
                    origin: glyph_origin,
                    size: max_glyph_size,
                };
                if max_glyph_bounds.intersects(&window.content_mask().bounds) {
                    let vertical_offset = point(px(0.0), glyph.position.y);
                    if glyph.is_emoji {
                        window.paint_emoji(
                            glyph_origin + baseline_offset + vertical_offset,
                            run.font_id,
                            glyph.id,
                            layout.font_size,
                        )?;
                    } else {
                        window.paint_glyph(
                            glyph_origin + baseline_offset + vertical_offset,
                            run.font_id,
                            glyph.id,
                            layout.font_size,
                            color,
                        )?;
                    }
                }
            }
        }

        let mut last_line_end_x = first_glyph_x + layout.width;
        if let Some(boundary) = wrap_boundaries.last() {
            let run = &layout.runs[boundary.run_ix];
            let glyph = &run.glyphs[boundary.glyph_ix];
            last_line_end_x -= glyph.position.x;
        }

        if let Some((mut underline_start, underline_style)) = current_underline.take() {
            if last_line_end_x == underline_start.x {
                underline_start.x -= max_glyph_size.width.half();
            }
            window.paint_underline(
                underline_start,
                last_line_end_x - underline_start.x,
                &underline_style,
            );
        }

        Ok(())
    })
}

fn line_paint_bounds(
    origin: Point<Pixels>,
    layout: &LineLayout,
    line_height: Pixels,
    align: TextAlign,
    align_width: Option<Pixels>,
    wrap_boundaries: &[WrapBoundary],
) -> Bounds<Pixels> {
    let mut bounds = Bounds::new(
        origin,
        size(
            layout.width,
            line_height * (wrap_boundaries.len() as f32 + 1.),
        ),
    );
    if align == TextAlign::Left || layout.len == 0 {
        return bounds;
    }

    let align_width = align_width.unwrap_or(layout.width);
    let mut row_start = Pixels::ZERO;
    let row_ends = wrap_boundaries
        .iter()
        .map(|boundary| {
            layout.runs[boundary.run_ix].glyphs[boundary.glyph_ix]
                .position
                .x
        })
        .chain([layout.width]);
    for (row, row_end) in row_ends.enumerate() {
        let width = row_end - row_start;
        let offset = match align {
            TextAlign::Left => Pixels::ZERO,
            TextAlign::Center => (align_width - width) / 2.,
        };
        bounds = bounds.union(&Bounds::new(
            point(origin.x + offset, origin.y + line_height * row as f32),
            size(width, line_height),
        ));
        row_start = row_end;
    }
    bounds
}

fn aligned_origin_x(
    origin: Point<Pixels>,
    align_width: Pixels,
    last_glyph_x: Pixels,
    align: &TextAlign,
    layout: &LineLayout,
    wrap_boundary: Option<&&WrapBoundary>,
) -> Pixels {
    let end_of_line = if let Some(WrapBoundary { run_ix, glyph_ix }) = wrap_boundary {
        layout.runs[*run_ix].glyphs[*glyph_ix].position.x
    } else {
        layout.width
    };

    let line_width = end_of_line - last_glyph_x;
    match align {
        TextAlign::Left => origin.x,
        TextAlign::Center => (origin.x * 2.0 + align_width - line_width) / 2.0,
    }
}
