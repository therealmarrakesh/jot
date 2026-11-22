use super::*;
use windows::Win32::Foundation::{COLORREF, HWND, POINT, RECT};
use windows::Win32::Graphics::Dwm::{
    DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE, DWMWA_TRANSITIONS_FORCEDISABLED,
    DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND, DWMWINDOWATTRIBUTE, DwmSetWindowAttribute,
};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GWL_STYLE, GetWindowLongW, HWND_TOPMOST, LWA_ALPHA, SW_HIDE, SW_SHOWNOACTIVATE,
    SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
    SetLayeredWindowAttributes, SetWindowLongW, SetWindowPos, ShowWindow, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::{PCSTR, w};

#[repr(C)]
struct AccentPolicy {
    state: u32,
    flags: u32,
    color: u32,
    animation: u32,
}

#[repr(C)]
struct CompositionAttribute {
    attribute: u32,
    data: *mut std::ffi::c_void,
    size: usize,
}

const ACCENT_POLICY: u32 = 19;
const ACCENT_DISABLED: u32 = 0;

type SetCompositionAttribute =
    unsafe extern "system" fn(hwnd: HWND, data: *mut CompositionAttribute) -> i32;

fn clear_backdrop(hwnd: isize) {
    let Ok(module) = (unsafe { GetModuleHandleW(w!("user32.dll")) }) else {
        return;
    };
    let Some(address) = (unsafe {
        GetProcAddress(module, PCSTR(c"SetWindowCompositionAttribute".as_ptr().cast()))
    }) else {
        return;
    };
    let set: SetCompositionAttribute = unsafe { std::mem::transmute(address) };
    let mut policy = AccentPolicy {
        state: ACCENT_DISABLED,
        flags: 0,
        color: 0,
        animation: 0,
    };
    let mut data = CompositionAttribute {
        attribute: ACCENT_POLICY,
        data: (&raw mut policy).cast(),
        size: size_of::<AccentPolicy>(),
    };
    unsafe { set(HWND(hwnd as _), &mut data) };
}

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
        kind: WindowKind::PopUp,
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
    let window = HWND(hwnd as _);
    let set = |attribute: DWMWINDOWATTRIBUTE, value: u32| {
        let _ = unsafe {
            DwmSetWindowAttribute(
                window,
                attribute,
                (&raw const value).cast(),
                size_of::<u32>() as u32,
            )
        };
    };
    set(DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND.0 as u32);
    set(DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE);
    set(DWMWA_TRANSITIONS_FORCEDISABLED, 1);
    unsafe {
        SetWindowLongW(window, GWL_STYLE, WS_POPUP.0 as i32);
        let extended = GetWindowLongW(window, GWL_EXSTYLE);
        SetWindowLongW(
            window,
            GWL_EXSTYLE,
            extended | (WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE).0 as i32,
        );
        let _ = SetLayeredWindowAttributes(window, COLORREF(0), 255, LWA_ALPHA);
        let _ = SetWindowPos(
            window,
            None,
            0,
            0,
            0,
            0,
            SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
    clear_backdrop(hwnd);
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
