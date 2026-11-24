#![allow(unsafe_code)]

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use crate::app::{App, AppEvent};
use crate::document;
use crate::editor::RopeExt as _;
use crate::find;
use crate::geom::{Point, Rect, ScreenRect, point, size};
use crate::gfx::{Gpu, Surface, font};
use crate::ids::{EditorSlot, Focus, Frame, HitId};
use crate::menu::MenuKind;
use crate::platform::window::hit_test_control_areas;
use crate::platform::{
    Key, KeyEvent, Modifiers, MouseButton, NativeWindow, Platform, PlatformEvent, PlatformHandler,
    WindowAppearance, WindowBackdrop, WindowControlArea, WindowId, WindowKind, WindowOptions,
};
use crate::tabs::TabId;
use crate::ui::controls::{SMALL_FONT, TEXT_FONT};
use crate::ui::{AvailSize, box_, col, row, text};
use crate::window::{EDITOR_PADDINGS, MainWindow};

const EDITOR_PADDING_LEFT: f32 = EDITOR_PADDINGS.left;
const EDITOR_PADDING_TOP: f32 = EDITOR_PADDINGS.top;

fn parse_key(spec: &str) -> KeyEvent {
    let mut modifiers = Modifiers::default();
    let mut key_str = spec;
    for part in spec.split('-') {
        key_str = part;
        match part {
            "ctrl" => modifiers.ctrl = true,
            "alt" => modifiers.alt = true,
            "shift" => modifiers.shift = true,
            _ => {}
        }
    }
    let key = if spec.ends_with("--") || spec == "-" {
        Key::Char('-')
    } else {
        match key_str {
            "alt" => {
                modifiers.alt = false;
                Key::Alt
            }
            "escape" => Key::Escape,
            "enter" => Key::Enter,
            "tab" => Key::Tab,
            "space" => Key::Space,
            "backspace" => Key::Backspace,
            "delete" => Key::Delete,
            "home" => Key::Home,
            "end" => Key::End,
            "up" => Key::Up,
            "down" => Key::Down,
            "left" => Key::Left,
            "right" => Key::Right,
            "f3" => Key::F(3),
            "f4" => Key::F(4),
            "f5" => Key::F(5),
            "f10" => Key::F(10),
            s if s.len() == 1 => Key::Char(s.chars().next().unwrap()),
            other => panic!("unsupported test key spec: {spec:?} ({other:?})"),
        }
    };
    KeyEvent { key, modifiers }
}

struct TestHarness {
    app: Rc<RefCell<App>>,
    window_id: WindowId,
}

impl TestHarness {
    fn new() -> Self {
        let platform = Rc::new(Platform::new().expect("Platform::new failed"));
        let gpu = Gpu::new().expect("Gpu::new failed");
        let mut app = App::new(platform, gpu);
        app.shell.settings.reduce_motion = true;
        app.headless = true;
        let (window_id, _) = app
            .open_window(None, Some(size(1024., 700.)))
            .expect("open_window failed");
        let app = Rc::new(RefCell::new(app));
        let mut harness = Self { app, window_id };
        harness.draw();
        harness
    }

    fn draw(&mut self) {
        for _ in 0..2 {
            let mut app = self.app.borrow_mut();
            app.on_wake();
            if let Some(ix) = app.window_ix(self.window_id) {
                app.windows[ix].dirty = true;
            }
            app.drive_frames();
        }
    }

    fn window_open(&self) -> bool {
        self.app.borrow().window(self.window_id).is_some()
    }

    fn read<R>(&self, f: impl FnOnce(&MainWindow, &App) -> R) -> R {
        let app = self.app.borrow();
        let win = app
            .window(self.window_id)
            .expect("Jot window should be open");
        f(win, &app)
    }

    fn hit_rect(&self, hit: HitId) -> Option<Rect> {
        let app = self.app.borrow();
        app.window(self.window_id)?.ui.frame().rect_of(hit)
    }

    fn click_matching(&mut self, pred: impl Fn(HitId) -> bool, desc: &str) {
        let bounds = {
            let app = self.app.borrow();
            app.window(self.window_id)
                .and_then(|w| w.ui.frame().find_rect(&pred))
                .unwrap_or_else(|| panic!("{desc} was not laid out"))
        };
        assert!(!bounds.is_empty(), "{desc} laid out with empty bounds {bounds:?}");
        self.click_point(bounds.center(), MouseButton::Left);
    }

    fn click_menu_item(&mut self, ix: usize) {
        self.click_matching(
            |h| matches!(h, HitId::MenuItem { ix: i, .. } if i == ix),
            &format!("MenuItem({ix})"),
        );
    }

    fn click_submenu_item(&mut self, ix: usize) {
        self.click_matching(
            |h| matches!(h, HitId::SubmenuItem { ix: i, .. } if i == ix),
            &format!("SubmenuItem({ix})"),
        );
    }

    fn click_menu_command(&mut self, ix: usize) {
        self.click_matching(
            |h| matches!(h, HitId::MenuCommand { ix: i, .. } if i == ix),
            &format!("MenuCommand({ix})"),
        );
    }

    fn move_mouse(&mut self, pos: Point) {
        self.app.borrow_mut().on_event(PlatformEvent::MouseMove {
            window_id: self.window_id,
            position: pos,
            pressed_button: None,
        });
        self.draw();
    }

