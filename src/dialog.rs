use super::*;
use field::{BODY_LINE, BODY_SIZE, FieldStyle};
use futures::channel::oneshot;
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
const BORDER: f32 = 1.;

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

struct OpenDialog {
    kind: DialogKind,
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
    _line_observer: Subscription,
}

impl Dialogs {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Jot>) -> Self {
        let line = cx.new(|cx| {
            InputState::new(window, cx).validate(|text, _| text.chars().all(|c| c.is_ascii_digit()))
        });
        let _line_observer = Jot::watch_field(&line, cx);
        Self {
            open: Vec::new(),
            serial: 0,
            focus: cx.focus_handle(),
            line,
            _line_observer,
        }
    }

    fn active(&self) -> Option<&DialogKind> {
        self.open
            .iter()
            .rev()
            .find(|dialog| !dialog.closing)
            .map(|dialog| &dialog.kind)
    }

    fn active_mut(&mut self) -> Option<&mut DialogKind> {
        self.open
            .iter_mut()
            .rev()
            .find(|dialog| !dialog.closing)
            .map(|dialog| &mut dialog.kind)
    }

    fn show(&mut self, kind: DialogKind) {
        self.hide();
        self.serial += 1;
        self.open.push(OpenDialog {
            kind,
            serial: self.serial,
            closing: false,
            scale: OPEN_SCALE,
            opacity: 0.,
        });
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
        self.accent = accent_color(cx);
        self.dialogs.show(DialogKind::GoTo);
        self.dialogs.line.update(cx, |input, cx| {
            if let Some(line) = line {
                input.set_value(line.to_string(), window, cx);
            }
            input.select_all(window, cx);
            input.focus(window, cx);
        });
        self.restart_caret_blink(cx);
    }

    fn confirm_dialog(&mut self, _: &Confirm, window: &mut Window, cx: &mut Context<Self>) {
        match self.dialogs.active() {
            Some(DialogKind::GoTo) => self.submit_go_to(window, cx),
            Some(DialogKind::LineOutOfRange) => self.show_go_to(None, window, cx),
            Some(DialogKind::Prompt(_)) => self.answer_prompt(0, window, cx),
            None => {}
        }
    }

    fn cancel_dialog(&mut self, _: &Cancel, window: &mut Window, cx: &mut Context<Self>) {
        match self.dialogs.active() {
            Some(DialogKind::GoTo) => self.close_dialogs(window, cx),
            Some(DialogKind::LineOutOfRange) => self.show_go_to(None, window, cx),
            Some(DialogKind::Prompt(prompt)) => {
                let last = prompt.buttons.len().saturating_sub(1);
                self.answer_prompt(last, window, cx)
            }
            None => {}
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
        self.accent = accent_color(cx);
        self.dialogs.show(DialogKind::Prompt(Prompt {
            title: title.into(),
            message: message.into(),
            buttons,
            answer: Some(answer),
        }));
        self.dialogs.focus.focus(window, cx);
        cx.notify();
        receiver
    }

    fn answer_prompt(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(DialogKind::Prompt(prompt)) = self.dialogs.active_mut()
            && let Some(answer) = prompt.answer.take()
        {
            answer.send(index).ok();
        }
        self.close_dialogs(window, cx);
    }

    fn close_dialogs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dialogs.hide();
        self.active_editor().focus_handle(cx).focus(window, cx);
        self.restart_caret_blink(cx);
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
                self.dialogs.show(DialogKind::LineOutOfRange);
                self.dialogs.focus.focus(window, cx);
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
        if self.dialogs.open.is_empty() {
            return None;
        }
        let layers: Vec<_> = self
            .dialogs
            .open
            .iter()
            .map(|dialog| self.dialog_layer(dialog, window, cx))
            .collect();
        Some(
            div()
                .id("dialogs")
                .key_context(CONTEXT)
                .track_focus(&self.dialogs.focus)
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .on_action(cx.listener(Self::confirm_dialog))
                .on_action(cx.listener(Self::cancel_dialog))
                .on_action(cx.listener(|this, _: &input::Escape, window, cx| {
                    this.cancel_dialog(&Cancel, window, cx)
                }))
                .children(layers),
        )
    }

    fn dialog_layer(&self, dialog: &OpenDialog, window: &Window, cx: &mut Context<Self>) -> Div {
        let p = palette(cx);
        let style = field::ButtonStyle {
            surface: p.command_surface,
            accent: self.accent,
            scale: dialog.scale,
        };
        let frame = match &dialog.kind {
            DialogKind::GoTo => self.go_to_frame(p, style, window, cx),
            DialogKind::LineOutOfRange => out_of_range_frame(p, style, window, cx),
            DialogKind::Prompt(prompt) => prompt_frame(p, style, prompt, window, cx),
        };
        div()
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
            .child(frame)
    }

    fn go_to_frame(
        &self,
        p: Palette,
        style: field::ButtonStyle,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let s = style.scale;
        let body = div()
            .child(
                message_body("Line number".into(), s)
                    .h(px(BODY_LINE * s))
                    .mb(px(HEADER_GAP * s)),
            )
            .child(self.text_field(
                &self.dialogs.line,
                FieldStyle {
                    placeholder: None,
                    search_icon: false,
                },
                p.content_surface,
                s,
                window,
                cx,
            ));
        dialog_frame(
            p,
            s,
            GO_TO_WIDTH,
            GO_TO_TITLE.into(),
            body,
            vec![
                field::button(p, style, "dialog-go-to", "Go to", true, false)
                    .on_click(cx.listener(|this, _, window, cx| this.submit_go_to(window, cx))),
                field::button(p, style, "dialog-cancel", "Cancel", false, false)
                    .on_click(cx.listener(|this, _, window, cx| this.close_dialogs(window, cx))),
            ],
        )
    }
}

