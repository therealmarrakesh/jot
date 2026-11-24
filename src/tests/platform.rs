use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use crate::command::AppCommand;
use crate::geom::ScreenRect;
use crate::gfx::WindowSurface;
use crate::ids::{HitId, MenuHit, MenuKind, TabHit};
use crate::platform::{Jobs, NativeWindow, Platform, PlatformEvent, PlatformHandler};
use crate::shell::{AppEvent, WindowTimer};

use super::TestHarness;

#[test]
fn overlay_window_resizes_from_1x1_and_renders() {
    let h = TestHarness::new();
    let gpu = h.app.borrow().shell.gpu.clone();
    let overlay = NativeWindow::create_overlay().expect("overlay create");
    let mut ws = WindowSurface::new(gpu, overlay).expect("window surface");
    ws.window.hwnd().place_topmost_noactivate(ScreenRect {
        left: 10,
        top: 10,
        right: 170,
        bottom: 50,
    });
    let (w, h_px, scale) = ws
        .window
        .sync_live_metrics()
        .expect("overlay must sync resized client rect");
    ws.surface.resize(w, h_px, scale);
    assert!(
        ws.window.logical_size().width > 10.0 && ws.window.logical_size().height > 10.0,
        "overlay must sync resized client rect before drawing"
    );
    let needs_retry = ws.surface.draw(crate::color::transparent_black(), |_| {});
    assert!(!needs_retry);
}

