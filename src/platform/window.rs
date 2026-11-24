use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;
use std::rc::Rc;
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
use super::instance::HandoffCommand;
use super::os::WindowHwnd;
use super::settings::{refresh_system_settings, system_settings};
use super::{ACTIVE_HANDLER, DisplayId, WindowsDisplay};

const WINDOW_CLASS: PCWSTR = w!("JotWindow");
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

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum WindowControlArea {
    Drag,
    Min,
    Max,
    Close,
}

pub(crate) fn hit_test_control_areas(
    areas: &[(Rect, Option<WindowControlArea>)],
    pos: Point,
) -> Option<WindowControlArea> {
    areas
        .iter()
        .rev()
        .find(|(rect, _)| rect.contains(pos))
        .and_then(|(_, area)| *area)
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
    Handoff(HandoffCommand),
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

struct WindowState {
    scale_factor: Cell<f32>,
    physical_size: Cell<(u32, u32)>,
    logical_size: Cell<Size>,
    origin: Cell<Point>,
    active: Cell<bool>,
    in_size_move: Cell<bool>,
    ime_caret: Cell<Option<Rect>>,
    sys_caret_px: Cell<Option<(i32, i32, i32, i32)>>,
    block_shutdown: Cell<Option<&'static str>>,
}

impl WindowState {
    fn set_metrics(&self, w: u32, h: u32, scale: f32) {
        self.scale_factor.set(scale);
        self.physical_size.set((w, h));
        self.logical_size
            .set(size(w as f32 / scale, h as f32 / scale));
    }
}

struct HwndEntry {
    window_id: WindowId,
    state: Rc<WindowState>,
    is_overlay: bool,
    min_size: Option<Size>,
    cursor: CursorStyle,
    control_areas: Vec<(Rect, Option<WindowControlArea>)>,
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

thread_local! {
    static HWND_REGISTRY: RefCell<HashMap<isize, HwndEntry>> = RefCell::new(HashMap::new());
    static PENDING_EVENTS: RefCell<VecDeque<PlatformEvent>> = const { RefCell::new(VecDeque::new()) };
}

pub(crate) fn register_window_class() -> Result<()> {
    unsafe {
        let hinstance: HINSTANCE = GetModuleHandleW(None)?.into();
        let hicon = LoadIconW(Some(hinstance), PCWSTR(std::ptr::without_provenance(1)))
            .unwrap_or_default();
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW | CS_DBLCLKS,
            lpfnWndProc: Some(window_wnd_proc),
            hInstance: hinstance,
            hIcon: hicon,
            lpszClassName: WINDOW_CLASS,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            ..Default::default()
        };
        RegisterClassW(&wc);
    }
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

fn release_capture_suppressed(hwnd: HWND, release: impl FnOnce()) {
    with_entry_mut(hwnd, |e| e.releasing_capture = true);
    release();
    with_entry_mut(hwnd, |e| e.releasing_capture = false);
}

#[cfg(test)]
pub(crate) fn simulate_release_capture_callback(hwnd: WindowHwnd) {
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
            (PlatformEvent::ThemeChanged, Some(PlatformEvent::ThemeChanged)) => true,
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
    is_overlay: bool,
    state: Rc<WindowState>,
    destroyed: bool,
}

impl NativeWindow {
    pub(crate) fn create_main(
        config: &MainWindowConfig,
        appearance: WindowAppearance,
    ) -> Result<Self> {
        let ex_style = WS_EX_APPWINDOW | WS_EX_NOREDIRECTIONBITMAP;
        let style = WS_SYSMENU | WS_THICKFRAME | WS_MAXIMIZEBOX | WS_MINIMIZEBOX;
        let (id, hwnd, state, init_scale) = Self::create_and_register(
            ex_style,
            style,
            config.title.as_str(),
            (CW_USEDEFAULT, CW_USEDEFAULT, CW_USEDEFAULT, CW_USEDEFAULT),
            false,
            config.show && config.focus,
        )?;

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
        let win = Self::finish_create(id, hwnd, false, state);
        if config.show {
            win.show(config.focus);
        }
        Ok(win)
    }

    pub(crate) fn create_overlay() -> Result<Self> {
        let ex_style = WS_EX_NOACTIVATE
            | WS_EX_TOOLWINDOW
            | WS_EX_TOPMOST
            | WS_EX_TRANSPARENT
            | WS_EX_LAYERED
            | WS_EX_NOREDIRECTIONBITMAP;
        let (id, hwnd, state, _) =
            Self::create_and_register(ex_style, WS_POPUP, "", (0, 0, 1, 1), true, false)?;

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

        Ok(Self::finish_create(id, hwnd, true, state))
    }

    fn create_and_register(
        ex_style: windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE,
        style: windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE,
        title: &str,
        (x, y, w, h): (i32, i32, i32, i32),
        is_overlay: bool,
        active: bool,
    ) -> Result<(WindowId, HWND, Rc<WindowState>, f32)> {
        let id = WindowId::next();
        let hinstance: HINSTANCE = unsafe { GetModuleHandleW(None)? }.into();
        let wide_title = HSTRING::from(title);
        let hwnd = unsafe {
            CreateWindowExW(
                ex_style,
                WINDOW_CLASS,
                &wide_title,
                style,
                x,
                y,
                w,
                h,
                None,
                None,
                Some(hinstance),
                None,
            )?
        };

        let dpi = unsafe { GetDpiForWindow(hwnd) }.max(USER_DEFAULT_SCREEN_DPI);
        let init_scale = dpi as f32 / USER_DEFAULT_SCREEN_DPI as f32;
        let state = Rc::new(WindowState {
            scale_factor: Cell::new(init_scale),
            physical_size: Cell::new((1, 1)),
            logical_size: Cell::new(Size::ZERO),
            origin: Cell::new(Point::ZERO),
            active: Cell::new(active),
            in_size_move: Cell::new(false),
            ime_caret: Cell::new(None),
            sys_caret_px: Cell::new(None),
            block_shutdown: Cell::new(None),
        });
        HWND_REGISTRY.with(|map| {
            map.borrow_mut().insert(
                hwnd.0 as isize,
                HwndEntry {
                    window_id: id,
                    state: state.clone(),
                    is_overlay,
                    min_size: None,
                    cursor: CursorStyle::Arrow,
                    control_areas: Vec::new(),
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
                },
            );
        });
        Ok((id, hwnd, state, init_scale))
    }

    fn finish_create(id: WindowId, hwnd: HWND, is_overlay: bool, state: Rc<WindowState>) -> Self {
        let final_dpi = unsafe { GetDpiForWindow(hwnd) }.max(USER_DEFAULT_SCREEN_DPI);
        let scale_factor = final_dpi as f32 / USER_DEFAULT_SCREEN_DPI as f32;
        let (phys_w, phys_h) = query_physical_size(hwnd);
        let (_, origin) = query_client_metrics(hwnd, scale_factor);
        state.set_metrics(phys_w, phys_h, scale_factor);
        state.origin.set(origin);
        Self {
            id,
            hwnd,
            is_overlay,
            state,
            destroyed: false,
        }
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
        if !self.is_overlay {
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

    pub(crate) fn set_control_areas(&self, areas: Vec<(Rect, Option<WindowControlArea>)>) {
        with_entry_mut(self.hwnd, |e| e.control_areas = areas);
    }

    pub(crate) fn is_maximized(&self) -> bool {
        unsafe { IsZoomed(self.hwnd) }.as_bool()
    }

    pub(crate) fn hwnd(&self) -> WindowHwnd {
        WindowHwnd::from_raw(self.hwnd)
    }

    #[inline]
    pub(crate) fn scale_factor(&self) -> f32 {
        self.state.scale_factor.get()
    }

    #[inline]
    pub(crate) fn physical_size(&self) -> (u32, u32) {
        self.state.physical_size.get()
    }

    #[inline]
    pub(crate) fn logical_size(&self) -> Size {
        self.state.logical_size.get()
    }

    #[inline]
    pub(crate) fn is_active(&self) -> bool {
        self.state.active.get()
    }

    #[cfg(test)]
    #[inline]
    pub(crate) fn in_size_move(&self) -> bool {
        self.state.in_size_move.get()
    }

    pub(crate) fn set_text_caret(&self, ime_rect: Option<Rect>, show_system_caret: bool) {
        self.state.ime_caret.set(ime_rect);
        let scale = self.state.scale_factor.get();
        let next_px = show_system_caret.then_some(ime_rect).flatten().map(|r| {
            (
                (r.origin.x * scale).round() as i32,
                (r.origin.y * scale).round() as i32,
                (r.size.width * scale).round().max(1.0) as i32,
                (r.size.height * scale).round().max(1.0) as i32,
            )
        });
        if next_px == self.state.sys_caret_px.get() {
            return;
        }
        self.state.sys_caret_px.set(next_px);
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

    pub(crate) fn set_block_shutdown(&self, reason: Option<&'static str>) {
        let prev = self.state.block_shutdown.replace(reason);
        if prev.is_some() && reason.is_none() {
            let _ = unsafe { ShutdownBlockReasonDestroy(self.hwnd) };
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
        let dpi = unsafe { GetDpiForWindow(self.hwnd) }.max(USER_DEFAULT_SCREEN_DPI);
        let scale = dpi as f32 / USER_DEFAULT_SCREEN_DPI as f32;
        let (pw, ph) = (w as u32, h as u32);
        self.state.set_metrics(pw, ph, scale);
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
                .is_some_and(|e| e.state.in_size_move.get())
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
    let mut win_rc = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut client);
        let _ = GetWindowRect(hwnd, &mut win_rc);
    }
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
    if let Some((Some(rect), scale)) =
        with_entry(hwnd, |e| (e.state.ime_caret.get(), e.state.scale_factor.get()))
    {
        set_ime_candidate_pos(hwnd, rect, scale);
    }
}

fn handle_mouse_move(hwnd: HWND, lparam: LPARAM, live_buttons: Option<MouseButtons>) {
    let non_client = live_buttons.is_none();
    let info = with_entry_mut(hwnd, |e| {
        if e.state.in_size_move.get() {
            return None;
        }
        let scale = e.state.scale_factor.get();
        let (need_track, position, pressed_button) = match live_buttons {
            None => (
                !e.tracking_nc_leave,
                screen_to_client_point(hwnd, lparam, scale),
                e.buttons.primary(),
            ),
            Some(live) => {
                e.buttons = live;
                (
                    !e.tracking_leave,
                    client_point_from_lparam(lparam, scale).2,
                    live.primary(),
                )
            }
        };
        e.hovered = true;
        Some((
            need_track,
            PlatformEvent::MouseMove {
                window_id: e.window_id,
                position,
                pressed_button,
            },
        ))
    })
    .flatten();
    if let Some((need_track, ev)) = info {
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
                with_entry_mut(hwnd, |e| {
                    if non_client {
                        e.tracking_nc_leave = true;
                    } else {
                        e.tracking_leave = true;
                    }
                });
            }
        }
        dispatch(ev);
    }
}

fn handle_mouse_leave(hwnd: HWND, non_client: bool) {
    let check_leave = with_entry_mut(hwnd, |e| {
        if non_client {
            e.tracking_nc_leave = false;
            e.nc_button_down = None;
            !e.tracking_leave
        } else {
            e.tracking_leave = false;
            !e.tracking_nc_leave
        }
    })
    .unwrap_or(false);
    if check_leave && !cursor_is_over_hwnd(hwnd) {
        let window_id = with_entry_mut(hwnd, |e| {
            e.hovered = false;
            e.window_id
        });
        if let Some(window_id) = window_id {
            dispatch(PlatformEvent::MouseLeave { window_id });
        }
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
                    handled = dispatch(PlatformEvent::KeyDown { window_id, event });
                    if handled {
                        with_entry_mut(hwnd, |e| e.suppress_next_char = true);
                    }
                }
            } else {
                dispatch(PlatformEvent::KeyUp { window_id, event });
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
    crate::diag::ffi_guard("window_wnd_proc", LRESULT(0), || {
        window_wnd_proc_inner(hwnd, msg, wparam, lparam)
    })
}

fn window_wnd_proc_inner(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_ERASEBKGND => LRESULT(1),

        WM_PAINT => {
            let _ = unsafe { ValidateRect(Some(hwnd), None) };
            if let Some(window_id) = with_entry(hwnd, |e| e.window_id) {
                dispatch(PlatformEvent::PaintRequested { window_id });
            }
            LRESULT(0)
        }

        WM_NCCALCSIZE if wparam.0 != 0 => {
            if with_entry(hwnd, |e| e.is_overlay).unwrap_or(false) {
                return LRESULT(0);
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
                let local = screen_to_client_point(hwnd, lparam, e.state.scale_factor.get());
                match hit_test_control_areas(&e.control_areas, local) {
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
                let ev = with_entry_mut(hwnd, |e| {
                    if area != HTCAPTION {
                        e.nc_button_down = Some(area);
                    }
                    e.hovered = true;
                    let position = screen_to_client_point(hwnd, lparam, e.state.scale_factor.get());
                    PlatformEvent::MouseDown {
                        window_id: e.window_id,
                        button: MouseButton::Left,
                        position,
                        modifiers: current_modifiers(),
                        click_count: 1,
                    }
                });
                if let Some(ev) = ev {
                    dispatch(ev);
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
                    let position = screen_to_client_point(hwnd, lparam, e.state.scale_factor.get());
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
                    dispatch(ev);
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
            handle_mouse_move(hwnd, lparam, Some(MouseButtons::from_wparam(wparam)));
            LRESULT(0)
        }

        WM_MOUSELEAVE => {
            handle_mouse_leave(hwnd, false);
            LRESULT(0)
        }

        WM_CAPTURECHANGED => {
            let window_id = with_entry_mut(hwnd, |e| {
                if e.releasing_capture {
                    return None;
                }
                e.buttons.clear();
                Some(e.window_id)
            })
            .flatten();
            if let Some(window_id) = window_id {
                dispatch(PlatformEvent::CaptureLost { window_id });
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
            let ev = with_entry_mut(hwnd, |e| {
                e.buttons.press(button);
                let (x, y, position) = client_point_from_lparam(lparam, e.state.scale_factor.get());
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
                dispatch(ev);
            }
            LRESULT(0)
        }

        WM_LBUTTONUP | WM_RBUTTONUP | WM_MBUTTONUP => {
            let button = match msg {
                WM_LBUTTONUP => MouseButton::Left,
                WM_RBUTTONUP => MouseButton::Right,
                _ => MouseButton::Middle,
            };
            let (release_cap, ev) = with_entry_mut(hwnd, |e| {
                e.buttons.release(button);
                let (_, _, position) = client_point_from_lparam(lparam, e.state.scale_factor.get());
                (
                    e.buttons.is_empty(),
                    PlatformEvent::MouseUp {
                        window_id: e.window_id,
                        button,
                        position,
                    },
                )
            })
            .map_or((true, None), |(rel, ev)| (rel, Some(ev)));
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
            let raw_amount = system_settings().wheel_scroll_amount(horizontal);
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
            let ev = with_entry(hwnd, |e| {
                let position = screen_to_client_point(hwnd, lparam, e.state.scale_factor.get());
                PlatformEvent::ScrollWheel {
                    window_id: e.window_id,
                    position,
                    delta_lines,
                    page_scroll,
                    modifiers,
                }
            });
            if let Some(ev) = ev {
                dispatch(ev);
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
                dispatch(ev);
            }
            LRESULT(0)
        }

        WM_IME_STARTCOMPOSITION => {
            update_ime_pos(hwnd);
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
                        dispatch(PlatformEvent::TextInput { window_id, text });
                    }
                    if (flags & GCS_COMPSTR.0) != 0 {
                        let text = read_ime_string(himc, GCS_COMPSTR).unwrap_or_default();
                        let cursor = unsafe {
                            ImmGetCompositionStringW(himc, GCS_CURSORPOS, None, 0)
                        }
                        .max(0) as usize;
                        dispatch(PlatformEvent::ImeComposition {
                            window_id,
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
            if let Some(window_id) = with_entry(hwnd, |e| e.window_id) {
                dispatch(PlatformEvent::ImeComposition {
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
            if w > 0
                && h > 0
                && let Some((window_id, scale)) = with_entry(hwnd, |e| {
                    let s = e.state.scale_factor.get();
                    e.state.set_metrics(w, h, s);
                    (e.window_id, s)
                })
            {
                dispatch(PlatformEvent::Resized {
                    window_id,
                    physical_width: w,
                    physical_height: h,
                    scale,
                });
            }
            LRESULT(0)
        }

        WM_MOVE => {
            if let Some((window_id, changed)) = with_entry(hwnd, |e| {
                let (_, origin) = query_client_metrics(hwnd, e.state.scale_factor.get());
                let prev = e.state.origin.replace(origin);
                (e.window_id, prev != origin)
            }) && changed
            {
                dispatch(PlatformEvent::Moved { window_id });
            }
            LRESULT(0)
        }

        WM_DPICHANGED => {
            let new_dpi = (wparam.0 & 0xFFFF) as u32;
            let scale =
                new_dpi.max(USER_DEFAULT_SCREEN_DPI) as f32 / USER_DEFAULT_SCREEN_DPI as f32;
            // Update scale_factor before SetWindowPos so nested WM_SIZE sees the new DPI; never resize overlays.
            let info = with_entry(hwnd, |e| {
                e.state.scale_factor.set(scale);
                (e.window_id, e.is_overlay)
            });
            if let Some((window_id, is_overlay)) = info {
                if !is_overlay {
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
                }
                let (w, h) = query_physical_size(hwnd);
                with_entry(hwnd, |e| e.state.set_metrics(w, h, scale));
                dispatch(PlatformEvent::Resized {
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
            if let Some(window_id) = with_entry(hwnd, |e| {
                e.state.active.set(active);
                e.window_id
            }) {
                dispatch(PlatformEvent::Activated { window_id, active });
            }
            LRESULT(0)
        }

        WM_SETTINGCHANGE | WM_THEMECHANGED => {
            let relevant = with_entry(hwnd, |e| !e.is_overlay).unwrap_or(false)
                && (msg == WM_THEMECHANGED || is_relevant_setting_change(lparam));
            if relevant {
                refresh_system_settings();
                dispatch(PlatformEvent::ThemeChanged);
            }
            LRESULT(0)
        }

        WM_ENTERSIZEMOVE | WM_ENTERMENULOOP => {
            if let Some((window_id, was_hovered)) = with_entry_mut(hwnd, |e| {
                e.state.in_size_move.set(true);
                let h = e.hovered;
                e.hovered = false;
                (e.window_id, h)
            }) {
                if was_hovered {
                    dispatch(PlatformEvent::MouseLeave { window_id });
                }
                dispatch(PlatformEvent::SizeMoveChanged {
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
            super::mark_wake_pending();
            service();
            LRESULT(0)
        }

        WM_EXITSIZEMOVE | WM_EXITMENULOOP => {
            let _ = unsafe { KillTimer(Some(hwnd), SIZE_MOVE_TIMER_ID) };
            if let Some(window_id) = with_entry(hwnd, |e| {
                e.state.in_size_move.set(false);
                e.window_id
            }) {
                dispatch(PlatformEvent::SizeMoveChanged {
                    window_id,
                    entering: false,
                });
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }

        WM_QUERYENDSESSION => {
            let block_reason = HWND_REGISTRY.with(|map| {
                map.borrow()
                    .values()
                    .find_map(|e| e.state.block_shutdown.get())
            });
            if let Some(reason) = block_reason {
                let wide = HSTRING::from(reason);
                let _ = unsafe { ShutdownBlockReasonCreate(hwnd, &wide) };
                if let Some(window_id) = with_entry(hwnd, |e| e.window_id) {
                    dispatch(PlatformEvent::CloseRequested { window_id });
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
                && let Some(window_id) = with_entry(hwnd, |e| e.window_id)
            {
                dispatch(PlatformEvent::FilesDropped { window_id, paths });
            }
            LRESULT(0)
        }

        WM_CLOSE => {
            if let Some(window_id) = with_entry(hwnd, |e| e.window_id) {
                dispatch(PlatformEvent::CloseRequested { window_id });
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
