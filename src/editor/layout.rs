use std::borrow::Cow;
use std::ops::Range;

use crate::color::Rgba;
use crate::geom::{Point, Rect, point, size};
use crate::gfx::{Canvas, TextStyle, TextSystem};
use crate::theme::Palette;
use crate::types::SharedStr;

use super::buffer::RopeExt as _;
use super::frame::{
    EditorPaintParams, EditorPaintStyle, FrameLayout, RowLayout, VisibleLine, VisualRow,
};
use super::scroll::{caret_vertical_margin, clamp_horizontal_scroll, reveal_horizontal_scroll};
use super::wrap::LineRanges;
use super::{DocumentView, Editor, MoveDirection, RIGHT_MARGIN};

const HUGE_UNWRAPPED_LINE_BYTES: usize = 4096;
const HORIZONTAL_WINDOW_PAD: f32 = 256.0;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct LongestLineCache {
    revision: u64,
    row: usize,
    len: usize,
    style: TextStyle,
    wrap_width: Option<f32>,
}

struct VisibleSlice {
    lines: Range<usize>,
    top_rel: f32,
    shape_rows: Range<usize>,
}

#[inline]
fn snap_point(pt: Point, scale: f32) -> Point {
    point(
        (pt.x * scale).round() / scale,
        (pt.y * scale).round() / scale,
    )
}

fn content_width_with_margin(max_line_w: f32, viewport_width: f32) -> f32 {
    if max_line_w + RIGHT_MARGIN > viewport_width {
        max_line_w + RIGHT_MARGIN
    } else {
        max_line_w
    }
}

impl DocumentView {
    fn visible_slice(&self, scroll_y: f64, line_height: f32, viewport_height: f32) -> VisibleSlice {
        let total_rows = self.wrap.visual_row_count();
        let total_lines = self.wrap.line_count();
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

        let start_line = self.wrap.visual_row_to_line(first_row);
        let end_line = self.wrap.visual_row_to_line(last_row);
        let first_vis_row = self.wrap.first_visual_row(start_line);
        let top_rel = (first_vis_row as f64 * lh + scroll_y) as f32;
        VisibleSlice {
            lines: start_line..(end_line + 2).min(total_lines),
            top_rel,
            shape_rows,
        }
    }
}

impl Editor {
    pub(crate) fn prepare(
        &mut self,
        input_bounds: Rect,
        params: EditorPaintParams<'_>,
        ts: &TextSystem,
    ) -> bool {
        self.input_bounds = input_bounds;
        if self.doc.is_none() {
            self.prepare_field(input_bounds, params, ts);
            return false;
        }
        self.prepare_document(input_bounds, params, ts)
    }

    pub(crate) fn paint(&self, p: Palette, canvas: &mut Canvas) {
        if let Some(frame) = self.frame.as_ref() {
            frame.paint(p, canvas);
        }
    }

    fn finish_frame(
        &mut self,
        mut frame: FrameLayout,
        head_pt: Point,
        is_empty_field: bool,
        params: EditorPaintParams<'_>,
    ) {
        // Vertical scroll is baked into each VisibleLine::top in f64 so D2D coordinates stay exact on huge files.
        frame.scroll_x = self.scroll.x;
        frame.scroll_y = self.scroll.y;
        frame.content_bounds = Rect::new(
            point(
                frame.input_bounds.origin.x + self.scroll.x,
                frame.input_bounds.origin.y,
            ),
            frame.input_bounds.size,
        );
        frame.caret_rect = self.compute_caret_rect(
            frame.content_bounds.origin,
            head_pt,
            frame.line_height,
            params.scale,
            params.caret_width_px,
        );
        frame.show_caret = params.focused
            && params.window_active
            && self.blink.visible()
            && !self.caret_hidden
            && (self.model.selection().is_empty() || self.model.composition().is_some());
        frame.marked_range = (!is_empty_field)
            .then(|| self.model.composition_range())
            .flatten();
        if params.window_active
            && params.show_selection
            && self.model.composition().is_none()
            && !self.model.selection().is_empty()
        {
            frame.selection_rects = frame.range_rects(&self.model.selected_range());
        }
        self.frame = Some(frame);
    }

