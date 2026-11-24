use windows::Win32::{
    Foundation::{HWND, LPARAM, POINT, RECT, WPARAM},
    Graphics::{
        Dwm::{DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute},
        Gdi::{
            ClientToScreen, GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO,
            MonitorFromPoint, PtInRect,
        },
    },
    UI::{
        HiDpi::{GetDpiForMonitor, GetDpiForWindow, MDT_EFFECTIVE_DPI},
        Input::KeyboardAndMouse::{
            GetAsyncKeyState, IsWindowEnabled, ReleaseCapture, SetCapture, VK_LBUTTON, VK_RBUTTON,
        },
        WindowsAndMessaging::{
            EnumWindows, GUI_INMOVESIZE, GUITHREADINFO, GWL_EXSTYLE, GetClientRect, GetCursorPos,
            GetGUIThreadInfo, GetSystemMetrics, GetWindowLongW, GetWindowPlacement, GetWindowRect,
            GetWindowThreadProcessId, HTCAPTION, HWND_TOPMOST, IsIconic, IsWindowVisible, IsZoomed,
            PostMessageW, SC_MOVE, SM_SWAPBUTTON, SW_HIDE, SW_RESTORE, SW_SHOWNOACTIVATE,
            SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, SetForegroundWindow, SetWindowPlacement,
            SetWindowPos, ShowWindow, USER_DEFAULT_SCREEN_DPI, WINDOWPLACEMENT, WM_CANCELMODE,
            WM_SYSCOMMAND, WS_EX_TRANSPARENT,
        },
    },
};
use windows::core::BOOL;

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

#[derive(Copy, Clone)]
pub(crate) struct WindowPlacement(WINDOWPLACEMENT);

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

    pub(crate) fn is_enabled(self) -> bool {
        unsafe { IsWindowEnabled(self.0) }.as_bool()
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

    pub(crate) fn restored_logical_size(self) -> Option<crate::geom::Size> {
        let placement = self.saved_placement()?;
        let rc = placement.0.rcNormalPosition;
        let w = (rc.right - rc.left).max(1) as f32;
        let h = (rc.bottom - rc.top).max(1) as f32;
        let scale = self.dpi_scale().max(0.5);
        Some(crate::geom::size(w / scale, h / scale))
    }

    pub(crate) fn restore_placement(self, placement: WindowPlacement) {
        let _ = unsafe { SetWindowPlacement(self.0, &placement.0) };
    }

    pub(crate) fn complete_ime_composition(self) {
        if !self.is_null() {
            super::input::complete_ime_composition(self.0);
        }
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
