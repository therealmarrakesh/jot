//! A text editor: a tab's document, or a one-line field (find, replace, go to line).
//!
//! The window forwards keys, text, IME and pointer input; `prepare` lays the editor out where
//! the frame placed it, works out the caret, selection and scrolling, and `paint` draws them. A
//! document also has a line index (wrapping and windowed shaping), scrollbars, and auto-scroll
//! while drag-selecting past its edge.

use std::ops::Range;
use std::time::{Duration, Instant};

use crate::geom::{Axis, Corners, Edges, Point, Rect, Rgba, Size, point, size};
use crate::gfx::{Canvas, PaintQuad, TextStyle, TextSystem, fill};
use crate::layout::round_px;
use crate::lines::{FrameLayout, LineIndex, Viewport};
use crate::platform::{
    CursorStyle, Modifiers, MouseButton, SystemSettings, read_clipboard_text, write_clipboard_text,
};
use crate::text::{
    EditPolicy, LineChange, Rope, RopeExt as _, TextModel, ValidateFn, next_grapheme_boundary,
    next_word_end, prev_grapheme_boundary, prev_word_start,
};
use crate::theme::Palette;
use crate::types::{SharedStr, take_due};
use crate::ui::{
    AnimKey, AppNode, AppUi, Curve, CustomDraw, Easing, EditorSlot, HitId, TabId, box_, custom, row,
};

// ---- 1. Options and commands ----------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
pub(crate) struct DocOptions {
    pub(crate) tab: TabId,
    pub(crate) soft_wrap: bool,
    pub(crate) paddings: Edges<f32>,
}

#[derive(Clone, Debug)]
pub(crate) struct FieldOptions {
    pub(crate) slot: EditorSlot,
    pub(crate) placeholder: SharedStr,
    pub(crate) validate: Option<ValidateFn>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Motion {
    Left,
    Right,
    Up,
    Down,
    LineStart,
    LineEnd,
    PageUp,
    PageDown,
    DocStart,
    DocEnd,
    PrevWord,
    NextWord,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum EditCommand {
    Enter,
    Backspace,
    Delete,
    DeletePrevWord,
    DeleteNextWord,
    Indent,
    Outdent,
    IndentInline,
    Move { motion: Motion, extend: bool },
    SelectAll,
    Copy,
    Cut,
    Paste,
    Undo,
    Redo,
    Escape,
}

/// How the editor is drawn: its font, line height, text colour and caret height.
#[derive(Clone, Debug)]
pub(crate) struct EditorPaintStyle {
    pub(crate) text: TextStyle,
    pub(crate) line_height: f32,
    pub(crate) color: Rgba,
    pub(crate) caret_height: f32,
}

/// Everything a layout of the editor is drawn with.
#[derive(Copy, Clone)]
pub(crate) struct EditorPaintParams<'a> {
    pub(crate) style: &'a EditorPaintStyle,
    pub(crate) palette: Palette,
    pub(crate) window_active: bool,
    pub(crate) show_selection: bool,
    pub(crate) scale: f32,
    pub(crate) caret_width_px: u32,
    /// The frame's time, for the scrollbars' fades.
    pub(crate) now: Instant,
}

// ---- 2. The editor --------------------------------------------------------------------------

pub(crate) struct Editor {
    /// Which editor of the window this is: its hits, drawing and scrollbars are keyed by it.
    pub(crate) slot: EditorSlot,
    /// Whether it has the keyboard: its caret shows and blinks, and the pointer can drag-select.
    focused: bool,
    model: TextModel,
    /// What only a document has; a field has none.
    doc: Option<Box<DocumentView>>,
    blink: BlinkState,
    /// The x the caret keeps moving up and down at, and the column it came from.
    goal: Option<(f32, usize)>,
    /// The word a double-click selected, which a drag extends by.
    word_anchor: Option<Range<usize>>,
    /// The left button is down on the text (a drag selects).
    selecting: bool,
    scroll: Scroll,
    /// How to bring the caret into view at the next layout.
    reveal: Option<RevealRequest>,
    /// Where the text area was laid out.
    input_bounds: Rect,
    frame: Option<Frame>,
    /// What an empty field shows.
    placeholder: SharedStr,
}

/// A document's view of its text: how it's laid out and scrolled.
struct DocumentView {
    lines: LineIndex,
    soft_wrap: bool,
    /// The line height of the last layout.
    last_line_height: f32,
    paddings: Edges<f32>,
    scrollbar: ScrollbarState,
    auto_scroll: AutoScroll,
    /// A right press, waiting for its release to open the context menu: where, and at which
    /// offset.
    pending_context_menu: Option<(Point, usize)>,
}

#[derive(Copy, Clone, Debug, Default)]
struct Scroll {
    x: f32,
    y: f64,
    content_w: f32,
    content_h: f64,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum MoveDirection {
    Up,
    Down,
}

/// A request to scroll the caret into view at the next layout: only up or down (`direction`)
/// if it moved one way, and keeping a few rows' margin from the edges (`with_margin`).
#[derive(Clone, Copy, Debug)]
struct RevealRequest {
    direction: Option<MoveDirection>,
    with_margin: bool,
}

/// The editor's last layout, with where it was placed and what was drawn over it.
struct Frame {
    layout: FrameLayout,
    revision: u64,
    scroll_x: f32,
    scroll_y: f64,
    /// Where the text was drawn from: the editor's top left, scrolled horizontally.
    origin: Point,
    text_color: Rgba,
    caret_rect: Rect,
    show_caret: bool,
    selection_rects: Vec<Rect>,
    marked_range: Option<Range<usize>>,
    last_cursor: usize,
}

impl Editor {
    pub(crate) fn document(rope: Rope, opts: DocOptions) -> Self {
        let model = TextModel::new(EditPolicy {
            multi_line: true,
            validate: None,
        });
        let doc = DocumentView {
            lines: LineIndex::new(TextStyle::default(), None),
            soft_wrap: opts.soft_wrap,
            last_line_height: 20.0,
            paddings: opts.paddings,
            scrollbar: ScrollbarState::default(),
            auto_scroll: AutoScroll::default(),
            pending_context_menu: None,
        };
        Self {
            doc: Some(Box::new(doc)),
            ..Self::blank(EditorSlot::Tab(opts.tab), model.with_rope(rope))
        }
    }

    pub(crate) fn field(opts: FieldOptions) -> Self {
        let model = TextModel::new(EditPolicy {
            multi_line: false,
            validate: opts.validate,
        });
        Self {
            placeholder: opts.placeholder,
            ..Self::blank(opts.slot, model)
        }
    }

    fn blank(slot: EditorSlot, model: TextModel) -> Self {
        Self {
            slot,
            focused: false,
            model,
            doc: None,
            blink: BlinkState::default(),
            goal: None,
            word_anchor: None,
            selecting: false,
            scroll: Scroll::default(),
            reveal: None,
            input_bounds: Rect::ZERO,
            frame: None,
            placeholder: SharedStr::default(),
        }
    }

    pub(crate) fn model(&self) -> &TextModel {
        &self.model
    }

    /// The last layout, if the text hasn't changed since.
    fn fresh_frame(&self) -> Option<&Frame> {
        let frame = self.frame.as_ref()?;
        (frame.revision == self.model.revision()).then_some(frame)
    }

    pub(crate) fn caret_bounds(&self) -> Option<Rect> {
        self.fresh_frame().map(|f| f.caret_rect)
    }

    /// Where the IME's window goes: by the composition, or the caret.
    pub(crate) fn ime_caret_bounds(&self) -> Option<Rect> {
        let frame = self.fresh_frame()?;
        let selection = self.model.selection().range();
        let text = self.model.text();
        let bounds =
            frame
                .layout
                .bounds_for_range(text, selection, frame.last_cursor, frame.origin);
        bounds.or(Some(frame.caret_rect))
    }

    #[cfg(test)]
    pub(crate) fn soft_wrap_enabled(&self) -> bool {
        self.doc.as_deref().is_some_and(|d| d.soft_wrap)
    }

    pub(crate) fn scroll_offset(&self) -> Point {
        point(self.scroll.x, self.scroll.y as f32)
    }

    /// Turns soft wrap on or off; the next layout rewraps, keeping the top line in view.
    pub(crate) fn set_soft_wrap(&mut self, wrap: bool) {
        if let Some(doc) = &mut self.doc
            && doc.soft_wrap != wrap
        {
            doc.soft_wrap = wrap;
            if wrap {
                self.scroll.x = 0.0;
            }
        }
    }

    pub(crate) fn begin_save(&mut self) -> (u64, Rope) {
        self.model.begin_save()
    }

    pub(crate) fn mark_saved_version(&mut self, version: u64) {
        self.model.mark_saved_version(version);
    }

    /// The keyboard comes to the editor: its caret shows and starts blinking.
    pub(crate) fn focus(&mut self, settings: &SystemSettings) {
        self.focused = true;
        self.blink
            .start((settings.caret_blink_interval, settings.caret_blink_timeout));
    }

    /// The keyboard leaves: any composition is committed, the next edit starts a new undo step,
    /// the caret stops and a pointer selection in progress ends.
    pub(crate) fn blur(&mut self) {
        self.focused = false;
        self.model.break_coalescing();
        self.blink = BlinkState::default();
        self.end_pointer_selection();
    }

    /// Holds the caret steady for a moment while keys come in.
    pub(crate) fn pause_blink(&mut self) {
        self.blink.pause();
    }

