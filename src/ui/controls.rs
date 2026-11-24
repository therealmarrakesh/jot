use std::hash::Hash;

use crate::color::{Rgba, black, transparent_black, white};
use crate::geom::Edges;
use crate::platform::CursorStyle;
use crate::theme::{Palette, Surface};
use crate::types::SharedStr;

use super::scene::{Brushes, Node, UiState, box_, row, text};

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

pub(crate) fn subtle_brushes(p: Palette) -> Brushes {
    Brushes {
        rest: transparent_black(),
        hover: p.subtle_secondary,
        pressed: p.subtle_tertiary,
    }
}

#[derive(Clone, Copy)]
pub(crate) struct ButtonStyle {
    pub(crate) surface: Surface,
    pub(crate) accent: Rgba,
}

pub(crate) fn button<H: Copy + Eq + Hash, C: Copy, K: Copy + Eq + Hash + From<H>>(
    p: Palette,
    style: ButtonStyle,
    hit: H,
    label: impl Into<SharedStr>,
    accented: bool,
    focused: bool,
    ui: &mut UiState<H, C, K>,
) -> Node<H, C> {
    let ButtonStyle { surface, accent } = style;
    let ring = focused && ui.last_input_was_keyboard();
    let pressed = ui.is_pressed(hit);

    let (fill, hover, pressed_fill, edge, bottom, text_color) = if accented {
        let panel = surface.panel;
        (
            accent,
            accent.mix_opaque(panel, 1. - ACCENT_HOVER_OPACITY),
            accent.mix_opaque(panel, 1. - ACCENT_PRESSED_OPACITY),
            accent.mix_opaque(white(), ACCENT_TOP_EDGE),
            accent.mix_opaque(black(), p.accent_bottom_edge),
            p.on_accent,
        )
    } else {
        (
            surface.fill,
            surface.hover,
            surface.button_pressed,
            surface.button_stroke,
            surface.button_bottom,
            p.text_primary,
        )
    };
    let (pressed_edge, pressed_bottom, pressed_text) = if accented {
        (pressed_fill, pressed_fill, p.on_accent_secondary)
    } else {
        (edge, edge, p.text_secondary)
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
        .h(CONTROL_HEIGHT)
        .rounded(CONTROL_RADIUS)
        .bg(if pressed { pressed_bottom } else { bottom })
        .pb(1.)
        .cursor(CursorStyle::Arrow)
        .font_family(TEXT_FONT)
        .text_size(BODY_SIZE)
        .child(
            row()
                .items_center()
                .justify_center()
                .size_full()
                .rounded(CONTROL_RADIUS)
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
            this.child(focus_ring(p, Edges::all(3.), CONTROL_RADIUS + 3.))
        })
}

#[derive(Clone, Copy)]
pub(crate) struct IconButtonStyle {
    pub(crate) glyph_size: f32,
    pub(crate) width: f32,
    pub(crate) height: f32,
    pub(crate) radius: f32,
}

pub(crate) fn icon_button<H: Copy + Eq + Hash, C: Copy, K: Copy + Eq + Hash + From<H>>(
    p: Palette,
    hit: H,
    tip: impl Into<SharedStr>,
    glyph: &'static str,
    style: IconButtonStyle,
    enabled: bool,
    ui: &mut UiState<H, C, K>,
) -> Node<H, C> {
    let bg = ui.brush(hit, subtle_brushes(p), enabled);
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
        .text_color(fg)
        .tooltip(tip)
        .child(text(glyph))
}

pub(crate) fn focus_ring<H, C>(p: Palette, outset: Edges<f32>, radius: f32) -> Node<H, C> {
    box_()
        .absolute()
        .top(-outset.top)
        .left(-outset.left)
        .right(-outset.right)
        .bottom(-outset.bottom)
        .rounded(radius)
        .border(2., p.focus_outer)
        .child(
            box_()
                .size_full()
                .rounded((radius - 2.).max(0.))
                .border(1., p.focus_inner),
        )
}

fn field_glyph<H, C>(p: Palette, glyph: &'static str, right: f32) -> Node<H, C> {
    row()
        .absolute()
        .top(2.)
        .bottom(2.)
        .right(right)
        .w(FIELD_GLYPH_WIDTH)
        .items_center()
        .justify_center()
        .rounded(CONTROL_RADIUS)
        .cursor(CursorStyle::Arrow)
        .font_family(ICON_FONT)
        .text_size(FIELD_GLYPH_SIZE)
        .text_color(p.text_secondary)
        .child(text(glyph))
}

#[derive(Clone, Copy)]
pub(crate) struct TextFieldStyle {
    pub(crate) surface: Surface,
    pub(crate) focused: bool,
    pub(crate) empty: bool,
    pub(crate) search_icon: bool,
}

pub(crate) fn text_field_frame<H: Copy + Eq + Hash, C: Copy, K: Copy + Eq + Hash>(
    p: Palette,
    field_hit: H,
    clear_hit: H,
    style: TextFieldStyle,
    editor_node: Node<H, C>,
    ui: &UiState<H, C, K>,
) -> Node<H, C> {
    let TextFieldStyle {
        surface,
        focused,
        empty,
        search_icon,
    } = style;
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
        p.field_focused
    } else if hovered {
        surface.hover
    } else {
        surface.fill
    };
    let clear_hovered = ui.is_hovered(clear_hit);
    let clear_pressed = ui.is_pressed(clear_hit);

    box_()
        .hit(field_hit)
        .cursor(CursorStyle::IBeam)
        .h(CONTROL_HEIGHT)
        .rounded(CONTROL_RADIUS)
        .overflow_hidden()
        .bg(bg)
        .border_widths(
            Edges {
                top: 1.,
                left: 1.,
                right: 1.,
                bottom: 0.,
            },
            surface.field_stroke,
        )
        .pb(1.)
        .child(
            row()
                .h_full()
                .items_center()
                .pl(FIELD_PADDING - 1.)
                .pr(trailing)
                .font_family(TEXT_FONT)
                .text_size(BODY_SIZE)
                .line_height(BODY_LINE + 2.)
                .text_color(p.text_primary)
                .child(editor_node),
        )
        .child(
            box_()
                .absolute()
                .left(0.)
                .right(0.)
                .bottom(0.)
                .h(if focused { 2. } else { 1. })
                .rounded_b(CONTROL_RADIUS)
                .bg(if focused {
                    p.accent
                } else {
                    surface.field_bottom
                }),
        )
        .when(search_icon, |this| {
            this.child(
                field_glyph(p, SEARCH_GLYPH, FIELD_GLYPH_RIGHT)
                    .when(!focused, |g| g.text_color(p.text_primary)),
            )
        })
        .when(focused && !empty, |this| {
            this.child(
                field_glyph(p, CLEAR_GLYPH, clear_right)
                    .hit(clear_hit)
                    .when(clear_pressed, |g| {
                        g.bg(p.subtle_tertiary).text_color(p.text_tertiary)
                    })
                    .when(!clear_pressed && clear_hovered, |g| {
                        g.bg(p.subtle_secondary)
                    }),
            )
        })
}
