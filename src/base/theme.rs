use gpui::{App, Global};

use crate::base::{ScrollbarMode, ScrollbarMotion, ScrollbarStyles, SemanticThemeTokens};

/// Application-wide defaults for Base behavior modules.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThemeAppearance {
    #[default]
    Light,
    Dark,
}

#[derive(Clone, Default)]
pub struct Theme {
    pub appearance: ThemeAppearance,
    pub tokens: SemanticThemeTokens,
    pub scrollbar: ScrollbarTheme,
}

impl Global for Theme {}

impl Theme {
    pub fn global(cx: &App) -> Self {
        cx.try_global::<Self>().cloned().unwrap_or_default()
    }

    pub fn global_mut(cx: &mut App) -> &mut Self {
        if !cx.has_global::<Self>() {
            cx.set_global(Self::default());
        }
        cx.global_mut::<Self>()
    }
}

/// Access to the active base theme through an application context.
pub(crate) trait ActiveTheme {
    fn theme(&self) -> Theme;
}

impl ActiveTheme for App {
    #[inline(always)]
    fn theme(&self) -> Theme {
        Theme::global(self)
    }
}

/// Global defaults used by [`crate::Scrollbar`].
#[derive(Clone, Default)]
pub struct ScrollbarTheme {
    mode: ScrollbarMode,
    motion: ScrollbarMotion,
    styles: ScrollbarStyles,
}

impl ScrollbarTheme {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_mode(mut self, mode: ScrollbarMode) -> Self {
        self.mode = mode;
        self
    }

    pub fn with_motion(mut self, motion: ScrollbarMotion) -> Self {
        self.motion = motion;
        self
    }

    pub fn with_styles(mut self, styles: ScrollbarStyles) -> Self {
        self.styles = styles;
        self
    }

    pub fn mode(&self) -> ScrollbarMode {
        self.mode
    }

    pub fn motion(&self) -> ScrollbarMotion {
        self.motion
    }

    pub fn styles(&self) -> &ScrollbarStyles {
        &self.styles
    }
}
