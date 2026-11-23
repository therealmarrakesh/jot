//! Clean-room owned plain-text editor (single-line and multi-line) with integrated soft-wrap,
//! transactional undo/redo, IME composition, caret blinking, drag auto-scroll, and scrollbar.

mod actions;
mod buffer;
mod history;
mod view;
mod wrap;

use std::ops::Range;
use std::sync::atomic::{AtomicUsize, Ordering};

use ropey::RopeSlice;

use crate::scrollbar::ScrollbarState;
use crate::tabs::TabId;
use crate::ui::{
    App, Bounds, Command, Div, Edges, FocusHandle, KeyBinding, MouseButton, MouseDownEvent,
    MouseUpEvent, Pixels, Point as UiPoint, ScrollWheelEvent, SharedString, Size, WeakFocusHandle,
    Window, WindowId, div, point, px, size,
};

pub(crate) use buffer::{Position, RopeExt};
pub(crate) use ropey::{Rope, RopeBuilder};
#[cfg(test)]
pub(crate) use wrap::{DisplayPos as WrapDisplayPoint, WrapIndex as TextWrapper};

use buffer::CaretSelection;
use history::History;
use view::{
    AutoScroll, BlinkState, EditorScrollbarWidget, EditorTextWidget, LongestLineCache,
    ScrollbarSnapshot, ViewLayout,
};
use wrap::WrapIndex;

const CONTEXT: &str = "Input";

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum MoveDirection {
    Up,
    Down,
}

/// Tab indentation style.
#[derive(Debug, Copy, Clone)]
pub(crate) struct TabSize {
    pub(crate) tab_size: usize,
    pub(crate) hard_tabs: bool,
}

impl Default for TabSize {
    fn default() -> Self {
        Self {
            tab_size: 2,
            hard_tabs: false,
        }
    }
}

impl TabSize {
    pub(super) fn as_str(self) -> SharedString {
        if self.hard_tabs {
            "\t".into()
        } else {
            " ".repeat(self.tab_size).into()
        }
    }
}

/// Identifies which owned `Editor` slot inside a `Jot` window a widget or timer addresses.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum EditorSlot {
    Tab(TabId),
    FindQuery,
    FindReplace,
    GoToLine,
}

/// Lightweight copyable handle routing widget, IME, and timer callbacks to an owned `Editor`.
#[derive(Copy, Clone)]
pub(crate) struct EditorHost {
    window_id: WindowId,
    slot: EditorSlot,
}

impl EditorHost {
    pub(crate) fn new(window_id: WindowId, slot: EditorSlot) -> Self {
        Self { window_id, slot }
    }

    pub(crate) fn with<R>(&self, cx: &App, f: impl FnOnce(&Editor) -> R) -> Option<R> {
        cx.jot(self.window_id)?.editor_by_slot(self.slot).map(f)
    }

    pub(crate) fn with_mut<R>(
        &self,
        cx: &mut App,
        f: impl FnOnce(&mut Editor, &mut App) -> R,
    ) -> Option<R> {
        let slot = self.slot;
        cx.with_jot(self.window_id, |jot, cx| {
            jot.editor_by_slot_mut(slot).map(|ed| f(ed, cx))
        })
        .flatten()
    }

    pub(crate) fn notify(&self, cx: &mut App) {
        cx.refresh_window(self.window_id);
    }

    pub(crate) fn open_context_menu(
        &self,
        pos: UiPoint<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) {
        cx.with_jot(self.window_id, |jot, cx| {
            jot.open_context_menu(pos, window, cx);
        });
    }
}

