use crate::color::{Rgba, black, rgb, rgba, transparent_black};
use crate::geom::{BoxShadow, Shadows, point};
use crate::platform::HighContrastColors;

pub(crate) const TEXT_FONT: &str = "Segoe UI Variable Text";
pub(crate) const SMALL_FONT: &str = "Segoe UI Variable Small";
pub(crate) const ICON_FONT: &str = "Segoe Fluent Icons";

pub(crate) const CONTROL_HEIGHT: f32 = 32.;
pub(crate) const CONTROL_RADIUS: f32 = 4.;
pub(crate) const OVERLAY_RADIUS: f32 = 8.;
pub(crate) const BODY_SIZE: f32 = 14.;
pub(crate) const BODY_LINE: f32 = 18.;
pub(crate) const FIELD_CARET: f32 = 19.;

#[derive(Clone, Copy, Debug)]
pub(crate) struct ControlColors {
    pub(crate) panel: Rgba,
    pub(crate) fill: Rgba,
    pub(crate) field_stroke: Rgba,
    pub(crate) field_bottom: Rgba,
    pub(crate) hover: Rgba,
    pub(crate) button_pressed: Rgba,
    pub(crate) button_stroke: Rgba,
    pub(crate) button_bottom: Rgba,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Palette {
    pub(crate) dark: bool,
    pub(crate) window_clear: Rgba,
    pub(crate) layer: Rgba,
    pub(crate) stroke: Rgba,
    pub(crate) stroke_on_layer: Rgba,
    pub(crate) editor: Rgba,
    pub(crate) text_primary: Rgba,
    pub(crate) text_secondary: Rgba,
    pub(crate) text_tertiary: Rgba,
    pub(crate) text_disabled: Rgba,
    pub(crate) subtle_secondary: Rgba,
    pub(crate) subtle_tertiary: Rgba,
    pub(crate) layer_hover: Rgba,
    pub(crate) divider: Rgba,
    pub(crate) on_accent_secondary: Rgba,
    pub(crate) caption_glyph: Rgba,
    pub(crate) caption_glyph_inactive: Rgba,
    pub(crate) caption_glyph_pressed: Rgba,
    pub(crate) close_hover: Rgba,
    pub(crate) close_pressed: Rgba,
    pub(crate) close_glyph_hover: Rgba,
    pub(crate) close_glyph_pressed: Rgba,
    pub(crate) keytip_fill: Rgba,
    pub(crate) keytip_text: Rgba,
    pub(crate) caret: Rgba,
    pub(crate) menu_fill: Rgba,
    pub(crate) menu_separator: Rgba,
    pub(crate) smoke: Rgba,
    pub(crate) surface_stroke: Rgba,
    pub(crate) dialog_divider: Rgba,
    pub(crate) content_surface: ControlColors,
    pub(crate) command_surface: ControlColors,
    pub(crate) card_surface: ControlColors,
    pub(crate) notice_fill: Rgba,
    pub(crate) field_focused: Rgba,
    pub(crate) focus_outer: Rgba,
    pub(crate) focus_inner: Rgba,
    pub(crate) acrylic_fallback: Rgba,
    pub(crate) acrylic_luminosity: Rgba,
    pub(crate) flyout_stroke: Rgba,
    pub(crate) on_accent: Rgba,
    pub(crate) accent: Rgba,
    pub(crate) accent_bottom_edge: f32,
    pub(crate) selection: Rgba,
}

impl Palette {
    pub(crate) fn resolve(
        dark: bool,
        system_accent: Option<u32>,
        high_contrast: Option<HighContrastColors>,
    ) -> Self {
        if let Some(hc) = high_contrast {
            return Self::from_high_contrast(hc);
        }
        let mut palette = if dark { DARK } else { LIGHT };
        if let Some(accent) = system_accent {
            palette.accent = rgb(accent);
        }
        palette
    }

