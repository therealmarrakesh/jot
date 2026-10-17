use super::*;
use field::Surface;

#[derive(Clone, Copy)]
pub(crate) struct Palette {
    pub dark: bool,
    pub layer: u32,
    pub stroke: u32,
    pub stroke_on_layer: u32,
    pub editor: u32,
    pub text_primary: u32,
    pub text_secondary: u32,
    pub text_tertiary: u32,
    pub text_disabled: u32,
    pub subtle_secondary: u32,
    pub subtle_tertiary: u32,
    pub menu_open: u32,
    pub caption_glyph: u32,
    pub caption_glyph_inactive: u32,
    pub caption_glyph_pressed: u32,
    pub caret: u32,
    pub plus_line: u32,
    pub plus_cap_lead: u32,
    pub plus_cap_trail: u32,
    pub plus_center: u32,
    pub menu_fill: u32,
    pub menu_border: u32,
    pub menu_separator: u32,
    pub shadow: f32,
    pub smoke: u32,
    pub surface_stroke: u32,
    pub dialog_content: u32,
    pub dialog_command: u32,
    pub dialog_divider: u32,
    pub content_surface: Surface,
    pub command_surface: Surface,
    pub card_surface: Surface,
    pub card_fill: u32,
    pub notice_fill: u32,
    pub notice_border: u32,
    pub field_focused: u32,
    pub focus_outer: u32,
    pub focus_inner: u32,
    pub tooltip_fallback: u32,
    pub tooltip_luminosity: u32,
    pub tooltip_border: u32,
    pub on_accent: u32,
    pub accent_shade: usize,
    pub accent_bottom_edge: f32,
}

impl Global for Palette {}

pub(crate) const LIGHT: Palette = Palette {
    dark: false,
    layer: 0xffffffb3,
    stroke: 0x0000000f,
    stroke_on_layer: 0xeaeaeab8,
    editor: 0xf9f9f9,
    text_primary: 0x000000e4,
    text_secondary: 0x0000009e,
    text_tertiary: 0x00000072,
    text_disabled: 0x0000005c,
    subtle_secondary: 0x0000000a,
    subtle_tertiary: 0x00000006,
    menu_open: 0x0000000a,
    caption_glyph: 0x000000ff,
    caption_glyph_inactive: 0x0000005c,
    caption_glyph_pressed: 0x0000009e,
    caret: 0x060606,
    plus_line: 0x000000a4,
    plus_cap_lead: 0x00000029,
    plus_cap_trail: 0x0000004f,
    plus_center: 0x000000d4,
    menu_fill: 0xfcfcfc,
    menu_border: 0xdcdcdc,
    menu_separator: 0xededed,
    shadow: 0.14,
    smoke: 0x0000004d,
    surface_stroke: 0x75757566,
    dialog_content: 0xffffff,
    dialog_command: 0xf3f3f3,
    dialog_divider: 0xe5e5e5,
    content_surface: Surface {
        field_fill: 0xffffff,
        field_stroke: 0xf0f0f0,
        field_bottom: 0x8d8d8d,
        button_fill: 0xffffff,
        button_hover: 0xf9f9f9,
        button_pressed: 0xf9f9f9,
        button_stroke: 0xf0f0f0,
        button_bottom: 0xd6d6d6,
    },
    command_surface: Surface {
        field_fill: 0xfbfbfb,
        field_stroke: 0xe5e5e5,
        field_bottom: 0x8d8d8d,
        button_fill: 0xfbfbfb,
        button_hover: 0xf6f6f6,
        button_pressed: 0xf5f5f5,
        button_stroke: 0xe5e5e5,
        button_bottom: 0xcccccc,
    },
    card_surface: Surface {
        field_fill: 0xfdfdfd,
        field_stroke: 0xeaeaea,
        field_bottom: 0x8a8a8a,
        button_fill: 0xfdfdfd,
        button_hover: 0xf9f9f9,
        button_pressed: 0xf9f9f9,
        button_stroke: 0xeaeaea,
        button_bottom: 0xd1d1d1,
    },
    card_fill: 0xf9f9f9,
    notice_fill: 0xf9f9f9,
    notice_border: 0xdcdcdc,
    field_focused: 0xffffffff,
    focus_outer: 0x1a1a1a,
    focus_inner: 0xfbfbfb,
    tooltip_fallback: 0xf9f9f9,
    tooltip_luminosity: 0xfcfcfcd9,
    tooltip_border: 0x0000000f,
    on_accent: 0xffffff,
    accent_shade: 4,
    accent_bottom_edge: 0x66 as f32 / 255.,
};

