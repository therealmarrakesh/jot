use crate::document;
use crate::geom::point;
use crate::gfx::TextStyle;
use crate::platform::{MouseButton, PlatformEvent, PlatformHandler};
use crate::ui::HitId;

use super::{EDITOR_PADDING_LEFT, EDITOR_PADDING_TOP, TestHarness};

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
        assert_eq!(
            std::fs::read(&p).unwrap(),
            *bytes,
            "round-trip mismatch for {name}"
        );
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
    let rope = crate::text::Rope::from("after locked write");
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

    h.app
        .borrow_mut()
        .on_event(PlatformEvent::ImeComposition {
            window_id: h.window_id,
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

#[test]
fn wrap_handles_tabs_emoji_combining_marks_and_u2028_single_line() {
    use crate::editor::WrapIndex;
    use crate::gfx::TextSystem;

    let ts = TextSystem::new().unwrap();
    let sample = "\tindented e\u{0301} 👨‍👩‍👧\u{2028}after_lsep more words to wrap cleanly across rows";
    let rope = crate::text::Rope::from(sample);
    let style = TextStyle::regular("Consolas", 14.0);
    let mut wrap = WrapIndex::new(style.clone(), Some(180.0));
    wrap.sync_layout(&rope, &style, Some(180.0), &ts);
    assert!(wrap.visual_row_count() >= 2);

    let mut wrap_zero = WrapIndex::new(style.clone(), Some(0.0));
    wrap_zero.sync_layout(&rope, &style, Some(0.0), &ts);
    assert!(wrap_zero.visual_row_count() >= 1);

    let shaped = ts.shape_row(sample.into(), &style);
    assert!(shaped.width > 0.0);
}
