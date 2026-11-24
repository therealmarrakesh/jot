#![allow(unsafe_code)]

pub(crate) mod clipboard;
mod codepage;
mod dialogs;
mod fs;
mod handle;
mod instance;
mod jobs;
mod pump;
mod settings;
mod window;

use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;

use handle::DisplayId;
use windows::Win32::{
    Foundation::{HWND, POINT, RECT},
    System::Diagnostics::Debug::OutputDebugStringW,
    UI::{
        Input::{
            Ime::{
                CANDIDATEFORM, CFS_CANDIDATEPOS, CFS_POINT, COMPOSITIONFORM, CPS_COMPLETE, HIMC,
                IME_COMPOSITION_STRING, ImmGetCompositionStringW, ImmGetContext, ImmNotifyIME,
                ImmReleaseContext, ImmSetCandidateWindow, ImmSetCompositionWindow,
                NI_COMPOSITIONSTR,
            },
            KeyboardAndMouse::{
                GetDoubleClickTime, GetKeyState, VIRTUAL_KEY, VK_ADD, VK_APPS, VK_BACK, VK_CONTROL,
                VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE, VK_F1, VK_F4, VK_F12, VK_HOME, VK_INSERT,
                VK_LCONTROL, VK_LEFT, VK_LWIN, VK_MENU, VK_NEXT, VK_NUMPAD0, VK_NUMPAD9, VK_OEM_4,
                VK_OEM_6, VK_OEM_MINUS, VK_OEM_PLUS, VK_PRIOR, VK_RCONTROL, VK_RETURN, VK_RIGHT,
                VK_RMENU, VK_RWIN, VK_SHIFT, VK_SPACE, VK_SUBTRACT, VK_TAB, VK_UP,
            },
        },
        WindowsAndMessaging::{
            GetMessageTime, GetSystemMetrics, MB_ICONERROR, MB_OK, MessageBoxW, SM_CXDOUBLECLK,
            SM_CYDOUBLECLK,
        },
    },
};
use windows::core::HSTRING;

use crate::geom::{Point, Rect};
use crate::types::SharedStr;

pub(crate) use codepage::AnsiCodePage;
pub(crate) use dialogs::{
    OpenDialogOptions, SaveChoice, SaveDialogOptions, SaveDialogResult, run_open_dialog,
    run_save_dialog,
};
pub(crate) use fs::{
    HIDDEN_FILE_ATTRIBUTE, ReplaceFileError, documents_dir, file_hard_link_count, replace_file,
};
pub(crate) use handle::{
    WindowHandle, WindowPlacement, WindowsDisplay, cursor_screen_pos, monitor_work_area_and_scale,
    primary_mouse_button_down, release_mouse_capture, top_visible_window_at,
};
pub(crate) use instance::{Arbitration, arbitrate as arbitrate_instance};
pub(crate) use jobs::{Jobs, TimerHandle};
pub(crate) use pump::Platform;
pub(crate) use settings::{HighContrastColors, SystemSettings, format_local_time_date};
pub(crate) use window::NativeWindow;

#[cfg(not(test))]
use std::sync::atomic::{AtomicBool, Ordering};

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

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct Modifiers {
    pub(crate) ctrl: bool,
    pub(crate) alt: bool,
    pub(crate) shift: bool,
    pub(crate) win: bool,
    pub(crate) alt_gr: bool,
}

impl Modifiers {
    #[inline]
    pub(crate) const fn none(self) -> bool {
        !self.ctrl && !self.alt && !self.shift && !self.win
    }

