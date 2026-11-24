use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use windows::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, WPARAM},
    Graphics::Dwm::DwmFlush,
    System::{
        Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize},
        DataExchange::COPYDATASTRUCT,
        LibraryLoader::GetModuleHandleW,
        Shutdown::ShutdownBlockReasonDestroy,
    },
    UI::{
        HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext},
        WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
            HWND_MESSAGE, KillTimer, MSG, PostMessageW, PostQuitMessage, RegisterClassW, SetTimer,
            SetWindowTextW, TranslateMessage, USER_TIMER_MAXIMUM, USER_TIMER_MINIMUM, WM_APP,
            WM_COPYDATA, WM_TIMER, WINDOW_EX_STYLE, WINDOW_STYLE, WNDCLASSW,
        },
    },
};
use windows::core::{HSTRING, PCWSTR, w};

use crate::types::Result;

use super::diag;
use super::event::{PlatformEvent, PlatformHandler};
use super::instance::{self, InstanceLock};
use super::settings::SystemSettings;
use super::window::{self, WindowShared};

const WM_APP_WAKE: u32 = WM_APP + 1;
const WM_APP_VSYNC: u32 = WM_APP + 2;
const JOBS_TIMER_ID: usize = 2;
pub(super) const WAKE_CLASS_NAME: PCWSTR = w!("JotWakeWindow");

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

pub(super) fn register_window(hwnd: HWND, shared: Rc<WindowShared>) {
    PUMP.with(|p| {
        p.borrow_mut().windows.insert(hwnd.0 as isize, shared);
    });
}

pub(super) fn unregister_window(hwnd: HWND) -> Option<Rc<WindowShared>> {
    PUMP.with(|p| p.borrow_mut().windows.remove(&(hwnd.0 as isize)))
}

pub(super) fn lookup_window(hwnd: HWND) -> Option<Rc<WindowShared>> {
    PUMP.with(|p| p.borrow().windows.get(&(hwnd.0 as isize)).cloned())
}

pub(super) fn cached_settings() -> SystemSettings {
    if let Some(s) = PUMP.with(|p| p.borrow().settings) {
        return s;
    }
    let fresh = SystemSettings::query();
    set_cached_settings(fresh);
    fresh
}

pub(super) fn set_cached_settings(settings: SystemSettings) {
    PUMP.with(|p| p.borrow_mut().settings = Some(settings));
}

pub(super) fn mark_wake_pending() {
    PUMP.with(|p| p.borrow_mut().wake_pending = true);
}

pub(super) fn shutdown_block() -> Option<&'static str> {
    PUMP.with(|p| p.borrow().shutdown_block)
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

pub(super) fn service() {
    let Some(handler_rc) = active_handler() else {
        return;
    };
    drain_pending_only();
    if let Ok(mut handler) = handler_rc.try_borrow_mut() {
        handler.drive_frames();
    }
    drain_pending_only();
}

pub(super) fn dispatch(event: PlatformEvent) -> bool {
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

    pub(super) fn disable_handoff(&self) {
        PUMP.with(|p| p.borrow_mut().handoff_accepting = false);
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
            service();
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == JOBS_TIMER_ID => {
            let _ = unsafe { KillTimer(Some(hwnd), JOBS_TIMER_ID) };
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
            let Some(paths) = instance::decode_copydata(cds) else {
                return LRESULT(0);
            };
            dispatch(PlatformEvent::Launch { paths });
            LRESULT(1)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::WindowsAndMessaging::{
        SendMessageW, WM_ACTIVATE, WM_ENTERSIZEMOVE, WM_EXITSIZEMOVE, WM_TIMER,
    };

    use crate::platform::event::{MainWindowConfig, WindowAppearance};
    use crate::platform::jobs::Jobs;
    use crate::platform::window::NativeWindow;

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
            platform: Rc<Platform>,
            jobs: Jobs<()>,
            woke: bool,
        }

        impl PlatformHandler for LoopProbe {
            fn on_event(&mut self, _event: PlatformEvent) -> bool {
                false
            }

            fn on_wake(&mut self) {
                while self.jobs.pop().is_some() {
                    self.woke = true;
                    self.platform.post_quit();
                }
            }

            fn drive_frames(&mut self) {}
        }

        let platform = Rc::new(Platform::new(None).expect("Platform::new failed"));
        let jobs = Jobs::new();
        jobs.attach_platform(&platform);
        let probe = Rc::new(RefCell::new(LoopProbe {
            platform,
            jobs,
            woke: false,
        }));
        let mut _timer = None;
        Platform::run(probe.clone(), |p| {
            _timer = Some(p.jobs.after(Duration::from_millis(25), ()));
        });
        assert!(probe.borrow().woke);
    }
}