    /// The editor's node: a field's single row, or a document's padded text with its scrollbar
    /// tracks over the padding.
    pub(crate) fn view(&self, line_height: f32) -> AppNode {
        let (hit, cursor) = (HitId::Editor(self.slot), CursorStyle::IBeam);
        let text = custom(CustomDraw::Editor(self.slot));
        let Some(doc) = &self.doc else {
            return row()
                .hit(hit)
                .cursor(cursor)
                .flex_1()
                .overflow_x_hidden()
                .min_h_full()
                .items_center()
                .child(text.w_full().h(line_height));
        };
        let text = text.w_full().h_full().flex_1().min_h(2.0 * line_height);
        box_()
            .hit(hit)
            .cursor(cursor)
            .flex_1()
            .overflow_x_hidden()
            .h_full()
            .child(box_().size_full().padding(doc.paddings).child(text))
            .children(doc.scrollbar.tracks(self.slot, doc.paddings.left))
    }

    fn request_reveal(&mut self, direction: Option<MoveDirection>, with_margin: bool) {
        self.reveal = Some(RevealRequest {
            direction,
            with_margin,
        });
    }

    /// A drag-selection (and any auto-scroll it started) ends.
    fn end_pointer_selection(&mut self) {
        self.word_anchor = None;
        self.selecting = false;
        if let Some(doc) = &mut self.doc {
            doc.auto_scroll.stop();
        }
    }
}

// ---- 3. Editing -----------------------------------------------------------------------------

impl Editor {
    /// Takes in an edit the model made: rewraps the lines it changed and reveals the caret.
    fn apply_change(&mut self, change: Option<LineChange>, ts: &TextSystem) {
        let Some(lines) = change else {
            return;
        };
        self.goal = None;
        self.end_pointer_selection();
        if let Some(doc) = &mut self.doc {
            doc.lines.apply_line_change(self.model.text(), &lines, ts);
        }
        self.request_reveal(None, true);
    }

    /// An edit typed or commanded: the caret holds steady while it's taken in.
    fn commit_edit(&mut self, change: Option<LineChange>, ts: &TextSystem) {
        self.pause_blink();
        self.apply_change(change, ts);
    }

    fn selected_string(&self) -> Option<String> {
        let selection = self.model.selection();
        (!selection.is_empty()).then(|| self.model.text().slice(selection.range()).to_string())
    }

    /// Runs `command`; whether the editor handled it.
    pub(crate) fn handle_command(&mut self, command: EditCommand, ts: &TextSystem) -> bool {
        let m = &mut self.model;
        let change = match command {
            // With nothing to do these go unhandled, so the key can mean something else.
            EditCommand::Backspace => match m.delete_backward() {
                None => return false,
                change => change,
            },
            EditCommand::Enter => match m.enter() {
                None => return false,
                change => change,
            },
            EditCommand::Escape => {
                let Some(change) = m.cancel_composition() else {
                    return false;
                };
                self.apply_change(Some(change), ts);
                return true;
            }
            EditCommand::IndentInline | EditCommand::Indent | EditCommand::Outdent
                if !m.is_multi_line() =>
            {
                return false;
            }
            EditCommand::Move { motion, extend } => return self.move_caret(motion, extend),
            EditCommand::SelectAll => {
                self.select_all();
                return true;
            }
            EditCommand::Copy => {
                if let Some(text) = self.selected_string() {
                    write_clipboard_text(&text);
                }
                return true;
            }
            EditCommand::Delete => m.delete_forward(),
            EditCommand::DeletePrevWord => m.delete_prev_word(),
            EditCommand::DeleteNextWord => m.delete_next_word(),
            EditCommand::IndentInline | EditCommand::Indent => {
                m.indent(command == EditCommand::Indent)
            }
            EditCommand::Outdent => m.outdent(),
            EditCommand::Undo => m.undo(),
            EditCommand::Redo => m.redo(),
            EditCommand::Cut => {
                m.commit_composition();
                match self.selected_string() {
                    Some(text) if write_clipboard_text(&text) => self.model.replace_selection(""),
                    _ => None,
                }
            }
            EditCommand::Paste => match read_clipboard_text().filter(|s| !s.is_empty()) {
                Some(text) => m.replace_selection(&text),
                None => None,
            },
        };
        self.commit_edit(change, ts);
        true
    }

    pub(crate) fn handle_text_input(&mut self, text: &str, ts: &TextSystem) {
        let change = self.model.insert_text(text);
        self.commit_edit(change, ts);
    }

    pub(crate) fn handle_ime_composition(
        &mut self,
        text: &str,
        cursor_utf16: usize,
        ts: &TextSystem,
    ) {
        if text.is_empty() && self.model.composition().is_none() {
            return;
        }
        let change = self.model.set_composition(text, cursor_utf16);
        self.apply_change(change, ts);
    }

    /// Replaces the whole text, scrolled back to its start.
    pub(crate) fn set_value(&mut self, value: &str, ts: &TextSystem) {
        self.end_pointer_selection();
        let change = self.model.set_value(value);
        self.apply_change(change, ts);
        (self.scroll.x, self.scroll.y) = (0.0, 0.0);
        self.reveal = None;
    }

    /// Replaces the selection with `text`.
    pub(crate) fn replace(&mut self, text: &str, ts: &TextSystem) {
        let change = self.model.replace_selection(text);
        self.apply_change(change, ts);
    }

    pub(crate) fn replace_all(
        &mut self,
        ranges: &[Range<usize>],
        replacement: &str,
        ts: &TextSystem,
    ) {
        let change = self.model.replace_all(ranges, replacement);
        self.apply_change(change, ts);
    }

    pub(crate) fn select_all(&mut self) {
        self.goal = None;
        self.model.select_all();
    }

    pub(crate) fn set_selected_range(&mut self, range: Range<usize>) {
        self.goal = None;
        self.word_anchor = None;
        self.model.set_selected_range(range);
        self.request_reveal(None, true);
    }

    pub(crate) fn go_to_line(&mut self, line: usize) {
        self.goal = None;
        self.model.go_to_line(line);
        self.request_reveal(None, true);
    }
}

// ---- 4. Caret motion ------------------------------------------------------------------------

impl Editor {
    fn move_caret(&mut self, motion: Motion, extend: bool) -> bool {
        let vertical = matches!(
            motion,
            Motion::Up | Motion::Down | Motion::PageUp | Motion::PageDown
        );
        if vertical && self.doc.is_none() {
            return false;
        }
        self.model.commit_composition();
        let sel = self.model.selection();
        let (text, len) = (self.model.text(), self.model.text().len());
        // Against the start or end with nothing selected, Left and Right do nothing.
        if !extend && sel.is_empty() {
            let cursor = self.model.cursor();
            if (motion == Motion::Left && cursor == 0) || (motion == Motion::Right && cursor == len)
            {
                return false;
            }
        }
        let collapsing = !extend && !sel.is_empty();
        let (target, target_aff, next_goal, direction) = match motion {
            Motion::Left if collapsing => (sel.start(), false, None, None),
            Motion::Right if collapsing => (sel.end(), false, None, None),
            Motion::Left => (prev_grapheme_boundary(text, sel.head), false, None, None),
            Motion::Right => (next_grapheme_boundary(text, sel.head), false, None, None),
            Motion::PrevWord => (prev_word_start(text, sel.head), false, None, None),
            Motion::NextWord => (next_word_end(text, sel.head), false, None, None),
            Motion::LineStart => {
                let (row, _) = self.visual_row_for(sel.head, sel.end_affinity);
                (row.start, false, None, Some(MoveDirection::Up))
            }
            Motion::LineEnd => {
                let (row, soft_end) = self.visual_row_for(sel.head, sel.end_affinity);
                (row.end, soft_end, None, Some(MoveDirection::Down))
            }
            Motion::DocStart => (0, false, None, None),
            Motion::DocEnd => (len, false, None, None),
            Motion::Up | Motion::Down | Motion::PageUp | Motion::PageDown => {
                let Some(doc) = &self.doc else {
                    return false;
                };
                let page = || {
                    let lh = doc.last_line_height.max(1.0);
                    ((self.input_bounds.size.height / lh).floor() as isize).max(1)
                };
                let delta = match motion {
                    Motion::Up => -1,
                    Motion::Down => 1,
                    Motion::PageUp => -page(),
                    _ => page(),
                };
                let dir = if delta < 0 {
                    MoveDirection::Up
                } else {
                    MoveDirection::Down
                };
                // Up or Down collapses a selection from its edge in that direction; otherwise the
                // caret moves, keeping the column it was moving in.
                let collapse = collapsing && matches!(motion, Motion::Up | Motion::Down);
                let (base, aff) = if !collapse {
                    (sel.head, sel.end_affinity)
                } else if delta < 0 {
                    (sel.start(), false)
                } else {
                    let at_head = sel.end() == self.model.cursor();
                    (sel.end(), at_head && sel.end_affinity)
                };
                let goal = self
                    .goal
                    .filter(|_| !collapse)
                    .or_else(|| self.preferred_x_for(base, aff));
                let mut target = self.vertical_target(doc, base, goal, aff, delta);
                // Extending from the first or last row selects to the document's start or end.
                if extend && doc.lines.visual_row_count() > 0 {
                    let row = |(offset, aff)| doc.lines.offset_to_display(text, offset, aff).row;
                    if row((base, aff)) == row(target) {
                        target = (if delta < 0 { 0 } else { len }, false);
                    }
                }
                (target.0, target.1, goal, Some(dir))
            }
        };
        self.goal = next_goal;
        self.pause_blink();
        if extend {
            self.model.break_coalescing();
            self.model.extend_selection(target, target_aff, None);
        } else {
            self.model.place_caret(target, target_aff);
        }
        self.request_reveal(direction.filter(|_| !extend), true);
        true
    }

