use std::cell::{Cell, RefCell};
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;
use std::rc::Rc;

use windows::Win32::{
    Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
    Graphics::{
        Dwm::{
            DWM_SYSTEMBACKDROP_TYPE, DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE,
            DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_TRANSITIONS_FORCEDISABLED,
            DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND,
            DWMWINDOWATTRIBUTE, DwmExtendFrameIntoClientArea, DwmSetWindowAttribute,
        },
        Gdi::{ScreenToClient, ValidateRect},
    },
    System::{
        LibraryLoader::GetModuleHandleW,
        Shutdown::{ShutdownBlockReasonCreate, ShutdownBlockReasonDestroy},
    },
    UI::{
        Controls::{MARGINS, WM_MOUSELEAVE},
        HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi},
        Input::{
            Ime::{
                GCS_COMPSTR, GCS_CURSORPOS, GCS_RESULTSTR, ImmGetCompositionStringW, ImmGetContext,
                ImmReleaseContext,
            },
            KeyboardAndMouse::{
                ReleaseCapture, SetCapture, TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT,
                TrackMouseEvent,
            },
        },
        Shell::{DragAcceptFiles, DragFinish, DragQueryFileW, HDROP},
        WindowsAndMessaging::{
            CS_DBLCLKS, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, CreateCaret, CreateWindowExW,
            DefWindowProcW, DestroyCaret, DestroyWindow, GetClientRect, GetCursorPos,
            GetWindowRect, HTCAPTION, HTCLIENT, HTCLOSE, HTMAXBUTTON, HTMINBUTTON, HTTOP,
            HTTOPLEFT, HTTOPRIGHT, IDC_ARROW, IDC_IBEAM, IsZoomed, KillTimer, LWA_ALPHA,
            LoadCursorW, LoadIconW, MINMAXINFO, NCCALCSIZE_PARAMS, PostMessageW, RegisterClassW,
            SC_CLOSE, SC_MAXIMIZE, SC_MINIMIZE, SC_RESTORE, SM_CXFRAME, SM_CXPADDEDBORDER,
            SM_CYFRAME, SW_SHOW, SW_SHOWNOACTIVATE, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOZORDER,
            SetCaretPos, SetCursor, SetLayeredWindowAttributes, SetTimer, SetWindowPos,
            SetWindowTextW, ShowWindow, USER_DEFAULT_SCREEN_DPI, USER_TIMER_MINIMUM, WM_ACTIVATE,
            WM_CAPTURECHANGED, WM_CHAR, WM_CLOSE, WM_DPICHANGED, WM_DROPFILES, WM_ENDSESSION,
            WM_ENTERMENULOOP, WM_ENTERSIZEMOVE, WM_ERASEBKGND, WM_EXITMENULOOP, WM_EXITSIZEMOVE,
            WM_GETMINMAXINFO, WM_IME_COMPOSITION, WM_IME_ENDCOMPOSITION, WM_IME_STARTCOMPOSITION,
            WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN,
            WM_MBUTTONUP, WM_MOUSEHWHEEL, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_MOVE, WM_NCCALCSIZE,
            WM_NCHITTEST, WM_NCLBUTTONDBLCLK, WM_NCLBUTTONDOWN, WM_NCLBUTTONUP, WM_NCMOUSELEAVE,
            WM_NCMOUSEMOVE, WM_PAINT, WM_QUERYENDSESSION, WM_RBUTTONDBLCLK, WM_RBUTTONDOWN,
            WM_RBUTTONUP, WM_SETCURSOR, WM_SETTINGCHANGE, WM_SIZE, WM_SYSCHAR, WM_SYSCOMMAND,
            WM_SYSKEYDOWN, WM_SYSKEYUP, WM_THEMECHANGED, WM_TIMER, WNDCLASSW, WS_EX_APPWINDOW,
            WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW,
            WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_POPUP, WS_SYSMENU,
            WS_THICKFRAME, WindowFromPoint,
        },
    },
};
use windows::core::{BOOL, HSTRING, PCWSTR, w};

use crate::geom::{Point, Rect, Size, point, size};
use crate::types::Result;

use super::ClickTracker;
use super::CursorStyle;
use super::Key;
use super::KeyEvent;
use super::MainWindowConfig;
use super::Modifiers;
use super::MouseButton;
use super::PlatformEvent;
use super::WindowAppearance;
use super::WindowControlArea;
use super::WindowId;
use super::WindowsDisplay;
use super::current_modifiers;
use super::ffi_guard;
use super::handle::WindowHandle;
use super::hit_test_control_areas;
use super::is_syskey_passthrough;
use super::pump::{self, dispatch, lookup_window, service};
use super::read_ime_string;
use super::set_ime_candidate_pos;
use super::settings::SystemSettings;
use super::translate_vk;

const WINDOW_CLASS: PCWSTR = w!("JotWindow");
const OVERLAY_CLASS: PCWSTR = w!("JotOverlay");
const DWMSBT_TABBEDWINDOW: i32 = 4;
const SIZE_MOVE_TIMER_ID: usize = 1;

#[derive(Default, Copy, Clone)]
struct MouseButtons(u8);

impl MouseButtons {
    fn bit(button: MouseButton) -> u8 {
        match button {
            MouseButton::Left => 0x1,
            MouseButton::Right => 0x2,
            MouseButton::Middle => 0x4,
        }
    }

    fn press(&mut self, button: MouseButton) {
        self.0 |= Self::bit(button);
    }

    fn release(&mut self, button: MouseButton) {
        self.0 &= !Self::bit(button);
    }

    fn clear(&mut self) {
        self.0 = 0;
    }

    fn is_empty(self) -> bool {
        self.0 == 0
    }

