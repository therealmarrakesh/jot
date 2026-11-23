use crate::ui::{
    AlignItems, AlignSelf, BoxShadow, CursorStyle, DefiniteLength, Display, FlexDirection,
    FontWeight, JustifyContent, Length, Overflow, Pixels, Position, Rgba, SharedString,
    StyleRefinement, TextAlign, TextStyleRefinement, WhiteSpace, px, relative,
};

/// A trait for elements that can be styled.
pub(crate) trait Styled: Sized {
    /// Returns a reference to the style memory of this element.
    fn style(&mut self) -> &mut StyleRefinement;

    // -- Position --
    fn relative(mut self) -> Self {
        self.style().position = Some(Position::Relative);
        self
    }
    fn absolute(mut self) -> Self {
        self.style().position = Some(Position::Absolute);
        self
    }
    fn top(mut self, length: impl Into<Length>) -> Self {
        self.style().inset.top = Some(length.into());
        self
    }
    fn top_0(self) -> Self {
        self.top(px(0.))
    }
    fn bottom(mut self, length: impl Into<Length>) -> Self {
        self.style().inset.bottom = Some(length.into());
        self
    }
    fn bottom_0(self) -> Self {
        self.bottom(px(0.))
    }
    fn left(mut self, length: impl Into<Length>) -> Self {
        self.style().inset.left = Some(length.into());
        self
    }
    fn left_0(self) -> Self {
        self.left(px(0.))
    }
    fn right(mut self, length: impl Into<Length>) -> Self {
        self.style().inset.right = Some(length.into());
        self
    }
    fn right_0(self) -> Self {
        self.right(px(0.))
    }

    // -- Overflow --
    fn overflow_hidden(mut self) -> Self {
        self.style().overflow.x = Some(Overflow::Hidden);
        self.style().overflow.y = Some(Overflow::Hidden);
        self
    }
    fn overflow_x_hidden(mut self) -> Self {
        self.style().overflow.x = Some(Overflow::Hidden);
        self
    }

    // -- Cursor --
    fn cursor(mut self, cursor: CursorStyle) -> Self {
        self.style().mouse_cursor = Some(cursor);
        self
    }
    fn cursor_default(self) -> Self {
        self.cursor(CursorStyle::Arrow)
    }
    fn cursor_text(self) -> Self {
        self.cursor(CursorStyle::IBeam)
    }

    // -- Margin --
    fn mt(mut self, length: impl Into<Length>) -> Self {
        self.style().margin.top = Some(length.into());
        self
    }
    fn mb(mut self, length: impl Into<Length>) -> Self {
        self.style().margin.bottom = Some(length.into());
        self
    }
    fn ml(mut self, length: impl Into<Length>) -> Self {
        self.style().margin.left = Some(length.into());
        self
    }

    // -- Padding --
    fn p(mut self, length: impl Clone + Into<DefiniteLength>) -> Self {
        let s = self.style();
        s.padding.top = Some(length.clone().into());
        s.padding.bottom = Some(length.clone().into());
        s.padding.left = Some(length.clone().into());
        s.padding.right = Some(length.into());
        self
    }
    fn px(mut self, length: impl Clone + Into<DefiniteLength>) -> Self {
        let s = self.style();
        s.padding.left = Some(length.clone().into());
        s.padding.right = Some(length.into());
        self
    }
    fn py(mut self, length: impl Clone + Into<DefiniteLength>) -> Self {
        let s = self.style();
        s.padding.top = Some(length.clone().into());
        s.padding.bottom = Some(length.into());
        self
    }
    fn pt(mut self, length: impl Into<DefiniteLength>) -> Self {
        self.style().padding.top = Some(length.into());
        self
    }
    fn pb(mut self, length: impl Into<DefiniteLength>) -> Self {
        self.style().padding.bottom = Some(length.into());
        self
    }
    fn pl(mut self, length: impl Into<DefiniteLength>) -> Self {
        self.style().padding.left = Some(length.into());
        self
    }
    fn pr(mut self, length: impl Into<DefiniteLength>) -> Self {
        self.style().padding.right = Some(length.into());
        self
    }

    // -- Sizing & Gap --
    fn w(mut self, length: impl Into<Length>) -> Self {
        self.style().size.width = Some(length.into());
        self
    }
    fn w_full(self) -> Self {
        self.w(relative(1.))
    }
    fn h(mut self, length: impl Into<Length>) -> Self {
        self.style().size.height = Some(length.into());
        self
    }
    fn h_full(self) -> Self {
        self.h(relative(1.))
    }
    fn size(mut self, length: impl Clone + Into<Length>) -> Self {
        let s = self.style();
        s.size.width = Some(length.clone().into());
        s.size.height = Some(length.into());
        self
    }
    fn size_full(self) -> Self {
        self.size(relative(1.))
    }
    fn min_w(mut self, length: impl Into<Length>) -> Self {
        self.style().min_size.width = Some(length.into());
        self
    }
    fn min_w_0(self) -> Self {
        self.min_w(px(0.))
    }
    fn min_h(mut self, length: impl Into<Length>) -> Self {
        self.style().min_size.height = Some(length.into());
        self
    }
    fn min_h_full(self) -> Self {
        self.min_h(relative(1.))
    }
    fn max_w(mut self, length: impl Into<Length>) -> Self {
        self.style().max_size.width = Some(length.into());
        self
    }
    fn max_w_full(self) -> Self {
        self.max_w(relative(1.))
    }
    fn gap(mut self, length: impl Clone + Into<DefiniteLength>) -> Self {
        let s = self.style();
        s.gap.width = Some(length.clone().into());
        s.gap.height = Some(length.into());
        self
    }

