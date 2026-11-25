#![allow(unsafe_code)]
//! Everything Jot asks of Windows, in layers that only depend on the ones above them:
//!
//! 1. Win32 helpers and diagnostics.
//! 2. Events and input: what the app hears about.
//! 3. The event loop: delivering events to the app, waking it, pacing frames.
//! 4. Windows: creating them and turning their messages into events.
//! 5. Displays and the desktop.
//! 6. The single-instance handoff: a second launch passing its files to the first.
//! 7. System settings.
//! 8. OS services the app calls directly: dialogs, clipboard, code pages, files.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::ffi::{OsString, c_void};
use std::io;
use std::os::windows::{
    ffi::{OsStrExt, OsStringExt},
    io::AsRawHandle,
};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use windows::Win32::{
    Foundation::{
        COLORREF, ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, ERROR_CANCELLED, ERROR_LOCK_VIOLATION,
        ERROR_SHARING_VIOLATION, ERROR_UNABLE_TO_MOVE_REPLACEMENT_2, GetLastError, HANDLE, HGLOBAL,
        HINSTANCE, HLOCAL, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
    },
    Globalization::{
        CP_UTF8, DATE_SHORTDATE, GetACP, GetDateFormatEx, GetTimeFormatEx, IsDBCSLeadByteEx,
        MB_ERR_INVALID_CHARS, MultiByteToWideChar, TIME_NOSECONDS, WC_NO_BEST_FIT_CHARS,
        WideCharToMultiByte,
    },
    Graphics::{
        Dwm::{
            DWM_SYSTEMBACKDROP_TYPE, DWMWA_BORDER_COLOR, DWMWA_CLOAKED, DWMWA_COLOR_NONE,
            DWMWA_EXTENDED_FRAME_BOUNDS, DWMWA_SYSTEMBACKDROP_TYPE,
            DWMWA_TRANSITIONS_FORCEDISABLED, DWMWA_USE_IMMERSIVE_DARK_MODE,
            DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND, DWMWINDOWATTRIBUTE,
            DwmExtendFrameIntoClientArea, DwmFlush, DwmGetWindowAttribute, DwmSetWindowAttribute,
        },
        Gdi::{
            COLOR_BTNFACE, COLOR_BTNTEXT, COLOR_GRAYTEXT, COLOR_HIGHLIGHT, COLOR_HIGHLIGHTTEXT,
            COLOR_HOTLIGHT, COLOR_WINDOW, COLOR_WINDOWFRAME, COLOR_WINDOWTEXT, ClientToScreen,
            GetMonitorInfoW, GetSysColor, HMONITOR, MONITOR_DEFAULTTONEAREST,
            MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MonitorFromPoint, PtInRect, SYS_COLOR_INDEX,
            ScreenToClient, ValidateRect,
        },
    },
    Security::{
        Authorization::ConvertSidToStringSidW, GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY,
        TOKEN_USER, TokenElevation, TokenUser,
    },
    Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_HIDDEN, GetFileInformationByHandle,
        REPLACEFILE_IGNORE_ACL_ERRORS, REPLACEFILE_IGNORE_MERGE_ERRORS, ReplaceFileW,
    },
    System::{
        Com::{
            CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
            CoTaskMemFree, CoUninitialize,
        },
        DataExchange::{
            COPYDATASTRUCT, CloseClipboard, EmptyClipboard, GetClipboardData,
            IsClipboardFormatAvailable, OpenClipboard, SetClipboardData,
        },
        Diagnostics::Debug::OutputDebugStringW,
        LibraryLoader::GetModuleHandleW,
        Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock},
        Ole::{CF_UNICODETEXT, OleInitialize, OleUninitialize},
        Registry::{HKEY_CURRENT_USER, RRF_RT_REG_BINARY, RRF_RT_REG_DWORD, RegGetValueW},
        Shutdown::{ShutdownBlockReasonCreate, ShutdownBlockReasonDestroy},
        SystemInformation::GetLocalTime,
        Threading::{CreateMutexW, GetCurrentProcess, GetCurrentThreadId, OpenProcessToken},
    },
    UI::{
        Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW},
        Controls::{MARGINS, WM_MOUSELEAVE},
        HiDpi::{
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForMonitor, GetDpiForWindow,
            GetSystemMetricsForDpi, MDT_EFFECTIVE_DPI, SetProcessDpiAwarenessContext,
        },
        Input::{
            Ime::{
                CANDIDATEFORM, CFS_CANDIDATEPOS, CFS_POINT, COMPOSITIONFORM, CPS_COMPLETE,
                GCS_COMPSTR, GCS_CURSORPOS, GCS_RESULTSTR, HIMC, IME_COMPOSITION_STRING,
                ImmGetCompositionStringW, ImmGetContext, ImmNotifyIME, ImmReleaseContext,
                ImmSetCandidateWindow, ImmSetCompositionWindow, NI_COMPOSITIONSTR,
            },
            KeyboardAndMouse::{
                GetAsyncKeyState, GetDoubleClickTime, GetKeyState, IsWindowEnabled, ReleaseCapture,
                SetCapture, TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT, TrackMouseEvent,
                VIRTUAL_KEY, VK_ADD, VK_APPS, VK_BACK, VK_CONTROL, VK_DELETE, VK_DOWN, VK_END,
                VK_ESCAPE, VK_F1, VK_F4, VK_F12, VK_HOME, VK_INSERT, VK_LBUTTON, VK_LCONTROL,
                VK_LEFT, VK_LWIN, VK_MENU, VK_NEXT, VK_NUMPAD0, VK_NUMPAD9, VK_OEM_4, VK_OEM_6,
                VK_OEM_MINUS, VK_OEM_PLUS, VK_PRIOR, VK_RBUTTON, VK_RCONTROL, VK_RETURN, VK_RIGHT,
                VK_RMENU, VK_RWIN, VK_SHIFT, VK_SPACE, VK_SUBTRACT, VK_TAB, VK_UP,
            },
        },
        Shell::{
            Common::COMDLG_FILTERSPEC, DragAcceptFiles, DragFinish, DragQueryFileW,
            FOLDERID_Documents, FOS_ALLOWMULTISELECT, FOS_FORCEFILESYSTEM, FOS_OVERWRITEPROMPT,
            FOS_PATHMUSTEXIST, FileOpenDialog, FileSaveDialog, HDROP, IFileDialog,
            IFileDialogCustomize, IFileOpenDialog, IFileSaveDialog, IShellItem, KF_FLAG_DEFAULT,
            SHCreateItemFromParsingName, SHGetKnownFolderPath, SIGDN_FILESYSPATH,
        },
        WindowsAndMessaging::{
            AllowSetForegroundWindow, CS_DBLCLKS, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT,
            CreateCaret, CreateWindowExW, DefWindowProcW, DestroyCaret, DestroyWindow,
            DispatchMessageW, EnumThreadWindows, EnumWindows, FindWindowExW, GUI_INMOVESIZE,
            GUITHREADINFO, GWL_EXSTYLE, GWLP_USERDATA, GetCaretBlinkTime, GetClientRect,
            GetCursorPos, GetGUIThreadInfo, GetMessageTime, GetMessageW, GetSystemMetrics,
            GetWindowLongPtrW, GetWindowLongW, GetWindowPlacement, GetWindowRect,
            GetWindowTextLengthW, GetWindowThreadProcessId, HTCAPTION, HTCLIENT, HTCLOSE,
            HTMAXBUTTON, HTMINBUTTON, HTTOP, HTTOPLEFT, HTTOPRIGHT, HWND_MESSAGE, HWND_TOPMOST,
            IDC_ARROW, IDC_IBEAM, IsIconic, IsWindowVisible, IsZoomed, KillTimer, LWA_ALPHA,
            LoadCursorW, LoadIconW, MB_ICONERROR, MB_OK, MINMAXINFO, MSG, MessageBoxW,
            NCCALCSIZE_PARAMS, PostMessageW, PostQuitMessage, RegisterClassW, SC_CLOSE,
            SC_MAXIMIZE, SC_MINIMIZE, SC_MOVE, SC_RESTORE, SM_CXDOUBLECLK, SM_CXFRAME,
            SM_CXPADDEDBORDER, SM_CYDOUBLECLK, SM_CYFRAME, SM_SWAPBUTTON, SMTO_ABORTIFHUNG,
            SMTO_BLOCK, SPI_GETCARETWIDTH, SPI_GETCLIENTAREAANIMATION, SPI_GETHIGHCONTRAST,
            SPI_GETMENUSHOWDELAY, SPI_GETMOUSEHOVERTIME, SPI_GETWHEELSCROLLCHARS,
            SPI_GETWHEELSCROLLLINES, SW_HIDE, SW_RESTORE, SW_SHOW, SW_SHOWNOACTIVATE,
            SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER,
            SYSTEM_PARAMETERS_INFO_ACTION, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
            SendMessageTimeoutW, SetCaretPos, SetCursor, SetForegroundWindow,
            SetLayeredWindowAttributes, SetTimer, SetWindowLongPtrW, SetWindowPlacement,
            SetWindowPos, SetWindowTextW, ShowWindow, SystemParametersInfoW, TranslateMessage,
            USER_DEFAULT_SCREEN_DPI, USER_TIMER_MAXIMUM, USER_TIMER_MINIMUM, WINDOW_EX_STYLE,
            WINDOW_STYLE, WINDOWPLACEMENT, WM_ACTIVATE, WM_APP, WM_CANCELMODE, WM_CAPTURECHANGED,
            WM_CHAR, WM_CLOSE, WM_COPYDATA, WM_DPICHANGED, WM_DROPFILES, WM_ENDSESSION,
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
use windows::core::{BOOL, HSTRING, Interface, Owned, PCWSTR, PWSTR, w};

use crate::geom::{Point, Rect, ScreenPoint, ScreenRect, Size, point, size};
use crate::types::{Error, Result, SharedStr};

// ---- 1. Win32 helpers and diagnostics -------------------------------------------------------

/// The signed x and y packed into a message's `LPARAM`.
fn lparam_xy(lparam: LPARAM) -> (i32, i32) {
    (
        (lparam.0 & 0xFFFF) as i16 as i32,
        ((lparam.0 >> 16) & 0xFFFF) as i16 as i32,
    )
}

/// The scale factor for `dpi` (never below 1).
fn scale_for_dpi(dpi: u32) -> f32 {
    dpi.max(USER_DEFAULT_SCREEN_DPI) as f32 / USER_DEFAULT_SCREEN_DPI as f32
}

fn window_dpi(hwnd: HWND) -> u32 {
    unsafe { GetDpiForWindow(hwnd) }.max(USER_DEFAULT_SCREEN_DPI)
}

/// The resize border's width and height at `dpi`.
fn frame_thickness(dpi: u32) -> (i32, i32) {
    unsafe {
        let pad = GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi);
        (
            GetSystemMetricsForDpi(SM_CXFRAME, dpi) + pad,
            GetSystemMetricsForDpi(SM_CYFRAME, dpi) + pad,
        )
    }
}

/// The client area's size in pixels (at least 1x1).
fn client_size(hwnd: HWND) -> (u32, u32) {
    let mut rc = RECT::default();
    let _ = unsafe { GetClientRect(hwnd, &mut rc) };
    (
        (rc.right - rc.left).max(1) as u32,
        (rc.bottom - rc.top).max(1) as u32,
    )
}

/// A screen point (packed into `lparam`) in `hwnd`'s client coordinates.
fn screen_to_client(hwnd: HWND, lparam: LPARAM, scale: f32) -> Point {
    let (x, y) = lparam_xy(lparam);
    let mut pt = POINT { x, y };
    let _ = unsafe { ScreenToClient(hwnd, &mut pt) };
    point(pt.x as f32 / scale, pt.y as f32 / scale)
}

fn set_dwm<T>(hwnd: HWND, attr: DWMWINDOWATTRIBUTE, value: T) {
    let size = size_of::<T>() as u32;
    let _ = unsafe { DwmSetWindowAttribute(hwnd, attr, (&raw const value).cast(), size) };
}

fn system_parameters_u32(action: u32) -> Option<u32> {
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

#[cold]
pub(crate) fn debug_log(message: &str) {
    unsafe { OutputDebugStringW(&HSTRING::from(format!("[Jot] {message}\n"))) };
}

#[cold]
pub(crate) fn show_error_dialog(title: &str, message: &str) {
    let (message, title) = (HSTRING::from(message), HSTRING::from(title));
    let _ = unsafe { MessageBoxW(None, &message, &title, MB_OK | MB_ICONERROR) };
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
        let at = info.location().map_or_else(
            || "<unknown>".to_owned(),
            |l| format!("{}:{}:{}", l.file(), l.line(), l.column()),
        );
        debug_log(&format!("PANIC at {at}: {}", panic_message(info.payload())));
        prev(info);
    }));
}

/// Runs `f`, which Windows called into: a panic must not unwind into Windows, so it's logged,
/// reported once, and `fallback` returned instead.
fn ffi_guard<R>(context: &str, fallback: R, f: impl FnOnce() -> R) -> R {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|payload| {
        let msg = panic_message(&*payload);
        debug_log(&format!(
            "Caught panic at Win32 boundary ({context}): {msg}"
        ));
        #[cfg(not(test))]
        {
            use std::sync::atomic::{AtomicBool, Ordering};
            static SHOWN: AtomicBool = AtomicBool::new(false);
            if !SHOWN.swap(true, Ordering::AcqRel) {
                show_error_dialog(
                    crate::types::APP_NAME,
                    &format!(
                        "Jot encountered an unexpected error and prevented a crash.\n\n\
                         Please save any open documents and restart Jot.\n\nDetails: {msg}"
                    ),
                );
            }
        }
        fallback
    })
}

// ---- 2. Events and input --------------------------------------------------------------------

