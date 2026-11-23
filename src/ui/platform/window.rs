//! Clean-room Win32 window creation, Mica Alt framing, Snap Layouts hit-testing, IME,
//! mouse/keyboard/wheel dispatch, and OLE file drag-and-drop.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use windows::Win32::{
    Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
    Graphics::{
        Dwm::{
            DWM_SYSTEMBACKDROP_TYPE, DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE,
            DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_TRANSITIONS_FORCEDISABLED,
            DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE,
            DWMWCP_DONOTROUND, DwmExtendFrameIntoClientArea, DwmSetWindowAttribute,
        },
        Gdi::{ScreenToClient, ValidateRect},
    },
    System::{
        Com::{DVASPECT_CONTENT, FORMATETC, IDataObject, TYMED_HGLOBAL},
        LibraryLoader::GetModuleHandleW,
        Memory::{GlobalLock, GlobalUnlock},
        Ole::{
            CF_HDROP, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_NONE, IDropTarget, IDropTarget_Impl,
            RegisterDragDrop, RevokeDragDrop,
        },
    },
    UI::{
        Controls::{MARGINS, WM_MOUSELEAVE},
        HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi},
        Input::{
            Ime::{
                CANDIDATEFORM, CFS_CANDIDATEPOS, CFS_POINT, COMPOSITIONFORM, GCS_COMPSTR,
                GCS_CURSORPOS, GCS_RESULTSTR, ImmGetCompositionStringW, ImmGetContext,
                ImmReleaseContext, ImmSetCandidateWindow, ImmSetCompositionWindow,
            },
            KeyboardAndMouse::{
                GetDoubleClickTime, GetKeyState, ReleaseCapture, SetCapture, TME_LEAVE,
                TME_NONCLIENT, TRACKMOUSEEVENT, TrackMouseEvent, VK_ADD, VK_APPS, VK_BACK,
                VK_CONTROL, VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE, VK_F1, VK_F4, VK_F12, VK_HOME,
                VK_LEFT, VK_LWIN, VK_MENU, VK_NEXT, VK_NUMPAD0, VK_NUMPAD9, VK_OEM_4, VK_OEM_6,
                VK_OEM_MINUS, VK_OEM_PLUS, VK_PRIOR, VK_RETURN, VK_RIGHT, VK_RWIN, VK_SHIFT,
                VK_SPACE, VK_SUBTRACT, VK_TAB, VK_UP, VIRTUAL_KEY,
            },
        },
        Shell::HDROP,
        WindowsAndMessaging::{
            CS_DBLCLKS, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW,
            DestroyWindow, GetClientRect, GetCursorPos, GetSystemMetrics, GetWindowRect, HTCAPTION,
            HTCLIENT, HTCLOSE, HTMAXBUTTON, HTMINBUTTON, HTTOP, HTTOPLEFT, HTTOPRIGHT, IDC_ARROW,
            IDC_IBEAM, IsZoomed, LWA_ALPHA, LoadCursorW, LoadIconW, MINMAXINFO, NCCALCSIZE_PARAMS,
            PostMessageW, RegisterClassW, SC_CLOSE, SC_MAXIMIZE, SC_MINIMIZE, SC_RESTORE,
            SM_CXDOUBLECLK, SM_CXFRAME, SM_CXPADDEDBORDER, SM_CYDOUBLECLK, SM_CYFRAME, SW_SHOW,
            SW_SHOWNOACTIVATE, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOZORDER, SetCursor,
            SetLayeredWindowAttributes, SetWindowPos, SetWindowTextW, ShowWindow,
            USER_DEFAULT_SCREEN_DPI, WM_ACTIVATE, WM_CAPTURECHANGED, WM_CHAR, WM_CLOSE,
            WM_DPICHANGED, WM_ENTERSIZEMOVE, WM_ERASEBKGND, WM_EXITSIZEMOVE, WM_GETMINMAXINFO,
            WM_IME_COMPOSITION, WM_IME_ENDCOMPOSITION, WM_IME_STARTCOMPOSITION, WM_KEYDOWN,
            WM_KEYUP, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN,
            WM_MBUTTONUP, WM_MOUSEHWHEEL, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_MOVE, WM_NCCALCSIZE,
            WM_NCHITTEST, WM_NCLBUTTONDOWN, WM_NCLBUTTONUP, WM_NCMOUSELEAVE, WM_NCMOUSEMOVE,
            WM_PAINT, WM_RBUTTONDBLCLK, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SETCURSOR,
            WM_SETTINGCHANGE, WM_SIZE, WM_SYSCHAR, WM_SYSCOMMAND, WM_SYSKEYDOWN, WM_SYSKEYUP,
            WM_THEMECHANGED, WNDCLASSW, WS_EX_APPWINDOW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
            WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT,
            WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_POPUP, WS_SYSMENU, WS_THICKFRAME, WindowFromPoint,
        },
    },
};
use windows::core::{BOOL, HSTRING, PCWSTR, implement, w};

use crate::win32::WindowHwnd;

use super::{
    ACTIVE_APP, CursorStyle, ExternalPaths, WindowAppearance, WindowBackgroundAppearance,
    WindowControlArea, WindowKind, WindowOptions, WindowsDisplay, clear_window_min_size,
    drive_frames, set_window_min_size, window_min_size,
};
use crate::ui::anyhow::Result;
use crate::ui::geometry::{Bounds, Pixels, Point, Size, point, px, size};
use crate::ui::keymap::{Keystroke, Modifiers};
use crate::ui::window::{
    KeyDownEvent, ModifiersChangedEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ScrollDelta, ScrollWheelEvent, WindowId,
};

