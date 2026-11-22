use std::{
    hash::{Hash, Hasher},
    iter, mem,
    ops::Range,
};

use crate::{
    AbsoluteLength, App, Background, BorderStyle, Bounds, ContentMask, Corners, CursorStyle,
    DefiniteLength, DevicePixels, Edges, Font, FontFallbacks, FontFeatures, FontStyle, FontWeight,
    Hsla, Length, Pixels, Point, Rgba, SharedString, Size, Styled, TextRun, Window, black, phi,
    point, px, quad, rems, size,
};
use crate::collections::HashSet;
use crate::refineable::{IsEmpty, Refineable};

/// Use this struct for interfacing with the 'debug_below' styling from your own elements.
/// If a parent element has this style set on it, then this struct will be set as a global in
/// GPUI.
#[cfg(debug_assertions)]
pub struct DebugBelow;

#[cfg(debug_assertions)]
impl crate::Global for DebugBelow {}

/// How to fit the image into the bounds of the element.
pub enum ObjectFit {
    /// The image will be stretched to fill the bounds of the element.
    Fill,
    /// The image will be scaled to fit within the bounds of the element.
    Contain,
    /// The image will be scaled to cover the bounds of the element.
    Cover,
    /// The image will be scaled down to fit within the bounds of the element.
    ScaleDown,
    /// The image will maintain its original size.
    None,
}

impl ObjectFit {
    /// Get the bounds of the image within the given bounds.
    pub fn get_bounds(
        &self,
        bounds: Bounds<Pixels>,
        image_size: Size<DevicePixels>,
    ) -> Bounds<Pixels> {
        let image_size = image_size.map(|dimension| Pixels::from(u32::from(dimension)));
        let image_ratio = image_size.width / image_size.height;
        let bounds_ratio = bounds.size.width / bounds.size.height;

        match self {
            ObjectFit::Fill => bounds,
            ObjectFit::Contain => {
                let new_size = if bounds_ratio > image_ratio {
                    size(
                        image_size.width * (bounds.size.height / image_size.height),
                        bounds.size.height,
                    )
                } else {
                    size(
                        bounds.size.width,
                        image_size.height * (bounds.size.width / image_size.width),
                    )
                };

                Bounds {
                    origin: point(
                        bounds.origin.x + (bounds.size.width - new_size.width) / 2.0,
                        bounds.origin.y + (bounds.size.height - new_size.height) / 2.0,
                    ),
                    size: new_size,
                }
            }
            ObjectFit::ScaleDown => {
                // Check if the image is larger than the bounds in either dimension.
                if image_size.width > bounds.size.width || image_size.height > bounds.size.height {
                    // If the image is larger, use the same logic as Contain to scale it down.
                    let new_size = if bounds_ratio > image_ratio {
                        size(
                            image_size.width * (bounds.size.height / image_size.height),
                            bounds.size.height,
                        )
                    } else {
                        size(
                            bounds.size.width,
                            image_size.height * (bounds.size.width / image_size.width),
                        )
                    };

                    Bounds {
                        origin: point(
                            bounds.origin.x + (bounds.size.width - new_size.width) / 2.0,
                            bounds.origin.y + (bounds.size.height - new_size.height) / 2.0,
                        ),
                        size: new_size,
                    }
                } else {
                    // If the image is smaller than or equal to the container, display it at its original size,
                    // centered within the container.
                    let original_size = size(image_size.width, image_size.height);
                    Bounds {
                        origin: point(
                            bounds.origin.x + (bounds.size.width - original_size.width) / 2.0,
                            bounds.origin.y + (bounds.size.height - original_size.height) / 2.0,
                        ),
                        size: original_size,
                    }
                }
            }
            ObjectFit::Cover => {
                let new_size = if bounds_ratio > image_ratio {
                    size(
                        bounds.size.width,
                        image_size.height * (bounds.size.width / image_size.width),
                    )
                } else {
                    size(
                        image_size.width * (bounds.size.height / image_size.height),
                        bounds.size.height,
                    )
                };

                Bounds {
                    origin: point(
                        bounds.origin.x + (bounds.size.width - new_size.width) / 2.0,
                        bounds.origin.y + (bounds.size.height - new_size.height) / 2.0,
                    ),
                    size: new_size,
                }
            }
            ObjectFit::None => Bounds {
                origin: bounds.origin,
                size: image_size,
            },
        }
    }
}

/// The CSS styling that can be applied to an element via the `Styled` trait
#[derive(Clone, Debug)]
pub struct Style {
    /// What layout strategy should be used?
    pub display: Display,

    /// Should the element be painted on screen?
    pub visibility: Visibility,

    // Overflow properties
    /// How children overflowing their container should affect layout
    pub overflow: Point<Overflow>,
    /// How much space (in points) should be reserved for the scrollbars of `Overflow::Scroll` and `Overflow::Auto` nodes.
    pub scrollbar_width: AbsoluteLength,
    /// Whether both x and y axis should be scrollable at the same time.
    pub allow_concurrent_scroll: bool,
    /// Whether scrolling should be restricted to the input gesture's axis.
    pub restrict_scroll_to_axis: bool,

    // Position properties
    /// What should the `position` value of this struct use as a base offset?
    pub position: Position,
    /// How should the position of this element be tweaked relative to the layout defined?
    pub inset: Edges<Length>,

    // Size properties
    /// Sets the initial size of the item
    pub size: Size<Length>,
    /// Controls the minimum size of the item
    pub min_size: Size<Length>,
    /// Controls the maximum size of the item
    pub max_size: Size<Length>,
    /// Sets the preferred aspect ratio for the item. The ratio is calculated as width divided by height.
    pub aspect_ratio: Option<f32>,

    // Spacing Properties
    /// How large should the margin be on each side?
    pub margin: Edges<Length>,
    /// How large should the padding be on each side?
    pub padding: Edges<DefiniteLength>,
    /// How large should the border be on each side?
    pub border_widths: Edges<AbsoluteLength>,

    // Alignment properties
    /// How this node's children aligned in the cross/block axis?
    pub align_items: Option<AlignItems>,
    /// How this node should be aligned in the cross/block axis. Falls back to the parents [`AlignItems`] if not set
    pub align_self: Option<AlignSelf>,
    /// How should content contained within this item be aligned in the cross/block axis
    pub align_content: Option<AlignContent>,
    /// How should contained within this item be aligned in the main/inline axis
    pub justify_content: Option<JustifyContent>,
    /// How large should the gaps between items in a flex container be?
    pub gap: Size<DefiniteLength>,

    // Flexbox properties
    /// Which direction does the main axis flow in?
    pub flex_direction: FlexDirection,
    /// Should elements wrap, or stay in a single line?
    pub flex_wrap: FlexWrap,
    /// Sets the initial main axis size of the item
    pub flex_basis: Length,
    /// The relative rate at which this item grows when it is expanding to fill space, 0.0 is the default value, and this value must be positive.
    pub flex_grow: f32,
    /// The relative rate at which this item shrinks when it is contracting to fit into space, 1.0 is the default value, and this value must be positive.
    pub flex_shrink: f32,

    /// The fill color of this element
    pub background: Option<Fill>,

    /// The border color of this element
    pub border_color: Option<Hsla>,

    /// The border style of this element
    pub border_style: BorderStyle,

    /// The radius of the corners of this element
    pub corner_radii: Corners<AbsoluteLength>,

    /// Box shadow of the element
    pub box_shadow: Vec<BoxShadow>,

    /// The text style of this element
    pub text: TextStyleRefinement,

    /// The mouse cursor style shown when the mouse pointer is over an element.
    pub mouse_cursor: Option<CursorStyle>,

    /// The opacity of this element
    pub opacity: Option<f32>,

    /// Whether to draw a red debugging outline around this element
    #[cfg(debug_assertions)]
    pub debug: bool,

    /// Whether to draw a red debugging outline around this element and all of its conforming children
    #[cfg(debug_assertions)]
    pub debug_below: bool,
}

#[derive(Clone, Default, PartialEq)]
pub struct StyleRefinement {
    pub display: Option<Display>,
    pub visibility: Option<Visibility>,
    pub overflow: crate::PointRefinement<Overflow>,
    pub scrollbar_width: Option<AbsoluteLength>,
    pub allow_concurrent_scroll: Option<bool>,
    pub restrict_scroll_to_axis: Option<bool>,
    pub position: Option<Position>,
    pub inset: crate::EdgesRefinement<Length>,
    pub size: crate::SizeRefinement<Length>,
    pub min_size: crate::SizeRefinement<Length>,
    pub max_size: crate::SizeRefinement<Length>,
    pub aspect_ratio: Option<f32>,
    pub margin: crate::EdgesRefinement<Length>,
    pub padding: crate::EdgesRefinement<DefiniteLength>,
    pub border_widths: crate::EdgesRefinement<AbsoluteLength>,
    pub align_items: Option<AlignItems>,
    pub align_self: Option<AlignSelf>,
    pub align_content: Option<AlignContent>,
    pub justify_content: Option<JustifyContent>,
    pub gap: crate::SizeRefinement<DefiniteLength>,
    pub flex_direction: Option<FlexDirection>,
    pub flex_wrap: Option<FlexWrap>,
    pub flex_basis: Option<Length>,
    pub flex_grow: Option<f32>,
    pub flex_shrink: Option<f32>,
    pub background: Option<Fill>,
    pub border_color: Option<Hsla>,
    pub border_style: Option<BorderStyle>,
    pub corner_radii: crate::CornersRefinement<AbsoluteLength>,
    pub box_shadow: Option<Vec<BoxShadow>>,
    pub text: TextStyleRefinement,
    pub mouse_cursor: Option<CursorStyle>,
    pub opacity: Option<f32>,
    #[cfg(debug_assertions)]
    pub debug: Option<bool>,
    #[cfg(debug_assertions)]
    pub debug_below: Option<bool>,
}

