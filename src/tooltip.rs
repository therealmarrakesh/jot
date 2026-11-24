use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::app::AppEvent;
use crate::geom::{ScreenPoint, ScreenRect, Size, size};
use crate::gfx::{Gpu, LabelLayout, TextAlign, TextStyle, TextSystem};
use crate::ids::{Frame, HitId};
use crate::platform::{
    Jobs, Modifiers, TimerHandle, WindowAppearance, WindowId, cursor_screen_pos,
    monitor_work_area_and_scale, system_settings,
};
use crate::theme::{Palette, theme_shadow};
use crate::types::SharedStr;
use crate::ui::controls::{CONTROL_RADIUS, SMALL_FONT};
use crate::ui::{
    AnimTracks, AvailSize, Easing, Frame as GenericFrame, Node as GenericNode, OverlaySurface,
    PointerState, box_, text,
};

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
const SHOW_DELAY_FACTOR: f32 = 2.;
const RESHOW_DELAY_FACTOR: f32 = 1.5;
const RESHOW_WINDOW: Duration = Duration::from_millis(200);
const FADE: Duration = Duration::from_millis(167);

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct TooltipTarget {
    pub(crate) window: WindowId,
    pub(crate) hit: HitId,
}

type Key = TooltipTarget;

pub(crate) struct TooltipProbe<'a> {
    pub(crate) window: WindowId,
    pub(crate) pointer: Option<PointerState>,
    pub(crate) origin: ScreenPoint,
    pub(crate) scale: f32,
    pub(crate) frame: &'a Frame,
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
            Self::Waiting { snap } | Self::Showing { snap } => Some(snap.key),
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
    Timer(Key),
}

fn transition_state(state: State, report: Report) -> State {
    match (state, report) {
        (State::Waiting { snap }, Report::Owner(seen)) if snap.key == seen.key => State::Waiting {
            snap: Snap {
                text: seen.text,
                owner: seen.owner,
                ..snap
            },
        },
        (state, Report::Owner(snap)) if state.key() == Some(snap.key) => state,
        (_, Report::Owner(snap)) => State::Waiting { snap },
        (state, Report::NoneIn(window)) if state.key().is_some_and(|k| k.window == window) => {
            State::Idle
        }
        (state, Report::Gone(key)) if state.key() == Some(key) => State::Idle,
        (State::Waiting { snap } | State::Showing { snap }, Report::Suppress(window))
            if snap.key.window == window =>
        {
            State::Suppressed { key: snap.key }
        }
        (State::Waiting { snap } | State::Showing { snap }, Report::Dismiss) => {
            State::Suppressed { key: snap.key }
        }
        (State::Waiting { snap }, Report::Timer(key)) if snap.key == key => {
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
    overlay: OverlaySurface,
    fade: AnimTracks<()>,
    text: SharedStr,
    card_size: Size,
    dirty: bool,
    state: State,
    pressed: bool,
    shown: Option<Shown>,
    closed_at: Option<Instant>,
    control_only: bool,
    opening: Option<(Key, TimerHandle)>,
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

    pub(crate) fn evaluate(
        &mut self,
        probe: TooltipProbe<'_>,
        gpu: &Rc<Gpu>,
        appearance: WindowAppearance,
        ts: &TextSystem,
        jobs: &Jobs<AppEvent>,
    ) {
        let TooltipProbe {
            window,
            pointer,
            origin,
            scale,
            frame,
        } = probe;
        if let Some(ptr) = pointer {
            self.pressed = ptr.pressed;
        }
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
        let observed = match pointer {
            None => Some(Report::NoneIn(window)),
            Some(_) if self.pressed => None,
            Some(ptr) => Some(match frame.tooltip_at(ptr.pos) {
                Some((hit, tip, rect)) => Report::Owner(Snap {
                    key: Key { window, hit },
                    text: tip,
                    owner: rect.to_screen(scale, origin),
                    entered: cursor_screen_pos(),
                }),
                None => Report::NoneIn(window),
            }),
        };

        if let Some(key) = gone {
            self.transition(Report::Gone(key));
        }
        if let Some(obs) = observed {
            self.transition(obs);
        }
        self.sync(gpu, appearance, ts, jobs);
    }

    pub(crate) fn on_show_timer(
        &mut self,
        target: TooltipTarget,
        probe: Option<TooltipProbe<'_>>,
        gpu: &Rc<Gpu>,
        appearance: WindowAppearance,
        ts: &TextSystem,
        jobs: &Jobs<AppEvent>,
    ) {
        match probe {
            Some(probe) => {
                self.evaluate(probe, gpu, appearance, ts, jobs);
            }
            None => {
                if let Some(key) = self.state.key().filter(|k| k.window == target.window) {
                    self.transition(Report::Gone(key));
                }
            }
        }
        self.transition(Report::Timer(target));
        self.sync(gpu, appearance, ts, jobs);
    }

    pub(crate) fn on_hide_timer(&mut self) {
        if self.shown.is_none() {
            self.overlay.hide();
        }
        self.hiding = None;
    }

    fn transition(&mut self, report: Report) {
        let prev = std::mem::take(&mut self.state);
        self.state = transition_state(prev, report);
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
            State::Waiting { snap } => Some(snap.key),
            _ => None,
        };
        match waiting {
            Some(key) if self.opening.as_ref().is_some_and(|(k, _)| *k == key) => {}
            Some(key) => {
                let reshow = self
                    .closed_at
                    .is_some_and(|closed| closed.elapsed() < RESHOW_WINDOW);
                let delay = show_delay(reshow);
                let timer = jobs.after(
                    delay,
                    AppEvent::TooltipShowTimer {
                        window: key.window,
                        hit: key.hit,
                    },
                );
                self.opening = Some((key, timer));
            }
            None => self.opening = None,
        }
    }

    fn sync(
        &mut self,
        gpu: &Rc<Gpu>,
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
            self.show(snap, gpu, appearance, ts);
        }
    }

    fn hide(&mut self, jobs: &Jobs<AppEvent>) {
        if self.shown.take().is_none() {
            return;
        }
        self.closed_at = Some(Instant::now());
        self.dirty = true;
        self.hiding = Some(jobs.after(FADE, AppEvent::TooltipHideTimer));
    }

    fn show(
        &mut self,
        snap: Snap,
        gpu: &Rc<Gpu>,
        appearance: WindowAppearance,
        ts: &TextSystem,
    ) {
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
        self.dirty = true;
        self.hiding = None;
        self.shown = Some(Shown {
            key: snap.key,
            owner: snap.owner,
        });
        self.overlay.show(gpu, rect, appearance);
    }

    pub(crate) fn paint(
        &mut self,
        p: Palette,
        reduce_motion: bool,
        now: Instant,
        ts: &TextSystem,
    ) -> bool {
        if !self.dirty {
            return false;
        }
        self.dirty = false;
        let visible = self.shown.is_some();
        let text = self.text.clone();
        let card_size = self.card_size;
        let fade = &mut self.fade;
        let mut animating = false;
        let needs_retry = self.overlay.paint(|logical, scale, renderer| {
            fade.begin_frame();
            let opacity = fade.animate_f32(
                (),
                if visible { 1. } else { 0. },
                FADE,
                Easing::EaseOutCubic,
                now,
                reduce_motion,
            );
            let tree: GenericNode<(), ()> = box_().size_full().opacity(opacity).child(
                box_()
                    .absolute()
                    .left(SHADOW_SIDE)
                    .top(SHADOW_TOP)
                    .child(tooltip_card(p, text, card_size)),
            );
            let frame = GenericFrame::layout(
                tree,
                AvailSize::definite(logical.width, logical.height),
                scale,
                ts,
            );
            frame.paint(renderer, &mut |_, _, _| {});
            animating = fade.end_frame();
        });
        if animating || needs_retry {
            self.dirty = true;
        }
        animating || needs_retry
    }
}

