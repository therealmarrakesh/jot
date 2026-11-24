use std::ops::{Add, AddAssign, Mul, Neg, Sub, SubAssign};

use crate::color::Rgba;

#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub(crate) struct Point {
    pub(crate) x: f32,
    pub(crate) y: f32,
}

impl Point {
    pub(crate) const ZERO: Self = Self { x: 0.0, y: 0.0 };

    #[inline]
    pub(crate) const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

#[inline]
pub(crate) const fn point(x: f32, y: f32) -> Point {
    Point { x, y }
}

impl Add for Point {
    type Output = Self;
    #[inline]
    fn add(self, rhs: Self) -> Self {
        Self {
            x: self.x + rhs.x,
            y: self.y + rhs.y,
        }
    }
}

impl AddAssign for Point {
    #[inline]
    fn add_assign(&mut self, rhs: Self) {
        self.x += rhs.x;
        self.y += rhs.y;
    }
}

impl Sub for Point {
    type Output = Self;
    #[inline]
    fn sub(self, rhs: Self) -> Self {
        Self {
            x: self.x - rhs.x,
            y: self.y - rhs.y,
        }
    }
}

impl SubAssign for Point {
    #[inline]
    fn sub_assign(&mut self, rhs: Self) {
        self.x -= rhs.x;
        self.y -= rhs.y;
    }
}

impl Neg for Point {
    type Output = Self;
    #[inline]
    fn neg(self) -> Self {
        Self {
            x: -self.x,
            y: -self.y,
        }
    }
}

impl Mul<f32> for Point {
    type Output = Self;
    #[inline]
    fn mul(self, rhs: f32) -> Self {
        Self {
            x: self.x * rhs,
            y: self.y * rhs,
        }
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub(crate) struct Size {
    pub(crate) width: f32,
    pub(crate) height: f32,
}

impl Size {
    pub(crate) const ZERO: Self = Self {
        width: 0.0,
        height: 0.0,
    };

    #[inline]
    pub(crate) const fn new(width: f32, height: f32) -> Self {
        Self { width, height }
    }

    #[inline]
    pub(crate) fn is_empty(self) -> bool {
        self.width <= 0.0 || self.height <= 0.0
    }
}

#[inline]
pub(crate) const fn size(width: f32, height: f32) -> Size {
    Size { width, height }
}

#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub(crate) struct Rect {
    pub(crate) origin: Point,
    pub(crate) size: Size,
}

impl Rect {
    pub(crate) const ZERO: Self = Self {
        origin: Point::ZERO,
        size: Size::ZERO,
    };

    #[inline]
    pub(crate) const fn new(origin: Point, size: Size) -> Self {
        Self { origin, size }
    }

    #[inline]
    pub(crate) fn from_corners(origin: Point, lower_right: Point) -> Self {
        Self {
            origin,
            size: Size {
                width: (lower_right.x - origin.x).max(0.0),
                height: (lower_right.y - origin.y).max(0.0),
            },
        }
    }

    #[inline]
    pub(crate) const fn left(self) -> f32 {
        self.origin.x
    }

    #[inline]
    pub(crate) const fn top(self) -> f32 {
        self.origin.y
    }

    #[inline]
    pub(crate) fn right(self) -> f32 {
        self.origin.x + self.size.width
    }

    #[inline]
    pub(crate) fn bottom(self) -> f32 {
        self.origin.y + self.size.height
    }

    #[inline]
    pub(crate) fn center(self) -> Point {
        point(
            self.origin.x + self.size.width * 0.5,
            self.origin.y + self.size.height * 0.5,
        )
    }

    #[inline]
    pub(crate) fn is_empty(self) -> bool {
        self.size.is_empty()
    }

    #[inline]
    pub(crate) fn contains(self, pt: Point) -> bool {
        pt.x >= self.left() && pt.x < self.right() && pt.y >= self.top() && pt.y < self.bottom()
    }

    #[inline]
    pub(crate) fn intersect(self, other: Self) -> Self {
        let left = self.left().max(other.left());
        let top = self.top().max(other.top());
        let right = self.right().min(other.right()).max(left);
        let bottom = self.bottom().min(other.bottom()).max(top);
        Self::from_corners(point(left, top), point(right, bottom))
    }

