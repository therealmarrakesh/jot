use crate::ui::{Capslock, Keystroke, Modifiers, Pixels, Point, point, seal::Sealed};
use crate::ui::smallvec::SmallVec;
use std::{any::Any, fmt::Debug, ops::Deref, path::PathBuf};

/// An event from a platform input source.
pub(crate) trait InputEvent: Sealed + 'static {}

/// A key event from the platform.
pub(crate) trait KeyEvent: InputEvent {}

/// A mouse event from the platform.
pub(crate) trait MouseEvent: InputEvent {}

/// The key down event equivalent for the platform.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct KeyDownEvent {
    pub(crate) keystroke: Keystroke,
    pub(crate) is_held: bool,
    pub(crate) prefer_character_input: bool,
}

impl Sealed for KeyDownEvent {}
impl InputEvent for KeyDownEvent {}
impl KeyEvent for KeyDownEvent {}

/// The key up event equivalent for the platform.
#[derive(Clone, Debug)]
pub(crate) struct KeyUpEvent {
    pub(crate) keystroke: Keystroke,
}

impl Sealed for KeyUpEvent {}
impl InputEvent for KeyUpEvent {}
impl KeyEvent for KeyUpEvent {}

/// The modifiers changed event equivalent for the platform.
#[derive(Clone, Debug, Default)]
pub(crate) struct ModifiersChangedEvent {
    pub(crate) modifiers: Modifiers,
    pub(crate) capslock: Capslock,
}

impl Sealed for ModifiersChangedEvent {}
impl InputEvent for ModifiersChangedEvent {}
impl KeyEvent for ModifiersChangedEvent {}

impl Deref for ModifiersChangedEvent {
    type Target = Modifiers;

    fn deref(&self) -> &Self::Target {
        &self.modifiers
    }
}

/// A mouse down event from the platform.
#[derive(Clone, Debug, Default)]
pub(crate) struct MouseDownEvent {
    pub(crate) button: MouseButton,
    pub(crate) position: Point<Pixels>,
    pub(crate) modifiers: Modifiers,
    pub(crate) click_count: usize,
}

impl Sealed for MouseDownEvent {}
impl InputEvent for MouseDownEvent {}
impl MouseEvent for MouseDownEvent {}

/// A mouse up event from the platform.
#[derive(Clone, Debug, Default)]
pub(crate) struct MouseUpEvent {
    pub(crate) button: MouseButton,
    pub(crate) position: Point<Pixels>,
    pub(crate) modifiers: Modifiers,
}

impl Sealed for MouseUpEvent {}
impl InputEvent for MouseUpEvent {}
impl MouseEvent for MouseUpEvent {}

/// A click event, generated when a mouse button is pressed and released.
#[derive(Clone, Debug, Default)]
pub(crate) struct MouseClickEvent {
    pub(crate) down: MouseDownEvent,
    pub(crate) up: MouseUpEvent,
}

/// A click event, generated when a mouse button or keyboard button is pressed and released.
#[derive(Clone, Debug, Default)]
pub(crate) enum ClickEvent {
    Mouse(MouseClickEvent),
    #[default]
    Keyboard,
}

impl ClickEvent {
    pub(crate) fn is_middle_click(&self) -> bool {
        match self {
            ClickEvent::Keyboard => false,
            ClickEvent::Mouse(event) => {
                event.down.button == MouseButton::Middle && event.up.button == MouseButton::Middle
            }
        }
    }
}

/// An enum representing the mouse button that was pressed.
#[derive(Hash, Default, PartialEq, Eq, Copy, Clone, Debug)]
pub(crate) enum MouseButton {
    #[default]
    Left,
    Right,
    Middle,
    Navigate(NavigationDirection),
}

/// A navigation direction, such as back or forward.
#[derive(Hash, Default, PartialEq, Eq, Copy, Clone, Debug)]
pub(crate) enum NavigationDirection {
    #[default]
    Back,
    Forward,
}

/// A mouse move event from the platform.
#[derive(Clone, Debug, Default)]
pub(crate) struct MouseMoveEvent {
    pub(crate) position: Point<Pixels>,
    pub(crate) pressed_button: Option<MouseButton>,
    pub(crate) modifiers: Modifiers,
}

