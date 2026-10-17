use super::*;
use field::{BODY_LINE, BODY_SIZE, FieldStyle, Surface};
use gpui_kit::base::actions::{Cancel, Confirm};
use gpui_kit::base::input::Position;
use gpui_kit::base::{MotionStatus, Presence, Transition, transition_with_status};
use gpui_kit::component::input::InputState;

const CONTEXT: &str = "JotDialog";
const SMOKE: u32 = 0x0000004d;
const BORDER: u32 = 0x75757566;
const CONTENT_FILL: u32 = 0xffffff;
const COMMAND_FILL: u32 = 0xf3f3f3;
const DIVIDER: u32 = 0xe5e5e5;
const TITLE_FONT: &str = "Segoe UI Variable Display";
const OPEN_SCALE: f32 = 1.05;
const SHOW_DURATION: Duration = Duration::from_millis(250);
const HIDE_DURATION: Duration = Duration::from_millis(167);
const FADE_DURATION: Duration = Duration::from_millis(83);
const MIN_WIDTH: f32 = 320.;
const MAX_WIDTH: f32 = 548.;
const PADDING: f32 = 24.;
const RADIUS: f32 = 8.;
const TITLE_SIZE: f32 = 20.;
const TITLE_LINE: f32 = 28.;
const TITLE_GAP: f32 = 12.;
const HEADER_GAP: f32 = 8.;
const BUTTON_GAP: f32 = 8.;
const GO_TO_WIDTH: f32 = 312.;
const CONTENT_SURFACE: Surface = Surface {
    field_fill: 0xffffff,
    field_stroke: 0xf0f0f0,
    field_bottom: 0x8d8d8d,
    button_fill: 0xffffff,
    button_hover: 0xf9f9f9,
    button_pressed: 0xf9f9f9,
    button_stroke: 0xf0f0f0,
    button_bottom: 0xd6d6d6,
};
const COMMAND_SURFACE: Surface = Surface {
    field_fill: 0xfbfbfb,
    field_stroke: 0xe5e5e5,
    field_bottom: 0x8d8d8d,
    button_fill: 0xfbfbfb,
    button_hover: 0xf6f6f6,
    button_pressed: 0xf5f5f5,
    button_stroke: 0xe5e5e5,
    button_bottom: 0xcccccc,
};

pub fn key_bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("enter", Confirm { secondary: false }, Some(CONTEXT)),
        KeyBinding::new("escape", Cancel, Some(CONTEXT)),
    ]
}

