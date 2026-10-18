use super::*;
use tabs::{CAPTION_WIDTH, STRIP_LEFT, STRIP_TOP, TAB_HEIGHT, window_listeners};
use windows_sys::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
use windows_sys::Win32::Graphics::Dwm::{
    DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute,
};
use windows_sys::Win32::Graphics::Gdi::PtInRect;
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, ReleaseCapture, SetCapture, VK_LBUTTON, VK_RBUTTON,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GUI_INMOVESIZE, GUITHREADINFO, GWL_EXSTYLE, GetClientRect, GetGUIThreadInfo,
    GetSystemMetrics, GetWindowLongW, GetWindowPlacement, GetWindowRect, GetWindowThreadProcessId,
    HTCAPTION, IsIconic, IsWindowVisible, IsZoomed, PostMessageW, SC_MOVE, SM_SWAPBUTTON,
    SW_RESTORE, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, SetForegroundWindow, SetWindowPlacement,
    SetWindowPos, ShowWindow, WINDOWPLACEMENT, WM_CANCELMODE, WM_SYSCOMMAND, WS_EX_TRANSPARENT,
};
use windows_sys::core::BOOL;

const DRAG_THRESHOLD: f32 = 10.;
const DETACH_MAGNETISM: f32 = 15.;
const MOVE_POLL: Duration = Duration::from_millis(10);
const MOVE_LOOP_WAIT_POLLS: u32 = 50;

#[derive(Clone)]
pub(crate) struct Owner {
    pub handle: AnyWindowHandle,
    pub jot: WeakEntity<Jot>,
    pub hwnd: isize,
}

#[derive(Default)]
struct JotWindows(Vec<Owner>);

impl Global for JotWindows {}

#[derive(Clone, Copy, Default)]
struct Moving {
    entered: bool,
    attaching: bool,
    waited: u32,
}

#[derive(Clone)]
struct TabDrag {
    owner: Owner,
    tab: EntityId,
    grab: Point<f32>,
    start: f32,
    press: POINT,
    started: bool,
    moving: Option<Moving>,
    source: Owner,
    source_index: usize,
    source_placement: Option<WINDOWPLACEMENT>,
}

impl Global for TabDrag {}

impl Jot {
    pub(crate) fn owner(&self, window: &Window, cx: &Context<Self>) -> Owner {
        Owner {
            handle: window.window_handle(),
            jot: cx.weak_entity(),
            hwnd: self.hwnd,
        }
    }
}

pub(crate) fn register(owner: Owner, cx: &mut App) {
    let windows = &mut cx.default_global::<JotWindows>().0;
    windows.retain(|window| window.jot.upgrade().is_some());
    windows.push(owner);
}

pub(crate) fn press(
    owner: Owner,
    tab: EntityId,
    index: usize,
    grab: Point<f32>,
    start: f32,
    alone: bool,
    cx: &mut App,
) {
    if alone && unsafe { IsZoomed(owner.hwnd as HWND) } == 0 {
        cx.defer(lift);
    }
    cx.set_global(TabDrag {
        source: owner.clone(),
        owner,
        tab,
        grab,
        start,
        press: cursor(),
        started: false,
        moving: None,
        source_index: index,
        source_placement: None,
    });
}

pub(crate) fn active(cx: &App) -> bool {
    cx.has_global::<TabDrag>()
}

pub(crate) fn dragging(cx: &App) -> bool {
    cx.try_global::<TabDrag>()
        .is_some_and(|drag| drag.started && drag.moving.is_none())
}

pub(crate) fn forget(tab: EntityId, cx: &mut App) {
    if cx
        .try_global::<TabDrag>()
        .is_some_and(|drag| drag.tab == tab)
    {
        end(cx);
    }
}

pub(crate) fn listeners() -> impl IntoElement {
    window_listeners(|window| {
        window.on_mouse_event(|event: &MouseMoveEvent, phase, _, cx| {
            if phase != DispatchPhase::Bubble {
                return;
            }
            if event.pressed_button == Some(MouseButton::Left) {
                cx.defer(moved);
            } else {
                cx.defer(released);
            }
        });
        window.on_mouse_event(|event: &MouseUpEvent, phase, _, cx| {
            if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
                cx.defer(released);
            }
        });
    })
}

