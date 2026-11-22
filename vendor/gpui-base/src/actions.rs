//! Common actions shared by keyboard-driven GPUI controls.

use gpui::{Action, actions};

#[derive(Clone, Action, PartialEq, Eq)]
#[action(namespace = ui, no_json)]
pub struct Confirm {
    /// Whether the confirmation uses the secondary action.
    pub secondary: bool,
}

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