pub(crate) fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("backspace", Command::Backspace, Some(CONTEXT)),
        KeyBinding::new("shift-backspace", Command::Backspace, Some(CONTEXT)),
        KeyBinding::new("delete", Command::Delete, Some(CONTEXT)),
        KeyBinding::new("shift-delete", Command::Delete, Some(CONTEXT)),
        KeyBinding::new("ctrl-backspace", Command::DeletePrevWord, Some(CONTEXT)),
        KeyBinding::new("ctrl-delete", Command::DeleteNextWord, Some(CONTEXT)),
        KeyBinding::new("enter", Command::Enter, Some(CONTEXT)),
        KeyBinding::new("shift-enter", Command::Enter, Some(CONTEXT)),
        KeyBinding::new("secondary-enter", Command::Enter, Some(CONTEXT)),
        KeyBinding::new("escape", Command::Escape, Some(CONTEXT)),
        KeyBinding::new("up", Command::MoveUp, Some(CONTEXT)),
        KeyBinding::new("down", Command::MoveDown, Some(CONTEXT)),
        KeyBinding::new("left", Command::MoveLeft, Some(CONTEXT)),
        KeyBinding::new("right", Command::MoveRight, Some(CONTEXT)),
        KeyBinding::new("pageup", Command::MovePageUp, Some(CONTEXT)),
        KeyBinding::new("pagedown", Command::MovePageDown, Some(CONTEXT)),
        KeyBinding::new("tab", Command::IndentInline, Some(CONTEXT)),
        KeyBinding::new("shift-tab", Command::OutdentInline, Some(CONTEXT)),
        KeyBinding::new("ctrl-]", Command::Indent, Some(CONTEXT)),
        KeyBinding::new("ctrl-[", Command::Outdent, Some(CONTEXT)),
        KeyBinding::new("shift-left", Command::SelectLeft, Some(CONTEXT)),
        KeyBinding::new("shift-right", Command::SelectRight, Some(CONTEXT)),
        KeyBinding::new("shift-up", Command::SelectUp, Some(CONTEXT)),
        KeyBinding::new("shift-down", Command::SelectDown, Some(CONTEXT)),
        KeyBinding::new("shift-alt-left", Command::SelectLeft, Some(CONTEXT)),
        KeyBinding::new("shift-alt-right", Command::SelectRight, Some(CONTEXT)),
        KeyBinding::new("home", Command::MoveHome, Some(CONTEXT)),
        KeyBinding::new("end", Command::MoveEnd, Some(CONTEXT)),
        KeyBinding::new("ctrl-home", Command::MoveToStart, Some(CONTEXT)),
        KeyBinding::new("ctrl-end", Command::MoveToEnd, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-home", Command::SelectToStart, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-end", Command::SelectToEnd, Some(CONTEXT)),
        KeyBinding::new("shift-home", Command::SelectToStartOfLine, Some(CONTEXT)),
        KeyBinding::new("shift-end", Command::SelectToEndOfLine, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-left", Command::SelectPrevWord, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-right", Command::SelectNextWord, Some(CONTEXT)),
        KeyBinding::new("ctrl-a", Command::SelectAll, Some(CONTEXT)),
        KeyBinding::new("ctrl-c", Command::Copy, Some(CONTEXT)),
        KeyBinding::new("ctrl-x", Command::Cut, Some(CONTEXT)),
        KeyBinding::new("ctrl-v", Command::Paste, Some(CONTEXT)),
        KeyBinding::new("ctrl-left", Command::MovePrevWord, Some(CONTEXT)),
        KeyBinding::new("ctrl-right", Command::MoveNextWord, Some(CONTEXT)),
        KeyBinding::new("ctrl-z", Command::Undo, Some(CONTEXT)),
        KeyBinding::new("ctrl-y", Command::Redo, Some(CONTEXT)),
    ]);
}

type ValidateFn = Box<dyn Fn(&str) -> bool + 'static>;