fn end(cx: &mut App) -> Option<TabDrag> {
    let drag = cx
        .has_global::<TabDrag>()
        .then(|| cx.remove_global::<TabDrag>());
    cx.refresh_windows();
    drag
}

fn with_jot<R>(
    owner: &Owner,
    cx: &mut App,
    f: impl FnOnce(&mut Jot, &mut Window, &mut Context<Jot>) -> R,
) -> Option<R> {
    let jot = owner.jot.upgrade()?;
    owner
        .handle
        .update(cx, |_, window, cx| {
            jot.update(cx, |jot, cx| f(jot, window, cx))
        })
        .ok()
}

fn moved(cx: &mut App) {
    let Some(mut drag) = cx.try_global::<TabDrag>().cloned() else {
        return;
    };
    if drag.moving.is_some() {
        return;
    }
    let point = cursor();
    if !drag.started {
        let distance = ((point.x - drag.press.x) as f32).hypot((point.y - drag.press.y) as f32);
        if distance / scale(drag.owner.hwnd) <= DRAG_THRESHOLD {
            return;
        }
        drag.started = true;
    }
    let alone = with_jot(&drag.owner, cx, |jot, _, _| jot.tabs.len() == 1).unwrap_or(true);
    if !alone && in_strip(drag.owner.hwnd, point) {
        let x = local(drag.owner.hwnd, point).x;
        let (tab, grab, start) = (drag.tab, drag.grab.x, drag.start);
        with_jot(&drag.owner, cx, |jot, _, cx| {
            jot.drag_tab(tab, x, grab, start, cx)
        });
    } else if !alone && let Some(target) = strip_window(point, 0, cx) {
        attach(&mut drag, target, point, cx);
    } else {
        detach(&mut drag, alone, point, cx);
    }
    cx.set_global(drag);
}

fn lift(cx: &mut App) {
    let Some(mut drag) = cx.try_global::<TabDrag>().cloned() else {
        return;
    };
    let point = drag.press;
    drag.started = true;
    drag.source_placement = placement(drag.owner.hwnd);
    start_move_loop(&mut drag, point, cx);
    cx.set_global(drag);
}

fn released(cx: &mut App) {
    if cx
        .try_global::<TabDrag>()
        .is_none_or(|drag| drag.moving.is_some())
    {
        return;
    }
    let Some(drag) = end(cx) else {
        return;
    };
    if drag.started {
        with_jot(&drag.owner, cx, |jot, _, cx| jot.release_tab(cx));
        if drag.owner.hwnd != drag.source.hwnd {
            foreground(drag.owner.hwnd);
        }
    }
}

pub(crate) fn cancel(cx: &mut App) {
    let Some(drag) = end(cx) else {
        return;
    };
    unsafe { ReleaseCapture() };
    if !drag.started {
        return;
    }
    let tab = drag.tab;
    let index = drag.source_index;
    if drag.owner.hwnd == drag.source.hwnd {
        if drag.moving.is_some() {
            restore_placement(drag.source.hwnd, drag.source_placement);
        } else {
            with_jot(&drag.source, cx, |jot, window, cx| {
                jot.return_tab(tab, index, window, cx)
            });
        }
        return;
    }
    let Some(data) = with_jot(&drag.owner, cx, |jot, window, cx| {
        jot.take_tab(tab, window, cx)
    })
    .flatten() else {
        return;
    };
    if drag.source.jot.upgrade().is_some() {
        with_jot(&drag.source, cx, |jot, window, cx| {
            jot.restore_tab(data, index, window, cx)
        });
        foreground(drag.source.hwnd);
    } else {
        let (_, jot) = open_jot_window(Some(data), None, cx);
        restore_placement(jot.read(cx).hwnd, drag.source_placement);
    }
}

fn attach(drag: &mut TabDrag, target: Owner, point: POINT, cx: &mut App) {
    let tab = drag.tab;
    let Some(data) = with_jot(&drag.owner, cx, |jot, window, cx| {
        jot.take_tab(tab, window, cx)
    })
    .flatten() else {
        return;
    };
    let x = local(target.hwnd, point).x;
    let grab = drag.grab.x;
    with_jot(&target, cx, |jot, window, cx| {
        jot.insert_dragged_tab(data, x, grab, window, cx)
    });
    unsafe { SetCapture(target.hwnd as HWND) };
    drag.owner = target;
    drag.moving = None;
}

