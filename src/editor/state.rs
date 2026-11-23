use crate::ui::{
    App, AppContext, Bounds, Context, Edges, Entity, EntityInputHandler,
    FocusHandle, Focusable, InteractiveElement as _, IntoElement, KeyBinding, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, Point, Render,
    ScrollDelta, ScrollHandle, ScrollWheelEvent, SharedString, Styled as _, Subscription,
    FluentBuilder as _, UTF16Selection, WeakFocusHandle, Window, actions, div, point, px,
};
use ropey::{Rope, RopeSlice};
use std::borrow::Cow;
use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;
use unicode_segmentation::UnicodeSegmentation;

use super::rope_ext::Bias;
use super::{
    TabSize,
    blink_cursor::BlinkCursor,
    change::Change,
    cursor::CursorSelection,
    element::{
        EditorScrollbar, EditorScrollbarSnapshot, LongestLineKey, RIGHT_MARGIN, TextElement,
        clamp_horizontal_scroll_offset,
    },
    layout::LastLayout,
    movement::MoveDirection,
    text_wrapper::TextWrapper,
    undo_manager::{EditIntent, UndoManager},
};
use super::auto_scroll::AutoScroll;
use crate::ui::{SelectDown, SelectLeft, SelectRight, SelectUp};
use super::{Position, RopeExt as _};

const DEFAULT_WHEEL_SCROLL: u32 = 3;
const WHEEL_STEP_FRACTION: f32 = 0.15;
const MIN_WHEEL_STEP: f32 = 48.;
const WORD_SCAN_WINDOW_BYTES: usize = 256;

fn wheel_scroll_amount(horizontal: bool) -> u32 {
    crate::win32::wheel_scroll_amount(horizontal).unwrap_or(DEFAULT_WHEEL_SCROLL)
}

actions!(
    input,
    [
        Enter,
        Backspace,
        Delete,
        DeleteToPreviousWordStart,
        DeleteToNextWordEnd,
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
        SelectAll,
        SelectToStartOfLine,
        SelectToEndOfLine,
        SelectToStart,
        SelectToEnd,
        SelectToPreviousWordStart,
        SelectToNextWordEnd,
        Copy,
        Cut,
        Paste,
        Undo,
        Redo,
        MoveToStart,
        MoveToEnd,
        MoveToPreviousWord,
        MoveToNextWord,
        Escape,
    ]
);

pub(super) const CONTEXT: &str = "Input";

pub(crate) fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, Some(CONTEXT)),
        KeyBinding::new("shift-backspace", Backspace, Some(CONTEXT)),
        KeyBinding::new("delete", Delete, Some(CONTEXT)),
        KeyBinding::new("shift-delete", Delete, Some(CONTEXT)),
        KeyBinding::new("ctrl-backspace", DeleteToPreviousWordStart, Some(CONTEXT)),
        KeyBinding::new("ctrl-delete", DeleteToNextWordEnd, Some(CONTEXT)),
        KeyBinding::new("enter", Enter, Some(CONTEXT)),
        KeyBinding::new("shift-enter", Enter, Some(CONTEXT)),
        KeyBinding::new("secondary-enter", Enter, Some(CONTEXT)),
        KeyBinding::new("escape", Escape, Some(CONTEXT)),
        KeyBinding::new("up", MoveUp, Some(CONTEXT)),
        KeyBinding::new("down", MoveDown, Some(CONTEXT)),
        KeyBinding::new("left", MoveLeft, Some(CONTEXT)),
        KeyBinding::new("right", MoveRight, Some(CONTEXT)),
        KeyBinding::new("pageup", MovePageUp, Some(CONTEXT)),
        KeyBinding::new("pagedown", MovePageDown, Some(CONTEXT)),
        KeyBinding::new("tab", IndentInline, Some(CONTEXT)),
        KeyBinding::new("shift-tab", OutdentInline, Some(CONTEXT)),
        KeyBinding::new("ctrl-]", Indent, Some(CONTEXT)),
        KeyBinding::new("ctrl-[", Outdent, Some(CONTEXT)),
        KeyBinding::new("shift-left", SelectLeft, Some(CONTEXT)),
        KeyBinding::new("shift-right", SelectRight, Some(CONTEXT)),
        KeyBinding::new("shift-up", SelectUp, Some(CONTEXT)),
        KeyBinding::new("shift-down", SelectDown, Some(CONTEXT)),
        KeyBinding::new("shift-alt-left", SelectLeft, Some(CONTEXT)),
        KeyBinding::new("shift-alt-right", SelectRight, Some(CONTEXT)),
        KeyBinding::new("home", MoveHome, Some(CONTEXT)),
        KeyBinding::new("end", MoveEnd, Some(CONTEXT)),
        KeyBinding::new("ctrl-home", MoveToStart, Some(CONTEXT)),
        KeyBinding::new("ctrl-end", MoveToEnd, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-home", SelectToStart, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-end", SelectToEnd, Some(CONTEXT)),
        KeyBinding::new("shift-home", SelectToStartOfLine, Some(CONTEXT)),
        KeyBinding::new("shift-end", SelectToEndOfLine, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-left", SelectToPreviousWordStart, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-right", SelectToNextWordEnd, Some(CONTEXT)),
        KeyBinding::new("ctrl-a", SelectAll, Some(CONTEXT)),
        KeyBinding::new("ctrl-c", Copy, Some(CONTEXT)),
        KeyBinding::new("ctrl-x", Cut, Some(CONTEXT)),
        KeyBinding::new("ctrl-v", Paste, Some(CONTEXT)),
        KeyBinding::new("ctrl-left", MoveToPreviousWord, Some(CONTEXT)),
        KeyBinding::new("ctrl-right", MoveToNextWord, Some(CONTEXT)),
        KeyBinding::new("ctrl-z", Undo, Some(CONTEXT)),
        KeyBinding::new("ctrl-y", Redo, Some(CONTEXT)),
    ]);
}

/// Unified single-caret text editor state for both single-line inputs and multi-line documents.
type ValidateFn = Box<dyn Fn(&str, &mut App) -> bool + 'static>;
type ContextMenuHandler = Rc<dyn Fn(Point<Pixels>, &mut Window, &mut App)>;

pub(crate) struct EditorState {
    multi_line: bool,
    pub(super) focus_handle: FocusHandle,
    selection_focus: Option<WeakFocusHandle>,
    pub(super) text: Rope,
    pub(super) text_wrapper: TextWrapper,
    pub(super) tab_size: TabSize,
    pub(super) undo_manager: UndoManager,
    pub(super) document_revision: u64,
    pub(super) soft_wrap: bool,
    pub(super) blink_cursor: Entity<BlinkCursor>,
    pub(super) selection: CursorSelection,
    pub(super) selected_word_range: Option<CursorSelection>,
    pub(super) ime_marked_range: Option<CursorSelection>,
    pub(super) last_layout: Option<LastLayout>,
    pub(super) caret_bounds: Option<Bounds<Pixels>>,
    pub(crate) caret_height: Cell<Pixels>,
    pub(crate) caret_hidden: Cell<bool>,
    pub(super) last_cursor: Option<usize>,
    pub(super) input_bounds: Bounds<Pixels>,
    pub(super) last_bounds: Option<Bounds<Pixels>>,
    pub(super) last_selected_range: Option<CursorSelection>,
    pub(super) selecting: bool,
    pub(crate) cursor_line_end_affinity: bool,
    pub(super) validate: Option<ValidateFn>,
    pub(crate) scroll_handle: ScrollHandle,
    pub(crate) deferred_scroll_offset: Option<Point<Pixels>>,
    pub(crate) scroll_size: crate::ui::Size<Pixels>,
    pub(super) editor_scrollbar_snapshot: Cell<Option<EditorScrollbarSnapshot>>,
    viewport_wheel_step: bool,
    pub(super) longest_line_width: Cell<Option<(LongestLineKey, Pixels)>>,
    pub(super) editor_paddings: Edges<Pixels>,
    pub(super) placeholder: SharedString,
    context_menu_handler: Option<ContextMenuHandler>,
    pending_context_menu: Option<(Point<Pixels>, usize)>,
    _subscriptions: Vec<Subscription>,
    _window_subscriptions: Vec<Subscription>,
    pub(super) auto_scroll: AutoScroll,
}

