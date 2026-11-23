use super::*;

pub(crate) struct Overlay<V> {
    pub view: Entity<V>,
    pub handle: AnyWindowHandle,
    pub hwnd: WindowHwnd,
}

impl<V> Clone for Overlay<V> {
    fn clone(&self) -> Self {
        Self {
            view: self.view.clone(),
            handle: self.handle,
            hwnd: self.hwnd,
        }
    }
}

pub(crate) fn open<V: Render>(
    build: impl FnOnce(&mut Context<V>) -> V,
    cx: &mut App,
) -> Option<Overlay<V>> {
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
            point(px(0.), px(0.)),
            size(px(1.), px(1.)),
        ))),
        titlebar: None,
        focus: false,
        show: false,
        kind: WindowKind::Overlay,
        inactive_frame_interval: None,
        window_background: WindowBackgroundAppearance::Transparent,
        ..Default::default()
    };
    let mut created = None;
    let handle = cx
        .open_window(options, |window, cx| {
            let view = cx.new(build);
            created = Some((view.clone(), window.hwnd()));
            view
        })
        .ok()?;
    let (view, hwnd) = created?;
    Some(Overlay {
        view,
        handle: handle.into(),
        hwnd,
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
