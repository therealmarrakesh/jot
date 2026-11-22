use crate::ui::{
    AbsoluteLength, App, Background, Bounds, ContentMask, Corners, CursorStyle, DefiniteLength,
    Edges, Font, FontFallbacks, FontFeatures, FontStyle, FontWeight, Hsla, Length, Pixels, Point,
    Rgba, SharedString, Size, Styled, TextRun, Window, black, phi, point, quad, rems,
};
use crate::ui::refineable::{IsEmpty, Refineable};

/// The CSS styling that can be applied to an element via the `Styled` trait
#[derive(Clone, Debug)]
pub(crate) struct Style {
    /// What layout strategy should be used?
    pub(crate) display: Display,

    /// Should the element be painted on screen?
    pub(crate) visibility: Visibility,

    // Overflow properties
    /// How children overflowing their container should affect layout
    pub(crate) overflow: Point<Overflow>,
    /// How much space (in points) should be reserved for the scrollbars of `Overflow::Scroll` and `Overflow::Auto` nodes.
    pub(crate) scrollbar_width: AbsoluteLength,
    /// Whether both x and y axis should be scrollable at the same time.
    pub(crate) allow_concurrent_scroll: bool,
    /// Whether scrolling should be restricted to the input gesture's axis.
    pub(crate) restrict_scroll_to_axis: bool,

    // Position properties
    /// What should the `position` value of this struct use as a base offset?
    pub(crate) position: Position,
    /// How should the position of this element be tweaked relative to the layout defined?
    pub(crate) inset: Edges<Length>,

    // Size properties
    /// Sets the initial size of the item
    pub(crate) size: Size<Length>,
    /// Controls the minimum size of the item
    pub(crate) min_size: Size<Length>,
    /// Controls the maximum size of the item
    pub(crate) max_size: Size<Length>,
    /// Sets the preferred aspect ratio for the item. The ratio is calculated as width divided by height.
    pub(crate) aspect_ratio: Option<f32>,

    // Spacing Properties
    /// How large should the margin be on each side?
    pub(crate) margin: Edges<Length>,
    /// How large should the padding be on each side?
    pub(crate) padding: Edges<DefiniteLength>,
    /// How large should the border be on each side?
    pub(crate) border_widths: Edges<AbsoluteLength>,

    // Alignment properties
    /// How this node's children aligned in the cross/block axis?
    pub(crate) align_items: Option<AlignItems>,
    /// How this node should be aligned in the cross/block axis. Falls back to the parents [`AlignItems`] if not set
    pub(crate) align_self: Option<AlignSelf>,
    /// How should content contained within this item be aligned in the cross/block axis
    pub(crate) align_content: Option<AlignContent>,
    /// How should contained within this item be aligned in the main/inline axis
    pub(crate) justify_content: Option<JustifyContent>,
    /// How large should the gaps between items in a flex container be?
    pub(crate) gap: Size<DefiniteLength>,

    // Flexbox properties
    /// Which direction does the main axis flow in?
    pub(crate) flex_direction: FlexDirection,
    /// Sets the initial main axis size of the item
    pub(crate) flex_basis: Length,
    /// The relative rate at which this item grows when it is expanding to fill space, 0.0 is the default value, and this value must be positive.
    pub(crate) flex_grow: f32,
    /// The relative rate at which this item shrinks when it is contracting to fit into space, 1.0 is the default value, and this value must be positive.
    pub(crate) flex_shrink: f32,

    /// The fill color of this element
    pub(crate) background: Option<Fill>,

    /// The border color of this element
    pub(crate) border_color: Option<Hsla>,

    /// The radius of the corners of this element
    pub(crate) corner_radii: Corners<AbsoluteLength>,

    /// Box shadow of the element
    pub(crate) box_shadow: Vec<BoxShadow>,

    /// The text style of this element
    pub(crate) text: TextStyleRefinement,

    /// The mouse cursor style shown when the mouse pointer is over an element.
    pub(crate) mouse_cursor: Option<CursorStyle>,

    /// The opacity of this element
    pub(crate) opacity: Option<f32>,
}

