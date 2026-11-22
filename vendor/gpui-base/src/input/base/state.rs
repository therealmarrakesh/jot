//! A text input field that allows the user to enter text.
//!
//! Based on the `Input` example from the `gpui` crate.
//! https://github.com/zed-industries/zed/blob/main/crates/gpui/examples/input.rs
use gpui::TextAlign;
use gpui::{
    Action, App, AppContext, Bounds, ClipboardItem, Context, Edges, Entity, EntityInputHandler,
    EventEmitter, FocusHandle, Focusable, InteractiveElement as _, IntoElement, KeyBinding,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, Point,
    Render, ScrollDelta, ScrollHandle, ScrollWheelEvent, SharedString, Styled as _, Subscription,
    UTF16Selection, WeakFocusHandle, Window, actions, div, point, prelude::FluentBuilder as _, px,
};
use ropey::{Rope, RopeSlice};
use std::borrow::Cow;
use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;
use sum_tree::Bias;
use unicode_segmentation::*;

use super::{
    DisplayMap, InputContextMenuCapabilities, InputEditorStyle, MASK_CHAR, MaskPattern, NativeMenu,
    WrappingIndent,
    blink_cursor::BlinkCursor,
    change::Change,
    cursor::{CursorSelection, Selections},
    element::{
        EditorScrollbar, EditorScrollbarSnapshot, LongestLineKey, TextElement,
        clamp_horizontal_scroll_offset,
    },
    kind::InputModeKind,
    mask_pattern::normalize_number_input,
    mode::LayoutMode,
    undo_manager::{EditIntent, UndoManager},
};
use crate::AutoScroll;
use crate::actions::{SelectDown, SelectLeft, SelectRight, SelectUp};
use crate::input::blink_cursor::CURSOR_WIDTH;
use crate::input::movement::MoveDirection;
use crate::input::{Position, RopeExt as _, element::RIGHT_MARGIN, layout::LastLayout};

const DEFAULT_WHEEL_SCROLL: u32 = 3;
const WHEEL_STEP_FRACTION: f32 = 0.15;
const MIN_WHEEL_STEP: f32 = 48.;

#[cfg(target_os = "windows")]
fn wheel_scroll_amount(horizontal: bool) -> u32 {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SPI_GETWHEELSCROLLCHARS, SPI_GETWHEELSCROLLLINES, SystemParametersInfoW,
    };
    let action = if horizontal {
        SPI_GETWHEELSCROLLCHARS
    } else {
        SPI_GETWHEELSCROLLLINES
    };
    let mut amount = DEFAULT_WHEEL_SCROLL;
    let _ = unsafe {
        SystemParametersInfoW(action, 0, (&mut amount as *mut u32).cast(), 0)
    };
    amount
}

#[cfg(not(target_os = "windows"))]
fn wheel_scroll_amount(_horizontal: bool) -> u32 {
    DEFAULT_WHEEL_SCROLL
}

/// Vertical clearance to retain when revealing a text position.
pub(crate) enum ScrollPadding {
    Minimal,
    SurroundingLines,
}

#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = input, no_json)]
pub struct Enter {
    /// Is confirm with secondary.
    pub secondary: bool,
    /// Whether the Shift modifier was held when Enter was pressed.
    pub shift: bool,
}

impl Enter {
    /// Returns true if `action` is a primary `Enter` action (`secondary: false`),
    /// regardless of whether Shift was held.
    pub fn is_primary(action: &dyn Action) -> bool {
        action.partial_eq(&Enter {
            secondary: false,
            shift: false,
        }) || action.partial_eq(&Enter {
            secondary: false,
            shift: true,
        })
    }
}

actions!(
    input,
    [
        ActivateToken,
        Backspace,
        Delete,
        DeleteToBeginningOfLine,
        DeleteToEndOfLine,
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
        AddCursorAbove,
        AddCursorBelow,
        SelectAll,
        SelectToStartOfLine,
        SelectToEndOfLine,
        SelectToStart,
        SelectToEnd,
        SelectToPreviousWordStart,
        SelectToNextWordEnd,
        ShowCharacterPalette,
        Copy,
        Cut,
        Paste,
        Undo,
        Redo,
        MoveToStartOfLine,
        MoveToEndOfLine,
        MoveToStart,
        MoveToEnd,
        MoveToPreviousWord,
        MoveToNextWord,
        Escape,
    ]
);

#[derive(Clone)]
pub enum InputEvent {
    Change,
    PressEnter { secondary: bool, shift: bool },
    Focus,
    Blur,
}

pub(super) const CONTEXT: &str = "Input";

