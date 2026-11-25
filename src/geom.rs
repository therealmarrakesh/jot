//! Geometry and colour: logical points, sizes and rects (in DIPs), per-edge and per-corner
//! values, shadows, physical screen coordinates, and colours.

use std::ops::{Add, Sub, SubAssign};

/// `value` rounded, halves up (where `round` would take them away from zero).
pub(crate) fn round_half_up(value: f32) -> f32 {
    (value + 0.5).floor()
}

// ---- Points, sizes and rects ----------------------------------------------------------------

#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub(crate) struct Point {
    pub(crate) x: f32,
    pub(crate) y: f32,
}

pub(crate) const fn point(x: f32, y: f32) -> Point {
    Point { x, y }
}

impl Point {
    pub(crate) const ZERO: Self = point(0.0, 0.0);
}

impl Add for Point {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        point(self.x + rhs.x, self.y + rhs.y)
    }
}

impl Sub for Point {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        point(self.x - rhs.x, self.y - rhs.y)
    }
}

impl SubAssign for Point {
    fn sub_assign(&mut self, rhs: Self) {
        *self = *self - rhs;
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub(crate) struct Size {
    pub(crate) width: f32,
    pub(crate) height: f32,
}

pub(crate) const fn size(width: f32, height: f32) -> Size {
    Size { width, height }
}

impl Size {
    pub(crate) const ZERO: Self = size(0.0, 0.0);

    pub(crate) fn is_empty(self) -> bool {
        self.width <= 0.0 || self.height <= 0.0
    }

    /// Extent along `axis` (0 = width, 1 = height).
    pub(crate) fn along(self, axis: usize) -> f32 {
        if axis == 0 { self.width } else { self.height }
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub(crate) struct Rect {
    pub(crate) origin: Point,
    pub(crate) size: Size,
}

#[cfg(test)]
pub(crate) const fn rect(x: f32, y: f32, width: f32, height: f32) -> Rect {
    Rect::new(point(x, y), size(width, height))
}

impl Rect {
    pub(crate) const ZERO: Self = Self::new(Point::ZERO, Size::ZERO);

    pub(crate) const fn new(origin: Point, size: Size) -> Self {
        Self { origin, size }
    }

    /// The rect from `origin` to `lower_right`, empty if that's above or left of it.
    pub(crate) fn from_corners(origin: Point, lower_right: Point) -> Self {
        let (w, h) = (lower_right.x - origin.x, lower_right.y - origin.y);
        Self::new(origin, size(w.max(0.0), h.max(0.0)))
    }

    pub(crate) const fn left(self) -> f32 {
        self.origin.x
    }

    pub(crate) const fn top(self) -> f32 {
        self.origin.y
    }

    pub(crate) fn right(self) -> f32 {
        self.origin.x + self.size.width
    }

    pub(crate) fn bottom(self) -> f32 {
        self.origin.y + self.size.height
    }

    pub(crate) fn center(self) -> Point {
        let o = self.origin;
        point(o.x + self.size.width * 0.5, o.y + self.size.height * 0.5)
    }

    pub(crate) fn is_empty(self) -> bool {
        self.size.is_empty()
    }

    /// Whether `pt` is inside, the right and bottom edges excluded.
    pub(crate) fn contains(self, pt: Point) -> bool {
        pt.x >= self.left() && pt.x < self.right() && pt.y >= self.top() && pt.y < self.bottom()
    }

    pub(crate) fn intersect(self, other: Self) -> Self {
        let (left, top) = (self.left().max(other.left()), self.top().max(other.top()));
        let right = self.right().min(other.right()).max(left);
        let bottom = self.bottom().min(other.bottom()).max(top);
        Self::from_corners(point(left, top), point(right, bottom))
    }

    /// The smallest rect holding both (an empty one adds nothing).
    pub(crate) fn union(self, other: Self) -> Self {
        if self.is_empty() {
            return other;
        }
        if other.is_empty() {
            return self;
        }
        let (left, top) = (self.left().min(other.left()), self.top().min(other.top()));
        let (right, bottom) = (
            self.right().max(other.right()),
            self.bottom().max(other.bottom()),
        );
        Self::from_corners(point(left, top), point(right, bottom))
    }

    pub(crate) fn inset(self, edges: Edges) -> Self {
        let origin = point(self.origin.x + edges.left, self.origin.y + edges.top);
        let (w, h) = (
            self.size.width - edges.along(0),
            self.size.height - edges.along(1),
        );
        Self::new(origin, size(w.max(0.0), h.max(0.0)))
    }

    pub(crate) fn inflate(self, amount: f32) -> Self {
        let origin = point(self.origin.x - amount, self.origin.y - amount);
        let (w, h) = (
            self.size.width + amount * 2.0,
            self.size.height + amount * 2.0,
        );
        Self::new(origin, size(w.max(0.0), h.max(0.0)))
    }

    pub(crate) fn translate(self, delta: Point) -> Self {
        Self::new(self.origin + delta, self.size)
    }

    /// The rect on screen, at `scale` from a client area at `origin`.
    pub(crate) fn to_screen(self, scale: f32, origin: ScreenPoint) -> ScreenRect {
        let phys = |v: f32| (v * scale).round() as i32;
        ScreenRect {
            left: origin.x + phys(self.origin.x),
            top: origin.y + phys(self.origin.y),
            right: origin.x + phys(self.right()),
            bottom: origin.y + phys(self.bottom()),
        }
    }
}

// ---- Edges and corners ----------------------------------------------------------------------

#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub(crate) struct Edges<T = f32> {
    pub(crate) top: T,
    pub(crate) right: T,
    pub(crate) bottom: T,
    pub(crate) left: T,
}

impl<T: Copy> Edges<T> {
    pub(crate) const fn xy(horiz: T, vert: T) -> Self {
        Self {
            top: vert,
            right: horiz,
            bottom: vert,
            left: horiz,
        }
    }

    pub(crate) const fn all(value: T) -> Self {
        Self::xy(value, value)
    }

    /// Leading edge along `axis` (0 = horizontal, 1 = vertical).
    pub(crate) fn start(self, axis: usize) -> T {
        if axis == 0 { self.left } else { self.top }
    }

    /// Trailing edge along `axis`.
    pub(crate) fn end(self, axis: usize) -> T {
        if axis == 0 { self.right } else { self.bottom }
    }

    pub(crate) fn map<U>(self, f: impl Fn(T) -> U) -> Edges<U> {
        let (top, right, bottom, left) = (f(self.top), f(self.right), f(self.bottom), f(self.left));
        Edges {
            top,
            right,
            bottom,
            left,
        }
    }
}

impl Edges<f32> {
    pub(crate) const ZERO: Self = Self::all(0.0);

    /// Both edges along `axis`.
    pub(crate) fn along(self, axis: usize) -> f32 {
        if axis == 0 {
            self.left + self.right
        } else {
            self.top + self.bottom
        }
    }

    pub(crate) fn any_nonzero(self) -> bool {
        self.top != 0.0 || self.right != 0.0 || self.bottom != 0.0 || self.left != 0.0
    }

    pub(crate) fn add(self, other: Self) -> Self {
        let (top, right) = (self.top + other.top, self.right + other.right);
        let (bottom, left) = (self.bottom + other.bottom, self.left + other.left);
        Self {
            top,
            right,
            bottom,
            left,
        }
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub(crate) struct Corners {
    pub(crate) top_left: f32,
    pub(crate) top_right: f32,
    pub(crate) bottom_right: f32,
    pub(crate) bottom_left: f32,
}

impl Corners {
    pub(crate) const ZERO: Self = Self::all(0.0);

    const fn new(top_left: f32, top_right: f32, bottom_right: f32, bottom_left: f32) -> Self {
        Self {
            top_left,
            top_right,
            bottom_right,
            bottom_left,
        }
    }

    pub(crate) const fn all(r: f32) -> Self {
        Self::new(r, r, r, r)
    }

    pub(crate) const fn top(r: f32) -> Self {
        Self::new(r, r, 0.0, 0.0)
    }

    pub(crate) const fn bottom(r: f32) -> Self {
        Self::new(0.0, 0.0, r, r)
    }

    /// The radii (none negative) scaled down together until each side holds its two.
    pub(crate) fn clamp_to(self, size: Size) -> Self {
        let tl = self.top_left.max(0.0);
        let tr = self.top_right.max(0.0);
        let br = self.bottom_right.max(0.0);
        let bl = self.bottom_left.max(0.0);
        let (w, h) = (size.width.max(0.0), size.height.max(0.0));
        let mut factor = 1.0f32;
        for (sum, span) in [(tl + tr, w), (bl + br, w), (tl + bl, h), (tr + br, h)] {
            if sum > 0.0 {
                factor = factor.min(span / sum);
            }
        }
        Self::new(tl * factor, tr * factor, br * factor, bl * factor)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Axis {
    Vertical,
    Horizontal,
}

#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub(crate) struct BoxShadow {
    pub(crate) color: Rgba,
    pub(crate) offset: Point,
    pub(crate) blur_radius: f32,
    pub(crate) spread_radius: f32,
}

/// Up to two shadows under a box.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub(crate) struct Shadows(pub(crate) [Option<BoxShadow>; 2]);

impl Shadows {
    pub(crate) const NONE: Self = Self([None, None]);

    pub(crate) fn is_empty(self) -> bool {
        self.0[0].is_none() && self.0[1].is_none()
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &BoxShadow> {
        self.0.iter().flatten()
    }
}

// ---- The screen -----------------------------------------------------------------------------

/// Physical pixels on the screen.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ScreenPoint {
    pub(crate) x: i32,
    pub(crate) y: i32,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ScreenRect {
    pub(crate) left: i32,
    pub(crate) top: i32,
    pub(crate) right: i32,
    pub(crate) bottom: i32,
}

// ---- Colours --------------------------------------------------------------------------------

/// A colour, straight (not premultiplied) alpha, channels from 0 to 1.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub(crate) struct Rgba {
    pub(crate) r: f32,
    pub(crate) g: f32,
    pub(crate) b: f32,
    pub(crate) a: f32,
}

impl Rgba {
    pub(crate) const fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    pub(crate) const fn alpha(self, a: f32) -> Self {
        Self::new(self.r, self.g, self.b, a)
    }

    pub(crate) fn opacity(self, factor: f32) -> Self {
        self.alpha(self.a * factor)
    }

    pub(crate) fn is_transparent(self) -> bool {
        self.a <= 0.0
    }

    /// `amount` of the way to `toward`, opaque.
    pub(crate) fn mix_opaque(self, toward: Self, amount: f32) -> Self {
        let mix = |from: f32, to: f32| from + (to - from) * amount;
        Self::new(
            mix(self.r, toward.r),
            mix(self.g, toward.g),
            mix(self.b, toward.b),
            1.0,
        )
    }
}

/// The byte `shift` bits up in `hex`, as a channel.
const fn channel(hex: u32, shift: u32) -> f32 {
    ((hex >> shift) & 0xFF) as f32 / 255.0
}

/// An opaque colour from `0xRRGGBB`.
pub(crate) const fn rgb(hex: u32) -> Rgba {
    Rgba::new(channel(hex, 16), channel(hex, 8), channel(hex, 0), 1.0)
}

/// A colour from `0xRRGGBBAA`.
pub(crate) const fn rgba(hex: u32) -> Rgba {
    Rgba::new(
        channel(hex, 24),
        channel(hex, 16),
        channel(hex, 8),
        channel(hex, 0),
    )
}

pub(crate) const fn black() -> Rgba {
    Rgba::new(0.0, 0.0, 0.0, 1.0)
}

pub(crate) const fn white() -> Rgba {
    Rgba::new(1.0, 1.0, 1.0, 1.0)
}

pub(crate) const fn transparent_black() -> Rgba {
    Rgba::new(0.0, 0.0, 0.0, 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_operations_handle_contains_intersect_and_inset() {
        let a = rect(10.0, 20.0, 100.0, 50.0);
        assert!(a.contains(point(10.0, 20.0)));
        assert!(!a.contains(point(110.0, 20.0)));
        assert_eq!(a.center(), point(60.0, 45.0));

        let b = rect(50.0, 40.0, 100.0, 50.0);
        assert_eq!(a.intersect(b), rect(50.0, 40.0, 60.0, 30.0));

        let disjoint = rect(200.0, 200.0, 10.0, 10.0);
        assert!(a.intersect(disjoint).is_empty());

        let inset = a.inset(Edges {
            top: 5.0,
            right: 10.0,
            bottom: 5.0,
            left: 10.0,
        });
        assert_eq!(inset, rect(20.0, 25.0, 80.0, 40.0));
    }
}
