use super::*;
use crate::win32::{
    WindowPlacement, cursor_screen_pos, primary_mouse_button_down, release_mouse_capture,
    top_visible_window_at,
};
use std::time::Instant;
use tabs::{CAPTION_WIDTH, STRIP_LEFT, STRIP_TOP, TAB_HEIGHT, window_listeners};

const DRAG_THRESHOLD: f32 = 10.;
const DETACH_MAGNETISM: f32 = 15.;
const MOVE_POLL: Duration = Duration::from_millis(10);
const MOVE_LOOP_START_TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Clone, Copy)]
pub(crate) struct Owner {
    window: WindowHandle<Jot>,
    hwnd: WindowHwnd,
}

#[derive(Clone, Copy)]
enum Phase {
    Pressed,
    Dragging,
    Entering { since: Instant },
    Moving { attaching: bool },
}

#[derive(Clone)]
pub(crate) struct TabDrag {
    owner: Owner,
    source: Owner,
    tab: EntityId,
    grab: Point<f32>,
    start: f32,
    press: ScreenPoint,
    phase: Phase,
    source_index: usize,
    source_placement: Option<WindowPlacement>,
}

pub(crate) fn owner(window: &Window) -> Option<Owner> {
    Some(Owner {
        window: window.window_handle().downcast::<Jot>()?,
        hwnd: window.hwnd(),
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
    if alone && !owner.hwnd.is_maximized() {
        cx.defer(lift);
    }
    cx.shared.tab_drag = Some(TabDrag {
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
    cx.refresh_windows();
}

pub(crate) fn active(cx: &App) -> bool {
    cx.shared.tab_drag.is_some()
}

pub(crate) fn dragging(cx: &App) -> bool {
    cx.shared
        .tab_drag
        .as_ref()
        .is_some_and(|drag| matches!(drag.phase, Phase::Dragging))
}

pub(crate) fn forget(tab: EntityId, cx: &mut App) {
    if cx
        .shared
        .tab_drag
        .as_ref()
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
    let drag = cx.shared.tab_drag.take();
    cx.refresh_windows();
    drag
}

fn moved(cx: &mut App) {
    let Some(mut drag) = cx.shared.tab_drag.clone() else {
        return;
    };
    let point = cursor_screen_pos();
    match drag.phase {
        Phase::Pressed => {
            let distance = ((point.x - drag.press.x) as f32).hypot((point.y - drag.press.y) as f32);
            if distance / drag.owner.hwnd.dpi_scale() <= DRAG_THRESHOLD {
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
    } else if !alone && let Some(target) = strip_window(point, None, cx) {
        attach(&mut drag, target, point, cx);
    } else {
        detach(&mut drag, alone, point, cx);
    }
    cx.shared.tab_drag = Some(drag);
    cx.refresh_windows();
}

fn lift(cx: &mut App) {
    let Some(mut drag) = cx.shared.tab_drag.clone() else {
        return;
    };
    let point = drag.press;
    drag.source_placement = drag.owner.hwnd.saved_placement();
    start_move_loop(&mut drag, point, cx);
    cx.shared.tab_drag = Some(drag);
    cx.refresh_windows();
}

fn released(cx: &mut App) {
    if cx
        .shared
        .tab_drag
        .as_ref()
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
            drag.owner.hwnd.set_foreground();
        }
    }
}

pub(crate) fn cancel(cx: &mut App) {
    let Some(drag) = end(cx) else {
        return;
    };
    release_mouse_capture();
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
        } else if let Some(placement) = drag.source_placement {
            drag.source.hwnd.restore_placement(placement);
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
        drag.source.hwnd.set_foreground();
    } else {
        let (_, jot) = open_jot_window(Some(data), None, cx);
        if let Some(placement) = drag.source_placement {
            jot.read(cx).hwnd.restore_placement(placement);
        }
    }
}

fn attach(drag: &mut TabDrag, target: Owner, point: ScreenPoint, cx: &mut App) {
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
    target.hwnd.set_capture();
    drag.owner = target;
    drag.phase = Phase::Dragging;
}

fn detach(drag: &mut TabDrag, alone: bool, point: ScreenPoint, cx: &mut App) {
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
            drag.source_placement = hwnd.saved_placement();
        }
        if hwnd.is_maximized() {
            hwnd.restore();
        }
        with_jot(drag.owner, cx, |jot, _, cx| jot.release_tab(cx));
    }
    run_move_loop(drag, point, cx);
}

fn run_move_loop(drag: &mut TabDrag, point: ScreenPoint, cx: &mut App) {
    place(
        drag.owner.hwnd,
        point,
        self::point(STRIP_LEFT + drag.grab.x, STRIP_TOP + drag.grab.y),
    );
    start_move_loop(drag, point, cx);
}

fn start_move_loop(drag: &mut TabDrag, point: ScreenPoint, cx: &mut App) {
    drag.owner.hwnd.begin_system_move(point);
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
    let Some(mut drag) = cx.shared.tab_drag.clone() else {
        return false;
    };
    let attaching = match drag.phase {
        Phase::Entering { .. } => false,
        Phase::Moving { attaching } => attaching,
        Phase::Pressed | Phase::Dragging => return false,
    };
    let hwnd = drag.owner.hwnd;
    let point = cursor_screen_pos();
    if hwnd.in_move_loop() {
        let attaching = attaching || strip_window(point, Some(hwnd), cx).is_some();
        if attaching && !matches!(drag.phase, Phase::Moving { attaching: true }) {
            hwnd.cancel_mode();
        }
        drag.phase = Phase::Moving { attaching };
        cx.shared.tab_drag = Some(drag);
        return true;
    }
    if let Phase::Entering { since } = drag.phase
        && since.elapsed() < MOVE_LOOP_START_TIMEOUT
    {
        return true;
    }
    let pressed = primary_mouse_button_down();
    if let Some(target) = strip_window(point, Some(hwnd), cx) {
        attach(&mut drag, target, point, cx);
        cx.shared.tab_drag = Some(drag);
        cx.refresh_windows();
        if !pressed {
            released(cx);
        }
    } else if pressed && attaching {
        run_move_loop(&mut drag, point, cx);
        cx.shared.tab_drag = Some(drag);
        cx.refresh_windows();
    } else if pressed {
        cx.shared.tab_drag = Some(drag);
        cancel(cx);
    } else {
        end(cx);
        hwnd.set_foreground();
    }
    false
}

fn strip_window(point: ScreenPoint, exclude: Option<WindowHwnd>, cx: &App) -> Option<Owner> {
    let hwnd = top_visible_window_at(point, exclude)?;
    if !in_strip(hwnd, point) {
        return None;
    }
    jot_windows(cx).find(|owner| owner.hwnd == hwnd)
}

fn local(hwnd: WindowHwnd, point: ScreenPoint) -> Point<f32> {
    let origin = hwnd.client_origin();
    let scale = hwnd.dpi_scale();
    self::point(
        (point.x - origin.x) as f32 / scale,
        (point.y - origin.y) as f32 / scale,
    )
}

fn screen(hwnd: WindowHwnd, position: Point<Pixels>) -> ScreenPoint {
    let origin = hwnd.client_origin();
    let scale = hwnd.dpi_scale();
    ScreenPoint {
        x: origin.x + (position.x.as_f32() * scale).round() as i32,
        y: origin.y + (position.y.as_f32() * scale).round() as i32,
    }
}

fn in_strip(hwnd: WindowHwnd, point: ScreenPoint) -> bool {
    let client = hwnd.client_rect();
    let width = client.right as f32 / hwnd.dpi_scale();
    let position = local(hwnd, point);
    (STRIP_LEFT..width - CAPTION_WIDTH).contains(&position.x)
        && (STRIP_TOP - DETACH_MAGNETISM..STRIP_TOP + TAB_HEIGHT + DETACH_MAGNETISM)
            .contains(&position.y)
}

fn place(hwnd: WindowHwnd, point: ScreenPoint, offset: Point<f32>) {
    let scale = hwnd.dpi_scale();
    let origin = hwnd.client_origin();
    let frame = hwnd.window_rect();
    let x = point.x - (offset.x * scale).round() as i32 - (origin.x - frame.left);
    let y = point.y - (offset.y * scale).round() as i32 - (origin.y - frame.top);
    hwnd.move_noactivate(x, y);
}
