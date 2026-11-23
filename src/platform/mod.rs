//! Unified Win32 platform layer: display enumeration, message loop, vsync pacing, windowing,
//! native dialogs, clipboard, named pipes, OS helpers, and typed jobs/timers.

pub(crate) mod clipboard;
pub(crate) mod dialogs;
mod drop_target;
pub(crate) mod input;
pub(crate) mod jobs;
pub(crate) mod os;
pub(crate) mod pipe;
pub(crate) mod window;

pub(crate) use dialogs::{
    OpenDialogOptions, SaveChoice, SaveDialogOptions, SaveDialogResult, run_open_dialog,
    run_save_dialog,
};
pub(crate) use input::{Key, KeyEvent, Modifiers, MouseButton};
pub(crate) use jobs::{Jobs, TimerHandle};
pub(crate) use os::{
    AnsiCodePage, HIDDEN_FILE_ATTRIBUTE, HighContrastColors, ReplaceFileError, WindowHwnd,
    WindowPlacement, caret_blink_interval, caret_blink_timeout, caret_width_px,
    client_area_animations_enabled, cursor_screen_pos, documents_dir, file_hard_link_count,
    format_local_time_date, high_contrast_colors, menu_show_delay_ms, monitor_work_area_and_scale,
    mouse_hover_time_ms, primary_mouse_button_down, release_mouse_capture, replace_file,
    system_accent_color, top_visible_window_at,
};
pub(crate) use pipe::{
    PipeClaim, PipeServer, current_process_identity, send_framed_handoff, try_claim_pipe_server,
};
pub(crate) use window::{
    CursorStyle, NativeWindow, PlatformEvent, PlatformHandler, WindowAppearance, WindowBackdrop,
    WindowControlArea, WindowId, WindowKind, WindowOptions,
};

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;
#[cfg(not(test))]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
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
        Diagnostics::Debug::OutputDebugStringW,
        LibraryLoader::GetModuleHandleW,
        Ole::{OleInitialize, OleUninitialize},
        Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW},
    },
    UI::{
        HiDpi::{
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForMonitor, MDT_EFFECTIVE_DPI,
            SetProcessDpiAwarenessContext,
        },
        WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
            HWND_MESSAGE, MB_ICONERROR, MB_OK, MSG, MessageBoxW, PostMessageW, PostQuitMessage,
            RegisterClassW, TranslateMessage, USER_DEFAULT_SCREEN_DPI, WM_APP, WINDOW_EX_STYLE,
            WINDOW_STYLE, WNDCLASSW,
        },
    },
};
use windows::core::{HSTRING, PCWSTR, w};

use crate::geom::{Rect, Size, point, size};
use crate::gfx::Direct2DDevices;
use crate::types::{Error, Result};

const WM_APP_WAKE: u32 = WM_APP + 1;
const WM_APP_VSYNC: u32 = WM_APP + 2;
const WAKE_CLASS_NAME: PCWSTR = w!("JotWakeWindow");
const DEBUG_RING_CAPACITY: usize = 64;

fn debug_ring() -> &'static Mutex<VecDeque<String>> {
    static RING: OnceLock<Mutex<VecDeque<String>>> = OnceLock::new();
    RING.get_or_init(|| Mutex::new(VecDeque::with_capacity(DEBUG_RING_CAPACITY)))
}

/// Logs a diagnostic line to the in-memory ring buffer and the Windows debug output stream
/// (`OutputDebugStringW`) without touching disk.
pub(crate) fn debug_log(message: &str) {
    {
        let mut ring = debug_ring().lock().unwrap_or_else(|e| e.into_inner());
        if ring.len() >= DEBUG_RING_CAPACITY {
            ring.pop_front();
        }
        ring.push_back(message.to_owned());
    }
    let wide = HSTRING::from(format!("[Jot] {message}\n"));
    // SAFETY: `wide` is a valid NUL-terminated UTF-16 string for the duration of the call.
    unsafe { OutputDebugStringW(&wide) };
}

/// Displays a native Win32 error dialog (used only for fatal startup or caught FFI-boundary errors).
pub(crate) fn show_error_dialog(title: &str, message: &str) {
    let title_w = HSTRING::from(title);
    let msg_w = HSTRING::from(message);
    // SAFETY: Both HSTRING pointers are valid NUL-terminated UTF-16 strings.
    unsafe {
        let _ = MessageBoxW(None, &msg_w, &title_w, MB_OK | MB_ICONERROR);
    }
}

