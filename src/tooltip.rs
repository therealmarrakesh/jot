use super::*;
use std::collections::HashMap;
use std::time::Instant;
use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Dwm::{
    DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE, DWMWA_TRANSITIONS_FORCEDISABLED,
    DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND, DwmSetWindowAttribute,
};
use windows_sys::Win32::Graphics::Gdi::{
    ClientToScreen, GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows_sys::Win32::UI::Controls::{
    CloseThemeData, GetThemeAnimationProperty, GetThemeAnimationTransform, GetThemeTimingFunction,
    OpenThemeData, TA_CUBIC_BEZIER, TA_TRANSFORM_OPACITY, TAP_TRANSFORMCOUNT, TATT_OPACITY,
    TTFT_CUBIC_BEZIER,
};
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GWL_STYLE, GetCursorPos, HWND_TOPMOST, SPI_GETMOUSEHOVERTIME, SW_HIDE, SW_SHOWNOACTIVATE,
    SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetWindowLongW,
    SetWindowPos, ShowWindow, SystemParametersInfoW, WS_POPUP,
};

const PADDING_X: f32 = 9.;
const PADDING_TOP: f32 = 6.;
const PADDING_BOTTOM: f32 = 8.;
const BORDER: f32 = 1.;
const MAX_WIDTH: f32 = 320.;
const FONT_SIZE: f32 = 12.;
const LINE_HEIGHT: f32 = 16.;
const RADIUS: f32 = 4.;
const ELEVATION: f32 = 8.;
const SHADOW_SIDE: f32 = ELEVATION;
const SHADOW_TOP: f32 = ELEVATION / 2.;
const SHADOW_BOTTOM: f32 = ELEVATION * 1.5;
const POINTER_OFFSET: f32 = 20.;
const DEFAULT_HOVER_TIME: u32 = 400;
const RESHOW_WINDOW: Duration = Duration::from_millis(200);
const SAFE_ZONE_CHECK: Duration = Duration::from_secs(1);
const FADE_IN_STORYBOARD: i32 = 4;
const FADE_OUT_STORYBOARD: i32 = 5;
const FADE_TARGET: i32 = 1;
const DEFAULT_FADE: Duration = Duration::from_millis(167);

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
    let module: Vec<u16> = "user32.dll".encode_utf16().chain(Some(0)).collect();
    let Some(address) = (unsafe {
        GetProcAddress(
            GetModuleHandleW(module.as_ptr()),
            c"SetWindowCompositionAttribute".as_ptr().cast(),
        )
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
        data: (&mut policy as *mut AccentPolicy).cast(),
        size: size_of::<AccentPolicy>(),
    };
    unsafe { set(hwnd as HWND, &mut data) };
}

#[derive(Clone, Copy)]
struct Fade {
    delay: Duration,
    duration: Duration,
    curve: [f32; 4],
}

impl Fade {
    const LINEAR: Self = Self {
        delay: Duration::ZERO,
        duration: DEFAULT_FADE,
        curve: [0., 0., 1., 1.],
    };

    fn total(&self) -> Duration {
        self.delay + self.duration
    }

    fn progress(&self, since: Instant, now: Instant) -> f32 {
        let elapsed = now.saturating_duration_since(since);
        if elapsed <= self.delay {
            return 0.;
        }
        if self.duration.is_zero() {
            return 1.;
        }
        let t = ((elapsed - self.delay).as_secs_f32() / self.duration.as_secs_f32()).min(1.);
        let [x1, y1, x2, y2] = self.curve;
        cubic_bezier(x1, y1, x2, y2)(t)
    }
}

