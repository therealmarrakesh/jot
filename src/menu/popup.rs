use std::time::Duration;

use crate::geom::Point;
use crate::ids::{AnimKey, EditorSlot, HitId, MenuKind, UiState};
use crate::ui::{Anchor, AnimStatus, Curve, Easing, Transition};

use super::MENU_BAR_HEIGHT;
use super::bar::FocusRequest;
use super::model::{CommandButton, PADDING_Y, Panel, cycle};

const OPEN_CURVE: Curve = Curve::new(Duration::from_millis(250), Easing::DECELERATE);
const FADE_CURVE: Curve = Curve::new(Duration::from_millis(83), Easing::Linear);
const FLYOUT_GAP: f32 = 3.;
pub(super) const COMMAND_ROW: f32 = 59.;
const OVERFLOW_TOP: f32 = 1.;

pub(super) enum PopupKind {
    Bar(MenuKind),
    Context {
        anchor: Point,
        commands: Vec<CommandButton>,
    },
    Flyout {
        target: HitId,
        return_slot: EditorSlot,
    },
}

impl PopupKind {
    pub(super) fn bar_kind(&self) -> Option<MenuKind> {
        match self {
            Self::Bar(kind) => Some(*kind),
            Self::Context { .. } | Self::Flyout { .. } => None,
        }
    }

    pub(super) fn commands(&self) -> &[CommandButton] {
        match self {
            Self::Context { commands, .. } => commands,
            Self::Bar(_) | Self::Flyout { .. } => &[],
        }
    }

    pub(super) fn anchor(&self) -> Anchor<HitId> {
        match self {
            Self::Bar(kind) => Anchor::BelowStart {
                target: HitId::MenuTrigger(*kind),
                gap: 0.,
            },
            Self::Context { anchor, .. } => Anchor::Point(*anchor),
            Self::Flyout { target, .. } => Anchor::AboveOrBelowCenter {
                target: *target,
                gap: FLYOUT_GAP,
                min_top: MENU_BAR_HEIGHT,
            },
        }
    }

    pub(super) fn return_focus(&self) -> FocusRequest {
        match self {
            Self::Flyout { return_slot, .. } => FocusRequest::Editor(*return_slot),
            Self::Bar(_) | Self::Context { .. } => FocusRequest::RestoreEditor,
        }
    }
}

pub(super) struct Motion {
    pub(super) transition: Transition,
    fade_in: bool,
    pub(super) unroll: f32,
    pub(super) opacity: f32,
}

impl Motion {
    pub(super) fn new(fade_in: bool) -> Self {
        Self {
            transition: Transition::open_new(),
            fade_in,
            unroll: 0.,
            opacity: 1.,
        }
    }

    #[inline]
    pub(super) fn serial(&self) -> usize {
        self.transition.serial()
    }

    #[inline]
    pub(super) fn is_closing(&self) -> bool {
        self.transition.is_closing()
    }

    pub(super) fn close(&mut self) {
        self.transition.close();
    }

