mod actions;
mod buffer;
mod history;
mod model;
mod scrollbar;
mod view;
pub(crate) mod wrap;

pub(crate) use buffer::RopeExt;
pub(crate) use model::TextModel;
pub(crate) use ropey::Rope;
pub(crate) use view::{EditorPaintParams, EditorPaintStyle};

use std::ops::Range;
use std::time::Duration;

use crate::geom::{Axis, Edges, Point, Rect, point};
use crate::gfx::{TextStyle, TextSystem};
use crate::platform::TimerHandle;
use crate::types::SharedStr;
use crate::ui::{Node, box_, custom, row};

use model::{EditPolicy, LineChange, ValidateFn};
use scrollbar::ScrollbarState;
use view::{AutoScroll, BlinkState, FrameLayout, LongestLineCache};
use wrap::WrapIndex;

#[derive(Clone, Copy, Debug)]
pub(crate) struct DocOptions {
    pub(crate) soft_wrap: bool,
    pub(crate) paddings: Edges<f32>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct FieldOptions {
    pub(crate) placeholder: SharedStr,
    pub(crate) caret_height: f32,
    pub(crate) validate: Option<ValidateFn>,
}

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

#[derive(Copy, Clone, Debug, Default)]
pub(super) struct Scroll {
    pub(super) x: f32,
    pub(super) y: f64,
    pub(super) content_w: f32,
    pub(super) content_h: f64,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum EditorTimer {
    Blink,
    AutoScroll,
    ScrollbarIdle,
    ScrollbarDrag,
}

pub(crate) trait EditorTimers {
    fn after(&mut self, delay: Duration, kind: EditorTimer) -> Option<TimerHandle>;
    fn every(
        &mut self,
        delay: Duration,
        interval: Duration,
        kind: EditorTimer,
    ) -> Option<TimerHandle>;
    fn blink_timing(&self) -> (Option<Duration>, Option<Duration>);
}

#[cfg(test)]
pub(crate) struct NoopTimers;

#[cfg(test)]
impl EditorTimers for NoopTimers {
    fn after(&mut self, _delay: Duration, _kind: EditorTimer) -> Option<TimerHandle> {
        None
    }

    fn every(
        &mut self,
        _delay: Duration,
        _interval: Duration,
        _kind: EditorTimer,
    ) -> Option<TimerHandle> {
        None
    }

    fn blink_timing(&self) -> (Option<Duration>, Option<Duration>) {
        (None, None)
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
    last_line_height: f32,
    paddings: Edges<f32>,
    scrollbar: ScrollbarState,
    auto_scroll: AutoScroll,
    longest_line_cache: Option<(LongestLineCache, f32)>,
    pending_context_menu: Option<(Point, usize)>,
}

impl DocumentView {
    fn new(opts: DocOptions) -> Self {
        Self {
            wrap: WrapIndex::new(TextStyle::default(), None),
            soft_wrap: opts.soft_wrap,
            last_line_height: 20.0,
            paddings: opts.paddings,
            scrollbar: ScrollbarState::new(),
            auto_scroll: AutoScroll::default(),
            longest_line_cache: None,
            pending_context_menu: None,
        }
    }
}

pub(crate) struct Editor {
    model: TextModel,
    doc: Option<Box<DocumentView>>,
    blink: BlinkState,
    goal: Option<(f32, usize)>,
    word_anchor: Option<Range<usize>>,
    selecting: bool,
    scroll: Scroll,
    reveal: Option<RevealRequest>,
    input_bounds: Rect,
    frame: Option<FrameLayout>,
    caret_height: f32,
    caret_hidden: bool,
    placeholder: SharedStr,
}

impl Editor {
    pub(crate) fn document(rope: Rope, opts: DocOptions) -> Self {
        Self {
            model: TextModel::new(EditPolicy {
                multi_line: true,
                validate: None,
            })
            .with_rope(rope),
            doc: Some(Box::new(DocumentView::new(opts))),
            blink: BlinkState::new(),
            goal: None,
            word_anchor: None,
            selecting: false,
            scroll: Scroll::default(),
            reveal: None,
            input_bounds: Rect::ZERO,
            frame: None,
            caret_height: 0.0,
            caret_hidden: false,
            placeholder: SharedStr::default(),
        }
    }

    pub(crate) fn field(opts: FieldOptions) -> Self {
        Self {
            model: TextModel::new(EditPolicy {
                multi_line: false,
                validate: opts.validate,
            }),
            doc: None,
            blink: BlinkState::new(),
            goal: None,
            word_anchor: None,
            selecting: false,
            scroll: Scroll::default(),
            reveal: None,
            input_bounds: Rect::ZERO,
            frame: None,
            caret_height: opts.caret_height,
            caret_hidden: false,
            placeholder: opts.placeholder,
        }
    }

