//! Clean-room fluent `Styled` trait covering the exact shorthand builders used by Jot.

use super::color::Rgba;
use super::geometry::{DefiniteLength, Length, Pixels, px, relative};
use super::graphics::d2d_renderer::BoxShadow;
use super::graphics::dwrite::{FontWeight, TextAlign};
use super::platform::CursorStyle;
use super::style::{
    AlignItems, Display, FlexDirection, JustifyContent, Overflow, Position, StyleRefinement,
    WhiteSpace,
};
use super::types::SharedString;

pub(crate) trait Styled: Sized {
    fn style(&mut self) -> &mut StyleRefinement;

    fn flex(mut self) -> Self {
        self.style().display = Some(Display::Flex);
        self
    }

    fn flex_col(mut self) -> Self {
        self.style().flex_direction = Some(FlexDirection::Column);
        self
    }

    fn flex_1(mut self) -> Self {
        let s = self.style();
        s.flex_grow = Some(1.0);
        s.flex_shrink = Some(1.0);
        s.flex_basis = Some(Length::Definite(relative(0.0)));
        self
    }

    fn flex_grow_1(mut self) -> Self {
        self.style().flex_grow = Some(1.0);
        self
    }

    fn flex_none(mut self) -> Self {
        let s = self.style();
        s.flex_grow = Some(0.0);
        s.flex_shrink = Some(0.0);
        self
    }

    fn relative(mut self) -> Self {
        self.style().position = Some(Position::Relative);
        self
    }

    fn absolute(mut self) -> Self {
        self.style().position = Some(Position::Absolute);
        self
    }

    fn items_center(mut self) -> Self {
        self.style().align_items = Some(AlignItems::Center);
        self
    }

    fn justify_center(mut self) -> Self {
        self.style().justify_content = Some(JustifyContent::Center);
        self
    }

    fn self_start(mut self) -> Self {
        self.style().align_self = Some(AlignItems::Start);
        self
    }

    fn w(mut self, width: impl Into<Length>) -> Self {
        self.style().size.width = Some(width.into());
        self
    }

    fn w_full(mut self) -> Self {
        self.style().size.width = Some(Length::Definite(relative(1.0)));
        self
    }

    fn h(mut self, height: impl Into<Length>) -> Self {
        self.style().size.height = Some(height.into());
        self
    }

    fn h_full(mut self) -> Self {
        self.style().size.height = Some(Length::Definite(relative(1.0)));
        self
    }

    fn size(mut self, length: impl Into<Length>) -> Self {
        let l = length.into();
        let s = self.style();
        s.size.width = Some(l);
        s.size.height = Some(l);
        self
    }

    fn size_full(mut self) -> Self {
        let s = self.style();
        s.size.width = Some(Length::Definite(relative(1.0)));
        s.size.height = Some(Length::Definite(relative(1.0)));
        self
    }

    fn min_w(mut self, width: impl Into<Length>) -> Self {
        self.style().min_size.width = Some(width.into());
        self
    }

    fn min_w_0(mut self) -> Self {
        self.style().min_size.width = Some(px(0.0).into());
        self
    }

    fn min_h(mut self, height: impl Into<Length>) -> Self {
        self.style().min_size.height = Some(height.into());
        self
    }

    fn min_h_full(mut self) -> Self {
        self.style().min_size.height = Some(Length::Definite(relative(1.0)));
        self
    }

    fn max_w_full(mut self) -> Self {
        self.style().max_size.width = Some(Length::Definite(relative(1.0)));
        self
    }

    fn gap(mut self, gap: impl Into<DefiniteLength>) -> Self {
        let g = gap.into();
        let s = self.style();
        s.gap.width = Some(g);
        s.gap.height = Some(g);
        self
    }

    fn top(mut self, v: impl Into<Length>) -> Self {
        self.style().inset.top = Some(v.into());
        self
    }

    fn top_0(mut self) -> Self {
        self.style().inset.top = Some(px(0.0).into());
        self
    }

    fn right(mut self, v: impl Into<Length>) -> Self {
        self.style().inset.right = Some(v.into());
        self
    }

    fn right_0(mut self) -> Self {
        self.style().inset.right = Some(px(0.0).into());
        self
    }

    fn bottom(mut self, v: impl Into<Length>) -> Self {
        self.style().inset.bottom = Some(v.into());
        self
    }

    fn bottom_0(mut self) -> Self {
        self.style().inset.bottom = Some(px(0.0).into());
        self
    }

    fn left(mut self, v: impl Into<Length>) -> Self {
        self.style().inset.left = Some(v.into());
        self
    }

    fn left_0(mut self) -> Self {
        self.style().inset.left = Some(px(0.0).into());
        self
    }

    fn p(mut self, v: impl Into<DefiniteLength>) -> Self {
        let v = v.into();
        let p = &mut self.style().padding;
        p.top = Some(v);
        p.right = Some(v);
        p.bottom = Some(v);
        p.left = Some(v);
        self
    }

    fn px(mut self, v: impl Into<DefiniteLength>) -> Self {
        let v = v.into();
        let p = &mut self.style().padding;
        p.left = Some(v);
        p.right = Some(v);
        self
    }

