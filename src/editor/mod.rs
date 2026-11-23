//! Shared plain-text editing engine for single-line and multi-line inputs.

use crate::ui::App;

mod auto_scroll;
pub(crate) mod blink_cursor;
mod change;
mod cursor;
mod element;
mod indent;
mod layout;
mod movement;
mod rope_ext;
mod selection;
mod state;
mod text_boundary;
mod text_wrapper;
mod undo_manager;

#[cfg(test)]
pub(crate) use text_wrapper::{TextWrapper, WrapDisplayPoint};

pub(crate) fn init(cx: &mut App) {
    state::init(cx);
}

pub(crate) use cursor::Selection;
pub(crate) use indent::TabSize;
pub(crate) use rope_ext::{Point, Position, RopeExt};
pub(crate) use ropey::{Rope, RopeBuilder};
pub(crate) use state::*;
