//! Clean-room 2D geometry, pixel units, and CSS length primitives for Jot.

use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::iter::Sum;
use std::ops::{Add, AddAssign, Div, Mul, Neg, Rem, Sub, SubAssign};

use super::app::App;
use super::platform::DisplayId;

/// Logical DIP pixel measurement (`1.0 == 1/96 inch`).
#[derive(Copy, Clone, Default)]
#[repr(transparent)]
pub(crate) struct Pixels(pub(crate) f32);

#[inline]
pub(crate) const fn px(value: f32) -> Pixels {
    Pixels(value)
}

impl Pixels {
    pub(crate) const ZERO: Self = Self(0.0);

    #[inline]
    pub(crate) const fn as_f32(self) -> f32 {
        self.0
    }

    #[inline]
    pub(crate) fn ceil(self) -> Self {
        Self(self.0.ceil())
    }

    #[inline]
    pub(crate) fn max(self, other: Self) -> Self {
        Self(self.0.max(other.0))
    }

    #[inline]
    pub(crate) fn min(self, other: Self) -> Self {
        Self(self.0.min(other.0))
    }

    #[inline]
    pub(crate) fn clamp(self, min: Self, max: Self) -> Self {
        Self(self.0.clamp(min.0, max.0))
    }
}

impl fmt::Debug for Pixels {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}px", self.0)
    }
}

impl fmt::Display for Pixels {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}px", self.0)
    }
}

impl PartialEq for Pixels {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl Eq for Pixels {}

impl PartialOrd for Pixels {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Pixels {
    #[inline]
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.total_cmp(&other.0)
    }
}

impl Hash for Pixels {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        let bits = if self.0 == 0.0 { 0u32 } else { self.0.to_bits() };
        bits.hash(state);
    }
}

impl Add for Pixels {
    type Output = Self;
    #[inline]
    fn add(self, rhs: Self) -> Self {
        Self(self.0 + rhs.0)
    }
}

impl AddAssign for Pixels {
    #[inline]
    fn add_assign(&mut self, rhs: Self) {
        self.0 += rhs.0;
    }
}

impl Sub for Pixels {
    type Output = Self;
    #[inline]
    fn sub(self, rhs: Self) -> Self {
        Self(self.0 - rhs.0)
    }
}

impl SubAssign for Pixels {
    #[inline]
    fn sub_assign(&mut self, rhs: Self) {
        self.0 -= rhs.0;
    }
}

impl Neg for Pixels {
    type Output = Self;
    #[inline]
    fn neg(self) -> Self {
        Self(-self.0)
    }
}

impl Mul<f32> for Pixels {
    type Output = Self;
    #[inline]
    fn mul(self, rhs: f32) -> Self {
        Self(self.0 * rhs)
    }
}

impl Mul<Pixels> for f32 {
    type Output = Pixels;
    #[inline]
    fn mul(self, rhs: Pixels) -> Pixels {
        Pixels(self * rhs.0)
    }
}

impl Mul<usize> for Pixels {
    type Output = Self;
    #[inline]
    fn mul(self, rhs: usize) -> Self {
        Self(self.0 * rhs as f32)
    }
}

impl Mul<Pixels> for usize {
    type Output = Pixels;
    #[inline]
    fn mul(self, rhs: Pixels) -> Pixels {
        Pixels(self as f32 * rhs.0)
    }
}

impl Div<f32> for Pixels {
    type Output = Self;
    #[inline]
    fn div(self, rhs: f32) -> Self {
        Self(self.0 / rhs)
    }
}

impl Div<usize> for Pixels {
    type Output = Self;
    #[inline]
    fn div(self, rhs: usize) -> Self {
        Self(self.0 / rhs as f32)
    }
}

impl Div<Pixels> for Pixels {
    type Output = f32;
    #[inline]
    fn div(self, rhs: Pixels) -> f32 {
        self.0 / rhs.0
    }
}

impl Rem<Pixels> for Pixels {
    type Output = Self;
    #[inline]
    fn rem(self, rhs: Self) -> Self {
        Self(self.0 % rhs.0)
    }
}

impl Sum for Pixels {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        Self(iter.map(|p| p.0).sum())
    }
}

impl From<f32> for Pixels {
    #[inline]
    fn from(value: f32) -> Self {
        Self(value)
    }
}

impl From<f64> for Pixels {
    #[inline]
    fn from(value: f64) -> Self {
        Self(value as f32)
    }
}