    fn py(mut self, v: impl Into<DefiniteLength>) -> Self {
        let v = v.into();
        let p = &mut self.style().padding;
        p.top = Some(v);
        p.bottom = Some(v);
        self
    }

    fn pt(mut self, v: impl Into<DefiniteLength>) -> Self {
        self.style().padding.top = Some(v.into());
        self
    }

    fn pr(mut self, v: impl Into<DefiniteLength>) -> Self {
        self.style().padding.right = Some(v.into());
        self
    }

    fn pb(mut self, v: impl Into<DefiniteLength>) -> Self {
        self.style().padding.bottom = Some(v.into());
        self
    }

    fn pl(mut self, v: impl Into<DefiniteLength>) -> Self {
        self.style().padding.left = Some(v.into());
        self
    }

    fn mt(mut self, v: impl Into<Length>) -> Self {
        self.style().margin.top = Some(v.into());
        self
    }

    fn mb(mut self, v: impl Into<Length>) -> Self {
        self.style().margin.bottom = Some(v.into());
        self
    }

    fn ml(mut self, v: impl Into<Length>) -> Self {
        self.style().margin.left = Some(v.into());
        self
    }

    fn border_1(mut self) -> Self {
        let b = &mut self.style().border_widths;
        b.top = Some(px(1.0));
        b.right = Some(px(1.0));
        b.bottom = Some(px(1.0));
        b.left = Some(px(1.0));
        self
    }

    fn border_2(mut self) -> Self {
        let b = &mut self.style().border_widths;
        b.top = Some(px(2.0));
        b.right = Some(px(2.0));
        b.bottom = Some(px(2.0));
        b.left = Some(px(2.0));
        self
    }

    fn border_t_1(mut self) -> Self {
        self.style().border_widths.top = Some(px(1.0));
        self
    }

    fn border_r_1(mut self) -> Self {
        self.style().border_widths.right = Some(px(1.0));
        self
    }

    fn border_b_1(mut self) -> Self {
        self.style().border_widths.bottom = Some(px(1.0));
        self
    }

    fn border_l_1(mut self) -> Self {
        self.style().border_widths.left = Some(px(1.0));
        self
    }

    fn border_color(mut self, color: Rgba) -> Self {
        self.style().border_color = Some(color);
        self
    }

    fn rounded(mut self, radius: Pixels) -> Self {
        let c = &mut self.style().corner_radii;
        c.top_left = Some(radius);
        c.top_right = Some(radius);
        c.bottom_right = Some(radius);
        c.bottom_left = Some(radius);
        self
    }

    fn rounded_t(mut self, radius: Pixels) -> Self {
        let c = &mut self.style().corner_radii;
        c.top_left = Some(radius);
        c.top_right = Some(radius);
        self
    }

    fn rounded_b(mut self, radius: Pixels) -> Self {
        let c = &mut self.style().corner_radii;
        c.bottom_left = Some(radius);
        c.bottom_right = Some(radius);
        self
    }

    fn rounded_full(self) -> Self {
        self.rounded(px(9999.0))
    }

    fn bg(mut self, color: Rgba) -> Self {
        self.style().background = Some(color);
        self
    }

    fn shadow(mut self, shadows: Vec<BoxShadow>) -> Self {
        self.style().box_shadow = Some(shadows);
        self
    }

    fn opacity(mut self, opacity: f32) -> Self {
        self.style().opacity = Some(opacity);
        self
    }

    fn overflow_hidden(mut self) -> Self {
        let o = &mut self.style().overflow;
        o.x = Some(Overflow::Hidden);
        o.y = Some(Overflow::Hidden);
        self
    }

    fn overflow_x_hidden(mut self) -> Self {
        self.style().overflow.x = Some(Overflow::Hidden);
        self
    }

    fn cursor_default(mut self) -> Self {
        self.style().cursor = Some(CursorStyle::Arrow);
        self
    }

    fn cursor_text(mut self) -> Self {
        self.style().cursor = Some(CursorStyle::IBeam);
        self
    }

    fn font_family(mut self, family: impl Into<SharedString>) -> Self {
        self.style().text.font_family = Some(family.into());
        self
    }

    fn font_weight(mut self, weight: FontWeight) -> Self {
        self.style().text.font_weight = Some(weight);
        self
    }

    fn text_size(mut self, size: Pixels) -> Self {
        self.style().text.font_size = Some(size);
        self
    }

    fn line_height(mut self, line_height: impl Into<DefiniteLength>) -> Self {
        self.style().text.line_height = Some(line_height.into());
        self
    }

    fn text_color(mut self, color: Rgba) -> Self {
        self.style().text.color = Some(color);
        self
    }

    fn text_center(mut self) -> Self {
        self.style().text.text_align = Some(TextAlign::Center);
        self
    }

    fn whitespace_nowrap(mut self) -> Self {
        self.style().text.white_space = Some(WhiteSpace::Nowrap);
        self
    }

    fn line_clamp(mut self, lines: usize) -> Self {
        let s = self.style();
        s.overflow.x = Some(Overflow::Hidden);
        s.overflow.y = Some(Overflow::Hidden);
        s.text.line_clamp = Some(lines);
        self
    }
}

impl Styled for StyleRefinement {
    fn style(&mut self) -> &mut StyleRefinement {
        self
    }
}
