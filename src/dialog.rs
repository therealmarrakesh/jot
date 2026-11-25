//! The window's dialogs: go to line, its out-of-range notice, and the prompts file work asks.
//!
//! One dialog shows at a time, over a smoke backdrop; more wait in a queue. A dialog grows in
//! from slightly large and fades, and on closing grows and fades away, drawn (inert) until it's
//! gone. Keys move focus around its field and buttons, press them, or cancel.

use std::collections::VecDeque;
use std::time::Duration;

use crate::editor::{Editor, FieldOptions};
use crate::files::{Prompt, PromptId};
use crate::gfx::TextSystem;
use crate::platform::{CursorStyle, Key, KeyEvent};
use crate::theme::{BODY_LINE, BODY_SIZE, BORDER, OVERLAY_RADIUS, Palette, TEXT_FONT};
use crate::types::SharedStr;
use crate::ui::{
    AnimStatus, AppNode, AppUi, Curve, DialogHit, Easing, EditorSlot, Focus, HitId, TabId,
    Transition, box_, row, text,
};
use crate::widgets::{button, card, text_field};

// How a dialog comes and goes.
const OPEN_SCALE: f32 = 1.05;
const SHOW_CURVE: Curve = Curve::new(Duration::from_millis(250), Easing::DECELERATE);
const HIDE_CURVE: Curve = Curve::new(Duration::from_millis(167), Easing::DECELERATE);
const FADE_CURVE: Curve = Curve::new(Duration::from_millis(83), Easing::Linear);

// Its card.
const TITLE_FONT: &str = "Segoe UI Variable Display";
const DIALOG_Z: f32 = 128.;
const MIN_WIDTH: f32 = 320.;
const MAX_WIDTH: f32 = 548.;
const PADDING: f32 = 24.;
const TITLE_SIZE: f32 = 20.;
const TITLE_LINE: f32 = 28.;
const TITLE_GAP: f32 = 12.;
const HEADER_GAP: f32 = 8.;
const BUTTON_GAP: f32 = 8.;
const GO_TO_WIDTH: f32 = 312.;
/// Lifts the body text a pixel onto the baseline Notepad's has.
const BODY_BASELINE_SNAP: f32 = -1.;

// What the go-to dialogs say.
const GO_TO_TITLE: &str = "Go to line";
const GO_TO_BUTTONS: &[&str] = &["Go to", "Cancel"];
const OUT_OF_RANGE_MESSAGE: &str = "The line number is beyond the total number of lines";
const OK_BUTTONS: &[&str] = &["OK"];

// ---- 1. Dialogs and their keys --------------------------------------------------------------

/// What a key in a dialog does.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum DialogKey {
    /// Not the dialog's: the system's (Alt+F4, Alt+Space).
    PassThrough,
    Focus(Focus),
    PressButton(usize),
    Cancel,
    /// An editing key for the go-to field.
    EditGoToLine,
    Consumed,
}

#[derive(Clone)]
pub(crate) enum DialogKind {
    GoTo,
    LineOutOfRange,
    Prompt(Prompt),
}

impl DialogKind {
    fn prompt(&self) -> Option<&Prompt> {
        match self {
            Self::Prompt(prompt) => Some(prompt),
            Self::GoTo | Self::LineOutOfRange => None,
        }
    }

    fn prompt_id(&self) -> Option<PromptId> {
        self.prompt()?.reply.map(|(id, _)| id)
    }

    fn button_labels(&self) -> Vec<&'static str> {
        match self {
            Self::GoTo => GO_TO_BUTTONS.to_vec(),
            Self::LineOutOfRange => OK_BUTTONS.to_vec(),
            Self::Prompt(prompt) => prompt.choices.iter().map(|&(label, _)| label).collect(),
        }
    }
}

/// A dialog on screen: coming in, showing, or going away; and how big and opaque it is now.
struct ActiveDialog {
    kind: DialogKind,
    transition: Transition,
    scale: f32,
    opacity: f32,
}

impl ActiveDialog {
    /// Samples this frame's scale and opacity; whether a closing dialog is gone.
    fn sample(&mut self, ui: &mut AppUi) -> bool {
        let t = self.transition;
        let closing = t.is_closing();
        let shown = t.progress(ui, 0, true, SHOW_CURVE).value;
        let faded_in = t.progress(ui, 1, true, FADE_CURVE).value;
        let hidden = t.progress(ui, 2, closing, HIDE_CURVE).value;
        let fade_out = t.progress(ui, 3, closing, FADE_CURVE);
        let growth = OPEN_SCALE - 1.;
        self.scale = 1. + growth * (1. - shown) + growth * hidden;
        self.opacity = faded_in * (1. - fade_out.value);
        closing && fade_out.status == AnimStatus::Finished
    }
}