    pub(super) fn sample(&mut self, ui: &mut UiState) -> bool {
        let serial = self.transition.serial();
        let closing = self.transition.is_closing();
        if !closing {
            self.unroll = ui
                .progress(AnimKey::Transition { serial, track: 0 }, true, OPEN_CURVE)
                .value;
        }
        let appear = if self.fade_in {
            ui.progress(AnimKey::Transition { serial, track: 1 }, true, FADE_CURVE)
                .value
        } else {
            1.
        };
        let fade = ui.progress(AnimKey::Transition { serial, track: 2 }, closing, FADE_CURVE);
        self.opacity = (1. - fade.value) * appear;
        closing && fade.status == AnimStatus::Finished
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(super) enum PopupFocus {
    #[default]
    None,
    Command(usize),
    Row(usize),
    SubRow(usize),
}

pub(super) struct OpenSubmenu {
    pub(super) ix: usize,
    pub(super) panel: Panel,
    pub(super) motion: Motion,
}

pub(super) struct OpenMenu {
    pub(super) kind: PopupKind,
    pub(super) panel: Panel,
    pub(super) submenu: Option<OpenSubmenu>,
    leaving_submenu: Option<OpenSubmenu>,
    pub(super) motion: Motion,
    pub(super) focus: PopupFocus,
}

impl OpenMenu {
    pub(super) fn new(kind: PopupKind, panel: Panel, focus: PopupFocus) -> Self {
        let fade_in = !kind.commands().is_empty();
        Self {
            kind,
            panel,
            submenu: None,
            leaving_submenu: None,
            motion: Motion::new(fade_in),
            focus,
        }
    }

    pub(super) fn is_closing(&self) -> bool {
        self.motion.is_closing()
    }

    pub(super) fn visible_submenu(&self) -> Option<&OpenSubmenu> {
        self.submenu.as_ref().or(self.leaving_submenu.as_ref())
    }

    pub(super) fn focused_submenu(&self) -> Option<&OpenSubmenu> {
        matches!(self.focus, PopupFocus::SubRow(_))
            .then_some(self.submenu.as_ref())
            .flatten()
    }

    pub(super) fn in_submenu(&self) -> bool {
        self.focused_submenu().is_some()
    }

    pub(super) fn set_submenu(&mut self, sub: OpenSubmenu) {
        self.submenu = Some(sub);
        self.leaving_submenu = None;
    }

    pub(super) fn close_submenu(&mut self) -> bool {
        let Some(mut sub) = self.submenu.take() else {
            return false;
        };
        if matches!(self.focus, PopupFocus::SubRow(_)) {
            self.focus = PopupFocus::Row(sub.ix);
        }
        sub.motion.close();
        self.leaving_submenu = Some(sub);
        true
    }

    pub(super) fn step(&mut self, delta: isize) {
        if let PopupFocus::SubRow(ix) = self.focus
            && let Some(sub) = &self.submenu
        {
            if let Some(next) = sub.panel.step(Some(ix), delta) {
                self.focus = PopupFocus::SubRow(next);
            }
            return;
        }
        let cmds = self.kind.commands();
        if cmds.is_empty() {
            let current = match self.focus {
                PopupFocus::Row(ix) => Some(ix),
                _ => None,
            };
            if let Some(next) = self.panel.step(current, delta) {
                self.focus = PopupFocus::Row(next);
            }
            return;
        }
        let commands = cmds.len();
        let current = match self.focus {
            PopupFocus::Command(ix) => Some(ix),
            PopupFocus::Row(ix) => Some(commands + ix),
            PopupFocus::None | PopupFocus::SubRow(_) => None,
        };
        let count = commands + self.panel.entries.len();
        if let Some(at) = cycle(current, count, delta, true, |at| {
            if at < commands {
                cmds[at].command.enabled
            } else {
                self.panel.entries[at - commands].selectable()
            }
        }) {
            self.focus = if at < commands {
                PopupFocus::Command(at)
            } else {
                PopupFocus::Row(at - commands)
            };
        }
    }

    pub(super) fn step_command(&mut self, delta: isize) {
        let PopupFocus::Command(ix) = self.focus else {
            return;
        };
        let cmds = self.kind.commands();
        if let Some(next) = cycle(Some(ix), cmds.len(), delta, false, |i| {
            cmds[i].command.enabled
        }) {
            self.focus = PopupFocus::Command(next);
        }
    }

    pub(super) fn focus_edge(&mut self, last: bool) {
        if self.in_submenu() {
            if let Some(sub) = &self.submenu
                && let Some(ix) = sub.panel.edge(last)
            {
                self.focus = PopupFocus::SubRow(ix);
            }
            return;
        }
        let cmds = self.kind.commands();
        if cmds.is_empty() {
            self.focus = self.panel.edge(last).map_or(PopupFocus::None, PopupFocus::Row);
        } else if last {
            self.focus = self
                .panel
                .edge(true)
                .map(PopupFocus::Row)
                .or_else(|| {
                    cmds.iter()
                        .rposition(|b| b.command.enabled)
                        .map(PopupFocus::Command)
                })
                .unwrap_or(PopupFocus::None);
        } else {
            self.focus = cmds
                .iter()
                .position(|b| b.command.enabled)
                .map(PopupFocus::Command)
                .or_else(|| self.panel.edge(false).map(PopupFocus::Row))
                .unwrap_or(PopupFocus::None);
        }
    }

    pub(super) fn sample(&mut self, ui: &mut UiState) -> bool {
        if let Some(sub) = &mut self.submenu {
            sub.motion.sample(ui);
        }
        if self
            .leaving_submenu
            .as_mut()
            .is_some_and(|sub| sub.motion.sample(ui))
        {
            self.leaving_submenu = None;
        }
        self.motion.sample(ui)
    }

    pub(super) fn total_height(&self) -> (f32, f32) {
        let bar = if !self.kind.commands().is_empty() {
            COMMAND_ROW + 1. + OVERFLOW_TOP - PADDING_Y
        } else {
            0.
        };
        (self.panel.height() + bar, bar)
    }
}
