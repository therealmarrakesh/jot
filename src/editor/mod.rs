mod actions;
mod buffer;
mod frame;
mod history;
mod layout;
mod model;
mod scroll;
mod scrollbar;
pub(crate) mod wrap;

use std::ops::Range;
use std::time::{Duration, Instant};

use frame::FrameLayout;
use layout::LongestLineCache;
use model::{EditPolicy, LineChange, ValidateFn};
use scrollbar::ScrollbarState;
use wrap::WrapIndex;

use crate::geom::{Axis, Edges, Point, Rect, point};
use crate::gfx::{TextStyle, TextSystem};
use crate::platform::{Modifiers, MouseButton, TimerHandle};
use crate::types::SharedStr;
use crate::ui::{Node, box_, custom, row};

pub(crate) use buffer::RopeExt;
pub(crate) use frame::{EditorPaintParams, EditorPaintStyle};
pub(crate) use model::TextModel;
pub(crate) use ropey::Rope;

const RIGHT_MARGIN: f32 = 10.0;

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
enum MoveDirection {
    Up,
    Down,
}

#[derive(Clone, Copy, Debug)]
struct RevealRequest {
    offset: usize,
    end_affinity: bool,
    direction: Option<MoveDirection>,
    with_margin: bool,
}

#[derive(Copy, Clone, Debug, Default)]
struct Scroll {
    x: f32,
    y: f64,
    content_w: f32,
    content_h: f64,
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
    fn request_reveal(
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
    fn reset_pointer_selection_state(&mut self) {
        self.word_anchor = None;
        self.selecting = false;
        if let Some(doc) = &mut self.doc {
            doc.auto_scroll.stop();
        }
    }

    fn apply_change(&mut self, change: Option<LineChange>, ts: &TextSystem) {
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
                    (self.input_bounds.size.width > RIGHT_MARGIN)
                        .then_some(self.input_bounds.size.width - RIGHT_MARGIN)
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

pub(crate) struct BlinkState {
    active: bool,
    visible: bool,
    paused: bool,
    started_at: Option<Instant>,
    timer: Option<TimerHandle>,
}

impl Default for BlinkState {
    fn default() -> Self {
        Self::new()
    }
}

impl BlinkState {
    pub(crate) fn new() -> Self {
        Self {
            active: false,
            visible: false,
            paused: false,
            started_at: None,
            timer: None,
        }
    }

    #[inline]
    pub(crate) fn visible(&self) -> bool {
        self.paused || self.visible
    }

    pub(crate) fn stop(&mut self) {
        self.active = false;
        self.paused = false;
        self.visible = false;
        self.started_at = None;
        self.timer = None;
    }

    pub(crate) fn start(&mut self, timers: &mut dyn EditorTimers) {
        self.active = true;
        self.paused = false;
        self.visible = false;
        self.started_at = Some(Instant::now());
        self.step(timers);
    }

    pub(crate) fn pause(&mut self, timers: &mut dyn EditorTimers) {
        if !self.active && !self.visible {
            return;
        }
        self.paused = true;
        self.visible = true;
        self.started_at = Some(Instant::now());
        let Some(delay) = timers.blink_timing().0 else {
            self.timer = None;
            return;
        };
        self.timer = timers.after(delay, EditorTimer::Blink);
    }

    pub(crate) fn on_timer(&mut self, timers: &mut dyn EditorTimers) -> bool {
        if !self.active {
            return false;
        }
        self.paused = false;
        self.step(timers);
        true
    }

    fn step(&mut self, timers: &mut dyn EditorTimers) {
        if self.paused {
            self.visible = true;
            return;
        }
        let (interval, timeout) = timers.blink_timing();
        let Some(interval) = interval else {
            self.visible = true;
            self.timer = None;
            return;
        };
        if let Some(timeout) = timeout
            && self
                .started_at
                .is_some_and(|start| start.elapsed() >= timeout)
        {
            self.visible = true;
            self.timer = None;
            return;
        }
        self.visible = !self.visible;
        self.timer = timers.after(interval, EditorTimer::Blink);
    }
}

#[derive(Default)]
pub(crate) struct AutoScroll {
    delta: Option<f32>,
    timer: Option<TimerHandle>,
    pub(crate) last_drag_pos: Option<Point>,
}

impl AutoScroll {
    pub(crate) fn compute_delta(y: f32, bounds: Rect) -> Option<f32> {
        const MIN_SPEED: f32 = 12.0;
        const MAX_SPEED: f32 = 64.0;
        const INNER_ZONE: f32 = 16.0;
        const OUTER_RAMP: f32 = 80.0;

        let bottom = bounds.bottom() - INNER_ZONE;
        let top = bounds.top() + INNER_ZONE;

        if y > bottom {
            let t = ((y - bottom) / (INNER_ZONE + OUTER_RAMP)).min(1.0);
            Some(MIN_SPEED + t * (MAX_SPEED - MIN_SPEED))
        } else if y < top {
            let t = ((top - y) / (INNER_ZONE + OUTER_RAMP)).min(1.0);
            Some(-(MIN_SPEED + t * (MAX_SPEED - MIN_SPEED)))
        } else {
            None
        }
    }

    pub(crate) fn stop(&mut self) {
        self.delta = None;
        self.timer = None;
        self.last_drag_pos = None;
    }

    pub(crate) fn update(&mut self, delta: Option<f32>, timers: &mut dyn EditorTimers) {
        let was_active = self.delta.is_some();
        self.delta = delta;
        if delta.is_none() {
            self.timer = None;
            return;
        }
        if !was_active {
            let interval = Duration::from_millis(16);
            self.timer = timers.every(interval, interval, EditorTimer::AutoScroll);
        }
    }
}

impl Editor {
    fn hit_test_point(&self, pos: Point) -> (usize, bool) {
        let Some(frame) = self.fresh_frame() else {
            return (0, false);
        };
        frame.hit_test(
            pos,
            self.scroll.x,
            self.scroll.y,
            self.model.text().len(),
            self.model.is_multi_line(),
        )
    }

    pub(crate) fn on_mouse_down(
        &mut self,
        button: MouseButton,
        position: Point,
        modifiers: Modifiers,
        click_count: usize,
        timers: &mut dyn EditorTimers,
    ) {
        self.model.commit_composition();
        self.model.break_coalescing();
        self.selecting = true;
        let (offset, aff) = self.hit_test_point(position);

        if button == MouseButton::Left && click_count >= 3 {
            self.select_line_at(offset);
            return;
        }
        if button == MouseButton::Left && click_count == 2 {
            self.select_word_at(offset);
            return;
        }
        if button == MouseButton::Right {
            if !self.model.selection().contains(offset) {
                self.move_to_with_affinity(offset, None, aff, false, Some(timers));
            }
            if let Some(doc) = &mut self.doc {
                doc.pending_context_menu = Some((position, offset));
            }
            return;
        }
        if modifiers.shift {
            self.select_to_with_affinity(offset, aff);
            self.request_reveal(self.model.cursor(), aff, None, false);
        } else {
            self.move_to_with_affinity(offset, None, aff, false, Some(timers));
        }
    }

    pub(crate) fn on_mouse_move(
        &mut self,
        position: Point,
        dragging_left: bool,
        focused: bool,
        timers: &mut dyn EditorTimers,
    ) -> bool {
        if dragging_left
            && self.selecting
            && focused
            && self.model.text().len() > 0
            && self.fresh_frame().is_some()
        {
            let (offset, aff) = self.hit_test_point(position);
            self.select_to_with_affinity(offset, aff);
            if let Some(doc) = &mut self.doc {
                doc.auto_scroll.last_drag_pos = Some(position);
                let delta = AutoScroll::compute_delta(position.y, self.input_bounds);
                doc.auto_scroll.update(delta.map(|d| -d), timers);
            }
            self.request_reveal(self.model.cursor(), aff, None, false);
            return true;
        }
        false
    }

    pub(crate) fn on_mouse_up(
        &mut self,
        button: MouseButton,
        timers: &mut dyn EditorTimers,
    ) -> Option<Point> {
        let mut open_context_menu = None;
        if button == MouseButton::Right
            && let Some(doc) = &mut self.doc
            && let Some((pos, offset)) = doc.pending_context_menu.take()
        {
            if !self.model.selection().contains(offset) {
                self.move_to_with_affinity(offset, None, false, false, Some(timers));
            }
            open_context_menu = Some(pos);
        }
        self.reset_pointer_selection_state();
        open_context_menu
    }
}

#[cfg(test)]
mod tests {
    use crate::geom::rect;

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
}
