use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::editor::Editor;
use crate::ids::{AnimKey, CustomDraw, EditorSlot, Focus, HitId, Node, UiState};
use crate::platform::{CursorStyle, Key, KeyEvent};
use crate::tabs::TabId;
use crate::theme::{Palette, theme_shadow};
use crate::types::SharedStr;
use crate::ui::controls::{
    BODY_LINE, BODY_SIZE, ButtonStyle, FIELD_CARET, OVERLAY_RADIUS, TEXT_FONT, TextFieldStyle,
    button, text_field_frame,
};
use crate::ui::{AnimStatus, Easing, box_, row, text};

const TITLE_FONT: &str = "Segoe UI Variable Display";
const OPEN_SCALE: f32 = 1.05;
const DIALOG_Z: f32 = 128.;
const SHOW_DURATION: Duration = Duration::from_millis(250);
const HIDE_DURATION: Duration = Duration::from_millis(167);
const FADE_DURATION: Duration = Duration::from_millis(83);
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

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct PromptId(u64);

impl PromptId {
    pub(crate) fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Prompt {
    pub(crate) id: Option<PromptId>,
    pub(crate) title: SharedStr,
    pub(crate) message: SharedStr,
    pub(crate) buttons: &'static [&'static str],
    pub(crate) tab: Option<TabId>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum DialogKey {
    PassThrough,
    Focus(Focus),
    PressButton(usize),
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
            Self::Prompt(p) => p.tab,
            Self::GoTo | Self::LineOutOfRange => None,
        }
    }

    fn buttons(&self) -> &'static [&'static str] {
        match self {
            Self::GoTo => GO_TO_BUTTONS,
            Self::LineOutOfRange => OK_BUTTONS,
            Self::Prompt(p) => p.buttons,
        }
    }

    fn is_go_to(&self) -> bool {
        matches!(self, Self::GoTo)
    }
}

struct OpenDialog {
    kind: DialogKind,
    serial: usize,
    closing: bool,
    scale: f32,
    opacity: f32,
}

impl OpenDialog {
    fn sample(&mut self, ui: &mut UiState) -> bool {
        let shown = ui
            .animate_f32_from(
                AnimKey::DialogShow(self.serial),
                0.0,
                1.0,
                SHOW_DURATION,
                Easing::DECELERATE,
            )
            .value;
        let faded_in = ui
            .animate_f32_from(
                AnimKey::DialogFadeIn(self.serial),
                0.0,
                1.0,
                FADE_DURATION,
                Easing::Linear,
            )
            .value;
        let hidden = ui.animate_f32_from(
            AnimKey::DialogHide(self.serial),
            0.0,
            if self.closing { 1. } else { 0. },
            HIDE_DURATION,
            Easing::DECELERATE,
        );
        let fade_out = ui.animate_f32_from(
            AnimKey::DialogFadeOut(self.serial),
            1.0,
            if self.closing { 0. } else { 1. },
            FADE_DURATION,
            Easing::Linear,
        );
        let growth = OPEN_SCALE - 1.;
        self.scale = 1. + growth * (1. - shown) + growth * hidden.value;
        self.opacity = faded_in * fade_out.value;
        self.closing && fade_out.status == AnimStatus::Finished
    }
}

pub(crate) struct Dialogs {
    open: Vec<OpenDialog>,
    pending: VecDeque<DialogKind>,
    serial: usize,
    pub(crate) line: Editor,
}

impl Dialogs {
    pub(crate) fn new() -> Self {
        let line = Editor::single_line()
            .with_caret_height(FIELD_CARET)
            .with_validate(|t| t.chars().all(|c| c.is_ascii_digit()));
        Self {
            open: Vec::new(),
            pending: VecDeque::new(),
            serial: 0,
            line,
        }
    }

    pub(crate) fn is_open(&self) -> bool {
        self.active().is_some()
    }

    fn active(&self) -> Option<&OpenDialog> {
        self.open.iter().rev().find(|d| !d.closing)
    }

