//! Win11 Acrylic-style floating tooltip overlay with hover/reshow delay state machine,
//! monitor work-area clamping, and smooth opacity fade.

use std::time::{Duration, Instant};

use crate::app::AppEvent;
use crate::color::{rgb, rgba, transparent_black};
use crate::geom::{Rect, ScreenPoint, ScreenRect, Size, point, size};
use crate::gfx::{Direct2DDevices, TextAlign, TextSystem, font};
use crate::platform::{
    Jobs, Modifiers, NativeWindow, TimerHandle, WindowAppearance, WindowBackdrop, WindowId,
    WindowKind, WindowOptions, cursor_screen_pos, monitor_work_area_and_scale, mouse_hover_time_ms,
};
use crate::theme::{Palette, theme_shadow};
use crate::types::SharedStr;
use crate::ui::controls::{CONTROL_RADIUS, SMALL_FONT};
use crate::ui::{AnimKey, AvailSize, Easing, Frame, HitId, Node, UiState, box_, text};

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

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct Key {
    window: WindowId,
    hit: HitId,
}

#[derive(Clone)]
struct Snap {
    key: Key,
    text: SharedStr,
    owner: ScreenRect,
    entered: ScreenPoint,
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
    fn key(&self) -> Option<Key> {
        match self {
            Self::Idle => None,
            Self::Waiting { snap, .. } | Self::Showing { snap } => Some(snap.key),
            Self::Suppressed { key } => Some(*key),
        }
    }
}

enum Report {
    Owner(Snap),
    NoneIn(WindowId),
    Gone(Key),
    Suppress(WindowId),
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
        (state, Report::Owner(snap)) if state.key() == Some(snap.key) => state,
        (_, Report::Owner(snap)) => wait(snap),
        (state, Report::NoneIn(window)) if state.key().is_some_and(|k| k.window == window) => {
            State::Idle
        }
        (state, Report::Gone(key)) if state.key() == Some(key) => State::Idle,
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

struct Shown {
    key: Key,
    owner: ScreenRect,
}

#[derive(Default)]
pub(crate) struct Tooltips {
    popup: Option<NativeWindow>,
    ui: UiState,
    text: SharedStr,
    card_size: Size,
    visible: bool,
    dirty: bool,
    state: State,
    epoch: u64,
    pressed: bool,
    shown: Option<Shown>,
    closed_at: Option<Instant>,
    control_only: bool,
    opening: Option<(u64, TimerHandle)>,
    hiding: Option<TimerHandle>,
}

impl Tooltips {
    pub(crate) fn on_mouse_down(&mut self, window: WindowId, jobs: &Jobs<AppEvent>) {
        self.pressed = true;
        self.apply_report(Report::Suppress(window), jobs);
    }

    pub(crate) fn on_mouse_up(&mut self) {
        self.pressed = false;
    }

    pub(crate) fn on_key_down(&mut self) {
        self.control_only = false;
    }

    pub(crate) fn on_modifiers_changed(&mut self, mods: Modifiers, jobs: &Jobs<AppEvent>) {
        let only_ctrl = mods.ctrl_only();
        let released = mods.none();
        let was = std::mem::replace(&mut self.control_only, only_ctrl);
        if released && was {
            self.apply_report(Report::Dismiss, jobs);
        }
    }

    pub(crate) fn window_deactivated(&mut self, window: WindowId, jobs: &Jobs<AppEvent>) {
        self.apply_report(Report::Suppress(window), jobs);
    }

    pub(crate) fn window_closed(&mut self, window: WindowId, jobs: &Jobs<AppEvent>) {
        if let Some(key) = self.state.key().filter(|k| k.window == window) {
            self.apply_report(Report::Gone(key), jobs);
        }
    }

    pub(crate) fn on_mouse_leave(&mut self, window: WindowId, jobs: &Jobs<AppEvent>) {
        self.apply_report(Report::NoneIn(window), jobs);
    }

