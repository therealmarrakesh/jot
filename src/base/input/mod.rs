//! Text input: the shared editing engine for single-line and multi-line plain text.

use gpui::App;

mod base;
#[path = "base/blink_cursor.rs"]
pub(crate) mod blink_cursor;
#[path = "base/change.rs"]
mod change;
#[path = "base/cursor.rs"]
mod cursor;
#[path = "editor/display_map/mod.rs"]
mod display_map;
#[cfg(test)]
pub(crate) use display_map::{TextWrapper, WrapDisplayPoint};
#[path = "base/element.rs"]
mod element;
#[path = "editor/indent.rs"]
mod indent;
mod input;
#[path = "base/kind.rs"]
mod kind;
#[path = "base/layout.rs"]
mod layout;
#[path = "base/mode.rs"]
mod mode;
#[path = "base/movement.rs"]
mod movement;
#[path = "base/rope_ext.rs"]
mod rope_ext;
#[path = "base/selection.rs"]
mod selection;
#[path = "base/state.rs"]
mod state;
mod textarea;
#[path = "base/undo_manager.rs"]
mod undo_manager;

pub(crate) fn init(cx: &mut App) {
    state::init(cx);
}

pub use base::InputEditorStyle;
pub use cursor::Selection;
pub use display_map::{BufferPoint, DisplayMap, WrappingIndent};
pub use indent::TabSize;
pub use input::{Input, InputState};
pub use kind::{InputMode, InputModeKind, MultiLineMode, TextareaMode};
pub use rope_ext::{Point, Position, RopeExt};
pub use ropey::{Rope, RopeBuilder};
pub use state::*;
pub use textarea::{Textarea, TextareaState};
