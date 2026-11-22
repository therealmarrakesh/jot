//! The GPUI geometry module is a collection of types and traits that
//! can be used to describe common units, concepts, and the relationships
//! between them.

use crate::ui::anyhow::{Context as _, anyhow};
use core::fmt::Debug;
use crate::ui::refineable::{IsEmpty, Refineable};
use std::ops::{AddAssign, DivAssign, SubAssign};
use std::{
    cmp::{self, PartialOrd},
    fmt::{self, Display},
    hash::Hash,
    ops::{Add, Div, Mul, MulAssign, Neg, Sub},
};

use crate::ui::{App, DisplayId};

/// Axis in a 2D cartesian space.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum Axis {
    /// The y axis, or up and down
    Vertical,
    /// The x axis, or left and right
    Horizontal,
}

/// Describes a location in a 2D cartesian space.
///
/// It holds two public fields, `x` and `y`, which represent the coordinates in the space.
/// The type `T` for the coordinates can be any type that implements `Default`, `Clone`, and `Debug`.
///
/// # Examples
///
/// ```
/// # use crate::ui::Point;
/// let point = Point { x: 10, y: 20 };
/// println!("{:?}", point); // Outputs: Point { x: 10, y: 20 }
/// ```
#[derive(Default, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(C)]
pub(crate) struct Point<T: Clone + Debug + Default + PartialEq> {
    /// The x coordinate of the point.
    pub(crate) x: T,
    /// The y coordinate of the point.
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

/// Constructs a new `Point<T>` with the given x and y coordinates.
///
/// # Arguments
///
/// * `x` - The x coordinate of the point.
/// * `y` - The y coordinate of the point.
///
/// # Returns
///
/// Returns a `Point<T>` with the specified coordinates.
///
/// # Examples
///
/// ```
/// use crate::ui::point;
/// let p = point(10, 20);
/// assert_eq!(p.x, 10);
/// assert_eq!(p.y, 20);
/// ```
pub(crate) const fn point<T: Clone + Debug + Default + PartialEq>(x: T, y: T) -> Point<T> {
    Point { x, y }
}

impl<T: Clone + Debug + Default + PartialEq> Point<T> {
    /// Creates a new `Point` with the specified `x` and `y` coordinates.
    ///
    /// # Arguments
    ///
    /// * `x` - The horizontal coordinate of the point.
    /// * `y` - The vertical coordinate of the point.
    ///
    /// # Examples
    ///
    /// ```
    /// use crate::ui::Point;
    /// let p = Point::new(10, 20);
    /// assert_eq!(p.x, 10);
    /// assert_eq!(p.y, 20);
    /// ```
    pub(crate) const fn new(x: T, y: T) -> Self {
        Self { x, y }
    }

    /// Transforms the point to a `Point<U>` by applying the given function to both coordinates.
    ///
    /// This method allows for converting a `Point<T>` to a `Point<U>` by specifying a closure
    /// that defines how to convert between the two types. The closure is applied to both the `x`
    /// and `y` coordinates, resulting in a new point of the desired type.
    ///
    /// # Arguments
    ///
    /// * `f` - A closure that takes a value of type `T` and returns a value of type `U`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use crate::ui::Point;
    /// let p = Point { x: 3, y: 4 };
    /// let p_float = p.map(|coord| coord as f32);
    /// assert_eq!(p_float, Point { x: 3.0, y: 4.0 });
    /// ```
    #[must_use]
    pub(crate) fn map<U: Clone + Debug + Default + PartialEq>(&self, f: impl Fn(T) -> U) -> Point<U> {
        Point {
            x: f(self.x.clone()),
            y: f(self.y.clone()),
        }
    }
}

impl Point<Pixels> {
    /// Scales the point by a given factor, which is typically derived from the resolution
    /// of a target display to ensure proper sizing of UI elements.
    ///
    /// # Arguments
    ///
    /// * `factor` - The scaling factor to apply to both the x and y coordinates.
    ///
    /// # Examples
    ///
    /// ```
    /// # use crate::ui::{Point, Pixels, ScaledPixels};
    /// let p = Point { x: Pixels::from(10.0), y: Pixels::from(20.0) };
    /// let scaled_p = p.scale(1.5);
    /// assert_eq!(scaled_p, Point { x: ScaledPixels::from(15.0), y: ScaledPixels::from(30.0) });
    /// ```
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

impl<T> Point<T>
where
    T: PartialOrd + Clone + Debug + Default + PartialEq,
{
    /// Returns a new point with the maximum values of each dimension from `self` and `other`.
    ///
    /// # Arguments
    ///
    /// * `other` - A reference to another `Point` to compare with `self`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use crate::ui::Point;
    /// let p1 = Point { x: 3, y: 7 };
    /// let p2 = Point { x: 5, y: 2 };
    /// let max_point = p1.max(&p2);
    /// assert_eq!(max_point, Point { x: 5, y: 7 });
    /// ```
    pub(crate) fn max(&self, other: &Self) -> Self {
        Point {
            x: if self.x > other.x {
                self.x.clone()
            } else {
                other.x.clone()
            },
            y: if self.y > other.y {
                self.y.clone()
            } else {
                other.y.clone()
            },
        }
    }

    /// Returns a new point with the minimum values of each dimension from `self` and `other`.
    ///
    /// # Arguments
    ///
    /// * `other` - A reference to another `Point` to compare with `self`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use crate::ui::Point;
    /// let p1 = Point { x: 3, y: 7 };
    /// let p2 = Point { x: 5, y: 2 };
    /// let min_point = p1.min(&p2);
    /// assert_eq!(min_point, Point { x: 3, y: 2 });
    /// ```
    pub(crate) fn min(&self, other: &Self) -> Self {
        Point {
            x: if self.x <= other.x {
                self.x.clone()
            } else {
                other.x.clone()
            },
            y: if self.y <= other.y {
                self.y.clone()
            } else {
                other.y.clone()
            },
        }
    }
}

impl<T: Clone + Debug + Default + PartialEq> Clone for Point<T> {
    fn clone(&self) -> Self {
        Self {
            x: self.x.clone(),
            y: self.y.clone(),
        }
    }
}

impl<T: Clone + Debug + Default + PartialEq + Display> Display for Point<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({}, {})", self.x, self.y)
    }
}

