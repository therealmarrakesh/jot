use crate::ui::{
    App, AppContext, Bounds, Command, Context, Edges, Entity, EntityInputHandler as _, FocusHandle,
    Focusable, KeyBinding, MouseButton, Pixels, Point, Render, ScrollHandle, SharedString,
    Subscription, UiNode, WeakFocusHandle, Window, div, point, px,
};
use ropey::{Rope, RopeSlice};
use std::borrow::Cow;
use std::ops::Range;
use std::rc::Rc;

use super::auto_scroll::AutoScroll;
use super::rope_ext::Bias;
use super::{
    Position, RopeExt as _, TabSize,
    blink_cursor::BlinkCursor,
    cursor::CursorSelection,
    element::{EditorScrollbar, EditorScrollbarSnapshot, LongestLineKey, TextElement},
    layout::LastLayout,
    text_wrapper::TextWrapper,
    undo_manager::{EditIntent, UndoManager},
};

pub(super) const CONTEXT: &str = "Input";

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
    pub(super) caret_height: Pixels,
    pub(super) caret_hidden: bool,
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
    pub(super) editor_scrollbar_snapshot: Option<EditorScrollbarSnapshot>,
    pub(super) viewport_wheel_step: bool,
    pub(super) longest_line_width: Option<(LongestLineKey, Pixels)>,
    pub(super) editor_paddings: Edges<Pixels>,
    pub(super) placeholder: SharedString,
    pub(super) context_menu_handler: Option<ContextMenuHandler>,
    pub(super) pending_context_menu: Option<(Point<Pixels>, usize)>,
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
            caret_height: px(0.),
            caret_hidden: false,
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
            editor_scrollbar_snapshot: None,
            viewport_wheel_step: false,
            longest_line_width: None,
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

    pub(crate) fn caret_height(mut self, height: Pixels) -> Self {
        self.caret_height = height;
        self
    }

    pub(crate) fn set_caret_height(&mut self, height: Pixels) {
        self.caret_height = height;
    }

    pub(crate) fn set_caret_hidden(&mut self, hidden: bool) {
        self.caret_hidden = hidden;
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

    #[cfg(test)]
    pub(crate) fn soft_wrap_enabled(&self) -> bool {
        self.soft_wrap
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
}

impl Focusable for EditorState {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EditorState {
    fn handle_command(
        &mut self,
        command: Command,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        match command {
            Command::Backspace => return self.backspace(window, cx),
            Command::Delete => self.delete(window, cx),
            Command::DeletePrevWord => self.delete_previous_word(window, cx),
            Command::DeleteNextWord => self.delete_next_word(window, cx),
            Command::Enter => return self.enter(window, cx),
            Command::Escape => return self.escape(),
            Command::Paste => self.paste(window, cx),
            Command::Cut => self.cut(window, cx),
            Command::Copy => self.copy(cx),
            Command::Undo => self.undo(window, cx),
            Command::Redo => self.redo(window, cx),
            Command::IndentInline if self.is_multi_line() => {
                return self.indent_inline(window, cx);
            }
            Command::OutdentInline if self.is_multi_line() => {
                return self.outdent_inline(window, cx);
            }
            Command::Indent if self.is_multi_line() => return self.indent_block(window, cx),
            Command::Outdent if self.is_multi_line() => return self.outdent_block(window, cx),
            Command::MoveLeft => return self.left(cx),
            Command::MoveRight => return self.right(cx),
            Command::SelectLeft => self.select_left(cx),
            Command::SelectRight => self.select_right(cx),
            Command::MoveUp if self.is_multi_line() => self.up(cx),
            Command::MoveDown if self.is_multi_line() => self.down(cx),
            Command::SelectUp if self.is_multi_line() => self.select_up(cx),
            Command::SelectDown if self.is_multi_line() => self.select_down(cx),
            Command::MovePageUp if self.is_multi_line() => self.page_up(cx),
            Command::MovePageDown if self.is_multi_line() => self.page_down(cx),
            Command::SelectAll => self.select_all(window, cx),
            Command::SelectToStartOfLine => self.select_to_start_of_line(cx),
            Command::SelectToEndOfLine => self.select_to_end_of_line(cx),
            Command::SelectPrevWord => self.select_to_previous_word(cx),
            Command::SelectNextWord => self.select_to_next_word(cx),
            Command::MoveHome => self.home(cx),
            Command::MoveEnd => self.end(cx),
            Command::MoveToStart => self.move_to_start(cx),
            Command::MoveToEnd => self.move_to_end(cx),
            Command::MovePrevWord => self.move_to_previous_word(cx),
            Command::MoveNextWord => self.move_to_next_word(cx),
            Command::SelectToStart => self.select_to_start(cx),
            Command::SelectToEnd => self.select_to_end(cx),
            _ => return false,
        }
        true
    }
}

impl Render for EditorState {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl Into<UiNode> {
        let entity = cx.entity();

        div()
            .id("input-state")
            .key_context(CONTEXT)
            .track_focus(&self.focus_handle)
            .on_command(window.command_listener_for(&entity, EditorState::handle_command))
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