impl StyleRefinement {
    pub fn is_some(&self) -> bool {
        !self.is_empty()
    }
}

impl std::fmt::Debug for StyleRefinement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = f.debug_struct("StyleRefinement");
        if self.display.is_some() { s.field("display", &self.display); }
        if self.visibility.is_some() { s.field("visibility", &self.visibility); }
        if self.overflow.is_some() { s.field("overflow", &self.overflow); }
        if self.position.is_some() { s.field("position", &self.position); }
        if self.size.is_some() { s.field("size", &self.size); }
        if self.background.is_some() { s.field("background", &self.background); }
        if self.text.is_some() { s.field("text", &self.text); }
        s.finish_non_exhaustive()
    }
}

impl IsEmpty for StyleRefinement {
    fn is_empty(&self) -> bool {
        let base = self.display.is_none()
            && self.visibility.is_none()
            && self.overflow.is_empty()
            && self.scrollbar_width.is_none()
            && self.allow_concurrent_scroll.is_none()
            && self.restrict_scroll_to_axis.is_none()
            && self.position.is_none()
            && self.inset.is_empty()
            && self.size.is_empty()
            && self.min_size.is_empty()
            && self.max_size.is_empty()
            && self.aspect_ratio.is_none()
            && self.margin.is_empty()
            && self.padding.is_empty()
            && self.border_widths.is_empty()
            && self.align_items.is_none()
            && self.align_self.is_none()
            && self.align_content.is_none()
            && self.justify_content.is_none()
            && self.gap.is_empty()
            && self.flex_direction.is_none()
            && self.flex_wrap.is_none()
            && self.flex_basis.is_none()
            && self.flex_grow.is_none()
            && self.flex_shrink.is_none()
            && self.background.is_none()
            && self.border_color.is_none()
            && self.border_style.is_none()
            && self.corner_radii.is_empty()
            && self.box_shadow.is_none()
            && self.text.is_empty()
            && self.mouse_cursor.is_none()
            && self.opacity.is_none();
        #[cfg(debug_assertions)]
        let base = base && self.debug.is_none() && self.debug_below.is_none();
        base
    }
}

impl From<StyleRefinement> for Style {
    fn from(v: StyleRefinement) -> Self {
        Self {
            display: v.display.unwrap_or_default(),
            visibility: v.visibility.unwrap_or_default(),
            overflow: v.overflow.into(),
            scrollbar_width: v.scrollbar_width.unwrap_or_default(),
            allow_concurrent_scroll: v.allow_concurrent_scroll.unwrap_or_default(),
            restrict_scroll_to_axis: v.restrict_scroll_to_axis.unwrap_or_default(),
            position: v.position.unwrap_or_default(),
            inset: v.inset.into(),
            size: v.size.into(),
            min_size: v.min_size.into(),
            max_size: v.max_size.into(),
            aspect_ratio: v.aspect_ratio,
            margin: v.margin.into(),
            padding: v.padding.into(),
            border_widths: v.border_widths.into(),
            align_items: v.align_items,
            align_self: v.align_self,
            align_content: v.align_content,
            justify_content: v.justify_content,
            gap: v.gap.into(),
            flex_direction: v.flex_direction.unwrap_or_default(),
            flex_wrap: v.flex_wrap.unwrap_or_default(),
            flex_basis: v.flex_basis.unwrap_or_default(),
            flex_grow: v.flex_grow.unwrap_or_default(),
            flex_shrink: v.flex_shrink.unwrap_or_default(),
            background: v.background,
            border_color: v.border_color,
            border_style: v.border_style.unwrap_or_default(),
            corner_radii: v.corner_radii.into(),
            box_shadow: v.box_shadow.unwrap_or_default(),
            text: v.text,
            mouse_cursor: v.mouse_cursor,
            opacity: v.opacity,
            #[cfg(debug_assertions)]
            debug: v.debug.unwrap_or_default(),
            #[cfg(debug_assertions)]
            debug_below: v.debug_below.unwrap_or_default(),
        }
    }
}

impl Refineable for Style {
    type Refinement = StyleRefinement;
    fn refine(&mut self, r: &Self::Refinement) {
        if let Some(v) = &r.display { self.display = *v; }
        if let Some(v) = &r.visibility { self.visibility = *v; }
        self.overflow.refine(&r.overflow);
        if let Some(v) = &r.scrollbar_width { self.scrollbar_width = *v; }
        if let Some(v) = &r.allow_concurrent_scroll { self.allow_concurrent_scroll = *v; }
        if let Some(v) = &r.restrict_scroll_to_axis { self.restrict_scroll_to_axis = *v; }
        if let Some(v) = &r.position { self.position = *v; }
        self.inset.refine(&r.inset);
        self.size.refine(&r.size);
        self.min_size.refine(&r.min_size);
        self.max_size.refine(&r.max_size);
        if let Some(v) = &r.aspect_ratio { self.aspect_ratio = Some(*v); }
        self.margin.refine(&r.margin);
        self.padding.refine(&r.padding);
        self.border_widths.refine(&r.border_widths);
        if let Some(v) = &r.align_items { self.align_items = Some(*v); }
        if let Some(v) = &r.align_self { self.align_self = Some(*v); }
        if let Some(v) = &r.align_content { self.align_content = Some(*v); }
        if let Some(v) = &r.justify_content { self.justify_content = Some(*v); }
        self.gap.refine(&r.gap);
        if let Some(v) = &r.flex_direction { self.flex_direction = *v; }
        if let Some(v) = &r.flex_wrap { self.flex_wrap = *v; }
        if let Some(v) = &r.flex_basis { self.flex_basis = *v; }
        if let Some(v) = &r.flex_grow { self.flex_grow = *v; }
        if let Some(v) = &r.flex_shrink { self.flex_shrink = *v; }
        if let Some(v) = &r.background { self.background = Some(v.clone()); }
        if let Some(v) = &r.border_color { self.border_color = Some(*v); }
        if let Some(v) = &r.border_style { self.border_style = *v; }
        self.corner_radii.refine(&r.corner_radii);
        if let Some(v) = &r.box_shadow { self.box_shadow = v.clone(); }
        self.text.refine(&r.text);
        if let Some(v) = &r.mouse_cursor { self.mouse_cursor = Some(*v); }
        if let Some(v) = &r.opacity { self.opacity = Some(*v); }
        #[cfg(debug_assertions)]
        if let Some(v) = &r.debug { self.debug = *v; }
        #[cfg(debug_assertions)]
        if let Some(v) = &r.debug_below { self.debug_below = *v; }
    }
    fn refined(mut self, r: Self::Refinement) -> Self {
        self.refine(&r);
        self
    }
    fn is_superset_of(&self, r: &Self::Refinement) -> bool {
        if let Some(v) = &r.display && &self.display != v { return false; }
        if let Some(v) = &r.visibility && &self.visibility != v { return false; }
        if !self.overflow.is_superset_of(&r.overflow) { return false; }
        if let Some(v) = &r.scrollbar_width && &self.scrollbar_width != v { return false; }
        if let Some(v) = &r.allow_concurrent_scroll && &self.allow_concurrent_scroll != v { return false; }
        if let Some(v) = &r.restrict_scroll_to_axis && &self.restrict_scroll_to_axis != v { return false; }
        if let Some(v) = &r.position && &self.position != v { return false; }
        if !self.inset.is_superset_of(&r.inset) { return false; }
        if !self.size.is_superset_of(&r.size) { return false; }
        if !self.min_size.is_superset_of(&r.min_size) { return false; }
        if !self.max_size.is_superset_of(&r.max_size) { return false; }
        if r.aspect_ratio.is_some() && self.aspect_ratio != r.aspect_ratio { return false; }
        if !self.margin.is_superset_of(&r.margin) { return false; }
        if !self.padding.is_superset_of(&r.padding) { return false; }
        if !self.border_widths.is_superset_of(&r.border_widths) { return false; }
        if r.align_items.is_some() && self.align_items != r.align_items { return false; }
        if r.align_self.is_some() && self.align_self != r.align_self { return false; }
        if r.align_content.is_some() && self.align_content != r.align_content { return false; }
        if r.justify_content.is_some() && self.justify_content != r.justify_content { return false; }
        if !self.gap.is_superset_of(&r.gap) { return false; }
        if let Some(v) = &r.flex_direction && &self.flex_direction != v { return false; }
        if let Some(v) = &r.flex_wrap && &self.flex_wrap != v { return false; }
        if let Some(v) = &r.flex_basis && &self.flex_basis != v { return false; }
        if let Some(v) = &r.flex_grow && &self.flex_grow != v { return false; }
        if let Some(v) = &r.flex_shrink && &self.flex_shrink != v { return false; }
        if r.background.is_some() && self.background != r.background { return false; }
        if r.border_color.is_some() && self.border_color != r.border_color { return false; }
        if let Some(v) = &r.border_style && &self.border_style != v { return false; }
        if !self.corner_radii.is_superset_of(&r.corner_radii) { return false; }
        if let Some(v) = &r.box_shadow && &self.box_shadow != v { return false; }
        if !self.text.is_superset_of(&r.text) { return false; }
        if r.mouse_cursor.is_some() && self.mouse_cursor != r.mouse_cursor { return false; }
        if r.opacity.is_some() && self.opacity != r.opacity { return false; }
        #[cfg(debug_assertions)]
        if let Some(v) = &r.debug && &self.debug != v { return false; }
        #[cfg(debug_assertions)]
        if let Some(v) = &r.debug_below && &self.debug_below != v { return false; }
        true
    }
    fn subtract(&self, r: &Self::Refinement) -> Self::Refinement {
        StyleRefinement {
            display: if r.display == Some(self.display) { None } else { Some(self.display) },
            visibility: if r.visibility == Some(self.visibility) { None } else { Some(self.visibility) },
            overflow: self.overflow.subtract(&r.overflow),
            scrollbar_width: if r.scrollbar_width == Some(self.scrollbar_width) { None } else { Some(self.scrollbar_width) },
            allow_concurrent_scroll: if r.allow_concurrent_scroll == Some(self.allow_concurrent_scroll) { None } else { Some(self.allow_concurrent_scroll) },
            restrict_scroll_to_axis: if r.restrict_scroll_to_axis == Some(self.restrict_scroll_to_axis) { None } else { Some(self.restrict_scroll_to_axis) },
            position: if r.position == Some(self.position) { None } else { Some(self.position) },
            inset: self.inset.subtract(&r.inset),
            size: self.size.subtract(&r.size),
            min_size: self.min_size.subtract(&r.min_size),
            max_size: self.max_size.subtract(&r.max_size),
            aspect_ratio: if self.aspect_ratio == r.aspect_ratio { None } else { self.aspect_ratio },
            margin: self.margin.subtract(&r.margin),
            padding: self.padding.subtract(&r.padding),
            border_widths: self.border_widths.subtract(&r.border_widths),
            align_items: if self.align_items == r.align_items { None } else { self.align_items },
            align_self: if self.align_self == r.align_self { None } else { self.align_self },
            align_content: if self.align_content == r.align_content { None } else { self.align_content },
            justify_content: if self.justify_content == r.justify_content { None } else { self.justify_content },
            gap: self.gap.subtract(&r.gap),
            flex_direction: if r.flex_direction == Some(self.flex_direction) { None } else { Some(self.flex_direction) },
            flex_wrap: if r.flex_wrap == Some(self.flex_wrap) { None } else { Some(self.flex_wrap) },
            flex_basis: if r.flex_basis == Some(self.flex_basis) { None } else { Some(self.flex_basis) },
            flex_grow: if r.flex_grow == Some(self.flex_grow) { None } else { Some(self.flex_grow) },
            flex_shrink: if r.flex_shrink == Some(self.flex_shrink) { None } else { Some(self.flex_shrink) },
            background: if self.background == r.background { None } else { self.background.clone() },
            border_color: if self.border_color == r.border_color { None } else { self.border_color },
            border_style: if r.border_style == Some(self.border_style) { None } else { Some(self.border_style) },
            corner_radii: self.corner_radii.subtract(&r.corner_radii),
            box_shadow: if r.box_shadow.as_ref() == Some(&self.box_shadow) { None } else { Some(self.box_shadow.clone()) },
            text: self.text.subtract(&r.text),
            mouse_cursor: if self.mouse_cursor == r.mouse_cursor { None } else { self.mouse_cursor },
            opacity: if self.opacity == r.opacity { None } else { self.opacity },
            #[cfg(debug_assertions)]
            debug: if r.debug == Some(self.debug) { None } else { Some(self.debug) },
            #[cfg(debug_assertions)]
            debug_below: if r.debug_below == Some(self.debug_below) { None } else { Some(self.debug_below) },
        }
    }
}