    /// The visual row `offset` is on, as text offsets, and whether it ends at a soft wrap; a
    /// field's row is all of it.
    fn visual_row_for(&self, offset: usize, end_affinity: bool) -> (Range<usize>, bool) {
        let text = self.model.text();
        let Some(doc) = &self.doc else {
            return (0..text.len(), false);
        };
        let row = text.offset_to_point(offset).row;
        let line_start = text.line_start_offset(row);
        if doc.soft_wrap {
            let disp = doc.lines.offset_to_display(text, offset, end_affinity);
            if let Some((range, soft_end)) = doc.lines.visual_row_range(row, disp.sub_row) {
                return (line_start + range.start..line_start + range.end, soft_end);
            }
        }
        (line_start..text.line_end_offset(row), false)
    }

    /// Where `offset` is drawn across its row, and its column, from the last layout.
    fn preferred_x_for(&self, offset: usize, end_affinity: bool) -> Option<(f32, usize)> {
        let frame = self.fresh_frame()?;
        let pt = self.model.text().offset_to_point(offset);
        let line = frame.layout.line(pt.row)?;
        let pos = line.position_for_index(pt.column, frame.layout.line_height, end_affinity)?;
        Some((pos.x, pt.column))
    }

    /// Where `delta_rows` visual rows from `offset` the caret lands: at `goal`'s x where the row
    /// was laid out, else at its column.
    fn vertical_target(
        &self,
        doc: &DocumentView,
        offset: usize,
        goal: Option<(f32, usize)>,
        end_affinity: bool,
        delta_rows: isize,
    ) -> (usize, bool) {
        if doc.lines.visual_row_count() == 0 {
            return (offset, end_affinity);
        }
        let text = self.model.text();
        let mut display = doc.lines.offset_to_display(text, offset, end_affinity);
        let last_row = doc.lines.visual_row_count().saturating_sub(1);
        display.row = display.row.saturating_add_signed(delta_rows).min(last_row);
        display.col = 0;
        let row_start = doc.lines.display_to_offset(text, display);
        let Some((x, column)) = goal else {
            return (row_start, false);
        };
        let mut row = doc.lines.offset_to_display(text, row_start, false);
        row.col = 0;
        let line = text
            .offset_to_point(doc.lines.display_to_offset(text, row))
            .row;
        let line_start = text.line_start_offset(line);
        if let Some(frame) = self.fresh_frame()
            && let Some(shown) = frame.layout.line(line)
        {
            let lh = frame.layout.line_height;
            let at = point(x, row.sub_row as f32 * lh);
            return match shown.closest_index_for_position(at, lh) {
                Some((ix, affinity)) => (line_start + ix, affinity),
                None => (row_start, false),
            };
        }
        if let Some((range, soft_end)) = doc.lines.visual_row_range(line, row.sub_row) {
            let col = column.min(range.len());
            return (
                line_start + range.start + col,
                soft_end && col == range.len(),
            );
        }
        (line_start + column.min(text.slice_line(line).len()), false)
    }

    fn move_to_with_affinity(&mut self, offset: usize, end_affinity: bool) {
        self.goal = None;
        self.model.place_caret(offset, end_affinity);
        self.request_reveal(None, false);
        self.pause_blink();
    }

    fn select_to_with_affinity(&mut self, offset: usize, end_affinity: bool) {
        self.goal = None;
        self.model
            .extend_selection(offset, end_affinity, self.word_anchor.clone());
    }
}

// ---- 5. The pointer -------------------------------------------------------------------------

impl Editor {
    /// The offset at `pos` (and whether it's at the end of a wrapped row), in the last layout as
    /// it's scrolled now.
    fn hit_test_point(&self, pos: Point) -> (usize, bool) {
        let Some(frame) = self.fresh_frame() else {
            return (0, false);
        };
        let scrolled = point(
            self.scroll.x - frame.scroll_x,
            (self.scroll.y - frame.scroll_y) as f32,
        );
        let rel = pos - frame.origin - scrolled;
        let len = self.model.text().len();
        frame.layout.hit_test(rel, len, self.model.is_multi_line())
    }

    pub(crate) fn on_mouse_down(
        &mut self,
        button: MouseButton,
        position: Point,
        modifiers: Modifiers,
        click_count: usize,
    ) {
        self.model.break_coalescing();
        self.selecting = true;
        let (offset, aff) = self.hit_test_point(position);
        match button {
            MouseButton::Left if click_count >= 3 => {
                self.goal = None;
                self.model.select_line_at(offset);
                self.word_anchor = None;
            }
            MouseButton::Left if click_count == 2 => {
                self.goal = None;
                if let Some(word) = self.model.select_word_at(offset) {
                    self.word_anchor = Some(word);
                }
            }
            // A right press inside the selection keeps it for the context menu, which opens on
            // the release.
            MouseButton::Right => {
                if !self.model.selection().contains(offset) {
                    self.move_to_with_affinity(offset, aff);
                }
                if let Some(doc) = &mut self.doc {
                    doc.pending_context_menu = Some((position, offset));
                }
            }
            _ if modifiers.shift => {
                self.select_to_with_affinity(offset, aff);
                self.request_reveal(None, false);
            }
            _ => self.move_to_with_affinity(offset, aff),
        }
    }

    /// The pointer moved; dragging with the left button extends the selection, and past the
    /// edge auto-scrolls. Whether the editor needs repainting.
    pub(crate) fn on_mouse_move(&mut self, position: Point, dragging_left: bool) -> bool {
        let dragging = dragging_left && self.selecting && self.model.text().len() > 0;
        if !dragging || self.fresh_frame().is_none() {
            return false;
        }
        let (offset, aff) = self.hit_test_point(position);
        self.select_to_with_affinity(offset, aff);
        if let Some(doc) = &mut self.doc {
            doc.auto_scroll.last_drag_pos = Some(position);
            let delta = AutoScroll::compute_delta(position.y, self.input_bounds);
            doc.auto_scroll.update(delta.map(|d| -d));
        }
        self.request_reveal(None, false);
        true
    }

    /// A button came up: where to open the context menu, if a right click asked for one.
    pub(crate) fn on_mouse_up(&mut self, button: MouseButton) -> Option<Point> {
        let mut context_menu = None;
        if button == MouseButton::Right
            && let Some(doc) = &mut self.doc
            && let Some((pos, offset)) = doc.pending_context_menu.take()
        {
            if !self.model.selection().contains(offset) {
                self.move_to_with_affinity(offset, false);
            }
            context_menu = Some(pos);
        }
        self.end_pointer_selection();
        context_menu
    }
}

// ---- 6. Scrolling ---------------------------------------------------------------------------

/// The space kept right of the caret, and right of the longest line.
const RIGHT_MARGIN: f32 = 10.0;
/// Rows kept between a revealed caret and the edge, in a view tall enough.
const BOTTOM_MARGIN_ROWS: usize = 3;

/// Where the top of the view is in the text, to keep it there through a reflow: the line, how
/// far into it the top row starts, and how far into that row the view starts.
#[derive(Copy, Clone, Debug)]
struct ScrollAnchor {
    line: usize,
    byte_in_line: usize,
    frac_px: f64,
}

/// How wide the content is: the longest line, and the margin past it if it scrolls.
fn content_width_with_margin(max_line_w: f32, viewport_width: f32) -> f32 {
    if max_line_w + RIGHT_MARGIN > viewport_width {
        max_line_w + RIGHT_MARGIN
    } else {
        max_line_w
    }
}

fn clamp_horizontal_scroll(offset_x: f32, content_width: f32, viewport_width: f32) -> f32 {
    let min_x = (viewport_width - content_width).min(0.0);
    offset_x.clamp(min_x, 0.0)
}

/// The scroll that brings `caret_x` into view, moving as little as it can.
fn reveal_horizontal_scroll(scroll_x: f32, caret_x: f32, viewport_width: f32) -> f32 {
    if scroll_x + caret_x > viewport_width - RIGHT_MARGIN {
        viewport_width - RIGHT_MARGIN - caret_x
    } else if scroll_x + caret_x < 0.0 {
        -caret_x
    } else {
        scroll_x
    }
}

/// The margin kept above and below a revealed caret: a row in a short view, a few in a tall one,
/// never more than half of it.
fn caret_vertical_margin(viewport_height: f32, line_height: f32) -> f32 {
    let rows = (viewport_height / line_height.max(1.0)) as usize;
    let margin = if rows < BOTTOM_MARGIN_ROWS * 8 {
        line_height
    } else {
        BOTTOM_MARGIN_ROWS as f32 * line_height
    };
    margin.min(rows as f32 * line_height * 0.5)
}

impl DocumentView {
    fn capture_scroll_anchor(&self, scroll_y: f64, line_height: f32) -> Option<ScrollAnchor> {
        let rows = self.lines.visual_row_count();
        if rows == 0 || line_height <= 0.0 {
            return None;
        }
        let lh = f64::from(line_height);
        let top = (-scroll_y).max(0.0);
        let top_row = (top / lh).floor() as usize;
        let frac_px = top - top_row as f64 * lh;
        let (line, sub_row) = self
            .lines
            .visual_row_to_line_and_sub_row(top_row.min(rows.saturating_sub(1)));
        let byte_in_line = self
            .lines
            .visual_row_range(line, sub_row)
            .map_or(0, |(r, _)| r.start);
        Some(ScrollAnchor {
            line,
            byte_in_line,
            frac_px,
        })
    }

