use super::*;
use gpui_kit::base::actions::{Cancel, Confirm};
use gpui_kit::base::input::Position;
use gpui_kit::base::{MotionStatus, Presence, Transition, transition_with_status};
use gpui_kit::component::input::{Input, InputState};
use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_BINARY, RegGetValueW};

const CONTEXT: &str = "JotDialog";
const SMOKE: u32 = 0x0000004d;
const BORDER: u32 = 0x75757566;
const CONTENT_FILL: u32 = 0xffffff;
const COMMAND_FILL: u32 = 0xf3f3f3;
const DIVIDER: u32 = 0xe5e5e5;
const BUTTON_FILL: u32 = 0xfbfbfb;
const BUTTON_HOVER: u32 = 0xf6f6f6;
const BUTTON_PRESSED: u32 = 0xf5f5f5;
const BUTTON_STROKE: u32 = 0xe5e5e5;
const BUTTON_STROKE_BOTTOM: u32 = 0xcccccc;
const FOCUS_OUTER: u32 = 0x1a1a1a;
const FOCUS_INNER: u32 = 0xfbfbfb;
const TEXT_BOX_STROKE: u32 = 0xf0f0f0;
const TEXT_BOX_STROKE_BOTTOM: u32 = 0x8d8d8d;
const ON_ACCENT: u32 = 0xffffff;
const DEFAULT_ACCENT: u32 = 0x005fb8;
const ACCENT_TOP_EDGE: f32 = 0x14 as f32 / 255.;
const ACCENT_BOTTOM_EDGE: f32 = 0x66 as f32 / 255.;
const ACCENT_HOVER_OPACITY: f32 = 0.9;
const ACCENT_PRESSED_OPACITY: f32 = 0.8;
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
const BODY_SIZE: f32 = 14.;
const BODY_LINE: f32 = 18.;
const HEADER_GAP: f32 = 8.;
const CONTROL_HEIGHT: f32 = 32.;
const CONTROL_RADIUS: f32 = 4.;
const BUTTON_GAP: f32 = 8.;
const TEXT_BOX_PADDING: f32 = 10.;
const TEXT_BOX_CARET: f32 = 19.;
const CLEAR_BUTTON_WIDTH: f32 = 31.;
const CLEAR_BUTTON_RIGHT: f32 = 2.;
const CLEAR_GLYPH_SIZE: f32 = 12.;
const GO_TO_WIDTH: f32 = 312.;

pub fn key_bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("enter", Confirm { secondary: false }, Some(CONTEXT)),
        KeyBinding::new("escape", Cancel, Some(CONTEXT)),
    ]
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

fn accent_color() -> Rgba {
    let mut palette = [0u8; 32];
    let mut len = palette.len() as u32;
    let key = wide(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Accent");
    let value = wide("AccentPalette");
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_BINARY,
            std::ptr::null_mut(),
            palette.as_mut_ptr().cast(),
            &mut len,
        )
    };
    if status != 0 || len < 20 {
        return rgb(DEFAULT_ACCENT);
    }
    rgb(u32::from_be_bytes([
        0,
        palette[16],
        palette[17],
        palette[18],
    ]))
}

fn mix(color: Rgba, toward: f32, amount: f32) -> Rgba {
    let blend = |c: f32| c + (toward - c) * amount;
    Rgba {
        r: blend(color.r),
        g: blend(color.g),
        b: blend(color.b),
        a: 1.,
    }
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
    line_caret: (usize, Range<usize>),
    accent: Rgba,
    _line_observer: Subscription,
}

