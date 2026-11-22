use crate::input::InputModeKind;
use gpui::Corners;
use gpui::Half;
use gpui::{
    AnyElement, App, Bounds, Edges, Element, ElementId, ElementInputHandler, Entity,
    GlobalElementId,
};
use gpui::{
    Hitbox, HitboxBehavior, Hsla, IntoElement, LayoutId, LongPressEvent,
    MouseButton, MouseMoveEvent, MouseUpEvent, ParentElement as _, Path, Pixels, Point, Position,
    SharedString, Size, Style, Styled as _, TextAlign, TextRun, TouchDragEvent,
    TouchPhase, UnderlineStyle, Window, fill, point, px, relative, size,
};
use ropey::Rope;
use smallvec::SmallVec;
use std::{ops::Range, rc::Rc};

use crate::{
    Scrollbar,
    input::{RopeExt as _, blink_cursor::CURSOR_WIDTH, display_map::LineLayout},
};

use super::{InputBaseState, PrepaintCaretGeometry, layout::LastLayout, mode::LayoutMode};

const BOTTOM_MARGIN_ROWS: usize = 3;
pub(super) const RIGHT_MARGIN: Pixels = px(10.);
pub(super) const LINE_NUMBER_RIGHT_MARGIN: Pixels = px(6.);

#[derive(Clone, Copy, Debug, PartialEq)]
struct EditorScrollbarLayout {
    bounds: Bounds<Pixels>,
    scroll_size: Size<Pixels>,
}

/// What the unwrapped width of the longest line depends on. While it holds,
/// prepaint reuses the stored width instead of copying the line and asking
/// the text system for it again on every frame.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct LongestLineKey {
    document_revision: u64,
    row: usize,
    len: usize,
    font: gpui::Font,
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
    fn new<M: InputModeKind>(
        input_bounds: Bounds<Pixels>,
        last_layout: &LastLayout,
        scroll_size: Size<Pixels>,
        cursor_scroll_offset: Point<Pixels>,
        state: &InputBaseState<M>,
    ) -> Self {
        Self {
            layout: EditorScrollbarLayout::new(
                input_bounds,
                last_layout.line_number_width,
                scroll_size,
                state.editor_paddings,
            ),
            cursor_scroll_offset,
            soft_wrap: state.soft_wrap,
        }
    }
}

impl EditorScrollbarLayout {
    fn new(
        input_bounds: Bounds<Pixels>,
        line_number_width: Pixels,
        scroll_size: Size<Pixels>,
        paddings: Edges<Pixels>,
    ) -> Self {
        let left = if line_number_width == px(0.) {
            px(0.)
        } else {
            paddings.left + line_number_width - LINE_NUMBER_RIGHT_MARGIN
        };

        Self {
            bounds: Bounds::new(
                point(
                    input_bounds.origin.x + left,
                    input_bounds.origin.y - paddings.top,
                ),
                size(
                    input_bounds.size.width - left + paddings.right,
                    input_bounds.size.height + paddings.top + paddings.bottom,
                ),
            ),
            scroll_size: size(
                scroll_size.width - left + paddings.right + RIGHT_MARGIN,
                scroll_size.height,
            ),
        }
    }
}

pub(super) struct EditorScrollbar<M: InputModeKind> {
    state: Entity<InputBaseState<M>>,
}

impl<M: InputModeKind> EditorScrollbar<M> {
    pub(super) fn new(state: Entity<InputBaseState<M>>) -> Self {
        Self { state }
    }
}

impl<M: InputModeKind> IntoElement for EditorScrollbar<M> {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl<M: InputModeKind> Element for EditorScrollbar<M> {
    type RequestLayoutState = ();
    type PrepaintState = Option<AnyElement>;

    fn id(&self) -> Option<ElementId> {
        Some("editor-scrollbar".into())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
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
        _: Option<&gpui::InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let state = self.state.read(cx);
        let Some(snapshot) = state.editor_scrollbar_snapshot.get() else {
            return None;
        };
        let scroll_handle = state.scroll_handle.clone();

        if scroll_handle.offset() != snapshot.cursor_scroll_offset {
            scroll_handle.set_offset(snapshot.cursor_scroll_offset);
        }

        let mut scrollbar = if !snapshot.soft_wrap {
            Scrollbar::new(&scroll_handle)
        } else {
            Scrollbar::vertical(&scroll_handle)
        }
        .viewport_bounds(snapshot.layout.bounds)
        .scroll_size(snapshot.layout.scroll_size)
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
        _: Option<&gpui::InspectorElementId>,
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

// Prepaint and retained state must use the same range, including after content shrinks.
pub(super) fn clamp_horizontal_scroll_offset(
    offset: Pixels,
    scroll_width: Pixels,
    input_width: Pixels,
    text_align: TextAlign,
) -> Pixels {
    let caret_clearance = if text_align == TextAlign::Left {
        px(0.)
    } else {
        -CURSOR_WIDTH
    };
    let min_offset = (input_width - scroll_width + caret_clearance).min(caret_clearance);
    offset.clamp(min_offset, px(0.))
}

fn clamp_auto_grow_vertical_scroll_offset(
    mode: &LayoutMode,
    scroll_top: Pixels,
    scroll_height: Pixels,
    input_height: Pixels,
) -> Pixels {
    if mode.is_auto_grow() {
        scroll_top.clamp((input_height - scroll_height).min(px(0.)), px(0.))
    } else {
        scroll_top
    }
}

use super::MASK_CHAR;

/// Convert a byte offset in the original text to a byte offset in the masked display string.
///
/// The masked string consists of `MASK_CHAR` repeated once per character in the original text.
/// Since `MASK_CHAR` may be multi-byte in UTF-8, the byte offset in the masked string is
/// `char_index * MASK_CHAR.len_utf8()`.
fn masked_display_offset(text: &Rope, original_offset: usize) -> usize {
    text.offset_to_char_index(original_offset) * MASK_CHAR.len_utf8()
}

/// Move the IME marked range (tracked against the original text) into the display text
/// coordinate space, so that a run boundary can't land inside a multi-byte `MASK_CHAR`
/// and panic text shaping on a non-char-boundary slice.
fn ime_marked_display_range(
    text: &Rope,
    marked_range: Option<Range<usize>>,
    masked: bool,
) -> Option<Range<usize>> {
    let marked = marked_range?;
    if masked {
        Some(masked_display_offset(text, marked.start)..masked_display_offset(text, marked.end))
    } else {
        Some(marked)
    }
}

/// Viewport capacity in display rows, independent of logical buffer lines.
///
/// Shared by both scroll-into-view paths (`layout_cursors` and
/// `scroll_to_with_padding`) so they agree on the visible-line count even
/// when soft-wrapped lines make logical and display rows diverge.
pub(super) fn viewport_visible_lines(viewport_height: Pixels, line_height: Pixels) -> usize {
    (viewport_height / line_height) as usize
}

/// Minimum pixel padding the cursor is kept clear of the viewport's
/// top/bottom edges before auto-scroll engages. Backs
/// [`InputBaseState::cursor_surrounding_lines`].
///
/// Auto-grow uses one line. Otherwise `None` falls back to the historical
/// heuristic ([`BOTTOM_MARGIN_ROWS`] lines, or one line on small
/// viewports); `Some(n)` uses `n` lines. The result is saturated against
/// half the viewport so an oversized override can't invert the
/// top/bottom thresholds into a scroll feedback loop. `visible_lines` is the
/// viewport capacity in display rows, independent of logical buffer lines.
pub(super) fn cursor_surrounding_padding(
    is_auto_grow: bool,
    override_lines: Option<usize>,
    visible_lines: usize,
    line_height: Pixels,
) -> Pixels {
    if is_auto_grow {
        return line_height;
    }
    let raw = match override_lines {
        Some(lines) => lines as f32 * line_height,
        None => {
            if visible_lines < BOTTOM_MARGIN_ROWS * 8 {
                line_height
            } else {
                BOTTOM_MARGIN_ROWS * line_height
            }
        }
    };
    // Saturate against half the viewport so top + bottom margins can coexist.
    let viewport_half = (visible_lines as f32 * line_height).half();
    raw.min(viewport_half)
}

pub(super) struct TextElement<M: InputModeKind> {
    pub(crate) state: Entity<InputBaseState<M>>,
    placeholder: SharedString,
}

impl<M: InputModeKind> TextElement<M> {
    pub(super) fn new(state: Entity<InputBaseState<M>>) -> Self {
        Self {
            state,
            placeholder: SharedString::default(),
        }
    }

    /// Set the placeholder text of the input field.
    pub(super) fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    fn paint_mouse_listeners(&mut self, hitbox: &Hitbox, window: &mut Window, _: &mut App) {
        // Every touch is offered as a drag first; that is how a tap's mouse
        // events are later told apart from a mouse's.
        window.on_mouse_event(move |event: &TouchDragEvent, phase, _, cx| {
            if phase.capture() && event.phase == TouchPhase::Started {
                crate::GlobalState::note_touch(cx);
            }
        });

        // A long press is touch's way to select: the word under the finger,
        // then whatever the finger sweeps over. Claiming it keeps the moves
        // out of the pan recognizer, so the input does not scroll instead.
        window.on_mouse_event({
            let state = self.state.clone();
            let hitbox = hitbox.clone();
            move |event: &LongPressEvent, phase, window, cx| {
                if !phase.bubble() {
                    return;
                }
                if event.phase == TouchPhase::Started {
                    if window.default_prevented() || !hitbox.is_hovered(window) {
                        return;
                    }
                    if !state.update(cx, |state, cx| state.on_long_press(event, window, cx)) {
                        return;
                    }
                    window.capture_long_press(&state);
                } else if !window.has_long_press_capture(&state) {
                    return;
                } else {
                    state.update(cx, |state, cx| {
                        state.on_long_press(event, window, cx);
                    });
                }
                window.prevent_default();
                cx.stop_propagation();
            }
        });

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

                // Stop auto-scroll when mouse up, and also stop selecting.
                state.update(cx, |state, _| {
                    state.auto_scroll.stop();
                    state.selecting = false;
                    state.column_select_start = None;
                });
            }
        });
    }