pub(crate) fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, Some(CONTEXT)),
        KeyBinding::new("shift-backspace", Backspace, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("ctrl-backspace", Backspace, Some(CONTEXT)),
        KeyBinding::new("delete", Delete, Some(CONTEXT)),
        KeyBinding::new("shift-delete", Delete, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-backspace", DeleteToBeginningOfLine, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-delete", DeleteToEndOfLine, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("alt-backspace", DeleteToPreviousWordStart, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-backspace", DeleteToPreviousWordStart, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("alt-delete", DeleteToNextWordEnd, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-delete", DeleteToNextWordEnd, Some(CONTEXT)),
        KeyBinding::new(
            "enter",
            Enter {
                secondary: false,
                shift: false,
            },
            Some(CONTEXT),
        ),
        KeyBinding::new(
            "shift-enter",
            Enter {
                secondary: false,
                shift: true,
            },
            Some(CONTEXT),
        ),
        KeyBinding::new(
            "secondary-enter",
            Enter {
                secondary: true,
                shift: false,
            },
            Some(CONTEXT),
        ),
        KeyBinding::new("escape", Escape, Some(CONTEXT)),
        KeyBinding::new("up", MoveUp, Some(CONTEXT)),
        KeyBinding::new("down", MoveDown, Some(CONTEXT)),
        KeyBinding::new("left", MoveLeft, Some(CONTEXT)),
        KeyBinding::new("right", MoveRight, Some(CONTEXT)),
        KeyBinding::new("pageup", MovePageUp, Some(CONTEXT)),
        KeyBinding::new("pagedown", MovePageDown, Some(CONTEXT)),
        KeyBinding::new("tab", IndentInline, Some(CONTEXT)),
        KeyBinding::new("shift-tab", OutdentInline, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-]", Indent, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-]", Indent, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-[", Outdent, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-[", Outdent, Some(CONTEXT)),
        KeyBinding::new("shift-left", SelectLeft, Some(CONTEXT)),
        KeyBinding::new("shift-right", SelectRight, Some(CONTEXT)),
        KeyBinding::new("shift-up", SelectUp, Some(CONTEXT)),
        KeyBinding::new("shift-down", SelectDown, Some(CONTEXT)),
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        KeyBinding::new("shift-alt-left", SelectLeft, Some(CONTEXT)),
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        KeyBinding::new("shift-alt-right", SelectRight, Some(CONTEXT)),
        // Avoid Ctrl+Alt+arrows on Linux, where desktops may reserve them.
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-alt-up", AddCursorAbove, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-alt-down", AddCursorBelow, Some(CONTEXT)),
        #[cfg(target_os = "windows")]
        KeyBinding::new("ctrl-alt-up", AddCursorAbove, Some(CONTEXT)),
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        KeyBinding::new("shift-alt-up", AddCursorAbove, Some(CONTEXT)),
        #[cfg(target_os = "windows")]
        KeyBinding::new("ctrl-alt-down", AddCursorBelow, Some(CONTEXT)),
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        KeyBinding::new("shift-alt-down", AddCursorBelow, Some(CONTEXT)),
        KeyBinding::new("home", MoveHome, Some(CONTEXT)),
        KeyBinding::new("end", MoveEnd, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-home", MoveToStart, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-end", MoveToEnd, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-shift-home", SelectToStart, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-shift-end", SelectToEnd, Some(CONTEXT)),
        KeyBinding::new("shift-home", SelectToStartOfLine, Some(CONTEXT)),
        KeyBinding::new("shift-end", SelectToEndOfLine, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("ctrl-shift-a", SelectToStartOfLine, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("ctrl-shift-e", SelectToEndOfLine, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("shift-cmd-left", SelectToStartOfLine, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("shift-cmd-right", SelectToEndOfLine, Some(CONTEXT)),
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        KeyBinding::new("alt-shift-left", SelectToPreviousWordStart, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-shift-left", SelectToPreviousWordStart, Some(CONTEXT)),
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        KeyBinding::new("alt-shift-right", SelectToNextWordEnd, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-shift-right", SelectToNextWordEnd, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("ctrl-cmd-space", ShowCharacterPalette, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-a", SelectAll, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-a", SelectAll, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-c", Copy, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-c", Copy, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-x", Cut, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-x", Cut, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-v", Paste, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-v", Paste, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("ctrl-a", MoveHome, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-left", MoveHome, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("ctrl-e", MoveEnd, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-right", MoveEnd, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-z", Undo, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-shift-z", Redo, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-up", MoveToStart, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-down", MoveToEnd, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("alt-left", MoveToPreviousWord, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("alt-right", MoveToNextWord, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-left", MoveToPreviousWord, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-right", MoveToNextWord, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-shift-up", SelectToStart, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-shift-down", SelectToEnd, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-z", Undo, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-y", Redo, Some(CONTEXT)),
    ]);
}

/// A mouse position resolved for a columnar (block) selection.
///
/// A position past the end of a short row has no glyph to land on, so `offset` clips to
/// that row's end and `columns_past_line_end` keeps what the clip dropped. Together they
/// say which column the pointer really reached, which is what a block spanning rows of
/// different lengths has to be measured in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ColumnarPoint {
    offset: usize,
    columns_past_line_end: usize,
}

impl ColumnarPoint {
    pub(super) fn new(offset: usize, columns_past_line_end: usize) -> Self {
        Self {
            offset,
            columns_past_line_end,
        }
    }
}

/// The document and selections a paste was asked to replace, taken when the
/// `Paste` action ran so a clipboard read that resolves later can tell
/// whether it still applies. See [`InputBaseState::paste_target`].
#[derive(Debug, PartialEq)]
struct PasteTarget {
    document_revision: u64,
    selections: Vec<CursorSelection>,
}

/// Caret positioning geometry from the input's latest prepaint.
///
/// Unlike `last_layout` (published at paint time), this is stashed during
/// prepaint so caret-anchored popups can position themselves from the
/// current frame's geometry in their deferred prepaint, instead of the
/// previous frame's paint.
#[derive(Clone, Copy, Debug)]
#[doc(hidden)]
pub struct PrepaintCaretGeometry {
    cursor_bounds: Bounds<Pixels>,
    line_height: Pixels,
    input_bounds: Bounds<Pixels>,
    scroll_offset: Point<Pixels>,
}

impl PrepaintCaretGeometry {
    pub(super) fn new(
        cursor_bounds: Bounds<Pixels>,
        line_height: Pixels,
        input_bounds: Bounds<Pixels>,
        scroll_offset: Point<Pixels>,
    ) -> Self {
        Self {
            cursor_bounds,
            line_height,
            input_bounds,
            scroll_offset,
        }
    }

    /// The active caret's bounds from the fresh prepaint layout.
    pub fn cursor_bounds(&self) -> Bounds<Pixels> {
        self.cursor_bounds
    }

    /// Line height from the fresh prepaint layout.
    pub fn line_height(&self) -> Pixels {
        self.line_height
    }

    /// The input container bounds from the fresh prepaint.
    pub fn input_bounds(&self) -> Bounds<Pixels> {
        self.input_bounds
    }

    /// The scroll offset that will be in effect after paint, clamped like
    /// `InputBaseState::update_scroll_offset` clamps it.
    pub fn scroll_offset(&self) -> Point<Pixels> {
        self.scroll_offset
    }
}

/// The shared text-editing engine behind [`crate::input::InputState`] and
/// [`crate::input::TextareaState`].
pub struct InputBaseState<M: InputModeKind> {
    _mode_marker: std::marker::PhantomData<M>,
    pub(super) focus_handle: FocusHandle,
    selection_focus: Option<WeakFocusHandle>,
    pub(super) mode: LayoutMode,
    pub(super) text: Rope,
    pub(super) display_map: DisplayMap,
    pub(super) undo_manager: UndoManager,
    pub(super) inline_tokens: Option<Box<super::inline_tokens::InlineTokenStore>>,
    pub(super) pending_token: Option<super::InlineToken>,
    pub(super) replaying_history: bool,
    pub(super) validated_token_edit: bool,
    pub(super) document_revision: u64,
    pub(super) token_presentation: super::InlineTokenPresentation,
    pub(super) token_layout_cache: Option<Box<super::token_presentation::TokenLayoutCache>>,
    /// Real per-frame bounds of the placed token elements, keyed by token
    /// start offset; used for hover payloads and stale-hover reconciliation.
    pub(super) token_bounds: std::collections::HashMap<usize, Bounds<Pixels>>,
    /// The currently hovered token: its entered span, last measured bounds,
    /// and UTF-16 range as of entry. Retained so hover exit can still be
    /// delivered when the token is removed, replaced, scrolled out, or
    /// disabled.
    pub(super) hovered_token: Option<super::token_presentation::HoverSnapshot>,
    pub(super) pending_token_hover_exits: Vec<super::token_presentation::HoverSnapshot>,
    /// The start offset of a pressed token, with the document revision and
    /// pointer position at the press.
    pub(super) pressed_token: Option<(usize, u64, Point<Pixels>)>,
    pub(super) soft_wrap: bool,
    pub(super) wrapping_indent: WrappingIndent,
    pub(super) cursor_surrounding_lines: Option<usize>,
    pub(super) blink_cursor: Entity<BlinkCursor>,
    pub(super) loading: bool,
    /// The cursors and selections.
    ///
    /// Always contains at least one selection where index 0 is the active cursor.
    pub(super) selections: Selections,
    /// Range for save the selected word, use to keep word range when drag move.
    pub(super) selected_word_range: Option<CursorSelection>,
    /// The marked range is the temporary insert text on IME typing.
    pub(super) ime_marked_range: Option<CursorSelection>,
    pub(super) last_layout: Option<LastLayout>,
    /// Fresh caret geometry stashed during prepaint for caret-anchored popups.
    pub(crate) prepaint_caret_geometry: Option<PrepaintCaretGeometry>,
    pub(super) last_cursor: Option<usize>,
    /// The input container bounds
    pub(super) input_bounds: Bounds<Pixels>,
    /// The text bounds
    pub(super) last_bounds: Option<Bounds<Pixels>>,
    pub(super) last_selected_range: Option<CursorSelection>,
    pub(super) selecting: bool,
    /// Anchor point of an in-progress columnar (block) selection.
    pub(super) column_select_start: Option<ColumnarPoint>,
    /// The selection a long press made, with its handles and edit menu.
    pub(super) touch_selection: super::touch::TouchSelection,
    pub(crate) disabled: bool,
    pub(crate) readonly: bool,
    pub(crate) text_align: TextAlign,
    pub(super) masked: bool,
    pub(super) clean_on_escape: bool,
    pub(super) submit_on_enter: bool,
    /// This flag tells the renderer to prefer the end of the current visual line.
    pub(crate) cursor_line_end_affinity: bool,
    pub(super) validate: Option<Box<dyn Fn(&str, &mut App) -> bool + 'static>>,
    pub(crate) scroll_handle: ScrollHandle,
    /// The deferred scroll offset to apply on next layout.
    pub(crate) deferred_scroll_offset: Option<Point<Pixels>>,
    /// The size of the scrollable content.
    pub(crate) scroll_size: gpui::Size<Pixels>,
    pub(super) editor_scrollbar_snapshot: Cell<Option<EditorScrollbarSnapshot>>,
    viewport_wheel_step: bool,
    /// The unwrapped width of the longest line and what it was measured for.
    pub(super) longest_line_width: Cell<Option<(LongestLineKey, Pixels)>>,
    pub(super) editor_paddings: Edges<Pixels>,
    /// The style this state paints with, resolved from the current theme on render.
    pub(super) editor_style: InputEditorStyle,
    /// What a consumer projected, kept verbatim so resolution never consumes it.
    projected_editor_style: InputEditorStyle,

    /// The mask pattern for formatting the input text
    pub(crate) mask_pattern: MaskPattern,
    pub(super) mask_pattern_set: bool,
    pub(super) placeholder: SharedString,

    context_menu_handler: Option<
        Rc<dyn Fn(NativeMenu, InputContextMenuCapabilities, Point<Pixels>, &mut Window, &mut App)>,
    >,
    pending_context_menu: Option<(Point<Pixels>, usize)>,

    /// Whether the context menu that shows on right-click is enabled.
    pub(super) enable_context_menu: bool,

    pub(super) silent_replace_text: bool,
    /// A flag to indicate if we should emit InputEvents.
    pub(super) emit_events: bool,

    _subscriptions: Vec<Subscription>,
    _window_subscriptions: Vec<Subscription>,

    pub(super) auto_scroll: AutoScroll,
}

/// Read-only styling data exposed to presentation facades.
#[derive(Clone)]
pub struct InputPresentation {
    focus_handle: FocusHandle,
    disabled: bool,
    readonly: bool,
    loading: bool,
    masked: bool,
    multi_line: bool,
    text_align: TextAlign,
    placeholder: SharedString,
    mask_placeholder: Option<String>,
}

impl InputPresentation {
    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus_handle
    }

    pub fn is_disabled(&self) -> bool {
        self.disabled
    }

    pub fn is_readonly(&self) -> bool {
        self.readonly
    }

    /// Returns true if the user is allowed to change the text.
    ///
    /// See also: [`InputBaseState::is_editable`].
    pub fn is_editable(&self) -> bool {
        !self.disabled && !self.readonly
    }

    pub fn is_loading(&self) -> bool {
        self.loading
    }

    pub fn is_masked(&self) -> bool {
        self.masked
    }

    pub fn is_multi_line(&self) -> bool {
        self.multi_line
    }

    pub fn text_align(&self) -> TextAlign {
        self.text_align
    }

    pub fn placeholder(&self) -> &SharedString {
        &self.placeholder
    }

    /// The placeholder derived from the mask pattern, e.g.: `(___) ___-____`.
    pub fn mask_placeholder(&self) -> Option<&str> {
        self.mask_placeholder.as_deref()
    }
}

impl<M: InputModeKind> EventEmitter<InputEvent> for InputBaseState<M> {}

impl<M: InputModeKind> InputBaseState<M> {
    #[doc(hidden)]
    pub fn cursor_layout(&self) -> Option<(Bounds<Pixels>, Pixels)> {
        let layout = self.last_layout.as_ref()?;
        Some((layout.cursor_bounds?, layout.line_height))
    }

    /// Caret geometry from the latest prepaint, for caret-anchored popups.
    #[doc(hidden)]
    pub fn prepaint_caret_geometry(&self) -> Option<PrepaintCaretGeometry> {
        self.prepaint_caret_geometry
    }

    pub fn input_bounds(&self) -> Bounds<Pixels> {
        self.input_bounds
    }

    pub fn text_bounds(&self) -> Option<Bounds<Pixels>> {
        self.last_bounds
    }

    pub fn presentation(&self) -> InputPresentation {
        InputPresentation {
            focus_handle: self.focus_handle.clone(),
            disabled: self.disabled,
            readonly: self.readonly,
            loading: self.loading,
            masked: self.masked,
            multi_line: self.is_multi_line(),
            text_align: self.text_align,
            placeholder: self.placeholder.clone(),
            mask_placeholder: self.mask_pattern.placeholder(),
        }
    }

    /// Whether this input spans more than one line.
    ///
    /// Answered by the mode marker, which is fixed when the state is built.
    /// [`LayoutMode`] holds the row counts and growth policy, not the kind.
    #[inline]
    /// Whether this input paints scrollbars.
    ///
    /// Only a multi-line input can scroll: a single-line input keeps its
    /// caret in view by moving its own offset, and never has a viewport a
    /// user could drag. Adding the editor scrollbar to every input put a
    /// thumb inside every text field, which is a control the field does not
    /// have.
    pub(crate) fn shows_scrollbar(&self) -> bool {
        self.is_multi_line()
    }

    pub fn viewport_wheel_step(mut self) -> Self {
        self.viewport_wheel_step = true;
        self
    }

    pub fn is_multi_line(&self) -> bool {
        M::MULTI_LINE
    }

    /// Whether this input is a single-line text field. See [`Self::is_multi_line`].
    #[inline]
    pub fn is_single_line(&self) -> bool {
        !M::MULTI_LINE
    }

    /// Whether the user is allowed to copy the selection out.
    ///
    /// A masked input keeps its value out of the clipboard.
    pub fn is_copyable(&self) -> bool {
        self.selections.iter().any(|sel| !sel.is_empty()) && !self.masked
    }

    fn window_subscriptions(
        focus_handle: &FocusHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<Subscription> {
        vec![
            // Blink the cursor when the window is active, pause when it's not.
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

    pub fn attach_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self._window_subscriptions = Self::window_subscriptions(&self.focus_handle, window, cx);
    }

    pub fn can_undo(&self) -> bool {
        self.undo_manager.has_undos()
    }

    pub fn begin_save(&mut self) -> u64 {
        self.undo_manager.begin_save()
    }

    pub fn mark_saved_version(&mut self, version: u64) {
        self.undo_manager.mark_saved_version(version);
    }

    pub fn is_clean(&self) -> bool {
        self.undo_manager.is_clean()
    }

    pub fn context_menu_capabilities(&self) -> InputContextMenuCapabilities {
        InputContextMenuCapabilities::new()
            .disabled(self.disabled)
            .readonly(self.readonly)
            .selection(!self.active_selection().is_empty())
            .masked(self.masked)
    }

    pub fn set_text_align(&mut self, text_align: TextAlign, cx: &mut Context<Self>) {
        if !self.is_single_line() || self.text_align == text_align {
            return;
        }

        self.text_align = text_align;
        cx.notify();
    }

    /// Flip the password mask.
    ///
    /// Setting the mask is a single-line method, but flipping it stays here:
    /// the reveal button is rendered from the generic path, and it can only be
    /// switched on through [`crate::input::InputState`] anyway.
    pub fn toggle_masked(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.masked = !self.masked;
        cx.notify();
    }

    pub fn on_context_menu(
        &mut self,
        handler: Rc<
            dyn Fn(NativeMenu, InputContextMenuCapabilities, Point<Pixels>, &mut Window, &mut App),
        >,
    ) {
        self.context_menu_handler = Some(handler);
    }

    /// Keeps the selection highlighted while this popup or one of its children has focus.
    ///
    /// This does not transfer keyboard focus or keep the caret visible. The
    /// association holds only a weak focus handle. Pass `None` to clear it.
    pub fn set_selection_focus(&mut self, focus: Option<FocusHandle>, cx: &mut Context<Self>) {
        self.selection_focus = focus.as_ref().map(FocusHandle::downgrade);
        cx.notify();
    }

    /// Whether the input or its associated selection popup has focus.
    pub fn has_selection_focus(&self, window: &Window, cx: &App) -> bool {
        self.focus_handle.is_focused(window)
            || self
                .selection_focus
                .as_ref()
                .and_then(WeakFocusHandle::upgrade)
                .is_some_and(|focus| focus.is_focused(window) || focus.contains_focused(window, cx))
    }

    /// Build the engine. Each mode's own `new` sets its layout on top of this.
    fn new_in_mode(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle().tab_stop(true);
        let blink_cursor = cx.new(|_| BlinkCursor::new());
        let undo_manager = UndoManager::new();

        let _subscriptions = vec![
            // Key bindings can consume events before on_key_down. Observe input
            // before action dispatch so every keystroke resets the blink delay.
            cx.intercept_keystrokes({
                let focus_handle = focus_handle.clone();
                let blink_cursor = blink_cursor.downgrade();
                move |_, window, cx| {
                    if focus_handle.is_focused(window) {
                        _ = blink_cursor.update(cx, |cursor, cx| cursor.pause(cx));
                    }
                }
            }),
            // Observe the blink cursor to repaint the view when it changes.
            cx.observe(&blink_cursor, |_, _, cx| cx.notify()),
        ];
        let _window_subscriptions = Self::window_subscriptions(&focus_handle, window, cx);

        let text_style = window.text_style();

        Self {
            _mode_marker: std::marker::PhantomData,
            focus_handle: focus_handle.clone(),
            selection_focus: None,
            text: "".into(),
            display_map: DisplayMap::new(text_style.font(), window.rem_size(), None),
            soft_wrap: true,
            wrapping_indent: WrappingIndent::default(),
            cursor_surrounding_lines: None,
            blink_cursor,
            undo_manager,
            inline_tokens: None,
            pending_token: None,
            replaying_history: false,
            validated_token_edit: false,
            document_revision: 0,
            token_presentation: Default::default(),
            token_bounds: Default::default(),
            hovered_token: None,
            pending_token_hover_exits: Vec::new(),
            token_layout_cache: None,
            pressed_token: None,
            selections: Selections::default(),
            selected_word_range: None,
            ime_marked_range: None,
            input_bounds: Bounds::default(),
            selecting: false,
            touch_selection: Default::default(),
            disabled: false,
            readonly: false,
            text_align: TextAlign::Left,
            masked: false,
            clean_on_escape: false,
            submit_on_enter: false,
            loading: false,
            validate: None,
            mode: LayoutMode::default(),
            last_layout: None,
            prepaint_caret_geometry: None,
            last_bounds: None,
            last_selected_range: None,
            column_select_start: None,
            last_cursor: None,
            scroll_handle: ScrollHandle::new(),
            scroll_size: gpui::size(px(0.), px(0.)),
            editor_scrollbar_snapshot: Cell::new(None),
            viewport_wheel_step: false,
            longest_line_width: Cell::new(None),
            editor_paddings: Edges::default(),
            deferred_scroll_offset: None,
            placeholder: SharedString::default(),
            mask_pattern: MaskPattern::default(),
            mask_pattern_set: false,
            editor_style: InputEditorStyle::default(),
            projected_editor_style: InputEditorStyle::default(),
            context_menu_handler: None,
            pending_context_menu: None,
            enable_context_menu: true,
            silent_replace_text: false,
            emit_events: true,
            _subscriptions,
            _window_subscriptions,
            cursor_line_end_affinity: false,
            auto_scroll: AutoScroll::default(),
        }
    }

    /// Sets whether the context menu that shows on right-click is enabled.
    pub fn context_menu(mut self, enable: bool) -> Self {
        self.enable_context_menu = enable;
        self
    }

    pub fn set_context_menu_enabled(&mut self, enabled: bool) {
        self.enable_context_menu = enabled;
    }

    /// Set placeholder
    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    pub fn set_editor_style(&mut self, style: InputEditorStyle) {
        self.editor_style = style.clone();
        self.projected_editor_style = style;
    }

    /// Set presentation padding for multi-line text and its scrollbar layout.
    #[doc(hidden)]
    pub fn set_editor_paddings(&mut self, paddings: Edges<Pixels>) {
        self.editor_paddings = paddings;
    }

    /// Set placeholder
    pub fn set_placeholder(
        &mut self,
        placeholder: impl Into<SharedString>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.placeholder = placeholder.into();
        cx.notify();
    }

    /// Find which line and sub-line the given offset belongs to, along with the position within that sub-line.
    ///
    /// Returns:
    ///
    /// - The index of the line (zero-based) containing the offset.
    /// - The index of the sub-line (zero-based) within the line containing the offset.
    /// - The position of the offset.
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
                let adjusted_pos = point(pos.x + last_layout.line_number_width, pos.y + y_offset);
                return (vi, sub_line_index, Some(adjusted_pos));
            }

            y_offset += line.size(line_height).height;
        }
        (0, 0, None)
    }

    /// Set the value of the input field: plain text, or [`InputContent`] to
    /// restore text together with its inline tokens. Editing history is
    /// cleared and no [`InputEvent::Change`] is emitted.
    ///
    /// For single-line inputs the caret is placed at the end of the text while
    /// the view is scrolled back to the start, so a long value shows its
    /// beginning instead of its tail (matching HTML `<input>`). Multi-line
    /// inputs reset the selection to `0..0`.
    pub fn set_value(
        &mut self,
        value: impl Into<super::InputContent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let content = value.into();
        self.inline_tokens = None;
        self.undo_manager.set_ignoring(true);
        self.emit_events = false;
        self.replace_text(content.text().clone(), window, cx);
        self.install_tokens(content);
        self.undo_manager.set_ignoring(false);
        self.emit_events = true;

        self.reset_selection();
        self.reset_scroll_to_start();

        self.undo_manager.clear();
        cx.notify();
    }

    /// Replace the entire text content while preserving undo history.
    ///
    /// Unlike [`set_value`](Self::set_value), this method records the
    /// replacement in the undo stack, allowing the user to undo/redo
    /// the change. The selection is placed at the end of the new text
    /// for single-line inputs, or cleared (0..0) for multi-line inputs.
    ///
    /// Use this when programmatically replacing the full text but the
    /// user should still be able to undo the operation — e.g. formatting.
    pub fn replace_all(
        &mut self,
        text: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.replace_text(text, window, cx);
        self.reset_selection();
        self.reset_scroll_to_start();

        cx.notify();
    }

    /// Perform `f` with the user-facing edit restrictions lifted.
    ///
    /// The `disabled` and `readonly` modes only reject the changes made by the
    /// user, the programmatic APIs must always be able to update the text.
    pub(super) fn with_edits_allowed(&mut self, f: impl FnOnce(&mut Self)) {
        let (was_disabled, was_readonly) = (self.disabled, self.readonly);
        (self.disabled, self.readonly) = (false, false);
        f(self);
        (self.disabled, self.readonly) = (was_disabled, was_readonly);
    }

    /// Insert text at the current cursor position.
    ///
    /// And the cursor will be moved to the end of inserted text.
    pub fn insert(
        &mut self,
        text: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text: SharedString = text.into();
        self.with_edits_allowed(|this| {
            this.undo_manager.set_pending_intent(EditIntent::Atomic);
            let range_utf16 = this.range_to_utf16(&(this.cursor()..this.cursor()));
            this.replace_text_in_range_silent(Some(range_utf16), &text, window, cx);
            let end = this.active_selection().end;
            this.set_cursor_to(end);
        });
    }

    /// Replace text at the current cursor position.
    ///
    /// And the cursor will be moved to the end of replaced text.
    pub fn replace(
        &mut self,
        text: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text: SharedString = text.into();
        self.with_edits_allowed(|this| {
            this.undo_manager.set_pending_intent(EditIntent::Atomic);
            this.replace_text_in_range_silent(None, &text, window, cx);
            let end = this.active_selection().end;
            this.set_cursor_to(end);
        });
    }

    pub fn replace_ranges(
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
        self.with_edits_allowed(|this| {
            this.undo_manager.set_pending_intent(EditIntent::Atomic);
            let range = 0..this.text.chars().map(|c| c.len_utf16()).sum();
            this.replace_text_in_range_silent(Some(range), &text, window, cx);
        });
    }

    fn reset_selection(&mut self) {
        self.selections.remove_all_but_active();

        // For single-line inputs the caret is placed at the end of the text
        // (matching HTML `<input>`); multi-line inputs reset the selection to
        // `0..0`.
        if self.is_single_line() {
            let end = self.text.len();
            self.set_cursor_to(end);
        } else {
            self.active_selection_mut().clear();
        }
    }

    fn reset_scroll_to_start(&mut self) {
        // Move scroll to the start. For single-line the caret is at the end, so
        // override the cursor-follow scroll for the next painted frame to keep
        // the start visible; the deferred offset is consumed during that paint.
        self.scroll_handle.set_offset(point(px(0.), px(0.)));
        if self.is_single_line() {
            self.deferred_scroll_offset = Some(point(px(0.), px(0.)));
        }
    }

    /// Set with disabled mode.
    ///
    /// See also: [`Self::set_disabled`].
    #[allow(unused)]
    pub(crate) fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn set_disabled(&mut self, disabled: bool, cx: &mut Context<Self>) {
        if self.disabled == disabled {
            return;
        }

        self.disabled = disabled;
        cx.notify();
    }

    /// Set with read-only mode.
    ///
    /// Unlike [`Self::disabled`], a read-only input keeps the normal appearance,
    /// focus, cursor, selection and copy behavior, it only rejects any change
    /// of the text made by the user.
    ///
    /// See also: [`Self::set_readonly`].
    #[allow(unused)]
    pub(crate) fn readonly(mut self, readonly: bool) -> Self {
        self.readonly = readonly;
        self
    }

    pub fn set_readonly(&mut self, readonly: bool, cx: &mut Context<Self>) {
        if self.readonly == readonly {
            return;
        }

        self.readonly = readonly;
        cx.notify();
    }

    /// Returns true if the user is allowed to change the text.
    ///
    /// This is false when the input is `disabled` or `readonly`, the programmatic
    /// APIs (e.g.: [`Self::set_value`], [`Self::insert`]) are not limited by this.
    pub fn is_editable(&self) -> bool {
        !self.disabled && !self.readonly
    }

    /// Set true to clear the input by pressing Escape key.
    pub fn clean_on_escape(mut self) -> Self {
        self.clean_on_escape = true;
        self
    }

    pub fn set_clean_on_escape(&mut self, clean: bool) {
        self.clean_on_escape = clean;
    }

    /// Set true to treat `Enter` as a submit action in multi-line mode,
    /// while `Shift+Enter` inserts a newline.
    ///
    /// Default is `false` (both `Enter` and `Shift+Enter` insert a newline).
    #[doc(hidden)]
    pub fn submit_on_enter(mut self, submit: bool) -> Self {
        self.submit_on_enter = submit;
        self
    }

    pub fn set_submit_on_enter(&mut self, submit: bool, cx: &mut Context<Self>) {
        self.submit_on_enter = submit;
        cx.notify();
    }

    /// Minimum number of lines the cursor is kept clear of the viewport's
    /// top/bottom edge before auto-scroll engages.
    pub fn cursor_surrounding_lines(mut self, lines: Option<usize>) -> Self {
        self.cursor_surrounding_lines = lines;
        self
    }

    /// Update [`Self::cursor_surrounding_lines`] after construction.
    pub fn set_cursor_surrounding_lines(
        &mut self,
        lines: Option<usize>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.cursor_surrounding_lines == lines {
            return;
        }
        self.cursor_surrounding_lines = lines;
        cx.notify();
    }

    /// Set the default value of the input field.
    pub fn default_value(mut self, value: impl Into<SharedString>) -> Self {
        let text: SharedString = value.into();
        self.text = Rope::from(self.normalize_input(&text).as_ref());
        self
    }

    /// Return the value of the input field as an owned string.
    ///
    /// The string is materialized on each call. See [`Self::text`] for the
    /// [`Rope`] the state owns, which is borrowed and costs nothing to read.
    pub fn value(&self) -> SharedString {
        SharedString::new(self.text.to_string())
    }

    /// Return the portion of the value within the input field that
    /// is selected by the user, as an owned string.
    ///
    /// The string is materialized on each call. See [`Self::selected_text`]
    /// for the same selection borrowed out of the [`Rope`] the state owns.
    pub fn selected_value(&self) -> SharedString {
        SharedString::new(self.selected_text().to_string())
    }

    /// Return the value without mask.
    pub fn unmask_value(&self) -> SharedString {
        self.mask_pattern.unmask(&self.text.to_string()).into()
    }

    /// Kept so existing render paths keep compiling.
    ///
    /// Configuration used to be collected by a facade and applied here; the
    /// state now configures itself, so this does nothing and can be deleted at
    /// the call site.
    #[doc(hidden)]
    pub fn prepare(&mut self, _: &mut Window, _: &mut Context<Self>) {}

    /// Return the text [`Rope`] of the input field.
    ///
    /// Borrowed from the state, so reading even a large document copies
    /// nothing. See [`Self::value`] when an owned string is wanted.
    pub fn text(&self) -> &Rope {
        &self.text
    }

    /// Return the (0-based) [`Position`] of the cursor.
    pub fn cursor_position(&self) -> Position {
        let offset = self.cursor();
        self.text.offset_to_position(offset)
    }

    /// Set (0-based) [`Position`] of the cursor.
    ///
    /// This will move the cursor to the specified line and column, and update the selection range.
    pub fn set_cursor_position(
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

    /// Focus the input field.
    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_handle.focus(window, cx);
        self.blink_cursor.update(cx, |cursor, cx| {
            cursor.start(cx);
        });
    }

    /// Request a redraw of the input.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        cx.notify();
    }

    pub(super) fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.undo_manager.break_transaction_coalescing();
        self.select_all_cursors_to(|s, sel| s.previous_boundary(sel.cursor_offset()), cx);
    }

    pub(super) fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_all_cursors_to(|s, sel| s.next_boundary(sel.cursor_offset()), cx);
    }

    pub(super) fn select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_single_line() {
            return;
        }
        self.select_all_cursors_to_with_affinity(
            |s, sel| {
                s.vertical_selection_target(
                    sel.cursor_offset(),
                    sel.column_anchor,
                    s.line_end_affinity_for(sel),
                    -1,
                )
            },
            true,
            cx,
        );
    }

    pub(super) fn select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_single_line() {
            return;
        }
        self.select_all_cursors_to_with_affinity(
            |s, sel| {
                s.vertical_selection_target(
                    sel.cursor_offset(),
                    sel.column_anchor,
                    s.line_end_affinity_for(sel),
                    1,
                )
            },
            true,
            cx,
        );
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
        self.undo_manager.break_transaction_coalescing();
        self.select_all_cursors_to(|_, _| 0, cx);
    }

    pub(super) fn select_to_end(
        &mut self,
        _: &SelectToEnd,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.undo_manager.break_transaction_coalescing();
        let end = self.text.len();
        self.select_all_cursors_to(move |_, _| end, cx);
    }

    pub(super) fn select_to_start_of_line(
        &mut self,
        _: &SelectToStartOfLine,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.undo_manager.break_transaction_coalescing();
        self.select_all_cursors_to(
            |s, sel| s.start_of_line_at(sel.cursor_offset(), s.line_end_affinity_for(sel)),
            cx,
        );
    }

    pub(super) fn select_to_end_of_line(
        &mut self,
        _: &SelectToEndOfLine,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.undo_manager.break_transaction_coalescing();
        self.select_all_cursors_to_with_affinity(
            |s, sel| {
                (
                    s.end_of_line_at(sel.cursor_offset(), s.line_end_affinity_for(sel)),
                    true,
                )
            },
            false,
            cx,
        );
    }

    pub(super) fn select_to_previous_word(
        &mut self,
        _: &SelectToPreviousWordStart,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.undo_manager.break_transaction_coalescing();
        self.select_all_cursors_to(
            |s, sel| s.previous_start_of_word_at(sel.cursor_offset()),
            cx,
        );
    }

    pub(super) fn select_to_next_word(
        &mut self,
        _: &SelectToNextWordEnd,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.undo_manager.break_transaction_coalescing();
        self.select_all_cursors_to(|s, sel| s.next_end_of_word_at(sel.cursor_offset()), cx);
    }

    /// Return the start offset of the previous word.
    /// Return the previous start offset of the word before `offset`.
    pub(super) fn previous_start_of_word_at(&self, offset: usize) -> usize {
        if self.masked {
            // The mask replaces every character, so the displayed text has no
            // word boundaries to move or delete by. Collapse the word to the
            // whole text.
            return 0;
        }

        let offset = self.offset_from_utf16(self.offset_to_utf16(offset));
        // FIXME: Avoid to_string
        let left_part = self.text.slice(0..offset).to_string();

        let target = UnicodeSegmentation::split_word_bound_indices(left_part.as_str())
            .rfind(|(_, s)| !s.trim_start().is_empty())
            .map(|(i, _)| i)
            .unwrap_or(0);
        self.token_boundary(target, Bias::Left)
    }

    /// Return the next end offset of the word after `offset`.
    pub(super) fn next_end_of_word_at(&self, offset: usize) -> usize {
        if self.masked {
            // See `previous_start_of_word_at`.
            return self.text.len();
        }

        let offset = self.offset_from_utf16(self.offset_to_utf16(offset));
        let right_part = self.text.slice(offset..self.text.len()).to_string();

        let target = UnicodeSegmentation::split_word_bound_indices(right_part.as_str())
            .find(|(_, s)| !s.trim_start().is_empty())
            .map(|(i, s)| offset + i + s.len())
            .unwrap_or(self.text.len());
        self.token_boundary(target, Bias::Right)
    }

    /// Get start of line byte offset for the given `offset`.
    pub(super) fn start_of_line_at(&self, offset: usize, _line_end_affinity: bool) -> usize {
        if self.is_single_line() {
            return 0;
        }

        let row = self.text.offset_to_point(offset).row;
        self.text.line_start_offset(row)
    }

    /// Get end of line byte offset for the given `offset`.
    pub(super) fn end_of_line_at(&self, offset: usize, _line_end_affinity: bool) -> usize {
        if self.is_single_line() {
            return self.text.len();
        }

        let row = self.text.offset_to_point(offset).row;
        self.text.line_end_offset(row)
    }

    /// Delete every selection as one batch. Collapsed cursors are first
    /// expanded to a deletion range by `collapsed_target` and non-empty
    /// selections delete their own range.
    ///
    /// `collapsed_intent` is the intent to record when every cursor is
    /// collapsed, which is what makes a run of single-character deletes undo
    /// as one gesture. Deleting a real selection is always atomic.
    fn delete_selections(
        &mut self,
        silent: bool,
        collapsed_intent: EditIntent,
        mut collapsed_target: impl FnMut(&mut Self, usize) -> Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.is_editable() {
            return;
        }
        let cursors: Vec<CursorSelection> = self.selections.iter().copied().collect();
        let intent = if cursors.iter().all(|sel| sel.is_empty()) {
            collapsed_intent
        } else {
            EditIntent::Atomic
        };
        let mut new_selections: Vec<CursorSelection> = Vec::with_capacity(cursors.len());
        for sel in &cursors {
            let range = if sel.is_empty() {
                collapsed_target(self, sel.cursor_offset())
            } else {
                sel.start..sel.end
            };
            let (start, end) = (range.start.min(range.end), range.start.max(range.end));
            let mut selection = *sel;
            selection.start = start;
            selection.end = end;
            new_selections.push(selection);
        }
        // Capture the user's selections before expanding or merging deletion
        // ranges. The edit ranges cannot reconstruct their original carets.
        self.undo_manager.begin_transaction_with(intent);
        self.undo_manager
            .record_selections(cursors.clone(), cursors.clone());
        self.selections.replace_all(new_selections);
        self.undo_manager.set_pending_intent(intent);

        let was_silent = self.silent_replace_text;
        self.silent_replace_text = silent;
        self.replace_text_in_range(None, "", window, cx);
        self.silent_replace_text = was_silent;
        self.undo_manager
            .record_selections(cursors, self.selections.iter().copied().collect());
        self.undo_manager.commit_transaction();
        self.pause_blink_cursor(cx);
    }

    pub(super) fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        // Nothing to delete at the start of the text. Propagate so an ancestor
        // (e.g. a command palette navigating back a level) can act on it.
        // This is harmless when nothing upstream is bound to backspace. With multiple
        // cursors the others can still delete, so only the lone-cursor case
        // propagates.
        if self.selections.is_single() && self.active_selection().is_empty() && self.cursor() == 0 {
            cx.propagate();
            return;
        }

        self.delete_selections(
            false,
            EditIntent::Backspace,
            |s, offset| s.previous_boundary(offset)..offset,
            window,
            cx,
        );
    }

    pub(super) fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        self.delete_selections(
            false,
            EditIntent::DeleteForward,
            |s, offset| offset..s.next_boundary(offset),
            window,
            cx,
        );
    }

    pub(super) fn delete_to_beginning_of_line(
        &mut self,
        _: &DeleteToBeginningOfLine,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.delete_selections(
            true,
            EditIntent::Atomic,
            |s, offset| {
                let mut start = s.start_of_line_at(offset, s.line_end_affinity_at(offset));
                if start == offset {
                    start = start.saturating_sub(1);
                }
                start..offset
            },
            window,
            cx,
        );
    }

    pub(super) fn delete_to_end_of_line(
        &mut self,
        _: &DeleteToEndOfLine,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.delete_selections(
            true,
            EditIntent::Atomic,
            |s, offset| {
                let mut end = s.end_of_line_at(offset, s.line_end_affinity_at(offset));
                if end == offset {
                    end = (end + 1).clamp(0, s.text.len());
                }
                offset..end
            },
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
        self.delete_selections(
            true,
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
        self.delete_selections(
            true,
            EditIntent::Atomic,
            |s, offset| offset..s.next_end_of_word_at(offset),
            window,
            cx,
        );
    }

    pub(super) fn enter(&mut self, action: &Enter, window: &mut Window, cx: &mut Context<Self>) {
        // In multi-line mode with `submit_on_enter` enabled, a plain `Enter`
        // (without Shift) is treated as submit: propagate the action and emit
        // PressEnter without inserting a newline. `Shift+Enter` still inserts
        // a newline.
        let insert_newline = self.is_multi_line() && (!self.submit_on_enter || action.shift);

        if insert_newline {
            if !self.selections.is_single() {
                self.selections.merge_overlapping();
                let selections: Vec<CursorSelection> = self.selections.iter().copied().collect();
                let mut edits: Vec<(Range<usize>, String)> = Vec::with_capacity(selections.len());
                for sel in &selections {
                    edits.push((sel.start..sel.end, "\n".to_string()));
                }
                edits.sort_by_key(|(range, _)| range.start);
                self.replace_text_in_ranges(&edits, window, cx);
                self.pause_blink_cursor(cx);
            } else {
                self.replace_text_in_range_silent(None, "\n", window, cx);
                self.pause_blink_cursor(cx);
            }
        } else {
            // Single line input or submit-on-enter: just emit the event
            // (e.g.: in a dialog to confirm, or a chat textarea to send).
            self.undo_manager.break_transaction_coalescing();
            cx.propagate();
        }

        cx.emit(InputEvent::PressEnter {
            secondary: action.secondary,
            shift: action.shift,
        });
    }

    pub fn clean(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.replace_text("", window, cx);
        self.set_selection(0, 0);
        self.scroll_to(0, None, cx);
    }

    pub(super) fn escape(&mut self, _action: &Escape, window: &mut Window, cx: &mut Context<Self>) {
        // Collapse extra cursors back to the active one first.
        if !self.selections.is_single() {
            self.undo_manager.break_transaction_coalescing();
            self.selections.remove_all_but_active();
            cx.notify();
            return;
        }

        // The handles and the edit menu are the topmost surface to dismiss.
        if self.touch_selection().is_some() {
            self.dismiss_touch_selection(cx);
            return;
        }

        if self.ime_marked_range.is_some() {
            self.unmark_text(window, cx);
        }

        if self.clean_on_escape {
            return self.clean(window, cx);
        }

        cx.propagate();
    }

    /// Show the right-click context menu as a native OS menu.
    pub(crate) fn handle_right_click_menu(
        &mut self,
        position: Point<Pixels>,
        offset: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.disabled {
            return;
        }
        if crate::GlobalState::is_in_deferred_context(cx) {
            return;
        }

        if !self.active_selection().contains(offset) {
            self.move_to(offset, None, cx);
        }

        if let Some(handler) = self.context_menu_handler.clone() {
            let capabilities = self.context_menu_capabilities();
            let input = cx.weak_entity();
            // The handler runs with the input released, so a custom builder can
            // read it (cursor, selection, text) to build its items.
            window.defer(cx, move |window, cx| {
                if input.upgrade().is_none() {
                    return;
                }
                let previous_focus = window.focused(cx);
                handler(NativeMenu::new(), capabilities, position, window, cx);
                // A drawn menu takes focus synchronously; native menus leave it
                // on the input. Capture it here rather than in the handler.
                input
                    .update(cx, |this, cx| {
                        if window.focused(cx) == previous_focus
                            && !this.focus_handle.is_focused(window)
                            && this.has_selection_focus(window, cx)
                        {
                            // A custom handler can reuse an already-focused popup.
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

    pub(super) fn add_cursor_above(
        &mut self,
        _: &AddCursorAbove,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.add_cursor_vertical(-1, cx);
    }

    pub(super) fn add_cursor_below(
        &mut self,
        _: &AddCursorBelow,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.add_cursor_vertical(1, cx);
    }

    /// Add a new cursor one display line above (`move_lines < 0`) or below each
    /// existing cursor, preserving the column. Cursors that would not move (at
    /// the first/last display row) or that would duplicate an existing cursor
    /// are skipped.
    fn add_cursor_vertical(&mut self, move_lines: isize, cx: &mut Context<Self>) {
        if !self.is_multi_line() {
            return;
        }

        self.pause_blink_cursor(cx);
        // Changing the cursor set ends the editing gesture that came before it.
        self.undo_manager.break_transaction_coalescing();

        let sources: Vec<(usize, Option<(Pixels, usize)>, bool)> = self
            .selections
            .iter()
            .map(|sel| {
                (
                    sel.cursor_offset(),
                    sel.column_anchor,
                    self.line_end_affinity_for(sel),
                )
            })
            .collect();
        let mut offsets: std::collections::HashSet<usize> =
            sources.iter().map(|(offset, _, _)| *offset).collect();

        let mut newest: Option<usize> = None;
        for (offset, anchor, line_end_affinity) in sources {
            let anchor = anchor.or_else(|| self.preferred_column_for(offset));
            let (target, _) = self.vertical_target(offset, anchor, line_end_affinity, move_lines);
            if target == offset || offsets.contains(&target) {
                continue;
            }
            offsets.insert(target);
            let id = self.selections.generate_id();
            let mut cursor = CursorSelection::new(id, target, target);
            cursor.column_anchor = anchor;
            self.selections.add(cursor);
            newest = Some(target);
        }

        if let Some(newest) = newest {
            self.scroll_to(newest, None, cx);
        }
        cx.notify();
    }

    /// Add an additional collapsed cursor at `offset`.
    ///
    /// Rejected when `offset` lands inside an existing selection or exactly on
    /// an existing cursor.
    pub(super) fn add_cursor_at(&mut self, offset: usize, cx: &mut Context<Self>) {
        if !self.is_multi_line() {
            return;
        }

        for sel in self.selections.iter() {
            if sel.contains(offset) {
                return;
            }
            if sel.is_collapsed() && sel.cursor_offset() == offset {
                return;
            }
        }

        self.undo_manager.break_transaction_coalescing();
        let id = self.selections.generate_id();
        let mut selection = CursorSelection::new(id, offset, offset);
        selection.column_anchor = self.preferred_column_for(offset);
        self.selections.add(selection);
        cx.notify();
    }

    /// Build a columnar (block) selection spanning the rows between the two
    /// points, one selection per display row at the same column span.
    ///
    /// The span keeps the columns the pointer sat past the end of a short row, so a row
    /// that runs out of text does not narrow the block for the rows that do reach that
    /// far. Each row is then clipped to its own end, selecting as much of the span as it
    /// actually has.
    pub(super) fn build_columnar_selection(
        &mut self,
        start: ColumnarPoint,
        end: ColumnarPoint,
        cx: &mut Context<Self>,
    ) {
        if !self.is_multi_line() {
            return;
        }

        self.undo_manager.break_transaction_coalescing();

        let (start_row, start_col) = self.columnar_row_column(start);
        let (end_row, end_col) = self.columnar_row_column(end);
        let (start_row, end_row) = (start_row.min(end_row), start_row.max(end_row));
        let (start_col, end_col) = (start_col.min(end_col), start_col.max(end_col));

        let mut new_selections = Vec::with_capacity(end_row - start_row + 1);
        for row in start_row..=end_row {
            let sel_start = self
                .display_map
                .display_row_column_to_offset(row, start_col);
            let sel_end = self.display_map.display_row_column_to_offset(row, end_col);
            let id = self.selections.generate_id();
            let sel_start = self.text.clip_offset(sel_start, Bias::Left);
            let sel_end = self.text.clip_offset(sel_end, Bias::Left);
            new_selections.push(CursorSelection::new(id, sel_start, sel_end));
        }

        self.selections.replace_all(new_selections);
        cx.notify();
    }

    /// Resolve a columnar point to the display row it is on and the column it reaches,
    /// counting the columns beyond the row's end so a short row keeps the full span.
    fn columnar_row_column(&self, point: ColumnarPoint) -> (usize, usize) {
        let display_point = self.display_map.offset_to_wrap_display_point(point.offset);
        let row = self
            .display_map
            .wrap_row_to_display_row(display_point.row)
            .unwrap_or_else(|| {
                self.display_map
                    .nearest_visible_display_row(display_point.row)
            });

        (row, display_point.column + point.columns_past_line_end)
    }

    pub(super) fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A token that consumed the press (a double click selecting it) must
        // not also place the caret.
        if window.default_prevented() {
            return;
        }
        self.undo_manager.break_transaction_coalescing();
        // Input has its own text selection; suppress the window-level text
        // selection (Root) so it does not start a drag from here.
        crate::global_state::GlobalState::suppress_text_selection(cx);

        // A tap on the touch selection asks for its menu back. Any other press
        // places the caret or starts a new drag, and the handles and the menu
        // no longer belong to what is selected.
        if event.button == MouseButton::Left
            && event.click_count == 1
            && self.reopen_edit_menu_at(event.position, cx)
        {
            return;
        }
        self.dismiss_touch_selection(cx);

        // If there have IME marked range and is empty (Means pressed Esc to abort IME typing)
        // Clear the marked range.
        if let Some(ime_marked_range) = &self.ime_marked_range {
            if ime_marked_range.len() == 0 {
                self.ime_marked_range = None;
            }
        }

        self.selecting = true;
        let (offset, line_end_affinity, columns_past_line_end) =
            self.resolve_mouse_position(event.position);

        // Triple click to select line
        if event.button == MouseButton::Left && event.click_count >= 3 {
            self.select_line(offset, window, cx);
            return;
        }

        // Double click to select word
        if event.button == MouseButton::Left && event.click_count == 2 {
            self.select_word(offset, window, cx);
            // A double tap is touch's other way to select a word, and it
            // gets the handles and the menu like a long press does.
            if crate::GlobalState::is_touch_press(cx) {
                self.keep_touch_selection(cx);
            }
            return;
        }

        // Show Mouse context menu
        if event.button == MouseButton::Right {
            if self.enable_context_menu {
                if !self.active_selection().contains(offset) {
                    self.move_to(offset, None, cx);
                }
                self.pending_context_menu = Some((event.position, offset));
            }
            return;
        }

        // Multi-cursor placement, multi-line only.
        if self.is_multi_line() && event.button == MouseButton::Left {
            if event.modifiers.alt
                && (event.modifiers.shift || (cfg!(target_os = "linux") && event.modifiers.control))
            {
                // Alt+Shift starts a block; Linux also accepts Ghostty's Ctrl+Alt.
                // Mark selecting so the drag handler extends the block.
                self.column_select_start = Some(ColumnarPoint::new(offset, columns_past_line_end));
                self.selecting = true;
                self.move_to_with_affinity(offset, None, line_end_affinity, cx);
                return;
            } else if event.modifiers.alt {
                self.add_cursor_at(offset, cx);
                // Keep click-to-add behavior, but use this press as the block
                // anchor if the user continues dragging with the left button.
                self.column_select_start = Some(ColumnarPoint::new(offset, columns_past_line_end));
                return;
            }
        }

        if event.modifiers.shift {
            self.select_to_with_affinity(offset, line_end_affinity, cx);
        } else {
            self.move_to_with_affinity(offset, None, line_end_affinity, cx)
        }
    }

    pub(super) fn on_mouse_up(
        &mut self,
        event: &MouseUpEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.button == MouseButton::Right {
            if let Some((position, offset)) = self.pending_context_menu.take() {
                self.handle_right_click_menu(position, offset, window, cx);
            }
        }
        if self.active_selection().is_empty() {
            self.active_selection_mut().reversed = false;
        }
        self.selecting = false;
        self.selected_word_range = None;
        self.column_select_start = None;
        self.auto_scroll.stop();
    }

    pub(super) fn on_scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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

        // Only stop propagation if the offset actually changed
        if self.scroll_handle.offset() != old_offset {
            cx.stop_propagation();
        }

        // The handles follow the text; the menu would sit over whatever
        // scrolls underneath it, so it steps aside until the finger lifts.
        self.edit_menu_on_scroll(event.touch_phase, cx);
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
            self.text_align,
        );
        if self.scroll_handle.offset() != offset {
            self.scroll_handle.set_offset(offset);
            cx.notify();
        }
    }

    /// Scroll to make the given offset visible.
    ///
    /// If `direction` is Some, will keep edges at the same side.
    pub(crate) fn scroll_to(
        &mut self,
        offset: usize,
        direction: Option<MoveDirection>,
        cx: &mut Context<Self>,
    ) {
        let padding = if direction.is_some() {
            ScrollPadding::SurroundingLines
        } else {
            ScrollPadding::Minimal
        };
        self.scroll_to_with_padding(offset, direction, padding, cx);
    }

    /// Reveal an offset with independently chosen direction restriction and padding.
    /// Search uses surrounding lines without restricting movement to match order.
    pub(crate) fn scroll_to_with_padding(
        &mut self,
        offset: usize,
        direction: Option<MoveDirection>,
        _padding: ScrollPadding,
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

        // Resolve the wrapped row even when the target is outside the last layout.
        let display_pos = self
            .display_map
            .buffer_pos_to_display_pos(crate::input::BufferPoint::new(row, point.column));
        let row_offset_y = line_height * display_pos.row;

        // For Right alignment use 0 margin: the cursor indicator is clamped inside bounds
        // in layout_cursors, so shifting the text here would cause a first-click visual jump.
        let safety_margin = match last_layout.text_align {
            TextAlign::Left => RIGHT_MARGIN,
            TextAlign::Right => px(0.),
            TextAlign::Center => CURSOR_WIDTH,
        };
        if let Some(vi) = last_layout
            .visible_buffer_lines
            .iter()
            .position(|&line| line == row)
        {
            let line = &last_layout.lines[vi];
            let local_offset = offset.saturating_sub(last_layout.visible_line_byte_offsets[vi]);
            if let Some(pos) = line.position_for_index(local_offset, last_layout, false) {
                let bounds_width = bounds.size.width - last_layout.line_number_width;
                let col_offset_x = pos.x;
                if col_offset_x - safety_margin < -scroll_offset.x {
                    // If the position is out of the visible area, scroll to make it visible
                    scroll_offset.x = -col_offset_x + safety_margin;
                } else if col_offset_x + safety_margin > -scroll_offset.x + bounds_width {
                    scroll_offset.x = -(col_offset_x - bounds_width + safety_margin);
                }
            }
        }

        let edge_height = line_height;
        if row_offset_y - edge_height + line_height < -scroll_offset.y {
            // Scroll up
            scroll_offset.y = -row_offset_y + edge_height - line_height;
        } else if row_offset_y + edge_height > -scroll_offset.y + bounds.size.height {
            // Scroll down
            scroll_offset.y = -(row_offset_y - bounds.size.height + edge_height);
        }

        // Avoid necessary scroll, when it was already in the correct position.
        if direction == Some(MoveDirection::Up) {
            scroll_offset.y = scroll_offset.y.max(was_offset.y);
        } else if direction == Some(MoveDirection::Down) {
            scroll_offset.y = scroll_offset.y.min(was_offset.y);
        }

        // Clamp the deferred target into the same safe range that
        // `update_scroll_offset` enforces on persist, so paint never shows an
        // over-scrolled frame before the post-paint clamp pulls it back.
        let safe_y_min = (-self.scroll_size.height + self.input_bounds.size.height).min(px(0.));
        scroll_offset.x = scroll_offset.x.min(px(0.));
        scroll_offset.y = scroll_offset.y.clamp(safe_y_min, px(0.));
        self.deferred_scroll_offset = Some(scroll_offset);
        cx.notify();
    }

    pub(super) fn show_character_palette(
        &mut self,
        _: &ShowCharacterPalette,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        window.show_character_palette();
    }

    /// The text of every non-empty selection, in document order.
    fn selected_texts(&self) -> Vec<String> {
        let mut selections: Vec<CursorSelection> = self
            .selections
            .iter()
            .copied()
            .filter(|sel| !sel.is_empty())
            .collect();
        selections.sort_by_key(|sel| sel.start);
        selections
            .iter()
            .map(|sel| self.text.slice(*sel).to_string())
            .collect()
    }

    pub(super) fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.is_copyable() {
            return;
        }

        let texts = self.selected_texts();

        cx.write_to_clipboard(ClipboardItem::new_string(texts.join("\n")));
    }

    pub(super) fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_copyable() {
            return;
        }

        let texts = self.selected_texts();

        cx.write_to_clipboard(ClipboardItem::new_string(texts.join("\n")));

        self.undo_manager.set_pending_intent(EditIntent::Atomic);
        self.replace_text_in_range_silent(None, "", window, cx);
    }

    pub(super) fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_editable() {
            return;
        }
        if let Some(clipboard) = cx.read_from_clipboard() {
            self.insert_clipboard(clipboard, window, cx);
            return;
        }
        // The synchronous read is empty on platforms whose clipboard is
        // asynchronous and permission-gated (the web), so fall back to the
        // real read. It has to start here, still inside the user activation
        // that dispatched `Paste`, or the browser refuses the read.
        let read = cx.read_from_clipboard_async();
        let target = self.paste_target();
        cx.spawn_in(window, async move |this, cx| match read.await {
            Ok(Some(clipboard)) => {
                this.update_in(cx, |this, window, cx| {
                    // The read can sit behind a permission prompt for as
                    // long as the user likes. If they edited, moved the
                    // caret or left the input meanwhile, the paste would
                    // land where they no longer mean it to; drop it, as the
                    // browser's own paste event only reaches the focused
                    // input too.
                    if this.is_editable()
                        && this.focus_handle.is_focused(window)
                        && this.paste_target() == target
                    {
                        this.insert_clipboard(clipboard, window, cx);
                    }
                })
                .ok();
            }
            Ok(None) => {}
            Err(_) => {}
        })
        .detach();
    }

    /// Where a paste would go right now: the document as edited so far and
    /// the selections it would replace. Two equal targets mean an edit made
    /// for one still applies to the other.
    fn paste_target(&self) -> PasteTarget {
        PasteTarget {
            document_revision: self.document_revision,
            selections: self.selections.iter().copied().collect(),
        }
    }

    /// Inserts the clipboard text the way `Paste` does: one atomic edit,
    /// newlines dropped on a single-line input, one line per selection when
    /// the counts match on a multi-line one. A clipboard without text (an
    /// image, say) is left alone rather than replacing the selection with
    /// nothing.
    fn insert_clipboard(
        &mut self,
        clipboard: ClipboardItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(mut new_text) = clipboard.text().filter(|text| !text.is_empty()) else {
            return;
        };
        if new_text.contains('\r') {
            new_text = new_text.replace("\r\n", "\n").replace('\r', "\n");
        }
        // A paste is one atomic edit, never part of a typing run.
        self.undo_manager.set_pending_intent(EditIntent::Atomic);

        if !self.is_multi_line() {
            new_text = new_text.replace('\n', "");
            self.replace_text_in_range_silent(None, &new_text, window, cx);
            self.scroll_to(self.cursor(), None, cx);
            return;
        }

        // Distribute one clipboard line per selection when the counts match.
        // Otherwise insert the whole clipboard text at each cursor.
        if !self.selections.is_single() {
            self.selections.merge_overlapping();
        }
        let count = self.selections.len();
        if count > 1 && new_text.split('\n').count() == count {
            let mut selections: Vec<CursorSelection> = self.selections.iter().copied().collect();
            selections.sort_by_key(|sel| sel.start);
            let edits: Vec<(Range<usize>, String)> = selections
                .iter()
                .zip(new_text.split('\n'))
                .map(|(sel, line)| (sel.start..sel.end, line.to_string()))
                .collect();
            self.replace_text_in_ranges(&edits, window, cx);
        } else {
            self.replace_text_in_range_silent(None, &new_text, window, cx);
        }
        self.scroll_to(self.cursor(), None, cx);
    }

    /// The intent of a batch the caller did not label: inserting text at
    /// collapsed cursors is typing, anything else stands on its own.
    fn typing_intent(&self, edits: &[(Range<usize>, String)], new_text: &str) -> EditIntent {
        if !new_text.is_empty()
            && !new_text.contains(['\n', '\r'])
            && edits.iter().all(|(range, _)| range.is_empty())
        {
            EditIntent::Typing
        } else {
            EditIntent::Atomic
        }
    }

    /// Where a cursor stood before an edit made with this intent.
    ///
    /// Backspace and forward delete expand a collapsed cursor over the text
    /// they are about to remove, so the recorded cursor has to collapse back to
    /// the side it came from for undo to restore it where the user left it.
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

    /// Whether an edit rewrites the character before a collapsed cursor, as the
    /// macOS Korean IME does on each keystroke instead of marking text.
    fn rewrites_typed_char(
        &self,
        selection: CursorSelection,
        range: &Range<usize>,
        old_text: &str,
        new_text: &str,
    ) -> bool {
        !self.silent_replace_text
            && selection.is_collapsed()
            && selection.cursor_offset() == range.end
            && old_text.chars().count() == 1
            && !old_text.contains(['\n', '\r'])
            && !new_text.contains(['\n', '\r'])
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
        let token_delta = self.edit_tokens(range, new_text.len());
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
                || self.rewrites_typed_char(selection_before, &range, &old_text, new_text)
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
        let mut change = Change::new(range, &old_text, new_range, new_text);
        change.token_delta = token_delta;
        let recorded = self.undo_manager.record_transaction(change, intent);
        // A batch records its own cursor sets. This covers a change that is a
        // transaction on its own.
        if recorded && !open_transaction {
            self.undo_manager
                .record_selections(vec![selection_before], vec![selection_after]);
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
        if !self.is_editable() {
            return;
        }
        let replay = if undo {
            self.undo_manager.undo()
        } else {
            self.undo_manager.redo()
        };
        let Some(replay) = replay else {
            return;
        };
        let token_aware = self.inline_tokens.is_some()
            || replay
                .changes
                .iter()
                .any(|change| change.token_delta.is_some());
        let emit_events = self.emit_events;
        if token_aware {
            self.emit_events = false;
        }
        self.replaying_history = token_aware;
        self.undo_manager.set_ignoring(true);
        for change in &replay.changes {
            let (range, text) = if undo {
                (change.new_range.into(), &change.old_text)
            } else {
                (change.old_range.into(), &change.new_text)
            };
            let range_utf16 = self.range_to_utf16(&range);
            self.replace_text_in_range_silent(Some(range_utf16), text, window, cx);
            self.replay_tokens(&range, text.len(), change.token_delta.as_deref(), undo);
        }
        self.restore_selections(replay.selections);
        if token_aware {
            // Token replay bypasses IME normalization; a stale marked range
            // would otherwise describe text that no longer exists.
            self.ime_marked_range = None;
        }
        self.undo_manager.set_ignoring(false);
        self.replaying_history = false;
        self.emit_events = emit_events;
        if token_aware && emit_events {
            cx.emit(InputEvent::Change);
        }
        cx.notify();
    }

    /// Restore a set of selections captured in a transaction, clamping offsets
    /// to the current text length. `None` leaves the current selections as the
    /// replay left them.
    fn restore_selections(&mut self, selections: Option<Vec<CursorSelection>>) {
        let Some(selections) = selections else {
            return;
        };

        let len = self.text.len();
        let restored: Vec<CursorSelection> = selections
            .into_iter()
            .map(|mut sel| {
                sel.start = sel.start.min(len);
                sel.end = sel.end.min(len);
                sel
            })
            .collect();
        self.selections.replace_all(restored);
        self.selections.merge_overlapping();
    }

    /// Get byte offset of the cursor.
    ///
    /// The offset is the UTF-8 offset.
    pub fn cursor(&self) -> usize {
        if let Some(ime_marked_range) = &self.ime_marked_range {
            return ime_marked_range.end;
        }

        self.selections.active().cursor_offset()
    }

    /// Returns the active selection.
    pub(super) fn active_selection(&self) -> &CursorSelection {
        self.selections.active()
    }

    /// Returns a mutable reference to the active selection.
    pub(super) fn active_selection_mut(&mut self) -> &mut CursorSelection {
        self.selections.active_mut()
    }

    /// Sets the active selection to the given range, keeping its `reversed`
    /// and `column_anchor` state untouched.
    pub(super) fn set_selection(&mut self, start: usize, end: usize) {
        let range = self.normalize_token_range(start..end);
        let (start, end) = (range.start, range.end);
        let active = self.active_selection_mut();
        active.start = start;
        active.end = end;
    }

    /// Collapses the active selection to a cursor at the given offset,
    /// clearing `reversed`.
    pub(super) fn set_cursor_to(&mut self, offset: usize) {
        let offset = self.token_boundary(offset, Bias::Right);
        let active = self.active_selection_mut();
        active.start = offset;
        active.end = offset;
        active.reversed = false;
    }

    /// Visible row range in the last laid-out viewport, `None` before first layout.
    pub fn visible_row_range(&self) -> Option<std::ops::Range<usize>> {
        self.last_layout.as_ref().map(|l| l.visible_range.clone())
    }

    /// Current scroll offset of the editor viewport.
    pub fn scroll_offset(&self) -> gpui::Point<gpui::Pixels> {
        self.scroll_handle.offset()
    }

    /// Set scroll offset of the editor viewport.
    ///
    /// The offset will be clamped to the valid range, and applied after the next layout.
    pub fn set_scroll_offset(&mut self, offset: gpui::Point<gpui::Pixels>, cx: &mut Context<Self>) {
        self.deferred_scroll_offset = Some(offset);
        cx.notify();
    }

    /// Laid-out line height; `None` before first layout.
    pub fn line_height(&self) -> Option<gpui::Pixels> {
        self.last_layout.as_ref().map(|l| l.line_height)
    }

    /// Returns the active selection as a byte range into the text.
    ///
    /// With multiple cursors, this reads only the active selection.
    ///
    /// The range is empty (`start == end`) when no text is selected; in
    /// that case the offset equals `cursor()`. Byte offsets are measured
    /// in the underlying rope's byte units.
    pub fn selected_range(&self) -> std::ops::Range<usize> {
        (*self.selections.active()).into()
    }

    pub fn select_all(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.undo_manager.break_transaction_coalescing();
        self.selections.remove_all_but_active();
        self.set_selection(0, self.text.len());
        cx.notify();
    }

    /// Set the selected range using UTF-8 byte offsets, removing additional cursors.
    ///
    /// Non-empty ranges expand to character boundaries. Empty ranges remain empty and are
    /// clipped to the preceding character boundary.
    pub fn set_selected_range(&mut self, range: Range<usize>, cx: &mut Context<Self>) {
        let range = self.normalize_token_range(range);
        let end_bias = if range.start == range.end {
            Bias::Left
        } else {
            Bias::Right
        };
        let start = self.text.clip_offset(range.start, Bias::Left);
        let end = self.text.clip_offset(range.end, end_bias);

        self.move_to(start, None, cx);
        self.active_selection_mut().reversed = false;
        self.selected_word_range = None;
        self.select_to(end, cx);
    }

    /// Add `range` as an additional selection, keeping the existing ones —
    /// the programmatic equivalent of Alt-clicking another cursor into the
    /// text.
    ///
    /// The range is clipped to the text and expanded to character
    /// boundaries, like [`Self::set_selected_range`]. A range already
    /// covered by a selection adds nothing. Overlapping selections are merged.
    /// On a single-line input there is no second cursor to add, so the call
    /// replaces the selection instead.
    /// The view scrolls to the new selection.
    pub fn add_selection(&mut self, range: Range<usize>, cx: &mut Context<Self>) {
        if !self.is_multi_line() {
            self.set_selected_range(range, cx);
            return;
        }

        let range = self.normalize_token_range(range);
        let end_bias = if range.start == range.end {
            Bias::Left
        } else {
            Bias::Right
        };
        let start = self.cursor_boundary(range.start, Bias::Left);
        // Match set_selected_range: expand UTF-8 boundaries first, then place
        // the selection endpoint without splitting a CRLF newline.
        let end = self.cursor_boundary(self.text.clip_offset(range.end, end_bias), Bias::Left);

        for sel in self.selections.iter() {
            if sel.start <= start && end <= sel.end {
                return;
            }
        }

        self.undo_manager.break_transaction_coalescing();
        self.selected_word_range = None;
        self.pause_blink_cursor(cx);
        let id = self.selections.generate_id();
        let mut selection = CursorSelection::new(id, start, end);
        selection.column_anchor = self.preferred_column_for(end);
        self.selections.add(selection);
        self.selections.merge_overlapping();
        self.scroll_to(end, None, cx);
        cx.notify();
    }

    /// Resolve a mouse position to a text offset, the caret affinity to use for it, and
    /// how many columns the pointer sits past the end of that row.
    pub(super) fn resolve_mouse_position(&self, position: Point<Pixels>) -> (usize, bool, usize) {
        // If the text is empty, always return 0
        if self.text.len() == 0 {
            return (0, false, 0);
        }

        let (Some(bounds), Some(last_layout)) =
            (self.last_bounds.as_ref(), self.last_layout.as_ref())
        else {
            return (0, false, 0);
        };

        let line_height = last_layout.line_height;
        let line_number_width = last_layout.line_number_width;

        // TIP: About the IBeam cursor
        //
        // If cursor style is IBeam, the mouse mouse position is in the middle of the cursor (This is special in OS)

        // The position is relative to the bounds of the text input
        //
        // bounds.origin:
        //
        // - included the input padding.
        // - included the scroll offset.
        let inner_position = position - bounds.origin - point(line_number_width, px(0.));

        let mut y_offset = last_layout.visible_top;
        // Position relative to the last line walked, kept for a pointer that ends up
        // below every line.
        let mut last_line_pos = None;

        // Traverse visible buffer lines (compact, no hidden entries)
        for (vi, (line_layout, _buffer_line)) in last_layout
            .lines
            .iter()
            .zip(last_layout.visible_buffer_lines.iter())
            .enumerate()
        {
            let line_start_offset = last_layout.visible_line_byte_offsets[vi];

            // Calculate line origin for this display row
            let line_origin = point(px(0.), y_offset);
            let pos = inner_position - line_origin;

            // Return offset by use closest_index_for_x if is single line mode.
            if self.is_single_line() {
                let local_index = line_layout.closest_index_for_x(pos.x, last_layout);
                // A single line never wraps, so there is no boundary to disambiguate.
                return (
                    self.resolve_index(line_start_offset + local_index),
                    false,
                    0,
                );
            }

            // Check if mouse is in this line's bounds
            if let Some((local_index, line_end_affinity)) =
                line_layout.closest_index_for_position(pos, last_layout)
            {
                return (
                    self.resolve_index(line_start_offset + local_index),
                    line_end_affinity,
                    line_layout.columns_past_line_end(pos, last_layout),
                );
            } else if pos.y < px(0.) {
                // Mouse is above this line, return start of this line
                return (self.resolve_index(line_start_offset), false, 0);
            }

            y_offset += line_layout.size(line_height).height;
            last_line_pos = Some(pos);
        }

        // Clamp to the last laid-out row. Returning the end of the whole document
        // would select unseen text before drag autoscroll has reached it.
        let last_position = last_layout
            .lines
            .last()
            .zip(last_line_pos)
            .zip(last_layout.visible_line_byte_offsets.last())
            .map(|((line_layout, pos), line_start)| {
                let last_row_top = (line_layout.size(line_height).height - line_height).max(px(0.));
                let pos = point(pos.x, last_row_top);
                (
                    self.resolve_index(line_start + line_layout.len()),
                    false,
                    line_layout.columns_past_line_end(pos, last_layout),
                )
            });

        last_position.unwrap_or((self.text.len(), false, 0))
    }

    /// Map a display byte index back to a text offset, undoing the mask expansion when the input
    /// is masked.
    fn resolve_index(&self, index: usize) -> usize {
        if self.masked {
            self.text.char_index_to_offset(index / MASK_CHAR.len_utf8())
        } else {
            index.min(self.text.len())
        }
    }

    /// Returns a y offsetted point for the line origin.
    /// Select the text from the current cursor position to the given offset.
    ///
    /// The offset is the UTF-8 offset.
    ///
    /// Ensure the offset use self.next_boundary or self.previous_boundary to get the correct offset.
    /// Extend a single selection so its moving end lands at `offset`, flipping
    /// `reversed` when the ends cross. When a sticky `word_range` is given the
    /// selection is kept covering it.
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

    /// Extend only the active selection to `offset`. Used by mouse drag.
    pub(crate) fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.select_to_with_affinity(offset, false, cx);
    }

    /// Like [`Self::select_to`], but also carries the caret's line-end affinity.
    ///
    /// See [`Self::move_to_with_affinity`] for why the affinity travels with the offset. Note
    /// that plain [`Self::select_to`] clears the affinity: every offset it is given came from
    /// the text rather than from a visual position, so the caret has no reason to keep sticking
    /// to the end of a wrapped row.
    pub(crate) fn select_to_with_affinity(
        &mut self,
        offset: usize,
        line_end_affinity: bool,
        cx: &mut Context<Self>,
    ) {
        self.cursor_line_end_affinity = line_end_affinity;
        let offset = self.cursor_boundary(offset, Bias::Left);
        let word_range = self.selected_word_range;
        Self::extend_selection(self.active_selection_mut(), offset, word_range);
        let range = self.normalize_token_range(self.selected_range());
        self.set_selection(range.start, range.end);

        if self.active_selection().is_empty() {
            self.update_preferred_column();
        }
        cx.notify()
    }

    /// Extend every selection to the offset produced by `f`, then merge any
    /// selections that now overlap. Used by keyboard selection commands.
    fn select_all_cursors_to(
        &mut self,
        f: impl Fn(&Self, &CursorSelection) -> usize,
        cx: &mut Context<Self>,
    ) {
        self.select_all_cursors_to_with_affinity(|s, sel| (f(s, sel), false), false, cx);
    }

    /// Extend selections with caret affinity, preserving their column anchors even
    /// when vertical movement collapses a selection to a cursor.
    fn select_all_cursors_to_with_affinity(
        &mut self,
        f: impl Fn(&Self, &CursorSelection) -> (usize, bool),
        preserve_column: bool,
        cx: &mut Context<Self>,
    ) {
        self.pause_blink_cursor(cx);
        self.undo_manager.break_transaction_coalescing();

        let mut active_affinity = false;
        let new_selections: Vec<CursorSelection> = self
            .selections
            .iter()
            .map(|sel| {
                let (offset, affinity) = f(self, sel);
                if sel.id == self.active_selection().id {
                    active_affinity = affinity;
                }
                let offset = self.cursor_boundary(offset, Bias::Left);
                let mut new_sel = *sel;
                Self::extend_selection(&mut new_sel, offset, None);
                let range = self.normalize_token_range(new_sel.start..new_sel.end);
                new_sel.start = range.start;
                new_sel.end = range.end;
                if !preserve_column {
                    new_sel.column_anchor =
                        self.preferred_column_for_with_affinity(new_sel.cursor_offset(), affinity);
                } else if new_sel.column_anchor.is_none() {
                    new_sel.column_anchor = self.preferred_column_for_with_affinity(
                        sel.cursor_offset(),
                        self.line_end_affinity_for(sel),
                    );
                }
                new_sel
            })
            .collect();
        self.cursor_line_end_affinity = active_affinity;
        self.selections.replace_all(new_selections);
        self.selections.merge_overlapping();

        self.scroll_to(self.cursor(), None, cx);
        cx.notify()
    }

    /// Unselects the currently selected text.
    pub fn unselect(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.undo_manager.break_transaction_coalescing();
        let offset = self.cursor();
        self.set_cursor_to(offset);
        cx.notify()
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

    /// Clip a cursor/selection offset without splitting a CRLF newline. This does
    /// not alter the rope or its byte/UTF-16 conversion used for exact source APIs.
    pub(super) fn cursor_boundary(&self, offset: usize, bias: Bias) -> usize {
        let offset = self.token_boundary(self.text.clip_offset(offset, bias), bias);
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

    /// Returns true to let InputElement render cursor when Input is focused and BlinkCursor is visible.
    pub fn show_cursor(&self, window: &Window, cx: &App) -> bool {
        self.focus_handle.is_focused(window)
            && !self.disabled
            && self.blink_cursor.read(cx).visible()
            && window.is_window_active()
    }

    fn on_focus(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.blink_cursor.update(cx, |cursor, cx| {
            cursor.start(cx);
        });
        cx.emit(InputEvent::Focus);
    }

    fn on_blur(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.undo_manager.break_transaction_coalescing();

        // NOTE: Do not cancel select, when blur.
        // Because maybe user want to copy the selected text by AppMenuBar (will take focus handle).

        self.dismiss_touch_selection(cx);
        self.blink_cursor.update(cx, |cursor, cx| {
            cursor.stop(cx);
        });
        cx.emit(InputEvent::Blur);
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
        if self.text.len() == 0 {
            return;
        }

        if self.last_layout.is_none() {
            return;
        }

        if !self.focus_handle.is_focused(window) {
            return;
        }

        if !self.selecting {
            return;
        }

        self.auto_scroll.last_drag_position = Some(event.position);
        let (offset, line_end_affinity, columns_past_line_end) =
            self.resolve_mouse_position(event.position);
        if let Some(start) = self.column_select_start {
            let end = ColumnarPoint::new(offset, columns_past_line_end);
            self.build_columnar_selection(start, end, cx);
        } else {
            self.select_to_with_affinity(offset, line_end_affinity, cx);
        }

        if !self.is_single_line() {
            let delta = AutoScroll::compute_delta(event.position.y, self.input_bounds);
            // Input's ScrollHandle uses negative-y-is-down; negate the positive-towards-bottom delta.
            let scroll_delta = delta.map(|d| -d);
            self.auto_scroll.set(scroll_delta, cx, |delta, state, cx| {
                let current = state.scroll_handle.offset();
                state.update_scroll_offset(Some(point(current.x, current.y + delta)), cx);
                if let Some(pos) = state.auto_scroll.last_drag_position {
                    let (offset, line_end_affinity, columns_past_line_end) =
                        state.resolve_mouse_position(pos);
                    if let Some(start) = state.column_select_start {
                        let end = ColumnarPoint::new(offset, columns_past_line_end);
                        state.build_columnar_selection(start, end, cx);
                    } else {
                        state.select_to_with_affinity(offset, line_end_affinity, cx);
                    }
                }
            });
        }
    }

    /// Normalize the inserted text before applying it to the input.
    ///
    /// For number inputs (with [`MaskPattern::Number`]), this converts
    /// full-width number characters into their ASCII equivalents,
    /// e.g. `12。5` -> `12.5`.
    pub(super) fn normalize_input<'a>(&self, new_text: &'a str) -> Cow<'a, str> {
        let normalized = if matches!(self.mask_pattern, MaskPattern::Number { .. }) {
            normalize_number_input(new_text)
        } else {
            Cow::Borrowed(new_text)
        };

        if self.is_single_line() && normalized.contains(['\n', '\r']) {
            Cow::Owned(normalized.replace(['\n', '\r'], ""))
        } else {
            normalized
        }
    }

    pub(crate) fn is_valid_input(&self, new_text: &str, cx: &mut Context<Self>) -> bool {
        if new_text.is_empty() {
            return true;
        }

        if let Some(validate) = &self.validate {
            if !validate(new_text, cx) {
                return false;
            }
        }

        self.mask_pattern.is_valid(new_text)
    }

    /// Set the mask pattern for formatting the input text.
    ///
    /// The pattern can contain:
    /// - 9: Any digit or dot
    /// - A: Any letter
    /// - *: Any character
    /// - Other characters will be treated as literal mask characters
    ///
    /// Example: "(999)999-999" for phone numbers
    pub fn mask_pattern(mut self, pattern: impl Into<MaskPattern>) -> Self {
        self.mask_pattern = pattern.into();
        self.mask_pattern_set = true;
        if let Some(placeholder) = self.mask_pattern.placeholder() {
            self.placeholder = placeholder.into();
        }
        self
    }

    pub fn set_mask_pattern(
        &mut self,
        pattern: impl Into<MaskPattern>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.mask_pattern = pattern.into();
        self.mask_pattern_set = true;
        if let Some(placeholder) = self.mask_pattern.placeholder() {
            self.placeholder = placeholder.into();
        }
        cx.notify();
    }

    /// Apply the default numeric mask unless the caller explicitly selected a mask.
    pub fn ensure_number_mask(&mut self) {
        if self.mask_pattern_set {
            return;
        }
        self.mask_pattern = MaskPattern::Number {
            separator: None,
            fraction: None,
        };
    }

    pub(super) fn set_input_bounds(&mut self, new_bounds: Bounds<Pixels>, cx: &mut Context<Self>) {
        let wrap_width_changed = self.input_bounds.size.width != new_bounds.size.width;
        self.input_bounds = new_bounds;

        // Update display_map wrap_width if changed.
        if let Some(last_layout) = self.last_layout.as_ref() {
            if wrap_width_changed {
                let wrap_width = if !self.soft_wrap {
                    // None to disable wrapping (will use Pixels::MAX)
                    None
                } else {
                    last_layout.wrap_width
                };

                self.display_map.on_layout_changed(wrap_width, cx);
                if self.is_multi_line() {
                    self.mode.update_auto_grow(&self.display_map);
                }
                cx.notify();
            }
        }
    }

    /// Return the active selection's text, borrowed out of the [`Rope`]
    /// the state owns.
    ///
    /// See [`Self::selected_value`] when an owned string is wanted.
    pub fn selected_text(&self) -> RopeSlice<'_> {
        let range_utf16 = self.range_to_utf16(&self.selected_range());
        let range = self.range_from_utf16(&range_utf16);
        self.text.slice(range)
    }

    /// Return the rendered bounds for a UTF-8 byte range in the current input contents.
    ///
    /// Returns `None` when the requested range is not currently laid out or visible.
    pub fn range_to_bounds(&self, range: &Range<usize>) -> Option<Bounds<Pixels>> {
        let Some(last_layout) = self.last_layout.as_ref() else {
            return None;
        };

        let Some(last_bounds) = self.last_bounds else {
            return None;
        };

        let (_, _, start_pos) = self.line_and_position_for_offset(range.start);
        let (_, _, end_pos) = self.line_and_position_for_offset(range.end);

        let Some(start_pos) = start_pos else {
            return None;
        };
        let Some(end_pos) = end_pos else {
            return None;
        };

        Some(Bounds::from_corners(
            last_bounds.origin + start_pos,
            last_bounds.origin + end_pos + point(px(0.), last_layout.line_height),
        ))
    }

    /// Replace text in range in silent.
    ///
    /// This will not trigger any UI interaction, such as auto-completion.
    pub(crate) fn replace_text_in_range_silent(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.silent_replace_text = true;
        self.replace_text_in_range(range_utf16, new_text, window, cx);
        self.silent_replace_text = false;
    }

    /// Apply a batch of edits as one atomic history transaction.
    ///
    /// `edits` are `(byte range in the current pre-edit document, replacement)`
    /// pairs.
    pub(crate) fn replace_text_in_ranges(
        &mut self,
        edits: &[(Range<usize>, String)],
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.is_editable() || edits.is_empty() {
            return;
        }

        // Sort descending by start so applying front-of-vec first edits the
        // highest offsets first, leaving lower offsets unchanged.
        let mut sorted: Vec<(Range<usize>, &str)> = edits
            .iter()
            .map(|(range, text)| (self.normalize_token_range(range.clone()), text.as_str()))
            .collect();
        sorted.sort_by_key(|edit| std::cmp::Reverse(edit.0.start));
        if !self.token_spans().is_empty() {
            let mut merged: Vec<(Range<usize>, &str)> = Vec::with_capacity(sorted.len());
            for (range, text) in sorted {
                if let Some(last) = merged
                    .last_mut()
                    .filter(|last| range.end > last.0.start || range == last.0)
                {
                    last.0.start = range.start.min(last.0.start);
                    last.0.end = range.end.max(last.0.end);
                } else {
                    merged.push((range, text));
                }
            }
            sorted = merged;
        }

        #[cfg(debug_assertions)]
        for pair in sorted.windows(2) {
            debug_assert!(
                pair[1].0.end <= pair[0].0.start,
                "replace_text_in_ranges requires disjoint ranges"
            );
        }

        // Wrap multiple edits in one explicit transaction so they undo as a
        // unit. A single edit records directly, which keeps it eligible for
        // the undo manager's typing coalescing.
        let requested_intent = self.undo_manager.take_pending_intent();
        let selection_before = *self.active_selection();
        let original_selections: Vec<CursorSelection> = self.selections.iter().copied().collect();
        // Snapshot the cursors before applying, so undo can restore them. A
        // delete has already expanded them over the text it removes, so they
        // collapse back to where the user left them.
        let selections_before: Vec<CursorSelection> = self
            .selections
            .iter()
            .map(|selection| {
                Self::collapse_for_intent(
                    requested_intent.unwrap_or(EditIntent::Atomic),
                    *selection,
                    &(selection.start..selection.end),
                )
            })
            .collect();
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
                selection_before,
                None,
            );

            self.display_map
                .on_text_changed(&self.text, range, &Rope::from(*new_text), cx);
        }

        if group {
            self.undo_manager.commit_transaction();
        }

        // Compute the resulting cursors.
        // One collapsed cursor per edit, at the end of its inserted text.
        let mut ascending: Vec<(Range<usize>, &str)> = sorted.clone();
        ascending.sort_by_key(|edit| edit.0.start);
        let text_len = self.text.len();
        let mut delta: isize = 0;
        let mut edit_results = Vec::with_capacity(ascending.len());
        for (range, new_text) in &ascending {
            let offset = ((range.start as isize + delta) as usize + new_text.len()).min(text_len);
            edit_results.push((range.clone(), offset));
            delta += new_text.len() as isize - (range.end as isize - range.start as isize);
        }

        let mut used = vec![false; edit_results.len()];
        let mut new_selections: Vec<CursorSelection> = Vec::with_capacity(ascending.len());
        for selection in original_selections {
            if let Ok(index) = edit_results
                .binary_search_by_key(&(selection.start, selection.end), |(range, _)| {
                    (range.start, range.end)
                })
                && !used[index]
            {
                let offset = edit_results[index].1;
                used[index] = true;
                let mut selection = selection;
                selection.place_at(offset, None);
                new_selections.push(selection);
            }
        }
        for (index, (_, offset)) in edit_results.into_iter().enumerate() {
            if !used[index] {
                new_selections.push(CursorSelection::new(
                    self.selections.generate_id(),
                    offset,
                    offset,
                ));
            }
        }
        self.selections.replace_all(new_selections);
        self.selections.merge_overlapping();

        // Record the cursor snapshots for undo/redo restore.
        let selections_after: Vec<CursorSelection> = self.selections.iter().copied().collect();
        if recorded {
            self.undo_manager
                .record_selections(selections_before, selections_after);
        }

        self.ime_marked_range.take();
        self.update_preferred_column();
        self.dismiss_touch_selection(cx);
        if self.is_multi_line() {
            self.mode.update_auto_grow(&self.display_map);
        }
        if self.emit_events {
            cx.emit(InputEvent::Change);
        }
        cx.notify();
    }
}