impl EditorState {
    pub(crate) fn single_line(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::new_inner(false, window, cx)
    }

    pub(crate) fn multi_line(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::new_inner(true, window, cx)
    }

    fn new_inner(multi_line: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let blink_cursor = cx.new(|_| BlinkCursor::new());
        let undo_manager = UndoManager::new();

        let _subscriptions = vec![
            cx.intercept_keystrokes({
                let focus_handle = focus_handle.clone();
                let blink_cursor = blink_cursor.downgrade();
                move |_, window, cx| {
                    if focus_handle.is_focused(window) {
                        _ = blink_cursor.update(cx, |cursor, cx| cursor.pause(cx));
                    }
                }
            }),
            cx.observe(&blink_cursor, |_, _, cx| cx.notify()),
        ];
        let _window_subscriptions = Self::window_subscriptions(&focus_handle, window, cx);
        let text_style = window.text_style();

        Self {
            multi_line,
            focus_handle,
            selection_focus: None,
            text: Rope::new(),
            text_wrapper: TextWrapper::new(text_style.font(), text_style.font_size, None),
            tab_size: TabSize::default(),
            undo_manager,
            document_revision: 0,
            soft_wrap: true,
            blink_cursor,
            selection: CursorSelection::default(),
            selected_word_range: None,
            ime_marked_range: None,
            last_layout: None,
            caret_bounds: None,
            caret_height: Cell::new(px(0.)),
            caret_hidden: Cell::new(false),
            last_cursor: None,
            input_bounds: Bounds::default(),
            last_bounds: None,
            last_selected_range: None,
            selecting: false,
            cursor_line_end_affinity: false,
            validate: None,
            scroll_handle: ScrollHandle::new(),
            deferred_scroll_offset: None,
            scroll_size: crate::ui::size(px(0.), px(0.)),
            editor_scrollbar_snapshot: Cell::new(None),
            viewport_wheel_step: false,
            longest_line_width: Cell::new(None),
            editor_paddings: Edges::default(),
            placeholder: SharedString::default(),
            context_menu_handler: None,
            pending_context_menu: None,
            _subscriptions,
            _window_subscriptions,
            auto_scroll: AutoScroll::default(),
        }
    }

    pub(crate) fn caret_bounds(&self) -> Option<Bounds<Pixels>> {
        self.caret_bounds
    }

    pub(crate) fn viewport_wheel_step(mut self) -> Self {
        self.viewport_wheel_step = true;
        self
    }

    #[inline]
    pub(crate) fn is_multi_line(&self) -> bool {
        self.multi_line
    }

    #[inline]
    pub(crate) fn is_single_line(&self) -> bool {
        !self.multi_line
    }

    pub(crate) fn is_copyable(&self) -> bool {
        !self.selection.is_empty()
    }

    fn window_subscriptions(
        focus_handle: &FocusHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<Subscription> {
        vec![
            cx.observe_window_activation(window, |input, window, cx| {
                if !window.is_window_active() {
                    input.blink_cursor.update(cx, |blink_cursor, cx| {
                        blink_cursor.stop(cx);
                    });
                } else if input.focus_handle.is_focused(window) {
                    input.blink_cursor.update(cx, |blink_cursor, cx| {
                        blink_cursor.start(cx);
                    });
                }
            }),
            cx.on_focus(focus_handle, window, Self::on_focus),
            cx.on_blur(focus_handle, window, Self::on_blur),
        ]
    }

    pub(crate) fn attach_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self._window_subscriptions = Self::window_subscriptions(&self.focus_handle, window, cx);
    }

    pub(crate) fn can_undo(&self) -> bool {
        self.undo_manager.has_undos()
    }

    pub(crate) fn begin_save(&mut self) -> u64 {
        self.undo_manager.begin_save()
    }

    pub(crate) fn mark_saved_version(&mut self, version: u64) {
        self.undo_manager.mark_saved_version(version);
    }

    pub(crate) fn is_clean(&self) -> bool {
        self.undo_manager.is_clean()
    }

    pub(crate) fn on_context_menu(&mut self, handler: ContextMenuHandler) {
        self.context_menu_handler = Some(handler);
    }

    pub(crate) fn set_selection_focus(&mut self, focus: Option<FocusHandle>, cx: &mut Context<Self>) {
        self.selection_focus = focus.as_ref().map(FocusHandle::downgrade);
        cx.notify();
    }

    pub(crate) fn has_selection_focus(&self, window: &Window, cx: &App) -> bool {
        self.focus_handle.is_focused(window)
            || self
                .selection_focus
                .as_ref()
                .and_then(WeakFocusHandle::upgrade)
                .is_some_and(|focus| focus.is_focused(window) || focus.contains_focused(window, cx))
    }

    pub(crate) fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    pub(crate) fn validate(mut self, f: impl Fn(&str, &mut App) -> bool + 'static) -> Self {
        self.validate = Some(Box::new(f));
        self
    }

    pub(crate) fn soft_wrap(mut self, wrap: bool) -> Self {
        self.soft_wrap = wrap;
        self
    }

    pub(crate) fn set_soft_wrap(&mut self, wrap: bool, _: &mut Window, cx: &mut Context<Self>) {
        self.soft_wrap = wrap;
        if wrap {
            let wrap_width = self
                .last_layout
                .as_ref()
                .and_then(|b| b.wrap_width)
                .unwrap_or(self.input_bounds.size.width);

            self.text_wrapper.set_wrap_width(Some(wrap_width), cx);

            let mut offset = self.scroll_handle.offset();
            offset.x = px(0.);
            self.scroll_handle.set_offset(offset);
        } else {
            self.text_wrapper.set_wrap_width(None, cx);
        }
        cx.notify();
    }

    pub(crate) fn set_editor_paddings(&mut self, paddings: Edges<Pixels>) {
        self.editor_paddings = paddings;
    }

    pub(crate) fn default_rope(mut self, value: Rope) -> Self {
        self.text = value;
        self
    }

    pub(crate) fn value(&self) -> SharedString {
        SharedString::new(self.text.to_string())
    }

    pub(crate) fn text(&self) -> &Rope {
        &self.text
    }

