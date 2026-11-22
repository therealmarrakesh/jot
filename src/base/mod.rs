//! Text editing, overlay scrollbars, and motion primitives.
#![allow(
    clippy::style,
    clippy::if_same_then_else,
    clippy::single_range_in_vec_init,
    clippy::type_complexity
)]

pub(crate) mod actions;
mod auto_scroll;
pub(crate) mod input;
pub(crate) mod motion;
mod reduce_motion;
mod scrollbar;
mod text_boundary;

pub(crate) use auto_scroll::AutoScroll;
pub(crate) use motion::{Easing, Interpolate, MotionStatus, transition, transition_from};
pub(crate) use scrollbar::Scrollbar;

use crate::ui::App;

pub(crate) fn init(cx: &mut App) {
    reduce_motion::init(cx);
    input::init(cx);
}
