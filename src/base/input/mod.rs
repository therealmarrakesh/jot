//! Text input: the shared editing engine for single-line and multi-line plain text.

use gpui::App;

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
#[path = "base/layout.rs"]
mod layout;
#[path = "base/movement.rs"]
mod movement;
#[path = "base/rope_ext.rs"]
mod rope_ext;
#[path = "base/selection.rs"]
mod selection;
#[path = "base/state.rs"]
mod state;
#[path = "base/undo_manager.rs"]
mod undo_manager;

pub(crate) fn init(cx: &mut App) {
    state::init(cx);
}

pub use cursor::Selection;
pub use display_map::WrappingIndent;
pub use indent::TabSize;
pub use rope_ext::{Point, Position, RopeExt};
pub use ropey::{Rope, RopeBuilder};
pub use state::*;
