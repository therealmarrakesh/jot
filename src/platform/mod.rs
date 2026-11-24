#![allow(unsafe_code)]

pub(crate) mod clipboard;
mod codepage;
pub(crate) mod diag;
mod dialogs;
mod display;
mod event;
mod fs;
mod handle;
mod input;
mod instance;
mod jobs;
mod pump;
mod settings;
mod window;
mod wndproc;

pub(crate) use codepage::AnsiCodePage;
pub(crate) use dialogs::{
    OpenDialogOptions, SaveChoice, SaveDialogOptions, SaveDialogResult, run_open_dialog,
    run_save_dialog,
};
pub(crate) use display::WindowsDisplay;
#[cfg(test)]
pub(crate) use event::hit_test_control_areas;
pub(crate) use event::{
    CursorStyle, MainWindowConfig, PlatformEvent, PlatformHandler, WindowAppearance,
    WindowControlArea, WindowId,
};
pub(crate) use fs::{
    HIDDEN_FILE_ATTRIBUTE, ReplaceFileError, documents_dir, file_hard_link_count, replace_file,
};
pub(crate) use handle::{
    WindowHandle, WindowPlacement, cursor_screen_pos, monitor_work_area_and_scale,
    primary_mouse_button_down, release_mouse_capture, top_visible_window_at,
};
pub(crate) use input::{Key, KeyEvent, Modifiers, MouseButton};
pub(crate) use instance::{Arbitration, arbitrate as arbitrate_instance};
pub(crate) use jobs::{Jobs, TimerHandle};
pub(crate) use pump::Platform;
pub(crate) use settings::{HighContrastColors, SystemSettings, format_local_time_date};
pub(crate) use window::NativeWindow;

use windows::Win32::{
    System::Diagnostics::Debug::OutputDebugStringW,
    UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW},
};
use windows::core::HSTRING;

pub(crate) fn output_debug_string(message: &str) {
    let wide = HSTRING::from(message);
    unsafe { OutputDebugStringW(&wide) };
}

pub(crate) fn message_box_error(title: &str, message: &str) {
    let title_w = HSTRING::from(title);
    let msg_w = HSTRING::from(message);
    unsafe {
        let _ = MessageBoxW(None, &msg_w, &title_w, MB_OK | MB_ICONERROR);
    }
}