impl Refineable for StyleRefinement {
    type Refinement = StyleRefinement;
    fn refine(&mut self, r: &Self::Refinement) {
        if let Some(v) = &r.display { self.display = Some(*v); }
        if let Some(v) = &r.visibility { self.visibility = Some(*v); }
        self.overflow.refine(&r.overflow);
        if let Some(v) = &r.scrollbar_width { self.scrollbar_width = Some(*v); }
        if let Some(v) = &r.allow_concurrent_scroll { self.allow_concurrent_scroll = Some(*v); }
        if let Some(v) = &r.restrict_scroll_to_axis { self.restrict_scroll_to_axis = Some(*v); }
        if let Some(v) = &r.position { self.position = Some(*v); }
        self.inset.refine(&r.inset);
        self.size.refine(&r.size);
        self.min_size.refine(&r.min_size);
        self.max_size.refine(&r.max_size);
        if let Some(v) = &r.aspect_ratio { self.aspect_ratio = Some(*v); }
        self.margin.refine(&r.margin);
        self.padding.refine(&r.padding);
        self.border_widths.refine(&r.border_widths);
        if let Some(v) = &r.align_items { self.align_items = Some(*v); }
        if let Some(v) = &r.align_self { self.align_self = Some(*v); }
        if let Some(v) = &r.align_content { self.align_content = Some(*v); }
        if let Some(v) = &r.justify_content { self.justify_content = Some(*v); }
        self.gap.refine(&r.gap);
        if let Some(v) = &r.flex_direction { self.flex_direction = Some(*v); }
        if let Some(v) = &r.flex_wrap { self.flex_wrap = Some(*v); }
        if let Some(v) = &r.flex_basis { self.flex_basis = Some(*v); }
        if let Some(v) = &r.flex_grow { self.flex_grow = Some(*v); }
        if let Some(v) = &r.flex_shrink { self.flex_shrink = Some(*v); }
        if let Some(v) = &r.background { self.background = Some(v.clone()); }
        if let Some(v) = &r.border_color { self.border_color = Some(*v); }
        if let Some(v) = &r.border_style { self.border_style = Some(*v); }
        self.corner_radii.refine(&r.corner_radii);
        if let Some(v) = &r.box_shadow { self.box_shadow = Some(v.clone()); }
        self.text.refine(&r.text);
        if let Some(v) = &r.mouse_cursor { self.mouse_cursor = Some(*v); }
        if let Some(v) = &r.opacity { self.opacity = Some(*v); }
        #[cfg(debug_assertions)]
        if let Some(v) = &r.debug { self.debug = Some(*v); }
        #[cfg(debug_assertions)]
        if let Some(v) = &r.debug_below { self.debug_below = Some(*v); }
    }
    fn refined(mut self, r: Self::Refinement) -> Self {
        self.refine(&r);
        self
    }
    fn is_superset_of(&self, r: &Self::Refinement) -> bool {
        if r.display.is_some() && self.display != r.display { return false; }
        if r.visibility.is_some() && self.visibility != r.visibility { return false; }
        if !self.overflow.is_superset_of(&r.overflow) { return false; }
        if r.scrollbar_width.is_some() && self.scrollbar_width != r.scrollbar_width { return false; }
        if r.allow_concurrent_scroll.is_some() && self.allow_concurrent_scroll != r.allow_concurrent_scroll { return false; }
        if r.restrict_scroll_to_axis.is_some() && self.restrict_scroll_to_axis != r.restrict_scroll_to_axis { return false; }
        if r.position.is_some() && self.position != r.position { return false; }
        if !self.inset.is_superset_of(&r.inset) { return false; }
        if !self.size.is_superset_of(&r.size) { return false; }
        if !self.min_size.is_superset_of(&r.min_size) { return false; }
        if !self.max_size.is_superset_of(&r.max_size) { return false; }
        if r.aspect_ratio.is_some() && self.aspect_ratio != r.aspect_ratio { return false; }
        if !self.margin.is_superset_of(&r.margin) { return false; }
        if !self.padding.is_superset_of(&r.padding) { return false; }
        if !self.border_widths.is_superset_of(&r.border_widths) { return false; }
        if r.align_items.is_some() && self.align_items != r.align_items { return false; }
        if r.align_self.is_some() && self.align_self != r.align_self { return false; }
        if r.align_content.is_some() && self.align_content != r.align_content { return false; }
        if r.justify_content.is_some() && self.justify_content != r.justify_content { return false; }
        if !self.gap.is_superset_of(&r.gap) { return false; }
        if r.flex_direction.is_some() && self.flex_direction != r.flex_direction { return false; }
        if r.flex_wrap.is_some() && self.flex_wrap != r.flex_wrap { return false; }
        if r.flex_basis.is_some() && self.flex_basis != r.flex_basis { return false; }
        if r.flex_grow.is_some() && self.flex_grow != r.flex_grow { return false; }
        if r.flex_shrink.is_some() && self.flex_shrink != r.flex_shrink { return false; }
        if r.background.is_some() && self.background != r.background { return false; }
        if r.border_color.is_some() && self.border_color != r.border_color { return false; }
        if r.border_style.is_some() && self.border_style != r.border_style { return false; }
        if !self.corner_radii.is_superset_of(&r.corner_radii) { return false; }
        if r.box_shadow.is_some() && self.box_shadow != r.box_shadow { return false; }
        if !self.text.is_superset_of(&r.text) { return false; }
        if r.mouse_cursor.is_some() && self.mouse_cursor != r.mouse_cursor { return false; }
        if r.opacity.is_some() && self.opacity != r.opacity { return false; }
        #[cfg(debug_assertions)]
        if r.debug.is_some() && self.debug != r.debug { return false; }
        #[cfg(debug_assertions)]
        if r.debug_below.is_some() && self.debug_below != r.debug_below { return false; }
        true
    }
    fn subtract(&self, r: &Self::Refinement) -> Self::Refinement {
        StyleRefinement {
            display: if self.display == r.display { None } else { self.display },
            visibility: if self.visibility == r.visibility { None } else { self.visibility },
            overflow: self.overflow.subtract(&r.overflow),
            scrollbar_width: if self.scrollbar_width == r.scrollbar_width { None } else { self.scrollbar_width },
            allow_concurrent_scroll: if self.allow_concurrent_scroll == r.allow_concurrent_scroll { None } else { self.allow_concurrent_scroll },
            restrict_scroll_to_axis: if self.restrict_scroll_to_axis == r.restrict_scroll_to_axis { None } else { self.restrict_scroll_to_axis },
            position: if self.position == r.position { None } else { self.position },
            inset: self.inset.subtract(&r.inset),
            size: self.size.subtract(&r.size),
            min_size: self.min_size.subtract(&r.min_size),
            max_size: self.max_size.subtract(&r.max_size),
            aspect_ratio: if self.aspect_ratio == r.aspect_ratio { None } else { self.aspect_ratio },
            margin: self.margin.subtract(&r.margin),
            padding: self.padding.subtract(&r.padding),
            border_widths: self.border_widths.subtract(&r.border_widths),
            align_items: if self.align_items == r.align_items { None } else { self.align_items },
            align_self: if self.align_self == r.align_self { None } else { self.align_self },
            align_content: if self.align_content == r.align_content { None } else { self.align_content },
            justify_content: if self.justify_content == r.justify_content { None } else { self.justify_content },
            gap: self.gap.subtract(&r.gap),
            flex_direction: if self.flex_direction == r.flex_direction { None } else { self.flex_direction },
            flex_wrap: if self.flex_wrap == r.flex_wrap { None } else { self.flex_wrap },
            flex_basis: if self.flex_basis == r.flex_basis { None } else { self.flex_basis },
            flex_grow: if self.flex_grow == r.flex_grow { None } else { self.flex_grow },
            flex_shrink: if self.flex_shrink == r.flex_shrink { None } else { self.flex_shrink },
            background: if self.background == r.background { None } else { self.background.clone() },
            border_color: if self.border_color == r.border_color { None } else { self.border_color },
            border_style: if self.border_style == r.border_style { None } else { self.border_style },
            corner_radii: self.corner_radii.subtract(&r.corner_radii),
            box_shadow: if self.box_shadow == r.box_shadow { None } else { self.box_shadow.clone() },
            text: self.text.subtract(&r.text),
            mouse_cursor: if self.mouse_cursor == r.mouse_cursor { None } else { self.mouse_cursor },
            opacity: if self.opacity == r.opacity { None } else { self.opacity },
            #[cfg(debug_assertions)]
            debug: if self.debug == r.debug { None } else { self.debug },
            #[cfg(debug_assertions)]
            debug_below: if self.debug_below == r.debug_below { None } else { self.debug_below },
        }
    }
}