    /// Lays out the carets and updates the scroll position for the active one.
    fn layout_cursors(
        &self,
        last_layout: &LastLayout,
        bounds: &mut Bounds<Pixels>,
        scroll_size: Size<Pixels>,
        _: &mut Window,
        cx: &mut App,
    ) -> (Vec<CursorRenderInfo>, Point<Pixels>, Option<usize>) {
        let state = self.state.read(cx);

        let line_height = last_layout.line_height;
        let visible_range = &last_layout.visible_range;
        let lines = &last_layout.lines;
        let line_number_width = last_layout.line_number_width;

        let active_id = state.active_selection().id;
        let mut scroll_offset = state.scroll_handle.offset();
        let mut current_row = None;
        let mut cursor_infos: Vec<CursorRenderInfo> = Vec::with_capacity(state.selections.len());

        // Padding kept between the cursor and the viewport's top/bottom
        // edges, used by the auto-scroll-into-view computation below.
        let top_bottom_margin = cursor_surrounding_padding(
            state.mode.is_auto_grow(),
            state.cursor_surrounding_lines,
            viewport_visible_lines(bounds.size.height, line_height),
            line_height,
        );

        // Resolve a cursor or selection endpoint to a content-space position.
        let visible_buffer_lines = &last_layout.visible_buffer_lines;
        let caret_for = |row: usize, offset: usize, affinity: bool| -> Point<Pixels> {
            // y of the top of buffer line `row` in content space.
            let top = line_height * state.display_map.buffer_line_to_display_row(row);
            let line_origin = point(px(0.), top);

            if let Some(vi) = visible_buffer_lines.iter().position(|&bl| bl == row) {
                let line = &lines[vi];
                let line_start = last_layout.visible_line_byte_offsets[vi];
                let local = offset.saturating_sub(line_start);
                if let Some(pos) = line.position_for_index(local, last_layout, affinity) {
                    return line_origin + pos;
                }
            }
            line_origin
        };

        let cursor_height = 0.85 * line_height;

        for selection in state.selections.iter() {
            let is_active = selection.id == active_id;

            let mut selected_range = *selection;
            let mut cursor = selection.cursor_offset();
            if is_active {
                if let Some(ime_marked_range) = &state.ime_marked_range {
                    selected_range = (ime_marked_range.end..ime_marked_range.end).into();
                    cursor = ime_marked_range.end;
                }
            }
            let is_selected_all = selected_range.len() == state.text.len();

            // Buffer rows from the raw (pre-mask) offsets, used to locate the cursor line.
            let cursor_row = state.text.offset_to_point(cursor).row;

            // Skip inactive cursors that are far outside the visible range. The
            // active cursor is always processed so scroll tracking keeps working.
            if !is_active
                && (cursor_row + 2 < visible_range.start || cursor_row > visible_range.end + 2)
            {
                continue;
            }

            let sel_start_row = state.text.offset_to_point(selected_range.start).row;
            let sel_end_row = state.text.offset_to_point(selected_range.end).row;
            if state.masked {
                selected_range.start = masked_display_offset(&state.text, selected_range.start);
                selected_range.end = masked_display_offset(&state.text, selected_range.end);
                cursor = masked_display_offset(&state.text, cursor);
            }

            let affinity = is_active && state.cursor_line_end_affinity;
            let cursor_pos = caret_for(cursor_row, cursor, affinity);
            let cursor_start = caret_for(sel_start_row, selected_range.start, false);
            let cursor_end = caret_for(sel_end_row, selected_range.end, false);

            if is_active {
                current_row = Some(cursor_row);

                let selection_changed = state.last_selected_range != Some(selected_range);
                let auto_scrolling = state.auto_scroll.is_active();
                if selection_changed && !is_selected_all {
                    // For Right alignment use 0 margin: cursor is clamped to bounds separately,
                    // so we never scroll the text for cursor-at-edge, avoiding a first-click jump.
                    let safety_margin = match last_layout.text_align {
                        TextAlign::Left => RIGHT_MARGIN,
                        TextAlign::Right => px(0.),
                        TextAlign::Center => CURSOR_WIDTH,
                    };

                    scroll_offset.x = if scroll_offset.x + cursor_pos.x
                        > (bounds.size.width - line_number_width - safety_margin)
                    {
                        // cursor is out of right
                        bounds.size.width - line_number_width - safety_margin - cursor_pos.x
                    } else if scroll_offset.x + cursor_pos.x < px(0.) {
                        // cursor is out of left
                        -cursor_pos.x
                    } else {
                        scroll_offset.x
                    };

                    // Vertical cursor-follow is suppressed while auto-scroll manages the y axis,
                    // to prevent fighting the background scroll task.
                    if !auto_scrolling {
                        // Scroll straight to the caret's line. This runs only on
                        // the frame the selection changed, so a one-line step
                        // would leave a far-off caret (e.g. after typing at the
                        // end of a long paste) outside the viewport.
                        scroll_offset.y = if scroll_offset.y + cursor_pos.y
                            > bounds.size.height - top_bottom_margin
                        {
                            // cursor is out of bottom
                            bounds.size.height - top_bottom_margin - cursor_pos.y
                        } else if scroll_offset.y + cursor_pos.y < top_bottom_margin {
                            // cursor is out of top
                            (top_bottom_margin - cursor_pos.y).min(px(0.))
                        } else {
                            scroll_offset.y
                        };
                    }

                    // For selection to move scroll
                    if selection.reversed {
                        if scroll_offset.x + cursor_start.x < px(0.) {
                            // selection start is out of left
                            scroll_offset.x = -cursor_start.x;
                        }
                        if !auto_scrolling && scroll_offset.y + cursor_start.y < px(0.) {
                            // selection start is out of top
                            scroll_offset.y = -cursor_start.y;
                        }
                    } else {
                        // TODO: Consider to remove this part,
                        // maybe is not necessary (But selection_reversed is needed).
                        if scroll_offset.x + cursor_end.x <= px(0.) {
                            // selection end is out of left
                            scroll_offset.x = -cursor_end.x;
                        }
                        if !auto_scrolling && scroll_offset.y + cursor_end.y <= px(0.) {
                            // selection end is out of top
                            scroll_offset.y = -cursor_end.y;
                        }
                    }
                }
            }

            // Apply the final horizontal offset to every caret after cursor-follow and
            // deferred scrolling have been resolved, regardless of selection order.
            let cursor_x = bounds.left() + cursor_pos.x + line_number_width;
            cursor_infos.push(CursorRenderInfo {
                bounds: Bounds::new(
                    point(
                        cursor_x,
                        bounds.top() + cursor_pos.y + ((line_height - cursor_height) / 2.),
                    ),
                    size(CURSOR_WIDTH, cursor_height),
                ),
                is_active,
            });
        }

        if let Some(deferred_scroll_offset) = state.deferred_scroll_offset {
            scroll_offset = deferred_scroll_offset;
        }
        scroll_offset.x = clamp_horizontal_scroll_offset(
            scroll_offset.x,
            scroll_size.width,
            bounds.size.width,
            last_layout.text_align,
        );
        for info in &mut cursor_infos {
            info.bounds.origin.x += scroll_offset.x;
            // Right-aligned text keeps the caret inside the viewport edge.
            if last_layout.text_align == TextAlign::Right {
                info.bounds.origin.x = info.bounds.origin.x.min(bounds.right() - CURSOR_WIDTH);
            }
        }
        scroll_offset.y = clamp_auto_grow_vertical_scroll_offset(
            &state.mode,
            scroll_offset.y,
            scroll_size.height,
            bounds.size.height,
        );

        bounds.origin = bounds.origin + scroll_offset;

        (cursor_infos, scroll_offset, current_row)
    }