// ---- 2. The queue ---------------------------------------------------------------------------

pub(crate) struct Dialogs {
    /// What's on screen, the newest last: at most one open, the rest closing.
    open: Vec<ActiveDialog>,
    pending: VecDeque<DialogKind>,
    /// The go-to dialog's field, which takes digits only.
    pub(crate) line: Editor,
}

impl Dialogs {
    pub(crate) fn new() -> Self {
        let line = Editor::field(FieldOptions {
            slot: EditorSlot::GoToLine,
            placeholder: SharedStr::default(),
            validate: Some(|t| t.chars().all(|c| c.is_ascii_digit())),
        });
        Self {
            open: Vec::new(),
            pending: VecDeque::new(),
            line,
        }
    }

    fn active(&self) -> Option<&ActiveDialog> {
        self.open.iter().rev().find(|d| d.transition.is_open())
    }

    pub(crate) fn is_open(&self) -> bool {
        self.active().is_some()
    }

    /// Shows `kind`, or queues it behind the open one; where focus goes if it shows.
    pub(crate) fn enqueue(&mut self, kind: DialogKind) -> Option<Focus> {
        if self.is_open() {
            self.pending.push_back(kind);
            return None;
        }
        Some(self.show_now(kind))
    }

    /// Shows `kind` in place of the open one; where focus goes (the go-to field, or the first
    /// button).
    pub(crate) fn show_now(&mut self, kind: DialogKind) -> Focus {
        self.hide();
        let focus = match kind {
            DialogKind::GoTo => Focus::Editor(EditorSlot::GoToLine),
            _ => Focus::DialogButton(0),
        };
        self.open.push(ActiveDialog {
            kind,
            transition: Transition::open_new(),
            scale: OPEN_SCALE,
            opacity: 0.,
        });
        focus
    }

    /// Shows the go-to dialog, its field holding `line` (or what was typed before) selected;
    /// where focus goes.
    pub(crate) fn show_go_to(&mut self, line: Option<usize>, ts: &TextSystem) -> Focus {
        let focus = self.show_now(DialogKind::GoTo);
        if let Some(line) = line {
            self.line.set_value(&line.to_string(), ts);
        }
        self.line.select_all();
        focus
    }

    /// The line (from zero) the go-to field asks for, if the document's `lines` have it.
    pub(crate) fn go_to_target(&self, lines: usize) -> Option<usize> {
        let line = self.line.model().text().to_string().parse::<usize>().ok()?;
        (1..=lines).contains(&line).then(|| line - 1)
    }

    /// The open dialog, which stays open until it's dismissed.
    pub(crate) fn take_active(&self) -> Option<DialogKind> {
        self.active().map(|d| d.kind.clone())
    }

    /// Closes the open dialog and shows the next queued one; where focus goes if one shows.
    pub(crate) fn dismiss(&mut self) -> Option<Focus> {
        self.hide();
        let next = self.pending.pop_front()?;
        Some(self.show_now(next))
    }

    pub(crate) fn hide(&mut self) {
        for d in &mut self.open {
            d.transition.close();
        }
    }

    /// Closes and drops the prompts about tab `id`.
    pub(crate) fn close_for_tab(&mut self, id: TabId) {
        let about = |kind: &DialogKind| kind.prompt().and_then(Prompt::tab) == Some(id);
        for d in &mut self.open {
            if d.transition.is_open() && about(&d.kind) {
                d.transition.close();
            }
        }
        self.pending.retain(|kind| !about(kind));
    }

    /// Closes the open prompt and drops the queued ones: the ids of all of them, which won't be
    /// answered now.
    pub(crate) fn drain_prompt_ids(&mut self) -> Vec<PromptId> {
        let mut ids = Vec::new();
        for d in &mut self.open {
            if d.transition.is_open()
                && let Some(id) = d.kind.prompt_id()
            {
                ids.push(id);
                d.transition.close();
            }
        }
        ids.extend(self.pending.drain(..).filter_map(|kind| kind.prompt_id()));
        ids
    }