/// A structure representing a two-dimensional size with width and height in a given unit.
///
/// This struct is generic over the type `T`, which can be any type that implements `Clone`, `Default`, and `Debug`.
/// It is commonly used to specify dimensions for elements in a UI, such as a window or element.
#[derive(Clone, Copy, Default, Hash, PartialEq)]
#[repr(C)]
pub(crate) struct Size<T: Clone + Debug + Default + PartialEq> {
    /// The width component of the size.
    pub(crate) width: T,
    /// The height component of the size.
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

/// Constructs a new `Size<T>` with the provided width and height.
///
/// # Arguments
///
/// * `width` - The width component of the `Size`.
/// * `height` - The height component of the `Size`.
///
/// # Examples
///
/// ```
/// use crate::ui::size;
/// let my_size = size(10, 20);
/// assert_eq!(my_size.width, 10);
/// assert_eq!(my_size.height, 20);
/// ```
pub(crate) const fn size<T>(width: T, height: T) -> Size<T>
where
    T: Clone + Debug + Default + PartialEq,
{
    Size { width, height }
}

impl<T> Size<T>
where
    T: Clone + Debug + Default + PartialEq,
{
    /// Applies a function to the width and height of the size, producing a new `Size<U>`.
    ///
    /// This method allows for converting a `Size<T>` to a `Size<U>` by specifying a closure
    /// that defines how to convert between the two types. The closure is applied to both the `width`
    /// and `height`, resulting in a new size of the desired type.
    ///
    /// # Arguments
    ///
    /// * `f` - A closure that takes a value of type `T` and returns a value of type `U`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use crate::ui::Size;
    /// let my_size = Size { width: 10, height: 20 };
    /// let my_new_size = my_size.map(|dimension| dimension as f32 * 1.5);
    /// assert_eq!(my_new_size, Size { width: 15.0, height: 30.0 });
    /// ```
    pub(crate) fn map<U>(&self, f: impl Fn(T) -> U) -> Size<U>
    where
        U: Clone + Debug + Default + PartialEq,
    {
        Size {
            width: f(self.width.clone()),
            height: f(self.height.clone()),
        }
    }
}

impl<T> Size<T>
where
    T: PartialOrd + Clone + Debug + Default + PartialEq,
{
    /// Returns a new `Size` with the maximum width and height from `self` and `other`.
    ///
    /// # Arguments
    ///
    /// * `other` - A reference to another `Size` to compare with `self`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use crate::ui::Size;
    /// let size1 = Size { width: 30, height: 40 };
    /// let size2 = Size { width: 50, height: 20 };
    /// let max_size = size1.max(&size2);
    /// assert_eq!(max_size, Size { width: 50, height: 40 });
    /// ```
    pub(crate) fn max(&self, other: &Self) -> Self {
        Size {
            width: if self.width >= other.width {
                self.width.clone()
            } else {
                other.width.clone()
            },
            height: if self.height >= other.height {
                self.height.clone()
            } else {
                other.height.clone()
            },
        }
    }

    /// Returns a new `Size` with the minimum width and height from `self` and `other`.
    ///
    /// # Arguments
    ///
    /// * `other` - A reference to another `Size` to compare with `self`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use crate::ui::Size;
    /// let size1 = Size { width: 30, height: 40 };
    /// let size2 = Size { width: 50, height: 20 };
    /// let min_size = size1.min(&size2);
    /// assert_eq!(min_size, Size { width: 30, height: 20 });
    /// ```
    pub(crate) fn min(&self, other: &Self) -> Self {
        Size {
            width: if self.width >= other.width {
                other.width.clone()
            } else {
                self.width.clone()
            },
            height: if self.height >= other.height {
                other.height.clone()
            } else {
                self.height.clone()
            },
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
        Size {
            width: self.width * rhs.clone(),
            height: self.height * rhs,
        }
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

impl<T> Eq for Size<T> where T: Eq + Clone + Debug + Default + PartialEq {}

impl<T> Debug for Size<T>
where
    T: Clone + Debug + Default + PartialEq,
{
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
        Self {
            width: point.x,
            height: point.y,
        }
    }
}

impl From<Size<Pixels>> for Size<DefiniteLength> {
    fn from(size: Size<Pixels>) -> Self {
        Size {
            width: size.width.into(),
            height: size.height.into(),
        }
    }
}

impl From<Size<Pixels>> for Size<AbsoluteLength> {
    fn from(size: Size<Pixels>) -> Self {
        Size {
            width: size.width.into(),
            height: size.height.into(),
        }
    }
}

impl Size<Length> {
    /// Returns a `Size` with both width and height set to `auto`, which allows the layout engine to determine the size.
    ///
    /// This function creates a `Size` instance where both the width and height are set to `Length::Auto`,
    /// indicating that their size should be computed based on the layout context, such as the content size or
    /// available space.
    ///
    /// # Returns
    ///
    /// A `Size<Length>` with width and height set to `Length::Auto`.
    pub(crate) fn auto() -> Self {
        Self {
            width: Length::Auto,
            height: Length::Auto,
        }
    }
}

/// Represents a rectangular area in a 2D space with an origin point and a size.
///
/// The `Bounds` struct is generic over a type `T` which represents the type of the coordinate system.
/// The origin is represented as a `Point<T>` which defines the top left corner of the rectangle,
/// and the size is represented as a `Size<T>` which defines the width and height of the rectangle.
///
/// # Examples
///
/// ```
/// # use crate::ui::{Bounds, Point, Size};
/// let origin = Point { x: 0, y: 0 };
/// let size = Size { width: 10, height: 20 };
/// let bounds = Bounds::new(origin, size);
///
/// assert_eq!(bounds.origin, origin);
/// assert_eq!(bounds.size, size);
/// ```
#[derive(Copy, Clone, Default, Debug, Eq, PartialEq, Hash)]
#[repr(C)]
pub(crate) struct Bounds<T: Clone + Debug + Default + PartialEq> {
    /// The origin point of this area.
    pub(crate) origin: Point<T>,
    /// The size of the rectangle.
    pub(crate) size: Size<T>,
}

#[derive(Clone)]
pub(crate) struct BoundsRefinement<T: Clone + Debug + Default + PartialEq> {
    pub(crate) origin: Option<Point<T>>,
    pub(crate) size: Option<Size<T>>,
}

impl<T: Clone + Debug + Default + PartialEq> Default for BoundsRefinement<T> {
    fn default() -> Self {
        Self { origin: None, size: None }
    }
}

impl<T: Clone + Debug + Default + PartialEq> fmt::Debug for BoundsRefinement<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut s = f.debug_struct("BoundsRefinement");
        let mut all = true;
        if self.origin.is_some() { s.field("origin", &self.origin); } else { all = false; }
        if self.size.is_some() { s.field("size", &self.size); } else { all = false; }
        if all { s.finish() } else { s.finish_non_exhaustive() }
    }
}

impl<T: Clone + Debug + Default + PartialEq> IsEmpty for BoundsRefinement<T> {
    fn is_empty(&self) -> bool {
        self.origin.is_none() && self.size.is_none()
    }
}

impl<T: Clone + Debug + Default + PartialEq> From<BoundsRefinement<T>> for Bounds<T> {
    fn from(v: BoundsRefinement<T>) -> Self {
        Self {
            origin: v.origin.unwrap_or_default(),
            size: v.size.unwrap_or_default(),
        }
    }
}

impl<T: Clone + Debug + Default + PartialEq> Refineable for Bounds<T> {
    type Refinement = BoundsRefinement<T>;
    fn refine(&mut self, r: &Self::Refinement) {
        if let Some(v) = &r.origin { self.origin = v.clone(); }
        if let Some(v) = &r.size { self.size = v.clone(); }
    }
}

impl<T: Clone + Debug + Default + PartialEq> Refineable for BoundsRefinement<T> {
    type Refinement = BoundsRefinement<T>;
    fn refine(&mut self, r: &Self::Refinement) {
        if let Some(v) = &r.origin { self.origin = Some(v.clone()); }
        if let Some(v) = &r.size { self.size = Some(v.clone()); }
    }
}

/// Create a bounds with the given origin and size
pub(crate) fn bounds<T: Clone + Debug + Default + PartialEq>(
    origin: Point<T>,
    size: Size<T>,
) -> Bounds<T> {
    Bounds { origin, size }
}

impl Bounds<Pixels> {
    /// Generate a centered bounds for the given display or primary display if none is provided
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

impl<T> Bounds<T>
where
    T: Clone + Debug + Default + PartialEq,
{
    /// Creates a new `Bounds` with the specified origin and size.
    ///
    /// # Arguments
    ///
    /// * `origin` - A `Point<T>` representing the origin of the bounds.
    /// * `size` - A `Size<T>` representing the size of the bounds.
    ///
    /// # Returns
    ///
    /// Returns a `Bounds<T>` that has the given origin and size.
    pub(crate) fn new(origin: Point<T>, size: Size<T>) -> Self {
        Bounds { origin, size }
    }
}

impl<T> Bounds<T>
where
    T: Sub<Output = T> + Clone + Debug + Default + PartialEq,
{
    /// Constructs a `Bounds` from two corner points: the top left and bottom right corners.
    ///
    /// This function calculates the origin and size of the `Bounds` based on the provided corner points.
    /// The origin is set to the top left corner, and the size is determined by the difference between
    /// the x and y coordinates of the bottom right and top left points.
    ///
    /// # Arguments
    ///
    /// * `top_left` - A `Point<T>` representing the top left corner of the rectangle.
    /// * `bottom_right` - A `Point<T>` representing the bottom right corner of the rectangle.
    ///
    /// # Returns
    ///
    /// Returns a `Bounds<T>` that encompasses the area defined by the two corner points.
    ///
    /// # Examples
    ///
    /// ```
    /// # use crate::ui::{Bounds, Point};
    /// let top_left = Point { x: 0, y: 0 };
    /// let bottom_right = Point { x: 10, y: 10 };
    /// let bounds = Bounds::from_corners(top_left, bottom_right);
    ///
    /// assert_eq!(bounds.origin, top_left);
    /// assert_eq!(bounds.size.width, 10);
    /// assert_eq!(bounds.size.height, 10);
    /// ```
    pub(crate) fn from_corners(top_left: Point<T>, bottom_right: Point<T>) -> Self {
        let origin = Point {
            x: top_left.x.clone(),
            y: top_left.y.clone(),
        };
        let size = Size {
            width: bottom_right.x - top_left.x,
            height: bottom_right.y - top_left.y,
        };
        Bounds { origin, size }
    }
}

impl<T> Bounds<T>
where
    T: Sub<Output = T> + Half + Clone + Debug + Default + PartialEq,
{
    /// Constructs a `Bounds` from a corner point and size. The specified corner will be placed at
    /// the specified origin.
    pub(crate) fn from_anchor_and_size(corner: Anchor, origin: Point<T>, size: Size<T>) -> Bounds<T> {
        let origin = match corner {
            Anchor::TopLeft => origin,
            Anchor::TopRight => Point {
                x: origin.x - size.width.clone(),
                y: origin.y,
            },
            Anchor::BottomLeft => Point {
                x: origin.x,
                y: origin.y - size.height.clone(),
            },
            Anchor::BottomRight => Point {
                x: origin.x - size.width.clone(),
                y: origin.y - size.height.clone(),
            },
            Anchor::TopCenter => Point {
                x: origin.x - size.width.half(),
                y: origin.y,
            },
            Anchor::BottomCenter => Point {
                x: origin.x - size.width.half(),
                y: origin.y - size.height.clone(),
            },
            Anchor::LeftCenter => Point {
                x: origin.x,
                y: origin.y - size.height.half(),
            },
            Anchor::RightCenter => Point {
                x: origin.x - size.width.clone(),
                y: origin.y - size.height.half(),
            },
        };

        Bounds { origin, size }
    }
}

impl<T> Bounds<T>
where
    T: Sub<T, Output = T> + Half + Clone + Debug + Default + PartialEq,
{
    /// Creates a new bounds centered at the given point.
    pub(crate) fn centered_at(center: Point<T>, size: Size<T>) -> Self {
        let origin = Point {
            x: center.x - size.width.half(),
            y: center.y - size.height.half(),
        };
        Self::new(origin, size)
    }
}

impl<T> Bounds<T>
where
    T: PartialOrd + Add<T, Output = T> + Clone + Debug + Default + PartialEq,
{
    /// Checks if this `Bounds` intersects with another `Bounds`.
    ///
    /// Two `Bounds` instances intersect if they overlap in the 2D space they occupy.
    /// This method checks if there is any overlapping area between the two bounds.
    ///
    /// # Arguments
    ///
    /// * `other` - A reference to another `Bounds` to check for intersection with.
    ///
    /// # Returns
    ///
    /// Returns `true` if there is any intersection between the two bounds, `false` otherwise.
    ///
    /// # Examples
    ///
    /// ```
    /// # use crate::ui::{Bounds, Point, Size};
    /// let bounds1 = Bounds {
    ///     origin: Point { x: 0, y: 0 },
    ///     size: Size { width: 10, height: 10 },
    /// };
    /// let bounds2 = Bounds {
    ///     origin: Point { x: 5, y: 5 },
    ///     size: Size { width: 10, height: 10 },
    /// };
    /// let bounds3 = Bounds {
    ///     origin: Point { x: 20, y: 20 },
    ///     size: Size { width: 10, height: 10 },
    /// };
    ///
    /// assert_eq!(bounds1.intersects(&bounds2), true); // Overlapping bounds
    /// assert_eq!(bounds1.intersects(&bounds3), false); // Non-overlapping bounds
    /// ```
    pub(crate) fn intersects(&self, other: &Bounds<T>) -> bool {
        let my_lower_right = self.bottom_right();
        let their_lower_right = other.bottom_right();

        self.origin.x < their_lower_right.x
            && my_lower_right.x > other.origin.x
            && self.origin.y < their_lower_right.y
            && my_lower_right.y > other.origin.y
    }
}

impl<T> Bounds<T>
where
    T: Add<T, Output = T> + Half + Clone + Debug + Default + PartialEq,
{
    /// Returns the center point of the bounds.
    ///
    /// Calculates the center by taking the origin's x and y coordinates and adding half the width and height
    /// of the bounds, respectively. The center is represented as a `Point<T>` where `T` is the type of the
    /// coordinate system.
    ///
    /// # Returns
    ///
    /// A `Point<T>` representing the center of the bounds.
    ///
    /// # Examples
    ///
    /// ```
    /// # use crate::ui::{Bounds, Point, Size};
    /// let bounds = Bounds {
    ///     origin: Point { x: 0, y: 0 },
    ///     size: Size { width: 10, height: 20 },
    /// };
    /// let center = bounds.center();
    /// assert_eq!(center, Point { x: 5, y: 10 });
    /// ```
    pub(crate) fn center(&self) -> Point<T> {
        Point {
            x: self.origin.x.clone() + self.size.width.clone().half(),
            y: self.origin.y.clone() + self.size.height.clone().half(),
        }
    }
}

impl<T> Bounds<T>
where
    T: Add<T, Output = T> + Clone + Debug + Default + PartialEq,
{
    /// Calculates the half perimeter of a rectangle defined by the bounds.
    ///
    /// The half perimeter is calculated as the sum of the width and the height of the rectangle.
    /// This method is generic over the type `T` which must implement the `Sub` trait to allow
    /// calculation of the width and height from the bounds' origin and size, as well as the `Add` trait
    /// to sum the width and height for the half perimeter.
    ///
    /// # Examples
    ///
    /// ```
    /// # use crate::ui::{Bounds, Point, Size};
    /// let bounds = Bounds {
    ///     origin: Point { x: 0, y: 0 },
    ///     size: Size { width: 10, height: 20 },
    /// };
    /// let half_perimeter = bounds.half_perimeter();
    /// assert_eq!(half_perimeter, 30);
    /// ```
    pub(crate) fn half_perimeter(&self) -> T {
        self.size.width.clone() + self.size.height.clone()
    }
}

impl<T> Bounds<T>
where
    T: Add<T, Output = T> + Sub<Output = T> + Clone + Debug + Default + PartialEq,
{
    /// Dilates the bounds by a specified amount in all directions.
    ///
    /// This method expands the bounds by the given `amount`, increasing the size
    /// and adjusting the origin so that the bounds grow outwards equally in all directions.
    /// The resulting bounds will have its width and height increased by twice the `amount`
    /// (since it grows in both directions), and the origin will be moved by `-amount`
    /// in both the x and y directions.
    ///
    /// # Arguments
    ///
    /// * `amount` - The amount by which to dilate the bounds.
    ///
    /// # Examples
    ///
    /// ```
    /// # use crate::ui::{Bounds, Point, Size};
    /// let mut bounds = Bounds {
    ///     origin: Point { x: 10, y: 10 },
    ///     size: Size { width: 10, height: 10 },
    /// };
    /// let expanded_bounds = bounds.dilate(5);
    /// assert_eq!(expanded_bounds, Bounds {
    ///     origin: Point { x: 5, y: 5 },
    ///     size: Size { width: 20, height: 20 },
    /// });
    /// ```
    #[must_use]
    pub(crate) fn dilate(&self, amount: T) -> Bounds<T> {
        let double_amount = amount.clone() + amount.clone();
        Bounds {
            origin: self.origin.clone() - point(amount.clone(), amount),
            size: self.size.clone() + size(double_amount.clone(), double_amount),
        }
    }
}

impl<T: PartialOrd + Add<T, Output = T> + Sub<Output = T> + Clone + Debug + Default + PartialEq>
    Bounds<T>
{
    /// Calculates the intersection of two `Bounds` objects.
    ///
    /// This method computes the overlapping region of two `Bounds`. If the bounds do not intersect,
    /// the resulting `Bounds` will have a size with width and height of zero.
    ///
    /// # Arguments
    ///
    /// * `other` - A reference to another `Bounds` to intersect with.
    ///
    /// # Returns
    ///
    /// Returns a `Bounds` representing the intersection area. If there is no intersection,
    /// the returned `Bounds` will have a size with width and height of zero.
    ///
    /// # Examples
    ///
    /// ```
    /// # use crate::ui::{Bounds, Point, Size};
    /// let bounds1 = Bounds {
    ///     origin: Point { x: 0, y: 0 },
    ///     size: Size { width: 10, height: 10 },
    /// };
    /// let bounds2 = Bounds {
    ///     origin: Point { x: 5, y: 5 },
    ///     size: Size { width: 10, height: 10 },
    /// };
    /// let intersection = bounds1.intersect(&bounds2);
    ///
    /// assert_eq!(intersection, Bounds {
    ///     origin: Point { x: 5, y: 5 },
    ///     size: Size { width: 5, height: 5 },
    /// });
    /// ```
    pub(crate) fn intersect(&self, other: &Self) -> Self {
        let upper_left = self.origin.max(&other.origin);
        let bottom_right = self
            .bottom_right()
            .min(&other.bottom_right())
            .max(&upper_left);
        Self::from_corners(upper_left, bottom_right)
    }

    /// Computes the union of two `Bounds`.
    ///
    /// This method calculates the smallest `Bounds` that contains both the current `Bounds` and the `other` `Bounds`.
    /// The resulting `Bounds` will have an origin that is the minimum of the origins of the two `Bounds`,
    /// and a size that encompasses the furthest extents of both `Bounds`.
    ///
    /// # Arguments
    ///
    /// * `other` - A reference to another `Bounds` to create a union with.
    ///
    /// # Returns
    ///
    /// Returns a `Bounds` representing the union of the two `Bounds`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use crate::ui::{Bounds, Point, Size};
    /// let bounds1 = Bounds {
    ///     origin: Point { x: 0, y: 0 },
    ///     size: Size { width: 10, height: 10 },
    /// };
    /// let bounds2 = Bounds {
    ///     origin: Point { x: 5, y: 5 },
    ///     size: Size { width: 15, height: 15 },
    /// };
    /// let union_bounds = bounds1.union(&bounds2);
    ///
    /// assert_eq!(union_bounds, Bounds {
    ///     origin: Point { x: 0, y: 0 },
    ///     size: Size { width: 20, height: 20 },
    /// });
    /// ```
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
        Bounds {
            origin: self.origin * rhs.clone(),
            size: self.size * rhs,
        }
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
        Self {
            origin: self.origin / rhs.clone(),
            size: self.size / rhs,
        }
    }
}

impl<T> Add<Point<T>> for Bounds<T>
where
    T: Add<T, Output = T> + Clone + Debug + Default + PartialEq,
{
    type Output = Self;

    fn add(self, rhs: Point<T>) -> Self {
        Self {
            origin: self.origin + rhs,
            size: self.size,
        }
    }
}

impl<T> Sub<Point<T>> for Bounds<T>
where
    T: Sub<T, Output = T> + Clone + Debug + Default + PartialEq,
{
    type Output = Self;

    fn sub(self, rhs: Point<T>) -> Self {
        Self {
            origin: self.origin - rhs,
            size: self.size,
        }
    }
}

impl<T: Clone + Debug + Default + PartialEq> From<Size<T>> for Point<T> {
    fn from(size: Size<T>) -> Self {
        Self {
            x: size.width,
            y: size.height,
        }
    }
}

impl<T> Bounds<T>
where
    T: Add<T, Output = T> + Clone + Debug + Default + PartialEq,
{
    /// Returns the top edge of the bounds.
    ///
    /// # Returns
    ///
    /// A value of type `T` representing the y-coordinate of the top edge of the bounds.
    pub(crate) fn top(&self) -> T {
        self.origin.y.clone()
    }

    /// Returns the bottom edge of the bounds.
    ///
    /// # Returns
    ///
    /// A value of type `T` representing the y-coordinate of the bottom edge of the bounds.
    pub(crate) fn bottom(&self) -> T {
        self.origin.y.clone() + self.size.height.clone()
    }

    /// Returns the left edge of the bounds.
    ///
    /// # Returns
    ///
    /// A value of type `T` representing the x-coordinate of the left edge of the bounds.
    pub(crate) fn left(&self) -> T {
        self.origin.x.clone()
    }

    /// Returns the right edge of the bounds.
    ///
    /// # Returns
    ///
    /// A value of type `T` representing the x-coordinate of the right edge of the bounds.
    pub(crate) fn right(&self) -> T {
        self.origin.x.clone() + self.size.width.clone()
    }

    /// Returns the top right corner point of the bounds.
    ///
    /// # Returns
    ///
    /// A `Point<T>` representing the top right corner of the bounds.
    ///
    /// # Examples
    ///
    /// ```
    /// # use crate::ui::{Bounds, Point, Size};
    /// let bounds = Bounds {
    ///     origin: Point { x: 0, y: 0 },
    ///     size: Size { width: 10, height: 20 },
    /// };
    /// let top_right = bounds.top_right();
    /// assert_eq!(top_right, Point { x: 10, y: 0 });
    /// ```
    pub(crate) fn top_right(&self) -> Point<T> {
        Point {
            x: self.origin.x.clone() + self.size.width.clone(),
            y: self.origin.y.clone(),
        }
    }

    /// Returns the bottom right corner point of the bounds.
    ///
    /// # Returns
    ///
    /// A `Point<T>` representing the bottom right corner of the bounds.
    ///
    /// # Examples
    ///
    /// ```
    /// # use crate::ui::{Bounds, Point, Size};
    /// let bounds = Bounds {
    ///     origin: Point { x: 0, y: 0 },
    ///     size: Size { width: 10, height: 20 },
    /// };
    /// let bottom_right = bounds.bottom_right();
    /// assert_eq!(bottom_right, Point { x: 10, y: 20 });
    /// ```
    pub(crate) fn bottom_right(&self) -> Point<T> {
        Point {
            x: self.origin.x.clone() + self.size.width.clone(),
            y: self.origin.y.clone() + self.size.height.clone(),
        }
    }

    /// Returns the bottom left corner point of the bounds.
    ///
    /// # Returns
    ///
    /// A `Point<T>` representing the bottom left corner of the bounds.
    ///
    /// # Examples
    ///
    /// ```
    /// # use crate::ui::{Bounds, Point, Size};
    /// let bounds = Bounds {
    ///     origin: Point { x: 0, y: 0 },
    ///     size: Size { width: 10, height: 20 },
    /// };
    /// let bottom_left = bounds.bottom_left();
    /// assert_eq!(bottom_left, Point { x: 0, y: 20 });
    /// ```
    pub(crate) fn bottom_left(&self) -> Point<T> {
        Point {
            x: self.origin.x.clone(),
            y: self.origin.y.clone() + self.size.height.clone(),
        }
    }
}

impl<T> Bounds<T>
where
    T: Add<T, Output = T> + PartialOrd + Clone + Debug + Default + PartialEq,
{
    /// Checks if the given point is within the bounds.
    ///
    /// This method determines whether a point lies inside the rectangle defined by the bounds,
    /// including the edges. The point is considered inside if its x-coordinate is greater than
    /// or equal to the left edge and less than or equal to the right edge, and its y-coordinate
    /// is greater than or equal to the top edge and less than or equal to the bottom edge of the bounds.
    ///
    /// # Arguments
    ///
    /// * `point` - A reference to a `Point<T>` that represents the point to check.
    ///
    /// # Returns
    ///
    /// Returns `true` if the point is within the bounds, `false` otherwise.
    ///
    /// # Examples
    ///
    /// ```
    /// # use crate::ui::{Point, Bounds, Size};
    /// let bounds = Bounds {
    ///     origin: Point { x: 0, y: 0 },
    ///     size: Size { width: 10, height: 10 },
    /// };
    /// let inside_point = Point { x: 5, y: 5 };
    /// let outside_point = Point { x: 15, y: 15 };
    ///
    /// assert!(bounds.contains(&inside_point));
    /// assert!(!bounds.contains(&outside_point));
    /// ```
    pub(crate) fn contains(&self, point: &Point<T>) -> bool {
        point.x >= self.origin.x
            && point.x < self.origin.x.clone() + self.size.width.clone()
            && point.y >= self.origin.y
            && point.y < self.origin.y.clone() + self.size.height.clone()
    }

    /// Applies a function to the origin and size of the bounds, producing a new `Bounds<U>`.
    ///
    /// This method allows for converting a `Bounds<T>` to a `Bounds<U>` by specifying a closure
    /// that defines how to convert between the two types. The closure is applied to the `origin` and
    /// `size` fields, resulting in new bounds of the desired type.
    ///
    /// # Arguments
    ///
    /// * `f` - A closure that takes a value of type `T` and returns a value of type `U`.
    ///
    /// # Returns
    ///
    /// Returns a new `Bounds<U>` with the origin and size mapped by the provided function.
    ///
    /// # Examples
    ///
    /// ```
    /// # use crate::ui::{Bounds, Point, Size};
    /// let bounds = Bounds {
    ///     origin: Point { x: 10.0, y: 10.0 },
    ///     size: Size { width: 10.0, height: 20.0 },
    /// };
    /// let new_bounds = bounds.map(|value| value as f64 * 1.5);
    ///
    /// assert_eq!(new_bounds, Bounds {
    ///     origin: Point { x: 15.0, y: 15.0 },
    ///     size: Size { width: 15.0, height: 30.0 },
    /// });
    /// ```
    pub(crate) fn map<U>(&self, f: impl Fn(T) -> U) -> Bounds<U>
    where
        U: Clone + Debug + Default + PartialEq,
    {
        Bounds {
            origin: self.origin.map(&f),
            size: self.size.map(f),
        }
    }
}

/// Checks if the bounds represent an empty area.
///
/// # Returns
///
/// Returns `true` if either the width or the height of the bounds is less than or equal to zero, indicating an empty area.
impl<T: PartialOrd + Clone + Debug + Default + PartialEq> Bounds<T> {
    /// Checks if the bounds represent an empty area.
    ///
    /// # Returns
    ///
    /// Returns `true` if either the width or the height of the bounds is less than or equal to zero, indicating an empty area.
    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.size.width <= T::default() || self.size.height <= T::default()
    }
}