    /// Layout the match range to a Path.
    pub(crate) fn layout_match_range(
        range: Range<usize>,
        last_layout: &LastLayout,
        bounds: &Bounds<Pixels>,
    ) -> Option<Path<Pixels>> {
        let corners = Self::layout_range_corners(&range, last_layout)?;
        let points = frame_outline_points(&corners);
        let origin = bounds.origin + point(last_layout.line_number_width, px(0.));
        let mut builder = gpui::PathBuilder::fill();
        builder.move_to(origin + *points.first()?);
        for point in points.iter().skip(1) {
            builder.line_to(origin + *point);
        }
        builder.close();
        builder.build().ok()
    }

    /// Project a buffer range through the visible (non-folded) shaped lines.
    /// Half-open intersections give soft-wrap ends their trailing affinity and
    /// prevent ranges on later lines from painting an earlier line's first glyph.
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
            let alignment = last_layout.alignment_offset(line.longest_width);
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
                // A selected newline has a one-space cell, including empty lines.
                // A wrap boundary is not a newline and must not get such a cell.
                let newline = is_last && range.start <= end && range.end > end;
                if start_ix < end_ix || newline {
                    let indent = if row == 0 { px(0.) } else { line.wrap_indent };
                    let left = alignment + indent + shaped.x_for_index(start_ix.min(end) - offset);
                    let right = alignment
                        + indent
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

    fn layout_selections(
        &self,
        last_layout: &LastLayout,
        bounds: &mut Bounds<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) -> Vec<Path<Pixels>> {
        let state = self.state.read(cx);
        if !state.has_selection_focus(window, cx) {
            return vec![];
        }

        let active_id = state.active_selection().id;
        let mut paths = Vec::new();

        for selection in state.selections.iter() {
            let is_active = selection.id == active_id;
            let mut selected_range = *selection;

            // IME composition replaces the active selection with a collapsed
            // caret, so it never paints a selection highlight.
            if is_active {
                if let Some(ime_marked_range) = &state.ime_marked_range {
                    if !ime_marked_range.is_empty() {
                        selected_range = (ime_marked_range.end..ime_marked_range.end).into();
                    }
                }
            }
            if selected_range.is_empty() {
                continue;
            }

            if state.masked {
                selected_range.start = masked_display_offset(&state.text, selected_range.start);
                selected_range.end = masked_display_offset(&state.text, selected_range.end);
            }

            let (start_ix, end_ix) = if selected_range.start < selected_range.end {
                (selected_range.start, selected_range.end)
            } else {
                (selected_range.end, selected_range.start)
            };

            let range = start_ix.max(last_layout.visible_range_offset.start)
                ..end_ix.min(last_layout.visible_range_offset.end);

            if range.is_empty() {
                continue;
            }

            // A selected token shows its own selected state, so the text
            // highlight stops at its edges instead of painting behind it.
            let mut start = range.start;
            if state.tokens_visible() {
                let spans = state.token_spans();
                let first = spans.partition_point(|s| s.range().end <= range.start);
                for span in spans[first..]
                    .iter()
                    .take_while(|s| s.range().start < range.end)
                {
                    if start < span.range().start {
                        paths.extend(Self::layout_match_range(
                            start..span.range().start,
                            last_layout,
                            bounds,
                        ));
                    }
                    start = span.range().end.max(start);
                }
            }
            if start < range.end {
                paths.extend(Self::layout_match_range(
                    start..range.end,
                    last_layout,
                    bounds,
                ));
            }
        }

        paths
    }

    /// Calculate the visible range of lines in the viewport.
    ///
    /// Returns
    ///
    /// - visible_range: The visible range is based on unwrapped lines (Zero based).
    /// - visible_buffer_lines: Indices of non-hidden buffer lines within the visible range.
    /// - visible_top: The top position of the first visible line in the scroll viewport.
    fn calculate_visible_range(
        &self,
        state: &InputBaseState<M>,
        line_height: Pixels,
        input_height: Pixels,
    ) -> (Range<usize>, Vec<usize>, Pixels, Range<usize>) {
        // Add extra rows to avoid showing empty space when scroll to bottom.
        let extra_rows = 1;
        if state.is_single_line() {
            return (0..1, vec![0], px(0.), 0..1);
        }

        let total_lines = state.display_map.wrap_row_count();
        let display_count = state.display_map.display_row_count();
        let buffer_line_count = state.display_map.buffer_line_count();
        if display_count == 0 || buffer_line_count == 0 {
            return (0..0, Vec::new(), px(0.), 0..0);
        }

        let mut scroll_top = if let Some(deferred_scroll_offset) = state.deferred_scroll_offset {
            deferred_scroll_offset.y
        } else {
            state.scroll_handle.offset().y
        };
        scroll_top = clamp_auto_grow_vertical_scroll_offset(
            &state.mode,
            scroll_top,
            line_height * total_lines,
            input_height,
        );

        // Display rows are uniformly `line_height` tall, so the visible window maps
        // directly to a display-row range.
        let viewport_top = (-scroll_top).max(px(0.));
        let viewport_bottom = viewport_top + input_height;
        let line_height_f = f32::from(line_height);
        let first_display =
            ((f32::from(viewport_top) / line_height_f).floor() as usize).min(display_count - 1);
        let last_display =
            ((f32::from(viewport_bottom) / line_height_f).ceil() as usize).min(display_count - 1);
        let visible_display_rows =
            first_display.saturating_sub(2)..(last_display + 3).min(display_count);

        let start_line = state.display_map.display_row_to_buffer_line(first_display);
        let end_line = state.display_map.display_row_to_buffer_line(last_display);

        // y of the top of the first visible buffer line (in content space).
        let visible_top = match state
            .display_map
            .buffer_line_to_display_row_range(start_line)
        {
            Some(range) => line_height * range.start,
            None => line_height * first_display,
        };

        let visible_range = start_line..(end_line + 1 + extra_rows).min(buffer_line_count);

        // Collect non-hidden buffer lines within the visible range
        let mut visible_buffer_lines = Vec::with_capacity(visible_range.len());
        for ix in visible_range.clone() {
            if state.display_map.visible_wrap_row_count_for_buffer_line(ix) > 0 {
                visible_buffer_lines.push(ix);
            }
        }

        (
            visible_range,
            visible_buffer_lines,
            visible_top,
            visible_display_rows,
        )
    }

    fn token_element(
        &self,
        token: &super::InlineTokenContext,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        use gpui::{
            InteractiveElement as _, StatefulInteractiveElement as _, prelude::FluentBuilder as _,
        };
        let presentation = self.state.read(cx).token_presentation.clone();
        let child = presentation.render(token, window, cx);
        // Preserve interaction state only for the same token occurrence and
        // hover eligibility. Replacements and re-enabling start fresh.
        let start = token.range().start;
        let down_state = self.state.clone();
        let click_state = self.state.clone();
        let move_state = self.state.clone();
        let hover_state = self.state.clone();
        let hover_token = token.token().clone();
        let disabled = token.is_disabled();
        let hover_enabled = presentation.has_hover_listener() && !disabled;
        let id = gpui::ElementId::from(("inline-token", start));
        let id = gpui::ElementId::from((id, token.token().id().clone()));
        let id = gpui::ElementId::from((id, token.token().text().clone()));
        let id = gpui::ElementId::from((id, token.token().label().clone()));
        let id = gpui::ElementId::from((
            id,
            gpui::SharedString::from(if hover_enabled {
                "hover-enabled"
            } else {
                "hover-disabled"
            }),
        ));
        let accessible = presentation.has_listener() && !disabled;
        let accessible_state = self.state.clone();
        gpui::div()
            .id(id)
            .flex()
            .items_center()
            .h(token.line_height())
            .max_w(token.available_width())
            .overflow_hidden()
            // A token is an object, not text: the arrow, never the I-beam.
            .cursor_default()
            .when(accessible, |this| {
                this.role(gpui::Role::Button)
                    .aria_label(token.token().label().clone())
                    .on_a11y_action(gpui::AccessibleAction::Click, move |_, window, cx| {
                        let state = accessible_state.read(cx);
                        let activation = state
                            .token_spans()
                            .iter()
                            .find(|s| s.range().start == start)
                            .and_then(|span| {
                                state.range_to_bounds(&span.range()).and_then(|bounds| {
                                    state.token_activation(
                                        start,
                                        bounds,
                                        gpui::ClickEvent::Keyboard(gpui::KeyboardClickEvent {
                                            bounds,
                                            ..Default::default()
                                        }),
                                    )
                                })
                            });
                        if let Some((listener, event)) = activation {
                            listener(&event, window, cx);
                        }
                    })
            })
            .capture_any_mouse_down(move |_, window, cx| {
                if disabled {
                    window.prevent_default();
                    cx.stop_propagation();
                }
            })
            .capture_any_mouse_up(move |_, window, cx| {
                if disabled {
                    window.prevent_default();
                    cx.stop_propagation();
                }
            })
            .capture_key_down(move |_, _, cx| {
                if disabled {
                    cx.stop_propagation();
                }
            })
            .on_mouse_down(MouseButton::Left, move |event, window, cx| {
                if window.default_prevented() {
                    return;
                }
                down_state.update(cx, |state, cx| {
                    state.pressed_token = None;
                    if event.click_count == 2 {
                        if let Some(span) = state
                            .token_spans()
                            .iter()
                            .find(|s| s.range().start == start)
                        {
                            state.set_selected_range(span.range(), cx);
                            state.focus(window, cx);
                            window.prevent_default();
                        }
                    } else if event.click_count == 1
                        && !event.modifiers.shift
                        && !event.modifiers.alt
                    {
                        state.pressed_token =
                            Some((start, state.document_revision, event.position));
                    }
                });
            })
            .on_mouse_move(move |event, _, cx| {
                move_state.update(cx, |state, _| {
                    if state.pressed_token.as_ref().is_some_and(|(_, _, p)| {
                        (event.position.x - p.x).abs() + (event.position.y - p.y).abs() > px(4.)
                    }) {
                        state.pressed_token = None;
                    }
                });
            })
            .on_click(move |event, window, cx| {
                let activation = click_state.update(cx, |state, cx| {
                    let (pressed, revision, _) = state.pressed_token.take()?;
                    if pressed != start || revision != state.document_revision {
                        return None;
                    }
                    if event.modifiers().shift || event.modifiers().alt {
                        return None;
                    }
                    let span = state
                        .token_spans()
                        .iter()
                        .find(|s| s.range().start == start)?
                        .clone();
                    // A click selects the whole token; opening it is the
                    // listener's decision.
                    state.set_selected_range(span.range(), cx);
                    let bounds = state.range_to_bounds(&span.range())?;
                    state.token_activation(start, bounds, event.clone())
                });
                if let Some((listener, event)) = activation {
                    listener(&event, window, cx);
                }
            })
            .on_hover(move |hovered, window, cx| {
                // Hover never selects or edits; it only reports presence so the
                // application can show a tooltip or run custom logic.
                let hovered = *hovered;
                let activation = hover_state.update(cx, |state, _| {
                    // Anchor to the real laid-out token rect, not the
                    // range-to-bounds guess: a token at a soft-wrap boundary
                    // would otherwise report a zero/negative width or a
                    // two-row height.
                    let bounds = if hovered {
                        state.token_bounds.get(&start).copied()?
                    } else {
                        // Exits use entry geometry even if an earlier callback
                        // removed the token and its current bounds.
                        Bounds::default()
                    };
                    state.token_hover(start, bounds, hovered, Some(&hover_token))
                });
                if let Some((listener, event)) = activation {
                    listener(&event, window, cx);
                }
            })
            .child(child)
            .into_any_element()
    }

    fn measure_tokens(
        &self,
        width: Pixels,
        line_height: Pixels,
        viewport: Pixels,
        window: &mut Window,
        cx: &mut App,
    ) -> std::collections::HashMap<usize, (AnyElement, Size<Pixels>)> {
        let style = window.text_style();
        let state = self.state.read(cx);
        let key = (
            style.font(),
            style.font_size.to_pixels(window.rem_size()),
            width,
            line_height,
            state.is_single_line() || !state.soft_wrap,
        );
        if !state.tokens_visible() {
            if state.token_layout_cache.is_none() {
                let mut exit = self.state.update(cx, |state, _| {
                    state.token_bounds.clear();
                    state.reconcile_token_hover()
                });
                while let Some((listener, event)) = exit {
                    listener(&event, window, cx);
                    exit = self
                        .state
                        .update(cx, |state, _| state.reconcile_token_hover());
                }
                return Default::default();
            }
            self.state.update(cx, |state, cx| {
                state.display_map.set_inline_metrics(Rc::from([]), cx)
            });
            let mut exit = self.state.update(cx, |state, _| {
                state.token_bounds.clear();
                state.reconcile_token_hover()
            });
            while let Some((listener, event)) = exit {
                listener(&event, window, cx);
                exit = self
                    .state
                    .update(cx, |state, _| state.reconcile_token_hover());
            }
            return Default::default();
        }
        let revision = state.document_revision;
        let cache = state.token_layout_cache.as_ref();
        let all = cache.is_none_or(|cache| {
            cache.key.as_ref() != Some(&key)
                || cache.revision != revision
                || state
                    .token_spans()
                    .iter()
                    .any(|span| !cache.widths.contains_key(span.token()))
        });
        let (visible, _, _, _) = self.calculate_visible_range(state, line_height, viewport);
        let start = state.text.line_start_offset(visible.start);
        let end = state.text.line_end_offset(visible.end.saturating_sub(1));
        let spans = state.token_spans();
        let first = if all {
            0
        } else {
            spans.partition_point(|s| s.range().end <= start)
        };
        let contexts: Vec<_> = spans[first..]
            .iter()
            .take_while(|s| all || s.range().start <= end)
            .filter(|span| {
                let range = span.range();
                (range.start <= end && range.end >= start)
                    || cache.is_none_or(|cache| {
                        cache.key.as_ref() != Some(&key) || !cache.widths.contains_key(span.token())
                    })
            })
            .map(|span| state.token_context(span, line_height, width))
            .collect();
        let mut elements = std::collections::HashMap::new();
        let mut measured = Vec::new();
        for token in contexts {
            let mut element = self.token_element(&token, window, cx);
            let size = element.layout_as_root(
                size(
                    gpui::AvailableSpace::MaxContent,
                    gpui::AvailableSpace::Definite(line_height),
                ),
                window,
                cx,
            );
            measured.push((token.token().clone(), size.width.min(width).max(px(1.))));
            elements.insert(token.range().start, (element, size));
        }
        self.state.update(cx, |state, cx| {
            let mut cache = state.token_layout_cache.take().unwrap_or_default();
            let mut changed = cache.key.as_ref() != Some(&key) || cache.revision != revision;
            if cache.key.as_ref() != Some(&key) {
                cache.widths.clear();
            }
            for (token, width) in measured {
                changed |= cache.widths.get(&token) != Some(&width);
                cache.widths.insert(token, width);
            }
            cache.key = Some(key);
            if changed {
                let spans = state.token_spans();
                let tokens: std::collections::HashSet<_> =
                    spans.iter().map(|s| s.token()).collect();
                cache.widths.retain(|token, _| tokens.contains(token));
                cache.metrics = spans
                    .iter()
                    .filter_map(|span| {
                        cache
                            .widths
                            .get(span.token())
                            .map(|width| (span.range(), *width))
                    })
                    .collect();
                cache.revision = revision;
                if state.is_single_line() || !state.soft_wrap {
                    let mut rows: Vec<_> = spans
                        .iter()
                        .map(|s| state.text.offset_to_point(s.range().start).row)
                        .collect();
                    rows.push(state.display_map.longest_row());
                    rows.sort_unstable();
                    rows.dedup();
                    cache.unwrapped_width = rows
                        .into_iter()
                        .map(|row| {
                            let start = state.text.line_start_offset(row);
                            let text = state.text.slice_line(row).to_string();
                            let mut offset = 0;
                            let mut width = px(0.);
                            let first = spans.partition_point(|s| s.range().end <= start);
                            for span in spans[first..]
                                .iter()
                                .take_while(|s| s.range().start < start + text.len())
                            {
                                let local = span.range().start - start..span.range().end - start;
                                let part = &text[offset..local.start];
                                width += window
                                    .text_system()
                                    .shape_line(
                                        part.to_owned().into(),
                                        style.font_size.to_pixels(window.rem_size()),
                                        &[style.to_run(part.len())],
                                        None,
                                    )
                                    .width;
                                width +=
                                    cache.widths.get(span.token()).copied().unwrap_or_default();
                                offset = local.end;
                            }
                            let part = &text[offset..];
                            width
                                + window
                                    .text_system()
                                    .shape_line(
                                        part.to_owned().into(),
                                        style.font_size.to_pixels(window.rem_size()),
                                        &[style.to_run(part.len())],
                                        None,
                                    )
                                    .width
                        })
                        .max()
                        .unwrap_or_default();
                }
            }
            let metrics = cache.metrics.clone();
            state.token_layout_cache = Some(cache);
            state.display_map.set_inline_metrics(metrics, cx);
            if state.mode.is_auto_grow() {
                let rows = state.mode.rows();
                state.mode.update_auto_grow(&state.display_map);
                if state.mode.rows() != rows {
                    cx.notify();
                }
            }
        });
        elements
    }

    fn layout_token_lines(
        state: &InputBaseState<M>,
        last_layout: &LastLayout,
        font_size: Pixels,
        runs: &[TextRun],
        window: &mut Window,
    ) -> Vec<LineLayout> {
        use crate::input::display_map::{InlineFragment, InputLine};
        let spans = state.token_spans();
        let cache = state
            .token_layout_cache
            .as_ref()
            .expect("tokens measured before shaping");
        let mut run_offset = 0;
        last_layout
            .visible_buffer_lines
            .iter()
            .map(|&row| {
                let line_start = state.text.line_start_offset(row);
                let text: String = state.text.slice_line(row).into();
                let ranges = if state.is_single_line() {
                    smallvec::smallvec![0..text.len()]
                } else {
                    state
                        .display_map
                        .line(row)
                        .expect("prepared line")
                        .wrapped_lines
                        .clone()
                };
                let mut lines: SmallVec<[InputLine; 1]> = SmallVec::new();
                for range in ranges {
                    let mut fragments = Vec::new();
                    let mut offset = range.start;
                    let mut x = px(0.);
                    let first =
                        spans.partition_point(|s| s.range().end <= line_start + range.start);
                    for span in spans[first..]
                        .iter()
                        .take_while(|s| s.range().start < line_start + range.end)
                    {
                        let local = span.range().start - line_start..span.range().end - line_start;
                        if offset < local.start {
                            let part = offset..local.start;
                            let shaped = window.text_system().shape_line(
                                text[part.clone()].to_owned().into(),
                                font_size,
                                &runs_for_range(runs, run_offset, &part),
                                None,
                            );
                            let width = shaped.width;
                            fragments.push(InlineFragment {
                                range: part.start - range.start..part.end - range.start,
                                x,
                                width,
                                text: Some(shaped),
                            });
                            x += width;
                        }
                        let width = cache.widths.get(span.token()).copied().unwrap_or_default();
                        fragments.push(InlineFragment {
                            range: local.start - range.start..local.end - range.start,
                            x,
                            width,
                            text: None,
                        });
                        x += width;
                        offset = local.end;
                    }
                    if offset < range.end {
                        let part = offset..range.end;
                        let shaped = window.text_system().shape_line(
                            text[part.clone()].to_owned().into(),
                            font_size,
                            &runs_for_range(runs, run_offset, &part),
                            None,
                        );
                        let width = shaped.width;
                        fragments.push(InlineFragment {
                            range: part.start - range.start..part.end - range.start,
                            x,
                            width,
                            text: Some(shaped),
                        });
                    }
                    lines.push(InputLine::inline(text[range].to_owned().into(), fragments));
                }
                let indent = state.display_map.line(row).map_or(0, |line| line.indent);
                let wrap_indent = if indent > 0 && lines.len() > 1 {
                    let bytes = text
                        .char_indices()
                        .nth(indent as usize)
                        .map_or(text.len(), |(ix, _)| ix);
                    lines[0].x_for_index(bytes)
                } else {
                    px(0.)
                };
                run_offset += text.len() + 1;
                LineLayout::new()
                    .inline_lines(lines)
                    .wrap_indent(wrap_indent)
            })
            .collect()
    }

    fn prepaint_tokens(
        &self,
        layout: &LastLayout,
        bounds: Bounds<Pixels>,
        mut measured: std::collections::HashMap<usize, (AnyElement, Size<Pixels>)>,
        window: &mut Window,
        cx: &mut App,
    ) -> Vec<AnyElement> {
        let state = self.state.read(cx);
        if !state.tokens_visible() {
            let mut exit = self.state.update(cx, |state, _| {
                state.token_bounds.clear();
                state.reconcile_token_hover()
            });
            while let Some((listener, event)) = exit {
                listener(&event, window, cx);
                exit = self
                    .state
                    .update(cx, |state, _| state.reconcile_token_hover());
            }
            return vec![];
        }
        let width = state
            .token_layout_cache
            .as_ref()
            .and_then(|c| c.key.as_ref())
            .map_or(bounds.size.width, |k| k.2);
        let mut placements = Vec::new();
        let mut y = layout.visible_top;
        for (ix, &row) in layout.visible_buffer_lines.iter().enumerate() {
            let start = state.text.line_start_offset(row);
            let end = state.text.line_end_offset(row);
            let spans = state.token_spans();
            let first = spans.partition_point(|s| s.range().end <= start);
            for span in spans[first..].iter().take_while(|s| s.range().start < end) {
                if let Some(position) =
                    layout.lines[ix].position_for_index(span.range().start - start, layout, false)
                {
                    placements.push((
                        state.token_context(span, layout.line_height, width),
                        bounds.origin + position + point(layout.line_number_width, y),
                    ));
                }
            }
            y += layout.lines[ix].size(layout.line_height).height;
        }
        let mut out = Vec::new();
        let mut token_bounds = std::collections::HashMap::new();
        for (token, origin) in placements {
            let (mut element, element_size) = match measured.remove(&token.range().start) {
                Some(found) => found,
                None => {
                    let mut element = self.token_element(&token, window, cx);
                    let element_size = element.layout_as_root(
                        size(
                            gpui::AvailableSpace::MaxContent,
                            gpui::AvailableSpace::Definite(layout.line_height),
                        ),
                        window,
                        cx,
                    );
                    (element, element_size)
                }
            };
            element.prepaint_at(origin, window, cx);
            token_bounds.insert(token.range().start, Bounds::new(origin, element_size));
            out.push(element);
        }
        let mut exit = self.state.update(cx, |state, _| {
            state.token_bounds = token_bounds;
            state.reconcile_token_hover()
        });
        while let Some((listener, event)) = exit {
            listener(&event, window, cx);
            exit = self
                .state
                .update(cx, |state, _| state.reconcile_token_hover());
        }
        out
    }

    fn layout_lines(
        state: &InputBaseState<M>,
        display_text: &Rope,
        last_layout: &LastLayout,
        visible_display_rows: &Range<usize>,
        font_size: Pixels,
        runs: &[TextRun],
        window: &mut Window,
    ) -> Vec<LineLayout> {
        use crate::input::display_map::InputLine;

        let is_single_line = state.is_single_line();
        if state.tokens_visible() {
            return Self::layout_token_lines(state, last_layout, font_size, runs, window);
        }

        if is_single_line {
            let text: SharedString = display_text.to_string().into();
            let aligned_runs = align_runs_to_char_boundaries(&text, runs);
            let line_runs = aligned_runs.as_deref().unwrap_or(runs);
            let shaped_line = window
                .text_system()
                .shape_line(text, font_size, line_runs, None);

            let line_layout = LineLayout::new()
                .lines(smallvec::smallvec![shaped_line])
                .with_background(has_background(line_runs));
            return vec![line_layout];
        }

        // Empty to use placeholder, the placeholder is not in the wrapper map.
        if state.text.len() == 0 {
            let placeholder_text = display_text.to_string();
            let mut placeholder_lines = SmallVec::new();
            let mut line_has_background = false;

            for (line, line_runs) in placeholder_line_runs(&placeholder_text, runs) {
                let shaped_line = window.text_system().shape_line(
                    line.to_string().into(),
                    font_size,
                    &line_runs,
                    None,
                );
                line_has_background |= has_background(&line_runs);
                placeholder_lines.push(shaped_line);
            }

            let line_layout = LineLayout::new()
                .lines(placeholder_lines)
                .with_background(line_has_background);
            return vec![line_layout];
        }

        let mut lines = Vec::with_capacity(last_layout.visible_buffer_lines.len());
        let mut run_offset = 0;
        let mut display_row = last_layout
            .visible_buffer_lines
            .first()
            .map_or(0, |&row| state.display_map.buffer_line_to_display_row(row));

        for (vi, &buffer_line) in last_layout.visible_buffer_lines.iter().enumerate() {
            let line_slice = display_text.slice_line(buffer_line);
            let line_len = line_slice.len();
            let line_byte_start = last_layout.visible_line_byte_offsets[vi];
            let line_item = state
                .display_map
                .line(buffer_line)
                .expect("line should exists in wrapper");

            debug_assert_eq!(line_item.len(), line_len);

            let mut wrapped_lines: SmallVec<[InputLine; 1]> =
                SmallVec::with_capacity(line_item.wrapped_lines.len());
            let mut line_has_background = false;

            for (sub_ix, range) in line_item.wrapped_lines.iter().enumerate() {
                let sub_start = line_byte_start + range.start;
                let sub_end = line_byte_start + range.end;
                let has_cursor = state.selections.iter().any(|sel| {
                    let c = sel.cursor_offset();
                    c >= sub_start && c <= sub_end
                });
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

                    line_has_background |= has_background(&line_runs);
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

            let line_layout = LineLayout::new()
                .inline_lines(wrapped_lines)
                .wrap_indent(wrap_indent)
                .with_background(line_has_background);
            lines.push(line_layout);

            // +1 for the `\n`
            run_offset += line_len + 1;
        }

        lines
    }
}

/// Layout data for a single caret, produced by [`TextElement::layout_cursors`].
#[derive(Clone, Debug)]
struct CursorRenderInfo {
    bounds: Bounds<Pixels>,
    is_active: bool,
}

pub(super) struct PrepaintState {
    last_layout: LastLayout,
    token_elements: Vec<AnyElement>,
    scroll_size: Size<Pixels>,
    cursor_infos: Vec<CursorRenderInfo>,
    cursor_scroll_offset: Point<Pixels>,
    selection_paths: Vec<Path<Pixels>>,
    hitbox: Hitbox,
    bounds: Bounds<Pixels>,
}

impl PrepaintState {
    /// Returns all cursor infos adjusted for scroll offset.
    fn cursor_infos_with_scroll(&self) -> Vec<CursorRenderInfo> {
        self.cursor_infos
            .iter()
            .map(|info| {
                let mut bounds = info.bounds;
                bounds.origin.y += self.cursor_scroll_offset.y;
                CursorRenderInfo {
                    bounds,
                    is_active: info.is_active,
                }
            })
            .collect()
    }
}

impl<M: InputModeKind> IntoElement for TextElement<M> {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

/// A debug function to print points as SVG path.
#[allow(unused)]
fn print_points_as_svg_path(
    line_corners: &Vec<gpui::Corners<Pixels>>,
    points: &Vec<Point<Pixels>>,
) {
    for corners in line_corners {
        println!(
            "tl: ({}, {}), tr: ({}, {}), bl: ({}, {}), br: ({}, {})",
            corners.top_left.as_f32() as i32,
            corners.top_left.as_f32() as i32,
            corners.top_right.as_f32() as i32,
            corners.top_right.as_f32() as i32,
            corners.bottom_left.as_f32() as i32,
            corners.bottom_left.as_f32() as i32,
            corners.bottom_right.as_f32() as i32,
            corners.bottom_right.as_f32() as i32,
        );
    }

    if points.len() > 0 {
        println!(
            "M{},{}",
            points[0].x.as_f32() as i32,
            points[0].y.as_f32() as i32
        );
        for p in points.iter().skip(1) {
            println!("L{},{}", p.x.as_f32() as i32, p.y.as_f32() as i32);
        }
    }
}

fn frame_outline_points(corners: &[Corners<Point<Pixels>>]) -> Vec<Point<Pixels>> {
    let rects = corners
        .iter()
        .map(|corners| (corners.top_left, corners.bottom_right))
        .collect::<Vec<_>>();
    let mut points = Vec::with_capacity(rects.len() * 4);
    let first = rects[0];
    let last = rects[rects.len() - 1];
    points.push(first.0);
    points.push(point(first.1.x, first.0.y));
    for pair in rects.windows(2) {
        let current = pair[0];
        let next = pair[1];
        points.push(current.1);
        points.push(point(next.1.x, current.1.y));
        points.push(point(next.1.x, next.1.y));
    }
    if points.last() != Some(&last.1) {
        points.push(last.1);
    }
    points.push(point(last.0.x, last.1.y));
    for pair in rects.windows(2).rev() {
        let current = pair[1];
        let next = pair[0];
        points.push(point(current.0.x, current.0.y));
        points.push(point(next.0.x, current.0.y));
        points.push(point(next.0.x, next.0.y));
    }
    if points.last() != points.first() {
        points.push(points[0]);
    }
    points
}

impl<M: InputModeKind> Element for TextElement<M> {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
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
            // At least `rows` tall (auto grow: the content's rows, capped at
            // `max_rows`); a taller parent still fills it.
            let rows = state.mode.max_rows().min(state.mode.rows());
            style.min_size.height = (rows * line_height).into();
        } else {
            // For single-line inputs, the minimum height should be the line height
            style.size.height = line_height.into();
        };

        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let style = window.text_style();
        let font = style.font();
        let text_size = style.font_size.to_pixels(window.rem_size());
        // Past the end of a line there are no glyphs to hit-test against, so a pointer
        // out there is measured in spaces instead.
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
            state.display_map.prepare_layout(
                &state.text,
                font,
                text_size,
                wrap_width,
                wrapping_indent,
                cx,
            );
        });