#[derive(Clone, Default, PartialEq)]
pub(crate) struct StyleRefinement {
    pub(crate) display: Option<Display>,
    pub(crate) visibility: Option<Visibility>,
    pub(crate) overflow: crate::ui::PointRefinement<Overflow>,
    pub(crate) scrollbar_width: Option<AbsoluteLength>,
    pub(crate) allow_concurrent_scroll: Option<bool>,
    pub(crate) restrict_scroll_to_axis: Option<bool>,
    pub(crate) position: Option<Position>,
    pub(crate) inset: crate::ui::EdgesRefinement<Length>,
    pub(crate) size: crate::ui::SizeRefinement<Length>,
    pub(crate) min_size: crate::ui::SizeRefinement<Length>,
    pub(crate) max_size: crate::ui::SizeRefinement<Length>,
    pub(crate) aspect_ratio: Option<f32>,
    pub(crate) margin: crate::ui::EdgesRefinement<Length>,
    pub(crate) padding: crate::ui::EdgesRefinement<DefiniteLength>,
    pub(crate) border_widths: crate::ui::EdgesRefinement<AbsoluteLength>,
    pub(crate) align_items: Option<AlignItems>,
    pub(crate) align_self: Option<AlignSelf>,
    pub(crate) align_content: Option<AlignContent>,
    pub(crate) justify_content: Option<JustifyContent>,
    pub(crate) gap: crate::ui::SizeRefinement<DefiniteLength>,
    pub(crate) flex_direction: Option<FlexDirection>,
    pub(crate) flex_basis: Option<Length>,
    pub(crate) flex_grow: Option<f32>,
    pub(crate) flex_shrink: Option<f32>,
    pub(crate) background: Option<Fill>,
    pub(crate) border_color: Option<Hsla>,
    pub(crate) corner_radii: crate::ui::CornersRefinement<AbsoluteLength>,
    pub(crate) box_shadow: Option<Vec<BoxShadow>>,
    pub(crate) text: TextStyleRefinement,
    pub(crate) mouse_cursor: Option<CursorStyle>,
    pub(crate) opacity: Option<f32>,
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
        self.display.is_none()
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
            && self.flex_basis.is_none()
            && self.flex_grow.is_none()
            && self.flex_shrink.is_none()
            && self.background.is_none()
            && self.border_color.is_none()
            && self.corner_radii.is_empty()
            && self.box_shadow.is_none()
            && self.text.is_empty()
            && self.mouse_cursor.is_none()
            && self.opacity.is_none()
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
            flex_basis: v.flex_basis.unwrap_or_default(),
            flex_grow: v.flex_grow.unwrap_or_default(),
            flex_shrink: v.flex_shrink.unwrap_or_default(),
            background: v.background,
            border_color: v.border_color,
            corner_radii: v.corner_radii.into(),
            box_shadow: v.box_shadow.unwrap_or_default(),
            text: v.text,
            mouse_cursor: v.mouse_cursor,
            opacity: v.opacity,
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
        if let Some(v) = &r.flex_basis { self.flex_basis = *v; }
        if let Some(v) = &r.flex_grow { self.flex_grow = *v; }
        if let Some(v) = &r.flex_shrink { self.flex_shrink = *v; }
        if let Some(v) = &r.background { self.background = Some(v.clone()); }
        if let Some(v) = &r.border_color { self.border_color = Some(*v); }
        self.corner_radii.refine(&r.corner_radii);
        if let Some(v) = &r.box_shadow { self.box_shadow = v.clone(); }
        self.text.refine(&r.text);
        if let Some(v) = &r.mouse_cursor { self.mouse_cursor = Some(*v); }
        if let Some(v) = &r.opacity { self.opacity = Some(*v); }
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
        if let Some(v) = &r.flex_basis { self.flex_basis = Some(*v); }
        if let Some(v) = &r.flex_grow { self.flex_grow = Some(*v); }
        if let Some(v) = &r.flex_shrink { self.flex_shrink = Some(*v); }
        if let Some(v) = &r.background { self.background = Some(v.clone()); }
        if let Some(v) = &r.border_color { self.border_color = Some(*v); }
        self.corner_radii.refine(&r.corner_radii);
        if let Some(v) = &r.box_shadow { self.box_shadow = Some(v.clone()); }
        self.text.refine(&r.text);
        if let Some(v) = &r.mouse_cursor { self.mouse_cursor = Some(*v); }
        if let Some(v) = &r.opacity { self.opacity = Some(*v); }
    }
}

impl Styled for StyleRefinement {
    fn style(&mut self) -> &mut StyleRefinement {
        self
    }
}

/// The value of the visibility property, similar to the CSS property `visibility`
#[derive(Default, Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Visibility {
    /// The element should be drawn as normal.
    #[default]
    Visible,
    /// The element should not be drawn, but should still take up space in the layout.
    Hidden,
}

/// The possible values of the box-shadow property
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct BoxShadow {
    /// What color should the shadow have?
    pub(crate) color: Hsla,
    /// How should it be offset from its element?
    pub(crate) offset: Point<Pixels>,
    /// How much should the shadow be blurred?
    pub(crate) blur_radius: Pixels,
    /// How much should the shadow spread?
    pub(crate) spread_radius: Pixels,
    /// Whether this is an inset shadow (drawn inside the element's bounds).
    pub(crate) inset: bool,
}

