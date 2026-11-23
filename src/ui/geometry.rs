//! 2D geometry primitives and unit types (`Pixels`, `DevicePixels`, `ScaledPixels`, lengths).

use std::{
    cmp,
    fmt::{self, Debug, Display},
    hash::Hash,
    ops::{Add, AddAssign, Div, DivAssign, Mul, MulAssign, Neg, Sub, SubAssign},
};

use crate::ui::{
    App, DisplayId,
    refineable::{IsEmpty, Refineable},
};

/// Axis in 2D cartesian space.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum Axis {
    Vertical,
    Horizontal,
}

/// 2D point `(x, y)`.
#[derive(Default, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(C)]
pub(crate) struct Point<T: Clone + Debug + Default + PartialEq> {
    pub(crate) x: T,
    pub(crate) y: T,
}

#[derive(Clone, PartialEq)]
pub(crate) struct PointRefinement<T: Clone + Debug + Default + PartialEq> {
    pub(crate) x: Option<T>,
    pub(crate) y: Option<T>,
}

impl<T: Clone + Debug + Default + PartialEq> Default for PointRefinement<T> {
    fn default() -> Self {
        Self { x: None, y: None }
    }
}

impl<T: Clone + Debug + Default + PartialEq> PointRefinement<T> {
    pub(crate) fn is_some(&self) -> bool {
        self.x.is_some() || self.y.is_some()
    }
}

impl<T: Clone + Debug + Default + PartialEq> fmt::Debug for PointRefinement<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut s = f.debug_struct("PointRefinement");
        let mut all = true;
        if self.x.is_some() { s.field("x", &self.x); } else { all = false; }
        if self.y.is_some() { s.field("y", &self.y); } else { all = false; }
        if all { s.finish() } else { s.finish_non_exhaustive() }
    }
}

impl<T: Clone + Debug + Default + PartialEq> IsEmpty for PointRefinement<T> {
    fn is_empty(&self) -> bool {
        self.x.is_none() && self.y.is_none()
    }
}

impl<T: Clone + Debug + Default + PartialEq> From<PointRefinement<T>> for Point<T> {
    fn from(v: PointRefinement<T>) -> Self {
        Self {
            x: v.x.unwrap_or_default(),
            y: v.y.unwrap_or_default(),
        }
    }
}

impl<T: Clone + Debug + Default + PartialEq> Refineable for Point<T> {
    type Refinement = PointRefinement<T>;
    fn refine(&mut self, r: &Self::Refinement) {
        if let Some(v) = &r.x { self.x = v.clone(); }
        if let Some(v) = &r.y { self.y = v.clone(); }
    }
}

impl<T: Clone + Debug + Default + PartialEq> Refineable for PointRefinement<T> {
    type Refinement = PointRefinement<T>;
    fn refine(&mut self, r: &Self::Refinement) {
        if let Some(v) = &r.x { self.x = Some(v.clone()); }
        if let Some(v) = &r.y { self.y = Some(v.clone()); }
    }
}

impl<T: Add<Output = O> + Clone + Debug + Default + PartialEq, O: Clone + Debug + Default + PartialEq> Add for Point<T> {
    type Output = Point<O>;
    fn add(self, rhs: Self) -> Self::Output { Point { x: self.x + rhs.x, y: self.y + rhs.y } }
}
impl<T: AddAssign + Clone + Debug + Default + PartialEq> AddAssign for Point<T> {
    fn add_assign(&mut self, rhs: Self) { self.x += rhs.x; self.y += rhs.y; }
}
impl<T: Sub<Output = O> + Clone + Debug + Default + PartialEq, O: Clone + Debug + Default + PartialEq> Sub for Point<T> {
    type Output = Point<O>;
    fn sub(self, rhs: Self) -> Self::Output { Point { x: self.x - rhs.x, y: self.y - rhs.y } }
}
impl<T: SubAssign + Clone + Debug + Default + PartialEq> SubAssign for Point<T> {
    fn sub_assign(&mut self, rhs: Self) { self.x -= rhs.x; self.y -= rhs.y; }
}
impl<T: Neg<Output = O> + Clone + Debug + Default + PartialEq, O: Clone + Debug + Default + PartialEq> Neg for Point<T> {
    type Output = Point<O>;
    fn neg(self) -> Self::Output { Point { x: -self.x, y: -self.y } }
}

pub(crate) const fn point<T: Clone + Debug + Default + PartialEq>(x: T, y: T) -> Point<T> {
    Point { x, y }
}

impl<T: Clone + Debug + Default + PartialEq> Point<T> {
    pub(crate) const fn new(x: T, y: T) -> Self {
        Self { x, y }
    }

    #[must_use]
    pub(crate) fn map<U: Clone + Debug + Default + PartialEq>(&self, f: impl Fn(T) -> U) -> Point<U> {
        Point {
            x: f(self.x.clone()),
            y: f(self.y.clone()),
        }
    }
}

impl Point<Pixels> {
    pub(crate) fn scale(&self, factor: f32) -> Point<ScaledPixels> {
        Point {
            x: self.x.scale(factor),
            y: self.y.scale(factor),
        }
    }
}

impl<T, Rhs> Mul<Rhs> for Point<T>
where
    T: Mul<Rhs, Output = T> + Clone + Debug + Default + PartialEq,
    Rhs: Clone + Debug,
{
    type Output = Point<T>;

    fn mul(self, rhs: Rhs) -> Self::Output {
        Point {
            x: self.x * rhs.clone(),
            y: self.y * rhs,
        }
    }
}

impl<T, S> MulAssign<S> for Point<T>
where
    T: Mul<S, Output = T> + Clone + Debug + Default + PartialEq,
    S: Clone,
{
    fn mul_assign(&mut self, rhs: S) {
        self.x = self.x.clone() * rhs.clone();
        self.y = self.y.clone() * rhs;
    }
}

impl<T, S> Div<S> for Point<T>
where
    T: Div<S, Output = T> + Clone + Debug + Default + PartialEq,
    S: Clone,
{
    type Output = Self;

    fn div(self, rhs: S) -> Self::Output {
        Self {
            x: self.x / rhs.clone(),
            y: self.y / rhs,
        }
    }
}

impl<T: PartialOrd + Clone + Debug + Default + PartialEq> Point<T> {
    pub(crate) fn max(&self, other: &Self) -> Self {
        Point {
            x: if self.x > other.x { self.x.clone() } else { other.x.clone() },
            y: if self.y > other.y { self.y.clone() } else { other.y.clone() },
        }
    }

