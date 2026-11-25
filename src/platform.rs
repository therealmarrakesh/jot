#![allow(unsafe_code)]

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::ffi::{OsString, c_void};
use std::io;
use std::os::windows::{
    ffi::{OsStrExt, OsStringExt},
    io::AsRawHandle,
};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Condvar, Mutex};
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
        Authorization::ConvertSidToStringSidW, EqualSid, GetTokenInformation, PSID,
        TOKEN_ELEVATION, TOKEN_QUERY, TOKEN_USER, TokenElevation, TokenUser,
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
        Threading::{
            CreateMutexW, GetCurrentProcess, OpenProcess, OpenProcessToken,
            PROCESS_QUERY_LIMITED_INFORMATION,
        },
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
            DispatchMessageW, EnumWindows, FindWindowExW, GUI_INMOVESIZE, GUITHREADINFO,
            GWL_EXSTYLE, GetCaretBlinkTime, GetClientRect, GetCursorPos, GetGUIThreadInfo,
            GetMessageTime, GetMessageW, GetSystemMetrics, GetWindowLongW, GetWindowPlacement,
            GetWindowRect, GetWindowThreadProcessId, HTCAPTION, HTCLIENT, HTCLOSE, HTMAXBUTTON,
            HTMINBUTTON, HTTOP, HTTOPLEFT, HTTOPRIGHT, HWND_MESSAGE, HWND_TOPMOST, IDC_ARROW,
            IDC_IBEAM, IsIconic, IsWindowVisible, IsZoomed, KillTimer, LWA_ALPHA, LoadCursorW,
            LoadIconW, MB_ICONERROR, MB_OK, MINMAXINFO, MSG, MessageBoxW, NCCALCSIZE_PARAMS,
            PostMessageW, PostQuitMessage, RegisterClassW, SC_CLOSE, SC_MAXIMIZE, SC_MINIMIZE,
            SC_MOVE, SC_RESTORE, SM_CXDOUBLECLK, SM_CXFRAME, SM_CXPADDEDBORDER, SM_CYDOUBLECLK,
            SM_CYFRAME, SM_SWAPBUTTON, SMTO_ABORTIFHUNG, SMTO_BLOCK, SPI_GETCARETWIDTH,
            SPI_GETCLIENTAREAANIMATION, SPI_GETHIGHCONTRAST, SPI_GETMENUSHOWDELAY,
            SPI_GETMOUSEHOVERTIME, SPI_GETWHEELSCROLLCHARS, SPI_GETWHEELSCROLLLINES, SW_HIDE,
            SW_RESTORE, SW_SHOW, SW_SHOWNOACTIVATE, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOSIZE,
            SWP_NOZORDER, SYSTEM_PARAMETERS_INFO_ACTION, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
            SendMessageTimeoutW, SetCaretPos, SetCursor, SetForegroundWindow,
            SetLayeredWindowAttributes, SetTimer, SetWindowPlacement, SetWindowPos, SetWindowTextW,
            ShowWindow, SystemParametersInfoW, TranslateMessage, USER_DEFAULT_SCREEN_DPI,
            USER_TIMER_MAXIMUM, USER_TIMER_MINIMUM, WINDOW_EX_STYLE, WINDOW_STYLE, WINDOWPLACEMENT,
            WM_ACTIVATE, WM_APP, WM_CANCELMODE, WM_CAPTURECHANGED, WM_CHAR, WM_CLOSE, WM_COPYDATA,
            WM_DPICHANGED, WM_DROPFILES, WM_ENDSESSION, WM_ENTERMENULOOP, WM_ENTERSIZEMOVE,
            WM_ERASEBKGND, WM_EXITMENULOOP, WM_EXITSIZEMOVE, WM_GETMINMAXINFO, WM_IME_COMPOSITION,
            WM_IME_ENDCOMPOSITION, WM_IME_STARTCOMPOSITION, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDBLCLK,
            WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEHWHEEL,
            WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_MOVE, WM_NCCALCSIZE, WM_NCHITTEST, WM_NCLBUTTONDBLCLK,
            WM_NCLBUTTONDOWN, WM_NCLBUTTONUP, WM_NCMOUSELEAVE, WM_NCMOUSEMOVE, WM_PAINT,
            WM_QUERYENDSESSION, WM_RBUTTONDBLCLK, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SETCURSOR,
            WM_SETTINGCHANGE, WM_SIZE, WM_SYSCHAR, WM_SYSCOMMAND, WM_SYSKEYDOWN, WM_SYSKEYUP,
            WM_THEMECHANGED, WM_TIMER, WNDCLASSW, WS_EX_APPWINDOW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
            WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT,
            WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_POPUP, WS_SYSMENU, WS_THICKFRAME, WindowFromPoint,
        },
    },
};
use windows::core::{BOOL, HSTRING, Interface, Owned, PCWSTR, PWSTR, w};

use crate::geom::{Point, Rect, ScreenPoint, ScreenRect, Size, point, size};
use crate::types::{Error, Result, SharedStr};

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
            title: SharedStr::Static("Jot"),
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

#[derive(Default)]
struct ClickTracker {
    last_button: Option<MouseButton>,
    last_pos: (i32, i32),
    last_time_ms: Option<u32>,
    count: usize,
}

impl ClickTracker {
    fn record(&mut self, button: MouseButton, x: i32, y: i32) -> usize {
        let now_ms = unsafe { GetMessageTime() } as u32;
        let dbl_ms = unsafe { GetDoubleClickTime() };
        let tol_x = (unsafe { GetSystemMetrics(SM_CXDOUBLECLK) } / 2).max(2);
        let tol_y = (unsafe { GetSystemMetrics(SM_CYDOUBLECLK) } / 2).max(2);
        let same_spot = self.last_button == Some(button)
            && self
                .last_time_ms
                .is_some_and(|t| now_ms.wrapping_sub(t) <= dbl_ms)
            && (x - self.last_pos.0).abs() <= tol_x
            && (y - self.last_pos.1).abs() <= tol_y;
        self.count = if same_spot { self.count + 1 } else { 1 };
        self.last_button = Some(button);
        self.last_pos = (x, y);
        self.last_time_ms = Some(now_ms);
        self.count
    }
}

pub(crate) fn current_modifiers() -> Modifiers {
    let down = |vk: VIRTUAL_KEY| (unsafe { GetKeyState(i32::from(vk.0)) } as u16 & 0x8000) != 0;
    let lctrl = down(VK_LCONTROL);
    let rctrl = down(VK_RCONTROL);
    let ralt = down(VK_RMENU);
    // Windows synthesizes VK_LCONTROL + VK_RMENU for AltGr.
    let alt_gr = ralt && lctrl && !rctrl;
    Modifiers {
        ctrl: down(VK_CONTROL),
        alt: down(VK_MENU),
        shift: down(VK_SHIFT),
        win: down(VK_LWIN) || down(VK_RWIN),
        alt_gr,
    }
}

fn is_syskey_passthrough(vk: u16) -> bool {
    vk == VK_F4.0 || vk == VK_SPACE.0
}

