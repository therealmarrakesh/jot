//! Semantic design tokens shared by application-owned components.

use gpui::{Hsla, hsla, rgb};

#[derive(Debug, Default, Clone, PartialEq)]
pub struct SemanticThemeTokens {
    pub colors: ColorTokens,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorTokens {
    pub background: Hsla,
    pub foreground: Hsla,
    pub surface: Hsla,
    pub muted_foreground: Hsla,
    pub accent: Hsla,
    pub border: Hsla,
    pub selection: Hsla,
}

impl Default for ColorTokens {
    fn default() -> Self {
        Self::light()
    }
}

impl ColorTokens {
    pub fn light() -> Self {
        Self {
            background: hsla(0., 0., 1., 1.),
            foreground: hsla(0., 0., 0.039, 1.),
            surface: hsla(0., 0., 1., 1.),
            muted_foreground: hsla(0., 0., 0.451, 1.),
            accent: hsla(0., 0., 0.961, 1.),
            border: hsla(0., 0., 0.898, 1.),
            selection: Hsla::from(rgb(0x55a0fc)).alpha(0.3),
        }
    }

    pub fn dark() -> Self {
        Self {
            background: hsla(0., 0., 0.039, 1.),
            foreground: hsla(0., 0., 0.98, 1.),
            surface: hsla(0., 0., 0.039, 1.),
            muted_foreground: hsla(0., 0., 0.639, 1.),
            accent: hsla(0., 0., 0.149, 1.),
            border: hsla(0., 0., 0.149, 1.),
            selection: Hsla::from(rgb(0x1d4ed8)).alpha(0.3),
        }
    }
}