impl Styled for StyleRefinement {
    fn style(&mut self) -> &mut StyleRefinement {
        self
    }
}

/// The value of the visibility property, similar to the CSS property `visibility`
#[derive(Default, Clone, Copy, Debug, Eq, PartialEq)]
pub enum Visibility {
    /// The element should be drawn as normal.
    #[default]
    Visible,
    /// The element should not be drawn, but should still take up space in the layout.
    Hidden,
}

/// The possible values of the box-shadow property
#[derive(Clone, Debug, PartialEq)]
pub struct BoxShadow {
    /// What color should the shadow have?
    pub color: Hsla,
    /// How should it be offset from its element?
    pub offset: Point<Pixels>,
    /// How much should the shadow be blurred?
    pub blur_radius: Pixels,
    /// How much should the shadow spread?
    pub spread_radius: Pixels,
    /// Whether this is an inset shadow (drawn inside the element's bounds).
    pub inset: bool,
}

impl BoxShadow {
    /// Creates a new [`BoxShadow`] with the given offset and color, matching the order
    /// of the CSS `box-shadow` property. Use the builder methods to set blur radius,
    /// spread radius, and inset.
    pub fn new(offset_x: Pixels, offset_y: Pixels, color: Hsla) -> Self {
        Self {
            color,
            offset: point(offset_x, offset_y),
            blur_radius: px(0.),
            spread_radius: px(0.),
            inset: false,
        }
    }

    /// Sets the shadow blur radius.
    pub fn blur_radius(mut self, blur_radius: Pixels) -> Self {
        self.blur_radius = blur_radius;
        self
    }

    /// Sets the shadow spread radius.
    pub fn spread_radius(mut self, spread_radius: Pixels) -> Self {
        self.spread_radius = spread_radius;
        self
    }

    /// Marks the shadow as inset (drawn inside the element's bounds).
    pub fn inset(mut self) -> Self {
        self.inset = true;
        self
    }
}

/// How to handle whitespace in text
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum WhiteSpace {
    /// Normal line wrapping when text overflows the width of the element
    #[default]
    Normal,
    /// No line wrapping, text will overflow the width of the element
    Nowrap,
}

/// How to truncate text that overflows the width of the element
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextOverflow {
    /// Truncate the text at the end when it doesn't fit, and represent this truncation by
    /// displaying the provided string (e.g., "very long te…").
    Truncate(SharedString),
    /// Truncate the text at the start when it doesn't fit, and represent this truncation by
    /// displaying the provided string at the beginning (e.g., "…ong text here").
    /// Typically more adequate for file paths where the end is more important than the beginning.
    TruncateStart(SharedString),
    /// Truncate the text in the middle when it doesn't fit, preserving both the start and end
    /// of the string (e.g., "long fi…name.rs"). Useful for filenames where both the prefix
    /// and the extension are important context.
    TruncateMiddle(SharedString),
}

/// How to align text within the element
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum TextAlign {
    /// Align the text to the left of the element
    #[default]
    Left,

    /// Center the text within the element
    Center,

    /// Align the text to the right of the element
    Right,
}

/// The properties that can be used to style text in GPUI
#[derive(Clone, Debug, PartialEq)]
pub struct TextStyle {
    /// The color of the text
    pub color: Hsla,
    /// The font family to use
    pub font_family: SharedString,
    /// The font features to use
    pub font_features: FontFeatures,
    /// The fallback fonts to use
    pub font_fallbacks: Option<FontFallbacks>,
    /// The font size to use, in pixels or rems.
    pub font_size: AbsoluteLength,
    /// The line height to use, in pixels or fractions
    pub line_height: DefiniteLength,
    /// The font weight, e.g. bold
    pub font_weight: FontWeight,
    /// The font style, e.g. italic
    pub font_style: FontStyle,
    /// The background color of the text
    pub background_color: Option<Hsla>,
    /// The underline style of the text
    pub underline: Option<UnderlineStyle>,
    /// How to handle whitespace in the text
    pub white_space: WhiteSpace,
    /// The text should be truncated if it overflows the width of the element
    pub text_overflow: Option<TextOverflow>,
    /// How the text should be aligned within the element
    pub text_align: TextAlign,
    /// The number of lines to display before truncating the text
    pub line_clamp: Option<usize>,
}

#[derive(Clone, Default, PartialEq)]
pub struct TextStyleRefinement {
    pub color: Option<Hsla>,
    pub font_family: Option<SharedString>,
    pub font_features: Option<FontFeatures>,
    pub font_fallbacks: Option<FontFallbacks>,
    pub font_size: Option<AbsoluteLength>,
    pub line_height: Option<DefiniteLength>,
    pub font_weight: Option<FontWeight>,
    pub font_style: Option<FontStyle>,
    pub background_color: Option<Hsla>,
    pub underline: Option<UnderlineStyle>,
    pub white_space: Option<WhiteSpace>,
    pub text_overflow: Option<TextOverflow>,
    pub text_align: Option<TextAlign>,
    pub line_clamp: Option<usize>,
}

impl TextStyleRefinement {
    pub fn is_some(&self) -> bool {
        !self.is_empty()
    }
}