    pub(crate) fn min(&self, other: &Self) -> Self {
        Point {
            x: if self.x <= other.x { self.x.clone() } else { other.x.clone() },
            y: if self.y <= other.y { self.y.clone() } else { other.y.clone() },
        }
    }
}

impl<T: Clone + Debug + Default + PartialEq> Clone for Point<T> {
    fn clone(&self) -> Self {
        Self { x: self.x.clone(), y: self.y.clone() }
    }
}

impl<T: Clone + Debug + Default + PartialEq + Display> Display for Point<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({}, {})", self.x, self.y)
    }
}

/// 2D size `(width, height)`.
#[derive(Clone, Copy, Default, Hash, PartialEq)]
#[repr(C)]
pub(crate) struct Size<T: Clone + Debug + Default + PartialEq> {
    pub(crate) width: T,
    pub(crate) height: T,
}

#[derive(Clone, PartialEq)]
pub(crate) struct SizeRefinement<T: Clone + Debug + Default + PartialEq> {
    pub(crate) width: Option<T>,
    pub(crate) height: Option<T>,
}

impl<T: Clone + Debug + Default + PartialEq> Default for SizeRefinement<T> {
    fn default() -> Self {
        Self { width: None, height: None }
    }
}

impl<T: Clone + Debug + Default + PartialEq> SizeRefinement<T> {
    pub(crate) fn is_some(&self) -> bool {
        self.width.is_some() || self.height.is_some()
    }
}

impl<T: Clone + Debug + Default + PartialEq> fmt::Debug for SizeRefinement<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut s = f.debug_struct("SizeRefinement");
        let mut all = true;
        if self.width.is_some() { s.field("width", &self.width); } else { all = false; }
        if self.height.is_some() { s.field("height", &self.height); } else { all = false; }
        if all { s.finish() } else { s.finish_non_exhaustive() }
    }
}

impl<T: Clone + Debug + Default + PartialEq> IsEmpty for SizeRefinement<T> {
    fn is_empty(&self) -> bool {
        self.width.is_none() && self.height.is_none()
    }
}

impl<T: Clone + Debug + Default + PartialEq> From<SizeRefinement<T>> for Size<T> {
    fn from(v: SizeRefinement<T>) -> Self {
        Self {
            width: v.width.unwrap_or_default(),
            height: v.height.unwrap_or_default(),
        }
    }
}

impl<T: Clone + Debug + Default + PartialEq> Refineable for Size<T> {
    type Refinement = SizeRefinement<T>;
    fn refine(&mut self, r: &Self::Refinement) {
        if let Some(v) = &r.width { self.width = v.clone(); }
        if let Some(v) = &r.height { self.height = v.clone(); }
    }
}

impl<T: Clone + Debug + Default + PartialEq> Refineable for SizeRefinement<T> {
    type Refinement = SizeRefinement<T>;
    fn refine(&mut self, r: &Self::Refinement) {
        if let Some(v) = &r.width { self.width = Some(v.clone()); }
        if let Some(v) = &r.height { self.height = Some(v.clone()); }
    }
}

impl<T: Add<Output = O> + Clone + Debug + Default + PartialEq, O: Clone + Debug + Default + PartialEq> Add for Size<T> {
    type Output = Size<O>;
    fn add(self, rhs: Self) -> Self::Output { Size { width: self.width + rhs.width, height: self.height + rhs.height } }
}
impl<T: Sub<Output = O> + Clone + Debug + Default + PartialEq, O: Clone + Debug + Default + PartialEq> Sub for Size<T> {
    type Output = Size<O>;
    fn sub(self, rhs: Self) -> Self::Output { Size { width: self.width - rhs.width, height: self.height - rhs.height } }
}
impl<T: Div<Rhs, Output = O> + Clone + Debug + Default + PartialEq, Rhs: Copy, O: Clone + Debug + Default + PartialEq> Div<Rhs> for Size<T> {
    type Output = Size<O>;
    fn div(self, rhs: Rhs) -> Self::Output { Size { width: self.width / rhs, height: self.height / rhs } }
}
impl<T: Neg<Output = O> + Clone + Debug + Default + PartialEq, O: Clone + Debug + Default + PartialEq> Neg for Size<T> {
    type Output = Size<O>;
    fn neg(self) -> Self::Output { Size { width: -self.width, height: -self.height } }
}

pub(crate) const fn size<T: Clone + Debug + Default + PartialEq>(width: T, height: T) -> Size<T> {
    Size { width, height }
}

impl<T: Clone + Debug + Default + PartialEq> Size<T> {
    pub(crate) fn map<U: Clone + Debug + Default + PartialEq>(&self, f: impl Fn(T) -> U) -> Size<U> {
        Size {
            width: f(self.width.clone()),
            height: f(self.height.clone()),
        }
    }
}

impl<T: PartialOrd + Clone + Debug + Default + PartialEq> Size<T> {
    pub(crate) fn max(&self, other: &Self) -> Self {
        Size {
            width: if self.width >= other.width { self.width.clone() } else { other.width.clone() },
            height: if self.height >= other.height { self.height.clone() } else { other.height.clone() },
        }
    }

    pub(crate) fn min(&self, other: &Self) -> Self {
        Size {
            width: if self.width >= other.width { other.width.clone() } else { self.width.clone() },
            height: if self.height >= other.height { other.height.clone() } else { self.height.clone() },
        }
    }
}

impl<T, Rhs> Mul<Rhs> for Size<T>
where
    T: Mul<Rhs, Output = Rhs> + Clone + Debug + Default + PartialEq,
    Rhs: Clone + Debug + Default + PartialEq,
{
    type Output = Size<Rhs>;
    fn mul(self, rhs: Rhs) -> Self::Output {
        Size { width: self.width * rhs.clone(), height: self.height * rhs }
    }
}

impl<T, S> MulAssign<S> for Size<T>
where
    T: Mul<S, Output = T> + Clone + Debug + Default + PartialEq,
    S: Clone,
{
    fn mul_assign(&mut self, rhs: S) {
        self.width = self.width.clone() * rhs.clone();
        self.height = self.height.clone() * rhs;
    }
}

impl<T: Eq + Clone + Debug + Default + PartialEq> Eq for Size<T> {}

impl<T: Clone + Debug + Default + PartialEq> Debug for Size<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Size {{ {:?} × {:?} }}", self.width, self.height)
    }
}

impl<T: Clone + Debug + Default + PartialEq + Display> Display for Size<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} × {}", self.width, self.height)
    }
}