/// How to handle whitespace in text
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum WhiteSpace {
    /// Normal line wrapping when text overflows the width of the element
    #[default]
    Normal,
    /// No line wrapping, text will overflow the width of the element
    Nowrap,
}

/// How to align text within the element
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum TextAlign {
    /// Align the text to the left of the element
    #[default]
    Left,

    /// Center the text within the element
    Center,
}

/// The properties that can be used to style text in GPUI
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TextStyle {
    /// The color of the text
    pub(crate) color: Hsla,
    /// The font family to use
    pub(crate) font_family: SharedString,
    /// The font features to use
    pub(crate) font_features: FontFeatures,
    /// The fallback fonts to use
    pub(crate) font_fallbacks: Option<FontFallbacks>,
    /// The font size to use, in pixels or rems.
    pub(crate) font_size: AbsoluteLength,
    /// The line height to use, in pixels or fractions
    pub(crate) line_height: DefiniteLength,
    /// The font weight, e.g. bold
    pub(crate) font_weight: FontWeight,
    /// The font style, e.g. italic
    pub(crate) font_style: FontStyle,
    /// The background color of the text
    pub(crate) background_color: Option<Hsla>,
    /// The underline style of the text
    pub(crate) underline: Option<UnderlineStyle>,
    /// How to handle whitespace in the text
    pub(crate) white_space: WhiteSpace,
    /// How the text should be aligned within the element
    pub(crate) text_align: TextAlign,
    /// The number of lines to display before truncating the text
    pub(crate) line_clamp: Option<usize>,
}

#[derive(Clone, Default, PartialEq)]
pub(crate) struct TextStyleRefinement {
    pub(crate) color: Option<Hsla>,
    pub(crate) font_family: Option<SharedString>,
    pub(crate) font_features: Option<FontFeatures>,
    pub(crate) font_fallbacks: Option<FontFallbacks>,
    pub(crate) font_size: Option<AbsoluteLength>,
    pub(crate) line_height: Option<DefiniteLength>,
    pub(crate) font_weight: Option<FontWeight>,
    pub(crate) font_style: Option<FontStyle>,
    pub(crate) background_color: Option<Hsla>,
    pub(crate) underline: Option<UnderlineStyle>,
    pub(crate) white_space: Option<WhiteSpace>,
    pub(crate) text_align: Option<TextAlign>,
    pub(crate) line_clamp: Option<usize>,
}

impl TextStyleRefinement {
    pub(crate) fn is_some(&self) -> bool {
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
        if let Some(v) = &r.text_align { self.text_align = *v; }
        if let Some(v) = &r.line_clamp { self.line_clamp = Some(*v); }
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
        if let Some(v) = &r.text_align { self.text_align = Some(*v); }
        if let Some(v) = &r.line_clamp { self.line_clamp = Some(*v); }
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
            text_align: TextAlign::default(),
            line_clamp: None,
        }
    }
}

impl TextStyle {
    /// Get the font configured for this text style.
    pub(crate) fn font(&self) -> Font {
        Font {
            family: self.font_family.clone(),
            features: self.font_features.clone(),
            fallbacks: self.font_fallbacks.clone(),
            weight: self.font_weight,
            style: self.font_style,
        }
    }

    /// Returns the rounded line height in pixels.
    pub(crate) fn line_height_in_pixels(&self, rem_size: Pixels) -> Pixels {
        self.line_height.to_pixels(self.font_size, rem_size).round()
    }

    /// Convert this text style into a [`TextRun`], for the given length of the text.
    pub(crate) fn to_run(&self, len: usize) -> TextRun {
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

impl Style {
    /// Get the text style in this element style.
    pub(crate) fn text_style(&self) -> Option<&TextStyleRefinement> {
        if self.text.is_some() {
            Some(&self.text)
        } else {
            None
        }
    }

    /// Get the content mask for this element style, based on the given bounds.
    /// If the element does not hide its overflow, this will return `None`.
    pub(crate) fn overflow_mask(
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
    pub(crate) fn paint(
        &self,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut App,
        continuation: impl FnOnce(&mut Window, &mut App),
    ) {
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
            ));
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
            flex_grow: 0.0,
            flex_shrink: 1.0,
            flex_basis: Length::Auto,
            background: None,
            border_color: None,
            corner_radii: Corners::default(),
            box_shadow: Default::default(),
            text: TextStyleRefinement::default(),
            mouse_cursor: None,
            opacity: None,
        }
    }
}

/// The properties that can be applied to an underline.
#[derive(Copy, Clone, Default, Debug, PartialEq, Eq, Hash)]
pub(crate) struct UnderlineStyle {
    /// The thickness of the underline.
    pub(crate) thickness: Pixels,
    /// The color of the underline.
    pub(crate) color: Option<Hsla>,
}

