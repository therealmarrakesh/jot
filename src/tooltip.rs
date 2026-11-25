//! The overlays above the windows, each in a window of its own: tooltips, which show after
//! the pointer rests on their owner (above it, or below without room) and fade, and the Alt-key
//! tips over the menus.

use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::geom::{
    Rect, ScreenPoint, ScreenRect, Size, point, round_half_up, size, transparent_black,
};
use crate::gfx::{Gpu, TextStyle, TextSystem, WindowSurface};
use crate::layout::AvailSize;
use crate::platform::{Modifiers, NativeWindow, WindowId, WindowsDisplay, cursor_screen_pos};
use crate::shell::Shell;
use crate::theme::{CONTROL_RADIUS, Palette, SMALL_FONT};
use crate::types::SharedStr;
use crate::ui::{AnimTracks, AppFrame, AppNode, Curve, Easing, HitId, PointerState, box_, text};
use crate::widgets::card;

const PADDING_X: f32 = 9.;
const PADDING_TOP: f32 = 6.;
const PADDING_BOTTOM: f32 = 8.;
const MAX_WIDTH: f32 = 320.;
const FONT_SIZE: f32 = 12.;
const LINE_HEIGHT: f32 = 16.;
const ELEVATION: f32 = 8.;
const SHADOW_SIDE: f32 = ELEVATION;
const SHADOW_TOP: f32 = ELEVATION / 2.;
const SHADOW_BOTTOM: f32 = ELEVATION * 1.5;
const POINTER_OFFSET: f32 = 20.;

const SHOW_DELAY_FACTOR: f32 = 2.;
const RESHOW_DELAY_FACTOR: f32 = 1.5;
/// How soon after one closes another shows sooner.
const RESHOW_WINDOW: Duration = Duration::from_millis(200);
const FADE_CURVE: Curve = Curve::new(Duration::from_millis(167), Easing::EaseOutCubic);

const KEY_TIP_PADDING: f32 = 4.;
const KEY_TIP_MARGIN: f32 = 1.;
const KEY_TIP_FONT: &str = "Segoe UI";

// ---- 1. The overlay surface -----------------------------------------------------------------

/// Where a window's overlays are placed from: its client area's top left on screen, and scale.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct OverlayAnchor {
    pub(crate) window: WindowId,
    pub(crate) origin: ScreenPoint,
    pub(crate) scale: f32,
}

/// An overlay's window, made the first time it shows (and not tried again if that fails).
#[derive(Default)]
struct OverlaySurface {
    live: Option<WindowSurface>,
    create_failed: bool,
}

impl OverlaySurface {
    fn show(&mut self, gpu: &Rc<Gpu>, rect: ScreenRect) {
        if self.live.is_none() && !self.create_failed {
            let created = NativeWindow::create_overlay()
                .and_then(|window| WindowSurface::new(gpu.clone(), window));
            match created {
                Ok(live) => self.live = Some(live),
                Err(err) => {
                    self.create_failed = true;
                    crate::platform::debug_log(&format!("overlay create failed: {err}"));
                }
            }
        }
        if let Some(live) = &self.live {
            live.window.place_overlay(rect);
        }
    }

    fn hide(&self) {
        if let Some(live) = &self.live {
            live.window.hwnd().hide();
        }
    }

    /// Lays out and paints what `build` makes; whether to paint again (the device was lost).
    fn paint_scene(&mut self, ts: &TextSystem, build: impl FnOnce(Size) -> AppNode) -> bool {
        let Some(live) = &mut self.live else {
            return false;
        };
        let (w, h) = live.window.physical_size();
        if w == 0 || h == 0 {
            return false;
        }
        let (scale, logical) = (live.window.scale_factor(), live.window.logical_size());
        live.draw(transparent_black(), |canvas| {
            let available = AvailSize::definite(logical.width, logical.height);
            let frame = AppFrame::layout(build(logical), available, scale, ts);
            frame.paint(canvas, &mut |_, _, _| {});
        })
    }
}

