#![allow(unsafe_code)]

mod editor;
mod platform;
mod tabs;
mod ui;

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use crate::{App, WindowLaunch};
use crate::command::Command;
use crate::document;
use crate::geom::{Point, Rect, size};
use crate::gfx::{Gpu, TextSystem};
use crate::platform::{
    Key, KeyEvent, Modifiers, MouseButton, Platform, PlatformEvent, PlatformHandler, WindowId,
};
use crate::ui::HitId;
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
        let platform = Rc::new(Platform::new(None).expect("Platform::new failed"));
        let gpu = Gpu::new().expect("Gpu::new failed");
        let text_system = TextSystem::new().expect("TextSystem::new failed");
        let mut app = App::with_launch(platform, gpu, text_system, WindowLaunch::HIDDEN);
        app.shell.settings.reduce_motion = true;
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
            app.windows.mark_dirty(self.window_id);
            app.drive_frames();
        }
    }

    fn window_open(&self) -> bool {
        self.app.borrow().windows.get(self.window_id).is_some()
    }

    fn read<R>(&self, f: impl FnOnce(&MainWindow, &App) -> R) -> R {
        let app = self.app.borrow();
        let win = app
            .windows
            .get(self.window_id)
            .expect("Jot window should be open");
        f(win, &app)
    }

    fn hit_rect(&self, hit: HitId) -> Option<Rect> {
        let app = self.app.borrow();
        app.windows.get(self.window_id)?.ui.frame().rect_of(hit)
    }

    fn click_menu_command(&mut self, cmd: impl Into<Command>) {
        let cmd = cmd.into();
        let hit = self
            .read(|w, _| w.menus.hit_for_command(cmd))
            .unwrap_or_else(|| panic!("menu command {cmd:?} is not visible"));
        self.click_hit(hit);
    }

    fn click_menu_submenu(&mut self, title: &str) {
        let hit = self
            .read(|w, _| w.menus.hit_for_submenu(title))
            .unwrap_or_else(|| panic!("menu submenu {title:?} is not visible"));
        self.click_hit(hit);
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
            delta_lines: crate::geom::point(0.0, lines_y),
            page_scroll: false,
            modifiers,
        });
        self.draw();
    }

    fn close_all(&mut self) {
        let mut app = self.app.borrow_mut();
        for w in app.windows.iter_mut() {
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
        app.windows[0].replace_tab(0, data, &app.shell);
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
        let data = app.windows[0].take_tab(tab_id, &app.shell).unwrap();
        let _ = app
            .open_window(Some(data), Some(size(600.0, 400.0)))
            .unwrap();
    }
    for _ in 0..100 {
        h.draw();
        let clean = {
            let app = h.app.borrow();
            let (wix, tix) = app.windows.owner_of(tab_id).expect("tab must exist in win2");
            !app.windows[wix].tabs()[tix].data.is_dirty()
        };
        if clean {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    {
        let app = h.app.borrow();
        let (wix, tix) = app.windows.owner_of(tab_id).expect("tab must exist in win2");
        assert_eq!(wix, 1);
        assert!(
            !app.windows[wix].tabs()[tix].data.is_dirty(),
            "WriteDone must route by TabId to migrated tab"
        );
    }
    assert_eq!(std::fs::read_to_string(&path).unwrap(), " migratedinitial");
    let _ = std::fs::remove_dir_all(&dir);
}
