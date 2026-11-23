//! Safe, typed wrappers around Win32 window, desktop, system-setting, shell, and ANSI APIs.

use std::{
    ffi::OsString,
    io,
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
    time::Duration,
};

use windows::Win32::{
    Foundation::{ERROR_UNABLE_TO_MOVE_REPLACEMENT_2, HWND, LPARAM, POINT, RECT, WPARAM},
    Globalization::{
        CP_UTF8, DATE_SHORTDATE, GetACP, GetDateFormatEx, GetTimeFormatEx,
        MULTI_BYTE_TO_WIDE_CHAR_FLAGS, MultiByteToWideChar, TIME_NOSECONDS, WC_NO_BEST_FIT_CHARS,
        WideCharToMultiByte,
    },
    Graphics::{
        Dwm::{DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute},
        Gdi::{
            ClientToScreen, GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO,
            MonitorFromPoint, PtInRect,
        },
    },
    Storage::FileSystem::{
        REPLACEFILE_IGNORE_ACL_ERRORS, REPLACEFILE_IGNORE_MERGE_ERRORS, ReplaceFileW,
    },
    System::{
        Com::CoTaskMemFree,
        Registry::{HKEY_CURRENT_USER, RRF_RT_REG_BINARY, RegGetValueW},
        SystemInformation::GetLocalTime,
    },
    UI::{
        HiDpi::{GetDpiForMonitor, GetDpiForWindow, MDT_EFFECTIVE_DPI},
        Input::KeyboardAndMouse::{
            GetAsyncKeyState, ReleaseCapture, SetCapture, VK_LBUTTON, VK_RBUTTON,
        },
        Shell::{FOLDERID_Documents, KF_FLAG_DEFAULT, SHGetKnownFolderPath},
        WindowsAndMessaging::{
            EnumWindows, GUI_INMOVESIZE, GUITHREADINFO, GWL_EXSTYLE, GetCaretBlinkTime,
            GetClientRect, GetCursorPos, GetGUIThreadInfo, GetSystemMetrics, GetWindowLongW,
            GetWindowPlacement, GetWindowRect, GetWindowThreadProcessId, HTCAPTION, HWND_TOPMOST,
            IsIconic, IsWindowVisible, IsZoomed, PostMessageW, SC_MOVE, SM_SWAPBUTTON,
            SPI_GETCLIENTAREAANIMATION, SPI_GETMENUSHOWDELAY, SPI_GETMOUSEHOVERTIME,
            SPI_GETWHEELSCROLLCHARS, SPI_GETWHEELSCROLLLINES, SW_HIDE, SW_RESTORE,
            SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER,
            SYSTEM_PARAMETERS_INFO_ACTION, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
            SetForegroundWindow, SetWindowPlacement, SetWindowPos, ShowWindow,
            SystemParametersInfoW, USER_DEFAULT_SCREEN_DPI, WINDOWPLACEMENT, WM_CANCELMODE,
            WM_SYSCOMMAND, WS_EX_TRANSPARENT,
        },
    },
};
use windows::core::{BOOL, PCWSTR};

use crate::geom::{ScreenPoint, ScreenRect};

impl From<POINT> for ScreenPoint {
    fn from(p: POINT) -> Self {
        Self { x: p.x, y: p.y }
    }
}

impl From<ScreenPoint> for POINT {
    fn from(p: ScreenPoint) -> Self {
        Self { x: p.x, y: p.y }
    }
}

impl From<RECT> for ScreenRect {
    fn from(r: RECT) -> Self {
        Self {
            left: r.left,
            top: r.top,
            right: r.right,
            bottom: r.bottom,
        }
    }
}