    fn click_point_count(&mut self, pos: Point, button: MouseButton, click_count: usize) {
        self.move_mouse(pos);
        self.app.borrow_mut().on_event(PlatformEvent::MouseDown {
            window_id: self.window_id,
            button,
            position: pos,
            modifiers: Modifiers::default(),
            click_count,
        });
        self.draw();
        self.app.borrow_mut().on_event(PlatformEvent::MouseUp {
            window_id: self.window_id,
            button,
            position: pos,
        });
        self.draw();
    }

    fn click_point(&mut self, pos: Point, button: MouseButton) {
        self.click_point_count(pos, button, 1);
    }

    fn drag_points(&mut self, from: Point, to: Point) {
        self.move_mouse(from);
        self.app.borrow_mut().on_event(PlatformEvent::MouseDown {
            window_id: self.window_id,
            button: MouseButton::Left,
            position: from,
            modifiers: Modifiers::default(),
            click_count: 1,
        });
        self.draw();
        self.app.borrow_mut().on_event(PlatformEvent::MouseMove {
            window_id: self.window_id,
            position: to,
            pressed_button: Some(MouseButton::Left),
        });
        self.draw();
        self.app.borrow_mut().on_event(PlatformEvent::MouseUp {
            window_id: self.window_id,
            button: MouseButton::Left,
            position: to,
        });
        self.draw();
    }

    fn click_hit(&mut self, hit: HitId) {
        let bounds = self
            .hit_rect(hit)
            .unwrap_or_else(|| panic!("hit {hit:?} was not laid out"));
        assert!(
            !bounds.is_empty(),
            "hit {hit:?} laid out with empty bounds {bounds:?}"
        );
        self.click_point(bounds.center(), MouseButton::Left);
    }

    fn middle_click_hit(&mut self, hit: HitId) {
        let bounds = self
            .hit_rect(hit)
            .unwrap_or_else(|| panic!("hit {hit:?} was not laid out"));
        self.click_point(bounds.center(), MouseButton::Middle);
    }

    fn hover_hit(&mut self, hit: HitId) {
        let bounds = self
            .hit_rect(hit)
            .unwrap_or_else(|| panic!("hit {hit:?} was not laid out"));
        self.move_mouse(bounds.center());
    }

    fn press_key(&mut self, spec: &str) {
        let event = parse_key(spec);
        self.app.borrow_mut().on_event(PlatformEvent::KeyDown {
            window_id: self.window_id,
            event,
        });
        self.app.borrow_mut().on_event(PlatformEvent::KeyUp {
            window_id: self.window_id,
            event,
        });
        self.draw();
    }

    fn type_text(&mut self, s: &str) {
        self.app.borrow_mut().on_event(PlatformEvent::TextInput {
            window_id: self.window_id,
            text: s.to_string(),
        });
        self.draw();
    }

    fn scroll_wheel(&mut self, pos: Point, lines_y: f32, modifiers: Modifiers) {
        self.app.borrow_mut().on_event(PlatformEvent::ScrollWheel {
            window_id: self.window_id,
            position: pos,
            delta_lines: point(0.0, lines_y),
            page_scroll: false,
            modifiers,
        });
        self.draw();
    }

    fn close_all(&mut self) {
        let mut app = self.app.borrow_mut();
        for w in &mut app.windows {
            w.pending_close = true;
        }
        app.sweep_closed_windows();
    }
}

impl Drop for TestHarness {
    fn drop(&mut self) {
        self.close_all();
    }
}

#[test]
fn tab_close_button_new_tab_hover_and_middle_click() {
    let mut h = TestHarness::new();

    h.click_hit(HitId::NewTab);
    h.click_hit(HitId::NewTab);
    let ids: Vec<TabId> = h.read(|w, _| {
        assert_eq!(w.tabs.len(), 3);
        assert_eq!(w.tabs.active_index(), 2);
        w.tabs.iter().map(|t| t.data.id).collect()
    });

    h.click_hit(HitId::Tab(ids[0]));
    h.read(|w, _| assert_eq!(w.tabs.active_index(), 0));
    h.click_hit(HitId::TabClose(ids[0]));
    h.read(|w, app| {
        assert_eq!(w.tabs.len(), 2, "clicking active tab 'x' must close tab");
        assert!(w.tab_index(ids[0]).is_none());
        assert!(
            app.tab_drag.is_none(),
            "tab close button must not start parent tab drag"
        );
    });

    h.hover_hit(HitId::Tab(ids[2]));
    assert!(
        h.hit_rect(HitId::TabClose(ids[2])).is_some(),
        "hovering inactive tab must reveal its 'x' button"
    );
    h.click_hit(HitId::TabClose(ids[2]));
    h.read(|w, _| {
        assert_eq!(
            w.tabs.len(),
            1,
            "clicking hovered inactive tab 'x' must close tab"
        );
        assert_eq!(w.tabs[0].data.id, ids[1]);
    });

    h.click_hit(HitId::NewTab);
    let second_id = h.read(|w, _| {
        assert_eq!(w.tabs.len(), 2);
        w.tabs[1].data.id
    });
    h.middle_click_hit(HitId::Tab(second_id));
    h.read(|w, _| assert_eq!(w.tabs.len(), 1, "middle-clicking tab must close it"));

    h.type_text("unsaved text");
    let last_id = h.read(|w, _| w.tabs[0].data.id);
    h.hover_hit(HitId::Tab(last_id));
    h.click_hit(HitId::TabClose(last_id));
    h.read(|w, _| {
        assert!(
            w.dialogs.is_open(),
            "closing dirty tab via 'x' must open save confirmation dialog"
        );
    });
    h.click_hit(HitId::DialogButton(1));
    assert!(
        !h.window_open(),
        "confirming Don't save on last tab must close the window"
    );

    let mut h2 = TestHarness::new();
    let sole_id = h2.read(|w, _| w.tabs[0].data.id);
    h2.click_hit(HitId::TabClose(sole_id));
    assert!(
        !h2.window_open(),
        "clicking 'x' on sole clean tab must close the window"
    );
}

