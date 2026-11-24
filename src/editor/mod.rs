mod actions;
mod buffer;
mod view;
pub(crate) mod wrap;

pub(crate) use buffer::RopeExt;
pub(crate) use ropey::Rope;
pub(crate) use view::EditorPaintStyle;

use std::ops::Range;
use std::time::Duration;

use ropey::RopeSlice;

use crate::geom::{Edges, Point, Rect, Size};
use crate::gfx::{Font, TextSystem};
use crate::platform::TimerHandle;
use crate::scrollbar::ScrollbarState;
use crate::types::SharedStr;
use crate::ui::{Node, box_, custom};

use buffer::CaretSelection;
use history::History;
use view::{AutoScroll, BlinkState, LongestLineCache, ViewLayout};
use wrap::WrapIndex;

mod history;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum MoveDirection {
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Composition {
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) cursor: usize,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct RevealRequest {
    pub(super) offset: usize,
    pub(super) end_affinity: bool,
    pub(super) direction: Option<MoveDirection>,
    pub(super) with_margin: bool,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum EditorTimer {
    Blink,
    AutoScroll,
    ScrollbarIdle,
    ScrollbarDrag,
}

pub(crate) trait EditorTimers {
    fn after(&mut self, delay: Duration, kind: EditorTimer) -> TimerHandle;
    fn every(&mut self, delay: Duration, interval: Duration, kind: EditorTimer) -> TimerHandle;
}

#[cfg(test)]
pub(crate) struct NoopTimers;

#[cfg(test)]
impl EditorTimers for NoopTimers {
    fn after(&mut self, _delay: Duration, _kind: EditorTimer) -> TimerHandle {
        TimerHandle::default()
    }

    fn every(&mut self, _delay: Duration, _interval: Duration, _kind: EditorTimer) -> TimerHandle {
        TimerHandle::default()
    }
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
    OutdentInline,
    MoveUp,
    MoveDown,
    MoveLeft,
    MoveRight,
    MoveHome,
    MoveEnd,
    MovePageUp,
    MovePageDown,
    MoveToStart,
    MoveToEnd,
    MovePrevWord,
    MoveNextWord,
    SelectAll,
    SelectUp,
    SelectDown,
    SelectLeft,
    SelectRight,
    SelectPageUp,
    SelectPageDown,
    SelectToStartOfLine,
    SelectToEndOfLine,
    SelectToStart,
    SelectToEnd,
    SelectPrevWord,
    SelectNextWord,
    Copy,
    Cut,
    Paste,
    Undo,
    Redo,
    Escape,
}

type ValidateFn = fn(&str) -> bool;

pub(crate) struct Editor {
    multi_line: bool,
    text: Rope,
    wrap: WrapIndex,
    history: History,
    revision: u64,
    soft_wrap: bool,
    blink: BlinkState,
    auto_scroll: AutoScroll,
    scrollbar: ScrollbarState,
    selection: CaretSelection,
    word_anchor: Option<CaretSelection>,
    composition: Option<Composition>,
    layout: Option<ViewLayout>,
    caret_bounds: Option<Rect>,
    caret_height: f32,
    caret_hidden: bool,
    last_cursor: Option<usize>,
    last_line_height: f32,
    input_bounds: Rect,
    content_bounds: Option<Rect>,
    selecting: bool,
    line_end_affinity: bool,
    validate: Option<ValidateFn>,
    scroll_x: f32,
    scroll_y: f64,
    reveal: Option<RevealRequest>,
    content_size: Size,
    content_height_f64: f64,
    longest_line_cache: Option<(LongestLineCache, f32)>,
    paddings: Edges<f32>,
    placeholder: SharedStr,
    pending_context_menu: Option<(Point, usize)>,
}

impl Editor {
    pub(crate) fn single_line() -> Self {
        Self::new_inner(false)
    }

    pub(crate) fn multi_line() -> Self {
        Self::new_inner(true)
    }

    fn new_inner(multi_line: bool) -> Self {
        Self {
            multi_line,
            text: Rope::new(),
            wrap: WrapIndex::new(Font::default(), 14.0, None),
            history: History::new(),
            revision: 0,
            soft_wrap: true,
            blink: BlinkState::new(),
            auto_scroll: AutoScroll::default(),
            scrollbar: ScrollbarState::new(),
            selection: CaretSelection::default(),
            word_anchor: None,
            composition: None,
            layout: None,
            caret_bounds: None,
            caret_height: 0.0,
            caret_hidden: false,
            last_cursor: None,
            last_line_height: 20.0,
            input_bounds: Rect::ZERO,
            content_bounds: None,
            selecting: false,
            line_end_affinity: false,
            validate: None,
            scroll_x: 0.0,
            scroll_y: 0.0,
            reveal: None,
            content_size: Size::ZERO,
            content_height_f64: 0.0,
            longest_line_cache: None,
            paddings: Edges::ZERO,
            placeholder: SharedStr::default(),
            pending_context_menu: None,
        }
    }

    pub(crate) fn with_caret_height(mut self, height: f32) -> Self {
        self.caret_height = height;
        self
    }

    pub(crate) fn with_placeholder(mut self, placeholder: impl Into<SharedStr>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    pub(crate) fn with_validate(mut self, f: ValidateFn) -> Self {
        self.validate = Some(f);
        self
    }

    pub(crate) fn with_soft_wrap(mut self, wrap: bool) -> Self {
        self.soft_wrap = wrap;
        self
    }

    pub(crate) fn with_rope(mut self, rope: Rope) -> Self {
        self.text = rope;
        self
    }

    pub(crate) fn with_paddings(mut self, paddings: Edges<f32>) -> Self {
        self.paddings = paddings;
        self
    }

    #[inline]
    pub(crate) fn text(&self) -> &Rope {
        &self.text
    }

    #[inline]
    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }

    pub(crate) fn value(&self) -> SharedStr {
        self.text.to_string().into()
    }

    #[inline]
    pub(crate) fn caret_bounds(&self) -> Option<Rect> {
        self.caret_bounds
    }

    pub(crate) fn set_caret_height(&mut self, height: f32) {
        self.caret_height = height;
    }

    pub(crate) fn set_caret_hidden(&mut self, hidden: bool) {
        self.caret_hidden = hidden;
    }

    #[cfg(test)]
    #[inline]
    pub(crate) fn soft_wrap_enabled(&self) -> bool {
        self.soft_wrap
    }

    #[inline]
    pub(crate) fn scroll_offset(&self) -> Point {
        crate::geom::point(self.scroll_x, self.scroll_y as f32)
    }

    #[inline]
    pub(super) fn request_reveal(
        &mut self,
        offset: usize,
        end_affinity: bool,
        direction: Option<MoveDirection>,
        with_margin: bool,
    ) {
        self.reveal = Some(RevealRequest {
            offset,
            end_affinity,
            direction,
            with_margin,
        });
    }

    #[inline]
    pub(super) fn reset_pointer_selection_state(&mut self) {
        self.word_anchor = None;
        self.selecting = false;
        self.auto_scroll.stop();
    }

    #[inline]
    pub(super) fn debug_assert_invariants(&self) {
        debug_assert!(self.selection.start <= self.selection.end);
        debug_assert!(self.selection.end <= self.text.len());
        debug_assert!(self.text.is_char_boundary(self.selection.start));
        debug_assert!(self.text.is_char_boundary(self.selection.end));
        if let Some(comp) = self.composition {
            debug_assert!(comp.start <= comp.end && comp.end <= self.text.len());
        }
    }

    #[inline]
    pub(crate) fn is_selecting(&self) -> bool {
        self.selecting
    }

    #[inline]
    pub(crate) fn is_scrollbar_dragging(&self) -> bool {
        self.scrollbar.is_dragging()
    }

    #[inline]
    pub(crate) fn hits_scrollbar_track(&self, pos: Point) -> bool {
        self.multi_line && self.scrollbar.hits_track(pos)
    }

    pub(crate) fn set_soft_wrap(&mut self, wrap: bool, ts: &TextSystem) {
        if self.soft_wrap == wrap {
            return;
        }
        let anchor = self.capture_scroll_anchor(self.last_line_height);
        self.soft_wrap = wrap;
        if wrap {
            let width = self
                .layout
                .as_ref()
                .and_then(|l| l.wrap_width)
                .or_else(|| {
                    (self.input_bounds.size.width > view::RIGHT_MARGIN)
                        .then_some(self.input_bounds.size.width - view::RIGHT_MARGIN)
                });
            self.wrap.set_wrap_width(width, ts);
            self.scroll_x = 0.0;
        } else {
            self.wrap.set_wrap_width(None, ts);
        }
        if let Some(anchor) = anchor {
            self.restore_scroll_anchor(anchor, self.last_line_height);
        }
    }

    pub(crate) fn cursor(&self) -> usize {
        if let Some(comp) = self.composition {
            let start = comp.start.min(self.text.len());
            let end = comp.end.min(self.text.len());
            if start <= end {
                return start.saturating_add(comp.cursor).clamp(start, end);
            }
        }
        self.selection.head().min(self.text.len())
    }

    pub(crate) fn selected_range(&self) -> Range<usize> {
        let len = self.text.len();
        self.selection.start.min(len)..self.selection.end.min(len)
    }

    pub(crate) fn selected_text(&self) -> RopeSlice<'_> {
        let range = self.selected_range();
        let start = self.text.clip_offset(range.start, buffer::Bias::Left);
        let end = self.text.clip_offset(range.end, buffer::Bias::Right);
        self.text.slice(start..end)
    }

    pub(crate) fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    pub(crate) fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    pub(crate) fn begin_save(&mut self) -> u64 {
        self.commit_composition();
        self.history.begin_save()
    }

    pub(crate) fn mark_saved_version(&mut self, version: u64) {
        self.history.mark_saved(version);
    }

    pub(crate) fn is_clean(&self) -> bool {
        self.history.is_clean()
    }

    pub(crate) fn start_blink(&mut self, timers: &mut dyn EditorTimers) {
        self.blink.start(timers);
    }

    pub(crate) fn pause_blink(&mut self, timers: &mut dyn EditorTimers) {
        self.blink.pause(timers);
    }

    pub(crate) fn stop_blink(&mut self) {
        self.blink.stop();
    }

    pub(crate) fn on_blur(&mut self) {
        self.commit_composition();
        self.history.break_coalescing();
        self.blink.stop();
        self.reset_pointer_selection_state();
    }

    pub(crate) fn view<H, C>(
        &self,
        hit: H,
        editor_draw: C,
        scrollbar_draw: Option<C>,
        line_height: f32,
    ) -> Node<H, C> {
        let cursor = crate::platform::CursorStyle::IBeam;
        let text_leaf = if self.multi_line {
            custom(editor_draw)
                .w_full()
                .h_full()
                .flex_1()
                .min_h(2.0 * line_height)
        } else {
            custom(editor_draw).w_full().h(line_height)
        };

        box_()
            .hit(hit)
            .cursor(cursor)
            .flex_1()
            .overflow_x_hidden()
            .when(self.multi_line, |b| b.h_full().padding(self.paddings))
            .when(!self.multi_line, |b| b.row().min_h_full().items_center())
            .child(text_leaf)
            .when_some(scrollbar_draw.filter(|_| self.multi_line), |b, sb| {
                b.child(custom(sb).absolute().size_full())
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_direct_typing_undo_redo_and_word_navigation() {
        let ts = TextSystem::new();
        let mut timers = NoopTimers;
        let mut ed = Editor::multi_line();

        ed.handle_text_input("hello", &ts, &mut timers);
        ed.handle_text_input(" world", &ts, &mut timers);
        assert_eq!(ed.text().to_string(), "hello world");
        assert!(!ed.is_clean());

        assert!(ed.handle_command(EditCommand::Undo, &ts, &mut timers));
        assert_eq!(ed.text().to_string(), "");
        assert!(ed.is_clean());

        assert!(ed.handle_command(EditCommand::Redo, &ts, &mut timers));
        assert_eq!(ed.text().to_string(), "hello world");

        assert!(ed.handle_command(EditCommand::MovePrevWord, &ts, &mut timers));
        assert_eq!(ed.cursor(), 6);
        assert!(ed.handle_command(EditCommand::SelectNextWord, &ts, &mut timers));
        assert_eq!(ed.selected_text().to_string(), "world");
    }
}
