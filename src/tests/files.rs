use std::time::Duration;

use crate::document;
use crate::geom::size;
use crate::window::EDITOR_PADDINGS;

use super::TestHarness;

#[test]
fn real_disk_save_routes_completion_to_migrated_tab() {
    let dir = std::env::temp_dir().join(format!("jot-save-mig-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("doc.txt");
    std::fs::write(&path, "initial").unwrap();

    let mut h = TestHarness::new();
    let loaded = document::load(&path).unwrap();
    let tab_id = {
        let mut app_guard = h.app.borrow_mut();
        let app = &mut *app_guard;
        let data = crate::tabs::TabData::create(Some(loaded), true, EDITOR_PADDINGS);
        let id = data.id;
        app.windows[0].replace_tab(0, data, &app.shell.jobs);
        id
    };
    h.draw();
    h.type_text(" migrated");
    h.press_key("ctrl-n");
    h.press_key("ctrl-1");
    h.press_key("ctrl-s");
    {
        let mut app_guard = h.app.borrow_mut();
        let app = &mut *app_guard;
        let data = app.windows[0].take_tab(tab_id, &app.shell.jobs).unwrap();
        let _ = app
            .open_window(Some(data), Some(size(600.0, 400.0)))
            .unwrap();
    }
    for _ in 0..100 {
        h.draw();
        let clean = {
            let app = h.app.borrow();
            let (wix, tix) = app.find_tab_owner(tab_id).expect("tab must exist in win2");
            !app.windows[wix].tabs[tix].data.is_dirty()
        };
        if clean {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    {
        let app = h.app.borrow();
        let (wix, tix) = app.find_tab_owner(tab_id).expect("tab must exist in win2");
        assert_eq!(wix, 1);
        assert!(
            !app.windows[wix].tabs[tix].data.is_dirty(),
            "WriteDone must route by TabId to migrated tab"
        );
    }
    assert_eq!(std::fs::read_to_string(&path).unwrap(), " migratedinitial");
    let _ = std::fs::remove_dir_all(&dir);
}