fn out_of_range_frame(
    p: Palette,
    style: field::ButtonStyle,
    window: &Window,
    cx: &mut Context<Jot>,
) -> Div {
    let s = style.scale;
    let message = "The line number is beyond the total number of lines";
    let width = text_width(window, message, TEXT_FONT, BODY_SIZE);
    dialog_frame(
        p,
        s,
        width,
        GO_TO_TITLE.into(),
        message_body(message.into(), s),
        vec![
            field::button(p, style, "dialog-ok", "OK", true, true)
                .on_click(cx.listener(|this, _, window, cx| this.show_go_to(None, window, cx))),
        ],
    )
}

fn prompt_frame(
    p: Palette,
    style: field::ButtonStyle,
    prompt: &Prompt,
    window: &Window,
    cx: &mut Context<Jot>,
) -> Div {
    let s = style.scale;
    let width = text_width(window, &prompt.message, TEXT_FONT, BODY_SIZE).max(text_width(
        window,
        &prompt.title,
        TITLE_FONT,
        TITLE_SIZE,
    ));
    let buttons = prompt
        .buttons
        .iter()
        .enumerate()
        .map(|(ix, &label)| {
            field::button(p, style, ("dialog-prompt", ix), label, ix == 0, ix == 0).on_click(
                cx.listener(move |this, _, window, cx| this.answer_prompt(ix, window, cx)),
            )
        })
        .collect();
    dialog_frame(
        p,
        s,
        width,
        prompt.title.clone(),
        message_body(prompt.message.clone(), s),
        buttons,
    )
}

fn message_body(message: SharedString, s: f32) -> Div {
    div()
        .relative()
        .top(px(-1.))
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
        .border_1()
        .border_color(rgba(p.surface_stroke))
        .rounded(u(field::OVERLAY_RADIUS))
        .shadow(theme_shadow(p.dark, DIALOG_Z, s))
        .child(
            div()
                .flex()
                .flex_col()
                .rounded(u(field::OVERLAY_RADIUS - BORDER))
                .bg(rgb(p.dialog_command))
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
        .bg(rgb(p.dialog_content))
        .border_b_1()
        .border_color(rgb(p.dialog_divider))
        .child(
            div()
                .h(u(TITLE_LINE))
                .mb(u(TITLE_GAP))
                .font_family(TITLE_FONT)
                .font_weight(FontWeight::SEMIBOLD)
                .text_size(u(TITLE_SIZE))
                .line_height(u(TITLE_LINE))
                .child(title),
        )
        .child(body)
}
