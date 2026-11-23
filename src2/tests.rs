//! End-to-end GUI regression tests and ignored large-file smoke benchmark ported from `src/`.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use crate::app::{App, AppEvent};
use crate::color::rgba;
use crate::document;
use crate::editor::RopeExt as _;
use crate::find;
use crate::geom::{Point, Rect, ScreenRect, point, size};
use crate::gfx::font;
use crate::menu::MenuKind;
use crate::platform::{
    Key, KeyEvent, Modifiers, MouseButton, NativeWindow, Platform, PlatformEvent, PlatformHandler,
    WindowAppearance, WindowBackdrop, WindowControlArea, WindowId, WindowKind, WindowOptions,
};
use crate::tabs::TabId;
use crate::ui::controls::{SMALL_FONT, TEXT_FONT};
use crate::ui::{AvailSize, EditorSlot, Frame, HitId, box_, col, row, text};
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
    // Special case: "ctrl--" splits into ["ctrl", "", ""] so last part is empty -> '-'.
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
        let mut app = App::new(platform);
        app.reduce_motion = true;
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
        app.window(self.window_id)?.ui.frame.rect_of(hit)
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
        assert_eq!(w.active, 2);
        w.tabs.iter().map(|t| t.data.id).collect()
    });

    // Click tab 0 to switch to it, then click its 'x' close button.
    h.click_hit(HitId::Tab(ids[0].0));
    h.read(|w, _| assert_eq!(w.active, 0));
    h.click_hit(HitId::TabClose(ids[0].0));
    h.read(|w, app| {
        assert_eq!(w.tabs.len(), 2, "clicking active tab 'x' must close tab");
        assert!(w.tab_index(ids[0]).is_none());
        assert!(
            app.tab_drag.is_none(),
            "tab close button must not start parent tab drag"
        );
    });

    // Hover inactive tab (ids[2]) so its 'x' appears, then click 'x' to close it.
    h.hover_hit(HitId::Tab(ids[2].0));
    assert!(
        h.hit_rect(HitId::TabClose(ids[2].0)).is_some(),
        "hovering inactive tab must reveal its 'x' button"
    );
    h.click_hit(HitId::TabClose(ids[2].0));
    h.read(|w, _| {
        assert_eq!(
            w.tabs.len(),
            1,
            "clicking hovered inactive tab 'x' must close tab"
        );
        assert_eq!(w.tabs[0].data.id, ids[1]);
    });

    // Add a tab and close it with middle-click.
    h.click_hit(HitId::NewTab);
    let second_id = h.read(|w, _| {
        assert_eq!(w.tabs.len(), 2);
        w.tabs[1].data.id
    });
    h.middle_click_hit(HitId::Tab(second_id.0));
    h.read(|w, _| assert_eq!(w.tabs.len(), 1, "middle-clicking tab must close it"));

    // Dirty sole remaining tab -> click 'x' -> discard prompt opens -> click "Don't save" -> closes window.
    h.type_text("unsaved text");
    let last_id = h.read(|w, _| w.tabs[0].data.id);
    h.hover_hit(HitId::Tab(last_id.0));
    h.click_hit(HitId::TabClose(last_id.0));
    h.read(|w, _| {
        assert!(
            w.dialogs.is_open(),
            "closing dirty tab via 'x' must open save confirmation dialog"
        );
    });
    h.click_hit(HitId::DialogButton(1)); // "Don't save"
    assert!(
        !h.window_open(),
        "confirming Don't save on last tab must close the window"
    );

    // Closing a clean sole tab via 'x' immediately closes the window.
    let mut h2 = TestHarness::new();
    let sole_id = h2.read(|w, _| w.tabs[0].data.id);
    h2.click_hit(HitId::TabClose(sole_id.0));
    assert!(
        !h2.window_open(),
        "clicking 'x' on sole clean tab must close the window"
    );
}