#[test]
fn menus_shortcuts_toggles_submenus_and_context_menu() {
    let mut h = TestHarness::new();

    h.click_hit(HitId::MenuTrigger(MenuKind::File));
    h.read(|w, _| assert!(w.menu_bar.is_open()));
    h.click_menu_item(0);
    h.read(|w, _| {
        assert_eq!(w.tabs.len(), 2, "File -> New tab must add a tab");
        assert!(!w.menu_bar.is_open(), "menu must close after activation");
    });

    h.click_hit(HitId::MenuTrigger(MenuKind::View));
    h.click_menu_item(0);
    h.click_submenu_item(0);
    h.read(|w, _| {
        assert_eq!(
            (w.zoom() * 100.).round() as u32,
            110,
            "View -> Zoom -> Zoom in must zoom to 110%"
        );
    });

    h.click_hit(HitId::MenuTrigger(MenuKind::View));
    h.click_menu_item(2);
    h.read(|_, app| assert!(!app.shell.view.word_wrap));

    h.click_hit(HitId::MenuTrigger(MenuKind::View));
    h.click_menu_item(1);
    h.read(|_, app| assert!(!app.shell.view.status_bar));

    h.click_hit(HitId::MenuTrigger(MenuKind::File));
    h.hover_hit(HitId::MenuTrigger(MenuKind::Edit));
    h.read(|w, _| {
        assert_eq!(w.menu_bar.open_kind(), Some(MenuKind::Edit));
    });
    let editor_bounds = h.hit_rect(HitId::EditorArea).unwrap();
    h.click_point(editor_bounds.center(), MouseButton::Left);
    h.read(|w, _| {
        assert!(
            !w.menu_bar.is_open(),
            "clicking outside menu must close it"
        )
    });

    h.press_key("alt");
    h.read(|w, _| assert!(w.menu_bar.access_active()));
    h.press_key("f");
    h.read(|w, _| assert!(w.menu_bar.is_open(), "Alt then F must open File menu"));
    h.press_key("down");
    h.press_key("escape");
    h.press_key("escape");
    h.read(|w, _| {
        assert!(!w.menu_bar.is_open());
        assert!(!w.menu_bar.access_active());
    });

    h.type_text("delete me");
    h.press_key("ctrl-a");
    let on_text = point(
        editor_bounds.left() + EDITOR_PADDING_LEFT + 12.0,
        editor_bounds.top() + EDITOR_PADDING_TOP + 10.0,
    );
    h.click_point(on_text, MouseButton::Right);
    h.read(|w, _| {
        assert!(
            w.menu_bar.is_open(),
            "right-click must open context menu"
        )
    });
    h.click_menu_command(4);
    h.read(|w, _| {
        assert!(!w.menu_bar.is_open());
        assert_eq!(
            w.active_editor().text().len(),
            0,
            "context menu Delete command must erase selected text"
        );
    });
}

#[test]
fn find_replace_bar_clear_button_and_key_contexts() {
    let mut h = TestHarness::new();
    h.type_text("alpha beta alpha gamma");

    h.press_key("ctrl-f");
    h.read(|w, _| assert!(w.find.is_open()));

    h.type_text("alpha");
    h.press_key("enter");
    h.read(|w, _| assert_eq!(w.active_editor().selected_range(), 0..5));
    h.press_key("enter");
    h.read(|w, _| assert_eq!(w.active_editor().selected_range(), 11..16));
    h.press_key("shift-enter");
    h.read(|w, _| assert_eq!(w.active_editor().selected_range(), 0..5));

    assert!(
        h.hit_rect(HitId::FieldClear(EditorSlot::FindQuery))
            .is_some(),
        "find query clear 'x' button must be laid out when query is non-empty"
    );
    h.click_hit(HitId::FieldClear(EditorSlot::FindQuery));
    assert!(
        h.hit_rect(HitId::FieldClear(EditorSlot::FindQuery))
            .is_none(),
        "clicking field-clear 'x' must empty query so clear button disappears"
    );

    h.type_text("ALPHA");
    h.click_hit(HitId::FindOptions);
    h.read(|w, _| assert!(w.menu_bar.is_open(), "find-options must open flyout"));
    h.click_menu_item(0);
    h.read(|w, _| assert!(w.find.match_case));
    h.click_hit(HitId::FindOptions);
    h.click_menu_item(0);
    h.read(|w, _| assert!(!w.find.match_case));

    h.click_hit(HitId::FindToggleReplace);
    h.click_hit(HitId::Editor(EditorSlot::FindReplace));
    h.type_text("OMEGA");
    h.read(|w, _| {
        assert_eq!(w.active_editor().value().as_str(), "alpha beta alpha gamma");
    });
    h.click_hit(HitId::FindNext);
    h.press_key("enter");
    h.read(|w, _| {
        assert_eq!(
            w.active_editor().value().as_str(),
            "alpha beta OMEGA gamma",
            "Enter in replacement field must replace current match"
        );
    });
    h.click_hit(HitId::FindReplaceAll);
    h.read(|w, _| {
        assert_eq!(
            w.active_editor().value().as_str(),
            "OMEGA beta OMEGA gamma",
            "Replace all must replace remaining matches"
        );
    });

    let ed_bounds = h.hit_rect(HitId::EditorArea).unwrap();
    let ed_click = point(ed_bounds.right() - 40.0, ed_bounds.bottom() - 40.0);
    h.click_point(ed_click, MouseButton::Left);
    h.type_text("!");
    h.read(|w, _| {
        assert_eq!(
            w.active_editor().value().as_str(),
            "OMEGA beta OMEGA gamma!",
            "clicking main editor while Find bar is open must focus main editor"
        );
    });

    h.click_hit(HitId::FindClose);
    h.read(|w, _| assert!(!w.find.is_open(), "find-close 'x' must close Find bar"));
}