// ---- 2. Tooltips ----------------------------------------------------------------------------

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct TooltipTarget {
    pub(crate) window: WindowId,
    pub(crate) hit: HitId,
}

pub(crate) struct TooltipProbe<'a> {
    pub(crate) anchor: OverlayAnchor,
    pub(crate) pointer: Option<PointerState>,
    pub(crate) frame: &'a AppFrame,
}

/// A tooltip seen under the pointer: its text, owner's rect on screen, and where it entered.
#[derive(Clone)]
struct Snap {
    key: TooltipTarget,
    text: SharedStr,
    owner: ScreenRect,
    entered: ScreenPoint,
}

/// Where the tooltip is: none, waiting, showing, or put away until the pointer moves on.
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
        key: TooltipTarget,
    },
}

impl State {
    fn key(&self) -> Option<TooltipTarget> {
        match self {
            Self::Idle => None,
            Self::Waiting { snap } | Self::Showing { snap } => Some(snap.key),
            Self::Suppressed { key } => Some(*key),
        }
    }
}

/// What happened: the pointer is over an owner, or none, or it's gone; a press, Ctrl, a timer.
enum Report {
    Owner(Snap),
    NoneIn(WindowId),
    Gone(TooltipTarget),
    Suppress(WindowId),
    Dismiss,
    Timer(TooltipTarget),
}

fn transition_state(state: State, report: Report) -> State {
    match (state, report) {
        // Still waiting on the same one, its text and rect brought up to date.
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
        (State::Waiting { snap }, Report::Timer(key)) if snap.key == key => State::Showing { snap },
        (state, _) => state,
    }
}

/// A tooltip `size` with its shadow, centred over `entered` above it (or below), on `work`.
fn place(entered: ScreenPoint, work: ScreenRect, scale: f32, size: Size) -> ScreenRect {
    let phys = |v: f32| (v * scale).round() as i32;
    let (w, h) = (phys(size.width), phys(size.height));
    let gap = phys(POINTER_OFFSET);
    let left = (entered.x - w / 2).clamp(work.left, (work.right - w).max(work.left));
    let above = entered.y - gap - h;
    let top = if above < work.top {
        entered.y + gap
    } else {
        above
    };
    let side = phys(SHADOW_SIDE);
    ScreenRect {
        left: left - side,
        top: top - phys(SHADOW_TOP),
        right: left + w + side,
        bottom: top + h + phys(SHADOW_BOTTOM),
    }
}

#[derive(Default)]
pub(crate) struct Tooltips {
    overlay: OverlaySurface,
    fade: AnimTracks<()>,
    text: SharedStr,
    dirty: bool,
    state: State,
    shown: Option<(TooltipTarget, ScreenRect)>,
    closed_at: Option<Instant>,
    /// Only Ctrl is held: releasing it puts the tooltip away.
    control_only: bool,
    /// The tooltip waiting to show, and when it shows.
    pub(crate) opening: Option<(TooltipTarget, Instant)>,
    /// The system's hover time, as of the last look at the pointer.
    hover_time: Duration,
}

impl Tooltips {
    pub(crate) fn on_mouse_down(&mut self, window: WindowId) {
        self.apply_report(Report::Suppress(window));
    }

    pub(crate) fn on_key_down(&mut self) {
        self.control_only = false;
    }

    pub(crate) fn on_modifiers_changed(&mut self, mods: Modifiers) {
        let was = std::mem::replace(&mut self.control_only, mods.ctrl_only());
        if was && mods.none() {
            self.apply_report(Report::Dismiss);
        }
    }

    pub(crate) fn window_deactivated(&mut self, window: WindowId) {
        self.apply_report(Report::Suppress(window));
    }

    pub(crate) fn window_closed(&mut self, window: WindowId) {
        if let Some(key) = self.state.key().filter(|k| k.window == window) {
            self.apply_report(Report::Gone(key));
        }
    }

