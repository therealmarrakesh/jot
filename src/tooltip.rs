use super::*;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use windows_sys::Win32::Foundation::{POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
};
use windows_sys::Win32::UI::Controls::{
    CloseThemeData, GetThemeAnimationProperty, GetThemeAnimationTransform, GetThemeTimingFunction,
    OpenThemeData, TA_CUBIC_BEZIER, TA_TRANSFORM_OPACITY, TAP_TRANSFORMCOUNT, TATT_OPACITY,
    TTFT_CUBIC_BEZIER,
};
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows_sys::Win32::UI::WindowsAndMessaging::SPI_GETMOUSEHOVERTIME;

const PADDING_X: f32 = 9.;
const PADDING_TOP: f32 = 6.;
const PADDING_BOTTOM: f32 = 8.;
const BORDER: f32 = 1.;
const MAX_WIDTH: f32 = 320.;
const FONT_SIZE: f32 = 12.;
const LINE_HEIGHT: f32 = 16.;
const SINGLE_HEIGHT: f32 = PADDING_TOP + LINE_HEIGHT + PADDING_BOTTOM + 2. * BORDER;
const ELEVATION: f32 = 8.;
const SHADOW_SIDE: f32 = ELEVATION;
const SHADOW_TOP: f32 = ELEVATION / 2.;
const SHADOW_BOTTOM: f32 = ELEVATION * 1.5;
const POINTER_OFFSET: f32 = 20.;
const DEFAULT_HOVER_TIME: u32 = 400;
const SHOW_DELAY_FACTOR: f32 = 2.;
const RESHOW_DELAY_FACTOR: f32 = 1.5;
const RESHOW_WINDOW: Duration = Duration::from_millis(200);
const SAFE_ZONE_CHECK: Duration = Duration::from_secs(1);
const FADE_IN_STORYBOARD: i32 = 4;
const FADE_OUT_STORYBOARD: i32 = 5;
const FADE_TARGET: i32 = 1;
const DEFAULT_FADE: Duration = Duration::from_millis(167);
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
    unsafe {
        let theme = OpenThemeData(std::ptr::null_mut(), windows_sys::w!("Animations"));
        let timing = OpenThemeData(std::ptr::null_mut(), windows_sys::w!("timingfunction"));
        let mut fade = None;
        let mut count = 0u32;
        let mut written = 0u32;
        if GetThemeAnimationProperty(
            theme,
            storyboard,
            FADE_TARGET,
            TAP_TRANSFORMCOUNT,
            (&raw mut count).cast(),
            size_of::<u32>() as u32,
            &mut written,
        ) == 0
        {
            for index in 0..count {
                let mut transform = TA_TRANSFORM_OPACITY::default();
                if GetThemeAnimationTransform(
                    theme,
                    storyboard,
                    FADE_TARGET,
                    index,
                    (&raw mut transform).cast(),
                    size_of::<TA_TRANSFORM_OPACITY>() as u32,
                    &mut written,
                ) != 0
                    || transform.header.eTransformType != TATT_OPACITY
                {
                    continue;
                }
                let mut bezier = TA_CUBIC_BEZIER::default();
                let curve = if GetThemeTimingFunction(
                    timing,
                    transform.header.dwTimingFunctionId as i32,
                    (&raw mut bezier).cast(),
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
    noise: Arc<Image>,
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
        let columns = (self.size.width.as_f32() / NOISE_SIZE).ceil() as usize;
        let rows = (self.size.height.as_f32() / NOISE_SIZE).ceil() as usize;
        let noise: Vec<_> = (0..rows)
            .flat_map(|row| (0..columns).map(move |column| (row, column)))
            .map(|(row, column)| {
                img(self.noise.clone())
                    .absolute()
                    .left(px(column as f32 * NOISE_SIZE))
                    .top(px(row as f32 * NOISE_SIZE))
                    .size(px(NOISE_SIZE))
                    .opacity(NOISE_OPACITY)
            })
            .collect();
        div().size_full().opacity(self.opacity(now)).child(
            div()
                .absolute()
                .left(px(SHADOW_SIDE))
                .top(px(SHADOW_TOP))
                .w(self.size.width)
                .h(self.size.height)
                .rounded(px(field::CONTROL_RADIUS))
                .border_1()
                .border_color(rgba(p.flyout_stroke))
                .shadow(theme_shadow(p.dark, ELEVATION * 2., 1.))
                .child(
                    div()
                        .relative()
                        .size_full()
                        .rounded(px(field::CONTROL_RADIUS - BORDER))
                        .overflow_hidden()
                        .bg(rgb(p.acrylic_fallback))
                        .child(div().absolute().size_full().bg(rgba(p.acrylic_luminosity)))
                        .children(noise)
                        .child(
                            div()
                                .relative()
                                .size_full()
                                .pl(px(PADDING_X))
                                .pr(px(PADDING_X))
                                .pt(px(PADDING_TOP))
                                .pb(px(PADDING_BOTTOM))
                                .font_family(SMALL_FONT)
                                .text_size(px(FONT_SIZE))
                                .line_height(px(LINE_HEIGHT))
                                .text_color(rgba(p.text_primary))
                                .when(self.size.height <= px(SINGLE_HEIGHT), |this| {
                                    this.whitespace_nowrap()
                                })
                                .child(self.text.clone()),
                        ),
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

struct Visible {
    id: ElementId,
    owner: RECT,
    tip: RECT,
    checked: POINT,
}

#[derive(Default)]
struct Tooltips {
    popup: Option<overlay::Overlay<TooltipView>>,
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
            let origin = client_origin(window_hwnd(window));
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
            tips.anchors.insert(id, rect);
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

fn show_delay(reshow: bool) -> Duration {
    let hover = system_u32(SPI_GETMOUSEHOVERTIME).unwrap_or(DEFAULT_HOVER_TIME) as f32;
    let factor = if reshow {
        RESHOW_DELAY_FACTOR
    } else {
        SHOW_DELAY_FACTOR
    };
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
                overlay::hide(hwnd);
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
    let (fade_in, fade_out) = (
        theme_fade(FADE_IN_STORYBOARD),
        theme_fade(FADE_OUT_STORYBOARD),
    );
    let popup = overlay::open(
        |_| TooltipView {
            text: "".into(),
            size: size(px(0.), px(0.)),
            noise: acrylic_noise(),
            phase: Phase::Out(Instant::now(), 0.),
            fade_in,
            fade_out,
        },
        cx,
    )?;
    cx.default_global::<Tooltips>().popup = Some(popup.clone());
    Some((popup.view, popup.handle, popup.hwnd))
}

fn measure(text: &SharedString, window: &mut Window) -> (f32, f32) {
    let runs = [TextRun {
        len: text.len(),
        font: gpui_kit::font(SMALL_FONT),
        color: gpui_kit::black(),
        background_color: None,
        underline: None,
        strikethrough: None,
    }];
    let chrome = 2. * (PADDING_X + BORDER);
    let wrap = MAX_WIDTH - chrome;
    let Ok(lines) =
        window
            .text_system()
            .shape_text(text.clone(), px(FONT_SIZE), &runs, Some(px(wrap)), None)
    else {
        return (MAX_WIDTH, SINGLE_HEIGHT);
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
        (widest.ceil() + chrome).min(MAX_WIDTH),
        SINGLE_HEIGHT + (count.max(1) - 1) as f32 * LINE_HEIGHT,
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
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    unsafe { GetMonitorInfoW(monitor, &mut info) };
    let (mut dpi_x, mut dpi_y) = (USER_DEFAULT_SCREEN_DPI, USER_DEFAULT_SCREEN_DPI);
    unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) };
    let scale = dpi_x as f32 / USER_DEFAULT_SCREEN_DPI as f32;
    let work = info.rcWork;
    overlay::place(hwnd, entered.x, entered.y, 1, 1);
    overlay::clear_backdrop(hwnd);
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
    overlay::place(
        hwnd,
        left - side,
        top - above,
        w + 2 * side,
        h + above + below,
    );
    overlay::show(hwnd);
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
