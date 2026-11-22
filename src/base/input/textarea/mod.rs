use gpui::{App, Entity, IntoElement, RenderOnce, Window};

use super::{InputBaseState, TextareaMode};

/// State for editing ordinary multi-line text.
pub type TextareaState = InputBaseState<TextareaMode>;

/// An unstyled ordinary multi-line text input.
pub struct Textarea {
    state: Entity<TextareaState>,
}

impl IntoElement for Textarea {
    type Element = gpui::ViewElement<Self>;

    fn into_element(self) -> Self::Element {
        gpui::ViewElement::new(self)
    }
}

impl Textarea {
    pub fn new(state: &Entity<TextareaState>) -> Self {
        Self {
            state: state.clone(),
        }
    }
}

impl RenderOnce for Textarea {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        self.state
    }
}
