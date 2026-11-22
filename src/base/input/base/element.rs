use crate::ui::smallvec::SmallVec;
use crate::ui::{
    AnyElement, App, Bounds, ContentMask, Corners, Edges, Element, ElementId, ElementInputHandler,
    Entity, GlobalElementId, Half, HitboxBehavior, IntoElement, LayoutId, MouseButton,
    MouseMoveEvent, MouseUpEvent, Pixels, Point, Position, SharedString, Size, Style, TextRun,
    UnderlineStyle, Window, fill, point, px, relative, rgb, rgba, size,
};
use ropey::Rope;
use std::{ops::Range, rc::Rc};

use crate::base::{
    Scrollbar,
    input::{
        RopeExt as _,
        display_map::{InputLine, LineLayout},
    },
};
use crate::theme::palette;

use super::{EditorState, layout::LastLayout};

const BOTTOM_MARGIN_ROWS: usize = 3;
const CARET_WIDTH: Pixels = px(1.);
pub(super) const RIGHT_MARGIN: Pixels = px(10.);

#[derive(Clone, Copy, Debug, PartialEq)]
struct EditorScrollbarLayout {
    bounds: Bounds<Pixels>,
    scroll_size: Size<Pixels>,
}

/// What the unwrapped width of the longest line depends on.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct LongestLineKey {
    document_revision: u64,
    row: usize,
    len: usize,
    font: crate::ui::Font,
    text_size: Pixels,
    wrap_width: Option<Pixels>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct EditorScrollbarSnapshot {
    layout: EditorScrollbarLayout,
    cursor_scroll_offset: Point<Pixels>,
    soft_wrap: bool,
}

impl EditorScrollbarSnapshot {
    fn new(
        input_bounds: Bounds<Pixels>,
        scroll_size: Size<Pixels>,
        cursor_scroll_offset: Point<Pixels>,
        state: &EditorState,
    ) -> Self {
        Self {
            layout: EditorScrollbarLayout::new(input_bounds, scroll_size, state.editor_paddings),
            cursor_scroll_offset,
            soft_wrap: state.soft_wrap,
        }
    }
}

impl EditorScrollbarLayout {
    fn new(input_bounds: Bounds<Pixels>, scroll_size: Size<Pixels>, paddings: Edges<Pixels>) -> Self {
        Self {
            bounds: Bounds::new(
                point(input_bounds.origin.x, input_bounds.origin.y - paddings.top),
                size(
                    input_bounds.size.width + paddings.right,
                    input_bounds.size.height + paddings.top + paddings.bottom,
                ),
            ),
            scroll_size: size(
                scroll_size.width + paddings.right + RIGHT_MARGIN,
                scroll_size.height,
            ),
        }
    }
}

pub(super) struct EditorScrollbar {
    state: Entity<EditorState>,
}

impl EditorScrollbar {
    pub(super) fn new(state: Entity<EditorState>) -> Self {
        Self { state }
    }
}

impl IntoElement for EditorScrollbar {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for EditorScrollbar {
    type RequestLayoutState = ();
    type PrepaintState = Option<AnyElement>;

    fn id(&self) -> Option<ElementId> {
        Some("editor-scrollbar".into())
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.position = Position::Absolute;
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let state = self.state.read(cx);
        let snapshot = state.editor_scrollbar_snapshot.get()?;
        let scroll_handle = state.scroll_handle.clone();

        if scroll_handle.offset() != snapshot.cursor_scroll_offset {
            scroll_handle.set_offset(snapshot.cursor_scroll_offset);
        }

        let mut scrollbar = Scrollbar::new(
            scroll_handle,
            snapshot.layout.bounds,
            snapshot.layout.scroll_size,
            !snapshot.soft_wrap,
        )
        .into_any_element();

        scrollbar.prepaint_as_root(
            snapshot.layout.bounds.origin,
            snapshot.layout.bounds.size.into(),
            window,
            cx,
        );
        Some(scrollbar)
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(scrollbar) = prepaint.as_mut() {
            scrollbar.paint(window, cx);
        }
    }
}

pub(super) fn clamp_horizontal_scroll_offset(
    offset: Pixels,
    scroll_width: Pixels,
    input_width: Pixels,
) -> Pixels {
    let min_offset = (input_width - scroll_width).min(px(0.));
    offset.clamp(min_offset, px(0.))
}

fn cursor_surrounding_padding(viewport_height: Pixels, line_height: Pixels) -> Pixels {
    let visible_lines = (viewport_height / line_height) as usize;
    let raw = if visible_lines < BOTTOM_MARGIN_ROWS * 8 {
        line_height
    } else {
        BOTTOM_MARGIN_ROWS * line_height
    };
    let viewport_half = (visible_lines as f32 * line_height).half();
    raw.min(viewport_half)
}

pub(super) struct TextElement {
    pub(crate) state: Entity<EditorState>,
    placeholder: SharedString,
}

impl TextElement {
    pub(super) fn new(state: Entity<EditorState>) -> Self {
        Self {
            state,
            placeholder: SharedString::default(),
        }
    }

