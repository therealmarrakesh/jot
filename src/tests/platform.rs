use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use crate::app::{App, AppEvent};
use crate::document;
use crate::find;
use crate::geom::ScreenRect;
use crate::gfx::{Gpu, Surface, font};
use crate::platform::{NativeWindow, Platform, PlatformHandler};

use super::TestHarness;

#[test]
fn overlay_window_resizes_from_1x1_and_renders() {
    let h = TestHarness::new();
    let gpu = h.app.borrow().shell.gpu.clone();
    let mut overlay = NativeWindow::create_overlay().expect("overlay create");
    let (pw, ph) = overlay.physical_size();
    let mut surface =
        Surface::new(gpu, overlay.hwnd().raw(), pw, ph, overlay.scale_factor()).expect("surface");
    overlay.hwnd().place_topmost_noactivate(ScreenRect {
        left: 10,
        top: 10,
        right: 170,
        bottom: 50,
    });
    let (w, h_px, scale) = overlay
        .sync_live_metrics()
        .expect("overlay must sync resized client rect");
    surface.resize(w, h_px, scale);
    assert!(
        overlay.logical_size().width > 10.0 && overlay.logical_size().height > 10.0,
        "overlay must sync resized client rect before drawing"
    );
    let needs_retry = surface.draw(crate::color::transparent_black(), |_| {});
    assert!(!needs_retry);
    drop(surface);
    overlay.destroy();
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
    let hwnd = h.read(|w, _| w.native.hwnd());
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
                hwnd.raw(),
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
            w.native.is_active(),
            "queued re-entrant WM_ACTIVATE must apply once drained"
        );
        assert_eq!(app.active_window, Some(h.window_id));
    });

    let ran = Arc::new(AtomicBool::new(false));
    h.app
        .borrow()
        .shell
        .jobs
        .post(AppEvent::TestFlag(ran.clone()));

    unsafe {
        let _ = SendMessageW(
            hwnd.raw(),
            WM_ENTERSIZEMOVE,
            Some(WPARAM(0)),
            Some(LPARAM(0)),
        );
    }
    h.read(|w, _| assert!(w.native.in_size_move()));
    unsafe {
        let _ = SendMessageW(hwnd.raw(), WM_TIMER, Some(WPARAM(1)), Some(LPARAM(0)));
    }
    assert!(
        ran.load(Ordering::SeqCst),
        "WM_TIMER during modal size/move must drain ready foreground jobs"
    );
    unsafe {
        let _ = SendMessageW(
            hwnd.raw(),
            WM_EXITSIZEMOVE,
            Some(WPARAM(0)),
            Some(LPARAM(0)),
        );
    }
    h.read(|w, _| assert!(!w.native.in_size_move()));

    h.close_all();
    Platform::set_active_handler_for_test(None);
    assert!(
        !crate::platform::window::has_hwnd_entry(hwnd),
        "closing window must remove entry from HWND_REGISTRY"
    );
}

#[test]
fn real_win32_message_loop_wakes_timers_and_quits_cleanly() {
    let platform = Rc::new(Platform::new().expect("Platform::new failed"));
    let gpu = Gpu::new().expect("Gpu::new failed");
    let app = Rc::new(RefCell::new(App::new(platform, gpu)));
    let mut _timer = None;
    Platform::run(app, |app| {
        app.headless = true;
        let (id, _) = app.open_window(None, None).expect("open_window failed");
        _timer = Some(
            app.shell
                .jobs
                .after(Duration::from_millis(25), AppEvent::TestCloseWindow(id)),
        );
    });
}

