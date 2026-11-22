use std::cell::Cell;

use windows::Win32::UI::WindowsAndMessaging::{
    SPI_GETWHEELSCROLLCHARS, SPI_GETWHEELSCROLLLINES, SYSTEM_PARAMETERS_INFO_ACTION,
};

use super::system_parameters_u32;

/// Windows settings pulled from SystemParametersInfo.
#[derive(Default, Debug, Clone)]
pub(crate) struct WindowsSystemSettings {
    pub(crate) mouse_wheel_settings: MouseWheelSettings,
}

#[derive(Default, Debug, Clone)]
pub(crate) struct MouseWheelSettings {
    pub(crate) wheel_scroll_chars: Cell<u32>,
    pub(crate) wheel_scroll_lines: Cell<u32>,
}

impl WindowsSystemSettings {
    pub(crate) fn new() -> Self {
        let settings = Self::default();
        settings.mouse_wheel_settings.update();
        settings
    }

    pub(crate) fn update(&self, wparam: usize) {
        match SYSTEM_PARAMETERS_INFO_ACTION(wparam as u32) {
            SPI_GETWHEELSCROLLLINES | SPI_GETWHEELSCROLLCHARS => self.mouse_wheel_settings.update(),
            _ => {}
        }
    }
}

impl MouseWheelSettings {
    fn update(&self) {
        if let Some(value) = system_parameters_u32(SPI_GETWHEELSCROLLCHARS.0) {
            self.wheel_scroll_chars.set(value);
        }
        if let Some(value) = system_parameters_u32(SPI_GETWHEELSCROLLLINES.0) {
            self.wheel_scroll_lines.set(value);
        }
    }
}
