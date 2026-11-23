//! Clean-room Win32 window creation, Mica Alt framing, Snap Layouts hit-testing, consolidated
//! lock-free HWND registry, re-entrant Win32 event queue, modal size/move timer, and `wnd_proc`.

mod drop_target;
mod input;

pub(crate) use input::current_modifiers;

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use windows::Win32::{
    Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
    Graphics::{
        Dwm::{
            DWM_SYSTEMBACKDROP_TYPE, DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE,
            DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_TRANSITIONS_FORCEDISABLED,
            DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND,
            DwmExtendFrameIntoClientArea, DwmSetWindowAttribute,
        },
        Gdi::{ScreenToClient, ValidateRect},
    },
    System::{
        LibraryLoader::GetModuleHandleW,
        Ole::{IDropTarget, RegisterDragDrop, RevokeDragDrop},
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
        WindowsAndMessaging::{
            CS_DBLCLKS, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW,
            DestroyWindow, GetClientRect, GetCursorPos, GetWindowRect, HTCAPTION, HTCLIENT,
            HTCLOSE, HTMAXBUTTON, HTMINBUTTON, HTTOP, HTTOPLEFT, HTTOPRIGHT, IDC_ARROW, IDC_IBEAM,
            IsZoomed, KillTimer, LWA_ALPHA, LoadCursorW, LoadIconW, MINMAXINFO, NCCALCSIZE_PARAMS,
            PostMessageW, RegisterClassW, SC_CLOSE, SC_MAXIMIZE, SC_MINIMIZE, SC_RESTORE,
            SM_CXFRAME, SM_CXPADDEDBORDER, SM_CYFRAME, SW_SHOW, SW_SHOWNOACTIVATE,
            SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOZORDER, SetCursor, SetLayeredWindowAttributes,
            SetTimer, SetWindowPos, SetWindowTextW, ShowWindow, USER_DEFAULT_SCREEN_DPI,
            USER_TIMER_MINIMUM, WM_ACTIVATE, WM_CAPTURECHANGED, WM_CHAR, WM_CLOSE, WM_DPICHANGED,
            WM_ENTERMENULOOP, WM_ENTERSIZEMOVE, WM_ERASEBKGND, WM_EXITMENULOOP, WM_EXITSIZEMOVE,
            WM_GETMINMAXINFO, WM_IME_COMPOSITION, WM_IME_ENDCOMPOSITION, WM_IME_STARTCOMPOSITION,
            WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN,
            WM_MBUTTONUP, WM_MOUSEHWHEEL, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_MOVE, WM_NCCALCSIZE,
            WM_NCHITTEST, WM_NCLBUTTONDOWN, WM_NCLBUTTONUP, WM_NCMOUSELEAVE, WM_NCMOUSEMOVE,
            WM_PAINT, WM_RBUTTONDBLCLK, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SETCURSOR,
            WM_SETTINGCHANGE, WM_SIZE, WM_SYSCHAR, WM_SYSCOMMAND, WM_SYSKEYDOWN, WM_SYSKEYUP,
            WM_THEMECHANGED, WM_TIMER, WNDCLASSW, WS_EX_APPWINDOW, WS_EX_LAYERED,
            WS_EX_NOACTIVATE, WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
            WS_EX_TRANSPARENT, WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_POPUP, WS_SYSMENU, WS_THICKFRAME,
            WindowFromPoint,
        },
    },
};
use windows::core::{BOOL, HSTRING, PCWSTR, w};

use crate::win32::WindowHwnd;

use super::{
    ACTIVE_APP, CursorStyle, ExternalPaths, WindowAppearance, WindowBackgroundAppearance,
    WindowControlArea, WindowKind, WindowOptions, WindowsDisplay, drive_frames,
};
use crate::ui::types::UiResult;
use crate::ui::app::AppCell;
use crate::ui::geometry::{Bounds, Pixels, Point, Size, point, px, size};
use crate::ui::keymap::{Keystroke, Modifiers};
use crate::ui::window::{
    KeyDownEvent, ModifiersChangedEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ScrollDelta, ScrollWheelEvent, WindowId,
};

use drop_target::JotDropTarget;
use input::{
    ClickTracker, is_syskey_passthrough, read_ime_string, translate_vk, update_ime_candidate_pos,
};

const WINDOW_CLASS: PCWSTR = w!("JotD2DWindow");
const DWMSBT_TABBEDWINDOW: i32 = 4;
const SIZE_MOVE_TIMER_ID: usize = 1;