    pub(super) fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    fn paint_mouse_listeners(&mut self, window: &mut Window) {
        window.on_mouse_event({
            let state = self.state.clone();
            move |event: &MouseMoveEvent, _, window, cx| {
                if event.pressed_button == Some(MouseButton::Left) {
                    state.update(cx, |state, cx| {
                        state.on_drag_move(event, window, cx);
                    });
                }
            }
        });

        window.on_mouse_event({
            let state = self.state.clone();
            move |_: &MouseUpEvent, phase, _, cx| {
                if !phase.bubble() {
                    return;
                }
                state.update(cx, |state, _| {
                    state.auto_scroll.stop();
                    state.selecting = false;
                });
            }
        });
    }

    fn layout_cursor(
        &self,
        last_layout: &LastLayout,
        bounds: &mut Bounds<Pixels>,
        scroll_size: Size<Pixels>,
        cx: &mut App,
    ) -> (Bounds<Pixels>, Point<Pixels>) {
        let state = self.state.read(cx);
        let line_height = last_layout.line_height;
        let lines = &last_layout.lines;
        let visible_buffer_lines = &last_layout.visible_buffer_lines;

        let mut scroll_offset = state.scroll_handle.offset();
        let top_bottom_margin = cursor_surrounding_padding(bounds.size.height, line_height);

        let caret_for = |row: usize, offset: usize, affinity: bool| -> Point<Pixels> {
            let top = line_height * state.text_wrapper.buffer_line_to_first_wrap_row(row);
            let line_origin = point(px(0.), top);

            if let Ok(vi) = visible_buffer_lines.binary_search(&row) {
                let line = &lines[vi];
                let line_start = last_layout.visible_line_byte_offsets[vi];
                let local = offset.saturating_sub(line_start);
                if let Some(pos) = line.position_for_index(local, last_layout, affinity) {
                    return line_origin + pos;
                }
            }
            line_origin
        };

        let mut selected_range = state.selection;
        let mut cursor = state.selection.cursor_offset();
        if let Some(ime_marked_range) = &state.ime_marked_range {
            selected_range = (ime_marked_range.end..ime_marked_range.end).into();
            cursor = ime_marked_range.end;
        }
        let is_selected_all = selected_range.len() == state.text.len();

        let cursor_row = state.text.offset_to_point(cursor).row;
        let sel_start_row = state.text.offset_to_point(selected_range.start).row;
        let sel_end_row = state.text.offset_to_point(selected_range.end).row;

        let cursor_pos = caret_for(cursor_row, cursor, state.cursor_line_end_affinity);
        let cursor_start = caret_for(sel_start_row, selected_range.start, false);
        let cursor_end = caret_for(sel_end_row, selected_range.end, false);

        let selection_changed = state.last_selected_range != Some(selected_range);
        let auto_scrolling = state.auto_scroll.is_active();
        if selection_changed && !is_selected_all {
            scroll_offset.x = if scroll_offset.x + cursor_pos.x > (bounds.size.width - RIGHT_MARGIN)
            {
                bounds.size.width - RIGHT_MARGIN - cursor_pos.x
            } else if scroll_offset.x + cursor_pos.x < px(0.) {
                -cursor_pos.x
            } else {
                scroll_offset.x
            };

            if !auto_scrolling {
                scroll_offset.y = if scroll_offset.y + cursor_pos.y
                    > bounds.size.height - top_bottom_margin
                {
                    bounds.size.height - top_bottom_margin - cursor_pos.y
                } else if scroll_offset.y + cursor_pos.y < top_bottom_margin {
                    (top_bottom_margin - cursor_pos.y).min(px(0.))
                } else {
                    scroll_offset.y
                };
            }

            if state.selection.reversed {
                if scroll_offset.x + cursor_start.x < px(0.) {
                    scroll_offset.x = -cursor_start.x;
                }
                if !auto_scrolling && scroll_offset.y + cursor_start.y < px(0.) {
                    scroll_offset.y = -cursor_start.y;
                }
            } else {
                if scroll_offset.x + cursor_end.x <= px(0.) {
                    scroll_offset.x = -cursor_end.x;
                }
                if !auto_scrolling && scroll_offset.y + cursor_end.y <= px(0.) {
                    scroll_offset.y = -cursor_end.y;
                }
            }
        }

        if let Some(deferred_scroll_offset) = state.deferred_scroll_offset {
            scroll_offset = deferred_scroll_offset;
        }
        scroll_offset.x =
            clamp_horizontal_scroll_offset(scroll_offset.x, scroll_size.width, bounds.size.width);

        let cursor_height = 0.85 * line_height;
        let cursor_bounds = Bounds::new(
            point(
                bounds.left() + cursor_pos.x + scroll_offset.x,
                bounds.top() + cursor_pos.y + ((line_height - cursor_height) / 2.),
            ),
            size(CARET_WIDTH, cursor_height),
        );
        bounds.origin = bounds.origin + scroll_offset;

        (cursor_bounds, scroll_offset)
    }

