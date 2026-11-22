use gpui::{App, Entity, IntoElement, RenderOnce, Window};

use super::{InputBaseState, InputMode};

/// State for a single-line text input.
pub type InputState = InputBaseState<InputMode>;

/// An unstyled single-line text input.
#[derive(IntoElement)]
pub struct Input {
    state: Entity<InputState>,
}

impl Input {
    pub fn new(state: &Entity<InputState>) -> Self {
        Self {
            state: state.clone(),
        }
    }
}

impl RenderOnce for Input {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        self.state
    }
}
