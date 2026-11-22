use crate::ui::{
    App, Bounds, Half, Hsla, LineLayout, Pixels, Point, Result, TextAlign, UnderlineStyle, Window,
    WrapBoundary, WrappedLineLayout, black, fill, point, px, size, underline_y_offset,
};
use crate::ui::smallvec::SmallVec;
use std::{ops::Range, sync::Arc};

/// Set the text decoration for a run of text.
#[derive(Debug, Clone)]
pub(crate) struct DecorationRun {
    /// The length of the run in utf-8 bytes.
    pub(crate) len: u32,

    /// The color for this run
    pub(crate) color: Hsla,

    /// The background color for this run
    pub(crate) background_color: Option<Hsla>,

    /// The underline style for this run
    pub(crate) underline: Option<UnderlineStyle>,
}

/// A line of text that has been shaped and decorated.
#[derive(Clone, Default, Debug)]
pub(crate) struct ShapedLine {
    pub(crate) layout: Arc<LineLayout>,
    pub(crate) decoration_runs: SmallVec<[DecorationRun; 32]>,
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
    /// Override the len (used by display-map unit tests).
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
        self.paint_with_underline_handler(
            origin,
            line_height,
            align,
            align_width,
            window,
            cx,
            |_, origin, width, style, window| window.paint_underline(origin, width, style),
        )
    }

    /// Paint the line with a handler for each underline.
    pub(crate) fn paint_with_underline_handler(
        &self,
        origin: Point<Pixels>,
        line_height: Pixels,
        align: TextAlign,
        align_width: Option<Pixels>,
        window: &mut Window,
        cx: &mut App,
        mut paint_underline: impl FnMut(
            Range<usize>,
            Point<Pixels>,
            Pixels,
            &UnderlineStyle,
            &mut Window,
        ),
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
            &mut paint_underline,
        )
    }
}

/// A line of text that has been shaped, decorated, and wrapped by the text layout system.
#[derive(Default, Debug)]
pub(crate) struct WrappedLine {
    pub(crate) layout: Arc<WrappedLineLayout>,
    pub(crate) decoration_runs: Vec<DecorationRun>,
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
    /// Paint this line of text to the window.
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
            &mut |_, origin, width, style, window| window.paint_underline(origin, width, style),
        )?;

        Ok(())
    }

    /// Paint the background of line of text to the window.
    pub(crate) fn paint_background(
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

        paint_line_background(
            origin,
            &self.layout.unwrapped_layout,
            line_height,
            align,
            align_width,
            &self.decoration_runs,
            &self.wrap_boundaries,
            window,
            cx,
        )?;

        Ok(())
    }
}

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
    paint_underline: &mut dyn FnMut(
        Range<usize>,
        Point<Pixels>,
        Pixels,
        &UnderlineStyle,
        &mut Window,
    ),
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
        let mut current_underline: Option<(Point<Pixels>, UnderlineStyle, Range<usize>)> = None;
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
                    if let Some((underline_origin, underline_style, underline_range)) =
                        current_underline.as_mut()
                    {
                        if glyph_origin.x == underline_origin.x {
                            underline_origin.x -= max_glyph_size.width.half();
                        };
                        paint_underline(
                            underline_range.clone(),
                            *underline_origin,
                            glyph_origin.x - underline_origin.x,
                            underline_style,
                            window,
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

                let mut finished_underline: Option<(Point<Pixels>, UnderlineStyle, Range<usize>)> =
                    None;
                if glyph.index >= run_end {
                    let mut style_run = decoration_runs.next();

                    // ignore style runs that apply to a partial glyph
                    while let Some(run) = style_run {
                        if glyph.index < run_end + (run.len as usize) {
                            break;
                        }
                        run_end += run.len as usize;
                        style_run = decoration_runs.next();
                    }

                    if let Some(style_run) = style_run {
                        let style_run_start = run_end;
                        if let Some((_, underline_style, underline_range)) = &mut current_underline
                        {
                            if style_run.underline.as_ref() != Some(underline_style) {
                                finished_underline = current_underline.take();
                            } else {
                                underline_range.end = style_run_start + style_run.len as usize;
                            }
                        }
                        if let Some(run_underline) = style_run.underline.as_ref() {
                            current_underline.get_or_insert((
                                point(glyph_origin.x, glyph_origin.y + underline_y_offset),
                                UnderlineStyle {
                                    color: Some(run_underline.color.unwrap_or(style_run.color)),
                                    thickness: run_underline.thickness,
                                },
                                style_run_start..style_run_start + style_run.len as usize,
                            ));
                        }

                        run_end += style_run.len as usize;
                        color = style_run.color;
                    } else {
                        run_end = layout.len;
                        finished_underline = current_underline.take();
                    }
                }

                if let Some((mut underline_origin, underline_style, underline_range)) =
                    finished_underline
                {
                    if underline_origin.x == glyph_origin.x {
                        underline_origin.x -= max_glyph_size.width.half();
                    };
                    paint_underline(
                        underline_range,
                        underline_origin,
                        glyph_origin.x - underline_origin.x,
                        &underline_style,
                        window,
                    );
                }

                let max_glyph_bounds = Bounds {
                    origin: glyph_origin,
                    size: max_glyph_size,
                };

                let content_mask = window.content_mask();
                if max_glyph_bounds.intersects(&content_mask.bounds) {
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

        if let Some((mut underline_start, underline_style, underline_range)) =
            current_underline.take()
        {
            if last_line_end_x == underline_start.x {
                underline_start.x -= max_glyph_size.width.half()
            };
            paint_underline(
                underline_range,
                underline_start,
                last_line_end_x - underline_start.x,
                &underline_style,
                window,
            );
        }

        Ok(())
    })
}

