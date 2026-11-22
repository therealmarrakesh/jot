use gpui::Hsla;

use crate::base::SemanticThemeTokens;

/// Application-owned colors consumed by input painting.
#[derive(Clone, Default)]
pub struct InputEditorStyle {
    pub foreground: Hsla,
    pub muted_foreground: Hsla,
    pub background: Hsla,
    pub border: Hsla,
    pub selection: Hsla,
    pub caret: Option<Hsla>,
}

impl InputEditorStyle {
    pub fn resolved(&self, tokens: &SemanticThemeTokens) -> Self {
        let colors = &tokens.colors;
        let unset = |value: Hsla| value.a == 0.;
        let or = |value: Hsla, fallback: Hsla| if unset(value) { fallback } else { value };

        Self {
            foreground: or(self.foreground, colors.foreground),
            muted_foreground: or(self.muted_foreground, colors.muted_foreground),
            background: or(self.background, colors.surface),
            border: or(self.border, colors.border),
            selection: or(self.selection, colors.selection),
            caret: self.caret,
        }
    }
}