#[test]
fn device_loss_recreates_devices_and_renders_new_windows_cleanly() {
    let mut h = TestHarness::new();
    let gen0 = h.read(|_, app| app.shell.gpu.generation());
    {
        let mut app = h.app.borrow_mut();
        app.windows[0].surface.mark_device_lost_for_test();
        app.windows[0].dirty = true;
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
        assert!(app.windows[1].surface.is_ready());
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
    use crate::ids::HitId;

    let mut h = TestHarness::new();
    let dyn_handler: Rc<RefCell<dyn PlatformHandler>> = h.app.clone();
    Platform::set_active_handler_for_test(Some(dyn_handler));

    let (hwnd, scale) = h.read(|w, _| (w.native.hwnd(), w.native.scale_factor()));
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
            let _ = ClientToScreen(hwnd.raw(), &mut p);
        }
        LPARAM((((p.y as u16 as u32) << 16) | (p.x as u16 as u32)) as isize)
    };

    let new_tab_pt = h.hit_rect(HitId::NewTab).expect("NewTab rect").center();
    let nchittest = unsafe {
        SendMessageW(
            hwnd.raw(),
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
        let _ = SendMessageW(hwnd.raw(), WM_MOUSEMOVE, Some(WPARAM(0)), Some(lp));
        let _ = SendMessageW(hwnd.raw(), WM_LBUTTONDOWN, Some(WPARAM(1)), Some(lp));
    }
    crate::platform::window::simulate_release_capture_callback(hwnd);
    unsafe {
        let _ = SendMessageW(hwnd.raw(), WM_LBUTTONUP, Some(WPARAM(0)), Some(lp));
    }
    h.read(|w, _| {
        assert_eq!(
            w.tabs.len(),
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
        let _ = SendMessageW(hwnd.raw(), WM_MOUSEMOVE, Some(WPARAM(0)), Some(theme_lp));
        let _ = SendMessageW(hwnd.raw(), WM_LBUTTONDOWN, Some(WPARAM(1)), Some(theme_lp));
    }
    crate::platform::window::simulate_release_capture_callback(hwnd);
    unsafe {
        let _ = SendMessageW(hwnd.raw(), WM_LBUTTONUP, Some(WPARAM(0)), Some(theme_lp));
    }
    h.read(|_, app| assert_eq!(app.shell.palette.dark, !initial_dark));

    // Middle-clicking a tab through wnd_proc must preserve middle_pressed_tab across ReleaseCapture.
    let second_id = h.read(|w, _| w.tabs[1].data.id);
    let tab_pt = h
        .hit_rect(HitId::Tab(second_id))
        .expect("second tab rect")
        .center();
    let tab_lp = pack_client(tab_pt);
    unsafe {
        let _ = SendMessageW(hwnd.raw(), WM_MOUSEMOVE, Some(WPARAM(0)), Some(tab_lp));
        let _ = SendMessageW(hwnd.raw(), WM_MBUTTONDOWN, Some(WPARAM(0x10)), Some(tab_lp));
    }
    crate::platform::window::simulate_release_capture_callback(hwnd);
    unsafe {
        let _ = SendMessageW(hwnd.raw(), WM_MBUTTONUP, Some(WPARAM(0)), Some(tab_lp));
    }
    h.read(|w, _| assert_eq!(w.tabs.len(), 1, "Win32 middle-click must close tab"));

    // Genuine external WM_CAPTURECHANGED (not from our own ReleaseCapture) must cancel the click.
    let new_tab_pt = h.hit_rect(HitId::NewTab).expect("NewTab rect").center();
    let lp = pack_client(new_tab_pt);
    unsafe {
        let _ = SendMessageW(hwnd.raw(), WM_MOUSEMOVE, Some(WPARAM(0)), Some(lp));
        let _ = SendMessageW(hwnd.raw(), WM_LBUTTONDOWN, Some(WPARAM(1)), Some(lp));
        let _ = SendMessageW(hwnd.raw(), WM_CAPTURECHANGED, Some(WPARAM(0)), Some(LPARAM(0)));
        let _ = SendMessageW(hwnd.raw(), WM_LBUTTONUP, Some(WPARAM(0)), Some(lp));
    }
    h.read(|w, _| {
        assert_eq!(
            w.tabs.len(),
            1,
            "external WM_CAPTURECHANGED must cancel an in-progress click"
        );
    });

    h.close_all();
    Platform::set_active_handler_for_test(None);
}

mod large_file_smoke {
    use super::*;
    use crate::editor::wrap::{DisplayPos, WrapIndex};
    use crate::gfx::TextSystem;
    use std::io::Write as _;
    use std::time::Instant;

    fn run_profile(target_mb: usize) {
        let dir = std::env::temp_dir()
            .join(format!("jot-bench-{target_mb}mb-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("big.txt");

        let target_bytes = target_mb * 1024 * 1024;
        {
            let mut w = std::io::BufWriter::with_capacity(
                256 * 1024,
                std::fs::File::create(&file).unwrap(),
            );
            let mut written = 0usize;
            let mut line_ix = 0usize;
            while written < target_bytes {
                let line = if line_ix.is_multiple_of(50) {
                    format!("Line {line_ix:07}: long wrapped paragraph with repeated words alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi omicron pi rho sigma tau upsilon phi chi psi omega end.\r\n")
                } else {
                    format!("Line {line_ix:07}: standard 80-col plain text log entry for Jot benchmark.\r\n")
                };
                w.write_all(line.as_bytes()).unwrap();
                written += line.len();
                line_ix += 1;
            }
            w.write_all(b"FINAL_SENTINEL_NEEDLE_42\r\n").unwrap();
            w.flush().unwrap();
        }

        let disk_len = std::fs::metadata(&file).unwrap().len();
        println!("\n=== Large-File Smoke Profile ({target_mb} MB, {disk_len} bytes on disk) ===");

        let t0 = Instant::now();
        let loaded = document::load(&file).unwrap();
        println!(
            "load:                 {:8.2} ms",
            t0.elapsed().as_secs_f64() * 1000.0
        );

        let ts = TextSystem::new();
        let mut wrap_none = WrapIndex::new(font("Consolas"), 14.0, None);
        let t0 = Instant::now();
        wrap_none.sync_layout(&loaded.text, font("Consolas"), 14.0, None, &ts);
        println!(
            "wrap (no-wrap):       {:8.2} ms  ({} rows)",
            t0.elapsed().as_secs_f64() * 1000.0,
            wrap_none.visual_row_count()
        );

        let mut wrap_900 = WrapIndex::new(font("Consolas"), 14.0, Some(900.0));
        let t0 = Instant::now();
        wrap_900.sync_layout(&loaded.text, font("Consolas"), 14.0, Some(900.0), &ts);
        println!(
            "wrap (word-wrap):     {:8.2} ms  ({} rows)",
            t0.elapsed().as_secs_f64() * 1000.0,
            wrap_900.visual_row_count()
        );

        let total_rows = wrap_900.visual_row_count();
        let t0 = Instant::now();
        for i in 0..1000 {
            let r = (i * (total_rows / 1000)).min(total_rows.saturating_sub(1));
            let off = wrap_900.display_to_offset(&loaded.text, DisplayPos::new(r, 0, 0));
            let _ = wrap_900.offset_to_display(&loaded.text, off, false);
        }
        println!(
            "1000 scroll queries:  {:8.2} ms",
            t0.elapsed().as_secs_f64() * 1000.0
        );

        let mut buf = String::new();
        let m_cs = find::Matcher::new("FINAL_SENTINEL_NEEDLE_42", true).unwrap();
        let hit = m_cs.next(&loaded.text, 0, usize::MAX, &mut buf);
        assert!(hit.is_some());

        let m_ci = find::Matcher::new("final_sentinel_needle_42", false).unwrap();
        assert_eq!(m_ci.next(&loaded.text, 0, usize::MAX, &mut buf), hit);

        let m_all = find::Matcher::new("omicron", false).unwrap();
        let _ = m_all.all(&loaded.text, &mut buf);

        let before_bytes = std::fs::read(&file).unwrap();
        document::write_rope(&file, &loaded.text, loaded.format).unwrap();
        let after_bytes = std::fs::read(&file).unwrap();
        assert_eq!(
            before_bytes, after_bytes,
            "large-file round-trip must be byte-exact"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    #[ignore]
    fn large_file_smoke_profile() {
        run_profile(50);
        run_profile(100);
    }
}