#[test]
fn menus_shortcuts_toggles_submenus_and_context_menu() {
    let mut h = TestHarness::new();

    // Open File menu -> click "New tab" (row 0).
    h.click_hit(HitId::MenuTrigger(MenuKind::File.index()));
    h.read(|w, _| assert!(w.menu_bar.is_open()));
    h.click_hit(HitId::MenuItem(0));
    h.read(|w, _| {
        assert_eq!(w.tabs.len(), 2, "File -> New tab must add a tab");
        assert!(!w.menu_bar.is_open(), "menu must close after activation");
    });

    // Open View menu -> open Zoom submenu (row 0) -> click "Zoom in" (submenu row 0).
    h.click_hit(HitId::MenuTrigger(MenuKind::View.index()));
    h.click_hit(HitId::MenuItem(0));
    h.click_hit(HitId::SubmenuItem(0));
    h.read(|w, _| {
        assert_eq!(
            (w.zoom() * 100.).round() as u32,
            110,
            "View -> Zoom -> Zoom in must zoom to 110%"
        );
    });

    // Open View menu -> toggle Word wrap (row 2) and Status bar (row 1).
    h.click_hit(HitId::MenuTrigger(MenuKind::View.index()));
    h.click_hit(HitId::MenuItem(2));
    h.read(|_, app| assert!(!app.view.word_wrap));

    h.click_hit(HitId::MenuTrigger(MenuKind::View.index()));
    h.click_hit(HitId::MenuItem(1));
    h.read(|_, app| assert!(!app.view.status_bar));

    // Open File menu -> hover Edit trigger -> switches open menu; click editor -> closes menu.
    h.click_hit(HitId::MenuTrigger(MenuKind::File.index()));
    h.hover_hit(HitId::MenuTrigger(MenuKind::Edit.index()));
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

    // Alt activates access key mode; 'f' opens File menu; Down + Escape + Escape navigates & closes.
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

    // Right-click on selected text opens context menu with Delete enabled; clicking Delete erases it.
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
    h.click_hit(HitId::MenuCommand(4)); // Delete
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

    // Type query, Enter -> first "alpha"; Enter -> second "alpha"; Shift+Enter -> first "alpha".
    h.type_text("alpha");
    h.press_key("enter");
    h.read(|w, _| assert_eq!(w.active_editor().selected_range(), 0..5));
    h.press_key("enter");
    h.read(|w, _| assert_eq!(w.active_editor().selected_range(), 11..16));
    h.press_key("shift-enter");
    h.read(|w, _| assert_eq!(w.active_editor().selected_range(), 0..5));

    // Inline clear 'x' button clears query field and disappears.
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

    // Re-type query, open Find options flyout -> toggle Match case on & off.
    h.type_text("ALPHA");
    h.click_hit(HitId::FindOptions);
    h.read(|w, _| assert!(w.menu_bar.is_open(), "find-options must open flyout"));
    h.click_hit(HitId::MenuItem(0)); // Match case ON
    h.read(|w, _| assert!(w.find.match_case));
    h.click_hit(HitId::FindOptions);
    h.click_hit(HitId::MenuItem(0)); // Match case OFF
    h.read(|w, _| assert!(!w.find.match_case));

    // Expand replace row -> click replacement field -> type "OMEGA" -> click FindNext -> Enter -> Replace all.
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

    // Clicking inside main editor while Find bar is open transfers focus back to main editor.
    let ed_bounds = h.hit_rect(HitId::EditorArea).unwrap();
    let ed_click = point(
        ed_bounds.left() + EDITOR_PADDING_LEFT + 5.0,
        ed_bounds.bottom() - 40.0,
    );
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

    // Ctrl+G -> type "2" -> Enter jumps cursor to line 2 (row 1).
    h.press_key("ctrl-g");
    h.read(|w, _| assert!(w.dialogs.is_open()));
    h.type_text("2");
    h.press_key("enter");
    h.read(|w, _| {
        assert!(!w.dialogs.is_open());
        let ed = w.active_editor();
        assert_eq!(ed.text().offset_to_point(ed.cursor()).row, 1);
    });

    // Out-of-range line -> error dialog -> OK returns to Go To -> Tab / Shift+Tab -> Escape closes.
    h.press_key("ctrl-g");
    h.type_text("999");
    h.press_key("enter");
    h.read(|w, _| assert!(w.dialogs.is_open()));
    h.click_hit(HitId::DialogButton(0)); // OK -> returns to Go To
    h.read(|w, _| assert!(w.dialogs.is_open()));
    h.press_key("tab");
    h.press_key("shift-tab");
    h.press_key("escape");
    h.read(|w, _| assert!(!w.dialogs.is_open()));
}