impl Dialogs {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Jot>) -> Self {
        let line = cx.new(|cx| {
            InputState::new(window, cx).validate(|text, _| text.chars().all(|c| c.is_ascii_digit()))
        });
        let _line_observer = cx.observe(&line, |jot, line, cx| {
            let line = line.read(cx);
            let key = (line.cursor(), line.selected_range());
            if key != jot.dialogs.line_caret {
                jot.dialogs.line_caret = key;
                jot.restart_caret_blink(cx);
            }
        });
        Self {
            open: Vec::new(),
            serials: 0,
            focus: cx.focus_handle(),
            line,
            line_caret: (0, 0..0),
            accent: rgb(DEFAULT_ACCENT),
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
        self.accent = accent_color();
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

    pub(crate) fn line_input_caret(&self, window: &Window, cx: &App) -> impl IntoElement + use<> {
        let input = self.dialogs.line.clone();
        let shown = self.caret_visible
            && window.is_window_active()
            && input.focus_handle(cx).is_focused(window);
        caret_canvas(
            move |cx| {
                let input = input.read(cx);
                input
                    .selected_range()
                    .is_empty()
                    .then(|| input.prepaint_caret_geometry())
                    .flatten()
            },
            px(TEXT_BOX_CARET),
            shown,
        )
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
        let accent = self.dialogs.accent;
        let frame = match dialog.kind {
            DialogKind::GoTo => {
                let focused = self.dialogs.line.focus_handle(cx).is_focused(window);
                let has_text = self.dialogs.line.read(cx).text().len() > 0;
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
                    .child(self.text_box(focused, has_text, s, window, cx));
                dialog_frame(
                    s,
                    GO_TO_WIDTH,
                    "Go to line",
                    body,
                    Some(
                        dialog_button("dialog-go-to", "Go to", true, false, accent, s).on_click(
                            cx.listener(|this, _, window, cx| this.submit_go_to(window, cx)),
                        ),
                    ),
                    dialog_button("dialog-cancel", "Cancel", false, false, accent, s).on_click(
                        cx.listener(|this, _, window, cx| this.close_dialogs(window, cx)),
                    ),
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
                    dialog_button("dialog-ok", "OK", true, true, accent, s).on_click(
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

    fn text_box(
        &self,
        focused: bool,
        has_text: bool,
        s: f32,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let u = |v: f32| px(v * s);
        let accent = self.dialogs.accent;
        div()
            .relative()
            .h(u(CONTROL_HEIGHT))
            .rounded(u(CONTROL_RADIUS))
            .bg(rgb(CONTENT_FILL))
            .border_t_1()
            .border_l_1()
            .border_r_1()
            .border_color(rgb(TEXT_BOX_STROKE))
            .pb(px(1.))
            .child(
                Input::new(&self.dialogs.line)
                    .appearance(false)
                    .h_full()
                    .py(px(0.))
                    .pl(u(TEXT_BOX_PADDING - 1.))
                    .pr(u(CLEAR_BUTTON_WIDTH + CLEAR_BUTTON_RIGHT))
                    .font_family(MENU_FONT)
                    .text_size(u(BODY_SIZE))
                    .line_height(u(BODY_LINE + 2.))
                    .text_color(rgba(TEXT_PRIMARY)),
            )
            .child(self.line_input_caret(window, cx))
            .child(
                div()
                    .absolute()
                    .left(px(-1.))
                    .right(px(-1.))
                    .bottom_0()
                    .h(px(if focused { 2. } else { 1. }))
                    .rounded_b(u(CONTROL_RADIUS))
                    .bg(if focused {
                        accent
                    } else {
                        rgb(TEXT_BOX_STROKE_BOTTOM)
                    }),
            )
            .when(focused && has_text, |this| {
                this.child(
                    div()
                        .id("dialog-clear")
                        .absolute()
                        .top(u(2.))
                        .bottom(u(2.))
                        .right(u(CLEAR_BUTTON_RIGHT))
                        .w(u(CLEAR_BUTTON_WIDTH))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(u(CONTROL_RADIUS))
                        .cursor_default()
                        .font_family("Segoe Fluent Icons")
                        .text_size(u(CLEAR_GLYPH_SIZE))
                        .text_color(rgba(TEXT_SECONDARY))
                        .hover(|style| style.bg(rgba(SUBTLE_FILL_SECONDARY)))
                        .active(|style| style.bg(rgba(SUBTLE_FILL_TERTIARY)))
                        .on_mouse_down(MouseButton::Left, |_, window, cx| {
                            window.prevent_default();
                            cx.stop_propagation();
                        })
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.dialogs.line.update(cx, |input, cx| {
                                input.set_value("", window, cx);
                                input.focus(window, cx);
                            });
                        }))
                        .child("\u{E894}"),
                )
            })
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

fn dialog_button(
    id: &'static str,
    label: &'static str,
    accent_style: bool,
    focused: bool,
    accent: Rgba,
    s: f32,
) -> Stateful<Div> {
    let u = |v: f32| px(v * s);
    let (fill, hover, pressed, edge, bottom, text) = if accent_style {
        let backdrop = (COMMAND_FILL & 0xff) as f32 / 255.;
        (
            accent,
            mix(accent, backdrop, 1. - ACCENT_HOVER_OPACITY),
            mix(accent, backdrop, 1. - ACCENT_PRESSED_OPACITY),
            mix(accent, 1., ACCENT_TOP_EDGE),
            mix(accent, 0., ACCENT_BOTTOM_EDGE),
            rgb(ON_ACCENT),
        )
    } else {
        (
            rgb(BUTTON_FILL),
            rgb(BUTTON_HOVER),
            rgb(BUTTON_PRESSED),
            rgb(BUTTON_STROKE),
            rgb(BUTTON_STROKE_BOTTOM),
            rgba(TEXT_PRIMARY),
        )
    };
    div()
        .id(id)
        .relative()
        .h(u(CONTROL_HEIGHT))
        .rounded(u(CONTROL_RADIUS))
        .bg(bottom)
        .pb(px(1.))
        .cursor_default()
        .child(
            div()
                .id((id, 1usize))
                .flex()
                .items_center()
                .justify_center()
                .size_full()
                .rounded(u(CONTROL_RADIUS))
                .border_t_1()
                .border_l_1()
                .border_r_1()
                .border_color(edge)
                .bg(fill)
                .text_color(text)
                .hover(move |style| style.bg(hover))
                .active(move |style| style.bg(pressed))
                .child(label),
        )
        .when(focused, |this| {
            this.child(
                div()
                    .absolute()
                    .top(px(-3.))
                    .left(px(-3.))
                    .right(px(-3.))
                    .bottom(px(-3.))
                    .rounded(u(CONTROL_RADIUS + 3.))
                    .border_2()
                    .border_color(rgb(FOCUS_OUTER))
                    .child(
                        div()
                            .size_full()
                            .rounded(u(CONTROL_RADIUS + 1.))
                            .border_1()
                            .border_color(rgb(FOCUS_INNER)),
                    ),
            )
        })
}
