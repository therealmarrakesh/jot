//! Common actions shared by keyboard-driven GPUI controls.

use crate::ui::actions;

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Confirm {
    /// Whether the confirmation uses the secondary action.
    pub(crate) secondary: bool,
}
crate::ui::impl_action!(ui, Confirm);

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