    fn from_wparam(wparam: WPARAM) -> Self {
        let bits = wparam.0 as u32;
        let mut mask = 0u8;
        if (bits & 0x0001) != 0 {
            mask |= 0x1;
        }
        if (bits & 0x0002) != 0 {
            mask |= 0x2;
        }
        if (bits & 0x0010) != 0 {
            mask |= 0x4;
        }
        Self(mask)
    }

    fn primary(self) -> Option<MouseButton> {
        if (self.0 & 0x1) != 0 {
            Some(MouseButton::Left)
        } else if (self.0 & 0x2) != 0 {
            Some(MouseButton::Right)
        } else if (self.0 & 0x4) != 0 {
            Some(MouseButton::Middle)
        } else {
            None
        }
    }
}

#[derive(Copy, Clone)]
struct WindowMetrics {
    scale: f32,
    physical: (u32, u32),
}

struct WindowChrome {
    min_size: Option<Size>,
    cursor: CursorStyle,
    control_areas: Vec<(Rect, WindowControlArea)>,
}

struct WindowInput {
    hovered: bool,
    nc_button_down: Option<u32>,
    pending_surrogate: Option<u16>,
    suppress_next_char: bool,
    last_modifiers: Modifiers,
    clicks: ClickTracker,
    buttons: MouseButtons,
    releasing_capture: bool,
    tracking_leave: bool,
    tracking_nc_leave: bool,
}

pub(super) struct WindowShared {
    id: WindowId,
    metrics: Cell<WindowMetrics>,
    active: Cell<bool>,
    in_size_move: Cell<bool>,
    ime_caret: Cell<Option<Rect>>,
    sys_caret_px: Cell<Option<(i32, i32, i32, i32)>>,
    last_origin_px: Cell<Option<(i32, i32)>>,
    chrome: RefCell<WindowChrome>,
    input: RefCell<WindowInput>,
}

impl WindowShared {
    fn new(id: WindowId, scale: f32, physical: (u32, u32), active: bool) -> Rc<Self> {
        Rc::new(Self {
            id,
            metrics: Cell::new(WindowMetrics { scale, physical }),
            active: Cell::new(active),
            in_size_move: Cell::new(false),
            ime_caret: Cell::new(None),
            sys_caret_px: Cell::new(None),
            last_origin_px: Cell::new(None),
            chrome: RefCell::new(WindowChrome {
                min_size: None,
                cursor: CursorStyle::Arrow,
                control_areas: Vec::new(),
            }),
            input: RefCell::new(WindowInput {
                hovered: false,
                nc_button_down: None,
                pending_surrogate: None,
                suppress_next_char: false,
                last_modifiers: current_modifiers(),
                clicks: ClickTracker::default(),
                buttons: MouseButtons::default(),
                releasing_capture: false,
                tracking_leave: false,
                tracking_nc_leave: false,
            }),
        })
    }

    #[inline]
    fn scale(&self) -> f32 {
        self.metrics.get().scale
    }

    fn set_metrics(&self, w: u32, h: u32, scale: f32) {
        self.metrics.set(WindowMetrics {
            scale,
            physical: (w, h),
        });
    }

    fn logical_size(&self) -> Size {
        let m = self.metrics.get();
        size(m.physical.0 as f32 / m.scale, m.physical.1 as f32 / m.scale)
    }
}

unsafe extern "system" fn overlay_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

pub(super) fn register_window_class() -> Result<()> {
    unsafe {
        let hinstance: HINSTANCE = GetModuleHandleW(None)?.into();
        let hicon = LoadIconW(Some(hinstance), PCWSTR(std::ptr::without_provenance(1)))
            .unwrap_or_default();
        let main_wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW | CS_DBLCLKS,
            lpfnWndProc: Some(window_wnd_proc),
            hInstance: hinstance,
            hIcon: hicon,
            lpszClassName: WINDOW_CLASS,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            ..Default::default()
        };
        RegisterClassW(&main_wc);

        let overlay_wc = WNDCLASSW {
            lpfnWndProc: Some(overlay_wnd_proc),
            hInstance: hinstance,
            lpszClassName: OVERLAY_CLASS,
            ..Default::default()
        };
        RegisterClassW(&overlay_wc);
    }
    Ok(())
}

pub(crate) struct NativeWindow {
    pub(crate) id: WindowId,
    hwnd: HWND,
    shared: Rc<WindowShared>,
    destroyed: bool,
}