impl<T: Clone + Debug + Default + PartialEq + Display + Add<T, Output = T>> Display for Bounds<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} - {} (size {})",
            self.origin,
            self.bottom_right(),
            self.size
        )
    }
}

impl Size<DevicePixels> {
    /// Converts the size from physical to logical pixels.
    pub(crate) fn to_pixels(self, scale_factor: f32) -> Size<Pixels> {
        size(
            px(self.width.0 as f32 / scale_factor),
            px(self.height.0 as f32 / scale_factor),
        )
    }
}

impl Size<Pixels> {
    /// Converts the size from logical to physical pixels.
    pub(crate) fn to_device_pixels(self, scale_factor: f32) -> Size<DevicePixels> {
        size(
            DevicePixels((self.width.0 * scale_factor).round() as i32),
            DevicePixels((self.height.0 * scale_factor).round() as i32),
        )
    }
}

impl Bounds<Pixels> {
    /// Convert the bounds from logical pixels to physical pixels
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

/// Represents the edges of a box in a 2D space, such as padding or margin.
///
/// Each field represents the size of the edge on one side of the box: `top`, `right`, `bottom`, and `left`.
///
/// # Examples
///
/// ```
/// # use crate::ui::Edges;
/// let edges = Edges {
///     top: 10.0,
///     right: 20.0,
///     bottom: 30.0,
///     left: 40.0,
/// };
///
/// assert_eq!(edges.top, 10.0);
/// assert_eq!(edges.right, 20.0);
/// assert_eq!(edges.bottom, 30.0);
/// assert_eq!(edges.left, 40.0);
/// ```
#[derive(Clone, Default, Debug, Eq, PartialEq)]
#[repr(C)]
pub(crate) struct Edges<T: Clone + Debug + Default + PartialEq> {
    /// The size of the top edge.
    pub(crate) top: T,
    /// The size of the right edge.
    pub(crate) right: T,
    /// The size of the bottom edge.
    pub(crate) bottom: T,
    /// The size of the left edge.
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

impl<T> Mul for Edges<T>
where
    T: Mul<Output = T> + Clone + Debug + Default + PartialEq,
{
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

impl<T, S> MulAssign<S> for Edges<T>
where
    T: Mul<S, Output = T> + Clone + Debug + Default + PartialEq,
    S: Clone,
{
    fn mul_assign(&mut self, rhs: S) {
        self.top = self.top.clone() * rhs.clone();
        self.right = self.right.clone() * rhs.clone();
        self.bottom = self.bottom.clone() * rhs.clone();
        self.left = self.left.clone() * rhs;
    }
}

impl<T: Clone + Debug + Default + PartialEq + Copy> Copy for Edges<T> {}

impl<T: Clone + Debug + Default + PartialEq> Edges<T> {
    /// Constructs `Edges` where all sides are set to the same specified value.
    ///
    /// This function creates an `Edges` instance with the `top`, `right`, `bottom`, and `left` fields all initialized
    /// to the same value provided as an argument. This is useful when you want to have uniform edges around a box,
    /// such as padding or margin with the same size on all sides.
    ///
    /// # Arguments
    ///
    /// * `value` - The value to set for all four sides of the edges.
    ///
    /// # Returns
    ///
    /// An `Edges` instance with all sides set to the given value.
    ///
    /// # Examples
    ///
    /// ```
    /// # use crate::ui::Edges;
    /// let uniform_edges = Edges::all(10.0);
    /// assert_eq!(uniform_edges.top, 10.0);
    /// assert_eq!(uniform_edges.right, 10.0);
    /// assert_eq!(uniform_edges.bottom, 10.0);
    /// assert_eq!(uniform_edges.left, 10.0);
    /// ```
    pub(crate) fn all(value: T) -> Self {
        Self {
            top: value.clone(),
            right: value.clone(),
            bottom: value.clone(),
            left: value,
        }
    }

    /// Applies a function to each field of the `Edges`, producing a new `Edges<U>`.
    ///
    /// This method allows for converting an `Edges<T>` to an `Edges<U>` by specifying a closure
    /// that defines how to convert between the two types. The closure is applied to each field
    /// (`top`, `right`, `bottom`, `left`), resulting in new edges of the desired type.
    ///
    /// # Arguments
    ///
    /// * `f` - A closure that takes a reference to a value of type `T` and returns a value of type `U`.
    ///
    /// # Returns
    ///
    /// Returns a new `Edges<U>` with each field mapped by the provided function.
    ///
    /// # Examples
    ///
    /// ```
    /// # use crate::ui::Edges;
    /// let edges = Edges { top: 10, right: 20, bottom: 30, left: 40 };
    /// let edges_float = edges.map(|&value| value as f32 * 1.1);
    /// assert_eq!(edges_float, Edges { top: 11.0, right: 22.0, bottom: 33.0, left: 44.0 });
    /// ```
    pub(crate) fn map<U>(&self, f: impl Fn(&T) -> U) -> Edges<U>
    where
        U: Clone + Debug + Default + PartialEq,
    {
        Edges {
            top: f(&self.top),
            right: f(&self.right),
            bottom: f(&self.bottom),
            left: f(&self.left),
        }
    }

    /// Checks if any of the edges satisfy a given predicate.
    ///
    /// This method applies a predicate function to each field of the `Edges` and returns `true` if any field satisfies the predicate.
    ///
    /// # Arguments
    ///
    /// * `predicate` - A closure that takes a reference to a value of type `T` and returns a `bool`.
    ///
    /// # Returns
    ///
    /// Returns `true` if the predicate returns `true` for any of the edge values, `false` otherwise.
    ///
    /// # Examples
    ///
    /// ```
    /// # use crate::ui::Edges;
    /// let edges = Edges {
    ///     top: 10,
    ///     right: 0,
    ///     bottom: 5,
    ///     left: 0,
    /// };
    ///
    /// assert!(edges.any(|value| *value == 0));
    /// assert!(edges.any(|value| *value > 0));
    /// assert!(!edges.any(|value| *value > 10));
    /// ```
    pub(crate) fn any<F: Fn(&T) -> bool>(&self, predicate: F) -> bool {
        predicate(&self.top)
            || predicate(&self.right)
            || predicate(&self.bottom)
            || predicate(&self.left)
    }
}

impl Edges<Length> {
    /// Sets the edges of the `Edges` struct to `auto`, which is a special value that allows the layout engine to automatically determine the size of the edges.
    ///
    /// This is typically used in layout contexts where the exact size of the edges is not important, or when the size should be calculated based on the content or container.
    ///
    /// # Returns
    ///
    /// Returns an `Edges<Length>` with all edges set to `Length::Auto`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use crate::ui::{Edges, Length};
    /// let auto_edges = Edges::auto();
    /// assert_eq!(auto_edges.top, Length::Auto);
    /// assert_eq!(auto_edges.right, Length::Auto);
    /// assert_eq!(auto_edges.bottom, Length::Auto);
    /// assert_eq!(auto_edges.left, Length::Auto);
    /// ```
    pub(crate) fn auto() -> Self {
        Self {
            top: Length::Auto,
            right: Length::Auto,
            bottom: Length::Auto,
            left: Length::Auto,
        }
    }

    /// Sets the edges of the `Edges` struct to zero, which means no size or thickness.
    ///
    /// This is typically used when you want to specify that a box (like a padding or margin area)
    /// should have no edges, effectively making it non-existent or invisible in layout calculations.
    ///
    /// # Returns
    ///
    /// Returns an `Edges<Length>` with all edges set to zero length.
    ///
    /// # Examples
    ///
    /// ```
    /// # use crate::ui::{DefiniteLength, Edges, Length, Pixels};
    /// let no_edges = Edges::<Length>::zero();
    /// assert_eq!(no_edges.top, Length::Definite(DefiniteLength::from(Pixels::ZERO)));
    /// assert_eq!(no_edges.right, Length::Definite(DefiniteLength::from(Pixels::ZERO)));
    /// assert_eq!(no_edges.bottom, Length::Definite(DefiniteLength::from(Pixels::ZERO)));
    /// assert_eq!(no_edges.left, Length::Definite(DefiniteLength::from(Pixels::ZERO)));
    /// ```
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
    /// Sets the edges of the `Edges` struct to zero, which means no size or thickness.
    ///
    /// This is typically used when you want to specify that a box (like a padding or margin area)
    /// should have no edges, effectively making it non-existent or invisible in layout calculations.
    ///
    /// # Returns
    ///
    /// Returns an `Edges<DefiniteLength>` with all edges set to zero length.
    ///
    /// # Examples
    ///
    /// ```
    /// # use crate::ui::{px, DefiniteLength, Edges};
    /// let no_edges = Edges::<DefiniteLength>::zero();
    /// assert_eq!(no_edges.top, DefiniteLength::from(px(0.)));
    /// assert_eq!(no_edges.right, DefiniteLength::from(px(0.)));
    /// assert_eq!(no_edges.bottom, DefiniteLength::from(px(0.)));
    /// assert_eq!(no_edges.left, DefiniteLength::from(px(0.)));
    /// ```
    pub(crate) fn zero() -> Self {
        Self {
            top: px(0.).into(),
            right: px(0.).into(),
            bottom: px(0.).into(),
            left: px(0.).into(),
        }
    }
}

impl Edges<AbsoluteLength> {
    /// Sets the edges of the `Edges` struct to zero, which means no size or thickness.
    ///
    /// This is typically used when you want to specify that a box (like a padding or margin area)
    /// should have no edges, effectively making it non-existent or invisible in layout calculations.
    ///
    /// # Returns
    ///
    /// Returns an `Edges<AbsoluteLength>` with all edges set to zero length.
    ///
    /// # Examples
    ///
    /// ```
    /// # use crate::ui::{AbsoluteLength, Edges, Pixels};
    /// let no_edges = Edges::<AbsoluteLength>::zero();
    /// assert_eq!(no_edges.top, AbsoluteLength::Pixels(Pixels::ZERO));
    /// assert_eq!(no_edges.right, AbsoluteLength::Pixels(Pixels::ZERO));
    /// assert_eq!(no_edges.bottom, AbsoluteLength::Pixels(Pixels::ZERO));
    /// assert_eq!(no_edges.left, AbsoluteLength::Pixels(Pixels::ZERO));
    /// ```
    pub(crate) fn zero() -> Self {
        Self {
            top: px(0.).into(),
            right: px(0.).into(),
            bottom: px(0.).into(),
            left: px(0.).into(),
        }
    }

    /// Converts the `AbsoluteLength` to `Pixels` based on the `rem_size`.
    ///
    /// If the `AbsoluteLength` is already in pixels, it simply returns the corresponding `Pixels` value.
    /// If the `AbsoluteLength` is in rems, it multiplies the number of rems by the `rem_size` to convert it to pixels.
    ///
    /// # Arguments
    ///
    /// * `rem_size` - The size of one rem unit in pixels.
    ///
    /// # Returns
    ///
    /// Returns an `Edges<Pixels>` representing the edges with lengths converted to pixels.
    ///
    /// # Examples
    ///
    /// ```
    /// # use crate::ui::{Edges, AbsoluteLength, Pixels, px, rems};
    /// let edges = Edges {
    ///     top: AbsoluteLength::Pixels(px(10.0)),
    ///     right: AbsoluteLength::Rems(rems(1.0)),
    ///     bottom: AbsoluteLength::Pixels(px(20.0)),
    ///     left: AbsoluteLength::Rems(rems(2.0)),
    /// };
    /// let rem_size = px(16.0);
    /// let edges_in_pixels = edges.to_pixels(rem_size);
    ///
    /// assert_eq!(edges_in_pixels.top, px(10.0)); // Already in pixels
    /// assert_eq!(edges_in_pixels.right, px(16.0)); // 1 rem converted to pixels
    /// assert_eq!(edges_in_pixels.bottom, px(20.0)); // Already in pixels
    /// assert_eq!(edges_in_pixels.left, px(32.0)); // 2 rems converted to pixels
    /// ```
    pub(crate) fn to_pixels(self, rem_size: Pixels) -> Edges<Pixels> {
        Edges {
            top: self.top.to_pixels(rem_size),
            right: self.right.to_pixels(rem_size),
            bottom: self.bottom.to_pixels(rem_size),
            left: self.left.to_pixels(rem_size),
        }
    }
}

impl From<f32> for Edges<Pixels> {
    fn from(val: f32) -> Self {
        let val: Pixels = val.into();
        val.into()
    }
}

impl From<Pixels> for Edges<Pixels> {
    fn from(val: Pixels) -> Self {
        Edges {
            top: val,
            right: val,
            bottom: val,
            left: val,
        }
    }
}

/// Identifies a reference point on a 2D box, used to anchor positioned elements.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Anchor {
    /// The top left corner
    TopLeft,
    /// The top right corner
    TopRight,
    /// The bottom left corner
    BottomLeft,
    /// The bottom right corner
    BottomRight,
    /// The top center position
    TopCenter,
    /// The bottom center position
    BottomCenter,
    /// The left center position
    LeftCenter,
    /// The right center position
    RightCenter,
}

impl Anchor {
    /// Returns the corner across from this corner, moving along the specified axis.
    ///
    /// # Examples
    ///
    /// ```
    /// # use crate::ui::{Axis, Anchor};
    /// let result = Anchor::TopLeft.other_side_along(Axis::Horizontal);
    /// assert_eq!(result, Anchor::TopRight);
    /// ```
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

/// Represents the corners of a box in a 2D space, such as border radius.
///
/// Each field represents the size of the corner on one side of the box: `top_left`, `top_right`, `bottom_right`, and `bottom_left`.
#[derive(Clone, Default, Debug, Eq, PartialEq)]
#[repr(C)]
pub(crate) struct Corners<T: Clone + Debug + Default + PartialEq> {
    /// The value associated with the top left corner.
    pub(crate) top_left: T,
    /// The value associated with the top right corner.
    pub(crate) top_right: T,
    /// The value associated with the bottom right corner.
    pub(crate) bottom_right: T,
    /// The value associated with the bottom left corner.
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
        self.top_left.is_none() && self.top_right.is_none() && self.bottom_right.is_none() && self.bottom_left.is_none()
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

impl Corners<AbsoluteLength> {
    /// Converts the `AbsoluteLength` to `Pixels` based on the provided rem size.
    ///
    /// # Arguments
    ///
    /// * `rem_size` - The size of one REM unit in pixels, used for conversion if the `AbsoluteLength` is in REMs.
    ///
    /// # Returns
    ///
    /// Returns a `Corners<Pixels>` instance with each corner's length converted to pixels.
    ///
    /// # Examples
    ///
    /// ```
    /// # use crate::ui::{Corners, AbsoluteLength, Pixels, Rems, Size};
    /// let corners = Corners {
    ///     top_left: AbsoluteLength::Pixels(Pixels::from(15.0)),
    ///     top_right: AbsoluteLength::Rems(Rems(1.0)),
    ///     bottom_right: AbsoluteLength::Pixels(Pixels::from(30.0)),
    ///     bottom_left: AbsoluteLength::Rems(Rems(2.0)),
    /// };
    /// let rem_size = Pixels::from(16.0);
    /// let corners_in_pixels = corners.to_pixels(rem_size);
    ///
    /// assert_eq!(corners_in_pixels.top_left, Pixels::from(15.0));
    /// assert_eq!(corners_in_pixels.top_right, Pixels::from(16.0)); // 1 rem converted to pixels
    /// assert_eq!(corners_in_pixels.bottom_right, Pixels::from(30.0));
    /// assert_eq!(corners_in_pixels.bottom_left, Pixels::from(32.0)); // 2 rems converted to pixels
    /// ```
    pub(crate) fn to_pixels(self, rem_size: Pixels) -> Corners<Pixels> {
        Corners {
            top_left: self.top_left.to_pixels(rem_size),
            top_right: self.top_right.to_pixels(rem_size),
            bottom_right: self.bottom_right.to_pixels(rem_size),
            bottom_left: self.bottom_left.to_pixels(rem_size),
        }
    }
}

impl Corners<Pixels> {
    /// Scales the `Corners<Pixels>` by a given factor, returning `Corners<ScaledPixels>`.
    ///
    /// This method is typically used for adjusting the corner sizes for different display densities or scaling factors.
    ///
    /// # Arguments
    ///
    /// * `factor` - The scaling factor to apply to each corner.
    ///
    /// # Returns
    ///
    /// Returns a new `Corners<ScaledPixels>` where each corner is the result of scaling the original corner by the given factor.
    ///
    /// # Examples
    ///
    /// ```
    /// # use crate::ui::{Corners, Pixels, ScaledPixels};
    /// let corners = Corners {
    ///     top_left: Pixels::from(10.0),
    ///     top_right: Pixels::from(20.0),
    ///     bottom_right: Pixels::from(30.0),
    ///     bottom_left: Pixels::from(40.0),
    /// };
    /// let scaled_corners = corners.scale(2.0);
    /// assert_eq!(scaled_corners.top_left, ScaledPixels::from(20.0));
    /// assert_eq!(scaled_corners.top_right, ScaledPixels::from(40.0));
    /// assert_eq!(scaled_corners.bottom_right, ScaledPixels::from(60.0));
    /// assert_eq!(scaled_corners.bottom_left, ScaledPixels::from(80.0));
    /// ```
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
    /// Clamps corner radii to be less than or equal to half the shortest side of a quad.
    ///
    /// # Arguments
    ///
    /// * `size` - The size of the quad which limits the size of the corner radii.
    ///
    /// # Returns
    ///
    /// Anchor radii values clamped to fit.
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

impl<T> Mul for Corners<T>
where
    T: Mul<Output = T> + Clone + Debug + Default + PartialEq,
{
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

impl<T, S> MulAssign<S> for Corners<T>
where
    T: Mul<S, Output = T> + Clone + Debug + Default + PartialEq,
    S: Clone,
{
    fn mul_assign(&mut self, rhs: S) {
        self.top_left = self.top_left.clone() * rhs.clone();
        self.top_right = self.top_right.clone() * rhs.clone();
        self.bottom_right = self.bottom_right.clone() * rhs.clone();
        self.bottom_left = self.bottom_left.clone() * rhs;
    }
}

impl<T> Copy for Corners<T> where T: Copy + Clone + Debug + Default + PartialEq {}

impl From<f32> for Corners<Pixels> {
    fn from(val: f32) -> Self {
        Corners {
            top_left: val.into(),
            top_right: val.into(),
            bottom_right: val.into(),
            bottom_left: val.into(),
        }
    }
}

impl From<Pixels> for Corners<Pixels> {
    fn from(val: Pixels) -> Self {
        Corners {
            top_left: val,
            top_right: val,
            bottom_right: val,
            bottom_left: val,
        }
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

/// Represents a length in pixels, the base unit of measurement in the UI framework.
///
/// `Pixels` is a value type that represents an absolute length in pixels, which is used
/// for specifying sizes, positions, and distances in the UI. It is the fundamental unit
/// of measurement for all visual elements and layout calculations.
///
/// The inner value is an `f32`, allowing for sub-pixel precision which can be useful for
/// anti-aliasing and animations. However, when applied to actual pixel grids, the value
/// is typically rounded to the nearest integer.
///
/// # Examples
///
/// ```
/// use crate::ui::{Pixels, ScaledPixels};
///
/// // Define a length of 10 pixels
/// let length = Pixels::from(10.0);
///
/// // Define a length and scale it by a factor of 2
/// let scaled_length = length.scale(2.0);
/// assert_eq!(scaled_length, ScaledPixels::from(20.0));
/// ```
#[derive(Clone, Copy, Default, PartialEq)]
#[repr(transparent)]
pub(crate) struct Pixels(pub(crate) f32);
impl_scalar_newtype_ops!(Pixels, f32);

impl Div for Pixels {
    type Output = f32;

    fn div(self, rhs: Self) -> Self::Output {
        self.0 / rhs.0
    }
}

impl std::ops::DivAssign for Pixels {
    fn div_assign(&mut self, rhs: Self) {
        *self = Self(self.0 / rhs.0);
    }
}

impl std::ops::RemAssign for Pixels {
    fn rem_assign(&mut self, rhs: Self) {
        self.0 %= rhs.0;
    }
}

impl std::ops::Rem for Pixels {
    type Output = Self;

    fn rem(self, rhs: Self) -> Self {
        Self(self.0 % rhs.0)
    }
}

impl Mul<f32> for Pixels {
    type Output = Self;

    fn mul(self, rhs: f32) -> Self {
        Self(self.0 * rhs)
    }
}

impl Mul<Pixels> for f32 {
    type Output = Pixels;

    fn mul(self, rhs: Pixels) -> Self::Output {
        rhs * self
    }
}

impl Mul<usize> for Pixels {
    type Output = Self;

    fn mul(self, rhs: usize) -> Self {
        self * (rhs as f32)
    }
}

impl Mul<Pixels> for usize {
    type Output = Pixels;

    fn mul(self, rhs: Pixels) -> Pixels {
        rhs * self
    }
}

impl MulAssign<f32> for Pixels {
    fn mul_assign(&mut self, rhs: f32) {
        self.0 *= rhs;
    }
}

impl Display for Pixels {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}px", self.0)
    }
}

impl Debug for Pixels {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Display::fmt(self, f)
    }
}

impl std::iter::Sum for Pixels {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        iter.fold(Self::ZERO, |a, b| a + b)
    }
}

impl<'a> std::iter::Sum<&'a Pixels> for Pixels {
    fn sum<I: Iterator<Item = &'a Self>>(iter: I) -> Self {
        iter.fold(Self::ZERO, |a, b| a + *b)
    }
}

impl TryFrom<&'_ str> for Pixels {
    type Error = crate::ui::anyhow::Error;