fn detach(drag: &mut TabDrag, alone: bool, point: POINT, cx: &mut App) {
    let tab = drag.tab;
    if !alone {
        let Some((data, size)) = with_jot(&drag.owner, cx, |jot, window, cx| {
            let size = window.window_bounds().get_bounds().size;
            jot.take_tab(tab, window, cx).map(|data| (data, size))
        })
        .flatten() else {
            return;
        };
        let (handle, jot) = open_jot_window(Some(data), Some(size), cx);
        drag.owner = Owner {
            handle,
            hwnd: jot.read(cx).hwnd,
            jot: jot.downgrade(),
        };
    } else {
        let hwnd = drag.owner.hwnd;
        if hwnd == drag.source.hwnd && drag.source_placement.is_none() {
            drag.source_placement = placement(hwnd);
        }
        if unsafe { IsZoomed(hwnd as HWND) } != 0 {
            unsafe { ShowWindow(hwnd as HWND, SW_RESTORE) };
        }
        with_jot(&drag.owner, cx, |jot, _, cx| jot.release_tab(cx));
    }
    run_move_loop(drag, point, cx);
}

fn run_move_loop(drag: &mut TabDrag, point: POINT, cx: &mut App) {
    place(
        drag.owner.hwnd,
        point,
        gpui_kit::point(STRIP_LEFT + drag.grab.x, STRIP_TOP + drag.grab.y),
    );
    start_move_loop(drag, point, cx);
}

fn start_move_loop(drag: &mut TabDrag, point: POINT, cx: &mut App) {
    let hwnd = drag.owner.hwnd;
    let position = (((point.y as u16 as u32) << 16) | point.x as u16 as u32) as LPARAM;
    unsafe {
        ReleaseCapture();
        PostMessageW(
            hwnd as HWND,
            WM_SYSCOMMAND,
            SC_MOVE as usize | HTCAPTION as usize,
            position,
        );
    }
    drag.moving = Some(Moving::default());
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(MOVE_POLL).await;
            if !cx.update(poll) {
                break;
            }
        }
    })
    .detach();
}

fn poll(cx: &mut App) -> bool {
    let Some(mut drag) = cx.try_global::<TabDrag>().cloned() else {
        return false;
    };
    let Some(mut moving) = drag.moving else {
        return false;
    };
    let hwnd = drag.owner.hwnd;
    let point = cursor();
    if in_move_loop(hwnd) {
        moving.entered = true;
        if !moving.attaching && strip_window(point, hwnd, cx).is_some() {
            unsafe { PostMessageW(hwnd as HWND, WM_CANCELMODE, 0, 0) };
            moving.attaching = true;
        }
    } else if !moving.entered && moving.waited < MOVE_LOOP_WAIT_POLLS {
        moving.waited += 1;
    } else {
        let pressed = button_down();
        if let Some(target) = strip_window(point, hwnd, cx) {
            attach(&mut drag, target, point, cx);
            cx.set_global(drag);
            if !pressed {
                released(cx);
            }
        } else if pressed && moving.attaching {
            run_move_loop(&mut drag, point, cx);
            cx.set_global(drag);
        } else if pressed {
            cx.set_global(drag);
            cancel(cx);
        } else {
            end(cx);
            foreground(hwnd);
        }
        return false;
    }
    drag.moving = Some(moving);
    cx.set_global(drag);
    true
}

fn strip_window(point: POINT, exclude: isize, cx: &App) -> Option<Owner> {
    let hwnd = window_at(point, exclude);
    if hwnd == 0 || !in_strip(hwnd, point) {
        return None;
    }
    cx.try_global::<JotWindows>()?
        .0
        .iter()
        .find(|owner| owner.hwnd == hwnd && owner.jot.upgrade().is_some())
        .cloned()
}

fn button_down() -> bool {
    let button = if unsafe { GetSystemMetrics(SM_SWAPBUTTON) } != 0 {
        VK_RBUTTON
    } else {
        VK_LBUTTON
    };
    (unsafe { GetAsyncKeyState(button as i32) }) < 0
}