/// Queries a `u32` setting via `SystemParametersInfoW`.
pub(crate) fn system_parameters_u32(action: u32) -> Option<u32> {
    let mut value = 0u32;
    unsafe {
        SystemParametersInfoW(
            SYSTEM_PARAMETERS_INFO_ACTION(action),
            0,
            Some((&raw mut value).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    }
    .is_ok()
    .then_some(value)
}

/// Opaque snapshot of a Win32 window's placement state.
#[derive(Copy, Clone)]
pub(crate) struct WindowPlacement(WINDOWPLACEMENT);

/// Opaque, typed handle to a Win32 window (`HWND`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct WindowHwnd(HWND);

impl WindowHwnd {
    pub(crate) fn from_raw(hwnd: HWND) -> Self {
        Self(hwnd)
    }

    pub(crate) fn raw(self) -> HWND {
        self.0
    }

    pub(crate) fn is_null(self) -> bool {
        self.0.is_invalid()
    }

    pub(crate) fn client_origin(self) -> ScreenPoint {
        let mut origin = POINT::default();
        let _ = unsafe { ClientToScreen(self.0, &mut origin) };
        origin.into()
    }

    pub(crate) fn dpi_scale(self) -> f32 {
        match unsafe { GetDpiForWindow(self.0) } {
            0 => 1.,
            dpi => dpi as f32 / USER_DEFAULT_SCREEN_DPI as f32,
        }
    }

    pub(crate) fn client_rect(self) -> ScreenRect {
        let mut rect = RECT::default();
        let _ = unsafe { GetClientRect(self.0, &mut rect) };
        rect.into()
    }

    pub(crate) fn window_rect(self) -> ScreenRect {
        let mut rect = RECT::default();
        let _ = unsafe { GetWindowRect(self.0, &mut rect) };
        rect.into()
    }

    pub(crate) fn is_maximized(self) -> bool {
        unsafe { IsZoomed(self.0) }.as_bool()
    }

    pub(crate) fn restore(self) {
        let _ = unsafe { ShowWindow(self.0, SW_RESTORE) };
    }

    pub(crate) fn bring_to_front(self) {
        if self.is_null() {
            return;
        }
        unsafe {
            if IsIconic(self.0).as_bool() {
                let _ = ShowWindow(self.0, SW_RESTORE);
            }
            let _ = SetForegroundWindow(self.0);
        }
    }

    pub(crate) fn set_foreground(self) {
        let _ = unsafe { SetForegroundWindow(self.0) };
    }

    pub(crate) fn place_topmost_noactivate(self, rect: ScreenRect) {
        let _ = unsafe {
            SetWindowPos(
                self.0,
                Some(HWND_TOPMOST),
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
                SWP_NOACTIVATE,
            )
        };
    }

    pub(crate) fn show_noactivate(self) {
        let _ = unsafe { ShowWindow(self.0, SW_SHOWNOACTIVATE) };
    }

    pub(crate) fn hide(self) {
        let _ = unsafe { ShowWindow(self.0, SW_HIDE) };
    }

    pub(crate) fn move_noactivate(self, x: i32, y: i32) {
        let _ = unsafe {
            SetWindowPos(
                self.0,
                None,
                x,
                y,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            )
        };
    }

    pub(crate) fn set_capture(self) {
        unsafe { SetCapture(self.0) };
    }

    pub(crate) fn begin_system_move(self, point: ScreenPoint) {
        let position = LPARAM((((point.y as u16 as u32) << 16) | point.x as u16 as u32) as isize);
        unsafe {
            let _ = ReleaseCapture();
            let _ = PostMessageW(
                Some(self.0),
                WM_SYSCOMMAND,
                WPARAM(SC_MOVE as usize | HTCAPTION as usize),
                position,
            );
        }
    }

    pub(crate) fn cancel_mode(self) {
        let _ = unsafe { PostMessageW(Some(self.0), WM_CANCELMODE, WPARAM(0), LPARAM(0)) };
    }

    pub(crate) fn in_move_loop(self) -> bool {
        let mut info = GUITHREADINFO {
            cbSize: size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        unsafe {
            let thread = GetWindowThreadProcessId(self.0, None);
            GetGUIThreadInfo(thread, &mut info).is_ok() && (info.flags & GUI_INMOVESIZE).0 != 0
        }
    }

    pub(crate) fn saved_placement(self) -> Option<WindowPlacement> {
        let mut placement = WINDOWPLACEMENT {
            length: size_of::<WINDOWPLACEMENT>() as u32,
            ..Default::default()
        };
        unsafe { GetWindowPlacement(self.0, &mut placement) }
            .is_ok()
            .then_some(WindowPlacement(placement))
    }

    pub(crate) fn restore_placement(self, placement: WindowPlacement) {
        let _ = unsafe { SetWindowPlacement(self.0, &placement.0) };
    }
}

pub(crate) fn cursor_screen_pos() -> ScreenPoint {
    let mut point = POINT::default();
    let _ = unsafe { GetCursorPos(&mut point) };
    point.into()
}

pub(crate) fn release_mouse_capture() {
    let _ = unsafe { ReleaseCapture() };
}

pub(crate) fn primary_mouse_button_down() -> bool {
    let button = if unsafe { GetSystemMetrics(SM_SWAPBUTTON) } != 0 {
        VK_RBUTTON
    } else {
        VK_LBUTTON
    };
    (unsafe { GetAsyncKeyState(button.0 as i32) }) < 0
}

pub(crate) fn monitor_work_area_and_scale(point: ScreenPoint) -> (ScreenRect, f32) {
    let monitor = unsafe { MonitorFromPoint(point.into(), MONITOR_DEFAULTTONEAREST) };
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    let _ = unsafe { GetMonitorInfoW(monitor, &mut info) };
    let (mut dpi_x, mut dpi_y) = (USER_DEFAULT_SCREEN_DPI, USER_DEFAULT_SCREEN_DPI);
    let _ = unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) };
    let scale = dpi_x as f32 / USER_DEFAULT_SCREEN_DPI as f32;
    (info.rcWork.into(), scale)
}

pub(crate) fn top_visible_window_at(
    point: ScreenPoint,
    exclude: Option<WindowHwnd>,
) -> Option<WindowHwnd> {
    struct Search {
        point: POINT,
        exclude: Option<HWND>,
        found: Option<HWND>,
    }

    fn is_shown(hwnd: HWND) -> bool {
        let mut cloaked = 0u32;
        unsafe {
            IsWindowVisible(hwnd).as_bool()
                && !IsIconic(hwnd).as_bool()
                && GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TRANSPARENT.0 == 0
                && (DwmGetWindowAttribute(
                    hwnd,
                    DWMWA_CLOAKED,
                    (&raw mut cloaked).cast(),
                    size_of::<u32>() as u32,
                )
                .is_err()
                    || cloaked == 0)
        }
    }

    fn extended_frame(hwnd: HWND) -> RECT {
        let mut frame = RECT::default();
        let status = unsafe {
            DwmGetWindowAttribute(
                hwnd,
                DWMWA_EXTENDED_FRAME_BOUNDS,
                (&raw mut frame).cast(),
                size_of::<RECT>() as u32,
            )
        };
        if status.is_err() {
            let _ = unsafe { GetWindowRect(hwnd, &mut frame) };
        }
        frame
    }

    unsafe extern "system" fn visit(hwnd: HWND, data: LPARAM) -> BOOL {
        let search = unsafe { &mut *(data.0 as *mut Search) };
        if search.exclude == Some(hwnd) || !is_shown(hwnd) {
            return BOOL(1);
        }
        let frame = extended_frame(hwnd);
        if unsafe { PtInRect(&frame, search.point) }.as_bool() {
            search.found = Some(hwnd);
            return BOOL(0);
        }
        BOOL(1)
    }

    let mut search = Search {
        point: point.into(),
        exclude: exclude.map(WindowHwnd::raw),
        found: None,
    };
    let _ = unsafe { EnumWindows(Some(visit), LPARAM((&raw mut search) as isize)) };
    search.found.map(WindowHwnd::from_raw)
}

pub(crate) fn caret_blink_interval() -> Option<Duration> {
    let millis = unsafe { GetCaretBlinkTime() };
    (millis != 0 && millis != u32::MAX).then(|| Duration::from_millis(millis.into()))
}

pub(crate) fn wheel_scroll_amount(horizontal: bool) -> Option<u32> {
    let action = if horizontal {
        SPI_GETWHEELSCROLLCHARS
    } else {
        SPI_GETWHEELSCROLLLINES
    };
    system_parameters_u32(action.0)
}

pub(crate) fn client_area_animations_enabled() -> Option<bool> {
    system_parameters_u32(SPI_GETCLIENTAREAANIMATION.0).map(|v| v != 0)
}

pub(crate) fn mouse_hover_time_ms() -> Option<u32> {
    system_parameters_u32(SPI_GETMOUSEHOVERTIME.0)
}

pub(crate) fn menu_show_delay_ms() -> Option<u32> {
    system_parameters_u32(SPI_GETMENUSHOWDELAY.0)
}

pub(crate) fn system_accent_color(dark: bool) -> Option<u32> {
    let mut shades = [0u8; 32];
    let mut len = shades.len() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            windows::core::w!(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Accent"),
            windows::core::w!("AccentPalette"),
            RRF_RT_REG_BINARY,
            None,
            Some(shades.as_mut_ptr().cast()),
            Some(&mut len),
        )
    };
    let at = if dark { 4 } else { 16 };
    (status.0 == 0 && len as usize >= at + 3)
        .then(|| u32::from_be_bytes([0, shades[at], shades[at + 1], shades[at + 2]]))
}