    fn try_from(value: &'_ str) -> Result<Self, Self::Error> {
        value
            .strip_suffix("px")
            .context("expected 'px' suffix")
            .and_then(|number| Ok(number.parse()?))
            .map(Self)
    }
}

impl Pixels {
    /// Represents zero pixels.
    pub(crate) const ZERO: Pixels = Pixels(0.0);
    /// The maximum value that can be represented by `Pixels`.
    pub(crate) const MAX: Pixels = Pixels(f32::MAX);

    /// Returns the raw `f32` value of this `Pixels`.
    pub(crate) fn as_f32(self) -> f32 {
        self.0
    }

    /// Rounds the `Pixels` value to the nearest whole number.
    pub(crate) fn round(&self) -> Self {
        Self(self.0.round())
    }

    /// Returns the ceiling of the `Pixels` value to the nearest whole number.
    pub(crate) fn ceil(&self) -> Self {
        Self(self.0.ceil())
    }

    /// Scales the `Pixels` value by a given factor, producing `ScaledPixels`.
    #[must_use]
    pub(crate) fn scale(&self, factor: f32) -> ScaledPixels {
        ScaledPixels(self.0 * factor)
    }

    /// Returns the absolute value of the `Pixels`.
    pub(crate) fn abs(&self) -> Self {
        Self(self.0.abs())
    }
}

impl Eq for Pixels {}

impl PartialOrd for Pixels {
    fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Pixels {
    fn cmp(&self, other: &Self) -> cmp::Ordering {
        self.0.total_cmp(&other.0)
    }
}

impl std::hash::Hash for Pixels {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.to_bits().hash(state);
    }
}

impl From<f64> for Pixels {
    fn from(pixels: f64) -> Self {
        Pixels(pixels as f32)
    }
}

impl From<f32> for Pixels {
    fn from(pixels: f32) -> Self {
        Pixels(pixels)
    }
}

impl From<Pixels> for f32 {
    fn from(pixels: Pixels) -> Self {
        pixels.0
    }
}

impl From<&Pixels> for f32 {
    fn from(pixels: &Pixels) -> Self {
        pixels.0
    }
}

impl From<Pixels> for f64 {
    fn from(pixels: Pixels) -> Self {
        pixels.0 as f64
    }
}

impl From<Pixels> for u32 {
    fn from(pixels: Pixels) -> Self {
        pixels.0 as u32
    }
}

impl From<&Pixels> for u32 {
    fn from(pixels: &Pixels) -> Self {
        pixels.0 as u32
    }
}

impl From<u32> for Pixels {
    fn from(pixels: u32) -> Self {
        Pixels(pixels as f32)
    }
}

impl From<Pixels> for usize {
    fn from(pixels: Pixels) -> Self {
        pixels.0 as usize
    }
}

impl From<usize> for Pixels {
    fn from(pixels: usize) -> Self {
        Pixels(pixels as f32)
    }
}

/// Represents physical pixels on the display.
///
/// `DevicePixels` is a unit of measurement that refers to the actual pixels on a device's screen.
/// This type is used when precise pixel manipulation is required, such as rendering graphics or
/// interfacing with hardware that operates on the pixel level. Unlike logical pixels that may be
/// affected by the device's scale factor, `DevicePixels` always correspond to real pixels on the
/// display.
#[derive(Clone, Copy, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub(crate) struct DevicePixels(pub(crate) i32);

impl Add for DevicePixels {
    type Output = Self;
    fn add(self, rhs: Self) -> Self { Self(self.0 + rhs.0) }
}
impl AddAssign for DevicePixels {
    fn add_assign(&mut self, rhs: Self) { self.0 += rhs.0; }
}
impl Sub for DevicePixels {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self { Self(self.0 - rhs.0) }
}
impl SubAssign for DevicePixels {
    fn sub_assign(&mut self, rhs: Self) { self.0 -= rhs.0; }
}
impl Div<i32> for DevicePixels {
    type Output = Self;
    fn div(self, rhs: i32) -> Self { Self(self.0 / rhs) }
}

impl DevicePixels {
    /// Converts the `DevicePixels` value to the number of bytes needed to represent it in memory.
    ///
    /// This function is useful when working with graphical data that needs to be stored in a buffer,
    /// such as images or framebuffers, where each pixel may be represented by a specific number of bytes.
    ///
    /// # Arguments
    ///
    /// * `bytes_per_pixel` - The number of bytes used to represent a single pixel.
    ///
    /// # Returns
    ///
    /// The number of bytes required to represent the `DevicePixels` value in memory.
    ///
    /// # Examples
    ///
    /// ```
    /// # use crate::ui::DevicePixels;
    /// let pixels = DevicePixels(10); // 10 device pixels
    /// let bytes_per_pixel = 4; // Assume each pixel is represented by 4 bytes (e.g., RGBA)
    /// let total_bytes = pixels.to_bytes(bytes_per_pixel);
    /// assert_eq!(total_bytes, 40); // 10 pixels * 4 bytes/pixel = 40 bytes
    /// ```
    pub(crate) fn to_bytes(self, bytes_per_pixel: u8) -> u32 {
        self.0 as u32 * bytes_per_pixel as u32
    }
}

impl fmt::Debug for DevicePixels {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} px (device)", self.0)
    }
}