fn show_delay(reshow: bool) -> Duration {
    let hover = system_settings().mouse_hover_time_ms as f32;
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
    let label = ts.shape_label(
        content.clone(),
        &TextStyle::regular(SMALL_FONT, FONT_SIZE),
        LabelLayout {
            line_height: LINE_HEIGHT,
            wrap_width: Some(wrap),
            line_clamp: None,
            align: TextAlign::Left,
        },
    );
    let widest = if label.line_count > 1 && !content.contains('\n') {
        wrap
    } else {
        label.width
    };
    size(
        (widest.ceil() + chrome).min(MAX_WIDTH),
        SINGLE_HEIGHT + label.line_count.saturating_sub(1) as f32 * LINE_HEIGHT,
    )
}

fn tooltip_card(p: Palette, content: SharedStr, sz: Size) -> GenericNode<(), ()> {
    box_()
        .w(sz.width)
        .h(sz.height)
        .rounded(CONTROL_RADIUS)
        .border(BORDER, p.flyout_stroke)
        .shadows(theme_shadow(p.dark, ELEVATION * 2., 1.))
        .child(
            box_()
                .relative()
                .size_full()
                .rounded(CONTROL_RADIUS - BORDER)
                .overflow_hidden()
                .bg(p.acrylic_fallback)
                .child(
                    box_()
                        .absolute()
                        .size_full()
                        .bg(p.acrylic_luminosity),
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
                        .text_color(p.text_primary)
                        .when(sz.height <= SINGLE_HEIGHT, GenericNode::nowrap)
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
        let k1 = Key {
            window: w1,
            hit: HitId::NewTab,
        };
        let k2 = Key {
            window: w1,
            hit: HitId::ThemeToggle,
        };

        let s = transition_state(State::Idle, Report::Owner(snap(w1, HitId::NewTab, "One")));
        assert!(matches!(s, State::Waiting { .. }));

        let s = transition_state(s, Report::Timer(k2));
        assert!(matches!(s, State::Waiting { .. }));
        let s = transition_state(s, Report::Timer(k1));
        assert!(matches!(s, State::Showing { .. }));

        let s = transition_state(s, Report::Dismiss);
        assert!(matches!(s, State::Suppressed { .. }));
        let s = transition_state(s, Report::Owner(snap(w1, HitId::NewTab, "One")));
        assert!(matches!(s, State::Suppressed { .. }));

        let s = transition_state(s, Report::NoneIn(w2));
        assert!(matches!(s, State::Suppressed { .. }));
        let s = transition_state(s, Report::NoneIn(w1));
        assert!(matches!(s, State::Idle));

        let s = transition_state(s, Report::Owner(snap(w1, HitId::NewTab, "One")));
        let s = transition_state(s, Report::Owner(snap(w1, HitId::ThemeToggle, "Two")));
        assert!(matches!(s, State::Waiting { ref snap } if snap.key == k2));

        let s = transition_state(s, Report::Suppress(w1));
        assert!(matches!(s, State::Suppressed { .. }));
    }
}