fn translate_vk(vk: u16, modifiers: Modifiers) -> Option<Key> {
    let v = VIRTUAL_KEY(vk);
    let key = match v {
        VK_BACK => Key::Backspace,
        VK_TAB => Key::Tab,
        VK_RETURN => Key::Enter,
        VK_ESCAPE => Key::Escape,
        VK_SPACE => Key::Space,
        VK_PRIOR => Key::PageUp,
        VK_NEXT => Key::PageDown,
        VK_END => Key::End,
        VK_HOME => Key::Home,
        VK_LEFT => Key::Left,
        VK_UP => Key::Up,
        VK_RIGHT => Key::Right,
        VK_DOWN => Key::Down,
        VK_INSERT => Key::Insert,
        VK_DELETE => Key::Delete,
        VK_MENU => Key::Alt,
        VK_APPS => Key::Menu,
        VK_OEM_PLUS => Key::Char(if modifiers.shift { '+' } else { '=' }),
        VK_ADD => Key::Char('+'),
        VK_OEM_MINUS | VK_SUBTRACT => Key::Char('-'),
        VK_OEM_4 => Key::Char('['),
        VK_OEM_6 => Key::Char(']'),
        _ if (VK_F1.0..=VK_F12.0).contains(&vk) => Key::F((vk - VK_F1.0 + 1) as u8),
        _ if (VK_NUMPAD0.0..=VK_NUMPAD9.0).contains(&vk) => {
            Key::Char((b'0' + (vk - VK_NUMPAD0.0) as u8) as char)
        }
        _ if (b'0' as u16..=b'9' as u16).contains(&vk) => Key::Char(vk as u8 as char),
        _ if (b'A' as u16..=b'Z' as u16).contains(&vk) => {
            Key::Char((vk as u8 as char).to_ascii_lowercase())
        }
        _ => return None,
    };
    Some(key)
}

fn read_ime_string(himc: HIMC, flag: IME_COMPOSITION_STRING) -> Option<String> {
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

fn set_ime_candidate_pos(hwnd: HWND, caret_rect: Rect, scale: f32) {
    let pt = POINT {
        x: (caret_rect.origin.x * scale).round() as i32,
        y: (caret_rect.bottom() * scale).round() as i32,
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
}

pub(crate) fn complete_ime_composition(hwnd: HWND) {
    let himc = unsafe { ImmGetContext(hwnd) };
    if himc.is_invalid() {
        return;
    }
    unsafe {
        let _ = ImmNotifyIME(himc, NI_COMPOSITIONSTR, CPS_COMPLETE, 0);
        let _ = ImmReleaseContext(hwnd, himc);
    }
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
        let msg = panic_message(info.payload());
        let loc = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "<unknown>".to_owned());
        debug_log(&format!("PANIC at {loc}: {msg}"));
        prev(info);
    }));
}

pub(crate) fn ffi_guard<R>(context: &str, fallback: R, f: impl FnOnce() -> R) -> R {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(res) => res,
        Err(payload) => {
            let msg = panic_message(&*payload);
            debug_log(&format!("Caught panic at Win32 boundary ({context}): {msg}"));
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
        }
    }
}

const WM_APP_WAKE: u32 = WM_APP + 1;
const WM_APP_VSYNC: u32 = WM_APP + 2;
const WAKE_TIMER_ID: usize = 2;
const WAKE_CLASS_NAME: PCWSTR = w!("JotWakeWindow");

#[derive(Default)]
struct PumpState {
    handler: Option<Rc<RefCell<dyn PlatformHandler>>>,
    wake_pending: bool,
    handoff_accepting: bool,
    settings: Option<SystemSettings>,
    windows: HashMap<isize, Rc<WindowShared>>,
    pending_events: VecDeque<PlatformEvent>,
    shutdown_block: Option<&'static str>,
}

thread_local! {
    static PUMP: RefCell<PumpState> = RefCell::new(PumpState::default());
}

fn lookup_window(hwnd: HWND) -> Option<Rc<WindowShared>> {
    PUMP.with(|p| p.borrow().windows.get(&(hwnd.0 as isize)).cloned())
}

fn cached_settings() -> SystemSettings {
    if let Some(s) = PUMP.with(|p| p.borrow().settings) {
        return s;
    }
    let fresh = SystemSettings::query();
    set_cached_settings(fresh);
    fresh
}

fn set_cached_settings(settings: SystemSettings) {
    PUMP.with(|p| p.borrow_mut().settings = Some(settings));
}

fn mark_wake_pending() {
    PUMP.with(|p| p.borrow_mut().wake_pending = true);
}

fn active_handler() -> Option<Rc<RefCell<dyn PlatformHandler>>> {
    PUMP.with(|p| p.borrow().handler.clone())
}

fn queue_pending_event(event: PlatformEvent) {
    PUMP.with(|p| {
        let mut state = p.borrow_mut();
        let coalesced = match (&event, state.pending_events.back_mut()) {
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
            state.pending_events.push_back(event);
        }
    });
}

fn drain_pending_only() {
    let Some(handler_rc) = active_handler() else {
        return;
    };
    loop {
        if PUMP.with(|p| std::mem::take(&mut p.borrow_mut().wake_pending)) {
            match handler_rc.try_borrow_mut() {
                Ok(mut handler) => handler.on_wake(),
                Err(_) => {
                    mark_wake_pending();
                    return;
                }
            }
        }
        let next = PUMP.with(|p| p.borrow_mut().pending_events.pop_front());
        let Some(event) = next else {
            break;
        };
        let Ok(mut handler) = handler_rc.try_borrow_mut() else {
            PUMP.with(|p| p.borrow_mut().pending_events.push_front(event));
            return;
        };
        handler.on_event(event);
    }
}

fn service() {
    let Some(handler_rc) = active_handler() else {
        return;
    };
    drain_pending_only();
    if let Ok(mut handler) = handler_rc.try_borrow_mut() {
        handler.drive_frames();
    }
    drain_pending_only();
}

fn dispatch(event: PlatformEvent) -> bool {
    let Some(handler_rc) = active_handler() else {
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

struct ActiveHandlerGuard;

impl Drop for ActiveHandlerGuard {
    fn drop(&mut self) {
        PUMP.with(|p| {
            let mut state = p.borrow_mut();
            state.handler = None;
            state.wake_pending = false;
            state.pending_events.clear();
        });
    }
}

struct VsyncPacer {
    armed: Mutex<bool>,
    condvar: Condvar,
}

impl VsyncPacer {
    fn spawn(wake_hwnd_raw: isize) -> Result<Arc<Self>> {
        let pacer = Arc::new(Self {
            armed: Mutex::new(false),
            condvar: Condvar::new(),
        });
        let bg = pacer.clone();
        std::thread::Builder::new()
            .name("jot-vsync".into())
            .spawn(move || loop {
                {
                    let mut armed = bg.armed.lock().unwrap_or_else(|e| e.into_inner());
                    while !*armed {
                        armed = bg
                            .condvar
                            .wait(armed)
                            .unwrap_or_else(|e| e.into_inner());
                    }
                    *armed = false;
                }
                if unsafe { DwmFlush() }.is_err() {
                    std::thread::sleep(Duration::from_millis(16));
                }
                unsafe {
                    let _ = PostMessageW(
                        Some(HWND(wake_hwnd_raw as *mut _)),
                        WM_APP_VSYNC,
                        WPARAM(0),
                        LPARAM(0),
                    );
                }
            })?;
        Ok(pacer)
    }

    fn arm(&self) {
        let mut armed = self.armed.lock().unwrap_or_else(|e| e.into_inner());
        if !*armed {
            *armed = true;
            self.condvar.notify_one();
        }
    }
}

pub(crate) struct Platform {
    wake_hwnd: HWND,
    vsync: Arc<VsyncPacer>,
    com_initialized: bool,
    _instance_lock: Option<InstanceLock>,
}

impl Platform {
    pub(crate) fn new(instance_lock: Option<InstanceLock>) -> Result<Self> {
        unsafe {
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }
        let com_initialized =
            unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
        register_window_class()?;

        let hinstance = unsafe { GetModuleHandleW(None)? };
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wake_wnd_proc),
            hInstance: hinstance.into(),
            lpszClassName: WAKE_CLASS_NAME,
            ..Default::default()
        };
        unsafe { RegisterClassW(&wc) };

        let wake_hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                WAKE_CLASS_NAME,
                w!(""),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                Some(hinstance.into()),
                None,
            )?
        };

        set_cached_settings(SystemSettings::query());
        let vsync = VsyncPacer::spawn(wake_hwnd.0 as isize)?;

        let platform = Self {
            wake_hwnd,
            vsync,
            com_initialized,
            _instance_lock: instance_lock,
        };
        if let Some(lock) = &platform._instance_lock {
            PUMP.with(|p| p.borrow_mut().handoff_accepting = true);
            let wide = HSTRING::from(lock.window_title.as_str());
            let _ = unsafe { SetWindowTextW(platform.wake_hwnd, &wide) };
        }
        Ok(platform)
    }

    pub(crate) fn settings(&self) -> SystemSettings {
        cached_settings()
    }

    pub(crate) fn set_block_shutdown(&self, reason: Option<&'static str>) {
        let hwnds = PUMP.with(|p| {
            let mut state = p.borrow_mut();
            let prev = std::mem::replace(&mut state.shutdown_block, reason);
            if prev.is_some() && reason.is_none() {
                state.windows.keys().copied().collect::<Vec<_>>()
            } else {
                Vec::new()
            }
        });
        for raw in hwnds {
            let _ = unsafe { ShutdownBlockReasonDestroy(HWND(raw as *mut _)) };
        }
    }

    fn disable_handoff(&self) {
        PUMP.with(|p| p.borrow_mut().handoff_accepting = false);
        let _ = unsafe { SetWindowTextW(self.wake_hwnd, w!("")) };
    }

    /// A handle any thread can use to wake the loop (`PlatformHandler::on_wake`).
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

    pub(crate) fn request_frame_tick(&self) {
        self.vsync.arm();
    }

    pub(crate) fn post_quit(&self) {
        self.disable_handoff();
        unsafe { PostQuitMessage(0) };
    }

    #[cfg(test)]
    pub(crate) fn with_test_handler<R>(
        handler: Rc<RefCell<dyn PlatformHandler>>,
        f: impl FnOnce() -> R,
    ) -> R {
        PUMP.with(|p| p.borrow_mut().handler = Some(handler));
        let _guard = ActiveHandlerGuard;
        f()
    }

    pub(crate) fn run<H: PlatformHandler + 'static>(
        handler: Rc<RefCell<H>>,
        on_ready: impl FnOnce(&mut H),
    ) {
        let dyn_handler: Rc<RefCell<dyn PlatformHandler>> = handler.clone();
        PUMP.with(|p| p.borrow_mut().handler = Some(dyn_handler));
        let _guard = ActiveHandlerGuard;

        {
            let mut h = handler.borrow_mut();
            on_ready(&mut h);
        }
        service();

        let mut msg = MSG::default();
        while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            service();
        }
    }
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

