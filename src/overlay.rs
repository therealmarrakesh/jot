use super::*;
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::UI::WindowsAndMessaging::{
    HWND_TOPMOST, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SetWindowPos, ShowWindow,
};

pub(crate) struct Overlay<V> {
    pub view: Entity<V>,
    pub handle: AnyWindowHandle,
    pub hwnd: isize,
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
            created = Some((view.clone(), window_hwnd(window)));
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

pub(crate) fn screen_rect(bounds: Bounds<Pixels>, scale: f32, origin: POINT) -> RECT {
    let physical = |v: Pixels| (v.as_f32() * scale).round() as i32;
    RECT {
        left: origin.x + physical(bounds.origin.x),
        top: origin.y + physical(bounds.origin.y),
        right: origin.x + physical(bounds.origin.x + bounds.size.width),
        bottom: origin.y + physical(bounds.origin.y + bounds.size.height),
    }
}

pub(crate) fn same_rect(a: RECT, b: RECT) -> bool {
    (a.left, a.top, a.right, a.bottom) == (b.left, b.top, b.right, b.bottom)
}

pub(crate) fn place(hwnd: isize, rect: RECT) {
    let _ = unsafe {
        SetWindowPos(
            HWND(hwnd as _),
            Some(HWND_TOPMOST),
            rect.left,
            rect.top,
            rect.right - rect.left,
            rect.bottom - rect.top,
            SWP_NOACTIVATE,
        )
    };
}

pub(crate) fn show(hwnd: isize) {
    let _ = unsafe { ShowWindow(HWND(hwnd as _), SW_SHOWNOACTIVATE) };
}

pub(crate) fn hide(hwnd: isize) {
    let _ = unsafe { ShowWindow(HWND(hwnd as _), SW_HIDE) };
}