#[test]
fn go_to_dialog_navigation_and_out_of_range_flow() {
    let mut h = TestHarness::new();
    h.type_text("one");
    h.press_key("enter");
    h.type_text("two");
    h.press_key("enter");
    h.type_text("three");
    h.press_key("enter");
    h.type_text("four");

    h.press_key("ctrl-g");
    h.read(|w, _| assert!(w.dialogs.is_open()));
    h.type_text("2");
    h.press_key("enter");
    h.read(|w, _| {
        assert!(!w.dialogs.is_open());
        let ed = w.active_editor();
        assert_eq!(ed.text().offset_to_point(ed.cursor()).row, 1);
    });

    h.press_key("ctrl-g");
    h.type_text("999");
    h.press_key("enter");
    h.read(|w, _| assert!(w.dialogs.is_open()));
    h.click_hit(HitId::DialogButton(0));
    h.read(|w, _| {
        assert!(w.dialogs.is_open());
        assert_eq!(w.focus, Focus::Editor(EditorSlot::GoToLine));
    });
    h.press_key("tab");
    h.read(|w, _| assert_eq!(w.focus, Focus::DialogButton(0)));
    h.press_key("shift-tab");
    h.read(|w, _| assert_eq!(w.focus, Focus::Editor(EditorSlot::GoToLine)));
    h.press_key("escape");
    h.read(|w, _| assert!(!w.dialogs.is_open()));
}