impl<M: InputModeKind> EntityInputHandler for InputBaseState<M> {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        adjusted_range.replace(self.range_to_utf16(&range));
        Some(self.text.slice(range).to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range()),
            reversed: self.active_selection().reversed,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.ime_marked_range
            .map(|range| self.range_to_utf16(&range.into()))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.ime_marked_range = None;
        self.undo_manager.commit_transaction();
    }

    /// Replace text in range.
    ///
    /// - If the new text is invalid, it will not be replaced.
    /// - If `range_utf16` is not provided, the current selected range will be used.
    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let requested_intent = self.undo_manager.take_pending_intent();
        if !self.is_editable() {
            return;
        }
        let selection_before = *self.active_selection();
        // Committing a composition ends the transaction it opened, whether or
        // not the platform follows up with `unmark_text`.
        let ends_composition = self.ime_marked_range.is_some();

        self.pause_blink_cursor(cx);

        // NOTE: The normalization keeps the UTF-16 length, but may change the
        // UTF-8 byte length, so all the byte-offset calculations below must
        // use the normalized text.
        let new_text = if self.replaying_history {
            Cow::Borrowed(new_text)
        } else {
            self.normalize_input(new_text)
        };
        let new_text: &str = &new_text;

        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.ime_marked_range.map(|range| {
                let range = self.range_to_utf16(&(range.start..range.end));
                self.range_from_utf16(&range)
            }))
            .unwrap_or(self.selected_range());
        let range = self.normalize_token_range(range);

        if self.is_multi_line() {
            let multi_cursor = range_utf16.is_none()
                && self.ime_marked_range.is_none()
                && !self.selections.is_single();
            if multi_cursor {
                self.selections.merge_overlapping();
                let mut edits: Vec<(Range<usize>, String)> = self
                    .selections
                    .iter()
                    .map(|sel| (sel.start..sel.end, new_text.to_string()))
                    .collect();
                edits.sort_by_key(|(range, _)| range.start);
                // One keystroke across several cursors is one batch, committed
                // with the intent of the keystroke so a run of them coalesces
                // into one undo just like single-cursor typing.
                let intent =
                    requested_intent.unwrap_or_else(|| self.typing_intent(&edits, new_text));
                self.undo_manager.begin_transaction_with(intent);
                self.undo_manager.set_pending_intent(intent);
                self.replace_text_in_ranges(&edits, window, cx);
                self.undo_manager.commit_transaction();
            } else {
                if range_utf16.is_some() {
                    self.selections.remove_all_but_active();
                }
                if let Some(intent) = requested_intent {
                    self.undo_manager.set_pending_intent(intent);
                }
                self.replace_text_in_ranges(&[(range.clone(), new_text.to_string())], window, cx);
            }
            if ends_composition {
                self.undo_manager.commit_transaction();
            }
            return;
        }

        // Single-line path
        let old_text = self.text.clone();
        self.text.replace(range.clone(), new_text);

        let mut new_offset = (range.start + new_text.len()).min(self.text.len());

        // True if the mask has changed the text, e.g. regrouping the
        // separators or completing a leading dot.
        let mut mask_changed = false;

        if self.is_single_line() {
            let pending_text = self.text.to_string();
            // Check if the new text is valid.
            //
            // Only reject the edit if the old text was valid, to avoid
            // trapping a pre-existing invalid text (e.g. a `default_value`
            // that does not conform), the user can still edit to fix it.
            if !self.replaying_history
                && !self.validated_token_edit
                && !self.is_valid_input(&pending_text, cx)
                && self.is_valid_input(&old_text.to_string(), cx)
            {
                self.text = old_text;
                return;
            }

            if !self.replaying_history && !self.mask_pattern.is_none() {
                let mask_text = self.mask_pattern.mask(&pending_text);
                mask_changed = mask_text.as_str() != pending_text;
                self.text = Rope::from(mask_text.as_str());
                let new_text_len =
                    (new_text.len() + mask_text.len()).saturating_sub(pending_text.len());
                new_offset = (range.start + new_text_len).min(mask_text.len());
            }
        }

        if mask_changed {
            // A segment-based history entry no longer matches the masked
            // document, record a whole-document change instead, so that
            // undo/redo can restore the text exactly.
            self.push_history(
                &old_text,
                &(0..old_text.len()),
                &self.text.to_string(),
                Some(EditIntent::Atomic),
                selection_before,
                Some((new_offset..new_offset).into()),
            );
        } else {
            self.push_history(
                &old_text,
                &range,
                &new_text,
                requested_intent,
                selection_before,
                None,
            );
        }
        self.display_map
            .on_text_changed(&self.text, &range, &Rope::from(new_text), cx);

        self.set_cursor_to(new_offset);
        self.ime_marked_range.take();
        // A commit ends the IME composition: macOS delivers `insertText:` for
        // the confirmed candidate without a following `unmarkText`, so close
        // the transaction here. Leaving it open would keep merging every later
        // edit into the same change, which then carries the text and selection
        // of the first composition.
        if ends_composition {
            self.undo_manager
                .record_selections(vec![selection_before], vec![*self.active_selection()]);
            self.undo_manager.commit_transaction();
        }
        self.update_preferred_column();
        self.dismiss_touch_selection(cx);
        if self.is_multi_line() {
            self.mode.update_auto_grow(&self.display_map);
        }
        if self.emit_events {
            cx.emit(InputEvent::Change);
        }
        cx.notify();
    }

    /// Mark text is the IME temporary insert on typing.
    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let requested_intent = self.undo_manager.take_pending_intent();
        if !self.is_editable() {
            return;
        }
        let selection_before = *self.active_selection();

        let starts_composition = self.ime_marked_range.is_none();
        if starts_composition {
            // Even a canceled preedit separates the typing gestures on either
            // side; its no-op transaction must not reconnect those gestures.
            self.undo_manager.break_transaction_coalescing();
            self.undo_manager.begin_transaction();
        }

        // Collapse any extra cursors so we never leave stale secondary cursors behind.
        self.selections.remove_all_but_active();

        // See the same NOTE in `replace_text_in_range`.
        let new_text = if self.replaying_history {
            Cow::Borrowed(new_text)
        } else {
            self.normalize_input(new_text)
        };
        let new_text: &str = &new_text;

        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.ime_marked_range.map(|range| {
                let range = self.range_to_utf16(&(range.start..range.end));
                self.range_from_utf16(&range)
            }))
            .unwrap_or(self.selected_range());
        let range = self.normalize_token_range(range);

        let old_text = self.text.clone();
        self.text.replace(range.clone(), new_text);

        if self.is_single_line() {
            let pending_text = self.text.to_string();
            // See the same NOTE in `replace_text_in_range`.
            if !self.replaying_history
                && !self.validated_token_edit
                && !self.is_valid_input(&pending_text, cx)
                && self.is_valid_input(&old_text.to_string(), cx)
            {
                self.text = old_text;
                if starts_composition {
                    self.undo_manager.commit_transaction();
                }
                return;
            }
        }

        self.display_map
            .on_text_changed(&self.text, &range, &Rope::from(new_text), cx);

        if new_text.is_empty() {
            // Cancel selection, when cancel IME input.
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
        if self.is_multi_line() {
            self.mode.update_auto_grow(&self.display_map);
        }
        if self.push_history(
            &old_text,
            &range,
            new_text,
            requested_intent,
            selection_before,
            Some(*self.active_selection()),
        ) {
            self.undo_manager
                .record_selections(vec![selection_before], vec![*self.active_selection()]);
        }
        if new_text.is_empty() {
            self.undo_manager.commit_transaction();
        }
        cx.notify();
    }

    /// Used to position IME candidates.
    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let last_layout = self.last_layout.as_ref()?;
        let line_height = last_layout.line_height;
        let line_number_width = last_layout.line_number_width;
        let range = self.range_from_utf16(&range_utf16);

        let mut start_origin = None;
        let mut end_origin = None;
        let line_number_origin = point(line_number_width, px(0.));
        let mut y_offset = last_layout.visible_top;

        for (vi, line) in last_layout.lines.iter().enumerate() {
            if start_origin.is_some() && end_origin.is_some() {
                break;
            }

            let index_offset = last_layout.visible_line_byte_offsets[vi];

            if start_origin.is_none() {
                if let Some(p) = line.position_for_index(
                    range.start.saturating_sub(index_offset),
                    last_layout,
                    false,
                ) {
                    start_origin = Some(p + point(px(0.), y_offset));
                }
            }

            if end_origin.is_none() {
                if let Some(p) = line.position_for_index(
                    range.end.saturating_sub(index_offset),
                    last_layout,
                    false,
                ) {
                    end_origin = Some(p + point(px(0.), y_offset));
                }
            }

            y_offset += line.size(line_height).height;
        }

        let start_origin = start_origin.or_else(|| {
            let offset = self.last_cursor.or(Some(self.cursor()))?;
            let (_, _, origin) = self.line_and_position_for_offset(offset);
            origin.map(|origin| origin - line_number_origin)
        })?;
        let mut end_origin = end_origin.unwrap_or(start_origin);
        // Ensure at same line.
        end_origin.y = start_origin.y;

        Some(Bounds::from_corners(
            bounds.origin + line_number_origin + start_origin,
            // + line_height for show IME panel under the cursor line.
            bounds.origin + line_number_origin + point(end_origin.x, end_origin.y + line_height),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let last_layout = self.last_layout.as_ref()?;
        let line_point = self.last_bounds?.localize(&point)?;

        for (vi, line) in last_layout.lines.iter().enumerate() {
            let offset = last_layout.visible_line_byte_offsets[vi];
            if let Some(utf8_index) = line.index_for_position(line_point, last_layout) {
                return Some(self.offset_to_utf16(offset + utf8_index));
            }
        }

        None
    }
}