impl NativeWindow {
    pub(crate) fn create_main(
        config: &MainWindowConfig,
        appearance: WindowAppearance,
    ) -> Result<Self> {
        let id = WindowId::next();
        let hinstance: HINSTANCE = unsafe { GetModuleHandleW(None)? }.into();
        let wide_title = HSTRING::from(config.title.as_str());
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_APPWINDOW | WS_EX_NOREDIRECTIONBITMAP,
                WINDOW_CLASS,
                &wide_title,
                WS_SYSMENU | WS_THICKFRAME | WS_MAXIMIZEBOX | WS_MINIMIZEBOX,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                None,
                None,
                Some(hinstance),
                None,
            )?
        };

        let init_scale = query_scale(hwnd);
        let shared = WindowShared::new(id, init_scale, (1, 1), config.show && config.focus);
        pump::register_window(hwnd, shared.clone());

        configure_backdrop(hwnd);
        configure_dark_mode(hwnd, appearance);

        let target_scale = config
            .display_id
            .and_then(WindowsDisplay::from_id)
            .map_or(init_scale, |d| d.scale_factor());

        if let Some(b) = config.bounds {
            let target_dpi = (target_scale * USER_DEFAULT_SCREEN_DPI as f32).round() as u32;
            let (bx, by) = frame_thickness_for_dpi(target_dpi);
            let x = (b.origin.x * target_scale).round() as i32 - bx;
            let y = (b.origin.y * target_scale).round() as i32;
            let w = (b.size.width * target_scale).round().max(1.0) as i32 + bx * 2;
            let h = (b.size.height * target_scale).round().max(1.0) as i32 + by;
            unsafe {
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    x,
                    y,
                    w,
                    h,
                    SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
                );
            }
        }

        unsafe { DragAcceptFiles(hwnd, true) };
        let scale = query_scale(hwnd);
        let (pw, ph) = query_physical_size(hwnd);
        shared.set_metrics(pw, ph, scale);
        let win = Self {
            id,
            hwnd,
            shared,
            destroyed: false,
        };
        if config.show {
            win.show(config.focus);
        }
        Ok(win)
    }

    pub(crate) fn create_overlay() -> Result<Self> {
        let id = WindowId::next();
        let hinstance: HINSTANCE = unsafe { GetModuleHandleW(None)? }.into();
        let wide_title = HSTRING::from("");
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_NOACTIVATE
                    | WS_EX_TOOLWINDOW
                    | WS_EX_TOPMOST
                    | WS_EX_TRANSPARENT
                    | WS_EX_LAYERED
                    | WS_EX_NOREDIRECTIONBITMAP,
                OVERLAY_CLASS,
                &wide_title,
                WS_POPUP,
                0,
                0,
                1,
                1,
                None,
                None,
                Some(hinstance),
                None,
            )?
        };

        set_dwm_u32(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            DWMWCP_DONOTROUND.0 as u32,
        );
        set_dwm_u32(hwnd, DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE);
        set_dwm_u32(hwnd, DWMWA_TRANSITIONS_FORCEDISABLED, 1);
        unsafe {
            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
        }

        let scale = query_scale(hwnd);
        let (pw, ph) = query_physical_size(hwnd);
        let shared = WindowShared::new(id, scale, (pw, ph), false);
        Ok(Self {
            id,
            hwnd,
            shared,
            destroyed: false,
        })
    }

    pub(crate) fn show(&self, focus: bool) {
        unsafe {
            let _ = ShowWindow(self.hwnd, if focus { SW_SHOW } else { SW_SHOWNOACTIVATE });
        }
    }

    pub(crate) fn set_title(&self, title: &str) {
        let wide = HSTRING::from(title);
        unsafe {
            let _ = SetWindowTextW(self.hwnd, &wide);
        }
    }

    pub(crate) fn set_appearance(&self, appearance: WindowAppearance) {
        configure_dark_mode(self.hwnd, appearance);
    }

    pub(crate) fn set_cursor(&self, style: CursorStyle) {
        let mut chrome = self.shared.chrome.borrow_mut();
        if chrome.cursor != style {
            chrome.cursor = style;
            drop(chrome);
            apply_cursor(style);
        }
    }

    pub(crate) fn set_min_size(&self, min_size: Option<Size>) {
        self.shared.chrome.borrow_mut().min_size = min_size;
    }

    pub(crate) fn set_control_areas(&self, areas: Vec<(Rect, WindowControlArea)>) {
        self.shared.chrome.borrow_mut().control_areas = areas;
    }

    pub(crate) fn is_maximized(&self) -> bool {
        unsafe { IsZoomed(self.hwnd) }.as_bool()
    }

    pub(crate) fn hwnd(&self) -> WindowHandle {
        WindowHandle::from_raw(self.hwnd)
    }

    #[inline]
    pub(crate) fn scale_factor(&self) -> f32 {
        self.shared.scale()
    }

    #[inline]
    pub(crate) fn physical_size(&self) -> (u32, u32) {
        self.shared.metrics.get().physical
    }

    #[inline]
    pub(crate) fn logical_size(&self) -> Size {
        self.shared.logical_size()
    }

    #[inline]
    pub(crate) fn is_active(&self) -> bool {
        self.shared.active.get()
    }

    #[cfg(test)]
    #[inline]
    pub(super) fn in_size_move(&self) -> bool {
        self.shared.in_size_move.get()
    }

    pub(crate) fn set_text_caret(&self, ime_rect: Option<Rect>, show_system_caret: bool) {
        self.shared.ime_caret.set(ime_rect);
        let scale = self.shared.scale();
        let next_px = show_system_caret.then_some(ime_rect).flatten().map(|r| {
            (
                (r.origin.x * scale).round() as i32,
                (r.origin.y * scale).round() as i32,
                (r.size.width * scale).round().max(1.0) as i32,
                (r.size.height * scale).round().max(1.0) as i32,
            )
        });
        if next_px == self.shared.sys_caret_px.get() {
            return;
        }
        self.shared.sys_caret_px.set(next_px);
        match next_px {
            Some((x, y, w, h)) => unsafe {
                let _ = CreateCaret(self.hwnd, None, w.max(1), h.max(1));
                let _ = SetCaretPos(x, y);
            },
            None => unsafe {
                let _ = DestroyCaret();
            },
        }
    }

    pub(crate) fn sync_live_metrics(&self) -> Option<(u32, u32, f32)> {
        let mut rc = RECT::default();
        if unsafe { GetClientRect(self.hwnd, &mut rc) }.is_err() {
            return None;
        }
        let w = rc.right - rc.left;
        let h = rc.bottom - rc.top;
        if w <= 0 || h <= 0 {
            return None;
        }
        let scale = query_scale(self.hwnd);
        let (pw, ph) = (w as u32, h as u32);
        self.shared.set_metrics(pw, ph, scale);
        Some((pw, ph, scale))
    }

    pub(crate) fn destroy(&mut self) {
        if self.destroyed {
            return;
        }
        self.destroyed = true;
        let in_size_move = pump::unregister_window(self.hwnd)
            .is_some_and(|s| s.in_size_move.get());
        if in_size_move {
            let _ = unsafe { KillTimer(Some(self.hwnd), SIZE_MOVE_TIMER_ID) };
        }
        let _ = unsafe { DestroyWindow(self.hwnd) };
    }
}