    #[inline]
    pub(crate) const fn ctrl_only(self) -> bool {
        self.ctrl && !self.alt && !self.shift && !self.win && !self.alt_gr
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Key {
    Char(char),
    Backspace,
    Tab,
    Enter,
    Escape,
    Space,
    PageUp,
    PageDown,
    End,
    Home,
    Left,
    Up,
    Right,
    Down,
    Insert,
    Delete,
    Alt,
    Menu,
    F(u8),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct KeyEvent {
    pub(crate) key: Key,
    pub(crate) modifiers: Modifiers,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum MouseButton {
    Left,
    Right,
    Middle,
}

#[derive(Default)]
struct ClickTracker {
    last_button: Option<MouseButton>,
    last_pos: (i32, i32),
    last_time_ms: Option<u32>,
    count: usize,
}

impl ClickTracker {
    fn record(&mut self, button: MouseButton, x: i32, y: i32) -> usize {
        let now_ms = unsafe { GetMessageTime() } as u32;
        let dbl_ms = unsafe { GetDoubleClickTime() };
        let tol_x = (unsafe { GetSystemMetrics(SM_CXDOUBLECLK) } / 2).max(2);
        let tol_y = (unsafe { GetSystemMetrics(SM_CYDOUBLECLK) } / 2).max(2);
        let same_spot = self.last_button == Some(button)
            && self
                .last_time_ms
                .is_some_and(|t| now_ms.wrapping_sub(t) <= dbl_ms)
            && (x - self.last_pos.0).abs() <= tol_x
            && (y - self.last_pos.1).abs() <= tol_y;
        self.count = if same_spot { self.count + 1 } else { 1 };
        self.last_button = Some(button);
        self.last_pos = (x, y);
        self.last_time_ms = Some(now_ms);
        self.count
    }
}

pub(crate) fn current_modifiers() -> Modifiers {
    let down = |vk: VIRTUAL_KEY| (unsafe { GetKeyState(i32::from(vk.0)) } as u16 & 0x8000) != 0;
    let lctrl = down(VK_LCONTROL);
    let rctrl = down(VK_RCONTROL);
    let ralt = down(VK_RMENU);
    // Windows synthesizes VK_LCONTROL + VK_RMENU for AltGr.
    let alt_gr = ralt && lctrl && !rctrl;
    Modifiers {
        ctrl: down(VK_CONTROL),
        alt: down(VK_MENU),
        shift: down(VK_SHIFT),
        win: down(VK_LWIN) || down(VK_RWIN),
        alt_gr,
    }
}

fn is_syskey_passthrough(vk: u16) -> bool {
    vk == VK_F4.0 || vk == VK_SPACE.0
}

fn translate_vk(vk: u16, modifiers: Modifiers) -> Option<Key> {
    let v = VIRTUAL_KEY(vk);
    let key = match v {
        VK_BACK => Key::Backspace,
        VK_TAB => Key::Tab,
        VK_RETURN => Key::Enter,
        VK_ESCAPE => Key::Escape,
        VK_SPACE => Key::Space,
        VK_PRIOR => Key::PageUp,
        VK_NEXT => Key::PageDown,
        VK_END => Key::End,
        VK_HOME => Key::Home,
        VK_LEFT => Key::Left,
        VK_UP => Key::Up,
        VK_RIGHT => Key::Right,
        VK_DOWN => Key::Down,
        VK_INSERT => Key::Insert,
        VK_DELETE => Key::Delete,
        VK_MENU => Key::Alt,
        VK_APPS => Key::Menu,
        VK_OEM_PLUS => Key::Char(if modifiers.shift { '+' } else { '=' }),
        VK_ADD => Key::Char('+'),
        VK_OEM_MINUS | VK_SUBTRACT => Key::Char('-'),
        VK_OEM_4 => Key::Char('['),
        VK_OEM_6 => Key::Char(']'),
        _ if (VK_F1.0..=VK_F12.0).contains(&vk) => Key::F((vk - VK_F1.0 + 1) as u8),
        _ if (VK_NUMPAD0.0..=VK_NUMPAD9.0).contains(&vk) => {
            Key::Char((b'0' + (vk - VK_NUMPAD0.0) as u8) as char)
        }
        _ if (b'0' as u16..=b'9' as u16).contains(&vk) => Key::Char(vk as u8 as char),
        _ if (b'A' as u16..=b'Z' as u16).contains(&vk) => {
            Key::Char((vk as u8 as char).to_ascii_lowercase())
        }
        _ => return None,
    };
    Some(key)
}

fn read_ime_string(himc: HIMC, flag: IME_COMPOSITION_STRING) -> Option<String> {
    let bytes = unsafe { ImmGetCompositionStringW(himc, flag, None, 0) };
    if bytes <= 0 {
        return None;
    }
    let len = (bytes as usize) / 2;
    let mut buf = vec![0u16; len];
    let written = unsafe {
        ImmGetCompositionStringW(himc, flag, Some(buf.as_mut_ptr().cast()), bytes as u32)
    };
    if written <= 0 {
        return None;
    }
    String::from_utf16(&buf[..(written as usize) / 2]).ok()
}

fn set_ime_candidate_pos(hwnd: HWND, caret_rect: Rect, scale: f32) {
    let pt = POINT {
        x: (caret_rect.origin.x * scale).round() as i32,
        y: (caret_rect.bottom() * scale).round() as i32,
    };
    let himc = unsafe { ImmGetContext(hwnd) };
    if himc.is_invalid() {
        return;
    }
    let comp_form = COMPOSITIONFORM {
        dwStyle: CFS_POINT,
        ptCurrentPos: pt,
        rcArea: RECT::default(),
    };
    let cand_form = CANDIDATEFORM {
        dwIndex: 0,
        dwStyle: CFS_CANDIDATEPOS,
        ptCurrentPos: pt,
        rcArea: RECT::default(),
    };
    unsafe {
        let _ = ImmSetCompositionWindow(himc, &comp_form);
        let _ = ImmSetCandidateWindow(himc, &cand_form);
        let _ = ImmReleaseContext(hwnd, himc);
    }
}

pub(crate) fn complete_ime_composition(hwnd: HWND) {
    let himc = unsafe { ImmGetContext(hwnd) };
    if himc.is_invalid() {
        return;
    }
    unsafe {
        let _ = ImmNotifyIME(himc, NI_COMPOSITIONSTR, CPS_COMPLETE, 0);
        let _ = ImmReleaseContext(hwnd, himc);
    }
}

pub(crate) fn debug_log(message: &str) {
    output_debug_string(&format!("[Jot] {message}\n"));
}

pub(crate) fn show_error_dialog(title: &str, message: &str) {
    message_box_error(title, message);
}

pub(crate) fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "Internal error".to_owned()
    }
}

pub(crate) fn install_panic_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let msg = panic_message(info.payload());
        let loc = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "<unknown>".to_owned());
        debug_log(&format!("PANIC at {loc}: {msg}"));
        prev(info);
    }));
}

pub(crate) fn ffi_guard<R>(context: &str, fallback: R, f: impl FnOnce() -> R) -> R {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(res) => res,
        Err(payload) => {
            let msg = panic_message(&*payload);
            debug_log(&format!("Caught panic at Win32 boundary ({context}): {msg}"));
            #[cfg(not(test))]
            {
                static SHOWN: AtomicBool = AtomicBool::new(false);
                if !SHOWN.swap(true, Ordering::AcqRel) {
                    show_error_dialog(
                        crate::APP_NAME,
                        &format!(
                            "Jot encountered an unexpected error and prevented a crash.\n\n\
                             Please save any open documents and restart Jot.\n\nDetails: {msg}"
                        ),
                    );
                }
            }
            fallback
        }
    }
}
