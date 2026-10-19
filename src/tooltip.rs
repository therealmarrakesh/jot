use super::*;
use gpui_kit::base::{Transition, transition};
use std::collections::HashMap;
use std::time::Instant;
use windows_sys::Win32::Foundation::{POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
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
const FADE: Duration = Duration::from_millis(167);

fn card(p: Palette, text: SharedString, size: gpui_kit::Size<Pixels>) -> Div {
    div()
        .w(size.width)
        .h(size.height)
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
                        .when(size.height <= px(SINGLE_HEIGHT), |this| {
                            this.whitespace_nowrap()
                        })
                        .child(text),
                ),
        )
}

struct TooltipView {
    text: SharedString,
    size: gpui_kit::Size<Pixels>,
    visible: bool,
}

impl Render for TooltipView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let opacity = transition(
            "tooltip-fade",
            if self.visible { 1. } else { 0. },
            Transition::new(FADE),
            window,
            cx,
        );
        div().size_full().opacity(opacity).child(
            div()
                .absolute()
                .left(px(SHADOW_SIDE))
                .top(px(SHADOW_TOP))
                .child(card(p, self.text.clone(), self.size)),
        )
    }
}

#[derive(Clone, PartialEq, Eq)]
struct Key {
    window: AnyWindowHandle,
    id: ElementId,
}

#[derive(Clone)]
struct Snap {
    key: Key,
    text: SharedString,
    owner: RECT,
    entered: POINT,
}

#[derive(Default)]
enum State {
    #[default]
    Idle,
    Waiting {
        snap: Snap,
        epoch: u64,
    },
    Showing {
        snap: Snap,
    },
    Suppressed {
        key: Key,
    },
}

impl State {
    fn key(&self) -> Option<&Key> {
        match self {
            Self::Idle => None,
            Self::Waiting { snap, .. } | Self::Showing { snap } => Some(&snap.key),
            Self::Suppressed { key } => Some(key),
        }
    }
}

enum Report {
    Owner(Snap),
    NoneIn(AnyWindowHandle),
    Gone(Key),
    Suppress(AnyWindowHandle),
    Dismiss,
    Timer(u64),
}

fn transition_state(state: State, report: Report, epoch: &mut u64) -> State {
    let mut wait = |snap| {
        *epoch += 1;
        State::Waiting {
            snap,
            epoch: *epoch,
        }
    };
    match (state, report) {
        (State::Waiting { snap, epoch }, Report::Owner(seen)) if snap.key == seen.key => {
            State::Waiting {
                snap: Snap {
                    text: seen.text,
                    owner: seen.owner,
                    ..snap
                },
                epoch,
            }
        }
        (state, Report::Owner(snap)) if state.key() == Some(&snap.key) => state,
        (_, Report::Owner(snap)) => wait(snap),
        (state, Report::NoneIn(window)) if state.key().is_some_and(|key| key.window == window) => {
            State::Idle
        }
        (state, Report::Gone(key)) if state.key() == Some(&key) => State::Idle,
        (State::Waiting { snap, .. } | State::Showing { snap }, Report::Suppress(window))
            if snap.key.window == window =>
        {
            State::Suppressed { key: snap.key }
        }
        (State::Waiting { snap, .. } | State::Showing { snap }, Report::Dismiss) => {
            State::Suppressed { key: snap.key }
        }
        (State::Waiting { snap, epoch }, Report::Timer(fired)) if epoch == fired => {
            State::Showing { snap }
        }
        (state, _) => state,
    }
}

struct Entry {
    id: ElementId,
    text: SharedString,
    hitbox: Hitbox,
}

struct Shown {
    key: Key,
    owner: RECT,
}