    fn restore_scroll_anchor(&self, text: &Rope, anchor: ScrollAnchor, line_height: f32) -> f64 {
        let lines = self.lines.line_count();
        if lines == 0 || line_height <= 0.0 {
            return 0.0;
        }
        let line = anchor.line.min(lines.saturating_sub(1));
        let target = text.line_start_offset(line) + anchor.byte_in_line.min(text.line_len(line));
        let row = self.lines.offset_to_display(text, target, false).row;
        -(row as f64 * f64::from(line_height) + anchor.frac_px).max(0.0)
    }
}

impl Editor {
    /// Scrolls a document to (`x`, `y`), kept within its content; whether it moved.
    fn scroll_to(&mut self, x: f32, y: f64) -> bool {
        let view = self.input_bounds.size;
        let min_y = (f64::from(view.height) - self.scroll.content_h).min(0.0);
        let next = (
            clamp_horizontal_scroll(x, self.scroll.content_w, view.width),
            y.clamp(min_y, 0.0),
        );
        let moved = next != (self.scroll.x, self.scroll.y);
        (self.scroll.x, self.scroll.y) = next;
        moved
    }

    pub(crate) fn on_scroll_wheel(
        &mut self,
        delta_lines: Point,
        page_scroll: bool,
        modifiers: Modifiers,
    ) -> bool {
        let Some(doc) = self.doc.as_deref().filter(|_| !modifiers.ctrl) else {
            return false;
        };
        let lh = doc.last_line_height.max(1.0);
        let view = self.input_bounds.size;
        // Sideways by three spaces a notch, never more than a view.
        let dx = if delta_lines.x == 0.0 {
            0.0
        } else {
            let space = self
                .frame
                .as_ref()
                .map_or(lh * 0.5, |f| f.layout.space_width.max(1.0));
            let most = view.width.max(lh);
            (delta_lines.x * space * 3.0).round().clamp(-most, most)
        };
        let row = if page_scroll {
            (view.height - lh).max(lh)
        } else {
            lh
        };
        let dy = f64::from(delta_lines.y) * f64::from(row);
        self.reveal = None;
        self.scroll_to(self.scroll.x + dx, self.scroll.y + dy)
    }

    fn apply_scrollbar_scroll(&mut self, next: Option<Point>) {
        if let Some(next) = next {
            self.reveal = None;
            self.scroll_to(next.x, f64::from(next.y));
        }
    }

    /// A press on a scrollbar track: on the thumb it starts a drag, elsewhere it pages. Whether
    /// the scrollbar took it, which it doesn't while faded out.
    pub(crate) fn press_scrollbar(&mut self, axis: Axis, pos: Point) -> bool {
        let scroll = self.scroll_offset();
        let Some(doc) = self.doc.as_deref_mut() else {
            return false;
        };
        if !doc.scrollbar.is_interactive(axis) {
            return false;
        }
        let next = doc.scrollbar.on_mouse_down(axis, pos, scroll);
        self.apply_scrollbar_scroll(next);
        true
    }

    /// The pointer moved: whether a scrollbar thumb needs redrawing because it came onto or off
    /// one.
    pub(crate) fn hover_scrollbar(&self, pos: Point, ui: &AppUi) -> bool {
        self.doc
            .as_deref()
            .is_some_and(|doc| doc.scrollbar.hover_changed(self.slot, pos, ui))
    }

    /// While the thumb is dragged, follows the pointer: whether to repaint now (`None` when no
    /// drag is going on).
    pub(crate) fn drag_scrollbar(&mut self, pos: Point, pressed: bool) -> Option<bool> {
        let scroll = self.scroll_offset();
        let (next, notify) = self
            .doc
            .as_deref_mut()?
            .scrollbar
            .on_mouse_move(pos, pressed, scroll)?;
        self.apply_scrollbar_scroll(next);
        Some(notify)
    }

    /// Ends a thumb drag at `pos`.
    pub(crate) fn release_scrollbar(&mut self, pos: Point) {
        let scroll = self.scroll_offset();
        let next = self
            .doc
            .as_deref_mut()
            .and_then(|d| d.scrollbar.on_mouse_up(pos, scroll));
        self.apply_scrollbar_scroll(next);
    }
}

// ---- 7. Layout and painting -----------------------------------------------------------------

impl Editor {
    /// Lays the editor out in `input_bounds`, where its text landed, and works out what to draw;
    /// whether another frame should follow (a document's windowed lines or scrollbars changed
    /// in ways this frame's layout couldn't show).
    pub(crate) fn prepare(
        &mut self,
        input_bounds: Rect,
        params: EditorPaintParams<'_>,
        ui: &mut AppUi,
        ts: &TextSystem,
    ) -> bool {
        self.input_bounds = input_bounds;
        if self.doc.is_none() {
            self.prepare_field(params, ts);
            return false;
        }
        let reshaped = self.prepare_document(params, ts);
        let scroll = self.scroll_offset();
        let content = size(self.scroll.content_w, self.scroll.content_h as f32);
        let doc = self.doc.as_deref_mut().expect("a document");
        let vp = ScrollbarViewport {
            input: input_bounds,
            paddings: doc.paddings,
            scale: params.scale,
            content_size: content,
            scroll_offset: scroll,
            horizontal: !doc.soft_wrap,
        };
        let bars_changed = doc
            .scrollbar
            .update(self.slot, vp, params.palette, params.now, ui);
        reshaped || bars_changed
    }

    /// A field's one row: its text, or its placeholder while empty.
    fn prepare_field(&mut self, params: EditorPaintParams<'_>, ts: &TextSystem) {
        let style = params.style;
        let text = self.model.text();
        let empty = text.len() == 0;
        let (shown, color): (SharedStr, Rgba) = if empty {
            (self.placeholder.clone(), params.palette.text_secondary)
        } else {
            (text.to_string().into(), style.color)
        };
        let lh = style.line_height.max(1.0);
        let layout = FrameLayout::single_row(shown, text.len(), &style.text, lh, ts);
        let head_pt = layout
            .position_for_offset(text, self.model.cursor(), false)
            .unwrap_or(Point::ZERO);
        let width = self.input_bounds.size.width;
        self.scroll.content_w = content_width_with_margin(layout.width().max(head_pt.x), width);
        if self.reveal.take().is_some() {
            self.scroll.x = reveal_horizontal_scroll(self.scroll.x, head_pt.x, width);
        }
        self.scroll.x = clamp_horizontal_scroll(self.scroll.x, self.scroll.content_w, width);
        self.finish_frame(layout, color, head_pt, empty, params);
    }

    /// A document's visible lines, scrolled to keep its top line through a reflow and to reveal
    /// the caret when asked; whether a long line's shaped window moved out from under the view.
    fn prepare_document(&mut self, params: EditorPaintParams<'_>, ts: &TextSystem) -> bool {
        let style = params.style;
        let lh = style.line_height.max(1.0);
        let text = self.model.text();
        let reveal = self.reveal.take();
        let (head, end_affinity) = (self.model.cursor(), self.model.selection().end_affinity);
        let view = self.input_bounds.size;
        let doc = self.doc.as_deref_mut().expect("a document");
        let wrap_width = doc
            .soft_wrap
            .then_some((view.width - RIGHT_MARGIN).max(1.0));

        // A reflow keeps the top line in view, unless the caret is about to be revealed anyway.
        let anchor = reveal
            .is_none()
            .then(|| doc.capture_scroll_anchor(self.scroll.y, doc.last_line_height))
            .flatten();
        let reflowed = doc.lines.sync_layout(text, &style.text, wrap_width, ts)
            || (doc.last_line_height - lh).abs() > 0.01;
        doc.last_line_height = lh;
        if let Some(anchor) = anchor.filter(|_| reflowed) {
            self.scroll.y = doc.restore_scroll_anchor(text, anchor, lh);
        }

        let rows = doc.lines.visual_row_count().max(1);
        let content_h = (rows as f64 * f64::from(lh)).max(f64::from(view.height));
        self.scroll.content_h = content_h;
        if let Some(req) = reveal {
            // Scrolls the least that brings the caret's row (and its margin) into view, but
            // only the way the caret moved.
            let (lh, height) = (f64::from(lh), f64::from(view.height));
            let row_y = doc.lines.offset_to_display(text, head, end_affinity).row as f64 * lh;
            let margin = if req.with_margin {
                f64::from(caret_vertical_margin(
                    view.height,
                    style.line_height.max(1.0),
                ))
            } else {
                0.0
            };
            let was = self.scroll.y;
            let mut y = was;
            if row_y < -y + margin {
                y = (margin - row_y).min(0.0);
            } else if row_y + lh > -y + height - margin {
                y = height - margin - (row_y + lh);
            }
            self.scroll.y = match req.direction {
                Some(MoveDirection::Up) => y.max(was),
                Some(MoveDirection::Down) => y.min(was),
                None => y,
            };
        }
        let min_y = (-content_h + f64::from(view.height)).min(0.0);
        self.scroll.y = self.scroll.y.clamp(min_y, 0.0);

        // Unwrapped, the horizontal extent and reveal are settled before the windowed lines are
        // shaped, then refined from what was shaped.
        let width = view.width;
        let mut widest = match wrap_width {
            Some(wrap) => {
                self.scroll.x = 0.0;
                wrap
            }
            None => {
                let mut widest = doc.lines.longest_line_width(text, ts);
                if reveal.is_some() {
                    let caret_x = doc.lines.unwrapped_x(text, head, ts);
                    widest = widest.max(caret_x);
                    self.scroll.x = reveal_horizontal_scroll(self.scroll.x, caret_x, width);
                }
                let estimate = content_width_with_margin(widest, width);
                self.scroll.x = clamp_horizontal_scroll(self.scroll.x, estimate, width);
                widest
            }
        };
        let shaped_at = self.scroll.x;
        let shown = Viewport {
            size: view,
            scroll_x: self.scroll.x,
            scroll_y: self.scroll.y,
        };
        let (layout, windowed) = doc
            .lines
            .layout_visible(text, shown, head, end_affinity, lh, ts);
        let head_pt = layout
            .position_for_offset(text, head, end_affinity)
            .unwrap_or_else(|| {
                let row = doc.lines.offset_to_display(text, head, end_affinity).row;
                point(0.0, (row as f64 * f64::from(lh) + self.scroll.y) as f32)
            });
        if wrap_width.is_none() {
            widest = widest.max(layout.width()).max(head_pt.x);
            if reveal.is_some() {
                self.scroll.x = reveal_horizontal_scroll(self.scroll.x, head_pt.x, width);
            }
        }
        self.scroll.content_w = content_width_with_margin(widest, width);
        self.scroll.x = clamp_horizontal_scroll(self.scroll.x, self.scroll.content_w, width);
        self.finish_frame(layout, style.color, head_pt, false, params);
        windowed && (self.scroll.x - shaped_at).abs() > 1.0
    }