fn theme_fade(storyboard: i32) -> Fade {
    let class: Vec<u16> = "Animations".encode_utf16().chain(Some(0)).collect();
    let timing_class: Vec<u16> = "timingfunction".encode_utf16().chain(Some(0)).collect();
    unsafe {
        let theme = OpenThemeData(std::ptr::null_mut(), class.as_ptr());
        let timing = OpenThemeData(std::ptr::null_mut(), timing_class.as_ptr());
        let mut fade = None;
        let mut count = 0u32;
        let mut written = 0u32;
        if GetThemeAnimationProperty(
            theme,
            storyboard,
            FADE_TARGET,
            TAP_TRANSFORMCOUNT,
            (&mut count as *mut u32).cast(),
            size_of::<u32>() as u32,
            &mut written,
        ) == 0
        {
            for index in 0..count {
                let mut transform: TA_TRANSFORM_OPACITY = std::mem::zeroed();
                if GetThemeAnimationTransform(
                    theme,
                    storyboard,
                    FADE_TARGET,
                    index,
                    (&mut transform as *mut TA_TRANSFORM_OPACITY).cast(),
                    size_of::<TA_TRANSFORM_OPACITY>() as u32,
                    &mut written,
                ) != 0
                    || transform.header.eTransformType != TATT_OPACITY
                {
                    continue;
                }
                let mut bezier: TA_CUBIC_BEZIER = std::mem::zeroed();
                let curve = if GetThemeTimingFunction(
                    timing,
                    transform.header.dwTimingFunctionId as i32,
                    (&mut bezier as *mut TA_CUBIC_BEZIER).cast(),
                    size_of::<TA_CUBIC_BEZIER>() as u32,
                    &mut written,
                ) == 0
                    && bezier.header.eTimingFunctionType == TTFT_CUBIC_BEZIER
                {
                    [bezier.rX0, bezier.rY0, bezier.rX1, bezier.rY1]
                } else {
                    Fade::LINEAR.curve
                };
                fade = Some(Fade {
                    delay: Duration::from_millis(transform.header.dwStartTime.into()),
                    duration: Duration::from_millis(transform.header.dwDurationTime.into()),
                    curve,
                });
                break;
            }
        }
        CloseThemeData(theme);
        CloseThemeData(timing);
        fade.unwrap_or(Fade::LINEAR)
    }
}

#[derive(Clone, Copy)]
enum Phase {
    In(Instant),
    Out(Instant, f32),
}

pub(crate) struct TooltipView {
    text: SharedString,
    size: gpui_kit::Size<Pixels>,
    phase: Phase,
    fade_in: Fade,
    fade_out: Fade,
}

impl TooltipView {
    fn opacity(&self, now: Instant) -> f32 {
        match self.phase {
            Phase::In(since) => self.fade_in.progress(since, now),
            Phase::Out(since, from) => from * (1. - self.fade_out.progress(since, now)),
        }
    }

    fn settled(&self, now: Instant) -> bool {
        match self.phase {
            Phase::In(since) => now.saturating_duration_since(since) >= self.fade_in.total(),
            Phase::Out(since, _) => now.saturating_duration_since(since) >= self.fade_out.total(),
        }
    }
}

impl Render for TooltipView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let now = Instant::now();
        if !self.settled(now) {
            window.request_animation_frame();
        }
        div()
            .size_full()
            .opacity(self.opacity(now))
            .on_mouse_move(|_, _, cx| pointer_moved(cx))
            .child(
                div()
                    .absolute()
                    .left(px(SHADOW_SIDE))
                    .top(px(SHADOW_TOP))
                    .w(self.size.width)
                    .h(self.size.height)
                    .rounded(px(RADIUS))
                    .border_1()
                    .border_color(rgba(p.tooltip_border))
                    .shadow(vec![BoxShadow {
                        color: hsla(0., 0., 0., p.shadow),
                        offset: point(px(0.), px(ELEVATION / 2.)),
                        blur_radius: px(ELEVATION),
                        spread_radius: px(0.),
                        inset: false,
                    }])
                    .child(
                        div()
                            .size_full()
                            .rounded(px(RADIUS - BORDER))
                            .bg(rgb(p.tooltip_fill))
                            .pl(px(PADDING_X))
                            .pr(px(PADDING_X))
                            .pt(px(PADDING_TOP))
                            .pb(px(PADDING_BOTTOM))
                            .font_family(UI_FONT)
                            .text_size(px(FONT_SIZE))
                            .line_height(px(LINE_HEIGHT))
                            .text_color(rgba(p.text_primary))
                            .child(self.text.clone()),
                    ),
            )
    }
}

struct Owner {
    id: ElementId,
    depth: u8,
    text: SharedString,
    entered: POINT,
}

struct Popup {
    view: Entity<TooltipView>,
    handle: AnyWindowHandle,
    hwnd: isize,
}

struct Visible {
    id: ElementId,
    owner: RECT,
    tip: RECT,
    checked: POINT,
}

#[derive(Default)]
struct Tooltips {
    popup: Option<Popup>,
    hovered: Vec<Owner>,
    current: Option<ElementId>,
    visible: Option<Visible>,
    anchors: HashMap<ElementId, RECT>,
    closed_at: Option<Instant>,
    control_only: bool,
    opening: Option<Task<()>>,
    hiding: Option<Task<()>>,
    watching: Option<Task<()>>,
}

impl Global for Tooltips {}

fn cursor() -> POINT {
    let mut point = POINT { x: 0, y: 0 };
    unsafe { GetCursorPos(&mut point) };
    point
}

