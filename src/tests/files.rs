use std::time::Duration;

use crate::app::AppEvent;
use crate::document;
use crate::geom::size;
use crate::ids::HitId;
use crate::platform::{PlatformEvent, PlatformHandler};
use crate::window::EDITOR_PADDINGS;

use super::TestHarness;

#[test]
fn stale_file_events_do_not_advance_unrelated_discard_workflow() {
    let mut h = TestHarness::new();
    h.type_text("tab A content");
    h.press_key("ctrl-n");
    h.type_text("tab B unsaved");
    let tab_b = h.read(|w, _| w.tabs[1].data.id);

    h.press_key("ctrl-w");
    h.read(|w, _| {
        assert!(w.dialogs.is_open(), "ConfirmDiscard must be open for tab B");
        assert_eq!(w.tabs.len(), 2);
    });

    let stale_wf = crate::files::WorkflowId::next();
    h.app
        .borrow()
        .shell
        .jobs
        .post(AppEvent::File(crate::files::FileEvent::WriteDone {
            wf: stale_wf,
            outcome: crate::files::WriteOutcome::Saved {
                doc_path: document::DocPath::quick(std::path::PathBuf::from(r"C:\Temp\stale.txt")),
                format: document::Format::default(),
            },
        }));
    h.draw();

    h.read(|w, _| {
        assert!(
            w.dialogs.is_open(),
            "stale WriteDone must not dismiss ConfirmDiscard"
        );
        assert_eq!(
            w.tabs.len(),
            2,
            "stale WriteDone must not silently close tab B"
        );
        assert!(w.tab_index(tab_b).is_some());
    });
}

#[test]
fn close_save_keeps_window_open_when_edited_mid_write_and_migrated_save_cleans_tab() {
    let dir = std::env::temp_dir().join(format!("jot-save-mig-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("doc.txt");
    std::fs::write(&path, "initial").unwrap();

    let mut h = TestHarness::new();
    let loaded = document::load(&path).unwrap();
    {
        let mut app_guard = h.app.borrow_mut();
        let app = &mut *app_guard;
        let data = crate::tabs::TabData::create(Some(loaded), true, EDITOR_PADDINGS);
        app.windows[0].replace_tab(0, data, &app.shell.jobs);
    }
    h.draw();
    h.type_text(" v2");
    h.app
        .borrow_mut()
        .on_event(PlatformEvent::CloseRequested {
            window_id: h.window_id,
        });
    h.draw();
    h.read(|w, _| assert!(w.dialogs.is_open()));
    h.click_hit(HitId::DialogButton(0));
    h.type_text(" extra");
    for _ in 0..100 {
        h.draw();
        if !h.window_open() || h.read(|w, _| w.dialogs.is_open()) {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        h.window_open(),
        "typing during close->Save write must keep window open and re-prompt"
    );
    h.read(|w, _| {
        assert!(w.tabs.active().is_dirty());
        assert!(w.dialogs.is_open());
    });
    h.click_hit(HitId::DialogButton(1));
    assert!(!h.window_open());

    let mut h2 = TestHarness::new();
    let loaded2 = document::load(&path).unwrap();
    let tab_id = {
        let mut app_guard = h2.app.borrow_mut();
        let app = &mut *app_guard;
        let data = crate::tabs::TabData::create(Some(loaded2), true, EDITOR_PADDINGS);
        let id = data.id;
        app.windows[0].replace_tab(0, data, &app.shell.jobs);
        id
    };
    h2.draw();
    h2.type_text(" migrated");
    h2.press_key("ctrl-n");
    // Trigger Ctrl+S on tab_id (via real FileOps workflow), then migrate tab_id to a 2nd window.
    h2.press_key("ctrl-1");
    h2.press_key("ctrl-s");
    {
        let mut app_guard = h2.app.borrow_mut();
        let app = &mut *app_guard;
        let data = app.windows[0].take_tab(tab_id, &app.shell.jobs).unwrap();
        let _ = app
            .open_window(Some(data), Some(size(600.0, 400.0)))
            .unwrap();
    }
    for _ in 0..100 {
        h2.draw();
        let clean = {
            let app = h2.app.borrow();
            let (wix, tix) = app.find_tab_owner(tab_id).expect("tab must exist in win2");
            !app.windows[wix].tabs[tix].data.is_dirty()
        };
        if clean {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    {
        let app = h2.app.borrow();
        let (wix, tix) = app.find_tab_owner(tab_id).expect("tab must exist in win2");
        assert_eq!(wix, 1);
        assert!(
            !app.windows[wix].tabs[tix].data.is_dirty(),
            "WriteDone must route by TabId to migrated tab"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}