impl Drop for NativeWindow {
    fn drop(&mut self) {
        self.destroy();
    }
}

fn query_scale(hwnd: HWND) -> f32 {
    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(USER_DEFAULT_SCREEN_DPI);
    dpi as f32 / USER_DEFAULT_SCREEN_DPI as f32
}

fn query_physical_size(hwnd: HWND) -> (u32, u32) {
    let mut rc = RECT::default();
    let _ = unsafe { GetClientRect(hwnd, &mut rc) };
    (
        (rc.right - rc.left).max(1) as u32,
        (rc.bottom - rc.top).max(1) as u32,
    )
}

fn set_dwm_u32(hwnd: HWND, attr: DWMWINDOWATTRIBUTE, value: u32) {
    let _ = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            attr,
            (&raw const value).cast(),
            std::mem::size_of::<u32>() as u32,
        )
    };
}

fn configure_dark_mode(hwnd: HWND, appearance: WindowAppearance) {
    let dark = BOOL(i32::from(matches!(appearance, WindowAppearance::Dark)));
    let _ = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            (&raw const dark).cast(),
            std::mem::size_of::<BOOL>() as u32,
        )
    };
}

fn frame_thickness_for_dpi(dpi: u32) -> (i32, i32) {
    unsafe {
        let pad = GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi);
        let bx = GetSystemMetricsForDpi(SM_CXFRAME, dpi) + pad;
        let by = GetSystemMetricsForDpi(SM_CYFRAME, dpi) + pad;
        (bx, by)
    }
}

fn configure_backdrop(hwnd: HWND) {
    let margins = MARGINS {
        cxLeftWidth: -1,
        cxRightWidth: -1,
        cyTopHeight: -1,
        cyBottomHeight: -1,
    };
    let _ = unsafe { DwmExtendFrameIntoClientArea(hwnd, &margins) };
    let b = DWM_SYSTEMBACKDROP_TYPE(DWMSBT_TABBEDWINDOW);
    let _ = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE,
            (&raw const b).cast(),
            std::mem::size_of::<DWM_SYSTEMBACKDROP_TYPE>() as u32,
        )
    };
}

fn apply_cursor(style: CursorStyle) {
    let id = match style {
        CursorStyle::Arrow => IDC_ARROW,
        CursorStyle::IBeam => IDC_IBEAM,
    };
    unsafe {
        if let Ok(cursor) = LoadCursorW(None, id) {
            SetCursor(Some(cursor));
        }
    }
}

fn client_point_from_lparam(lparam: LPARAM, scale: f32) -> (i32, i32, Point) {
    let x = (lparam.0 & 0xFFFF) as i16 as i32;
    let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
    (x, y, point(x as f32 / scale, y as f32 / scale))
}

fn screen_to_client_point(hwnd: HWND, lparam: LPARAM, scale: f32) -> Point {
    let mut pt = POINT {
        x: (lparam.0 & 0xFFFF) as i16 as i32,
        y: ((lparam.0 >> 16) & 0xFFFF) as i16 as i32,
    };
    let _ = unsafe { ScreenToClient(hwnd, &mut pt) };
    point(pt.x as f32 / scale, pt.y as f32 / scale)
}

fn cursor_is_over_hwnd(hwnd: HWND) -> bool {
    let mut pt = POINT::default();
    unsafe { GetCursorPos(&mut pt).is_ok() && WindowFromPoint(pt) == hwnd }
}

fn is_relevant_setting_change(lparam: LPARAM) -> bool {
    if lparam.0 == 0 {
        return true;
    }
    let ptr = lparam.0 as *const u16;
    let pcw = PCWSTR(ptr);
    let wide = unsafe { pcw.as_wide() };
    let name = String::from_utf16_lossy(wide);
    name.eq_ignore_ascii_case("ImmersiveColorSet")
        || name.eq_ignore_ascii_case("WindowMetrics")
        || name.eq_ignore_ascii_case("HighContrast")
}

fn update_ime_pos(hwnd: HWND) {
    if let Some(s) = lookup_window(hwnd)
        && let Some(rect) = s.ime_caret.get()
    {
        set_ime_candidate_pos(hwnd, rect, s.scale());
    }
}

fn release_capture_suppressed(hwnd: HWND, release: impl FnOnce()) {
    let shared = lookup_window(hwnd);
    if let Some(s) = &shared {
        s.input.borrow_mut().releasing_capture = true;
    }
    release();
    if let Some(s) = &shared {
        s.input.borrow_mut().releasing_capture = false;
    }
}

fn handle_mouse_move(hwnd: HWND, lparam: LPARAM, live_buttons: Option<MouseButtons>) {
    let Some(s) = lookup_window(hwnd) else {
        return;
    };
    if s.in_size_move.get() {
        return;
    }
    let non_client = live_buttons.is_none();
    let scale = s.scale();
    let (need_track, ev) = {
        let mut input = s.input.borrow_mut();
        let (need_track, position, pressed_button) = match live_buttons {
            None => (
                !input.tracking_nc_leave,
                screen_to_client_point(hwnd, lparam, scale),
                input.buttons.primary(),
            ),
            Some(live) => {
                input.buttons = live;
                (
                    !input.tracking_leave,
                    client_point_from_lparam(lparam, scale).2,
                    live.primary(),
                )
            }
        };
        input.hovered = true;
        (
            need_track,
            PlatformEvent::MouseMove {
                window_id: s.id,
                position,
                pressed_button,
            },
        )
    };
    if need_track {
        let mut tme = TRACKMOUSEEVENT {
            cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
            dwFlags: if non_client {
                TME_LEAVE | TME_NONCLIENT
            } else {
                TME_LEAVE
            },
            hwndTrack: hwnd,
            dwHoverTime: 0,
        };
        if unsafe { TrackMouseEvent(&mut tme) }.is_ok() {
            let mut input = s.input.borrow_mut();
            if non_client {
                input.tracking_nc_leave = true;
            } else {
                input.tracking_leave = true;
            }
        }
    }
    dispatch(ev);
}

