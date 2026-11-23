//! Shared Windows 11 Fluent controls built directly on `ui::scene`.

use crate::color::{Rgba, rgb, rgba};
use crate::geom::Edges;
use crate::platform::CursorStyle;
use crate::theme::{Palette, Surface};
use crate::types::SharedStr;

use super::scene::{Brushes, EditorSlot, HitId, Node, UiState, box_, row, text};

pub(crate) const TEXT_FONT: &str = "Segoe UI Variable Text";
pub(crate) const SMALL_FONT: &str = "Segoe UI Variable Small";
pub(crate) const ICON_FONT: &str = "Segoe Fluent Icons";

pub(crate) const CONTROL_HEIGHT: f32 = 32.;
pub(crate) const CONTROL_RADIUS: f32 = 4.;
pub(crate) const OVERLAY_RADIUS: f32 = 8.;
pub(crate) const BODY_SIZE: f32 = 14.;
pub(crate) const BODY_LINE: f32 = 18.;
pub(crate) const FIELD_CARET: f32 = 19.;

const ACCENT_TOP_EDGE: f32 = 0x14 as f32 / 255.;
const ACCENT_HOVER_OPACITY: f32 = 0.9;
const ACCENT_PRESSED_OPACITY: f32 = 0.8;
const FIELD_PADDING: f32 = 10.;
const FIELD_GLYPH_WIDTH: f32 = 31.;
const FIELD_GLYPH_RIGHT: f32 = 2.;
const FIELD_GLYPH_SIZE: f32 = 12.;
const CLEAR_GLYPH: &str = "\u{E894}";
const SEARCH_GLYPH: &str = "\u{E721}";

#[derive(Clone, Copy)]
pub(crate) struct ButtonStyle {
    pub(crate) surface: Surface,
    pub(crate) accent: Rgba,
    pub(crate) scale: f32,
}