#[derive(Default)]
struct Tooltips {
    popup: Option<overlay::Overlay<TooltipView>>,
    owners: HashMap<AnyWindowHandle, Vec<Entry>>,
    state: State,
    epoch: u64,
    pressed: bool,
    shown: Option<Shown>,
    closed_at: Option<Instant>,
    control_only: bool,
    opening: Option<(u64, Task<()>)>,
    hiding: Option<Task<()>>,
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
                report(Report::Suppress(window.window_handle()), cx);
            }
        });
        window.on_mouse_event(|_: &MouseUpEvent, phase, _, cx| {
            if phase == DispatchPhase::Capture {
                cx.default_global::<Tooltips>().pressed = false;
            }
        });
        window.on_key_event(|_: &KeyDownEvent, phase, _, cx| {
            if phase == DispatchPhase::Capture {
                cx.default_global::<Tooltips>().control_only = false;
            }
        });
        window.on_modifiers_changed(|event, _, cx| modifiers_changed(&event.modifiers, cx));
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
                            && !overlay::same_rect(
                                shown.owner,
                                overlay::screen_rect(entry.hitbox.bounds, scale, origin),
                            )
                    })
                })
        })
        .cloned();
    let observed = if !window.is_window_hovered() {
        Some(Report::NoneIn(handle))
    } else if tips.pressed {
        None
    } else {
        let hovered = entries
            .iter()
            .rev()
            .find(|entry| entry.hitbox.is_hovered_at(position, window));
        Some(hovered.map_or(Report::NoneIn(handle), |entry| {
            Report::Owner(Snap {
                key: Key {
                    window: handle,
                    id: entry.id.clone(),
                },
                text: entry.text.clone(),
                owner: overlay::screen_rect(entry.hitbox.bounds, scale, origin),
                entered: cursor(),
            })
        }))
    };
    if let Some(key) = gone {
        report(Report::Gone(key), cx);
    }
    if let Some(observed) = observed {
        report(observed, cx);
    }
}

fn report(report: Report, cx: &mut App) {
    let tips = cx.default_global::<Tooltips>();
    let state = std::mem::take(&mut tips.state);
    tips.state = transition_state(state, report, &mut tips.epoch);
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

pub(crate) fn window_deactivated(window: AnyWindowHandle, cx: &mut App) {
    report(Report::Suppress(window), cx);
}

fn modifiers_changed(modifiers: &Modifiers, cx: &mut App) {
    let only_control = *modifiers == Modifiers::control();
    let released = !modifiers.modified();
    let tips = cx.default_global::<Tooltips>();
    let was = std::mem::replace(&mut tips.control_only, only_control);
    if released && was {
        report(Report::Dismiss, cx);
    }
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

fn hide(cx: &mut App) {
    let tips = cx.default_global::<Tooltips>();
    if tips.shown.take().is_none() {
        return;
    }
    tips.closed_at = Some(Instant::now());
    let Some(popup) = &tips.popup else {
        return;
    };
    let (view, hwnd) = (popup.view.clone(), popup.hwnd);
    view.update(cx, |view, cx| {
        view.visible = false;
        cx.notify();
    });
    let task = cx.spawn(async move |cx| {
        cx.background_executor().timer(FADE).await;
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
    let popup = overlay::open(
        |_| TooltipView {
            text: "".into(),
            size: size(px(0.), px(0.)),
            visible: false,
        },
        cx,
    )?;
    cx.default_global::<Tooltips>().popup = Some(popup.clone());
    Some(popup)
}

fn measure(text: &SharedString, window: &mut Window) -> gpui_kit::Size<Pixels> {
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
        return size(px(MAX_WIDTH), px(SINGLE_HEIGHT));
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
    size(
        px((widest.ceil() + chrome).min(MAX_WIDTH)),
        px(SINGLE_HEIGHT + count.saturating_sub(1) as f32 * LINE_HEIGHT),
    )
}

fn show(snap: Snap, cx: &mut App) {
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
    overlay::place(
        popup.hwnd,
        RECT {
            left: entered.x,
            top: entered.y,
            right: entered.x + 1,
            bottom: entered.y + 1,
        },
    );
    let text = snap.text.clone();
    let Ok(measured) = popup.handle.update(cx, |_, window, cx| {
        let measured = measure(&text, window);
        popup.view.update(cx, |view, cx| {
            view.text = text.clone();
            view.size = measured;
            view.visible = true;
            cx.notify();
        });
        measured
    }) else {
        return;
    };
    let physical = |v: Pixels| (v.as_f32() * scale).round() as i32;
    let (w, h) = (physical(measured.width), physical(measured.height));
    let gap = physical(px(POINTER_OFFSET));
    let left = (entered.x - w / 2).clamp(work.left, (work.right - w).max(work.left));
    let mut top = entered.y - gap - h;
    if top < work.top {
        top = entered.y + gap;
    }
    let (side, above, below) = (
        physical(px(SHADOW_SIDE)),
        physical(px(SHADOW_TOP)),
        physical(px(SHADOW_BOTTOM)),
    );
    overlay::place(
        popup.hwnd,
        RECT {
            left: left - side,
            top: top - above,
            right: left + w + side,
            bottom: top + h + below,
        },
    );
    overlay::show(popup.hwnd);
    let tips = cx.default_global::<Tooltips>();
    tips.hiding = None;
    tips.shown = Some(Shown {
        key: snap.key,
        owner: snap.owner,
    });
}