    pub(crate) fn on_mouse_leave(&mut self, window: WindowId) {
        self.apply_report(Report::NoneIn(window));
    }

    /// Looks at what's under a window's pointer, and whether the tooltip's owner went or moved.
    pub(crate) fn evaluate(&mut self, probe: TooltipProbe<'_>, shell: &Shell) {
        self.hover_time = shell.settings.mouse_hover_time;
        let TooltipProbe {
            anchor:
                OverlayAnchor {
                    window,
                    origin,
                    scale,
                },
            pointer,
            frame,
        } = probe;
        let gone = self.state.key().filter(|k| k.window == window).filter(|k| {
            frame.rect_of(k.hit).is_none_or(|r| {
                let moved = |&(shown, owner): &(TooltipTarget, ScreenRect)| {
                    shown == *k && owner != r.to_screen(scale, origin)
                };
                self.shown.as_ref().is_some_and(moved)
            })
        });
        let observed = match pointer {
            Some(ptr) if ptr.pressed => None,
            Some(ptr) => Some(match frame.tooltip_at(ptr.pos) {
                Some((hit, tip, rect)) => Report::Owner(Snap {
                    key: TooltipTarget { window, hit },
                    text: tip,
                    owner: rect.to_screen(scale, origin),
                    entered: cursor_screen_pos(),
                }),
                None => Report::NoneIn(window),
            }),
            None => Some(Report::NoneIn(window)),
        };
        if let Some(key) = gone {
            self.transition(Report::Gone(key));
        }
        if let Some(observed) = observed {
            self.transition(observed);
        }
        self.sync(shell);
    }

    /// The waiting tooltip's time came: it shows if the pointer is still on its owner.
    pub(crate) fn on_show_timer(
        &mut self,
        target: TooltipTarget,
        probe: Option<TooltipProbe<'_>>,
        shell: &Shell,
    ) {
        match probe {
            Some(probe) => self.evaluate(probe, shell),
            None => {
                if let Some(key) = self.state.key().filter(|k| k.window == target.window) {
                    self.transition(Report::Gone(key));
                }
            }
        }
        self.transition(Report::Timer(target));
        self.sync(shell);
    }

    fn transition(&mut self, report: Report) {
        self.state = transition_state(std::mem::take(&mut self.state), report);
    }

    fn apply_report(&mut self, report: Report) {
        self.transition(report);
        self.sync_hide_and_timer();
    }

    /// Takes down a tooltip no longer showing, and times the waiting one (sooner after a close).
    fn sync_hide_and_timer(&mut self) {
        let (showing, waiting) = match &self.state {
            State::Showing { snap } => (Some(snap.key), None),
            State::Waiting { snap } => (None, Some(snap.key)),
            State::Idle | State::Suppressed { .. } => (None, None),
        };
        if self.shown.is_some_and(|(key, _)| showing != Some(key)) {
            self.shown = None;
            self.closed_at = Some(Instant::now());
            self.dirty = true;
        }
        self.opening = match waiting {
            Some(key) if self.opening.is_some_and(|(k, _)| k == key) => self.opening,
            Some(key) => {
                let reshow = self
                    .closed_at
                    .is_some_and(|at| at.elapsed() < RESHOW_WINDOW);
                let factor = if reshow {
                    RESHOW_DELAY_FACTOR
                } else {
                    SHOW_DELAY_FACTOR
                };
                Some((key, Instant::now() + self.hover_time.mul_f32(factor)))
            }
            None => None,
        };
    }

    fn sync(&mut self, shell: &Shell) {
        self.sync_hide_and_timer();
        let State::Showing { snap } = &self.state else {
            return;
        };
        if self.shown.is_some() {
            return;
        }
        let display = WindowsDisplay::from_point(snap.entered);
        let light = Palette::resolve(false, None, None);
        let card = tooltip_card(light, snap.text.clone());
        let measured = AppFrame::measure(
            card,
            AvailSize::MAX,
            display.scale_factor(),
            &shell.text_system,
        );
        let rect = place(
            snap.entered,
            display.work_area(),
            display.scale_factor(),
            measured,
        );
        self.text = snap.text.clone();
        self.shown = Some((snap.key, snap.owner));
        self.dirty = true;
        self.overlay.show(&shell.gpu, rect);
    }

