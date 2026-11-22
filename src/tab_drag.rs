use super::*;
use std::time::Instant;
use tabs::{CAPTION_WIDTH, STRIP_LEFT, STRIP_TOP, TAB_HEIGHT, window_listeners};
use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::PtInRect;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, ReleaseCapture, SetCapture, VK_LBUTTON, VK_RBUTTON,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GUI_INMOVESIZE, GUITHREADINFO, GWL_EXSTYLE, GetClientRect, GetGUIThreadInfo,
    GetSystemMetrics, GetWindowLongW, GetWindowPlacement, GetWindowRect, GetWindowThreadProcessId,
    HTCAPTION, IsIconic, IsWindowVisible, IsZoomed, PostMessageW, SC_MOVE, SM_SWAPBUTTON,
    SW_RESTORE, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, SetForegroundWindow, SetWindowPlacement,
    SetWindowPos, ShowWindow, WINDOWPLACEMENT, WM_CANCELMODE, WM_SYSCOMMAND, WS_EX_TRANSPARENT,
};
use windows::core::BOOL;

const DRAG_THRESHOLD: f32 = 10.;
const DETACH_MAGNETISM: f32 = 15.;
const MOVE_POLL: Duration = Duration::from_millis(10);
const MOVE_LOOP_START_TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Clone, Copy)]
pub(crate) struct Owner {
    window: WindowHandle<Jot>,
    hwnd: isize,
}

#[derive(Clone, Copy)]
enum Phase {
    Pressed,
    Dragging,
    Entering { since: Instant },
    Moving { attaching: bool },
}

#[derive(Clone)]
struct TabDrag {
    owner: Owner,
    source: Owner,
    tab: EntityId,
    grab: Point<f32>,
    start: f32,
    press: POINT,
    phase: Phase,
    source_index: usize,
    source_placement: Option<WINDOWPLACEMENT>,
}

impl Global for TabDrag {}

pub(crate) fn owner(window: &Window) -> Option<Owner> {
    Some(Owner {
        window: window.window_handle().downcast::<Jot>()?,
        hwnd: window_hwnd(window),
    })
}

fn with_jot<R>(
    owner: Owner,
    cx: &mut App,
    f: impl FnOnce(&mut Jot, &mut Window, &mut Context<Jot>) -> R,
) -> Option<R> {
    owner.window.update(cx, |jot, window, cx| f(jot, window, cx)).ok()
}

fn jot_windows(cx: &App) -> impl Iterator<Item = Owner> + '_ {
    cx.windows().into_iter().filter_map(move |handle| {
        let window = handle.downcast::<Jot>()?;
        let hwnd = window.read(cx).ok()?.hwnd;
        Some(Owner { window, hwnd })
    })
}

pub(crate) fn press(
    owner: Owner,
    tab: EntityId,
    index: usize,
    grab: Point<f32>,
    position: Point<Pixels>,
    alone: bool,
    cx: &mut App,
) {
    if alone && !unsafe { IsZoomed(HWND(owner.hwnd as _)) }.as_bool() {
        cx.defer(lift);
    }
    cx.set_global(TabDrag {
        owner,
        source: owner,
        tab,
        grab,
        start: position.x.as_f32(),
        press: screen(owner.hwnd, position),
        phase: Phase::Pressed,
        source_index: index,
        source_placement: None,
    });
}

pub(crate) fn active(cx: &App) -> bool {
    cx.has_global::<TabDrag>()
}