impl Sealed for MouseMoveEvent {}
impl InputEvent for MouseMoveEvent {}
impl MouseEvent for MouseMoveEvent {}

impl MouseMoveEvent {
    /// Returns true if the left mouse button is currently held down.
    pub(crate) fn dragging(&self) -> bool {
        self.pressed_button == Some(MouseButton::Left)
    }
}

/// A mouse wheel event from the platform.
#[derive(Clone, Debug, Default)]
pub(crate) struct ScrollWheelEvent {
    pub(crate) position: Point<Pixels>,
    pub(crate) delta: ScrollDelta,
    pub(crate) modifiers: Modifiers,
}

impl Sealed for ScrollWheelEvent {}
impl InputEvent for ScrollWheelEvent {}
impl MouseEvent for ScrollWheelEvent {}

impl Deref for ScrollWheelEvent {
    type Target = Modifiers;

    fn deref(&self) -> &Self::Target {
        &self.modifiers
    }
}

/// The scroll delta for a scroll wheel event.
#[derive(Clone, Copy, Debug)]
pub(crate) enum ScrollDelta {
    Pixels(Point<Pixels>),
    Lines(Point<f32>),
}

impl Default for ScrollDelta {
    fn default() -> Self {
        Self::Lines(Default::default())
    }
}

impl ScrollDelta {
    /// Converts this scroll event into exact pixels.
    pub(crate) fn pixel_delta(&self, line_height: Pixels) -> Point<Pixels> {
        match self {
            ScrollDelta::Pixels(delta) => *delta,
            ScrollDelta::Lines(delta) => point(line_height * delta.x, line_height * delta.y),
        }
    }
}

/// A collection of paths from the platform, such as from a file drop.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub(crate) struct ExternalPaths(pub(crate) SmallVec<[PathBuf; 2]>);

impl ExternalPaths {
    pub(crate) fn paths(&self) -> &[PathBuf] {
        &self.0
    }
}

/// A file drop event from the platform, generated when files are dragged and dropped onto the window.
#[derive(Debug, Clone)]
pub(crate) enum FileDropEvent {
    Entered {
        position: Point<Pixels>,
        paths: ExternalPaths,
    },
    Pending {
        position: Point<Pixels>,
    },
    Submit {
        position: Point<Pixels>,
    },
    Exited,
}

impl Sealed for FileDropEvent {}
impl InputEvent for FileDropEvent {}
impl MouseEvent for FileDropEvent {}

/// An enum corresponding to all kinds of platform input events.
#[derive(Clone, Debug)]
pub(crate) enum PlatformInput {
    KeyDown(KeyDownEvent),
    KeyUp(KeyUpEvent),
    ModifiersChanged(ModifiersChangedEvent),
    MouseDown(MouseDownEvent),
    MouseUp(MouseUpEvent),
    MouseMove(MouseMoveEvent),
    ScrollWheel(ScrollWheelEvent),
    FileDrop(FileDropEvent),
}

impl PlatformInput {
    pub(crate) fn mouse_event(&self) -> Option<&dyn Any> {
        match self {
            PlatformInput::KeyDown { .. } => None,
            PlatformInput::KeyUp { .. } => None,
            PlatformInput::ModifiersChanged { .. } => None,
            PlatformInput::MouseDown(event) => Some(event),
            PlatformInput::MouseUp(event) => Some(event),
            PlatformInput::MouseMove(event) => Some(event),
            PlatformInput::ScrollWheel(event) => Some(event),
            PlatformInput::FileDrop(event) => Some(event),
        }
    }

    pub(crate) fn keyboard_event(&self) -> Option<&dyn Any> {
        match self {
            PlatformInput::KeyDown(event) => Some(event),
            PlatformInput::KeyUp(event) => Some(event),
            PlatformInput::ModifiersChanged(event) => Some(event),
            PlatformInput::MouseDown(_) => None,
            PlatformInput::MouseUp(_) => None,
            PlatformInput::MouseMove(_) => None,
            PlatformInput::ScrollWheel(_) => None,
            PlatformInput::FileDrop(_) => None,
        }
    }
}
