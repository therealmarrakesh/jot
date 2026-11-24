use std::time::Duration;

use crate::geom::Point;
use crate::ids::{AnimKey, EditorSlot, HitId, MenuKind, UiState};
use crate::ui::{Anchor, AnimStatus, Curve, Easing, Transition};

use super::MENU_BAR_HEIGHT;
use super::bar::FocusRequest;
use super::model::{CommandButton, Entry, PADDING_Y, Panel, cycle};

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

pub(super) struct OpenSubmenu {
    pub(super) ix: usize,
    pub(super) panel: Panel,
    pub(super) motion: Motion,
}

pub(super) struct OpenMenu {
    pub(super) kind: PopupKind,
    pub(super) panel: Panel,
    pub(super) submenu: Option<OpenSubmenu>,
    pub(super) motion: Motion,
    pub(super) focused_command: Option<usize>,
}

impl OpenMenu {
    pub(super) fn new(kind: PopupKind, panel: Panel) -> Self {
        let fade_in = !kind.commands().is_empty();
        Self {
            kind,
            panel,
            submenu: None,
            motion: Motion::new(fade_in),
            focused_command: None,
        }
    }

    pub(super) fn is_closing(&self) -> bool {
        self.motion.is_closing()
    }

    pub(super) fn live_submenu(&self) -> Option<&OpenSubmenu> {
        self.submenu.as_ref().filter(|s| !s.motion.is_closing())
    }

    pub(super) fn live_submenu_mut(&mut self) -> Option<&mut OpenSubmenu> {
        self.submenu.as_mut().filter(|s| !s.motion.is_closing())
    }

    pub(super) fn focused_submenu(&self) -> Option<&OpenSubmenu> {
        self.live_submenu().filter(|s| s.panel.focused.is_some())
    }

    pub(super) fn in_submenu(&self) -> bool {
        self.focused_submenu().is_some()
    }

    pub(super) fn step_linear(&mut self, delta: isize) {
        let cmds = self.kind.commands();
        let commands = cmds.len();
        let current = self
            .focused_command
            .or(self.panel.focused.map(|ix| commands + ix));
        let count = commands + self.panel.entries.len();
        if let Some(at) = cycle(current, count, delta, true, |at| {
            if at < commands {
                cmds[at].command.enabled
            } else {
                self.panel.entries[at - commands].selectable()
            }
        }) {
            self.focused_command = (at < commands).then_some(at);
            self.panel.focused = at.checked_sub(commands);
        }
    }

    pub(super) fn step_command(&mut self, delta: isize) {
        let Some(ix) = self.focused_command else {
            return;
        };
        let cmds = self.kind.commands();
        if let Some(ix) = cycle(Some(ix), cmds.len(), delta, false, |ix| {
            cmds[ix].command.enabled
        }) {
            self.focused_command = Some(ix);
        }
    }

    pub(super) fn focus_edge(&mut self, last: bool) {
        if self.in_submenu() {
            if let Some(sub) = self.live_submenu_mut() {
                sub.panel.focus_edge(last);
            }
            return;
        }
        let cmds = self.kind.commands();
        if cmds.is_empty() {
            self.panel.focus_edge(last);
        } else if last {
            self.focused_command = None;
            self.panel.focus_edge(true);
            if self.panel.focused.is_none() {
                self.focused_command = cmds.iter().rposition(|b| b.command.enabled);
            }
        } else {
            self.focused_command = cmds.iter().position(|b| b.command.enabled);
            self.panel.focused = if self.focused_command.is_none() {
                self.panel.entries.iter().position(Entry::selectable)
            } else {
                None
            };
        }
    }

    pub(super) fn sample(&mut self, ui: &mut UiState) -> bool {
        if self.submenu.as_mut().is_some_and(|sub| sub.motion.sample(ui)) {
            self.submenu = None;
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