impl From<DevicePixels> for i32 {
    fn from(device_pixels: DevicePixels) -> Self {
        device_pixels.0
    }
}

impl From<i32> for DevicePixels {
    fn from(device_pixels: i32) -> Self {
        DevicePixels(device_pixels)
    }
}

impl From<u32> for DevicePixels {
    fn from(device_pixels: u32) -> Self {
        DevicePixels(device_pixels as i32)
    }
}

impl From<DevicePixels> for u32 {
    fn from(device_pixels: DevicePixels) -> Self {
        device_pixels.0 as u32
    }
}

impl From<DevicePixels> for u64 {
    fn from(device_pixels: DevicePixels) -> Self {
        device_pixels.0 as u64
    }
}

impl From<u64> for DevicePixels {
    fn from(device_pixels: u64) -> Self {
        DevicePixels(device_pixels as i32)
    }
}

impl From<DevicePixels> for usize {
    fn from(device_pixels: DevicePixels) -> Self {
        device_pixels.0 as usize
    }
}

impl From<usize> for DevicePixels {
    fn from(device_pixels: usize) -> Self {
        DevicePixels(device_pixels as i32)
    }
}

/// Represents scaled pixels that take into account the device's scale factor.
///
/// `ScaledPixels` are used to ensure that UI elements appear at the correct size on devices
/// with different pixel densities. When a device has a higher scale factor (such as Retina displays),
/// a single logical pixel may correspond to multiple physical pixels. By using `ScaledPixels`,
/// dimensions and positions can be specified in a way that scales appropriately across different
/// display resolutions.
#[derive(Clone, Copy, Default, PartialEq)]
#[repr(transparent)]
pub(crate) struct ScaledPixels(pub(crate) f32);

