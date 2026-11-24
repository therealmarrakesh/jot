#![allow(unsafe_code)]

pub(crate) mod clipboard;
mod codepage;
pub(crate) mod diag;
pub(crate) mod dialogs;
mod fs;
pub(crate) mod input;
mod instance;
pub(crate) mod jobs;
pub(crate) mod os;
mod settings;
pub(crate) mod window;

pub(crate) use codepage::AnsiCodePage;
pub(crate) use dialogs::{
    OpenDialogOptions, SaveChoice, SaveDialogOptions, SaveDialogResult, run_open_dialog,
    run_save_dialog,
};
pub(crate) use fs::{
    HIDDEN_FILE_ATTRIBUTE, ReplaceFileError, documents_dir, file_hard_link_count, replace_file,
};
pub(crate) use input::{Key, KeyEvent, Modifiers, MouseButton};
pub(crate) use instance::{Arbitration, HandoffCommand, arbitrate as arbitrate_instance};
pub(crate) use jobs::{Jobs, TimerHandle};
pub(crate) use os::{
    WindowHwnd, WindowPlacement, cursor_screen_pos, monitor_work_area_and_scale,
    primary_mouse_button_down, release_mouse_capture, top_visible_window_at,
};
pub(crate) use settings::{
    HighContrastColors, SystemSettings, format_local_time_date, system_settings,
};
pub(crate) use window::{
    CursorStyle, MainWindowConfig, NativeWindow, PlatformEvent, PlatformHandler, WindowAppearance,
    WindowControlArea, WindowId,
};

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use windows::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM},
    Graphics::{
        Dwm::DwmFlush,
        Gdi::{
            GetMonitorInfoW, HMONITOR, MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTOPRIMARY,
            MONITORINFO, MonitorFromPoint,
        },
    },
    System::{
        Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize},
        DataExchange::COPYDATASTRUCT,
        Diagnostics::Debug::OutputDebugStringW,
        LibraryLoader::GetModuleHandleW,
    },
    UI::{
        HiDpi::{
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForMonitor, MDT_EFFECTIVE_DPI,
            SetProcessDpiAwarenessContext,
        },
        WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
            HWND_MESSAGE, KillTimer, MB_ICONERROR, MB_OK, MSG, MessageBoxW, PostMessageW,
            PostQuitMessage, RegisterClassW, SetTimer, SetWindowTextW, TranslateMessage,
            USER_DEFAULT_SCREEN_DPI, USER_TIMER_MAXIMUM, USER_TIMER_MINIMUM, WM_APP, WM_COPYDATA,
            WM_TIMER, WINDOW_EX_STYLE, WINDOW_STYLE, WNDCLASSW,
        },
    },
};
use windows::core::{HSTRING, PCWSTR, w};

use crate::geom::{Rect, Size, point, size};
use crate::types::Result;

const WM_APP_WAKE: u32 = WM_APP + 1;
const WM_APP_VSYNC: u32 = WM_APP + 2;
const JOBS_TIMER_ID: usize = 2;
pub(super) const WAKE_CLASS_NAME: PCWSTR = w!("JotWakeWindow");

pub(crate) fn output_debug_string(message: &str) {
    let wide = HSTRING::from(message);
    unsafe { OutputDebugStringW(&wide) };
}