crate::types::id_type!(WindowId);

/// What a point in a window's client area is for, as far as Windows is concerned.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum WindowControlArea {
    Client,
    Drag,
    Min,
    Max,
    Close,
}

/// The topmost of `areas` containing `pos` (later areas are on top).
pub(crate) fn hit_test_control_areas(
    areas: &[(Rect, WindowControlArea)],
    pos: Point,
) -> WindowControlArea {
    let hit = areas.iter().rev().find(|(rect, _)| rect.contains(pos));
    hit.map_or(WindowControlArea::Client, |&(_, area)| area)
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
            title: SharedStr::Static("Jot"),
            display_id: None,
            focus: true,
            show: true,
        }
    }
}

/// What the app hears about: something that happened to one of its windows, or to the app.
#[derive(Clone, Debug)]
pub(crate) enum PlatformEvent {
    Window(WindowId, WindowEvent),
    SettingsChanged(SystemSettings),
    ModifiersChanged(Modifiers),
    /// Another launch handed over the files it was asked to open.
    Launch(Vec<PathBuf>),
    SessionEnding(bool),
    /// Something asked to wake the loop: finished background work, or a deadline.
    Wake,
}

#[derive(Clone, Debug)]
pub(crate) enum WindowEvent {
    /// The client area changed size (the window's state already has the new size).
    Resized,
    Moved,
    Activated(bool),
    PaintRequested,
    MouseMove {
        position: Point,
        pressed_button: Option<MouseButton>,
    },
    MouseLeave,
    MouseDown {
        button: MouseButton,
        position: Point,
        modifiers: Modifiers,
        click_count: usize,
    },
    MouseUp {
        button: MouseButton,
        position: Point,
    },
    ScrollWheel {
        position: Point,
        delta_lines: Point,
        page_scroll: bool,
        modifiers: Modifiers,
    },
    KeyDown(KeyEvent),
    KeyUp(KeyEvent),
    CaptureLost,
    TextInput(String),
    ImeComposition {
        text: String,
        cursor: usize,
    },
    FilesDropped(Vec<PathBuf>),
    /// Windows' own move/resize or menu loop starting (`true`) or ending.
    SizeMoveChanged(bool),
    CloseRequested,
}

pub(crate) trait PlatformHandler {
    /// Handles `event`; for key presses, whether the app used it.
    fn on_event(&mut self, event: PlatformEvent) -> bool;
    /// Runs after each batch of events: the app's chance to draw.
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
    pub(crate) const fn none(self) -> bool {
        !self.ctrl && !self.alt && !self.shift && !self.win
    }

    pub(crate) const fn ctrl_only(self) -> bool {
        self.ctrl && !self.alt && !self.shift && !self.win && !self.alt_gr
    }

    fn current() -> Self {
        let down = |vk: VIRTUAL_KEY| (unsafe { GetKeyState(i32::from(vk.0)) } as u16 & 0x8000) != 0;
        Self {
            ctrl: down(VK_CONTROL),
            alt: down(VK_MENU),
            shift: down(VK_SHIFT),
            win: down(VK_LWIN) || down(VK_RWIN),
            // Windows reports AltGr as left Ctrl plus right Alt.
            alt_gr: down(VK_RMENU) && down(VK_LCONTROL) && !down(VK_RCONTROL),
        }
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

impl Key {
    fn from_vk(vk: u16, modifiers: Modifiers) -> Option<Self> {
        Some(match VIRTUAL_KEY(vk) {
            VK_BACK => Self::Backspace,
            VK_TAB => Self::Tab,
            VK_RETURN => Self::Enter,
            VK_ESCAPE => Self::Escape,
            VK_SPACE => Self::Space,
            VK_PRIOR => Self::PageUp,
            VK_NEXT => Self::PageDown,
            VK_END => Self::End,
            VK_HOME => Self::Home,
            VK_LEFT => Self::Left,
            VK_UP => Self::Up,
            VK_RIGHT => Self::Right,
            VK_DOWN => Self::Down,
            VK_INSERT => Self::Insert,
            VK_DELETE => Self::Delete,
            VK_MENU => Self::Alt,
            VK_APPS => Self::Menu,
            VK_OEM_PLUS => Self::Char(if modifiers.shift { '+' } else { '=' }),
            VK_ADD => Self::Char('+'),
            VK_OEM_MINUS | VK_SUBTRACT => Self::Char('-'),
            VK_OEM_4 => Self::Char('['),
            VK_OEM_6 => Self::Char(']'),
            _ if (VK_F1.0..=VK_F12.0).contains(&vk) => Self::F((vk - VK_F1.0 + 1) as u8),
            _ if (VK_NUMPAD0.0..=VK_NUMPAD9.0).contains(&vk) => {
                Self::Char((b'0' + (vk - VK_NUMPAD0.0) as u8) as char)
            }
            _ if (u16::from(b'0')..=u16::from(b'9')).contains(&vk) => Self::Char(vk as u8 as char),
            _ if (u16::from(b'A')..=u16::from(b'Z')).contains(&vk) => {
                Self::Char((vk as u8 as char).to_ascii_lowercase())
            }
            _ => return None,
        })
    }
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

impl MouseButton {
    /// The button's `MK_*` flag, as mouse messages report held buttons.
    fn flag(self) -> u32 {
        match self {
            Self::Left => 0x1,
            Self::Right => 0x2,
            Self::Middle => 0x10,
        }
    }

    /// The first of the buttons in `flags`.
    fn first(flags: u32) -> Option<Self> {
        [Self::Left, Self::Right, Self::Middle]
            .into_iter()
            .find(|b| flags & b.flag() != 0)
    }
}

/// Counts presses of the same button in the same spot within the double-click time.
#[derive(Default)]
struct ClickTracker {
    last: Option<(MouseButton, (i32, i32), u32)>,
    count: usize,
}

impl ClickTracker {
    fn record(&mut self, button: MouseButton, at: (i32, i32)) -> usize {
        let now = unsafe { GetMessageTime() } as u32;
        let (dbl_ms, tol_x, tol_y) = unsafe {
            (
                GetDoubleClickTime(),
                (GetSystemMetrics(SM_CXDOUBLECLK) / 2).max(2),
                (GetSystemMetrics(SM_CYDOUBLECLK) / 2).max(2),
            )
        };
        let again = self.last.is_some_and(|(b, (x, y), t)| {
            b == button
                && now.wrapping_sub(t) <= dbl_ms
                && (at.0 - x).abs() <= tol_x
                && (at.1 - y).abs() <= tol_y
        });
        self.count = if again { self.count + 1 } else { 1 };
        self.last = Some((button, at, now));
        self.count
    }
}

// ---- 3. The event loop ----------------------------------------------------------------------

const WM_APP_WAKE: u32 = WM_APP + 1;
const WM_APP_VSYNC: u32 = WM_APP + 2;
const WAKE_TIMER_ID: usize = 2;
const WAKE_CLASS: PCWSTR = w!("JotWakeWindow");

#[derive(Default)]
struct Loop {
    handler: Option<Rc<RefCell<dyn PlatformHandler>>>,
    /// Events that arrived while the app was busy (Windows calls back in while the app is
    /// itself calling Windows), delivered as soon as it's free.
    queue: VecDeque<PlatformEvent>,
    shutdown_block: Option<&'static str>,
}

thread_local! {
    static LOOP: RefCell<Loop> = RefCell::new(Loop::default());
}

/// Delivers `event` (if any) and everything queued to the app, then lets it draw. If the app is
/// busy, `event` is queued instead. Returns whether the app used `event`.
fn deliver(event: Option<PlatformEvent>) -> bool {
    fn drain(app: &mut dyn PlatformHandler) {
        while let Some(event) = LOOP.with(|l| l.borrow_mut().queue.pop_front()) {
            app.on_event(event);
        }
    }
    let Some(handler) = LOOP.with(|l| l.borrow().handler.clone()) else {
        return false;
    };
    let Ok(mut app) = handler.try_borrow_mut() else {
        if let Some(event) = event {
            LOOP.with(|l| queue(&mut l.borrow_mut().queue, event));
        }
        return false;
    };
    drain(&mut *app);
    let handled = event.is_some_and(|event| app.on_event(event));
    drain(&mut *app);
    app.drive_frames();
    drain(&mut *app);
    handled
}

/// Queues `event`, folding it into the last one if that's the same kind of update.
fn queue(queue: &mut VecDeque<PlatformEvent>, event: PlatformEvent) {
    use PlatformEvent::{SettingsChanged, Wake, Window};
    use WindowEvent::{Moved, Resized};
    match (queue.back_mut(), event) {
        (Some(Wake), Wake) => {}
        (Some(Window(a, Moved)), Window(b, Moved)) if *a == b => {}
        (Some(Window(a, Resized)), Window(b, Resized)) if *a == b => {}
        (Some(SettingsChanged(last)), SettingsChanged(next)) => *last = next,
        (_, event) => queue.push_back(event),
    }
}

/// Makes `handler` the app until the guard drops.
fn install(handler: Rc<RefCell<dyn PlatformHandler>>) -> impl Drop {
    struct Uninstall;
    impl Drop for Uninstall {
        fn drop(&mut self) {
            LOOP.with(|l| *l.borrow_mut() = Loop::default());
        }
    }
    LOOP.with(|l| l.borrow_mut().handler = Some(handler));
    Uninstall
}

/// Wakes the event loop from any thread.
#[derive(Copy, Clone)]
pub(crate) struct Waker(isize);

impl Waker {
    pub(crate) fn wake(self) {
        let hwnd = HWND(self.0 as *mut _);
        let _ = unsafe { PostMessageW(Some(hwnd), WM_APP_WAKE, WPARAM(0), LPARAM(0)) };
    }
}

/// Paces frames to the display: when armed, a thread waits for the next composition and then
/// pokes the loop.
struct Vsync {
    armed: Mutex<bool>,
    ready: Condvar,
}

impl Vsync {
    fn spawn(wake_hwnd: isize) -> Result<Arc<Self>> {
        let vsync = Arc::new(Self {
            armed: Mutex::new(false),
            ready: Condvar::new(),
        });
        let pacer = vsync.clone();
        thread::Builder::new()
            .name("jot-vsync".into())
            .spawn(move || {
                loop {
                    {
                        let armed = pacer.armed.lock().unwrap_or_else(PoisonError::into_inner);
                        let mut armed = pacer
                            .ready
                            .wait_while(armed, |armed| !*armed)
                            .unwrap_or_else(PoisonError::into_inner);
                        *armed = false;
                    }
                    if unsafe { DwmFlush() }.is_err() {
                        thread::sleep(Duration::from_millis(16));
                    }
                    let hwnd = HWND(wake_hwnd as *mut _);
                    let _ = unsafe { PostMessageW(Some(hwnd), WM_APP_VSYNC, WPARAM(0), LPARAM(0)) };
                }
            })?;
        Ok(vsync)
    }

    fn arm(&self) {
        let mut armed = self.armed.lock().unwrap_or_else(PoisonError::into_inner);
        if !*armed {
            *armed = true;
            self.ready.notify_one();
        }
    }
}

/// The process's link to Windows: per-monitor DPI, COM, the window classes, and a hidden
/// window the loop is woken through (which also receives other launches' files).
pub(crate) struct Platform {
    wake_hwnd: HWND,
    vsync: Arc<Vsync>,
    com_initialized: bool,
    _instance_lock: Option<InstanceLock>,
}

impl Platform {
    pub(crate) fn new(instance_lock: Option<InstanceLock>) -> Result<Self> {
        unsafe {
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }
        let com_initialized = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
        let hinstance: HINSTANCE = unsafe { GetModuleHandleW(None)? }.into();
        let icon = unsafe { LoadIconW(Some(hinstance), PCWSTR(std::ptr::without_provenance(1))) };
        let classes = [
            WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW | CS_DBLCLKS,
                lpfnWndProc: Some(window_proc),
                hInstance: hinstance,
                hIcon: icon.unwrap_or_default(),
                hCursor: unsafe { LoadCursorW(None, IDC_ARROW) }.unwrap_or_default(),
                lpszClassName: WINDOW_CLASS,
                ..Default::default()
            },
            WNDCLASSW {
                lpfnWndProc: Some(overlay_proc),
                hInstance: hinstance,
                lpszClassName: OVERLAY_CLASS,
                ..Default::default()
            },
            WNDCLASSW {
                lpfnWndProc: Some(wake_proc),
                hInstance: hinstance,
                lpszClassName: WAKE_CLASS,
                ..Default::default()
            },
        ];
        for class in &classes {
            unsafe { RegisterClassW(class) };
        }
        let title = instance_lock.as_ref().map_or_else(HSTRING::new, |lock| {
            HSTRING::from(lock.window_title.as_str())
        });
        let wake_hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                WAKE_CLASS,
                &title,
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                Some(hinstance),
                None,
            )?
        };
        Ok(Self {
            wake_hwnd,
            vsync: Vsync::spawn(wake_hwnd.0 as isize)?,
            com_initialized,
            _instance_lock: instance_lock,
        })
    }

    /// A handle any thread can use to wake the loop (delivering `PlatformEvent::Wake`).
    pub(crate) fn waker(&self) -> Waker {
        Waker(self.wake_hwnd.0 as isize)
    }

    /// Wakes the loop at `at`, replacing the previous request; `None` cancels it.
    pub(crate) fn wake_at(&self, at: Option<Instant>) {
        let Some(at) = at else {
            let _ = unsafe { KillTimer(Some(self.wake_hwnd), WAKE_TIMER_ID) };
            return;
        };
        let wait = at.saturating_duration_since(Instant::now());
        if wait.is_zero() {
            self.waker().wake();
            return;
        }
        let ms = (wait.as_millis().min(u128::from(u32::MAX)) as u32)
            .clamp(USER_TIMER_MINIMUM, USER_TIMER_MAXIMUM);
        let _ = unsafe { SetTimer(Some(self.wake_hwnd), WAKE_TIMER_ID, ms, None) };
    }

