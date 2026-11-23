//! Clean-room Win32 + Direct2D 1.1 + DirectWrite UI core.

pub(crate) mod anim;
pub(crate) mod app;
pub(crate) mod color;
pub(crate) mod div;
pub(crate) mod element;
pub(crate) mod entity;
pub(crate) mod focus;
pub(crate) mod geometry;
pub(crate) mod graphics;
pub(crate) mod jobs;
pub(crate) mod keymap;
pub(crate) mod platform;
pub(crate) mod style;
pub(crate) mod types;
pub(crate) mod window;

pub(crate) use anim::{AnimStatus, Easing, animate, animate_from};
pub(crate) use app::*;
pub(crate) use color::*;
pub(crate) use div::*;
pub(crate) use element::*;
pub(crate) use entity::*;
pub(crate) use focus::*;
pub(crate) use geometry::*;
pub(crate) use graphics::d2d_renderer::{BoxShadow, fill};
#[cfg(test)]
pub(crate) use graphics::dwrite::TextSystem;
pub(crate) use graphics::dwrite::{
    Boundary, Font, FontRun, FontWeight, MAX_WRAP_INDENT, ShapedLine, TextAlign, TextRun,
    UnderlineStyle, font,
};
pub(crate) use jobs::TimerHandle;
pub(crate) use keymap::*;
pub(crate) use platform::{
    CursorStyle, ExternalPaths, PathPromptOptions, SaveChoice, SavePathOptions, TitlebarOptions,
    WindowAppearance, WindowBackgroundAppearance, WindowBounds, WindowControlArea, WindowKind,
    WindowOptions, WindowsPlatform, set_window_min_size,
};
pub(crate) use style::*;
pub(crate) use types::{SharedString, UiError, UiResult};
pub(crate) use window::*;

pub(crate) use crate::layout::{AvailableSpace, LayoutId};
pub(crate) use crate::win32::{ScreenPoint, ScreenRect, WindowHwnd};

/// Clean-room symbols re-exported through `crate::win32` when `--features d2d` is active.
pub(crate) mod win32_bridge {
    pub(crate) use super::graphics::dwrite::line_break_opportunities;
}