pub(crate) const DARK: Palette = Palette {
    dark: true,
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
    menu_open: 0xffffff0a,
    caption_glyph: 0xffffffff,
    caption_glyph_inactive: 0xffffff5d,
    caption_glyph_pressed: 0xffffffc5,
    caret: 0xffffff,
    plus_line: 0xffffffb7,
    plus_cap_lead: 0xffffff2e,
    plus_cap_trail: 0xffffff58,
    plus_center: 0xffffffed,
    menu_fill: 0x2c2c2c,
    menu_border: 0x1c1c1c,
    menu_separator: 0x3d3d3d,
    shadow: 0.26,
    smoke: 0x0000004d,
    surface_stroke: 0x75757566,
    dialog_content: 0x2b2b2b,
    dialog_command: 0x202020,
    dialog_divider: 0x1d1d1d,
    content_surface: Surface {
        field_fill: 0x373737,
        field_stroke: 0x3a3a3a,
        field_bottom: 0x9e9e9e,
        button_fill: 0x373737,
        button_hover: 0x3c3c3c,
        button_pressed: 0x313131,
        button_stroke: 0x3c3c3c,
        button_bottom: 0x353535,
    },
    command_surface: Surface {
        field_fill: 0x2d2d2d,
        field_stroke: 0x353535,
        field_bottom: 0x9a9a9a,
        button_fill: 0x2d2d2d,
        button_hover: 0x323232,
        button_pressed: 0x272727,
        button_stroke: 0x353535,
        button_bottom: 0x2f2f2f,
    },
    card_surface: Surface {
        field_fill: 0x353535,
        field_stroke: 0x373737,
        field_bottom: 0x9d9d9d,
        button_fill: 0x353535,
        button_hover: 0x3a3a3a,
        button_pressed: 0x2f2f2f,
        button_stroke: 0x3c3c3c,
        button_bottom: 0x373737,
    },
    card_fill: 0x282828,
    notice_fill: 0x2d2d2d,
    notice_border: 0x1c1c1c,
    field_focused: 0x1e1e1eb3,
    focus_outer: 0xffffff,
    focus_inner: 0x0a0a0a,
    tooltip_fallback: 0x2c2c2c,
    tooltip_luminosity: 0x2c2c2cf5,
    tooltip_border: 0x00000033,
    on_accent: 0x000000,
    accent_shade: 1,
    accent_bottom_edge: 0x23 as f32 / 255.,
};

pub(crate) fn palette(cx: &App) -> Palette {
    cx.try_global::<Palette>().copied().unwrap_or(LIGHT)
}

pub(crate) fn apply(appearance: WindowAppearance, cx: &mut App) {
    let dark = matches!(
        appearance,
        WindowAppearance::Dark | WindowAppearance::VibrantDark
    );
    if cx
        .try_global::<Palette>()
        .is_some_and(|palette| palette.dark == dark)
    {
        return;
    }
    cx.set_global(if dark { DARK } else { LIGHT });
    Theme::change(
        if dark {
            ThemeMode::Dark
        } else {
            ThemeMode::Light
        },
        None,
        cx,
    );
    let theme = Theme::global_mut(cx);
    theme.background = transparent_black();
    theme.tokens.background = transparent_black().into();
    theme.caret = hsla(0., 0., 0., f32::EPSILON);
    theme.tokens.caret = hsla(0., 0., 0., f32::EPSILON).into();
    cx.refresh_windows();
}