#[derive(Clone, Default)]
pub(crate) struct UnderlineStyleRefinement {
    pub(crate) thickness: Option<Pixels>,
    pub(crate) color: Option<Hsla>,
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
}

impl Refineable for UnderlineStyleRefinement {
    type Refinement = UnderlineStyleRefinement;
    fn refine(&mut self, r: &Self::Refinement) {
        if let Some(v) = r.thickness { self.thickness = Some(v); }
        if let Some(v) = r.color { self.color = Some(v); }
    }
}

/// The kinds of fill that can be applied to a shape.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Fill {
    /// A solid color fill.
    Color(Background),
}

impl Fill {
    /// Unwrap this fill into a solid color, if it is one.
    ///
    /// If the fill is not a solid color, this method returns `None`.
    pub(crate) fn color(&self) -> Option<Background> {
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

/// Used to control how child nodes are aligned.
/// For Flexbox it controls alignment in the cross axis
/// For Grid it controls alignment in the block axis
///
/// [MDN](https://developer.mozilla.org/en-US/docs/Web/CSS/align-items)
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
// Copy of taffy::style type of the same name.
pub(crate) enum AlignItems {
    /// Items are packed toward the start of the axis
    Start,
    /// Items are packed along the center of the cross axis
    Center,
}
/// Used to control how the specified nodes is aligned.
/// Overrides the parent Node's `AlignItems` property.
/// For Flexbox it controls alignment in the cross axis
/// For Grid it controls alignment in the block axis
///
/// [MDN](https://developer.mozilla.org/en-US/docs/Web/CSS/align-self)
pub(crate) type AlignSelf = AlignItems;

/// Sets the distribution of space between and around content items
/// For Flexbox it controls alignment in the cross axis
/// For Grid it controls alignment in the block axis
///
/// [MDN](https://developer.mozilla.org/en-US/docs/Web/CSS/align-content)
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
// Copy of taffy::style type of the same name.
pub(crate) enum AlignContent {
    /// Items are centered around the middle of the axis
    Center,
}

/// Sets the distribution of space between and around content items
/// For Flexbox it controls alignment in the main axis
/// For Grid it controls alignment in the inline axis
///
/// [MDN](https://developer.mozilla.org/en-US/docs/Web/CSS/justify-content)
pub(crate) type JustifyContent = AlignContent;

/// Sets the layout used for the children of this node
///
/// The default values depends on on which feature flags are enabled. The order of precedence is: Flex, Grid, Block, None.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
// Copy of taffy::style type of the same name.
pub(crate) enum Display {
    /// The children will follow the block layout algorithm
    Block,
    /// The children will follow the flexbox layout algorithm
    #[default]
    Flex,
    /// The children will not be laid out, and will follow absolute positioning
    None,
}

/// The direction of the flexbox layout main axis.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
// Copy of taffy::style type of the same name.
pub(crate) enum FlexDirection {
    /// Defines +x as the main axis
    ///
    /// Items will be added from left to right in a row.
    #[default]
    Row,
    /// Defines +y as the main axis
    ///
    /// Items will be added from top to bottom in a column.
    Column,
}

/// How children overflowing their container should affect layout
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
// Copy of taffy::style type of the same name.
pub(crate) enum Overflow {
    /// The automatic minimum size of this node as a flexbox/grid item should be based on the size of its content.
    /// Content that overflows this node *should* contribute to the scroll region of its parent.
    #[default]
    Visible,
    /// The automatic minimum size of this node as a flexbox/grid item should be `0`.
    /// Content that overflows this node should *not* contribute to the scroll region of its parent.
    Hidden,
}

/// The positioning strategy for this item.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
// Copy of taffy::style type of the same name.
pub(crate) enum Position {
    /// The offset is computed relative to the final position given by the layout algorithm.
    #[default]
    Relative,
    /// The offset is computed relative to this item's closest positioned ancestor, if any.
    Absolute,
}

impl From<AlignItems> for taffy::style::AlignItems {
    fn from(value: AlignItems) -> Self {
        match value {
            AlignItems::Start => Self::START,
            AlignItems::Center => Self::CENTER,
        }
    }
}

impl From<AlignContent> for taffy::style::AlignContent {
    fn from(value: AlignContent) -> Self {
        match value {
            AlignContent::Center => Self::CENTER,
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

impl From<FlexDirection> for taffy::style::FlexDirection {
    fn from(value: FlexDirection) -> Self {
        match value {
            FlexDirection::Row => Self::Row,
            FlexDirection::Column => Self::Column,
        }
    }
}

impl From<Overflow> for taffy::style::Overflow {
    fn from(value: Overflow) -> Self {
        match value {
            Overflow::Visible => Self::Visible,
            Overflow::Hidden => Self::Hidden,
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