fn scale(hwnd: isize) -> f32 {
    match unsafe { GetDpiForWindow(hwnd as HWND) } {
        0 => 1.,
        dpi => dpi as f32 / USER_DEFAULT_SCREEN_DPI as f32,
    }
}

fn local(hwnd: isize, point: POINT) -> Point<f32> {
    let origin = client_origin(hwnd);
    let scale = scale(hwnd);
    gpui_kit::point(
        (point.x - origin.x) as f32 / scale,
        (point.y - origin.y) as f32 / scale,
    )
}

fn in_strip(hwnd: isize, point: POINT) -> bool {
    let mut client = RECT::default();
    unsafe { GetClientRect(hwnd as HWND, &mut client) };
    let width = client.right as f32 / scale(hwnd);
    let position = local(hwnd, point);
    (STRIP_LEFT..width - CAPTION_WIDTH).contains(&position.x)
        && (STRIP_TOP - DETACH_MAGNETISM..STRIP_TOP + TAB_HEIGHT + DETACH_MAGNETISM)
            .contains(&position.y)
}

fn place(hwnd: isize, point: POINT, offset: Point<f32>) {
    let scale = scale(hwnd);
    let origin = client_origin(hwnd);
    let mut frame = RECT::default();
    unsafe { GetWindowRect(hwnd as HWND, &mut frame) };
    let x = point.x - (offset.x * scale).round() as i32 - (origin.x - frame.left);
    let y = point.y - (offset.y * scale).round() as i32 - (origin.y - frame.top);
    unsafe {
        SetWindowPos(
            hwnd as HWND,
            std::ptr::null_mut(),
            x,
            y,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        )
    };
}

fn placement(hwnd: isize) -> Option<WINDOWPLACEMENT> {
    let mut placement = WINDOWPLACEMENT {
        length: size_of::<WINDOWPLACEMENT>() as u32,
        ..Default::default()
    };
    (unsafe { GetWindowPlacement(hwnd as HWND, &mut placement) } != 0).then_some(placement)
}

fn restore_placement(hwnd: isize, placement: Option<WINDOWPLACEMENT>) {
    if let Some(placement) = placement {
        unsafe { SetWindowPlacement(hwnd as HWND, &placement) };
    }
}

fn foreground(hwnd: isize) {
    unsafe { SetForegroundWindow(hwnd as HWND) };
}

fn in_move_loop(hwnd: isize) -> bool {
    let mut info: GUITHREADINFO = unsafe { std::mem::zeroed() };
    info.cbSize = size_of::<GUITHREADINFO>() as u32;
    unsafe {
        let thread = GetWindowThreadProcessId(hwnd as HWND, std::ptr::null_mut());
        GetGUIThreadInfo(thread, &mut info) != 0 && info.flags & GUI_INMOVESIZE != 0
    }
}

struct Search {
    point: POINT,
    exclude: isize,
    found: isize,
}

fn shown(hwnd: HWND) -> bool {
    let mut cloaked = 0u32;
    unsafe {
        IsWindowVisible(hwnd) != 0
            && IsIconic(hwnd) == 0
            && GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TRANSPARENT == 0
            && (DwmGetWindowAttribute(
                hwnd,
                DWMWA_CLOAKED as _,
                (&raw mut cloaked).cast(),
                size_of::<u32>() as u32,
            ) != 0
                || cloaked == 0)
    }
}

fn frame(hwnd: HWND) -> RECT {
    let mut frame = RECT::default();
    let status = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS as _,
            (&raw mut frame).cast(),
            size_of::<RECT>() as u32,
        )
    };
    if status != 0 {
        unsafe { GetWindowRect(hwnd, &mut frame) };
    }
    frame
}

unsafe extern "system" fn visit(hwnd: HWND, data: LPARAM) -> BOOL {
    let search = unsafe { &mut *(data as *mut Search) };
    if hwnd as isize == search.exclude || !shown(hwnd) {
        return 1;
    }
    let frame = frame(hwnd);
    if unsafe { PtInRect(&frame, search.point) } != 0 {
        search.found = hwnd as isize;
        return 0;
    }
    1
}

fn window_at(point: POINT, exclude: isize) -> isize {
    let mut search = Search {
        point,
        exclude,
        found: 0,
    };
    unsafe { EnumWindows(Some(visit), (&raw mut search) as LPARAM) };
    search.found
}