pub(crate) fn hover(
    id: impl Into<ElementId>,
    depth: u8,
    text: impl Into<SharedString>,
    hovered: bool,
    cx: &mut App,
) {
    let id = id.into();
    let tips = cx.default_global::<Tooltips>();
    tips.hovered.retain(|owner| owner.id != id);
    if hovered {
        tips.hovered.push(Owner {
            id,
            depth,
            text: text.into(),
            entered: cursor(),
        });
    }
    let candidate = tips
        .hovered
        .iter()
        .rev()
        .max_by_key(|owner| owner.depth)
        .map(|owner| (owner.id.clone(), owner.text.clone(), owner.entered));
    if candidate.as_ref().map(|(id, _, _)| id) == tips.current.as_ref() {
        return;
    }
    tips.current = candidate.as_ref().map(|(id, _, _)| id.clone());
    tips.opening = None;
    let Some((id, text, entered)) = candidate else {
        return;
    };
    if tips
        .visible
        .as_ref()
        .is_some_and(|visible| visible.id == id)
    {
        return;
    }
    close(cx);
    let tips = cx.default_global::<Tooltips>();
    let reshow = tips
        .closed_at
        .is_some_and(|closed| closed.elapsed() < RESHOW_WINDOW);
    let delay = show_delay(reshow);
    let task = cx.spawn(async move |cx| {
        cx.background_executor().timer(delay).await;
        cx.update(|cx| {
            if cx.default_global::<Tooltips>().current.as_ref() == Some(&id) {
                show(id, text, entered, cx);
            }
        });
    });
    cx.default_global::<Tooltips>().opening = Some(task);
}

pub(crate) fn anchor(id: impl Into<ElementId>) -> impl IntoElement {
    let id = id.into();
    canvas(
        |_, _, _| {},
        move |bounds, _, window, cx| {
            let tips = cx.default_global::<Tooltips>();
            let tracked = tips.hovered.iter().any(|owner| owner.id == id)
                || tips
                    .visible
                    .as_ref()
                    .is_some_and(|visible| visible.id == id);
            if !tracked {
                return;
            }
            let scale = window.scale_factor();
            let mut origin = POINT { x: 0, y: 0 };
            unsafe { ClientToScreen(detach::hwnd(window) as HWND, &mut origin) };
            let physical = |v: Pixels| (v.as_f32() * scale).round() as i32;
            let rect = RECT {
                left: origin.x + physical(bounds.origin.x),
                top: origin.y + physical(bounds.origin.y),
                right: origin.x + physical(bounds.origin.x + bounds.size.width),
                bottom: origin.y + physical(bounds.origin.y + bounds.size.height),
            };
            let moved = tips.visible.as_mut().is_some_and(|visible| {
                if visible.id != id {
                    return false;
                }
                let moved = visible.owner.left != rect.left
                    || visible.owner.top != rect.top
                    || visible.owner.right != rect.right
                    || visible.owner.bottom != rect.bottom;
                visible.owner = rect;
                moved
            });
            tips.anchors.insert(id.clone(), rect);
            if moved {
                cx.defer(close);
            }
        },
    )
    .absolute()
    .size_full()
}

pub(crate) fn pointer_moved(cx: &mut App) {
    let point = cursor();
    let Some(tips) = cx.try_global::<Tooltips>() else {
        return;
    };
    let Some(visible) = &tips.visible else {
        return;
    };
    if !in_safe_zone(point, visible.owner, visible.tip) {
        close(cx);
    }
}

pub(crate) fn key_down(cx: &mut App) {
    if let Some(tips) = cx.try_global::<Tooltips>()
        && tips.control_only
    {
        cx.default_global::<Tooltips>().control_only = false;
    }
}

pub(crate) fn modifiers_changed(modifiers: &Modifiers, cx: &mut App) {
    let only_control = modifiers.control
        && !modifiers.alt
        && !modifiers.shift
        && !modifiers.platform
        && !modifiers.function;
    let released = *modifiers == Modifiers::default();
    let tips = cx.default_global::<Tooltips>();
    if only_control {
        tips.control_only = true;
    } else if released && tips.control_only {
        tips.control_only = false;
        tips.current = None;
        tips.opening = None;
        close(cx);
    } else {
        tips.control_only = false;
    }
}

pub(crate) fn prepare(cx: &mut App) {
    popup(cx);
}

pub(crate) fn is_popup(handle: AnyWindowHandle, cx: &App) -> bool {
    cx.try_global::<Tooltips>()
        .and_then(|tips| tips.popup.as_ref())
        .is_some_and(|popup| popup.handle == handle)
}