    #[inline]
    pub(crate) fn model(&self) -> &TextModel {
        &self.model
    }

    #[inline]
    fn fresh_frame(&self) -> Option<&FrameLayout> {
        self.frame
            .as_ref()
            .filter(|f| f.revision == self.model.revision())
    }

    #[inline]
    pub(crate) fn caret_bounds(&self) -> Option<Rect> {
        self.fresh_frame().map(|f| f.caret_rect)
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
        self.doc.as_deref().is_some_and(|d| d.soft_wrap)
    }

    #[inline]
    pub(crate) fn scroll_offset(&self) -> Point {
        point(self.scroll.x, self.scroll.y as f32)
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
        if let Some(doc) = &mut self.doc {
            doc.auto_scroll.stop();
        }
    }

    pub(super) fn apply_change(&mut self, change: Option<LineChange>, ts: &TextSystem) {
        let Some(lines) = change else {
            return;
        };
        self.goal = None;
        self.reset_pointer_selection_state();
        if let Some(doc) = &mut self.doc {
            doc.wrap.apply_line_change(self.model.text(), &lines, ts);
        }
        let aff = self.model.selection().end_affinity;
        self.request_reveal(self.model.cursor(), aff, None, true);
    }

    #[inline]
    pub(crate) fn is_selecting(&self) -> bool {
        self.selecting
    }

    #[inline]
    pub(crate) fn scrollbar_drag_axis(&self) -> Option<Axis> {
        self.doc.as_deref().and_then(|d| d.scrollbar.drag_axis())
    }

    #[inline]
    pub(crate) fn scrollbar_interactive(&self, axis: Axis) -> bool {
        self.doc
            .as_deref()
            .is_some_and(|d| d.scrollbar.is_interactive(axis))
    }

    pub(crate) fn set_soft_wrap(&mut self, wrap: bool, ts: &TextSystem) {
        let Some(doc) = &mut self.doc else {
            return;
        };
        if doc.soft_wrap == wrap {
            return;
        }
        let anchor = doc.capture_scroll_anchor(self.scroll.y, doc.last_line_height);
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
            self.scroll.x = 0.0;
        } else {
            doc.wrap.set_wrap_width(text, None, ts);
        }
        if let Some(anchor) = anchor {
            self.scroll.y = doc.restore_scroll_anchor(text, anchor, doc.last_line_height);
        }
    }

    pub(crate) fn begin_save(&mut self) -> u64 {
        self.model.begin_save()
    }

    pub(crate) fn mark_saved_version(&mut self, version: u64) {
        self.model.mark_saved_version(version);
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
        if let Some(doc) = &self.doc {
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
        } else {
            row()
                .hit(hit)
                .cursor(cursor)
                .flex_1()
                .overflow_x_hidden()
                .min_h_full()
                .items_center()
                .child(custom(editor_draw).w_full().h(line_height))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_direct_typing_undo_redo_and_word_navigation() {
        let ts = TextSystem::new().unwrap();
        let mut timers = NoopTimers;
        let mut ed = Editor::document(
            Rope::new(),
            DocOptions {
                soft_wrap: true,
                paddings: Edges::ZERO,
            },
        );

        ed.handle_text_input("hello", &ts, &mut timers);
        ed.handle_text_input(" world", &ts, &mut timers);
        assert_eq!(ed.model().text().to_string(), "hello world");
        assert!(!ed.model().is_clean());

        assert!(ed.handle_command(EditCommand::Undo, &ts, &mut timers));
        assert_eq!(ed.model().text().to_string(), "");
        assert!(ed.model().is_clean());

        assert!(ed.handle_command(EditCommand::Redo, &ts, &mut timers));
        assert_eq!(ed.model().text().to_string(), "hello world");

        assert!(ed.handle_command(
            EditCommand::Move {
                motion: Motion::PrevWord,
                extend: false,
            },
            &ts,
            &mut timers,
        ));
        assert_eq!(ed.model().cursor(), 6);
        assert!(ed.handle_command(
            EditCommand::Move {
                motion: Motion::NextWord,
                extend: true,
            },
            &ts,
            &mut timers,
        ));
        assert_eq!(ed.model().selected_text().to_string(), "world");

        let copied = ed.selected_string();
        assert_eq!(copied.as_deref(), Some("world"));
        let cut = ed.prepare_cut();
        assert_eq!(cut.as_deref(), Some("world"));
        ed.delete_selection(&ts, &mut timers);
        assert_eq!(ed.model().text().to_string(), "hello ");
        ed.paste("world", &ts, &mut timers);
        assert_eq!(ed.model().text().to_string(), "hello world");
    }
}
