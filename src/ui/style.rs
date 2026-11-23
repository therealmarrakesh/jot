//! Clean-room layout and visual style definitions (`Style`, `StyleRefinement`, `TextStyle`).

use super::color::{Rgba, black};
use super::geometry::{
    Corners, DefiniteLength, Edges, Length, Pixels, Point, Size, px,
};
use super::graphics::d2d_renderer::BoxShadow;
use super::graphics::dwrite::{Font, FontStyle, FontWeight, TextAlign};
use super::platform::CursorStyle;
use super::types::SharedString;

#[cfg(test)]
pub(crate) trait Refineable {
    type Refinement;
    fn refine(&mut self, refinement: &Self::Refinement);
}

#[cfg(test)]
impl Refineable for Style {
    type Refinement = StyleRefinement;

    fn refine(&mut self, refinement: &StyleRefinement) {
        Style::refine(self, refinement);
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum Display {
    Block,
    #[default]
    Flex,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum Position {
    #[default]
    Relative,
    Absolute,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum Overflow {
    #[default]
    Visible,
    Hidden,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum FlexDirection {
    #[default]
    Row,
    Column,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum AlignItems {
    Start,
    Center,
}

pub(crate) type AlignSelf = AlignItems;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum JustifyContent {
    Center,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum WhiteSpace {
    #[default]
    Normal,
    Nowrap,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum TextOverflow {
    Truncate,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TextStyle {
    pub(crate) color: Rgba,
    pub(crate) font_family: SharedString,
    pub(crate) font_weight: FontWeight,
    pub(crate) font_style: FontStyle,
    pub(crate) font_size: Pixels,
    pub(crate) line_height: DefiniteLength,
    pub(crate) white_space: WhiteSpace,
    pub(crate) text_overflow: Option<TextOverflow>,
    pub(crate) text_align: TextAlign,
    pub(crate) line_clamp: Option<usize>,
}

impl Default for TextStyle {
    fn default() -> Self {
        Self {
            color: black(),
            font_family: SharedString::new_static("Segoe UI Variable Text"),
            font_weight: FontWeight::NORMAL,
            font_style: FontStyle::Normal,
            font_size: px(14.0),
            line_height: DefiniteLength::Fraction(1.3),
            white_space: WhiteSpace::Normal,
            text_overflow: None,
            text_align: TextAlign::Left,
            line_clamp: None,
        }
    }
}

impl TextStyle {
    pub(crate) fn font(&self) -> Font {
        Font {
            family: self.font_family.clone(),
            weight: self.font_weight,
            style: self.font_style,
        }
    }

    pub(crate) fn line_height_in_pixels(&self) -> Pixels {
        match self.line_height {
            DefiniteLength::Absolute(px_val) => px_val,
            DefiniteLength::Fraction(mult) => self.font_size * mult,
        }
    }

    pub(crate) fn refine(&mut self, r: &TextStyleRefinement) {
        if let Some(c) = r.color {
            self.color = c;
        }
        if let Some(f) = &r.font_family {
            self.font_family = f.clone();
        }
        if let Some(w) = r.font_weight {
            self.font_weight = w;
        }
        if let Some(s) = r.font_style {
            self.font_style = s;
        }
        if let Some(sz) = r.font_size {
            self.font_size = sz;
        }
        if let Some(lh) = r.line_height {
            self.line_height = lh;
        }
        if let Some(ws) = r.white_space {
            self.white_space = ws;
        }
        if let Some(ov) = r.text_overflow {
            self.text_overflow = Some(ov);
        }
        if let Some(al) = r.text_align {
            self.text_align = al;
        }
        if let Some(lc) = r.line_clamp {
            self.line_clamp = Some(lc);
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct TextStyleRefinement {
    pub(crate) color: Option<Rgba>,
    pub(crate) font_family: Option<SharedString>,
    pub(crate) font_weight: Option<FontWeight>,
    pub(crate) font_style: Option<FontStyle>,
    pub(crate) font_size: Option<Pixels>,
    pub(crate) line_height: Option<DefiniteLength>,
    pub(crate) white_space: Option<WhiteSpace>,
    pub(crate) text_overflow: Option<TextOverflow>,
    pub(crate) text_align: Option<TextAlign>,
    pub(crate) line_clamp: Option<usize>,
}

impl TextStyleRefinement {
    pub(crate) fn refine(&mut self, other: &Self) {
        if let Some(c) = other.color {
            self.color = Some(c);
        }
        if let Some(f) = &other.font_family {
            self.font_family = Some(f.clone());
        }
        if let Some(w) = other.font_weight {
            self.font_weight = Some(w);
        }
        if let Some(s) = other.font_style {
            self.font_style = Some(s);
        }
        if let Some(sz) = other.font_size {
            self.font_size = Some(sz);
        }
        if let Some(lh) = other.line_height {
            self.line_height = Some(lh);
        }
        if let Some(ws) = other.white_space {
            self.white_space = Some(ws);
        }
        if let Some(ov) = other.text_overflow {
            self.text_overflow = Some(ov);
        }
        if let Some(al) = other.text_align {
            self.text_align = Some(al);
        }
        if let Some(lc) = other.line_clamp {
            self.line_clamp = Some(lc);
        }
    }
}

/// Complete resolved layout and visual style for an element.
/// Field names match `src/layout.rs::NodeStyle::from_style` directly.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Style {
    pub(crate) display: Display,
    pub(crate) position: Position,
    pub(crate) overflow: Point<Overflow>,
    pub(crate) inset: Edges<Length>,
    pub(crate) size: Size<Length>,
    pub(crate) min_size: Size<Length>,
    pub(crate) max_size: Size<Length>,
    pub(crate) margin: Edges<Length>,
    pub(crate) padding: Edges<DefiniteLength>,
    pub(crate) border_widths: Edges<Pixels>,
    pub(crate) gap: Size<DefiniteLength>,
    pub(crate) align_items: Option<AlignItems>,
    pub(crate) align_self: Option<AlignSelf>,
    pub(crate) justify_content: Option<JustifyContent>,
    pub(crate) flex_direction: FlexDirection,
    pub(crate) flex_basis: Length,
    pub(crate) flex_grow: f32,
    pub(crate) flex_shrink: f32,
    pub(crate) background: Option<Rgba>,
    pub(crate) border_color: Option<Rgba>,
    pub(crate) corner_radii: Corners<Pixels>,
    pub(crate) box_shadow: Vec<BoxShadow>,
    pub(crate) opacity: Option<f32>,
    pub(crate) cursor: Option<CursorStyle>,
    pub(crate) text: TextStyleRefinement,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            display: Display::Block,
            position: Position::Relative,
            overflow: Point::new(Overflow::Visible, Overflow::Visible),
            inset: Edges::auto(),
            size: Size::auto(),
            min_size: Size::auto(),
            max_size: Size::auto(),
            margin: Edges::<Length>::zero(),
            padding: Edges::<DefiniteLength>::zero(),
            border_widths: Edges::default(),
            gap: Size::zero(),
            align_items: None,
            align_self: None,
            justify_content: None,
            flex_direction: FlexDirection::Row,
            flex_basis: Length::Auto,
            flex_grow: 0.0,
            flex_shrink: 1.0,
            background: None,
            border_color: None,
            corner_radii: Corners::default(),
            box_shadow: Vec::new(),
            opacity: None,
            cursor: None,
            text: TextStyleRefinement::default(),
        }
    }
}

impl Style {
    pub(crate) fn refine(&mut self, r: &StyleRefinement) {
        if let Some(v) = r.display {
            self.display = v;
        }
        if let Some(v) = r.position {
            self.position = v;
        }
        if let Some(v) = r.overflow.x {
            self.overflow.x = v;
        }
        if let Some(v) = r.overflow.y {
            self.overflow.y = v;
        }
        if let Some(v) = r.inset.top {
            self.inset.top = v;
        }
        if let Some(v) = r.inset.right {
            self.inset.right = v;
        }
        if let Some(v) = r.inset.bottom {
            self.inset.bottom = v;
        }
        if let Some(v) = r.inset.left {
            self.inset.left = v;
        }
        if let Some(v) = r.size.width {
            self.size.width = v;
        }
        if let Some(v) = r.size.height {
            self.size.height = v;
        }
        if let Some(v) = r.min_size.width {
            self.min_size.width = v;
        }
        if let Some(v) = r.min_size.height {
            self.min_size.height = v;
        }
        if let Some(v) = r.max_size.width {
            self.max_size.width = v;
        }
        if let Some(v) = r.max_size.height {
            self.max_size.height = v;
        }
        if let Some(v) = r.margin.top {
            self.margin.top = v;
        }
        if let Some(v) = r.margin.right {
            self.margin.right = v;
        }
        if let Some(v) = r.margin.bottom {
            self.margin.bottom = v;
        }
        if let Some(v) = r.margin.left {
            self.margin.left = v;
        }
        if let Some(v) = r.padding.top {
            self.padding.top = v;
        }
        if let Some(v) = r.padding.right {
            self.padding.right = v;
        }
        if let Some(v) = r.padding.bottom {
            self.padding.bottom = v;
        }
        if let Some(v) = r.padding.left {
            self.padding.left = v;
        }
        if let Some(v) = r.border_widths.top {
            self.border_widths.top = v;
        }
        if let Some(v) = r.border_widths.right {
            self.border_widths.right = v;
        }
        if let Some(v) = r.border_widths.bottom {
            self.border_widths.bottom = v;
        }
        if let Some(v) = r.border_widths.left {
            self.border_widths.left = v;
        }
        if let Some(v) = r.gap.width {
            self.gap.width = v;
        }
        if let Some(v) = r.gap.height {
            self.gap.height = v;
        }
        if let Some(v) = r.align_items {
            self.align_items = Some(v);
        }
        if let Some(v) = r.align_self {
            self.align_self = Some(v);
        }
        if let Some(v) = r.justify_content {
            self.justify_content = Some(v);
        }
        if let Some(v) = r.flex_direction {
            self.flex_direction = v;
        }
        if let Some(v) = r.flex_basis {
            self.flex_basis = v;
        }
        if let Some(v) = r.flex_grow {
            self.flex_grow = v;
        }
        if let Some(v) = r.flex_shrink {
            self.flex_shrink = v;
        }
        if let Some(v) = r.background {
            self.background = Some(v);
        }
        if let Some(v) = r.border_color {
            self.border_color = Some(v);
        }
        if let Some(v) = r.corner_radii.top_left {
            self.corner_radii.top_left = v;
        }
        if let Some(v) = r.corner_radii.top_right {
            self.corner_radii.top_right = v;
        }
        if let Some(v) = r.corner_radii.bottom_right {
            self.corner_radii.bottom_right = v;
        }
        if let Some(v) = r.corner_radii.bottom_left {
            self.corner_radii.bottom_left = v;
        }
        if let Some(v) = &r.box_shadow {
            self.box_shadow = v.clone();
        }
        if let Some(v) = r.opacity {
            self.opacity = Some(v);
        }
        if let Some(v) = r.cursor {
            self.cursor = Some(v);
        }
        self.text.refine(&r.text);
    }
}

/// Partial style overlay accumulated on `Div` and hover/active/group_hover refinements.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct StyleRefinement {
    pub(crate) display: Option<Display>,
    pub(crate) position: Option<Position>,
    pub(crate) overflow: Point<Option<Overflow>>,
    pub(crate) inset: Edges<Option<Length>>,
    pub(crate) size: Size<Option<Length>>,
    pub(crate) min_size: Size<Option<Length>>,
    pub(crate) max_size: Size<Option<Length>>,
    pub(crate) margin: Edges<Option<Length>>,
    pub(crate) padding: Edges<Option<DefiniteLength>>,
    pub(crate) border_widths: Edges<Option<Pixels>>,
    pub(crate) gap: Size<Option<DefiniteLength>>,
    pub(crate) align_items: Option<AlignItems>,
    pub(crate) align_self: Option<AlignSelf>,
    pub(crate) justify_content: Option<JustifyContent>,
    pub(crate) flex_direction: Option<FlexDirection>,
    pub(crate) flex_basis: Option<Length>,
    pub(crate) flex_grow: Option<f32>,
    pub(crate) flex_shrink: Option<f32>,
    pub(crate) background: Option<Rgba>,
    pub(crate) border_color: Option<Rgba>,
    pub(crate) corner_radii: Corners<Option<Pixels>>,
    pub(crate) box_shadow: Option<Vec<BoxShadow>>,
    pub(crate) opacity: Option<f32>,
    pub(crate) cursor: Option<CursorStyle>,
    pub(crate) text: TextStyleRefinement,
}

impl StyleRefinement {
    pub(crate) fn refine(&mut self, other: &Self) {
        if other.display.is_some() {
            self.display = other.display;
        }
        if other.position.is_some() {
            self.position = other.position;
        }
        if other.overflow.x.is_some() {
            self.overflow.x = other.overflow.x;
        }
        if other.overflow.y.is_some() {
            self.overflow.y = other.overflow.y;
        }
        if other.inset.top.is_some() {
            self.inset.top = other.inset.top;
        }
        if other.inset.right.is_some() {
            self.inset.right = other.inset.right;
        }
        if other.inset.bottom.is_some() {
            self.inset.bottom = other.inset.bottom;
        }
        if other.inset.left.is_some() {
            self.inset.left = other.inset.left;
        }
        if other.size.width.is_some() {
            self.size.width = other.size.width;
        }
        if other.size.height.is_some() {
            self.size.height = other.size.height;
        }
        if other.min_size.width.is_some() {
            self.min_size.width = other.min_size.width;
        }
        if other.min_size.height.is_some() {
            self.min_size.height = other.min_size.height;
        }
        if other.max_size.width.is_some() {
            self.max_size.width = other.max_size.width;
        }
        if other.max_size.height.is_some() {
            self.max_size.height = other.max_size.height;
        }
        if other.margin.top.is_some() {
            self.margin.top = other.margin.top;
        }
        if other.margin.right.is_some() {
            self.margin.right = other.margin.right;
        }
        if other.margin.bottom.is_some() {
            self.margin.bottom = other.margin.bottom;
        }
        if other.margin.left.is_some() {
            self.margin.left = other.margin.left;
        }
        if other.padding.top.is_some() {
            self.padding.top = other.padding.top;
        }
        if other.padding.right.is_some() {
            self.padding.right = other.padding.right;
        }
        if other.padding.bottom.is_some() {
            self.padding.bottom = other.padding.bottom;
        }
        if other.padding.left.is_some() {
            self.padding.left = other.padding.left;
        }
        if other.border_widths.top.is_some() {
            self.border_widths.top = other.border_widths.top;
        }
        if other.border_widths.right.is_some() {
            self.border_widths.right = other.border_widths.right;
        }
        if other.border_widths.bottom.is_some() {
            self.border_widths.bottom = other.border_widths.bottom;
        }
        if other.border_widths.left.is_some() {
            self.border_widths.left = other.border_widths.left;
        }
        if other.gap.width.is_some() {
            self.gap.width = other.gap.width;
        }
        if other.gap.height.is_some() {
            self.gap.height = other.gap.height;
        }
        if other.align_items.is_some() {
            self.align_items = other.align_items;
        }
        if other.align_self.is_some() {
            self.align_self = other.align_self;
        }
        if other.justify_content.is_some() {
            self.justify_content = other.justify_content;
        }
        if other.flex_direction.is_some() {
            self.flex_direction = other.flex_direction;
        }
        if other.flex_basis.is_some() {
            self.flex_basis = other.flex_basis;
        }
        if other.flex_grow.is_some() {
            self.flex_grow = other.flex_grow;
        }
        if other.flex_shrink.is_some() {
            self.flex_shrink = other.flex_shrink;
        }
        if other.background.is_some() {
            self.background = other.background;
        }
        if other.border_color.is_some() {
            self.border_color = other.border_color;
        }
        if other.corner_radii.top_left.is_some() {
            self.corner_radii.top_left = other.corner_radii.top_left;
        }
        if other.corner_radii.top_right.is_some() {
            self.corner_radii.top_right = other.corner_radii.top_right;
        }
        if other.corner_radii.bottom_right.is_some() {
            self.corner_radii.bottom_right = other.corner_radii.bottom_right;
        }
        if other.corner_radii.bottom_left.is_some() {
            self.corner_radii.bottom_left = other.corner_radii.bottom_left;
        }
        if other.box_shadow.is_some() {
            self.box_shadow = other.box_shadow.clone();
        }
        if other.opacity.is_some() {
            self.opacity = other.opacity;
        }
        if other.cursor.is_some() {
            self.cursor = other.cursor;
        }
        self.text.refine(&other.text);
    }
}