    /// Keeps `layout` as the frame, with the caret at `head_pt` and the selection over it.
    fn finish_frame(
        &mut self,
        layout: FrameLayout,
        text_color: Rgba,
        head_pt: Point,
        placeholder: bool,
        params: EditorPaintParams<'_>,
    ) {
        // Vertical scroll is baked into each line's top in f64 so D2D coordinates stay exact on
        // huge files.
        let origin = self.input_bounds.origin + point(self.scroll.x, 0.);
        let caret_rect = caret_rect(
            origin + head_pt,
            layout.line_height,
            params.style.caret_height,
            params.scale,
            params.caret_width_px,
        );
        let m = &self.model;
        let composing = m.composition().is_some();
        let show_caret = self.focused
            && params.window_active
            && self.blink.visible
            && (m.selection().is_empty() || composing);
        let marked_range = m
            .composition()
            .filter(|_| !placeholder)
            .map(|c| c.start..c.end);
        let selection_rects = if params.window_active
            && params.show_selection
            && !composing
            && !m.selection().is_empty()
        {
            layout.range_rects(&m.selection().range(), origin)
        } else {
            Vec::new()
        };
        self.frame = Some(Frame {
            layout,
            revision: m.revision(),
            scroll_x: self.scroll.x,
            scroll_y: self.scroll.y,
            origin,
            text_color,
            caret_rect,
            show_caret,
            selection_rects,
            marked_range,
            last_cursor: m.cursor(),
        });
    }

    pub(crate) fn paint(&self, p: Palette, canvas: &mut Canvas) {
        let Some(frame) = self.frame.as_ref() else {
            return;
        };
        canvas.with_clip(self.input_bounds, |canvas| {
            for &rect in &frame.selection_rects {
                canvas.paint_quad(fill(rect, p.selection));
            }
            let marked = frame.marked_range.as_ref();
            frame
                .layout
                .paint(frame.origin, frame.text_color, marked, canvas);
            if frame.show_caret {
                canvas.paint_quad(fill(frame.caret_rect, p.caret));
            }
        });
        if let Some(doc) = &self.doc {
            doc.scrollbar.paint(canvas);
        }
    }
}

/// The caret at `at` (the top of its line), `height` tall and centred on the line, on whole
/// pixels.
fn caret_rect(at: Point, line_height: f32, height: f32, scale: f32, width_px: u32) -> Rect {
    let top = point(at.x, at.y + (line_height - height) * 0.5);
    let origin = point(
        (top.x * scale).round() / scale,
        (top.y * scale).round() / scale,
    );
    let width = (width_px.max(1) as f32 / scale).max(1.0 / scale);
    Rect::new(origin, size(width, height))
}

// ---- 8. Timers: blinking and auto-scroll ----------------------------------------------------

impl Editor {
    /// Acts on whatever is due at `now`: blinking, auto-scrolling, scrollbar fading. Returns
    /// whether the editor needs repainting.
    pub(crate) fn tick(&mut self, now: Instant) -> bool {
        let mut changed = self.blink.tick(now);
        let Some(doc) = &mut self.doc else {
            return changed;
        };
        changed |= doc.scrollbar.tick(now);
        if take_due(&mut doc.auto_scroll.next, now) {
            let Some(delta) = doc.auto_scroll.delta else {
                doc.auto_scroll.stop();
                return changed;
            };
            doc.auto_scroll.next = Some(now + AUTO_SCROLL_INTERVAL);
            let drag_pos = doc.auto_scroll.last_drag_pos;
            self.reveal = None;
            self.scroll_to(self.scroll.x, self.scroll.y + f64::from(delta));
            // The selection follows the text scrolling under the pointer.
            if let Some(pos) = drag_pos {
                let (offset, aff) = self.hit_test_point(pos);
                self.select_to_with_affinity(offset, aff);
            }
            changed = true;
        }
        changed
    }

    /// When the editor next needs a `tick`.
    pub(crate) fn deadline(&self) -> Option<Instant> {
        let doc = self.doc.as_deref();
        let auto_scroll = doc.and_then(|doc| doc.auto_scroll.next);
        let scrollbar = doc.and_then(|doc| doc.scrollbar.deadline());
        [self.blink.next, auto_scroll, scrollbar]
            .into_iter()
            .flatten()
            .min()
    }
}

/// The caret's blinking, on while the editor has the keyboard: it holds steady while keys come
/// in, and ends visible after the system's timeout.
#[derive(Default)]
struct BlinkState {
    /// While on: the system's blink interval and timeout, and when blinking last (re)started.
    on: Option<(Option<Duration>, Option<Duration>, Instant)>,
    visible: bool,
    next: Option<Instant>,
}

impl BlinkState {
    fn start(&mut self, (interval, timeout): (Option<Duration>, Option<Duration>)) {
        let now = Instant::now();
        *self = Self {
            on: Some((interval, timeout, now)),
            visible: false,
            next: None,
        };
        self.step(now);
    }

    /// Holds the caret visible for an interval (after typing or moving), then blinks again.
    fn pause(&mut self) {
        if let Some((interval, _, since)) = &mut self.on {
            let now = Instant::now();
            *since = now;
            self.visible = true;
            self.next = interval.map(|interval| now + interval);
        }
    }

    /// Blinks if it's due; whether the caret changed.
    fn tick(&mut self, now: Instant) -> bool {
        let due = take_due(&mut self.next, now);
        if due {
            self.step(now);
        }
        due
    }

    /// Shows or hides the caret and schedules the next blink, or leaves it shown once blinking
    /// has timed out (or the system doesn't blink).
    fn step(&mut self, now: Instant) {
        let Some((interval, timeout, since)) = self.on else {
            return;
        };
        let timed_out = timeout.is_some_and(|t| now.saturating_duration_since(since) >= t);
        match interval {
            Some(interval) if !timed_out => {
                self.visible = !self.visible;
                self.next = Some(now + interval);
            }
            _ => (self.visible, self.next) = (true, None),
        }
    }
}

/// Scrolling while a drag-selection is past the editor's top or bottom edge.
#[derive(Default)]
struct AutoScroll {
    /// How far each step scrolls (negative is down the text), while past an edge.
    delta: Option<f32>,
    next: Option<Instant>,
    last_drag_pos: Option<Point>,
}

/// How often a drag-selection past the editor's edge scrolls.
const AUTO_SCROLL_INTERVAL: Duration = Duration::from_millis(16);

impl AutoScroll {
    /// How fast a drag at `y` scrolls: not within the edges, faster the further past.
    fn compute_delta(y: f32, bounds: Rect) -> Option<f32> {
        const MIN_SPEED: f32 = 12.0;
        const MAX_SPEED: f32 = 64.0;
        const INNER_ZONE: f32 = 16.0;
        const OUTER_RAMP: f32 = 80.0;
        let speed = |past: f32| {
            let t = (past / (INNER_ZONE + OUTER_RAMP)).min(1.0);
            MIN_SPEED + t * (MAX_SPEED - MIN_SPEED)
        };
        let (top, bottom) = (bounds.top() + INNER_ZONE, bounds.bottom() - INNER_ZONE);
        if y > bottom {
            Some(speed(y - bottom))
        } else if y < top {
            Some(-speed(top - y))
        } else {
            None
        }
    }

    fn stop(&mut self) {
        *self = Self::default();
    }