    pub(crate) fn paint(&mut self, now: Instant, shell: &Shell) -> bool {
        if !self.dirty {
            return false;
        }
        let (p, ts) = (shell.palette, &shell.text_system);
        let reduce_motion = shell.settings.reduce_motion;
        let visible = self.shown.is_some();
        let text = self.text.clone();
        let fade = &mut self.fade;
        let mut animating = false;
        let needs_retry = self.overlay.paint_scene(ts, |_| {
            fade.begin_frame(now, reduce_motion);
            let opacity = fade.progress((), visible, FADE_CURVE).value;
            animating = fade.end_frame();
            let card = box_()
                .absolute()
                .left(SHADOW_SIDE)
                .top(SHADOW_TOP)
                .child(tooltip_card(p, text));
            box_().size_full().opacity(opacity).child(card)
        });
        if !visible && !animating && !needs_retry {
            self.overlay.hide();
        }
        self.dirty = animating || needs_retry;
        self.dirty
    }
}

fn tooltip_card(p: Palette, content: SharedStr) -> AppNode {
    let label = box_()
        .px(PADDING_X)
        .pt(PADDING_TOP)
        .pb(PADDING_BOTTOM)
        .font_family(SMALL_FONT)
        .text_size(FONT_SIZE)
        .line_height(LINE_HEIGHT)
        .text_color(p.text_primary)
        .child(text(content));
    let face = box_()
        .overflow_hidden()
        .child(box_().absolute().inset_0().bg(p.acrylic_luminosity))
        .child(label);
    card(
        p,
        p.flyout_stroke,
        p.acrylic_fallback,
        CONTROL_RADIUS,
        ELEVATION * 2.,
        face,
    )
    .max_w(MAX_WIDTH)
}

// ---- 3. Key tips ----------------------------------------------------------------------------

#[derive(Clone, Copy)]
pub(crate) enum Placement {
    Below,
    Left,
}

#[derive(Clone, PartialEq)]
pub(crate) struct KeyTip {
    pub(crate) key: char,
    pub(crate) bounds: Rect,
}

pub(crate) fn layout_tip(ts: &TextSystem, key: char, anchor: Rect, placement: Placement) -> KeyTip {
    let mut buf = [0u8; 4];
    let style = TextStyle::regular(KEY_TIP_FONT, FONT_SIZE);
    let width = ts.face(&style).measure(key.encode_utf8(&mut buf)).ceil();
    let pad = 2. * KEY_TIP_PADDING;
    let fill = size(width + pad, LINE_HEIGHT + pad);
    let (outer_w, outer_h) = (
        fill.width + 2. * KEY_TIP_MARGIN,
        fill.height + 2. * KEY_TIP_MARGIN,
    );
    let (x, y) = match placement {
        Placement::Below => (
            round_half_up(anchor.center().x - outer_w / 2.),
            anchor.bottom(),
        ),
        Placement::Left => (
            anchor.left() - outer_w,
            round_half_up(anchor.top() + (anchor.size.height - outer_h) / 2.),
        ),
    };
    let origin = point(x + KEY_TIP_MARGIN, y + KEY_TIP_MARGIN);
    KeyTip {
        key,
        bounds: Rect::new(origin, fill),
    }
}

/// The key tips showing over one window, in one overlay spanning them all.
#[derive(Default)]
pub(crate) struct KeyTips {
    overlay: OverlaySurface,
    owner: Option<WindowId>,
    rect: ScreenRect,
    /// Each tip, from the overlay's top left.
    tips: Vec<KeyTip>,
    dirty: bool,
}

