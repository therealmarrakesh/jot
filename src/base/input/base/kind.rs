//! Compile-time input modes.

/// A single-line text field: the mode of [`crate::input::InputState`].
pub struct InputMode;

/// Ordinary multi-line text: the mode of [`crate::input::TextareaState`].
pub struct TextareaMode;

mod sealed {
    pub trait Sealed {}
}

impl sealed::Sealed for InputMode {}
impl sealed::Sealed for TextareaMode {}

/// The modes whose layout spans more than one line: [`TextareaMode`].
pub trait MultiLineMode: InputModeKind {}

impl MultiLineMode for TextareaMode {}

/// Marker trait distinguishing single-line and multi-line input modes.
pub trait InputModeKind: sealed::Sealed + Sized + 'static {
    const MULTI_LINE: bool;
}

impl InputModeKind for InputMode {
    const MULTI_LINE: bool = false;
}

impl InputModeKind for TextareaMode {
    const MULTI_LINE: bool = true;
}