    /// Evaluates the currently hovered tooltip in `window` directly from its laid-out `Frame`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn evaluate(
        &mut self,
        window: WindowId,
        hovered: bool,
        pressed_any: bool,
        pos: crate::geom::Point,
        origin: ScreenPoint,
        scale: f32,
        frame: &Frame,
        devices: &Direct2DDevices,
        appearance: WindowAppearance,
        ts: &TextSystem,
        jobs: &Jobs<AppEvent>,
    ) {
        self.pressed = pressed_any;
        let gone = self
            .state
            .key()
            .filter(|k| k.window == window)
            .filter(|k| {
                frame.rect_of(k.hit).is_none_or(|r| {
                    self.shown.as_ref().is_some_and(|shown| {
                        shown.key == *k && shown.owner != r.to_screen(scale, origin)
                    })
                })
            });
        let observed = if !hovered {
            Some(Report::NoneIn(window))
        } else if self.pressed {
            None
        } else {
            Some(match frame.tooltip_at(pos) {
                Some((hit, tip, rect)) => Report::Owner(Snap {
                    key: Key { window, hit },
                    text: tip,
                    owner: rect.to_screen(scale, origin),
                    entered: cursor_screen_pos(),
                }),
                None => Report::NoneIn(window),
            })
        };

        if let Some(key) = gone {
            self.transition(Report::Gone(key));
        }
        if let Some(obs) = observed {
            self.transition(obs);
        }
        self.sync(devices, appearance, ts, jobs);
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn on_show_timer(
        &mut self,
        window: WindowId,
        epoch: u64,
        window_state: Option<(bool, crate::geom::Point, ScreenPoint, f32, &Frame)>,
        devices: &Direct2DDevices,
        appearance: WindowAppearance,
        ts: &TextSystem,
        jobs: &Jobs<AppEvent>,
    ) {
        match window_state {
            Some((hovered, pos, origin, scale, frame)) => {
                self.evaluate(
                    window,
                    hovered,
                    self.pressed,
                    pos,
                    origin,
                    scale,
                    frame,
                    devices,
                    appearance,
                    ts,
                    jobs,
                );
            }
            None => {
                if let Some(key) = self.state.key().filter(|k| k.window == window) {
                    self.transition(Report::Gone(key));
                }
            }
        }
        self.transition(Report::Timer(epoch));
        self.sync(devices, appearance, ts, jobs);
    }

    pub(crate) fn on_hide_timer(&mut self) {
        if self.shown.is_none()
            && let Some(popup) = &self.popup
        {
            popup.hwnd().hide();
        }
        self.hiding = None;
    }

    pub(crate) fn recreate_renderer(&mut self, devices: Direct2DDevices) {
        if let Some(popup) = &mut self.popup {
            let _ = popup.recreate_renderer(devices);
            if self.visible {
                self.dirty = true;
            }
        }
    }

    fn transition(&mut self, report: Report) {
        let prev = std::mem::take(&mut self.state);
        self.state = transition_state(prev, report, &mut self.epoch);
    }

    fn apply_report(&mut self, report: Report, jobs: &Jobs<AppEvent>) {
        self.transition(report);
        self.sync_hide_and_timer(jobs);
    }

    fn sync_hide_and_timer(&mut self, jobs: &Jobs<AppEvent>) {
        let showing = match &self.state {
            State::Showing { snap } => Some(snap.key),
            _ => None,
        };
        if self
            .shown
            .as_ref()
            .is_some_and(|shown| showing != Some(shown.key))
        {
            self.hide(jobs);
        }
        let waiting = match &self.state {
            State::Waiting { snap, epoch } => Some((snap.key.window, *epoch)),
            _ => None,
        };
        match waiting {
            Some((_, epoch)) if self.opening.as_ref().is_some_and(|(e, _)| *e == epoch) => {}
            Some((window, epoch)) => {
                let reshow = self
                    .closed_at
                    .is_some_and(|closed| closed.elapsed() < RESHOW_WINDOW);
                let delay = show_delay(reshow);
                let timer = jobs.after(delay, AppEvent::TooltipShowTimer { window, epoch });
                self.opening = Some((epoch, timer));
            }
            None => self.opening = None,
        }
    }

    fn sync(
        &mut self,
        devices: &Direct2DDevices,
        appearance: WindowAppearance,
        ts: &TextSystem,
        jobs: &Jobs<AppEvent>,
    ) {
        self.sync_hide_and_timer(jobs);
        let showing = match &self.state {
            State::Showing { snap } => Some(snap.clone()),
            _ => None,
        };
        if let Some(snap) = showing
            && self.shown.is_none()
        {
            self.show(snap, devices, appearance, ts);
        }
    }

    fn hide(&mut self, jobs: &Jobs<AppEvent>) {
        if self.shown.take().is_none() {
            return;
        }
        self.closed_at = Some(Instant::now());
        self.visible = false;
        self.dirty = true;
        self.hiding = Some(jobs.after(FADE, AppEvent::TooltipHideTimer));
    }

    fn show(
        &mut self,
        snap: Snap,
        devices: &Direct2DDevices,
        appearance: WindowAppearance,
        ts: &TextSystem,
    ) {
        if self.popup.is_none() {
            let opts = WindowOptions {
                bounds: Some(Rect::new(point(0., 0.), size(1., 1.))),
                title: "".into(),
                hide_titlebar: true,
                focus: false,
                show: false,
                kind: WindowKind::Overlay,
                is_resizable: false,
                is_minimizable: false,
                display_id: None,
                backdrop: WindowBackdrop::Transparent,
                min_size: None,
            };
            let Ok(win) = NativeWindow::create(devices.clone(), &opts, appearance) else {
                return;
            };
            self.popup = Some(win);
        }

        let entered = snap.entered;
        let (work, scale) = monitor_work_area_and_scale(entered);
        let measured = measure_tooltip(&snap.text, ts);
        let phys = |v: f32| (v * scale).round() as i32;
        let (w, h) = (phys(measured.width), phys(measured.height));
        let gap = phys(POINTER_OFFSET);
        let left = (entered.x - w / 2).clamp(work.left, (work.right - w).max(work.left));
        let mut top = entered.y - gap - h;
        if top < work.top {
            top = entered.y + gap;
        }
        let (side, above, below) = (phys(SHADOW_SIDE), phys(SHADOW_TOP), phys(SHADOW_BOTTOM));
        let rect = ScreenRect {
            left: left - side,
            top: top - above,
            right: left + w + side,
            bottom: top + h + below,
        };

        self.text = snap.text;
        self.card_size = measured;
        self.visible = true;
        self.dirty = true;
        self.hiding = None;
        self.shown = Some(Shown {
            key: snap.key,
            owner: snap.owner,
        });

        if let Some(popup) = &self.popup {
            let hwnd = popup.hwnd();
            hwnd.place_topmost_noactivate(rect);
            hwnd.show_noactivate();
        }
    }

    /// Paints the tooltip overlay if dirty. Returns `(needs_vsync, device_lost)`.
    pub(crate) fn paint(
        &mut self,
        p: Palette,
        reduce_motion: bool,
        now: Instant,
        ts: &TextSystem,
    ) -> (bool, bool) {
        if !self.dirty {
            return (false, false);
        }
        self.dirty = false;
        let Some(popup) = &mut self.popup else {
            return (false, false);
        };
        if !popup.sync_live_metrics() {
            return (false, false);
        }
        let logical = popup.logical_size;
        let scale = popup.scale_factor;

        self.ui.begin_frame(now, reduce_motion);
        let opacity = self.ui.animate_f32(
            AnimKey::TooltipFade,
            if self.visible { 1. } else { 0. },
            FADE,
            Easing::EaseOutCubic,
        );
        let tree = box_()
            .size_full()
            .opacity(opacity)
            .child(
                box_()
                    .absolute()
                    .left(SHADOW_SIDE)
                    .top(SHADOW_TOP)
                    .child(tooltip_card(p, self.text.clone(), self.card_size)),
            );
        let frame = Frame::layout(
            tree,
            AvailSize::definite(logical.width, logical.height),
            scale,
            ts,
        );
        popup.renderer.begin_frame(transparent_black());
        frame.paint(&mut popup.renderer, &mut |_, _, _| {});
        let lost = popup.renderer.end_frame().unwrap_or(false);
        let animating = self.ui.finish_frame(frame);
        if animating {
            self.dirty = true;
        }
        (animating, lost)
    }
}

fn show_delay(reshow: bool) -> Duration {
    let hover = mouse_hover_time_ms().unwrap_or(DEFAULT_HOVER_TIME) as f32;
    let factor = if reshow {
        RESHOW_DELAY_FACTOR
    } else {
        SHOW_DELAY_FACTOR
    };
    Duration::from_millis((hover * factor) as u64)
}

fn measure_tooltip(content: &SharedStr, ts: &TextSystem) -> Size {
    let chrome = 2. * (PADDING_X + BORDER);
    let wrap = MAX_WIDTH - chrome;
    let lines = ts.shape_wrapped(
        content.clone(),
        FONT_SIZE,
        &font(SMALL_FONT),
        Some(wrap),
        None,
        None,
        TextAlign::Left,
    );
    let count: usize = lines.iter().map(|l| l.line_count.max(1)).sum();
    let widest = lines
        .iter()
        .map(|l| if l.line_count <= 1 { l.width } else { wrap })
        .fold(0., f32::max);
    size(
        (widest.ceil() + chrome).min(MAX_WIDTH),
        SINGLE_HEIGHT + count.saturating_sub(1) as f32 * LINE_HEIGHT,
    )
}

fn tooltip_card(p: Palette, content: SharedStr, sz: Size) -> Node {
    box_()
        .w(sz.width)
        .h(sz.height)
        .rounded(CONTROL_RADIUS)
        .border(BORDER, rgba(p.flyout_stroke))
        .shadows(theme_shadow(p.dark, ELEVATION * 2., 1.))
        .child(
            box_()
                .relative()
                .size_full()
                .rounded(CONTROL_RADIUS - BORDER)
                .overflow_hidden()
                .bg(rgb(p.acrylic_fallback))
                .child(
                    box_()
                        .absolute()
                        .size_full()
                        .bg(rgba(p.acrylic_luminosity)),
                )
                .child(
                    box_()
                        .relative()
                        .size_full()
                        .px(PADDING_X)
                        .pt(PADDING_TOP)
                        .pb(PADDING_BOTTOM)
                        .font_family(SMALL_FONT)
                        .text_size(FONT_SIZE)
                        .line_height(LINE_HEIGHT)
                        .text_color(rgba(p.text_primary))
                        .when(sz.height <= SINGLE_HEIGHT, Node::nowrap)
                        .child(text(content)),
                ),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(window: WindowId, hit: HitId, label: &'static str) -> Snap {
        Snap {
            key: Key { window, hit },
            text: label.into(),
            owner: ScreenRect {
                left: 0,
                top: 0,
                right: 20,
                bottom: 20,
            },
            entered: ScreenPoint { x: 10, y: 10 },
        }
    }

    #[test]
    fn tooltip_state_machine_transitions() {
        let w1 = WindowId(1);
        let w2 = WindowId(2);
        let mut epoch = 0;

        let s = transition_state(
            State::Idle,
            Report::Owner(snap(w1, HitId::NewTab, "One")),
            &mut epoch,
        );
        assert_eq!(epoch, 1);
        assert!(matches!(s, State::Waiting { epoch: 1, .. }));

        let s = transition_state(s, Report::Timer(99), &mut epoch);
        assert!(matches!(s, State::Waiting { epoch: 1, .. }));
        let s = transition_state(s, Report::Timer(1), &mut epoch);
        assert!(matches!(s, State::Showing { .. }));

        let s = transition_state(s, Report::Dismiss, &mut epoch);
        assert!(matches!(s, State::Suppressed { .. }));
        let s = transition_state(
            s,
            Report::Owner(snap(w1, HitId::NewTab, "One")),
            &mut epoch,
        );
        assert!(matches!(s, State::Suppressed { .. }));

        let s = transition_state(s, Report::NoneIn(w2), &mut epoch);
        assert!(matches!(s, State::Suppressed { .. }));
        let s = transition_state(s, Report::NoneIn(w1), &mut epoch);
        assert!(matches!(s, State::Idle));

        let s = transition_state(
            s,
            Report::Owner(snap(w1, HitId::NewTab, "One")),
            &mut epoch,
        );
        assert_eq!(epoch, 2);
        let s = transition_state(
            s,
            Report::Owner(snap(w1, HitId::ThemeToggle, "Two")),
            &mut epoch,
        );
        assert_eq!(epoch, 3);
        assert!(matches!(s, State::Waiting { epoch: 3, .. }));

        let s = transition_state(s, Report::Suppress(w1), &mut epoch);
        assert!(matches!(s, State::Suppressed { .. }));
    }
}