    /// Lets the app draw again once the display has shown the current frame.
    pub(crate) fn request_frame_tick(&self) {
        self.vsync.arm();
    }

    /// Asks Windows to stop a shutdown or logoff (giving `reason`) while there are unsaved
    /// changes; `None` lifts it.
    pub(crate) fn set_block_shutdown(&self, reason: Option<&'static str>) {
        let prev = LOOP.with(|l| std::mem::replace(&mut l.borrow_mut().shutdown_block, reason));
        if prev.is_some() && reason.is_none() {
            unsafe extern "system" fn clear(hwnd: HWND, _: LPARAM) -> BOOL {
                let _ = unsafe { ShutdownBlockReasonDestroy(hwnd) };
                BOOL(1)
            }
            let _ = unsafe { EnumThreadWindows(GetCurrentThreadId(), Some(clear), LPARAM(0)) };
        }
    }

    /// Stops taking files from other launches (they find the wake window by its title).
    fn stop_handoff(&self) {
        let _ = unsafe { SetWindowTextW(self.wake_hwnd, w!("")) };
    }

    pub(crate) fn post_quit(&self) {
        self.stop_handoff();
        unsafe { PostQuitMessage(0) };
    }

    /// Runs the message loop with `handler` as the app until the app quits.
    pub(crate) fn run<H: PlatformHandler + 'static>(
        handler: Rc<RefCell<H>>,
        on_ready: impl FnOnce(&mut H),
    ) {
        let _installed = install(handler.clone());
        on_ready(&mut handler.borrow_mut());
        deliver(None);
        let mut msg = MSG::default();
        while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            deliver(None);
        }
    }

    #[cfg(test)]
    pub(crate) fn with_test_handler<R>(
        handler: Rc<RefCell<dyn PlatformHandler>>,
        f: impl FnOnce() -> R,
    ) -> R {
        let _installed = install(handler);
        f()
    }
}

impl Drop for Platform {
    fn drop(&mut self) {
        self.stop_handoff();
        unsafe {
            let _ = DestroyWindow(self.wake_hwnd);
            if self.com_initialized {
                CoUninitialize();
            }
        }
    }
}

unsafe extern "system" fn wake_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    ffi_guard("wake_proc", LRESULT(0), || match msg {
        WM_APP_WAKE => {
            deliver(Some(PlatformEvent::Wake));
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == WAKE_TIMER_ID => {
            let _ = unsafe { KillTimer(Some(hwnd), WAKE_TIMER_ID) };
            deliver(Some(PlatformEvent::Wake));
            LRESULT(0)
        }
        WM_APP_VSYNC => {
            deliver(None);
            LRESULT(0)
        }
        WM_COPYDATA => {
            let cds = unsafe { &*(lparam.0 as *const COPYDATASTRUCT) };
            let accepting = unsafe { GetWindowTextLengthW(hwnd) } > 0;
            match decode_paths(cds).filter(|_| accepting) {
                Some(paths) => {
                    deliver(Some(PlatformEvent::Launch(paths)));
                    LRESULT(1)
                }
                None => LRESULT(0),
            }
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    })
}

// ---- 4. Windows -----------------------------------------------------------------------------

const WINDOW_CLASS: PCWSTR = w!("JotWindow");
const OVERLAY_CLASS: PCWSTR = w!("JotOverlay");
const DWMSBT_TABBEDWINDOW: i32 = 4;
/// Keeps the app drawing while Windows runs a modal move/resize or menu loop.
const SIZE_MOVE_TIMER_ID: usize = 1;

/// A window's state between messages, kept in its user data.
struct WindowState {
    id: WindowId,
    scale: Cell<f32>,
    physical: Cell<(u32, u32)>,
    active: Cell<bool>,
    in_size_move: Cell<bool>,
    min_size: Cell<Option<Size>>,
    cursor: Cell<CursorStyle>,
    control_areas: RefCell<Vec<(Rect, WindowControlArea)>>,
    /// Where an IME's composition and candidates go, and the system caret (in pixels) shown
    /// there for accessibility tools.
    ime_caret: Cell<Option<Rect>>,
    system_caret: Cell<Option<(i32, i32, i32, i32)>>,
    origin: Cell<Option<(i32, i32)>>,
    input: RefCell<Input>,
}

#[derive(Default)]
struct Input {
    hovered: bool,
    tracking_leave: bool,
    tracking_nc_leave: bool,
    /// The caption button pressed, to act on when it's released over the same button.
    caption_button: Option<u32>,
    /// The buttons held (`MK_*` flags) since mouse capture was last lost: capture is released
    /// when the last of them is.
    buttons: u32,
    /// Set while the app releases capture itself, so that isn't reported as capture lost.
    releasing_capture: bool,
    clicks: ClickTracker,
    modifiers: Modifiers,
    /// A key press the app used: the character it would type is dropped.
    swallow_char: bool,
    high_surrogate: Option<u16>,
}

impl Input {
    /// Whether Windows will report the mouse leaving the client area (or the frame).
    fn tracking_leave(&mut self, client: bool) -> &mut bool {
        if client {
            &mut self.tracking_leave
        } else {
            &mut self.tracking_nc_leave
        }
    }
}

impl WindowState {
    /// The state of a window `NativeWindow` created.
    fn of(hwnd: HWND) -> Option<Rc<Self>> {
        let state = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const Self;
        (!state.is_null()).then(|| unsafe {
            Rc::increment_strong_count(state);
            Rc::from_raw(state)
        })
    }

    fn send(&self, event: WindowEvent) -> bool {
        deliver(Some(PlatformEvent::Window(self.id, event)))
    }

    fn client_point(&self, lparam: LPARAM) -> Point {
        let (x, y) = lparam_xy(lparam);
        point(x as f32 / self.scale.get(), y as f32 / self.scale.get())
    }
}

/// A window this module created, destroyed when dropped.
pub(crate) struct NativeWindow {
    pub(crate) id: WindowId,
    hwnd: HWND,
    state: Rc<WindowState>,
}

impl NativeWindow {
    pub(crate) fn create_main(config: &MainWindowConfig, dark: bool) -> Result<Self> {
        let hinstance = unsafe { GetModuleHandleW(None)? };
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_APPWINDOW | WS_EX_NOREDIRECTIONBITMAP,
                WINDOW_CLASS,
                &HSTRING::from(config.title.as_str()),
                WS_SYSMENU | WS_THICKFRAME | WS_MAXIMIZEBOX | WS_MINIMIZEBOX,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                None,
                None,
                Some(hinstance.into()),
                None,
            )?
        };
        let window = Self::adopt(hwnd, config.show && config.focus);
        unsafe {
            SetWindowLongPtrW(
                hwnd,
                GWLP_USERDATA,
                Rc::into_raw(window.state.clone()) as isize,
            )
        };
        let margins = MARGINS {
            cxLeftWidth: -1,
            cxRightWidth: -1,
            cyTopHeight: -1,
            cyBottomHeight: -1,
        };
        let _ = unsafe { DwmExtendFrameIntoClientArea(hwnd, &margins) };
        set_dwm(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE,
            DWM_SYSTEMBACKDROP_TYPE(DWMSBT_TABBEDWINDOW),
        );
        window.set_dark(dark);
        if let Some(b) = config.bounds {
            let display = config.display_id.and_then(WindowsDisplay::from_id);
            let scale = display.map_or(window.state.scale.get(), |d| d.scale_factor());
            let (bx, by) = frame_thickness((scale * USER_DEFAULT_SCREEN_DPI as f32).round() as u32);
            let px = |v: f32| (v * scale).round() as i32;
            let (w, h) = (px(b.size.width).max(1), px(b.size.height).max(1));
            let flags = SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED;
            let _ = unsafe {
                SetWindowPos(
                    hwnd,
                    None,
                    px(b.origin.x) - bx,
                    px(b.origin.y),
                    w + bx * 2,
                    h + by,
                    flags,
                )
            };
        }
        unsafe { DragAcceptFiles(hwnd, true) };
        window.state.scale.set(window_scale(hwnd));
        window.state.physical.set(client_size(hwnd));
        if config.show {
            window.show(config.focus);
        }
        Ok(window)
    }

    /// A borderless, click-through, never-activated layered window for drawing popups.
    pub(crate) fn create_overlay() -> Result<Self> {
        let hinstance = unsafe { GetModuleHandleW(None)? };
        let ex_style = WS_EX_NOACTIVATE
            | WS_EX_TOOLWINDOW
            | WS_EX_TOPMOST
            | WS_EX_TRANSPARENT
            | WS_EX_LAYERED
            | WS_EX_NOREDIRECTIONBITMAP;
        let hwnd = unsafe {
            CreateWindowExW(
                ex_style,
                OVERLAY_CLASS,
                &HSTRING::new(),
                WS_POPUP,
                0,
                0,
                1,
                1,
                None,
                None,
                Some(hinstance.into()),
                None,
            )?
        };
        set_dwm(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            DWMWCP_DONOTROUND.0 as u32,
        );
        set_dwm(hwnd, DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE);
        set_dwm(hwnd, DWMWA_TRANSITIONS_FORCEDISABLED, 1u32);
        let _ = unsafe { SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA) };
        Ok(Self::adopt(hwnd, false))
    }

    fn adopt(hwnd: HWND, active: bool) -> Self {
        let id = WindowId::next();
        let state = Rc::new(WindowState {
            id,
            scale: Cell::new(window_scale(hwnd)),
            physical: Cell::new(client_size(hwnd)),
            active: Cell::new(active),
            in_size_move: Cell::new(false),
            min_size: Cell::new(None),
            cursor: Cell::new(CursorStyle::default()),
            control_areas: RefCell::default(),
            ime_caret: Cell::new(None),
            system_caret: Cell::new(None),
            origin: Cell::new(None),
            input: RefCell::new(Input {
                modifiers: Modifiers::current(),
                ..Input::default()
            }),
        });
        Self { id, hwnd, state }
    }

    pub(crate) fn show(&self, focus: bool) {
        let _ = unsafe { ShowWindow(self.hwnd, if focus { SW_SHOW } else { SW_SHOWNOACTIVATE }) };
    }

    pub(crate) fn set_title(&self, title: &str) {
        let _ = unsafe { SetWindowTextW(self.hwnd, &HSTRING::from(title)) };
    }

    /// Whether the title bar and frame are drawn dark.
    pub(crate) fn set_dark(&self, dark: bool) {
        set_dwm(
            self.hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            BOOL(i32::from(dark)),
        );
    }

    pub(crate) fn set_cursor(&self, style: CursorStyle) {
        if self.state.cursor.replace(style) != style {
            apply_cursor(style);
        }
    }

    pub(crate) fn set_min_size(&self, min_size: Option<Size>) {
        self.state.min_size.set(min_size);
    }

    /// Which parts of the client area are the caption and its buttons.
    pub(crate) fn set_control_areas(&self, areas: Vec<(Rect, WindowControlArea)>) {
        *self.state.control_areas.borrow_mut() = areas;
    }

    pub(crate) fn hwnd(&self) -> WindowHandle {
        WindowHandle(self.hwnd.0 as isize)
    }

    pub(crate) fn scale_factor(&self) -> f32 {
        self.state.scale.get()
    }

    pub(crate) fn physical_size(&self) -> (u32, u32) {
        self.state.physical.get()
    }

    pub(crate) fn logical_size(&self) -> Size {
        let ((w, h), scale) = (self.state.physical.get(), self.state.scale.get());
        size(w as f32 / scale, h as f32 / scale)
    }

    /// The size the window has when not maximized.
    pub(crate) fn restored_size(&self) -> Size {
        let hwnd = self.hwnd();
        let current = self.logical_size();
        if hwnd.is_maximized() {
            hwnd.restored_logical_size().unwrap_or(current)
        } else {
            current
        }
    }

    pub(crate) fn is_active(&self) -> bool {
        self.state.active.get()
    }

    /// Where the caret is, for the IME's windows; `show_system_caret` also puts the system
    /// caret there.
    pub(crate) fn set_text_caret(&self, ime_rect: Option<Rect>, show_system_caret: bool) {
        self.state.ime_caret.set(ime_rect);
        let scale = self.state.scale.get();
        let px = |v: f32| (v * scale).round() as i32;
        let caret = ime_rect.filter(|_| show_system_caret).map(|r| {
            (
                px(r.origin.x),
                px(r.origin.y),
                px(r.size.width).max(1),
                px(r.size.height).max(1),
            )
        });
        if self.state.system_caret.replace(caret) == caret {
            return;
        }
        unsafe {
            match caret {
                Some((x, y, w, h)) => {
                    let _ = CreateCaret(self.hwnd, None, w, h);
                    let _ = SetCaretPos(x, y);
                }
                None => {
                    let _ = DestroyCaret();
                }
            }
        }
    }

    /// Re-reads the client size and scale (Windows doesn't send them during a live resize of
    /// another window); `None` while the window has no area.
    pub(crate) fn sync_live_metrics(&self) -> Option<(u32, u32, f32)> {
        let mut rc = RECT::default();
        unsafe { GetClientRect(self.hwnd, &mut rc) }.ok()?;
        let (w, h) = (rc.right - rc.left, rc.bottom - rc.top);
        if w <= 0 || h <= 0 {
            return None;
        }
        let scale = window_scale(self.hwnd);
        self.state.physical.set((w as u32, h as u32));
        self.state.scale.set(scale);
        Some((w as u32, h as u32, scale))
    }
}

