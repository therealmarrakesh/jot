//! Win32 window creation, Mica Alt framing, Snap Layouts hit-testing, lock-free HWND metadata
//! registry, re-entrant event queue, modal size/move repaint timer, and `wnd_proc`.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

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

use crate::geom::{Point, Rect, Size, point, size};
use crate::gfx::{D2DRenderer, Direct2DDevices};
use crate::types::{Result, SharedStr};

use super::drop_target::JotDropTarget;
use super::input::{
    ClickTracker, Key, KeyEvent, Modifiers, MouseButton, current_modifiers, is_syskey_passthrough,
    read_ime_string, set_ime_candidate_pos, translate_vk,
};
use super::os::{WindowHwnd, wheel_scroll_amount};
use super::{ACTIVE_HANDLER, DisplayId, WindowsDisplay};

const WINDOW_CLASS: PCWSTR = w!("JotD2DWindow");
const DWMSBT_TABBEDWINDOW: i32 = 4;
const SIZE_MOVE_TIMER_ID: usize = 1;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct WindowId(pub(crate) u64);

impl WindowId {
    pub(crate) fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum WindowAppearance {
    #[default]
    Light,
    Dark,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum WindowBackdrop {
    #[default]
    Opaque,
    Transparent,
    MicaAlt,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum WindowKind {
    #[default]
    Normal,
    Overlay,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum WindowControlArea {
    Drag,
    Min,
    Max,
    Close,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum CursorStyle {
    #[default]
    Arrow,
    IBeam,
}

#[derive(Clone, Debug)]
pub(crate) struct WindowOptions {
    pub(crate) bounds: Option<Rect>,
    pub(crate) title: SharedStr,
    pub(crate) hide_titlebar: bool,
    pub(crate) focus: bool,
    pub(crate) show: bool,
    pub(crate) kind: WindowKind,
    pub(crate) is_resizable: bool,
    pub(crate) is_minimizable: bool,
    pub(crate) display_id: Option<DisplayId>,
    pub(crate) backdrop: WindowBackdrop,
    pub(crate) min_size: Option<Size>,
}

impl Default for WindowOptions {
    fn default() -> Self {
        Self {
            bounds: None,
            title: SharedStr::new_static("Jot"),
            hide_titlebar: true,
            focus: true,
            show: true,
            kind: WindowKind::Normal,
            is_resizable: true,
            is_minimizable: true,
            display_id: None,
            backdrop: WindowBackdrop::MicaAlt,
            min_size: None,
        }
    }
}

/// Typed Win32 events delivered synchronously (or queued if re-entrant) to `PlatformHandler`.
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
        origin: Point,
    },
    DpiChanged {
        window_id: WindowId,
        physical_width: u32,
        physical_height: u32,
        scale: f32,
    },
    Activated {
        window_id: WindowId,
        active: bool,
    },
    ThemeChanged,
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
        modifiers: Modifiers,
    },
    KeyDown {
        window_id: WindowId,
        event: KeyEvent,
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
    SizeMoveChanged {
        window_id: WindowId,
        entering: bool,
    },
    CloseRequested {
        window_id: WindowId,
    },
}

/// Implemented by `App` to handle Win32 events, wake signals, painting, and frame driving.
pub(crate) trait PlatformHandler {
    /// Handles a Win32 event. Returning `true` suppresses follow-up `WM_CHAR` for `KeyDown`.
    fn on_event(&mut self, event: PlatformEvent) -> bool;
    /// Returns the logical caret rect and DPI scale for IME candidate window placement.
    fn query_ime_caret(&self, window_id: WindowId) -> Option<(Rect, f32)>;
    /// Drains ready timers, background jobs, and cross-thread posts.
    fn on_wake(&mut self);
    /// Renders a specific window in response to `WM_PAINT`.
    fn on_paint(&mut self, window_id: WindowId);
    /// Renders dirty windows and arms vsync if animations are running.
    fn drive_frames(&mut self);
    /// Returns `true` when the message loop should exit.
    fn should_quit(&self) -> bool;
}

/// Per-HWND cached metadata & Win32 input tracking stored lock-free on the UI thread.
///
/// Keeping `min_size`, `cursor`, `scale_factor`, and `control_areas` here allows `WM_NCCALCSIZE`,
/// `WM_GETMINMAXINFO`, `WM_NCHITTEST`, and `WM_SETCURSOR` to answer Win32 synchronously without
/// ever borrowing `App`.
struct HwndEntry {
    window_id: WindowId,
    is_overlay: bool,
    hide_titlebar: bool,
    is_resizable: bool,
    scale_factor: f32,
    min_size: Option<Size>,
    cursor: CursorStyle,
    control_areas: Vec<(Rect, WindowControlArea)>,
    in_size_move: bool,
    hovered: bool,
    nc_button_down: Option<u32>,
    pending_surrogate: Option<u16>,
    suppress_next_char: bool,
    last_modifiers: Modifiers,
    clicks: ClickTracker,
    pressed_button: Option<MouseButton>,
    tracking_leave: bool,
    tracking_nc_leave: bool,
}

thread_local! {
    static HWND_REGISTRY: RefCell<HashMap<isize, HwndEntry>> = RefCell::new(HashMap::new());
    static PENDING_EVENTS: RefCell<VecDeque<PlatformEvent>> = const { RefCell::new(VecDeque::new()) };
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

fn with_entry<R>(hwnd: HWND, f: impl FnOnce(&HwndEntry) -> R) -> Option<R> {
    HWND_REGISTRY.with(|map| map.borrow().get(&(hwnd.0 as isize)).map(f))
}

fn with_entry_mut<R>(hwnd: HWND, f: impl FnOnce(&mut HwndEntry) -> R) -> Option<R> {
    HWND_REGISTRY.with(|map| map.borrow_mut().get_mut(&(hwnd.0 as isize)).map(f))
}

#[cfg(test)]
pub(crate) fn has_hwnd_entry(hwnd: WindowHwnd) -> bool {
    with_entry(hwnd.raw(), |_| ()).is_some()
}

pub(super) fn queue_pending_event(event: PlatformEvent) {
    PENDING_EVENTS.with(|q| q.borrow_mut().push_back(event));
}

pub(super) fn drain_pending_events() {
    let Some(handler_rc) = ACTIVE_HANDLER.with(|slot| slot.borrow().clone()) else {
        return;
    };
    loop {
        let next = PENDING_EVENTS.with(|q| q.borrow_mut().pop_front());
        let Some(event) = next else {
            break;
        };
        let Ok(mut handler) = handler_rc.try_borrow_mut() else {
            PENDING_EVENTS.with(|q| q.borrow_mut().push_front(event));
            return;
        };
        handler.on_event(event);
    }
}

/// Dispatches `event` synchronously if `ACTIVE_HANDLER` is free, or queues it for immediate drain
/// if `wnd_proc` was re-entered during a Win32 call made inside the handler.
pub(super) fn dispatch_or_queue_event(event: PlatformEvent) -> bool {
    let Some(handler_rc) = ACTIVE_HANDLER.with(|slot| slot.borrow().clone()) else {
        return false;
    };
    let handled = match handler_rc.try_borrow_mut() {
        Ok(mut handler) => handler.on_event(event),
        Err(_) => {
            queue_pending_event(event);
            return false;
        }
    };
    drain_pending_events();
    if let Ok(mut handler) = handler_rc.try_borrow_mut() {
        handler.drive_frames();
    }
    handled
}

/// Dispatches an event that should only run when `ACTIVE_HANDLER` is free (not queued on re-entry).
fn dispatch_if_free(event: PlatformEvent) -> bool {
    let Some(handler_rc) = ACTIVE_HANDLER.with(|slot| slot.borrow().clone()) else {
        return false;
    };
    let handled = match handler_rc.try_borrow_mut() {
        Ok(mut handler) => handler.on_event(event),
        Err(_) => return false,
    };
    drain_pending_events();
    if let Ok(mut handler) = handler_rc.try_borrow_mut() {
        handler.drive_frames();
    }
    handled
}

/// Owned Win32 window + Direct2D swapchain renderer.
pub(crate) struct NativeWindow {
    pub(crate) id: WindowId,
    hwnd: HWND,
    pub(crate) kind: WindowKind,
    pub(crate) scale_factor: f32,
    pub(crate) logical_size: Size,
    pub(crate) origin: Point,
    pub(crate) active: bool,
    pub(crate) in_size_move: bool,
    pub(crate) renderer: D2DRenderer,
    drop_registered: bool,
    destroyed: bool,
}

impl NativeWindow {
    pub(crate) fn create(
        devices: Direct2DDevices,
        options: &WindowOptions,
        appearance: WindowAppearance,
    ) -> Result<Self> {
        let id = WindowId::next();
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

        let wide_title = HSTRING::from(options.title.as_str());
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
        let init_scale = dpi as f32 / USER_DEFAULT_SCREEN_DPI as f32;
        HWND_REGISTRY.with(|map| {
            map.borrow_mut().insert(
                hwnd.0 as isize,
                HwndEntry {
                    window_id: id,
                    is_overlay,
                    hide_titlebar: options.hide_titlebar,
                    is_resizable: options.is_resizable && !is_overlay,
                    scale_factor: init_scale,
                    min_size: if is_overlay { None } else { options.min_size },
                    cursor: CursorStyle::Arrow,
                    control_areas: Vec::new(),
                    in_size_move: false,
                    hovered: false,
                    nc_button_down: None,
                    pending_surrogate: None,
                    suppress_next_char: false,
                    last_modifiers: current_modifiers(),
                    clicks: ClickTracker::default(),
                    pressed_button: None,
                    tracking_leave: false,
                    tracking_nc_leave: false,
                },
            );
        });

        let target_scale = options
            .display_id
            .and_then(WindowsDisplay::from_id)
            .map_or(init_scale, |d| d.scale_factor());

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
            configure_backdrop(hwnd, options.backdrop);
            configure_dark_mode(hwnd, appearance);
        }

        if let Some(b) = options.bounds {
            let mut x = (b.origin.x * target_scale).round() as i32;
            let y = (b.origin.y * target_scale).round() as i32;
            let mut w = (b.size.width * target_scale).round().max(1.0) as i32;
            let mut h = (b.size.height * target_scale).round().max(1.0) as i32;
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
        with_entry_mut(hwnd, |e| e.scale_factor = scale_factor);

        let drop_registered = if !is_overlay {
            let target: IDropTarget = JotDropTarget { window_id: id }.into();
            unsafe { RegisterDragDrop(hwnd, &target) }.is_ok()
        } else {
            false
        };

        let (phys_w, phys_h) = query_physical_size(hwnd);
        let (logical_size, origin) = query_client_metrics(hwnd, scale_factor);
        let renderer = match D2DRenderer::new(devices, hwnd, phys_w, phys_h, scale_factor) {
            Ok(r) => r,
            Err(err) => {
                HWND_REGISTRY.with(|map| {
                    map.borrow_mut().remove(&(hwnd.0 as isize));
                });
                if drop_registered {
                    let _ = unsafe { RevokeDragDrop(hwnd) };
                }
                let _ = unsafe { DestroyWindow(hwnd) };
                return Err(err);
            }
        };

        let win = Self {
            id,
            hwnd,
            kind: options.kind,
            scale_factor,
            logical_size,
            origin,
            active: options.show && options.focus,
            in_size_move: false,
            renderer,
            drop_registered,
            destroyed: false,
        };
        if options.show {
            win.show(options.focus);
        }
        Ok(win)
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
        if matches!(self.kind, WindowKind::Normal) {
            configure_dark_mode(self.hwnd, appearance);
        }
    }

    pub(crate) fn set_cursor(&self, style: CursorStyle) {
        let changed = with_entry_mut(self.hwnd, |e| {
            let diff = e.cursor != style;
            e.cursor = style;
            diff
        })
        .unwrap_or(false);
        if changed {
            apply_cursor(style);
        }
    }

    pub(crate) fn set_min_size(&self, min_size: Option<Size>) {
        with_entry_mut(self.hwnd, |e| e.min_size = min_size);
    }

    pub(crate) fn set_control_areas(&self, areas: Vec<(Rect, WindowControlArea)>) {
        with_entry_mut(self.hwnd, |e| e.control_areas = areas);
    }

    pub(crate) fn is_maximized(&self) -> bool {
        unsafe { IsZoomed(self.hwnd) }.as_bool()
    }

    pub(crate) fn hwnd(&self) -> WindowHwnd {
        WindowHwnd::from_raw(self.hwnd)
    }

    pub(crate) fn sync_size(&mut self, physical_width: u32, physical_height: u32, scale: f32) {
        self.scale_factor = scale;
        self.logical_size = size(
            physical_width as f32 / scale,
            physical_height as f32 / scale,
        );
        with_entry_mut(self.hwnd, |e| e.scale_factor = scale);
        let _ = self
            .renderer
            .resize(physical_width, physical_height, scale);
    }

    /// Queries the live Win32 client rect and DPI (essential for overlay windows resized via
    /// `SetWindowPos` before `WM_SIZE` drains). Returns `false` if the window is minimized/empty.
    pub(crate) fn sync_live_metrics(&mut self) -> bool {
        let mut rc = RECT::default();
        if unsafe { GetClientRect(self.hwnd, &mut rc) }.is_err() {
            return false;
        }
        let w = rc.right - rc.left;
        let h = rc.bottom - rc.top;
        if w <= 0 || h <= 0 {
            return false;
        }
        let dpi = unsafe { GetDpiForWindow(self.hwnd) }.max(USER_DEFAULT_SCREEN_DPI);
        let scale = dpi as f32 / USER_DEFAULT_SCREEN_DPI as f32;
        self.sync_size(w as u32, h as u32, scale);
        true
    }

    pub(crate) fn recreate_renderer(&mut self, devices: Direct2DDevices) -> Result<()> {
        let (w, h) = query_physical_size(self.hwnd);
        self.renderer = D2DRenderer::new(devices, self.hwnd, w, h, self.scale_factor)?;
        Ok(())
    }

    pub(crate) fn destroy(&mut self) {
        if self.destroyed {
            return;
        }
        self.destroyed = true;
        let in_size_move = HWND_REGISTRY.with(|map| {
            map.borrow_mut()
                .remove(&(self.hwnd.0 as isize))
                .is_some_and(|e| e.in_size_move)
        });
        if in_size_move {
            let _ = unsafe { KillTimer(Some(self.hwnd), SIZE_MOVE_TIMER_ID) };
        }
        if self.drop_registered {
            let _ = unsafe { RevokeDragDrop(self.hwnd) };
            self.drop_registered = false;
        }
        let _ = unsafe { DestroyWindow(self.hwnd) };
    }
}

impl Drop for NativeWindow {
    fn drop(&mut self) {
        self.destroy();
    }
}

fn query_physical_size(hwnd: HWND) -> (u32, u32) {
    let mut rc = RECT::default();
    let _ = unsafe { GetClientRect(hwnd, &mut rc) };
    (
        (rc.right - rc.left).max(1) as u32,
        (rc.bottom - rc.top).max(1) as u32,
    )
}

fn query_client_metrics(hwnd: HWND, scale: f32) -> (Size, Point) {
    let mut client = RECT::default();
    let _ = unsafe { GetClientRect(hwnd, &mut client) };
    let mut win_rc = RECT::default();
    let _ = unsafe { GetWindowRect(hwnd, &mut win_rc) };
    (
        size(
            (client.right - client.left).max(1) as f32 / scale,
            (client.bottom - client.top).max(1) as f32 / scale,
        ),
        point(win_rc.left as f32 / scale, win_rc.top as f32 / scale),
    )
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
    let pad = unsafe { GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi) };
    let bx = unsafe { GetSystemMetricsForDpi(SM_CXFRAME, dpi) } + pad;
    let by = unsafe { GetSystemMetricsForDpi(SM_CYFRAME, dpi) } + pad;
    (bx, by)
}

fn configure_backdrop(hwnd: HWND, backdrop: WindowBackdrop) {
    let margins = MARGINS {
        cxLeftWidth: -1,
        cxRightWidth: -1,
        cyTopHeight: -1,
        cyBottomHeight: -1,
    };
    let _ = unsafe { DwmExtendFrameIntoClientArea(hwnd, &margins) };
    if matches!(backdrop, WindowBackdrop::MicaAlt) {
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

fn cursor_is_over_hwnd(hwnd: HWND) -> bool {
    let mut pt = POINT::default();
    unsafe { GetCursorPos(&mut pt).is_ok() && WindowFromPoint(pt) == hwnd }
}

fn update_ime_pos(hwnd: HWND, window_id: WindowId) {
    let Some(handler_rc) = ACTIVE_HANDLER.with(|s| s.borrow().clone()) else {
        return;
    };
    let caret = handler_rc
        .try_borrow()
        .ok()
        .and_then(|h| h.query_ime_caret(window_id));
    if let Some((rect, scale)) = caret {
        set_ime_candidate_pos(hwnd, rect, scale);
    }
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
            if let Some(window_id) = with_entry(hwnd, |e| e.window_id)
                && let Some(handler_rc) = ACTIVE_HANDLER.with(|s| s.borrow().clone())
                && let Ok(mut handler) = handler_rc.try_borrow_mut()
            {
                handler.on_paint(window_id);
            }
            LRESULT(0)
        }

        WM_NCCALCSIZE if wparam.0 != 0 => {
            let (is_overlay, hide_titlebar) =
                with_entry(hwnd, |e| (e.is_overlay, e.hide_titlebar)).unwrap_or((false, true));
            if is_overlay {
                return LRESULT(0);
            }
            if !hide_titlebar {
                return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
            }
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
            if let Some(min_size) = with_entry(hwnd, |e| e.min_size).flatten() {
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
            let hit = with_entry(hwnd, |e| {
                if e.is_overlay {
                    return HTCLIENT;
                }
                let mut rc = RECT::default();
                let _ = unsafe { GetWindowRect(hwnd, &mut rc) };
                let sx = (lparam.0 & 0xFFFF) as i16 as i32;
                let sy = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
                if e.is_resizable && !unsafe { IsZoomed(hwnd) }.as_bool() {
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
                let local = screen_to_client_point(hwnd, lparam, e.scale_factor);
                let area = e
                    .control_areas
                    .iter()
                    .rev()
                    .find(|(rect, _)| rect.contains(local))
                    .map(|(_, a)| *a);
                match area {
                    Some(WindowControlArea::Close) => HTCLOSE,
                    Some(WindowControlArea::Max) => HTMAXBUTTON,
                    Some(WindowControlArea::Min) => HTMINBUTTON,
                    Some(WindowControlArea::Drag) => HTCAPTION,
                    None => HTCLIENT,
                }
            });
            LRESULT(hit.unwrap_or(HTCLIENT) as isize)
        }

        WM_SETCURSOR if (lparam.0 & 0xFFFF) as u32 == HTCLIENT => {
            if let Some(cursor) = with_entry(hwnd, |e| e.cursor) {
                apply_cursor(cursor);
            }
            LRESULT(1)
        }

        WM_NCMOUSEMOVE => {
            let info = with_entry_mut(hwnd, |e| {
                if e.in_size_move {
                    return None;
                }
                let need_track = !e.tracking_nc_leave;
                e.hovered = true;
                let position = screen_to_client_point(hwnd, lparam, e.scale_factor);
                Some((
                    need_track,
                    PlatformEvent::MouseMove {
                        window_id: e.window_id,
                        position,
                        pressed_button: e.pressed_button,
                    },
                ))
            })
            .flatten();
            if let Some((need_track, ev)) = info {
                if need_track {
                    let mut tme = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE | TME_NONCLIENT,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    if unsafe { TrackMouseEvent(&mut tme) }.is_ok() {
                        with_entry_mut(hwnd, |e| e.tracking_nc_leave = true);
                    }
                }
                dispatch_if_free(ev);
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }

        WM_NCMOUSELEAVE => {
            let check_leave = with_entry_mut(hwnd, |e| {
                e.tracking_nc_leave = false;
                e.nc_button_down = None;
                !e.tracking_leave
            })
            .unwrap_or(false);
            if check_leave && !cursor_is_over_hwnd(hwnd) {
                let window_id = with_entry_mut(hwnd, |e| {
                    e.hovered = false;
                    e.window_id
                });
                if let Some(window_id) = window_id {
                    dispatch_if_free(PlatformEvent::MouseLeave { window_id });
                }
            }
            LRESULT(0)
        }

        WM_NCLBUTTONDOWN => {
            let area = wparam.0 as u32;
            if matches!(area, HTMINBUTTON | HTMAXBUTTON | HTCLOSE | HTCAPTION) {
                let ev = with_entry_mut(hwnd, |e| {
                    if area != HTCAPTION {
                        e.nc_button_down = Some(area);
                    }
                    e.hovered = true;
                    let position = screen_to_client_point(hwnd, lparam, e.scale_factor);
                    PlatformEvent::MouseDown {
                        window_id: e.window_id,
                        button: MouseButton::Left,
                        position,
                        modifiers: current_modifiers(),
                        click_count: 1,
                    }
                });
                if let Some(ev) = ev {
                    dispatch_if_free(ev);
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
                let info = with_entry_mut(hwnd, |e| {
                    let was = e.nc_button_down.take();
                    let position = screen_to_client_point(hwnd, lparam, e.scale_factor);
                    (
                        PlatformEvent::MouseUp {
                            window_id: e.window_id,
                            button: MouseButton::Left,
                            position,
                        },
                        was == Some(area),
                    )
                });
                if let Some((ev, matched)) = info {
                    dispatch_if_free(ev);
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
            let w_bits = wparam.0 as u32;
            let live_button = if (w_bits & 0x0001) != 0 {
                Some(MouseButton::Left)
            } else if (w_bits & 0x0002) != 0 {
                Some(MouseButton::Right)
            } else if (w_bits & 0x0010) != 0 {
                Some(MouseButton::Middle)
            } else {
                None
            };
            let info = with_entry_mut(hwnd, |e| {
                if e.in_size_move {
                    return None;
                }
                let need_track = !e.tracking_leave;
                e.hovered = true;
                e.pressed_button = live_button;
                let (_, _, position) = client_point_from_lparam(lparam, e.scale_factor);
                Some((
                    need_track,
                    PlatformEvent::MouseMove {
                        window_id: e.window_id,
                        position,
                        pressed_button: live_button,
                    },
                ))
            })
            .flatten();
            if let Some((need_track, ev)) = info {
                if need_track {
                    let mut tme = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    if unsafe { TrackMouseEvent(&mut tme) }.is_ok() {
                        with_entry_mut(hwnd, |e| e.tracking_leave = true);
                    }
                }
                dispatch_if_free(ev);
            }
            LRESULT(0)
        }

        WM_MOUSELEAVE => {
            let check_leave = with_entry_mut(hwnd, |e| {
                e.tracking_leave = false;
                !e.tracking_nc_leave
            })
            .unwrap_or(false);
            if check_leave && !cursor_is_over_hwnd(hwnd) {
                let window_id = with_entry_mut(hwnd, |e| {
                    e.hovered = false;
                    e.window_id
                });
                if let Some(window_id) = window_id {
                    dispatch_if_free(PlatformEvent::MouseLeave { window_id });
                }
            }
            LRESULT(0)
        }

        WM_CAPTURECHANGED => {
            with_entry_mut(hwnd, |e| e.pressed_button = None);
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
            let ev = with_entry_mut(hwnd, |e| {
                e.pressed_button = Some(button);
                let (x, y, position) = client_point_from_lparam(lparam, e.scale_factor);
                let click_count = e.clicks.record(button, x, y);
                PlatformEvent::MouseDown {
                    window_id: e.window_id,
                    button,
                    position,
                    modifiers: current_modifiers(),
                    click_count,
                }
            });
            if let Some(ev) = ev {
                dispatch_if_free(ev);
            }
            LRESULT(0)
        }

        WM_LBUTTONUP | WM_RBUTTONUP | WM_MBUTTONUP => {
            let button = match msg {
                WM_LBUTTONUP => MouseButton::Left,
                WM_RBUTTONUP => MouseButton::Right,
                _ => MouseButton::Middle,
            };
            let _ = unsafe { ReleaseCapture() };
            let ev = with_entry_mut(hwnd, |e| {
                if e.pressed_button == Some(button) {
                    e.pressed_button = None;
                }
                let (_, _, position) = client_point_from_lparam(lparam, e.scale_factor);
                PlatformEvent::MouseUp {
                    window_id: e.window_id,
                    button,
                    position,
                }
            });
            if let Some(ev) = ev {
                dispatch_if_free(ev);
            }
            LRESULT(0)
        }

        WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
            let horizontal = msg == WM_MOUSEHWHEEL;
            let raw_delta = ((wparam.0 >> 16) & 0xFFFF) as i16 as f32;
            let notches = raw_delta / 120.0;
            let amount = wheel_scroll_amount(horizontal).unwrap_or(3) as f32;
            let modifiers = current_modifiers();
            let delta_lines = if horizontal || modifiers.shift {
                point(
                    if horizontal {
                        -notches * amount
                    } else {
                        notches * amount
                    },
                    0.0,
                )
            } else {
                point(0.0, notches * amount)
            };
            let ev = with_entry(hwnd, |e| {
                let position = screen_to_client_point(hwnd, lparam, e.scale_factor);
                PlatformEvent::ScrollWheel {
                    window_id: e.window_id,
                    position,
                    delta_lines,
                    modifiers,
                }
            });
            if let Some(ev) = ev {
                dispatch_if_free(ev);
            }
            LRESULT(0)
        }

        WM_KEYDOWN | WM_SYSKEYDOWN => {
            let vk = (wparam.0 & 0xFFFF) as u16;
            let modifiers = current_modifiers();
            let is_held = (lparam.0 & (1 << 30)) != 0;

            let prep = with_entry_mut(hwnd, |e| {
                e.suppress_next_char = false;
                let mods_changed = (modifiers != e.last_modifiers).then(|| {
                    e.last_modifiers = modifiers;
                    PlatformEvent::ModifiersChanged { modifiers }
                });
                (e.window_id, mods_changed)
            });

            let mut handled = false;
            if let Some((window_id, mods_changed)) = prep {
                if let Some(ev) = mods_changed {
                    dispatch_if_free(ev);
                }
                if let Some(key) = translate_vk(vk, modifiers) {
                    if is_held && matches!(key, Key::Alt | Key::F(10)) {
                        handled = true;
                    } else {
                        let mut stroke_mods = modifiers;
                        if key == Key::Alt {
                            stroke_mods.alt = false;
                        }
                        handled = dispatch_if_free(PlatformEvent::KeyDown {
                            window_id,
                            event: KeyEvent {
                                key,
                                modifiers: stroke_mods,
                            },
                        });
                        if handled {
                            with_entry_mut(hwnd, |e| e.suppress_next_char = true);
                        }
                    }
                }
            }
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
            let mods_changed = with_entry_mut(hwnd, |e| {
                e.suppress_next_char = false;
                (modifiers != e.last_modifiers).then(|| {
                    e.last_modifiers = modifiers;
                    PlatformEvent::ModifiersChanged { modifiers }
                })
            })
            .flatten();
            if let Some(ev) = mods_changed {
                dispatch_if_free(ev);
            }
            if msg == WM_SYSKEYUP {
                return LRESULT(0);
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }

        WM_CHAR => {
            let code = (wparam.0 & 0xFFFF) as u16;
            let text_ev = with_entry_mut(hwnd, |e| {
                if std::mem::take(&mut e.suppress_next_char) {
                    return None;
                }
                if (0xD800..=0xDBFF).contains(&code) {
                    e.pending_surrogate = Some(code);
                    return None;
                }
                let text = if (0xDC00..=0xDFFF).contains(&code) {
                    let high = e.pending_surrogate.take()?;
                    String::from_utf16(&[high, code]).ok()?
                } else {
                    e.pending_surrogate = None;
                    if code < 0x20 || code == 0x7F {
                        return None;
                    }
                    char::from_u32(u32::from(code))?.to_string()
                };
                Some(PlatformEvent::TextInput {
                    window_id: e.window_id,
                    text,
                })
            })
            .flatten();
            if let Some(ev) = text_ev {
                dispatch_if_free(ev);
            }
            LRESULT(0)
        }

        WM_IME_STARTCOMPOSITION => {
            if let Some(window_id) = with_entry(hwnd, |e| e.window_id) {
                update_ime_pos(hwnd, window_id);
            }
            LRESULT(0)
        }

        WM_IME_COMPOSITION => {
            let flags = lparam.0 as u32;
            if let Some(window_id) = with_entry(hwnd, |e| e.window_id) {
                let himc = unsafe { ImmGetContext(hwnd) };
                if !himc.is_invalid() {
                    if (flags & GCS_RESULTSTR.0) != 0
                        && let Some(text) = read_ime_string(himc, GCS_RESULTSTR)
                    {
                        dispatch_if_free(PlatformEvent::TextInput { window_id, text });
                    }
                    if (flags & GCS_COMPSTR.0) != 0 {
                        let text = read_ime_string(himc, GCS_COMPSTR).unwrap_or_default();
                        let cursor = unsafe {
                            ImmGetCompositionStringW(himc, GCS_CURSORPOS, None, 0)
                        }
                        .max(0) as usize;
                        dispatch_if_free(PlatformEvent::ImeComposition {
                            window_id,
                            text,
                            cursor,
                        });
                    }
                    let _ = unsafe { ImmReleaseContext(hwnd, himc) };
                    update_ime_pos(hwnd, window_id);
                }
            }
            LRESULT(0)
        }

        WM_IME_ENDCOMPOSITION => {
            if let Some(window_id) = with_entry(hwnd, |e| e.window_id) {
                dispatch_if_free(PlatformEvent::ImeComposition {
                    window_id,
                    text: String::new(),
                    cursor: 0,
                });
            }
            LRESULT(0)
        }

        WM_SIZE => {
            let w = (lparam.0 & 0xFFFF) as u32;
            let h = ((lparam.0 >> 16) & 0xFFFF) as u32;
            if w > 0 && h > 0
                && let Some((window_id, scale)) = with_entry(hwnd, |e| (e.window_id, e.scale_factor))
            {
                dispatch_or_queue_event(PlatformEvent::Resized {
                    window_id,
                    physical_width: w,
                    physical_height: h,
                    scale,
                });
            }
            LRESULT(0)
        }

        WM_MOVE => {
            if let Some((window_id, scale)) = with_entry(hwnd, |e| (e.window_id, e.scale_factor)) {
                let (_, origin) = query_client_metrics(hwnd, scale);
                dispatch_if_free(PlatformEvent::Moved { window_id, origin });
            }
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
            if let Some(window_id) = with_entry_mut(hwnd, |e| {
                e.scale_factor = scale;
                e.window_id
            }) {
                let (w, h) = query_physical_size(hwnd);
                dispatch_or_queue_event(PlatformEvent::DpiChanged {
                    window_id,
                    physical_width: w,
                    physical_height: h,
                    scale,
                });
            }
            LRESULT(0)
        }

        WM_ACTIVATE => {
            let active = (wparam.0 & 0xFFFF) != 0;
            if let Some(window_id) = with_entry(hwnd, |e| e.window_id) {
                dispatch_or_queue_event(PlatformEvent::Activated { window_id, active });
            }
            LRESULT(0)
        }

        WM_SETTINGCHANGE | WM_THEMECHANGED => {
            if with_entry(hwnd, |_| ()).is_some() {
                dispatch_or_queue_event(PlatformEvent::ThemeChanged);
            }
            LRESULT(0)
        }

        WM_ENTERSIZEMOVE | WM_ENTERMENULOOP => {
            if let Some((window_id, was_hovered)) = with_entry_mut(hwnd, |e| {
                e.in_size_move = true;
                let h = e.hovered;
                e.hovered = false;
                (e.window_id, h)
            }) {
                if was_hovered {
                    dispatch_if_free(PlatformEvent::MouseLeave { window_id });
                }
                dispatch_if_free(PlatformEvent::SizeMoveChanged {
                    window_id,
                    entering: true,
                });
            }
            unsafe {
                let _ = SetTimer(Some(hwnd), SIZE_MOVE_TIMER_ID, USER_TIMER_MINIMUM, None);
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
        }

        WM_TIMER if wparam.0 == SIZE_MOVE_TIMER_ID => {
            if let Some(handler_rc) = ACTIVE_HANDLER.with(|s| s.borrow().clone()) {
                if let Ok(mut handler) = handler_rc.try_borrow_mut() {
                    handler.on_wake();
                }
                drain_pending_events();
                if let Ok(mut handler) = handler_rc.try_borrow_mut() {
                    handler.drive_frames();
                }
            }
            LRESULT(0)
        }

        WM_EXITSIZEMOVE | WM_EXITMENULOOP => {
            let _ = unsafe { KillTimer(Some(hwnd), SIZE_MOVE_TIMER_ID) };
            if let Some(window_id) = with_entry_mut(hwnd, |e| {
                e.in_size_move = false;
                e.window_id
            }) {
                dispatch_if_free(PlatformEvent::SizeMoveChanged {
                    window_id,
                    entering: false,
                });
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }

        WM_CLOSE => {
            if let Some(window_id) = with_entry(hwnd, |e| e.window_id) {
                dispatch_if_free(PlatformEvent::CloseRequested { window_id });
            }
            LRESULT(0)
        }

        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}