    #[cfg(test)]
    #[inline]
    pub(crate) fn inset(self, edges: Edges) -> Self {
        Self {
            origin: point(self.origin.x + edges.left, self.origin.y + edges.top),
            size: size(
                (self.size.width - edges.horiz()).max(0.0),
                (self.size.height - edges.vert()).max(0.0),
            ),
        }
    }

    #[inline]
    pub(crate) fn translate(self, delta: Point) -> Self {
        Self {
            origin: self.origin + delta,
            size: self.size,
        }
    }

    #[inline]
    pub(crate) fn union(self, other: Self) -> Self {
        if self.is_empty() {
            return other;
        }
        if other.is_empty() {
            return self;
        }
        let left = self.left().min(other.left());
        let top = self.top().min(other.top());
        let right = self.right().max(other.right());
        let bottom = self.bottom().max(other.bottom());
        Self::from_corners(point(left, top), point(right, bottom))
    }

    #[inline]
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

#[cfg(test)]
#[inline]
pub(crate) const fn rect(x: f32, y: f32, width: f32, height: f32) -> Rect {
    Rect {
        origin: Point { x, y },
        size: Size { width, height },
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub(crate) struct Edges<T = f32> {
    pub(crate) top: T,
    pub(crate) right: T,
    pub(crate) bottom: T,
    pub(crate) left: T,
}

impl<T: Copy> Edges<T> {
    #[inline]
    pub(crate) const fn all(value: T) -> Self {
        Self {
            top: value,
            right: value,
            bottom: value,
            left: value,
        }
    }

    #[cfg(test)]
    #[inline]
    pub(crate) const fn xy(horiz: T, vert: T) -> Self {
        Self {
            top: vert,
            right: horiz,
            bottom: vert,
            left: horiz,
        }
    }
}

impl Edges<f32> {
    pub(crate) const ZERO: Self = Self::all(0.0);

    #[inline]
    pub(crate) fn horiz(self) -> f32 {
        self.left + self.right
    }

    #[inline]
    pub(crate) fn vert(self) -> f32 {
        self.top + self.bottom
    }

    #[inline]
    pub(crate) fn any_nonzero(self) -> bool {
        self.top != 0.0 || self.right != 0.0 || self.bottom != 0.0 || self.left != 0.0
    }

    #[inline]
    pub(crate) fn add(self, other: Self) -> Self {
        Self {
            top: self.top + other.top,
            right: self.right + other.right,
            bottom: self.bottom + other.bottom,
            left: self.left + other.left,
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

    #[inline]
    pub(crate) const fn all(r: f32) -> Self {
        Self {
            top_left: r,
            top_right: r,
            bottom_right: r,
            bottom_left: r,
        }
    }

    #[inline]
    pub(crate) const fn top(r: f32) -> Self {
        Self {
            top_left: r,
            top_right: r,
            bottom_right: 0.0,
            bottom_left: 0.0,
        }
    }

    #[inline]
    pub(crate) const fn bottom(r: f32) -> Self {
        Self {
            top_left: 0.0,
            top_right: 0.0,
            bottom_right: r,
            bottom_left: r,
        }
    }

    #[inline]
    pub(crate) fn clamp_to(self, size: Size) -> Self {
        let tl = self.top_left.max(0.0);
        let tr = self.top_right.max(0.0);
        let br = self.bottom_right.max(0.0);
        let bl = self.bottom_left.max(0.0);
        let w = size.width.max(0.0);
        let h = size.height.max(0.0);
        let mut factor = 1.0f32;
        for (sum, span) in [(tl + tr, w), (bl + br, w), (tl + bl, h), (tr + br, h)] {
            if sum > 0.0 {
                factor = factor.min(span / sum);
            }
        }
        Self {
            top_left: tl * factor,
            top_right: tr * factor,
            bottom_right: br * factor,
            bottom_left: bl * factor,
        }
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

#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub(crate) struct Shadows(pub(crate) [Option<BoxShadow>; 2]);

impl Shadows {
    pub(crate) const NONE: Self = Self([None, None]);

    #[inline]
    pub(crate) fn is_empty(self) -> bool {
        self.0[0].is_none() && self.0[1].is_none()
    }

    #[inline]
    pub(crate) fn iter(&self) -> impl Iterator<Item = &BoxShadow> {
        self.0.iter().flatten()
    }
}

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