fn paint_line_background(
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
        let mut decoration_runs = decoration_runs.iter();
        let mut wraps = wrap_boundaries.iter().peekable();
        let mut run_end = 0;
        let mut current_background: Option<(Point<Pixels>, Hsla)> = None;
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
        for (run_ix, run) in layout.runs.iter().enumerate() {
            max_glyph_size = text_system.bounding_box(run.font_id, layout.font_size).size;

            for (glyph_ix, glyph) in run.glyphs.iter().enumerate() {
                glyph_origin.x += glyph.position.x - prev_glyph_position.x;

                if wraps.peek() == Some(&&WrapBoundary { run_ix, glyph_ix }) {
                    wraps.next();
                    if let Some((background_origin, background_color)) = current_background.as_mut()
                    {
                        if glyph_origin.x == background_origin.x {
                            background_origin.x -= max_glyph_size.width.half()
                        }
                        window.paint_quad(fill(
                            Bounds {
                                origin: *background_origin,
                                size: size(glyph_origin.x - background_origin.x, line_height),
                            },
                            *background_color,
                        ));
                        if glyph.index < run_end {
                            background_origin.x = origin.x;
                            background_origin.y += line_height;
                        } else {
                            current_background = None;
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

                let mut finished_background: Option<(Point<Pixels>, Hsla)> = None;
                if glyph.index >= run_end {
                    let mut style_run = decoration_runs.next();

                    // ignore style runs that apply to a partial glyph
                    while let Some(run) = style_run {
                        if glyph.index < run_end + (run.len as usize) {
                            break;
                        }
                        run_end += run.len as usize;
                        style_run = decoration_runs.next();
                    }

                    if let Some(style_run) = style_run {
                        if let Some((_, background_color)) = &mut current_background
                            && style_run.background_color.as_ref() != Some(background_color)
                        {
                            finished_background = current_background.take();
                        }
                        if let Some(run_background) = style_run.background_color {
                            current_background.get_or_insert((
                                point(glyph_origin.x, glyph_origin.y),
                                run_background,
                            ));
                        }
                        run_end += style_run.len as usize;
                    } else {
                        run_end = layout.len;
                        finished_background = current_background.take();
                    }
                }

                if let Some((mut background_origin, background_color)) = finished_background {
                    let width = glyph_origin.x - background_origin.x;
                    if background_origin.x == glyph_origin.x {
                        background_origin.x -= max_glyph_size.width.half();
                    };
                    window.paint_quad(fill(
                        Bounds {
                            origin: background_origin,
                            size: size(width, line_height),
                        },
                        background_color,
                    ));
                }
            }
        }

        let mut last_line_end_x = origin.x + layout.width;
        if let Some(boundary) = wrap_boundaries.last() {
            let run = &layout.runs[boundary.run_ix];
            let glyph = &run.glyphs[boundary.glyph_ix];
            last_line_end_x -= glyph.position.x;
        }

        if let Some((mut background_origin, background_color)) = current_background.take() {
            if last_line_end_x == background_origin.x {
                background_origin.x -= max_glyph_size.width.half()
            };
            window.paint_quad(fill(
                Bounds {
                    origin: background_origin,
                    size: size(last_line_end_x - background_origin.x, line_height),
                },
                background_color,
            ));
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