    pub(crate) fn set_cursor_position(
        &mut self,
        position: impl Into<Position>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let position: Position = position.into();
        let offset = self.text.position_to_offset(&position);

        self.move_to(offset, None, cx);
        self.update_preferred_column();
        self.focus(window, cx);
    }

    pub(crate) fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_handle.focus(window, cx);
        self.blink_cursor.update(cx, |cursor, cx| {
            cursor.start(cx);
        });
    }

    pub(super) fn line_and_position_for_offset(
        &self,
        offset: usize,
    ) -> (usize, usize, Option<Point<Pixels>>) {
        let Some(last_layout) = &self.last_layout else {
            return (0, 0, None);
        };
        let line_height = last_layout.line_height;

        let mut y_offset = last_layout.visible_top;
        for (vi, line) in last_layout.lines.iter().enumerate() {
            let prev_lines_offset = last_layout.visible_line_byte_offsets[vi];
            let local_offset = offset.saturating_sub(prev_lines_offset);
            if let Some(pos) = line.position_for_index(local_offset, last_layout, false) {
                let sub_line_index = (pos.y / line_height) as usize;
                return (vi, sub_line_index, Some(point(pos.x, pos.y + y_offset)));
            }
            y_offset += line.size(line_height).height;
        }
        (0, 0, None)
    }

    pub(crate) fn set_value(
        &mut self,
        value: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let value = value.into();
        self.undo_manager.set_ignoring(true);
        self.replace_text(value, window, cx);
        self.undo_manager.set_ignoring(false);

        self.reset_selection();
        self.reset_scroll_to_start();

        self.undo_manager.clear();
        cx.notify();
    }

    pub(crate) fn replace(
        &mut self,
        text: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text: SharedString = text.into();
        self.undo_manager.set_pending_intent(EditIntent::Atomic);
        self.replace_text_in_range(None, &text, window, cx);
        let end = self.selection.end;
        self.set_cursor_to(end);
    }

    pub(crate) fn replace_ranges(
        &mut self,
        edits: &[(Range<usize>, String)],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.undo_manager.set_pending_intent(EditIntent::Atomic);
        self.replace_text_in_ranges(edits, window, cx);
    }

    fn replace_text(
        &mut self,
        text: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text: SharedString = text.into();
        self.undo_manager.set_pending_intent(EditIntent::Atomic);
        let range = 0..self.text.chars().map(|c| c.len_utf16()).sum();
        self.replace_text_in_range(Some(range), &text, window, cx);
    }

    fn reset_selection(&mut self) {
        if self.is_single_line() {
            let end = self.text.len();
            self.set_cursor_to(end);
        } else {
            self.selection.clear();
        }
    }

    fn reset_scroll_to_start(&mut self) {
        self.scroll_handle.set_offset(point(px(0.), px(0.)));
        if self.is_single_line() {
            self.deferred_scroll_offset = Some(point(px(0.), px(0.)));
        }
    }

    pub(super) fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        let offset = self.previous_boundary(self.selection.cursor_offset());
        self.select_keyboard_to(offset, false, false, cx);
    }

    pub(super) fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        let offset = self.next_boundary(self.selection.cursor_offset());
        self.select_keyboard_to(offset, false, false, cx);
    }

    pub(super) fn select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_single_line() {
            return;
        }
        let (offset, affinity) = self.vertical_selection_target(
            self.selection.cursor_offset(),
            self.selection.column_anchor,
            self.cursor_line_end_affinity,
            -1,
        );
        self.select_keyboard_to(offset, affinity, true, cx);
    }

    pub(super) fn select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_single_line() {
            return;
        }
        let (offset, affinity) = self.vertical_selection_target(
            self.selection.cursor_offset(),
            self.selection.column_anchor,
            self.cursor_line_end_affinity,
            1,
        );
        self.select_keyboard_to(offset, affinity, true, cx);
    }

    pub(super) fn on_action_select_all(
        &mut self,
        _: &SelectAll,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_all(window, cx);
    }

    pub(super) fn select_to_start(
        &mut self,
        _: &SelectToStart,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_keyboard_to(0, false, false, cx);
    }

    pub(super) fn select_to_end(
        &mut self,
        _: &SelectToEnd,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let end = self.text.len();
        self.select_keyboard_to(end, false, false, cx);
    }

    pub(super) fn select_to_start_of_line(
        &mut self,
        _: &SelectToStartOfLine,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let offset = self.start_of_line_at(self.selection.cursor_offset());
        self.select_keyboard_to(offset, false, false, cx);
    }

    pub(super) fn select_to_end_of_line(
        &mut self,
        _: &SelectToEndOfLine,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let offset = self.end_of_line_at(self.selection.cursor_offset());
        self.select_keyboard_to(offset, true, false, cx);
    }

    pub(super) fn select_to_previous_word(
        &mut self,
        _: &SelectToPreviousWordStart,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let offset = self.previous_start_of_word_at(self.selection.cursor_offset());
        self.select_keyboard_to(offset, false, false, cx);
    }

    pub(super) fn select_to_next_word(
        &mut self,
        _: &SelectToNextWordEnd,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let offset = self.next_end_of_word_at(self.selection.cursor_offset());
        self.select_keyboard_to(offset, false, false, cx);
    }

    pub(super) fn previous_start_of_word_at(&self, offset: usize) -> usize {
        let offset = self.offset_from_utf16(self.offset_to_utf16(offset));
        previous_word_start(&self.text, offset)
    }

    pub(super) fn next_end_of_word_at(&self, offset: usize) -> usize {
        let offset = self.offset_from_utf16(self.offset_to_utf16(offset));
        next_word_end(&self.text, offset)
    }

    pub(super) fn start_of_line_at(&self, offset: usize) -> usize {
        if self.is_single_line() {
            return 0;
        }
        let row = self.text.offset_to_point(offset).row;
        self.text.line_start_offset(row)
    }

    pub(super) fn end_of_line_at(&self, offset: usize) -> usize {
        if self.is_single_line() {
            return self.text.len();
        }
        let row = self.text.offset_to_point(offset).row;
        self.text.line_end_offset(row)
    }

    fn delete_selection(
        &mut self,
        collapsed_intent: EditIntent,
        collapsed_target: impl FnOnce(&Self, usize) -> Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let before = self.selection;
        let intent = if before.is_empty() {
            collapsed_intent
        } else {
            EditIntent::Atomic
        };
        let range = if before.is_empty() {
            collapsed_target(self, before.cursor_offset())
        } else {
            before.start..before.end
        };
        let (start, end) = (range.start.min(range.end), range.start.max(range.end));

        self.undo_manager.begin_transaction_with(intent);
        self.undo_manager.record_selection(before, before);
        self.selection.start = start;
        self.selection.end = end;
        self.undo_manager.set_pending_intent(intent);

        self.replace_text_in_range(None, "", window, cx);
        self.undo_manager.record_selection(before, self.selection);
        self.undo_manager.commit_transaction();
        self.pause_blink_cursor(cx);
    }

    pub(super) fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selection.is_empty() && self.cursor() == 0 {
            cx.propagate();
            return;
        }

        self.delete_selection(
            EditIntent::Backspace,
            |s, offset| s.previous_boundary(offset)..offset,
            window,
            cx,
        );
    }

    pub(super) fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        self.delete_selection(
            EditIntent::DeleteForward,
            |s, offset| offset..s.next_boundary(offset),
            window,
            cx,
        );
    }

    pub(super) fn delete_previous_word(
        &mut self,
        _: &DeleteToPreviousWordStart,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.delete_selection(
            EditIntent::Atomic,
            |s, offset| s.previous_start_of_word_at(offset)..offset,
            window,
            cx,
        );
    }

    pub(super) fn delete_next_word(
        &mut self,
        _: &DeleteToNextWordEnd,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.delete_selection(
            EditIntent::Atomic,
            |s, offset| offset..s.next_end_of_word_at(offset),
            window,
            cx,
        );
    }

    pub(super) fn enter(&mut self, _: &Enter, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_multi_line() {
            self.replace_text_in_range(None, "\n", window, cx);
            self.pause_blink_cursor(cx);
        } else {
            self.undo_manager.break_transaction_coalescing();
            cx.propagate();
        }
    }

    pub(super) fn escape(&mut self, _: &Escape, _window: &mut Window, cx: &mut Context<Self>) {
        if self.ime_marked_range.is_some() {
            self.unmark_text();
        }
        cx.propagate();
    }

    pub(crate) fn handle_right_click_menu(
        &mut self,
        position: Point<Pixels>,
        offset: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.selection.contains(offset) {
            self.move_to(offset, None, cx);
        }

        if let Some(handler) = self.context_menu_handler.clone() {
            let input = cx.weak_entity();
            window.defer(cx, move |window, cx| {
                if input.upgrade().is_none() {
                    return;
                }
                let previous_focus = window.focused(cx);
                handler(position, window, cx);
                input
                    .update(cx, |this, cx| {
                        if window.focused(cx) == previous_focus
                            && !this.focus_handle.is_focused(window)
                            && this.has_selection_focus(window, cx)
                        {
                            return;
                        }
                        let menu_focus = window.focused(cx).filter(|focus| {
                            Some(focus) != previous_focus.as_ref() && *focus != this.focus_handle
                        });
                        this.set_selection_focus(menu_focus, cx);
                    })
                    .ok();
            });
        }
    }

    pub(super) fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if window.default_prevented() {
            return;
        }
        self.undo_manager.break_transaction_coalescing();

        if let Some(ime_marked_range) = &self.ime_marked_range
            && ime_marked_range.is_empty()
        {
            self.ime_marked_range = None;
        }

        self.selecting = true;
        let (offset, line_end_affinity) = self.resolve_mouse_position(event.position);

        if event.button == MouseButton::Left && event.click_count >= 3 {
            self.select_line(offset, window, cx);
            return;
        }

        if event.button == MouseButton::Left && event.click_count == 2 {
            self.select_word(offset, window, cx);
            return;
        }

        if event.button == MouseButton::Right {
            if !self.selection.contains(offset) {
                self.move_to(offset, None, cx);
            }
            self.pending_context_menu = Some((event.position, offset));
            return;
        }

        if event.modifiers.shift {
            self.select_to_with_affinity(offset, line_end_affinity, cx);
        } else {
            self.move_to_with_affinity(offset, None, line_end_affinity, cx);
        }
    }

    pub(super) fn on_mouse_up(
        &mut self,
        event: &MouseUpEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.button == MouseButton::Right
            && let Some((position, offset)) = self.pending_context_menu.take()
        {
            self.handle_right_click_menu(position, offset, window, cx);
        }
        if self.selection.is_empty() {
            self.selection.reversed = false;
        }
        self.selecting = false;
        self.selected_word_range = None;
        self.auto_scroll.stop();
    }

    pub(super) fn on_scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.modifiers.control {
            return;
        }
        let line_height = self
            .last_layout
            .as_ref()
            .map(|layout| layout.line_height)
            .unwrap_or(window.line_height());
        let delta = match event.delta {
            ScrollDelta::Lines(lines) if self.viewport_wheel_step => {
                let chars_per_notch = wheel_scroll_amount(true).max(1) as f32;
                let lines_per_notch = wheel_scroll_amount(false).max(1) as f32;
                let size = self.input_bounds.size;
                let step = |notches: f32, extent: Pixels| {
                    let extent = extent.as_f32();
                    let notch = (extent * WHEEL_STEP_FRACTION).round().max(MIN_WHEEL_STEP);
                    px((notches * notch)
                        .round()
                        .clamp(-extent.floor(), extent.floor()))
                };
                point(
                    step(lines.x / chars_per_notch, size.width),
                    step(lines.y / lines_per_notch, size.height),
                )
            }
            _ => event.delta.pixel_delta(line_height),
        };

        let old_offset = self.scroll_handle.offset();
        self.update_scroll_offset(Some(old_offset + delta), cx);

        if self.scroll_handle.offset() != old_offset {
            cx.stop_propagation();
        }
    }

    pub(super) fn update_scroll_offset(
        &mut self,
        offset: Option<Point<Pixels>>,
        cx: &mut Context<Self>,
    ) {
        let mut offset = offset.unwrap_or(self.scroll_handle.offset());
        let safe_y_range =
            (-self.scroll_size.height + self.input_bounds.size.height).min(px(0.0))..px(0.);

        offset.y = if self.is_single_line() {
            px(0.)
        } else {
            offset.y.clamp(safe_y_range.start, safe_y_range.end)
        };
        offset.x = clamp_horizontal_scroll_offset(
            offset.x,
            self.scroll_size.width,
            self.input_bounds.size.width,
        );
        if self.scroll_handle.offset() != offset {
            self.scroll_handle.set_offset(offset);
            cx.notify();
        }
    }

    pub(crate) fn scroll_to(
        &mut self,
        offset: usize,
        direction: Option<MoveDirection>,
        cx: &mut Context<Self>,
    ) {
        let Some(last_layout) = self.last_layout.as_ref() else {
            return;
        };
        let Some(bounds) = self.last_bounds.as_ref() else {
            return;
        };

        let mut scroll_offset = self.scroll_handle.offset();
        let was_offset = scroll_offset;
        let line_height = last_layout.line_height;

        let point = self.text.offset_to_point(offset);
        let row = point.row;
        let display_pos = self.text_wrapper.point_to_display_point(point);
        let row_offset_y = line_height * display_pos.row;

        if let Ok(vi) = last_layout.visible_buffer_lines.binary_search(&row) {
            let line = &last_layout.lines[vi];
            let local_offset = offset.saturating_sub(last_layout.visible_line_byte_offsets[vi]);
            if let Some(pos) = line.position_for_index(local_offset, last_layout, false) {
                let bounds_width = bounds.size.width;
                let col_offset_x = pos.x;
                if col_offset_x - RIGHT_MARGIN < -scroll_offset.x {
                    scroll_offset.x = -col_offset_x + RIGHT_MARGIN;
                } else if col_offset_x + RIGHT_MARGIN > -scroll_offset.x + bounds_width {
                    scroll_offset.x = -(col_offset_x - bounds_width + RIGHT_MARGIN);
                }
            }
        }

        let edge_height = line_height;
        if row_offset_y - edge_height + line_height < -scroll_offset.y {
            scroll_offset.y = -row_offset_y + edge_height - line_height;
        } else if row_offset_y + edge_height > -scroll_offset.y + bounds.size.height {
            scroll_offset.y = -(row_offset_y - bounds.size.height + edge_height);
        }

        if direction == Some(MoveDirection::Up) {
            scroll_offset.y = scroll_offset.y.max(was_offset.y);
        } else if direction == Some(MoveDirection::Down) {
            scroll_offset.y = scroll_offset.y.min(was_offset.y);
        }

        let safe_y_min = (-self.scroll_size.height + self.input_bounds.size.height).min(px(0.));
        scroll_offset.x = scroll_offset.x.min(px(0.));
        scroll_offset.y = scroll_offset.y.clamp(safe_y_min, px(0.));
        self.deferred_scroll_offset = Some(scroll_offset);
        cx.notify();
    }

    pub(super) fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.is_copyable() {
            return;
        }
        let text = self.text.slice(self.selection).to_string();
        cx.write_clipboard_text(&text);
    }

    pub(super) fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_copyable() {
            return;
        }
        let text = self.text.slice(self.selection).to_string();
        cx.write_clipboard_text(&text);
        self.undo_manager.set_pending_intent(EditIntent::Atomic);
        self.replace_text_in_range(None, "", window, cx);
    }

    pub(super) fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(mut new_text) = cx.read_clipboard_text().filter(|text| !text.is_empty()) {
            if new_text.contains('\r') {
                new_text = new_text.replace("\r\n", "\n").replace('\r', "\n");
            }
            if self.is_single_line() {
                new_text = new_text.replace('\n', "");
            }
            self.undo_manager.set_pending_intent(EditIntent::Atomic);
            self.replace_text_in_range(None, &new_text, window, cx);
            self.scroll_to(self.cursor(), None, cx);
        }
    }

    fn collapse_for_intent(
        intent: EditIntent,
        mut selection: CursorSelection,
        range: &Range<usize>,
    ) -> CursorSelection {
        match intent {
            EditIntent::Backspace => selection.place_at(range.end, None),
            EditIntent::DeleteForward => selection.place_at(range.start, None),
            EditIntent::Typing | EditIntent::Atomic => {}
        }
        selection
    }

    fn push_history(
        &mut self,
        text: &Rope,
        range: &Range<usize>,
        new_text: &str,
        requested_intent: Option<EditIntent>,
        selection_before: CursorSelection,
        selection_after: Option<CursorSelection>,
    ) -> bool {
        self.document_revision = self.document_revision.wrapping_add(1);
        if self.undo_manager.is_ignoring() {
            return false;
        }

        let range =
            text.clip_offset(range.start, Bias::Left)..text.clip_offset(range.end, Bias::Right);
        let old_text = text.slice(range.clone()).to_string();
        let new_range = range.start..range.start + new_text.len();

        let intent = requested_intent.unwrap_or_else(|| {
            if range.is_empty()
                && old_text.is_empty()
                && !new_text.is_empty()
                && !new_text.contains(['\n', '\r'])
            {
                EditIntent::Typing
            } else {
                EditIntent::Atomic
            }
        });

        let selection_before = Self::collapse_for_intent(intent, selection_before, &range);
        let selection_after =
            selection_after.unwrap_or_else(|| (new_range.end..new_range.end).into());

        let open_transaction = self.undo_manager.has_open_transaction();
        let change = Change::new(range, &old_text, new_range, new_text);
        let recorded = self.undo_manager.record_transaction(change, intent);
        if recorded && !open_transaction {
            self.undo_manager
                .record_selection(selection_before, selection_after);
        }
        recorded
    }

    pub(super) fn undo(&mut self, _: &Undo, window: &mut Window, cx: &mut Context<Self>) {
        self.replay_history(true, window, cx);
    }

    pub(super) fn redo(&mut self, _: &Redo, window: &mut Window, cx: &mut Context<Self>) {
        self.replay_history(false, window, cx);
    }

    fn replay_history(&mut self, undo: bool, window: &mut Window, cx: &mut Context<Self>) {
        let replay = if undo {
            self.undo_manager.undo()
        } else {
            self.undo_manager.redo()
        };
        let Some(replay) = replay else {
            return;
        };
        self.undo_manager.set_ignoring(true);
        for change in &replay.changes {
            let (range, text) = if undo {
                (change.new_range.into(), &change.old_text)
            } else {
                (change.old_range.into(), &change.new_text)
            };
            let range_utf16 = self.range_to_utf16(&range);
            self.replace_text_in_range(Some(range_utf16), text, window, cx);
        }
        if let Some(mut sel) = replay.selection {
            let len = self.text.len();
            sel.start = sel.start.min(len);
            sel.end = sel.end.min(len);
            self.selection = sel;
        }
        self.undo_manager.set_ignoring(false);
        cx.notify();
    }

    pub(crate) fn cursor(&self) -> usize {
        if let Some(ime_marked_range) = &self.ime_marked_range {
            return ime_marked_range.end;
        }
        self.selection.cursor_offset()
    }

    pub(super) fn set_selection(&mut self, start: usize, end: usize) {
        self.selection.start = start;
        self.selection.end = end;
    }

    pub(super) fn set_cursor_to(&mut self, offset: usize) {
        self.selection.start = offset;
        self.selection.end = offset;
        self.selection.reversed = false;
    }

    pub(crate) fn selected_range(&self) -> Range<usize> {
        self.selection.into()
    }

    pub(crate) fn select_all(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.undo_manager.break_transaction_coalescing();
        self.set_selection(0, self.text.len());
        cx.notify();
    }

    pub(crate) fn set_selected_range(&mut self, range: Range<usize>, cx: &mut Context<Self>) {
        let end_bias = if range.start == range.end {
            Bias::Left
        } else {
            Bias::Right
        };
        let start = self.text.clip_offset(range.start, Bias::Left);
        let end = self.text.clip_offset(range.end, end_bias);

        self.move_to(start, None, cx);
        self.selection.reversed = false;
        self.selected_word_range = None;
        self.select_to(end, cx);
    }

    pub(super) fn resolve_mouse_position(&self, position: Point<Pixels>) -> (usize, bool) {
        if self.text.len() == 0 {
            return (0, false);
        }
        let (Some(bounds), Some(last_layout)) =
            (self.last_bounds.as_ref(), self.last_layout.as_ref())
        else {
            return (0, false);
        };

        let line_height = last_layout.line_height;
        let inner_position = position - bounds.origin;
        let mut y_offset = last_layout.visible_top;

        for (vi, line_layout) in last_layout.lines.iter().enumerate() {
            let line_start_offset = last_layout.visible_line_byte_offsets[vi];
            let pos = inner_position - point(px(0.), y_offset);

            if self.is_single_line() {
                let local_index = line_layout.closest_index_for_x(pos.x);
                return (self.resolve_index(line_start_offset + local_index), false);
            }

            if let Some((local_index, line_end_affinity)) =
                line_layout.closest_index_for_position(pos, last_layout)
            {
                return (
                    self.resolve_index(line_start_offset + local_index),
                    line_end_affinity,
                );
            } else if pos.y < px(0.) {
                return (self.resolve_index(line_start_offset), false);
            }

            y_offset += line_layout.size(line_height).height;
        }

        let last_offset = last_layout
            .lines
            .last()
            .zip(last_layout.visible_line_byte_offsets.last())
            .map(|(line_layout, line_start)| self.resolve_index(line_start + line_layout.len()))
            .unwrap_or(self.text.len());
        (last_offset, false)
    }

    fn resolve_index(&self, index: usize) -> usize {
        index.min(self.text.len())
    }

    fn extend_selection(
        sel: &mut CursorSelection,
        offset: usize,
        word_range: Option<CursorSelection>,
    ) {
        if sel.reversed {
            sel.start = offset;
        } else {
            sel.end = offset;
        }

        if sel.end < sel.start {
            sel.reversed = !sel.reversed;
            std::mem::swap(&mut sel.start, &mut sel.end);
        }

        if let Some(word_range) = word_range {
            if sel.start > word_range.start {
                sel.start = word_range.start;
            }
            if sel.end < word_range.end {
                sel.end = word_range.end;
            }
        }
    }

    pub(crate) fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.select_to_with_affinity(offset, false, cx);
    }

    pub(crate) fn select_to_with_affinity(
        &mut self,
        offset: usize,
        line_end_affinity: bool,
        cx: &mut Context<Self>,
    ) {
        self.cursor_line_end_affinity = line_end_affinity;
        let offset = self.cursor_boundary(offset, Bias::Left);
        let word_range = self.selected_word_range;
        Self::extend_selection(&mut self.selection, offset, word_range);

        if self.selection.is_empty() {
            self.update_preferred_column();
        }
        cx.notify();
    }

    fn select_keyboard_to(
        &mut self,
        offset: usize,
        affinity: bool,
        preserve_column: bool,
        cx: &mut Context<Self>,
    ) {
        self.pause_blink_cursor(cx);
        self.undo_manager.break_transaction_coalescing();

        let prev_cursor = self.selection.cursor_offset();
        let prev_affinity = self.cursor_line_end_affinity;
        let offset = self.cursor_boundary(offset, Bias::Left);
        Self::extend_selection(&mut self.selection, offset, None);
        self.cursor_line_end_affinity = affinity;

        if !preserve_column {
            self.selection.column_anchor =
                self.preferred_column_for_with_affinity(self.selection.cursor_offset(), affinity);
        } else if self.selection.column_anchor.is_none() {
            self.selection.column_anchor =
                self.preferred_column_for_with_affinity(prev_cursor, prev_affinity);
        }

        self.scroll_to(self.cursor(), None, cx);
        cx.notify();
    }

    #[inline]
    pub(super) fn offset_from_utf16(&self, offset: usize) -> usize {
        self.text.offset_utf16_to_offset(offset)
    }

    #[inline]
    pub(super) fn offset_to_utf16(&self, offset: usize) -> usize {
        self.text.offset_to_offset_utf16(offset)
    }

    #[inline]
    pub(crate) fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    #[inline]
    pub(super) fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range_utf16.start)..self.offset_from_utf16(range_utf16.end)
    }

    pub(super) fn cursor_boundary(&self, offset: usize, bias: Bias) -> usize {
        let offset = self.text.clip_offset(offset, bias);
        if offset > 0
            && self.text.char_at(offset - 1) == Some('\r')
            && self.text.char_at(offset) == Some('\n')
        {
            if bias == Bias::Left {
                offset - 1
            } else {
                offset + 1
            }
        } else {
            offset
        }
    }

    pub(super) fn previous_boundary(&self, offset: usize) -> usize {
        self.cursor_boundary(offset.saturating_sub(1), Bias::Left)
    }

    pub(super) fn next_boundary(&self, offset: usize) -> usize {
        self.cursor_boundary(offset.saturating_add(1), Bias::Right)
    }

    pub(crate) fn show_cursor(&self, window: &Window, cx: &App) -> bool {
        self.focus_handle.is_focused(window)
            && self.blink_cursor.read(cx).visible()
            && window.is_window_active()
    }

    fn on_focus(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.blink_cursor.update(cx, |cursor, cx| {
            cursor.start(cx);
        });
    }

    fn on_blur(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.undo_manager.break_transaction_coalescing();
        self.blink_cursor.update(cx, |cursor, cx| {
            cursor.stop(cx);
        });
        cx.notify();
    }

    pub(super) fn pause_blink_cursor(&mut self, cx: &mut Context<Self>) {
        self.blink_cursor.update(cx, |cursor, cx| {
            cursor.pause(cx);
        });
    }

    pub(super) fn on_drag_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.text.len() == 0
            || self.last_layout.is_none()
            || !self.focus_handle.is_focused(window)
            || !self.selecting
        {
            return;
        }

        self.auto_scroll.last_drag_position = Some(event.position);
        let (offset, line_end_affinity) = self.resolve_mouse_position(event.position);
        self.select_to_with_affinity(offset, line_end_affinity, cx);

        if self.is_multi_line() {
            let delta = AutoScroll::compute_delta(event.position.y, self.input_bounds);
            let scroll_delta = delta.map(|d| -d);
            self.auto_scroll.set(scroll_delta, cx, |delta, state, cx| {
                let current = state.scroll_handle.offset();
                state.update_scroll_offset(Some(point(current.x, current.y + delta)), cx);
                if let Some(pos) = state.auto_scroll.last_drag_position {
                    let (offset, line_end_affinity) = state.resolve_mouse_position(pos);
                    state.select_to_with_affinity(offset, line_end_affinity, cx);
                }
            });
        }
    }

    pub(super) fn normalize_input<'a>(&self, new_text: &'a str) -> Cow<'a, str> {
        if self.is_single_line() && new_text.contains(['\n', '\r']) {
            Cow::Owned(new_text.replace(['\n', '\r'], ""))
        } else {
            Cow::Borrowed(new_text)
        }
    }

    pub(crate) fn is_valid_input(&self, new_text: &str, cx: &mut Context<Self>) -> bool {
        if new_text.is_empty() {
            return true;
        }
        if let Some(validate) = &self.validate
            && !validate(new_text, cx)
        {
            return false;
        }
        true
    }

    pub(super) fn set_input_bounds(&mut self, new_bounds: Bounds<Pixels>, cx: &mut Context<Self>) {
        let wrap_width_changed = self.input_bounds.size.width != new_bounds.size.width;
        self.input_bounds = new_bounds;

        if let Some(last_layout) = self.last_layout.as_ref()
            && wrap_width_changed
        {
            let wrap_width = if self.soft_wrap {
                last_layout.wrap_width
            } else {
                None
            };
            self.text_wrapper.set_wrap_width(wrap_width, cx);
            cx.notify();
        }
    }

    pub(crate) fn selected_text(&self) -> RopeSlice<'_> {
        let range_utf16 = self.range_to_utf16(&self.selected_range());
        let range = self.range_from_utf16(&range_utf16);
        self.text.slice(range)
    }

    pub(crate) fn replace_text_in_ranges(
        &mut self,
        edits: &[(Range<usize>, String)],
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if edits.is_empty() {
            return;
        }

        let mut sorted: Vec<(Range<usize>, &str)> = edits
            .iter()
            .map(|(range, text)| (range.clone(), text.as_str()))
            .collect();
        sorted.sort_by_key(|edit| std::cmp::Reverse(edit.0.start));

        #[cfg(debug_assertions)]
        for pair in sorted.windows(2) {
            debug_assert!(
                pair[1].0.end <= pair[0].0.start,
                "replace_text_in_ranges requires disjoint ranges"
            );
        }

        let requested_intent = self.undo_manager.take_pending_intent();
        let selection_before = Self::collapse_for_intent(
            requested_intent.unwrap_or(EditIntent::Atomic),
            self.selection,
            &(self.selection.start..self.selection.end),
        );
        let group = sorted.len() > 1;
        if group {
            self.undo_manager.begin_transaction();
        }

        let mut recorded = false;
        for (range, new_text) in &sorted {
            let old_text = self.text.clone();
            self.text.replace(range.clone(), new_text);

            recorded |= self.push_history(
                &old_text,
                range,
                new_text,
                requested_intent,
                self.selection,
                None,
            );

            self.text_wrapper
                .update(&self.text, range, &Rope::from(*new_text), cx);
        }

        if group {
            self.undo_manager.commit_transaction();
        }

        let mut ascending = sorted;
        ascending.sort_by_key(|edit| edit.0.start);
        let text_len = self.text.len();
        let mut delta: isize = 0;
        let mut last_offset = 0;
        for (range, new_text) in &ascending {
            last_offset = ((range.start as isize + delta) as usize + new_text.len()).min(text_len);
            delta += new_text.len() as isize - (range.end as isize - range.start as isize);
        }
        self.selection.place_at(last_offset, None);

        if recorded {
            self.undo_manager
                .record_selection(selection_before, self.selection);
        }

        self.ime_marked_range.take();
        self.update_preferred_column();
        cx.notify();
    }

    fn unmark_text(&mut self) {
        self.ime_marked_range = None;
        self.undo_manager.commit_transaction();
    }
}

