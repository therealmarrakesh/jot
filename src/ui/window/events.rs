//! Input, pointer, scroll, and keyboard event types for `Window`.

use crate::ui::geometry::{Pixels, Point, point};
use crate::ui::keymap::{Keystroke, Modifiers};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum MouseButton {
    Left,
    Right,
    Middle,
}

#[derive(Clone, Debug)]
pub(crate) struct MouseDownEvent {
    pub(crate) button: MouseButton,
    pub(crate) position: Point<Pixels>,
    pub(crate) modifiers: Modifiers,
    pub(crate) click_count: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct MouseUpEvent {
    pub(crate) button: MouseButton,
    pub(crate) position: Point<Pixels>,
    pub(crate) modifiers: Modifiers,
    pub(crate) click_count: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct MouseMoveEvent {
    pub(crate) position: Point<Pixels>,
    pub(crate) pressed_button: Option<MouseButton>,
    #[allow(dead_code)]
    pub(crate) modifiers: Modifiers,
}

impl MouseMoveEvent {
    #[inline]
    pub(crate) fn dragging(&self) -> bool {
        self.pressed_button.is_some()
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ClickEvent {
    pub(crate) button: MouseButton,
    #[allow(dead_code)]
    pub(crate) position: Point<Pixels>,
    #[allow(dead_code)]
    pub(crate) modifiers: Modifiers,
    #[allow(dead_code)]
    pub(crate) click_count: usize,
}

impl ClickEvent {
    #[inline]
    pub(crate) fn is_middle_click(&self) -> bool {
        self.button == MouseButton::Middle
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) enum ScrollDelta {
    Lines(Point<f32>),
    #[allow(dead_code)]
    Pixels(Point<Pixels>),
}

impl ScrollDelta {
    pub(crate) fn pixel_delta(self, line_height: Pixels) -> Point<Pixels> {
        match self {
            Self::Lines(l) => point(line_height * l.x, line_height * l.y),
            Self::Pixels(p) => p,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ScrollWheelEvent {
    pub(crate) position: Point<Pixels>,
    pub(crate) delta: ScrollDelta,
    pub(crate) modifiers: Modifiers,
}

#[derive(Clone, Debug)]
pub(crate) struct KeyDownEvent {
    pub(crate) keystroke: Keystroke,
    #[allow(dead_code)]
    pub(crate) is_held: bool,
}

#[derive(Copy, Clone, Debug)]
pub(crate) struct ModifiersChangedEvent {
    pub(crate) modifiers: Modifiers,
}

pub(crate) trait MouseEvent: 'static {}
impl MouseEvent for MouseDownEvent {}
impl MouseEvent for MouseUpEvent {}
impl MouseEvent for MouseMoveEvent {}
impl MouseEvent for ScrollWheelEvent {}
