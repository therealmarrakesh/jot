mod actions;
mod buffer;
mod history;
mod model;
mod view;
pub(crate) mod wrap;

pub(crate) use buffer::RopeExt;
pub(crate) use ropey::Rope;
pub(crate) use view::{EditorPaintParams, EditorPaintStyle};

use std::ops::Range;
use std::time::Duration;

use ropey::RopeSlice;

use crate::geom::{Axis, Edges, Point, Rect};
use crate::gfx::{Font, TextSystem};
use crate::platform::TimerHandle;
use crate::scrollbar::ScrollbarState;
use crate::types::SharedStr;
use crate::ui::{Node, box_, custom, row};

use model::{EditEffects, EditPolicy, LineChange, TextModel, ValidateFn};
use view::{AutoScroll, BlinkState, FrameLayout, LongestLineCache};
use wrap::WrapIndex;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum MoveDirection {
    Up,
    Down,
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
    fn blink_timing(&self) -> (Option<Duration>, Option<Duration>);
}

pub(crate) trait Clipboard {
    fn read(&mut self) -> Option<String>;
    fn write(&mut self, text: &str) -> bool;
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

    fn blink_timing(&self) -> (Option<Duration>, Option<Duration>) {
        (None, None)
    }
}

#[cfg(test)]
#[derive(Default)]
pub(crate) struct MemoryClipboard(pub(crate) Option<String>);

#[cfg(test)]
impl Clipboard for MemoryClipboard {
    fn read(&mut self) -> Option<String> {
        self.0.clone()
    }

    fn write(&mut self, text: &str) -> bool {
        self.0 = Some(text.to_string());
        true
    }
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

struct DocumentView {
    wrap: WrapIndex,
    soft_wrap: bool,
    scroll_y: f64,
    content_height_f64: f64,
    last_line_height: f32,
    paddings: Edges<f32>,
    scrollbar: ScrollbarState,
    auto_scroll: AutoScroll,
    longest_line_cache: Option<(LongestLineCache, f32)>,
    pending_context_menu: Option<(Point, usize)>,
}

impl DocumentView {
    fn new() -> Self {
        Self {
            wrap: WrapIndex::new(Font::default(), 14.0, None),
            soft_wrap: true,
            scroll_y: 0.0,
            content_height_f64: 0.0,
            last_line_height: 20.0,
            paddings: Edges::ZERO,
            scrollbar: ScrollbarState::new(),
            auto_scroll: AutoScroll::default(),
            longest_line_cache: None,
            pending_context_menu: None,
        }
    }
}

enum ViewKind {
    Document(Box<DocumentView>),
    Field,
}

pub(crate) struct Editor {
    model: TextModel,
    view: ViewKind,
    blink: BlinkState,
    word_anchor: Option<Range<usize>>,
    selecting: bool,
    scroll_x: f32,
    reveal: Option<RevealRequest>,
    content_width: f32,
    input_bounds: Rect,
    frame: Option<FrameLayout>,
    caret_height: f32,
    caret_hidden: bool,
    placeholder: SharedStr,
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
            model: TextModel::new(EditPolicy {
                multi_line,
                validate: None,
            }),
            view: if multi_line {
                ViewKind::Document(Box::new(DocumentView::new()))
            } else {
                ViewKind::Field
            },
            blink: BlinkState::new(),
            word_anchor: None,
            selecting: false,
            scroll_x: 0.0,
            reveal: None,
            content_width: 0.0,
            input_bounds: Rect::ZERO,
            frame: None,
            caret_height: 0.0,
            caret_hidden: false,
            placeholder: SharedStr::default(),
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
        self.model = self.model.with_validate(f);
        self
    }

    pub(crate) fn with_soft_wrap(mut self, wrap: bool) -> Self {
        if let ViewKind::Document(doc) = &mut self.view {
            doc.soft_wrap = wrap;
        }
        self
    }

    pub(crate) fn with_rope(mut self, rope: Rope) -> Self {
        self.model = self.model.with_rope(rope);
        self
    }

    pub(crate) fn with_paddings(mut self, paddings: Edges<f32>) -> Self {
        if let ViewKind::Document(doc) = &mut self.view {
            doc.paddings = paddings;
        }
        self
    }

    #[inline]
    pub(crate) fn text(&self) -> &Rope {
        self.model.text()
    }

    #[inline]
    pub(crate) fn revision(&self) -> u64 {
        self.model.revision()
    }

    pub(crate) fn value(&self) -> SharedStr {
        self.model.value().into()
    }

    #[inline]
    pub(crate) fn caret_bounds(&self) -> Option<Rect> {
        self.frame.as_ref().map(|f| f.caret_rect)
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
        match &self.view {
            ViewKind::Document(doc) => doc.soft_wrap,
            ViewKind::Field => false,
        }
    }

    #[inline]
    pub(crate) fn scroll_offset(&self) -> Point {
        let y = match &self.view {
            ViewKind::Document(doc) => doc.scroll_y as f32,
            ViewKind::Field => 0.0,
        };
        crate::geom::point(self.scroll_x, y)
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
        if let ViewKind::Document(doc) = &mut self.view {
            doc.auto_scroll.stop();
        }
    }

    pub(super) fn apply_effects(&mut self, fx: EditEffects, ts: &TextSystem) {
        if fx.changed {
            self.reset_pointer_selection_state();
            if fx.lines != LineChange::None
                && let ViewKind::Document(doc) = &mut self.view
            {
                doc.wrap.apply_line_change(self.model.text(), &fx.lines, ts);
            }
        }
        if fx.caret_moved {
            let aff = self.model.selection().end_affinity;
            self.request_reveal(self.cursor(), aff, None, true);
            self.update_preferred_x();
        }
    }