impl EntityInputHandler for EditorState {
    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range()),
        })
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let requested_intent = self.undo_manager.take_pending_intent();
        let selection_before = self.selection;
        let ends_composition = self.ime_marked_range.is_some();

        self.pause_blink_cursor(cx);

        let new_text = self.normalize_input(new_text);
        let new_text: &str = &new_text;

        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.ime_marked_range.map(|range| {
                let range = self.range_to_utf16(&(range.start..range.end));
                self.range_from_utf16(&range)
            }))
            .unwrap_or(self.selected_range());

        if self.is_multi_line() {
            if let Some(intent) = requested_intent {
                self.undo_manager.set_pending_intent(intent);
            }
            self.replace_text_in_ranges(&[(range, new_text.to_string())], window, cx);
            if ends_composition {
                self.undo_manager.commit_transaction();
            }
            return;
        }

        let old_text = self.text.clone();
        self.text.replace(range.clone(), new_text);
        let new_offset = (range.start + new_text.len()).min(self.text.len());

        let pending_text = self.text.to_string();
        if !self.is_valid_input(&pending_text, cx)
            && self.is_valid_input(&old_text.to_string(), cx)
        {
            self.text = old_text;
            return;
        }

        self.push_history(
            &old_text,
            &range,
            new_text,
            requested_intent,
            selection_before,
            None,
        );
        self.text_wrapper
            .update(&self.text, &range, &Rope::from(new_text), cx);

        self.set_cursor_to(new_offset);
        self.ime_marked_range.take();
        if ends_composition {
            self.undo_manager
                .record_selection(selection_before, self.selection);
            self.undo_manager.commit_transaction();
        }
        self.update_preferred_column();
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let requested_intent = self.undo_manager.take_pending_intent();
        let selection_before = self.selection;

        let starts_composition = self.ime_marked_range.is_none();
        if starts_composition {
            self.undo_manager.break_transaction_coalescing();
            self.undo_manager.begin_transaction();
        }

        let new_text = self.normalize_input(new_text);
        let new_text: &str = &new_text;

        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.ime_marked_range.map(|range| {
                let range = self.range_to_utf16(&(range.start..range.end));
                self.range_from_utf16(&range)
            }))
            .unwrap_or(self.selected_range());

        let old_text = self.text.clone();
        self.text.replace(range.clone(), new_text);

        if self.is_single_line() {
            let pending_text = self.text.to_string();
            if !self.is_valid_input(&pending_text, cx)
                && self.is_valid_input(&old_text.to_string(), cx)
            {
                self.text = old_text;
                if starts_composition {
                    self.undo_manager.commit_transaction();
                }
                return;
            }
        }

        self.text_wrapper
            .update(&self.text, &range, &Rope::from(new_text), cx);

        if new_text.is_empty() {
            self.set_cursor_to(range.start);
            self.ime_marked_range = None;
        } else {
            self.ime_marked_range = Some((range.start..range.start + new_text.len()).into());
            let new_range = new_selected_range_utf16
                .as_ref()
                .map(|range_utf16| {
                    let new_text = Rope::from(new_text);
                    range.start + new_text.offset_utf16_to_offset(range_utf16.start)
                        ..range.start + new_text.offset_utf16_to_offset(range_utf16.end)
                })
                .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());
            self.set_selection(new_range.start, new_range.end);
        }
        if self.push_history(
            &old_text,
            &range,
            new_text,
            requested_intent,
            selection_before,
            Some(self.selection),
        ) {
            self.undo_manager
                .record_selection(selection_before, self.selection);
        }
        if new_text.is_empty() {
            self.undo_manager.commit_transaction();
        }
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let last_layout = self.last_layout.as_ref()?;
        let line_height = last_layout.line_height;
        let range = self.range_from_utf16(&range_utf16);

        let mut start_origin = None;
        let mut end_origin = None;
        let mut y_offset = last_layout.visible_top;

        for (vi, line) in last_layout.lines.iter().enumerate() {
            if start_origin.is_some() && end_origin.is_some() {
                break;
            }
            let index_offset = last_layout.visible_line_byte_offsets[vi];
            if start_origin.is_none()
                && let Some(p) = line.position_for_index(
                    range.start.saturating_sub(index_offset),
                    last_layout,
                    false,
                )
            {
                start_origin = Some(p + point(px(0.), y_offset));
            }
            if end_origin.is_none()
                && let Some(p) = line.position_for_index(
                    range.end.saturating_sub(index_offset),
                    last_layout,
                    false,
                )
            {
                end_origin = Some(p + point(px(0.), y_offset));
            }
            y_offset += line.size(line_height).height;
        }

        let start_origin = start_origin.or_else(|| {
            let offset = self.last_cursor.or(Some(self.cursor()))?;
            self.line_and_position_for_offset(offset).2
        })?;
        let mut end_origin = end_origin.unwrap_or(start_origin);
        end_origin.y = start_origin.y;

        Some(Bounds::from_corners(
            bounds.origin + start_origin,
            bounds.origin + point(end_origin.x, end_origin.y + line_height),
        ))
    }
}

