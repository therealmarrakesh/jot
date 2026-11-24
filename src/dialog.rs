use std::collections::VecDeque;
use std::time::Duration;

use crate::editor::{Editor, FieldOptions};
use crate::files::{Prompt, PromptId};
use crate::platform::{CursorStyle, Key, KeyEvent};
use crate::theme::{
    BODY_LINE, BODY_SIZE, FIELD_CARET, OVERLAY_RADIUS, Palette, TEXT_FONT, theme_shadow,
};
use crate::types::SharedStr;
use crate::ui::{
    AnimKey, AnimStatus, AppNode, AppUi, Curve, DialogHit, Easing, EditorSlot, Focus, HitId, TabId,
    Transition, box_, row, text,
};
use crate::widgets::{button, text_field};

const TITLE_FONT: &str = "Segoe UI Variable Display";
const OPEN_SCALE: f32 = 1.05;
const DIALOG_Z: f32 = 128.;
const SHOW_CURVE: Curve = Curve::new(Duration::from_millis(250), Easing::DECELERATE);
const HIDE_CURVE: Curve = Curve::new(Duration::from_millis(167), Easing::DECELERATE);
const FADE_CURVE: Curve = Curve::new(Duration::from_millis(83), Easing::Linear);
const MIN_WIDTH: f32 = 320.;
const MAX_WIDTH: f32 = 548.;
const PADDING: f32 = 24.;
const TITLE_SIZE: f32 = 20.;
const TITLE_LINE: f32 = 28.;
const TITLE_GAP: f32 = 12.;
const HEADER_GAP: f32 = 8.;
const BUTTON_GAP: f32 = 8.;
const GO_TO_WIDTH: f32 = 312.;
const GO_TO_TITLE: &str = "Go to line";
const GO_TO_BUTTONS: &[&str] = &["Go to", "Cancel"];
const OUT_OF_RANGE_MESSAGE: &str = "The line number is beyond the total number of lines";
const OK_BUTTONS: &[&str] = &["OK"];
const BORDER: f32 = 1.;
const BODY_BASELINE_SNAP: f32 = -1.;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum DialogKey {
    PassThrough,
    Focus(Focus),
    PressButton(usize),
    Cancel,
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
    fn referenced_tab(&self) -> Option<TabId> {
        match self {
            Self::Prompt(p) => p.tab(),
            Self::GoTo | Self::LineOutOfRange => None,
        }
    }

    fn prompt_id(&self) -> Option<PromptId> {
        match self {
            Self::Prompt(p) => p.reply.map(|(id, _)| id),
            Self::GoTo | Self::LineOutOfRange => None,
        }
    }

    fn button_count(&self) -> usize {
        match self {
            Self::GoTo => GO_TO_BUTTONS.len(),
            Self::LineOutOfRange => OK_BUTTONS.len(),
            Self::Prompt(p) => p.choices.len(),
        }
    }

    fn button_labels(&self) -> Vec<&'static str> {
        match self {
            Self::GoTo => GO_TO_BUTTONS.to_vec(),
            Self::LineOutOfRange => OK_BUTTONS.to_vec(),
            Self::Prompt(p) => p.choices.iter().map(|&(label, _)| label).collect(),
        }
    }

    fn is_go_to(&self) -> bool {
        matches!(self, Self::GoTo)
    }
}

struct ActiveDialog {
    kind: DialogKind,
    transition: Transition,
    scale: f32,
    opacity: f32,
}

impl ActiveDialog {
    fn sample(&mut self, ui: &mut AppUi) -> bool {
        let serial = self.transition.serial();
        let closing = self.transition.is_closing();
        let shown = ui
            .progress(AnimKey::Transition { serial, track: 0 }, true, SHOW_CURVE)
            .value;
        let faded_in = ui
            .progress(AnimKey::Transition { serial, track: 1 }, true, FADE_CURVE)
            .value;
        let hidden = ui
            .progress(AnimKey::Transition { serial, track: 2 }, closing, HIDE_CURVE)
            .value;
        let fade_out = ui.progress(AnimKey::Transition { serial, track: 3 }, closing, FADE_CURVE);
        let growth = OPEN_SCALE - 1.;
        self.scale = 1. + growth * (1. - shown) + growth * hidden;
        self.opacity = faded_in * (1. - fade_out.value);
        closing && fade_out.status == AnimStatus::Finished
    }
}

pub(crate) struct Dialogs {
    open: Vec<ActiveDialog>,
    pending: VecDeque<DialogKind>,
    pub(crate) line: Editor,
}