impl Drop for NativeWindow {
    fn drop(&mut self) {
        let state = unsafe { SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0) } as *const WindowState;
        if !state.is_null() {
            drop(unsafe { Rc::from_raw(state) });
        }
        if self.state.in_size_move.get() {
            let _ = unsafe { KillTimer(Some(self.hwnd), SIZE_MOVE_TIMER_ID) };
        }
        let _ = unsafe { DestroyWindow(self.hwnd) };
    }
}

fn window_scale(hwnd: HWND) -> f32 {
    scale_for_dpi(unsafe { GetDpiForWindow(hwnd) })
}

fn apply_cursor(style: CursorStyle) {
    let id = match style {
        CursorStyle::Arrow => IDC_ARROW,
        CursorStyle::IBeam => IDC_IBEAM,
    };
    if let Ok(cursor) = unsafe { LoadCursorW(None, id) } {
        unsafe { SetCursor(Some(cursor)) };
    }
}

unsafe extern "system" fn overlay_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    ffi_guard("window_proc", LRESULT(0), || {
        let result = match WindowState::of(hwnd) {
            Some(w) => on_message(&w, hwnd, msg, wparam, lparam),
            // While it's created or destroyed, a window has no state: only its frame matters.
            None if msg == WM_NCCALCSIZE && wparam.0 != 0 => {
                Some(calc_frame(hwnd, msg, wparam, lparam))
            }
            None => None,
        };
        result.unwrap_or_else(|| unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) })
    })
}

/// Handles a message for window `w`; `None` leaves it to Windows' default handling.
fn on_message(
    w: &WindowState,
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> Option<LRESULT> {
    let handled = |_| Some(LRESULT(0));
    match msg {
        WM_ERASEBKGND => Some(LRESULT(1)),
        WM_PAINT => {
            let _ = unsafe { ValidateRect(Some(hwnd), None) };
            handled(w.send(WindowEvent::PaintRequested))
        }

        // The frame: no title bar (the app draws its own caption), and a top border that's
        // still there for resizing.
        WM_NCCALCSIZE if wparam.0 != 0 => Some(calc_frame(hwnd, msg, wparam, lparam)),
        WM_GETMINMAXINFO => {
            let min = w.min_size.get()?;
            let dpi = window_dpi(hwnd);
            let (scale, (bx, by)) = (scale_for_dpi(dpi), frame_thickness(dpi));
            let mmi = unsafe { &mut *(lparam.0 as *mut MINMAXINFO) };
            mmi.ptMinTrackSize.x = (min.width * scale).ceil() as i32 + bx * 2;
            mmi.ptMinTrackSize.y = (min.height * scale).ceil() as i32 + by;
            Some(LRESULT(0))
        }
        WM_NCHITTEST => {
            let hit = unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
            if hit.0 != HTCLIENT as isize {
                return Some(hit);
            }
            Some(LRESULT(hit_test(w, hwnd, lparam) as isize))
        }
        WM_SETCURSOR if (lparam.0 & 0xFFFF) as u32 == HTCLIENT => {
            apply_cursor(w.cursor.get());
            Some(LRESULT(1))
        }

        // The caption buttons are drawn by the app; Windows only gets to know where they are
        // (so it can show snap layouts), and clicks on them are played back as commands.
        WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK => {
            let area = wparam.0 as u32;
            if msg == WM_NCLBUTTONDBLCLK && area == HTCAPTION
                || !matches!(area, HTMINBUTTON | HTMAXBUTTON | HTCLOSE | HTCAPTION)
            {
                return None;
            }
            {
                let mut input = w.input.borrow_mut();
                if area != HTCAPTION {
                    input.caption_button = Some(area);
                }
                input.hovered = true;
            }
            w.send(WindowEvent::MouseDown {
                button: MouseButton::Left,
                position: screen_to_client(hwnd, lparam, w.scale.get()),
                modifiers: Modifiers::current(),
                click_count: 1,
            });
            (area != HTCAPTION).then_some(LRESULT(0))
        }
        WM_NCLBUTTONUP => {
            let area = wparam.0 as u32;
            if !matches!(area, HTMINBUTTON | HTMAXBUTTON | HTCLOSE) {
                return None;
            }
            let pressed = w.input.borrow_mut().caption_button.take();
            w.send(WindowEvent::MouseUp {
                button: MouseButton::Left,
                position: screen_to_client(hwnd, lparam, w.scale.get()),
            });
            let command = match area {
                HTMINBUTTON => SC_MINIMIZE,
                HTMAXBUTTON if unsafe { IsZoomed(hwnd) }.as_bool() => SC_RESTORE,
                HTMAXBUTTON => SC_MAXIMIZE,
                _ => SC_CLOSE,
            };
            if pressed == Some(area) {
                let _ = unsafe {
                    PostMessageW(
                        Some(hwnd),
                        WM_SYSCOMMAND,
                        WPARAM(command as usize),
                        LPARAM(0),
                    )
                };
            }
            Some(LRESULT(0))
        }

        // The mouse.
        WM_MOUSEMOVE => handled(mouse_move(w, hwnd, lparam, Some(wparam))),
        WM_NCMOUSEMOVE => {
            mouse_move(w, hwnd, lparam, None);
            None
        }
        WM_MOUSELEAVE | WM_NCMOUSELEAVE => handled(mouse_leave(w, hwnd, msg == WM_NCMOUSELEAVE)),
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK | WM_RBUTTONDOWN | WM_RBUTTONDBLCLK | WM_MBUTTONDOWN => {
            let button = match msg {
                WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => MouseButton::Left,
                WM_RBUTTONDOWN | WM_RBUTTONDBLCLK => MouseButton::Right,
                _ => MouseButton::Middle,
            };
            unsafe { SetCapture(hwnd) };
            let click_count = {
                let mut input = w.input.borrow_mut();
                input.buttons |= button.flag();
                input.clicks.record(button, lparam_xy(lparam))
            };
            handled(w.send(WindowEvent::MouseDown {
                button,
                position: w.client_point(lparam),
                modifiers: Modifiers::current(),
                click_count,
            }))
        }
        WM_LBUTTONUP | WM_RBUTTONUP | WM_MBUTTONUP => {
            let button = match msg {
                WM_LBUTTONUP => MouseButton::Left,
                WM_RBUTTONUP => MouseButton::Right,
                _ => MouseButton::Middle,
            };
            let released_all = {
                let mut input = w.input.borrow_mut();
                input.buttons &= !button.flag();
                input.buttons == 0
            };
            if released_all {
                release_capture_suppressed(w, || {
                    let _ = unsafe { ReleaseCapture() };
                });
            }
            handled(w.send(WindowEvent::MouseUp {
                button,
                position: w.client_point(lparam),
            }))
        }
        WM_CAPTURECHANGED => {
            let lost = {
                let mut input = w.input.borrow_mut();
                if !input.releasing_capture {
                    input.buttons = 0;
                }
                !input.releasing_capture
            };
            handled(lost && w.send(WindowEvent::CaptureLost))
        }
        WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
            let modifiers = Modifiers::current();
            let horizontal = msg == WM_MOUSEHWHEEL || modifiers.shift;
            let notches = f32::from(((wparam.0 >> 16) & 0xFFFF) as i16) / 120.0;
            let setting = if horizontal {
                SPI_GETWHEELSCROLLCHARS
            } else {
                SPI_GETWHEELSCROLLLINES
            };
            let per_notch = system_parameters_u32(setting.0).unwrap_or(DEFAULT_WHEEL_STEP);
            // A vertical setting of u32::MAX (WHEEL_PAGESCROLL) scrolls by pages.
            let page_scroll = !horizontal && per_notch == u32::MAX;
            let step = if page_scroll { 1 } else { per_notch.max(1) };
            let lines = notches * step as f32;
            let delta_lines = match (horizontal, msg == WM_MOUSEHWHEEL) {
                (false, _) => point(0.0, lines),
                (true, true) => point(-lines, 0.0),
                (true, false) => point(lines, 0.0),
            };
            handled(w.send(WindowEvent::ScrollWheel {
                position: screen_to_client(hwnd, lparam, w.scale.get()),
                delta_lines,
                page_scroll,
                modifiers,
            }))
        }

        // The keyboard and text.
        WM_KEYDOWN | WM_SYSKEYDOWN | WM_KEYUP | WM_SYSKEYUP => key(w, msg, wparam, lparam),
        // Alt+Space opens the system menu; other Alt characters are the app's.
        WM_SYSCHAR => ((wparam.0 & 0xFFFF) != usize::from(b' ')).then_some(LRESULT(0)),
        WM_CHAR => {
            let unit = (wparam.0 & 0xFFFF) as u16;
            let text = {
                let mut input = w.input.borrow_mut();
                if std::mem::take(&mut input.swallow_char) {
                    None
                } else if (0xD800..=0xDBFF).contains(&unit) {
                    input.high_surrogate = Some(unit);
                    None
                } else if (0xDC00..=0xDFFF).contains(&unit) {
                    let high = input.high_surrogate.take();
                    high.and_then(|high| String::from_utf16(&[high, unit]).ok())
                } else {
                    input.high_surrogate = None;
                    let printable = unit >= 0x20 && unit != 0x7F;
                    printable
                        .then(|| char::from_u32(u32::from(unit)))
                        .flatten()
                        .map(String::from)
                }
            };
            handled(text.is_some_and(|text| w.send(WindowEvent::TextInput(text))))
        }
        WM_IME_STARTCOMPOSITION => handled(place_ime(w, hwnd)),
        WM_IME_COMPOSITION => handled(ime_composition(w, hwnd, lparam.0 as u32)),
        WM_IME_ENDCOMPOSITION => handled(w.send(WindowEvent::ImeComposition {
            text: String::new(),
            cursor: 0,
        })),

        // Size, position, DPI, activation and settings.
        WM_SIZE => {
            let (width, height) = (
                (lparam.0 & 0xFFFF) as u32,
                ((lparam.0 >> 16) & 0xFFFF) as u32,
            );
            let resized = width > 0 && height > 0 && {
                w.physical.set((width, height));
                w.send(WindowEvent::Resized)
            };
            handled(resized)
        }
        WM_MOVE => {
            let origin = Some(lparam_xy(lparam));
            handled(w.origin.replace(origin) != origin && w.send(WindowEvent::Moved))
        }
        WM_DPICHANGED => {
            // The new scale is in place before the move below, so the WM_SIZE it sends uses it.
            w.scale.set(scale_for_dpi((wparam.0 & 0xFFFF) as u32));
            let r = unsafe { &*(lparam.0 as *const RECT) };
            let flags = SWP_NOZORDER | SWP_NOACTIVATE;
            let _ = unsafe {
                SetWindowPos(
                    hwnd,
                    None,
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    flags,
                )
            };
            let (width, height) = client_size(hwnd);
            w.physical.set((width, height));
            handled(w.send(WindowEvent::Resized))
        }
        WM_ACTIVATE => {
            let active = (wparam.0 & 0xFFFF) != 0;
            w.active.set(active);
            handled(w.send(WindowEvent::Activated(active)))
        }
        WM_SETTINGCHANGE | WM_THEMECHANGED => {
            if msg == WM_THEMECHANGED || is_relevant_setting(lparam) {
                deliver(Some(
                    PlatformEvent::SettingsChanged(SystemSettings::query()),
                ));
            }
            Some(LRESULT(0))
        }

        // Windows' modal move/resize and menu loops: the hover ends, and a timer keeps the app
        // drawing until the loop does.
        WM_ENTERSIZEMOVE | WM_ENTERMENULOOP => {
            w.in_size_move.set(true);
            if std::mem::take(&mut w.input.borrow_mut().hovered) {
                w.send(WindowEvent::MouseLeave);
            }
            w.send(WindowEvent::SizeMoveChanged(true));
            let _ = unsafe { SetTimer(Some(hwnd), SIZE_MOVE_TIMER_ID, USER_TIMER_MINIMUM, None) };
            None
        }
        WM_TIMER if wparam.0 == SIZE_MOVE_TIMER_ID => handled(deliver(Some(PlatformEvent::Wake))),
        WM_EXITSIZEMOVE | WM_EXITMENULOOP => {
            let _ = unsafe { KillTimer(Some(hwnd), SIZE_MOVE_TIMER_ID) };
            w.in_size_move.set(false);
            w.send(WindowEvent::SizeMoveChanged(false));
            None
        }

        // Shutdown, dropped files, closing.
        WM_QUERYENDSESSION => match LOOP.with(|l| l.borrow().shutdown_block) {
            Some(reason) => {
                let _ = unsafe { ShutdownBlockReasonCreate(hwnd, &HSTRING::from(reason)) };
                handled(w.send(WindowEvent::CloseRequested))
            }
            None => {
                let _ = unsafe { ShutdownBlockReasonDestroy(hwnd) };
                Some(LRESULT(1))
            }
        },
        WM_ENDSESSION => {
            let ending = wparam.0 != 0;
            if !ending {
                let _ = unsafe { ShutdownBlockReasonDestroy(hwnd) };
            }
            handled(deliver(Some(PlatformEvent::SessionEnding(ending))))
        }
        WM_DROPFILES => {
            let paths = dropped_paths(HDROP(wparam.0 as *mut _));
            handled(!paths.is_empty() && w.send(WindowEvent::FilesDropped(paths)))
        }
        WM_CLOSE => handled(w.send(WindowEvent::CloseRequested)),
        _ => None,
    }
}

/// `WM_NCCALCSIZE`: the client area takes the title bar, keeping the default side and bottom
/// borders; maximized, the top border stays off screen.
fn calc_frame(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let params = unsafe { &mut *(lparam.0 as *mut NCCALCSIZE_PARAMS) };
    let top = params.rgrc[0].top;
    let result = unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    params.rgrc[0].top = top;
    if unsafe { IsZoomed(hwnd) }.as_bool() {
        params.rgrc[0].top += frame_thickness(window_dpi(hwnd)).1;
    }
    result
}