pub(crate) fn button(
    p: Palette,
    style: ButtonStyle,
    hit: HitId,
    label: impl Into<SharedStr>,
    accented: bool,
    focused: bool,
    ui: &mut UiState,
) -> Node {
    let ButtonStyle {
        surface,
        accent,
        scale: s,
    } = style;
    let u = |v: f32| v * s;
    let ring = focused && ui.last_input_was_keyboard;
    let pressed = ui.is_pressed(hit);

    let (fill, hover, pressed_fill, edge, bottom, text_color) = if accented {
        let panel = rgb(surface.panel);
        (
            accent,
            accent.mix_opaque(panel, 1. - ACCENT_HOVER_OPACITY),
            accent.mix_opaque(panel, 1. - ACCENT_PRESSED_OPACITY),
            accent.mix_opaque(rgb(0xffffff), ACCENT_TOP_EDGE),
            accent.mix_opaque(rgb(0x000000), p.accent_bottom_edge),
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
        (pressed_fill, pressed_fill, rgba(p.on_accent_secondary))
    } else {
        (edge, edge, rgba(p.text_secondary))
    };

    let face_bg = ui.brush(
        hit,
        Brushes {
            rest: fill,
            hover,
            pressed: pressed_fill,
        },
        true,
    );

    box_()
        .hit(hit)
        .relative()
        .h(u(CONTROL_HEIGHT))
        .rounded(u(CONTROL_RADIUS))
        .bg(if pressed { pressed_bottom } else { bottom })
        .pb(1.)
        .cursor(CursorStyle::Arrow)
        .font_family(TEXT_FONT)
        .text_size(u(BODY_SIZE))
        .child(
            row()
                .items_center()
                .justify_center()
                .size_full()
                .rounded(u(CONTROL_RADIUS))
                .border_widths(
                    Edges {
                        top: 1.,
                        left: 1.,
                        right: 1.,
                        bottom: 0.,
                    },
                    if pressed { pressed_edge } else { edge },
                )
                .bg(face_bg)
                .text_color(if pressed { pressed_text } else { text_color })
                .child(text(label)),
        )
        .when(ring, |this| {
            this.child(focus_ring(p, Edges::all(3.), u(CONTROL_RADIUS + 3.)))
        })
}

#[derive(Clone, Copy)]
pub(crate) struct IconButtonStyle {
    pub(crate) glyph_size: f32,
    pub(crate) width: f32,
    pub(crate) height: f32,
    pub(crate) radius: f32,
}

pub(crate) fn icon_button(
    p: Palette,
    hit: HitId,
    tip: impl Into<SharedStr>,
    glyph: &'static str,
    style: IconButtonStyle,
    enabled: bool,
    ui: &mut UiState,
) -> Node {
    let bg = ui.brush(hit, Brushes::subtle(p), enabled);
    let fg = if !enabled {
        p.text_disabled
    } else if ui.is_pressed(hit) {
        p.text_secondary
    } else {
        p.text_primary
    };
    row()
        .hit(hit)
        .flex_none()
        .items_center()
        .justify_center()
        .w(style.width)
        .h(style.height)
        .rounded(style.radius)
        .bg(bg)
        .cursor(CursorStyle::Arrow)
        .font_family(ICON_FONT)
        .text_size(style.glyph_size)
        .line_height(style.glyph_size)
        .text_color(rgba(fg))
        .tooltip(tip)
        .child(text(glyph))
}

pub(crate) fn focus_ring(p: Palette, outset: Edges<f32>, radius: f32) -> Node {
    box_()
        .absolute()
        .top(-outset.top)
        .left(-outset.left)
        .right(-outset.right)
        .bottom(-outset.bottom)
        .rounded(radius)
        .border(2., rgb(p.focus_outer))
        .child(
            box_()
                .size_full()
                .rounded((radius - 2.).max(0.))
                .border(1., rgb(p.focus_inner)),
        )
}

fn field_glyph(p: Palette, glyph: &'static str, right: f32, s: f32) -> Node {
    let u = |v: f32| v * s;
    row()
        .absolute()
        .top(u(2.))
        .bottom(u(2.))
        .right(u(right))
        .w(u(FIELD_GLYPH_WIDTH))
        .items_center()
        .justify_center()
        .rounded(u(CONTROL_RADIUS))
        .cursor(CursorStyle::Arrow)
        .font_family(ICON_FONT)
        .text_size(u(FIELD_GLYPH_SIZE))
        .text_color(rgba(p.text_secondary))
        .child(text(glyph))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn text_field_frame(
    p: Palette,
    slot: EditorSlot,
    surface: Surface,
    scale: f32,
    focused: bool,
    empty: bool,
    search_icon: bool,
    editor_node: Node,
    ui: &UiState,
) -> Node {
    let u = |v: f32| v * scale;
    let field_hit = HitId::Editor(slot);
    let hovered = ui.is_hovered(field_hit);
    let (trailing, clear_right) = if search_icon {
        (
            2. * FIELD_GLYPH_WIDTH + FIELD_GLYPH_RIGHT,
            FIELD_GLYPH_RIGHT + FIELD_GLYPH_WIDTH,
        )
    } else {
        (FIELD_GLYPH_WIDTH + FIELD_GLYPH_RIGHT, FIELD_GLYPH_RIGHT)
    };
    let bg = if focused {
        rgba(p.field_focused)
    } else if hovered {
        rgb(surface.hover)
    } else {
        rgb(surface.fill)
    };
    let clear_hit = HitId::FieldClear(slot);
    let clear_hovered = ui.is_hovered(clear_hit);
    let clear_pressed = ui.is_pressed(clear_hit);

    box_()
        .hit(field_hit)
        .cursor(CursorStyle::IBeam)
        .relative()
        .h(u(CONTROL_HEIGHT))
        .rounded(u(CONTROL_RADIUS))
        .overflow_hidden()
        .bg(bg)
        .border_widths(
            Edges {
                top: 1.,
                left: 1.,
                right: 1.,
                bottom: 0.,
            },
            rgb(surface.field_stroke),
        )
        .pb(1.)
        .child(
            row()
                .h_full()
                .items_center()
                .pl(u(FIELD_PADDING - 1.))
                .pr(u(trailing))
                .font_family(TEXT_FONT)
                .text_size(u(BODY_SIZE))
                .line_height(u(BODY_LINE + 2.))
                .text_color(rgba(p.text_primary))
                .child(editor_node),
        )
        .child(
            box_()
                .absolute()
                .left(0.)
                .right(0.)
                .bottom(0.)
                .h(if focused { 2. } else { 1. })
                .rounded_b(CONTROL_RADIUS * scale)
                .bg(if focused {
                    rgb(p.accent)
                } else {
                    rgb(surface.field_bottom)
                }),
        )
        .when(search_icon, |this| {
            this.child(
                field_glyph(p, SEARCH_GLYPH, FIELD_GLYPH_RIGHT, scale)
                    .when(!focused, |g| g.text_color(rgba(p.text_primary))),
            )
        })
        .when(focused && !empty, |this| {
            this.child(
                field_glyph(p, CLEAR_GLYPH, clear_right, scale)
                    .hit(clear_hit)
                    .when(clear_pressed, |g| {
                        g.bg(rgba(p.subtle_tertiary))
                            .text_color(rgba(p.text_tertiary))
                    })
                    .when(!clear_pressed && clear_hovered, |g| {
                        g.bg(rgba(p.subtle_secondary))
                    }),
            )
        })
}