impl<T: Clone + Debug + Default + PartialEq> From<Point<T>> for Size<T> {
    fn from(point: Point<T>) -> Self {
        Self { width: point.x, height: point.y }
    }
}

impl From<Size<Pixels>> for Size<DefiniteLength> {
    fn from(size: Size<Pixels>) -> Self {
        Size { width: size.width.into(), height: size.height.into() }
    }
}

impl Size<Length> {
    pub(crate) fn auto() -> Self {
        Self { width: Length::Auto, height: Length::Auto }
    }
}

/// Axis-aligned rectangle defined by `origin` and `size`.
#[derive(Copy, Clone, Default, Debug, Eq, PartialEq, Hash)]
#[repr(C)]
pub(crate) struct Bounds<T: Clone + Debug + Default + PartialEq> {
    pub(crate) origin: Point<T>,
    pub(crate) size: Size<T>,
}

pub(crate) fn bounds<T: Clone + Debug + Default + PartialEq>(
    origin: Point<T>,
    size: Size<T>,
) -> Bounds<T> {
    Bounds { origin, size }
}

impl Bounds<Pixels> {
    /// Generate centered bounds for the given display (or primary display if `None`).
    pub(crate) fn centered(display_id: Option<DisplayId>, size: Size<Pixels>, cx: &App) -> Self {
        let display = display_id
            .and_then(|id| cx.find_display(id))
            .or_else(|| cx.primary_display());

        display
            .map(|display| {
                let visible_bounds = display.visible_bounds();
                Bounds::centered_at(visible_bounds.center(), size.min(&visible_bounds.size))
            })
            .unwrap_or_else(|| Bounds {
                origin: point(px(0.), px(0.)),
                size,
            })
    }
}

impl<T: Clone + Debug + Default + PartialEq> Bounds<T> {
    pub(crate) fn new(origin: Point<T>, size: Size<T>) -> Self {
        Bounds { origin, size }
    }
}

impl<T: Sub<Output = T> + Clone + Debug + Default + PartialEq> Bounds<T> {
    pub(crate) fn from_corners(top_left: Point<T>, bottom_right: Point<T>) -> Self {
        let origin = Point { x: top_left.x.clone(), y: top_left.y.clone() };
        let size = Size {
            width: bottom_right.x - top_left.x,
            height: bottom_right.y - top_left.y,
        };
        Bounds { origin, size }
    }
}

impl<T: Sub<Output = T> + Half + Clone + Debug + Default + PartialEq> Bounds<T> {
    pub(crate) fn from_anchor_and_size(corner: Anchor, origin: Point<T>, size: Size<T>) -> Bounds<T> {
        let origin = match corner {
            Anchor::TopLeft => origin,
            Anchor::TopRight => Point { x: origin.x - size.width.clone(), y: origin.y },
            Anchor::BottomLeft => Point { x: origin.x, y: origin.y - size.height.clone() },
            Anchor::BottomRight => Point {
                x: origin.x - size.width.clone(),
                y: origin.y - size.height.clone(),
            },
            Anchor::TopCenter => Point { x: origin.x - size.width.half(), y: origin.y },
            Anchor::BottomCenter => Point {
                x: origin.x - size.width.half(),
                y: origin.y - size.height.clone(),
            },
            Anchor::LeftCenter => Point { x: origin.x, y: origin.y - size.height.half() },
            Anchor::RightCenter => Point {
                x: origin.x - size.width.clone(),
                y: origin.y - size.height.half(),
            },
        };
        Bounds { origin, size }
    }

    pub(crate) fn centered_at(center: Point<T>, size: Size<T>) -> Self {
        let origin = Point {
            x: center.x - size.width.half(),
            y: center.y - size.height.half(),
        };
        Self::new(origin, size)
    }
}

impl<T: PartialOrd + Add<T, Output = T> + Clone + Debug + Default + PartialEq> Bounds<T> {
    pub(crate) fn intersects(&self, other: &Bounds<T>) -> bool {
        let my_lower_right = self.bottom_right();
        let their_lower_right = other.bottom_right();
        self.origin.x < their_lower_right.x
            && my_lower_right.x > other.origin.x
            && self.origin.y < their_lower_right.y
            && my_lower_right.y > other.origin.y
    }
}

impl<T: Add<T, Output = T> + Half + Clone + Debug + Default + PartialEq> Bounds<T> {
    pub(crate) fn center(&self) -> Point<T> {
        Point {
            x: self.origin.x.clone() + self.size.width.clone().half(),
            y: self.origin.y.clone() + self.size.height.clone().half(),
        }
    }
}

impl<T: Add<T, Output = T> + Clone + Debug + Default + PartialEq> Bounds<T> {
    pub(crate) fn half_perimeter(&self) -> T {
        self.size.width.clone() + self.size.height.clone()
    }
}

impl<T: Add<T, Output = T> + Sub<Output = T> + Clone + Debug + Default + PartialEq> Bounds<T> {
    #[must_use]
    pub(crate) fn dilate(&self, amount: T) -> Bounds<T> {
        let double_amount = amount.clone() + amount.clone();
        Bounds {
            origin: self.origin.clone() - point(amount.clone(), amount),
            size: self.size.clone() + size(double_amount.clone(), double_amount),
        }
    }
}

impl<T: PartialOrd + Add<T, Output = T> + Sub<Output = T> + Clone + Debug + Default + PartialEq> Bounds<T> {
    pub(crate) fn intersect(&self, other: &Self) -> Self {
        let upper_left = self.origin.max(&other.origin);
        let bottom_right = self.bottom_right().min(&other.bottom_right()).max(&upper_left);
        Self::from_corners(upper_left, bottom_right)
    }

    pub(crate) fn union(&self, other: &Self) -> Self {
        let top_left = self.origin.min(&other.origin);
        let bottom_right = self.bottom_right().max(&other.bottom_right());
        Bounds::from_corners(top_left, bottom_right)
    }
}

impl<T, Rhs> Mul<Rhs> for Bounds<T>
where
    T: Mul<Rhs, Output = Rhs> + Clone + Debug + Default + PartialEq,
    Point<T>: Mul<Rhs, Output = Point<Rhs>>,
    Rhs: Clone + Debug + Default + PartialEq,
{
    type Output = Bounds<Rhs>;
    fn mul(self, rhs: Rhs) -> Self::Output {
        Bounds { origin: self.origin * rhs.clone(), size: self.size * rhs }
    }
}