    fn prepare_field(&mut self, input_bounds: Rect, params: EditorPaintParams<'_>, ts: &TextSystem) {
        let style = params.style;
        let text_style = &style.text;
        let line_height = style.line_height.max(1.0);
        let space_width = ts.face(text_style).space_width();
        let is_empty = self.model.text().len() == 0;
        let (display_str, text_color): (SharedStr, Rgba) = if is_empty {
            (self.placeholder.clone(), params.palette.text_secondary)
        } else {
            (self.model.value().into(), style.color)
        };
        let shaped = ts.shape_row(display_str, text_style);
        let row_layout = RowLayout::from_shaped(vec![shaped]);
        let head = self.model.cursor();
        let head_pt = row_layout
            .position_for_index(head, line_height, false)
            .unwrap_or(Point::ZERO);
        let max_line_w = row_layout.size(line_height).width.max(head_pt.x);
        self.scroll.content_w = content_width_with_margin(max_line_w, input_bounds.size.width);
        if self.reveal.take().is_some() {
            self.scroll.x =
                reveal_horizontal_scroll(self.scroll.x, head_pt.x, input_bounds.size.width);
        }
        self.scroll.x =
            clamp_horizontal_scroll(self.scroll.x, self.scroll.content_w, input_bounds.size.width);
        let text_len = self.model.text().len();
        let frame = FrameLayout {
            revision: self.model.revision(),
            scroll_x: 0.0,
            scroll_y: 0.0,
            input_bounds,
            content_bounds: Rect::ZERO,
            first_line: 0,
            lines: vec![VisibleLine {
                byte_offset: 0,
                top: 0.0,
                layout: row_layout,
            }],
            visible_byte_range: 0..text_len,
            line_height,
            wrap_width: None,
            space_width,
            text_color,
            caret_rect: Rect::ZERO,
            show_caret: false,
            selection_rects: Vec::new(),
            marked_range: None,
            last_cursor: head,
        };
        self.finish_frame(frame, head_pt, is_empty, params);
    }