pub(crate) fn format_local_time_date() -> String {
    let now = unsafe { GetLocalTime() };
    let mut time = [0u16; 128];
    let mut date = [0u16; 128];
    let (time_len, date_len) = unsafe {
        (
            GetTimeFormatEx(None, TIME_NOSECONDS, Some(&now), None, Some(&mut time)),
            GetDateFormatEx(
                None,
                DATE_SHORTDATE,
                Some(&now),
                None,
                Some(&mut date),
                None,
            ),
        )
    };
    let text =
        |buffer: &[u16], len: i32| String::from_utf16_lossy(&buffer[..(len.max(1) - 1) as usize]);
    format!("{} {}", text(&time, time_len), text(&date, date_len))
}

pub(crate) fn documents_dir() -> Option<PathBuf> {
    let path = unsafe { SHGetKnownFolderPath(&FOLDERID_Documents, KF_FLAG_DEFAULT, None) }.ok()?;
    let documents =
        (!path.0.is_null()).then(|| PathBuf::from(OsString::from_wide(unsafe { path.as_wide() })));
    unsafe { CoTaskMemFree(Some(path.0.cast())) };
    documents
}

pub(crate) enum ReplaceFileError {
    UnableToMoveReplacement(io::Error),
    Other(io::Error),
}

pub(crate) fn replace_file(
    target: &Path,
    replacement: &Path,
    backup: &Path,
) -> Result<(), ReplaceFileError> {
    let wide = |path: &Path| -> Vec<u16> { path.as_os_str().encode_wide().chain(Some(0)).collect() };
    let (replaced, replacement_w, backed_up) = (wide(target), wide(replacement), wide(backup));
    let done = unsafe {
        ReplaceFileW(
            PCWSTR(replaced.as_ptr()),
            PCWSTR(replacement_w.as_ptr()),
            PCWSTR(backed_up.as_ptr()),
            REPLACEFILE_IGNORE_MERGE_ERRORS | REPLACEFILE_IGNORE_ACL_ERRORS,
            None,
            None,
        )
    }
    .is_ok();
    if done {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(ERROR_UNABLE_TO_MOVE_REPLACEMENT_2.0 as i32) {
        Err(ReplaceFileError::UnableToMoveReplacement(error))
    } else {
        Err(ReplaceFileError::Other(error))
    }
}

/// Active Windows ANSI code page (`GetACP`).
#[derive(Copy, Clone)]
pub(crate) struct AnsiCodePage(u32);

impl AnsiCodePage {
    pub(crate) fn current() -> Self {
        Self(unsafe { GetACP() })
    }

    pub(crate) fn is_utf8(self) -> bool {
        self.0 == CP_UTF8
    }

    pub(crate) fn decode(self, bytes: &[u8]) -> Option<String> {
        if bytes.is_empty() {
            return Some(String::new());
        }
        let _ = i32::try_from(bytes.len()).ok()?;
        let length =
            unsafe { MultiByteToWideChar(self.0, MULTI_BYTE_TO_WIDE_CHAR_FLAGS(0), bytes, None) };
        let mut wide = vec![0u16; usize::try_from(length).ok().filter(|&n| n > 0)?];
        let written = unsafe {
            MultiByteToWideChar(
                self.0,
                MULTI_BYTE_TO_WIDE_CHAR_FLAGS(0),
                bytes,
                Some(&mut wide),
            )
        };
        wide.truncate(usize::try_from(written).ok()?);
        Some(String::from_utf16_lossy(&wide))
    }

    pub(crate) fn wide_is_lossy(self, wide: &[u16]) -> io::Result<bool> {
        if wide.is_empty() {
            return Ok(false);
        }
        let mut used_default = BOOL(0);
        let written = unsafe {
            WideCharToMultiByte(
                self.0,
                WC_NO_BEST_FIT_CHARS,
                wide,
                None,
                None,
                Some(&mut used_default),
            )
        };
        if written == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(used_default.as_bool())
    }

    pub(crate) fn encode_wide(self, wide: &[u16], out: &mut Vec<u8>) -> io::Result<()> {
        out.clear();
        if wide.is_empty() {
            return Ok(());
        }
        let length =
            unsafe { WideCharToMultiByte(self.0, WC_NO_BEST_FIT_CHARS, wide, None, None, None) };
        if length == 0 {
            return Err(io::Error::last_os_error());
        }
        out.resize(usize::try_from(length).unwrap_or(0), 0);
        let written = unsafe {
            WideCharToMultiByte(self.0, WC_NO_BEST_FIT_CHARS, wide, Some(out), None, None)
        };
        if written == 0 {
            return Err(io::Error::last_os_error());
        }
        out.truncate(usize::try_from(written).unwrap_or(0));
        Ok(())
    }
}