pub(crate) fn message_box_error(title: &str, message: &str) {
    let title_w = HSTRING::from(title);
    let msg_w = HSTRING::from(message);
    unsafe {
        let _ = MessageBoxW(None, &msg_w, &title_w, MB_OK | MB_ICONERROR);
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct DisplayId(pub(crate) usize);

#[derive(Copy, Clone, Debug)]
pub(crate) struct WindowsDisplay {
    monitor: HMONITOR,
}

impl WindowsDisplay {
    pub(crate) fn primary() -> Option<Self> {
        let monitor = unsafe { MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY) };
        (!monitor.is_invalid()).then_some(Self { monitor })
    }

    pub(crate) fn from_point(pt: crate::geom::ScreenPoint) -> Option<Self> {
        let monitor = unsafe {
            MonitorFromPoint(
                POINT { x: pt.x, y: pt.y },
                MONITOR_DEFAULTTONEAREST,
            )
        };
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

thread_local! {
    pub(super) static ACTIVE_HANDLER: RefCell<Option<Rc<RefCell<dyn PlatformHandler>>>> =
        const { RefCell::new(None) };
    static WAKE_PENDING: Cell<bool> = const { Cell::new(false) };
    static HANDOFF_ACCEPTING: Cell<bool> = const { Cell::new(false) };
}

pub(super) fn mark_wake_pending() {
    WAKE_PENDING.with(|flag| flag.set(true));
}

pub(super) fn take_wake_pending() -> bool {
    WAKE_PENDING.with(|flag| flag.replace(false))
}

struct ActiveHandlerGuard;

impl Drop for ActiveHandlerGuard {
    fn drop(&mut self) {
        ACTIVE_HANDLER.with(|slot| *slot.borrow_mut() = None);
        WAKE_PENDING.with(|flag| flag.set(false));
    }
}

struct VsyncPacer {
    armed: Mutex<bool>,
    condvar: Condvar,
}

impl VsyncPacer {
    fn spawn(wake_hwnd_raw: isize) -> Arc<Self> {
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
            })
            .expect("failed to spawn vsync thread");
        pacer
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
}

impl Platform {
    pub(crate) fn new() -> Result<Self> {
        unsafe {
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }
        let com_initialized =
            unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
        window::register_window_class()?;

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

        let vsync = VsyncPacer::spawn(wake_hwnd.0 as isize);

        Ok(Self {
            wake_hwnd,
            vsync,
            com_initialized,
        })
    }

    pub(crate) fn enable_handoff(&self, title: &str) {
        HANDOFF_ACCEPTING.with(|flag| flag.set(true));
        let wide = HSTRING::from(title);
        let _ = unsafe { SetWindowTextW(self.wake_hwnd, &wide) };
    }

    fn disable_handoff(&self) {
        HANDOFF_ACCEPTING.with(|flag| flag.set(false));
        let _ = unsafe { SetWindowTextW(self.wake_hwnd, w!("")) };
    }

    pub(super) fn wake_callback(&self) -> Arc<dyn Fn() + Send + Sync> {
        let wake_raw = self.wake_hwnd.0 as isize;
        Arc::new(move || {
            unsafe {
                let _ = PostMessageW(
                    Some(HWND(wake_raw as *mut _)),
                    WM_APP_WAKE,
                    WPARAM(0),
                    LPARAM(0),
                );
            }
        })
    }

    pub(super) fn timer_callback(&self) -> Box<dyn Fn(Option<u32>)> {
        let wake_hwnd = self.wake_hwnd;
        Box::new(move |delay_ms| match delay_ms {
            Some(0) => unsafe {
                let _ = PostMessageW(Some(wake_hwnd), WM_APP_WAKE, WPARAM(0), LPARAM(0));
            },
            Some(ms) => unsafe {
                let clamped = ms.clamp(USER_TIMER_MINIMUM, USER_TIMER_MAXIMUM);
                let _ = SetTimer(Some(wake_hwnd), JOBS_TIMER_ID, clamped, None);
            },
            None => unsafe {
                let _ = KillTimer(Some(wake_hwnd), JOBS_TIMER_ID);
            },
        })
    }

    pub(crate) fn request_frame_tick(&self) {
        self.vsync.arm();
    }

    pub(crate) fn post_quit(&self) {
        self.disable_handoff();
        unsafe { PostQuitMessage(0) };
    }

    #[cfg(test)]
    pub(crate) fn set_active_handler_for_test(handler: Option<Rc<RefCell<dyn PlatformHandler>>>) {
        ACTIVE_HANDLER.with(|slot| *slot.borrow_mut() = handler);
    }

    #[cfg(test)]
    pub(crate) fn drain_pending_events_for_test() {
        window::service();
    }

    pub(crate) fn run<H: PlatformHandler + 'static>(
        handler: Rc<RefCell<H>>,
        on_ready: impl FnOnce(&mut H),
    ) {
        let dyn_handler: Rc<RefCell<dyn PlatformHandler>> = handler.clone();
        ACTIVE_HANDLER.with(|slot| *slot.borrow_mut() = Some(dyn_handler));
        let _guard = ActiveHandlerGuard;

        {
            let mut h = handler.borrow_mut();
            on_ready(&mut h);
        }
        window::service();

        let mut msg = MSG::default();
        while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            window::service();
        }
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
    diag::ffi_guard("wake_wnd_proc", LRESULT(0), || {
        wake_wnd_proc_inner(hwnd, msg, wparam, lparam)
    })
}

fn wake_wnd_proc_inner(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_APP_WAKE => {
            mark_wake_pending();
            window::service();
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == JOBS_TIMER_ID => {
            let _ = unsafe { KillTimer(Some(hwnd), JOBS_TIMER_ID) };
            mark_wake_pending();
            window::service();
            LRESULT(0)
        }
        WM_APP_VSYNC => {
            window::service();
            LRESULT(0)
        }
        WM_COPYDATA => {
            if !HANDOFF_ACCEPTING.with(Cell::get) {
                return LRESULT(0);
            }
            let cds = unsafe { &*(lparam.0 as *const COPYDATASTRUCT) };
            let Some(cmd) = instance::decode_copydata(cds) else {
                return LRESULT(0);
            };
            window::dispatch(PlatformEvent::Handoff(cmd));
            LRESULT(1)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}