#[test]
fn theme_zoom_editing_undo_scrollbar_and_caption_hit_tests() {
    let mut h = TestHarness::new();

    let initial_dark = h.read(|_, app| app.shell.palette.dark);
    h.click_hit(HitId::ThemeToggle);
    h.read(|_, app| assert_eq!(app.shell.palette.dark, !initial_dark));
    h.click_hit(HitId::ThemeToggle);
    h.read(|_, app| assert_eq!(app.shell.palette.dark, initial_dark));

    h.press_key("ctrl-=");
    h.read(|w, _| assert_eq!((w.zoom() * 100.).round() as u32, 110));
    h.press_key("ctrl--");
    h.read(|w, _| assert_eq!((w.zoom() * 100.).round() as u32, 100));
    let editor_center = h.hit_rect(HitId::EditorArea).unwrap().center();
    h.scroll_wheel(
        editor_center,
        3.0,
        Modifiers {
            ctrl: true,
            ..Default::default()
        },
    );
    h.read(|w, _| assert_eq!((w.zoom() * 100.).round() as u32, 110));
    h.press_key("ctrl-0");
    h.read(|w, _| assert_eq!((w.zoom() * 100.).round() as u32, 100));

    h.type_text("hello world");
    h.press_key("ctrl-a");
    h.read(|w, _| assert_eq!(w.active_editor().selected_range(), 0..11));
    h.press_key("backspace");
    h.read(|w, _| assert_eq!(w.active_editor().text().len(), 0));
    h.press_key("ctrl-z");
    h.read(|w, _| assert_eq!(w.active_editor().value().as_str(), "hello world"));
    h.press_key("ctrl-y");
    h.read(|w, _| assert_eq!(w.active_editor().text().len(), 0));

    let big: String = (0..120).map(|i| format!("Line {i}\n")).collect();
    h.type_text(&big);
    h.press_key("ctrl-home");
    h.scroll_wheel(editor_center, -6.0, Modifiers::default());
    let after_wheel_y = h.read(|w, _| w.active_editor().scroll_offset().y);
    assert!(
        after_wheel_y < 0.0,
        "scrolling down with mouse wheel must scroll editor viewport"
    );
    let ed_bounds = h.hit_rect(HitId::EditorArea).unwrap();
    let track_pt = point(ed_bounds.right() - 6.0, ed_bounds.bottom() - 40.0);
    h.click_point(track_pt, MouseButton::Left);
    h.read(|w, _| {
        let ed = w.active_editor();
        assert!(
            ed.scroll_offset().y < after_wheel_y,
            "clicking vertical scrollbar track must jump scroll offset further down"
        );
        assert!(
            ed.selected_range().is_empty(),
            "clicking scrollbar must not select editor text"
        );
    });

    h.press_key("ctrl-a");
    h.press_key("backspace");
    h.type_text("alpha beta gamma");
    let first_char = point(
        ed_bounds.left() + EDITOR_PADDING_LEFT + 12.0,
        ed_bounds.top() + EDITOR_PADDING_TOP + 10.0,
    );
    h.click_point_count(first_char, MouseButton::Left, 2);
    h.read(|w, _| {
        assert_eq!(
            w.active_editor().selected_range(),
            0..5,
            "double-click must select word"
        );
    });
    h.click_point_count(first_char, MouseButton::Left, 3);
    h.read(|w, _| {
        assert_eq!(
            w.active_editor().selected_range(),
            0..16,
            "triple-click must select full line"
        );
    });
    let drag_start = point(
        ed_bounds.left() + EDITOR_PADDING_LEFT + 1.0,
        ed_bounds.top() + EDITOR_PADDING_TOP + 10.0,
    );
    let drag_end = point(
        ed_bounds.left() + EDITOR_PADDING_LEFT + 70.0,
        ed_bounds.top() + EDITOR_PADDING_TOP + 10.0,
    );
    h.drag_points(drag_start, drag_end);
    h.read(|w, _| {
        let sel = w.active_editor().selected_range();
        assert!(
            !sel.is_empty(),
            "dragging in editor must create a text selection"
        );
    });

    let close_pt = h.hit_rect(HitId::CaptionClose).unwrap().center();
    let max_pt = h.hit_rect(HitId::CaptionMax).unwrap().center();
    let min_pt = h.hit_rect(HitId::CaptionMin).unwrap().center();
    h.read(|w, _| {
        let areas = w.ui.frame().control_areas();
        assert_eq!(
            hit_test_control_areas(&areas, close_pt),
            Some(WindowControlArea::Close)
        );
        assert_eq!(
            hit_test_control_areas(&areas, max_pt),
            Some(WindowControlArea::Max)
        );
        assert_eq!(
            hit_test_control_areas(&areas, min_pt),
            Some(WindowControlArea::Min)
        );
    });
}

