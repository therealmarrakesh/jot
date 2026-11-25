//! The controls the windows are built from, drawn the way Windows 11 draws them: buttons
//! (accented or not), subtle icon buttons, text fields, cards and focus rings.

use crate::editor::Editor;
use crate::geom::{Edges, Rgba, Size, black, transparent_black, white};
use crate::platform::CursorStyle;
use crate::theme::{
    BODY_SIZE, BORDER, CONTROL_HEIGHT, CONTROL_RADIUS, ControlColors, FIELD_LINE, ICON_FONT,
    Palette, TEXT_FONT, theme_shadow,
};
use crate::types::SharedStr;
use crate::ui::{AppNode, AppUi, Brushes, HitId, box_, row, text};

/// How an accented button's top edge lightens, and how its face fades on hover and press.
const ACCENT_TOP_EDGE: f32 = 0x14 as f32 / 255.;
const ACCENT_HOVER_OPACITY: f32 = 0.9;
const ACCENT_PRESSED_OPACITY: f32 = 0.8;
const FIELD_PADDING: f32 = 10.;
const FIELD_GLYPH_WIDTH: f32 = 31.;
const FIELD_GLYPH_RIGHT: f32 = 2.;
const FIELD_GLYPH_SIZE: f32 = 12.;
/// A control's face border: its bottom edge is drawn by what's under it.
const NO_BOTTOM_EDGE: Edges = Edges {
    top: 1.,
    right: 1.,
    bottom: 0.,
    left: 1.,
};
const CLEAR_GLYPH: &str = "\u{E894}";
const SEARCH_GLYPH: &str = "\u{E721}";

/// A subtle control's fill: none at rest, a tint on hover and press.
pub(crate) fn subtle_brushes(p: Palette) -> Brushes {
    Brushes {
        rest: transparent_black(),
        hover: p.subtle_secondary,
        pressed: p.subtle_tertiary,
    }
}

/// The text (or glyph) colour on a subtle control, which dims while pressed.
pub(crate) fn subtle_text(p: Palette, enabled: bool, pressed: bool) -> Rgba {
    match (enabled, pressed) {
        (false, _) => p.text_disabled,
        (true, true) => p.text_secondary,
        (true, false) => p.text_primary,
    }
}

/// A raised card: a `stroke` border with `radius` corners and the shadow for elevation `z`,
/// around `face`, which is filled with `fill` inside the border.
pub(crate) fn card(
    p: Palette,
    stroke: Rgba,
    fill: Rgba,
    radius: f32,
    z: f32,
    face: AppNode,
) -> AppNode {
    box_()
        .border(BORDER, stroke)
        .rounded(radius)
        .shadows(theme_shadow(p.dark, z))
        .child(face.rounded(radius - BORDER).bg(fill))
}

/// A push button on `surface`: its face over a darker bottom edge, the accent colour when
/// `accented`, and a focus ring when it has the keyboard's focus.
pub(crate) fn button(
    p: Palette,
    surface: ControlColors,
    hit: HitId,
    label: impl Into<SharedStr>,
    accented: bool,
    focused: bool,
    ui: &mut AppUi,
) -> AppNode {
    let accent = p.accent;
    let (rest, hover, pressed_fill, edge, bottom, text_color) = if accented {
        (
            accent,
            accent.mix_opaque(surface.panel, 1. - ACCENT_HOVER_OPACITY),
            accent.mix_opaque(surface.panel, 1. - ACCENT_PRESSED_OPACITY),
            accent.mix_opaque(white(), ACCENT_TOP_EDGE),
            accent.mix_opaque(black(), p.accent_bottom_edge),
            p.on_accent,
        )
    } else {
        let s = surface;
        (
            s.fill,
            s.hover,
            s.button_pressed,
            s.button_stroke,
            s.button_bottom,
            p.text_primary,
        )
    };
    // Pressed, the edges flatten: into the face for an accented button, into the border else.
    let (edge, bottom, text_color) = match (ui.is_pressed(hit), accented) {
        (false, _) => (edge, bottom, text_color),
        (true, true) => (pressed_fill, pressed_fill, p.on_accent_secondary),
        (true, false) => (edge, edge, p.text_secondary),
    };
    let brushes = Brushes {
        rest,
        hover,
        pressed: pressed_fill,
    };
    let face = row()
        .items_center()
        .justify_center()
        .size_full()
        .rounded(CONTROL_RADIUS)
        .border_widths(NO_BOTTOM_EDGE, edge)
        .bg(ui.brush(hit, brushes, true))
        .text_color(text_color)
        .child(text(label));
    let ring = focused && ui.last_input_was_keyboard;
    box_()
        .hit(hit)
        .h(CONTROL_HEIGHT)
        .rounded(CONTROL_RADIUS)
        .bg(bottom)
        .pb(1.)
        .cursor(CursorStyle::Arrow)
        .font_family(TEXT_FONT)
        .text_size(BODY_SIZE)
        .child(face)
        .when(ring, |this| {
            this.child(focus_ring(p, Edges::all(3.), CONTROL_RADIUS + 3.))
        })
}

