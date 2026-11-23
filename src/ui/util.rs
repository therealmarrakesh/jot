//! Fluent builder helpers and device-pixel rounding utilities.

/// Fluent conditional helpers implemented for all element builders.
pub(crate) trait FluentBuilder {
    #[inline]
    fn map<U>(self, f: impl FnOnce(Self) -> U) -> U
    where
        Self: Sized,
    {
        f(self)
    }

    #[inline]
    fn when(self, condition: bool, then: impl FnOnce(Self) -> Self) -> Self
    where
        Self: Sized,
    {
        if condition { then(self) } else { self }
    }

    #[inline]
    fn when_some<T>(self, option: Option<T>, then: impl FnOnce(Self, T) -> Self) -> Self
    where
        Self: Sized,
    {
        if let Some(value) = option {
            then(self, value)
        } else {
            self
        }
    }
}

impl<T: super::element::IntoElement> FluentBuilder for T {}

/// Rounds to the nearest integer with exact `.5` ties rounded toward zero.
#[inline]
pub(crate) fn round_half_toward_zero(value: f32) -> f32 {
    (value.abs() - 0.5).ceil().copysign(value)
}

#[inline]
pub(crate) fn round_to_device_pixel(logical: f32, scale_factor: f32) -> f32 {
    round_half_toward_zero(logical * scale_factor)
}

#[inline]
pub(crate) fn round_stroke_to_device_pixel(logical: f32, scale_factor: f32) -> f32 {
    if logical == 0.0 {
        0.0
    } else {
        round_to_device_pixel(logical.max(0.0), scale_factor).max(1.0)
    }
}

#[inline]
pub(crate) fn ceil_to_device_pixel(logical: f32, scale_factor: f32) -> f32 {
    (logical * scale_factor).ceil()
}