    fn layout_match_range(
        range: Range<usize>,
        last_layout: &LastLayout,
        bounds: &Bounds<Pixels>,
    ) -> Vec<Bounds<Pixels>> {
        let Some(corners) = Self::layout_range_corners(&range, last_layout) else {
            return Vec::new();
        };
        let origin = bounds.origin;
        corners
            .into_iter()
            .map(|c| Bounds::from_corners(origin + c.top_left, origin + c.bottom_right))
            .collect()
    }

    fn layout_range_corners(
        range: &Range<usize>,
        last_layout: &LastLayout,
    ) -> Option<Vec<Corners<Point<Pixels>>>> {
        let range = range.start.max(last_layout.visible_range_offset.start)
            ..range.end.min(last_layout.visible_range_offset.end);
        if range.is_empty() {
            return None;
        }

        let mut y = last_layout.visible_top;
        let mut corners = Vec::new();
        for (&line_offset, line) in last_layout
            .visible_line_byte_offsets
            .iter()
            .zip(last_layout.lines.iter())
        {
            let mut offset = line_offset;
            for (row, shaped) in line.wrapped_lines.iter().enumerate() {
                let end = offset + shaped.len;
                if !shaped.is_shaped() {
                    offset = end;
                    y += last_layout.line_height;
                    continue;
                }
                let start_ix = range.start.max(offset);
                let end_ix = range.end.min(end);
                let is_last = row + 1 == line.wrapped_lines.len();
                let newline = is_last && range.start <= end && range.end > end;
                if start_ix < end_ix || newline {
                    let indent = if row == 0 { px(0.) } else { line.wrap_indent };
                    let left = indent + shaped.x_for_index(start_ix.min(end) - offset);
                    let right = indent
                        + if newline {
                            shaped.width + last_layout.space_width
                        } else {
                            shaped.x_for_index(end_ix - offset)
                        };
                    corners.push(Corners {
                        top_left: point(left, y),
                        top_right: point(right, y),
                        bottom_left: point(left, y + last_layout.line_height),
                        bottom_right: point(right, y + last_layout.line_height),
                    });
                }
                offset = end;
                y += last_layout.line_height;
            }
        }
        (!corners.is_empty()).then_some(corners)
    }

    fn layout_selection(
        &self,
        last_layout: &LastLayout,
        bounds: &Bounds<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) -> Vec<Bounds<Pixels>> {
        let state = self.state.read(cx);
        if !state.has_selection_focus(window, cx) {
            return vec![];
        }

        let mut selected_range = state.selection;
        if let Some(ime_marked_range) = &state.ime_marked_range
            && !ime_marked_range.is_empty()
        {
            selected_range = (ime_marked_range.end..ime_marked_range.end).into();
        }
        if selected_range.is_empty() {
            return vec![];
        }

        let (start_ix, end_ix) = if selected_range.start < selected_range.end {
            (selected_range.start, selected_range.end)
        } else {
            (selected_range.end, selected_range.start)
        };

        let range = start_ix.max(last_layout.visible_range_offset.start)
            ..end_ix.min(last_layout.visible_range_offset.end);
        if range.is_empty() {
            return vec![];
        }

        Self::layout_match_range(range, last_layout, bounds)
    }