fn line_count(text: &Rope) -> usize {
    text.offset_to_point(text.len()).row + 1
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum DialogKind {
    GoTo,
    LineOutOfRange,
}

pub(crate) struct OpenDialog {
    kind: DialogKind,
    serial: usize,
    closing: bool,
    scale: f32,
    opacity: f32,
}

impl OpenDialog {
    fn sample(&mut self, window: &mut Window, cx: &mut App) -> bool {
        let shown = Presence::new(ElementId::from(("jot-dialog-show", self.serial)), true)
            .transition(Transition::new(SHOW_DURATION).ease(cubic_bezier(0., 0., 0., 1.)))
            .sample(window, cx)
            .progress;
        let faded_in = Presence::new(ElementId::from(("jot-dialog-fade-in", self.serial)), true)
            .transition(Transition::new(FADE_DURATION).ease(|t| t))
            .sample(window, cx)
            .progress;
        let hidden = transition_with_status(
            ElementId::from(("jot-dialog-hide", self.serial)),
            if self.closing { 1. } else { 0. },
            Transition::new(HIDE_DURATION).ease(cubic_bezier(0., 0., 0., 1.)),
            window,
            cx,
        );
        let fade_out = transition_with_status(
            ElementId::from(("jot-dialog-fade-out", self.serial)),
            if self.closing { 0. } else { 1. },
            Transition::new(FADE_DURATION).ease(|t| t),
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
    serials: usize,
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
            serials: 0,
            focus: cx.focus_handle(),
            line,
            _line_observer,
        }
    }

    fn active(&self) -> Option<DialogKind> {
        self.open
            .iter()
            .rev()
            .find(|dialog| !dialog.closing)
            .map(|dialog| dialog.kind)
    }

    fn show(&mut self, kind: DialogKind) {
        for dialog in &mut self.open {
            dialog.closing = true;
        }
        self.serials += 1;
        self.open.push(OpenDialog {
            kind,
            serial: self.serials,
            closing: false,
            scale: OPEN_SCALE,
            opacity: 0.,
        });
    }

    fn hide(&mut self) {
        for dialog in &mut self.open {
            dialog.closing = true;
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
        self.accent = field::accent_color();
        self.dialogs.show(DialogKind::GoTo);
        let input = self.dialogs.line.clone();
        input.update(cx, |input, cx| {
            if let Some(line) = line {
                input.set_value(line.to_string(), window, cx);
            }
            input.select_all(window, cx);
            input.focus(window, cx);
        });
        self.restart_caret_blink(cx);
        cx.notify();
    }

    pub(crate) fn confirm_dialog(
        &mut self,
        _: &Confirm,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.dialogs.active() {
            Some(DialogKind::GoTo) => self.submit_go_to(window, cx),
            Some(DialogKind::LineOutOfRange) => self.show_go_to(None, window, cx),
            None => {}
        }
    }

    pub(crate) fn cancel_dialog(
        &mut self,
        _: &Cancel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.dialogs.active() {
            Some(DialogKind::GoTo) => self.close_dialogs(window, cx),
            Some(DialogKind::LineOutOfRange) => self.show_go_to(None, window, cx),
            None => {}
        }
    }

    fn close_dialogs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dialogs.hide();
        let editor = self.active_editor().clone();
        editor.focus_handle(cx).focus(window, cx);
        self.restart_caret_blink(cx);
        cx.notify();
    }

    fn submit_go_to(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let value = self.dialogs.line.read(cx).value();
        let editor = self.active_editor().clone();
        let lines = line_count(editor.read(cx).text());
        match value.parse::<usize>() {
            Ok(line) if (1..=lines).contains(&line) => {
                self.close_dialogs(window, cx);
                editor.update(cx, |editor, cx| {
                    editor.set_cursor_position(Position::new(line as u32 - 1, 0), window, cx)
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
        let s = dialog.scale;
        let accent = self.accent;
        let frame = match dialog.kind {
            DialogKind::GoTo => {
                let body = div()
                    .child(
                        div()
                            .h(px(BODY_LINE * s))
                            .mb(px(HEADER_GAP * s))
                            .relative()
                            .top(px(-1.))
                            .line_height(px(BODY_LINE * s))
                            .child("Line number"),
                    )
                    .child(self.text_field(
                        &self.dialogs.line,
                        FieldStyle {
                            placeholder: None,
                            search_icon: false,
                        },
                        CONTENT_SURFACE,
                        s,
                        window,
                        cx,
                    ));
                dialog_frame(
                    s,
                    GO_TO_WIDTH,
                    "Go to line",
                    body,
                    Some(
                        field::button(
                            "dialog-go-to",
                            "Go to",
                            true,
                            false,
                            COMMAND_SURFACE,
                            accent,
                            s,
                        )
                        .on_click(cx.listener(|this, _, window, cx| this.submit_go_to(window, cx))),
                    ),
                    field::button(
                        "dialog-cancel",
                        "Cancel",
                        false,
                        false,
                        COMMAND_SURFACE,
                        accent,
                        s,
                    )
                    .on_click(cx.listener(|this, _, window, cx| this.close_dialogs(window, cx))),
                )
            }
            DialogKind::LineOutOfRange => {
                let message = "The line number is beyond the total number of lines";
                let width = menu::text_width(window, message, MENU_FONT, BODY_SIZE).ceil();
                dialog_frame(
                    s,
                    width,
                    "Go to line",
                    div()
                        .relative()
                        .top(px(-1.))
                        .line_height(px(BODY_LINE * s))
                        .child(message),
                    None,
                    field::button("dialog-ok", "OK", true, true, COMMAND_SURFACE, accent, s)
                        .on_click(
                            cx.listener(|this, _, window, cx| this.show_go_to(None, window, cx)),
                        ),
                )
            }
        };
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(SMOKE))
            .opacity(dialog.opacity)
            .when(!dialog.closing, |this| this.occlude())
            .child(frame)
    }
}

fn dialog_frame(
    s: f32,
    content_width: f32,
    title: &'static str,
    body: Div,
    primary: Option<Stateful<Div>>,
    close: Stateful<Div>,
) -> Div {
    let u = |v: f32| px(v * s);
    let width = (content_width + 2. * PADDING + 2.).clamp(MIN_WIDTH, MAX_WIDTH);
    div()
        .w(u(width))
        .border_1()
        .border_color(rgba(BORDER))
        .rounded(u(RADIUS))
        .shadow(vec![
            BoxShadow {
                color: hsla(0., 0., 0., 0.14),
                offset: point(px(0.), u(12.)),
                blur_radius: u(28.),
                spread_radius: px(0.),
                inset: false,
            },
            BoxShadow {
                color: hsla(0., 0., 0., 0.1),
                offset: point(px(0.), u(24.)),
                blur_radius: u(48.),
                spread_radius: px(0.),
                inset: false,
            },
        ])
        .child(
            div()
                .flex()
                .flex_col()
                .rounded(u(RADIUS - 1.))
                .bg(rgb(COMMAND_FILL))
                .font_family(MENU_FONT)
                .text_size(u(BODY_SIZE))
                .text_color(rgba(TEXT_PRIMARY))
                .child(dialog_content(s, title, body))
                .child(
                    div()
                        .flex()
                        .p(u(PADDING))
                        .gap(u(BUTTON_GAP))
                        .child(div().flex_1().children(primary))
                        .child(div().flex_1().child(close)),
                ),
        )
}

fn dialog_content(s: f32, title: &'static str, body: Div) -> Div {
    let u = |v: f32| px(v * s);
    div()
        .p(u(PADDING))
        .rounded_t(u(RADIUS - 1.))
        .bg(rgb(CONTENT_FILL))
        .border_b_1()
        .border_color(rgb(DIVIDER))
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
