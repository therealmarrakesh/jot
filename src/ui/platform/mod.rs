//! Clean-room Win32 platform layer: display enumeration, platform run loop, window options,
//! native file dialogs, and Win32 windowing.

pub(crate) mod dialogs;
pub(crate) mod window;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

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

use super::app::{App, AppCell};
use super::geometry::{Bounds, Pixels, Size, point, px, size};
use super::graphics::d2d_renderer::Direct2DDevices;
use super::types::{SharedString, UiError, UiResult};

pub(crate) use window::set_window_min_size;

pub(crate) const WM_APP_WAKE: u32 = WM_APP + 1;
pub(crate) const WM_APP_VSYNC: u32 = WM_APP + 2;

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
        let _ = unsafe { GetDpiForMonitor(self.monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) };
        (dpi_x.max(USER_DEFAULT_SCREEN_DPI) as f32) / (USER_DEFAULT_SCREEN_DPI as f32)
    }

    pub(crate) fn visible_bounds(&self) -> Bounds<Pixels> {
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !unsafe { GetMonitorInfoW(self.monitor, &mut info) }.as_bool() {
            return Bounds::new(point(px(0.0), px(0.0)), size(px(1280.0), px(800.0)));
        }
        let scale = self.scale_factor();
        let rc = info.rcWork;
        Bounds::new(
            point(px(rc.left as f32 / scale), px(rc.top as f32 / scale)),
            size(
                px((rc.right - rc.left) as f32 / scale),
                px((rc.bottom - rc.top) as f32 / scale),
            ),
        )
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum WindowAppearance {
    #[default]
    Light,
    Dark,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum WindowBackgroundAppearance {
    #[default]
    Opaque,
    Transparent,
    MicaAltBackdrop,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum WindowKind {
    #[default]
    Normal,
    Overlay,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) enum WindowBounds {
    Windowed(Bounds<Pixels>),
    Maximized(Bounds<Pixels>),
}

impl WindowBounds {
    pub(crate) fn get_bounds(&self) -> Bounds<Pixels> {
        match *self {
            Self::Windowed(b) | Self::Maximized(b) => b,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct TitlebarOptions {
    pub(crate) title: Option<SharedString>,
    pub(crate) appears_transparent: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct WindowOptions {
    pub(crate) window_bounds: Option<WindowBounds>,
    pub(crate) titlebar: Option<TitlebarOptions>,
    pub(crate) focus: bool,
    pub(crate) show: bool,
    pub(crate) kind: WindowKind,
    pub(crate) is_resizable: bool,
    pub(crate) is_minimizable: bool,
    pub(crate) display_id: Option<DisplayId>,
    pub(crate) window_background: WindowBackgroundAppearance,
    pub(crate) window_min_size: Option<Size<Pixels>>,
    #[allow(dead_code)]
    pub(crate) inactive_frame_interval: Option<Duration>,
}

impl Default for WindowOptions {
    fn default() -> Self {
        Self {
            window_bounds: None,
            titlebar: None,
            focus: true,
            show: true,
            kind: WindowKind::Normal,
            is_resizable: true,
            is_minimizable: true,
            display_id: None,
            window_background: WindowBackgroundAppearance::Opaque,
            window_min_size: None,
            inactive_frame_interval: None,
        }
    }
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

#[derive(Clone, Debug, Default)]
pub(crate) struct PathPromptOptions {
    pub(crate) directories: bool,
    pub(crate) multiple: bool,
    pub(crate) prompt: Option<SharedString>,
}

#[derive(Clone, Debug)]
pub(crate) struct SaveChoice {
    pub(crate) label: SharedString,
    pub(crate) items: Vec<SharedString>,
    pub(crate) selected: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct SavePathOptions {
    pub(crate) title: Option<SharedString>,
    pub(crate) directory: PathBuf,
    pub(crate) file_name: Option<String>,
    pub(crate) file_types: Vec<(SharedString, SharedString)>,
    pub(crate) default_extension: Option<SharedString>,
    pub(crate) choice: Option<SaveChoice>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ExternalPaths(pub(crate) Vec<PathBuf>);

impl ExternalPaths {
    pub(crate) fn paths(&self) -> &[PathBuf] {
        &self.0
    }
}

thread_local! {
    pub(crate) static ACTIVE_APP: RefCell<Option<Rc<AppCell>>> = const { RefCell::new(None) };
}

/// Pacing thread that blocks in `Condvar` at 0% CPU until armed, then waits one `DwmFlush`
/// and posts `WM_APP_VSYNC` back to the hidden UI wake window.
struct VsyncPacer {
    armed: std::sync::Mutex<bool>,
    condvar: std::sync::Condvar,
}

impl VsyncPacer {
    fn spawn(wake_hwnd_raw: isize) -> Arc<Self> {
        let pacer = Arc::new(Self {
            armed: std::sync::Mutex::new(false),
            condvar: std::sync::Condvar::new(),
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

pub(crate) struct WindowsPlatform {
    pub(crate) devices: Direct2DDevices,
    wake_hwnd: HWND,
    vsync: Arc<VsyncPacer>,
    ole_initialized: bool,
}

const WAKE_CLASS_NAME: PCWSTR = w!("JotD2DWakeWindow");

impl WindowsPlatform {
    pub(crate) fn new() -> UiResult<Self> {
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
            .map_err(|err| UiError::new(format!("initializing Direct3D11/Direct2D devices: {err}")))?;
        let vsync = VsyncPacer::spawn(wake_hwnd.0 as isize);

        Ok(Self {
            devices,
            wake_hwnd,
            vsync,
            ole_initialized,
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

    pub(crate) fn run(&self, app_cell: Rc<AppCell>, on_ready: impl FnOnce(&mut App) + 'static) {
        let wake_raw = self.wake_hwnd.0 as isize;
        app_cell
            .inner
            .borrow()
            .jobs
            .set_wake_callback(Arc::new(move || unsafe {
                let _ = PostMessageW(
                    Some(HWND(wake_raw as *mut _)),
                    WM_APP_WAKE,
                    WPARAM(0),
                    LPARAM(0),
                );
            }));

        ACTIVE_APP.with(|slot| *slot.borrow_mut() = Some(app_cell.clone()));

        {
            let mut app = app_cell.inner.borrow_mut();
            on_ready(&mut app);
            app.flush_effects();
        }
        drive_frames(&app_cell);

        let mut msg = MSG::default();
        while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            if app_cell.inner.borrow().quit_requested {
                break;
            }
        }

        ACTIVE_APP.with(|slot| *slot.borrow_mut() = None);
    }
}

impl Drop for WindowsPlatform {
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

pub(crate) fn drive_frames(app_cell: &Rc<AppCell>) {
    window::drain_pending_win32_events(app_cell);
    let ids: Vec<_> = {
        let Ok(app) = app_cell.inner.try_borrow() else {
            return;
        };
        app.window_order.clone()
    };
    let mut needs_vsync = false;
    for id in ids {
        let want_draw = {
            let Ok(app) = app_cell.inner.try_borrow() else {
                return;
            };
            app.windows
                .get(&id)
                .and_then(Option::as_ref)
                .is_some_and(|w| w.needs_redraw())
        };
        if want_draw {
            let Ok(mut app) = app_cell.inner.try_borrow_mut() else {
                return;
            };
            let _ = app.update_window_impl(id, |root, window, cx| {
                window.draw_frame(&root, cx);
            });
            app.flush_effects();
        }
        let still_animating = {
            let Ok(app) = app_cell.inner.try_borrow() else {
                return;
            };
            app.windows
                .get(&id)
                .and_then(Option::as_ref)
                .is_some_and(|w| w.needs_redraw())
        };
        needs_vsync |= still_animating;
    }
    if needs_vsync && let Ok(app) = app_cell.inner.try_borrow() {
        app.platform.request_frame_tick();
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
            if let Some(app_cell) = ACTIVE_APP.with(|slot| slot.borrow().clone()) {
                match app_cell.inner.try_borrow_mut() {
                    Ok(mut app) => {
                        app.drain_jobs();
                        app.flush_effects();
                        app.entities.prune_dropped();
                    }
                    Err(_) => {
                        let _ = unsafe {
                            PostMessageW(Some(hwnd), WM_APP_WAKE, WPARAM(0), LPARAM(0))
                        };
                        return LRESULT(0);
                    }
                }
                drive_frames(&app_cell);
            }
            LRESULT(0)
        }
        WM_APP_VSYNC => {
            if let Some(app_cell) = ACTIVE_APP.with(|slot| slot.borrow().clone()) {
                drive_frames(&app_cell);
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}