    fn calculate_visible_range(
        &self,
        state: &EditorState,
        line_height: Pixels,
        input_height: Pixels,
    ) -> (Range<usize>, Vec<usize>, Pixels, Range<usize>) {
        if state.is_single_line() {
            return (0..1, vec![0], px(0.), 0..1);
        }

        let display_count = state.text_wrapper.len();
        let buffer_line_count = state.text_wrapper.lines_count();
        if display_count == 0 || buffer_line_count == 0 {
            return (0..0, Vec::new(), px(0.), 0..0);
        }

        let scroll_top = state
            .deferred_scroll_offset
            .map_or_else(|| state.scroll_handle.offset().y, |offset| offset.y);

        let viewport_top = (-scroll_top).max(px(0.));
        let viewport_bottom = viewport_top + input_height;
        let line_height_f = f32::from(line_height);
        let first_display =
            ((f32::from(viewport_top) / line_height_f).floor() as usize).min(display_count - 1);
        let last_display =
            ((f32::from(viewport_bottom) / line_height_f).ceil() as usize).min(display_count - 1);
        let visible_display_rows =
            first_display.saturating_sub(2)..(last_display + 3).min(display_count);

        let start_line = state.text_wrapper.wrap_row_to_buffer_line(first_display);
        let end_line = state.text_wrapper.wrap_row_to_buffer_line(last_display);
        let visible_top = line_height * state.text_wrapper.buffer_line_to_first_wrap_row(start_line);

        let extra_rows = 1;
        let visible_range = start_line..(end_line + 1 + extra_rows).min(buffer_line_count);
        let visible_buffer_lines: Vec<usize> = visible_range.clone().collect();

        (
            visible_range,
            visible_buffer_lines,
            visible_top,
            visible_display_rows,
        )
    }

    fn layout_lines(
        state: &EditorState,
        display_text: &Rope,
        last_layout: &LastLayout,
        visible_display_rows: &Range<usize>,
        font_size: Pixels,
        runs: &[TextRun],
        window: &mut Window,
    ) -> Vec<LineLayout> {
        if state.is_single_line() {
            let text: SharedString = display_text.to_string().into();
            let aligned_runs = align_runs_to_char_boundaries(&text, runs);
            let line_runs = aligned_runs.as_deref().unwrap_or(runs);
            let shaped_line = window
                .text_system()
                .shape_line(text, font_size, line_runs, None);
            return vec![LineLayout::new().lines(crate::ui::smallvec::smallvec![shaped_line])];
        }

        if state.text.len() == 0 {
            let placeholder_text = display_text.to_string();
            let mut placeholder_lines = SmallVec::new();
            for (line, line_runs) in placeholder_line_runs(&placeholder_text, runs) {
                let shaped_line = window.text_system().shape_line(
                    line.to_string().into(),
                    font_size,
                    &line_runs,
                    None,
                );
                placeholder_lines.push(shaped_line);
            }
            return vec![LineLayout::new().lines(placeholder_lines)];
        }

        let mut lines = Vec::with_capacity(last_layout.visible_buffer_lines.len());
        let mut run_offset = 0;
        let mut display_row = last_layout
            .visible_buffer_lines
            .first()
            .map_or(0, |&row| state.text_wrapper.buffer_line_to_first_wrap_row(row));
        let cursor = state.selection.cursor_offset();

        for (vi, &buffer_line) in last_layout.visible_buffer_lines.iter().enumerate() {
            let line_slice = display_text.slice_line(buffer_line);
            let line_len = line_slice.len();
            let line_byte_start = last_layout.visible_line_byte_offsets[vi];
            let line_item = state
                .text_wrapper
                .line(buffer_line)
                .expect("line should exist in wrapper");

            debug_assert_eq!(line_item.len(), line_len);

            let mut wrapped_lines: SmallVec<[InputLine; 1]> =
                SmallVec::with_capacity(line_item.wrapped_lines.len());

            for (sub_ix, range) in line_item.wrapped_lines.iter().enumerate() {
                let sub_start = line_byte_start + range.start;
                let sub_end = line_byte_start + range.end;
                let has_cursor = cursor >= sub_start && cursor <= sub_end;
                let needs_shape = visible_display_rows.contains(&display_row)
                    || has_cursor
                    || (sub_ix == 0 && line_item.indent > 0);
                if needs_shape {
                    let line_runs = runs_for_range(runs, run_offset, range);
                    let sub_line: SharedString = line_slice.slice(range.clone()).to_string().into();
                    let line_runs =
                        align_runs_to_char_boundaries(&sub_line, &line_runs).unwrap_or(line_runs);
                    let shaped_line = window
                        .text_system()
                        .shape_line(sub_line, font_size, &line_runs, None);
                    wrapped_lines.push(InputLine::from(shaped_line));
                } else {
                    wrapped_lines.push(InputLine::unshaped(range.len()));
                }
                display_row += 1;
            }

            let wrap_indent = if line_item.indent > 0 && wrapped_lines.len() > 1 {
                let indent_byte_len = line_slice
                    .chars()
                    .take(line_item.indent as usize)
                    .map(char::len_utf8)
                    .sum();
                wrapped_lines[0].x_for_index(indent_byte_len)
            } else {
                px(0.)
            };

            lines.push(
                LineLayout::new()
                    .inline_lines(wrapped_lines)
                    .wrap_indent(wrap_indent),
            );
            run_offset += line_len + 1;
        }

        lines
    }
}

pub(super) struct PrepaintState {
    last_layout: LastLayout,
    scroll_size: Size<Pixels>,
    caret_rect: Bounds<Pixels>,
    cursor_scroll_offset: Point<Pixels>,
    selection_bounds: Vec<Bounds<Pixels>>,
    bounds: Bounds<Pixels>,
}

impl IntoElement for TextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let state = self.state.read(cx);
        let line_height = window.line_height();