impl Drop for Platform {
    fn drop(&mut self) {
        self.disable_handoff();
        unsafe {
            let _ = DestroyWindow(self.wake_hwnd);
            if self.com_initialized {
                CoUninitialize();
            }
        }
    }
}

unsafe extern "system" fn wake_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    ffi_guard("wake_wnd_proc", LRESULT(0), || {
        wake_wnd_proc_inner(hwnd, msg, wparam, lparam)
    })
}

fn wake_wnd_proc_inner(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_APP_WAKE => {
            mark_wake_pending();
            service();
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == WAKE_TIMER_ID => {
            let _ = unsafe { KillTimer(Some(hwnd), WAKE_TIMER_ID) };
            mark_wake_pending();
            service();
            LRESULT(0)
        }
        WM_APP_VSYNC => {
            service();
            LRESULT(0)
        }
        WM_COPYDATA => {
            if !PUMP.with(|p| p.borrow().handoff_accepting) {
                return LRESULT(0);
            }
            let cds = unsafe { &*(lparam.0 as *const COPYDATASTRUCT) };
            let Some(paths) = decode_copydata(cds) else {
                return LRESULT(0);
            };
            dispatch(PlatformEvent::Launch { paths });
            LRESULT(1)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

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
        Self((bits & 0x3) as u8 | if bits & 0x10 != 0 { 0x4 } else { 0 })
    }

    fn primary(self) -> Option<MouseButton> {
        [MouseButton::Left, MouseButton::Right, MouseButton::Middle]
            .into_iter()
            .find(|&b| self.0 & Self::bit(b) != 0)
    }
}

#[derive(Copy, Clone)]
struct WindowMetrics {
    scale: f32,
    physical: (u32, u32),
}

#[derive(Default)]
struct WindowChrome {
    min_size: Option<Size>,
    cursor: CursorStyle,
    control_areas: Vec<(Rect, WindowControlArea)>,
}

#[derive(Default)]
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
            chrome: RefCell::default(),
            input: RefCell::new(WindowInput {
                last_modifiers: current_modifiers(),
                ..Default::default()
            }),
        })
    }

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

fn register_window_class() -> Result<()> {
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
        PUMP.with(|p| p.borrow_mut().windows.insert(hwnd.0 as isize, shared.clone()));

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

        set_dwm(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND.0 as u32);
        set_dwm(hwnd, DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE);
        set_dwm(hwnd, DWMWA_TRANSITIONS_FORCEDISABLED, 1u32);
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

    pub(crate) fn scale_factor(&self) -> f32 {
        self.shared.scale()
    }

    pub(crate) fn physical_size(&self) -> (u32, u32) {
        self.shared.metrics.get().physical
    }

    pub(crate) fn logical_size(&self) -> Size {
        self.shared.logical_size()
    }

    pub(crate) fn is_active(&self) -> bool {
        self.shared.active.get()
    }

    #[cfg(test)]
    fn in_size_move(&self) -> bool {
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
        let in_size_move = PUMP.with(|p| p.borrow_mut().windows.remove(&(self.hwnd.0 as isize)))
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

fn set_dwm<T>(hwnd: HWND, attr: DWMWINDOWATTRIBUTE, value: T) {
    let size = size_of::<T>() as u32;
    let _ = unsafe { DwmSetWindowAttribute(hwnd, attr, (&raw const value).cast(), size) };
}

fn configure_dark_mode(hwnd: HWND, appearance: WindowAppearance) {
    let dark = BOOL(i32::from(appearance == WindowAppearance::Dark));
    set_dwm(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, dark);
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
    set_dwm(hwnd, DWMWA_SYSTEMBACKDROP_TYPE, DWM_SYSTEMBACKDROP_TYPE(DWMSBT_TABBEDWINDOW));
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
            let raw_amount = cached_settings().wheel_scroll_amount(horizontal);
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
                set_cached_settings(fresh);
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
            mark_wake_pending();
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
            if let Some(reason) = PUMP.with(|p| p.borrow().shutdown_block) {
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

const COPYDATA_TAG: usize = 0x4A4F_5401;
const MAX_PAYLOAD_BYTES: usize = 1024 * 1024;
const SEND_TIMEOUT_MS: u32 = 2000;

pub(crate) struct InstanceLock {
    _mutex: Owned<HANDLE>,
    window_title: String,
}

pub(crate) enum Arbitration {
    Primary(Option<InstanceLock>),
    HandedOff,
}

struct ProcessIdentity {
    user_buf: Vec<u8>,
    key: String,
}

impl ProcessIdentity {
    fn current() -> Option<Self> {
        let mut raw_token = HANDLE::default();
        unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw_token).ok()? };
        let token = unsafe { Owned::new(raw_token) };

        let user_buf = query_token_user_bytes(*token)?;
        let sid = sid_from_user_buf(&user_buf)?;
        let sid_str = sid_to_string(sid)?;

        let mut elev = TOKEN_ELEVATION::default();
        let mut elev_len = 0u32;
        let elevated = unsafe {
            GetTokenInformation(
                *token,
                TokenElevation,
                Some((&raw mut elev).cast()),
                size_of::<TOKEN_ELEVATION>() as u32,
                &mut elev_len,
            )
            .is_ok()
        } && elev.TokenIsElevated != 0;

        let elev_tag = if elevated { "elev" } else { "std" };
        Some(Self {
            user_buf,
            key: format!("{elev_tag}.{sid_str}"),
        })
    }

    fn matches_pid(&self, pid: u32) -> bool {
        if pid == 0 {
            return false;
        }
        let Some(my_sid) = sid_from_user_buf(&self.user_buf) else {
            return false;
        };
        let Ok(raw_proc) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) })
        else {
            return false;
        };
        let proc_handle = unsafe { Owned::new(raw_proc) };

        let mut raw_token = HANDLE::default();
        if unsafe { OpenProcessToken(*proc_handle, TOKEN_QUERY, &mut raw_token) }.is_err() {
            return false;
        }
        let target_token = unsafe { Owned::new(raw_token) };
        let Some(target_buf) = query_token_user_bytes(*target_token) else {
            return false;
        };
        let Some(target_sid) = sid_from_user_buf(&target_buf) else {
            return false;
        };
        unsafe { EqualSid(my_sid, target_sid).is_ok() }
    }
}