const WINDOW_CLASS: PCWSTR = w!("JotD2DWindow");
const DWMSBT_TABBEDWINDOW: i32 = 4;

thread_local! {
    static HWND_TO_WINDOW: RefCell<HashMap<isize, WindowId>> = RefCell::new(HashMap::new());
    /// Lock-free `(is_overlay, hide_titlebar)` per HWND so `WM_NCCALCSIZE` works even during
    /// `CreateWindowExW`/`SetWindowPos`/`ShowWindow` while `AppCell` is mutably borrowed.
    static HWND_FRAME_FLAGS: RefCell<HashMap<isize, (bool, bool)>> = RefCell::new(HashMap::new());
}

pub(crate) fn register_window_class() -> Result<()> {
    let hinstance: HINSTANCE = unsafe { GetModuleHandleW(None)? }.into();
    let hicon =
        unsafe { LoadIconW(Some(hinstance), PCWSTR(std::ptr::without_provenance(1))) }
            .unwrap_or_default();
    let wc = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW | CS_DBLCLKS,
        lpfnWndProc: Some(window_wnd_proc),
        hInstance: hinstance,
        hIcon: hicon,
        lpszClassName: WINDOW_CLASS,
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW).unwrap_or_default() },
        ..Default::default()
    };
    unsafe { RegisterClassW(&wc) };
    Ok(())
}

/// Per-HWND native state tracked alongside `crate::ui::window::Window`.
pub(crate) struct NativeWindow {
    pub(crate) hwnd: HWND,
    pub(crate) kind: WindowKind,
    pub(crate) is_resizable: bool,
    pub(crate) scale_factor: f32,
    pub(crate) logical_size: Size<Pixels>,
    pub(crate) origin: Point<Pixels>,
    pub(crate) active: bool,
    pub(crate) hovered: bool,
    pub(crate) cursor: CursorStyle,
    pub(crate) nc_button_down: Option<u32>,
    pub(crate) pending_surrogate: Option<u16>,
    pub(crate) suppress_next_char: bool,
    pub(crate) last_modifiers: Modifiers,
    pub(crate) last_click_button: Option<MouseButton>,
    pub(crate) last_click_pos: (i32, i32),
    pub(crate) last_click_time: Option<Instant>,
    pub(crate) click_count: usize,
    pub(crate) pressed_button: Option<MouseButton>,
    pub(crate) tracking_leave: bool,
    pub(crate) tracking_nc_leave: bool,
    pub(crate) drop_registered: bool,
}