#[derive(Copy, Clone, Debug)]
struct HwndEntry {
    window_id: WindowId,
    is_overlay: bool,
    hide_titlebar: bool,
    min_size: Option<Size<Pixels>>,
}

/// State-changing Win32 notifications queued when `window_wnd_proc` is entered synchronously while
/// `AppCell` is mutably borrowed (e.g., inside `SetWindowPos` or `ShowWindow`).
pub(super) enum PendingWin32Event {
    Resized {
        hwnd: isize,
        width: u32,
        height: u32,
    },
    DpiChanged {
        hwnd: isize,
        scale: f32,
    },
    Activated {
        hwnd: isize,
        active: bool,
    },
    ThemeChanged {
        hwnd: isize,
    },
    DroppedFiles {
        window_id: WindowId,
        paths: ExternalPaths,
        client_px: (i32, i32),
    },
}

thread_local! {
    /// Single lock-free per-HWND metadata registry so `WM_NCCALCSIZE`, `WM_GETMINMAXINFO`, and
    /// HWND → `WindowId` resolution stay consistent and clean up atomically on window destruction.
    static HWND_REGISTRY: RefCell<HashMap<isize, HwndEntry>> = RefCell::new(HashMap::new());
    static PENDING_WIN32_EVENTS: RefCell<VecDeque<PendingWin32Event>> =
        const { RefCell::new(VecDeque::new()) };
}

pub(super) fn queue_pending_win32_event(event: PendingWin32Event) {
    PENDING_WIN32_EVENTS.with(|q| q.borrow_mut().push_back(event));
}

pub(crate) fn set_window_min_size(hwnd: WindowHwnd, min_size: Option<Size<Pixels>>) {
    HWND_REGISTRY.with(|map| {
        if let Some(entry) = map.borrow_mut().get_mut(&(hwnd.raw().0 as isize)) {
            entry.min_size = min_size;
        }
    });
}

fn hwnd_entry(hwnd: HWND) -> Option<HwndEntry> {
    HWND_REGISTRY.with(|map| map.borrow().get(&(hwnd.0 as isize)).copied())
}

#[cfg(test)]
pub(crate) fn has_hwnd_entry(hwnd: WindowHwnd) -> bool {
    hwnd_entry(hwnd.raw()).is_some()
}

pub(super) fn drain_pending_win32_events(app_cell: &Rc<AppCell>) {
    loop {
        let next = PENDING_WIN32_EVENTS.with(|q| q.borrow_mut().pop_front());
        let Some(event) = next else {
            break;
        };
        let Ok(mut app) = app_cell.inner.try_borrow_mut() else {
            PENDING_WIN32_EVENTS.with(|q| q.borrow_mut().push_front(event));
            return;
        };
        match event {
            PendingWin32Event::Resized {
                hwnd,
                width,
                height,
            } => {
                if let Some(entry) = hwnd_entry(HWND(hwnd as *mut _)) {
                    let _ = app.update_window_impl(entry.window_id, |_, window, cx| {
                        let scale = window.native.scale_factor;
                        window.native.logical_size =
                            size(px(width as f32 / scale), px(height as f32 / scale));
                        window.on_resize(width, height, scale, cx);
                    });
                }
            }
            PendingWin32Event::DpiChanged { hwnd, scale } => {
                if let Some(entry) = hwnd_entry(HWND(hwnd as *mut _)) {
                    let _ = app.update_window_impl(entry.window_id, |_, window, cx| {
                        window.native.scale_factor = scale;
                        let (w, h) = window.native.physical_size();
                        window.native.logical_size =
                            size(px(w as f32 / scale), px(h as f32 / scale));
                        window.on_resize(w, h, scale, cx);
                    });
                }
            }
            PendingWin32Event::Activated { hwnd, active } => {
                if let Some(entry) = hwnd_entry(HWND(hwnd as *mut _)) {
                    let _ = app.update_window_impl(entry.window_id, |_, window, cx| {
                        window.native.active = active;
                        if active {
                            cx.active_window = Some(window.window_id());
                        } else if cx.active_window == Some(window.window_id()) {
                            cx.active_window = None;
                        }
                        window.notify_activation_changed(cx);
                    });
                }
            }
            PendingWin32Event::ThemeChanged { hwnd } => {
                if let Some(entry) = hwnd_entry(HWND(hwnd as *mut _)) {
                    let _ = app.update_window_impl(entry.window_id, |_, window, cx| {
                        let appearance = cx.window_appearance();
                        window.apply_appearance(appearance, cx);
                        cx.sync_system_reduce_motion();
                    });
                }
            }
            PendingWin32Event::DroppedFiles {
                window_id,
                paths,
                client_px,
            } => {
                let _ = app.update_window_impl(window_id, |_, window, cx| {
                    let pos = point(
                        px(client_px.0 as f32 / window.native.scale_factor),
                        px(client_px.1 as f32 / window.native.scale_factor),
                    );
                    window.dispatch_file_drop(paths, pos, cx);
                });
            }
        }
        app.flush_effects();
    }
}

