use super::*;
use field::{BODY_LINE, BODY_SIZE, text_field};
use futures::channel::oneshot;
use gpui_kit::base::FocusTrapElement as _;
use gpui_kit::base::actions::{Cancel, Confirm};
use gpui_kit::base::input::Position;
use gpui_kit::base::{MotionStatus, Presence, Transition, transition_with_status};
use gpui_kit::component::input::InputState;

const CONTEXT: &str = "JotDialog";
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
const OUT_OF_RANGE_BUTTONS: &[&str] = &["OK"];
const BORDER: f32 = 1.;
const BODY_BASELINE_SNAP: f32 = -1.;

pub(crate) fn key_bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("enter", Confirm { secondary: false }, Some(CONTEXT)),
        KeyBinding::new("escape", Cancel, Some(CONTEXT)),
    ]
}

struct Prompt {
    title: SharedString,
    message: SharedString,
    buttons: &'static [&'static str],
    answer: Option<oneshot::Sender<usize>>,
}

enum DialogKind {
    GoTo,
    LineOutOfRange,
    Prompt(Prompt),
}

impl DialogKind {
    fn buttons(&self) -> &'static [&'static str] {
        match self {
            Self::GoTo => GO_TO_BUTTONS,
            Self::LineOutOfRange => OUT_OF_RANGE_BUTTONS,
            Self::Prompt(prompt) => prompt.buttons,
        }
    }
}

struct OpenDialog {
    kind: DialogKind,
    buttons: Vec<FocusHandle>,
    serial: usize,
    closing: bool,
    scale: f32,
    opacity: f32,
}

impl OpenDialog {
    fn sample(&mut self, window: &mut Window, cx: &mut App) -> bool {
        let shown = Presence::new(ElementId::from(("jot-dialog-show", self.serial)), true)
            .transition(Transition::new(SHOW_DURATION).easing(DECELERATE))
            .sample(window, cx)
            .progress;
        let faded_in = Presence::new(ElementId::from(("jot-dialog-fade-in", self.serial)), true)
            .transition(Transition::new(FADE_DURATION).easing(Easing::Linear))
            .sample(window, cx)
            .progress;
        let hidden = transition_with_status(
            ElementId::from(("jot-dialog-hide", self.serial)),
            if self.closing { 1. } else { 0. },
            Transition::new(HIDE_DURATION).easing(DECELERATE),
            window,
            cx,
        );
        let fade_out = transition_with_status(
            ElementId::from(("jot-dialog-fade-out", self.serial)),
            if self.closing { 0. } else { 1. },
            Transition::new(FADE_DURATION).easing(Easing::Linear),
            window,
            cx,
        );
        let growth = OPEN_SCALE - 1.;
        self.scale = 1. + growth * (1. - shown) + growth * hidden.value;
        self.opacity = faded_in * fade_out.value;
        self.closing && fade_out.status == MotionStatus::Finished
    }
}

pub(crate) struct Dialogs {
    open: Vec<OpenDialog>,
    serial: usize,
    focus: FocusHandle,
    line: Entity<InputState>,
}

impl Dialogs {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Jot>) -> Self {
        let line = cx.new(|cx| {
            InputState::new(window, cx).validate(|text, _| text.chars().all(|c| c.is_ascii_digit()))
        });
        line.read(cx).focus_handle(cx).tab_index(0).tab_stop(true);
        Self {
            open: Vec::new(),
            serial: 0,
            focus: cx.focus_handle(),
            line,
        }
    }

    fn active(&self) -> Option<&OpenDialog> {
        self.open.iter().rev().find(|dialog| !dialog.closing)
    }

    fn active_mut(&mut self) -> Option<&mut OpenDialog> {
        self.open.iter_mut().rev().find(|dialog| !dialog.closing)
    }

    fn show(&mut self, kind: DialogKind, cx: &mut App) {
        self.hide();
        self.serial += 1;
        let buttons = (0..kind.buttons().len())
            .map(|ix| cx.focus_handle().tab_index(ix as isize + 1).tab_stop(true))
            .collect();
        self.open.push(OpenDialog {
            kind,
            buttons,
            serial: self.serial,
            closing: false,
            scale: OPEN_SCALE,
            opacity: 0.,
        });
    }

    fn focus_default(&self, window: &mut Window, cx: &mut App) {
        if let Some(button) = self.active().and_then(|dialog| dialog.buttons.first()) {
            button.focus(window, cx);
        }
    }

    fn hide(&mut self) {
        for dialog in &mut self.open {
            dialog.closing = true;
            if let DialogKind::Prompt(prompt) = &mut dialog.kind {
                prompt.answer = None;
            }
        }
    }
}