impl std::fmt::Debug for TextStyleRefinement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = f.debug_struct("TextStyleRefinement");
        if self.color.is_some() { s.field("color", &self.color); }
        if self.font_family.is_some() { s.field("font_family", &self.font_family); }
        if self.font_size.is_some() { s.field("font_size", &self.font_size); }
        if self.font_weight.is_some() { s.field("font_weight", &self.font_weight); }
        s.finish_non_exhaustive()
    }
}

impl IsEmpty for TextStyleRefinement {
    fn is_empty(&self) -> bool {
        self.color.is_none()
            && self.font_family.is_none()
            && self.font_features.is_none()
            && self.font_fallbacks.is_none()
            && self.font_size.is_none()
            && self.line_height.is_none()
            && self.font_weight.is_none()
            && self.font_style.is_none()
            && self.background_color.is_none()
            && self.underline.is_none()
            && self.white_space.is_none()
            && self.text_overflow.is_none()
            && self.text_align.is_none()
            && self.line_clamp.is_none()
    }
}

impl From<TextStyleRefinement> for TextStyle {
    fn from(v: TextStyleRefinement) -> Self {
        Self {
            color: v.color.unwrap_or_default(),
            font_family: v.font_family.unwrap_or_default(),
            font_features: v.font_features.unwrap_or_default(),
            font_fallbacks: v.font_fallbacks,
            font_size: v.font_size.unwrap_or_default(),
            line_height: v.line_height.unwrap_or_default(),
            font_weight: v.font_weight.unwrap_or_default(),
            font_style: v.font_style.unwrap_or_default(),
            background_color: v.background_color,
            underline: v.underline,
            white_space: v.white_space.unwrap_or_default(),
            text_overflow: v.text_overflow,
            text_align: v.text_align.unwrap_or_default(),
            line_clamp: v.line_clamp,
        }
    }
}

impl Refineable for TextStyle {
    type Refinement = TextStyleRefinement;
    fn refine(&mut self, r: &Self::Refinement) {
        if let Some(v) = &r.color { self.color = *v; }
        if let Some(v) = &r.font_family { self.font_family = v.clone(); }
        if let Some(v) = &r.font_features { self.font_features = v.clone(); }
        if let Some(v) = &r.font_fallbacks { self.font_fallbacks = Some(v.clone()); }
        if let Some(v) = &r.font_size { self.font_size = *v; }
        if let Some(v) = &r.line_height { self.line_height = *v; }
        if let Some(v) = &r.font_weight { self.font_weight = *v; }
        if let Some(v) = &r.font_style { self.font_style = *v; }
        if let Some(v) = &r.background_color { self.background_color = Some(*v); }
        if let Some(v) = &r.underline { self.underline = Some(*v); }
        if let Some(v) = &r.white_space { self.white_space = *v; }
        if let Some(v) = &r.text_overflow { self.text_overflow = Some(v.clone()); }
        if let Some(v) = &r.text_align { self.text_align = *v; }
        if let Some(v) = &r.line_clamp { self.line_clamp = Some(*v); }
    }
    fn refined(mut self, r: Self::Refinement) -> Self {
        self.refine(&r);
        self
    }
    fn is_superset_of(&self, r: &Self::Refinement) -> bool {
        if let Some(v) = &r.color && &self.color != v { return false; }
        if let Some(v) = &r.font_family && &self.font_family != v { return false; }
        if let Some(v) = &r.font_features && &self.font_features != v { return false; }
        if r.font_fallbacks.is_some() && self.font_fallbacks != r.font_fallbacks { return false; }
        if let Some(v) = &r.font_size && &self.font_size != v { return false; }
        if let Some(v) = &r.line_height && &self.line_height != v { return false; }
        if let Some(v) = &r.font_weight && &self.font_weight != v { return false; }
        if let Some(v) = &r.font_style && &self.font_style != v { return false; }
        if r.background_color.is_some() && self.background_color != r.background_color { return false; }
        if r.underline.is_some() && self.underline != r.underline { return false; }
        if let Some(v) = &r.white_space && &self.white_space != v { return false; }
        if r.text_overflow.is_some() && self.text_overflow != r.text_overflow { return false; }
        if let Some(v) = &r.text_align && &self.text_align != v { return false; }
        if r.line_clamp.is_some() && self.line_clamp != r.line_clamp { return false; }
        true
    }
    fn subtract(&self, r: &Self::Refinement) -> Self::Refinement {
        TextStyleRefinement {
            color: if r.color == Some(self.color) { None } else { Some(self.color) },
            font_family: if r.font_family.as_ref() == Some(&self.font_family) { None } else { Some(self.font_family.clone()) },
            font_features: if r.font_features.as_ref() == Some(&self.font_features) { None } else { Some(self.font_features.clone()) },
            font_fallbacks: if self.font_fallbacks == r.font_fallbacks { None } else { self.font_fallbacks.clone() },
            font_size: if r.font_size == Some(self.font_size) { None } else { Some(self.font_size) },
            line_height: if r.line_height == Some(self.line_height) { None } else { Some(self.line_height) },
            font_weight: if r.font_weight == Some(self.font_weight) { None } else { Some(self.font_weight) },
            font_style: if r.font_style == Some(self.font_style) { None } else { Some(self.font_style) },
            background_color: if self.background_color == r.background_color { None } else { self.background_color },
            underline: if self.underline == r.underline { None } else { self.underline },
            white_space: if r.white_space == Some(self.white_space) { None } else { Some(self.white_space) },
            text_overflow: if self.text_overflow == r.text_overflow { None } else { self.text_overflow.clone() },
            text_align: if r.text_align == Some(self.text_align) { None } else { Some(self.text_align) },
            line_clamp: if self.line_clamp == r.line_clamp { None } else { self.line_clamp },
        }
    }
}

impl Refineable for TextStyleRefinement {
    type Refinement = TextStyleRefinement;
    fn refine(&mut self, r: &Self::Refinement) {
        if let Some(v) = &r.color { self.color = Some(*v); }
        if let Some(v) = &r.font_family { self.font_family = Some(v.clone()); }
        if let Some(v) = &r.font_features { self.font_features = Some(v.clone()); }
        if let Some(v) = &r.font_fallbacks { self.font_fallbacks = Some(v.clone()); }
        if let Some(v) = &r.font_size { self.font_size = Some(*v); }
        if let Some(v) = &r.line_height { self.line_height = Some(*v); }
        if let Some(v) = &r.font_weight { self.font_weight = Some(*v); }
        if let Some(v) = &r.font_style { self.font_style = Some(*v); }
        if let Some(v) = &r.background_color { self.background_color = Some(*v); }
        if let Some(v) = &r.underline { self.underline = Some(*v); }
        if let Some(v) = &r.white_space { self.white_space = Some(*v); }
        if let Some(v) = &r.text_overflow { self.text_overflow = Some(v.clone()); }
        if let Some(v) = &r.text_align { self.text_align = Some(*v); }
        if let Some(v) = &r.line_clamp { self.line_clamp = Some(*v); }
    }
    fn refined(mut self, r: Self::Refinement) -> Self {
        self.refine(&r);
        self
    }
    fn is_superset_of(&self, r: &Self::Refinement) -> bool {
        if r.color.is_some() && self.color != r.color { return false; }
        if r.font_family.is_some() && self.font_family != r.font_family { return false; }
        if r.font_features.is_some() && self.font_features != r.font_features { return false; }
        if r.font_fallbacks.is_some() && self.font_fallbacks != r.font_fallbacks { return false; }
        if r.font_size.is_some() && self.font_size != r.font_size { return false; }
        if r.line_height.is_some() && self.line_height != r.line_height { return false; }
        if r.font_weight.is_some() && self.font_weight != r.font_weight { return false; }
        if r.font_style.is_some() && self.font_style != r.font_style { return false; }
        if r.background_color.is_some() && self.background_color != r.background_color { return false; }
        if r.underline.is_some() && self.underline != r.underline { return false; }
        if r.white_space.is_some() && self.white_space != r.white_space { return false; }
        if r.text_overflow.is_some() && self.text_overflow != r.text_overflow { return false; }
        if r.text_align.is_some() && self.text_align != r.text_align { return false; }
        if r.line_clamp.is_some() && self.line_clamp != r.line_clamp { return false; }
        true
    }
    fn subtract(&self, r: &Self::Refinement) -> Self::Refinement {
        TextStyleRefinement {
            color: if self.color == r.color { None } else { self.color },
            font_family: if self.font_family == r.font_family { None } else { self.font_family.clone() },
            font_features: if self.font_features == r.font_features { None } else { self.font_features.clone() },
            font_fallbacks: if self.font_fallbacks == r.font_fallbacks { None } else { self.font_fallbacks.clone() },
            font_size: if self.font_size == r.font_size { None } else { self.font_size },
            line_height: if self.line_height == r.line_height { None } else { self.line_height },
            font_weight: if self.font_weight == r.font_weight { None } else { self.font_weight },
            font_style: if self.font_style == r.font_style { None } else { self.font_style },
            background_color: if self.background_color == r.background_color { None } else { self.background_color },
            underline: if self.underline == r.underline { None } else { self.underline },
            white_space: if self.white_space == r.white_space { None } else { self.white_space },
            text_overflow: if self.text_overflow == r.text_overflow { None } else { self.text_overflow.clone() },
            text_align: if self.text_align == r.text_align { None } else { self.text_align },
            line_clamp: if self.line_clamp == r.line_clamp { None } else { self.line_clamp },
        }
    }
}

