use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use crate::geom::point;
use crate::platform::{Platform, PlatformHandler, WindowEvent};
use crate::ui::{DialogHit, HitId, TabHit, TabId};

use super::TestHarness;

#[test]
fn tab_close_button_new_tab_hover_and_middle_click() {
    let mut h = TestHarness::new();

    h.click_hit(HitId::Tab(TabHit::New));
    h.click_hit(HitId::Tab(TabHit::New));
    let ids: Vec<TabId> = h.read(|w, _| {
        assert_eq!(w.tabs.len(), 3);
        assert_eq!(w.tabs.active_index(), 2);
        w.tabs.iter().map(|t| t.data.id).collect()
    });

    h.click_hit(HitId::Tab(TabHit::Item(ids[0])));
    h.read(|w, _| assert_eq!(w.tabs.active_index(), 0));
    h.click_hit(HitId::Tab(TabHit::Close(ids[0])));
    h.read(|w, app| {
        assert_eq!(w.tabs.len(), 2, "clicking active tab 'x' must close tab");
        assert!(w.tabs.index_of(ids[0]).is_none());
        assert!(
            !app.is_tab_drag_active(),
            "tab close button must not start parent tab drag"
        );
    });

    h.hover_hit(HitId::Tab(TabHit::Item(ids[2])));
    assert!(
        h.hit_rect(HitId::Tab(TabHit::Close(ids[2]))).is_some(),
        "hovering inactive tab must reveal its 'x' button"
    );
    h.click_hit(HitId::Tab(TabHit::Close(ids[2])));
    h.read(|w, _| {
        assert_eq!(
            w.tabs.len(),
            1,
            "clicking hovered inactive tab 'x' must close tab"
        );
        assert_eq!(w.tabs[0].data.id, ids[1]);
    });

    h.click_hit(HitId::Tab(TabHit::New));
    let second_id = h.read(|w, _| {
        assert_eq!(w.tabs.len(), 2);
        w.tabs[1].data.id
    });
    h.middle_click_hit(HitId::Tab(TabHit::Item(second_id)));
    h.read(|w, _| assert_eq!(w.tabs.len(), 1, "middle-clicking tab must close it"));

    h.type_text("unsaved text");
    let last_id = h.read(|w, _| w.tabs[0].data.id);
    h.hover_hit(HitId::Tab(TabHit::Item(last_id)));
    h.click_hit(HitId::Tab(TabHit::Close(last_id)));
    h.read(|w, _| {
        assert!(
            w.dialogs.is_open(),
            "closing dirty tab via 'x' must open save confirmation dialog"
        );
    });
    h.click_hit(HitId::Dialog(DialogHit::Button(1)));
    assert!(
        !h.window_open(),
        "confirming Don't save on last tab must close the window"
    );

    let mut h2 = TestHarness::new();
    let sole_id = h2.read(|w, _| w.tabs[0].data.id);
    h2.click_hit(HitId::Tab(TabHit::Close(sole_id)));
    assert!(
        !h2.window_open(),
        "clicking 'x' on sole clean tab must close the window"
    );
}

