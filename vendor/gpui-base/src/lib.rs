//! Trimmed behavior and interaction foundations for Jot.

pub mod actions;
pub mod animation;
mod auto_scroll;
mod geometry;
mod global_state;
pub mod input;
pub mod motion;
mod reduce_motion;
mod scrollbar;
mod text_boundary;
mod theme;
pub mod theme_tokens;
mod touch_selection;

pub use auto_scroll::AutoScroll;
pub use geometry::AxisExt;
pub use global_state::{DeferredPopover, GlobalState};
pub use input::{Input, InputEditorStyle, Textarea};
pub use motion::{
    Discrete, DiscreteError, Easing, EasingError, Interpolate, IterationCount, Keyframe,
    KeyframeError, Keyframes, LinearStop, MotionPhase, MotionReveal, MotionStatus, MotionTransform,
    MotionValue, PlaybackDirection, Presence, PresencePhase, PresenceSample, Sequence,
    SequenceSample, SequenceStep, SignedDuration, Spring, SpringError, Stagger, StaggerOrigin,
    StepPosition, Timing, TimingSample, Transition, TransitionId, animate_keyframes, spring,
    transition, transition_with_status,
};
pub use reduce_motion::apply_system_reduce_motion;
pub use scrollbar::{
    Scrollbar, ScrollbarAxis, ScrollbarEntrance, ScrollbarHandle, ScrollbarMode, ScrollbarMotion,
    ScrollbarStyles, ScrollbarThumbStyle, ScrollbarTrackStyle,
};
pub use theme::{ScrollbarTheme, Theme, ThemeAppearance};
pub use theme_tokens::{ColorTokens, SemanticThemeTokens};
pub use touch_selection::{SelectionEdge, TouchHandle, TouchSelectionSnapshot};

use gpui::App;

/// Initializes global infrastructure owned by the base layer.
pub fn init(cx: &mut App) {
    let _ = Theme::global_mut(cx);
    GlobalState::init(cx);
    reduce_motion::init(cx);
    input::init(cx);
}