pub(crate) fn dragging(cx: &App) -> bool {
    cx.try_global::<TabDrag>()
        .is_some_and(|drag| matches!(drag.phase, Phase::Dragging))
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

fn moved(cx: &mut App) {
    let Some(mut drag) = cx.try_global::<TabDrag>().cloned() else {
        return;
    };
    let point = cursor();
    match drag.phase {
        Phase::Pressed => {
            let distance = ((point.x - drag.press.x) as f32).hypot((point.y - drag.press.y) as f32);
            if distance / scale(drag.owner.hwnd) <= DRAG_THRESHOLD {
                return;
            }
            drag.phase = Phase::Dragging;
        }
        Phase::Dragging => {}
        Phase::Entering { .. } | Phase::Moving { .. } => return,
    }
    let alone = with_jot(drag.owner, cx, |jot, _, _| jot.tabs.len() == 1).unwrap_or(true);
    if !alone && in_strip(drag.owner.hwnd, point) {
        let x = local(drag.owner.hwnd, point).x;
        let (tab, grab, start) = (drag.tab, drag.grab.x, drag.start);
        with_jot(drag.owner, cx, |jot, _, cx| {
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
    drag.source_placement = placement(drag.owner.hwnd);
    start_move_loop(&mut drag, point, cx);
    cx.set_global(drag);
}

fn released(cx: &mut App) {
    if cx
        .try_global::<TabDrag>()
        .is_none_or(|drag| matches!(drag.phase, Phase::Entering { .. } | Phase::Moving { .. }))
    {
        return;
    }
    let Some(drag) = end(cx) else {
        return;
    };
    if let Phase::Dragging = drag.phase {
        with_jot(drag.owner, cx, |jot, _, cx| jot.release_tab(cx));
        if drag.owner.hwnd != drag.source.hwnd {
            foreground(drag.owner.hwnd);
        }
    }
}

pub(crate) fn cancel(cx: &mut App) {
    let Some(drag) = end(cx) else {
        return;
    };
    let _ = unsafe { ReleaseCapture() };
    if let Phase::Pressed = drag.phase {
        return;
    }
    let tab = drag.tab;
    let index = drag.source_index;
    if drag.owner.hwnd == drag.source.hwnd {
        if let Phase::Dragging = drag.phase {
            with_jot(drag.source, cx, |jot, window, cx| {
                jot.return_tab(tab, index, window, cx)
            });
        } else {
            restore_placement(drag.source.hwnd, drag.source_placement);
        }
        return;
    }
    let Some(data) = with_jot(drag.owner, cx, |jot, window, cx| {
        jot.take_tab(tab, window, cx)
    })
    .flatten() else {
        return;
    };
    if drag.source.window.read(cx).is_ok() {
        with_jot(drag.source, cx, |jot, window, cx| {
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
    let Some(data) = with_jot(drag.owner, cx, |jot, window, cx| {
        jot.take_tab(tab, window, cx)
    })
    .flatten() else {
        return;
    };
    let x = local(target.hwnd, point).x;
    let grab = drag.grab.x;
    with_jot(target, cx, |jot, window, cx| {
        jot.insert_dragged_tab(data, x, grab, window, cx)
    });
    unsafe { SetCapture(HWND(target.hwnd as _)) };
    drag.owner = target;
    drag.phase = Phase::Dragging;
}

fn detach(drag: &mut TabDrag, alone: bool, point: POINT, cx: &mut App) {
    let tab = drag.tab;
    if !alone {
        let Some((data, size)) = with_jot(drag.owner, cx, |jot, window, cx| {
            let size = window.window_bounds().get_bounds().size;
            jot.take_tab(tab, window, cx).map(|data| (data, size))
        })
        .flatten() else {
            return;
        };
        let (window, jot) = open_jot_window(Some(data), Some(size), cx);
        drag.owner = Owner {
            window,
            hwnd: jot.read(cx).hwnd,
        };
    } else {
        let hwnd = drag.owner.hwnd;
        if hwnd == drag.source.hwnd && drag.source_placement.is_none() {
            drag.source_placement = placement(hwnd);
        }
        if unsafe { IsZoomed(HWND(hwnd as _)) }.as_bool() {
            let _ = unsafe { ShowWindow(HWND(hwnd as _), SW_RESTORE) };
        }
        with_jot(drag.owner, cx, |jot, _, cx| jot.release_tab(cx));
    }
    run_move_loop(drag, point, cx);
}

fn run_move_loop(drag: &mut TabDrag, point: POINT, cx: &mut App) {
    place(
        drag.owner.hwnd,
        point,
        self::point(STRIP_LEFT + drag.grab.x, STRIP_TOP + drag.grab.y),
    );
    start_move_loop(drag, point, cx);
}

fn start_move_loop(drag: &mut TabDrag, point: POINT, cx: &mut App) {
    let hwnd = drag.owner.hwnd;
    let position = LPARAM((((point.y as u16 as u32) << 16) | point.x as u16 as u32) as isize);
    unsafe {
        let _ = ReleaseCapture();
        let _ = PostMessageW(
            Some(HWND(hwnd as _)),
            WM_SYSCOMMAND,
            WPARAM(SC_MOVE as usize | HTCAPTION as usize),
            position,
        );
    }
    drag.phase = Phase::Entering {
        since: Instant::now(),
    };
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
    let attaching = match drag.phase {
        Phase::Entering { .. } => false,
        Phase::Moving { attaching } => attaching,
        Phase::Pressed | Phase::Dragging => return false,
    };
    let hwnd = drag.owner.hwnd;
    let point = cursor();
    if in_move_loop(hwnd) {
        let attaching = attaching || strip_window(point, hwnd, cx).is_some();
        if attaching && !matches!(drag.phase, Phase::Moving { attaching: true }) {
            let _ = unsafe { PostMessageW(Some(HWND(hwnd as _)), WM_CANCELMODE, WPARAM(0), LPARAM(0)) };
        }
        drag.phase = Phase::Moving { attaching };
        cx.set_global(drag);
        return true;
    }
    if let Phase::Entering { since } = drag.phase
        && since.elapsed() < MOVE_LOOP_START_TIMEOUT
    {
        return true;
    }
    let pressed = button_down();
    if let Some(target) = strip_window(point, hwnd, cx) {
        attach(&mut drag, target, point, cx);
        cx.set_global(drag);
        if !pressed {
            released(cx);
        }
    } else if pressed && attaching {
        run_move_loop(&mut drag, point, cx);
        cx.set_global(drag);
    } else if pressed {
        cx.set_global(drag);
        cancel(cx);
    } else {
        end(cx);
        foreground(hwnd);
    }
    false
}

fn strip_window(point: POINT, exclude: isize, cx: &App) -> Option<Owner> {
    let hwnd = window_at(point, exclude);
    if hwnd == 0 || !in_strip(hwnd, point) {
        return None;
    }
    jot_windows(cx).find(|owner| owner.hwnd == hwnd)
}

fn button_down() -> bool {
    let button = if unsafe { GetSystemMetrics(SM_SWAPBUTTON) } != 0 {
        VK_RBUTTON
    } else {
        VK_LBUTTON
    };
    (unsafe { GetAsyncKeyState(button.0 as i32) }) < 0
}

fn scale(hwnd: isize) -> f32 {
    match unsafe { GetDpiForWindow(HWND(hwnd as _)) } {
        0 => 1.,
        dpi => dpi as f32 / USER_DEFAULT_SCREEN_DPI as f32,
    }
}

fn local(hwnd: isize, point: POINT) -> Point<f32> {
    let origin = client_origin(hwnd);
    let scale = scale(hwnd);
    self::point(
        (point.x - origin.x) as f32 / scale,
        (point.y - origin.y) as f32 / scale,
    )
}

fn screen(hwnd: isize, position: Point<Pixels>) -> POINT {
    let origin = client_origin(hwnd);
    let scale = scale(hwnd);
    POINT {
        x: origin.x + (position.x.as_f32() * scale).round() as i32,
        y: origin.y + (position.y.as_f32() * scale).round() as i32,
    }
}

fn in_strip(hwnd: isize, point: POINT) -> bool {
    let mut client = RECT::default();
    let _ = unsafe { GetClientRect(HWND(hwnd as _), &mut client) };
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
    let _ = unsafe { GetWindowRect(HWND(hwnd as _), &mut frame) };
    let x = point.x - (offset.x * scale).round() as i32 - (origin.x - frame.left);
    let y = point.y - (offset.y * scale).round() as i32 - (origin.y - frame.top);
    let _ = unsafe {
        SetWindowPos(
            HWND(hwnd as _),
            None,
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
    unsafe { GetWindowPlacement(HWND(hwnd as _), &mut placement) }
        .is_ok()
        .then_some(placement)
}

fn restore_placement(hwnd: isize, placement: Option<WINDOWPLACEMENT>) {
    if let Some(placement) = placement {
        let _ = unsafe { SetWindowPlacement(HWND(hwnd as _), &placement) };
    }
}

fn foreground(hwnd: isize) {
    let _ = unsafe { SetForegroundWindow(HWND(hwnd as _)) };
}

fn in_move_loop(hwnd: isize) -> bool {
    let mut info = GUITHREADINFO {
        cbSize: size_of::<GUITHREADINFO>() as u32,
        ..Default::default()
    };
    unsafe {
        let thread = GetWindowThreadProcessId(HWND(hwnd as _), None);
        GetGUIThreadInfo(thread, &mut info).is_ok() && (info.flags & GUI_INMOVESIZE).0 != 0
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
        IsWindowVisible(hwnd).as_bool()
            && !IsIconic(hwnd).as_bool()
            && GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TRANSPARENT.0 == 0
            && (DwmGetWindowAttribute(
                hwnd,
                DWMWA_CLOAKED,
                (&raw mut cloaked).cast(),
                size_of::<u32>() as u32,
            )
            .is_err()
                || cloaked == 0)
    }
}

fn frame(hwnd: HWND) -> RECT {
    let mut frame = RECT::default();
    let status = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            (&raw mut frame).cast(),
            size_of::<RECT>() as u32,
        )
    };
    if status.is_err() {
        let _ = unsafe { GetWindowRect(hwnd, &mut frame) };
    }
    frame
}

unsafe extern "system" fn visit(hwnd: HWND, data: LPARAM) -> BOOL {
    let search = unsafe { &mut *(data.0 as *mut Search) };
    if hwnd.0 as isize == search.exclude || !shown(hwnd) {
        return BOOL(1);
    }
    let frame = frame(hwnd);
    if unsafe { PtInRect(&frame, search.point) }.as_bool() {
        search.found = hwnd.0 as isize;
        return BOOL(0);
    }
    BOOL(1)
}

fn window_at(point: POINT, exclude: isize) -> isize {
    let mut search = Search {
        point,
        exclude,
        found: 0,
    };
    let _ = unsafe { EnumWindows(Some(visit), LPARAM((&raw mut search) as isize)) };
    search.found
}
