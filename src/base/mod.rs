//! Text editing, overlay scrollbars, and motion primitives.
#![allow(
    dead_code,
    clippy::style,
    clippy::if_same_then_else,
    clippy::single_range_in_vec_init,
    clippy::type_complexity
)]

pub mod actions;
mod auto_scroll;
pub mod input;
pub mod motion;
mod reduce_motion;
mod scrollbar;
mod text_boundary;

pub use auto_scroll::AutoScroll;
pub use motion::{Easing, Interpolate, MotionStatus, transition, transition_from};
pub(crate) use scrollbar::Scrollbar;

use gpui::App;

pub fn init(cx: &mut App) {
    reduce_motion::init(cx);
    input::init(cx);
}
