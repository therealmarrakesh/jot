use std::path::PathBuf;

use crate::geom::{Point, Rect};
use crate::types::SharedStr;

use super::display::DisplayId;
use super::input::{KeyEvent, Modifiers, MouseButton};
use super::settings::SystemSettings;

crate::types::id_type!(WindowId);

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum WindowAppearance {
    #[default]
    Light,
    Dark,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum WindowControlArea {
    Client,
    Drag,
    Min,
    Max,
    Close,
}

pub(crate) fn hit_test_control_areas(
    areas: &[(Rect, WindowControlArea)],
    pos: Point,
) -> WindowControlArea {
    areas
        .iter()
        .rev()
        .find(|(rect, _)| rect.contains(pos))
        .map_or(WindowControlArea::Client, |(_, area)| *area)
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum CursorStyle {
    #[default]
    Arrow,
    IBeam,
}

#[derive(Clone, Debug)]
pub(crate) struct MainWindowConfig {
    pub(crate) bounds: Option<Rect>,
    pub(crate) title: SharedStr,
    pub(crate) display_id: Option<DisplayId>,
    pub(crate) focus: bool,
    pub(crate) show: bool,
}

impl Default for MainWindowConfig {
    fn default() -> Self {
        Self {
            bounds: None,
            title: SharedStr::new_static("Jot"),
            display_id: None,
            focus: true,
            show: true,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum PlatformEvent {
    Resized {
        window_id: WindowId,
        physical_width: u32,
        physical_height: u32,
        scale: f32,
    },
    Moved {
        window_id: WindowId,
    },
    Activated {
        window_id: WindowId,
        active: bool,
    },
    PaintRequested {
        window_id: WindowId,
    },
    SettingsChanged(SystemSettings),
    MouseMove {
        window_id: WindowId,
        position: Point,
        pressed_button: Option<MouseButton>,
    },
    MouseLeave {
        window_id: WindowId,
    },
    MouseDown {
        window_id: WindowId,
        button: MouseButton,
        position: Point,
        modifiers: Modifiers,
        click_count: usize,
    },
    MouseUp {
        window_id: WindowId,
        button: MouseButton,
        position: Point,
    },
    ScrollWheel {
        window_id: WindowId,
        position: Point,
        delta_lines: Point,
        page_scroll: bool,
        modifiers: Modifiers,
    },
    KeyDown {
        window_id: WindowId,
        event: KeyEvent,
    },
    KeyUp {
        window_id: WindowId,
        event: KeyEvent,
    },
    CaptureLost {
        window_id: WindowId,
    },
    ModifiersChanged {
        modifiers: Modifiers,
    },
    TextInput {
        window_id: WindowId,
        text: String,
    },
    ImeComposition {
        window_id: WindowId,
        text: String,
        cursor: usize,
    },
    FilesDropped {
        window_id: WindowId,
        paths: Vec<PathBuf>,
    },
    Launch {
        paths: Vec<PathBuf>,
    },
    SizeMoveChanged {
        window_id: WindowId,
        entering: bool,
    },
    CloseRequested {
        window_id: WindowId,
    },
    SessionEnding {
        ending: bool,
    },
}

pub(crate) trait PlatformHandler {
    fn on_event(&mut self, event: PlatformEvent) -> bool;
    fn on_wake(&mut self);
    fn drive_frames(&mut self);
}
