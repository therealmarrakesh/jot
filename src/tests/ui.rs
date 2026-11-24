use crate::editor::RopeExt as _;
use crate::geom::point;
use crate::ids::{EditorSlot, Focus, Frame, HitId, MenuKind};
use crate::platform::window::hit_test_control_areas;
use crate::platform::{
    Key, KeyEvent, Modifiers, MouseButton, PlatformEvent, PlatformHandler, WindowControlArea,
};
use crate::ui::controls::{SMALL_FONT, TEXT_FONT};
use crate::ui::{AvailSize, box_, row, text};

use super::{EDITOR_PADDING_LEFT, EDITOR_PADDING_TOP, TestHarness};

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
    h.read(|w, _| {
        assert!(
            !w.ui.last_input_was_keyboard(),
            "clicking submenu item must not activate keyboard focus rings"
        );
    });
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
    let find_card = h.hit_rect(HitId::FindCard).unwrap();
    let beside_find = point(ed_bounds.right() - 30.0, find_card.center().y);
    h.read(|w, _| {
        let chain = w.ui.frame().hit_chain(beside_find);
        assert!(
            chain.contains(&HitId::Editor(w.active_slot())),
            "invisible Find bar row wrapper must not occlude editor hits beside FindCard: {chain:?}"
        );
        assert_eq!(
            w.ui.frame().cursor_at(beside_find),
            crate::platform::CursorStyle::IBeam
        );
    });
    h.click_point(beside_find, MouseButton::Left);
    h.type_text("!");
    h.read(|w, _| {
        assert_eq!(
            w.active_editor().value().as_str(),
            "OMEGA beta OMEGA gamma!",
            "clicking main editor beside Find card must focus main editor"
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
    let live_bar = app.windows[0].status_bar_for_test(p);
    let live_frame = Frame::layout(live_bar, AvailSize::definite(min_size.width, 32.), scale, ts);
    let spacer_w = live_frame
        .custom_rects()
        .find_map(|(tok, rect)| {
            (tok == crate::ids::CustomDraw::StatusSpacer).then_some(rect.size.width)
        })
        .expect("StatusSpacer missing");
    assert!(
        spacer_w < 1.5,
        "status bar spacer must collapse fully to divider width at min_size.width ({}), got {spacer_w}",
        min_size.width
    );
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
            .child(
                box_()
                    .hit(HitId::MenuItem { serial: 0, ix: 0 })
                    .child(text("Ln 1, Col 1")),
            )
            .child(
                box_()
                    .hit(HitId::MenuItem { serial: 0, ix: 1 })
                    .child(text("1 character")),
            )
            .child(box_().flex_1().min_w(1.).h(16.))
            .child(
                box_()
                    .hit(HitId::MenuItem { serial: 0, ix: 2 })
                    .child(text("100%")),
            )
            .child(
                box_()
                    .hit(HitId::MenuItem { serial: 0, ix: 3 })
                    .child(text("Windows (CRLF)")),
            )
            .child(
                box_()
                    .hit(HitId::MenuItem { serial: 0, ix: 4 })
                    .child(text("UTF-8")),
            );
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

    let dialog_body = box_()
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
