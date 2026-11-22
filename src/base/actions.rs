//! Common actions shared by keyboard-driven GPUI controls.

use gpui::actions;

#[derive(Clone, PartialEq, Eq)]
pub struct Confirm {
    /// Whether the confirmation uses the secondary action.
    pub secondary: bool,
}
gpui::impl_action!(ui, Confirm);

actions!(
    ui,
    [
        Cancel,
        SelectUp,
        SelectDown,
        SelectLeft,
        SelectRight,
    ]
);