    /// Enter presses the focused button (or the first), Space a focused button, Escape
    /// cancels, Tab and Shift+Tab move around the field and buttons; everything else stays in
    /// the dialog, editing the go-to field if that has focus.
    pub(crate) fn on_key(&self, ev: KeyEvent, focus: Focus) -> DialogKey {
        let m = ev.modifiers;
        if m.alt && !m.ctrl && !m.win && matches!(ev.key, Key::F(4) | Key::Space) {
            return DialogKey::PassThrough;
        }
        let shift_only = m.shift && !m.ctrl && !m.alt && !m.win;
        let step = |delta| {
            self.focus_step(focus, delta)
                .map_or(DialogKey::Consumed, DialogKey::Focus)
        };
        match (ev.key, focus) {
            (Key::Enter, Focus::DialogButton(ix)) if m.none() => DialogKey::PressButton(ix),
            (Key::Enter, _) if m.none() => DialogKey::PressButton(0),
            (Key::Space, Focus::DialogButton(ix)) if m.none() => DialogKey::PressButton(ix),
            (Key::Escape, _) if m.none() => DialogKey::Cancel,
            (Key::Tab, _) if m.none() => step(1),
            (Key::Tab, _) if shift_only => step(-1),
            (_, Focus::Editor(EditorSlot::GoToLine)) => DialogKey::EditGoToLine,
            _ => DialogKey::Consumed,
        }
    }

    /// Focus `delta` steps from `current` around the open dialog's field and buttons (from the
    /// first or last, if it's elsewhere).
    fn focus_step(&self, current: Focus, delta: isize) -> Option<Focus> {
        let kind = &self.active()?.kind;
        let field = matches!(kind, DialogKind::GoTo).then_some(Focus::Editor(EditorSlot::GoToLine));
        let buttons = (0..kind.button_labels().len()).map(Focus::DialogButton);
        let ring: Vec<Focus> = field.into_iter().chain(buttons).collect();
        let next = match ring.iter().position(|&f| f == current) {
            Some(ix) => (ix as isize + delta).rem_euclid(ring.len() as isize) as usize,
            None if delta > 0 => 0,
            None => ring.len().checked_sub(1)?,
        };
        ring.get(next).copied()
    }
}

// ---- 3. Drawing -----------------------------------------------------------------------------

impl Dialogs {
    /// Every dialog still on screen, each over its backdrop; drops the ones that have gone.
    pub(crate) fn render(&mut self, focus: Focus, p: Palette, ui: &mut AppUi) -> Option<AppNode> {
        self.open.retain_mut(|d| !d.sample(ui));
        if self.open.is_empty() {
            return None;
        }
        let layers: Vec<AppNode> = self
            .open
            .iter()
            .map(|d| self.render_layer(d, focus, p, ui))
            .collect();
        Some(box_().absolute().inset_0().children(layers))
    }

