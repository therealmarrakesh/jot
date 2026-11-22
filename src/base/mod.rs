//! Text editing, scrollbars, and motion foundations (adapted from `gpui-base`, Apache-2.0).
#![allow(
    dead_code,
    clippy::style,
    clippy::if_same_then_else,
    clippy::single_range_in_vec_init,
    clippy::type_complexity
)]

pub mod actions;
pub mod animation;
mod auto_scroll;
mod geometry;
pub mod input;
pub mod motion;
mod reduce_motion;
mod scrollbar;
mod text_boundary;
mod theme;
pub mod theme_tokens;

pub use auto_scroll::AutoScroll;
pub use geometry::AxisExt;
pub use motion::{
    Easing, Interpolate, MotionStatus, Presence, Sequence, Transition, TransitionId, transition,
    transition_with_status,
};
pub use scrollbar::{
    Scrollbar, ScrollbarEntrance, ScrollbarMode, ScrollbarMotion, ScrollbarStyles,
};
pub use theme::{ScrollbarTheme, Theme, ThemeAppearance};
pub use theme_tokens::{ColorTokens, SemanticThemeTokens};

use gpui::App;

/// Initializes global infrastructure owned by the base layer.
pub fn init(cx: &mut App) {
    let _ = Theme::global_mut(cx);
    reduce_motion::init(cx);
    input::init(cx);
}