/// What part of the frame a client-area point is: the top resize border (which the client area
/// covers), or a caption area the app declared.
fn hit_test(w: &WindowState, hwnd: HWND, lparam: LPARAM) -> u32 {
    if !unsafe { IsZoomed(hwnd) }.as_bool() {
        let mut rc = RECT::default();
        let _ = unsafe { GetWindowRect(hwnd, &mut rc) };
        let (x, y) = lparam_xy(lparam);
        let (bx, by) = frame_thickness(window_dpi(hwnd));
        if y < rc.top + by {
            return if x < rc.left + bx * 2 {
                HTTOPLEFT
            } else if x >= rc.right - bx * 2 {
                HTTOPRIGHT
            } else {
                HTTOP
            };
        }
    }
    let local = screen_to_client(hwnd, lparam, w.scale.get());
    match hit_test_control_areas(&w.control_areas.borrow(), local) {
        WindowControlArea::Close => HTCLOSE,
        WindowControlArea::Max => HTMAXBUTTON,
        WindowControlArea::Min => HTMINBUTTON,
        WindowControlArea::Drag => HTCAPTION,
        WindowControlArea::Client => HTCLIENT,
    }
}

/// A move over the client area (with the buttons held, from `wparam`) or the frame (`None`).
fn mouse_move(w: &WindowState, hwnd: HWND, lparam: LPARAM, client: Option<WPARAM>) -> bool {
    if w.in_size_move.get() {
        return false;
    }
    let in_client = client.is_some();
    let (position, pressed_button, start_tracking) = {
        let mut input = w.input.borrow_mut();
        let position = match client {
            Some(wparam) => {
                input.buttons = wparam.0 as u32 & (0x1 | 0x2 | 0x10);
                w.client_point(lparam)
            }
            None => screen_to_client(hwnd, lparam, w.scale.get()),
        };
        input.hovered = true;
        let start_tracking = !std::mem::replace(input.tracking_leave(in_client), true);
        (position, MouseButton::first(input.buttons), start_tracking)
    };
    if start_tracking {
        let mut tme = TRACKMOUSEEVENT {
            cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
            dwFlags: if in_client {
                TME_LEAVE
            } else {
                TME_LEAVE | TME_NONCLIENT
            },
            hwndTrack: hwnd,
            dwHoverTime: 0,
        };
        if unsafe { TrackMouseEvent(&mut tme) }.is_err() {
            *w.input.borrow_mut().tracking_leave(in_client) = false;
        }
    }
    w.send(WindowEvent::MouseMove {
        position,
        pressed_button,
    })
}

/// The mouse left the client area or the frame: it has left the window once it's off both.
fn mouse_leave(w: &WindowState, hwnd: HWND, frame: bool) -> bool {
    let still_tracked = {
        let mut input = w.input.borrow_mut();
        *input.tracking_leave(!frame) = false;
        if frame {
            input.caption_button = None;
        }
        *input.tracking_leave(frame)
    };
    let mut cursor = POINT::default();
    if still_tracked
        || unsafe { GetCursorPos(&mut cursor).is_ok() && WindowFromPoint(cursor) == hwnd }
    {
        return false;
    }
    w.input.borrow_mut().hovered = false;
    w.send(WindowEvent::MouseLeave)
}

/// Runs `release`, which releases mouse capture, without reporting that as capture lost.
fn release_capture_suppressed(w: &WindowState, release: impl FnOnce()) {
    w.input.borrow_mut().releasing_capture = true;
    release();
    w.input.borrow_mut().releasing_capture = false;
}

/// A key press or release. Keys the app uses don't type their character; Alt combinations
/// other than Alt+F4 and Alt+Space don't reach Windows' menu handling.
fn key(w: &WindowState, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
    let down = matches!(msg, WM_KEYDOWN | WM_SYSKEYDOWN);
    let vk = (wparam.0 & 0xFFFF) as u16;
    let modifiers = Modifiers::current();
    let changed = {
        let mut input = w.input.borrow_mut();
        input.swallow_char = false;
        std::mem::replace(&mut input.modifiers, modifiers) != modifiers
    };
    if changed {
        deliver(Some(PlatformEvent::ModifiersChanged(modifiers)));
    }
    let mut handled = false;
    if let Some(key) = Key::from_vk(vk, modifiers) {
        let modifiers = Modifiers {
            alt: modifiers.alt && key != Key::Alt,
            ..modifiers
        };
        let event = KeyEvent { key, modifiers };
        let repeat = lparam.0 & (1 << 30) != 0;
        if !down {
            w.send(WindowEvent::KeyUp(event));
        } else if repeat && matches!(key, Key::Alt | Key::F(10)) {
            handled = true;
        } else {
            handled = w.send(WindowEvent::KeyDown(event));
            w.input.borrow_mut().swallow_char = handled;
        }
    }
    let menu_key = msg == WM_SYSKEYDOWN && vk != VK_F4.0 && vk != VK_SPACE.0;
    (down && handled || menu_key || msg == WM_SYSKEYUP).then_some(LRESULT(0))
}

/// Puts the IME's composition and candidate windows at the caret.
fn place_ime(w: &WindowState, hwnd: HWND) -> bool {
    let Some(caret) = w.ime_caret.get() else {
        return false;
    };
    let scale = w.scale.get();
    let at = POINT {
        x: (caret.origin.x * scale).round() as i32,
        y: (caret.bottom() * scale).round() as i32,
    };
    let himc = unsafe { ImmGetContext(hwnd) };
    if himc.is_invalid() {
        return false;
    }
    unsafe {
        let composition = COMPOSITIONFORM {
            dwStyle: CFS_POINT,
            ptCurrentPos: at,
            rcArea: RECT::default(),
        };
        let candidates = CANDIDATEFORM {
            dwIndex: 0,
            dwStyle: CFS_CANDIDATEPOS,
            ptCurrentPos: at,
            rcArea: RECT::default(),
        };
        let _ = ImmSetCompositionWindow(himc, &composition);
        let _ = ImmSetCandidateWindow(himc, &candidates);
        let _ = ImmReleaseContext(hwnd, himc);
    }
    false
}

/// The IME committed text, or changed the text being composed.
fn ime_composition(w: &WindowState, hwnd: HWND, flags: u32) -> bool {
    let himc = unsafe { ImmGetContext(hwnd) };
    if himc.is_invalid() {
        return false;
    }
    if flags & GCS_RESULTSTR.0 != 0
        && let Some(text) = ime_string(himc, GCS_RESULTSTR)
    {
        w.send(WindowEvent::TextInput(text));
    }
    if flags & GCS_COMPSTR.0 != 0 {
        let text = ime_string(himc, GCS_COMPSTR).unwrap_or_default();
        let cursor =
            unsafe { ImmGetCompositionStringW(himc, GCS_CURSORPOS, None, 0) }.max(0) as usize;
        w.send(WindowEvent::ImeComposition { text, cursor });
    }
    let _ = unsafe { ImmReleaseContext(hwnd, himc) };
    place_ime(w, hwnd)
}

fn ime_string(himc: HIMC, which: IME_COMPOSITION_STRING) -> Option<String> {
    let bytes = unsafe { ImmGetCompositionStringW(himc, which, None, 0) };
    if bytes <= 0 {
        return None;
    }
    let mut buf = vec![0u16; bytes as usize / 2];
    let written = unsafe {
        ImmGetCompositionStringW(himc, which, Some(buf.as_mut_ptr().cast()), bytes as u32)
    };
    (written > 0)
        .then(|| String::from_utf16(&buf[..written as usize / 2]).ok())
        .flatten()
}

/// Whether a `WM_SETTINGCHANGE` is about something the app shows (theme, metrics, contrast).
fn is_relevant_setting(lparam: LPARAM) -> bool {
    if lparam.0 == 0 {
        return true;
    }
    let name = PCWSTR(lparam.0 as *const u16);
    let name = String::from_utf16_lossy(unsafe { name.as_wide() });
    ["ImmersiveColorSet", "WindowMetrics", "HighContrast"]
        .iter()
        .any(|setting| name.eq_ignore_ascii_case(setting))
}

/// The files dropped on a window (and the drop released).
fn dropped_paths(hdrop: HDROP) -> Vec<PathBuf> {
    let count = unsafe { DragQueryFileW(hdrop, u32::MAX, None) };
    let paths = (0..count)
        .filter_map(|ix| {
            let len = unsafe { DragQueryFileW(hdrop, ix, None) } as usize;
            let mut buf = vec![0u16; len + 1];
            let written = unsafe { DragQueryFileW(hdrop, ix, Some(buf.as_mut_slice())) } as usize;
            (len > 0 && written > 0).then(|| PathBuf::from(OsString::from_wide(&buf[..written])))
        })
        .collect();
    unsafe { DragFinish(hdrop) };
    paths
}

/// A window's handle, for the operations the app runs on its own and other windows.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct WindowHandle(isize);

/// A window's saved position, size and state (restored, maximized, minimized).
#[derive(Copy, Clone)]
pub(crate) struct WindowPlacement(WINDOWPLACEMENT);

impl WindowHandle {
    pub(crate) fn raw(self) -> HWND {
        HWND(self.0 as *mut _)
    }

    pub(crate) fn client_origin(self) -> ScreenPoint {
        let mut origin = POINT::default();
        let _ = unsafe { ClientToScreen(self.raw(), &mut origin) };
        origin.into()
    }

    fn dpi_scale(self) -> f32 {
        window_scale(self.raw())
    }

    /// `pt` in the client area's logical coordinates.
    pub(crate) fn to_client(self, pt: ScreenPoint) -> Point {
        let (origin, scale) = (self.client_origin(), self.dpi_scale());
        point(
            (pt.x - origin.x) as f32 / scale,
            (pt.y - origin.y) as f32 / scale,
        )
    }

    /// The screen point at `pos` in the client area's logical coordinates.
    pub(crate) fn to_screen(self, pos: Point) -> ScreenPoint {
        let (origin, scale) = (self.client_origin(), self.dpi_scale());
        ScreenPoint {
            x: origin.x + (pos.x * scale).round() as i32,
            y: origin.y + (pos.y * scale).round() as i32,
        }
    }

    pub(crate) fn window_rect(self) -> ScreenRect {
        let mut rect = RECT::default();
        let _ = unsafe { GetWindowRect(self.raw(), &mut rect) };
        rect.into()
    }

    pub(crate) fn is_maximized(self) -> bool {
        unsafe { IsZoomed(self.raw()) }.as_bool()
    }

    pub(crate) fn is_enabled(self) -> bool {
        unsafe { IsWindowEnabled(self.raw()) }.as_bool()
    }

    pub(crate) fn restore(self) {
        let _ = unsafe { ShowWindow(self.raw(), SW_RESTORE) };
    }

    pub(crate) fn hide(self) {
        let _ = unsafe { ShowWindow(self.raw(), SW_HIDE) };
    }

    pub(crate) fn show_noactivate(self) {
        let _ = unsafe { ShowWindow(self.raw(), SW_SHOWNOACTIVATE) };
    }

    pub(crate) fn set_foreground(self) {
        let _ = unsafe { SetForegroundWindow(self.raw()) };
    }

    /// Restores the window if it's minimized and brings it to the front.
    pub(crate) fn bring_to_front(self) {
        if unsafe { IsIconic(self.raw()) }.as_bool() {
            self.restore();
        }
        self.set_foreground();
    }

    pub(crate) fn place_topmost_noactivate(self, rect: ScreenRect) {
        let (w, h) = (rect.right - rect.left, rect.bottom - rect.top);
        let _ = unsafe {
            SetWindowPos(
                self.raw(),
                Some(HWND_TOPMOST),
                rect.left,
                rect.top,
                w,
                h,
                SWP_NOACTIVATE,
            )
        };
    }

    pub(crate) fn move_noactivate(self, x: i32, y: i32) {
        let flags = SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE;
        let _ = unsafe { SetWindowPos(self.raw(), None, x, y, 0, 0, flags) };
    }

    pub(crate) fn set_capture(self) {
        unsafe { SetCapture(self.raw()) };
    }

    /// Hands the window to Windows' own move loop, as if its caption were dragged from `point`.
    pub(crate) fn begin_system_move(self, point: ScreenPoint) {
        let at = LPARAM((((point.y as u16 as u32) << 16) | point.x as u16 as u32) as isize);
        let command = WPARAM(SC_MOVE as usize | HTCAPTION as usize);
        unsafe {
            let _ = ReleaseCapture();
            let _ = PostMessageW(Some(self.raw()), WM_SYSCOMMAND, command, at);
        }
    }

    /// Ends whatever modal loop (moving, menus) the window is in.
    pub(crate) fn cancel_mode(self) {
        let _ = unsafe { PostMessageW(Some(self.raw()), WM_CANCELMODE, WPARAM(0), LPARAM(0)) };
    }