#[test]
fn dirty_window_close_file_drop_and_multi_window() {
    let mut h = TestHarness::new();

    let tmp = std::env::temp_dir().join(format!("jot-drop-test-{}.txt", std::process::id()));
    std::fs::write(&tmp, "dropped content 123").unwrap();
    h.send(WindowEvent::FilesDropped(vec![tmp.clone()]));
    for _ in 0..200 {
        h.draw();
        if h.read(|w, _| w.tabs.active().editor.model().text() == "dropped content 123") {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    h.read(|w, _| {
        assert_eq!(
            w.tabs.active().editor.model().text().to_string(),
            "dropped content 123"
        );
        assert!(w.tabs[0].data.doc_path.is_some());
    });
    let _ = std::fs::remove_file(&tmp);

    h.press_key("ctrl-shift-n");
    {
        let mut app = h.app.borrow_mut();
        assert_eq!(app.windows.len(), 2, "Ctrl+Shift+N must open a 2nd window");
        app.windows[1].pending_close = true;
        app.sweep_closed_windows();
        assert_eq!(app.windows.len(), 1);
    }

    h.type_text("!");
    h.send(WindowEvent::CloseRequested);
    h.draw();
    assert!(
        h.window_open(),
        "dirty window must intercept close and show prompt"
    );
    h.read(|w, _| assert!(w.dialogs.is_open()));
    h.click_hit(HitId::Dialog(DialogHit::Button(2)));
    assert!(h.window_open(), "Cancel must keep window open");

    h.send(WindowEvent::CloseRequested);
    h.draw();
    h.click_hit(HitId::Dialog(DialogHit::Button(1)));
    assert!(!h.window_open(), "Don't save must close the window");
}

#[test]
fn tab_drag_pop_out_and_reattach_does_not_panic() {
    use windows::Win32::{
        Foundation::{LPARAM, WPARAM},
        UI::{
            Controls::WM_MOUSELEAVE,
            WindowsAndMessaging::{
                SendMessageW, WM_LBUTTONDOWN, WM_MOUSEMOVE, WM_NCHITTEST, WM_NCMOUSELEAVE,
            },
        },
    };

    let mut h = TestHarness::new();
    let dyn_handler: Rc<RefCell<dyn PlatformHandler>> = h.app.clone();
    Platform::with_test_handler(dyn_handler, || {
        h.press_key("ctrl-n");
        h.read(|w, _| assert_eq!(w.tabs.len(), 2));

        let (hwnd1, scale, tab_rect, tab_id) = h.read(|w, _| {
            let id = w.tabs[1].data.id;
            let rect = w
                .ui.frame
                .rect_of(HitId::Tab(TabHit::Item(id)))
                .expect("tab 1 rect");
            (w.hwnd, w.win.window.scale_factor(), rect, id)
        });

        let cx = (tab_rect.center().x * scale).round() as i32;
        let cy = (tab_rect.center().y * scale).round() as i32;
        let down_lp = LPARAM((((cy as u16 as u32) << 16) | (cx as u16 as u32)) as isize);
        unsafe {
            let _ = SendMessageW(hwnd1.raw(), WM_LBUTTONDOWN, Some(WPARAM(1)), Some(down_lp));
        }

        let move_y = ((tab_rect.center().y + 200.0) * scale).round() as i32;
        let move_lp = LPARAM((((move_y as u16 as u32) << 16) | (cx as u16 as u32)) as isize);
        unsafe {
            let _ = SendMessageW(hwnd1.raw(), WM_MOUSEMOVE, Some(WPARAM(1)), Some(move_lp));
        }

        let hwnd2 = {
            let mut app = h.app.borrow_mut();
            if app.windows.len() == 1 {
                app.tab_drag_start_and_detach_for_test(
                    h.window_id,
                    tab_id,
                    1,
                    point(20.0, 12.0),
                    tab_rect.center(),
                );
            }
            app.drive_frames();
            assert_eq!(app.windows.len(), 2, "popping out tab must create 2nd window");
            assert_eq!(app.windows[0].tabs.len(), 1);
            assert_eq!(app.windows[1].tabs.len(), 1);
            app.windows[1].hwnd
        };

        unsafe {
            let _ = SendMessageW(hwnd1.raw(), WM_MOUSELEAVE, Some(WPARAM(0)), Some(LPARAM(0)));
            let _ = SendMessageW(hwnd1.raw(), WM_NCMOUSELEAVE, Some(WPARAM(0)), Some(LPARAM(0)));
            let _ = SendMessageW(hwnd2.raw(), WM_NCHITTEST, Some(WPARAM(0)), Some(LPARAM(0)));
        }

        {
            let mut app = h.app.borrow_mut();
            app.tab_drag_poll_for_test();
            app.drive_frames();
            app.tab_drag_cancel_for_test();
            app.drive_frames();
            assert_eq!(
                app.windows.len(),
                1,
                "canceling detach must return tab to source"
            );
            assert_eq!(app.windows[0].tabs.len(), 2);
        }

        h.close_all();
    });
}