    fn prepare_document(
        &mut self,
        input_bounds: Rect,
        params: EditorPaintParams<'_>,
        ts: &TextSystem,
    ) -> bool {
        let style = params.style;
        let text_style = &style.text;
        let line_height = style.line_height.max(1.0);
        let face = ts.face(text_style);
        let space_width = face.space_width();
        let text = self.model.text();
        let reveal_req = self.reveal.take();
        let head = self.model.cursor();
        let end_affinity = self.model.selection().end_affinity;

        let doc = self.doc.as_deref_mut().expect("document view");
        let soft_wrap = doc.soft_wrap;
        let wrap_width = soft_wrap.then_some((input_bounds.size.width - RIGHT_MARGIN).max(1.0));

        let reflowing = doc.wrap.style() != text_style
            || doc.wrap.wrap_width() != wrap_width
            || (doc.last_line_height - line_height).abs() > 0.01;
        let anchor = if reflowing && reveal_req.is_none() {
            doc.capture_scroll_anchor(self.scroll.y, doc.last_line_height)
        } else {
            None
        };

        doc.wrap.sync_layout(text, text_style, wrap_width, ts);
        doc.last_line_height = line_height;

        if let Some(anchor) = anchor {
            self.scroll.y = doc.restore_scroll_anchor(text, anchor, line_height);
        }

        let total_rows = doc.wrap.visual_row_count().max(1);
        let content_h = (total_rows as f64 * f64::from(line_height))
            .max(f64::from(input_bounds.size.height));
        self.scroll.content_h = content_h;

        if let Some(req) = reveal_req {
            let target = req.offset.min(text.len());
            let disp = doc.wrap.offset_to_display(text, target, req.end_affinity);
            let row_y = disp.row as f64 * f64::from(line_height);
            let vp_h = f64::from(input_bounds.size.height);
            let lh_f64 = f64::from(line_height);
            let v_margin = if req.with_margin {
                f64::from(caret_vertical_margin(input_bounds.size.height, line_height))
            } else {
                0.0
            };
            let prev_y = self.scroll.y;
            let mut next_y = prev_y;
            if row_y < -next_y + v_margin {
                next_y = (v_margin - row_y).min(0.0);
            } else if row_y + lh_f64 > -next_y + vp_h - v_margin {
                next_y = vp_h - v_margin - (row_y + lh_f64);
            }
            match req.direction {
                Some(MoveDirection::Up) => next_y = next_y.max(prev_y),
                Some(MoveDirection::Down) => next_y = next_y.min(prev_y),
                None => {}
            }
            self.scroll.y = next_y;
        }
        let min_y = (-content_h + f64::from(input_bounds.size.height)).min(0.0);
        self.scroll.y = self.scroll.y.clamp(min_y, 0.0);

        // Compute horizontal extent and resolve horizontal reveal before shaping windowed lines.
        let mut max_line_w = wrap_width.unwrap_or(0.0);
        if soft_wrap {
            self.scroll.x = 0.0;
        } else {
            let longest_row = doc.wrap.longest_line();
            let slice = text.slice_line(longest_row);
            let key = LongestLineCache {
                revision: self.model.revision(),
                row: longest_row,
                len: slice.len(),
                style: text_style.clone(),
                wrap_width,
            };
            max_line_w = match &doc.longest_line_cache {
                Some((cached, w)) if *cached == key => *w,
                _ => {
                    let s: Cow<str> = slice.into();
                    let w = face.unwrapped_width(&s);
                    doc.longest_line_cache = Some((key, w));
                    w
                }
            };
            if reveal_req.is_some() {
                let pt = text.offset_to_point(head);
                let line = text.slice_line(pt.row);
                let prefix: Cow<str> = line.slice(..pt.column.min(line.len())).into();
                let caret_x = face.unwrapped_width(&prefix);
                max_line_w = max_line_w.max(caret_x);
                self.scroll.x =
                    reveal_horizontal_scroll(self.scroll.x, caret_x, input_bounds.size.width);
            }
            let est_w = content_width_with_margin(max_line_w, input_bounds.size.width);
            self.scroll.x = clamp_horizontal_scroll(self.scroll.x, est_w, input_bounds.size.width);
        }

        let vis = doc.visible_slice(self.scroll.y, line_height, input_bounds.size.height);
        let first_line = vis.lines.start;
        let start_off = text.line_start_offset(vis.lines.start);
        let end_off = text.line_end_offset(vis.lines.end.saturating_sub(1));

        let prev_scroll_x = self.scroll.x;
        let (lines, shaped_windowed) =
            self.shape_visible_lines(vis, style, line_height, input_bounds.size.width, ts);

        if !soft_wrap {
            for line in &lines {
                max_line_w = max_line_w.max(line.layout.size(line_height).width);
            }
        }

        let doc = self.doc.as_deref().expect("document view");
        let text = self.model.text();
        let head_pt = {
            let row = text.offset_to_point(head).row;
            row.checked_sub(first_line)
                .and_then(|vi| lines.get(vi))
                .and_then(|line| {
                    let local = head.saturating_sub(line.byte_offset);
                    line.layout
                        .position_for_index(local, line_height, end_affinity)
                        .map(|pt| point(pt.x, line.top + pt.y))
                })
                .unwrap_or_else(|| {
                    let disp = doc.wrap.offset_to_display(text, head, end_affinity);
                    point(
                        0.0,
                        (disp.row as f64 * f64::from(line_height) + self.scroll.y) as f32,
                    )
                })
        };
        if !soft_wrap {
            max_line_w = max_line_w.max(head_pt.x);
        }

        self.scroll.content_w = content_width_with_margin(max_line_w, input_bounds.size.width);
        if reveal_req.is_some() && !soft_wrap {
            self.scroll.x =
                reveal_horizontal_scroll(self.scroll.x, head_pt.x, input_bounds.size.width);
        }
        self.scroll.x = if soft_wrap {
            0.0
        } else {
            clamp_horizontal_scroll(self.scroll.x, self.scroll.content_w, input_bounds.size.width)
        };

        let frame = FrameLayout {
            revision: self.model.revision(),
            scroll_x: 0.0,
            scroll_y: 0.0,
            input_bounds,
            content_bounds: Rect::ZERO,
            first_line,
            lines,
            visible_byte_range: start_off..end_off,
            line_height,
            wrap_width,
            space_width,
            text_color: style.color,
            caret_rect: Rect::ZERO,
            show_caret: false,
            selection_rects: Vec::new(),
            marked_range: None,
            last_cursor: head,
        };
        self.finish_frame(frame, head_pt, false, params);

        shaped_windowed && (self.scroll.x - prev_scroll_x).abs() > 1.0
    }

