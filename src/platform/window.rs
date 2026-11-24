use std::cell::{Cell, RefCell};
use std::rc::Rc;

use windows::Win32::{
    Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM},
    Graphics::Dwm::{
        DWM_SYSTEMBACKDROP_TYPE, DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE, DWMWA_SYSTEMBACKDROP_TYPE,
        DWMWA_TRANSITIONS_FORCEDISABLED, DWMWA_USE_IMMERSIVE_DARK_MODE,
        DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND, DWMWINDOWATTRIBUTE,
        DwmExtendFrameIntoClientArea, DwmSetWindowAttribute,
    },
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Controls::MARGINS,
        HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi},
        Shell::DragAcceptFiles,
        WindowsAndMessaging::{
            CS_DBLCLKS, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, CreateCaret, CreateWindowExW,
            DefWindowProcW, DestroyCaret, DestroyWindow, GetClientRect, IDC_ARROW, IDC_IBEAM,
            IsZoomed, KillTimer, LWA_ALPHA, LoadCursorW, LoadIconW, RegisterClassW, SM_CXFRAME,
            SM_CXPADDEDBORDER, SM_CYFRAME, SW_SHOW, SW_SHOWNOACTIVATE, SWP_FRAMECHANGED,
            SWP_NOACTIVATE, SWP_NOZORDER, SetCaretPos, SetCursor, SetLayeredWindowAttributes,
            SetWindowPos, SetWindowTextW, ShowWindow, USER_DEFAULT_SCREEN_DPI, WNDCLASSW,
            WS_EX_APPWINDOW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_NOREDIRECTIONBITMAP,
            WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_MAXIMIZEBOX, WS_MINIMIZEBOX,
            WS_POPUP, WS_SYSMENU, WS_THICKFRAME,
        },
    },
};
use windows::core::{BOOL, HSTRING, PCWSTR, w};

use crate::geom::{Rect, Size, size};
use crate::types::Result;

use super::display::WindowsDisplay;
use super::event::{
    CursorStyle, MainWindowConfig, WindowAppearance, WindowControlArea, WindowId,
};
use super::handle::WindowHandle;
use super::input::{ClickTracker, Modifiers, MouseButton, current_modifiers};
use super::pump;
use super::wndproc::window_wnd_proc;

const WINDOW_CLASS: PCWSTR = w!("JotWindow");
const OVERLAY_CLASS: PCWSTR = w!("JotOverlay");
const DWMSBT_TABBEDWINDOW: i32 = 4;
pub(super) const SIZE_MOVE_TIMER_ID: usize = 1;

#[derive(Default, Copy, Clone)]
pub(super) struct MouseButtons(u8);

impl MouseButtons {
    fn bit(button: MouseButton) -> u8 {
        match button {
            MouseButton::Left => 0x1,
            MouseButton::Right => 0x2,
            MouseButton::Middle => 0x4,
        }
    }

    pub(super) fn press(&mut self, button: MouseButton) {
        self.0 |= Self::bit(button);
    }

    pub(super) fn release(&mut self, button: MouseButton) {
        self.0 &= !Self::bit(button);
    }

    pub(super) fn clear(&mut self) {
        self.0 = 0;
    }

    pub(super) fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub(super) fn from_wparam(wparam: WPARAM) -> Self {
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

    pub(super) fn primary(self) -> Option<MouseButton> {
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
pub(super) struct WindowMetrics {
    pub(super) scale: f32,
    pub(super) physical: (u32, u32),
}

pub(super) struct WindowChrome {
    pub(super) min_size: Option<Size>,
    pub(super) cursor: CursorStyle,
    pub(super) control_areas: Vec<(Rect, WindowControlArea)>,
}

pub(super) struct WindowInput {
    pub(super) hovered: bool,
    pub(super) nc_button_down: Option<u32>,
    pub(super) pending_surrogate: Option<u16>,
    pub(super) suppress_next_char: bool,
    pub(super) last_modifiers: Modifiers,
    pub(super) clicks: ClickTracker,
    pub(super) buttons: MouseButtons,
    pub(super) releasing_capture: bool,
    pub(super) tracking_leave: bool,
    pub(super) tracking_nc_leave: bool,
}

pub(super) struct WindowShared {
    pub(super) id: WindowId,
    pub(super) metrics: Cell<WindowMetrics>,
    pub(super) active: Cell<bool>,
    pub(super) in_size_move: Cell<bool>,
    pub(super) ime_caret: Cell<Option<Rect>>,
    sys_caret_px: Cell<Option<(i32, i32, i32, i32)>>,
    pub(super) last_origin_px: Cell<Option<(i32, i32)>>,
    pub(super) chrome: RefCell<WindowChrome>,
    pub(super) input: RefCell<WindowInput>,
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
    pub(super) fn scale(&self) -> f32 {
        self.metrics.get().scale
    }

    pub(super) fn set_metrics(&self, w: u32, h: u32, scale: f32) {
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

pub(super) fn query_scale(hwnd: HWND) -> f32 {
    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(USER_DEFAULT_SCREEN_DPI);
    dpi as f32 / USER_DEFAULT_SCREEN_DPI as f32
}

pub(super) fn query_physical_size(hwnd: HWND) -> (u32, u32) {
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

pub(super) fn frame_thickness_for_dpi(dpi: u32) -> (i32, i32) {
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

pub(super) fn apply_cursor(style: CursorStyle) {
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