impl<T, S> MulAssign<S> for Bounds<T>
where
    T: Mul<S, Output = T> + Clone + Debug + Default + PartialEq,
    S: Clone,
{
    fn mul_assign(&mut self, rhs: S) {
        self.origin *= rhs.clone();
        self.size *= rhs;
    }
}

impl<T, S> Div<S> for Bounds<T>
where
    Size<T>: Div<S, Output = Size<T>>,
    T: Div<S, Output = T> + Clone + Debug + Default + PartialEq,
    S: Clone,
{
    type Output = Self;
    fn div(self, rhs: S) -> Self {
        Self { origin: self.origin / rhs.clone(), size: self.size / rhs }
    }
}

impl<T: Add<T, Output = T> + Clone + Debug + Default + PartialEq> Add<Point<T>> for Bounds<T> {
    type Output = Self;
    fn add(self, rhs: Point<T>) -> Self {
        Self { origin: self.origin + rhs, size: self.size }
    }
}

impl<T: Sub<T, Output = T> + Clone + Debug + Default + PartialEq> Sub<Point<T>> for Bounds<T> {
    type Output = Self;
    fn sub(self, rhs: Point<T>) -> Self {
        Self { origin: self.origin - rhs, size: self.size }
    }
}

impl<T: Clone + Debug + Default + PartialEq> From<Size<T>> for Point<T> {
    fn from(size: Size<T>) -> Self {
        Self { x: size.width, y: size.height }
    }
}

impl<T: Add<T, Output = T> + Clone + Debug + Default + PartialEq> Bounds<T> {
    pub(crate) fn top(&self) -> T {
        self.origin.y.clone()
    }
    pub(crate) fn bottom(&self) -> T {
        self.origin.y.clone() + self.size.height.clone()
    }
    pub(crate) fn left(&self) -> T {
        self.origin.x.clone()
    }
    pub(crate) fn right(&self) -> T {
        self.origin.x.clone() + self.size.width.clone()
    }
    pub(crate) fn top_right(&self) -> Point<T> {
        Point { x: self.origin.x.clone() + self.size.width.clone(), y: self.origin.y.clone() }
    }
    pub(crate) fn bottom_right(&self) -> Point<T> {
        Point {
            x: self.origin.x.clone() + self.size.width.clone(),
            y: self.origin.y.clone() + self.size.height.clone(),
        }
    }
    pub(crate) fn bottom_left(&self) -> Point<T> {
        Point { x: self.origin.x.clone(), y: self.origin.y.clone() + self.size.height.clone() }
    }
}

impl<T: Add<T, Output = T> + PartialOrd + Clone + Debug + Default + PartialEq> Bounds<T> {
    pub(crate) fn contains(&self, point: &Point<T>) -> bool {
        point.x >= self.origin.x
            && point.x < self.origin.x.clone() + self.size.width.clone()
            && point.y >= self.origin.y
            && point.y < self.origin.y.clone() + self.size.height.clone()
    }

    pub(crate) fn map<U: Clone + Debug + Default + PartialEq>(&self, f: impl Fn(T) -> U) -> Bounds<U> {
        Bounds { origin: self.origin.map(&f), size: self.size.map(f) }
    }
}

impl<T: PartialOrd + Clone + Debug + Default + PartialEq> Bounds<T> {
    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.size.width <= T::default() || self.size.height <= T::default()
    }
}

impl<T: Clone + Debug + Default + PartialEq + Display + Add<T, Output = T>> Display for Bounds<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} - {} (size {})", self.origin, self.bottom_right(), self.size)
    }
}

impl Size<DevicePixels> {
    pub(crate) fn to_pixels(self, scale_factor: f32) -> Size<Pixels> {
        size(px(self.width.0 as f32 / scale_factor), px(self.height.0 as f32 / scale_factor))
    }
}

impl Size<Pixels> {
    pub(crate) fn to_device_pixels(self, scale_factor: f32) -> Size<DevicePixels> {
        size(
            DevicePixels((self.width.0 * scale_factor).round() as i32),
            DevicePixels((self.height.0 * scale_factor).round() as i32),
        )
    }
}

impl Bounds<Pixels> {
    pub(crate) fn to_device_pixels(self, factor: f32) -> Bounds<DevicePixels> {
        Bounds {
            origin: point(
                DevicePixels((self.origin.x.0 * factor).round() as i32),
                DevicePixels((self.origin.y.0 * factor).round() as i32),
            ),
            size: self.size.to_device_pixels(factor),
        }
    }
}

/// Box edges (`top`, `right`, `bottom`, `left`).
#[derive(Clone, Default, Debug, Eq, PartialEq)]
#[repr(C)]
pub(crate) struct Edges<T: Clone + Debug + Default + PartialEq> {
    pub(crate) top: T,
    pub(crate) right: T,
    pub(crate) bottom: T,
    pub(crate) left: T,
}

#[derive(Clone, PartialEq)]
pub(crate) struct EdgesRefinement<T: Clone + Debug + Default + PartialEq> {
    pub(crate) top: Option<T>,
    pub(crate) right: Option<T>,
    pub(crate) bottom: Option<T>,
    pub(crate) left: Option<T>,
}

impl<T: Clone + Debug + Default + PartialEq> Default for EdgesRefinement<T> {
    fn default() -> Self {
        Self { top: None, right: None, bottom: None, left: None }
    }
}

impl<T: Clone + Debug + Default + PartialEq> fmt::Debug for EdgesRefinement<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut s = f.debug_struct("EdgesRefinement");
        let mut all = true;
        if self.top.is_some() { s.field("top", &self.top); } else { all = false; }
        if self.right.is_some() { s.field("right", &self.right); } else { all = false; }
        if self.bottom.is_some() { s.field("bottom", &self.bottom); } else { all = false; }
        if self.left.is_some() { s.field("left", &self.left); } else { all = false; }
        if all { s.finish() } else { s.finish_non_exhaustive() }
    }
}

impl<T: Clone + Debug + Default + PartialEq> IsEmpty for EdgesRefinement<T> {
    fn is_empty(&self) -> bool {
        self.top.is_none() && self.right.is_none() && self.bottom.is_none() && self.left.is_none()
    }
}

impl<T: Clone + Debug + Default + PartialEq> From<EdgesRefinement<T>> for Edges<T> {
    fn from(v: EdgesRefinement<T>) -> Self {
        Self {
            top: v.top.unwrap_or_default(),
            right: v.right.unwrap_or_default(),
            bottom: v.bottom.unwrap_or_default(),
            left: v.left.unwrap_or_default(),
        }
    }
}

