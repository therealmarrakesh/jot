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
    AnsiCodePage, ReplaceFileError, WindowHwnd, WindowPlacement, caret_blink_interval,
    client_area_animations_enabled, cursor_screen_pos, documents_dir, format_local_time_date,
    menu_show_delay_ms, monitor_work_area_and_scale, mouse_hover_time_ms, primary_mouse_button_down,
    release_mouse_capture, replace_file, system_accent_color, top_visible_window_at,
};
pub(crate) use pipe::{allow_any_set_foreground_window, start_inbound_pipe_listener};
pub(crate) use window::{
    CursorStyle, NativeWindow, PlatformEvent, PlatformHandler, WindowAppearance, WindowBackdrop,
    WindowControlArea, WindowId, WindowKind, WindowOptions,
};

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Condvar, Mutex};

use windows::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM},
    Graphics::{
        Dwm::DwmFlush,
        Gdi::{
            GetMonitorInfoW, HMONITOR, MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MonitorFromPoint,
        },
    },
    System::{
        Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize},
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
            HWND_MESSAGE, MSG, PostMessageW, PostQuitMessage, RegisterClassW, TranslateMessage,
            USER_DEFAULT_SCREEN_DPI, WM_APP, WINDOW_EX_STYLE, WINDOW_STYLE, WNDCLASSW,
        },
    },
};
use windows::core::{PCWSTR, w};

use crate::geom::{Rect, Size, point, size};
use crate::gfx::Direct2DDevices;
use crate::types::{Error, Result};

const WM_APP_WAKE: u32 = WM_APP + 1;
const WM_APP_VSYNC: u32 = WM_APP + 2;
const WAKE_CLASS_NAME: PCWSTR = w!("JotD2DWakeWindow");

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
}

struct ActiveHandlerGuard;

impl Drop for ActiveHandlerGuard {
    fn drop(&mut self) {
        ACTIVE_HANDLER.with(|slot| *slot.borrow_mut() = None);
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
                    let mut armed = bg.armed.lock().unwrap();
                    while !*armed {
                        armed = bg.condvar.wait(armed).unwrap();
                    }
                    *armed = false;
                }
                let _ = unsafe { DwmFlush() };
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
        let mut armed = self.armed.lock().unwrap();
        if !*armed {
            *armed = true;
            self.condvar.notify_one();
        }
    }
}

pub(crate) struct Platform {
    pub(crate) devices: Direct2DDevices,
    wake_hwnd: HWND,
    vsync: Arc<VsyncPacer>,
    ole_initialized: bool,
}

impl Platform {
    pub(crate) fn new() -> Result<Self> {
        unsafe {
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        }
        let ole_initialized = unsafe { OleInitialize(None) }.is_ok();
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

        let devices = Direct2DDevices::new()
            .map_err(|err| Error::new(format!("initializing Direct3D11/Direct2D devices: {err}")))?;
        let vsync = VsyncPacer::spawn(wake_hwnd.0 as isize);

        Ok(Self {
            devices,
            wake_hwnd,
            vsync,
            ole_initialized,
        })
    }

    pub(crate) fn wake_callback(&self) -> Arc<dyn Fn() + Send + Sync> {
        let wake_raw = self.wake_hwnd.0 as isize;
        Arc::new(move || unsafe {
            let _ = PostMessageW(
                Some(HWND(wake_raw as *mut _)),
                WM_APP_WAKE,
                WPARAM(0),
                LPARAM(0),
            );
        })
    }

    pub(crate) fn request_frame_tick(&self) {
        self.vsync.arm();
    }

    pub(crate) fn system_appearance(&self) -> WindowAppearance {
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

    pub(crate) fn post_quit(&self) {
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
            if unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 <= 0 {
                break;
            }
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
        unsafe {
            let _ = DestroyWindow(self.wake_hwnd);
            if self.ole_initialized {
                OleUninitialize();
            }
            CoUninitialize();
        }
    }
}

unsafe extern "system" fn wake_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_APP_WAKE => {
            if let Some(handler_rc) = ACTIVE_HANDLER.with(|slot| slot.borrow().clone()) {
                match handler_rc.try_borrow_mut() {
                    Ok(mut handler) => handler.on_wake(),
                    Err(_) => {
                        let _ = unsafe {
                            PostMessageW(Some(hwnd), WM_APP_WAKE, WPARAM(0), LPARAM(0))
                        };
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
            if let Some(w) = &mut self.window {
                w.renderer.begin_frame(rgb(0x272727));
                w.renderer
                    .paint_quad(fill(rect(0.0, 0.0, 100.0, 50.0), rgb(0x005fb8)));
                let lost = w.renderer.end_frame().expect("end_frame failed");
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
            NativeWindow::create(platform.devices.clone(), &opts, WindowAppearance::Dark)
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
            unsafe {
                let _ = SendMessageW(
                    hwnd.raw(),
                    WM_SIZE,
                    Some(WPARAM(0)),
                    Some(LPARAM(((240isize) << 16) | 400isize)),
                );
            }
            // Exercise modal size/move timer entry & exit cleanly.
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