impl Dialogs {
    pub(crate) fn new() -> Self {
        let line = Editor::field(FieldOptions {
            caret_height: FIELD_CARET,
            validate: Some(|t| t.chars().all(|c| c.is_ascii_digit())),
            ..Default::default()
        });
        Self {
            open: Vec::new(),
            pending: VecDeque::new(),
            line,
        }
    }

    pub(crate) fn is_open(&self) -> bool {
        self.active().is_some()
    }

    fn active(&self) -> Option<&ActiveDialog> {
        self.open.iter().rev().find(|d| d.transition.is_open())
    }

    pub(crate) fn enqueue(&mut self, kind: DialogKind) -> Option<Focus> {
        if self.is_open() {
            self.pending.push_back(kind);
            return None;
        }
        Some(self.show_now(kind))
    }

    pub(crate) fn show_now(&mut self, kind: DialogKind) -> Focus {
        self.hide();
        let initial_focus = if kind.is_go_to() {
            Focus::Editor(EditorSlot::GoToLine)
        } else {
            Focus::DialogButton(0)
        };
        self.open.push(ActiveDialog {
            kind,
            transition: Transition::open_new(),
            scale: OPEN_SCALE,
            opacity: 0.,
        });
        initial_focus
    }

    pub(crate) fn dismiss(&mut self) -> Option<Focus> {
        self.hide();
        self.pending.pop_front().map(|next| self.show_now(next))
    }

    pub(crate) fn hide(&mut self) {
        for d in &mut self.open {
            d.transition.close();
        }
    }

    pub(crate) fn close_for_tab(&mut self, id: TabId) {
        for d in &mut self.open {
            if d.transition.is_open() && d.kind.referenced_tab() == Some(id) {
                d.transition.close();
            }
        }
        self.pending.retain(|d| d.referenced_tab() != Some(id));
    }

    pub(crate) fn drain_prompt_ids(&mut self) -> Vec<PromptId> {
        let mut ids = Vec::new();
        for d in &mut self.open {
            if d.transition.is_open()
                && let Some(pid) = d.kind.prompt_id()
            {
                ids.push(pid);
                d.transition.close();
            }
        }
        for d in self.pending.drain(..) {
            if let Some(pid) = d.prompt_id() {
                ids.push(pid);
            }
        }
        ids
    }

    pub(crate) fn take_active(&mut self) -> Option<DialogKind> {
        let ix = self.open.iter().rposition(|d| d.transition.is_open())?;
        Some(self.open[ix].kind.clone())
    }

    pub(crate) fn on_key(&self, ev: KeyEvent, focus: Focus) -> DialogKey {
        let m = ev.modifiers;
        if m.alt && !m.ctrl && !m.win && matches!(ev.key, Key::F(4) | Key::Space) {
            return DialogKey::PassThrough;
        }
        match (m.ctrl, m.alt, m.shift, m.win, ev.key) {
            (false, false, false, false, Key::Enter) => {
                let btn = match focus {
                    Focus::DialogButton(ix) => ix,
                    _ => 0,
                };
                return DialogKey::PressButton(btn);
            }
            (false, false, false, false, Key::Space) => {
                if let Focus::DialogButton(ix) = focus {
                    return DialogKey::PressButton(ix);
                }
            }
            (false, false, false, false, Key::Escape) => {
                return DialogKey::Cancel;
            }
            (false, false, false, false, Key::Tab) => {
                return self
                    .next_focus_step(focus, 1)
                    .map_or(DialogKey::Consumed, DialogKey::Focus);
            }
            (false, false, true, false, Key::Tab) => {
                return self
                    .next_focus_step(focus, -1)
                    .map_or(DialogKey::Consumed, DialogKey::Focus);
            }
            _ => {}
        }
        if focus == Focus::Editor(EditorSlot::GoToLine) {
            DialogKey::EditGoToLine
        } else {
            DialogKey::Consumed
        }
    }

    fn next_focus_step(&self, current: Focus, delta: isize) -> Option<Focus> {
        let dialog = self.active()?;
        let count = dialog.kind.button_count();
        let mut ring = Vec::with_capacity(count + 1);
        if dialog.kind.is_go_to() {
            ring.push(Focus::Editor(EditorSlot::GoToLine));
        }
        for ix in 0..count {
            ring.push(Focus::DialogButton(ix));
        }
        if ring.is_empty() {
            return None;
        }
        let pos = ring.iter().position(|&f| f == current);
        let next = match pos {
            Some(ix) => (ix as isize + delta).rem_euclid(ring.len() as isize) as usize,
            None if delta > 0 => 0,
            None => ring.len() - 1,
        };
        Some(ring[next])
    }

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