impl KeyTips {
    pub(crate) fn hide(&mut self) {
        self.owner = None;
        self.rect = ScreenRect::default();
        self.tips.clear();
        self.dirty = false;
        self.overlay.hide();
    }

    pub(crate) fn hide_owner(&mut self, owner: WindowId) {
        if self.owner == Some(owner) {
            self.hide();
        }
    }

    /// Shows `tips` over `anchor`'s window (hiding them if none), redrawing only on a change.
    pub(crate) fn sync(&mut self, anchor: OverlayAnchor, mut tips: Vec<KeyTip>, shell: &Shell) {
        let Some(union) = tips.iter().map(|t| t.bounds).reduce(Rect::union) else {
            self.hide_owner(anchor.window);
            return;
        };
        for tip in &mut tips {
            tip.bounds.origin -= union.origin;
        }
        let rect = union.to_screen(anchor.scale, anchor.origin);
        if self.owner == Some(anchor.window) && self.rect == rect && self.tips == tips {
            return;
        }
        (self.tips, self.owner, self.rect, self.dirty) = (tips, Some(anchor.window), rect, true);
        self.overlay.show(&shell.gpu, rect);
    }

    pub(crate) fn paint(&mut self, shell: &Shell) -> bool {
        if !std::mem::take(&mut self.dirty) || self.owner.is_none() || self.tips.is_empty() {
            return false;
        }
        let (p, tips) = (shell.palette, &self.tips);
        self.dirty = self.overlay.paint_scene(&shell.text_system, |_| {
            let tip = |tip: &KeyTip| {
                box_()
                    .absolute()
                    .left(tip.bounds.origin.x)
                    .top(tip.bounds.origin.y)
                    .w(tip.bounds.size.width)
                    .h(tip.bounds.size.height)
                    .bg(p.keytip_fill)
                    .pl(KEY_TIP_PADDING)
                    .pt(KEY_TIP_PADDING)
                    .font_family(KEY_TIP_FONT)
                    .text_size(FONT_SIZE)
                    .line_height(LINE_HEIGHT)
                    .text_color(p.keytip_text)
                    .nowrap()
                    .child(text(tip.key.to_string()))
            };
            box_().size_full().children(tips.iter().map(tip))
        });
        self.dirty
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(window: WindowId, hit: HitId, label: &'static str) -> Snap {
        Snap {
            key: TooltipTarget { window, hit },
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
        use crate::ui::TabHit;
        let w1 = WindowId(1);
        let w2 = WindowId(2);
        let new_tab = HitId::Tab(TabHit::New);
        let k1 = TooltipTarget {
            window: w1,
            hit: new_tab,
        };
        let k2 = TooltipTarget {
            window: w1,
            hit: HitId::ThemeToggle,
        };

        let s = transition_state(State::Idle, Report::Owner(snap(w1, new_tab, "One")));
        assert!(matches!(s, State::Waiting { .. }));

        let s = transition_state(s, Report::Timer(k2));
        assert!(matches!(s, State::Waiting { .. }));
        let s = transition_state(s, Report::Timer(k1));
        assert!(matches!(s, State::Showing { .. }));

        let s = transition_state(s, Report::Dismiss);
        assert!(matches!(s, State::Suppressed { .. }));
        let s = transition_state(s, Report::Owner(snap(w1, new_tab, "One")));
        assert!(matches!(s, State::Suppressed { .. }));

        let s = transition_state(s, Report::NoneIn(w2));
        assert!(matches!(s, State::Suppressed { .. }));
        let s = transition_state(s, Report::NoneIn(w1));
        assert!(matches!(s, State::Idle));

        let s = transition_state(s, Report::Owner(snap(w1, new_tab, "One")));
        let s = transition_state(s, Report::Owner(snap(w1, HitId::ThemeToggle, "Two")));
        assert!(matches!(s, State::Waiting { ref snap } if snap.key == k2));

        let s = transition_state(s, Report::Suppress(w1));
        assert!(matches!(s, State::Suppressed { .. }));
    }
}