impl From<Pixels> for f32 {
    #[inline]
    fn from(value: Pixels) -> f32 {
        value.0
    }
}

impl From<Pixels> for f64 {
    #[inline]
    fn from(value: Pixels) -> f64 {
        f64::from(value.0)
    }
}

pub(crate) trait Half {
    fn half(self) -> Self;
}

impl Half for Pixels {
    #[inline]
    fn half(self) -> Self {
        Self(self.0 * 0.5)
    }
}

impl Half for f32 {
    #[inline]
    fn half(self) -> Self {
        self * 0.5
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Axis {
    Horizontal,
    Vertical,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct Point<T> {
    pub(crate) x: T,
    pub(crate) y: T,
}

#[inline]
pub(crate) const fn point<T>(x: T, y: T) -> Point<T> {
    Point { x, y }
}

impl<T> Point<T> {
    #[inline]
    pub(crate) const fn new(x: T, y: T) -> Self {
        Self { x, y }
    }

    #[inline]
    pub(crate) fn map<U>(self, mut f: impl FnMut(T) -> U) -> Point<U> {
        Point {
            x: f(self.x),
            y: f(self.y),
        }
    }
}

impl<T: Add<Output = T>> Add for Point<T> {
    type Output = Self;
    #[inline]
    fn add(self, rhs: Self) -> Self {
        Self {
            x: self.x + rhs.x,
            y: self.y + rhs.y,
        }
    }
}

impl<T: AddAssign> AddAssign for Point<T> {
    #[inline]
    fn add_assign(&mut self, rhs: Self) {
        self.x += rhs.x;
        self.y += rhs.y;
    }
}

impl<T: Sub<Output = T>> Sub for Point<T> {
    type Output = Self;
    #[inline]
    fn sub(self, rhs: Self) -> Self {
        Self {
            x: self.x - rhs.x,
            y: self.y - rhs.y,
        }
    }
}

impl<T: SubAssign> SubAssign for Point<T> {
    #[inline]
    fn sub_assign(&mut self, rhs: Self) {
        self.x -= rhs.x;
        self.y -= rhs.y;
    }
}

impl<T: Neg<Output = T>> Neg for Point<T> {
    type Output = Self;
    #[inline]
    fn neg(self) -> Self {
        Self {
            x: -self.x,
            y: -self.y,
        }
    }
}

impl<T: Mul<f32, Output = T>> Mul<f32> for Point<T> {
    type Output = Self;
    #[inline]
    fn mul(self, rhs: f32) -> Self {
        Self {
            x: self.x * rhs,
            y: self.y * rhs,
        }
    }
}

impl<T: Div<f32, Output = T>> Div<f32> for Point<T> {
    type Output = Self;
    #[inline]
    fn div(self, rhs: f32) -> Self {
        Self {
            x: self.x / rhs,
            y: self.y / rhs,
        }
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct Size<T> {
    pub(crate) width: T,
    pub(crate) height: T,
}

#[inline]
pub(crate) const fn size<T>(width: T, height: T) -> Size<T> {
    Size { width, height }
}

impl<T> Size<T> {
    #[inline]
    pub(crate) fn map<U>(self, mut f: impl FnMut(T) -> U) -> Size<U> {
        Size {
            width: f(self.width),
            height: f(self.height),
        }
    }
}

impl Size<Pixels> {
    #[inline]
    pub(crate) fn is_empty(self) -> bool {
        self.width <= Pixels::ZERO || self.height <= Pixels::ZERO
    }
}

impl Size<Length> {
    #[inline]
    pub(crate) const fn auto() -> Self {
        Self {
            width: Length::Auto,
            height: Length::Auto,
        }
    }
}

impl Size<DefiniteLength> {
    #[inline]
    pub(crate) const fn zero() -> Self {
        Self {
            width: DefiniteLength::Absolute(Pixels::ZERO),
            height: DefiniteLength::Absolute(Pixels::ZERO),
        }
    }
}

impl<T: Add<Output = T>> Add for Size<T> {
    type Output = Self;
    #[inline]
    fn add(self, rhs: Self) -> Self {
        Self {
            width: self.width + rhs.width,
            height: self.height + rhs.height,
        }
    }
}

impl<T: Sub<Output = T>> Sub for Size<T> {
    type Output = Self;
    #[inline]
    fn sub(self, rhs: Self) -> Self {
        Self {
            width: self.width - rhs.width,
            height: self.height - rhs.height,
        }
    }
}

impl<T: Mul<f32, Output = T>> Mul<f32> for Size<T> {
    type Output = Self;
    #[inline]
    fn mul(self, rhs: f32) -> Self {
        Self {
            width: self.width * rhs,
            height: self.height * rhs,
        }
    }
}

impl From<Size<Pixels>> for Size<Option<Pixels>> {
    #[inline]
    fn from(s: Size<Pixels>) -> Self {
        Self {
            width: Some(s.width),
            height: Some(s.height),
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Anchor {
    TopRight,
    BottomLeft,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct Bounds<T> {
    pub(crate) origin: Point<T>,
    pub(crate) size: Size<T>,
}

impl<T> Bounds<T> {
    #[inline]
    pub(crate) const fn new(origin: Point<T>, size: Size<T>) -> Self {
        Self { origin, size }
    }

    #[inline]
    pub(crate) fn from_corners(top_left: Point<T>, bottom_right: Point<T>) -> Self
    where
        T: Copy + Sub<Output = T>,
    {
        Self {
            origin: top_left,
            size: Size {
                width: bottom_right.x - top_left.x,
                height: bottom_right.y - top_left.y,
            },
        }
    }

    #[inline]
    pub(crate) fn map<U>(self, mut f: impl FnMut(T) -> U) -> Bounds<U> {
        Bounds {
            origin: self.origin.map(&mut f),
            size: self.size.map(&mut f),
        }
    }
}

impl Div<f32> for Bounds<f32> {
    type Output = Self;

    #[inline]
    fn div(self, rhs: f32) -> Self {
        Self {
            origin: Point::new(self.origin.x / rhs, self.origin.y / rhs),
            size: Size {
                width: self.size.width / rhs,
                height: self.size.height / rhs,
            },
        }
    }
}

impl Bounds<Pixels> {
    pub(crate) fn centered(
        display_id: Option<DisplayId>,
        window_size: Size<Pixels>,
        cx: &App,
    ) -> Self {
        let display_bounds = display_id
            .and_then(|id| cx.find_display(id))
            .or_else(|| cx.primary_display())
            .map_or(
                Bounds::new(point(px(0.0), px(0.0)), size(px(1280.0), px(800.0))),
                |d| d.visible_bounds(),
            );
        let origin = point(
            display_bounds.origin.x
                + ((display_bounds.size.width - window_size.width) * 0.5).max(Pixels::ZERO),
            display_bounds.origin.y
                + ((display_bounds.size.height - window_size.height) * 0.5).max(Pixels::ZERO),
        );
        Self::new(origin, window_size)
    }

    #[inline]
    pub(crate) fn union(&self, other: &Self) -> Self {
        let left = self.left().min(other.left());
        let top = self.top().min(other.top());
        let right = self.right().max(other.right());
        let bottom = self.bottom().max(other.bottom());
        Self::from_corners(point(left, top), point(right, bottom))
    }

    #[inline]
    pub(crate) fn from_anchor_and_size(
        anchor: Anchor,
        pos: Point<Pixels>,
        rect_size: Size<Pixels>,
    ) -> Self {
        let origin = match anchor {
            Anchor::TopRight => point(pos.x - rect_size.width, pos.y),
            Anchor::BottomLeft => point(pos.x, pos.y - rect_size.height),
        };
        Self::new(origin, rect_size)
    }

    #[inline]
    pub(crate) fn left(&self) -> Pixels {
        self.origin.x
    }

    #[inline]
    pub(crate) fn right(&self) -> Pixels {
        self.origin.x + self.size.width
    }

    #[inline]
    pub(crate) fn top(&self) -> Pixels {
        self.origin.y
    }

    #[inline]
    pub(crate) fn bottom(&self) -> Pixels {
        self.origin.y + self.size.height
    }

    #[inline]
    pub(crate) fn top_right(&self) -> Point<Pixels> {
        point(self.right(), self.top())
    }

    #[inline]
    pub(crate) fn bottom_left(&self) -> Point<Pixels> {
        point(self.left(), self.bottom())
    }

    #[inline]
    pub(crate) fn center(&self) -> Point<Pixels> {
        point(
            self.origin.x + self.size.width.half(),
            self.origin.y + self.size.height.half(),
        )
    }

    #[inline]
    pub(crate) fn contains(&self, p: &Point<Pixels>) -> bool {
        p.x >= self.left() && p.x < self.right() && p.y >= self.top() && p.y < self.bottom()
    }

    #[inline]
    pub(crate) fn intersect(&self, other: &Self) -> Self {
        let left = self.left().max(other.left());
        let top = self.top().max(other.top());
        let right = self.right().min(other.right()).max(left);
        let bottom = self.bottom().min(other.bottom()).max(top);
        Self::from_corners(point(left, top), point(right, bottom))
    }

    #[inline]
    pub(crate) fn is_empty(&self) -> bool {
        self.size.is_empty()
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct Edges<T> {
    pub(crate) top: T,
    pub(crate) right: T,
    pub(crate) bottom: T,
    pub(crate) left: T,
}

impl<T: Clone> Edges<T> {
    #[inline]
    pub(crate) fn all(value: T) -> Self {
        Self {
            top: value.clone(),
            right: value.clone(),
            bottom: value.clone(),
            left: value,
        }
    }
}

impl Edges<Length> {
    #[inline]
    pub(crate) const fn auto() -> Self {
        Self {
            top: Length::Auto,
            right: Length::Auto,
            bottom: Length::Auto,
            left: Length::Auto,
        }
    }

    #[inline]
    pub(crate) const fn zero() -> Self {
        Self {
            top: Length::Definite(DefiniteLength::Absolute(Pixels::ZERO)),
            right: Length::Definite(DefiniteLength::Absolute(Pixels::ZERO)),
            bottom: Length::Definite(DefiniteLength::Absolute(Pixels::ZERO)),
            left: Length::Definite(DefiniteLength::Absolute(Pixels::ZERO)),
        }
    }
}

impl Edges<DefiniteLength> {
    #[inline]
    pub(crate) const fn zero() -> Self {
        Self {
            top: DefiniteLength::Absolute(Pixels::ZERO),
            right: DefiniteLength::Absolute(Pixels::ZERO),
            bottom: DefiniteLength::Absolute(Pixels::ZERO),
            left: DefiniteLength::Absolute(Pixels::ZERO),
        }
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct Corners<T> {
    pub(crate) top_left: T,
    pub(crate) top_right: T,
    pub(crate) bottom_right: T,
    pub(crate) bottom_left: T,
}

impl<T: Clone> Corners<T> {
    #[inline]
    pub(crate) fn all(value: T) -> Self {
        Self {
            top_left: value.clone(),
            top_right: value.clone(),
            bottom_right: value.clone(),
            bottom_left: value,
        }
    }
}

impl From<Pixels> for Corners<Pixels> {
    #[inline]
    fn from(radius: Pixels) -> Self {
        Self::all(radius)
    }
}

impl Corners<Pixels> {
    pub(crate) fn clamp_radii_for_quad_size(self, quad_size: Size<Pixels>) -> Self {
        let max = quad_size.width.min(quad_size.height).half().max(Pixels::ZERO);
        Self {
            top_left: self.top_left.clamp(Pixels::ZERO, max),
            top_right: self.top_right.clamp(Pixels::ZERO, max),
            bottom_right: self.bottom_right.clamp(Pixels::ZERO, max),
            bottom_left: self.bottom_left.clamp(Pixels::ZERO, max),
        }
    }
}

/// Definite CSS length: absolute logical pixels or fraction (`0.0..=1.0`) of parent.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) enum DefiniteLength {
    Absolute(Pixels),
    Fraction(f32),
}

impl Default for DefiniteLength {
    fn default() -> Self {
        Self::Absolute(Pixels::ZERO)
    }
}

impl From<Pixels> for DefiniteLength {
    #[inline]
    fn from(value: Pixels) -> Self {
        Self::Absolute(value)
    }
}

/// CSS length: definite or `auto`.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub(crate) enum Length {
    Definite(DefiniteLength),
    #[default]
    Auto,
}

impl From<Pixels> for Length {
    #[inline]
    fn from(value: Pixels) -> Self {
        Self::Definite(DefiniteLength::Absolute(value))
    }
}

impl From<DefiniteLength> for Length {
    #[inline]
    fn from(value: DefiniteLength) -> Self {
        Self::Definite(value)
    }
}

#[inline]
pub(crate) const fn relative(fraction: f32) -> DefiniteLength {
    DefiniteLength::Fraction(fraction)
}
