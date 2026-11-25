use crate::geom::point;
use crate::platform::{MouseButton, WindowEvent};
use crate::ui::HitId;

use super::{EDITOR_PADDING_LEFT, EDITOR_PADDING_TOP, TestHarness};

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
    h.send(WindowEvent::MouseMove {
        position: point(p0.x + 40.0, p0.y),
        pressed_button: Some(MouseButton::Left),
    });
    h.draw();
    h.read(|w, _| assert_eq!(w.active_editor().model().text().len(), 0));

    h.type_text("a👨‍👩‍👧b");
    h.press_key("home");
    h.press_key("right");
    h.press_key("right");
    h.read(|w, _| {
        let m = w.active_editor().model();
        assert_eq!(
            m.cursor(),
            "a👨‍👩‍👧".len(),
            "Right must step a full grapheme cluster"
        );
    });
    h.press_key("backspace");
    h.read(|w, _| assert_eq!(w.active_editor().model().text().to_string(), "ab"));

    h.press_key("ctrl-a");
    h.press_key("backspace");
    h.type_text("line1");
    h.press_key("enter");
    h.type_text("line2");
    h.press_key("ctrl-a");
    h.press_key("up");
    h.read(|w, _| {
        let m = w.active_editor().model();
        assert!(m.selection().range().is_empty());
        assert_eq!(m.cursor(), 0, "Up on selection must collapse to selection start");
    });

    h.send(WindowEvent::ImeComposition {
            text: "にほん".into(),
            cursor: 3,
        });
    h.draw();
    h.read(|w, _| assert!(w.active_editor().model().text().to_string().starts_with("にほん")));
    h.press_key("escape");
    h.read(|w, _| {
        assert!(
            !w.active_editor().model().text().to_string().starts_with("にほん"),
            "Escape must remove active IME preedit"
        );
    });

    h.press_key("ctrl-end");
    h.press_key("enter");
    h.press_key("ctrl-backspace");
    h.read(|w, _| {
        assert_eq!(
            w.active_editor().model().text().to_string(),
            "line1\nline2",
            "Ctrl+Backspace at start of empty line must delete only the newline"
        );
    });
}

/// The window reports its earliest deadline and acts on it when ticked: here, a failed search's
/// notice goes away when it comes due.
#[test]
fn window_ticks_its_deadlines() {
    let mut h = TestHarness::new();
    h.type_text("alpha beta");
    h.press_key("ctrl-f");
    h.type_text("gamma");
    h.press_key("enter");
    let due = h
        .read(|w, _| w.find.notice_until)
        .expect("a failed search shows a notice");
    assert!(h.read(|w, _| w.deadline()).is_some_and(|next| next <= due));

    let mut app = h.app.borrow_mut();
    let w = &mut app.windows[0];
    w.dirty = false;
    w.tick(due - std::time::Duration::from_millis(1));
    assert!(w.find.notice_until.is_some());
    w.tick(due);
    assert!(w.dirty && w.find.notice_until.is_none());
}