fn show_delay(reshow: bool) -> Duration {
    let mut hover = 0u32;
    let ok = unsafe {
        SystemParametersInfoW(SPI_GETMOUSEHOVERTIME, 0, (&mut hover as *mut u32).cast(), 0)
    };
    let hover = if ok == 0 { DEFAULT_HOVER_TIME } else { hover } as f32;
    let factor = if reshow { 1.5 } else { 2. };
    Duration::from_millis((hover * factor) as u64)
}

fn cross(o: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0)
}

fn in_safe_zone(point: POINT, owner: RECT, tip: RECT) -> bool {
    let inside = |r: RECT| {
        point.x >= r.left && point.x <= r.right && point.y >= r.top && point.y <= r.bottom
    };
    if inside(owner) || inside(tip) {
        return true;
    }
    let mut points: Vec<(f64, f64)> = [owner, tip]
        .iter()
        .flat_map(|r| {
            [
                (r.left, r.top),
                (r.left, r.bottom),
                (r.right, r.bottom),
                (r.right, r.top),
            ]
        })
        .map(|(x, y)| (x as f64, y as f64))
        .collect();
    points.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mut hull: Vec<(f64, f64)> = Vec::new();
    for pass in 0..2 {
        let start = hull.len();
        let ordered: Vec<(f64, f64)> = if pass == 0 {
            points.clone()
        } else {
            points.iter().rev().copied().collect()
        };
        for p in ordered {
            while hull.len() >= start + 2
                && cross(hull[hull.len() - 2], hull[hull.len() - 1], p) <= 0.
            {
                hull.pop();
            }
            hull.push(p);
        }
        hull.pop();
    }
    let p = (point.x as f64, point.y as f64);
    (0..hull.len()).all(|i| cross(hull[i], hull[(i + 1) % hull.len()], p) >= 0.)
}

fn close(cx: &mut App) {
    let tips = cx.default_global::<Tooltips>();
    if tips.visible.take().is_none() {
        return;
    }
    tips.closed_at = Some(Instant::now());
    tips.watching = None;
    let Some(popup) = &tips.popup else {
        return;
    };
    let (view, hwnd) = (popup.view.clone(), popup.hwnd);
    let fade = view.update(cx, |view, cx| {
        let now = Instant::now();
        view.phase = Phase::Out(now, view.opacity(now));
        cx.notify();
        view.fade_out.total()
    });
    let task = cx.spawn(async move |cx| {
        cx.background_executor().timer(fade).await;
        cx.update(|cx| {
            let tips = cx.default_global::<Tooltips>();
            if tips.visible.is_none() {
                unsafe { ShowWindow(hwnd as HWND, SW_HIDE) };
            }
            tips.hiding = None;
        });
    });
    cx.default_global::<Tooltips>().hiding = Some(task);
}