        let state = self.state.read(cx);
        let text = state.text.clone();
        let is_empty = text.len() == 0;
        let placeholder = self.placeholder.clone();

        let text_style = window.text_style();
        let disabled = state.disabled;
        let dim = |color: Hsla| if disabled { color.opacity(0.5) } else { color };
        let fg = dim(text_style.color);
        let (display_text, text_color) = if is_empty {
            (
                &Rope::from(placeholder.as_str()),
                dim(state.editor_style.muted_foreground),
            )
        } else if state.masked {
            (
                &Rope::from(MASK_CHAR.to_string().repeat(text.chars().count())),
                fg,
            )
        } else {
            (&text, fg)
        };

        let line_number_width = px(0.);

        let line_height = window.line_height();
        let token_elements = self.measure_tokens(
            (bounds.size.width - RIGHT_MARGIN).max(px(1.)),
            line_height,
            bounds.size.height,
            window,
            cx,
        );
        let state = self.state.read(cx);

        let (visible_range, visible_buffer_lines, visible_top, visible_display_rows) =
            self.calculate_visible_range(&state, line_height, bounds.size.height);
        let visible_start_offset = state.text.line_start_offset(visible_range.start);
        let visible_end_offset = state
            .text
            .line_end_offset(visible_range.end.saturating_sub(1));