/// A subtle button showing an icon `glyph` (at its size), `dims` big, with tooltip `tip`.
pub(crate) fn icon_button(
    p: Palette,
    hit: HitId,
    tip: impl Into<SharedStr>,
    (glyph, glyph_size): (&'static str, f32),
    dims: Size,
    enabled: bool,
    ui: &mut AppUi,
) -> AppNode {
    let bg = ui.brush(hit, subtle_brushes(p), enabled);
    let fg = subtle_text(p, enabled, ui.is_pressed(hit));
    row()
        .hit(hit)
        .flex_none()
        .items_center()
        .justify_center()
        .w(dims.width)
        .h(dims.height)
        .rounded(CONTROL_RADIUS)
        .bg(bg)
        .cursor(CursorStyle::Arrow)
        .font_family(ICON_FONT)
        .text_size(glyph_size)
        .line_height(glyph_size)
        .text_color(fg)
        .tooltip(tip)
        .child(text(glyph))
}

/// The keyboard focus ring, `outset` around its parent: a strong outer line, a light inner one.
pub(crate) fn focus_ring(p: Palette, outset: Edges<f32>, radius: f32) -> AppNode {
    let inner = box_()
        .size_full()
        .rounded((radius - 2.).max(0.))
        .border(1., p.focus_inner);
    box_()
        .absolute()
        .top(-outset.top)
        .left(-outset.left)
        .right(-outset.right)
        .bottom(-outset.bottom)
        .rounded(radius)
        .border(2., p.focus_outer)
        .child(inner)
}

/// A text field for `editor` on `surface`: its bottom line turns accent and thick when it has
/// focus, a search glyph sits at its end if asked for, and a clear button once it has text and
/// focus.
pub(crate) fn text_field(
    p: Palette,
    surface: ControlColors,
    editor: &Editor,
    focused: bool,
    search_icon: bool,
    ui: &AppUi,
) -> AppNode {
    let (field_hit, clear_hit) = (HitId::Editor(editor.slot), HitId::FieldClear(editor.slot));
    let empty = editor.model().text().len() == 0;
    let glyph = |glyph: &'static str, right: f32| {
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
    };
    // The clear button sits left of the search glyph, if there is one.
    let glyphs = if search_icon { 2. } else { 1. };
    let trailing = glyphs * FIELD_GLYPH_WIDTH + FIELD_GLYPH_RIGHT;
    let clear_right = trailing - FIELD_GLYPH_WIDTH;
    let bg = if focused {
        p.field_focused
    } else if ui.is_hovered(field_hit) {
        surface.hover
    } else {
        surface.fill
    };
    let text_row = row()
        .h_full()
        .items_center()
        .pl(FIELD_PADDING - 1.)
        .pr(trailing)
        .child(editor.view(FIELD_LINE));
    let underline = box_()
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
        });
    box_()
        .hit(field_hit)
        .cursor(CursorStyle::IBeam)
        .h(CONTROL_HEIGHT)
        .rounded(CONTROL_RADIUS)
        .overflow_hidden()
        .bg(bg)
        .border_widths(NO_BOTTOM_EDGE, surface.field_stroke)
        .pb(1.)
        .child(text_row)
        .child(underline)
        .when(search_icon, |this| {
            let search = glyph(SEARCH_GLYPH, FIELD_GLYPH_RIGHT);
            this.child(search.when(!focused, |g| g.text_color(p.text_primary)))
        })
        .when(focused && !empty, |this| {
            let pressed = ui.is_pressed(clear_hit);
            let clear = glyph(CLEAR_GLYPH, clear_right)
                .hit(clear_hit)
                .when(pressed, |g| {
                    g.bg(p.subtle_tertiary).text_color(p.text_tertiary)
                })
                .when(!pressed && ui.is_hovered(clear_hit), |g| {
                    g.bg(p.subtle_secondary)
                });
            this.child(clear)
        })
}