    #[inline]
    pub(crate) fn is_selecting(&self) -> bool {
        self.selecting
    }

    #[inline]
    pub(crate) fn scrollbar_drag_axis(&self) -> Option<Axis> {
        match &self.view {
            ViewKind::Document(doc) => doc.scrollbar.drag_axis(),
            ViewKind::Field => None,
        }
    }

    #[inline]
    pub(crate) fn scrollbar_interactive(&self, axis: Axis) -> bool {
        match &self.view {
            ViewKind::Document(doc) => doc.scrollbar.is_interactive(axis),
            ViewKind::Field => false,
        }
    }

    pub(crate) fn set_soft_wrap(&mut self, wrap: bool, ts: &TextSystem) {
        let ViewKind::Document(doc) = &mut self.view else {
            return;
        };
        if doc.soft_wrap == wrap {
            return;
        }
        let anchor = doc.capture_scroll_anchor(doc.last_line_height);
        doc.soft_wrap = wrap;
        let text = self.model.text();
        if wrap {
            let width = self
                .frame
                .as_ref()
                .and_then(|f| f.wrap_width)
                .or_else(|| {
                    (self.input_bounds.size.width > view::RIGHT_MARGIN)
                        .then_some(self.input_bounds.size.width - view::RIGHT_MARGIN)
                });
            doc.wrap.set_wrap_width(text, width, ts);
            self.scroll_x = 0.0;
        } else {
            doc.wrap.set_wrap_width(text, None, ts);
        }
        if let Some(anchor) = anchor {
            doc.restore_scroll_anchor(text, anchor, doc.last_line_height);
        }
    }

    pub(crate) fn cursor(&self) -> usize {
        self.model.cursor()
    }

    pub(crate) fn caret_line_col(&self) -> (usize, usize) {
        self.model.caret_line_col()
    }

    pub(crate) fn selected_range(&self) -> Range<usize> {
        self.model.selected_range()
    }

    pub(crate) fn selected_text(&self) -> RopeSlice<'_> {
        self.model.selected_text()
    }

    pub(crate) fn can_undo(&self) -> bool {
        self.model.can_undo()
    }

    pub(crate) fn can_redo(&self) -> bool {
        self.model.can_redo()
    }

    pub(crate) fn begin_save(&mut self) -> u64 {
        self.model.begin_save()
    }

    pub(crate) fn mark_saved_version(&mut self, version: u64) {
        self.model.mark_saved_version(version);
    }

    pub(crate) fn is_clean(&self) -> bool {
        self.model.is_clean()
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
        self.model.commit_composition();
        self.model.break_coalescing();
        self.blink.stop();
        self.reset_pointer_selection_state();
    }

    pub(crate) fn view<H, C>(&self, hit: H, editor_draw: C, line_height: f32) -> Node<H, C> {
        self.view_with_scrollbars(hit, editor_draw, line_height, Vec::new())
    }

    pub(crate) fn view_with_scrollbars<H, C>(
        &self,
        hit: H,
        editor_draw: C,
        line_height: f32,
        scrollbars: Vec<Node<H, C>>,
    ) -> Node<H, C> {
        let cursor = crate::platform::CursorStyle::IBeam;
        match &self.view {
            ViewKind::Document(doc) => {
                let text_leaf = custom(editor_draw)
                    .w_full()
                    .h_full()
                    .flex_1()
                    .min_h(2.0 * line_height);
                box_()
                    .hit(hit)
                    .cursor(cursor)
                    .flex_1()
                    .overflow_x_hidden()
                    .h_full()
                    .child(box_().size_full().padding(doc.paddings).child(text_leaf))
                    .children(scrollbars)
            }
            ViewKind::Field => row()
                .hit(hit)
                .cursor(cursor)
                .flex_1()
                .overflow_x_hidden()
                .min_h_full()
                .items_center()
                .child(custom(editor_draw).w_full().h(line_height)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_direct_typing_undo_redo_and_word_navigation() {
        let ts = TextSystem::new();
        let mut timers = NoopTimers;
        let mut clip = MemoryClipboard::default();
        let mut ed = Editor::multi_line();

        ed.handle_text_input("hello", &ts, &mut timers);
        ed.handle_text_input(" world", &ts, &mut timers);
        assert_eq!(ed.text().to_string(), "hello world");
        assert!(!ed.is_clean());

        assert!(ed.handle_command(EditCommand::Undo, &mut clip, &ts, &mut timers));
        assert_eq!(ed.text().to_string(), "");
        assert!(ed.is_clean());

        assert!(ed.handle_command(EditCommand::Redo, &mut clip, &ts, &mut timers));
        assert_eq!(ed.text().to_string(), "hello world");

        assert!(ed.handle_command(
            EditCommand::Move {
                motion: Motion::PrevWord,
                extend: false,
            },
            &mut clip,
            &ts,
            &mut timers,
        ));
        assert_eq!(ed.cursor(), 6);
        assert!(ed.handle_command(
            EditCommand::Move {
                motion: Motion::NextWord,
                extend: true,
            },
            &mut clip,
            &ts,
            &mut timers,
        ));
        assert_eq!(ed.selected_text().to_string(), "world");

        assert!(ed.handle_command(EditCommand::Copy, &mut clip, &ts, &mut timers));
        assert_eq!(clip.0.as_deref(), Some("world"));
        assert!(ed.handle_command(EditCommand::Cut, &mut clip, &ts, &mut timers));
        assert_eq!(ed.text().to_string(), "hello ");
        assert!(ed.handle_command(EditCommand::Paste, &mut clip, &ts, &mut timers));
        assert_eq!(ed.text().to_string(), "hello world");
    }
}