/// Installs a process-wide panic hook that records panics to `OutputDebugStringW` and the
/// in-memory diagnostic ring buffer.
pub(crate) fn install_panic_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let msg = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_owned()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "unknown panic".to_owned()
        };
        let loc = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "<unknown>".to_owned());
        debug_log(&format!("PANIC at {loc}: {msg}"));
        prev(info);
    }));
}

pub(super) fn report_caught_ffi_panic(context: &str, payload: &(dyn std::any::Any + Send)) {
    let msg = jobs::panic_message(payload);
    debug_log(&format!("Caught panic at Win32 boundary ({context}): {msg}"));
    #[cfg(not(test))]
    {
        static SHOWN: AtomicBool = AtomicBool::new(false);
        if !SHOWN.swap(true, Ordering::AcqRel) {
            show_error_dialog(
                "Jot",
                &format!(
                    "Jot encountered an unexpected error and prevented a crash.\n\n\
                     Please save any open documents and restart Jot.\n\nDetails: {msg}"
                ),
            );
        }
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
        // SAFETY: `MonitorFromPoint` accepts any coordinates with `MONITOR_DEFAULTTOPRIMARY`.
        let monitor = unsafe { MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY) };
        (!monitor.is_invalid()).then_some(Self { monitor })
    }

    pub(crate) fn from_point(pt: crate::geom::ScreenPoint) -> Option<Self> {
        // SAFETY: `MonitorFromPoint` accepts any coordinates with `MONITOR_DEFAULTTONEAREST`.
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
        // SAFETY: `self.monitor` is a valid HMONITOR and out-pointers point to stack locals.
        let _ =
            unsafe { GetDpiForMonitor(self.monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) };
        (dpi_x.max(USER_DEFAULT_SCREEN_DPI) as f32) / (USER_DEFAULT_SCREEN_DPI as f32)
    }

    pub(crate) fn visible_bounds(&self) -> Rect {
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        // SAFETY: `info.cbSize` is initialized and `self.monitor` is a valid HMONITOR.
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

/// Pacing thread that blocks in `Condvar` at 0% CPU until armed, then waits one `DwmFlush`
/// and posts `WM_APP_VSYNC` back to the hidden UI wake window.
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
                // SAFETY: `DwmFlush` synchronizes with DWM composition; if unavailable, fall back
                // to a ~60 Hz sleep so animations cannot busy-spin.
                if unsafe { DwmFlush() }.is_err() {
                    std::thread::sleep(Duration::from_millis(16));
                }
                // SAFETY: `PostMessageW` is thread-safe; posting to a destroyed HWND fails harmlessly.
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
    devices: RefCell<Direct2DDevices>,
    gpu_generation: Cell<u64>,
    wake_hwnd: HWND,
    vsync: Arc<VsyncPacer>,
    com_initialized: bool,
    ole_initialized: bool,
}