        let visible_line_byte_offsets: Vec<usize> = visible_buffer_lines
            .iter()
            .map(|&bl| state.text.line_start_offset(bl))
            .collect();

        let (visible_line_byte_offsets, visible_range_offset) = if state.masked {
            let offsets = visible_line_byte_offsets
                .iter()
                .map(|&o| masked_display_offset(&text, o))
                .collect();
            let range_offset = masked_display_offset(&text, visible_start_offset)
                ..masked_display_offset(&text, visible_end_offset);
            (offsets, range_offset)
        } else {
            (
                visible_line_byte_offsets,
                visible_start_offset..visible_end_offset,
            )
        };

        let mut last_layout = LastLayout {
            visible_range,
            visible_buffer_lines,
            visible_line_byte_offsets,
            visible_top,
            visible_range_offset,
            line_height,
            wrap_width,
            line_number_width,
            space_width,
            lines: Rc::new(vec![]),
            cursor_bounds: None,
            text_align: state.text_align,
            content_width: bounds.size.width,
        };

        let run = TextRun {
            len: display_text.len(),
            font: style.font(),
            color: text_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let marked_underline = Some(UnderlineStyle {
            thickness: px(1.),
            color: Some(text_color),
            wavy: false,
        });

        let ime_marked_range = ime_marked_display_range(
            &text,
            state.ime_marked_range.as_ref().map(|m| m.start..m.end),
            state.masked,
        );

        let runs = split_run_for_ime_underline(
            run,
            0..display_text.len(),
            ime_marked_range,
            marked_underline,
        )
        .into_vec();

        let lines = Self::layout_lines(
            &state,
            &display_text,
            &last_layout,
            &visible_display_rows,
            text_size,
            &runs,
            window,
        );

        let mut longest_line_width = wrap_width.unwrap_or(px(0.));
        if state.is_single_line() || !state.soft_wrap {
            let longest_row = state.display_map.longest_row();
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
                                color: gpui::black(),
                                background_color: None,
                                underline: None,
                                strikethrough: None,
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
        if state.tokens_visible() {
            longest_line_width = if let Some(width) = wrap_width {
                width
            } else {
                state
                    .token_layout_cache
                    .as_ref()
                    .map_or(px(0.), |cache| cache.unwrapped_width)
            };
        }
        last_layout.lines = Rc::new(lines);

        let total_wrapped_lines = state.display_map.wrap_row_count();
        let mut scroll_size = size(
            if longest_line_width + RIGHT_MARGIN > bounds.size.width {
                longest_line_width + RIGHT_MARGIN
            } else {
                longest_line_width
            },
            (total_wrapped_lines as f32 * line_height).max(bounds.size.height),
        );

        if last_layout.text_align == TextAlign::Right || last_layout.text_align == TextAlign::Center
        {
            scroll_size.width = longest_line_width;
        }

        let input_bounds = bounds;

        let (cursor_infos, cursor_scroll_offset, _current_row) =
            self.layout_cursors(&last_layout, &mut bounds, scroll_size, window, cx);
        last_layout.cursor_bounds = cursor_infos
            .iter()
            .find(|info| info.is_active)
            .map(|info| info.bounds);

        let caret_geometry = last_layout.cursor_bounds.map(|cursor_bounds| {
            let mut scroll_offset = cursor_scroll_offset;
            let state = self.state.read(cx);
            let safe_y_range = (-scroll_size.height + input_bounds.size.height).min(px(0.))..px(0.);
            scroll_offset.y = if state.is_single_line() {
                px(0.)
            } else {
                scroll_offset.y.clamp(safe_y_range.start, safe_y_range.end)
            };
            scroll_offset.x = clamp_horizontal_scroll_offset(
                scroll_offset.x,
                scroll_size.width,
                input_bounds.size.width,
                last_layout.text_align,
            );
            PrepaintCaretGeometry::new(cursor_bounds, line_height, input_bounds, scroll_offset)
        });
        self.state.update(cx, |state, _| {
            state.prepaint_caret_geometry = caret_geometry;
        });

        let selection_paths = self.layout_selections(&last_layout, &mut bounds, window, cx);

        let state = self.state.read(cx);
        state
            .editor_scrollbar_snapshot
            .set(Some(EditorScrollbarSnapshot::new(
                input_bounds,
                &last_layout,
                scroll_size,
                cursor_scroll_offset,
                state,
            )));
        let hitbox = window.insert_hitbox(input_bounds, HitboxBehavior::Normal);

        let token_elements = self.prepaint_tokens(&last_layout, bounds, token_elements, window, cx);
        PrepaintState {
            token_elements,
            hitbox,
            bounds,
            last_layout,
            scroll_size,
            cursor_infos,
            cursor_scroll_offset,
            selection_paths,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        input_bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let (focus_handle, show_cursor, selected_range, editor_style) = {
            let state = self.state.read(cx);
            (
                state.focus_handle.clone(),
                state.show_cursor(window, cx),
                *state.active_selection(),
                state.editor_style.clone(),
            )
        };
        let focused = focus_handle.is_focused(window);
        let bounds = prepaint.bounds;
        let text_align = prepaint.last_layout.text_align;

        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.state.clone()),
            cx,
        );

        let line_height = window.line_height();
        let origin = bounds.origin;
        let invisible_top_padding = prepaint.last_layout.visible_top;

        let scroll_offset = if text_align == TextAlign::Right {
            (prepaint.scroll_size.width - prepaint.bounds.size.width).max(px(0.))
        } else if text_align == TextAlign::Center {
            (prepaint.scroll_size.width - prepaint.bounds.size.width)
                .half()
                .max(px(0.))
        } else {
            px(0.)
        };

        let mut offset_y = invisible_top_padding;
        for line in prepaint.last_layout.lines.iter() {
            let p = point(origin.x + scroll_offset, origin.y + offset_y);
            line.paint_background(
                p,
                line_height,
                text_align,
                Some(prepaint.last_layout.content_width),
                window,
                cx,
            );
            offset_y += line.size(line_height).height;
        }

        if window.is_window_active() {
            for path in prepaint.selection_paths.drain(..) {
                window.paint_path(path, editor_style.selection);
            }
        }

        let mut offset_y = invisible_top_padding;
        for line in prepaint.last_layout.lines.iter() {
            let line_y = origin.y + offset_y;
            let p = point(origin.x + scroll_offset, line_y);
            _ = line.paint(
                p,
                line_height,
                text_align,
                Some(prepaint.last_layout.content_width),
                window,
                cx,
            );
            offset_y += line.size(line_height).height;
        }

        for element in &mut prepaint.token_elements {
            element.paint(window, cx);
        }

        if focused && show_cursor {
            if let Some(caret_color) = editor_style.caret {
                for cursor_info in prepaint.cursor_infos_with_scroll() {
                    window.paint_quad(fill(cursor_info.bounds, caret_color));
                }
            }
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

        self.paint_mouse_listeners(&prepaint.hitbox, window, cx);
    }
}

/// Split placeholder text into display lines and trim runs to each line.
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
        // Advance in the whole-placeholder coordinate space, including the separator.
        line_offset += line.len() + 1;
    }

    result
}

/// Get the runs for the given range.
///
/// The range is the byte range of the wrapped line.
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

/// Snap run boundaries to char boundaries of `text` and cap them at its
/// length. Style ranges are byte offsets that can drift off char boundaries
/// (a stale syntax tree, a range in the wrong coordinate space) and the
/// platform text system panics when a run splits a multi-byte character.
///
/// Returns `None` when the runs are already aligned (the common case).
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

/// Whether any of these runs paints a glyph background, used to skip the background
/// paint pass for lines without highlights.
fn has_background(runs: &[TextRun]) -> bool {
    runs.iter().any(|run| run.background_color.is_some())
}