impl Add for ScaledPixels {
    type Output = Self;
    fn add(self, rhs: Self) -> Self { Self(self.0 + rhs.0) }
}
impl AddAssign for ScaledPixels {
    fn add_assign(&mut self, rhs: Self) { self.0 += rhs.0; }
}
impl Sub for ScaledPixels {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self { Self(self.0 - rhs.0) }
}
impl SubAssign for ScaledPixels {
    fn sub_assign(&mut self, rhs: Self) { self.0 -= rhs.0; }
}
impl Div<f32> for ScaledPixels {
    type Output = Self;
    fn div(self, rhs: f32) -> Self { Self(self.0 / rhs) }
}
impl DivAssign<f32> for ScaledPixels {
    fn div_assign(&mut self, rhs: f32) { self.0 /= rhs; }
}

impl ScaledPixels {
    /// Returns the raw `f32` value of this `ScaledPixels`.
    pub(crate) fn as_f32(self) -> f32 {
        self.0
    }
}

impl Eq for ScaledPixels {}

impl PartialOrd for ScaledPixels {
    fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ScaledPixels {
    fn cmp(&self, other: &Self) -> cmp::Ordering {
        self.0.total_cmp(&other.0)
    }
}

impl Debug for ScaledPixels {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}px (scaled)", self.0)
    }
}