impl<T: Clone + Debug + Default + PartialEq> Refineable for Edges<T> {
    type Refinement = EdgesRefinement<T>;
    fn refine(&mut self, r: &Self::Refinement) {
        if let Some(v) = &r.top { self.top = v.clone(); }
        if let Some(v) = &r.right { self.right = v.clone(); }
        if let Some(v) = &r.bottom { self.bottom = v.clone(); }
        if let Some(v) = &r.left { self.left = v.clone(); }
    }
}

impl<T: Clone + Debug + Default + PartialEq> Refineable for EdgesRefinement<T> {
    type Refinement = EdgesRefinement<T>;
    fn refine(&mut self, r: &Self::Refinement) {
        if let Some(v) = &r.top { self.top = Some(v.clone()); }
        if let Some(v) = &r.right { self.right = Some(v.clone()); }
        if let Some(v) = &r.bottom { self.bottom = Some(v.clone()); }
        if let Some(v) = &r.left { self.left = Some(v.clone()); }
    }
}

impl<T: Mul<Output = T> + Clone + Debug + Default + PartialEq> Mul for Edges<T> {
    type Output = Self;
    fn mul(self, rhs: Self) -> Self::Output {
        Self {
            top: self.top.clone() * rhs.top,
            right: self.right.clone() * rhs.right,
            bottom: self.bottom.clone() * rhs.bottom,
            left: self.left * rhs.left,
        }
    }
}

impl<T: Mul<S, Output = T> + Clone + Debug + Default + PartialEq, S: Clone> MulAssign<S> for Edges<T> {
    fn mul_assign(&mut self, rhs: S) {
        self.top = self.top.clone() * rhs.clone();
        self.right = self.right.clone() * rhs.clone();
        self.bottom = self.bottom.clone() * rhs.clone();
        self.left = self.left.clone() * rhs;
    }
}

impl<T: Clone + Debug + Default + PartialEq + Copy> Copy for Edges<T> {}

impl<T: Clone + Debug + Default + PartialEq> Edges<T> {
    pub(crate) fn all(value: T) -> Self {
        Self { top: value.clone(), right: value.clone(), bottom: value.clone(), left: value }
    }

    pub(crate) fn map<U: Clone + Debug + Default + PartialEq>(&self, f: impl Fn(&T) -> U) -> Edges<U> {
        Edges {
            top: f(&self.top),
            right: f(&self.right),
            bottom: f(&self.bottom),
            left: f(&self.left),
        }
    }

    pub(crate) fn any<F: Fn(&T) -> bool>(&self, predicate: F) -> bool {
        predicate(&self.top)
            || predicate(&self.right)
            || predicate(&self.bottom)
            || predicate(&self.left)
    }
}

impl Edges<Length> {
    pub(crate) fn auto() -> Self {
        Self { top: Length::Auto, right: Length::Auto, bottom: Length::Auto, left: Length::Auto }
    }

    pub(crate) fn zero() -> Self {
        Self {
            top: px(0.).into(),
            right: px(0.).into(),
            bottom: px(0.).into(),
            left: px(0.).into(),
        }
    }
}

impl Edges<DefiniteLength> {
    pub(crate) fn zero() -> Self {
        Self {
            top: px(0.).into(),
            right: px(0.).into(),
            bottom: px(0.).into(),
            left: px(0.).into(),
        }
    }
}

impl From<f32> for Edges<Pixels> {
    fn from(val: f32) -> Self {
        Pixels(val).into()
    }
}

impl From<Pixels> for Edges<Pixels> {
    fn from(val: Pixels) -> Self {
        Edges { top: val, right: val, bottom: val, left: val }
    }
}

/// Reference point on a 2D box, used to anchor positioned elements.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Anchor {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    TopCenter,
    BottomCenter,
    LeftCenter,
    RightCenter,
}

impl Anchor {
    #[must_use]
    pub(crate) fn other_side_along(self, axis: Axis) -> Self {
        match axis {
            Axis::Vertical => match self {
                Anchor::TopLeft => Anchor::BottomLeft,
                Anchor::TopRight => Anchor::BottomRight,
                Anchor::BottomLeft => Anchor::TopLeft,
                Anchor::BottomRight => Anchor::TopRight,
                Anchor::TopCenter => Anchor::BottomCenter,
                Anchor::BottomCenter => Anchor::TopCenter,
                Anchor::LeftCenter => Anchor::LeftCenter,
                Anchor::RightCenter => Anchor::RightCenter,
            },
            Axis::Horizontal => match self {
                Anchor::TopLeft => Anchor::TopRight,
                Anchor::TopRight => Anchor::TopLeft,
                Anchor::BottomLeft => Anchor::BottomRight,
                Anchor::BottomRight => Anchor::BottomLeft,
                Anchor::TopCenter => Anchor::TopCenter,
                Anchor::BottomCenter => Anchor::BottomCenter,
                Anchor::LeftCenter => Anchor::RightCenter,
                Anchor::RightCenter => Anchor::LeftCenter,
            },
        }
    }
}

/// Box corners (`top_left`, `top_right`, `bottom_right`, `bottom_left`).
#[derive(Clone, Default, Debug, Eq, PartialEq)]
#[repr(C)]
pub(crate) struct Corners<T: Clone + Debug + Default + PartialEq> {
    pub(crate) top_left: T,
    pub(crate) top_right: T,
    pub(crate) bottom_right: T,
    pub(crate) bottom_left: T,
}

#[derive(Clone, PartialEq)]
pub(crate) struct CornersRefinement<T: Clone + Debug + Default + PartialEq> {
    pub(crate) top_left: Option<T>,
    pub(crate) top_right: Option<T>,
    pub(crate) bottom_right: Option<T>,
    pub(crate) bottom_left: Option<T>,
}

impl<T: Clone + Debug + Default + PartialEq> Default for CornersRefinement<T> {
    fn default() -> Self {
        Self { top_left: None, top_right: None, bottom_right: None, bottom_left: None }
    }
}

impl<T: Clone + Debug + Default + PartialEq> fmt::Debug for CornersRefinement<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut s = f.debug_struct("CornersRefinement");
        let mut all = true;
        if self.top_left.is_some() { s.field("top_left", &self.top_left); } else { all = false; }
        if self.top_right.is_some() { s.field("top_right", &self.top_right); } else { all = false; }
        if self.bottom_right.is_some() { s.field("bottom_right", &self.bottom_right); } else { all = false; }
        if self.bottom_left.is_some() { s.field("bottom_left", &self.bottom_left); } else { all = false; }
        if all { s.finish() } else { s.finish_non_exhaustive() }
    }
}