#[test]
fn hwnd_registry_cleanup_reentrant_queue_and_modal_size_move_timer() {
    use windows::Win32::{
        Foundation::{LPARAM, WPARAM},
        UI::WindowsAndMessaging::{
            SendMessageW, WM_ACTIVATE, WM_ENTERSIZEMOVE, WM_EXITSIZEMOVE, WM_TIMER,
        },
    };

    let mut h = TestHarness::new();
    let hwnd = h.read(|w, _| w.hwnd());
    assert!(
        crate::platform::window::has_hwnd_entry(hwnd),
        "open window must be registered in HWND_REGISTRY"
    );

    let dyn_handler: Rc<RefCell<dyn PlatformHandler>> = h.app.clone();
    Platform::set_active_handler_for_test(Some(dyn_handler));

    {
        let _borrow = h.app.borrow_mut();
        unsafe {
            let _ = SendMessageW(
                hwnd.raw_hwnd(),
                WM_ACTIVATE,
                Some(WPARAM(1)),
                Some(LPARAM(0)),
            );
        }
    }
    Platform::drain_pending_events_for_test();
    h.app.borrow_mut().drive_frames();
    h.read(|w, app| {
        assert!(
            w.is_active(),
            "queued re-entrant WM_ACTIVATE must apply once drained"
        );
        assert_eq!(app.windows.active_id(), Some(h.window_id));
        assert!(!w.is_dirty());
    });

    h.click_hit(HitId::Menu(MenuHit::Trigger(MenuKind::View)));
    h.read(|w, _| {
        assert!(w.menus().hit_for_command(AppCommand::ZoomIn.into()).is_none());
    });
    h.app.borrow().shell.jobs.post(AppEvent::Window {
        window: h.window_id,
        timer: WindowTimer::Submenu(Some(0)),
    });

    unsafe {
        let _ = SendMessageW(
            hwnd.raw_hwnd(),
            WM_ENTERSIZEMOVE,
            Some(WPARAM(0)),
            Some(LPARAM(0)),
        );
    }
    h.read(|w, _| assert!(w.in_size_move()));
    unsafe {
        let _ = SendMessageW(hwnd.raw_hwnd(), WM_TIMER, Some(WPARAM(1)), Some(LPARAM(0)));
    }
    h.read(|w, _| {
        assert!(
            w.menus().hit_for_command(AppCommand::ZoomIn.into()).is_some(),
            "WM_TIMER during modal size/move must drain ready foreground jobs"
        );
    });
    unsafe {
        let _ = SendMessageW(
            hwnd.raw_hwnd(),
            WM_EXITSIZEMOVE,
            Some(WPARAM(0)),
            Some(LPARAM(0)),
        );
    }
    h.read(|w, _| assert!(!w.in_size_move()));

    h.close_all();
    Platform::set_active_handler_for_test(None);
    assert!(
        !crate::platform::window::has_hwnd_entry(hwnd),
        "closing window must remove entry from HWND_REGISTRY"
    );
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

#[test]
fn device_loss_recreates_devices_and_renders_new_windows_cleanly() {
    let mut h = TestHarness::new();
    let gen0 = h.read(|_, app| app.shell.gpu.generation());
    {
        let mut app = h.app.borrow_mut();
        app.windows[0].mark_device_lost_for_test();
        app.windows[0].mark_dirty();
    }
    h.draw();
    let gen1 = h.read(|_, app| app.shell.gpu.generation());
    assert_eq!(
        gen1,
        gen0 + 1,
        "device loss must replace shared Gpu devices"
    );
    h.press_key("ctrl-shift-n");
    {
        let app = h.app.borrow();
        assert_eq!(app.windows.len(), 2);
        assert!(app.windows[1].surface_is_ready());
    }
}

#[test]
fn win32_client_clicks_and_capture_loss_via_wnd_proc() {
    use windows::Win32::{
        Foundation::{LPARAM, POINT, WPARAM},
        Graphics::Gdi::ClientToScreen,
        UI::WindowsAndMessaging::{
            HTCLIENT, SendMessageW, WM_CAPTURECHANGED, WM_LBUTTONDOWN, WM_LBUTTONUP,
            WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEMOVE, WM_NCHITTEST,
        },
    };

    let mut h = TestHarness::new();
    let dyn_handler: Rc<RefCell<dyn PlatformHandler>> = h.app.clone();
    Platform::set_active_handler_for_test(Some(dyn_handler));

    let (hwnd, scale) = h.read(|w, _| (w.hwnd(), w.scale_factor()));
    let pack_client = |pt: crate::geom::Point| -> LPARAM {
        let x = (pt.x * scale).round() as i32;
        let y = (pt.y * scale).round() as i32;
        LPARAM((((y as u16 as u32) << 16) | (x as u16 as u32)) as isize)
    };
    let pack_screen = |pt: crate::geom::Point| -> LPARAM {
        let mut p = POINT {
            x: (pt.x * scale).round() as i32,
            y: (pt.y * scale).round() as i32,
        };
        unsafe {
            let _ = ClientToScreen(hwnd.raw_hwnd(), &mut p);
        }
        LPARAM((((p.y as u16 as u32) << 16) | (p.x as u16 as u32)) as isize)
    };

    let new_tab_pt = h
        .hit_rect(HitId::Tab(TabHit::New))
        .expect("NewTab rect")
        .center();
    let nchittest = unsafe {
        SendMessageW(
            hwnd.raw_hwnd(),
            WM_NCHITTEST,
            Some(WPARAM(0)),
            Some(pack_screen(new_tab_pt)),
        )
    };
    assert_eq!(
        nchittest.0, HTCLIENT as isize,
        "NewTab button must hit-test as HTCLIENT, not HTCAPTION"
    );

    // Clicking NewTab through wnd_proc with synchronous ReleaseCapture -> WM_CAPTURECHANGED
    // must preserve UiState::pressed and open a new tab.
    let lp = pack_client(new_tab_pt);
    unsafe {
        let _ = SendMessageW(hwnd.raw_hwnd(), WM_MOUSEMOVE, Some(WPARAM(0)), Some(lp));
        let _ = SendMessageW(hwnd.raw_hwnd(), WM_LBUTTONDOWN, Some(WPARAM(1)), Some(lp));
    }
    crate::platform::window::simulate_release_capture_callback(hwnd);
    unsafe {
        let _ = SendMessageW(hwnd.raw_hwnd(), WM_LBUTTONUP, Some(WPARAM(0)), Some(lp));
    }
    h.read(|w, _| {
        assert_eq!(
            w.tabs().len(),
            2,
            "Win32 click on NewTab must open a new tab even when ReleaseCapture triggers WM_CAPTURECHANGED"
        );
    });

    // Clicking ThemeToggle through wnd_proc must also activate on MouseUp.
    let initial_dark = h.read(|_, app| app.shell.palette.dark);
    let theme_pt = h
        .hit_rect(HitId::ThemeToggle)
        .expect("ThemeToggle rect")
        .center();
    let theme_lp = pack_client(theme_pt);
    unsafe {
        let _ = SendMessageW(hwnd.raw_hwnd(), WM_MOUSEMOVE, Some(WPARAM(0)), Some(theme_lp));
        let _ = SendMessageW(hwnd.raw_hwnd(), WM_LBUTTONDOWN, Some(WPARAM(1)), Some(theme_lp));
    }
    crate::platform::window::simulate_release_capture_callback(hwnd);
    unsafe {
        let _ = SendMessageW(hwnd.raw_hwnd(), WM_LBUTTONUP, Some(WPARAM(0)), Some(theme_lp));
    }
    h.read(|_, app| assert_eq!(app.shell.palette.dark, !initial_dark));

    // Middle-clicking a tab through wnd_proc must preserve middle_pressed_tab across ReleaseCapture.
    let second_id = h.read(|w, _| w.tabs()[1].data.id);
    let tab_pt = h
        .hit_rect(HitId::Tab(TabHit::Item(second_id)))
        .expect("second tab rect")
        .center();
    let tab_lp = pack_client(tab_pt);
    unsafe {
        let _ = SendMessageW(hwnd.raw_hwnd(), WM_MOUSEMOVE, Some(WPARAM(0)), Some(tab_lp));
        let _ = SendMessageW(hwnd.raw_hwnd(), WM_MBUTTONDOWN, Some(WPARAM(0x10)), Some(tab_lp));
    }
    crate::platform::window::simulate_release_capture_callback(hwnd);
    unsafe {
        let _ = SendMessageW(hwnd.raw_hwnd(), WM_MBUTTONUP, Some(WPARAM(0)), Some(tab_lp));
    }
    h.read(|w, _| assert_eq!(w.tabs().len(), 1, "Win32 middle-click must close tab"));

    // Genuine external WM_CAPTURECHANGED (not from our own ReleaseCapture) must cancel the click.
    let new_tab_pt = h
        .hit_rect(HitId::Tab(TabHit::New))
        .expect("NewTab rect")
        .center();
    let lp = pack_client(new_tab_pt);
    unsafe {
        let _ = SendMessageW(hwnd.raw_hwnd(), WM_MOUSEMOVE, Some(WPARAM(0)), Some(lp));
        let _ = SendMessageW(hwnd.raw_hwnd(), WM_LBUTTONDOWN, Some(WPARAM(1)), Some(lp));
        let _ = SendMessageW(hwnd.raw_hwnd(), WM_CAPTURECHANGED, Some(WPARAM(0)), Some(LPARAM(0)));
        let _ = SendMessageW(hwnd.raw_hwnd(), WM_LBUTTONUP, Some(WPARAM(0)), Some(lp));
    }
    h.read(|w, _| {
        assert_eq!(
            w.tabs().len(),
            1,
            "external WM_CAPTURECHANGED must cancel an in-progress click"
        );
    });

    h.close_all();
    Platform::set_active_handler_for_test(None);
}