fn query_token_user_bytes(token: HANDLE) -> Option<Vec<u8>> {
    let mut needed = 0u32;
    let _ = unsafe { GetTokenInformation(token, TokenUser, None, 0, &mut needed) };
    if needed == 0 {
        return None;
    }
    let mut buf = vec![0u8; needed as usize];
    unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            Some(buf.as_mut_ptr().cast()),
            needed,
            &mut needed,
        )
        .ok()?;
    }
    Some(buf)
}

fn sid_from_user_buf(buf: &[u8]) -> Option<PSID> {
    if buf.len() < size_of::<TOKEN_USER>() {
        return None;
    }
    let user = unsafe { std::ptr::read_unaligned(buf.as_ptr().cast::<TOKEN_USER>()) };
    Some(user.User.Sid)
}

fn sid_to_string(sid: PSID) -> Option<String> {
    let mut wide = PWSTR::null();
    unsafe { ConvertSidToStringSidW(sid, &mut wide).ok()? };
    let _guard = unsafe { Owned::new(HLOCAL(wide.0.cast())) };
    Some(String::from_utf16_lossy(unsafe { wide.as_wide() }))
}

fn encode_paths(paths: &[PathBuf]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for (ix, path) in paths.iter().enumerate() {
        if ix > 0 {
            bytes.extend([0, 0]);
        }
        let abs = std::path::absolute(path).unwrap_or_else(|_| path.clone());
        for u in abs.as_os_str().encode_wide() {
            bytes.extend(u.to_le_bytes());
        }
    }
    bytes
}

fn decode_copydata(cds: &COPYDATASTRUCT) -> Option<Vec<PathBuf>> {
    if cds.dwData != COPYDATA_TAG {
        return None;
    }
    let len = cds.cbData as usize;
    if len == 0 {
        return Some(Vec::new());
    }
    if len > MAX_PAYLOAD_BYTES || !len.is_multiple_of(2) || cds.lpData.is_null() {
        return None;
    }
    let bytes = unsafe { std::slice::from_raw_parts(cds.lpData.cast::<u8>(), len) };
    let units: Vec<u16> = bytes.as_chunks::<2>().0.iter().map(|p| u16::from_le_bytes(*p)).collect();
    let paths: Vec<PathBuf> = units
        .split(|&u| u == 0)
        .filter(|part| !part.is_empty())
        .map(|part| PathBuf::from(OsString::from_wide(part)))
        .collect();
    (!paths.is_empty()).then_some(paths)
}

fn try_claim_mutex(name: &str) -> std::result::Result<Option<Owned<HANDLE>>, ()> {
    let wide = HSTRING::from(name);
    let handle = unsafe { CreateMutexW(None, false, &wide) }.map_err(|_| ())?;
    let err = unsafe { GetLastError() };
    let guard = unsafe { Owned::new(handle) };
    Ok((err != ERROR_ALREADY_EXISTS).then_some(guard))
}

fn send_handoff(
    identity: &ProcessIdentity,
    class_name: PCWSTR,
    window_title: &str,
    paths: &[PathBuf],
) -> bool {
    let title = HSTRING::from(window_title);
    let Ok(target) = (unsafe { FindWindowExW(Some(HWND_MESSAGE), None, class_name, &title) })
    else {
        return false;
    };
    if target.0.is_null() {
        return false;
    }
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(target, Some(&mut pid)) };
    if !identity.matches_pid(pid) {
        return false;
    }
    let _ = unsafe { AllowSetForegroundWindow(pid) };

    let payload = encode_paths(paths);
    let Ok(cb_data) = u32::try_from(payload.len()) else {
        return false;
    };
    if payload.len() > MAX_PAYLOAD_BYTES {
        return false;
    }
    let cds = COPYDATASTRUCT {
        dwData: COPYDATA_TAG,
        cbData: cb_data,
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
            SEND_TIMEOUT_MS,
            Some(&mut result),
        )
    };
    sent.0 != 0 && result == 1
}

pub(crate) fn arbitrate_instance(paths: &[PathBuf]) -> Arbitration {
    let Some(identity) = ProcessIdentity::current() else {
        return Arbitration::Primary(None);
    };
    arbitrate_for(&identity, &identity.key, paths)
}

fn arbitrate_for(identity: &ProcessIdentity, key: &str, paths: &[PathBuf]) -> Arbitration {
    let mutex_name = format!(r"Local\Jot.Mutex.{key}");
    let window_title = format!("Jot.Handoff.{key}");
    for _ in 0..20 {
        match try_claim_mutex(&mutex_name) {
            Ok(Some(mutex)) => {
                return Arbitration::Primary(Some(InstanceLock {
                    _mutex: mutex,
                    window_title,
                }));
            }
            Ok(None) => {
                if send_handoff(identity, WAKE_CLASS_NAME, &window_title, paths) {
                    return Arbitration::HandedOff;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(()) => return Arbitration::Primary(None),
        }
    }
    Arbitration::Primary(None)
}

const DEFAULT_WHEEL_LINES: u32 = 3;
const DEFAULT_WHEEL_CHARS: u32 = 3;
const DEFAULT_HOVER_TIME_MS: u32 = 400;
const DEFAULT_MENU_SHOW_DELAY_MS: u32 = 400;

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

#[derive(Copy, Clone, Debug)]
pub(crate) struct SystemSettings {
    pub(crate) appearance: WindowAppearance,
    pub(crate) accent_light: Option<u32>,
    pub(crate) accent_dark: Option<u32>,
    pub(crate) high_contrast: Option<HighContrastColors>,
    pub(crate) reduce_motion: bool,
    pub(crate) caret_blink_interval: Option<Duration>,
    pub(crate) caret_blink_timeout: Option<Duration>,
    pub(crate) caret_width_px: u32,
    pub(crate) mouse_hover_time: Duration,
    pub(crate) menu_show_delay: Duration,
    pub(crate) wheel_scroll_lines: u32,
    pub(crate) wheel_scroll_chars: u32,
}

#[cfg(test)]
impl Default for SystemSettings {
    fn default() -> Self {
        Self {
            appearance: WindowAppearance::Light,
            accent_light: None,
            accent_dark: None,
            high_contrast: None,
            reduce_motion: false,
            caret_blink_interval: None,
            caret_blink_timeout: None,
            caret_width_px: 1,
            mouse_hover_time: Duration::from_millis(u64::from(DEFAULT_HOVER_TIME_MS)),
            menu_show_delay: Duration::from_millis(u64::from(DEFAULT_MENU_SHOW_DELAY_MS)),
            wheel_scroll_lines: DEFAULT_WHEEL_LINES,
            wheel_scroll_chars: DEFAULT_WHEEL_CHARS,
        }
    }
}

impl SystemSettings {
    pub(crate) fn query() -> Self {
        let appearance = query_system_appearance();
        let (accent_light, accent_dark) = query_system_accents();
        let reduce_motion = system_parameters_u32(SPI_GETCLIENTAREAANIMATION.0)
            .is_some_and(|enabled| enabled == 0);
        let hover_ms =
            system_parameters_u32(SPI_GETMOUSEHOVERTIME.0).unwrap_or(DEFAULT_HOVER_TIME_MS);
        let menu_ms =
            system_parameters_u32(SPI_GETMENUSHOWDELAY.0).unwrap_or(DEFAULT_MENU_SHOW_DELAY_MS);
        const SPI_GETCARETTIMEOUT: u32 = 0x2022;
        let blink = |ms: u32| (ms != 0 && ms != u32::MAX).then(|| Duration::from_millis(ms.into()));
        Self {
            appearance,
            accent_light,
            accent_dark,
            high_contrast: query_high_contrast_colors(),
            reduce_motion,
            caret_blink_interval: blink(unsafe { GetCaretBlinkTime() }),
            caret_blink_timeout: blink(system_parameters_u32(SPI_GETCARETTIMEOUT).unwrap_or(5_000)),
            caret_width_px: system_parameters_u32(SPI_GETCARETWIDTH.0)
                .unwrap_or(1)
                .clamp(1, 20),
            mouse_hover_time: Duration::from_millis(u64::from(hover_ms)),
            menu_show_delay: Duration::from_millis(u64::from(menu_ms)),
            wheel_scroll_lines: system_parameters_u32(SPI_GETWHEELSCROLLLINES.0)
                .unwrap_or(DEFAULT_WHEEL_LINES),
            wheel_scroll_chars: system_parameters_u32(SPI_GETWHEELSCROLLCHARS.0)
                .unwrap_or(DEFAULT_WHEEL_CHARS),
        }
    }

    pub(crate) fn accent(self, dark: bool) -> Option<u32> {
        if dark {
            self.accent_dark
        } else {
            self.accent_light
        }
    }

    pub(crate) fn wheel_scroll_amount(self, horizontal: bool) -> u32 {
        if horizontal {
            self.wheel_scroll_chars
        } else {
            self.wheel_scroll_lines
        }
    }
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

fn query_system_appearance() -> WindowAppearance {
    let mut value = 1u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut value).cast()),
            Some(&mut size),
        )
    };
    if status.is_ok() && value == 0 {
        WindowAppearance::Dark
    } else {
        WindowAppearance::Light
    }
}