impl From<ScaledPixels> for DevicePixels {
    fn from(scaled: ScaledPixels) -> Self {
        DevicePixels(scaled.0.ceil() as i32)
    }
}

impl From<DevicePixels> for ScaledPixels {
    fn from(device: DevicePixels) -> Self {
        ScaledPixels(device.0 as f32)
    }
}

impl From<ScaledPixels> for f64 {
    fn from(scaled_pixels: ScaledPixels) -> Self {
        scaled_pixels.0 as f64
    }
}

impl From<ScaledPixels> for u32 {
    fn from(pixels: ScaledPixels) -> Self {
        pixels.0 as u32
    }
}

impl From<f32> for ScaledPixels {
    fn from(pixels: f32) -> Self {
        Self(pixels)
    }
}

impl Div for ScaledPixels {
    type Output = f32;

    fn div(self, rhs: Self) -> Self::Output {
        self.0 / rhs.0
    }
}

impl std::ops::DivAssign for ScaledPixels {
    fn div_assign(&mut self, rhs: Self) {
        *self = Self(self.0 / rhs.0);
    }
}

impl std::ops::RemAssign for ScaledPixels {
    fn rem_assign(&mut self, rhs: Self) {
        self.0 %= rhs.0;
    }
}

impl std::ops::Rem for ScaledPixels {
    type Output = Self;

    fn rem(self, rhs: Self) -> Self {
        Self(self.0 % rhs.0)
    }
}

impl Mul<f32> for ScaledPixels {
    type Output = Self;

    fn mul(self, rhs: f32) -> Self {
        Self(self.0 * rhs)
    }
}

impl Mul<ScaledPixels> for f32 {
    type Output = ScaledPixels;

    fn mul(self, rhs: ScaledPixels) -> Self::Output {
        rhs * self
    }
}

impl Mul<usize> for ScaledPixels {
    type Output = Self;

    fn mul(self, rhs: usize) -> Self {
        self * (rhs as f32)
    }
}

impl Mul<ScaledPixels> for usize {
    type Output = ScaledPixels;

    fn mul(self, rhs: ScaledPixels) -> ScaledPixels {
        rhs * self
    }
}

impl MulAssign<f32> for ScaledPixels {
    fn mul_assign(&mut self, rhs: f32) {
        self.0 *= rhs;
    }
}

/// Represents a length in rems, a unit based on the font-size of the window, which can be assigned with [`Window::set_rem_size`][set_rem_size].
///
/// Rems are used for defining lengths that are scalable and consistent across different UI elements.
/// The value of `1rem` is typically equal to the font-size of the root element (often the `<html>` element in browsers),
/// making it a flexible unit that adapts to the user's text size preferences. In this framework, `rems` serve a similar
/// purpose, allowing for scalable and accessible design that can adjust to different display settings or user preferences.
///
/// For example, if the root element's font-size is `16px`, then `1rem` equals `16px`. A length of `2rems` would then be `32px`.
///
/// [set_rem_size]: crate::ui::Window::set_rem_size
#[derive(Clone, Copy, Default, PartialEq)]
pub(crate) struct Rems(pub(crate) f32);

impl Add for Rems {
    type Output = Self;
    fn add(self, rhs: Self) -> Self { Self(self.0 + rhs.0) }
}
impl Sub for Rems {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self { Self(self.0 - rhs.0) }
}
impl Mul<f32> for Rems {
    type Output = Self;
    fn mul(self, rhs: f32) -> Self { Self(self.0 * rhs) }
}
impl Div<f32> for Rems {
    type Output = Self;
    fn div(self, rhs: f32) -> Self { Self(self.0 / rhs) }
}
impl Neg for Rems {
    type Output = Self;
    fn neg(self) -> Self { Self(-self.0) }
}

impl Rems {
    /// Convert this Rem value to pixels.
    pub(crate) fn to_pixels(self, rem_size: Pixels) -> Pixels {
        self * rem_size
    }
}

impl Mul<Pixels> for Rems {
    type Output = Pixels;

    fn mul(self, other: Pixels) -> Pixels {
        Pixels(self.0 * other.0)
    }
}

impl AddAssign<Rems> for Rems {
    fn add_assign(&mut self, rhs: Rems) {
        self.0 += rhs.0
    }
}

impl Display for Rems {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}rem", self.0)
    }
}

impl Debug for Rems {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Display::fmt(self, f)
    }
}

impl TryFrom<&'_ str> for Rems {
    type Error = crate::ui::anyhow::Error;

    fn try_from(value: &'_ str) -> Result<Self, Self::Error> {
        value
            .strip_suffix("rem")
            .context("expected 'rem' suffix")
            .and_then(|number| Ok(number.parse()?))
            .map(Self)
    }
}

/// Represents an absolute length in pixels or rems.
///
/// `AbsoluteLength` can be either a fixed number of pixels, which is an absolute measurement not
/// affected by the current font size, or a number of rems, which is relative to the font size of
/// the root element. It is used for specifying dimensions that are either independent of or
/// related to the typographic scale.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum AbsoluteLength {
    /// A length in pixels.
    Pixels(Pixels),
    /// A length in rems.
    Rems(Rems),
}

impl Neg for AbsoluteLength {
    type Output = Self;
    fn neg(self) -> Self {
        match self {
            AbsoluteLength::Pixels(p) => AbsoluteLength::Pixels(-p),
            AbsoluteLength::Rems(r) => AbsoluteLength::Rems(-r),
        }
    }
}

impl AbsoluteLength {
    /// Checks if the absolute length is zero.
    pub(crate) fn is_zero(&self) -> bool {
        match self {
            AbsoluteLength::Pixels(px) => px.0 == 0.0,
            AbsoluteLength::Rems(rems) => rems.0 == 0.0,
        }
    }
}

impl From<Pixels> for AbsoluteLength {
    fn from(pixels: Pixels) -> Self {
        AbsoluteLength::Pixels(pixels)
    }
}

impl From<Rems> for AbsoluteLength {
    fn from(rems: Rems) -> Self {
        AbsoluteLength::Rems(rems)
    }
}

impl AbsoluteLength {
    /// Converts an `AbsoluteLength` to `Pixels` based on a given `rem_size`.
    ///
    /// # Arguments
    ///
    /// * `rem_size` - The size of one rem in pixels.
    ///
    /// # Returns
    ///
    /// Returns the `AbsoluteLength` as `Pixels`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use crate::ui::{AbsoluteLength, Pixels, Rems};
    /// let length_in_pixels = AbsoluteLength::Pixels(Pixels::from(42.0));
    /// let length_in_rems = AbsoluteLength::Rems(Rems(2.0));
    /// let rem_size = Pixels::from(16.0);
    ///
    /// assert_eq!(length_in_pixels.to_pixels(rem_size), Pixels::from(42.0));
    /// assert_eq!(length_in_rems.to_pixels(rem_size), Pixels::from(32.0));
    /// ```
    pub(crate) fn to_pixels(self, rem_size: Pixels) -> Pixels {
        match self {
            AbsoluteLength::Pixels(pixels) => pixels,
            AbsoluteLength::Rems(rems) => rems.to_pixels(rem_size),
        }
    }
}

impl Default for AbsoluteLength {
    fn default() -> Self {
        px(0.).into()
    }
}

impl Display for AbsoluteLength {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pixels(pixels) => write!(f, "{pixels}"),
            Self::Rems(rems) => write!(f, "{rems}"),
        }
    }
}

impl Debug for AbsoluteLength {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Display::fmt(self, f)
    }
}

const EXPECTED_ABSOLUTE_LENGTH: &str = "number with 'px' or 'rem' suffix";

impl TryFrom<&'_ str> for AbsoluteLength {
    type Error = crate::ui::anyhow::Error;

    fn try_from(value: &'_ str) -> Result<Self, Self::Error> {
        if let Ok(pixels) = value.try_into() {
            Ok(Self::Pixels(pixels))
        } else if let Ok(rems) = value.try_into() {
            Ok(Self::Rems(rems))
        } else {
            Err(anyhow!(
                "invalid AbsoluteLength '{value}', expected {EXPECTED_ABSOLUTE_LENGTH}"
            ))
        }
    }
}

/// A non-auto length that can be defined in pixels, rems, or percent of parent.
///
/// This enum represents lengths that have a specific value, as opposed to lengths that are automatically
/// determined by the context. It includes absolute lengths in pixels or rems, and relative lengths as a
/// fraction of the parent's size.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum DefiniteLength {
    /// An absolute length specified in pixels or rems.
    Absolute(AbsoluteLength),
    /// A relative length specified as a fraction of the parent's size, between 0 and 1.
    Fraction(f32),
}