        let mut style = Style::default();
        style.size.width = relative(1.).into();
        if state.is_multi_line() {
            style.flex_grow = 1.0;
            style.size.height = relative(1.).into();
            style.min_size.height = (2 * line_height).into();
        } else {
            style.size.height = line_height.into();
        }

        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let style = window.text_style();
        let font = style.font();
        let text_size = style.font_size.to_pixels(window.rem_size());
        let space_width = {
            let font_id = window.text_system().resolve_font(&font);
            window.text_system().layout_width(font_id, text_size, ' ')
        };

        let mut bounds = bounds;
        let (wrap_width, wrapping_indent) = {
            let state = self.state.read(cx);
            let wrap_width = if state.is_multi_line() && state.soft_wrap {
                Some(bounds.size.width - RIGHT_MARGIN)
            } else {
                None
            };
            (wrap_width, state.wrapping_indent)
        };

        self.state.update(cx, |state, cx| {
            state.text_wrapper.prepare_layout(
                &state.text,
                font,
                text_size,
                wrap_width,
                wrapping_indent,
                cx,
            );
        });

        let p = palette(cx);
        let state = self.state.read(cx);
        let text = state.text.clone();
        let is_empty = text.len() == 0;
        let placeholder = self.placeholder.clone();

        let (display_text, text_color) = if is_empty {
            (&Rope::from(placeholder.as_str()), rgba(p.text_secondary).into())
        } else {
            (&text, style.color)
        };

        let line_height = window.line_height();
        let (visible_range, visible_buffer_lines, visible_top, visible_display_rows) =
            self.calculate_visible_range(state, line_height, bounds.size.height);
        let visible_start_offset = state.text.line_start_offset(visible_range.start);
        let visible_end_offset = state
            .text
            .line_end_offset(visible_range.end.saturating_sub(1));

        let visible_line_byte_offsets: Vec<usize> = visible_buffer_lines
            .iter()
            .map(|&bl| state.text.line_start_offset(bl))
            .collect();
        let visible_range_offset = visible_start_offset..visible_end_offset;

        let mut last_layout = LastLayout {
            visible_buffer_lines,
            visible_line_byte_offsets,
            visible_top,
            visible_range_offset,
            line_height,
            wrap_width,
            space_width,
            lines: Rc::new(vec![]),
            cursor_bounds: None,
        };

        let run = TextRun {
            len: display_text.len(),
            font: style.font(),
            color: text_color,
            background_color: None,
            underline: None,
        };
        let marked_underline = Some(UnderlineStyle {
            thickness: px(1.),
            color: Some(text_color),
        });
        let ime_marked_range = state.ime_marked_range.as_ref().map(|m| m.start..m.end);
        let runs = split_run_for_ime_underline(
            run,
            0..display_text.len(),
            ime_marked_range,
            marked_underline,
        )
        .into_vec();