impl Default for TextStyle {
    fn default() -> Self {
        TextStyle {
            color: black(),
            // todo(linux) make this configurable or choose better default
            font_family: ".SystemUIFont".into(),
            font_features: FontFeatures::default(),
            font_fallbacks: None,
            font_size: rems(1.).into(),
            line_height: phi(),
            font_weight: FontWeight::default(),
            font_style: FontStyle::default(),
            background_color: None,
            underline: None,
            white_space: WhiteSpace::Normal,
            text_overflow: None,
            text_align: TextAlign::default(),
            line_clamp: None,
        }
    }
}

impl TextStyle {
    /// Create a new text style with the given highlighting applied.
    pub fn highlight(mut self, style: impl Into<HighlightStyle>) -> Self {
        let style = style.into();
        if let Some(weight) = style.font_weight {
            self.font_weight = weight;
        }
        if let Some(style) = style.font_style {
            self.font_style = style;
        }

        if let Some(color) = style.color {
            self.color = self.color.blend(color);
        }

        if let Some(factor) = style.fade_out {
            self.color.fade_out(factor);
        }

        if let Some(background_color) = style.background_color {
            self.background_color = Some(background_color);
        }

        if let Some(underline) = style.underline {
            self.underline = Some(underline);
        }

        self
    }

    /// Get the font configured for this text style.
    pub fn font(&self) -> Font {
        Font {
            family: self.font_family.clone(),
            features: self.font_features.clone(),
            fallbacks: self.font_fallbacks.clone(),
            weight: self.font_weight,
            style: self.font_style,
        }
    }

    /// Returns the rounded line height in pixels.
    pub fn line_height_in_pixels(&self, rem_size: Pixels) -> Pixels {
        self.line_height.to_pixels(self.font_size, rem_size).round()
    }

    /// Convert this text style into a [`TextRun`], for the given length of the text.
    pub fn to_run(&self, len: usize) -> TextRun {
        TextRun {
            len,
            font: Font {
                family: self.font_family.clone(),
                features: self.font_features.clone(),
                fallbacks: self.font_fallbacks.clone(),
                weight: self.font_weight,
                style: self.font_style,
            },
            color: self.color,
            background_color: self.background_color,
            underline: self.underline,
        }
    }
}

/// A highlight style to apply, similar to a `TextStyle` except
/// for a single font, uniformly sized and spaced text.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct HighlightStyle {
    /// The color of the text
    pub color: Option<Hsla>,

    /// The font weight, e.g. bold
    pub font_weight: Option<FontWeight>,

    /// The font style, e.g. italic
    pub font_style: Option<FontStyle>,

    /// The background color of the text
    pub background_color: Option<Hsla>,

    /// The underline style of the text
    pub underline: Option<UnderlineStyle>,

    /// Similar to the CSS `opacity` property, this will cause the text to be less vibrant.
    pub fade_out: Option<f32>,
}

impl Eq for HighlightStyle {}

impl Hash for HighlightStyle {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.color.hash(state);
        self.font_weight.hash(state);
        self.font_style.hash(state);
        self.background_color.hash(state);
        self.underline.hash(state);
        state.write_u32(u32::from_be_bytes(
            self.fade_out.map(|f| f.to_be_bytes()).unwrap_or_default(),
        ));
    }
}

impl Style {
    /// Returns true if the style is visible and the background is opaque.
    pub fn has_opaque_background(&self) -> bool {
        self.background
            .as_ref()
            .is_some_and(|fill| fill.color().is_some_and(|color| !color.is_transparent()))
    }

    /// Get the text style in this element style.
    pub fn text_style(&self) -> Option<&TextStyleRefinement> {
        if self.text.is_some() {
            Some(&self.text)
        } else {
            None
        }
    }

    /// Get the content mask for this element style, based on the given bounds.
    /// If the element does not hide its overflow, this will return `None`.
    pub fn overflow_mask(
        &self,
        bounds: Bounds<Pixels>,
        rem_size: Pixels,
    ) -> Option<ContentMask<Pixels>> {
        match self.overflow {
            Point {
                x: Overflow::Visible,
                y: Overflow::Visible,
            } => None,
            _ => {
                let mut min = bounds.origin;
                let mut max = bounds.bottom_right();

                if self
                    .border_color
                    .is_some_and(|color| !color.is_transparent())
                {
                    min.x += self.border_widths.left.to_pixels(rem_size);
                    max.x -= self.border_widths.right.to_pixels(rem_size);
                    min.y += self.border_widths.top.to_pixels(rem_size);
                    max.y -= self.border_widths.bottom.to_pixels(rem_size);
                }

                let bounds = match (
                    self.overflow.x == Overflow::Visible,
                    self.overflow.y == Overflow::Visible,
                ) {
                    // x and y both visible
                    (true, true) => return None,
                    // x visible, y hidden
                    (true, false) => Bounds::from_corners(
                        point(min.x, bounds.origin.y),
                        point(max.x, bounds.bottom_right().y),
                    ),
                    // x hidden, y visible
                    (false, true) => Bounds::from_corners(
                        point(bounds.origin.x, min.y),
                        point(bounds.bottom_right().x, max.y),
                    ),
                    // both hidden
                    (false, false) => Bounds::from_corners(min, max),
                };

                Some(ContentMask { bounds })
            }
        }
    }

    /// Paints the background of an element styled with this style.
    pub fn paint(
        &self,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut App,
        continuation: impl FnOnce(&mut Window, &mut App),
    ) {
        #[cfg(debug_assertions)]
        if self.debug_below {
            cx.set_global(DebugBelow)
        }

        #[cfg(debug_assertions)]
        if self.debug || cx.has_global::<DebugBelow>() {
            window.paint_quad(crate::outline(bounds, crate::red(), BorderStyle::default()));
        }

        let rem_size = window.rem_size();
        let corner_radii = self
            .corner_radii
            .to_pixels(rem_size)
            .clamp_radii_for_quad_size(bounds.size);

        window.paint_drop_shadows(bounds, corner_radii, &self.box_shadow);

        let background_color = self.background.as_ref().and_then(Fill::color);
        if background_color.is_some_and(|color| !color.is_transparent()) {
            let mut border_color = background_color.map(|color| color.0).unwrap_or_default();
            border_color.a = 0.;
            window.paint_quad(quad(
                bounds,
                corner_radii,
                background_color.unwrap_or_default(),
                Edges::default(),
                border_color,
                self.border_style,
            ));
        }

        window.paint_inset_shadows(bounds, corner_radii, &self.box_shadow);

        continuation(window, cx);

        if self.is_border_visible() {
            let border_widths = self.border_widths.to_pixels(rem_size);
            let mut background = self.border_color.unwrap_or_default();
            background.a = 0.;
            window.paint_quad(quad(
                bounds,
                corner_radii,
                background,
                border_widths,
                self.border_color.unwrap_or_default(),
                self.border_style,
            ));
        }

        #[cfg(debug_assertions)]
        if self.debug_below {
            cx.remove_global::<DebugBelow>();
        }
    }

    fn is_border_visible(&self) -> bool {
        self.border_color
            .is_some_and(|color| !color.is_transparent())
            && self.border_widths.any(|length| !length.is_zero())
    }
}

impl Default for Style {
    fn default() -> Self {
        Style {
            display: Display::Block,
            visibility: Visibility::Visible,
            overflow: Point {
                x: Overflow::Visible,
                y: Overflow::Visible,
            },
            allow_concurrent_scroll: false,
            restrict_scroll_to_axis: false,
            scrollbar_width: AbsoluteLength::default(),
            position: Position::Relative,
            inset: Edges::auto(),
            margin: Edges::<Length>::zero(),
            padding: Edges::<DefiniteLength>::zero(),
            border_widths: Edges::<AbsoluteLength>::zero(),
            size: Size::auto(),
            min_size: Size::auto(),
            max_size: Size::auto(),
            aspect_ratio: None,
            gap: Size::default(),
            // Alignment
            align_items: None,
            align_self: None,
            align_content: None,
            justify_content: None,
            // Flexbox
            flex_direction: FlexDirection::Row,
            flex_wrap: FlexWrap::NoWrap,
            flex_grow: 0.0,
            flex_shrink: 1.0,
            flex_basis: Length::Auto,
            background: None,
            border_color: None,
            border_style: BorderStyle::default(),
            corner_radii: Corners::default(),
            box_shadow: Default::default(),
            text: TextStyleRefinement::default(),
            mouse_cursor: None,
            opacity: None,

            #[cfg(debug_assertions)]
            debug: false,
            #[cfg(debug_assertions)]
            debug_below: false,
        }
    }
}

/// The properties that can be applied to an underline.
#[derive(Copy, Clone, Default, Debug, PartialEq, Eq, Hash)]
pub struct UnderlineStyle {
    /// The thickness of the underline.
    pub thickness: Pixels,
    /// The color of the underline.
    pub color: Option<Hsla>,
}