impl Focusable for EditorState {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for EditorState {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();

        div()
            .id("input-state")
            .key_context(CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(window.listener_for(&entity, EditorState::backspace))
            .on_action(window.listener_for(&entity, EditorState::delete))
            .on_action(window.listener_for(&entity, EditorState::delete_previous_word))
            .on_action(window.listener_for(&entity, EditorState::delete_next_word))
            .on_action(window.listener_for(&entity, EditorState::enter))
            .on_action(window.listener_for(&entity, EditorState::escape))
            .on_action(window.listener_for(&entity, EditorState::paste))
            .on_action(window.listener_for(&entity, EditorState::cut))
            .on_action(window.listener_for(&entity, EditorState::undo))
            .on_action(window.listener_for(&entity, EditorState::redo))
            .when(self.is_multi_line(), |this| {
                this.on_action(window.listener_for(&entity, EditorState::indent_inline))
                    .on_action(window.listener_for(&entity, EditorState::outdent_inline))
                    .on_action(window.listener_for(&entity, EditorState::indent_block))
                    .on_action(window.listener_for(&entity, EditorState::outdent_block))
            })
            .on_action(window.listener_for(&entity, EditorState::left))
            .on_action(window.listener_for(&entity, EditorState::right))
            .on_action(window.listener_for(&entity, EditorState::select_left))
            .on_action(window.listener_for(&entity, EditorState::select_right))
            .when(self.is_multi_line(), |this| {
                this.on_action(window.listener_for(&entity, EditorState::up))
                    .on_action(window.listener_for(&entity, EditorState::down))
                    .on_action(window.listener_for(&entity, EditorState::select_up))
                    .on_action(window.listener_for(&entity, EditorState::select_down))
                    .on_action(window.listener_for(&entity, EditorState::page_up))
                    .on_action(window.listener_for(&entity, EditorState::page_down))
            })
            .on_action(window.listener_for(&entity, EditorState::on_action_select_all))
            .on_action(window.listener_for(&entity, EditorState::select_to_start_of_line))
            .on_action(window.listener_for(&entity, EditorState::select_to_end_of_line))
            .on_action(window.listener_for(&entity, EditorState::select_to_previous_word))
            .on_action(window.listener_for(&entity, EditorState::select_to_next_word))
            .on_action(window.listener_for(&entity, EditorState::home))
            .on_action(window.listener_for(&entity, EditorState::end))
            .on_action(window.listener_for(&entity, EditorState::move_to_start))
            .on_action(window.listener_for(&entity, EditorState::move_to_end))
            .on_action(window.listener_for(&entity, EditorState::move_to_previous_word))
            .on_action(window.listener_for(&entity, EditorState::move_to_next_word))
            .on_action(window.listener_for(&entity, EditorState::select_to_start))
            .on_action(window.listener_for(&entity, EditorState::select_to_end))
            .on_action(window.listener_for(&entity, EditorState::copy))
            .on_mouse_down(
                MouseButton::Left,
                window.listener_for(&entity, EditorState::on_mouse_down),
            )
            .on_mouse_down(
                MouseButton::Right,
                window.listener_for(&entity, EditorState::on_mouse_down),
            )
            .on_mouse_up(
                MouseButton::Left,
                window.listener_for(&entity, EditorState::on_mouse_up),
            )
            .on_mouse_up(
                MouseButton::Right,
                window.listener_for(&entity, EditorState::on_mouse_up),
            )
            .on_scroll_wheel(window.listener_for(&entity, EditorState::on_scroll_wheel))
            .cursor_text()
            .flex_1()
            .map(|this| {
                if self.is_multi_line() {
                    this.h_full()
                } else {
                    this.min_h_full().flex().items_center()
                }
            })
            .flex_grow_1()
            .overflow_x_hidden()
            .when(self.is_multi_line(), |this| {
                this.pt(self.editor_paddings.top)
                    .pr(self.editor_paddings.right)
                    .pb(self.editor_paddings.bottom)
                    .pl(self.editor_paddings.left)
            })
            .child(TextElement::new(entity.clone()).placeholder(self.placeholder.clone()))
            .when(self.is_multi_line(), |this| {
                this.child(EditorScrollbar::new(entity))
            })
    }
}

/// Find the start of the previous UAX #29 word before `offset` in O(word length) time.
fn previous_word_start(text: &Rope, offset: usize) -> usize {
    let offset = text.floor_char_boundary(offset.min(text.len()));
    if offset == 0 {
        return 0;
    }

    let mut word_end = offset;
    for ch in text.chars_at(offset).reversed() {
        if ch.is_whitespace() {
            word_end -= ch.len_utf8();
        } else {
            break;
        }
    }
    if word_end == 0 {
        return 0;
    }

    let mut window = WORD_SCAN_WINDOW_BYTES;
    loop {
        let win_start = text.floor_char_boundary(word_end.saturating_sub(window));
        let slice = text.slice(win_start..word_end).to_string();
        if let Some((i, _)) = UnicodeSegmentation::split_word_bound_indices(slice.as_str())
            .rfind(|(_, s)| !s.trim_start().is_empty())
            && (win_start == 0 || (i > 0 && slice[..i].chars().count() >= 4))
        {
            return win_start + i;
        }
        if win_start == 0 {
            return 0;
        }
        window = window.saturating_mul(2);
    }
}

/// Find the end of the next UAX #29 word after `offset` in O(word length) time.
fn next_word_end(text: &Rope, offset: usize) -> usize {
    let len = text.len();
    let offset = text.floor_char_boundary(offset.min(len));
    if offset >= len {
        return len;
    }

    let mut word_start = offset;
    for ch in text.chars_at(offset) {
        if ch.is_whitespace() {
            word_start += ch.len_utf8();
        } else {
            break;
        }
    }
    if word_start >= len {
        return len;
    }

    let mut window = WORD_SCAN_WINDOW_BYTES;
    loop {
        let win_end = text.ceil_char_boundary(word_start.saturating_add(window).min(len));
        let slice = text.slice(word_start..win_end).to_string();
        if let Some((i, s)) = UnicodeSegmentation::split_word_bound_indices(slice.as_str())
            .find(|(_, s)| !s.trim_start().is_empty())
        {
            let end = i + s.len();
            if win_end == len || slice[end..].chars().count() >= 4 {
                return word_start + end;
            }
        }
        if win_end == len {
            return len;
        }
        window = window.saturating_mul(2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reference_prev_word(text: &str, offset: usize) -> usize {
        UnicodeSegmentation::split_word_bound_indices(&text[..offset])
            .rfind(|(_, s)| !s.trim_start().is_empty())
            .map(|(i, _)| i)
            .unwrap_or(0)
    }

    fn reference_next_word(text: &str, offset: usize) -> usize {
        UnicodeSegmentation::split_word_bound_indices(&text[offset..])
            .find(|(_, s)| !s.trim_start().is_empty())
            .map(|(i, s)| offset + i + s.len())
            .unwrap_or(text.len())
    }

    #[test]
    fn bounded_word_navigation_matches_full_scan_everywhere() {
        let long_word = "a".repeat(WORD_SCAN_WINDOW_BYTES + 64);
        let long_spaces = " ".repeat(WORD_SCAN_WINDOW_BYTES + 64);
        let sample = format!(
            "  hello, don't 1,234.56 foo_bar\r\n日本語🎉 test{long_spaces}{long_word}  tail"
        );
        let rope = Rope::from(sample.as_str());

        for (offset, _) in sample.char_indices().chain([(sample.len(), '\0')]) {
            assert_eq!(
                previous_word_start(&rope, offset),
                reference_prev_word(&sample, offset),
                "prev mismatch at offset {offset}"
            );
            assert_eq!(
                next_word_end(&rope, offset),
                reference_next_word(&sample, offset),
                "next mismatch at offset {offset}"
            );
        }
    }
}