    fn from_high_contrast(hc: HighContrastColors) -> Self {
        let window = rgb(hc.window);
        let window_text = rgb(hc.window_text);
        let gray_text = rgb(hc.gray_text);
        let highlight = rgb(hc.highlight);
        let highlight_text = rgb(hc.highlight_text);
        let btn_face = rgb(hc.btn_face);
        let btn_text = rgb(hc.btn_text);
        let window_frame = rgb(hc.window_frame);
        let hotlight = rgb(hc.hotlight);
        let dark = (0.299 * window.r + 0.587 * window.g + 0.114 * window.b) < 128.0 / 255.0;
        let surface = ControlColors {
            panel: window,
            fill: btn_face,
            field_stroke: window_frame,
            field_bottom: window_text,
            hover: highlight,
            button_pressed: highlight,
            button_stroke: btn_text,
            button_bottom: btn_text,
        };
        Self {
            dark,
            window_clear: window,
            layer: window,
            stroke: window_text,
            stroke_on_layer: window_text,
            editor: window,
            text_primary: window_text,
            text_secondary: window_text,
            text_tertiary: window_text,
            text_disabled: gray_text,
            subtle_secondary: highlight,
            subtle_tertiary: highlight,
            layer_hover: highlight,
            divider: window_text,
            on_accent_secondary: highlight_text,
            caption_glyph: window_text,
            caption_glyph_inactive: gray_text,
            caption_glyph_pressed: highlight_text,
            close_hover: highlight,
            close_pressed: highlight,
            close_glyph_hover: highlight_text,
            close_glyph_pressed: highlight_text,
            keytip_fill: highlight,
            keytip_text: highlight_text,
            caret: window_text,
            menu_fill: window,
            menu_separator: window_text,
            smoke: rgba(0x00000080),
            surface_stroke: window_text,
            dialog_divider: window_text,
            content_surface: surface,
            command_surface: surface,
            card_surface: surface,
            notice_fill: window,
            field_focused: window,
            focus_outer: hotlight,
            focus_inner: window,
            acrylic_fallback: window,
            acrylic_luminosity: window,
            flyout_stroke: window_text,
            on_accent: highlight_text,
            accent: highlight,
            accent_bottom_edge: 1.0,
            selection: highlight.alpha(0.55),
        }
    }
}

pub(crate) const LIGHT: Palette = Palette {
    dark: false,
    window_clear: transparent_black(),
    layer: rgba(0xffffffb3),
    stroke: rgba(0x0000000f),
    stroke_on_layer: rgba(0xeaeaeab8),
    editor: rgb(0xf9f9f9),
    text_primary: rgba(0x000000e4),
    text_secondary: rgba(0x0000009e),
    text_tertiary: rgba(0x00000072),
    text_disabled: rgba(0x0000005c),
    subtle_secondary: rgba(0x00000009),
    subtle_tertiary: rgba(0x00000006),
    layer_hover: rgba(0x0000000a),
    divider: rgba(0x0000000f),
    on_accent_secondary: rgba(0xffffffb3),
    caption_glyph: rgba(0x000000ff),
    caption_glyph_inactive: rgba(0x0000005c),
    caption_glyph_pressed: rgba(0x0000009e),
    close_hover: rgba(0xc42b1cff),
    close_pressed: rgba(0xc42b1ce6),
    close_glyph_hover: rgba(0xffffffff),
    close_glyph_pressed: rgba(0xffffffb3),
    keytip_fill: rgb(0x767676),
    keytip_text: rgb(0xffffff),
    caret: rgb(0x060606),
    menu_fill: rgb(0xfcfcfc),
    menu_separator: rgb(0xededed),
    smoke: rgba(0x0000004d),
    surface_stroke: rgba(0x75757566),
    dialog_divider: rgb(0xe5e5e5),
    content_surface: ControlColors {
        panel: rgb(0xffffff),
        fill: rgb(0xffffff),
        field_stroke: rgb(0xf0f0f0),
        field_bottom: rgb(0x8d8d8d),
        hover: rgb(0xf9f9f9),
        button_pressed: rgb(0xf9f9f9),
        button_stroke: rgb(0xf0f0f0),
        button_bottom: rgb(0xd6d6d6),
    },
    command_surface: ControlColors {
        panel: rgb(0xf3f3f3),
        fill: rgb(0xfbfbfb),
        field_stroke: rgb(0xe5e5e5),
        field_bottom: rgb(0x8d8d8d),
        hover: rgb(0xf6f6f6),
        button_pressed: rgb(0xf5f5f5),
        button_stroke: rgb(0xe5e5e5),
        button_bottom: rgb(0xcccccc),
    },
    card_surface: ControlColors {
        panel: rgb(0xf9f9f9),
        fill: rgb(0xfdfdfd),
        field_stroke: rgb(0xeaeaea),
        field_bottom: rgb(0x8a8a8a),
        hover: rgb(0xf9f9f9),
        button_pressed: rgb(0xf9f9f9),
        button_stroke: rgb(0xeaeaea),
        button_bottom: rgb(0xd1d1d1),
    },
    notice_fill: rgb(0xf9f9f9),
    field_focused: rgba(0xffffffff),
    focus_outer: rgb(0x1a1a1a),
    focus_inner: rgb(0xfbfbfb),
    acrylic_fallback: rgb(0xf9f9f9),
    acrylic_luminosity: rgba(0xfcfcfcd9),
    flyout_stroke: rgba(0x0000000f),
    on_accent: rgb(0xffffff),
    accent: rgb(0x005fb8),
    accent_bottom_edge: 0x66 as f32 / 255.,
    selection: rgb(0x55a0fc).alpha(0.3),
};

pub(crate) const DARK: Palette = Palette {
    dark: true,
    window_clear: transparent_black(),
    layer: rgba(0x3a3a3a73),
    stroke: rgba(0x00000019),
    stroke_on_layer: rgba(0x2f2f2f81),
    editor: rgb(0x272727),
    text_primary: rgba(0xffffffff),
    text_secondary: rgba(0xffffffc5),
    text_tertiary: rgba(0xffffff87),
    text_disabled: rgba(0xffffff5d),
    subtle_secondary: rgba(0xffffff0f),
    subtle_tertiary: rgba(0xffffff0a),
    layer_hover: rgba(0xffffff0f),
    divider: rgba(0xffffff15),
    on_accent_secondary: rgba(0x00000080),
    caption_glyph: rgba(0xffffffff),
    caption_glyph_inactive: rgba(0xffffff5d),
    caption_glyph_pressed: rgba(0xffffffc5),
    close_hover: rgba(0xc42b1cff),
    close_pressed: rgba(0xc42b1ce6),
    close_glyph_hover: rgba(0xffffffff),
    close_glyph_pressed: rgba(0xffffffb3),
    keytip_fill: rgb(0x767676),
    keytip_text: rgb(0xffffff),
    caret: rgb(0xffffff),
    menu_fill: rgb(0x2c2c2c),
    menu_separator: rgb(0x3d3d3d),
    smoke: rgba(0x0000004d),
    surface_stroke: rgba(0x75757566),
    dialog_divider: rgb(0x1d1d1d),
    content_surface: ControlColors {
        panel: rgb(0x2b2b2b),
        fill: rgb(0x373737),
        field_stroke: rgb(0x3a3a3a),
        field_bottom: rgb(0x9e9e9e),
        hover: rgb(0x3c3c3c),
        button_pressed: rgb(0x313131),
        button_stroke: rgb(0x3c3c3c),
        button_bottom: rgb(0x353535),
    },
    command_surface: ControlColors {
        panel: rgb(0x202020),
        fill: rgb(0x2d2d2d),
        field_stroke: rgb(0x353535),
        field_bottom: rgb(0x9a9a9a),
        hover: rgb(0x323232),
        button_pressed: rgb(0x272727),
        button_stroke: rgb(0x353535),
        button_bottom: rgb(0x2f2f2f),
    },
    card_surface: ControlColors {
        panel: rgb(0x282828),
        fill: rgb(0x353535),
        field_stroke: rgb(0x373737),
        field_bottom: rgb(0x9d9d9d),
        hover: rgb(0x3a3a3a),
        button_pressed: rgb(0x2f2f2f),
        button_stroke: rgb(0x3c3c3c),
        button_bottom: rgb(0x373737),
    },
    notice_fill: rgb(0x2d2d2d),
    field_focused: rgba(0x1e1e1eb3),
    focus_outer: rgb(0xffffff),
    focus_inner: rgb(0x0a0a0a),
    acrylic_fallback: rgb(0x2c2c2c),
    acrylic_luminosity: rgba(0x2c2c2cf5),
    flyout_stroke: rgba(0x00000033),
    on_accent: rgb(0x000000),
    accent: rgb(0x60cdff),
    accent_bottom_edge: 0x23 as f32 / 255.,
    selection: rgb(0x1d4ed8).alpha(0.3),
};

const BLUR_RADIUS_PER_SIGMA: f32 = 3.;
const CASTER_INSET: f32 = 1.;

pub(crate) fn theme_shadow(dark: bool, z: f32) -> Shadows {
    let elevation = (z / 2.).min(64.);
    let (ambient, directional) = match (elevation, dark) {
        (e, _) if e < 2. => (0., 0.),
        (e, false) if e <= 16. => (0., (e / 100. + 0.06).min(0.14)),
        (e, true) if e <= 16. => (0., 0.26),
        (_, false) => (0.15, 0.19),
        (_, true) => (0.37, 0.37),
    };
    let make = |opacity: f32, blur: f32, y: f32| -> Option<BoxShadow> {
        (opacity > 0.).then(|| BoxShadow {
            color: black().alpha((opacity * 255.).floor() / 255.),
            offset: point(0., y),
            blur_radius: (blur + CASTER_INSET) / BLUR_RADIUS_PER_SIGMA,
            spread_radius: -CASTER_INSET,
        })
    };
    Shadows([
        make(ambient, elevation / 3., 2.),
        make(directional, elevation, elevation / 2.),
    ])
}