impl<T: Clone + Debug + Default + PartialEq> IsEmpty for CornersRefinement<T> {
    fn is_empty(&self) -> bool {
        self.top_left.is_none()
            && self.top_right.is_none()
            && self.bottom_right.is_none()
            && self.bottom_left.is_none()
    }
}

impl<T: Clone + Debug + Default + PartialEq> From<CornersRefinement<T>> for Corners<T> {
    fn from(v: CornersRefinement<T>) -> Self {
        Self {
            top_left: v.top_left.unwrap_or_default(),
            top_right: v.top_right.unwrap_or_default(),
            bottom_right: v.bottom_right.unwrap_or_default(),
            bottom_left: v.bottom_left.unwrap_or_default(),
        }
    }
}

impl<T: Clone + Debug + Default + PartialEq> Refineable for Corners<T> {
    type Refinement = CornersRefinement<T>;
    fn refine(&mut self, r: &Self::Refinement) {
        if let Some(v) = &r.top_left { self.top_left = v.clone(); }
        if let Some(v) = &r.top_right { self.top_right = v.clone(); }
        if let Some(v) = &r.bottom_right { self.bottom_right = v.clone(); }
        if let Some(v) = &r.bottom_left { self.bottom_left = v.clone(); }
    }
}

impl<T: Clone + Debug + Default + PartialEq> Refineable for CornersRefinement<T> {
    type Refinement = CornersRefinement<T>;
    fn refine(&mut self, r: &Self::Refinement) {
        if let Some(v) = &r.top_left { self.top_left = Some(v.clone()); }
        if let Some(v) = &r.top_right { self.top_right = Some(v.clone()); }
        if let Some(v) = &r.bottom_right { self.bottom_right = Some(v.clone()); }
        if let Some(v) = &r.bottom_left { self.bottom_left = Some(v.clone()); }
    }
}

impl Corners<Pixels> {
    #[must_use]
    pub(crate) fn scale(&self, factor: f32) -> Corners<ScaledPixels> {
        Corners {
            top_left: self.top_left.scale(factor),
            top_right: self.top_right.scale(factor),
            bottom_right: self.bottom_right.scale(factor),
            bottom_left: self.bottom_left.scale(factor),
        }
    }
}

impl<T: Div<f32, Output = T> + Ord + Clone + Debug + Default + PartialEq> Corners<T> {
    #[must_use]
    pub(crate) fn clamp_radii_for_quad_size(self, size: Size<T>) -> Corners<T> {
        let max = cmp::min(size.width, size.height) / 2.;
        Corners {
            top_left: cmp::min(self.top_left, max.clone()),
            top_right: cmp::min(self.top_right, max.clone()),
            bottom_right: cmp::min(self.bottom_right, max.clone()),
            bottom_left: cmp::min(self.bottom_left, max),
        }
    }
}

impl<T: Mul<Output = T> + Clone + Debug + Default + PartialEq> Mul for Corners<T> {
    type Output = Self;
    fn mul(self, rhs: Self) -> Self::Output {
        Self {
            top_left: self.top_left.clone() * rhs.top_left,
            top_right: self.top_right.clone() * rhs.top_right,
            bottom_right: self.bottom_right.clone() * rhs.bottom_right,
            bottom_left: self.bottom_left * rhs.bottom_left,
        }
    }
}

impl<T: Mul<S, Output = T> + Clone + Debug + Default + PartialEq, S: Clone> MulAssign<S> for Corners<T> {
    fn mul_assign(&mut self, rhs: S) {
        self.top_left = self.top_left.clone() * rhs.clone();
        self.top_right = self.top_right.clone() * rhs.clone();
        self.bottom_right = self.bottom_right.clone() * rhs.clone();
        self.bottom_left = self.bottom_left.clone() * rhs;
    }
}

impl<T: Copy + Clone + Debug + Default + PartialEq> Copy for Corners<T> {}

impl From<f32> for Corners<Pixels> {
    fn from(val: f32) -> Self {
        Pixels(val).into()
    }
}

impl From<Pixels> for Corners<Pixels> {
    fn from(val: Pixels) -> Self {
        Corners { top_left: val, top_right: val, bottom_right: val, bottom_left: val }
    }
}

macro_rules! impl_scalar_newtype_ops {
    ($name:ident, $inner:ty) => {
        impl Add for $name {
            type Output = Self;
            fn add(self, rhs: Self) -> Self { Self(self.0 + rhs.0) }
        }
        impl AddAssign for $name {
            fn add_assign(&mut self, rhs: Self) { self.0 += rhs.0; }
        }
        impl Sub for $name {
            type Output = Self;
            fn sub(self, rhs: Self) -> Self { Self(self.0 - rhs.0) }
        }
        impl SubAssign for $name {
            fn sub_assign(&mut self, rhs: Self) { self.0 -= rhs.0; }
        }
        impl Neg for $name {
            type Output = Self;
            fn neg(self) -> Self { Self(-self.0) }
        }
        impl Div<$inner> for $name {
            type Output = Self;
            fn div(self, rhs: $inner) -> Self { Self(self.0 / rhs) }
        }
        impl DivAssign<$inner> for $name {
            fn div_assign(&mut self, rhs: $inner) { self.0 /= rhs; }
        }
    };
}

/// Logical pixels (DPI-independent `f32`).
#[derive(Clone, Copy, Default, PartialEq)]
#[repr(transparent)]
pub(crate) struct Pixels(pub(crate) f32);
impl_scalar_newtype_ops!(Pixels, f32);