        let lines = Self::layout_lines(
            state,
            display_text,
            &last_layout,
            &visible_display_rows,
            text_size,
            &runs,
            window,
        );

        let mut longest_line_width = wrap_width.unwrap_or(px(0.));
        if state.is_single_line() || !state.soft_wrap {
            let longest_row = state.text_wrapper.longest_row();
            let longest_line = state.text.slice_line(longest_row);
            let key = LongestLineKey {
                document_revision: state.document_revision,
                row: longest_row,
                len: longest_line.len(),
                font: style.font(),
                text_size,
                wrap_width,
            };
            let cached = state
                .longest_line_width
                .take()
                .filter(|(cached_key, _)| *cached_key == key);
            longest_line_width = match cached {
                Some((_, width)) => width,
                None => {
                    let longest_line: SharedString = longest_line.to_string().into();
                    window
                        .text_system()
                        .shape_line(
                            longest_line.clone(),
                            text_size,
                            &[TextRun {
                                len: longest_line.len(),
                                font: key.font.clone(),
                                color: crate::ui::black(),
                                background_color: None,
                                underline: None,
                            }],
                            wrap_width,
                        )
                        .width
                }
            };
            state
                .longest_line_width
                .set(Some((key, longest_line_width)));
        }
        last_layout.lines = Rc::new(lines);

        let total_wrapped_lines = state.text_wrapper.len();
        let scroll_size = size(
            if longest_line_width + RIGHT_MARGIN > bounds.size.width {
                longest_line_width + RIGHT_MARGIN
            } else {
                longest_line_width
            },
            (total_wrapped_lines as f32 * line_height).max(bounds.size.height),
        );

        let input_bounds = bounds;
        let (cursor_bounds, cursor_scroll_offset) =
            self.layout_cursor(&last_layout, &mut bounds, scroll_size, cx);
        last_layout.cursor_bounds = Some(cursor_bounds);

        let caret_rect = {
            let state = self.state.read(cx);
            let mut clamped_scroll = cursor_scroll_offset;
            let safe_y_range = (-scroll_size.height + input_bounds.size.height).min(px(0.))..px(0.);
            clamped_scroll.y = if state.is_single_line() {
                px(0.)
            } else {
                clamped_scroll.y.clamp(safe_y_range.start, safe_y_range.end)
            };
            let height = match state.caret_height.get() {
                h if h > px(0.) => h,
                _ => cursor_bounds.size.height,
            };
            let origin = window.pixel_snap_point(point(
                cursor_bounds.origin.x,
                cursor_bounds.origin.y + clamped_scroll.y,
            ));
            Bounds::new(origin, size(CARET_WIDTH, height))
        };
        self.state.update(cx, |state, _| {
            state.caret_bounds = Some(caret_rect);
        });

        let selection_bounds = self.layout_selection(&last_layout, &bounds, window, cx);

        let state = self.state.read(cx);
        state
            .editor_scrollbar_snapshot
            .set(Some(EditorScrollbarSnapshot::new(
                input_bounds,
                scroll_size,
                cursor_scroll_offset,
                state,
            )));
        let _hitbox = window.insert_hitbox(input_bounds, HitboxBehavior::Normal);