fn next_editor_id() -> usize {
    static NEXT: AtomicUsize = AtomicUsize::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Owned plain-text editor state supporting both single-line fields and multi-line documents.
pub(crate) struct Editor {
    id: usize,
    multi_line: bool,
    focus_handle: FocusHandle,
    selection_focus: Option<WeakFocusHandle>,
    text: Rope,
    wrap: WrapIndex,
    tab_size: TabSize,
    history: History,
    revision: u64,
    soft_wrap: bool,
    blink: BlinkState,
    auto_scroll: AutoScroll,
    scrollbar: ScrollbarState,
    selection: CaretSelection,
    word_anchor: Option<CaretSelection>,
    ime_marked: Option<CaretSelection>,
    layout: Option<ViewLayout>,
    caret_bounds: Option<Bounds<Pixels>>,
    caret_height: Pixels,
    caret_hidden: bool,
    last_cursor: Option<usize>,
    input_bounds: Bounds<Pixels>,
    content_bounds: Option<Bounds<Pixels>>,
    last_selection: Option<CaretSelection>,
    selecting: bool,
    line_end_affinity: bool,
    validate: Option<ValidateFn>,
    scroll_offset: UiPoint<Pixels>,
    pending_scroll: Option<UiPoint<Pixels>>,
    content_size: Size<Pixels>,
    scrollbar_snapshot: Option<ScrollbarSnapshot>,
    viewport_wheel_step: bool,
    longest_line_cache: Option<(LongestLineCache, Pixels)>,
    paddings: Edges<Pixels>,
    placeholder: SharedString,
    pending_context_menu: Option<(UiPoint<Pixels>, usize)>,
}

impl Editor {
    pub(crate) fn single_line(window: &Window, cx: &App) -> Self {
        Self::new_inner(false, window, cx)
    }

    pub(crate) fn multi_line(window: &Window, cx: &App) -> Self {
        Self::new_inner(true, window, cx)
    }

    fn new_inner(multi_line: bool, window: &Window, cx: &App) -> Self {
        let style = window.text_style();
        Self {
            id: next_editor_id(),
            multi_line,
            focus_handle: cx.focus_handle(),
            selection_focus: None,
            text: Rope::new(),
            wrap: WrapIndex::new(style.font(), style.font_size, None),
            tab_size: TabSize::default(),
            history: History::new(),
            revision: 0,
            soft_wrap: true,
            blink: BlinkState::new(),
            auto_scroll: AutoScroll::default(),
            scrollbar: ScrollbarState::new(),
            selection: CaretSelection::default(),
            word_anchor: None,
            ime_marked: None,
            layout: None,
            caret_bounds: None,
            caret_height: px(0.),
            caret_hidden: false,
            last_cursor: None,
            input_bounds: Bounds::default(),
            content_bounds: None,
            last_selection: None,
            selecting: false,
            line_end_affinity: false,
            validate: None,
            scroll_offset: point(px(0.), px(0.)),
            pending_scroll: None,
            content_size: size(px(0.), px(0.)),
            scrollbar_snapshot: None,
            viewport_wheel_step: false,
            longest_line_cache: None,
            paddings: Edges::default(),
            placeholder: SharedString::default(),
            pending_context_menu: None,
        }
    }

    // ---- Builder configuration ----

    pub(crate) fn with_caret_height(mut self, height: Pixels) -> Self {
        self.caret_height = height;
        self
    }

    pub(crate) fn with_placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    pub(crate) fn with_validate(mut self, f: impl Fn(&str) -> bool + 'static) -> Self {
        self.validate = Some(Box::new(f));
        self
    }

    pub(crate) fn with_soft_wrap(mut self, wrap: bool) -> Self {
        self.soft_wrap = wrap;
        self
    }

    pub(crate) fn with_viewport_wheel_step(mut self) -> Self {
        self.viewport_wheel_step = true;
        self
    }

    pub(crate) fn with_rope(mut self, rope: Rope) -> Self {
        self.text = rope;
        self
    }

    pub(crate) fn with_paddings(mut self, paddings: Edges<Pixels>) -> Self {
        self.paddings = paddings;
        self
    }

    // ---- Accessors & configuration setters ----

    #[inline]
    pub(crate) fn id(&self) -> usize {
        self.id
    }

    #[inline]
    pub(crate) fn focus_handle(&self) -> &FocusHandle {
        &self.focus_handle
    }

    #[inline]
    pub(crate) fn text(&self) -> &Rope {
        &self.text
    }

    pub(crate) fn value(&self) -> SharedString {
        SharedString::new(self.text.to_string())
    }

    #[inline]
    pub(crate) fn caret_bounds(&self) -> Option<Bounds<Pixels>> {
        self.caret_bounds
    }

    pub(crate) fn set_caret_height(&mut self, height: Pixels) {
        self.caret_height = height;
    }

    pub(crate) fn set_caret_hidden(&mut self, hidden: bool) {
        self.caret_hidden = hidden;
    }

    #[cfg(test)]
    pub(crate) fn soft_wrap_enabled(&self) -> bool {
        self.soft_wrap
    }

    #[cfg(test)]
    pub(crate) fn scroll_offset(&self) -> UiPoint<Pixels> {
        self.scroll_offset
    }

    pub(crate) fn set_soft_wrap(&mut self, wrap: bool, cx: &mut App) {
        self.soft_wrap = wrap;
        if wrap {
            let width = self
                .layout
                .as_ref()
                .and_then(|l| l.wrap_width)
                .unwrap_or(self.input_bounds.size.width);
            self.wrap.set_wrap_width(Some(width), cx);
            self.scroll_offset.x = px(0.);
        } else {
            self.wrap.set_wrap_width(None, cx);
        }
    }

    pub(crate) fn set_selection_focus(&mut self, focus: Option<FocusHandle>) {
        self.selection_focus = focus.as_ref().map(FocusHandle::downgrade);
    }

    pub(crate) fn has_selection_focus(&self, window: &Window, cx: &App) -> bool {
        self.focus_handle.is_focused(window)
            || self
                .selection_focus
                .as_ref()
                .and_then(WeakFocusHandle::upgrade)
                .is_some_and(|f| f.is_focused(window) || f.contains_focused(window, cx))
    }

    pub(crate) fn cursor(&self) -> usize {
        if let Some(marked) = self.ime_marked {
            return marked.end;
        }
        self.selection.head()
    }

    pub(crate) fn selected_range(&self) -> Range<usize> {
        self.selection.into()
    }

    pub(crate) fn selected_text(&self) -> RopeSlice<'_> {
        let utf16 = self.range_to_utf16(&self.selected_range());
        let range = self.range_from_utf16(&utf16);
        self.text.slice(range)
    }

    #[inline]
    pub(crate) fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.text.offset_to_offset_utf16(range.start)..self.text.offset_to_offset_utf16(range.end)
    }

    #[inline]
    pub(super) fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.text.offset_utf16_to_offset(range.start)..self.text.offset_utf16_to_offset(range.end)
    }

    // ---- Save & history state ----

    pub(crate) fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    pub(crate) fn begin_save(&mut self) -> u64 {
        self.history.begin_save()
    }

    pub(crate) fn mark_saved_version(&mut self, version: u64) {
        self.history.mark_saved(version);
    }

    pub(crate) fn is_clean(&self) -> bool {
        self.history.is_clean()
    }

    // ---- Focus, activation, and caret blinking ----

    pub(crate) fn show_caret(&self, window: &Window) -> bool {
        self.focus_handle.is_focused(window)
            && self.blink.visible()
            && window.is_window_active()
    }

    pub(crate) fn start_blink(&mut self, host: &EditorHost, cx: &App) {
        self.blink.start(host, cx);
    }

    pub(crate) fn pause_blink(&mut self, host: &EditorHost, cx: &App) {
        self.blink.pause(host, cx);
    }

    pub(crate) fn stop_blink(&mut self) {
        self.blink.stop();
    }

    pub(crate) fn on_blur(&mut self) {
        self.history.break_coalescing();
        self.blink.stop();
    }

    // ---- Rendering ----

    pub(crate) fn render(&self, host: EditorHost) -> Div {
        div()
            .id(("editor", self.id))
            .key_context(CONTEXT)
            .track_focus(&self.focus_handle)
            .on_command(move |command, _, cx| {
                let handled = host
                    .with_mut(cx, |ed, cx| ed.handle_command(command, &host, cx))
                    .unwrap_or(false);
                if handled {
                    host.notify(cx);
                }
                handled
            })
            .on_mouse_down(
                MouseButton::Left,
                move |event: &MouseDownEvent, window, cx| {
                    host.with_mut(cx, |ed, cx| ed.on_mouse_down(event, &host, window, cx));
                    host.notify(cx);
                },
            )
            .on_mouse_down(
                MouseButton::Right,
                move |event: &MouseDownEvent, window, cx| {
                    host.with_mut(cx, |ed, cx| ed.on_mouse_down(event, &host, window, cx));
                    host.notify(cx);
                },
            )
            .on_mouse_up(
                MouseButton::Left,
                move |event: &MouseUpEvent, window, cx| {
                    host.with_mut(cx, |ed, cx| ed.on_mouse_up(event, &host, window, cx));
                    host.notify(cx);
                },
            )
            .on_mouse_up(
                MouseButton::Right,
                move |event: &MouseUpEvent, window, cx| {
                    host.with_mut(cx, |ed, cx| ed.on_mouse_up(event, &host, window, cx));
                    host.notify(cx);
                },
            )
            .on_scroll_wheel(move |event: &ScrollWheelEvent, window, cx| {
                let moved = host
                    .with_mut(cx, |ed, cx| ed.on_scroll_wheel(event, window, cx))
                    .unwrap_or(false);
                if moved {
                    host.notify(cx);
                }
            })
            .cursor_text()
            .flex_1()
            .map(|this| {
                if self.multi_line {
                    this.h_full()
                } else {
                    this.min_h_full().flex().items_center()
                }
            })
            .flex_grow_1()
            .overflow_x_hidden()
            .when(self.multi_line, |this| {
                this.pt(self.paddings.top)
                    .pr(self.paddings.right)
                    .pb(self.paddings.bottom)
                    .pl(self.paddings.left)
            })
            .child(EditorTextWidget::new(host, self.placeholder.clone()))
            .when(self.multi_line, |this| {
                this.child(EditorScrollbarWidget::new(host))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::TabSize;

    #[test]
    fn tab_size_formats_spaces_or_hard_tab() {
        assert_eq!(
            TabSize {
                tab_size: 2,
                hard_tabs: false
            }
            .as_str(),
            "  "
        );
        assert_eq!(
            TabSize {
                tab_size: 4,
                hard_tabs: false
            }
            .as_str(),
            "    "
        );
        assert_eq!(
            TabSize {
                tab_size: 4,
                hard_tabs: true
            }
            .as_str(),
            "\t"
        );
    }
}