    pub(crate) fn in_move_loop(self) -> bool {
        let mut info = GUITHREADINFO {
            cbSize: size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        unsafe {
            let thread = GetWindowThreadProcessId(self.raw(), None);
            GetGUIThreadInfo(thread, &mut info).is_ok() && (info.flags & GUI_INMOVESIZE).0 != 0
        }
    }

    pub(crate) fn saved_placement(self) -> Option<WindowPlacement> {
        let mut placement = WINDOWPLACEMENT {
            length: size_of::<WINDOWPLACEMENT>() as u32,
            ..Default::default()
        };
        unsafe { GetWindowPlacement(self.raw(), &mut placement) }
            .is_ok()
            .then_some(WindowPlacement(placement))
    }

    pub(crate) fn restore_placement(self, placement: WindowPlacement) {
        let _ = unsafe { SetWindowPlacement(self.raw(), &placement.0) };
    }

    /// The window's size when not maximized or minimized.
    fn restored_logical_size(self) -> Option<Size> {
        let rc = self.saved_placement()?.0.rcNormalPosition;
        let scale = self.dpi_scale();
        let (w, h) = (
            (rc.right - rc.left).max(1) as f32,
            (rc.bottom - rc.top).max(1) as f32,
        );
        Some(size(w / scale, h / scale))
    }

    /// Commits any text being composed in the window's IME.
    pub(crate) fn complete_ime_composition(self) {
        let himc = unsafe { ImmGetContext(self.raw()) };
        if !himc.is_invalid() {
            unsafe {
                let _ = ImmNotifyIME(himc, NI_COMPOSITIONSTR, CPS_COMPLETE, 0);
                let _ = ImmReleaseContext(self.raw(), himc);
            }
        }
    }
}

// ---- 5. Displays and the desktop ------------------------------------------------------------

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

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct DisplayId(pub(crate) usize);

/// A monitor.
#[derive(Copy, Clone, Debug)]
pub(crate) struct WindowsDisplay {
    monitor: HMONITOR,
}

impl WindowsDisplay {
    pub(crate) fn primary() -> Option<Self> {
        let monitor = unsafe { MonitorFromPoint(POINT::default(), MONITOR_DEFAULTTOPRIMARY) };
        (!monitor.is_invalid()).then_some(Self { monitor })
    }

    /// The monitor showing `pt`, or the nearest one.
    pub(crate) fn from_point(pt: ScreenPoint) -> Self {
        Self {
            monitor: unsafe { MonitorFromPoint(pt.into(), MONITOR_DEFAULTTONEAREST) },
        }
    }

    fn from_id(id: DisplayId) -> Option<Self> {
        let monitor = HMONITOR(id.0 as *mut _);
        (!monitor.is_invalid()).then_some(Self { monitor })
    }

    pub(crate) fn id(&self) -> DisplayId {
        DisplayId(self.monitor.0 as usize)
    }

    pub(crate) fn scale_factor(&self) -> f32 {
        let (mut dpi_x, mut dpi_y) = (USER_DEFAULT_SCREEN_DPI, USER_DEFAULT_SCREEN_DPI);
        let _ =
            unsafe { GetDpiForMonitor(self.monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) };
        scale_for_dpi(dpi_x)
    }

    /// The monitor's area minus the taskbar, in pixels.
    fn work_rect(&self) -> Option<RECT> {
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        unsafe { GetMonitorInfoW(self.monitor, &mut info) }
            .as_bool()
            .then_some(info.rcWork)
    }

    pub(crate) fn work_area(&self) -> ScreenRect {
        self.work_rect().unwrap_or_default().into()
    }

    /// The work area in logical units.
    pub(crate) fn visible_bounds(&self) -> Rect {
        let Some(rc) = self.work_rect() else {
            return Rect::new(point(0.0, 0.0), size(1280.0, 800.0));
        };
        let scale = self.scale_factor();
        let at = |x: i32, y: i32| (x as f32 / scale, y as f32 / scale);
        let ((x, y), (w, h)) = (
            at(rc.left, rc.top),
            at(rc.right - rc.left, rc.bottom - rc.top),
        );
        Rect::new(point(x, y), size(w, h))
    }

    /// A rect of `target` size centered in the work area (pinned to its top left if larger).
    pub(crate) fn centered_rect(&self, target: Size) -> Rect {
        let vis = self.visible_bounds();
        let x = vis.origin.x + ((vis.size.width - target.width) * 0.5).max(0.0);
        let y = vis.origin.y + ((vis.size.height - target.height) * 0.5).max(0.0);
        Rect::new(point(x, y), target)
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
    let swapped = unsafe { GetSystemMetrics(SM_SWAPBUTTON) } != 0;
    let button = if swapped { VK_RBUTTON } else { VK_LBUTTON };
    (unsafe { GetAsyncKeyState(i32::from(button.0)) }) < 0
}

/// The topmost visible window (other than `exclude`) whose frame contains `point`.
pub(crate) fn top_visible_window_at(
    point: ScreenPoint,
    exclude: Option<WindowHandle>,
) -> Option<WindowHandle> {
    struct Search {
        point: POINT,
        exclude: Option<HWND>,
        found: Option<HWND>,
    }

    unsafe extern "system" fn visit(hwnd: HWND, data: LPARAM) -> BOOL {
        let search = unsafe { &mut *(data.0 as *mut Search) };
        let dwm_get = |attr, out: *mut c_void, len: usize| unsafe {
            DwmGetWindowAttribute(hwnd, attr, out, len as u32).is_ok()
        };
        let mut cloaked = 0u32;
        let shown = unsafe {
            IsWindowVisible(hwnd).as_bool()
                && !IsIconic(hwnd).as_bool()
                && GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TRANSPARENT.0 == 0
        } && !(dwm_get(DWMWA_CLOAKED, (&raw mut cloaked).cast(), size_of::<u32>())
            && cloaked != 0);
        if search.exclude == Some(hwnd) || !shown {
            return BOOL(1);
        }
        let mut frame = RECT::default();
        if !dwm_get(
            DWMWA_EXTENDED_FRAME_BOUNDS,
            (&raw mut frame).cast(),
            size_of::<RECT>(),
        ) {
            let _ = unsafe { GetWindowRect(hwnd, &mut frame) };
        }
        if unsafe { PtInRect(&frame, search.point) }.as_bool() {
            search.found = Some(hwnd);
            return BOOL(0);
        }
        BOOL(1)
    }

    let mut search = Search {
        point: point.into(),
        exclude: exclude.map(WindowHandle::raw),
        found: None,
    };
    let _ = unsafe { EnumWindows(Some(visit), LPARAM((&raw mut search) as isize)) };
    search.found.map(|hwnd| WindowHandle(hwnd.0 as isize))
}

// ---- 6. The single-instance handoff ---------------------------------------------------------
//
// The first launch for a user (separately for elevated launches) holds a named mutex and titles
// its wake window after it. A later launch that finds the mutex taken sends its files to that
// window and exits.

const HANDOFF_TAG: usize = 0x4A4F_5401;
const MAX_HANDOFF_BYTES: usize = 1024 * 1024;
const HANDOFF_TIMEOUT_MS: u32 = 2000;

/// Held by the first launch for as long as it takes other launches' files.
pub(crate) struct InstanceLock {
    _mutex: Owned<HANDLE>,
    window_title: String,
}

pub(crate) enum Arbitration {
    /// This is the first launch (without a lock if the check couldn't be made).
    Primary(Option<InstanceLock>),
    HandedOff,
}

pub(crate) fn arbitrate_instance(paths: &[PathBuf]) -> Arbitration {
    match instance_key() {
        Some(key) => arbitrate_for(&key, paths),
        None => Arbitration::Primary(None),
    }
}

/// The user's SID, tagged with whether this launch is elevated: Windows won't pass messages
/// from a normal process to an elevated one, so each gets its own first launch.
fn instance_key() -> Option<String> {
    let mut raw = HANDLE::default();
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw).ok()? };
    let token = unsafe { Owned::new(raw) };
    let mut len = 0u32;
    let _ = unsafe { GetTokenInformation(*token, TokenUser, None, 0, &mut len) };
    let mut user = vec![0u8; len as usize];
    unsafe {
        GetTokenInformation(
            *token,
            TokenUser,
            Some(user.as_mut_ptr().cast()),
            len,
            &mut len,
        )
        .ok()?
    };
    if user.len() < size_of::<TOKEN_USER>() {
        return None;
    }
    let sid = unsafe { std::ptr::read_unaligned(user.as_ptr().cast::<TOKEN_USER>()) }
        .User
        .Sid;
    let mut text = PWSTR::null();
    unsafe { ConvertSidToStringSidW(sid, &mut text).ok()? };
    let _free = unsafe { Owned::new(HLOCAL(text.0.cast())) };
    let sid = String::from_utf16_lossy(unsafe { text.as_wide() });

    let mut elevation = TOKEN_ELEVATION::default();
    let elevated = unsafe {
        let len = size_of::<TOKEN_ELEVATION>() as u32;
        GetTokenInformation(
            *token,
            TokenElevation,
            Some((&raw mut elevation).cast()),
            len,
            &mut 0,
        )
    }
    .is_ok()
        && elevation.TokenIsElevated != 0;
    Some(format!("{}.{sid}", if elevated { "elev" } else { "std" }))
}

fn arbitrate_for(key: &str, paths: &[PathBuf]) -> Arbitration {
    let mutex_name = HSTRING::from(format!(r"Local\Jot.Mutex.{key}"));
    let window_title = format!("Jot.Handoff.{key}");
    // The first launch may have claimed the mutex but not yet titled its window: try a while.
    for _ in 0..20 {
        let Ok(handle) = (unsafe { CreateMutexW(None, false, &mutex_name) }) else {
            return Arbitration::Primary(None);
        };
        let taken = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        let mutex = unsafe { Owned::new(handle) };
        if !taken {
            return Arbitration::Primary(Some(InstanceLock {
                _mutex: mutex,
                window_title,
            }));
        }
        drop(mutex);
        if send_handoff(&window_title, paths) {
            return Arbitration::HandedOff;
        }
        thread::sleep(Duration::from_millis(25));
    }
    Arbitration::Primary(None)
}

/// Sends `paths` to the first launch's wake window; whether it took them.
fn send_handoff(window_title: &str, paths: &[PathBuf]) -> bool {
    let title = HSTRING::from(window_title);
    let target = unsafe { FindWindowExW(Some(HWND_MESSAGE), None, WAKE_CLASS, &title) };
    let Some(target) = target.ok().filter(|hwnd| !hwnd.is_invalid()) else {
        return false;
    };
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(target, Some(&mut pid)) };
    // Let the first launch bring its window to the front.
    let _ = unsafe { AllowSetForegroundWindow(pid) };

    let payload = encode_paths(paths);
    if payload.len() > MAX_HANDOFF_BYTES {
        return false;
    }
    let cds = COPYDATASTRUCT {
        dwData: HANDOFF_TAG,
        cbData: payload.len() as u32,
        lpData: if payload.is_empty() {
            std::ptr::null_mut()
        } else {
            payload.as_ptr() as *mut c_void
        },
    };
    let mut result = 0usize;
    let sent = unsafe {
        SendMessageTimeoutW(
            target,
            WM_COPYDATA,
            WPARAM(0),
            LPARAM((&raw const cds) as isize),
            SMTO_ABORTIFHUNG | SMTO_BLOCK,
            HANDOFF_TIMEOUT_MS,
            Some(&mut result),
        )
    };
    sent.0 != 0 && result == 1
}

/// Absolute paths as UTF-16, separated by NULs.
fn encode_paths(paths: &[PathBuf]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for (ix, path) in paths.iter().enumerate() {
        if ix > 0 {
            bytes.extend([0, 0]);
        }
        let path = std::path::absolute(path).unwrap_or_else(|_| path.clone());
        bytes.extend(path.as_os_str().encode_wide().flat_map(u16::to_le_bytes));
    }
    bytes
}

/// The paths another launch sent, if the message is a well-formed handoff.
fn decode_paths(cds: &COPYDATASTRUCT) -> Option<Vec<PathBuf>> {
    let len = cds.cbData as usize;
    if cds.dwData != HANDOFF_TAG {
        return None;
    }
    if len == 0 {
        return Some(Vec::new());
    }
    if len > MAX_HANDOFF_BYTES || !len.is_multiple_of(2) || cds.lpData.is_null() {
        return None;
    }
    let bytes = unsafe { std::slice::from_raw_parts(cds.lpData.cast::<u8>(), len) };
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&b| u16::from_le_bytes(b))
        .collect();
    let paths: Vec<PathBuf> = units
        .split(|&u| u == 0)
        .filter(|part| !part.is_empty())
        .map(|part| PathBuf::from(OsString::from_wide(part)))
        .collect();
    (!paths.is_empty()).then_some(paths)
}

// ---- 7. System settings ---------------------------------------------------------------------

/// Lines (or characters, sideways) per wheel notch when the system doesn't say.
const DEFAULT_WHEEL_STEP: u32 = 3;
const DEFAULT_HOVER_TIME_MS: u32 = 400;
const DEFAULT_MENU_SHOW_DELAY_MS: u32 = 400;

/// The high-contrast theme's colors, as 0xRRGGBB.
#[derive(Copy, Clone, Debug)]
pub(crate) struct HighContrastColors {
    pub(crate) window: u32,
    pub(crate) window_text: u32,
    pub(crate) highlight: u32,
    pub(crate) highlight_text: u32,
    pub(crate) gray_text: u32,
    pub(crate) btn_face: u32,
    pub(crate) btn_text: u32,
    pub(crate) window_frame: u32,
    pub(crate) hotlight: u32,
}

/// The user's settings that change how Jot looks and behaves.
#[derive(Copy, Clone, Debug)]
pub(crate) struct SystemSettings {
    /// Apps are set to the dark theme.
    pub(crate) dark: bool,
    /// The accent colour for the light theme and for the dark one.
    pub(crate) accent: [Option<u32>; 2],
    pub(crate) high_contrast: Option<HighContrastColors>,
    pub(crate) reduce_motion: bool,
    pub(crate) caret_blink_interval: Option<Duration>,
    pub(crate) caret_blink_timeout: Option<Duration>,
    pub(crate) caret_width_px: u32,
    pub(crate) mouse_hover_time: Duration,
    pub(crate) menu_show_delay: Duration,
}