        PrepaintState {
            bounds,
            last_layout,
            scroll_size,
            caret_rect,
            cursor_scroll_offset,
            selection_bounds,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        input_bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let (focus_handle, show_cursor, selected_range, caret_hidden) = {
            let state = self.state.read(cx);
            (
                state.focus_handle.clone(),
                state.show_cursor(window, cx),
                state.selection,
                state.caret_hidden.get(),
            )
        };
        let bounds = prepaint.bounds;

        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.state.clone()),
            cx,
        );

        let p = palette(cx);
        let line_height = window.line_height();
        let origin = bounds.origin;

        if window.is_window_active() {
            let selection_color = p.selection();
            for rect in prepaint.selection_bounds.drain(..) {
                window.paint_quad(fill(rect, selection_color));
            }
        }

        let mut offset_y = prepaint.last_layout.visible_top;
        for line in prepaint.last_layout.lines.iter() {
            line.paint(point(origin.x, origin.y + offset_y), line_height, window, cx);
            offset_y += line.size(line_height).height;
        }

        if show_cursor && !caret_hidden && selected_range.is_empty() {
            let caret = prepaint.caret_rect;
            window.with_content_mask(
                Some(ContentMask {
                    bounds: input_bounds,
                }),
                |window| window.paint_quad(fill(caret, rgb(p.caret))),
            );
        }

        self.state.update(cx, |state, cx| {
            let geometry_changed = state.last_bounds != Some(bounds)
                || state.input_bounds != input_bounds
                || state.scroll_size != prepaint.scroll_size
                || state.last_layout.as_ref().is_none_or(|layout| {
                    layout.cursor_bounds != prepaint.last_layout.cursor_bounds
                        || layout.line_height != prepaint.last_layout.line_height
                });
            state.last_layout = Some(prepaint.last_layout.clone());
            state.last_bounds = Some(bounds);
            state.last_cursor = Some(state.cursor());
            state.set_input_bounds(input_bounds, cx);
            state.last_selected_range = Some(selected_range);
            state.scroll_size = prepaint.scroll_size;
            state.update_scroll_offset(Some(prepaint.cursor_scroll_offset), cx);
            state.deferred_scroll_offset = None;

            if geometry_changed {
                cx.notify();
            }
        });

        self.paint_mouse_listeners(window);
    }
}

fn placeholder_line_runs<'a>(
    display_text: &'a str,
    runs: &[TextRun],
) -> Vec<(&'a str, Vec<TextRun>)> {
    let mut result = Vec::new();
    let mut line_offset = 0;

    for line in display_text.split('\n') {
        let line_runs = runs_for_range(runs, line_offset, &(0..line.len()));
        debug_assert_eq!(
            line_runs.iter().map(|run| run.len).sum::<usize>(),
            line.len()
        );
        result.push((line, line_runs));
        line_offset += line.len() + 1;
    }

    result
}

pub(super) fn runs_for_range(
    runs: &[TextRun],
    line_offset: usize,
    range: &Range<usize>,
) -> Vec<TextRun> {
    let mut result = vec![];
    let range = (line_offset + range.start)..(line_offset + range.end);
    let mut cursor = 0;

    for run in runs {
        let run_start = cursor;
        let run_end = cursor + run.len;

        if run_end <= range.start {
            cursor = run_end;
            continue;
        }
        if run_start >= range.end {
            break;
        }

        let start = range.start.max(run_start) - run_start;
        let end = range.end.min(run_end) - run_start;
        let len = end - start;
        if len > 0 {
            result.push(TextRun { len, ..run.clone() });
        }

        cursor = run_end;
    }

    result
}

pub(super) fn align_runs_to_char_boundaries(text: &str, runs: &[TextRun]) -> Option<Vec<TextRun>> {
    let mut end = 0;
    let aligned = runs.iter().all(|run| {
        end += run.len;
        end <= text.len() && text.is_char_boundary(end)
    });
    if aligned {
        return None;
    }

    let mut result = Vec::with_capacity(runs.len());
    let mut cursor = 0;
    let mut raw_end = 0;
    for run in runs {
        raw_end = (raw_end + run.len).min(text.len());
        let mut end = raw_end;
        while !text.is_char_boundary(end) {
            end += 1;
        }
        if end > cursor {
            result.push(TextRun {
                len: end - cursor,
                ..run.clone()
            });
            cursor = end;
        }
    }

    Some(result)
}

fn split_run_for_ime_underline(
    run: TextRun,
    run_range: Range<usize>,
    marked_range: Option<Range<usize>>,
    marked_underline: Option<UnderlineStyle>,
) -> SmallVec<[TextRun; 3]> {
    if run.len == 0 {
        return SmallVec::new();
    }

    let Some(marked) = marked_range else {
        return [run].into_iter().collect();
    };

    let intersection_start = run_range.start.max(marked.start);
    let intersection_end = run_range.end.min(marked.end);
    if intersection_start >= intersection_end {
        return [run].into_iter().collect();
    }

    [
        TextRun {
            len: intersection_start - run_range.start,
            ..run.clone()
        },
        TextRun {
            len: intersection_end - intersection_start,
            underline: marked_underline,
            ..run.clone()
        },
        TextRun {
            len: run_range.end - intersection_end,
            ..run
        },
    ]
    .into_iter()
    .filter(|run| run.len > 0)
    .collect()
}