    /// A dialog's backdrop (which takes the pointer while it's open) and card: its title and
    /// body over its buttons, the first accented unless another has focus.
    fn render_layer(&self, d: &ActiveDialog, focus: Focus, p: Palette, ui: &mut AppUi) -> AppNode {
        let focused = match focus {
            Focus::DialogButton(ix) => Some(ix),
            _ => None,
        };
        let closing = d.transition.is_closing();
        let labels = d.kind.button_labels();
        let lone = labels.len() == 1;
        let buttons = labels.into_iter().enumerate().map(|(ix, label)| {
            let accented = ix == 0 && focused.is_none_or(|f| f == ix);
            let has_focus = !closing && focused == Some(ix);
            let hit = HitId::Dialog(DialogHit::Button(ix));
            let button = button(p, p.command_surface, hit, label, accented, has_focus, ui);
            box_().flex_1().min_w_0().child(button)
        });
        // A single button sits in the right half.
        let buttons = row()
            .p(PADDING)
            .gap(BUTTON_GAP)
            .when(lone, |this| this.child(box_().flex_1().min_w_0()))
            .children(buttons.collect::<Vec<_>>());
        let (width, title, body) = match &d.kind {
            DialogKind::GoTo => {
                let field_focused = focus == Focus::Editor(EditorSlot::GoToLine);
                let field = text_field(p, p.content_surface, &self.line, field_focused, false, ui);
                let label = message("Line number".into()).h(BODY_LINE).mb(HEADER_GAP);
                let width = GO_TO_WIDTH + 2. * PADDING + 2. * BORDER;
                (
                    Some(width),
                    GO_TO_TITLE.into(),
                    box_().child(label).child(field),
                )
            }
            DialogKind::LineOutOfRange => (
                None,
                GO_TO_TITLE.into(),
                message(OUT_OF_RANGE_MESSAGE.into()),
            ),
            DialogKind::Prompt(prompt) => {
                (None, prompt.title.clone(), message(prompt.message.clone()))
            }
        };
        let title = box_()
            .mb(TITLE_GAP)
            .font_family(TITLE_FONT)
            .font_semibold()
            .text_size(TITLE_SIZE)
            .line_height(TITLE_LINE)
            .line_clamp(2)
            .child(text(title));
        let content = box_()
            .p(PADDING)
            .rounded_t(OVERLAY_RADIUS - BORDER)
            .bg(p.content_surface.panel)
            .border_b(1.)
            .border_color(p.dialog_divider)
            .child(title)
            .child(body);
        let face = box_()
            .font_family(TEXT_FONT)
            .text_size(BODY_SIZE)
            .text_color(p.text_primary)
            .child(content)
            .child(buttons);
        let fill = p.command_surface.panel;
        let card = card(p, p.surface_stroke, fill, OVERLAY_RADIUS, DIALOG_Z, face)
            .hit(HitId::Dialog(DialogHit::Card))
            .when_some(width, |this, w| this.w(w).max_w_full())
            .when(width.is_none(), |this| {
                this.min_w(MIN_WIDTH).max_w(MAX_WIDTH)
            })
            .scale(d.scale, d.scale)
            .when(closing, AppNode::inert);
        row()
            .absolute()
            .inset_0()
            .items_center()
            .justify_center()
            .bg(p.smoke)
            .opacity(d.opacity)
            .when(!closing, |this| {
                this.hit(HitId::Dialog(DialogHit::Backdrop))
                    .cursor(CursorStyle::Arrow)
            })
            .child(card)
    }
}

/// A dialog's message, on the body's baseline.
fn message(msg: SharedStr) -> AppNode {
    box_()
        .top(BODY_BASELINE_SNAP)
        .line_height(BODY_LINE)
        .child(text(msg))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::Modifiers;

    fn key(k: Key, m: Modifiers) -> KeyEvent {
        KeyEvent {
            key: k,
            modifiers: m,
        }
    }

    #[test]
    fn dialog_key_routing_handles_focus_ring_buttons_go_to_and_system_chords() {
        let mut dialogs = Dialogs::new();
        let initial = dialogs.show_now(DialogKind::GoTo);
        assert_eq!(initial, Focus::Editor(EditorSlot::GoToLine));

        let plain = Modifiers::default();
        let shift = Modifiers {
            shift: true,
            ..Default::default()
        };
        let alt = Modifiers {
            alt: true,
            ..Default::default()
        };

        assert_eq!(
            dialogs.on_key(key(Key::F(4), alt), initial),
            DialogKey::PassThrough
        );
        assert_eq!(
            dialogs.on_key(key(Key::Space, alt), initial),
            DialogKey::PassThrough
        );
        assert_eq!(
            dialogs.on_key(key(Key::Enter, plain), initial),
            DialogKey::PressButton(0)
        );
        assert_eq!(
            dialogs.on_key(key(Key::Escape, plain), initial),
            DialogKey::Cancel
        );
        assert_eq!(
            dialogs.on_key(key(Key::Char('a'), plain), initial),
            DialogKey::EditGoToLine
        );
        assert_eq!(
            dialogs.on_key(key(Key::Tab, plain), initial),
            DialogKey::Focus(Focus::DialogButton(0))
        );
        assert_eq!(
            dialogs.on_key(key(Key::Tab, plain), Focus::DialogButton(0)),
            DialogKey::Focus(Focus::DialogButton(1))
        );
        assert_eq!(
            dialogs.on_key(key(Key::Tab, shift), Focus::DialogButton(0)),
            DialogKey::Focus(Focus::Editor(EditorSlot::GoToLine))
        );
        assert_eq!(
            dialogs.on_key(key(Key::Space, plain), Focus::DialogButton(1)),
            DialogKey::PressButton(1)
        );
        assert_eq!(
            dialogs.on_key(key(Key::Char('x'), plain), Focus::DialogButton(0)),
            DialogKey::Consumed
        );
    }
}
