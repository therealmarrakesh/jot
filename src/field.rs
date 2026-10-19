use super::*;
use gpui_kit::component::input::{Input, InputState};

pub(crate) const CONTROL_HEIGHT: f32 = 32.;
pub(crate) const CONTROL_RADIUS: f32 = 4.;
pub(crate) const OVERLAY_RADIUS: f32 = 8.;
pub(crate) const BODY_SIZE: f32 = 14.;
pub(crate) const BODY_LINE: f32 = 18.;
const ACCENT_TOP_EDGE: f32 = 0x14 as f32 / 255.;
const ACCENT_HOVER_OPACITY: f32 = 0.9;
const ACCENT_PRESSED_OPACITY: f32 = 0.8;
const FIELD_PADDING: f32 = 10.;
const FIELD_CARET: f32 = 19.;
const FIELD_GLYPH_WIDTH: f32 = 31.;
const FIELD_GLYPH_RIGHT: f32 = 2.;
const FIELD_GLYPH_SIZE: f32 = 12.;
const CLEAR_GLYPH: &str = "\u{E894}";
const SEARCH_GLYPH: &str = "\u{E721}";

fn mix(color: Rgba, toward: Rgba, amount: f32) -> Rgba {
    let blend = |c: f32, t: f32| c + (t - c) * amount;
    Rgba {
        r: blend(color.r, toward.r),
        g: blend(color.g, toward.g),
        b: blend(color.b, toward.b),
        a: 1.,
    }
}

#[derive(Clone, Copy)]
pub(crate) struct ButtonStyle {
    pub surface: Surface,
    pub accent: Rgba,
    pub scale: f32,
}

pub(crate) fn button(
    p: Palette,
    style: ButtonStyle,
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    accented: bool,
    focus: Option<&FocusHandle>,
    window: &Window,
) -> Stateful<Div> {
    let ButtonStyle {
        surface,
        accent,
        scale: s,
    } = style;
    let u = |v: f32| px(v * s);
    let ring =
        focus.is_some_and(|focus| focus.is_focused(window) && window.last_input_was_keyboard());
    let (fill, hover, pressed, edge, bottom, text) = if accented {
        let panel = rgb(surface.panel);
        (
            accent,
            mix(accent, panel, 1. - ACCENT_HOVER_OPACITY),
            mix(accent, panel, 1. - ACCENT_PRESSED_OPACITY),
            mix(accent, rgb(0xffffff), ACCENT_TOP_EDGE),
            mix(accent, rgb(0x000000), p.accent_bottom_edge),
            rgb(p.on_accent),
        )
    } else {
        (
            rgb(surface.fill),
            rgb(surface.hover),
            rgb(surface.button_pressed),
            rgb(surface.button_stroke),
            rgb(surface.button_bottom),
            rgba(p.text_primary),
        )
    };
    let (pressed_edge, pressed_bottom, pressed_text) = if accented {
        (pressed, pressed, rgba(p.on_accent_secondary))
    } else {
        (edge, edge, rgba(p.text_secondary))
    };
    div()
        .id(id)
        .when_some(focus, |this, focus| this.track_focus(focus))
        .relative()
        .h(u(CONTROL_HEIGHT))
        .rounded(u(CONTROL_RADIUS))
        .bg(bottom)
        .active(move |style| style.bg(pressed_bottom))
        .pb(px(1.))
        .cursor_default()
        .font_family(TEXT_FONT)
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
                .text_color(text)
                .child(label.into())
                .brush_transition(Brushes {
                    rest: fill,
                    hover,
                    pressed,
                })
                .active(move |style| style.border_color(pressed_edge).text_color(pressed_text)),
        )
        .when(ring, |this| {
            this.child(focus_ring(p, Edges::all(3.), u(CONTROL_RADIUS + 3.)))
        })
}

pub(crate) fn icon_button(
    p: Palette,
    id: impl Into<ElementId>,
    glyph: &'static str,
    glyph_size: Pixels,
    size: gpui::Size<Pixels>,
    radius: Pixels,
    enabled: bool,
) -> brush::BrushTransition {
    div()
        .id(id)
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .w(size.width)
        .h(size.height)
        .rounded(radius)
        .cursor_default()
        .font_family(ICON_FONT)
        .text_size(glyph_size)
        .line_height(glyph_size)
        .text_color(rgba(if enabled {
            p.text_primary
        } else {
            p.text_disabled
        }))
        .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
        .child(glyph)
        .brush_transition(if enabled {
            Brushes::subtle(p)
        } else {
            Brushes::solid(transparent_black().into())
        })
        .when(enabled, |this| {
            this.active(move |style| style.text_color(rgba(p.text_secondary)))
        })
}