impl SystemSettings {
    pub(crate) fn query() -> Self {
        const SPI_GETCARETTIMEOUT: u32 = 0x2022;
        let ms = |action: u32, default: u32| {
            Duration::from_millis(system_parameters_u32(action).unwrap_or(default).into())
        };
        let blink = |ms: u32| (ms != 0 && ms != u32::MAX).then(|| Duration::from_millis(ms.into()));
        Self {
            dark: apps_use_dark(),
            accent: accent_colors(),
            high_contrast: high_contrast_colors(),
            reduce_motion: system_parameters_u32(SPI_GETCLIENTAREAANIMATION.0) == Some(0),
            caret_blink_interval: blink(unsafe { GetCaretBlinkTime() }),
            caret_blink_timeout: blink(system_parameters_u32(SPI_GETCARETTIMEOUT).unwrap_or(5_000)),
            caret_width_px: system_parameters_u32(SPI_GETCARETWIDTH.0)
                .unwrap_or(1)
                .clamp(1, 20),
            mouse_hover_time: ms(SPI_GETMOUSEHOVERTIME.0, DEFAULT_HOVER_TIME_MS),
            menu_show_delay: ms(SPI_GETMENUSHOWDELAY.0, DEFAULT_MENU_SHOW_DELAY_MS),
        }
    }
}

/// Reads registry value `name` under the current user's `key` into `out`; the bytes read.
fn read_user_registry(
    key: PCWSTR,
    name: PCWSTR,
    kind: windows::Win32::System::Registry::REG_ROUTINE_FLAGS,
    out: &mut [u8],
) -> Option<usize> {
    let mut len = out.len() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key,
            name,
            kind,
            None,
            Some(out.as_mut_ptr().cast()),
            Some(&mut len),
        )
    };
    status.is_ok().then_some(len as usize)
}

fn apps_use_dark() -> bool {
    let mut light = [0u8; 4];
    let read = read_user_registry(
        w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"),
        w!("AppsUseLightTheme"),
        RRF_RT_REG_DWORD,
        &mut light,
    );
    read.is_some() && u32::from_ne_bytes(light) == 0
}

/// The accent colors for light and dark backgrounds (0xRRGGBB), from the accent palette.
fn accent_colors() -> [Option<u32>; 2] {
    let mut shades = [0u8; 32];
    let Some(len) = read_user_registry(
        w!(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Accent"),
        w!("AccentPalette"),
        RRF_RT_REG_BINARY,
        &mut shades,
    ) else {
        return [None; 2];
    };
    let shade = |at: usize| {
        (len >= at + 3).then(|| u32::from_be_bytes([0, shades[at], shades[at + 1], shades[at + 2]]))
    };
    [shade(16), shade(4)]
}

fn high_contrast_colors() -> Option<HighContrastColors> {
    let mut hc = HIGHCONTRASTW {
        cbSize: size_of::<HIGHCONTRASTW>() as u32,
        ..Default::default()
    };
    let flags = SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0);
    unsafe {
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            hc.cbSize,
            Some((&raw mut hc).cast()),
            flags,
        )
    }
    .ok()?;
    if (hc.dwFlags & HCF_HIGHCONTRASTON).0 == 0 {
        return None;
    }
    let color = |idx: SYS_COLOR_INDEX| {
        let c = unsafe { GetSysColor(idx) }; // 0x00BBGGRR
        (c & 0xFF) << 16 | (c & 0xFF00) | (c >> 16) & 0xFF
    };
    Some(HighContrastColors {
        window: color(COLOR_WINDOW),
        window_text: color(COLOR_WINDOWTEXT),
        highlight: color(COLOR_HIGHLIGHT),
        highlight_text: color(COLOR_HIGHLIGHTTEXT),
        gray_text: color(COLOR_GRAYTEXT),
        btn_face: color(COLOR_BTNFACE),
        btn_text: color(COLOR_BTNTEXT),
        window_frame: color(COLOR_WINDOWFRAME),
        hotlight: color(COLOR_HOTLIGHT),
    })
}

// ---- 8. OS services -------------------------------------------------------------------------

// File dialogs.

const ENCODING_COMBO_ID: u32 = 1001;

/// The open dialog, which picks one or more files.
#[derive(Clone, Debug)]
pub(crate) struct OpenDialogOptions {
    pub(crate) directory: PathBuf,
    pub(crate) file_types: Vec<(SharedStr, SharedStr)>,
}

/// A drop-down added to the save dialog (the encoding).
#[derive(Clone, Debug)]
pub(crate) struct SaveChoice {
    pub(crate) label: SharedStr,
    pub(crate) items: Vec<SharedStr>,
    pub(crate) selected: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct SaveDialogOptions {
    pub(crate) title: SharedStr,
    pub(crate) directory: PathBuf,
    pub(crate) file_name: String,
    pub(crate) file_types: Vec<(SharedStr, SharedStr)>,
    pub(crate) default_extension: SharedStr,
    pub(crate) choice: SaveChoice,
}

/// The chosen path and drop-down item, or `None` if cancelled.
pub(crate) type SaveDialogResult = Result<Option<(PathBuf, Option<usize>)>>;

/// OLE for the calling thread (the dialogs run on their own threads), released on drop.
struct Ole(bool);

impl Ole {
    fn init() -> Self {
        Self(unsafe { OleInitialize(None) }.is_ok())
    }
}

impl Drop for Ole {
    fn drop(&mut self) {
        if self.0 {
            unsafe { OleUninitialize() };
        }
    }
}

fn shell_item_path(item: &IShellItem) -> Result<PathBuf> {
    let name = unsafe { item.GetDisplayName(SIGDN_FILESYSPATH)? };
    let path = PathBuf::from(OsString::from_wide(unsafe { name.as_wide() }));
    unsafe { CoTaskMemFree(Some(name.0.cast())) };
    Ok(path)
}

/// Sets the folder and file types, shows the dialog; whether something was chosen.
fn show_dialog(
    dialog: &IFileDialog,
    owner: WindowHandle,
    directory: &Path,
    file_types: &[(SharedStr, SharedStr)],
) -> Result<bool> {
    if !directory.as_os_str().is_empty()
        && let Ok(folder) = unsafe {
            SHCreateItemFromParsingName::<_, _, IShellItem>(
                &HSTRING::from(directory.as_os_str()),
                None,
            )
        }
    {
        let _ = unsafe { dialog.SetFolder(&folder) };
    }
    let wide: Vec<(HSTRING, HSTRING)> = file_types
        .iter()
        .map(|(label, spec)| (HSTRING::from(label.as_str()), HSTRING::from(spec.as_str())))
        .collect();
    let specs: Vec<COMDLG_FILTERSPEC> = wide
        .iter()
        .map(|(label, spec)| COMDLG_FILTERSPEC {
            pszName: PCWSTR(label.as_ptr()),
            pszSpec: PCWSTR(spec.as_ptr()),
        })
        .collect();
    unsafe { dialog.SetFileTypes(&specs)? };
    match unsafe { dialog.Show(Some(owner.raw())) } {
        Ok(()) => Ok(true),
        Err(err) if err.code() == ERROR_CANCELLED.to_hresult() => Ok(false),
        Err(err) => Err(err.into()),
    }
}

pub(crate) fn run_open_dialog(
    owner: WindowHandle,
    options: OpenDialogOptions,
) -> Result<Option<Vec<PathBuf>>> {
    let _ole = Ole::init();
    unsafe {
        let dialog: IFileOpenDialog =
            CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
        let flags = FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST | FOS_ALLOWMULTISELECT;
        dialog.SetOptions(dialog.GetOptions()? | flags)?;
        if !show_dialog(&dialog, owner, &options.directory, &options.file_types)? {
            return Ok(None);
        }
        let items = dialog.GetResults()?;
        let paths = (0..items.GetCount()?).map(|ix| shell_item_path(&items.GetItemAt(ix)?));
        Ok(Some(paths.collect::<Result<_>>()?))
    }
}

pub(crate) fn run_save_dialog(owner: WindowHandle, options: SaveDialogOptions) -> SaveDialogResult {
    let _ole = Ole::init();
    unsafe {
        let dialog: IFileSaveDialog =
            CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER)?;
        dialog.SetOptions(
            dialog.GetOptions()? | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST | FOS_OVERWRITEPROMPT,
        )?;
        dialog.SetTitle(&HSTRING::from(options.title.as_str()))?;
        dialog.SetFileName(&HSTRING::from(options.file_name.as_str()))?;
        dialog.SetDefaultExtension(&HSTRING::from(options.default_extension.as_str()))?;
        let choice = &options.choice;
        let customize = dialog
            .cast::<IFileDialogCustomize>()
            .ok()
            .inspect(|custom| {
                let _ = custom.StartVisualGroup(
                    ENCODING_COMBO_ID + 100,
                    &HSTRING::from(choice.label.as_str()),
                );
                let _ = custom.AddComboBox(ENCODING_COMBO_ID);
                for (ix, item) in choice.items.iter().enumerate() {
                    let _ = custom.AddControlItem(
                        ENCODING_COMBO_ID,
                        ix as u32,
                        &HSTRING::from(item.as_str()),
                    );
                }
                let _ = custom.SetSelectedControlItem(ENCODING_COMBO_ID, choice.selected as u32);
                let _ = custom.EndVisualGroup();
            });
        if !show_dialog(&dialog, owner, &options.directory, &options.file_types)? {
            return Ok(None);
        }
        let path = shell_item_path(&dialog.GetResult()?)?;
        let selected = customize.and_then(|c| c.GetSelectedControlItem(ENCODING_COMBO_ID).ok());
        Ok(Some((path, selected.map(|id| id as usize))))
    }
}

// The clipboard.

const CLIPBOARD_OPEN_RETRIES: usize = 8;
const CLIPBOARD_RETRY_DELAY: Duration = Duration::from_millis(12);

/// The clipboard, opened (retrying briefly while another program has it) until dropped.
struct Clipboard;

impl Clipboard {
    fn open() -> Option<Self> {
        for attempt in 0..CLIPBOARD_OPEN_RETRIES {
            if unsafe { OpenClipboard(None) }.is_ok() {
                return Some(Self);
            }
            if attempt + 1 < CLIPBOARD_OPEN_RETRIES {
                thread::sleep(CLIPBOARD_RETRY_DELAY);
            }
        }
        None
    }
}

impl Drop for Clipboard {
    fn drop(&mut self) {
        let _ = unsafe { CloseClipboard() };
    }
}

pub(crate) fn clipboard_has_text() -> bool {
    unsafe { IsClipboardFormatAvailable(CF_UNICODETEXT.0 as u32) }.is_ok()
}

pub(crate) fn read_clipboard_text() -> Option<String> {
    let _clipboard = Clipboard::open()?;
    let global = HGLOBAL(unsafe { GetClipboardData(CF_UNICODETEXT.0 as u32) }.ok()?.0);
    let data = unsafe { GlobalLock(global) } as *const u16;
    if data.is_null() {
        return None;
    }
    let units = unsafe { std::slice::from_raw_parts(data, GlobalSize(global) / 2) };
    let text = String::from_utf16_lossy(
        &units[..units.iter().position(|&u| u == 0).unwrap_or(units.len())],
    );
    let _ = unsafe { GlobalUnlock(global) };
    Some(text)
}

/// Puts `text` on the clipboard with Windows line endings; whether that worked.
pub(crate) fn write_clipboard_text(text: &str) -> bool {
    let Some(_clipboard) = Clipboard::open() else {
        return false;
    };
    let wide: Vec<u16> = clipboard_text(text).encode_utf16().chain(Some(0)).collect();
    let set = || -> Result<()> {
        unsafe {
            EmptyClipboard()?;
            let global = Owned::new(GlobalAlloc(GMEM_MOVEABLE, size_of_val(wide.as_slice()))?);
            let data = GlobalLock(*global) as *mut u16;
            if data.is_null() {
                return Err(Error::new("GlobalLock returned null"));
            }
            std::ptr::copy_nonoverlapping(wide.as_ptr(), data, wide.len());
            let _ = GlobalUnlock(*global);
            SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(global.0)))?;
            // The clipboard owns the memory now.
            std::mem::forget(global);
        }
        Ok(())
    };
    set().is_ok()
}

/// `text` with lone LFs as CRLF and NULs (which would end it early) as spaces.
fn clipboard_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + text.bytes().filter(|&b| b == b'\n').count());
    let mut after_cr = false;
    for ch in text.chars() {
        match ch {
            '\0' => out.push(' '),
            '\n' if !after_cr => out.push_str("\r\n"),
            ch => out.push(ch),
        }
        after_cr = ch == '\r';
    }
    out
}

// Code pages: the system's legacy "ANSI" encoding, for opening and saving such files.

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct AnsiCodePage(u32);

impl AnsiCodePage {
    pub(crate) const CP_1252: Self = Self(1252);

    pub(crate) fn current() -> Self {
        Self(unsafe { GetACP() })
    }

    #[cfg(test)]
    pub(crate) const fn from_raw(code_page: u32) -> Self {
        Self(code_page)
    }

    pub(crate) fn is_utf8(self) -> bool {
        self.0 == CP_UTF8
    }

    /// Whether `byte` starts a two-byte character.
    pub(crate) fn is_dbcs_lead_byte(self, byte: u8) -> bool {
        self.0 != CP_UTF8 && self.0 != 1252 && unsafe { IsDBCSLeadByteEx(self.0, byte) }.is_ok()
    }

