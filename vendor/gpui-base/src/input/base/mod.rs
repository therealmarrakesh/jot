use gpui::Hsla;

use crate::SemanticThemeTokens;

/// What the input can offer to its context menu, at the moment it is opened.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InputContextMenuCapabilities {
    disabled: bool,
    readonly: bool,
    selection: bool,
    masked: bool,
}

impl InputContextMenuCapabilities {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn readonly(mut self, readonly: bool) -> Self {
        self.readonly = readonly;
        self
    }

    pub fn selection(mut self, selection: bool) -> Self {
        self.selection = selection;
        self
    }

    pub fn masked(mut self, masked: bool) -> Self {
        self.masked = masked;
        self
    }

    pub fn is_disabled(&self) -> bool {
        self.disabled
    }

    pub fn is_readonly(&self) -> bool {
        self.readonly
    }

    pub fn is_editable(&self) -> bool {
        !self.disabled && !self.readonly
    }

    pub fn has_selection(&self) -> bool {
        self.selection
    }

    pub fn is_masked(&self) -> bool {
        self.masked
    }

    pub fn is_copyable(&self) -> bool {
        self.selection && !self.masked
    }
}

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