    fn update(&mut self, delta: Option<f32>) {
        let was_active = self.delta.is_some();
        self.delta = delta;
        if delta.is_none() {
            self.next = None;
        } else if !was_active {
            self.next = Some(Instant::now() + AUTO_SCROLL_INTERVAL);
        }
    }
}

// ---- 9. Scrollbars --------------------------------------------------------------------------

const TRACK_THICKNESS: f32 = 16.;
const MIN_THUMB_LENGTH: f32 = 48.;
const THUMB_MARGIN: f32 = 3.;
const THUMB_REST_THICKNESS: f32 = 6.;
const THUMB_ACTIVE_THICKNESS: f32 = 8.;
const THUMB_REST_RADIUS: f32 = 3.;
const THUMB_ACTIVE_RADIUS: f32 = 4.;

/// How long the bars stay after the last scroll or hover, then how they fade and thicken.
const IDLE_HOLD: Duration = Duration::from_secs(2);
const FADE_IN: Curve = Curve::new(Duration::from_millis(120), Easing::Linear);
const FADE_OUT: Curve = Curve::new(Duration::from_millis(200), Easing::EaseInCubic);
const EXPAND: Curve = Curve::new(Duration::from_millis(120), Easing::EaseOutCubic);
/// A dragged thumb repaints at most this often.
const DRAG_NOTIFY_INTERVAL: Duration = Duration::from_micros(8_333);

fn axis_val(p: Point, axis: Axis) -> f32 {
    match axis {
        Axis::Vertical => p.y,
        Axis::Horizontal => p.x,
    }
}

fn set_axis_val(p: &mut Point, axis: Axis, v: f32) {
    match axis {
        Axis::Vertical => p.y = v,
        Axis::Horizontal => p.x = v,
    }
}

fn axis_size(s: Size, axis: Axis) -> f32 {
    match axis {
        Axis::Vertical => s.height,
        Axis::Horizontal => s.width,
    }
}

/// A corner radius that fits a pill `width` by `height`.
fn clamp_radius(radius: f32, width: f32, height: f32) -> f32 {
    radius.min(width * 0.5).min(height * 0.5).max(0.0)
}

/// A bar's thumb along its track: its margin and length, how far it travels, and how far the
/// content scrolls.
#[derive(Clone, Copy, Debug)]
struct TrackMetrics {
    margin: f32,
    thumb_len: f32,
    travel: f32,
    max_scroll: f32,
}

impl TrackMetrics {
    fn compute(viewport_span: f32, content_span: f32, corner_reserve: f32) -> Self {
        let track_span = (viewport_span - corner_reserve).max(0.);
        let thumb = (viewport_span / content_span * viewport_span)
            .max(MIN_THUMB_LENGTH)
            .min(track_span);
        let margin = THUMB_MARGIN.clamp(0., thumb * 0.5);
        Self {
            margin,
            thumb_len: thumb - margin * 2.,
            travel: track_span - thumb,
            max_scroll: content_span - viewport_span,
        }
    }

    /// Where the thumb starts along the track at `scroll_offset`.
    fn thumb_local(self, scroll_offset: f32) -> f32 {
        let ratio = (-scroll_offset / self.max_scroll).clamp(0., 1.);
        self.margin + ratio * self.travel
    }

    /// `offset` with the axis scrolled to where the pointer, holding the thumb `grab` into it,
    /// puts it.
    fn apply_drag(
        self,
        axis: Axis,
        track_origin: f32,
        pointer: Point,
        grab: f32,
        mut offset: Point,
    ) -> Point {
        if self.travel <= 0. {
            return offset;
        }
        let along = axis_val(pointer, axis) - track_origin - self.margin - grab;
        set_axis_val(
            &mut offset,
            axis,
            -self.max_scroll * (along / self.travel).clamp(0., 1.),
        );
        offset
    }
}

/// What the scrollbars are drawn from: where the text was laid out (the tracks lie over its
/// padding), how big it is, and how far it's scrolled.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ScrollbarViewport {
    input: Rect,
    paddings: Edges<f32>,
    scale: f32,
    content_size: Size,
    scroll_offset: Point,
    horizontal: bool,
}

/// A thumb being dragged, held `grab` along from its start.
#[derive(Clone, Copy, Debug)]
struct Drag {
    axis: Axis,
    grab: f32,
}

/// One bar as last laid out.
#[derive(Clone, Copy, Debug)]
struct ActiveAxis {
    metrics: TrackMetrics,
    viewport_span: f32,
    thumb_local: f32,
    interactive: bool,
    /// The track, in window coordinates, and the thumb's span along it.
    track: Rect,
    thumb: (f32, f32),
    /// The thumb's pill as painted, and whether it shows at all.
    pill: Rect,
    radius: f32,
    color: Rgba,
    shown: bool,
    /// The pointer was on the thumb (which then shows highlighted).
    hovered: bool,
}

impl ActiveAxis {
    fn on_thumb(&self, axis: Axis, pos: Point) -> bool {
        self.shown && (self.thumb.0..self.thumb.1).contains(&axis_val(pos, axis))
    }
}

/// A document's scrollbars: overlay bars that show while scrolling or hovered, fade after a
/// while, thicken under the pointer, and drag or page.
#[derive(Default)]
struct ScrollbarState {
    drag: Option<Drag>,
    last_offset: Point,
    last_activity: Option<Instant>,
    was_hovered: bool,
    last_drag_notify: Option<Instant>,
    /// When a throttled drag next notifies, and when the bar next starts fading.
    drag_at: Option<Instant>,
    idle_at: Option<Instant>,
    /// Each bar as last laid out, by `Axis`.
    axes: [Option<ActiveAxis>; 2],
}

impl ScrollbarState {
    fn is_interactive(&self, axis: Axis) -> bool {
        self.axes[axis as usize].is_some_and(|a| a.interactive)
    }

    /// Whether the pointer at `pos` has come onto or off a thumb since it was last drawn (with
    /// nothing over its track).
    fn hover_changed(&self, slot: EditorSlot, pos: Point, ui: &AppUi) -> bool {
        [Axis::Vertical, Axis::Horizontal].into_iter().any(|axis| {
            self.axes[axis as usize].is_some_and(|a| {
                let over = ui.is_hovered(HitId::ScrollTrack(slot, axis));
                (a.interactive && over && a.on_thumb(axis, pos)) != a.hovered
            })
        })
    }

    fn tick(&mut self, now: Instant) -> bool {
        let idle = take_due(&mut self.idle_at, now);
        let drag = take_due(&mut self.drag_at, now);
        if drag {
            self.last_drag_notify = Some(now);
        }
        idle || drag
    }

    fn deadline(&self) -> Option<Instant> {
        self.idle_at.into_iter().chain(self.drag_at).min()
    }

    /// Works the bars out for the editor's new layout: which are needed, how far each has faded
    /// in, and where its thumb sits. Whether the set of bars changed, since their tracks are
    /// laid out from it (in the next frame).
    fn update(
        &mut self,
        slot: EditorSlot,
        vp: ScrollbarViewport,
        p: Palette,
        now: Instant,
        ui: &mut AppUi,
    ) -> bool {
        let before = self.axes.map(|a| a.is_some());
        let scroll_offset = vp.scroll_offset;
        let track_hovered = [Axis::Vertical, Axis::Horizontal]
            .map(|axis| ui.is_hovered(HitId::ScrollTrack(slot, axis)));
        let hovered = track_hovered.contains(&true);
        // Scrolling, and the pointer on a track or just leaving one, keep the bars awake.
        if scroll_offset != self.last_offset || hovered || self.was_hovered {
            self.last_activity = Some(now);
            self.idle_at = None;
        }
        (self.last_offset, self.was_hovered) = (scroll_offset, hovered);
        let recent = self
            .last_activity
            .filter(|&t| now.saturating_duration_since(t) < IDLE_HOLD);
        let visible = hovered || self.drag.is_some() || recent.is_some();
        // Left alone, they fade when the hold runs out.
        if !hovered
            && self.drag.is_none()
            && self.idle_at.is_none()
            && let Some(last) = recent
        {
            self.idle_at = Some(last + IDLE_HOLD);
        }

        // The bars measure the text area with the padding their tracks lie over.
        let pad = vp.paddings;
        let padded = |s: Size| size(s.width + pad.right, s.height + pad.top + pad.bottom);
        let (viewport, content) = (padded(vp.input.size), padded(vp.content_size));
        let vertical = viewport.height > 0. && content.height > viewport.height;
        let horizontal = vp.horizontal && viewport.width > 0. && content.width > viewport.width;
        self.axes = [None, None];
        if !vertical && !horizontal {
            return before != [false, false];
        }

        let fade = if visible { FADE_IN } else { FADE_OUT };
        let opacity = ui
            .anims
            .progress(AnimKey::ScrollOpacity(slot), visible, fade)
            .value;
        let interactive = visible || opacity > 0.0;

        // In device pixels, rounded value by value the way layout places nodes.
        let scale = vp.scale;
        let px = |v: f32| round_px(v, scale);
        let input = vp.input;
        let view = Rect::from_corners(
            point(
                input.left() * scale - px(pad.left),
                input.top() * scale - px(pad.top),
            ),
            point(
                input.right() * scale + px(pad.right),
                input.bottom() * scale + px(pad.bottom),
            ),
        );
        let logical = |r: Rect| {
            Rect::new(
                point(r.origin.x / scale, r.origin.y / scale),
                size(r.size.width / scale, r.size.height / scale),
            )
        };
        for (needed, axis) in [(horizontal, Axis::Horizontal), (vertical, Axis::Vertical)] {
            if !needed {
                continue;
            }
            let viewport_span = axis_size(viewport, axis);
            // The horizontal bar stops short of the vertical one's corner.
            let corner = if axis == Axis::Horizontal && vertical {
                TRACK_THICKNESS
            } else {
                0.
            };
            let metrics = TrackMetrics::compute(viewport_span, axis_size(content, axis), corner);
            let thumb_local = metrics.thumb_local(axis_val(scroll_offset, axis));

            // The track and the thumb's span along it, then whether the pointer is on the thumb
            // where it now sits.
            let (thumb_at, thumb_len) = (px(thumb_local), px(metrics.thumb_len));
            let track = match axis {
                Axis::Vertical => Rect::from_corners(
                    point(view.right() - px(TRACK_THICKNESS), view.top()),
                    point(view.right(), view.bottom()),
                ),
                Axis::Horizontal => Rect::from_corners(
                    point(
                        view.left() + px(pad.left),
                        view.bottom() - px(TRACK_THICKNESS),
                    ),
                    point(view.right() - px(corner), view.bottom()),
                ),
            };
            let thumb_start = axis_val(track.origin, axis) + thumb_at;
            let thumb = (thumb_start / scale, (thumb_start + thumb_len) / scale);
            let shown = opacity > 0.0;
            let pointer_on_thumb = ui
                .pointer
                .is_some_and(|ptr| (thumb.0..thumb.1).contains(&axis_val(ptr.pos, axis)));
            let hovered = interactive && shown && track_hovered[axis as usize] && pointer_on_thumb;

            let dragged = self.drag.is_some_and(|d| d.axis == axis);
            let (color, target_thickness, radius) = if dragged {
                (p.text_primary, THUMB_ACTIVE_THICKNESS, THUMB_ACTIVE_RADIUS)
            } else if hovered {
                (
                    p.text_secondary,
                    THUMB_ACTIVE_THICKNESS,
                    THUMB_ACTIVE_RADIUS,
                )
            } else {
                (p.text_tertiary, THUMB_REST_THICKNESS, THUMB_REST_RADIUS)
            };
            let key = AnimKey::ScrollThickness(slot, axis);
            let thickness = ui
                .anims
                .animate_f32_from(key, THUMB_REST_THICKNESS, target_thickness, EXPAND)
                .value;
            let (pill_w, pill_h) = match axis {
                Axis::Vertical => (thickness, metrics.thumb_len),
                Axis::Horizontal => (metrics.thumb_len, thickness),
            };
            let (margin, thick) = (px(THUMB_MARGIN), px(thickness));
            let pill = match axis {
                Axis::Vertical => Rect::new(
                    point(track.right() - margin - thick, track.top() + thumb_at),
                    size(thick, thumb_len),
                ),
                Axis::Horizontal => Rect::new(
                    point(track.left() + thumb_at, track.bottom() - margin - thick),
                    size(thumb_len, thick),
                ),
            };
            self.axes[axis as usize] = Some(ActiveAxis {
                metrics,
                viewport_span,
                thumb_local,
                interactive,
                track: logical(track),
                thumb,
                pill: logical(pill),
                radius: clamp_radius(radius, pill_w, pill_h),
                color: color.opacity(opacity),
                shown,
                hovered,
            });
        }
        self.axes.map(|a| a.is_some()) != before
    }

