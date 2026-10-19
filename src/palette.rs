use super::*;
use std::sync::{Arc, OnceLock};
use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_BINARY, RegGetValueW};

const DEFAULT_ACCENT: u32 = 0x005fb8;
const DEFAULT_ACCENT_DARK: u32 = 0x60cdff;
const NOISE: &[u8] = include_bytes!("../assets/acrylic-noise.png");
pub(crate) const NOISE_SIZE: f32 = 256.;
pub(crate) const NOISE_OPACITY: f32 = 0.02;

#[derive(Clone, Copy)]
pub(crate) struct Surface {
    pub panel: u32,
    pub fill: u32,
    pub field_stroke: u32,
    pub field_bottom: u32,
    pub hover: u32,
    pub button_pressed: u32,
    pub button_stroke: u32,
    pub button_bottom: u32,
}

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
    pub layer_hover: u32,
    pub divider: u32,
    pub on_accent_secondary: u32,
    pub caption_glyph: u32,
    pub caption_glyph_inactive: u32,
    pub caption_glyph_pressed: u32,
    pub caret: u32,
    pub plus_line: u32,
    pub plus_cap_lead: u32,
    pub plus_cap_trail: u32,
    pub plus_center: u32,
    pub menu_fill: u32,
    pub menu_separator: u32,
    pub smoke: u32,
    pub surface_stroke: u32,
    pub dialog_divider: u32,
    pub content_surface: Surface,
    pub command_surface: Surface,
    pub card_surface: Surface,
    pub notice_fill: u32,
    pub field_focused: u32,
    pub focus_outer: u32,
    pub focus_inner: u32,
    pub scrollbar_thumb: u32,
    pub acrylic_fallback: u32,
    pub acrylic_luminosity: u32,
    pub flyout_stroke: u32,
    pub on_accent: u32,
    pub accent_shade: usize,
    pub accent_bottom_edge: f32,
}

impl Global for Palette {}

const LIGHT: Palette = Palette {
    dark: false,
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
    caret: 0x060606,
    plus_line: 0x000000a4,
    plus_cap_lead: 0x00000029,
    plus_cap_trail: 0x0000004f,
    plus_center: 0x000000d4,
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
    scrollbar_thumb: 0x00000072,
    acrylic_fallback: 0xf9f9f9,
    acrylic_luminosity: 0xfcfcfcd9,
    flyout_stroke: 0x0000000f,
    on_accent: 0xffffff,
    accent_shade: 4,
    accent_bottom_edge: 0x66 as f32 / 255.,
};

const DARK: Palette = Palette {
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
    layer_hover: 0xffffff0f,
    divider: 0xffffff15,
    on_accent_secondary: 0x00000080,
    caption_glyph: 0xffffffff,
    caption_glyph_inactive: 0xffffff5d,
    caption_glyph_pressed: 0xffffffc5,
    caret: 0xffffff,
    plus_line: 0xffffffb7,
    plus_cap_lead: 0xffffff2e,
    plus_cap_trail: 0xffffff58,
    plus_center: 0xffffffed,
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
    scrollbar_thumb: 0xffffff8b,
    acrylic_fallback: 0x2c2c2c,
    acrylic_luminosity: 0x2c2c2cf5,
    flyout_stroke: 0x00000033,
    on_accent: 0x000000,
    accent_shade: 1,
    accent_bottom_edge: 0x23 as f32 / 255.,
};

pub(crate) fn palette(cx: &App) -> Palette {
    cx.try_global::<Palette>().copied().unwrap_or(LIGHT)
}

const BLUR_RADIUS_PER_SIGMA: f32 = 3.;
const CASTER_INSET: f32 = 1.;

pub(crate) fn theme_shadow(dark: bool, z: f32, scale: f32) -> Vec<BoxShadow> {
    let elevation = (z / 2.).min(64.);
    let (ambient, directional) = match (elevation, dark) {
        (e, _) if e < 2. => (0., 0.),
        (e, false) if e <= 16. => (0., (e / 100. + 0.06).min(0.14)),
        (e, true) if e <= 16. => (0., 0.26),
        (_, false) => (0.15, 0.19),
        (_, true) => (0.37, 0.37),
    };
    [
        (ambient, elevation / 3., 2.),
        (directional, elevation, elevation / 2.),
    ]
    .into_iter()
    .filter(|(opacity, ..)| *opacity > 0.)
    .map(|(opacity, blur, y)| BoxShadow {
        color: hsla(0., 0., 0., (opacity * 255.).floor() / 255.),
        offset: point(px(0.), px(y * scale)),
        blur_radius: px((blur + CASTER_INSET) * scale / BLUR_RADIUS_PER_SIGMA),
        spread_radius: px(-CASTER_INSET * scale),
        inset: false,
    })
    .collect()
}

pub(crate) fn apply(appearance: WindowAppearance, cx: &mut App) {
    let mode = ThemeMode::from(appearance);
    let dark = mode.is_dark();
    if cx
        .try_global::<Palette>()
        .is_some_and(|palette| palette.dark == dark)
    {
        return;
    }
    cx.set_global(if dark { DARK } else { LIGHT });
    Theme::change(mode, None, cx);
    let background = transparent_black();
    let caret = hsla(0., 0., 0., f32::EPSILON);
    let placeholder = Hsla::from(rgba(palette(cx).text_secondary));
    let theme = Theme::global_mut(cx);
    theme.background = background;
    theme.tokens.background = background.into();
    theme.caret = caret;
    theme.tokens.caret = caret.into();
    theme.muted_foreground = placeholder;
    theme.tokens.muted_foreground = placeholder.into();
    cx.refresh_windows();
}

pub(crate) fn accent_color(cx: &App) -> Rgba {
    let p = palette(cx);
    let mut shades = [0u8; 32];
    let mut len = shades.len() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            windows_sys::w!(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Accent"),
            windows_sys::w!("AccentPalette"),
            RRF_RT_REG_BINARY,
            std::ptr::null_mut(),
            shades.as_mut_ptr().cast(),
            &mut len,
        )
    };
    let shade = p.accent_shade * 4;
    if status != 0 || (len as usize) < shade + 3 {
        return rgb(if p.dark {
            DEFAULT_ACCENT_DARK
        } else {
            DEFAULT_ACCENT
        });
    }
    rgb(u32::from_be_bytes([
        0,
        shades[shade],
        shades[shade + 1],
        shades[shade + 2],
    ]))
}

pub(crate) fn acrylic_noise() -> Arc<Image> {
    static IMAGE: OnceLock<Arc<Image>> = OnceLock::new();
    IMAGE
        .get_or_init(|| Arc::new(Image::from_bytes(ImageFormat::Png, NOISE.to_vec())))
        .clone()
}
