use super::*;

#[derive(Clone, Copy)]
pub(crate) struct Overlay {
    pub(crate) window_id: WindowId,
    pub(crate) hwnd: WindowHwnd,
}

pub(crate) fn open(content: WindowContent, cx: &mut App) -> Option<Overlay> {
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
            point(px(0.), px(0.)),
            size(px(1.), px(1.)),
        ))),
        titlebar: None,
        focus: false,
        show: false,
        kind: WindowKind::Overlay,
        window_background: WindowBackgroundAppearance::Transparent,
        ..Default::default()
    };
    let mut hwnd = None;
    let window_id = cx
        .open_window(options, |window, _| {
            hwnd = Some(window.hwnd());
            content
        })
        .ok()?;
    Some(Overlay {
        window_id,
        hwnd: hwnd?,
    })
}

pub(crate) fn screen_rect(bounds: Bounds<Pixels>, scale: f32, origin: ScreenPoint) -> ScreenRect {
    let physical = |v: Pixels| (v.as_f32() * scale).round() as i32;
    ScreenRect {
        left: origin.x + physical(bounds.origin.x),
        top: origin.y + physical(bounds.origin.y),
        right: origin.x + physical(bounds.origin.x + bounds.size.width),
        bottom: origin.y + physical(bounds.origin.y + bounds.size.height),
    }
}

pub(crate) fn place(hwnd: WindowHwnd, rect: ScreenRect) {
    hwnd.place_topmost_noactivate(rect);
}

pub(crate) fn show(hwnd: WindowHwnd) {
    hwnd.show_noactivate();
}

pub(crate) fn hide(hwnd: WindowHwnd) {
    hwnd.hide();
}
