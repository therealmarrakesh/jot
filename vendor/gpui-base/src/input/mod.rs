//! Text input: the shared editing engine for single-line and multi-line plain text.
#![warn(unreachable_pub)]

use gpui::App;

/// Character used by masked editor modes.
pub(crate) const MASK_CHAR: char = '•';

mod base;
#[path = "base/blink_cursor.rs"]
pub(crate) mod blink_cursor;
#[path = "base/change.rs"]
mod change;
#[path = "base/cursor.rs"]
mod cursor;
#[path = "editor/display_map/mod.rs"]
mod display_map;
#[path = "base/element.rs"]
mod element;
#[path = "editor/indent.rs"]
mod indent;
#[path = "base/inline_tokens.rs"]
mod inline_tokens;
mod input;
#[path = "base/kind.rs"]
mod kind;
#[path = "base/layout.rs"]
mod layout;
#[path = "base/mask_pattern.rs"]
mod mask_pattern;
#[path = "base/mode.rs"]
mod mode;
#[path = "base/movement.rs"]
mod movement;
#[path = "base/native.rs"]
mod native;
#[path = "base/rope_ext.rs"]
mod rope_ext;
#[path = "base/selection.rs"]
mod selection;
#[path = "base/state.rs"]
mod state;
mod textarea;
#[path = "base/token_presentation.rs"]
mod token_presentation;
#[path = "base/touch.rs"]
mod touch;
#[path = "base/undo_manager.rs"]
mod undo_manager;

pub(crate) fn init(cx: &mut App) {
    state::init(cx);
}

pub use base::{InputContextMenuCapabilities, InputEditorStyle};
pub use cursor::Selection;
pub use display_map::{BufferPoint, DisplayMap, DisplayPoint, WrappingIndent};
pub use indent::TabSize;
pub use inline_tokens::{InlineToken, InlineTokenError, InlineTokenSpan, InputContent};
pub use input::{Input, InputState};
pub use kind::{InputMode, InputModeKind, MultiLineMode, TextareaMode};
pub use mask_pattern::MaskPattern;
pub use native::{NativeMenu, NativeMenuItem};
pub use rope_ext::{Point, Position, RopeExt, RopeLines};
pub use ropey::Rope;
pub use state::*;
pub use textarea::{Textarea, TextareaState};
pub(crate) use token_presentation::InlineTokenPresentation;
pub use token_presentation::{
    InlineTokenClickEvent, InlineTokenClickListener, InlineTokenContext, InlineTokenHoverEvent,
    InlineTokenHoverListener, InlineTokenRenderer,
};