    /// The bars' tracks, laid out over the editor's padding for hit-testing and the cursor.
    fn tracks(&self, slot: EditorSlot, horizontal_left: f32) -> Vec<AppNode> {
        let corner = if self.axes[Axis::Vertical as usize].is_some() {
            TRACK_THICKNESS
        } else {
            0.
        };
        [Axis::Horizontal, Axis::Vertical]
            .into_iter()
            .filter_map(|axis| {
                let active = self.axes[axis as usize]?;
                let track = match axis {
                    Axis::Vertical => box_()
                        .absolute()
                        .right(0.)
                        .top(0.)
                        .bottom(0.)
                        .w(TRACK_THICKNESS),
                    Axis::Horizontal => box_()
                        .absolute()
                        .left(horizontal_left)
                        .right(corner)
                        .bottom(0.)
                        .h(TRACK_THICKNESS),
                };
                let track = track.hit(HitId::ScrollTrack(slot, axis));
                Some(track.when(active.interactive, |n| n.cursor(CursorStyle::Arrow)))
            })
            .collect()
    }

    fn paint(&self, canvas: &mut Canvas) {
        for axis in [Axis::Horizontal, Axis::Vertical] {
            if let Some(active) = self.axes[axis as usize].filter(|a| a.shown) {
                canvas.paint_quad(PaintQuad {
                    bounds: active.pill,
                    corner_radii: Corners::all(active.radius),
                    background: active.color,
                    ..Default::default()
                });
            }
        }
    }

    /// A press on `axis`'s track: on the thumb it starts a drag, elsewhere it pages toward the
    /// pointer. The offset to scroll to.
    fn on_mouse_down(
        &mut self,
        axis: Axis,
        position: Point,
        current_offset: Point,
    ) -> Option<Point> {
        let active = self.axes[axis as usize].filter(|a| a.interactive)?;
        let track_origin = axis_val(active.track.origin, axis);
        let pointer = axis_val(position, axis);
        let thumb_origin = track_origin + active.thumb_local;
        if active.on_thumb(axis, position) {
            let grab = pointer - thumb_origin;
            self.drag = Some(Drag { axis, grab });
            let metrics = active.metrics;
            return Some(metrics.apply_drag(axis, track_origin, position, grab, current_offset));
        }
        let page = (active.viewport_span - 24.0).max(24.0);
        let delta = if pointer < thumb_origin { page } else { -page };
        let mut next = current_offset;
        set_axis_val(&mut next, axis, axis_val(current_offset, axis) + delta);
        Some(next)
    }

    /// A dragged thumb follows the pointer: the offset to scroll to (if it moved) and whether
    /// to repaint now, or `None` when no drag is going on.
    fn on_mouse_move(
        &mut self,
        position: Point,
        dragging_primary: bool,
        current_offset: Point,
    ) -> Option<(Option<Point>, bool)> {
        let drag = self.drag.filter(|_| dragging_primary)?;
        let active = self.axes[drag.axis as usize]?;
        let track_origin = axis_val(active.track.origin, drag.axis);
        let now = Instant::now();
        let next =
            active
                .metrics
                .apply_drag(drag.axis, track_origin, position, drag.grab, current_offset);
        let moved = next != current_offset;
        if moved {
            match self.last_drag_notify {
                // Too soon since the last repaint: one comes when the interval's up.
                Some(last) if now.saturating_duration_since(last) < DRAG_NOTIFY_INTERVAL => {
                    self.drag_at.get_or_insert(last + DRAG_NOTIFY_INTERVAL);
                }
                _ => {
                    self.last_drag_notify = Some(now);
                    self.drag_at = None;
                    return Some((Some(next), true));
                }
            }
        }
        Some((moved.then_some(next), false))
    }

    /// Ends a thumb drag at `position`: the offset to scroll to.
    fn on_mouse_up(&mut self, position: Point, current_offset: Point) -> Option<Point> {
        let drag = self.drag.take()?;
        self.drag_at = None;
        self.last_activity = Some(Instant::now());
        let active = self.axes[drag.axis as usize]?;
        let track_origin = axis_val(active.track.origin, drag.axis);
        Some(active.metrics.apply_drag(
            drag.axis,
            track_origin,
            position,
            drag.grab,
            current_offset,
        ))
    }
}

#[cfg(test)]
mod tests {
    use crate::geom::{point, rect, size};
    use crate::ui::TabId;

    use super::*;

    /// Drawing and clicking agree: clicking the middle of the drawn caret lands on the caret,
    /// clicking inside a drawn selection box lands inside the selection, and the IME underline
    /// covers the composition. Runs over wrapped, scrolled and horizontally windowed lines.
    #[test]
    fn clicks_land_where_the_caret_and_selection_are_drawn() {
        let ts = TextSystem::new().unwrap();
        let palette = Palette::resolve(false, None, None);
        let style = EditorPaintStyle {
            text: TextStyle::regular("Arial", 14.),
            line_height: 20.,
            color: palette.text_primary,
            caret_height: 17.,
        };
        let params = EditorPaintParams {
            style: &style,
            palette,
            window_active: true,
            show_selection: true,
            scale: 1.0,
            caret_width_px: 1,
            now: Instant::now(),
        };
        let mut ui = AppUi::default();
        ui.begin_frame(Instant::now(), true);
        let bounds = Rect::new(point(10., 20.), size(300., 120.));
        let words = "lorem ipsum dolor sit amet ".repeat(30);
        let text = format!("short line\n{words}\n\tindented {words}\n{}\nend", "x".repeat(6000));
        for soft_wrap in [true, false] {
            let paddings = Edges::ZERO;
            let opts = DocOptions {
                tab: TabId::next(),
                soft_wrap,
                paddings,
            };
            let mut ed = Editor::document(Rope::from(text.as_str()), opts);
            for offset in (0..=text.len()).step_by(37) {
                ed.set_selected_range(offset..offset);
                ed.prepare(bounds, params, &mut ui, &ts);
                let caret = ed.caret_bounds().unwrap();
                assert!(bounds.contains(caret.center()), "caret {caret:?} off screen at {offset}");
                let (hit, _) = ed.hit_test_point(caret.center());
                assert_eq!(hit, offset, "caret at {offset}, wrap {soft_wrap}");
            }

            // A click between a wheel scroll and the next layout still lands on the text under it.
            ed.set_selected_range(1700..1700);
            ed.prepare(bounds, params, &mut ui, &ts);
            let caret = ed.caret_bounds().unwrap();
            let before = ed.scroll_offset();
            let wheel = if soft_wrap { point(0., 1.) } else { point(-1., 0.) };
            ed.on_scroll_wheel(wheel, false, Modifiers::default());
            let moved = ed.scroll_offset() - before;
            assert!(moved != Point::ZERO, "wheel {wheel:?} didn't scroll");
            assert_eq!(ed.hit_test_point(caret.center() + moved).0, 1700);


            let selection = 15..90;
            ed.set_selected_range(selection.clone());
            ed.prepare(bounds, params, &mut ui, &ts);
            let rects = ed.frame.as_ref().unwrap().selection_rects.clone();
            assert!(!rects.is_empty());
            for rect in rects {
                let (hit, _) = ed.hit_test_point(rect.center());
                assert!(selection.contains(&hit) || hit == selection.end, "box {rect:?} hit {hit}");
            }

            ed.set_selected_range(40..40);
            ed.handle_ime_composition("にほん", 2, &ts);
            ed.prepare(bounds, params, &mut ui, &ts);
            let composing = ed.model().composition().map(|c| c.start..c.end);
            assert_eq!(ed.frame.as_ref().unwrap().marked_range, composing);
            let caret = ed.caret_bounds().unwrap();
            assert_eq!(ed.hit_test_point(caret.center()).0, ed.model().cursor());
        }
    }