    fn render_layer(&self, dialog: &ActiveDialog, focus: Focus, p: Palette, ui: &mut AppUi) -> AppNode {
        let focused_btn = match focus {
            Focus::DialogButton(ix) => Some(ix),
            _ => None,
        };
        let labels = dialog.kind.button_labels();
        let closing = dialog.transition.is_closing();
        let buttons: Vec<AppNode> = labels
            .iter()
            .enumerate()
            .map(|(ix, &label)| {
                let accented = ix == 0 && focused_btn.is_none_or(|f| f == ix);
                let is_focused = !closing && focused_btn == Some(ix);
                button(
                    p,
                    p.command_surface,
                    HitId::Dialog(DialogHit::Button(ix)),
                    label,
                    accented,
                    is_focused,
                    ui,
                )
            })
            .collect();

        let (fixed_width, title, body) = match &dialog.kind {
            DialogKind::GoTo => (
                Some(GO_TO_WIDTH + 2. * PADDING + 2. * BORDER),
                SharedStr::new_static(GO_TO_TITLE),
                self.render_go_to_body(focus, p, ui),
            ),
            DialogKind::LineOutOfRange => (
                None,
                SharedStr::new_static(GO_TO_TITLE),
                message_body(OUT_OF_RANGE_MESSAGE.into()),
            ),
            DialogKind::Prompt(prompt) => (
                None,
                prompt.title.clone(),
                message_body(prompt.message.clone()),
            ),
        };

        row()
            .absolute()
            .inset_0()
            .items_center()
            .justify_center()
            .bg(p.smoke)
            .opacity(dialog.opacity)
            .when(!closing, |this| {
                this.hit(HitId::Dialog(DialogHit::Backdrop))
                    .cursor(CursorStyle::Arrow)
            })
            .child(
                dialog_frame(p, fixed_width, title, body, buttons)
                    .scale(dialog.scale, dialog.scale)
                    .when(closing, AppNode::inert),
            )
    }

    fn render_go_to_body(&self, focus: Focus, p: Palette, ui: &AppUi) -> AppNode {
        let slot = EditorSlot::GoToLine;
        let field = text_field(
            p,
            p.content_surface,
            slot,
            &self.line,
            focus == Focus::Editor(slot),
            false,
            ui,
        );
        box_()
            .child(message_body("Line number".into()).h(BODY_LINE).mb(HEADER_GAP))
            .child(field)
    }
}

fn message_body(message: SharedStr) -> AppNode {
    box_()
        .top(BODY_BASELINE_SNAP)
        .line_height(BODY_LINE)
        .child(text(message))
}

fn dialog_frame(
    p: Palette,
    fixed_width: Option<f32>,
    title: SharedStr,
    body: AppNode,
    buttons: Vec<AppNode>,
) -> AppNode {
    let spacer = (buttons.len() == 1).then(|| box_().flex_1().min_w_0());
    box_()
        .hit(HitId::Dialog(DialogHit::Card))
        .when_some(fixed_width, |this, w| this.w(w).max_w_full())
        .when(fixed_width.is_none(), |this| {
            this.min_w(MIN_WIDTH).max_w(MAX_WIDTH)
        })
        .border(BORDER, p.surface_stroke)
        .rounded(OVERLAY_RADIUS)
        .shadows(theme_shadow(p.dark, DIALOG_Z))
        .child(
            box_()
                .rounded(OVERLAY_RADIUS - BORDER)
                .bg(p.command_surface.panel)
                .font_family(TEXT_FONT)
                .text_size(BODY_SIZE)
                .text_color(p.text_primary)
                .child(dialog_content(p, title, body))
                .child(
                    row()
                        .p(PADDING)
                        .gap(BUTTON_GAP)
                        .when_some(spacer, |this, sp| this.child(sp))
                        .children(
                            buttons
                                .into_iter()
                                .map(|btn| box_().flex_1().min_w_0().child(btn)),
                        ),
                ),
        )
}

fn dialog_content(p: Palette, title: SharedStr, body: AppNode) -> AppNode {
    box_()
        .p(PADDING)
        .rounded_t(OVERLAY_RADIUS - BORDER)
        .bg(p.content_surface.panel)
        .border_b(1.)
        .border_color(p.dialog_divider)
        .child(
            box_()
                .mb(TITLE_GAP)
                .font_family(TITLE_FONT)
                .font_semibold()
                .text_size(TITLE_SIZE)
                .line_height(TITLE_LINE)
                .line_clamp(2)
                .child(text(title)),
        )
        .child(body)
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
