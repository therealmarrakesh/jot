use crate::color::{Rgba, black, rgb};
use crate::geom::{BoxShadow, Shadows, point};
use crate::platform::HighContrastColors;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Surface {
    pub(crate) panel: u32,
    pub(crate) fill: u32,
    pub(crate) field_stroke: u32,
    pub(crate) field_bottom: u32,
    pub(crate) hover: u32,
    pub(crate) button_pressed: u32,
    pub(crate) button_stroke: u32,
    pub(crate) button_bottom: u32,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Palette {
    pub(crate) dark: bool,
    pub(crate) high_contrast: bool,
    pub(crate) layer: u32,
    pub(crate) stroke: u32,
    pub(crate) stroke_on_layer: u32,
    pub(crate) editor: u32,
    pub(crate) text_primary: u32,
    pub(crate) text_secondary: u32,
    pub(crate) text_tertiary: u32,
    pub(crate) text_disabled: u32,
    pub(crate) subtle_secondary: u32,
    pub(crate) subtle_tertiary: u32,
    pub(crate) layer_hover: u32,
    pub(crate) divider: u32,
    pub(crate) on_accent_secondary: u32,
    pub(crate) caption_glyph: u32,
    pub(crate) caption_glyph_inactive: u32,
    pub(crate) caption_glyph_pressed: u32,
    pub(crate) close_hover: u32,
    pub(crate) close_pressed: u32,
    pub(crate) close_glyph_hover: u32,
    pub(crate) close_glyph_pressed: u32,
    pub(crate) keytip_fill: u32,
    pub(crate) keytip_text: u32,
    pub(crate) caret: u32,
    pub(crate) menu_fill: u32,
    pub(crate) menu_separator: u32,
    pub(crate) smoke: u32,
    pub(crate) surface_stroke: u32,
    pub(crate) dialog_divider: u32,
    pub(crate) content_surface: Surface,
    pub(crate) command_surface: Surface,
    pub(crate) card_surface: Surface,
    pub(crate) notice_fill: u32,
    pub(crate) field_focused: u32,
    pub(crate) focus_outer: u32,
    pub(crate) focus_inner: u32,
    pub(crate) acrylic_fallback: u32,
    pub(crate) acrylic_luminosity: u32,
    pub(crate) flyout_stroke: u32,
    pub(crate) on_accent: u32,
    pub(crate) accent: u32,
    pub(crate) accent_bottom_edge: f32,
    pub(crate) selection_rgb: u32,
    pub(crate) selection_alpha: f32,
}

impl Default for Palette {
    fn default() -> Self {
        LIGHT
    }
}

#[inline]
fn opaque(rgb24: u32) -> u32 {
    (rgb24 << 8) | 0xff
}

impl Palette {
    pub(crate) fn for_appearance(dark: bool, system_accent: Option<u32>) -> Self {
        let mut palette = if dark { DARK } else { LIGHT };
        if let Some(accent) = system_accent {
            palette.accent = accent;
        }
        palette
    }

    pub(crate) fn from_high_contrast(hc: HighContrastColors) -> Self {
        let r = ((hc.window >> 16) & 0xFF) as f32;
        let g = ((hc.window >> 8) & 0xFF) as f32;
        let b = (hc.window & 0xFF) as f32;
        let dark = (0.299 * r + 0.587 * g + 0.114 * b) < 128.0;
        let surface = Surface {
            panel: hc.window,
            fill: hc.btn_face,
            field_stroke: hc.window_frame,
            field_bottom: hc.window_text,
            hover: hc.highlight,
            button_pressed: hc.highlight,
            button_stroke: hc.btn_text,
            button_bottom: hc.btn_text,
        };
        Self {
            dark,
            high_contrast: true,
            layer: opaque(hc.window),
            stroke: opaque(hc.window_text),
            stroke_on_layer: opaque(hc.window_text),
            editor: hc.window,
            text_primary: opaque(hc.window_text),
            text_secondary: opaque(hc.window_text),
            text_tertiary: opaque(hc.window_text),
            text_disabled: opaque(hc.gray_text),
            subtle_secondary: opaque(hc.highlight),
            subtle_tertiary: opaque(hc.highlight),
            layer_hover: opaque(hc.highlight),
            divider: opaque(hc.window_text),
            on_accent_secondary: opaque(hc.highlight_text),
            caption_glyph: opaque(hc.window_text),
            caption_glyph_inactive: opaque(hc.gray_text),
            caption_glyph_pressed: opaque(hc.highlight_text),
            close_hover: opaque(hc.highlight),
            close_pressed: opaque(hc.highlight),
            close_glyph_hover: opaque(hc.highlight_text),
            close_glyph_pressed: opaque(hc.highlight_text),
            keytip_fill: hc.highlight,
            keytip_text: hc.highlight_text,
            caret: hc.window_text,
            menu_fill: hc.window,
            menu_separator: hc.window_text,
            smoke: 0x00000080,
            surface_stroke: opaque(hc.window_text),
            dialog_divider: hc.window_text,
            content_surface: surface,
            command_surface: surface,
            card_surface: surface,
            notice_fill: hc.window,
            field_focused: opaque(hc.window),
            focus_outer: hc.hotlight,
            focus_inner: hc.window,
            acrylic_fallback: hc.window,
            acrylic_luminosity: opaque(hc.window),
            flyout_stroke: opaque(hc.window_text),
            on_accent: hc.highlight_text,
            accent: hc.highlight,
            accent_bottom_edge: 1.0,
            selection_rgb: hc.highlight,
            selection_alpha: 0.55,
        }
    }

    pub(crate) fn selection(self) -> Rgba {
        rgb(self.selection_rgb).alpha(self.selection_alpha)
    }
}

pub(crate) const LIGHT: Palette = Palette {
    dark: false,
    high_contrast: false,
    layer: 0xffffffb3,
    stroke: 0x0000000f,
    stroke_on_layer: 0xeaeaeab8,
    editor: 0xf9f9f9,
    text_primary: 0x000000e4,
    text_secondary: 0x0000009e,
    text_tertiary: 0x00000072,
    text_disabled: 0x0000005c,
    subtle_secondary: 0x00000009,
    subtle_tertiary: 0x00000006,
    layer_hover: 0x0000000a,
    divider: 0x0000000f,
    on_accent_secondary: 0xffffffb3,
    caption_glyph: 0x000000ff,
    caption_glyph_inactive: 0x0000005c,
    caption_glyph_pressed: 0x0000009e,
    close_hover: 0xc42b1cff,
    close_pressed: 0xc42b1ce6,
    close_glyph_hover: 0xffffffff,
    close_glyph_pressed: 0xffffffb3,
    keytip_fill: 0x767676,
    keytip_text: 0xffffff,
    caret: 0x060606,
    menu_fill: 0xfcfcfc,
    menu_separator: 0xededed,
    smoke: 0x0000004d,
    surface_stroke: 0x75757566,
    dialog_divider: 0xe5e5e5,
    content_surface: Surface {
        panel: 0xffffff,
        fill: 0xffffff,
        field_stroke: 0xf0f0f0,
        field_bottom: 0x8d8d8d,
        hover: 0xf9f9f9,
        button_pressed: 0xf9f9f9,
        button_stroke: 0xf0f0f0,
        button_bottom: 0xd6d6d6,
    },
    command_surface: Surface {
        panel: 0xf3f3f3,
        fill: 0xfbfbfb,
        field_stroke: 0xe5e5e5,
        field_bottom: 0x8d8d8d,
        hover: 0xf6f6f6,
        button_pressed: 0xf5f5f5,
        button_stroke: 0xe5e5e5,
        button_bottom: 0xcccccc,
    },
    card_surface: Surface {
        panel: 0xf9f9f9,
        fill: 0xfdfdfd,
        field_stroke: 0xeaeaea,
        field_bottom: 0x8a8a8a,
        hover: 0xf9f9f9,
        button_pressed: 0xf9f9f9,
        button_stroke: 0xeaeaea,
        button_bottom: 0xd1d1d1,
    },
    notice_fill: 0xf9f9f9,
    field_focused: 0xffffffff,
    focus_outer: 0x1a1a1a,
    focus_inner: 0xfbfbfb,
    acrylic_fallback: 0xf9f9f9,
    acrylic_luminosity: 0xfcfcfcd9,
    flyout_stroke: 0x0000000f,
    on_accent: 0xffffff,
    accent: 0x005fb8,
    accent_bottom_edge: 0x66 as f32 / 255.,
    selection_rgb: 0x55a0fc,
    selection_alpha: 0.3,
};

pub(crate) const DARK: Palette = Palette {
    dark: true,
    high_contrast: false,
    layer: 0x3a3a3a73,
    stroke: 0x00000019,
    stroke_on_layer: 0x2f2f2f81,
    editor: 0x272727,
    text_primary: 0xffffffff,
    text_secondary: 0xffffffc5,
    text_tertiary: 0xffffff87,
    text_disabled: 0xffffff5d,
    subtle_secondary: 0xffffff0f,
    subtle_tertiary: 0xffffff0a,
    layer_hover: 0xffffff0f,
    divider: 0xffffff15,
    on_accent_secondary: 0x00000080,
    caption_glyph: 0xffffffff,
    caption_glyph_inactive: 0xffffff5d,
    caption_glyph_pressed: 0xffffffc5,
    close_hover: 0xc42b1cff,
    close_pressed: 0xc42b1ce6,
    close_glyph_hover: 0xffffffff,
    close_glyph_pressed: 0xffffffb3,
    keytip_fill: 0x767676,
    keytip_text: 0xffffff,
    caret: 0xffffff,
    menu_fill: 0x2c2c2c,
    menu_separator: 0x3d3d3d,
    smoke: 0x0000004d,
    surface_stroke: 0x75757566,
    dialog_divider: 0x1d1d1d,
    content_surface: Surface {
        panel: 0x2b2b2b,
        fill: 0x373737,
        field_stroke: 0x3a3a3a,
        field_bottom: 0x9e9e9e,
        hover: 0x3c3c3c,
        button_pressed: 0x313131,
        button_stroke: 0x3c3c3c,
        button_bottom: 0x353535,
    },
    command_surface: Surface {
        panel: 0x202020,
        fill: 0x2d2d2d,
        field_stroke: 0x353535,
        field_bottom: 0x9a9a9a,
        hover: 0x323232,
        button_pressed: 0x272727,
        button_stroke: 0x353535,
        button_bottom: 0x2f2f2f,
    },
    card_surface: Surface {
        panel: 0x282828,
        fill: 0x353535,
        field_stroke: 0x373737,
        field_bottom: 0x9d9d9d,
        hover: 0x3a3a3a,
        button_pressed: 0x2f2f2f,
        button_stroke: 0x3c3c3c,
        button_bottom: 0x373737,
    },
    notice_fill: 0x2d2d2d,
    field_focused: 0x1e1e1eb3,
    focus_outer: 0xffffff,
    focus_inner: 0x0a0a0a,
    acrylic_fallback: 0x2c2c2c,
    acrylic_luminosity: 0x2c2c2cf5,
    flyout_stroke: 0x00000033,
    on_accent: 0x000000,
    accent: 0x60cdff,
    accent_bottom_edge: 0x23 as f32 / 255.,
    selection_rgb: 0x1d4ed8,
    selection_alpha: 0.3,
};

const BLUR_RADIUS_PER_SIGMA: f32 = 3.;
const CASTER_INSET: f32 = 1.;

pub(crate) fn theme_shadow(dark: bool, z: f32, scale: f32) -> Shadows {
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
            offset: point(0., y * scale),
            blur_radius: (blur + CASTER_INSET) * scale / BLUR_RADIUS_PER_SIGMA,
            spread_radius: -CASTER_INSET * scale,
        })
    };
    Shadows([
        make(ambient, elevation / 3., 2.),
        make(directional, elevation, elevation / 2.),
    ])
}