#[test]
fn theme_zoom_editing_undo_scrollbar_and_caption_hit_tests() {
    let mut h = TestHarness::new();

    // Theme button toggles dark/light and back.
    let initial_dark = h.read(|_, app| app.palette.dark);
    h.click_hit(HitId::ThemeToggle);
    h.read(|_, app| assert_eq!(app.palette.dark, !initial_dark));
    h.click_hit(HitId::ThemeToggle);
    h.read(|_, app| assert_eq!(app.palette.dark, initial_dark));

    // Zoom via keyboard and Ctrl+wheel.
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

    // Typing, Select All, Undo, Redo, Backspace.
    h.type_text("hello world");
    h.press_key("ctrl-a");
    h.read(|w, _| assert_eq!(w.active_editor().selected_range(), 0..11));
    h.press_key("backspace");
    h.read(|w, _| assert_eq!(w.active_editor().text().len(), 0));
    h.press_key("ctrl-z");
    h.read(|w, _| assert_eq!(w.active_editor().value().as_str(), "hello world"));
    h.press_key("ctrl-y");
    h.read(|w, _| assert_eq!(w.active_editor().text().len(), 0));

    // Scroll wheel scrolls multi-line document, activates Scrollbar, and clicking vertical scrollbar
    // track jumps scroll offset without selecting editor text.
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

    // Double-click word selection, triple-click line selection, and drag selection.
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

    // Caption button hit-testing resolves Min / Max / Close areas cleanly.
    let close_pt = h.hit_rect(HitId::CaptionClose).unwrap().center();
    let max_pt = h.hit_rect(HitId::CaptionMax).unwrap().center();
    let min_pt = h.hit_rect(HitId::CaptionMin).unwrap().center();
    h.read(|w, _| {
        assert_eq!(
            w.ui.frame.hit_test_control_area(close_pt),
            Some(WindowControlArea::Close)
        );
        assert_eq!(
            w.ui.frame.hit_test_control_area(max_pt),
            Some(WindowControlArea::Max)
        );
        assert_eq!(
            w.ui.frame.hit_test_control_area(min_pt),
            Some(WindowControlArea::Min)
        );
    });
}