pub(crate) fn register_window_class() -> UiResult<()> {
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
    pub(crate) in_size_move: bool,
    pub(crate) cursor: CursorStyle,
    pub(crate) nc_button_down: Option<u32>,
    pub(crate) pending_surrogate: Option<u16>,
    pub(crate) suppress_next_char: bool,
    pub(crate) last_modifiers: Modifiers,
    clicks: ClickTracker,
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
    ) -> UiResult<Self> {
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
        HWND_REGISTRY.with(|map| {
            map.borrow_mut().insert(
                hwnd.0 as isize,
                HwndEntry {
                    window_id,
                    is_overlay,
                    hide_titlebar,
                    min_size: if is_overlay {
                        None
                    } else {
                        options.window_min_size
                    },
                },
            );
        });

        let target_scale = options
            .display_id
            .and_then(WindowsDisplay::from_id)
            .map_or(dpi as f32 / USER_DEFAULT_SCREEN_DPI as f32, |d| {
                d.scale_factor()
            });

        if is_overlay {
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
        } else {
            configure_backdrop(hwnd, options.window_background);
            configure_dark_mode(hwnd, appearance);
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
            let target: IDropTarget = JotDropTarget { window_id, hwnd }.into();
            unsafe { RegisterDragDrop(hwnd, &target) }.is_ok()
        } else {
            false
        };

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
            in_size_move: false,
            cursor: CursorStyle::Arrow,
            nc_button_down: None,
            pending_surrogate: None,
            suppress_next_char: false,
            last_modifiers: current_modifiers(),
            clicks: ClickTracker::default(),
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
        HWND_REGISTRY.with(|map| {
            map.borrow_mut().remove(&(self.hwnd.0 as isize));
        });
        if self.in_size_move {
            let _ = unsafe { KillTimer(Some(self.hwnd), SIZE_MOVE_TIMER_ID) };
            self.in_size_move = false;
        }
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

    fn client_point(&self, lparam: LPARAM) -> (i32, i32, Point<Pixels>) {
        let x = (lparam.0 & 0xFFFF) as i16 as i32;
        let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
        (
            x,
            y,
            point(
                px(x as f32 / self.scale_factor),
                px(y as f32 / self.scale_factor),
            ),
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

// ---------------------------------------------------------------------------
// Main Win32 window procedure
// ---------------------------------------------------------------------------

fn with_window_for_hwnd<R>(
    hwnd: HWND,
    f: impl FnOnce(&mut crate::ui::window::Window, &mut crate::ui::app::App) -> R,
) -> Option<R> {
    let window_id = hwnd_entry(hwnd)?.window_id;
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
    let window_id = hwnd_entry(hwnd)?.window_id;
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
            if let Some(entry) = hwnd_entry(hwnd)
                && let Some(app_cell) = ACTIVE_APP.with(|s| s.borrow().clone())
                && let Ok(mut app) = app_cell.inner.try_borrow_mut()
            {
                let _ = app.update_window_impl(entry.window_id, |root, window, cx| {
                    window.draw_frame(&root, cx);
                });
            }
            LRESULT(0)
        }

        WM_NCCALCSIZE if wparam.0 != 0 => {
            let (is_overlay, hide_titlebar) =
                hwnd_entry(hwnd).map_or((false, true), |e| (e.is_overlay, e.hide_titlebar));
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
            if let Some(min_size) = hwnd_entry(hwnd).and_then(|e| e.min_size) {
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
                if window.native.in_size_move {
                    return;
                }
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
                if window.native.in_size_move {
                    return;
                }
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
                let click_count = window.native.clicks.record(button, x, y);
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
                let click_count = window.native.clicks.last_count();
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
                ScrollDelta::Lines(point(
                    if horizontal {
                        -notches * amount
                    } else {
                        notches * amount
                    },
                    0.0,
                ))
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
            if msg == WM_SYSKEYDOWN && !is_syskey_passthrough(vk) {
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
                let delivered = with_window_for_hwnd(hwnd, |window, cx| {
                    let scale = window.native.scale_factor;
                    window.native.logical_size = size(px(w as f32 / scale), px(h as f32 / scale));
                    window.on_resize(w, h, scale, cx);
                });
                if delivered.is_none() && hwnd_entry(hwnd).is_some() {
                    queue_pending_win32_event(PendingWin32Event::Resized {
                        hwnd: hwnd.0 as isize,
                        width: w,
                        height: h,
                    });
                }
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
            let scale =
                new_dpi.max(USER_DEFAULT_SCREEN_DPI) as f32 / USER_DEFAULT_SCREEN_DPI as f32;
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
            let delivered = with_window_for_hwnd(hwnd, |window, cx| {
                window.native.scale_factor = scale;
                let (w, h) = window.native.physical_size();
                window.native.logical_size = size(px(w as f32 / scale), px(h as f32 / scale));
                window.on_resize(w, h, scale, cx);
            });
            if delivered.is_none() && hwnd_entry(hwnd).is_some() {
                queue_pending_win32_event(PendingWin32Event::DpiChanged {
                    hwnd: hwnd.0 as isize,
                    scale,
                });
            }
            LRESULT(0)
        }

        WM_ACTIVATE => {
            let active = (wparam.0 & 0xFFFF) != 0;
            let delivered = with_window_for_hwnd(hwnd, |window, cx| {
                window.native.active = active;
                if active {
                    cx.active_window = Some(window.window_id());
                } else if cx.active_window == Some(window.window_id()) {
                    cx.active_window = None;
                }
                window.notify_activation_changed(cx);
            });
            if delivered.is_none() && hwnd_entry(hwnd).is_some() {
                queue_pending_win32_event(PendingWin32Event::Activated {
                    hwnd: hwnd.0 as isize,
                    active,
                });
            }
            LRESULT(0)
        }

        WM_SETTINGCHANGE | WM_THEMECHANGED => {
            let delivered = with_window_for_hwnd(hwnd, |window, cx| {
                let appearance = cx.window_appearance();
                window.apply_appearance(appearance, cx);
                cx.sync_system_reduce_motion();
            });
            if delivered.is_none() && hwnd_entry(hwnd).is_some() {
                queue_pending_win32_event(PendingWin32Event::ThemeChanged {
                    hwnd: hwnd.0 as isize,
                });
            }
            LRESULT(0)
        }

        WM_ENTERSIZEMOVE | WM_ENTERMENULOOP => {
            with_window_for_hwnd(hwnd, |window, cx| {
                window.native.in_size_move = true;
                if window.native.hovered {
                    window.on_mouse_leave(cx);
                }
            });
            unsafe {
                let _ = SetTimer(Some(hwnd), SIZE_MOVE_TIMER_ID, USER_TIMER_MINIMUM, None);
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
        }

        WM_TIMER if wparam.0 == SIZE_MOVE_TIMER_ID => {
            if let Some(app_cell) = ACTIVE_APP.with(|s| s.borrow().clone()) {
                if let Ok(mut app) = app_cell.inner.try_borrow_mut() {
                    app.drain_jobs();
                    app.flush_effects();
                }
                drive_frames(&app_cell);
            }
            LRESULT(0)
        }

        WM_EXITSIZEMOVE | WM_EXITMENULOOP => {
            let _ = unsafe { KillTimer(Some(hwnd), SIZE_MOVE_TIMER_ID) };
            with_window_for_hwnd(hwnd, |window, _| {
                window.native.in_size_move = false;
                window.mark_dirty();
            });
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }

        WM_CLOSE => {
            let should_close = with_window_for_hwnd(hwnd, |window, cx| window.check_should_close(cx))
                .unwrap_or(true);
            if should_close
                && let Some(entry) = hwnd_entry(hwnd)
                && let Some(app_cell) = ACTIVE_APP.with(|s| s.borrow().clone())
                && let Ok(mut app) = app_cell.inner.try_borrow_mut()
            {
                app.remove_window_by_id(entry.window_id);
            }
            LRESULT(0)
        }

        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}