    fn active_button_count(&self) -> usize {
        self.active().map_or(0, |d| d.kind.buttons().len())
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
        self.serial += 1;
        let initial_focus = if kind.is_go_to() {
            Focus::Editor(EditorSlot::GoToLine)
        } else {
            Focus::DialogButton(0)
        };
        self.open.push(OpenDialog {
            kind,
            serial: self.serial,
            closing: false,
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
            d.closing = true;
        }
    }

    pub(crate) fn close_for_tab(&mut self, id: TabId) {
        for d in &mut self.open {
            if !d.closing && d.kind.referenced_tab() == Some(id) {
                d.closing = true;
            }
        }
        self.pending.retain(|d| d.referenced_tab() != Some(id));
    }

    pub(crate) fn take_active(&mut self) -> Option<DialogKind> {
        let ix = self.open.iter().rposition(|d| !d.closing)?;
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
                return DialogKey::PressButton(self.active_button_count().saturating_sub(1));
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
        let count = dialog.kind.buttons().len();
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

    pub(crate) fn render(&mut self, focus: Focus, p: Palette, ui: &mut UiState) -> Option<Node> {
        self.open.retain_mut(|d| !d.sample(ui));
        if self.open.is_empty() {
            return None;
        }
        let layers: Vec<Node> = self
            .open
            .iter()
            .map(|d| self.render_layer(d, focus, p, ui))
            .collect();
        Some(box_().absolute().inset_0().children(layers))
    }

    fn render_layer(&self, dialog: &OpenDialog, focus: Focus, p: Palette, ui: &mut UiState) -> Node {
        let style = ButtonStyle {
            surface: p.command_surface,
            accent: p.accent,
        };
        let focused_btn = match focus {
            Focus::DialogButton(ix) => Some(ix),
            _ => None,
        };
        let labels = dialog.kind.buttons();
        let buttons: Vec<Node> = labels
            .iter()
            .enumerate()
            .map(|(ix, &label)| {
                let accented = ix == 0 && focused_btn.is_none_or(|f| f == ix);
                let is_focused = !dialog.closing && focused_btn == Some(ix);
                button(
                    p,
                    style,
                    HitId::DialogButton(ix),
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
            .when(!dialog.closing, |this| {
                this.hit(HitId::DialogBackdrop).cursor(CursorStyle::Arrow)
            })
            .child(
                dialog_frame(p, fixed_width, title, body, buttons)
                    .scale(dialog.scale, dialog.scale)
                    .when(dialog.closing, Node::inert),
            )
    }

    fn render_go_to_body(&self, focus: Focus, p: Palette, ui: &UiState) -> Node {
        let slot = EditorSlot::GoToLine;
        let line_editor = self
            .line
            .view(HitId::Editor(slot), CustomDraw::Editor(slot), BODY_LINE + 2.);
        let field = text_field_frame(
            p,
            HitId::Editor(slot),
            HitId::FieldClear(slot),
            TextFieldStyle {
                surface: p.content_surface,
                focused: focus == Focus::Editor(slot),
                empty: self.line.text().len() == 0,
                search_icon: false,
            },
            line_editor,
            ui,
        );
        box_()
            .child(message_body("Line number".into()).h(BODY_LINE).mb(HEADER_GAP))
            .child(field)
    }
}

fn message_body(message: SharedStr) -> Node {
    box_()
        .top(BODY_BASELINE_SNAP)
        .line_height(BODY_LINE)
        .child(text(message))
}

fn dialog_frame(
    p: Palette,
    fixed_width: Option<f32>,
    title: SharedStr,
    body: Node,
    buttons: Vec<Node>,
) -> Node {
    let spacer = (buttons.len() == 1).then(|| box_().flex_1().min_w_0());
    box_()
        .hit(HitId::DialogCard)
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

fn dialog_content(p: Palette, title: SharedStr, body: Node) -> Node {
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
            DialogKey::PressButton(1)
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