fn handle_mouse_leave(hwnd: HWND, non_client: bool) {
    let Some(s) = lookup_window(hwnd) else {
        return;
    };
    let check_leave = {
        let mut input = s.input.borrow_mut();
        if non_client {
            input.tracking_nc_leave = false;
            input.nc_button_down = None;
            !input.tracking_leave
        } else {
            input.tracking_leave = false;
            !input.tracking_nc_leave
        }
    };
    if check_leave && !cursor_is_over_hwnd(hwnd) {
        s.input.borrow_mut().hovered = false;
        dispatch(PlatformEvent::MouseLeave { window_id: s.id });
    }
}

fn dispatch_key_message(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    is_down: bool,
) -> LRESULT {
    let vk = (wparam.0 & 0xFFFF) as u16;
    let modifiers = current_modifiers();
    let mut handled = false;
    if let Some(s) = lookup_window(hwnd) {
        let mods_changed = {
            let mut input = s.input.borrow_mut();
            input.suppress_next_char = false;
            (modifiers != input.last_modifiers).then(|| {
                input.last_modifiers = modifiers;
                PlatformEvent::ModifiersChanged { modifiers }
            })
        };
        if let Some(ev) = mods_changed {
            dispatch(ev);
        }
        if let Some(key) = translate_vk(vk, modifiers) {
            let mut stroke_mods = modifiers;
            if key == Key::Alt {
                stroke_mods.alt = false;
            }
            let event = KeyEvent {
                key,
                modifiers: stroke_mods,
            };
            if is_down {
                let is_held = (lparam.0 & (1 << 30)) != 0;
                if is_held && matches!(key, Key::Alt | Key::F(10)) {
                    handled = true;
                } else {
                    handled = dispatch(PlatformEvent::KeyDown {
                        window_id: s.id,
                        event,
                    });
                    if handled {
                        s.input.borrow_mut().suppress_next_char = true;
                    }
                }
            } else {
                dispatch(PlatformEvent::KeyUp {
                    window_id: s.id,
                    event,
                });
            }
        }
    }
    if (is_down && handled)
        || (msg == WM_SYSKEYDOWN && !is_syskey_passthrough(vk))
        || msg == WM_SYSKEYUP
    {
        return LRESULT(0);
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

unsafe extern "system" fn window_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    ffi_guard("window_wnd_proc", LRESULT(0), || {
        window_wnd_proc_inner(hwnd, msg, wparam, lparam)
    })
}

fn window_wnd_proc_inner(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_ERASEBKGND => LRESULT(1),

        WM_PAINT => {
            let _ = unsafe { ValidateRect(Some(hwnd), None) };
            if let Some(s) = lookup_window(hwnd) {
                dispatch(PlatformEvent::PaintRequested { window_id: s.id });
            }
            LRESULT(0)
        }

        WM_NCCALCSIZE if wparam.0 != 0 => {
            let params = unsafe { &mut *(lparam.0 as *mut NCCALCSIZE_PARAMS) };
            let saved_top = params.rgrc[0].top;
            let result = unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
            params.rgrc[0].top = saved_top;
            if unsafe { IsZoomed(hwnd) }.as_bool() {
                let dpi = unsafe { GetDpiForWindow(hwnd) }.max(USER_DEFAULT_SCREEN_DPI);
                let (_, by) = frame_thickness_for_dpi(dpi);
                params.rgrc[0].top += by;
            }
            result
        }

        WM_GETMINMAXINFO => {
            if let Some(s) = lookup_window(hwnd)
                && let Some(min_size) = s.chrome.borrow().min_size
            {
                let dpi = unsafe { GetDpiForWindow(hwnd) }.max(USER_DEFAULT_SCREEN_DPI);
                let scale = dpi as f32 / USER_DEFAULT_SCREEN_DPI as f32;
                let (bx, by) = frame_thickness_for_dpi(dpi);
                let mmi = unsafe { &mut *(lparam.0 as *mut MINMAXINFO) };
                mmi.ptMinTrackSize.x = (min_size.width * scale).ceil() as i32 + bx * 2;
                mmi.ptMinTrackSize.y = (min_size.height * scale).ceil() as i32 + by;
                return LRESULT(0);
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }

        WM_NCHITTEST => {
            let def_hit = unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
            if def_hit.0 != HTCLIENT as isize {
                return def_hit;
            }
            let hit = lookup_window(hwnd).map(|s| {
                let mut rc = RECT::default();
                let _ = unsafe { GetWindowRect(hwnd, &mut rc) };
                let sx = (lparam.0 & 0xFFFF) as i16 as i32;
                let sy = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
                if !unsafe { IsZoomed(hwnd) }.as_bool() {
                    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(USER_DEFAULT_SCREEN_DPI);
                    let (bx, by) = frame_thickness_for_dpi(dpi);
                    if sy < rc.top + by {
                        if sx < rc.left + bx * 2 {
                            return HTTOPLEFT;
                        }
                        if sx >= rc.right - bx * 2 {
                            return HTTOPRIGHT;
                        }
                        return HTTOP;
                    }
                }
                let local = screen_to_client_point(hwnd, lparam, s.scale());
                match hit_test_control_areas(&s.chrome.borrow().control_areas, local) {
                    WindowControlArea::Close => HTCLOSE,
                    WindowControlArea::Max => HTMAXBUTTON,
                    WindowControlArea::Min => HTMINBUTTON,
                    WindowControlArea::Drag => HTCAPTION,
                    WindowControlArea::Client => HTCLIENT,
                }
            });
            LRESULT(hit.unwrap_or(HTCLIENT) as isize)
        }

        WM_SETCURSOR if (lparam.0 & 0xFFFF) as u32 == HTCLIENT => {
            if let Some(s) = lookup_window(hwnd) {
                apply_cursor(s.chrome.borrow().cursor);
            }
            LRESULT(1)
        }

        WM_NCMOUSEMOVE => {
            handle_mouse_move(hwnd, lparam, None);
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }

        WM_NCMOUSELEAVE => {
            handle_mouse_leave(hwnd, true);
            LRESULT(0)
        }

        WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK => {
            let area = wparam.0 as u32;
            if msg == WM_NCLBUTTONDBLCLK && area == HTCAPTION {
                return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
            }
            if matches!(area, HTMINBUTTON | HTMAXBUTTON | HTCLOSE | HTCAPTION) {
                if let Some(s) = lookup_window(hwnd) {
                    {
                        let mut input = s.input.borrow_mut();
                        if area != HTCAPTION {
                            input.nc_button_down = Some(area);
                        }
                        input.hovered = true;
                    }
                    let position = screen_to_client_point(hwnd, lparam, s.scale());
                    dispatch(PlatformEvent::MouseDown {
                        window_id: s.id,
                        button: MouseButton::Left,
                        position,
                        modifiers: current_modifiers(),
                        click_count: 1,
                    });
                }
                if area != HTCAPTION {
                    return LRESULT(0);
                }
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }

        WM_NCLBUTTONUP => {
            let area = wparam.0 as u32;
            if matches!(area, HTMINBUTTON | HTMAXBUTTON | HTCLOSE) {
                if let Some(s) = lookup_window(hwnd) {
                    let was = s.input.borrow_mut().nc_button_down.take();
                    let position = screen_to_client_point(hwnd, lparam, s.scale());
                    dispatch(PlatformEvent::MouseUp {
                        window_id: s.id,
                        button: MouseButton::Left,
                        position,
                    });
                    if was == Some(area) {
                        let cmd = match area {
                            HTMINBUTTON => SC_MINIMIZE,
                            HTMAXBUTTON => {
                                if unsafe { IsZoomed(hwnd) }.as_bool() {
                                    SC_RESTORE
                                } else {
                                    SC_MAXIMIZE
                                }
                            }
                            HTCLOSE => SC_CLOSE,
                            _ => 0,
                        };
                        if cmd != 0 {
                            let _ = unsafe {
                                PostMessageW(
                                    Some(hwnd),
                                    WM_SYSCOMMAND,
                                    WPARAM(cmd as usize),
                                    LPARAM(0),
                                )
                            };
                        }
                    }
                }
                return LRESULT(0);
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }

        WM_MOUSEMOVE => {
            handle_mouse_move(hwnd, lparam, Some(MouseButtons::from_wparam(wparam)));
            LRESULT(0)
        }

        WM_MOUSELEAVE => {
            handle_mouse_leave(hwnd, false);
            LRESULT(0)
        }

        WM_CAPTURECHANGED => {
            if let Some(s) = lookup_window(hwnd) {
                let emit = {
                    let mut input = s.input.borrow_mut();
                    if input.releasing_capture {
                        false
                    } else {
                        input.buttons.clear();
                        true
                    }
                };
                if emit {
                    dispatch(PlatformEvent::CaptureLost { window_id: s.id });
                }
            }
            LRESULT(0)
        }

        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK | WM_RBUTTONDOWN | WM_RBUTTONDBLCLK | WM_MBUTTONDOWN => {
            let button = match msg {
                WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => MouseButton::Left,
                WM_RBUTTONDOWN | WM_RBUTTONDBLCLK => MouseButton::Right,
                _ => MouseButton::Middle,
            };
            unsafe {
                SetCapture(hwnd);
            }
            if let Some(s) = lookup_window(hwnd) {
                let (x, y, position) = client_point_from_lparam(lparam, s.scale());
                let click_count = {
                    let mut input = s.input.borrow_mut();
                    input.buttons.press(button);
                    input.clicks.record(button, x, y)
                };
                dispatch(PlatformEvent::MouseDown {
                    window_id: s.id,
                    button,
                    position,
                    modifiers: current_modifiers(),
                    click_count,
                });
            }
            LRESULT(0)
        }

        WM_LBUTTONUP | WM_RBUTTONUP | WM_MBUTTONUP => {
            let button = match msg {
                WM_LBUTTONUP => MouseButton::Left,
                WM_RBUTTONUP => MouseButton::Right,
                _ => MouseButton::Middle,
            };
            let (release_cap, ev) = if let Some(s) = lookup_window(hwnd) {
                let release_cap = {
                    let mut input = s.input.borrow_mut();
                    input.buttons.release(button);
                    input.buttons.is_empty()
                };
                let (_, _, position) = client_point_from_lparam(lparam, s.scale());
                (
                    release_cap,
                    Some(PlatformEvent::MouseUp {
                        window_id: s.id,
                        button,
                        position,
                    }),
                )
            } else {
                (true, None)
            };
            if release_cap {
                release_capture_suppressed(hwnd, || {
                    let _ = unsafe { ReleaseCapture() };
                });
            }
            if let Some(ev) = ev {
                dispatch(ev);
            }
            LRESULT(0)
        }

        WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
            let modifiers = current_modifiers();
            let horizontal = msg == WM_MOUSEHWHEEL || modifiers.shift;
            let raw_delta = ((wparam.0 >> 16) & 0xFFFF) as i16 as f32;
            let notches = raw_delta / 120.0;
            let raw_amount = pump::cached_settings().wheel_scroll_amount(horizontal);
            // WHEEL_PAGESCROLL is u32::MAX.
            let page_scroll = !horizontal && raw_amount == u32::MAX;
            let amount = if page_scroll { 1.0 } else { raw_amount.max(1) as f32 };
            let delta_lines = if horizontal {
                point(
                    if msg == WM_MOUSEHWHEEL {
                        -notches * amount
                    } else {
                        notches * amount
                    },
                    0.0,
                )
            } else {
                point(0.0, notches * amount)
            };
            if let Some(s) = lookup_window(hwnd) {
                let position = screen_to_client_point(hwnd, lparam, s.scale());
                dispatch(PlatformEvent::ScrollWheel {
                    window_id: s.id,
                    position,
                    delta_lines,
                    page_scroll,
                    modifiers,
                });
            }
            LRESULT(0)
        }

        WM_KEYDOWN | WM_SYSKEYDOWN => dispatch_key_message(hwnd, msg, wparam, lparam, true),

        WM_SYSCHAR => {
            // Forward Alt+Space to DefWindowProcW so the native system menu opens.
            if (wparam.0 & 0xFFFF) == b' ' as usize {
                return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
            }
            LRESULT(0)
        }

        WM_KEYUP | WM_SYSKEYUP => dispatch_key_message(hwnd, msg, wparam, lparam, false),

        WM_CHAR => {
            let code = (wparam.0 & 0xFFFF) as u16;
            if let Some(s) = lookup_window(hwnd) {
                let text = {
                    let mut input = s.input.borrow_mut();
                    if std::mem::take(&mut input.suppress_next_char) {
                        None
                    } else if (0xD800..=0xDBFF).contains(&code) {
                        input.pending_surrogate = Some(code);
                        None
                    } else if (0xDC00..=0xDFFF).contains(&code) {
                        input
                            .pending_surrogate
                            .take()
                            .and_then(|high| String::from_utf16(&[high, code]).ok())
                    } else {
                        input.pending_surrogate = None;
                        (code >= 0x20 && code != 0x7F)
                            .then(|| char::from_u32(u32::from(code)).map(|c| c.to_string()))
                            .flatten()
                    }
                };
                if let Some(text) = text {
                    dispatch(PlatformEvent::TextInput {
                        window_id: s.id,
                        text,
                    });
                }
            }
            LRESULT(0)
        }

        WM_IME_STARTCOMPOSITION => {
            update_ime_pos(hwnd);
            LRESULT(0)
        }

        WM_IME_COMPOSITION => {
            let flags = lparam.0 as u32;
            if let Some(s) = lookup_window(hwnd) {
                let himc = unsafe { ImmGetContext(hwnd) };
                if !himc.is_invalid() {
                    if (flags & GCS_RESULTSTR.0) != 0
                        && let Some(text) = read_ime_string(himc, GCS_RESULTSTR)
                    {
                        dispatch(PlatformEvent::TextInput {
                            window_id: s.id,
                            text,
                        });
                    }
                    if (flags & GCS_COMPSTR.0) != 0 {
                        let text = read_ime_string(himc, GCS_COMPSTR).unwrap_or_default();
                        let cursor = unsafe {
                            ImmGetCompositionStringW(himc, GCS_CURSORPOS, None, 0)
                        }
                        .max(0) as usize;
                        dispatch(PlatformEvent::ImeComposition {
                            window_id: s.id,
                            text,
                            cursor,
                        });
                    }
                    let _ = unsafe { ImmReleaseContext(hwnd, himc) };
                    update_ime_pos(hwnd);
                }
            }
            LRESULT(0)
        }

        WM_IME_ENDCOMPOSITION => {
            if let Some(s) = lookup_window(hwnd) {
                dispatch(PlatformEvent::ImeComposition {
                    window_id: s.id,
                    text: String::new(),
                    cursor: 0,
                });
            }
            LRESULT(0)
        }

        WM_SIZE => {
            let w = (lparam.0 & 0xFFFF) as u32;
            let h = ((lparam.0 >> 16) & 0xFFFF) as u32;
            if w > 0
                && h > 0
                && let Some(s) = lookup_window(hwnd)
            {
                let scale = s.scale();
                s.set_metrics(w, h, scale);
                dispatch(PlatformEvent::Resized {
                    window_id: s.id,
                    physical_width: w,
                    physical_height: h,
                    scale,
                });
            }
            LRESULT(0)
        }

        WM_MOVE => {
            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            if let Some(s) = lookup_window(hwnd)
                && s.last_origin_px.replace(Some((x, y))) != Some((x, y))
            {
                dispatch(PlatformEvent::Moved { window_id: s.id });
            }
            LRESULT(0)
        }

        WM_DPICHANGED => {
            let new_dpi = (wparam.0 & 0xFFFF) as u32;
            let scale =
                new_dpi.max(USER_DEFAULT_SCREEN_DPI) as f32 / USER_DEFAULT_SCREEN_DPI as f32;
            // Update scale before SetWindowPos so nested WM_SIZE sees the new DPI.
            if let Some(s) = lookup_window(hwnd) {
                let (pw, ph) = s.metrics.get().physical;
                s.set_metrics(pw, ph, scale);
                let suggested = unsafe { &*(lparam.0 as *const RECT) };
                unsafe {
                    let _ = SetWindowPos(
                        hwnd,
                        None,
                        suggested.left,
                        suggested.top,
                        suggested.right - suggested.left,
                        suggested.bottom - suggested.top,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                }
                let (w, h) = query_physical_size(hwnd);
                s.set_metrics(w, h, scale);
                dispatch(PlatformEvent::Resized {
                    window_id: s.id,
                    physical_width: w,
                    physical_height: h,
                    scale,
                });
            }
            LRESULT(0)
        }

        WM_ACTIVATE => {
            let active = (wparam.0 & 0xFFFF) != 0;
            if let Some(s) = lookup_window(hwnd) {
                s.active.set(active);
                dispatch(PlatformEvent::Activated {
                    window_id: s.id,
                    active,
                });
            }
            LRESULT(0)
        }

        WM_SETTINGCHANGE | WM_THEMECHANGED => {
            if lookup_window(hwnd).is_some()
                && (msg == WM_THEMECHANGED || is_relevant_setting_change(lparam))
            {
                let fresh = SystemSettings::query();
                pump::set_cached_settings(fresh);
                dispatch(PlatformEvent::SettingsChanged(fresh));
            }
            LRESULT(0)
        }

        WM_ENTERSIZEMOVE | WM_ENTERMENULOOP => {
            if let Some(s) = lookup_window(hwnd) {
                s.in_size_move.set(true);
                let was_hovered = std::mem::take(&mut s.input.borrow_mut().hovered);
                if was_hovered {
                    dispatch(PlatformEvent::MouseLeave { window_id: s.id });
                }
                dispatch(PlatformEvent::SizeMoveChanged {
                    window_id: s.id,
                    entering: true,
                });
            }
            unsafe {
                let _ = SetTimer(Some(hwnd), SIZE_MOVE_TIMER_ID, USER_TIMER_MINIMUM, None);
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
        }

        WM_TIMER if wparam.0 == SIZE_MOVE_TIMER_ID => {
            pump::mark_wake_pending();
            service();
            LRESULT(0)
        }

        WM_EXITSIZEMOVE | WM_EXITMENULOOP => {
            let _ = unsafe { KillTimer(Some(hwnd), SIZE_MOVE_TIMER_ID) };
            if let Some(s) = lookup_window(hwnd) {
                s.in_size_move.set(false);
                dispatch(PlatformEvent::SizeMoveChanged {
                    window_id: s.id,
                    entering: false,
                });
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }

        WM_QUERYENDSESSION => {
            if let Some(reason) = pump::shutdown_block() {
                let wide = HSTRING::from(reason);
                let _ = unsafe { ShutdownBlockReasonCreate(hwnd, &wide) };
                if let Some(s) = lookup_window(hwnd) {
                    dispatch(PlatformEvent::CloseRequested { window_id: s.id });
                }
                LRESULT(0)
            } else {
                let _ = unsafe { ShutdownBlockReasonDestroy(hwnd) };
                LRESULT(1)
            }
        }

        WM_ENDSESSION => {
            let ending = wparam.0 != 0;
            if !ending {
                let _ = unsafe { ShutdownBlockReasonDestroy(hwnd) };
            }
            dispatch(PlatformEvent::SessionEnding { ending });
            LRESULT(0)
        }

        WM_DROPFILES => {
            let hdrop = HDROP(wparam.0 as *mut _);
            let paths = collect_drop_paths(hdrop);
            unsafe { DragFinish(hdrop) };
            if !paths.is_empty()
                && let Some(s) = lookup_window(hwnd)
            {
                dispatch(PlatformEvent::FilesDropped {
                    window_id: s.id,
                    paths,
                });
            }
            LRESULT(0)
        }

        WM_CLOSE => {
            if let Some(s) = lookup_window(hwnd) {
                dispatch(PlatformEvent::CloseRequested { window_id: s.id });
            }
            LRESULT(0)
        }

        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn collect_drop_paths(hdrop: HDROP) -> Vec<PathBuf> {
    let file_count = unsafe { DragQueryFileW(hdrop, 0xFFFF_FFFF, None) };
    let mut paths = Vec::with_capacity(file_count as usize);
    for ix in 0..file_count {
        let len = unsafe { DragQueryFileW(hdrop, ix, None) } as usize;
        if len == 0 {
            continue;
        }
        let mut buffer = vec![0u16; len + 1];
        let written = unsafe { DragQueryFileW(hdrop, ix, Some(buffer.as_mut_slice())) } as usize;
        if written > 0 {
            paths.push(PathBuf::from(OsString::from_wide(&buffer[..written])));
        }
    }
    paths
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use windows::Win32::UI::WindowsAndMessaging::SendMessageW;

    use crate::platform::{MainWindowConfig, PlatformHandler, WindowAppearance, pump::Platform, window::NativeWindow};

    use super::*;

    #[test]
    fn release_capture_suppresses_synchronous_capture_lost_but_external_emits() {
        #[derive(Default)]
        struct CaptureProbe {
            lost: usize,
        }

        impl PlatformHandler for CaptureProbe {
            fn on_event(&mut self, event: PlatformEvent) -> bool {
                if matches!(event, PlatformEvent::CaptureLost { .. }) {
                    self.lost += 1;
                }
                false
            }

            fn on_wake(&mut self) {}
            fn drive_frames(&mut self) {}
        }

        let _platform = Platform::new(None).expect("Platform::new failed");
        let cfg = MainWindowConfig {
            show: false,
            focus: false,
            ..Default::default()
        };
        let win = NativeWindow::create_main(&cfg, WindowAppearance::Light).expect("window");
        let hwnd = win.hwnd().raw();
        let probe = Rc::new(RefCell::new(CaptureProbe::default()));

        Platform::with_test_handler(probe.clone(), || {
            release_capture_suppressed(hwnd, || unsafe {
                let _ = SendMessageW(hwnd, WM_CAPTURECHANGED, Some(WPARAM(0)), Some(LPARAM(0)));
            });
            assert_eq!(
                probe.borrow().lost,
                0,
                "synchronous WM_CAPTURECHANGED during own ReleaseCapture must be suppressed"
            );

            unsafe {
                let _ = SendMessageW(hwnd, WM_CAPTURECHANGED, Some(WPARAM(0)), Some(LPARAM(0)));
            }
            assert_eq!(
                probe.borrow().lost,
                1,
                "external WM_CAPTURECHANGED must emit CaptureLost"
            );
        });
    }
}
