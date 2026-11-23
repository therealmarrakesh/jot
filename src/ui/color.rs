use std::{
    fmt::{self, Display, Formatter},
    hash::{Hash, Hasher},
};

/// Convert an RGB hex color code (`0xRRGGBB`) to an opaque [`Rgba`].
#[inline]
pub(crate) fn rgb(hex: u32) -> Rgba {
    let [_, r, g, b] = hex.to_be_bytes().map(|b| (b as f32) / 255.0);
    Rgba { r, g, b, a: 1.0 }
}

/// Convert an RGBA hex color code (`0xRRGGBBAA`) to [`Rgba`].
#[inline]
pub(crate) fn rgba(hex: u32) -> Rgba {
    let [r, g, b, a] = hex.to_be_bytes().map(|b| (b as f32) / 255.0);
    Rgba { r, g, b, a }
}

/// Pure black (`#000000ff`).
pub(crate) const fn black() -> Rgba {
    Rgba {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    }
}

/// Transparent black (`#00000000`).
pub(crate) const fn transparent_black() -> Rgba {
    Rgba {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 0.0,
    }
}

/// Pure white (`#ffffffff`).
pub(crate) const fn white() -> Rgba {
    Rgba {
        r: 1.0,
        g: 1.0,
        b: 1.0,
        a: 1.0,
    }
}

/// An RGBA color (`r`, `g`, `b`, `a` in `0.0..=1.0`), matching HLSL `float4`.
#[derive(Clone, Copy, Default)]
#[repr(C)]
pub(crate) struct Rgba {
    pub(crate) r: f32,
    pub(crate) g: f32,
    pub(crate) b: f32,
    pub(crate) a: f32,
}

impl Rgba {
    #[inline]
    pub(crate) fn is_transparent(&self) -> bool {
        self.a == 0.0
    }

    #[inline]
    pub(crate) fn opacity(&self, factor: f32) -> Self {
        Self {
            r: self.r,
            g: self.g,
            b: self.b,
            a: self.a * factor.clamp(0.0, 1.0),
        }
    }

    #[inline]
    pub(crate) fn alpha(&self, a: f32) -> Self {
        Self {
            r: self.r,
            g: self.g,
            b: self.b,
            a: a.clamp(0.0, 1.0),
        }
    }
}

impl PartialEq for Rgba {
    fn eq(&self, other: &Self) -> bool {
        self.r
            .total_cmp(&other.r)
            .then(self.g.total_cmp(&other.g))
            .then(self.b.total_cmp(&other.b))
            .then(self.a.total_cmp(&other.a))
            .is_eq()
    }
}

impl Eq for Rgba {}

impl Hash for Rgba {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_u32(self.r.to_bits());
        state.write_u32(self.g.to_bits());
        state.write_u32(self.b.to_bits());
        state.write_u32(self.a.to_bits());
    }
}

impl fmt::Debug for Rgba {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "rgba({:#010x})", u32::from(*self))
    }
}

impl Display for Rgba {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "rgba({:#010x})", u32::from(*self))
    }
}

impl From<Rgba> for u32 {
    fn from(rgba: Rgba) -> Self {
        let r = (rgba.r * 255.0) as u32;
        let g = (rgba.g * 255.0) as u32;
        let b = (rgba.b * 255.0) as u32;
        let a = (rgba.a * 255.0) as u32;
        (r << 24) | (g << 16) | (b << 8) | a
    }
}