fn popup(cx: &mut App) -> Option<(Entity<TooltipView>, AnyWindowHandle, isize)> {
    if let Some(popup) = &cx.default_global::<Tooltips>().popup {
        return Some((popup.view.clone(), popup.handle, popup.hwnd));
    }
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
            point(px(0.), px(0.)),
            size(px(1.), px(1.)),
        ))),
        titlebar: None,
        focus: false,
        show: false,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        inactive_frame_interval: None,
        window_background: WindowBackgroundAppearance::Transparent,
        ..Default::default()
    };
    let (fade_in, fade_out) = (
        theme_fade(FADE_IN_STORYBOARD),
        theme_fade(FADE_OUT_STORYBOARD),
    );
    let mut created = None;
    let handle = cx
        .open_window(options, |window, cx| {
            let view = cx.new(|_| TooltipView {
                text: "".into(),
                size: size(px(0.), px(0.)),
                phase: Phase::Out(Instant::now(), 0.),
                fade_in,
                fade_out,
            });
            created = Some((view.clone(), detach::hwnd(window)));
            view
        })
        .ok()?;
    let (view, hwnd) = created?;
    let window = hwnd as HWND;
    let set = |attribute: i32, value: u32| unsafe {
        DwmSetWindowAttribute(
            window,
            attribute as u32,
            (&value as *const u32).cast(),
            size_of::<u32>() as u32,
        )
    };
    set(DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND as u32);
    set(DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE);
    set(DWMWA_TRANSITIONS_FORCEDISABLED, 1);
    unsafe {
        SetWindowLongW(window, GWL_STYLE, WS_POPUP as i32);
        SetWindowPos(
            window,
            std::ptr::null_mut(),
            0,
            0,
            0,
            0,
            SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
    cx.default_global::<Tooltips>().popup = Some(Popup {
        view: view.clone(),
        handle: handle.into(),
        hwnd,
    });
    Some((view, handle.into(), hwnd))
}

fn measure(text: &SharedString, window: &mut Window) -> (f32, f32) {
    let runs = [TextRun {
        len: text.len(),
        font: gpui_kit::font(UI_FONT),
        color: gpui_kit::black(),
        background_color: None,
        underline: None,
        strikethrough: None,
    }];
    let chrome = 2. * (PADDING_X + BORDER);
    let wrap = MAX_WIDTH - chrome;
    let single = PADDING_TOP + LINE_HEIGHT + PADDING_BOTTOM + 2. * BORDER;
    let Ok(lines) =
        window
            .text_system()
            .shape_text(text.clone(), px(FONT_SIZE), &runs, Some(px(wrap)), None)
    else {
        return (MAX_WIDTH, single);
    };
    let count: usize = lines
        .iter()
        .map(|line| line.wrap_boundaries().len() + 1)
        .sum();
    let widest = lines
        .iter()
        .map(|line| {
            if line.wrap_boundaries().is_empty() {
                line.width().as_f32()
            } else {
                wrap
            }
        })
        .fold(0., f32::max);
    (
        (widest.round() + chrome).min(MAX_WIDTH),
        single + (count.max(1) - 1) as f32 * LINE_HEIGHT,
    )
}

fn watch_safe_zone(cx: &mut App) {
    let task = cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(SAFE_ZONE_CHECK).await;
            let open = cx.update(|cx| {
                let point = cursor();
                let tips = cx.default_global::<Tooltips>();
                let Some(visible) = tips.visible.as_mut() else {
                    return false;
                };
                if point.x == visible.checked.x && point.y == visible.checked.y {
                    return true;
                }
                visible.checked = point;
                if in_safe_zone(point, visible.owner, visible.tip) {
                    return true;
                }
                close(cx);
                false
            });
            if !open {
                break;
            }
        }
    });
    cx.default_global::<Tooltips>().watching = Some(task);
}

fn show(id: ElementId, text: SharedString, entered: POINT, cx: &mut App) {
    let Some((view, handle, hwnd)) = popup(cx) else {
        return;
    };
    let monitor = unsafe { MonitorFromPoint(entered, MONITOR_DEFAULTTONEAREST) };
    let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
    info.cbSize = size_of::<MONITORINFO>() as u32;
    unsafe { GetMonitorInfoW(monitor, &mut info) };
    let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
    unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) };
    let scale = dpi_x as f32 / 96.;
    let work: RECT = info.rcWork;
    unsafe {
        SetWindowPos(
            hwnd as HWND,
            HWND_TOPMOST,
            entered.x,
            entered.y,
            1,
            1,
            SWP_NOACTIVATE,
        )
    };
    clear_backdrop(hwnd);
    let Ok((width, height)) = handle.update(cx, |_, window, cx| {
        let measured = measure(&text, window);
        view.update(cx, |view, cx| {
            view.text = text.clone();
            view.size = size(px(measured.0), px(measured.1));
            view.phase = Phase::In(Instant::now());
            cx.notify();
        });
        measured
    }) else {
        return;
    };
    let physical = |v: f32| (v * scale).round() as i32;
    let (w, h) = (physical(width), physical(height));
    let gap = physical(POINTER_OFFSET);
    let left = (entered.x - w / 2).clamp(work.left, (work.right - w).max(work.left));
    let mut top = entered.y - gap - h;
    if top < work.top {
        top = entered.y + gap;
    }
    let (side, above, below) = (
        physical(SHADOW_SIDE),
        physical(SHADOW_TOP),
        physical(SHADOW_BOTTOM),
    );
    unsafe {
        SetWindowPos(
            hwnd as HWND,
            HWND_TOPMOST,
            left - side,
            top - above,
            w + 2 * side,
            h + above + below,
            SWP_NOACTIVATE,
        );
        ShowWindow(hwnd as HWND, SW_SHOWNOACTIVATE);
    }
    let tips = cx.default_global::<Tooltips>();
    tips.hiding = None;
    let owner = tips.anchors.get(&id).copied().unwrap_or(RECT {
        left: entered.x,
        top: entered.y,
        right: entered.x,
        bottom: entered.y,
    });
    tips.visible = Some(Visible {
        id,
        owner,
        tip: RECT {
            left,
            top,
            right: left + w,
            bottom: top + h,
        },
        checked: cursor(),
    });
    watch_safe_zone(cx);
}
