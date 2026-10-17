use super::*;
use gpui_kit::component::input::{Input, InputState};
use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_BINARY, RegGetValueW};

pub(crate) const CONTROL_HEIGHT: f32 = 32.;
pub(crate) const CONTROL_RADIUS: f32 = 4.;
pub(crate) const BODY_SIZE: f32 = 14.;
pub(crate) const BODY_LINE: f32 = 18.;
const DEFAULT_ACCENT: u32 = 0x005fb8;
const ON_ACCENT: u32 = 0xffffff;
const ACCENT_TOP_EDGE: f32 = 0x14 as f32 / 255.;
const ACCENT_BOTTOM_EDGE: f32 = 0x66 as f32 / 255.;
const ACCENT_HOVER_OPACITY: f32 = 0.9;
const ACCENT_PRESSED_OPACITY: f32 = 0.8;
const FOCUS_OUTER: u32 = 0x1a1a1a;
const FOCUS_INNER: u32 = 0xfbfbfb;
const FIELD_FOCUSED_FILL: u32 = 0xffffff;
const FIELD_PADDING: f32 = 10.;
const FIELD_CARET: f32 = 19.;
const FIELD_GLYPH_WIDTH: f32 = 31.;
const FIELD_GLYPH_RIGHT: f32 = 2.;
const FIELD_GLYPH_SIZE: f32 = 12.;
const CLEAR_GLYPH: &str = "\u{E894}";
const SEARCH_GLYPH: &str = "\u{E721}";

#[derive(Clone, Copy)]
pub(crate) struct Surface {
    pub field_fill: u32,
    pub field_stroke: u32,
    pub field_bottom: u32,
    pub button_fill: u32,
    pub button_hover: u32,
    pub button_pressed: u32,
    pub button_stroke: u32,
    pub button_bottom: u32,
}

pub(crate) struct FieldStyle {
    pub placeholder: Option<&'static str>,
    pub search_icon: bool,
}

pub(crate) fn accent_color() -> Rgba {
    let mut palette = [0u8; 32];
    let mut len = palette.len() as u32;
    let key: Vec<u16> = r"Software\Microsoft\Windows\CurrentVersion\Explorer\Accent"
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let value: Vec<u16> = "AccentPalette".encode_utf16().chain(Some(0)).collect();
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

pub(crate) fn button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    accent_style: bool,
    focused: bool,
    surface: Surface,
    accent: Rgba,
    s: f32,
) -> Stateful<Div> {
    let u = |v: f32| px(v * s);
    let (fill, hover, pressed, edge, bottom, text) = if accent_style {
        let backdrop = (surface.button_fill & 0xff) as f32 / 255.;
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
            rgb(surface.button_fill),
            rgb(surface.button_hover),
            rgb(surface.button_pressed),
            rgb(surface.button_stroke),
            rgb(surface.button_bottom),
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
        .font_family(MENU_FONT)
        .text_size(u(BODY_SIZE))
        .child(
            div()
                .id("face")
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
                .child(label.into()),
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

fn field_glyph(id: impl Into<ElementId>, glyph: &'static str, right: f32, s: f32) -> Stateful<Div> {
    let u = |v: f32| px(v * s);
    div()
        .id(id)
        .absolute()
        .top(u(2.))
        .bottom(u(2.))
        .right(u(right))
        .w(u(FIELD_GLYPH_WIDTH))
        .flex()
        .items_center()
        .justify_center()
        .rounded(u(CONTROL_RADIUS))
        .cursor_default()
        .font_family("Segoe Fluent Icons")
        .text_size(u(FIELD_GLYPH_SIZE))
        .text_color(rgba(TEXT_SECONDARY))
        .child(glyph)
}

impl Jot {
    pub(crate) fn watch_field(input: &Entity<InputState>, cx: &mut Context<Self>) -> Subscription {
        cx.observe(input, |jot, input, cx| {
            let id = input.entity_id();
            let input = input.read(cx);
            let key = (id, input.cursor(), input.selected_range());
            if jot.field_caret.as_ref() != Some(&key) {
                jot.field_caret = Some(key);
                jot.restart_caret_blink(cx);
            }
        })
    }

    fn field_caret(
        &self,
        input: &Entity<InputState>,
        window: &Window,
        cx: &App,
    ) -> impl IntoElement + use<> {
        let input = input.clone();
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
            px(FIELD_CARET),
            shown,
        )
    }

    pub(crate) fn text_field(
        &self,
        input: &Entity<InputState>,
        style: FieldStyle,
        surface: Surface,
        s: f32,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let u = |v: f32| px(v * s);
        let focused = input.focus_handle(cx).is_focused(window);
        let empty = input.read(cx).text().len() == 0;
        let id = input.entity_id().as_u64() as usize;
        let trailing = if style.search_icon {
            2. * FIELD_GLYPH_WIDTH + FIELD_GLYPH_RIGHT
        } else {
            FIELD_GLYPH_WIDTH + FIELD_GLYPH_RIGHT
        };
        let clear_right = if style.search_icon {
            FIELD_GLYPH_RIGHT + FIELD_GLYPH_WIDTH - 1.
        } else {
            FIELD_GLYPH_RIGHT
        };
        let field = input.clone();
        div()
            .relative()
            .h(u(CONTROL_HEIGHT))
            .rounded(u(CONTROL_RADIUS))
            .bg(rgb(if focused {
                FIELD_FOCUSED_FILL
            } else {
                surface.field_fill
            }))
            .border_t_1()
            .border_l_1()
            .border_r_1()
            .border_color(rgb(surface.field_stroke))
            .pb(px(1.))
            .child(
                Input::new(input)
                    .appearance(false)
                    .h_full()
                    .py(px(0.))
                    .pl(u(FIELD_PADDING - 1.))
                    .pr(u(trailing))
                    .font_family(MENU_FONT)
                    .text_size(u(BODY_SIZE))
                    .line_height(u(BODY_LINE + 2.))
                    .text_color(rgba(TEXT_PRIMARY)),
            )
            .when_some(style.placeholder.filter(|_| empty), |this, placeholder| {
                this.child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left(u(FIELD_PADDING - 1.))
                        .flex()
                        .items_center()
                        .font_family(MENU_FONT)
                        .text_size(u(BODY_SIZE))
                        .text_color(rgba(TEXT_SECONDARY))
                        .child(placeholder),
                )
            })
            .child(self.field_caret(input, window, cx))
            .child(
                div()
                    .absolute()
                    .left(px(-1.))
                    .right(px(-1.))
                    .bottom_0()
                    .h(px(if focused { 2. } else { 1. }))
                    .rounded_b(u(CONTROL_RADIUS))
                    .bg(if focused {
                        self.accent
                    } else {
                        rgb(surface.field_bottom)
                    }),
            )
            .when(style.search_icon, |this| {
                this.child(field_glyph(
                    ("field-search", id),
                    SEARCH_GLYPH,
                    FIELD_GLYPH_RIGHT,
                    s,
                ))
            })
            .when(focused && !empty, |this| {
                this.child(
                    field_glyph(("field-clear", id), CLEAR_GLYPH, clear_right, s)
                        .hover(|style| style.bg(rgba(SUBTLE_FILL_SECONDARY)))
                        .active(|style| style.bg(rgba(SUBTLE_FILL_TERTIARY)))
                        .on_mouse_down(MouseButton::Left, |_, window, cx| {
                            window.prevent_default();
                            cx.stop_propagation();
                        })
                        .on_click(move |_, window, cx| {
                            field.update(cx, |input, cx| {
                                input.set_value("", window, cx);
                                input.focus(window, cx);
                            });
                        }),
                )
            })
    }
}