impl Div for Pixels {
    type Output = f32;
    fn div(self, rhs: Self) -> Self::Output { self.0 / rhs.0 }
}
impl DivAssign for Pixels {
    fn div_assign(&mut self, rhs: Self) { *self = Self(self.0 / rhs.0); }
}
impl std::ops::Rem for Pixels {
    type Output = Self;
    fn rem(self, rhs: Self) -> Self { Self(self.0 % rhs.0) }
}
impl std::ops::RemAssign for Pixels {
    fn rem_assign(&mut self, rhs: Self) { self.0 %= rhs.0; }
}
impl Mul<f32> for Pixels {
    type Output = Self;
    fn mul(self, rhs: f32) -> Self { Self(self.0 * rhs) }
}
impl Mul<Pixels> for f32 {
    type Output = Pixels;
    fn mul(self, rhs: Pixels) -> Self::Output { rhs * self }
}
impl Mul<usize> for Pixels {
    type Output = Self;
    fn mul(self, rhs: usize) -> Self { self * (rhs as f32) }
}
impl Mul<Pixels> for usize {
    type Output = Pixels;
    fn mul(self, rhs: Pixels) -> Pixels { rhs * self }
}
impl MulAssign<f32> for Pixels {
    fn mul_assign(&mut self, rhs: f32) { self.0 *= rhs; }
}
impl Display for Pixels {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { write!(f, "{}px", self.0) }
}
impl Debug for Pixels {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { Display::fmt(self, f) }
}
impl std::iter::Sum for Pixels {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self { iter.fold(Self::ZERO, |a, b| a + b) }
}
impl<'a> std::iter::Sum<&'a Pixels> for Pixels {
    fn sum<I: Iterator<Item = &'a Self>>(iter: I) -> Self { iter.fold(Self::ZERO, |a, b| a + *b) }
}

impl Pixels {
    pub(crate) const ZERO: Pixels = Pixels(0.0);
    pub(crate) const MAX: Pixels = Pixels(f32::MAX);

    pub(crate) fn as_f32(self) -> f32 { self.0 }
    pub(crate) fn round(&self) -> Self { Self(self.0.round()) }
    pub(crate) fn ceil(&self) -> Self { Self(self.0.ceil()) }
    #[must_use]
    pub(crate) fn scale(&self, factor: f32) -> ScaledPixels { ScaledPixels(self.0 * factor) }
}

impl Eq for Pixels {}
impl PartialOrd for Pixels {
    fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> { Some(self.cmp(other)) }
}
impl Ord for Pixels {
    fn cmp(&self, other: &Self) -> cmp::Ordering { self.0.total_cmp(&other.0) }
}
impl Hash for Pixels {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) { self.0.to_bits().hash(state); }
}

impl From<f64> for Pixels { fn from(v: f64) -> Self { Pixels(v as f32) } }
impl From<f32> for Pixels { fn from(v: f32) -> Self { Pixels(v) } }
impl From<Pixels> for f32 { fn from(v: Pixels) -> Self { v.0 } }
impl From<&Pixels> for f32 { fn from(v: &Pixels) -> Self { v.0 } }
impl From<Pixels> for f64 { fn from(v: Pixels) -> Self { v.0 as f64 } }
impl From<Pixels> for u32 { fn from(v: Pixels) -> Self { v.0 as u32 } }
impl From<&Pixels> for u32 { fn from(v: &Pixels) -> Self { v.0 as u32 } }
impl From<u32> for Pixels { fn from(v: u32) -> Self { Pixels(v as f32) } }
impl From<Pixels> for usize { fn from(v: Pixels) -> Self { v.0 as usize } }
impl From<usize> for Pixels { fn from(v: usize) -> Self { Pixels(v as f32) } }

/// Physical display pixels (`i32`).
#[derive(Clone, Copy, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub(crate) struct DevicePixels(pub(crate) i32);

impl Add for DevicePixels { type Output = Self; fn add(self, rhs: Self) -> Self { Self(self.0 + rhs.0) } }
impl AddAssign for DevicePixels { fn add_assign(&mut self, rhs: Self) { self.0 += rhs.0; } }
impl Sub for DevicePixels { type Output = Self; fn sub(self, rhs: Self) -> Self { Self(self.0 - rhs.0) } }
impl SubAssign for DevicePixels { fn sub_assign(&mut self, rhs: Self) { self.0 -= rhs.0; } }
impl Div<i32> for DevicePixels { type Output = Self; fn div(self, rhs: i32) -> Self { Self(self.0 / rhs) } }

impl DevicePixels {
    pub(crate) fn to_bytes(self, bytes_per_pixel: u8) -> u32 {
        self.0 as u32 * bytes_per_pixel as u32
    }
}

impl fmt::Debug for DevicePixels {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { write!(f, "{} px (device)", self.0) }
}

impl From<DevicePixels> for i32 { fn from(v: DevicePixels) -> Self { v.0 } }
impl From<i32> for DevicePixels { fn from(v: i32) -> Self { DevicePixels(v) } }
impl From<u32> for DevicePixels { fn from(v: u32) -> Self { DevicePixels(v as i32) } }
impl From<DevicePixels> for u32 { fn from(v: DevicePixels) -> Self { v.0 as u32 } }
impl From<DevicePixels> for u64 { fn from(v: DevicePixels) -> Self { v.0 as u64 } }
impl From<u64> for DevicePixels { fn from(v: u64) -> Self { DevicePixels(v as i32) } }
impl From<DevicePixels> for usize { fn from(v: DevicePixels) -> Self { v.0 as usize } }
impl From<usize> for DevicePixels { fn from(v: usize) -> Self { DevicePixels(v as i32) } }

/// Scaled (physical-unit `f32`) pixels.
#[derive(Clone, Copy, Default, PartialEq)]
#[repr(transparent)]
pub(crate) struct ScaledPixels(pub(crate) f32);

impl Add for ScaledPixels { type Output = Self; fn add(self, rhs: Self) -> Self { Self(self.0 + rhs.0) } }
impl AddAssign for ScaledPixels { fn add_assign(&mut self, rhs: Self) { self.0 += rhs.0; } }
impl Sub for ScaledPixels { type Output = Self; fn sub(self, rhs: Self) -> Self { Self(self.0 - rhs.0) } }
impl SubAssign for ScaledPixels { fn sub_assign(&mut self, rhs: Self) { self.0 -= rhs.0; } }
impl Div<f32> for ScaledPixels { type Output = Self; fn div(self, rhs: f32) -> Self { Self(self.0 / rhs) } }
impl DivAssign<f32> for ScaledPixels { fn div_assign(&mut self, rhs: f32) { self.0 /= rhs; } }

impl ScaledPixels {
    pub(crate) fn as_f32(self) -> f32 { self.0 }
}

impl Eq for ScaledPixels {}
impl PartialOrd for ScaledPixels {
    fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> { Some(self.cmp(other)) }
}
impl Ord for ScaledPixels {
    fn cmp(&self, other: &Self) -> cmp::Ordering { self.0.total_cmp(&other.0) }
}
impl Debug for ScaledPixels {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { write!(f, "{}px (scaled)", self.0) }
}