    fn shape_visible_lines(
        &self,
        vis: VisibleSlice,
        style: &EditorPaintStyle,
        line_height: f32,
        viewport_width: f32,
        ts: &TextSystem,
    ) -> (Vec<VisibleLine>, bool) {
        let doc = self.doc.as_deref().expect("document view");
        let text = self.model.text();
        let text_style = &style.text;
        let caret = self.model.cursor();
        let mut result = Vec::with_capacity(vis.lines.len());
        let mut shaped_windowed = false;
        let mut y = vis.top_rel;

        for buf_line in vis.lines {
            let line_slice = text.slice_line(buf_line);
            let line_len = line_slice.len();
            let line_byte_start = text.line_start_offset(buf_line);
            let wrapped = doc.wrap.line(buf_line).expect("wrapped line exists");
            let total_sub_rows = wrapped.row_count();

            let layout =
                if !doc.soft_wrap && total_sub_rows == 1 && line_len > HUGE_UNWRAPPED_LINE_BYTES {
                    let line_str: Cow<str> = line_slice.into();
                    let min_x = (-self.scroll.x - HORIZONTAL_WINDOW_PAD).max(0.0);
                    let max_x =
                        (-self.scroll.x + viewport_width + HORIZONTAL_WINDOW_PAD).max(min_x);
                    if let Some((mut start_b, mut end_b, start_x, total_w)) =
                        ts.face(text_style).horizontal_window(&line_str, min_x, max_x)
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
                        let shaped = ts.shape_row(slice_str, text_style);
                        let vrow = VisualRow::Windowed {
                            start_byte: start_b,
                            end_byte: end_b,
                            total_bytes: line_len,
                            start_x,
                            total_width: total_w,
                            line: shaped,
                        };
                        shaped_windowed = true;
                        RowLayout::from_slice(vec![vrow], 0, 0, 1, 0.0)
                    } else {
                        self.shape_wrapped_line(buf_line, &vis.shape_rows, text_style, ts)
                    }
                } else {
                    self.shape_wrapped_line(buf_line, &vis.shape_rows, text_style, ts)
                };

            let h = layout.size(line_height).height;
            result.push(VisibleLine {
                byte_offset: line_byte_start,
                top: y,
                layout,
            });
            y += h;
        }
        (result, shaped_windowed)
    }

    fn shape_wrapped_line(
        &self,
        buf_line: usize,
        shape_rows: &Range<usize>,
        text_style: &TextStyle,
        ts: &TextSystem,
    ) -> RowLayout {
        let doc = self.doc.as_deref().expect("document view");
        let text = self.model.text();
        let line_slice = text.slice_line(buf_line);
        let line_len = line_slice.len();
        let line_byte_start = text.line_start_offset(buf_line);
        let wrapped = doc.wrap.line(buf_line).expect("wrapped line exists");
        let first_row_of_line = doc.wrap.first_visual_row(buf_line);
        let total_sub_rows = wrapped.row_count();
        let caret = self.model.cursor();
        let end_affinity = self.model.selection().end_affinity;

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
                let disp = doc.wrap.offset_to_display(text, caret, end_affinity);
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
                let shaped = ts.shape_row(sub_text, text_style);
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

    fn compute_caret_rect(
        &self,
        origin: Point,
        head_pt: Point,
        line_height: f32,
        scale: f32,
        caret_width_px: u32,
    ) -> Rect {
        let caret_h = if self.caret_height > 0.0 {
            self.caret_height
        } else {
            0.85 * line_height
        };
        let caret_origin = snap_point(
            point(
                origin.x + head_pt.x,
                origin.y + head_pt.y + (line_height - caret_h) * 0.5,
            ),
            scale,
        );
        let caret_w = (caret_width_px.max(1) as f32 / scale).max(1.0 / scale);
        Rect::new(caret_origin, size(caret_w, caret_h))
    }
}