impl NativeWindow {
    pub(crate) fn create(
        window_id: WindowId,
        options: &WindowOptions,
        appearance: WindowAppearance,
    ) -> Result<Self> {
        let hinstance: HINSTANCE = unsafe { GetModuleHandleW(None)? }.into();
        let is_overlay = matches!(options.kind, WindowKind::Overlay);

        let (mut ex_style, style) = if is_overlay {
            (
                WS_EX_NOACTIVATE
                    | WS_EX_TOOLWINDOW
                    | WS_EX_TOPMOST
                    | WS_EX_TRANSPARENT
                    | WS_EX_LAYERED,
                WS_POPUP,
            )
        } else {
            let mut st = WS_SYSMENU;
            if options.is_resizable {
                st |= WS_THICKFRAME | WS_MAXIMIZEBOX;
            }
            if options.is_minimizable {
                st |= WS_MINIMIZEBOX;
            }
            (WS_EX_APPWINDOW, st)
        };
        ex_style |= WS_EX_NOREDIRECTIONBITMAP;

        let title = options
            .titlebar
            .as_ref()
            .and_then(|t| t.title.as_deref())
            .unwrap_or("Jot");
        let wide_title = HSTRING::from(title);

        let (init_x, init_y, init_w, init_h) = if is_overlay {
            (0, 0, 1, 1)
        } else {
            (CW_USEDEFAULT, CW_USEDEFAULT, CW_USEDEFAULT, CW_USEDEFAULT)
        };
        let hwnd = unsafe {
            CreateWindowExW(
                ex_style,
                WINDOW_CLASS,
                &wide_title,
                style,
                init_x,
                init_y,
                init_w,
                init_h,
                None,
                None,
                Some(hinstance),
                None,
            )?
        };

        let dpi = unsafe { GetDpiForWindow(hwnd) }.max(USER_DEFAULT_SCREEN_DPI);
        let hide_titlebar = options
            .titlebar
            .as_ref()
            .is_none_or(|t| t.appears_transparent);
        HWND_FRAME_FLAGS.with(|map| {
            map.borrow_mut()
                .insert(hwnd.0 as isize, (is_overlay, hide_titlebar));
        });
        let target_scale = options
            .display_id
            .and_then(WindowsDisplay::from_id)
            .map_or(dpi as f32 / USER_DEFAULT_SCREEN_DPI as f32, |d| {
                d.scale_factor()
            });

        if is_overlay {
            set_dwm_u32(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND.0 as u32);
            set_dwm_u32(hwnd, DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE);
            set_dwm_u32(hwnd, DWMWA_TRANSITIONS_FORCEDISABLED, 1);
            unsafe {
                let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
            }
        } else {
            configure_backdrop(hwnd, options.window_background);
            configure_dark_mode(hwnd, appearance);
            if options.window_min_size.is_some() {
                set_window_min_size(WindowHwnd::from_raw(hwnd), options.window_min_size);
            }
        }

        if let Some(wb) = options.window_bounds {
            let b = wb.get_bounds();
            let mut x = (b.origin.x.as_f32() * target_scale).round() as i32;
            let y = (b.origin.y.as_f32() * target_scale).round() as i32;
            let mut w = (b.size.width.as_f32() * target_scale).round().max(1.0) as i32;
            let mut h = (b.size.height.as_f32() * target_scale).round().max(1.0) as i32;
            if !is_overlay && options.is_resizable {
                let target_dpi = (target_scale * USER_DEFAULT_SCREEN_DPI as f32).round() as u32;
                let (bx, by) = frame_thickness_for_dpi(target_dpi);
                x -= bx;
                w += bx * 2;
                h += by;
            }
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

        let final_dpi = unsafe { GetDpiForWindow(hwnd) }.max(USER_DEFAULT_SCREEN_DPI);
        let scale_factor = final_dpi as f32 / USER_DEFAULT_SCREEN_DPI as f32;

        let drop_registered = if !is_overlay {
            let target: IDropTarget = JotDropTarget { window_id }.into();
            unsafe { RegisterDragDrop(hwnd, &target) }.is_ok()
        } else {
            false
        };

        HWND_TO_WINDOW.with(|map| {
            map.borrow_mut().insert(hwnd.0 as isize, window_id);
        });

        let (logical_size, origin) = query_client_metrics(hwnd, scale_factor);

        Ok(Self {
            hwnd,
            kind: options.kind,
            is_resizable: options.is_resizable && !is_overlay,
            scale_factor,
            logical_size,
            origin,
            active: options.show && options.focus,
            hovered: false,
            cursor: CursorStyle::Arrow,
            nc_button_down: None,
            pending_surrogate: None,
            suppress_next_char: false,
            last_modifiers: current_modifiers(),
            last_click_button: None,
            last_click_pos: (0, 0),
            last_click_time: None,
            click_count: 0,
            pressed_button: None,
            tracking_leave: false,
            tracking_nc_leave: false,
            drop_registered,
        })
    }

    pub(crate) fn show(&self, show: bool, focus: bool) {
        if show {
            unsafe {
                let _ = ShowWindow(
                    self.hwnd,
                    if focus { SW_SHOW } else { SW_SHOWNOACTIVATE },
                );
            }
        }
    }

    pub(crate) fn set_title(&self, title: &str) {
        let wide = HSTRING::from(title);
        unsafe {
            let _ = SetWindowTextW(self.hwnd, &wide);
        }
    }

    pub(crate) fn set_appearance(&self, appearance: WindowAppearance) {
        if matches!(self.kind, WindowKind::Normal) {
            configure_dark_mode(self.hwnd, appearance);
        }
    }

    pub(crate) fn set_cursor(&mut self, style: CursorStyle) {
        self.cursor = style;
        apply_cursor(style);
    }

    pub(crate) fn is_maximized(&self) -> bool {
        unsafe { IsZoomed(self.hwnd) }.as_bool()
    }

    pub(crate) fn window_hwnd(&self) -> WindowHwnd {
        WindowHwnd::from_raw(self.hwnd)
    }

    pub(crate) fn bounds(&self) -> Bounds<Pixels> {
        Bounds::new(self.origin, self.logical_size)
    }

    pub(crate) fn physical_size(&self) -> (u32, u32) {
        let mut rc = RECT::default();
        let _ = unsafe { GetClientRect(self.hwnd, &mut rc) };
        (
            (rc.right - rc.left).max(1) as u32,
            (rc.bottom - rc.top).max(1) as u32,
        )
    }

    /// Queries the live Win32 client rect and DPI without clamping zero-sized minimized windows.
    /// Returns `Some((w, h, scale))` when the client area is non-empty.
    pub(crate) fn live_client_metrics(&mut self) -> Option<(u32, u32, f32)> {
        let mut rc = RECT::default();
        if unsafe { GetClientRect(self.hwnd, &mut rc) }.is_err() {
            return None;
        }
        let w = rc.right - rc.left;
        let h = rc.bottom - rc.top;
        if w <= 0 || h <= 0 {
            return None;
        }
        let dpi = unsafe { GetDpiForWindow(self.hwnd) }.max(USER_DEFAULT_SCREEN_DPI);
        let scale = dpi as f32 / USER_DEFAULT_SCREEN_DPI as f32;
        self.scale_factor = scale;
        self.logical_size = size(px(w as f32 / scale), px(h as f32 / scale));
        Some((w as u32, h as u32, scale))
    }

    pub(crate) fn destroy(&mut self) {
        HWND_TO_WINDOW.with(|map| {
            map.borrow_mut().remove(&(self.hwnd.0 as isize));
        });
        HWND_FRAME_FLAGS.with(|map| {
            map.borrow_mut().remove(&(self.hwnd.0 as isize));
        });
        clear_window_min_size(self.hwnd);
        if self.drop_registered {
            unsafe {
                let _ = RevokeDragDrop(self.hwnd);
            }
            self.drop_registered = false;
        }
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }

    fn record_click(&mut self, button: MouseButton, x: i32, y: i32) -> usize {
        let now = Instant::now();
        let dbl_ms = unsafe { GetDoubleClickTime() } as u128;
        let tol_x = unsafe { GetSystemMetrics(SM_CXDOUBLECLK) } / 2;
        let tol_y = unsafe { GetSystemMetrics(SM_CYDOUBLECLK) } / 2;
        let same_spot = self.last_click_button == Some(button)
            && self
                .last_click_time
                .is_some_and(|t| now.duration_since(t).as_millis() <= dbl_ms)
            && (x - self.last_click_pos.0).abs() <= tol_x.max(4)
            && (y - self.last_click_pos.1).abs() <= tol_y.max(4);
        self.click_count = if same_spot { self.click_count + 1 } else { 1 };
        self.last_click_button = Some(button);
        self.last_click_pos = (x, y);
        self.last_click_time = Some(now);
        self.click_count
    }

    fn client_point(&self, lparam: LPARAM) -> (i32, i32, Point<Pixels>) {
        let x = (lparam.0 & 0xFFFF) as i16 as i32;
        let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
        (
            x,
            y,
            point(px(x as f32 / self.scale_factor), px(y as f32 / self.scale_factor)),
        )
    }

    fn screen_to_client_point(&self, lparam: LPARAM) -> Point<Pixels> {
        let mut pt = POINT {
            x: (lparam.0 & 0xFFFF) as i16 as i32,
            y: ((lparam.0 >> 16) & 0xFFFF) as i16 as i32,
        };
        unsafe {
            let _ = ScreenToClient(self.hwnd, &mut pt);
        }
        point(
            px(pt.x as f32 / self.scale_factor),
            px(pt.y as f32 / self.scale_factor),
        )
    }
}

fn query_client_metrics(hwnd: HWND, scale: f32) -> (Size<Pixels>, Point<Pixels>) {
    let mut client = RECT::default();
    let _ = unsafe { GetClientRect(hwnd, &mut client) };
    let mut win_rc = RECT::default();
    let _ = unsafe { GetWindowRect(hwnd, &mut win_rc) };
    (
        size(
            px((client.right - client.left).max(1) as f32 / scale),
            px((client.bottom - client.top).max(1) as f32 / scale),
        ),
        point(px(win_rc.left as f32 / scale), px(win_rc.top as f32 / scale)),
    )
}

fn set_dwm_u32(hwnd: HWND, attr: windows::Win32::Graphics::Dwm::DWMWINDOWATTRIBUTE, value: u32) {
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
    let pad = unsafe { GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi) };
    let bx = unsafe { GetSystemMetricsForDpi(SM_CXFRAME, dpi) } + pad;
    let by = unsafe { GetSystemMetricsForDpi(SM_CYFRAME, dpi) } + pad;
    (bx, by)
}