#[test]
fn dirty_window_close_file_drop_and_multi_window() {
    let mut h = TestHarness::new();

    let tmp = std::env::temp_dir().join(format!("jot-drop-test-{}.txt", std::process::id()));
    std::fs::write(&tmp, "dropped content 123").unwrap();
    h.app.borrow_mut().on_event(PlatformEvent::FilesDropped {
        window_id: h.window_id,
        paths: vec![tmp.clone()],
    });
    for _ in 0..200 {
        h.draw();
        if h.read(|w, _| w.active_editor().value().as_str() == "dropped content 123") {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    h.read(|w, _| {
        assert_eq!(w.active_editor().value().as_str(), "dropped content 123");
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
    h.app
        .borrow_mut()
        .on_event(PlatformEvent::CloseRequested {
            window_id: h.window_id,
        });
    h.draw();
    assert!(h.window_open(), "dirty window must intercept close and show prompt");
    h.read(|w, _| assert!(w.dialogs.is_open()));
    h.click_hit(HitId::DialogButton(2));
    assert!(h.window_open(), "Cancel must keep window open");

    h.app
        .borrow_mut()
        .on_event(PlatformEvent::CloseRequested {
            window_id: h.window_id,
        });
    h.draw();
    h.click_hit(HitId::DialogButton(1));
    assert!(!h.window_open(), "Don't save must close the window");
}

#[test]
fn overlay_window_resizes_from_1x1_and_renders() {
    let h = TestHarness::new();
    let gpu = h.app.borrow().shell.gpu.clone();
    let opts = WindowOptions {
        bounds: Some(Rect::new(point(0., 0.), size(1., 1.))),
        title: "".into(),
        focus: false,
        show: false,
        kind: WindowKind::Overlay,
        is_resizable: false,
        is_minimizable: false,
        display_id: None,
        backdrop: WindowBackdrop::Transparent,
        min_size: None,
    };
    let mut overlay = NativeWindow::create(&opts, WindowAppearance::Dark).expect("overlay create");
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
fn status_bar_min_width_and_no_vertical_char_wrapping() {
    use windows::Win32::{
        Foundation::{LPARAM, WPARAM},
        UI::WindowsAndMessaging::{MINMAXINFO, SendMessageW, WM_GETMINMAXINFO},
    };

    let mut h = TestHarness::new();
    h.type_text("a");

    let (hwnd, min_size, scale) = h.read(|w, _| {
        let cached = w
            .min_size_cached()
            .expect("min_size cache must populate on first draw");
        (w.native.hwnd(), cached, w.native.scale_factor())
    });
    assert!(
        min_size.width >= 420.0,
        "status bar min width must reflect unwrapped single-line labels, got {}",
        min_size.width
    );

    let mut mmi = MINMAXINFO::default();
    unsafe {
        let _ = SendMessageW(
            hwnd.raw(),
            WM_GETMINMAXINFO,
            Some(WPARAM(0)),
            Some(LPARAM((&raw mut mmi) as isize)),
        );
    }
    assert!(
        mmi.ptMinTrackSize.x >= (min_size.width * scale).ceil() as i32,
        "WM_GETMINMAXINFO ptMinTrackSize.x ({}) must enforce min_size.width ({})",
        mmi.ptMinTrackSize.x,
        min_size.width
    );

    let app = h.app.borrow();
    let ts = &app.shell.text_system;
    let p = app.shell.palette;
    for test_width in [min_size.width, 220.0] {
        let bar = row()
            .w(test_width)
            .items_center()
            .gap(24.)
            .h(31.)
            .px(22.)
            .font_family(SMALL_FONT)
            .text_size(12.)
            .text_color(p.text_secondary)
            .nowrap()
            .child(box_().hit(HitId::MenuItem { serial: 0, ix: 0 }).child(text("Ln 1, Col 1")))
            .child(box_().hit(HitId::MenuItem { serial: 0, ix: 1 }).child(text("1 character")))
            .child(box_().flex_1().min_w(1.).h(16.))
            .child(box_().hit(HitId::MenuItem { serial: 0, ix: 2 }).child(text("100%")))
            .child(
                box_()
                    .hit(HitId::MenuItem { serial: 0, ix: 3 })
                    .child(text("Windows (CRLF)")),
            )
            .child(box_().hit(HitId::MenuItem { serial: 0, ix: 4 }).child(text("UTF-8")));
        let frame = Frame::layout(bar, AvailSize::definite(test_width, 31.), scale, ts);
        for ix in 0..5 {
            let b = frame
                .rect_of(HitId::MenuItem { serial: 0, ix })
                .expect("status bar label rect missing");
            assert!(
                b.size.height <= 31.0,
                "label {ix} wrapped vertically at width {test_width}: {b:?}"
            );
            assert!(
                b.size.width >= 25.0,
                "label {ix} squished below single-line width at {test_width}: {b:?}"
            );
        }
    }

    let dialog_body = col()
        .w(180.0)
        .font_family(TEXT_FONT)
        .text_size(14.0)
        .line_height(20.0)
        .child(
            box_()
                .hit(HitId::DialogCard)
                .child(text(
                    "Do you want to save changes to a very long untitled document?",
                )),
        );
    let dlg_frame = Frame::layout(
        dialog_body,
        AvailSize {
            width: crate::ui::layout::AvailableSpace::Definite(180.0),
            height: crate::ui::layout::AvailableSpace::MaxContent,
        },
        scale,
        ts,
    );
    let msg_rect = dlg_frame.rect_of(HitId::DialogCard).unwrap();
    assert!(
        msg_rect.size.height >= 40.0,
        "constrained dialog text must wrap across multiple lines: {msg_rect:?}"
    );
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
fn window_anim_tracks_prune_unused_tracks_and_word_wrap_updates_tabs() {
    let mut h = TestHarness::new();
    let baseline_tracks = h.read(|w, _| w.ui.track_count());

    h.press_key("ctrl-g");
    let open_tracks = h.read(|w, _| w.ui.track_count());
    assert!(
        open_tracks > baseline_tracks,
        "dialog open must allocate anim tracks ({open_tracks} > {baseline_tracks})"
    );

    h.press_key("escape");
    h.draw();
    let closed_tracks = h.read(|w, _| w.ui.track_count());
    assert_eq!(
        closed_tracks, baseline_tracks,
        "closing dialog must prune unused anim tracks back to baseline"
    );

    h.press_key("ctrl-n");
    h.read(|w, app| {
        assert_eq!(w.tabs.len(), 2);
        assert!(app.shell.view.word_wrap);
        for tab in w.tabs.iter() {
            assert!(tab.data.editor.soft_wrap_enabled());
        }
    });
    h.click_hit(HitId::MenuTrigger(MenuKind::View));
    h.click_menu_item(2);
    h.read(|w, app| {
        assert!(!app.shell.view.word_wrap);
        for tab in w.tabs.iter() {
            assert!(!tab.data.editor.soft_wrap_enabled());
        }
    });
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
    Platform::set_active_handler_for_test(Some(dyn_handler));

    h.press_key("ctrl-n");
    h.read(|w, _| assert_eq!(w.tabs.len(), 2));

    let (hwnd1, scale, tab_rect, tab_id) = h.read(|w, _| {
        let id = w.tabs[1].data.id;
        let rect = w.ui.frame().rect_of(HitId::Tab(id)).expect("tab 1 rect");
        (w.native.hwnd(), w.native.scale_factor(), rect, id)
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
            app.tab_drag_press(
                h.window_id,
                tab_id,
                1,
                point(20.0, 12.0),
                tab_rect.center(),
            );
            app.tab_drag_detach_for_test();
        }
        app.drive_frames();
        assert_eq!(app.windows.len(), 2, "popping out tab must create 2nd window");
        assert_eq!(app.windows[0].tabs.len(), 1);
        assert_eq!(app.windows[1].tabs.len(), 1);
        app.windows[1].native.hwnd()
    };

    unsafe {
        let _ = SendMessageW(hwnd1.raw(), WM_MOUSELEAVE, Some(WPARAM(0)), Some(LPARAM(0)));
        let _ = SendMessageW(hwnd1.raw(), WM_NCMOUSELEAVE, Some(WPARAM(0)), Some(LPARAM(0)));
        let _ = SendMessageW(hwnd2.raw(), WM_NCHITTEST, Some(WPARAM(0)), Some(LPARAM(0)));
    }

    {
        let mut app = h.app.borrow_mut();
        app.tab_drag_poll();
        app.drive_frames();
        app.tab_drag_cancel();
        app.drive_frames();
        assert_eq!(app.windows.len(), 1, "canceling detach must return tab to source");
        assert_eq!(app.windows[0].tabs.len(), 2);
    }

    h.close_all();
    Platform::set_active_handler_for_test(None);
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
        assert!(w.dialogs.is_open(), "stale WriteDone must not dismiss ConfirmDiscard");
        assert_eq!(w.tabs.len(), 2, "stale WriteDone must not silently close tab B");
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
        let _ = app.open_window(Some(data), Some(size(600.0, 400.0))).unwrap();
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

#[test]
fn byte_exact_document_round_trips_and_locked_target_fallback() {
    let dir = std::env::temp_dir().join(format!("jot-roundtrip-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);

    let cases: &[(&str, Vec<u8>)] = &[
        ("empty.txt", b"".to_vec()),
        ("bom_only.txt", b"\xEF\xBB\xBF".to_vec()),
        ("utf8_crlf.txt", b"one\r\ntwo\r\n".to_vec()),
        ("utf8_lf_notrail.txt", b"one\ntwo".to_vec()),
        ("utf8_cr.txt", b"one\rtwo\r".to_vec()),
        ("utf8_bom.txt", b"\xEF\xBB\xBFhello \xF0\x9F\x8E\x89\r\n".to_vec()),
        ("utf16le.txt", {
            let mut v = vec![0xFF, 0xFE];
            for u in "hi 日本語\r\n".encode_utf16() {
                v.extend_from_slice(&u.to_le_bytes());
            }
            v
        }),
        ("utf16be.txt", {
            let mut v = vec![0xFE, 0xFF];
            for u in "hi 日本語\n".encode_utf16() {
                v.extend_from_slice(&u.to_be_bytes());
            }
            v
        }),
    ];

    for (name, bytes) in cases {
        let p = dir.join(name);
        std::fs::write(&p, bytes).unwrap();
        let loaded = document::load(&p).unwrap();
        assert!(!loaded.format.mixed_line_endings, "{name} should not be mixed");
        document::write_rope(&p, &loaded.text, loaded.format).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), *bytes, "round-trip mismatch for {name}");
    }

    let mixed_path = dir.join("mixed.txt");
    std::fs::write(&mixed_path, b"a\r\nb\nc\r\n").unwrap();
    let mixed = document::load(&mixed_path).unwrap();
    assert!(mixed.format.mixed_line_endings);

    use std::os::windows::fs::OpenOptionsExt;
    let locked_path = dir.join("locked.txt");
    std::fs::write(&locked_path, b"before").unwrap();
    let _holder = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0x0000_0001 | 0x0000_0002)
        .open(&locked_path)
        .unwrap();
    let rope = crate::editor::Rope::from("after locked write");
    document::write_rope(&locked_path, &rope, document::Format::default()).unwrap();
    drop(_holder);
    assert_eq!(std::fs::read(&locked_path).unwrap(), b"after locked write");
    let leftovers: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().contains(".jot-"))
        .collect();
    assert!(leftovers.is_empty(), "leftover temp files: {leftovers:?}");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn editor_invariants_grapheme_ime_word_anchor_up_collapse_and_insert_keys() {
    let mut h = TestHarness::new();
    let ed_bounds = h.hit_rect(HitId::EditorArea).unwrap();

    h.type_text("hello world");
    let p0 = point(
        ed_bounds.left() + EDITOR_PADDING_LEFT + 10.0,
        ed_bounds.top() + EDITOR_PADDING_TOP + 10.0,
    );
    h.click_point_count(p0, MouseButton::Left, 2);
    h.press_key("ctrl-a");
    h.press_key("backspace");
    h.app.borrow_mut().on_event(PlatformEvent::MouseMove {
        window_id: h.window_id,
        position: point(p0.x + 40.0, p0.y),
        pressed_button: Some(MouseButton::Left),
    });
    h.draw();
    h.read(|w, _| assert_eq!(w.active_editor().text().len(), 0));

    h.type_text("a👨‍👩‍👧b");
    h.press_key("home");
    h.press_key("right");
    h.press_key("right");
    h.read(|w, _| {
        let ed = w.active_editor();
        assert_eq!(ed.cursor(), "a👨‍👩‍👧".len(), "Right must step a full grapheme cluster");
    });
    h.press_key("backspace");
    h.read(|w, _| assert_eq!(w.active_editor().value().as_str(), "ab"));

    h.press_key("ctrl-a");
    h.press_key("backspace");
    h.type_text("line1");
    h.press_key("enter");
    h.type_text("line2");
    h.press_key("ctrl-a");
    h.press_key("up");
    h.read(|w, _| {
        let ed = w.active_editor();
        assert!(ed.selected_range().is_empty());
        assert_eq!(ed.cursor(), 0, "Up on selection must collapse to selection start");
    });

    h.app
        .borrow_mut()
        .on_event(PlatformEvent::ImeComposition {
            window_id: h.window_id,
            text: "にほん".into(),
            cursor: 3,
        });
    h.draw();
    h.read(|w, _| assert!(w.active_editor().value().starts_with("にほん")));
    h.press_key("escape");
    h.read(|w, _| {
        assert!(
            !w.active_editor().value().starts_with("にほん"),
            "Escape must remove active IME preedit"
        );
    });

    h.press_key("ctrl-end");
    h.press_key("enter");
    h.press_key("ctrl-backspace");
    h.read(|w, _| {
        assert_eq!(
            w.active_editor().value().as_str(),
            "line1\nline2",
            "Ctrl+Backspace at start of empty line must delete only the newline"
        );
    });
}

#[test]
fn keyboard_system_chords_altgr_menu_gate_popup_occlusion_and_fractional_borders() {
    let mut h = TestHarness::new();
    h.press_key("ctrl-n");

    let alt_f4 = KeyEvent {
        key: Key::F(4),
        modifiers: Modifiers {
            alt: true,
            ..Default::default()
        },
    };
    let alt_space = KeyEvent {
        key: Key::Space,
        modifiers: Modifiers {
            alt: true,
            ..Default::default()
        },
    };
    assert!(!h.app.borrow_mut().on_event(PlatformEvent::KeyDown {
        window_id: h.window_id,
        event: alt_f4,
    }));
    assert!(!h.app.borrow_mut().on_event(PlatformEvent::KeyDown {
        window_id: h.window_id,
        event: alt_space,
    }));

    h.press_key("alt");
    h.read(|w, _| assert!(w.menu_bar.access_active()));
    assert!(!h.app.borrow_mut().on_event(PlatformEvent::KeyDown {
        window_id: h.window_id,
        event: alt_f4,
    }));

    h.click_hit(HitId::MenuTrigger(MenuKind::File));
    h.read(|w, _| assert!(w.menu_bar.is_open()));
    h.press_key("ctrl-w");
    h.read(|w, _| {
        assert_eq!(w.tabs.len(), 2, "Ctrl+W must not fire behind an open menu");
    });
    assert!(!h.app.borrow_mut().on_event(PlatformEvent::KeyDown {
        window_id: h.window_id,
        event: alt_f4,
    }));

    let altgr_s = KeyEvent {
        key: Key::Char('s'),
        modifiers: Modifiers {
            ctrl: true,
            alt: true,
            alt_gr: true,
            ..Default::default()
        },
    };
    assert!(
        crate::command::app_shortcut(altgr_s).is_none(),
        "AltGr+S must not match Ctrl+Alt+S SaveAll"
    );

    h.press_key("ctrl-g");
    h.read(|w, _| assert!(w.dialogs.is_open()));
    assert!(!h.app.borrow_mut().on_event(PlatformEvent::KeyDown {
        window_id: h.window_id,
        event: alt_f4,
    }));
    let close_pt = h.hit_rect(HitId::CaptionClose).unwrap().center();
    h.read(|w, _| {
        assert_eq!(
            hit_test_control_areas(&w.ui.frame().control_areas(), close_pt),
            None,
            "modal dialog backdrop must occlude caption buttons"
        );
    });
    h.press_key("escape");

    for scale in [1.25f32, 1.5, 1.75] {
        let snapped = crate::ui::layout::snap_border_edges(crate::geom::Edges::all(1.0), scale);
        let px = snapped.top * scale;
        assert!(
            (px - px.round()).abs() < 1e-4 && px >= 1.0,
            "1 DIP border at {scale}x must snap to integer device pixels, got {px}"
        );
    }
}

#[test]
fn wrap_handles_tabs_emoji_combining_marks_and_u2028_single_line() {
    use crate::editor::wrap::WrapIndex;
    use crate::gfx::TextSystem;

    let ts = TextSystem::new();
    let sample = "\tindented e\u{0301} 👨‍👩‍👧\u{2028}after_lsep more words to wrap cleanly across rows";
    let rope = crate::editor::Rope::from(sample);
    let mut wrap = WrapIndex::new(font("Consolas"), 14.0, Some(180.0));
    wrap.sync_layout(&rope, font("Consolas"), 14.0, Some(180.0), &ts);
    assert!(wrap.visual_row_count() >= 2);

    let mut wrap_zero = WrapIndex::new(font("Consolas"), 14.0, Some(0.0));
    wrap_zero.sync_layout(&rope, font("Consolas"), 14.0, Some(0.0), &ts);
    assert!(wrap_zero.visual_row_count() >= 1);

    let shaped = ts.shape_row(sample.into(), &crate::gfx::TextStyle::regular("Consolas", 14.0));
    assert!(shaped.width > 0.0);
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
        println!("load:                 {:8.2} ms", t0.elapsed().as_secs_f64() * 1000.0);

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
        println!("1000 scroll queries:  {:8.2} ms", t0.elapsed().as_secs_f64() * 1000.0);

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
        assert_eq!(before_bytes, after_bytes, "large-file round-trip must be byte-exact");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    #[ignore]
    fn large_file_smoke_profile() {
        run_profile(50);
        run_profile(100);
    }
}