impl Jot {
    pub(crate) fn go_to(&mut self, _: &GoTo, window: &mut Window, cx: &mut Context<Self>) {
        if self.dialogs.active().is_some() {
            return;
        }
        let editor = self.active_editor().read(cx);
        let row = editor.text().offset_to_point(editor.cursor()).row;
        self.show_go_to(Some(row + 1), window, cx);
    }

    fn show_go_to(&mut self, line: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        self.dialogs.show(DialogKind::GoTo, cx);
        self.dialogs.line.update(cx, |input, cx| {
            if let Some(line) = line {
                input.set_value(line.to_string(), window, cx);
            }
            input.select_all(window, cx);
            input.focus(window, cx);
        });
    }

    fn confirm_dialog(&mut self, _: &Confirm, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.dialogs.active() else {
            return;
        };
        let focused = dialog
            .buttons
            .iter()
            .position(|button| button.is_focused(window));
        self.press_dialog_button(focused.unwrap_or(0), window, cx);
    }

    fn cancel_dialog(&mut self, _: &Cancel, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.dialogs.active() else {
            return;
        };
        let last = dialog.buttons.len().saturating_sub(1);
        self.press_dialog_button(last, window, cx);
    }

    fn press_dialog_button(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.dialogs.active() else {
            return;
        };
        match dialog.kind {
            DialogKind::GoTo if ix == 0 => self.submit_go_to(window, cx),
            DialogKind::GoTo => self.close_dialogs(window, cx),
            DialogKind::LineOutOfRange => self.show_go_to(None, window, cx),
            DialogKind::Prompt(_) => self.answer_prompt(ix, window, cx),
        }
    }

    pub(crate) fn prompt(
        &mut self,
        title: impl Into<SharedString>,
        message: impl Into<SharedString>,
        buttons: &'static [&'static str],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> oneshot::Receiver<usize> {
        let (answer, receiver) = oneshot::channel();
        self.dialogs.show(
            DialogKind::Prompt(Prompt {
                title: title.into(),
                message: message.into(),
                buttons,
                answer: Some(answer),
            }),
            cx,
        );
        self.dialogs.focus_default(window, cx);
        cx.notify();
        receiver
    }

    fn answer_prompt(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(dialog) = self.dialogs.active_mut()
            && let DialogKind::Prompt(prompt) = &mut dialog.kind
            && let Some(answer) = prompt.answer.take()
        {
            answer.send(index).ok();
        }
        self.close_dialogs(window, cx);
    }