#[test]
fn dirty_window_close_file_drop_and_multi_window() {
    let mut h = TestHarness::new();

    // File drop opens a real file from disk into the blank initial tab.
    let tmp = std::env::temp_dir().join(format!("jot2-drop-test-{}.txt", std::process::id()));
    std::fs::write(&tmp, "dropped content 123").unwrap();
    h.app.borrow_mut().on_event(PlatformEvent::FilesDropped {
        window_id: h.window_id,
        paths: vec![tmp.clone()],
    });
    for _ in 0..40 {
        h.draw();
        if h.read(|w, _| w.active_editor().value().as_str() == "dropped content 123") {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    h.read(|w, _| {
        assert_eq!(w.active_editor().value().as_str(), "dropped content 123");
        assert!(w.tabs[0].data.path.is_some());
    });
    let _ = std::fs::remove_file(&tmp);

    // Multi-window via Ctrl+Shift+N creates a second window cleanly.
    h.press_key("ctrl-shift-n");
    {
        let mut app = h.app.borrow_mut();
        assert_eq!(app.windows.len(), 2, "Ctrl+Shift+N must open a 2nd window");
        app.windows[1].pending_close = true;
        app.sweep_closed_windows();
        assert_eq!(app.windows.len(), 1);
    }

    // Dirty window close prompt: Cancel keeps window open, Don't save closes it.
    h.type_text("!");
    h.app
        .borrow_mut()
        .on_event(PlatformEvent::CloseRequested {
            window_id: h.window_id,
        });
    h.draw();
    assert!(h.window_open(), "dirty window must intercept close and show prompt");
    h.read(|w, _| assert!(w.dialogs.is_open()));
    h.click_hit(HitId::DialogButton(2)); // Cancel
    assert!(h.window_open(), "Cancel must keep window open");

    h.app
        .borrow_mut()
        .on_event(PlatformEvent::CloseRequested {
            window_id: h.window_id,
        });
    h.draw();
    h.click_hit(HitId::DialogButton(1)); // Don't save
    assert!(!h.window_open(), "Don't save must close the window");
}

#[test]
fn overlay_window_resizes_from_1x1_and_renders() {
    let h = TestHarness::new();
    let devices = h.app.borrow().platform.devices.clone();
    let opts = WindowOptions {
        bounds: Some(Rect::new(point(0., 0.), size(1., 1.))),
        title: "".into(),
        hide_titlebar: true,
        focus: false,
        show: false,
        kind: WindowKind::Overlay,
        is_resizable: false,
        is_minimizable: false,
        display_id: None,
        backdrop: WindowBackdrop::Transparent,
        min_size: None,
    };
    let mut overlay =
        NativeWindow::create(devices, &opts, WindowAppearance::Dark).expect("overlay create");
    overlay.hwnd().place_topmost_noactivate(ScreenRect {
        left: 10,
        top: 10,
        right: 170,
        bottom: 50,
    });
    assert!(overlay.sync_live_metrics());
    assert!(
        overlay.logical_size.width > 10.0 && overlay.logical_size.height > 10.0,
        "overlay must sync resized client rect before drawing"
    );
    overlay.renderer.begin_frame(crate::color::transparent_black());
    let lost = overlay.renderer.end_frame().expect("overlay end_frame");
    assert!(!lost);
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
        (w.native.hwnd(), cached, w.native.scale_factor)
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

    // Status bar labels stay single-line even when squished, while constrained dialog text wraps.
    let app = h.app.borrow();
    let ts = &app.text_system;
    let p = app.palette;
    for test_width in [min_size.width, 220.0] {
        let bar = row()
            .w(test_width)
            .items_center()
            .gap(24.)
            .h(31.)
            .px(22.)
            .font_family(SMALL_FONT)
            .text_size(12.)
            .text_color(rgba(p.text_secondary))
            .nowrap()
            .child(box_().hit(HitId::MenuItem(0)).child(text("Ln 1, Col 1")))
            .child(box_().hit(HitId::MenuItem(1)).child(text("1 character")))
            .child(box_().flex_1().min_w(1.).h(16.))
            .child(box_().hit(HitId::MenuItem(2)).child(text("100%")))
            .child(
                box_()
                    .hit(HitId::MenuItem(3))
                    .child(text("Windows (CRLF)")),
            )
            .child(box_().hit(HitId::MenuItem(4)).child(text("UTF-8")));
        let frame = Frame::layout(bar, AvailSize::definite(test_width, 31.), scale, ts);
        for ix in 0..5 {
            let b = frame
                .rect_of(HitId::MenuItem(ix))
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

    // 1. Re-entrant WM_ACTIVATE while App is mutably borrowed must queue, not drop.
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
    h.read(|w, _| {
        assert!(
            w.native.active,
            "queued re-entrant WM_ACTIVATE must apply once drained"
        );
    });

    // 2. Modal size/move loop timer pumps jobs while dragging edges.
    let ran = Arc::new(AtomicBool::new(false));
    h.app
        .borrow()
        .jobs
        .poster()
        .post(AppEvent::TestFlag(ran.clone()));

    unsafe {
        let _ = SendMessageW(
            hwnd.raw(),
            WM_ENTERSIZEMOVE,
            Some(WPARAM(0)),
            Some(LPARAM(0)),
        );
    }
    h.read(|w, _| assert!(w.native.in_size_move));
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
    h.read(|w, _| assert!(!w.native.in_size_move));

    // 3. Closing window cleans up HWND_REGISTRY atomically.
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
    let app = Rc::new(RefCell::new(App::new(platform)));
    Platform::run(app, |app| {
        app.headless = true;
        let (id, _) = app.open_window(None, None).expect("open_window failed");
        app.jobs
            .after(Duration::from_millis(25), AppEvent::TestCloseWindow(id))
            .detach();
    });
}

#[test]
fn window_anim_tracks_prune_unused_tracks_and_word_wrap_updates_tabs() {
    let mut h = TestHarness::new();
    let baseline_tracks = h.read(|w, _| w.ui.anims.track_count());

    h.press_key("ctrl-g");
    let open_tracks = h.read(|w, _| w.ui.anims.track_count());
    assert!(
        open_tracks > baseline_tracks,
        "dialog open must allocate anim tracks ({open_tracks} > {baseline_tracks})"
    );

    h.press_key("escape");
    h.draw();
    let closed_tracks = h.read(|w, _| w.ui.anims.track_count());
    assert_eq!(
        closed_tracks, baseline_tracks,
        "closing dialog must prune unused anim tracks back to baseline"
    );

    h.press_key("ctrl-n");
    h.read(|w, app| {
        assert_eq!(w.tabs.len(), 2);
        assert!(app.view.word_wrap);
        for tab in &w.tabs {
            assert!(tab.data.editor.soft_wrap_enabled());
        }
    });
    h.click_hit(HitId::MenuTrigger(MenuKind::View.index()));
    h.click_hit(HitId::MenuItem(2));
    h.read(|w, app| {
        assert!(!app.view.word_wrap);
        for tab in &w.tabs {
            assert!(!tab.data.editor.soft_wrap_enabled());
        }
    });
}

mod large_file_smoke {
    use super::*;
    use crate::editor::wrap::{DisplayPos, WrapIndex};
    use crate::gfx::TextSystem;
    use std::io::Write as _;
    use std::time::Instant;

    fn run_profile(target_mb: usize) {
        let dir = std::env::temp_dir()
            .join(format!("jot2-bench-{target_mb}mb-{}", std::process::id()));
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
            let off = wrap_900.display_to_offset(DisplayPos::new(r, 0, 0));
            let _ = wrap_900.offset_to_display(off, false);
        }
        println!("1000 scroll queries:  {:8.2} ms", t0.elapsed().as_secs_f64() * 1000.0);

        let mut buf = Vec::new();
        let m_cs = find::Matcher::new("FINAL_SENTINEL_NEEDLE_42", true).unwrap();
        let hit = m_cs.next(&loaded.text, 0, usize::MAX, &mut buf);
        assert!(hit.is_some());

        let m_ci = find::Matcher::new("final_sentinel_needle_42", false).unwrap();
        assert_eq!(m_ci.next(&loaded.text, 0, usize::MAX, &mut buf), hit);

        let m_all = find::Matcher::new("omicron", false).unwrap();
        let _ = m_all.all(&loaded.text, &mut buf);

        document::write_rope(&file, &loaded.text, loaded.format).unwrap();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    #[ignore]
    fn large_file_smoke_profile() {
        run_profile(50);
        run_profile(100);
    }
}