#[derive(Clone, Default)]
pub struct UnderlineStyleRefinement {
    pub thickness: Option<Pixels>,
    pub color: Option<Hsla>,
}

impl UnderlineStyleRefinement {
    pub fn is_some(&self) -> bool {
        self.thickness.is_some() || self.color.is_some()
    }
}

impl IsEmpty for UnderlineStyleRefinement {
    fn is_empty(&self) -> bool {
        self.thickness.is_none() && self.color.is_none()
    }
}

impl From<UnderlineStyleRefinement> for UnderlineStyle {
    fn from(v: UnderlineStyleRefinement) -> Self {
        Self {
            thickness: v.thickness.unwrap_or_default(),
            color: v.color,
        }
    }
}

impl Refineable for UnderlineStyle {
    type Refinement = UnderlineStyleRefinement;
    fn refine(&mut self, r: &Self::Refinement) {
        if let Some(v) = r.thickness { self.thickness = v; }
        if let Some(v) = r.color { self.color = Some(v); }
    }
    fn refined(mut self, r: Self::Refinement) -> Self { self.refine(&r); self }
    fn is_superset_of(&self, r: &Self::Refinement) -> bool {
        if let Some(v) = r.thickness && self.thickness != v { return false; }
        if r.color.is_some() && self.color != r.color { return false; }
        true
    }
    fn subtract(&self, r: &Self::Refinement) -> Self::Refinement {
        UnderlineStyleRefinement {
            thickness: if r.thickness == Some(self.thickness) { None } else { Some(self.thickness) },
            color: if self.color == r.color { None } else { self.color },
        }
    }
}

impl Refineable for UnderlineStyleRefinement {
    type Refinement = UnderlineStyleRefinement;
    fn refine(&mut self, r: &Self::Refinement) {
        if let Some(v) = r.thickness { self.thickness = Some(v); }
        if let Some(v) = r.color { self.color = Some(v); }
    }
    fn refined(mut self, r: Self::Refinement) -> Self { self.refine(&r); self }
    fn is_superset_of(&self, r: &Self::Refinement) -> bool {
        if r.thickness.is_some() && self.thickness != r.thickness { return false; }
        if r.color.is_some() && self.color != r.color { return false; }
        true
    }
    fn subtract(&self, r: &Self::Refinement) -> Self::Refinement {
        UnderlineStyleRefinement {
            thickness: if self.thickness == r.thickness { None } else { self.thickness },
            color: if self.color == r.color { None } else { self.color },
        }
    }
}

/// The kinds of fill that can be applied to a shape.
#[derive(Clone, Debug, PartialEq)]
pub enum Fill {
    /// A solid color fill.
    Color(Background),
}

impl Fill {
    /// Unwrap this fill into a solid color, if it is one.
    ///
    /// If the fill is not a solid color, this method returns `None`.
    pub fn color(&self) -> Option<Background> {
        match self {
            Fill::Color(color) => Some(*color),
        }
    }
}

impl Default for Fill {
    fn default() -> Self {
        Self::Color(Background::default())
    }
}

impl From<Hsla> for Fill {
    fn from(color: Hsla) -> Self {
        Self::Color(color.into())
    }
}

impl From<Rgba> for Fill {
    fn from(color: Rgba) -> Self {
        Self::Color(color.into())
    }
}

impl From<Background> for Fill {
    fn from(background: Background) -> Self {
        Self::Color(background)
    }
}

impl From<TextStyle> for HighlightStyle {
    fn from(other: TextStyle) -> Self {
        Self::from(&other)
    }
}

impl From<&TextStyle> for HighlightStyle {
    fn from(other: &TextStyle) -> Self {
        Self {
            color: Some(other.color),
            font_weight: Some(other.font_weight),
            font_style: Some(other.font_style),
            background_color: other.background_color,
            underline: other.underline,
            fade_out: None,
        }
    }
}

impl HighlightStyle {
    /// Create a highlight style with just a color
    pub fn color(color: Hsla) -> Self {
        Self {
            color: Some(color),
            ..Default::default()
        }
    }
    /// Blend this highlight style with another.
    /// Non-continuous properties, like font_weight and font_style, are overwritten.
    #[must_use]
    pub fn highlight(self, other: HighlightStyle) -> Self {
        Self {
            color: other
                .color
                .map(|other_color| {
                    if let Some(color) = self.color {
                        color.blend(other_color)
                    } else {
                        other_color
                    }
                })
                .or(self.color),
            font_weight: other.font_weight.or(self.font_weight),
            font_style: other.font_style.or(self.font_style),
            background_color: other.background_color.or(self.background_color),
            underline: other.underline.or(self.underline),
            fade_out: other
                .fade_out
                .map(|source_fade| {
                    self.fade_out
                        .map(|dest_fade| (dest_fade * (1. + source_fade)).clamp(0., 1.))
                        .unwrap_or(source_fade)
                })
                .or(self.fade_out),
        }
    }
}

impl From<Hsla> for HighlightStyle {
    fn from(color: Hsla) -> Self {
        Self {
            color: Some(color),
            ..Default::default()
        }
    }
}

impl From<FontWeight> for HighlightStyle {
    fn from(font_weight: FontWeight) -> Self {
        Self {
            font_weight: Some(font_weight),
            ..Default::default()
        }
    }
}

impl From<FontStyle> for HighlightStyle {
    fn from(font_style: FontStyle) -> Self {
        Self {
            font_style: Some(font_style),
            ..Default::default()
        }
    }
}

impl From<Rgba> for HighlightStyle {
    fn from(color: Rgba) -> Self {
        Self {
            color: Some(color.into()),
            ..Default::default()
        }
    }
}

/// Combine and merge the highlights and ranges in the two iterators.
pub fn combine_highlights(
    a: impl IntoIterator<Item = (Range<usize>, HighlightStyle)>,
    b: impl IntoIterator<Item = (Range<usize>, HighlightStyle)>,
) -> impl Iterator<Item = (Range<usize>, HighlightStyle)> {
    let mut endpoints = Vec::new();
    let mut highlights = Vec::new();
    for (range, highlight) in a.into_iter().chain(b) {
        if !range.is_empty() {
            let highlight_id = highlights.len();
            endpoints.push((range.start, highlight_id, true));
            endpoints.push((range.end, highlight_id, false));
            highlights.push(highlight);
        }
    }
    endpoints.sort_unstable_by_key(|(position, _, _)| *position);
    let mut endpoints = endpoints.into_iter().peekable();

    let mut active_styles = HashSet::default();
    let mut ix = 0;
    iter::from_fn(move || {
        while let Some((endpoint_ix, highlight_id, is_start)) = endpoints.peek() {
            let prev_index = mem::replace(&mut ix, *endpoint_ix);
            if ix > prev_index && !active_styles.is_empty() {
                let current_style = active_styles
                    .iter()
                    .fold(HighlightStyle::default(), |acc, highlight_id| {
                        acc.highlight(highlights[*highlight_id])
                    });
                return Some((prev_index..ix, current_style));
            }

            if *is_start {
                active_styles.insert(*highlight_id);
            } else {
                active_styles.remove(highlight_id);
            }
            endpoints.next();
        }
        None
    })
}

/// Used to control how child nodes are aligned.
/// For Flexbox it controls alignment in the cross axis
/// For Grid it controls alignment in the block axis
///
/// [MDN](https://developer.mozilla.org/en-US/docs/Web/CSS/align-items)
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
// Copy of taffy::style type of the same name.
pub enum AlignItems {
    /// Items are packed toward the start of the axis
    Start,
    /// Items are packed toward the end of the axis
    End,
    /// Items are packed towards the flex-relative start of the axis.
    ///
    /// For flex containers with flex_direction RowReverse or ColumnReverse this is equivalent
    /// to End. In all other cases it is equivalent to Start.
    FlexStart,
    /// Items are packed towards the flex-relative end of the axis.
    ///
    /// For flex containers with flex_direction RowReverse or ColumnReverse this is equivalent
    /// to Start. In all other cases it is equivalent to End.
    FlexEnd,
    /// Items are packed along the center of the cross axis
    Center,
    /// Items are aligned such as their baselines align
    Baseline,
    /// Stretch to fill the container
    Stretch,
}
/// Used to control how child nodes are aligned.
/// Does not apply to Flexbox, and will be ignored if specified on a flex container
/// For Grid it controls alignment in the inline axis
///
/// [MDN](https://developer.mozilla.org/en-US/docs/Web/CSS/justify-items)
pub type JustifyItems = AlignItems;
/// Used to control how the specified nodes is aligned.
/// Overrides the parent Node's `AlignItems` property.
/// For Flexbox it controls alignment in the cross axis
/// For Grid it controls alignment in the block axis
///
/// [MDN](https://developer.mozilla.org/en-US/docs/Web/CSS/align-self)
pub type AlignSelf = AlignItems;
/// Used to control how the specified nodes is aligned.
/// Overrides the parent Node's `JustifyItems` property.
/// Does not apply to Flexbox, and will be ignored if specified on a flex child
/// For Grid it controls alignment in the inline axis
///
/// [MDN](https://developer.mozilla.org/en-US/docs/Web/CSS/justify-self)
pub type JustifySelf = AlignItems;

