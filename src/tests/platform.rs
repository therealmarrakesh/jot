use std::cell::RefCell;
use std::rc::Rc;

use crate::geom::ScreenRect;
use crate::gfx::WindowSurface;
use crate::ids::{HitId, TabHit};
use crate::platform::{NativeWindow, Platform, PlatformHandler};

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
    let needs_retry = ws.surface.draw(crate::geom::transparent_black(), |_| {});
    assert!(!needs_retry);
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
    Platform::with_test_handler(dyn_handler, || {
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

        let lp = pack_client(new_tab_pt);
        unsafe {
            let _ = SendMessageW(hwnd.raw_hwnd(), WM_MOUSEMOVE, Some(WPARAM(0)), Some(lp));
            let _ = SendMessageW(hwnd.raw_hwnd(), WM_LBUTTONDOWN, Some(WPARAM(1)), Some(lp));
            let _ = SendMessageW(hwnd.raw_hwnd(), WM_LBUTTONUP, Some(WPARAM(0)), Some(lp));
        }
        h.read(|w, _| {
            assert_eq!(w.tabs().len(), 2, "Win32 click on NewTab must open a new tab");
        });

        let initial_dark = h.read(|_, app| app.shell.palette.dark);
        let theme_pt = h
            .hit_rect(HitId::ThemeToggle)
            .expect("ThemeToggle rect")
            .center();
        let theme_lp = pack_client(theme_pt);
        unsafe {
            let _ = SendMessageW(hwnd.raw_hwnd(), WM_MOUSEMOVE, Some(WPARAM(0)), Some(theme_lp));
            let _ = SendMessageW(hwnd.raw_hwnd(), WM_LBUTTONDOWN, Some(WPARAM(1)), Some(theme_lp));
            let _ = SendMessageW(hwnd.raw_hwnd(), WM_LBUTTONUP, Some(WPARAM(0)), Some(theme_lp));
        }
        h.read(|_, app| assert_eq!(app.shell.palette.dark, !initial_dark));

        let second_id = h.read(|w, _| w.tabs()[1].data.id);
        let tab_pt = h
            .hit_rect(HitId::Tab(TabHit::Item(second_id)))
            .expect("second tab rect")
            .center();
        let tab_lp = pack_client(tab_pt);
        unsafe {
            let _ = SendMessageW(hwnd.raw_hwnd(), WM_MOUSEMOVE, Some(WPARAM(0)), Some(tab_lp));
            let _ = SendMessageW(hwnd.raw_hwnd(), WM_MBUTTONDOWN, Some(WPARAM(0x10)), Some(tab_lp));
            let _ = SendMessageW(hwnd.raw_hwnd(), WM_MBUTTONUP, Some(WPARAM(0)), Some(tab_lp));
        }
        h.read(|w, _| assert_eq!(w.tabs().len(), 1, "Win32 middle-click must close tab"));

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
    });
}
