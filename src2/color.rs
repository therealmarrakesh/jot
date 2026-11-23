//! Four-channel `[0.0, 1.0]` RGBA color representation and blending helpers.

/// Four-channel `[0.0, 1.0]` RGBA color.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub(crate) struct Rgba {
    pub(crate) r: f32,
    pub(crate) g: f32,
    pub(crate) b: f32,
    pub(crate) a: f32,
}

impl Rgba {
    #[inline]
    pub(crate) const fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    #[inline]
    pub(crate) const fn alpha(self, a: f32) -> Self {
        Self { a, ..self }
    }

    #[inline]
    pub(crate) fn opacity(self, factor: f32) -> Self {
        Self {
            a: self.a * factor,
            ..self
        }
    }

    #[inline]
    pub(crate) fn is_transparent(self) -> bool {
        self.a <= 0.0
    }

    #[inline]
    pub(crate) fn mix_opaque(self, toward: Self, amount: f32) -> Self {
        Self {
            r: self.r + (toward.r - self.r) * amount,
            g: self.g + (toward.g - self.g) * amount,
            b: self.b + (toward.b - self.b) * amount,
            a: 1.0,
        }
    }
}

/// Construct an opaque `Rgba` from `0xRRGGBB`.
#[inline]
pub(crate) fn rgb(hex: u32) -> Rgba {
    Rgba {
        r: ((hex >> 16) & 0xFF) as f32 / 255.0,
        g: ((hex >> 8) & 0xFF) as f32 / 255.0,
        b: (hex & 0xFF) as f32 / 255.0,
        a: 1.0,
    }
}

/// Construct an `Rgba` from `0xRRGGBBAA`.
#[inline]
pub(crate) fn rgba(hex: u32) -> Rgba {
    Rgba {
        r: ((hex >> 24) & 0xFF) as f32 / 255.0,
        g: ((hex >> 16) & 0xFF) as f32 / 255.0,
        b: ((hex >> 8) & 0xFF) as f32 / 255.0,
        a: (hex & 0xFF) as f32 / 255.0,
    }
}

#[inline]
pub(crate) const fn black() -> Rgba {
    Rgba::new(0.0, 0.0, 0.0, 1.0)
}

#[inline]
pub(crate) const fn white() -> Rgba {
    Rgba::new(1.0, 1.0, 1.0, 1.0)
}

#[inline]
pub(crate) const fn transparent_black() -> Rgba {
    Rgba::new(0.0, 0.0, 0.0, 0.0)
}
