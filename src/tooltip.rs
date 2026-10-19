use super::*;
use std::collections::HashMap;
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
    let theme = unsafe { OpenThemeData(std::ptr::null_mut(), windows_sys::w!("Animations")) };
    let timing = unsafe { OpenThemeData(std::ptr::null_mut(), windows_sys::w!("timingfunction")) };
    let mut fade = None;
    let mut count = 0u32;
    let mut written = 0u32;
    if unsafe {
        GetThemeAnimationProperty(
            theme,
            storyboard,
            FADE_TARGET,
            TAP_TRANSFORMCOUNT,
            (&raw mut count).cast(),
            size_of::<u32>() as u32,
            &mut written,
        )
    } == 0
    {
        for index in 0..count {
            let mut transform = TA_TRANSFORM_OPACITY::default();
            if unsafe {
                GetThemeAnimationTransform(
                    theme,
                    storyboard,
                    FADE_TARGET,
                    index,
                    (&raw mut transform).cast(),
                    size_of::<TA_TRANSFORM_OPACITY>() as u32,
                    &mut written,
                )
            } != 0
                || transform.header.eTransformType != TATT_OPACITY
            {
                continue;
            }
            let mut bezier = TA_CUBIC_BEZIER::default();
            let curve = if unsafe {
                GetThemeTimingFunction(
                    timing,
                    transform.header.dwTimingFunctionId as i32,
                    (&raw mut bezier).cast(),
                    size_of::<TA_CUBIC_BEZIER>() as u32,
                    &mut written,
                )
            } == 0
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
    unsafe {
        CloseThemeData(theme);
        CloseThemeData(timing);
    }
    fade.unwrap_or(Fade::LINEAR)
}

#[derive(Clone, Copy)]
enum Phase {
    In(Instant),
    Out(Instant, f32),
}

struct TooltipView {
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
        let (since, fade) = match self.phase {
            Phase::In(since) => (since, self.fade_in),
            Phase::Out(since, _) => (since, self.fade_out),
        };
        now.saturating_duration_since(since) >= fade.total()
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
        let image = acrylic_noise();
        let noise: Vec<_> = (0..rows)
            .flat_map(|row| (0..columns).map(move |column| (row, column)))
            .map(|(row, column)| {
                img(image.clone())
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
                                .px(px(PADDING_X))
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

#[derive(Clone, PartialEq, Eq)]
struct Key<W> {
    window: W,
    id: ElementId,
}

#[derive(Clone)]
struct Snap<W> {
    key: Key<W>,
    text: SharedString,
    owner: RECT,
    entered: POINT,
}

#[derive(Default)]
enum State<W> {
    #[default]
    Idle,
    Waiting {
        snap: Snap<W>,
        epoch: u64,
    },
    Showing {
        snap: Snap<W>,
    },
    Suppressed {
        key: Key<W>,
    },
}

impl<W> State<W> {
    fn key(&self) -> Option<&Key<W>> {
        match self {
            Self::Idle => None,
            Self::Waiting { snap, .. } | Self::Showing { snap } => Some(&snap.key),
            Self::Suppressed { key } => Some(key),
        }
    }
}

enum Report<W> {
    Owner(Snap<W>),
    NoneIn(W),
    Gone(Key<W>),
    Press(W),
    Dismiss,
    LeftSafeZone,
    Timer(u64),
}

fn transition<W: PartialEq>(state: State<W>, report: Report<W>, epoch: &mut u64) -> State<W> {
    let mut wait = |snap| {
        *epoch += 1;
        State::Waiting {
            snap,
            epoch: *epoch,
        }
    };
    match (state, report) {
        (state, Report::Owner(snap)) if state.key() == Some(&snap.key) => state,
        (_, Report::Owner(snap)) => wait(snap),
        (state @ (State::Waiting { .. } | State::Suppressed { .. }), Report::NoneIn(window))
            if state.key().is_some_and(|key| key.window == window) =>
        {
            State::Idle
        }
        (state, Report::Gone(key)) if state.key() == Some(&key) => State::Idle,
        (State::Waiting { snap, .. } | State::Showing { snap }, Report::Press(window))
            if snap.key.window == window =>
        {
            State::Suppressed { key: snap.key }
        }
        (State::Waiting { snap, .. } | State::Showing { snap }, Report::Dismiss) => {
            State::Suppressed { key: snap.key }
        }
        (State::Showing { .. }, Report::LeftSafeZone) => State::Idle,
        (State::Waiting { snap, epoch }, Report::Timer(fired)) if epoch == fired => {
            State::Showing { snap }
        }
        (state, _) => state,
    }
}

fn topmost<T>(entries: &[T], hovered: impl Fn(&T) -> bool) -> Option<&T> {
    entries.iter().rev().find(|entry| hovered(entry))
}

fn observe<W>(
    window: W,
    hovered: bool,
    pressed: bool,
    owner: impl FnOnce() -> Option<Snap<W>>,
) -> Option<Report<W>> {
    if !hovered {
        return Some(Report::NoneIn(window));
    }
    if pressed {
        return None;
    }
    Some(owner().map_or(Report::NoneIn(window), Report::Owner))
}

fn screen_rect(bounds: Bounds<Pixels>, scale: f32, origin: POINT) -> RECT {
    let physical = |v: Pixels| (v.as_f32() * scale).round() as i32;
    RECT {
        left: origin.x + physical(bounds.origin.x),
        top: origin.y + physical(bounds.origin.y),
        right: origin.x + physical(bounds.origin.x + bounds.size.width),
        bottom: origin.y + physical(bounds.origin.y + bounds.size.height),
    }
}

fn same_rect(a: RECT, b: RECT) -> bool {
    (a.left, a.top, a.right, a.bottom) == (b.left, b.top, b.right, b.bottom)
}

struct Entry {
    id: ElementId,
    text: SharedString,
    hitbox: Hitbox,
}

struct Shown {
    key: Key<AnyWindowHandle>,
    owner: RECT,
    tip: RECT,
    checked: POINT,
}

#[derive(Default)]
struct Tooltips {
    popup: Option<overlay::Overlay<TooltipView>>,
    owners: HashMap<AnyWindowHandle, Vec<Entry>>,
    state: State<AnyWindowHandle>,
    epoch: u64,
    pressed: bool,
    shown: Option<Shown>,
    closed_at: Option<Instant>,
    control_only: bool,
    opening: Option<(u64, Task<()>)>,
    hiding: Option<Task<()>>,
    watching: Option<Task<()>>,
}

impl Global for Tooltips {}

pub(crate) fn owner(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    child: impl IntoElement,
) -> measure::Measured {
    let (id, text) = (id.into(), text.into());
    measure::measured(child, move |bounds, window, cx| {
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        let owners = cx
            .default_global::<Tooltips>()
            .owners
            .entry(window.window_handle())
            .or_default();
        owners.retain(|entry| entry.id != id);
        owners.push(Entry { id, text, hitbox });
    })
}

pub(crate) struct Host {
    child: AnyElement,
}

pub(crate) fn host(child: impl IntoElement) -> Host {
    Host {
        child: child.into_any_element(),
    }
}

impl Element for Host {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        cx.default_global::<Tooltips>()
            .owners
            .insert(window.window_handle(), Vec::new());
        self.child.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        window.on_mouse_event(|event: &MouseMoveEvent, phase, window, cx| {
            if phase == DispatchPhase::Capture {
                cx.default_global::<Tooltips>().pressed = event.pressed_button.is_some();
                evaluate(window, cx);
            }
        });
        window.on_mouse_event(|_: &MouseDownEvent, phase, window, cx| {
            if phase == DispatchPhase::Capture {
                cx.default_global::<Tooltips>().pressed = true;
                report(Report::Press(window.window_handle()), cx);
            }
        });
        window.on_mouse_event(|_: &MouseUpEvent, phase, _, cx| {
            if phase == DispatchPhase::Capture {
                cx.default_global::<Tooltips>().pressed = false;
            }
        });
        window.defer(cx, evaluate);
        self.child.paint(window, cx);
    }
}

impl IntoElement for Host {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

fn evaluate(window: &mut Window, cx: &mut App) {
    let handle = window.window_handle();
    let position = window.mouse_position();
    let scale = window.scale_factor();
    let origin = client_origin(window_hwnd(window));
    let tips = cx.default_global::<Tooltips>();
    let entries = tips.owners.get(&handle).map_or(&[][..], Vec::as_slice);
    let gone = tips
        .state
        .key()
        .filter(|key| key.window == handle)
        .filter(|key| {
            entries
                .iter()
                .find(|entry| entry.id == key.id)
                .is_none_or(|entry| {
                    tips.shown.as_ref().is_some_and(|shown| {
                        shown.key == **key
                            && !same_rect(
                                shown.owner,
                                screen_rect(entry.hitbox.bounds, scale, origin),
                            )
                    })
                })
        })
        .cloned();
    let observed = observe(handle, window.is_window_hovered(), tips.pressed, || {
        topmost(entries, |entry| {
            entry.hitbox.is_hovered_at(position, window)
        })
        .map(|entry| Snap {
            key: Key {
                window: handle,
                id: entry.id.clone(),
            },
            text: entry.text.clone(),
            owner: screen_rect(entry.hitbox.bounds, scale, origin),
            entered: cursor(),
        })
    });
    if let Some(key) = gone {
        report(Report::Gone(key), cx);
    }
    if let Some(observed) = observed {
        report(observed, cx);
    }
}

fn report(report: Report<AnyWindowHandle>, cx: &mut App) {
    let tips = cx.default_global::<Tooltips>();
    let state = std::mem::take(&mut tips.state);
    tips.state = transition(state, report, &mut tips.epoch);
    cx.defer(render);
}

fn render(cx: &mut App) {
    let tips = cx.default_global::<Tooltips>();
    let showing = match &tips.state {
        State::Showing { snap } => Some(snap.clone()),
        _ => None,
    };
    if tips
        .shown
        .as_ref()
        .is_some_and(|shown| showing.as_ref().is_none_or(|snap| snap.key != shown.key))
    {
        hide(cx);
    }
    if let Some(snap) = showing
        && cx.default_global::<Tooltips>().shown.is_none()
    {
        show(snap, cx);
    }
    let tips = cx.default_global::<Tooltips>();
    let waiting = match &tips.state {
        State::Waiting { snap, epoch } => Some((snap.key.clone(), *epoch)),
        _ => None,
    };
    match waiting {
        Some((_, epoch))
            if tips
                .opening
                .as_ref()
                .is_some_and(|(opening, _)| *opening == epoch) => {}
        Some((key, epoch)) => {
            let reshow = tips
                .closed_at
                .is_some_and(|closed| closed.elapsed() < RESHOW_WINDOW);
            let delay = show_delay(reshow);
            let task = cx.spawn(async move |cx| {
                cx.background_executor().timer(delay).await;
                cx.update(|cx| {
                    let window = key.window;
                    if window
                        .update(cx, |_, window, cx| evaluate(window, cx))
                        .is_err()
                    {
                        report(Report::Gone(key), cx);
                    }
                    report(Report::Timer(epoch), cx);
                });
            });
            cx.default_global::<Tooltips>().opening = Some((epoch, task));
        }
        None => tips.opening = None,
    }
}

pub(crate) fn window_closed(window: WindowId, cx: &mut App) {
    let tips = cx.default_global::<Tooltips>();
    tips.owners.retain(|handle, _| handle.window_id() != window);
    if let Some(key) = tips
        .state
        .key()
        .filter(|key| key.window.window_id() == window)
        .cloned()
    {
        report(Report::Gone(key), cx);
    }
}

pub(crate) fn pointer_moved(cx: &mut App) {
    let point = cursor();
    let Some(tips) = cx.try_global::<Tooltips>() else {
        return;
    };
    if tips
        .shown
        .as_ref()
        .is_some_and(|shown| !in_safe_zone(point, shown.owner, shown.tip))
    {
        report(Report::LeftSafeZone, cx);
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
    let only_control = *modifiers == Modifiers::control();
    let released = !modifiers.modified();
    let tips = cx.default_global::<Tooltips>();
    let was = std::mem::replace(&mut tips.control_only, only_control);
    if released && was {
        report(Report::Dismiss, cx);
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

fn hide(cx: &mut App) {
    let tips = cx.default_global::<Tooltips>();
    if tips.shown.take().is_none() {
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
            if tips.shown.is_none() {
                overlay::hide(hwnd);
            }
            tips.hiding = None;
        });
    });
    cx.default_global::<Tooltips>().hiding = Some(task);
}

fn popup(cx: &mut App) -> Option<overlay::Overlay<TooltipView>> {
    if let Some(popup) = &cx.default_global::<Tooltips>().popup {
        return Some(popup.clone());
    }
    let (fade_in, fade_out) = (
        theme_fade(FADE_IN_STORYBOARD),
        theme_fade(FADE_OUT_STORYBOARD),
    );
    let popup = overlay::open(
        |_| TooltipView {
            text: "".into(),
            size: size(px(0.), px(0.)),
            phase: Phase::Out(Instant::now(), 0.),
            fade_in,
            fade_out,
        },
        cx,
    )?;
    cx.default_global::<Tooltips>().popup = Some(popup.clone());
    Some(popup)
}

fn measure(text: &SharedString, window: &mut Window) -> (f32, f32) {
    let runs = [TextRun {
        len: text.len(),
        font: font(SMALL_FONT),
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
        SINGLE_HEIGHT + count.saturating_sub(1) as f32 * LINE_HEIGHT,
    )
}

fn watch_safe_zone(cx: &mut App) {
    let task = cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(SAFE_ZONE_CHECK).await;
            let open = cx.update(|cx| {
                let point = cursor();
                let tips = cx.default_global::<Tooltips>();
                let Some(shown) = tips.shown.as_mut() else {
                    return false;
                };
                if point.x == shown.checked.x && point.y == shown.checked.y {
                    return true;
                }
                shown.checked = point;
                if in_safe_zone(point, shown.owner, shown.tip) {
                    return true;
                }
                report(Report::LeftSafeZone, cx);
                false
            });
            if !open {
                break;
            }
        }
    });
    cx.default_global::<Tooltips>().watching = Some(task);
}

fn show(snap: Snap<AnyWindowHandle>, cx: &mut App) {
    let Some(popup) = popup(cx) else {
        return;
    };
    let entered = snap.entered;
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
    overlay::place(popup.hwnd, entered.x, entered.y, 1, 1);
    overlay::clear_backdrop(popup.hwnd);
    let text = snap.text.clone();
    let Ok((width, height)) = popup.handle.update(cx, |_, window, cx| {
        let measured = measure(&text, window);
        popup.view.update(cx, |view, cx| {
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
        popup.hwnd,
        left - side,
        top - above,
        w + 2 * side,
        h + above + below,
    );
    overlay::show(popup.hwnd);
    let tips = cx.default_global::<Tooltips>();
    tips.hiding = None;
    tips.shown = Some(Shown {
        key: snap.key,
        owner: snap.owner,
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