fn query_system_accents() -> (Option<u32>, Option<u32>) {
    let mut shades = [0u8; 32];
    let mut len = shades.len() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Accent"),
            w!("AccentPalette"),
            RRF_RT_REG_BINARY,
            None,
            Some(shades.as_mut_ptr().cast()),
            Some(&mut len),
        )
    };
    if status.0 != 0 {
        return (None, None);
    }
    let read = |at: usize| {
        (len as usize >= at + 3)
            .then(|| u32::from_be_bytes([0, shades[at], shades[at + 1], shades[at + 2]]))
    };
    (read(16), read(4))
}

fn query_high_contrast_colors() -> Option<HighContrastColors> {
    let mut hc = HIGHCONTRASTW {
        cbSize: size_of::<HIGHCONTRASTW>() as u32,
        ..Default::default()
    };
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            hc.cbSize,
            Some((&raw mut hc).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    }
    .is_ok();
    if !ok || (hc.dwFlags & HCF_HIGHCONTRASTON).0 == 0 {
        return None;
    }
    let sys = |idx: SYS_COLOR_INDEX| {
        let c = unsafe { GetSysColor(idx) }; // COLORREF is 0x00BBGGRR
        (c & 0xFF) << 16 | (c & 0xFF00) | (c >> 16) & 0xFF
    };
    Some(HighContrastColors {
        window: sys(COLOR_WINDOW),
        window_text: sys(COLOR_WINDOWTEXT),
        highlight: sys(COLOR_HIGHLIGHT),
        highlight_text: sys(COLOR_HIGHLIGHTTEXT),
        gray_text: sys(COLOR_GRAYTEXT),
        btn_face: sys(COLOR_BTNFACE),
        btn_text: sys(COLOR_BTNTEXT),
        window_frame: sys(COLOR_WINDOWFRAME),
        hotlight: sys(COLOR_HOTLIGHT),
    })
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
pub(crate) struct WindowHandle(isize);

impl WindowHandle {
    fn from_raw(hwnd: HWND) -> Self {
        Self(hwnd.0 as isize)
    }

    pub(crate) fn raw(self) -> HWND {
        HWND(self.0 as *mut _)
    }

    pub(crate) fn is_null(self) -> bool {
        self.0 == 0
    }

    pub(crate) fn client_origin(self) -> ScreenPoint {
        let mut origin = POINT::default();
        let _ = unsafe { ClientToScreen(self.raw(), &mut origin) };
        origin.into()
    }

    pub(crate) fn dpi_scale(self) -> f32 {
        match unsafe { GetDpiForWindow(self.raw()) } {
            0 => 1.,
            dpi => dpi as f32 / USER_DEFAULT_SCREEN_DPI as f32,
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

    pub(crate) fn bring_to_front(self) {
        if self.is_null() {
            return;
        }
        unsafe {
            if IsIconic(self.raw()).as_bool() {
                let _ = ShowWindow(self.raw(), SW_RESTORE);
            }
            let _ = SetForegroundWindow(self.raw());
        }
    }

    pub(crate) fn set_foreground(self) {
        let _ = unsafe { SetForegroundWindow(self.raw()) };
    }

    pub(crate) fn place_topmost_noactivate(self, rect: ScreenRect) {
        let _ = unsafe {
            SetWindowPos(
                self.raw(),
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
        let _ = unsafe { ShowWindow(self.raw(), SW_SHOWNOACTIVATE) };
    }

    pub(crate) fn hide(self) {
        let _ = unsafe { ShowWindow(self.raw(), SW_HIDE) };
    }

    pub(crate) fn move_noactivate(self, x: i32, y: i32) {
        let _ = unsafe {
            SetWindowPos(
                self.raw(),
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
        unsafe { SetCapture(self.raw()) };
    }

    pub(crate) fn begin_system_move(self, point: ScreenPoint) {
        let position = LPARAM((((point.y as u16 as u32) << 16) | point.x as u16 as u32) as isize);
        unsafe {
            let _ = ReleaseCapture();
            let _ = PostMessageW(
                Some(self.raw()),
                WM_SYSCOMMAND,
                WPARAM(SC_MOVE as usize | HTCAPTION as usize),
                position,
            );
        }
    }

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

    pub(crate) fn restored_logical_size(self) -> Option<crate::geom::Size> {
        let placement = self.saved_placement()?;
        let rc = placement.0.rcNormalPosition;
        let w = (rc.right - rc.left).max(1) as f32;
        let h = (rc.bottom - rc.top).max(1) as f32;
        let scale = self.dpi_scale().max(0.5);
        Some(crate::geom::size(w / scale, h / scale))
    }

    pub(crate) fn restore_placement(self, placement: WindowPlacement) {
        let _ = unsafe { SetWindowPlacement(self.raw(), &placement.0) };
    }

    pub(crate) fn complete_ime_composition(self) {
        if !self.is_null() {
            complete_ime_composition(self.raw());
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
    exclude: Option<WindowHandle>,
) -> Option<WindowHandle> {
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
        exclude: exclude.map(WindowHandle::raw),
        found: None,
    };
    let _ = unsafe { EnumWindows(Some(visit), LPARAM((&raw mut search) as isize)) };
    search.found.map(WindowHandle::from_raw)
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct DisplayId(pub(crate) usize);

#[derive(Copy, Clone, Debug)]
pub(crate) struct WindowsDisplay {
    monitor: HMONITOR,
}

impl WindowsDisplay {
    pub(crate) fn primary() -> Option<Self> {
        let monitor = unsafe { MonitorFromPoint(POINT::default(), MONITOR_DEFAULTTOPRIMARY) };
        (!monitor.is_invalid()).then_some(Self { monitor })
    }

    pub(crate) fn from_point(pt: ScreenPoint) -> Option<Self> {
        let monitor = unsafe { MonitorFromPoint(pt.into(), MONITOR_DEFAULTTONEAREST) };
        (!monitor.is_invalid()).then_some(Self { monitor })
    }

    pub(crate) fn id(&self) -> DisplayId {
        DisplayId(self.monitor.0 as usize)
    }

    pub(crate) fn from_id(id: DisplayId) -> Option<Self> {
        let monitor = HMONITOR(id.0 as *mut _);
        (!monitor.is_invalid()).then_some(Self { monitor })
    }

    pub(crate) fn scale_factor(&self) -> f32 {
        let mut dpi_x = USER_DEFAULT_SCREEN_DPI;
        let mut dpi_y = USER_DEFAULT_SCREEN_DPI;
        let _ =
            unsafe { GetDpiForMonitor(self.monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) };
        (dpi_x.max(USER_DEFAULT_SCREEN_DPI) as f32) / (USER_DEFAULT_SCREEN_DPI as f32)
    }

    pub(crate) fn visible_bounds(&self) -> Rect {
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !unsafe { GetMonitorInfoW(self.monitor, &mut info) }.as_bool() {
            return Rect::new(point(0.0, 0.0), size(1280.0, 800.0));
        }
        let scale = self.scale_factor();
        let rc = info.rcWork;
        Rect::new(
            point(rc.left as f32 / scale, rc.top as f32 / scale),
            size(
                (rc.right - rc.left) as f32 / scale,
                (rc.bottom - rc.top) as f32 / scale,
            ),
        )
    }

    pub(crate) fn centered_rect(&self, target: Size) -> Rect {
        let vis = self.visible_bounds();
        Rect::new(
            point(
                vis.origin.x + ((vis.size.width - target.width) * 0.5).max(0.0),
                vis.origin.y + ((vis.size.height - target.height) * 0.5).max(0.0),
            ),
            target,
        )
    }
}

const ENCODING_COMBO_ID: u32 = 1001;

#[derive(Clone, Debug, Default)]
pub(crate) struct OpenDialogOptions {
    pub(crate) multiple: bool,
    pub(crate) directory: Option<PathBuf>,
    pub(crate) file_types: Vec<(SharedStr, SharedStr)>,
}

#[derive(Clone, Debug)]
pub(crate) struct SaveChoice {
    pub(crate) label: SharedStr,
    pub(crate) items: Vec<SharedStr>,
    pub(crate) selected: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct SaveDialogOptions {
    pub(crate) title: Option<SharedStr>,
    pub(crate) directory: PathBuf,
    pub(crate) file_name: Option<String>,
    pub(crate) file_types: Vec<(SharedStr, SharedStr)>,
    pub(crate) default_extension: Option<SharedStr>,
    pub(crate) choice: Option<SaveChoice>,
}

pub(crate) type SaveDialogResult = Result<Option<(PathBuf, Option<usize>)>>;

struct OleGuard(bool);

impl OleGuard {
    fn init() -> Self {
        Self(unsafe { OleInitialize(None) }.is_ok())
    }
}

impl Drop for OleGuard {
    fn drop(&mut self) {
        if self.0 {
            unsafe { OleUninitialize() };
        }
    }
}

unsafe fn shell_item_path(item: &IShellItem) -> Result<PathBuf> {
    let pwstr = unsafe { item.GetDisplayName(SIGDN_FILESYSPATH)? };
    let path = PathBuf::from(OsString::from_wide(unsafe { pwstr.as_wide() }));
    unsafe { CoTaskMemFree(Some(pwstr.0.cast())) };
    Ok(path)
}

unsafe fn configure_dialog(
    dialog: &IFileDialog,
    directory: Option<&Path>,
    file_types: &[(SharedStr, SharedStr)],
) -> Result<()> {
    if let Some(dir) = directory.filter(|d| !d.as_os_str().is_empty()) {
        let wide = HSTRING::from(dir.as_os_str());
        if let Ok(folder) =
            unsafe { SHCreateItemFromParsingName::<_, _, IShellItem>(&wide, None) }
        {
            let _ = unsafe { dialog.SetFolder(&folder) };
        }
    }
    if !file_types.is_empty() {
        let wide_types: Vec<(HSTRING, HSTRING)> = file_types
            .iter()
            .map(|(label, spec)| (HSTRING::from(label.as_str()), HSTRING::from(spec.as_str())))
            .collect();
        let specs: Vec<COMDLG_FILTERSPEC> = wide_types
            .iter()
            .map(|(label, spec)| COMDLG_FILTERSPEC {
                pszName: PCWSTR(label.as_ptr()),
                pszSpec: PCWSTR(spec.as_ptr()),
            })
            .collect();
        unsafe { dialog.SetFileTypes(&specs)? };
    }
    Ok(())
}

unsafe fn show_dialog(dialog: &IFileDialog, owner: WindowHandle) -> Result<bool> {
    let parent = (!owner.is_null()).then(|| owner.raw());
    match unsafe { dialog.Show(parent) } {
        Ok(()) => Ok(true),
        Err(err) if err.code() == ERROR_CANCELLED.to_hresult() => Ok(false),
        Err(err) => Err(err.into()),
    }
}

pub(crate) fn run_open_dialog(
    owner: WindowHandle,
    options: OpenDialogOptions,
) -> Result<Option<Vec<PathBuf>>> {
    let _ole = OleGuard::init();
    unsafe {
        let dialog: IFileOpenDialog =
            CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
        let mut flags = dialog.GetOptions()? | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST;
        if options.multiple {
            flags |= FOS_ALLOWMULTISELECT;
        }
        dialog.SetOptions(flags)?;
        configure_dialog(&dialog, options.directory.as_deref(), &options.file_types)?;

        if !show_dialog(&dialog, owner)? {
            return Ok(None);
        }
        let items = dialog.GetResults()?;
        let count = items.GetCount()?;
        let mut paths = Vec::with_capacity(count as usize);
        for ix in 0..count {
            let item = items.GetItemAt(ix)?;
            paths.push(shell_item_path(&item)?);
        }
        Ok(Some(paths))
    }
}

pub(crate) fn run_save_dialog(owner: WindowHandle, options: SaveDialogOptions) -> SaveDialogResult {
    let _ole = OleGuard::init();
    unsafe {
        let dialog: IFileSaveDialog =
            CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER)?;
        let flags =
            dialog.GetOptions()? | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST | FOS_OVERWRITEPROMPT;
        dialog.SetOptions(flags)?;

        if let Some(title) = &options.title {
            dialog.SetTitle(&HSTRING::from(title.as_str()))?;
        }
        configure_dialog(&dialog, Some(&options.directory), &options.file_types)?;
        if let Some(name) = &options.file_name {
            dialog.SetFileName(&HSTRING::from(name.as_str()))?;
        }
        if let Some(ext) = &options.default_extension {
            dialog.SetDefaultExtension(&HSTRING::from(ext.as_str()))?;
        }

        let customize: Option<IFileDialogCustomize> = options.choice.as_ref().and_then(|choice| {
            let cust = dialog.cast::<IFileDialogCustomize>().ok()?;
            let _ = cust.StartVisualGroup(ENCODING_COMBO_ID + 100, &HSTRING::from(choice.label.as_str()));
            let _ = cust.AddComboBox(ENCODING_COMBO_ID);
            for (ix, item) in choice.items.iter().enumerate() {
                let _ = cust.AddControlItem(ENCODING_COMBO_ID, ix as u32, &HSTRING::from(item.as_str()));
            }
            let _ = cust.SetSelectedControlItem(ENCODING_COMBO_ID, choice.selected as u32);
            let _ = cust.EndVisualGroup();
            Some(cust)
        });

        if !show_dialog(&dialog, owner)? {
            return Ok(None);
        }

        let item = dialog.GetResult()?;
        let path = shell_item_path(&item)?;
        let selected = customize.and_then(|cust| {
            cust.GetSelectedControlItem(ENCODING_COMBO_ID)
                .ok()
                .map(|id| id as usize)
        });
        Ok(Some((path, selected)))
    }
}

const OPEN_RETRIES: usize = 8;
const OPEN_RETRY_DELAY: Duration = Duration::from_millis(12);

pub(crate) fn clipboard_has_text() -> bool {
    unsafe { IsClipboardFormatAvailable(CF_UNICODETEXT.0 as u32) }.is_ok()
}

pub(crate) fn write_clipboard_text(text: &str) -> bool {
    let Some(_clip) = ClipboardGuard::open() else {
        return false;
    };
    (|| -> Result<()> {
        unsafe { EmptyClipboard()? };
        let crlf = normalize_for_clipboard(text);
        let wide: Vec<u16> = crlf.encode_utf16().chain(Some(0)).collect();
        set_clipboard_bytes(&wide, CF_UNICODETEXT.0 as u32)
    })()
    .is_ok()
}

pub(crate) fn read_clipboard_text() -> Option<String> {
    let _clip = ClipboardGuard::open()?;
    let global = HGLOBAL(unsafe { GetClipboardData(CF_UNICODETEXT.0 as u32).ok() }?.0);
    let locked = LockedGlobal::lock(global)?;
    let bytes = locked.as_bytes();
    let words_len = bytes.len() / std::mem::size_of::<u16>();
    if words_len == 0 {
        return Some(String::new());
    }
    let slice = unsafe { std::slice::from_raw_parts(bytes.as_ptr() as *const u16, words_len) };
    let actual_len = slice.iter().position(|&c| c == 0).unwrap_or(words_len);
    Some(String::from_utf16_lossy(&slice[..actual_len]))
}

fn normalize_for_clipboard(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + text.bytes().filter(|&b| b == b'\n').count());
    let mut prev_cr = false;
    for ch in text.chars() {
        match ch {
            '\0' => {
                out.push(' ');
                prev_cr = false;
            }
            '\n' if !prev_cr => {
                out.push_str("\r\n");
                prev_cr = false;
            }
            '\r' => {
                out.push('\r');
                prev_cr = true;
            }
            other => {
                out.push(other);
                prev_cr = false;
            }
        }
    }
    out
}

fn set_clipboard_bytes<T>(data: &[T], format: u32) -> Result<()> {
    unsafe {
        let global = Owned::new(GlobalAlloc(GMEM_MOVEABLE, std::mem::size_of_val(data))?);
        let ptr = GlobalLock(*global);
        if ptr.is_null() {
            return Err(Error::new("GlobalLock returned null"));
        }
        std::ptr::copy_nonoverlapping(data.as_ptr(), ptr as *mut T, data.len());
        let _ = GlobalUnlock(*global);
        SetClipboardData(format, Some(HANDLE(global.0)))?;
        // SetClipboardData succeeded; OS owns the handle now.
        std::mem::forget(global);
    }
    Ok(())
}

struct ClipboardGuard;

impl ClipboardGuard {
    fn open() -> Option<Self> {
        for attempt in 0..OPEN_RETRIES {
            if unsafe { OpenClipboard(None) }.is_ok() {
                return Some(Self);
            }
            if attempt + 1 < OPEN_RETRIES {
                thread::sleep(OPEN_RETRY_DELAY);
            }
        }
        None
    }
}

impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        let _ = unsafe { CloseClipboard() };
    }
}

struct LockedGlobal {
    global: HGLOBAL,
    ptr: *const u8,
    size: usize,
}

impl LockedGlobal {
    fn lock(global: HGLOBAL) -> Option<Self> {
        let size = unsafe { GlobalSize(global) };
        let ptr = unsafe { GlobalLock(global) };
        if ptr.is_null() {
            return None;
        }
        Some(Self {
            global,
            ptr: ptr as *const u8,
            size,
        })
    }

    fn as_bytes(&self) -> &[u8] {
        if self.size == 0 {
            return &[];
        }
        unsafe { std::slice::from_raw_parts(self.ptr, self.size) }
    }
}

impl Drop for LockedGlobal {
    fn drop(&mut self) {
        unsafe {
            let _ = GlobalUnlock(self.global);
        }
    }
}

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

    pub(crate) fn is_dbcs_lead_byte(self, byte: u8) -> bool {
        if self.0 == CP_UTF8 || self.0 == 1252 {
            return false;
        }
        unsafe { IsDBCSLeadByteEx(self.0, byte) }.is_ok()
    }

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
        let length = unsafe { MultiByteToWideChar(self.0, MB_ERR_INVALID_CHARS, bytes, None) };
        let Ok(needed) = usize::try_from(length) else {
            return Ok(false);
        };
        if needed == 0 {
            return Ok(false);
        }
        wide.clear();
        if wide.capacity() < needed {
            wide.try_reserve(needed - wide.capacity()).map_err(|_| {
                io::Error::new(io::ErrorKind::OutOfMemory, "Out of memory decoding file.")
            })?;
        }
        wide.resize(needed, 0);
        let written = unsafe {
            MultiByteToWideChar(self.0, MB_ERR_INVALID_CHARS, bytes, Some(wide.as_mut_slice()))
        };
        let Ok(written) = usize::try_from(written) else {
            return Ok(false);
        };
        if written == 0 {
            return Ok(false);
        }
        wide.truncate(written);
        out.try_reserve(written).map_err(|_| {
            io::Error::new(io::ErrorKind::OutOfMemory, "Out of memory decoding file.")
        })?;
        for ch in char::decode_utf16(wide.iter().copied()) {
            let Ok(ch) = ch else {
                return Ok(false);
            };
            out.push(ch);
        }
        Ok(true)
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

pub(crate) const HIDDEN_FILE_ATTRIBUTE: u32 = FILE_ATTRIBUTE_HIDDEN.0;

pub(crate) fn documents_dir() -> Option<PathBuf> {
    let path = unsafe { SHGetKnownFolderPath(&FOLDERID_Documents, KF_FLAG_DEFAULT, None) }.ok()?;
    let documents = (!path.0.is_null()).then(|| {
        PathBuf::from(OsString::from_wide(unsafe { path.as_wide() }))
    });
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

pub(crate) fn replace_file(
    target: &Path,
    replacement: &Path,
    backup: &Path,
) -> std::result::Result<(), ReplaceFileError> {
    let wide = |path: &Path| -> Vec<u16> { path.as_os_str().encode_wide().chain(Some(0)).collect() };
    let (replaced, replacement_w, backed_up) = (wide(target), wide(replacement), wide(backup));
    let res = unsafe {
        ReplaceFileW(
            PCWSTR(replaced.as_ptr()),
            PCWSTR(replacement_w.as_ptr()),
            PCWSTR(backed_up.as_ptr()),
            REPLACEFILE_IGNORE_MERGE_ERRORS | REPLACEFILE_IGNORE_ACL_ERRORS,
            None,
            None,
        )
    };
    let Err(err) = res else {
        return Ok(());
    };
    let code = err.code();
    let win32_err = (code.0 as u32) & 0xFFFF;
    let io_err = io::Error::from_raw_os_error(win32_err as i32);
    if code == ERROR_UNABLE_TO_MOVE_REPLACEMENT_2.to_hresult() {
        Err(ReplaceFileError::UnableToMoveReplacement(io_err))
    } else if code == ERROR_SHARING_VIOLATION.to_hresult()
        || code == ERROR_LOCK_VIOLATION.to_hresult()
        || code == ERROR_ACCESS_DENIED.to_hresult()
    {
        Err(ReplaceFileError::SharingOrAccess(io_err))
    } else {
        Err(ReplaceFileError::Other(io_err))
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::mpsc;

    use windows::Win32::UI::WindowsAndMessaging::{
        SendMessageW, WM_ACTIVATE, WM_ENTERSIZEMOVE, WM_EXITSIZEMOVE, WM_TIMER,
    };

    use super::*;

    #[test]
    fn hwnd_registry_cleanup_reentrant_queue_and_modal_size_move_timer() {
        #[derive(Default)]
        struct Probe {
            activated: Vec<bool>,
            wakes: usize,
        }

        impl PlatformHandler for Probe {
            fn on_event(&mut self, event: PlatformEvent) -> bool {
                if let PlatformEvent::Activated { active, .. } = event {
                    self.activated.push(active);
                }
                false
            }

            fn on_wake(&mut self) {
                self.wakes += 1;
            }

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
        assert!(lookup_window(hwnd).is_some(), "open window must be registered");

        let probe = Rc::new(RefCell::new(Probe::default()));
        Platform::with_test_handler(probe.clone(), || {
            // Re-entrant WM_ACTIVATE while handler is borrowed must queue and apply on service().
            {
                let _borrow = probe.borrow_mut();
                unsafe {
                    let _ = SendMessageW(hwnd, WM_ACTIVATE, Some(WPARAM(1)), Some(LPARAM(0)));
                }
            }
            assert!(probe.borrow().activated.is_empty());
            service();
            assert_eq!(probe.borrow().activated, vec![true]);
            assert!(win.is_active());

            unsafe {
                let _ = SendMessageW(hwnd, WM_ENTERSIZEMOVE, Some(WPARAM(0)), Some(LPARAM(0)));
            }
            assert!(win.in_size_move());
            let wakes_before = probe.borrow().wakes;
            unsafe {
                let _ = SendMessageW(hwnd, WM_TIMER, Some(WPARAM(1)), Some(LPARAM(0)));
            }
            assert!(
                probe.borrow().wakes > wakes_before,
                "WM_TIMER during modal size/move must invoke on_wake"
            );
            unsafe {
                let _ = SendMessageW(hwnd, WM_EXITSIZEMOVE, Some(WPARAM(0)), Some(LPARAM(0)));
            }
            assert!(!win.in_size_move());
        });

        drop(win);
        assert!(lookup_window(hwnd).is_none(), "closing window must unregister");
    }

    #[test]
    fn real_win32_message_loop_wakes_timers_and_quits_cleanly() {
        struct LoopProbe {
            platform: Platform,
            woke: bool,
        }

        impl PlatformHandler for LoopProbe {
            fn on_event(&mut self, _event: PlatformEvent) -> bool {
                false
            }

            fn on_wake(&mut self) {
                self.woke = true;
                self.platform.post_quit();
            }

            fn drive_frames(&mut self) {}
        }

        let platform = Platform::new(None).expect("Platform::new failed");
        let probe = Rc::new(RefCell::new(LoopProbe {
            platform,
            woke: false,
        }));
        Platform::run(probe.clone(), |p| {
            p.platform
                .wake_at(Some(Instant::now() + Duration::from_millis(25)));
        });
        assert!(probe.borrow().woke);
    }

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

    #[test]
    fn encodes_and_decodes_copydata_round_trip() {
        let input = vec![
            PathBuf::from(r"C:\Temp\hello world.txt"),
            PathBuf::from(r"C:\Temp\日本語.txt"),
        ];
        let bytes = encode_paths(&input);
        let cds = COPYDATASTRUCT {
            dwData: COPYDATA_TAG,
            cbData: bytes.len() as u32,
            lpData: bytes.as_ptr() as *mut c_void,
        };
        assert_eq!(decode_copydata(&cds), Some(input));

        let nw = COPYDATASTRUCT {
            dwData: COPYDATA_TAG,
            cbData: 0,
            lpData: std::ptr::null_mut(),
        };
        assert_eq!(decode_copydata(&nw), Some(Vec::new()));

        let bad_tag = COPYDATASTRUCT {
            dwData: 0xDEAD_BEEF,
            cbData: 0,
            lpData: std::ptr::null_mut(),
        };
        assert_eq!(decode_copydata(&bad_tag), None);

        let bad_len = COPYDATASTRUCT {
            dwData: COPYDATA_TAG,
            cbData: (bytes.len() - 1) as u32,
            lpData: bytes.as_ptr() as *mut c_void,
        };
        assert_eq!(decode_copydata(&bad_len), None);
    }

    #[test]
    fn mutex_and_copydata_handoff_delivers_concurrently_and_rejects_after_stop() {
        struct Server {
            platform: Platform,
            tx: mpsc::Sender<Vec<PathBuf>>,
            ctrl: mpsc::Receiver<bool>,
            stop_tx: Option<mpsc::Sender<()>>,
        }

        impl PlatformHandler for Server {
            fn on_event(&mut self, event: PlatformEvent) -> bool {
                if let PlatformEvent::Launch { paths } = event {
                    let _ = self.tx.send(paths);
                }
                false
            }
            fn on_wake(&mut self) {
                while let Ok(quit) = self.ctrl.try_recv() {
                    if quit {
                        self.platform.post_quit();
                    } else {
                        self.platform.disable_handoff();
                        if let Some(tx) = self.stop_tx.take() {
                            let _ = tx.send(());
                        }
                    }
                }
            }
            fn drive_frames(&mut self) {}
        }

        let identity = ProcessIdentity::current().expect("current process identity");
        let key = format!("Test.CopyData.{}", std::process::id());
        let window_title = format!("Jot.Handoff.{key}");

        let (tx, rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let (stop_tx, stop_rx) = mpsc::channel();
        let (ctrl_tx, ctrl_rx) = mpsc::channel();

        let server_key = key.clone();
        let pump = std::thread::spawn(move || {
            let id = ProcessIdentity::current().expect("server process identity");
            let Arbitration::Primary(Some(lock)) = arbitrate_for(&id, &server_key, &[]) else {
                panic!("first process must claim mutex");
            };
            let platform = Platform::new(Some(lock)).expect("Platform::new");
            let (post_tx, ctrl) = mpsc::channel();
            let waker = platform.waker();
            let _ = ctrl_tx.send(move |quit| {
                let _ = post_tx.send(quit);
                waker.wake();
            });
            let server = Rc::new(RefCell::new(Server {
                platform,
                tx,
                ctrl,
                stop_tx: Some(stop_tx),
            }));
            Platform::run(server, |_| {
                let _ = ready_tx.send(());
            });
        });
        let post_ctrl = ctrl_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        ready_rx.recv_timeout(Duration::from_secs(2)).unwrap();

        assert!(matches!(
            arbitrate_for(&identity, &key, &[]),
            Arbitration::HandedOff
        ));
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), Vec::<PathBuf>::new());

        let mut threads = Vec::new();
        for i in 0..5 {
            let k = key.clone();
            threads.push(std::thread::spawn(move || {
                let id = ProcessIdentity::current().unwrap();
                let paths = vec![PathBuf::from(format!(r"C:\Temp\multi_{i}_日本語.txt"))];
                matches!(arbitrate_for(&id, &k, &paths), Arbitration::HandedOff)
            }));
        }
        for t in threads {
            assert!(t.join().unwrap(), "concurrent handoff must succeed");
        }

        let mut received = Vec::new();
        for _ in 0..5 {
            received.extend(rx.recv_timeout(Duration::from_secs(3)).unwrap());
        }
        received.sort();
        assert_eq!(received.len(), 5);
        for (i, path) in received.iter().enumerate() {
            assert_eq!(
                path,
                &PathBuf::from(format!(r"C:\Temp\multi_{i}_日本語.txt"))
            );
        }

        post_ctrl(false);
        stop_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(
            !send_handoff(&identity, WAKE_CLASS_NAME, &window_title, &[]),
            "stopped server must reject late handoffs"
        );

        post_ctrl(true);
        let _ = pump.join();
    }

    #[test]
    fn clipboard_normalization_converts_lf_to_crlf_without_doubling() {
        assert_eq!(normalize_for_clipboard("a\nb\nc"), "a\r\nb\r\nc");
        assert_eq!(normalize_for_clipboard("a\r\nb\n"), "a\r\nb\r\n");
        assert_eq!(normalize_for_clipboard("a\0b"), "a b");
    }
}