impl<M: InputModeKind> Focusable for InputBaseState<M> {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl<M: InputModeKind> Render for InputBaseState<M> {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Before anything reads it: the element resolves this style during
        // layout and paint, and both happen after this call in the same frame.
        self.editor_style = self
            .projected_editor_style
            .resolved(&crate::Theme::global(cx).tokens);
        let entity = cx.entity();

        div()
            .id("input-state")
            .key_context(CONTEXT)
            .track_focus(&self.focus_handle)
            .when(self.is_editable(), |this| {
                this.on_action(window.listener_for(&entity, InputBaseState::backspace))
                    .on_action(window.listener_for(&entity, InputBaseState::delete))
                    .on_action(
                        window.listener_for(&entity, InputBaseState::delete_to_beginning_of_line),
                    )
                    .on_action(window.listener_for(&entity, InputBaseState::delete_to_end_of_line))
                    .on_action(window.listener_for(&entity, InputBaseState::delete_previous_word))
                    .on_action(window.listener_for(&entity, InputBaseState::delete_next_word))
                    .on_action(window.listener_for(&entity, InputBaseState::enter))
                    .on_action(window.listener_for(&entity, InputBaseState::escape))
                    .on_action(window.listener_for(&entity, InputBaseState::paste))
                    .on_action(window.listener_for(&entity, InputBaseState::cut))
                    .on_action(window.listener_for(&entity, InputBaseState::undo))
                    .on_action(window.listener_for(&entity, InputBaseState::redo))
                    .when(self.is_multi_line(), |this| {
                        this.on_action(window.listener_for(&entity, InputBaseState::indent_inline))
                            .on_action(window.listener_for(&entity, InputBaseState::outdent_inline))
                            .on_action(window.listener_for(&entity, InputBaseState::indent_block))
                            .on_action(window.listener_for(&entity, InputBaseState::outdent_block))
                    })
            })
            .on_action(window.listener_for(&entity, InputBaseState::left))
            .on_action(window.listener_for(&entity, InputBaseState::right))
            .on_action(window.listener_for(&entity, InputBaseState::select_left))
            .on_action(window.listener_for(&entity, InputBaseState::select_right))
            .when(self.is_multi_line(), |this| {
                this.on_action(window.listener_for(&entity, InputBaseState::up))
                    .on_action(window.listener_for(&entity, InputBaseState::down))
                    .on_action(window.listener_for(&entity, InputBaseState::select_up))
                    .on_action(window.listener_for(&entity, InputBaseState::select_down))
                    .on_action(window.listener_for(&entity, InputBaseState::page_up))
                    .on_action(window.listener_for(&entity, InputBaseState::page_down))
                    .on_action(window.listener_for(&entity, InputBaseState::add_cursor_above))
                    .on_action(window.listener_for(&entity, InputBaseState::add_cursor_below))
            })
            .on_action(window.listener_for(&entity, InputBaseState::on_action_select_all))
            .on_action(window.listener_for(&entity, InputBaseState::select_to_start_of_line))
            .on_action(window.listener_for(&entity, InputBaseState::select_to_end_of_line))
            .on_action(window.listener_for(&entity, InputBaseState::select_to_previous_word))
            .on_action(window.listener_for(&entity, InputBaseState::select_to_next_word))
            .on_action(window.listener_for(&entity, InputBaseState::home))
            .on_action(window.listener_for(&entity, InputBaseState::end))
            .on_action(window.listener_for(&entity, InputBaseState::move_to_start))
            .on_action(window.listener_for(&entity, InputBaseState::move_to_end))
            .on_action(window.listener_for(&entity, InputBaseState::move_to_previous_word))
            .on_action(window.listener_for(&entity, InputBaseState::move_to_next_word))
            .on_action(window.listener_for(&entity, InputBaseState::select_to_start))
            .on_action(window.listener_for(&entity, InputBaseState::select_to_end))
            .on_action(window.listener_for(&entity, InputBaseState::show_character_palette))
            .on_action({
                let entity = entity.clone();
                move |_: &ActivateToken, window, cx| {
                    let state = entity.read(cx);
                    let activation = state
                        .token_spans()
                        .iter()
                        .find(|span| span.range() == state.selected_range())
                        .and_then(|span| {
                            state.range_to_bounds(&span.range()).and_then(|bounds| {
                                state.token_activation(
                                    span.range().start,
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
                }
            })
            .on_action(window.listener_for(&entity, InputBaseState::copy))
            .on_mouse_down(
                MouseButton::Left,
                window.listener_for(&entity, InputBaseState::on_mouse_down),
            )
            .on_mouse_down(
                MouseButton::Right,
                window.listener_for(&entity, InputBaseState::on_mouse_down),
            )
            .on_mouse_up(
                MouseButton::Left,
                window.listener_for(&entity, InputBaseState::on_mouse_up),
            )
            .on_mouse_up(
                MouseButton::Right,
                window.listener_for(&entity, InputBaseState::on_mouse_up),
            )
            .on_scroll_wheel(window.listener_for(&entity, InputBaseState::on_scroll_wheel))
            .when(self.is_multi_line() && !self.disabled, |this| {
                this.on_modifiers_changed(cx.listener(|_, _, _, cx| cx.notify()))
            })
            .when(!self.disabled, |this| {
                if self.is_multi_line() && window.modifiers().alt {
                    this.cursor_crosshair()
                } else {
                    this.cursor_text()
                }
            })
            .flex_1()
            // A single line fills the frame and sits at its vertical center,
            // so the frame needs no layout of its own to hold it. It is never
            // shorter than its line, though: this root clips (overflow-x
            // hidden clips both axes), and a frame whose padding leaves less
            // than a line, like Component's Input, would cut off descenders.
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
            .when(self.shows_scrollbar(), |this| {
                this.child(EditorScrollbar::new(entity.clone()))
            })
    }
}

/// Methods that only a single-line input offers.
impl InputBaseState<crate::input::InputMode> {
    /// Create a single-line text input state.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::new_in_mode(window, cx)
    }

    /// Set with password masked state.
    pub fn masked(mut self, masked: bool) -> Self {
        self.masked = masked;
        self
    }

    /// Set the password masked state of the input field.
    pub fn set_masked(&mut self, masked: bool, _: &mut Window, cx: &mut Context<Self>) {
        self.masked = masked;
        cx.notify();
    }

    /// Set the validation function of the input field.
    pub fn validate(mut self, f: impl Fn(&str, &mut App) -> bool + 'static) -> Self {
        self.validate = Some(Box::new(f));
        self
    }

    pub fn set_validator(
        &mut self,
        validate: impl Fn(&str, &mut App) -> bool + 'static,
        _cx: &mut Context<Self>,
    ) {
        self.validate = Some(Box::new(validate));
    }

    /// Set true to show spinner at the input right.
    pub fn set_loading(&mut self, loading: bool, _: &mut Window, cx: &mut Context<Self>) {
        self.loading = loading;
        cx.notify();
    }
}

/// Methods shared by multi-line modes.
impl<M: crate::input::MultiLineMode> InputBaseState<M> {
    /// Set the soft wrap mode, default is true.
    #[doc(hidden)]
    pub fn soft_wrap(mut self, wrap: bool) -> Self {
        self.soft_wrap = wrap;
        self
    }

    /// Update the soft wrap mode, default is true.
    pub fn set_soft_wrap(&mut self, wrap: bool, _: &mut Window, cx: &mut Context<Self>) {
        self.soft_wrap = wrap;
        if wrap {
            let wrap_width = self
                .last_layout
                .as_ref()
                .and_then(|b| b.wrap_width)
                .unwrap_or(self.input_bounds.size.width);

            self.display_map.on_layout_changed(Some(wrap_width), cx);

            // Reset scroll to left 0
            let mut offset = self.scroll_handle.offset();
            offset.x = px(0.);
            self.scroll_handle.set_offset(offset);
        } else {
            self.display_map.on_layout_changed(None, cx);
        }
        cx.notify();
    }

    /// Set how soft-wrapped continuation lines are indented, default is [`WrappingIndent::Same`]
    #[doc(hidden)]
    pub fn wrapping_indent(mut self, wrapping_indent: WrappingIndent) -> Self {
        self.wrapping_indent = wrapping_indent;
        self
    }

    /// Update how soft-wrapped continuation lines are indented.
    pub fn set_wrapping_indent(
        &mut self,
        wrapping_indent: WrappingIndent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.wrapping_indent = wrapping_indent;
        self.display_map.set_wrapping_indent(wrapping_indent, cx);
        cx.notify();
    }
}

/// Methods that only ordinary multi-line text offers.
impl InputBaseState<crate::input::TextareaMode> {
    /// Create a multi-line text state.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::new_in_mode(window, cx)
    }

    pub fn set_auto_grow(&mut self, min_rows: usize, max_rows: usize, cx: &mut Context<Self>) {
        self.mode = LayoutMode::auto_grow(min_rows, max_rows.max(min_rows));
        self.mode.update_auto_grow(&self.display_map);
        cx.notify();
    }

    #[doc(hidden)]
    pub fn rows(mut self, rows: usize) -> Self {
        match &mut self.mode {
            LayoutMode::PlainText { rows: r, .. } => *r = rows,
            LayoutMode::AutoGrow {
                max_rows: max_r,
                rows: r,
                ..
            } => {
                *r = rows;
                *max_r = rows;
            }
        }
        self
    }

    pub fn set_rows(&mut self, rows: usize, cx: &mut Context<Self>) {
        match &mut self.mode {
            LayoutMode::PlainText { rows: value, .. } => *value = rows,
            LayoutMode::AutoGrow {
                rows: value,
                max_rows,
                ..
            } => {
                *value = rows;
                *max_rows = rows;
            }
        }
        cx.notify();
    }

    /// Grow with the content from `min_rows` through `max_rows`.
    pub fn auto_grow(mut self, min_rows: usize, max_rows: usize) -> Self {
        self.mode = LayoutMode::auto_grow(min_rows, max_rows);
        self
    }
}