    // -- Corners --
    fn rounded(mut self, radius: impl Into<Pixels>) -> Self {
        let radius = radius.into();
        let s = self.style();
        s.corner_radii.top_left = Some(radius);
        s.corner_radii.top_right = Some(radius);
        s.corner_radii.bottom_right = Some(radius);
        s.corner_radii.bottom_left = Some(radius);
        self
    }
    fn rounded_full(self) -> Self {
        self.rounded(px(9999.))
    }
    fn rounded_t(mut self, radius: impl Into<Pixels>) -> Self {
        let radius = radius.into();
        let s = self.style();
        s.corner_radii.top_left = Some(radius);
        s.corner_radii.top_right = Some(radius);
        self
    }
    fn rounded_b(mut self, radius: impl Into<Pixels>) -> Self {
        let radius = radius.into();
        let s = self.style();
        s.corner_radii.bottom_left = Some(radius);
        s.corner_radii.bottom_right = Some(radius);
        self
    }

    // -- Border --
    fn border_color(mut self, border_color: Rgba) -> Self {
        self.style().border_color = Some(border_color);
        self
    }
    fn border(mut self, width: impl Into<Pixels>) -> Self {
        let width = width.into();
        let s = self.style();
        s.border_widths.top = Some(width);
        s.border_widths.right = Some(width);
        s.border_widths.bottom = Some(width);
        s.border_widths.left = Some(width);
        self
    }
    fn border_1(self) -> Self {
        self.border(px(1.))
    }
    fn border_2(self) -> Self {
        self.border(px(2.))
    }
    fn border_t(mut self, width: impl Into<Pixels>) -> Self {
        self.style().border_widths.top = Some(width.into());
        self
    }
    fn border_t_1(self) -> Self {
        self.border_t(px(1.))
    }
    fn border_b(mut self, width: impl Into<Pixels>) -> Self {
        self.style().border_widths.bottom = Some(width.into());
        self
    }
    fn border_b_1(self) -> Self {
        self.border_b(px(1.))
    }
    fn border_l(mut self, width: impl Into<Pixels>) -> Self {
        self.style().border_widths.left = Some(width.into());
        self
    }
    fn border_l_1(self) -> Self {
        self.border_l(px(1.))
    }
    fn border_r(mut self, width: impl Into<Pixels>) -> Self {
        self.style().border_widths.right = Some(width.into());
        self
    }
    fn border_r_1(self) -> Self {
        self.border_r(px(1.))
    }

    // -- Box Shadow --
    fn shadow(mut self, shadows: Vec<BoxShadow>) -> Self {
        self.style().box_shadow = Some(shadows);
        self
    }

    // -- Display & Flex --
    fn flex(mut self) -> Self {
        self.style().display = Some(Display::Flex);
        self
    }
    fn flex_col(mut self) -> Self {
        self.style().flex_direction = Some(FlexDirection::Column);
        self
    }
    fn flex_1(mut self) -> Self {
        self.style().flex_grow = Some(1.);
        self.style().flex_shrink = Some(1.);
        self.style().flex_basis = Some(relative(0.).into());
        self
    }
    fn flex_none(mut self) -> Self {
        self.style().flex_grow = Some(0.);
        self.style().flex_shrink = Some(0.);
        self.style().flex_basis = Some(Length::Auto);
        self
    }
    fn flex_grow_1(mut self) -> Self {
        self.style().flex_grow = Some(1.);
        self
    }
    fn items_center(mut self) -> Self {
        self.style().align_items = Some(AlignItems::Center);
        self
    }
    fn self_start(mut self) -> Self {
        self.style().align_self = Some(AlignSelf::Start);
        self
    }
    fn justify_center(mut self) -> Self {
        self.style().justify_content = Some(JustifyContent::Center);
        self
    }

    // -- Background & Opacity --
    fn bg(mut self, color: Rgba) -> Self {
        self.style().background = Some(color);
        self
    }
    fn opacity(mut self, opacity: f32) -> Self {
        self.style().opacity = Some(opacity);
        self
    }

    // -- Text --
    fn text_style(&mut self) -> &mut TextStyleRefinement {
        &mut self.style().text
    }
    fn whitespace_nowrap(mut self) -> Self {
        self.text_style().white_space = Some(WhiteSpace::Nowrap);
        self
    }
    fn text_align(mut self, align: TextAlign) -> Self {
        self.text_style().text_align = Some(align);
        self
    }
    fn text_center(self) -> Self {
        self.text_align(TextAlign::Center)
    }
    fn line_clamp(mut self, lines: usize) -> Self {
        self.text_style().line_clamp = Some(lines);
        self.overflow_hidden()
    }
    fn text_color(mut self, color: Rgba) -> Self {
        self.text_style().color = Some(color);
        self
    }
    fn font_weight(mut self, weight: FontWeight) -> Self {
        self.text_style().font_weight = Some(weight);
        self
    }
    fn text_size(mut self, size: impl Into<Pixels>) -> Self {
        self.text_style().font_size = Some(size.into());
        self
    }
    fn font_family(mut self, family_name: impl Into<SharedString>) -> Self {
        self.text_style().font_family = Some(family_name.into());
        self
    }
    fn line_height(mut self, line_height: impl Into<DefiniteLength>) -> Self {
        self.text_style().line_height = Some(line_height.into());
        self
    }
}
