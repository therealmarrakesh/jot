use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
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
use crate::types::{Result, SharedStr};

use super::input::{
    ClickTracker, Key, KeyEvent, Modifiers, MouseButton, current_modifiers, is_syskey_passthrough,
    read_ime_string, set_ime_candidate_pos, translate_vk,
};
use super::os::WindowHandle;
use super::settings::SystemSettings;
use super::{ACTIVE_HANDLER, DisplayId, WindowsDisplay};

const WINDOW_CLASS: PCWSTR = w!("JotWindow");
const OVERLAY_CLASS: PCWSTR = w!("JotOverlay");
const DWMSBT_TABBEDWINDOW: i32 = 4;
const SIZE_MOVE_TIMER_ID: usize = 1;

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

struct WindowShared {
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

thread_local! {
    static HWND_REGISTRY: RefCell<HashMap<isize, Rc<WindowShared>>> = RefCell::new(HashMap::new());
    static PENDING_EVENTS: RefCell<VecDeque<PlatformEvent>> = const { RefCell::new(VecDeque::new()) };
    static SHUTDOWN_BLOCK: Cell<Option<&'static str>> = const { Cell::new(None) };
}

pub(super) fn set_block_shutdown(reason: Option<&'static str>) {
    let prev = SHUTDOWN_BLOCK.with(|slot| slot.replace(reason));
    if prev.is_some() && reason.is_none() {
        HWND_REGISTRY.with(|map| {
            for &raw in map.borrow().keys() {
                let _ = unsafe { ShutdownBlockReasonDestroy(HWND(raw as *mut _)) };
            }
        });
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

pub(crate) fn register_window_class() -> Result<()> {
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

fn lookup(hwnd: HWND) -> Option<Rc<WindowShared>> {
    HWND_REGISTRY.with(|map| map.borrow().get(&(hwnd.0 as isize)).cloned())
}

#[cfg(test)]
pub(crate) fn has_hwnd_entry(hwnd: WindowHandle) -> bool {
    lookup(hwnd.raw()).is_some()
}

fn release_capture_suppressed(hwnd: HWND, release: impl FnOnce()) {
    let shared = lookup(hwnd);
    if let Some(s) = &shared {
        s.input.borrow_mut().releasing_capture = true;
    }
    release();
    if let Some(s) = &shared {
        s.input.borrow_mut().releasing_capture = false;
    }
}

#[cfg(test)]
pub(crate) fn simulate_release_capture_callback(hwnd: WindowHandle) {
    release_capture_suppressed(hwnd.raw(), || unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::SendMessageW(
            hwnd.raw(),
            WM_CAPTURECHANGED,
            Some(WPARAM(0)),
            Some(LPARAM(0)),
        );
    });
}

fn queue_pending_event(event: PlatformEvent) {
    PENDING_EVENTS.with(|q| {
        let mut queue = q.borrow_mut();
        let coalesced = match (&event, queue.back_mut()) {
            (PlatformEvent::Moved { window_id: a }, Some(PlatformEvent::Moved { window_id: b }))
                if a == b =>
            {
                true
            }
            (
                PlatformEvent::Resized {
                    window_id: a,
                    physical_width: nw,
                    physical_height: nh,
                    scale: ns,
                },
                Some(PlatformEvent::Resized {
                    window_id: b,
                    physical_width,
                    physical_height,
                    scale,
                }),
            ) if a == b => {
                *physical_width = *nw;
                *physical_height = *nh;
                *scale = *ns;
                true
            }
            (
                PlatformEvent::SettingsChanged(fresh),
                Some(PlatformEvent::SettingsChanged(prev)),
            ) => {
                *prev = *fresh;
                true
            }
            _ => false,
        };
        if !coalesced {
            queue.push_back(event);
        }
    });
}

fn drain_pending_only() {
    let Some(handler_rc) = ACTIVE_HANDLER.with(|slot| slot.borrow().clone()) else {
        return;
    };
    loop {
        if super::take_wake_pending() {
            match handler_rc.try_borrow_mut() {
                Ok(mut handler) => handler.on_wake(),
                Err(_) => {
                    super::mark_wake_pending();
                    return;
                }
            }
        }
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

pub(super) fn service() {
    let Some(handler_rc) = ACTIVE_HANDLER.with(|slot| slot.borrow().clone()) else {
        return;
    };
    drain_pending_only();
    if let Ok(mut handler) = handler_rc.try_borrow_mut() {
        handler.drive_frames();
    }
    drain_pending_only();
}

pub(super) fn dispatch(event: PlatformEvent) -> bool {
    let Some(handler_rc) = ACTIVE_HANDLER.with(|slot| slot.borrow().clone()) else {
        return false;
    };
    drain_pending_only();
    let handled = match handler_rc.try_borrow_mut() {
        Ok(mut handler) => handler.on_event(event),
        Err(_) => {
            queue_pending_event(event);
            return false;
        }
    };
    service();
    handled
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
        HWND_REGISTRY.with(|map| {
            map.borrow_mut().insert(hwnd.0 as isize, shared.clone());
        });

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
    pub(crate) fn in_size_move(&self) -> bool {
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
        let in_size_move = HWND_REGISTRY.with(|map| {
            map.borrow_mut()
                .remove(&(self.hwnd.0 as isize))
                .is_some_and(|s| s.in_size_move.get())
        });
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
    if let Some(s) = lookup(hwnd)
        && let Some(rect) = s.ime_caret.get()
    {
        set_ime_candidate_pos(hwnd, rect, s.scale());
    }
}

fn handle_mouse_move(hwnd: HWND, lparam: LPARAM, live_buttons: Option<MouseButtons>) {
    let Some(s) = lookup(hwnd) else {
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
    let Some(s) = lookup(hwnd) else {
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
    if let Some(s) = lookup(hwnd) {
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
    super::diag::ffi_guard("window_wnd_proc", LRESULT(0), || {
        window_wnd_proc_inner(hwnd, msg, wparam, lparam)
    })
}

fn window_wnd_proc_inner(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_ERASEBKGND => LRESULT(1),

        WM_PAINT => {
            let _ = unsafe { ValidateRect(Some(hwnd), None) };
            if let Some(s) = lookup(hwnd) {
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
            if let Some(s) = lookup(hwnd)
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
            let hit = lookup(hwnd).map(|s| {
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
            if let Some(s) = lookup(hwnd) {
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
                if let Some(s) = lookup(hwnd) {
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
                if let Some(s) = lookup(hwnd) {
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
            if let Some(s) = lookup(hwnd) {
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
            if let Some(s) = lookup(hwnd) {
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
            let (release_cap, ev) = if let Some(s) = lookup(hwnd) {
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
            let raw_amount = super::cached_settings().wheel_scroll_amount(horizontal);
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
            if let Some(s) = lookup(hwnd) {
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
            if let Some(s) = lookup(hwnd) {
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
            if let Some(s) = lookup(hwnd) {
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
            if let Some(s) = lookup(hwnd) {
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
                && let Some(s) = lookup(hwnd)
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
            if let Some(s) = lookup(hwnd)
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
            if let Some(s) = lookup(hwnd) {
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
            if let Some(s) = lookup(hwnd) {
                s.active.set(active);
                dispatch(PlatformEvent::Activated {
                    window_id: s.id,
                    active,
                });
            }
            LRESULT(0)
        }

        WM_SETTINGCHANGE | WM_THEMECHANGED => {
            if lookup(hwnd).is_some()
                && (msg == WM_THEMECHANGED || is_relevant_setting_change(lparam))
            {
                let fresh = SystemSettings::query();
                super::set_cached_settings(fresh);
                dispatch(PlatformEvent::SettingsChanged(fresh));
            }
            LRESULT(0)
        }

        WM_ENTERSIZEMOVE | WM_ENTERMENULOOP => {
            if let Some(s) = lookup(hwnd) {
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
            super::mark_wake_pending();
            service();
            LRESULT(0)
        }

        WM_EXITSIZEMOVE | WM_EXITMENULOOP => {
            let _ = unsafe { KillTimer(Some(hwnd), SIZE_MOVE_TIMER_ID) };
            if let Some(s) = lookup(hwnd) {
                s.in_size_move.set(false);
                dispatch(PlatformEvent::SizeMoveChanged {
                    window_id: s.id,
                    entering: false,
                });
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }

        WM_QUERYENDSESSION => {
            if let Some(reason) = SHUTDOWN_BLOCK.with(Cell::get) {
                let wide = HSTRING::from(reason);
                let _ = unsafe { ShutdownBlockReasonCreate(hwnd, &wide) };
                if let Some(s) = lookup(hwnd) {
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
                && let Some(s) = lookup(hwnd)
            {
                dispatch(PlatformEvent::FilesDropped {
                    window_id: s.id,
                    paths,
                });
            }
            LRESULT(0)
        }

        WM_CLOSE => {
            if let Some(s) = lookup(hwnd) {
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