pub(crate) fn focus_ring(p: Palette, outset: Edges<f32>, radius: Pixels) -> Div {
    div()
        .absolute()
        .top(px(-outset.top))
        .left(px(-outset.left))
        .right(px(-outset.right))
        .bottom(px(-outset.bottom))
        .rounded(radius)
        .border_2()
        .border_color(rgb(p.focus_outer))
        .child(
            div()
                .size_full()
                .rounded(radius - px(2.))
                .border_1()
                .border_color(rgb(p.focus_inner)),
        )
}

fn field_glyph(
    p: Palette,
    id: impl Into<ElementId>,
    glyph: &'static str,
    right: f32,
    s: f32,
) -> Stateful<Div> {
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
        .font_family(ICON_FONT)
        .text_size(u(FIELD_GLYPH_SIZE))
        .text_color(rgba(p.text_secondary))
        .child(glyph)
}

fn field_caret(input: &Entity<InputState>, window: &Window, cx: &App) -> impl IntoElement + use<> {
    let input = input.clone();
    let shown = input.read(cx).show_cursor(window, cx);
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
    input: &Entity<InputState>,
    search_icon: bool,
    surface: Surface,
    s: f32,
    window: &Window,
    cx: &App,
) -> Div {
    let p = palette(cx);
    let u = |v: f32| px(v * s);
    let focused = input.focus_handle(cx).is_focused(window);
    let empty = input.read(cx).text().len() == 0;
    let id = input.entity_id();
    let (trailing, clear_right) = if search_icon {
        (
            2. * FIELD_GLYPH_WIDTH + FIELD_GLYPH_RIGHT,
            FIELD_GLYPH_RIGHT + FIELD_GLYPH_WIDTH - 1.,
        )
    } else {
        (FIELD_GLYPH_WIDTH + FIELD_GLYPH_RIGHT, FIELD_GLYPH_RIGHT)
    };
    div()
        .relative()
        .h(u(CONTROL_HEIGHT))
        .rounded(u(CONTROL_RADIUS))
        .bg(if focused {
            rgba(p.field_focused)
        } else {
            rgb(surface.fill)
        })
        .border_t_1()
        .border_l_1()
        .border_r_1()
        .border_color(rgb(surface.field_stroke))
        .when(!focused, |this| {
            this.hover(move |style| style.bg(rgb(surface.hover)))
        })
        .pb(px(1.))
        .child(
            Input::new(input)
                .appearance(false)
                .h_full()
                .py(px(0.))
                .pl(u(FIELD_PADDING - 1.))
                .pr(u(trailing))
                .font_family(TEXT_FONT)
                .text_size(u(BODY_SIZE))
                .line_height(u(BODY_LINE + 2.))
                .text_color(rgba(p.text_primary)),
        )
        .child(field_caret(input, window, cx))
        .child(bottom_edge(
            focused,
            if focused {
                rgb(p.accent)
            } else {
                rgb(surface.field_bottom)
            },
            s,
        ))
        .when(search_icon, |this| {
            this.child(
                field_glyph(p, ("field-search", id), SEARCH_GLYPH, FIELD_GLYPH_RIGHT, s)
                    .when(!focused, |this| this.text_color(rgba(p.text_primary))),
            )
        })
        .when(focused && !empty, |this| {
            this.child(clear_button(p, input.clone(), clear_right, s))
        })
}

fn bottom_edge(focused: bool, color: Rgba, s: f32) -> Div {
    div()
        .absolute()
        .left(px(-1.))
        .right(px(-1.))
        .bottom_0()
        .h(px(if focused { 2. } else { 1. }))
        .rounded_b(px(CONTROL_RADIUS * s))
        .bg(color)
}

fn clear_button(p: Palette, input: Entity<InputState>, right: f32, s: f32) -> Stateful<Div> {
    field_glyph(p, ("field-clear", input.entity_id()), CLEAR_GLYPH, right, s)
        .hover(move |style| style.bg(rgba(p.subtle_secondary)))
        .active(move |style| {
            style
                .bg(rgba(p.subtle_tertiary))
                .text_color(rgba(p.text_tertiary))
        })
        .on_mouse_down(MouseButton::Left, |_, window, cx| {
            window.prevent_default();
            cx.stop_propagation();
        })
        .on_click(move |_, window, cx| {
            input.update(cx, |input, cx| {
                input.set_value("", window, cx);
                input.focus(window, cx);
            });
        })
}