    /// Unwrapped, typing past the right edge scrolls the view along with the caret, and going
    /// back to the line's start scrolls it home.
    #[test]
    fn typing_past_the_right_edge_scrolls_the_caret_into_view() {
        let ts = TextSystem::new().unwrap();
        let palette = Palette::resolve(false, None, None);
        let style = EditorPaintStyle {
            text: TextStyle::regular("Arial", 14.),
            line_height: 20.,
            color: palette.text_primary,
            caret_height: 17.,
        };
        let params = EditorPaintParams {
            style: &style,
            palette,
            window_active: true,
            show_selection: true,
            scale: 1.0,
            caret_width_px: 1,
            now: Instant::now(),
        };
        let mut ui = AppUi::default();
        ui.begin_frame(Instant::now(), true);
        let bounds = Rect::new(point(10., 20.), size(200., 100.));
        let opts = DocOptions {
            tab: TabId::next(),
            soft_wrap: false,
            paddings: Edges::ZERO,
        };
        let mut ed = Editor::document(Rope::from("first\n"), opts);
        ed.prepare(bounds, params, &mut ui, &ts);
        for i in 0..120 {
            ed.handle_text_input(if i % 5 == 4 { " " } else { "w" }, &ts);
            ed.prepare(bounds, params, &mut ui, &ts);
            let caret = ed.caret_bounds().unwrap();
            assert!(
                bounds.contains(caret.center()),
                "caret {caret:?} off screen after {i}"
            );
        }
        assert!(
            ed.scroll_offset().x < -200.,
            "scrolled {:?}",
            ed.scroll_offset()
        );
        let home = EditCommand::Move {
            motion: Motion::LineStart,
            extend: false,
        };
        assert!(ed.handle_command(home, &ts));
        ed.prepare(bounds, params, &mut ui, &ts);
        assert_eq!(ed.scroll_offset().x, 0.);
        assert!(bounds.contains(ed.caret_bounds().unwrap().center()));
    }

    #[test]
    fn editor_direct_typing_undo_redo_and_word_navigation() {
        let ts = TextSystem::new().unwrap();
        let mut ed = Editor::document(
            Rope::new(),
            DocOptions {
                tab: TabId::next(),
                soft_wrap: true,
                paddings: Edges::ZERO,
            },
        );

        ed.handle_text_input("hello", &ts);
        ed.handle_text_input(" world", &ts);
        assert_eq!(ed.model().text().to_string(), "hello world");
        assert!(!ed.model().is_clean());

        assert!(ed.handle_command(EditCommand::Undo, &ts));
        assert_eq!(ed.model().text().to_string(), "");
        assert!(ed.model().is_clean());

        assert!(ed.handle_command(EditCommand::Redo, &ts));
        assert_eq!(ed.model().text().to_string(), "hello world");

        assert!(ed.handle_command(
            EditCommand::Move {
                motion: Motion::PrevWord,
                extend: false,
            },
            &ts,
        ));
        assert_eq!(ed.model().cursor(), 6);
        assert!(ed.handle_command(
            EditCommand::Move {
                motion: Motion::NextWord,
                extend: true,
            },
            &ts,
        ));
        assert_eq!(ed.model().text().slice(ed.model().selection().range()).to_string(), "world");

        let copied = ed.selected_string();
        assert_eq!(copied.as_deref(), Some("world"));
        assert!(ed.handle_command(EditCommand::Delete, &ts));
        assert_eq!(ed.model().text().to_string(), "hello ");
        ed.replace("world", &ts);
        assert_eq!(ed.model().text().to_string(), "hello world");
    }

    #[test]
    fn auto_scroll_delta_has_dead_zone_and_symmetric_ramps() {
        let bounds = rect(0., 100., 200., 100.);
        assert_eq!(AutoScroll::compute_delta(150., bounds), None);

        let top = AutoScroll::compute_delta(80., bounds).unwrap();
        let bottom = AutoScroll::compute_delta(220., bounds).unwrap();
        assert!(top < 0. && bottom > 0.);
        assert!((top.abs() - bottom.abs()).abs() < 1e-3);

        let mut scroll = AutoScroll::default();
        scroll.stop();
        assert!(scroll.delta.is_none());
    }

    #[test]
    fn thumb_radius_is_limited_by_its_actual_bounds() {
        assert_eq!(clamp_radius(6., 8., 80.), 4.);
        assert_eq!(clamp_radius(6., 80., 6.), 3.);
        assert_eq!(clamp_radius(0., 8., 80.), 0.);
    }

    #[test]
    fn full_track_thumb_preserves_scroll_offset() {
        let metrics = TrackMetrics::compute(200., 250., 0.);
        assert!(metrics.travel > 0.);
        let dragged = metrics.apply_drag(Axis::Vertical, 0., point(0., 100.), 10., point(-5., 0.));
        assert_eq!(dragged.x, -5.);
        assert!(dragged.y < 0.);

        let zero_travel = TrackMetrics::compute(40., 100., 0.);
        assert_eq!(zero_travel.travel, 0.);
        let kept = zero_travel.apply_drag(Axis::Vertical, 0., point(0., 20.), 5., point(0., -12.));
        assert_eq!(kept.y, -12.);
    }

    #[test]
    fn caret_blinks_at_its_deadlines_pauses_and_stops() {
        let interval = Duration::from_millis(500);
        let mut blink = BlinkState::default();
        blink.start((Some(interval), Some(Duration::from_secs(5))));
        let due = blink.next.expect("blinking schedules the first toggle");
        let shown = blink.visible;
        assert!(!blink.tick(due - Duration::from_millis(1)));
        assert!(blink.tick(due));
        assert_ne!(blink.visible, shown);
        assert_eq!(blink.next, Some(due + interval));

        blink.pause();
        assert!(blink.visible, "typing holds the caret visible");
        let resume = blink.next.expect("a pause resumes after an interval");
        assert!(blink.tick(resume));
        assert!(!blink.visible, "it blinks again after the pause");

        let late = blink.on.unwrap().2 + Duration::from_secs(6);
        blink.next = Some(late);
        assert!(blink.tick(late));
        assert!(
            blink.visible && blink.next.is_none(),
            "blinking ends visible after the timeout"
        );

        blink.start((Some(interval), None));
        blink = BlinkState::default();
        assert!(blink.next.is_none() && !blink.tick(Instant::now() + interval));
    }

    #[test]
    fn auto_scroll_repeats_while_dragging_past_the_edge() {
        let text = Rope::from("line\n".repeat(200).as_str());
        let mut ed = Editor::document(
            text,
            DocOptions {
                tab: TabId::next(),
                soft_wrap: false,
                paddings: Edges::ZERO,
            },
        );
        ed.doc
            .as_deref_mut()
            .unwrap()
            .auto_scroll
            .update(Some(30.0));
        let due = ed
            .deadline()
            .expect("dragging past the edge schedules scrolling");
        assert!(!ed.tick(due - Duration::from_millis(1)));
        assert!(ed.tick(due));
        assert_eq!(
            ed.deadline(),
            Some(due + AUTO_SCROLL_INTERVAL),
            "and it repeats"
        );
        ed.doc.as_deref_mut().unwrap().auto_scroll.update(None);
        assert_eq!(ed.deadline(), None, "leaving the edge stops it");
    }

    #[test]
    fn scene_scrollbar_renders_fades_and_snaps_on_reduce_motion() {
        let t0 = Instant::now();
        let mut sb = ScrollbarState::default();
        let mut ui = AppUi::default();
        let p = Palette::resolve(false, None, None);
        let slot = EditorSlot::Tab(TabId::next());
        let vp = ScrollbarViewport {
            input: rect(0., 0., 400., 200.),
            paddings: Edges::ZERO,
            scale: 1.,
            content_size: size(400., 1000.),
            scroll_offset: point(0., -40.),
            horizontal: false,
        };

        ui.begin_frame(t0, false);
        assert!(sb.update(slot, vp, p, t0, &mut ui), "the bar appears");
        assert_eq!(sb.tracks(slot, 0.).len(), 1);
        assert!(sb.is_interactive(Axis::Vertical));
        assert_eq!(
            sb.deadline(),
            Some(t0 + IDLE_HOLD),
            "scrolling arms the fade"
        );
        assert!(ui.finish_frame(crate::ui::AppFrame::default()));

        ui.begin_frame(t0 + FADE_IN.duration, false);
        assert!(!sb.update(slot, vp, p, t0 + FADE_IN.duration, &mut ui));
        assert!(!ui.finish_frame(crate::ui::AppFrame::default()));

        assert!(!sb.tick(t0 + IDLE_HOLD - Duration::from_millis(1)));
        assert!(sb.tick(t0 + IDLE_HOLD), "the fade comes due after the hold");
        assert_eq!(sb.deadline(), None);
        let after_idle = t0 + IDLE_HOLD + Duration::from_millis(10);
        ui.begin_frame(after_idle, true);
        sb.update(slot, vp, p, after_idle, &mut ui);
        assert!(!sb.is_interactive(Axis::Vertical));
    }
}