    /// Decodes `bytes` into `out` (through `wide`); `false` if they aren't valid in this code
    /// page.
    pub(crate) fn decode_chunk(
        self,
        bytes: &[u8],
        wide: &mut Vec<u16>,
        out: &mut String,
    ) -> io::Result<bool> {
        out.clear();
        if bytes.is_empty() {
            return Ok(true);
        }
        if i32::try_from(bytes.len()).is_err() {
            return Ok(false);
        }
        let oom = || io::Error::new(io::ErrorKind::OutOfMemory, "Out of memory decoding file.");
        let needed = unsafe { MultiByteToWideChar(self.0, MB_ERR_INVALID_CHARS, bytes, None) };
        let Some(needed) = usize::try_from(needed).ok().filter(|&n| n > 0) else {
            return Ok(false);
        };
        wide.clear();
        wide.try_reserve(needed).map_err(|_| oom())?;
        wide.resize(needed, 0);
        let written = unsafe {
            MultiByteToWideChar(
                self.0,
                MB_ERR_INVALID_CHARS,
                bytes,
                Some(wide.as_mut_slice()),
            )
        };
        let Some(written) = usize::try_from(written).ok().filter(|&n| n > 0) else {
            return Ok(false);
        };
        wide.truncate(written);
        out.try_reserve(written).map_err(|_| oom())?;
        for ch in char::decode_utf16(wide.iter().copied()) {
            let Ok(ch) = ch else {
                return Ok(false);
            };
            out.push(ch);
        }
        Ok(true)
    }

    /// Whether encoding `wide` would lose characters this code page doesn't have.
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
        let encode = |out: Option<&mut [u8]>| unsafe {
            WideCharToMultiByte(self.0, WC_NO_BEST_FIT_CHARS, wide, out, None, None)
        };
        let needed = encode(None);
        if needed == 0 {
            return Err(io::Error::last_os_error());
        }
        out.resize(needed as usize, 0);
        let written = encode(Some(out.as_mut_slice()));
        if written == 0 {
            return Err(io::Error::last_os_error());
        }
        out.truncate(written as usize);
        Ok(())
    }
}

// Files.

pub(crate) const HIDDEN_FILE_ATTRIBUTE: u32 = FILE_ATTRIBUTE_HIDDEN.0;

pub(crate) fn documents_dir() -> Option<PathBuf> {
    let path = unsafe { SHGetKnownFolderPath(&FOLDERID_Documents, KF_FLAG_DEFAULT, None) }.ok()?;
    let documents =
        (!path.0.is_null()).then(|| PathBuf::from(OsString::from_wide(unsafe { path.as_wide() })));
    unsafe { CoTaskMemFree(Some(path.0.cast())) };
    documents
}

pub(crate) fn file_hard_link_count(path: &Path) -> Option<u32> {
    let file = std::fs::File::open(path).ok()?;
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info) }.ok()?;
    Some(info.nNumberOfLinks)
}

pub(crate) enum ReplaceFileError {
    UnableToMoveReplacement(io::Error),
    SharingOrAccess(io::Error),
    Other(io::Error),
}

/// Replaces `target` with `replacement` (keeping `target`'s attributes and identity), moving
/// the old file to `backup`.
pub(crate) fn replace_file(
    target: &Path,
    replacement: &Path,
    backup: &Path,
) -> std::result::Result<(), ReplaceFileError> {
    let flags = REPLACEFILE_IGNORE_MERGE_ERRORS | REPLACEFILE_IGNORE_ACL_ERRORS;
    let Err(err) = (unsafe {
        ReplaceFileW(
            &HSTRING::from(target),
            &HSTRING::from(replacement),
            &HSTRING::from(backup),
            flags,
            None,
            None,
        )
    }) else {
        return Ok(());
    };
    let code = err.code();
    let io_err = io::Error::from_raw_os_error((code.0 as u32 & 0xFFFF) as i32);
    Err(if code == ERROR_UNABLE_TO_MOVE_REPLACEMENT_2.to_hresult() {
        ReplaceFileError::UnableToMoveReplacement(io_err)
    } else if [
        ERROR_SHARING_VIOLATION,
        ERROR_LOCK_VIOLATION,
        ERROR_ACCESS_DENIED,
    ]
    .iter()
    .any(|e| code == e.to_hresult())
    {
        ReplaceFileError::SharingOrAccess(io_err)
    } else {
        ReplaceFileError::Other(io_err)
    })
}

/// The current time and date, formatted as the user's locale does (Notepad's F5).
pub(crate) fn format_local_time_date() -> String {
    let now = unsafe { GetLocalTime() };
    let (mut time, mut date) = ([0u16; 128], [0u16; 128]);
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
    let text = |buf: &[u16], len: i32| String::from_utf16_lossy(&buf[..(len.max(1) - 1) as usize]);
    format!("{} {}", text(&time, time_len), text(&date, date_len))
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use windows::Win32::UI::WindowsAndMessaging::SendMessageW;

    use super::*;

    fn hidden_window() -> NativeWindow {
        let config = MainWindowConfig {
            show: false,
            focus: false,
            ..Default::default()
        };
        NativeWindow::create_main(&config, false).expect("window")
    }

    fn send(hwnd: HWND, msg: u32, wparam: usize, lparam: isize) -> LRESULT {
        unsafe { SendMessageW(hwnd, msg, Some(WPARAM(wparam)), Some(LPARAM(lparam))) }
    }

    /// Records what the app is told.
    #[derive(Default)]
    struct Recorder(Vec<PlatformEvent>);

    impl PlatformHandler for Recorder {
        fn on_event(&mut self, event: PlatformEvent) -> bool {
            self.0.push(event);
            false
        }
        fn drive_frames(&mut self) {}
    }

    #[test]
    fn windows_keep_their_state_events_queue_while_the_app_is_busy_and_modal_loops_keep_waking() {
        let _platform = Platform::new(None).expect("Platform::new failed");
        let win = hidden_window();
        let hwnd = win.hwnd().raw();
        assert!(
            WindowState::of(hwnd).is_some(),
            "an open window has its state"
        );

        let app = Rc::new(RefCell::new(Recorder::default()));
        Platform::with_test_handler(app.clone(), || {
            {
                let _busy = app.borrow_mut();
                send(hwnd, WM_ACTIVATE, 1, 0);
            }
            assert!(app.borrow().0.is_empty(), "a busy app gets events later");
            deliver(None);
            assert!(matches!(
                app.borrow().0[..],
                [PlatformEvent::Window(_, WindowEvent::Activated(true))]
            ));
            assert!(win.is_active());

            send(hwnd, WM_ENTERSIZEMOVE, 0, 0);
            assert!(win.state.in_size_move.get());
            send(hwnd, WM_TIMER, SIZE_MOVE_TIMER_ID, 0);
            assert!(
                matches!(app.borrow().0.last(), Some(PlatformEvent::Wake)),
                "the timer during a modal loop wakes the app"
            );
            send(hwnd, WM_EXITSIZEMOVE, 0, 0);
            assert!(!win.state.in_size_move.get());
        });
        drop(win);
        assert!(
            WindowState::of(hwnd).is_none(),
            "a closed window's state is gone"
        );
    }

    #[test]
    fn the_message_loop_wakes_at_a_deadline_and_quits() {
        struct Quitter {
            platform: Platform,
            woke: bool,
        }

        impl PlatformHandler for Quitter {
            fn on_event(&mut self, event: PlatformEvent) -> bool {
                if matches!(event, PlatformEvent::Wake) {
                    self.woke = true;
                    self.platform.post_quit();
                }
                false
            }
            fn drive_frames(&mut self) {}
        }

        let platform = Platform::new(None).expect("Platform::new failed");
        let app = Rc::new(RefCell::new(Quitter {
            platform,
            woke: false,
        }));
        Platform::run(app.clone(), |app| {
            app.platform
                .wake_at(Some(Instant::now() + Duration::from_millis(25)));
        });
        assert!(app.borrow().woke);
    }

    #[test]
    fn releasing_capture_ourselves_is_not_capture_lost() {
        let _platform = Platform::new(None).expect("Platform::new failed");
        let win = hidden_window();
        let hwnd = win.hwnd().raw();
        let app = Rc::new(RefCell::new(Recorder::default()));
        Platform::with_test_handler(app.clone(), || {
            release_capture_suppressed(&win.state, || {
                send(hwnd, WM_CAPTURECHANGED, 0, 0);
            });
            assert!(app.borrow().0.is_empty());
            send(hwnd, WM_CAPTURECHANGED, 0, 0);
            assert!(matches!(
                app.borrow().0[..],
                [PlatformEvent::Window(_, WindowEvent::CaptureLost)]
            ));
        });
    }

    #[test]
    fn wheel_notches_scroll_by_the_system_lines_per_notch() {
        let _platform = Platform::new(None).expect("Platform::new failed");
        let win = hidden_window();
        let app = Rc::new(RefCell::new(Recorder::default()));
        Platform::with_test_handler(app.clone(), || {
            send(
                win.hwnd().raw(),
                WM_MOUSEWHEEL,
                ((-120i16) as u16 as usize) << 16,
                0,
            );
        });
        let lines = system_parameters_u32(SPI_GETWHEELSCROLLLINES.0).unwrap_or(DEFAULT_WHEEL_STEP);
        let (expected, page) = if lines == u32::MAX {
            (-1.0, true)
        } else {
            (-(lines.max(1) as f32), false)
        };
        let events = &app.borrow().0;
        let [
            PlatformEvent::Window(
                _,
                WindowEvent::ScrollWheel {
                    delta_lines,
                    page_scroll,
                    ..
                },
            ),
        ] = events[..]
        else {
            panic!("expected one wheel event, got {events:?}");
        };
        assert_eq!((delta_lines, page_scroll), (point(0.0, expected), page));
    }

    #[test]
    fn handoff_paths_round_trip_and_malformed_messages_are_refused() {
        let paths = vec![
            PathBuf::from(r"C:\Temp\hello world.txt"),
            PathBuf::from(r"C:\Temp\日本語.txt"),
        ];
        let bytes = encode_paths(&paths);
        let message = |tag, len: usize, data: *const u8| COPYDATASTRUCT {
            dwData: tag,
            cbData: len as u32,
            lpData: data as *mut c_void,
        };
        let null = std::ptr::null();
        assert_eq!(
            decode_paths(&message(HANDOFF_TAG, bytes.len(), bytes.as_ptr())),
            Some(paths)
        );
        assert_eq!(
            decode_paths(&message(HANDOFF_TAG, 0, null)),
            Some(Vec::new())
        );
        assert_eq!(decode_paths(&message(0xDEAD_BEEF, 0, null)), None);
        assert_eq!(
            decode_paths(&message(HANDOFF_TAG, bytes.len() - 1, bytes.as_ptr())),
            None
        );
    }

    #[test]
    fn later_launches_hand_their_files_to_the_first_until_it_stops() {
        struct First {
            platform: Platform,
            received: mpsc::Sender<Vec<PathBuf>>,
            control: mpsc::Receiver<bool>,
            stopped: Option<mpsc::Sender<()>>,
        }

        impl PlatformHandler for First {
            fn on_event(&mut self, event: PlatformEvent) -> bool {
                match event {
                    PlatformEvent::Launch(paths) => {
                        let _ = self.received.send(paths);
                    }
                    PlatformEvent::Wake => {
                        while let Ok(quit) = self.control.try_recv() {
                            if quit {
                                self.platform.post_quit();
                            } else {
                                self.platform.stop_handoff();
                                if let Some(stopped) = self.stopped.take() {
                                    let _ = stopped.send(());
                                }
                            }
                        }
                    }
                    _ => {}
                }
                false
            }
            fn drive_frames(&mut self) {}
        }

        let key = format!("Test.CopyData.{}", std::process::id());
        let (received_tx, received) = mpsc::channel();
        let (ready_tx, ready) = mpsc::channel();
        let (stopped_tx, stopped) = mpsc::channel();
        let (control_tx, control) = mpsc::channel();
        let first_key = key.clone();
        let first = thread::spawn(move || {
            let Arbitration::Primary(Some(lock)) = arbitrate_for(&first_key, &[]) else {
                panic!("the first launch must get the lock");
            };
            let platform = Platform::new(Some(lock)).expect("Platform::new");
            let (post, commands) = mpsc::channel();
            let waker = platform.waker();
            let _ = control_tx.send(move |quit| {
                let _ = post.send(quit);
                waker.wake();
            });
            let app = Rc::new(RefCell::new(First {
                platform,
                received: received_tx,
                control: commands,
                stopped: Some(stopped_tx),
            }));
            Platform::run(app, |_| {
                let _ = ready_tx.send(());
            });
        });
        let command = control.recv_timeout(Duration::from_secs(2)).unwrap();
        ready.recv_timeout(Duration::from_secs(2)).unwrap();

        assert!(matches!(arbitrate_for(&key, &[]), Arbitration::HandedOff));
        assert_eq!(
            received.recv_timeout(Duration::from_secs(2)).unwrap(),
            Vec::<PathBuf>::new()
        );

        let path = |i: usize| PathBuf::from(format!(r"C:\Temp\multi_{i}_日本語.txt"));
        let launches: Vec<_> = (0..5)
            .map(|i| {
                let key = key.clone();
                thread::spawn(move || {
                    matches!(arbitrate_for(&key, &[path(i)]), Arbitration::HandedOff)
                })
            })
            .collect();
        for launch in launches {
            assert!(launch.join().unwrap(), "concurrent launches all hand off");
        }
        let mut got: Vec<PathBuf> = (0..5)
            .flat_map(|_| received.recv_timeout(Duration::from_secs(3)).unwrap())
            .collect();
        got.sort();
        assert_eq!(got, (0..5).map(path).collect::<Vec<_>>());

        command(false);
        stopped.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(
            !send_handoff(&format!("Jot.Handoff.{key}"), &[]),
            "a stopped first launch takes no files"
        );
        command(true);
        let _ = first.join();
    }

    #[test]
    fn clipboard_text_uses_crlf_without_doubling_and_no_nuls() {
        assert_eq!(clipboard_text("a\nb\nc"), "a\r\nb\r\nc");
        assert_eq!(clipboard_text("a\r\nb\n"), "a\r\nb\r\n");
        assert_eq!(clipboard_text("a\0b"), "a b");
    }
}