impl Neg for DefiniteLength {
    type Output = Self;
    fn neg(self) -> Self {
        match self {
            DefiniteLength::Absolute(a) => DefiniteLength::Absolute(-a),
            DefiniteLength::Fraction(f) => DefiniteLength::Fraction(-f),
        }
    }
}

impl DefiniteLength {
    /// Converts the `DefiniteLength` to `Pixels` based on a given `base_size` and `rem_size`.
    ///
    /// If the `DefiniteLength` is an absolute length, it will be directly converted to `Pixels`.
    /// If it is a fraction, the fraction will be multiplied by the `base_size` to get the length in pixels.
    ///
    /// # Arguments
    ///
    /// * `base_size` - The base size in `AbsoluteLength` to which the fraction will be applied.
    /// * `rem_size` - The size of one rem in pixels, used to convert rems to pixels.
    ///
    /// # Returns
    ///
    /// Returns the `DefiniteLength` as `Pixels`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use crate::ui::{DefiniteLength, AbsoluteLength, Pixels, px, rems};
    /// let length_in_pixels = DefiniteLength::Absolute(AbsoluteLength::Pixels(px(42.0)));
    /// let length_in_rems = DefiniteLength::Absolute(AbsoluteLength::Rems(rems(2.0)));
    /// let length_as_fraction = DefiniteLength::Fraction(0.5);
    /// let base_size = AbsoluteLength::Pixels(px(100.0));
    /// let rem_size = px(16.0);
    ///
    /// assert_eq!(length_in_pixels.to_pixels(base_size, rem_size), Pixels::from(42.0));
    /// assert_eq!(length_in_rems.to_pixels(base_size, rem_size), Pixels::from(32.0));
    /// assert_eq!(length_as_fraction.to_pixels(base_size, rem_size), Pixels::from(50.0));
    /// ```
    pub(crate) fn to_pixels(self, base_size: AbsoluteLength, rem_size: Pixels) -> Pixels {
        match self {
            DefiniteLength::Absolute(size) => size.to_pixels(rem_size),
            DefiniteLength::Fraction(fraction) => match base_size {
                AbsoluteLength::Pixels(px) => px * fraction,
                AbsoluteLength::Rems(rems) => rems * rem_size * fraction,
            },
        }
    }
}

impl Debug for DefiniteLength {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Display::fmt(self, f)
    }
}

impl Display for DefiniteLength {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DefiniteLength::Absolute(length) => write!(f, "{length}"),
            DefiniteLength::Fraction(fraction) => write!(f, "{}%", (fraction * 100.0) as i32),
        }
    }
}

const EXPECTED_DEFINITE_LENGTH: &str = "expected number with 'px', 'rem', or '%' suffix";

impl TryFrom<&'_ str> for DefiniteLength {
    type Error = crate::ui::anyhow::Error;

    fn try_from(value: &'_ str) -> Result<Self, Self::Error> {
        if let Some(percentage) = value.strip_suffix('%') {
            let fraction: f32 = percentage.parse::<f32>().with_context(|| {
                format!("invalid DefiniteLength '{value}', expected {EXPECTED_DEFINITE_LENGTH}")
            })?;
            Ok(DefiniteLength::Fraction(fraction / 100.0))
        } else if let Ok(absolute_length) = value.try_into() {
            Ok(DefiniteLength::Absolute(absolute_length))
        } else {
            Err(anyhow!(
                "invalid DefiniteLength '{value}', expected {EXPECTED_DEFINITE_LENGTH}"
            ))
        }
    }
}

impl From<Pixels> for DefiniteLength {
    fn from(pixels: Pixels) -> Self {
        Self::Absolute(pixels.into())
    }
}

impl From<Rems> for DefiniteLength {
    fn from(rems: Rems) -> Self {
        Self::Absolute(rems.into())
    }
}

impl From<AbsoluteLength> for DefiniteLength {
    fn from(length: AbsoluteLength) -> Self {
        Self::Absolute(length)
    }
}

impl Default for DefiniteLength {
    fn default() -> Self {
        Self::Absolute(AbsoluteLength::default())
    }
}

/// A length that can be defined in pixels, rems, percent of parent, or auto.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Length {
    /// A definite length specified either in pixels, rems, or as a fraction of the parent's size.
    Definite(DefiniteLength),
    /// An automatic length that is determined by the context in which it is used.
    Auto,
}

impl Debug for Length {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Display::fmt(self, f)
    }
}

impl Display for Length {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Length::Definite(definite_length) => write!(f, "{}", definite_length),
            Length::Auto => write!(f, "auto"),
        }
    }
}

const EXPECTED_LENGTH: &str = "expected 'auto' or number with 'px', 'rem', or '%' suffix";

impl TryFrom<&'_ str> for Length {
    type Error = crate::ui::anyhow::Error;

    fn try_from(value: &'_ str) -> Result<Self, Self::Error> {
        if value == "auto" {
            Ok(Length::Auto)
        } else if let Ok(definite_length) = value.try_into() {
            Ok(Length::Definite(definite_length))
        } else {
            Err(anyhow!(
                "invalid Length '{value}', expected {EXPECTED_LENGTH}"
            ))
        }
    }
}

/// Constructs a `DefiniteLength` representing a relative fraction of a parent size.
///
/// This function creates a `DefiniteLength` that is a specified fraction of a parent's dimension.
/// The fraction should be a floating-point number between 0.0 and 1.0, where 1.0 represents 100% of the parent's size.
///
/// # Arguments
///
/// * `fraction` - The fraction of the parent's size, between 0.0 and 1.0.
///
/// # Returns
///
/// A `DefiniteLength` representing the relative length as a fraction of the parent's size.
pub(crate) const fn relative(fraction: f32) -> DefiniteLength {
    DefiniteLength::Fraction(fraction)
}

/// Returns the Golden Ratio, i.e. `~(1.0 + sqrt(5.0)) / 2.0`.
#[allow(clippy::approx_constant)]
pub(crate) const fn phi() -> DefiniteLength {
    relative(1.618_034)
}

/// Constructs a `Rems` value representing a length in rems.
///
/// # Arguments
///
/// * `rems` - The number of rems for the length.
///
/// # Returns
///
/// A `Rems` representing the specified number of rems.
pub(crate) const fn rems(rems: f32) -> Rems {
    Rems(rems)
}

/// Constructs a `Pixels` value representing a length in pixels.
///
/// # Arguments
///
/// * `pixels` - The number of pixels for the length.
///
/// # Returns
///
/// A `Pixels` representing the specified number of pixels.
pub(crate) const fn px(pixels: f32) -> Pixels {
    Pixels(pixels)
}

impl From<Pixels> for Length {
    fn from(pixels: Pixels) -> Self {
        Self::Definite(pixels.into())
    }
}

impl From<Rems> for Length {
    fn from(rems: Rems) -> Self {
        Self::Definite(rems.into())
    }
}

impl From<DefiniteLength> for Length {
    fn from(length: DefiniteLength) -> Self {
        Self::Definite(length)
    }
}

impl From<AbsoluteLength> for Length {
    fn from(length: AbsoluteLength) -> Self {
        Self::Definite(length.into())
    }
}

impl Default for Length {
    fn default() -> Self {
        Self::Definite(DefiniteLength::default())
    }
}

impl From<()> for Length {
    fn from(_: ()) -> Self {
        Self::Definite(DefiniteLength::default())
    }
}

/// Provides a trait for types that can calculate half of their value.
///
/// The `Half` trait is used for types that can be evenly divided, returning a new instance of the same type
/// representing half of the original value. This is commonly used for types that represent measurements or sizes,
/// such as lengths or pixels, where halving is a frequent operation during layout calculations or animations.
pub(crate) trait Half {
    /// Returns half of the current value.
    ///
    /// # Returns
    ///
    /// A new instance of the implementing type, representing half of the original value.
    fn half(&self) -> Self;
}

impl Half for i32 {
    fn half(&self) -> Self {
        self / 2
    }
}

impl Half for f32 {
    fn half(&self) -> Self {
        self / 2.
    }
}

impl Half for DevicePixels {
    fn half(&self) -> Self {
        Self(self.0 / 2)
    }
}

impl Half for ScaledPixels {
    fn half(&self) -> Self {
        Self(self.0 / 2.)
    }
}

impl Half for Pixels {
    fn half(&self) -> Self {
        Self(self.0 / 2.)
    }
}

impl Half for Rems {
    fn half(&self) -> Self {
        Self(self.0 / 2.)
    }
}

/// A trait for checking if a value is zero.
///
/// This trait provides a method to determine if a value is considered to be zero.
/// It is implemented for various numeric and length-related types where the concept
/// of zero is applicable. This can be useful for comparisons, optimizations, or
/// determining if an operation has a neutral effect.
pub(crate) trait IsZero {
    /// Determines if the value is zero.
    ///
    /// # Returns
    ///
    /// Returns `true` if the value is zero, `false` otherwise.
    fn is_zero(&self) -> bool;
}

impl IsZero for DevicePixels {
    fn is_zero(&self) -> bool {
        self.0 == 0
    }
}

impl IsZero for ScaledPixels {
    fn is_zero(&self) -> bool {
        self.0 == 0.
    }
}

impl IsZero for Pixels {
    fn is_zero(&self) -> bool {
        self.0 == 0.
    }
}

impl IsZero for Rems {
    fn is_zero(&self) -> bool {
        self.0 == 0.
    }
}

impl IsZero for AbsoluteLength {
    fn is_zero(&self) -> bool {
        match self {
            AbsoluteLength::Pixels(pixels) => pixels.is_zero(),
            AbsoluteLength::Rems(rems) => rems.is_zero(),
        }
    }
}

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
    fn is_zero(&self) -> bool {
        self.x.is_zero() && self.y.is_zero()
    }
}

impl<T> IsZero for Size<T>
where
    T: IsZero + Clone + Debug + Default + PartialEq,
{
    fn is_zero(&self) -> bool {
        self.width.is_zero() || self.height.is_zero()
    }
}

impl<T: IsZero + Clone + Debug + Default + PartialEq> IsZero for Bounds<T> {
    fn is_zero(&self) -> bool {
        self.size.is_zero()
    }
}

impl<T> IsZero for Corners<T>
where
    T: IsZero + Clone + Debug + Default + PartialEq,
{
    fn is_zero(&self) -> bool {
        self.top_left.is_zero()
            && self.top_right.is_zero()
            && self.bottom_right.is_zero()
            && self.bottom_left.is_zero()
    }
}