fn configure_backdrop(hwnd: HWND, bg: WindowBackgroundAppearance) {
    let margins = MARGINS {
        cxLeftWidth: -1,
        cxRightWidth: -1,
        cyTopHeight: -1,
        cyBottomHeight: -1,
    };
    let _ = unsafe { DwmExtendFrameIntoClientArea(hwnd, &margins) };
    if matches!(bg, WindowBackgroundAppearance::MicaAltBackdrop) {
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
}

fn apply_cursor(style: CursorStyle) {
    let id = match style {
        CursorStyle::Arrow => IDC_ARROW,
        CursorStyle::IBeam => IDC_IBEAM,
    };
    if let Ok(cursor) = unsafe { LoadCursorW(None, id) } {
        unsafe {
            SetCursor(Some(cursor));
        }
    }
}

pub(crate) fn current_modifiers() -> Modifiers {
    let down = |vk: VIRTUAL_KEY| (unsafe { GetKeyState(i32::from(vk.0)) } as u16 & 0x8000) != 0;
    Modifiers {
        control: down(VK_CONTROL),
        alt: down(VK_MENU),
        shift: down(VK_SHIFT),
        platform: down(VK_LWIN) || down(VK_RWIN),
        function: false,
    }
}

fn translate_vk(vk: u16, modifiers: Modifiers) -> Option<String> {
    let v = VIRTUAL_KEY(vk);
    let s = match v {
        VK_BACK => "backspace",
        VK_TAB => "tab",
        VK_RETURN => "enter",
        VK_ESCAPE => "escape",
        VK_SPACE => "space",
        VK_PRIOR => "pageup",
        VK_NEXT => "pagedown",
        VK_END => "end",
        VK_HOME => "home",
        VK_LEFT => "left",
        VK_UP => "up",
        VK_RIGHT => "right",
        VK_DOWN => "down",
        VK_DELETE => "delete",
        VK_MENU => "alt",
        VK_APPS => "menu",
        VK_OEM_PLUS => {
            if modifiers.shift {
                "+"
            } else {
                "="
            }
        }
        VK_ADD => "+",
        VK_OEM_MINUS | VK_SUBTRACT => "-",
        VK_OEM_4 => "[",
        VK_OEM_6 => "]",
        _ if (VK_F1.0..=VK_F12.0).contains(&vk) => {
            return Some(format!("f{}", vk - VK_F1.0 + 1));
        }
        _ if (VK_NUMPAD0.0..=VK_NUMPAD9.0).contains(&vk) => {
            return Some(((b'0' + (vk - VK_NUMPAD0.0) as u8) as char).to_string());
        }
        _ if (b'0' as u16..=b'9' as u16).contains(&vk) => {
            return Some((vk as u8 as char).to_string());
        }
        _ if (b'A' as u16..=b'Z' as u16).contains(&vk) => {
            return Some(((vk as u8 as char).to_ascii_lowercase()).to_string());
        }
        _ => return None,
    };
    Some(s.to_owned())
}

// ---------------------------------------------------------------------------
// OLE Drop Target for dragging files onto Jot
// ---------------------------------------------------------------------------

#[implement(IDropTarget)]
struct JotDropTarget {
    window_id: WindowId,
}

impl IDropTarget_Impl for JotDropTarget_Impl {
    fn DragEnter(
        &self,
        _pdataobj: windows::core::Ref<'_, IDataObject>,
        _grfkeystate: windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS,
        _pt: &windows::Win32::Foundation::POINTL,
        pdweffect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        unsafe {
            *pdweffect = DROPEFFECT_COPY;
        }
        Ok(())
    }

    fn DragOver(
        &self,
        _grfkeystate: windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS,
        _pt: &windows::Win32::Foundation::POINTL,
        pdweffect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        unsafe {
            *pdweffect = DROPEFFECT_COPY;
        }
        Ok(())
    }

    fn DragLeave(&self) -> windows::core::Result<()> {
        Ok(())
    }

    fn Drop(
        &self,
        pdataobj: windows::core::Ref<'_, IDataObject>,
        _grfkeystate: windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS,
        pt: &windows::Win32::Foundation::POINTL,
        pdweffect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        let Some(data_obj) = pdataobj.as_ref() else {
            unsafe {
                *pdweffect = DROPEFFECT_NONE;
            }
            return Ok(());
        };
        let format = FORMATETC {
            cfFormat: CF_HDROP.0,
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        };
        let Ok(medium) = (unsafe { data_obj.GetData(&format) }) else {
            unsafe {
                *pdweffect = DROPEFFECT_NONE;
            }
            return Ok(());
        };
        let mut paths = Vec::new();
        unsafe {
            let hglobal = medium.u.hGlobal;
            let locked = GlobalLock(hglobal);
            if !locked.is_null() {
                crate::win32::with_file_names(HDROP(locked), |name| {
                    paths.push(PathBuf::from(name));
                });
                let _ = GlobalUnlock(hglobal);
            }
            *pdweffect = DROPEFFECT_COPY;
        }
        if !paths.is_empty()
            && let Some(app_cell) = ACTIVE_APP.with(|s| s.borrow().clone())
            && let Ok(mut app) = app_cell.inner.try_borrow_mut()
        {
            let _ = app.update_window_impl(self.window_id, |_, window, cx| {
                let mut client = POINT { x: pt.x, y: pt.y };
                let _ = unsafe { ScreenToClient(window.native.hwnd, &mut client) };
                let pos = point(
                    px(client.x as f32 / window.native.scale_factor),
                    px(client.y as f32 / window.native.scale_factor),
                );
                window.dispatch_file_drop(ExternalPaths(paths), pos, cx);
            });
            drop(app);
            drive_frames(&app_cell);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Main Win32 window procedure
// ---------------------------------------------------------------------------

fn with_window_for_hwnd<R>(
    hwnd: HWND,
    f: impl FnOnce(&mut crate::ui::window::Window, &mut crate::ui::app::App) -> R,
) -> Option<R> {
    let window_id = HWND_TO_WINDOW.with(|m| m.borrow().get(&(hwnd.0 as isize)).copied())?;
    let app_cell = ACTIVE_APP.with(|s| s.borrow().clone())?;
    let mut app = app_cell.inner.try_borrow_mut().ok()?;
    let result = app
        .update_window_impl(window_id, |_, window, cx| f(window, cx))
        .ok();
    drop(app);
    drive_frames(&app_cell);
    result
}

/// Read-only window query for high-frequency Win32 hit-test / cursor messages that must not
/// trigger frame rendering.
fn query_window_for_hwnd<R>(
    hwnd: HWND,
    f: impl FnOnce(&crate::ui::window::Window) -> R,
) -> Option<R> {
    let window_id = HWND_TO_WINDOW.with(|m| m.borrow().get(&(hwnd.0 as isize)).copied())?;
    let app_cell = ACTIVE_APP.with(|s| s.borrow().clone())?;
    let app = app_cell.inner.try_borrow().ok()?;
    let window = app.windows.get(&window_id)?.as_ref()?;
    Some(f(window))
}

fn cursor_is_over_hwnd(hwnd: HWND) -> bool {
    let mut pt = POINT::default();
    unsafe { GetCursorPos(&mut pt).is_ok() && WindowFromPoint(pt) == hwnd }
}

unsafe extern "system" fn window_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_ERASEBKGND => LRESULT(1),

        WM_PAINT => {
            let _ = unsafe { ValidateRect(Some(hwnd), None) };
            let window_id = HWND_TO_WINDOW.with(|m| m.borrow().get(&(hwnd.0 as isize)).copied());
            if let Some(id) = window_id
                && let Some(app_cell) = ACTIVE_APP.with(|s| s.borrow().clone())
                && let Ok(mut app) = app_cell.inner.try_borrow_mut()
            {
                let _ = app.update_window_impl(id, |root, window, cx| {
                    window.draw_frame(&root, cx);
                });
            }
            LRESULT(0)
        }

        WM_NCCALCSIZE if wparam.0 != 0 => {
            let (is_overlay, hide_titlebar) = HWND_FRAME_FLAGS
                .with(|m| m.borrow().get(&(hwnd.0 as isize)).copied())
                .unwrap_or((false, true));
            if is_overlay {
                return LRESULT(0);
            }
            if !hide_titlebar {
                return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
            }
            // Keep Windows 11's native invisible left/right/bottom resize border + DWM shadow,
            // removing only the top Win32 titlebar.
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
            if let Some(min_size) = window_min_size(hwnd) {
                let dpi = unsafe { GetDpiForWindow(hwnd) }.max(USER_DEFAULT_SCREEN_DPI);
                let scale = dpi as f32 / USER_DEFAULT_SCREEN_DPI as f32;
                let (bx, by) = frame_thickness_for_dpi(dpi);
                let mmi = unsafe { &mut *(lparam.0 as *mut MINMAXINFO) };
                mmi.ptMinTrackSize.x = (min_size.width.as_f32() * scale).ceil() as i32 + bx * 2;
                mmi.ptMinTrackSize.y = (min_size.height.as_f32() * scale).ceil() as i32 + by;
                return LRESULT(0);
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }

        WM_NCHITTEST => {
            let def_hit = unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
            if def_hit.0 != HTCLIENT as isize {
                return def_hit;
            }
            let hit = query_window_for_hwnd(hwnd, |window| {
                if matches!(window.native.kind, WindowKind::Overlay) {
                    return HTCLIENT;
                }
                let mut rc = RECT::default();
                let _ = unsafe { GetWindowRect(hwnd, &mut rc) };
                let sx = (lparam.0 & 0xFFFF) as i16 as i32;
                let sy = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
                if window.native.is_resizable && !window.native.is_maximized() {
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
                let local = window.native.screen_to_client_point(lparam);
                match window.hit_test_control_area(local) {
                    Some(WindowControlArea::Close) => HTCLOSE,
                    Some(WindowControlArea::Max) => HTMAXBUTTON,
                    Some(WindowControlArea::Min) => HTMINBUTTON,
                    Some(WindowControlArea::Drag) => HTCAPTION,
                    None => HTCLIENT,
                }
            });
            LRESULT(hit.unwrap_or(HTCLIENT) as isize)
        }

        WM_NCMOUSEMOVE => {
            with_window_for_hwnd(hwnd, |window, cx| {
                if !window.native.tracking_nc_leave {
                    let mut tme = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE | TME_NONCLIENT,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    if unsafe { TrackMouseEvent(&mut tme) }.is_ok() {
                        window.native.tracking_nc_leave = true;
                    }
                }
                window.native.hovered = true;
                let pos = window.native.screen_to_client_point(lparam);
                window.dispatch_mouse_move(
                    MouseMoveEvent {
                        position: pos,
                        pressed_button: window.native.pressed_button,
                        modifiers: current_modifiers(),
                    },
                    cx,
                );
            });
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }

        WM_NCMOUSELEAVE => {
            with_window_for_hwnd(hwnd, |window, cx| {
                window.native.tracking_nc_leave = false;
                window.native.nc_button_down = None;
                // Moving from titlebar/caption buttons into the client area fires `WM_NCMOUSELEAVE`
                // while the pointer is still inside this window; only clear hover if truly outside.
                if !window.native.tracking_leave && !cursor_is_over_hwnd(hwnd) {
                    window.native.hovered = false;
                    window.on_mouse_leave(cx);
                }
            });
            LRESULT(0)
        }

        WM_NCLBUTTONDOWN => {
            let area = wparam.0 as u32;
            if matches!(area, HTMINBUTTON | HTMAXBUTTON | HTCLOSE | HTCAPTION) {
                with_window_for_hwnd(hwnd, |window, cx| {
                    if area != HTCAPTION {
                        window.native.nc_button_down = Some(area);
                    }
                    window.native.hovered = true;
                    let pos = window.native.screen_to_client_point(lparam);
                    window.dispatch_mouse_down(
                        MouseDownEvent {
                            button: MouseButton::Left,
                            position: pos,
                            modifiers: current_modifiers(),
                            click_count: 1,
                        },
                        cx,
                    );
                });
                if area != HTCAPTION {
                    return LRESULT(0);
                }
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }

        WM_NCLBUTTONUP => {
            let area = wparam.0 as u32;
            if matches!(area, HTMINBUTTON | HTMAXBUTTON | HTCLOSE) {
                let matched = with_window_for_hwnd(hwnd, |window, cx| {
                    let was = window.native.nc_button_down.take();
                    let pos = window.native.screen_to_client_point(lparam);
                    window.dispatch_mouse_up(
                        MouseUpEvent {
                            button: MouseButton::Left,
                            position: pos,
                            modifiers: current_modifiers(),
                            click_count: 1,
                        },
                        cx,
                    );
                    was == Some(area)
                })
                .unwrap_or(false);
                if matched {
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
                            PostMessageW(Some(hwnd), WM_SYSCOMMAND, WPARAM(cmd as usize), LPARAM(0))
                        };
                    }
                }
                return LRESULT(0);
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }

        WM_SETCURSOR if (lparam.0 & 0xFFFF) as u32 == HTCLIENT => {
            query_window_for_hwnd(hwnd, |window| {
                apply_cursor(window.native.cursor);
            });
            LRESULT(1)
        }

        WM_MOUSEMOVE => {
            with_window_for_hwnd(hwnd, |window, cx| {
                if !window.native.tracking_leave {
                    let mut tme = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    if unsafe { TrackMouseEvent(&mut tme) }.is_ok() {
                        window.native.tracking_leave = true;
                    }
                }
                window.native.hovered = true;
                let (_, _, pos) = window.native.client_point(lparam);
                window.dispatch_mouse_move(
                    MouseMoveEvent {
                        position: pos,
                        pressed_button: window.native.pressed_button,
                        modifiers: current_modifiers(),
                    },
                    cx,
                );
            });
            LRESULT(0)
        }

        WM_MOUSELEAVE => {
            with_window_for_hwnd(hwnd, |window, cx| {
                window.native.tracking_leave = false;
                // Moving from client area onto non-client caption/drag area fires `WM_MOUSELEAVE`
                // while the pointer is still inside this window; only clear hover if truly outside.
                if !window.native.tracking_nc_leave && !cursor_is_over_hwnd(hwnd) {
                    window.native.hovered = false;
                    window.on_mouse_leave(cx);
                }
            });
            LRESULT(0)
        }

        WM_CAPTURECHANGED => {
            with_window_for_hwnd(hwnd, |window, _| {
                window.native.pressed_button = None;
            });
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
            with_window_for_hwnd(hwnd, |window, cx| {
                window.native.pressed_button = Some(button);
                let (x, y, pos) = window.native.client_point(lparam);
                let click_count = window.native.record_click(button, x, y);
                window.dispatch_mouse_down(
                    MouseDownEvent {
                        button,
                        position: pos,
                        modifiers: current_modifiers(),
                        click_count,
                    },
                    cx,
                );
            });
            LRESULT(0)
        }

        WM_LBUTTONUP | WM_RBUTTONUP | WM_MBUTTONUP => {
            let button = match msg {
                WM_LBUTTONUP => MouseButton::Left,
                WM_RBUTTONUP => MouseButton::Right,
                _ => MouseButton::Middle,
            };
            let _ = unsafe { ReleaseCapture() };
            with_window_for_hwnd(hwnd, |window, cx| {
                if window.native.pressed_button == Some(button) {
                    window.native.pressed_button = None;
                }
                let (_, _, pos) = window.native.client_point(lparam);
                let click_count = window.native.click_count.max(1);
                window.dispatch_mouse_up(
                    MouseUpEvent {
                        button,
                        position: pos,
                        modifiers: current_modifiers(),
                        click_count,
                    },
                    cx,
                );
            });
            LRESULT(0)
        }

        WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
            let horizontal = msg == WM_MOUSEHWHEEL;
            let raw_delta = ((wparam.0 >> 16) & 0xFFFF) as i16 as f32;
            let notches = raw_delta / 120.0;
            let amount = crate::win32::wheel_scroll_amount(horizontal).unwrap_or(3) as f32;
            let modifiers = current_modifiers();
            let delta = if horizontal || modifiers.shift {
                ScrollDelta::Lines(point(if horizontal { -notches * amount } else { notches * amount }, 0.0))
            } else {
                ScrollDelta::Lines(point(0.0, notches * amount))
            };
            with_window_for_hwnd(hwnd, |window, cx| {
                let pos = window.native.screen_to_client_point(lparam);
                window.dispatch_scroll_wheel(
                    ScrollWheelEvent {
                        position: pos,
                        delta,
                        modifiers,
                    },
                    cx,
                );
            });
            LRESULT(0)
        }

        WM_KEYDOWN | WM_SYSKEYDOWN => {
            let vk = (wparam.0 & 0xFFFF) as u16;
            let modifiers = current_modifiers();
            let handled = with_window_for_hwnd(hwnd, |window, cx| {
                window.native.suppress_next_char = false;
                if modifiers != window.native.last_modifiers {
                    window.native.last_modifiers = modifiers;
                    window.dispatch_modifiers_changed(ModifiersChangedEvent { modifiers }, cx);
                }
                let Some(key) = translate_vk(vk, modifiers) else {
                    return false;
                };
                // Standalone Alt/F10 should be delivered cleanly without `alt` set on itself.
                let mut stroke_mods = modifiers;
                if key == "alt" {
                    stroke_mods.alt = false;
                }
                let is_held = (lparam.0 & (1 << 30)) != 0;
                if is_held && matches!(key.as_str(), "alt" | "f10") {
                    return true;
                }
                let keystroke = Keystroke {
                    modifiers: stroke_mods,
                    key,
                    key_char: None,
                };
                let handled = window.dispatch_key_down(KeyDownEvent { keystroke, is_held }, cx);
                if handled {
                    window.native.suppress_next_char = true;
                }
                handled
            })
            .unwrap_or(false);
            if handled {
                return LRESULT(0);
            }
            if msg == WM_SYSKEYDOWN && vk != VK_F4.0 && vk != VK_SPACE.0 {
                return LRESULT(0);
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }

        WM_SYSCHAR => LRESULT(0),

        WM_KEYUP | WM_SYSKEYUP => {
            let modifiers = current_modifiers();
            with_window_for_hwnd(hwnd, |window, cx| {
                window.native.suppress_next_char = false;
                if modifiers != window.native.last_modifiers {
                    window.native.last_modifiers = modifiers;
                    window.dispatch_modifiers_changed(ModifiersChangedEvent { modifiers }, cx);
                }
            });
            if msg == WM_SYSKEYUP {
                return LRESULT(0);
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }

        WM_CHAR => {
            let code = (wparam.0 & 0xFFFF) as u16;
            with_window_for_hwnd(hwnd, |window, cx| {
                if std::mem::take(&mut window.native.suppress_next_char) {
                    return;
                }
                if (0xD800..=0xDBFF).contains(&code) {
                    window.native.pending_surrogate = Some(code);
                    return;
                }
                let text = if (0xDC00..=0xDFFF).contains(&code) {
                    let Some(high) = window.native.pending_surrogate.take() else {
                        return;
                    };
                    String::from_utf16(&[high, code]).ok()
                } else {
                    window.native.pending_surrogate = None;
                    // Ignore ASCII control characters (Ctrl+letters, Backspace, Enter, Escape, Tab)
                    // which are handled as keybindings/actions.
                    if code < 0x20 || code == 0x7F {
                        return;
                    }
                    char::from_u32(u32::from(code)).map(|c| c.to_string())
                };
                if let Some(text) = text {
                    window.dispatch_text_input(&text, cx);
                }
            });
            LRESULT(0)
        }

        WM_IME_STARTCOMPOSITION => {
            with_window_for_hwnd(hwnd, |window, cx| {
                update_ime_candidate_pos(hwnd, window, cx);
            });
            LRESULT(0)
        }

        WM_IME_COMPOSITION => {
            let flags = lparam.0 as u32;
            with_window_for_hwnd(hwnd, |window, cx| {
                let himc = unsafe { ImmGetContext(hwnd) };
                if himc.is_invalid() {
                    return;
                }
                if (flags & GCS_RESULTSTR.0) != 0
                    && let Some(text) = read_ime_string(himc, GCS_RESULTSTR)
                {
                    window.dispatch_text_input(&text, cx);
                }
                if (flags & GCS_COMPSTR.0) != 0 {
                    let comp = read_ime_string(himc, GCS_COMPSTR).unwrap_or_default();
                    let cursor =
                        unsafe { ImmGetCompositionStringW(himc, GCS_CURSORPOS, None, 0) }.max(0)
                            as usize;
                    window.dispatch_ime_composition(&comp, cursor, cx);
                }
                let _ = unsafe { ImmReleaseContext(hwnd, himc) };
                update_ime_candidate_pos(hwnd, window, cx);
            });
            LRESULT(0)
        }

        WM_IME_ENDCOMPOSITION => {
            with_window_for_hwnd(hwnd, |window, cx| {
                window.dispatch_ime_composition("", 0, cx);
            });
            LRESULT(0)
        }

        WM_SIZE => {
            let w = (lparam.0 & 0xFFFF) as u32;
            let h = ((lparam.0 >> 16) & 0xFFFF) as u32;
            if w > 0 && h > 0 {
                with_window_for_hwnd(hwnd, |window, cx| {
                    let scale = window.native.scale_factor;
                    window.native.logical_size = size(px(w as f32 / scale), px(h as f32 / scale));
                    window.on_resize(w, h, scale, cx);
                });
            }
            LRESULT(0)
        }

        WM_MOVE => {
            with_window_for_hwnd(hwnd, |window, cx| {
                let (_, origin) = query_client_metrics(hwnd, window.native.scale_factor);
                window.native.origin = origin;
                window.notify_bounds_changed(cx);
            });
            LRESULT(0)
        }

        WM_DPICHANGED => {
            let new_dpi = (wparam.0 & 0xFFFF) as u32;
            let scale = new_dpi.max(USER_DEFAULT_SCREEN_DPI) as f32 / USER_DEFAULT_SCREEN_DPI as f32;
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
            with_window_for_hwnd(hwnd, |window, cx| {
                window.native.scale_factor = scale;
                let (w, h) = window.native.physical_size();
                window.native.logical_size = size(px(w as f32 / scale), px(h as f32 / scale));
                window.on_resize(w, h, scale, cx);
            });
            LRESULT(0)
        }

        WM_ACTIVATE => {
            let active = (wparam.0 & 0xFFFF) != 0;
            with_window_for_hwnd(hwnd, |window, cx| {
                window.native.active = active;
                if active {
                    cx.active_window = Some(window.window_id());
                } else if cx.active_window == Some(window.window_id()) {
                    cx.active_window = None;
                }
                window.notify_activation_changed(cx);
            });
            LRESULT(0)
        }

        WM_SETTINGCHANGE | WM_THEMECHANGED => {
            with_window_for_hwnd(hwnd, |window, cx| {
                let appearance = cx.window_appearance();
                window.apply_appearance(appearance, cx);
                crate::ui::reduce_motion::init(cx);
            });
            LRESULT(0)
        }

        WM_ENTERSIZEMOVE | WM_EXITSIZEMOVE => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },

        WM_CLOSE => {
            let should_close = with_window_for_hwnd(hwnd, |window, cx| window.check_should_close(cx))
                .unwrap_or(true);
            if should_close {
                let id = HWND_TO_WINDOW.with(|m| m.borrow().get(&(hwnd.0 as isize)).copied());
                if let Some(id) = id
                    && let Some(app_cell) = ACTIVE_APP.with(|s| s.borrow().clone())
                    && let Ok(mut app) = app_cell.inner.try_borrow_mut()
                {
                    app.remove_window_by_id(id);
                }
            }
            LRESULT(0)
        }

        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn read_ime_string(
    himc: windows::Win32::UI::Input::Ime::HIMC,
    flag: windows::Win32::UI::Input::Ime::IME_COMPOSITION_STRING,
) -> Option<String> {
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

fn update_ime_candidate_pos(
    hwnd: HWND,
    window: &mut crate::ui::window::Window,
    cx: &mut crate::ui::app::App,
) {
    let Some(rect) = window.query_ime_caret_bounds(cx) else {
        return;
    };
    let scale = window.native.scale_factor;
    let pt = POINT {
        x: (rect.origin.x.as_f32() * scale).round() as i32,
        y: (rect.bottom().as_f32() * scale).round() as i32,
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
    let _ = Cell::new(0);
}