impl Platform {
    pub(crate) fn new() -> Result<Self> {
        // SAFETY: Best-effort DPI awareness for test binaries without an embedded manifest.
        unsafe {
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }
        // SAFETY: STA COM initialization on the UI thread; matched by `CoUninitialize` in `Drop`.
        let com_initialized =
            unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
        // SAFETY: OLE initialization on the UI thread for drag-drop; matched in `Drop`.
        let ole_initialized = unsafe { OleInitialize(None) }.is_ok();
        window::register_window_class()?;

        // SAFETY: `GetModuleHandleW(None)` returns the current executable's module handle.
        let hinstance = unsafe { GetModuleHandleW(None)? };
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wake_wnd_proc),
            hInstance: hinstance.into(),
            lpszClassName: WAKE_CLASS_NAME,
            ..Default::default()
        };
        // SAFETY: `wc` has a static class name and valid `wake_wnd_proc`.
        unsafe { RegisterClassW(&wc) };

        // SAFETY: Creates a message-only wake window using the class registered above.
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

        let devices = Direct2DDevices::new()
            .map_err(|err| Error::new(format!("initializing Direct3D11/Direct2D devices: {err}")))?;
        let vsync = VsyncPacer::spawn(wake_hwnd.0 as isize);

        Ok(Self {
            devices: RefCell::new(devices),
            gpu_generation: Cell::new(0),
            wake_hwnd,
            vsync,
            com_initialized,
            ole_initialized,
        })
    }

    pub(crate) fn devices(&self) -> Direct2DDevices {
        self.devices.borrow().clone()
    }

    pub(crate) fn replace_devices(&self, next: Direct2DDevices) {
        *self.devices.borrow_mut() = next;
        self.gpu_generation
            .set(self.gpu_generation.get().wrapping_add(1));
    }

    #[cfg(test)]
    pub(crate) fn gpu_generation(&self) -> u64 {
        self.gpu_generation.get()
    }

    pub(crate) fn wake_callback(&self) -> Arc<dyn Fn() + Send + Sync> {
        let wake_raw = self.wake_hwnd.0 as isize;
        Arc::new(move || {
            // SAFETY: `PostMessageW` is thread-safe; posting to a destroyed HWND fails harmlessly.
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

    pub(crate) fn request_frame_tick(&self) {
        self.vsync.arm();
    }

    pub(crate) fn system_appearance(&self) -> WindowAppearance {
        let mut value = 1u32;
        let mut size = std::mem::size_of::<u32>() as u32;
        // SAFETY: Reading a `REG_DWORD` into a 4-byte stack local with static subkey/value strings.
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

    pub(crate) fn post_quit(&self) {
        // SAFETY: Posts `WM_QUIT` to the current UI thread's message queue.
        unsafe { PostQuitMessage(0) };
    }

    #[cfg(test)]
    pub(crate) fn set_active_handler_for_test(handler: Option<Rc<RefCell<dyn PlatformHandler>>>) {
        ACTIVE_HANDLER.with(|slot| *slot.borrow_mut() = handler);
    }

    #[cfg(test)]
    pub(crate) fn drain_pending_events_for_test() {
        window::drain_pending_events();
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
        window::drain_pending_events();
        handler.borrow_mut().drive_frames();

        let mut msg = MSG::default();
        loop {
            if handler.borrow().should_quit() {
                break;
            }
            // SAFETY: Pumps messages on the UI thread into stack-allocated `msg`.
            if unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 <= 0 {
                break;
            }
            // SAFETY: `msg` was populated by `GetMessageW`.
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            window::drain_pending_events();
        }
    }
}

impl Drop for Platform {
    fn drop(&mut self) {
        // SAFETY: Tears down owned wake HWND and balances OLE/COM initialization on the UI thread.
        unsafe {
            let _ = DestroyWindow(self.wake_hwnd);
            if self.ole_initialized {
                OleUninitialize();
            }
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
    match catch_unwind(AssertUnwindSafe(|| {
        wake_wnd_proc_inner(hwnd, msg, wparam, lparam)
    })) {
        Ok(res) => res,
        Err(payload) => {
            report_caught_ffi_panic("wake_wnd_proc", &*payload);
            LRESULT(0)
        }
    }
}

fn wake_wnd_proc_inner(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_APP_WAKE => {
            if let Some(handler_rc) = ACTIVE_HANDLER.with(|slot| slot.borrow().clone()) {
                match handler_rc.try_borrow_mut() {
                    Ok(mut handler) => {
                        WAKE_PENDING.with(|flag| flag.set(false));
                        handler.on_wake();
                    }
                    Err(_) => {
                        WAKE_PENDING.with(|flag| flag.set(true));
                        return LRESULT(0);
                    }
                }
                window::drain_pending_events();
                if let Ok(mut handler) = handler_rc.try_borrow_mut() {
                    handler.drive_frames();
                }
            }
            LRESULT(0)
        }
        WM_APP_VSYNC => {
            if let Some(handler_rc) = ACTIVE_HANDLER.with(|slot| slot.borrow().clone()) {
                window::drain_pending_events();
                if let Ok(mut handler) = handler_rc.try_borrow_mut() {
                    handler.drive_frames();
                }
            }
            LRESULT(0)
        }
        // SAFETY: Forwards unhandled wake-window messages to `DefWindowProcW`.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::rgb;
    use crate::geom::rect;
    use crate::gfx::fill;
    use std::time::Duration;
    use windows::Win32::UI::WindowsAndMessaging::{
        SendMessageW, WM_ENTERSIZEMOVE, WM_EXITSIZEMOVE, WM_SIZE, WM_TIMER,
    };

    struct TestHandler {
        platform: Rc<Platform>,
        jobs: Jobs<u32>,
        window: Option<NativeWindow>,
        events: Vec<PlatformEvent>,
        wakes: usize,
        paints: usize,
        quit: bool,
        _timer: TimerHandle,
    }

    impl PlatformHandler for TestHandler {
        fn on_event(&mut self, event: PlatformEvent) -> bool {
            if let PlatformEvent::Resized {
                physical_width,
                physical_height,
                scale,
                ..
            } = &event
                && let Some(w) = &mut self.window
            {
                w.sync_size(*physical_width, *physical_height, *scale);
            }
            self.events.push(event);
            false
        }

        fn query_ime_caret(&self, _window_id: WindowId) -> Option<(Rect, f32)> {
            None
        }

        fn on_wake(&mut self) {
            while let Some(v) = self.jobs.pop() {
                self.wakes += 1;
                if v == 99 {
                    self.quit = true;
                    self.platform.post_quit();
                }
            }
        }

        fn on_paint(&mut self, _window_id: WindowId) {
            self.paints += 1;
        }

        fn drive_frames(&mut self) {
            if let Some(w) = &mut self.window
                && let Some(renderer) = w.renderer_mut()
            {
                renderer.begin_frame(rgb(0x272727));
                renderer.paint_quad(fill(rect(0.0, 0.0, 100.0, 50.0), rgb(0x005fb8)));
                let lost = renderer.end_frame().expect("end_frame failed");
                assert!(!lost);
                self.paints += 1;
            }
        }

        fn should_quit(&self) -> bool {
            self.quit
        }
    }

    #[test]
    fn real_win32_loop_wakes_timers_cleans_hwnd_registry_and_paints_d2d() {
        let platform = Rc::new(Platform::new().expect("Platform::new failed"));
        let jobs = Jobs::new();
        jobs.set_wake_callback(platform.wake_callback());

        let opts = WindowOptions {
            bounds: Some(rect(40.0, 40.0, 320.0, 200.0)),
            show: false,
            focus: false,
            ..Default::default()
        };
        let win =
            NativeWindow::create(platform.devices(), &opts, WindowAppearance::Dark)
                .expect("NativeWindow::create failed");
        let hwnd = win.hwnd();
        assert!(window::has_hwnd_entry(hwnd));

        let handler = Rc::new(RefCell::new(TestHandler {
            platform: platform.clone(),
            jobs,
            window: Some(win),
            events: Vec::new(),
            wakes: 0,
            paints: 0,
            quit: false,
            _timer: TimerHandle::default(),
        }));

        Platform::run(handler.clone(), |h| {
            // Re-entrant WM_SIZE while handler is borrowed must queue instead of dropping.
            // SAFETY: `hwnd` is a valid window owned by the test thread.
            unsafe {
                let _ = SendMessageW(
                    hwnd.raw(),
                    WM_SIZE,
                    Some(WPARAM(0)),
                    Some(LPARAM(((240isize) << 16) | 400isize)),
                );
            }
            // Exercise modal size/move timer entry & exit cleanly.
            // SAFETY: `hwnd` is a valid window owned by the test thread.
            unsafe {
                let _ = SendMessageW(hwnd.raw(), WM_ENTERSIZEMOVE, Some(WPARAM(0)), Some(LPARAM(0)));
                let _ = SendMessageW(hwnd.raw(), WM_TIMER, Some(WPARAM(1)), Some(LPARAM(0)));
                let _ = SendMessageW(hwnd.raw(), WM_EXITSIZEMOVE, Some(WPARAM(0)), Some(LPARAM(0)));
            }
            h._timer = h.jobs.after(Duration::from_millis(25), 99);
        });

        let mut h = handler.borrow_mut();
        assert!(h.wakes >= 1, "timer must wake the Win32 message loop");
        assert!(h.paints >= 1, "D2D frame must paint cleanly");
        assert!(
            h.events.iter().any(|e| matches!(
                e,
                PlatformEvent::Resized {
                    physical_width: 400,
                    physical_height: 240,
                    ..
                }
            )),
            "re-entrant WM_SIZE must drain into handler"
        );
        h.window.take().unwrap().destroy();
        assert!(
            !window::has_hwnd_entry(hwnd),
            "HWND_REGISTRY entry must be removed on destroy"
        );
    }
}