/// Sets the distribution of space between and around content items
/// For Flexbox it controls alignment in the cross axis
/// For Grid it controls alignment in the block axis
///
/// [MDN](https://developer.mozilla.org/en-US/docs/Web/CSS/align-content)
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
// Copy of taffy::style type of the same name.
pub enum AlignContent {
    /// Items are packed toward the start of the axis
    Start,
    /// Items are packed toward the end of the axis
    End,
    /// Items are packed towards the flex-relative start of the axis.
    ///
    /// For flex containers with flex_direction RowReverse or ColumnReverse this is equivalent
    /// to End. In all other cases it is equivalent to Start.
    FlexStart,
    /// Items are packed towards the flex-relative end of the axis.
    ///
    /// For flex containers with flex_direction RowReverse or ColumnReverse this is equivalent
    /// to Start. In all other cases it is equivalent to End.
    FlexEnd,
    /// Items are centered around the middle of the axis
    Center,
    /// Items are stretched to fill the container
    Stretch,
    /// The first and last items are aligned flush with the edges of the container (no gap)
    /// The gap between items is distributed evenly.
    SpaceBetween,
    /// The gap between the first and last items is exactly THE SAME as the gap between items.
    /// The gaps are distributed evenly
    SpaceEvenly,
    /// The gap between the first and last items is exactly HALF the gap between items.
    /// The gaps are distributed evenly in proportion to these ratios.
    SpaceAround,
}

/// Sets the distribution of space between and around content items
/// For Flexbox it controls alignment in the main axis
/// For Grid it controls alignment in the inline axis
///
/// [MDN](https://developer.mozilla.org/en-US/docs/Web/CSS/justify-content)
pub type JustifyContent = AlignContent;

/// Sets the layout used for the children of this node
///
/// The default values depends on on which feature flags are enabled. The order of precedence is: Flex, Grid, Block, None.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
// Copy of taffy::style type of the same name.
pub enum Display {
    /// The children will follow the block layout algorithm
    Block,
    /// The children will follow the flexbox layout algorithm
    #[default]
    Flex,
    /// The children will not be laid out, and will follow absolute positioning
    None,
}

/// Controls whether flex items are forced onto one line or can wrap onto multiple lines.
///
/// Defaults to [`FlexWrap::NoWrap`]
///
/// [Specification](https://www.w3.org/TR/css-flexbox-1/#flex-wrap-property)
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
// Copy of taffy::style type of the same name.
pub enum FlexWrap {
    /// Items will not wrap and stay on a single line
    #[default]
    NoWrap,
    /// Items will wrap according to this item's [`FlexDirection`]
    Wrap,
    /// Items will wrap in the opposite direction to this item's [`FlexDirection`]
    WrapReverse,
}

/// The direction of the flexbox layout main axis.
///
/// There are always two perpendicular layout axes: main (or primary) and cross (or secondary).
/// Adding items will cause them to be positioned adjacent to each other along the main axis.
/// By varying this value throughout your tree, you can create complex axis-aligned layouts.
///
/// Items are always aligned relative to the cross axis, and justified relative to the main axis.
///
/// The default behavior is [`FlexDirection::Row`].
///
/// [Specification](https://www.w3.org/TR/css-flexbox-1/#flex-direction-property)
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
// Copy of taffy::style type of the same name.
pub enum FlexDirection {
    /// Defines +x as the main axis
    ///
    /// Items will be added from left to right in a row.
    #[default]
    Row,
    /// Defines +y as the main axis
    ///
    /// Items will be added from top to bottom in a column.
    Column,
    /// Defines -x as the main axis
    ///
    /// Items will be added from right to left in a row.
    RowReverse,
    /// Defines -y as the main axis
    ///
    /// Items will be added from bottom to top in a column.
    ColumnReverse,
}

/// How children overflowing their container should affect layout
///
/// In CSS the primary effect of this property is to control whether contents of a parent container that overflow that container should
/// be displayed anyway, be clipped, or trigger the container to become a scroll container. However it also has secondary effects on layout,
/// the main ones being:
///
///   - The automatic minimum size Flexbox/CSS Grid items with non-`Visible` overflow is `0` rather than being content based
///   - `Overflow::Scroll` nodes have space in the layout reserved for a scrollbar (width controlled by the `scrollbar_width` property)
///
/// In Taffy, we only implement the layout related secondary effects as we are not concerned with drawing/painting. The amount of space reserved for
/// a scrollbar is controlled by the `scrollbar_width` property. If this is `0` then `Scroll` behaves identically to `Hidden`.
///
/// <https://developer.mozilla.org/en-US/docs/Web/CSS/overflow>
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
// Copy of taffy::style type of the same name.
pub enum Overflow {
    /// The automatic minimum size of this node as a flexbox/grid item should be based on the size of its content.
    /// Content that overflows this node *should* contribute to the scroll region of its parent.
    #[default]
    Visible,
    /// The automatic minimum size of this node as a flexbox/grid item should be based on the size of its content.
    /// Content that overflows this node should *not* contribute to the scroll region of its parent.
    Clip,
    /// The automatic minimum size of this node as a flexbox/grid item should be `0`.
    /// Content that overflows this node should *not* contribute to the scroll region of its parent.
    Hidden,
    /// The automatic minimum size of this node as a flexbox/grid item should be `0`. Additionally, space should be reserved
    /// for a scrollbar. The amount of space reserved is controlled by the `scrollbar_width` property.
    /// Content that overflows this node should *not* contribute to the scroll region of its parent.
    Scroll,
}

/// The positioning strategy for this item.
///
/// This controls both how the origin is determined for the [`Style::position`] field,
/// and whether or not the item will be controlled by flexbox's layout algorithm.
///
/// WARNING: this enum follows the behavior of [CSS's `position` property](https://developer.mozilla.org/en-US/docs/Web/CSS/position),
/// which can be unintuitive.
///
/// [`Position::Relative`] is the default value, in contrast to the default behavior in CSS.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
// Copy of taffy::style type of the same name.
pub enum Position {
    /// The offset is computed relative to the final position given by the layout algorithm.
    /// Offsets do not affect the position of any other items; they are effectively a correction factor applied at the end.
    #[default]
    Relative,
    /// The offset is computed relative to this item's closest positioned ancestor, if any.
    /// Otherwise, it is placed relative to the origin.
    /// No space is created for the item in the page layout, and its size will not be altered.
    ///
    /// WARNING: to opt-out of layouting entirely, you must use [`Display::None`] instead on your [`Style`] object.
    Absolute,
}

impl From<AlignItems> for taffy::style::AlignItems {
    fn from(value: AlignItems) -> Self {
        match value {
            AlignItems::Start => Self::START,
            AlignItems::End => Self::END,
            AlignItems::FlexStart => Self::FLEX_START,
            AlignItems::FlexEnd => Self::FLEX_END,
            AlignItems::Center => Self::CENTER,
            AlignItems::Baseline => Self::BASELINE,
            AlignItems::Stretch => Self::STRETCH,
        }
    }
}

impl From<AlignContent> for taffy::style::AlignContent {
    fn from(value: AlignContent) -> Self {
        match value {
            AlignContent::Start => Self::START,
            AlignContent::End => Self::END,
            AlignContent::FlexStart => Self::FLEX_START,
            AlignContent::FlexEnd => Self::FLEX_END,
            AlignContent::Center => Self::CENTER,
            AlignContent::Stretch => Self::STRETCH,
            AlignContent::SpaceBetween => Self::SPACE_BETWEEN,
            AlignContent::SpaceEvenly => Self::SPACE_EVENLY,
            AlignContent::SpaceAround => Self::SPACE_AROUND,
        }
    }
}

impl From<Display> for taffy::style::Display {
    fn from(value: Display) -> Self {
        match value {
            Display::Block => Self::Block,
            Display::Flex => Self::Flex,
            Display::None => Self::None,
        }
    }
}

impl From<FlexWrap> for taffy::style::FlexWrap {
    fn from(value: FlexWrap) -> Self {
        match value {
            FlexWrap::NoWrap => Self::NoWrap,
            FlexWrap::Wrap => Self::Wrap,
            FlexWrap::WrapReverse => Self::WrapReverse,
        }
    }
}

impl From<FlexDirection> for taffy::style::FlexDirection {
    fn from(value: FlexDirection) -> Self {
        match value {
            FlexDirection::Row => Self::Row,
            FlexDirection::Column => Self::Column,
            FlexDirection::RowReverse => Self::RowReverse,
            FlexDirection::ColumnReverse => Self::ColumnReverse,
        }
    }
}

impl From<Overflow> for taffy::style::Overflow {
    fn from(value: Overflow) -> Self {
        match value {
            Overflow::Visible => Self::Visible,
            Overflow::Clip => Self::Clip,
            Overflow::Hidden => Self::Hidden,
            Overflow::Scroll => Self::Scroll,
        }
    }
}

impl From<Position> for taffy::style::Position {
    fn from(value: Position) -> Self {
        match value {
            Position::Relative => Self::Relative,
            Position::Absolute => Self::Absolute,
        }
    }
}