    fn close_dialogs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dialogs.hide();
        self.active_editor().focus_handle(cx).focus(window, cx);
    }

    fn submit_go_to(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let value = self.dialogs.line.read(cx).value();
        let editor = self.active_editor().clone();
        let lines = editor.read(cx).text().lines_len();
        match value.parse::<usize>() {
            Ok(line) if (1..=lines).contains(&line) => {
                self.close_dialogs(window, cx);
                editor.update(cx, |editor, cx| {
                    editor.set_cursor_position(Position::new((line - 1) as u32, 0), window, cx)
                });
            }
            _ => {
                self.dialogs.show(DialogKind::LineOutOfRange, cx);
                self.dialogs.focus_default(window, cx);
                cx.notify();
            }
        }
    }

    pub(crate) fn render_dialogs(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        self.dialogs
            .open
            .retain_mut(|dialog| !dialog.sample(window, cx));
        let serial = self.dialogs.open.last()?.serial;
        let accent = rgb(palette(cx).accent);
        let layers: Vec<_> = self
            .dialogs
            .open
            .iter()
            .map(|dialog| self.dialog_layer(dialog, accent, window, cx))
            .collect();
        Some(
            div()
                .id("dialogs")
                .key_context(CONTEXT)
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .on_action(cx.listener(Self::confirm_dialog))
                .on_action(cx.listener(Self::cancel_dialog))
                .children(layers)
                .focus_trap(("jot-dialog-trap", serial), &self.dialogs.focus),
        )
    }

    fn dialog_layer(
        &self,
        dialog: &OpenDialog,
        accent: Rgba,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let p = palette(cx);
        let s = dialog.scale;
        let style = field::ButtonStyle {
            surface: p.command_surface,
            accent,
            scale: s,
        };
        let focused = dialog
            .buttons
            .iter()
            .position(|button| button.is_focused(window));
        let buttons =
            dialog
                .kind
                .buttons()
                .iter()
                .zip(&dialog.buttons)
                .enumerate()
                .map(|(ix, (&label, handle))| {
                    let accented = ix == 0 && focused.is_none_or(|focused| focused == ix);
                    field::button(
                        p,
                        style,
                        ("button", ix),
                        label,
                        accented,
                        Some(handle),
                        window,
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.press_dialog_button(ix, window, cx)
                    }))
                })
                .collect();
        let (width, title, body) = match &dialog.kind {
            DialogKind::GoTo => (
                GO_TO_WIDTH,
                GO_TO_TITLE.into(),
                self.go_to_body(p, s, window, cx),
            ),
            DialogKind::LineOutOfRange => (
                text_width(window, OUT_OF_RANGE_MESSAGE, TEXT_FONT, BODY_SIZE),
                GO_TO_TITLE.into(),
                message_body(OUT_OF_RANGE_MESSAGE.into(), s),
            ),
            DialogKind::Prompt(prompt) => (
                text_width(window, &prompt.message, TEXT_FONT, BODY_SIZE).max(text_width(
                    window,
                    &prompt.title,
                    TITLE_FONT,
                    TITLE_SIZE,
                )),
                prompt.title.clone(),
                message_body(prompt.message.clone(), s),
            ),
        };
        div()
            .id(("jot-dialog", dialog.serial))
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(p.smoke))
            .opacity(dialog.opacity)
            .when(!dialog.closing, |this| this.occlude())
            .child(dialog_frame(p, s, width, title, body, buttons))
    }

    fn go_to_body(&self, p: Palette, s: f32, window: &Window, cx: &mut Context<Self>) -> Div {
        div()
            .child(
                message_body("Line number".into(), s)
                    .h(px(BODY_LINE * s))
                    .mb(px(HEADER_GAP * s)),
            )
            .child(text_field(
                &self.dialogs.line,
                false,
                p.content_surface,
                s,
                window,
                cx,
            ))
    }
}

fn message_body(message: SharedString, s: f32) -> Div {
    div()
        .relative()
        .top(px(BODY_BASELINE_SNAP))
        .line_height(px(BODY_LINE * s))
        .child(message)
}

fn dialog_frame(
    p: Palette,
    s: f32,
    content_width: f32,
    title: SharedString,
    body: Div,
    buttons: Vec<Stateful<Div>>,
) -> Div {
    let u = |v: f32| px(v * s);
    let width = (content_width + 2. * PADDING + 2. * BORDER).clamp(MIN_WIDTH, MAX_WIDTH);
    let spacer = (buttons.len() == 1).then(|| div().flex_1().min_w_0());
    div()
        .w(u(width))
        .max_w_full()
        .border_1()
        .border_color(rgba(p.surface_stroke))
        .rounded(u(field::OVERLAY_RADIUS))
        .shadow(theme_shadow(p.dark, DIALOG_Z, s))
        .child(
            div()
                .flex()
                .flex_col()
                .rounded(u(field::OVERLAY_RADIUS - BORDER))
                .bg(rgb(p.command_surface.panel))
                .font_family(TEXT_FONT)
                .text_size(u(BODY_SIZE))
                .text_color(rgba(p.text_primary))
                .child(dialog_content(p, s, title, body))
                .child(
                    div()
                        .flex()
                        .p(u(PADDING))
                        .gap(u(BUTTON_GAP))
                        .children(spacer)
                        .children(
                            buttons
                                .into_iter()
                                .map(|button| div().flex_1().min_w_0().child(button)),
                        ),
                ),
        )
}

fn dialog_content(p: Palette, s: f32, title: SharedString, body: Div) -> Div {
    let u = |v: f32| px(v * s);
    div()
        .p(u(PADDING))
        .rounded_t(u(field::OVERLAY_RADIUS - BORDER))
        .bg(rgb(p.content_surface.panel))
        .border_b_1()
        .border_color(rgb(p.dialog_divider))
        .child(
            div()
                .mb(u(TITLE_GAP))
                .font_family(TITLE_FONT)
                .font_weight(FontWeight::SEMIBOLD)
                .text_size(u(TITLE_SIZE))
                .line_height(u(TITLE_LINE))
                .line_clamp(2)
                .child(title),
        )
        .child(body)
}