impl From<ScaledPixels> for DevicePixels { fn from(v: ScaledPixels) -> Self { DevicePixels(v.0.ceil() as i32) } }
impl From<DevicePixels> for ScaledPixels { fn from(v: DevicePixels) -> Self { ScaledPixels(v.0 as f32) } }
impl From<ScaledPixels> for f64 { fn from(v: ScaledPixels) -> Self { v.0 as f64 } }
impl From<ScaledPixels> for u32 { fn from(v: ScaledPixels) -> Self { v.0 as u32 } }
impl From<f32> for ScaledPixels { fn from(v: f32) -> Self { Self(v) } }

impl Div for ScaledPixels { type Output = f32; fn div(self, rhs: Self) -> Self::Output { self.0 / rhs.0 } }
impl DivAssign for ScaledPixels { fn div_assign(&mut self, rhs: Self) { *self = Self(self.0 / rhs.0); } }
impl std::ops::Rem for ScaledPixels { type Output = Self; fn rem(self, rhs: Self) -> Self { Self(self.0 % rhs.0) } }
impl std::ops::RemAssign for ScaledPixels { fn rem_assign(&mut self, rhs: Self) { self.0 %= rhs.0; } }
impl Mul<f32> for ScaledPixels { type Output = Self; fn mul(self, rhs: f32) -> Self { Self(self.0 * rhs) } }
impl Mul<ScaledPixels> for f32 { type Output = ScaledPixels; fn mul(self, rhs: ScaledPixels) -> Self::Output { rhs * self } }
impl Mul<usize> for ScaledPixels { type Output = Self; fn mul(self, rhs: usize) -> Self { self * (rhs as f32) } }
impl Mul<ScaledPixels> for usize { type Output = ScaledPixels; fn mul(self, rhs: ScaledPixels) -> ScaledPixels { rhs * self } }
impl MulAssign<f32> for ScaledPixels { fn mul_assign(&mut self, rhs: f32) { self.0 *= rhs; } }

/// Non-auto length: fixed logical pixels or fraction of a base dimension.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum DefiniteLength {
    Absolute(Pixels),
    Fraction(f32),
}

impl Neg for DefiniteLength {
    type Output = Self;
    fn neg(self) -> Self {
        match self {
            DefiniteLength::Absolute(px) => DefiniteLength::Absolute(-px),
            DefiniteLength::Fraction(f) => DefiniteLength::Fraction(-f),
        }
    }
}

impl DefiniteLength {
    pub(crate) fn to_pixels(self, base_size: Pixels) -> Pixels {
        match self {
            DefiniteLength::Absolute(size) => size,
            DefiniteLength::Fraction(fraction) => base_size * fraction,
        }
    }
}

impl Debug for DefiniteLength {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { Display::fmt(self, f) }
}

impl Display for DefiniteLength {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DefiniteLength::Absolute(length) => write!(f, "{length}"),
            DefiniteLength::Fraction(fraction) => write!(f, "{}%", (fraction * 100.0) as i32),
        }
    }
}

impl From<Pixels> for DefiniteLength {
    fn from(pixels: Pixels) -> Self { Self::Absolute(pixels) }
}

impl Default for DefiniteLength {
    fn default() -> Self { Self::Absolute(Pixels::ZERO) }
}

/// Layout length: definite or `Auto`.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Length {
    Definite(DefiniteLength),
    Auto,
}

impl Debug for Length {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { Display::fmt(self, f) }
}

impl Display for Length {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Length::Definite(definite_length) => write!(f, "{definite_length}"),
            Length::Auto => write!(f, "auto"),
        }
    }
}

pub(crate) const fn relative(fraction: f32) -> DefiniteLength {
    DefiniteLength::Fraction(fraction)
}

#[allow(clippy::approx_constant)]
pub(crate) const fn phi() -> DefiniteLength {
    relative(1.618_034)
}

pub(crate) const fn px(pixels: f32) -> Pixels {
    Pixels(pixels)
}

impl From<Pixels> for Length {
    fn from(pixels: Pixels) -> Self { Self::Definite(pixels.into()) }
}
impl From<DefiniteLength> for Length {
    fn from(length: DefiniteLength) -> Self { Self::Definite(length) }
}
impl Default for Length {
    fn default() -> Self { Self::Definite(DefiniteLength::default()) }
}

/// Halving trait for coordinate types.
pub(crate) trait Half {
    fn half(&self) -> Self;
}

impl Half for i32 { fn half(&self) -> Self { self / 2 } }
impl Half for f32 { fn half(&self) -> Self { self / 2. } }
impl Half for DevicePixels { fn half(&self) -> Self { Self(self.0 / 2) } }
impl Half for ScaledPixels { fn half(&self) -> Self { Self(self.0 / 2.) } }
impl Half for Pixels { fn half(&self) -> Self { Self(self.0 / 2.) } }

/// Zero check for coordinate and geometry types.
pub(crate) trait IsZero {
    fn is_zero(&self) -> bool;
}

impl IsZero for DevicePixels { fn is_zero(&self) -> bool { self.0 == 0 } }
impl IsZero for ScaledPixels { fn is_zero(&self) -> bool { self.0 == 0. } }
impl IsZero for Pixels { fn is_zero(&self) -> bool { self.0 == 0. } }
impl IsZero for DefiniteLength {
    fn is_zero(&self) -> bool {
        match self {
            DefiniteLength::Absolute(length) => length.is_zero(),
            DefiniteLength::Fraction(fraction) => *fraction == 0.,
        }
    }
}
impl IsZero for Length {
    fn is_zero(&self) -> bool {
        match self {
            Length::Definite(length) => length.is_zero(),
            Length::Auto => false,
        }
    }
}
impl<T: IsZero + Clone + Debug + Default + PartialEq> IsZero for Point<T> {
    fn is_zero(&self) -> bool { self.x.is_zero() && self.y.is_zero() }
}
impl<T: IsZero + Clone + Debug + Default + PartialEq> IsZero for Size<T> {
    fn is_zero(&self) -> bool { self.width.is_zero() || self.height.is_zero() }
}
impl<T: IsZero + Clone + Debug + Default + PartialEq> IsZero for Bounds<T> {
    fn is_zero(&self) -> bool { self.size.is_zero() }
}
impl<T: IsZero + Clone + Debug + Default + PartialEq> IsZero for Corners<T> {
    fn is_zero(&self) -> bool {
        self.top_left.is_zero()
            && self.top_right.is_zero()
            && self.bottom_right.is_zero()
            && self.bottom_left.is_zero()
    }
}
