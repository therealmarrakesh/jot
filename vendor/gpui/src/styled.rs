use crate::{
    self as gpui, AbsoluteLength, AlignContent, AlignItems, AlignSelf, CursorStyle, DefiniteLength,
    Display, Fill, FlexDirection, FlexWrap, Font, FontFeatures, FontStyle, FontWeight, Hsla,
    JustifyContent, Length, SharedString, StyleRefinement, TextAlign, TextOverflow,
    TextStyleRefinement, UnderlineStyle, WhiteSpace, px, relative, rems,
};
const ELLIPSIS: SharedString = SharedString::new_static("…");

/// A trait for elements that can be styled.
/// Use this to opt-in to a utility CSS-like styling API.
pub trait Styled: Sized {
    /// Returns a reference to the style memory of this element.
    fn style(&mut self) -> &mut StyleRefinement;

    // -- Visibility --
    fn visible(mut self) -> Self {
        self.style().visibility = Some(gpui::Visibility::Visible);
        self
    }
    fn invisible(mut self) -> Self {
        self.style().visibility = Some(gpui::Visibility::Hidden);
        self
    }

    // -- Position --
    fn relative(mut self) -> Self {
        self.style().position = Some(gpui::Position::Relative);
        self
    }
    fn absolute(mut self) -> Self {
        self.style().position = Some(gpui::Position::Absolute);
        self
    }
    fn inset(mut self, length: impl Clone + Into<Length>) -> Self {
        let s = self.style();
        s.inset.top = Some(length.clone().into());
        s.inset.right = Some(length.clone().into());
        s.inset.bottom = Some(length.clone().into());
        s.inset.left = Some(length.into());
        self
    }
    fn inset_0(self) -> Self { self.inset(px(0.)) }
    fn top(mut self, length: impl Into<Length>) -> Self { self.style().inset.top = Some(length.into()); self }
    fn top_0(self) -> Self { self.top(px(0.)) }
    fn bottom(mut self, length: impl Into<Length>) -> Self { self.style().inset.bottom = Some(length.into()); self }
    fn bottom_0(self) -> Self { self.bottom(px(0.)) }
    fn left(mut self, length: impl Into<Length>) -> Self { self.style().inset.left = Some(length.into()); self }
    fn left_0(self) -> Self { self.left(px(0.)) }
    fn right(mut self, length: impl Into<Length>) -> Self { self.style().inset.right = Some(length.into()); self }
    fn right_0(self) -> Self { self.right(px(0.)) }

    // -- Overflow --
    fn overflow_hidden(mut self) -> Self {
        self.style().overflow.x = Some(gpui::Overflow::Hidden);
        self.style().overflow.y = Some(gpui::Overflow::Hidden);
        self
    }
    fn overflow_x_hidden(mut self) -> Self {
        self.style().overflow.x = Some(gpui::Overflow::Hidden);
        self
    }
    fn overflow_y_hidden(mut self) -> Self {
        self.style().overflow.y = Some(gpui::Overflow::Hidden);
        self
    }

    // -- Cursor --
    fn cursor(mut self, cursor: CursorStyle) -> Self { self.style().mouse_cursor = Some(cursor); self }
    fn cursor_default(self) -> Self { self.cursor(CursorStyle::Arrow) }
    fn cursor_pointer(self) -> Self { self.cursor(CursorStyle::PointingHand) }
    fn cursor_text(self) -> Self { self.cursor(CursorStyle::IBeam) }
    fn cursor_move(self) -> Self { self.cursor(CursorStyle::ClosedHand) }
    fn cursor_not_allowed(self) -> Self { self.cursor(CursorStyle::OperationNotAllowed) }
    fn cursor_context_menu(self) -> Self { self.cursor(CursorStyle::ContextualMenu) }
    fn cursor_crosshair(self) -> Self { self.cursor(CursorStyle::Crosshair) }
    fn cursor_vertical_text(self) -> Self { self.cursor(CursorStyle::IBeamCursorForVerticalLayout) }
    fn cursor_alias(self) -> Self { self.cursor(CursorStyle::DragLink) }
    fn cursor_copy(self) -> Self { self.cursor(CursorStyle::DragCopy) }
    fn cursor_no_drop(self) -> Self { self.cursor(CursorStyle::OperationNotAllowed) }
    fn cursor_grab(self) -> Self { self.cursor(CursorStyle::OpenHand) }
    fn cursor_grabbing(self) -> Self { self.cursor(CursorStyle::ClosedHand) }
    fn cursor_ew_resize(self) -> Self { self.cursor(CursorStyle::ResizeLeftRight) }
    fn cursor_ns_resize(self) -> Self { self.cursor(CursorStyle::ResizeUpDown) }
    fn cursor_nesw_resize(self) -> Self { self.cursor(CursorStyle::ResizeUpRightDownLeft) }
    fn cursor_nwse_resize(self) -> Self { self.cursor(CursorStyle::ResizeUpLeftDownRight) }
    fn cursor_col_resize(self) -> Self { self.cursor(CursorStyle::ResizeColumn) }
    fn cursor_row_resize(self) -> Self { self.cursor(CursorStyle::ResizeRow) }
    fn cursor_n_resize(self) -> Self { self.cursor(CursorStyle::ResizeUp) }
    fn cursor_e_resize(self) -> Self { self.cursor(CursorStyle::ResizeRight) }
    fn cursor_s_resize(self) -> Self { self.cursor(CursorStyle::ResizeDown) }
    fn cursor_w_resize(self) -> Self { self.cursor(CursorStyle::ResizeLeft) }

    // -- Margin --
    fn m(mut self, length: impl Clone + Into<Length>) -> Self {
        let s = self.style();
        s.margin.top = Some(length.clone().into());
        s.margin.bottom = Some(length.clone().into());
        s.margin.left = Some(length.clone().into());
        s.margin.right = Some(length.into());
        self
    }
    fn m_0(self) -> Self { self.m(px(0.)) }
    fn m_1(self) -> Self { self.m(rems(0.25)) }
    fn m_2(self) -> Self { self.m(rems(0.5)) }
    fn m_auto(self) -> Self { self.m(gpui::auto()) }
    fn mx(mut self, length: impl Clone + Into<Length>) -> Self {
        let s = self.style();
        s.margin.left = Some(length.clone().into());
        s.margin.right = Some(length.into());
        self
    }
    fn mx_auto(self) -> Self { self.mx(gpui::auto()) }
    fn my(mut self, length: impl Clone + Into<Length>) -> Self {
        let s = self.style();
        s.margin.top = Some(length.clone().into());
        s.margin.bottom = Some(length.into());
        self
    }
    fn mt(mut self, length: impl Into<Length>) -> Self { self.style().margin.top = Some(length.into()); self }
    fn mt_0(self) -> Self { self.mt(px(0.)) }
    fn mt_1(self) -> Self { self.mt(rems(0.25)) }
    fn mt_2(self) -> Self { self.mt(rems(0.5)) }
    fn mb(mut self, length: impl Into<Length>) -> Self { self.style().margin.bottom = Some(length.into()); self }
    fn mb_0(self) -> Self { self.mb(px(0.)) }
    fn mb_1(self) -> Self { self.mb(rems(0.25)) }
    fn mb_2(self) -> Self { self.mb(rems(0.5)) }
    fn ml(mut self, length: impl Into<Length>) -> Self { self.style().margin.left = Some(length.into()); self }
    fn ml_auto(self) -> Self { self.ml(gpui::auto()) }
    fn mr(mut self, length: impl Into<Length>) -> Self { self.style().margin.right = Some(length.into()); self }

    // -- Padding --
    fn p(mut self, length: impl Clone + Into<DefiniteLength>) -> Self {
        let s = self.style();
        s.padding.top = Some(length.clone().into());
        s.padding.bottom = Some(length.clone().into());
        s.padding.left = Some(length.clone().into());
        s.padding.right = Some(length.into());
        self
    }
    fn p_0(self) -> Self { self.p(px(0.)) }
    fn p_1(self) -> Self { self.p(rems(0.25)) }
    fn p_2(self) -> Self { self.p(rems(0.5)) }
    fn p_3(self) -> Self { self.p(rems(0.75)) }
    fn p_4(self) -> Self { self.p(rems(1.)) }
    fn px(mut self, length: impl Clone + Into<DefiniteLength>) -> Self {
        let s = self.style();
        s.padding.left = Some(length.clone().into());
        s.padding.right = Some(length.into());
        self
    }
    fn px_1(self) -> Self { self.px(rems(0.25)) }
    fn px_2(self) -> Self { self.px(rems(0.5)) }
    fn px_3(self) -> Self { self.px(rems(0.75)) }
    fn px_4(self) -> Self { self.px(rems(1.)) }
    fn py(mut self, length: impl Clone + Into<DefiniteLength>) -> Self {
        let s = self.style();
        s.padding.top = Some(length.clone().into());
        s.padding.bottom = Some(length.into());
        self
    }
    fn py_1(self) -> Self { self.py(rems(0.25)) }
    fn py_2(self) -> Self { self.py(rems(0.5)) }
    fn pt(mut self, length: impl Into<DefiniteLength>) -> Self { self.style().padding.top = Some(length.into()); self }
    fn pb(mut self, length: impl Into<DefiniteLength>) -> Self { self.style().padding.bottom = Some(length.into()); self }
    fn pl(mut self, length: impl Into<DefiniteLength>) -> Self { self.style().padding.left = Some(length.into()); self }
    fn pr(mut self, length: impl Into<DefiniteLength>) -> Self { self.style().padding.right = Some(length.into()); self }

    // -- Sizing & Gap --
    fn w(mut self, length: impl Into<Length>) -> Self { self.style().size.width = Some(length.into()); self }
    fn w_0(self) -> Self { self.w(px(0.)) }
    fn w_72(self) -> Self { self.w(rems(18.)) }
    fn w_full(self) -> Self { self.w(relative(1.)) }
    fn w_auto(self) -> Self { self.w(gpui::auto()) }
    fn h(mut self, length: impl Into<Length>) -> Self { self.style().size.height = Some(length.into()); self }
    fn h_0(self) -> Self { self.h(px(0.)) }
    fn h_full(self) -> Self { self.h(relative(1.)) }
    fn h_auto(self) -> Self { self.h(gpui::auto()) }
    fn size(mut self, length: impl Clone + Into<Length>) -> Self {
        let s = self.style();
        s.size.width = Some(length.clone().into());
        s.size.height = Some(length.into());
        self
    }
    fn size_full(self) -> Self { self.size(relative(1.)) }
    fn min_size(mut self, length: impl Clone + Into<Length>) -> Self {
        let s = self.style();
        s.min_size.width = Some(length.clone().into());
        s.min_size.height = Some(length.into());
        self
    }
    fn min_w(mut self, length: impl Into<Length>) -> Self { self.style().min_size.width = Some(length.into()); self }
    fn min_w_0(self) -> Self { self.min_w(px(0.)) }
    fn min_w_full(self) -> Self { self.min_w(relative(1.)) }
    fn min_h(mut self, length: impl Into<Length>) -> Self { self.style().min_size.height = Some(length.into()); self }
    fn min_h_0(self) -> Self { self.min_h(px(0.)) }
    fn min_h_full(self) -> Self { self.min_h(relative(1.)) }
    fn max_size(mut self, length: impl Clone + Into<Length>) -> Self {
        let s = self.style();
        s.max_size.width = Some(length.clone().into());
        s.max_size.height = Some(length.into());
        self
    }
    fn max_w(mut self, length: impl Into<Length>) -> Self { self.style().max_size.width = Some(length.into()); self }
    fn max_w_full(self) -> Self { self.max_w(relative(1.)) }
    fn max_h(mut self, length: impl Into<Length>) -> Self { self.style().max_size.height = Some(length.into()); self }
    fn max_h_full(self) -> Self { self.max_h(relative(1.)) }
    fn gap(mut self, length: impl Clone + Into<DefiniteLength>) -> Self {
        let s = self.style();
        s.gap.width = Some(length.clone().into());
        s.gap.height = Some(length.into());
        self
    }
    fn gap_1(self) -> Self { self.gap(rems(0.25)) }
    fn gap_2(self) -> Self { self.gap(rems(0.5)) }
    fn gap_3(self) -> Self { self.gap(rems(0.75)) }
    fn gap_4(self) -> Self { self.gap(rems(1.)) }
    fn gap_x(mut self, length: impl Into<DefiniteLength>) -> Self { self.style().gap.width = Some(length.into()); self }
    fn gap_y(mut self, length: impl Into<DefiniteLength>) -> Self { self.style().gap.height = Some(length.into()); self }

    // -- Corners --
    fn rounded(mut self, radius: impl Clone + Into<AbsoluteLength>) -> Self {
        let s = self.style();
        s.corner_radii.top_left = Some(radius.clone().into());
        s.corner_radii.top_right = Some(radius.clone().into());
        s.corner_radii.bottom_right = Some(radius.clone().into());
        s.corner_radii.bottom_left = Some(radius.into());
        self
    }
    fn rounded_none(self) -> Self { self.rounded(px(0.)) }
    fn rounded_xs(self) -> Self { self.rounded(rems(0.125)) }
    fn rounded_sm(self) -> Self { self.rounded(rems(0.25)) }
    fn rounded_md(self) -> Self { self.rounded(rems(0.375)) }
    fn rounded_lg(self) -> Self { self.rounded(rems(0.5)) }
    fn rounded_xl(self) -> Self { self.rounded(rems(0.75)) }
    fn rounded_full(self) -> Self { self.rounded(px(9999.)) }
    fn rounded_t(mut self, radius: impl Clone + Into<AbsoluteLength>) -> Self {
        let s = self.style();
        s.corner_radii.top_left = Some(radius.clone().into());
        s.corner_radii.top_right = Some(radius.into());
        self
    }
    fn rounded_b(mut self, radius: impl Clone + Into<AbsoluteLength>) -> Self {
        let s = self.style();
        s.corner_radii.bottom_left = Some(radius.clone().into());
        s.corner_radii.bottom_right = Some(radius.into());
        self
    }
    fn rounded_l(mut self, radius: impl Clone + Into<AbsoluteLength>) -> Self {
        let s = self.style();
        s.corner_radii.top_left = Some(radius.clone().into());
        s.corner_radii.bottom_left = Some(radius.into());
        self
    }
    fn rounded_r(mut self, radius: impl Clone + Into<AbsoluteLength>) -> Self {
        let s = self.style();
        s.corner_radii.top_right = Some(radius.clone().into());
        s.corner_radii.bottom_right = Some(radius.into());
        self
    }
    fn rounded_tl(mut self, radius: impl Into<AbsoluteLength>) -> Self { self.style().corner_radii.top_left = Some(radius.into()); self }
    fn rounded_tr(mut self, radius: impl Into<AbsoluteLength>) -> Self { self.style().corner_radii.top_right = Some(radius.into()); self }
    fn rounded_bl(mut self, radius: impl Into<AbsoluteLength>) -> Self { self.style().corner_radii.bottom_left = Some(radius.into()); self }
    fn rounded_br(mut self, radius: impl Into<AbsoluteLength>) -> Self { self.style().corner_radii.bottom_right = Some(radius.into()); self }

    // -- Border --
    fn border_color<C: Into<Hsla>>(mut self, border_color: C) -> Self {
        self.style().border_color = Some(border_color.into());
        self
    }
    fn border(mut self, width: impl Clone + Into<AbsoluteLength>) -> Self {
        let s = self.style();
        s.border_widths.top = Some(width.clone().into());
        s.border_widths.right = Some(width.clone().into());
        s.border_widths.bottom = Some(width.clone().into());
        s.border_widths.left = Some(width.into());
        self
    }
    fn border_0(self) -> Self { self.border(px(0.)) }
    fn border_1(self) -> Self { self.border(px(1.)) }
    fn border_2(self) -> Self { self.border(px(2.)) }
    fn border_t(mut self, width: impl Into<AbsoluteLength>) -> Self { self.style().border_widths.top = Some(width.into()); self }
    fn border_t_1(self) -> Self { self.border_t(px(1.)) }
    fn border_b(mut self, width: impl Into<AbsoluteLength>) -> Self { self.style().border_widths.bottom = Some(width.into()); self }
    fn border_b_1(self) -> Self { self.border_b(px(1.)) }
    fn border_l(mut self, width: impl Into<AbsoluteLength>) -> Self { self.style().border_widths.left = Some(width.into()); self }
    fn border_l_1(self) -> Self { self.border_l(px(1.)) }
    fn border_r(mut self, width: impl Into<AbsoluteLength>) -> Self { self.style().border_widths.right = Some(width.into()); self }
    fn border_r_1(self) -> Self { self.border_r(px(1.)) }
    fn border_x(mut self, width: impl Clone + Into<AbsoluteLength>) -> Self {
        let s = self.style();
        s.border_widths.left = Some(width.clone().into());
        s.border_widths.right = Some(width.into());
        self
    }
    fn border_y(mut self, width: impl Clone + Into<AbsoluteLength>) -> Self {
        let s = self.style();
        s.border_widths.top = Some(width.clone().into());
        s.border_widths.bottom = Some(width.into());
        self
    }

    // -- Box Shadow --
    fn shadow(mut self, shadows: Vec<gpui::BoxShadow>) -> Self {
        self.style().box_shadow = Some(shadows);
        self
    }
    fn shadow_none(mut self) -> Self {
        self.style().box_shadow = Some(Default::default());
        self
    }
    fn shadow_sm(mut self) -> Self {
        use gpui::{BoxShadow, hsla};
        self.style().box_shadow = Some(vec![
            BoxShadow::new(px(0.), px(1.), hsla(0., 0., 0., 0.1)).blur_radius(px(3.)),
            BoxShadow::new(px(0.), px(1.), hsla(0., 0., 0., 0.1)).blur_radius(px(2.)).spread_radius(px(-1.)),
        ]);
        self
    }
    fn shadow_md(mut self) -> Self {
        use gpui::{BoxShadow, hsla};
        self.style().box_shadow = Some(vec![
            BoxShadow::new(px(0.), px(4.), hsla(0., 0., 0., 0.1)).blur_radius(px(6.)).spread_radius(px(-1.)),
            BoxShadow::new(px(0.), px(2.), hsla(0., 0., 0., 0.1)).blur_radius(px(4.)).spread_radius(px(-2.)),
        ]);
        self
    }
    fn shadow_lg(mut self) -> Self {
        use gpui::{BoxShadow, hsla};
        self.style().box_shadow = Some(vec![
            BoxShadow::new(px(0.), px(10.), hsla(0., 0., 0., 0.1)).blur_radius(px(15.)).spread_radius(px(-3.)),
            BoxShadow::new(px(0.), px(4.), hsla(0., 0., 0., 0.1)).blur_radius(px(6.)).spread_radius(px(-4.)),
        ]);
        self
    }

    /// Sets the display type of the element to `block`.
    /// [Docs](https://tailwindcss.com/docs/display)
    fn block(mut self) -> Self {
        self.style().display = Some(Display::Block);
        self
    }

    /// Sets the display type of the element to `flex`.
    /// [Docs](https://tailwindcss.com/docs/display)
    fn flex(mut self) -> Self {
        self.style().display = Some(Display::Flex);
        self
    }

    /// Sets the display type of the element to `none`.
    /// [Docs](https://tailwindcss.com/docs/display)
    fn hidden(mut self) -> Self {
        self.style().display = Some(Display::None);
        self
    }

    /// Set the space to be reserved for rendering the scrollbar.
    ///
    /// This will only affect the layout of the element when overflow for this element is set to
    /// `Overflow::Scroll`.
    fn scrollbar_width(mut self, width: impl Into<AbsoluteLength>) -> Self {
        self.style().scrollbar_width = Some(width.into());
        self
    }

    /// Sets the whitespace of the element to `normal`.
    /// [Docs](https://tailwindcss.com/docs/whitespace#normal)
    fn whitespace_normal(mut self) -> Self {
        self.text_style().white_space = Some(WhiteSpace::Normal);
        self
    }

    /// Sets the whitespace of the element to `nowrap`.
    /// [Docs](https://tailwindcss.com/docs/whitespace#nowrap)
    fn whitespace_nowrap(mut self) -> Self {
        self.text_style().white_space = Some(WhiteSpace::Nowrap);
        self
    }

    /// Sets the truncate overflowing text with an ellipsis (…) at the end if needed.
    /// [Docs](https://tailwindcss.com/docs/text-overflow#ellipsis)
    fn text_ellipsis(mut self) -> Self {
        self.text_style().text_overflow = Some(TextOverflow::Truncate(ELLIPSIS));
        self
    }

    /// Sets the truncate overflowing text with an ellipsis (…) at the start if needed.
    /// Typically more adequate for file paths where the end is more important than the beginning.
    /// Note: This doesn't exist in Tailwind CSS.
    fn text_ellipsis_start(mut self) -> Self {
        self.text_style().text_overflow = Some(TextOverflow::TruncateStart(ELLIPSIS));
        self
    }

    /// Sets the truncate overflowing text with an ellipsis (…) in the middle if needed.
    /// Preserves the beginning and end of the text. Useful for filenames.
    /// Note: This doesn't exist in Tailwind CSS.
    fn text_ellipsis_middle(mut self) -> Self {
        self.text_style().text_overflow = Some(TextOverflow::TruncateMiddle(ELLIPSIS));
        self
    }

    /// Sets the text overflow behavior of the element.
    fn text_overflow(mut self, overflow: TextOverflow) -> Self {
        self.text_style().text_overflow = Some(overflow);
        self
    }

    /// Set the text alignment of the element.
    fn text_align(mut self, align: TextAlign) -> Self {
        self.text_style().text_align = Some(align);
        self
    }

    /// Sets the text alignment to left
    fn text_left(mut self) -> Self {
        self.text_align(TextAlign::Left)
    }

    /// Sets the text alignment to center
    fn text_center(mut self) -> Self {
        self.text_align(TextAlign::Center)
    }

    /// Sets the text alignment to right
    fn text_right(mut self) -> Self {
        self.text_align(TextAlign::Right)
    }

    /// Sets the truncate to prevent text from wrapping and truncate overflowing text with an ellipsis (…) if needed.
    /// [Docs](https://tailwindcss.com/docs/text-overflow#truncate)
    fn truncate(mut self) -> Self {
        self.overflow_hidden().whitespace_nowrap().text_ellipsis()
    }

    /// Sets number of lines to show before truncating the text.
    /// [Docs](https://tailwindcss.com/docs/line-clamp)
    fn line_clamp(mut self, lines: usize) -> Self {
        let mut text_style = self.text_style();
        text_style.line_clamp = Some(lines);
        self.overflow_hidden()
    }

    /// Sets the flex direction of the element to `column`.
    /// [Docs](https://tailwindcss.com/docs/flex-direction#column)
    fn flex_col(mut self) -> Self {
        self.style().flex_direction = Some(FlexDirection::Column);
        self
    }

    /// Sets the flex direction of the element to `column-reverse`.
    /// [Docs](https://tailwindcss.com/docs/flex-direction#column-reverse)
    fn flex_col_reverse(mut self) -> Self {
        self.style().flex_direction = Some(FlexDirection::ColumnReverse);
        self
    }

    /// Sets the flex direction of the element to `row`.
    /// [Docs](https://tailwindcss.com/docs/flex-direction#row)
    fn flex_row(mut self) -> Self {
        self.style().flex_direction = Some(FlexDirection::Row);
        self
    }

    /// Sets the flex direction of the element to `row-reverse`.
    /// [Docs](https://tailwindcss.com/docs/flex-direction#row-reverse)
    fn flex_row_reverse(mut self) -> Self {
        self.style().flex_direction = Some(FlexDirection::RowReverse);
        self
    }

    /// Sets the element to allow a flex item to grow and shrink as needed, ignoring its initial size.
    /// [Docs](https://tailwindcss.com/docs/flex#flex-1)
    fn flex_1(mut self) -> Self {
        self.style().flex_grow = Some(1.);
        self.style().flex_shrink = Some(1.);
        self.style().flex_basis = Some(relative(0.).into());
        self
    }

    /// Sets the element to allow a flex item to grow and shrink, taking into account its initial size.
    /// [Docs](https://tailwindcss.com/docs/flex#auto)
    fn flex_auto(mut self) -> Self {
        self.style().flex_grow = Some(1.);
        self.style().flex_shrink = Some(1.);
        self.style().flex_basis = Some(Length::Auto);
        self
    }

    /// Sets the element to allow a flex item to shrink but not grow, taking into account its initial size.
    /// [Docs](https://tailwindcss.com/docs/flex#initial)
    fn flex_initial(mut self) -> Self {
        self.style().flex_grow = Some(0.);
        self.style().flex_shrink = Some(1.);
        self.style().flex_basis = Some(Length::Auto);
        self
    }

    /// Sets the element to prevent a flex item from growing or shrinking.
    /// [Docs](https://tailwindcss.com/docs/flex#none)
    fn flex_none(mut self) -> Self {
        self.style().flex_grow = Some(0.);
        self.style().flex_shrink = Some(0.);
        self.style().flex_basis = Some(Length::Auto);
        self
    }

    /// Sets the initial size of flex items for this element.
    /// [Docs](https://tailwindcss.com/docs/flex-basis)
    fn flex_basis(mut self, basis: impl Into<Length>) -> Self {
        self.style().flex_basis = Some(basis.into());
        self
    }

    /// Sets the flex item's grow factor.
    /// [Docs](https://tailwindcss.com/docs/flex-grow)
    fn flex_grow(mut self, grow: f32) -> Self {
        self.style().flex_grow = Some(grow);
        self
    }

    /// Disables flex item growth (flex-grow: 0).
    /// [Docs](https://tailwindcss.com/docs/flex-grow#dont-grow)
    fn flex_grow_0(mut self) -> Self {
        self.style().flex_grow = Some(0.);
        self
    }

    /// Enables flex item growth (flex-grow: 1).
    /// [Docs](https://tailwindcss.com/docs/flex-grow#grow-1)
    fn flex_grow_1(mut self) -> Self {
        self.style().flex_grow = Some(1.);
        self
    }

    /// Sets the flex item's shrink factor.
    /// [Docs](https://tailwindcss.com/docs/flex-shrink)
    fn flex_shrink(mut self, shrink: f32) -> Self {
        self.style().flex_shrink = Some(shrink);
        self
    }

    /// Disables flex item shrinking (flex-shrink: 0).
    /// [Docs](https://tailwindcss.com/docs/flex-shrink#dont-shrink)
    fn flex_shrink_0(mut self) -> Self {
        self.style().flex_shrink = Some(0.);
        self
    }

    /// Enables flex item shrinking (flex-shrink: 1).
    /// [Docs](https://tailwindcss.com/docs/flex-shrink#shrink-1)
    fn flex_shrink_1(mut self) -> Self {
        self.style().flex_shrink = Some(1.);
        self
    }

    /// Sets the element to allow flex items to wrap.
    /// [Docs](https://tailwindcss.com/docs/flex-wrap#wrap-normally)
    fn flex_wrap(mut self) -> Self {
        self.style().flex_wrap = Some(FlexWrap::Wrap);
        self
    }

    /// Sets the element wrap flex items in the reverse direction.
    /// [Docs](https://tailwindcss.com/docs/flex-wrap#wrap-reversed)
    fn flex_wrap_reverse(mut self) -> Self {
        self.style().flex_wrap = Some(FlexWrap::WrapReverse);
        self
    }

    /// Sets the element to prevent flex items from wrapping, causing inflexible items to overflow the container if necessary.
    /// [Docs](https://tailwindcss.com/docs/flex-wrap#dont-wrap)
    fn flex_nowrap(mut self) -> Self {
        self.style().flex_wrap = Some(FlexWrap::NoWrap);
        self
    }

    /// Sets the element to align flex items to the start of the container's cross axis.
    /// [Docs](https://tailwindcss.com/docs/align-items#start)
    fn items_start(mut self) -> Self {
        self.style().align_items = Some(AlignItems::FlexStart);
        self
    }

    /// Sets the element to align flex items to the end of the container's cross axis.
    /// [Docs](https://tailwindcss.com/docs/align-items#end)
    fn items_end(mut self) -> Self {
        self.style().align_items = Some(AlignItems::FlexEnd);
        self
    }

    /// Sets the element to align flex items along the center of the container's cross axis.
    /// [Docs](https://tailwindcss.com/docs/align-items#center)
    fn items_center(mut self) -> Self {
        self.style().align_items = Some(AlignItems::Center);
        self
    }

    /// Sets the element to align flex items along the baseline of the container's cross axis.
    /// [Docs](https://tailwindcss.com/docs/align-items#baseline)
    fn items_baseline(mut self) -> Self {
        self.style().align_items = Some(AlignItems::Baseline);
        self
    }

    /// Sets the element to stretch flex items to fill the available space along the container's cross axis.
    /// [Docs](https://tailwindcss.com/docs/align-items#stretch)
    fn items_stretch(mut self) -> Self {
        self.style().align_items = Some(AlignItems::Stretch);
        self
    }

    /// Sets how this specific element is aligned along the container's cross axis.
    /// [Docs](https://tailwindcss.com/docs/align-self#start)
    fn self_start(mut self) -> Self {
        self.style().align_self = Some(AlignSelf::Start);
        self
    }

    /// Sets this element to align against the end of the container's cross axis.
    /// [Docs](https://tailwindcss.com/docs/align-self#end)
    fn self_end(mut self) -> Self {
        self.style().align_self = Some(AlignSelf::End);
        self
    }

    /// Sets this element to align against the start of the container's cross axis.
    /// [Docs](https://tailwindcss.com/docs/align-self#start)
    fn self_flex_start(mut self) -> Self {
        self.style().align_self = Some(AlignSelf::FlexStart);
        self
    }

    /// Sets this element to align against the end of the container's cross axis.
    /// [Docs](https://tailwindcss.com/docs/align-self#end)
    fn self_flex_end(mut self) -> Self {
        self.style().align_self = Some(AlignSelf::FlexEnd);
        self
    }

    /// Sets this element to align along the center of the container's cross axis.
    /// [Docs](https://tailwindcss.com/docs/align-self#center)
    fn self_center(mut self) -> Self {
        self.style().align_self = Some(AlignSelf::Center);
        self
    }

    /// Sets this element to align along the baseline of the container's cross axis.
    /// [Docs](https://tailwindcss.com/docs/align-self#baseline)
    fn self_baseline(mut self) -> Self {
        self.style().align_self = Some(AlignSelf::Baseline);
        self
    }

    /// Sets this element to stretch to fill the available space along the container's cross axis.
    /// [Docs](https://tailwindcss.com/docs/align-self#stretch)
    fn self_stretch(mut self) -> Self {
        self.style().align_self = Some(AlignSelf::Stretch);
        self
    }

    /// Sets the element to justify flex items against the start of the container's main axis.
    /// [Docs](https://tailwindcss.com/docs/justify-content#start)
    fn justify_start(mut self) -> Self {
        self.style().justify_content = Some(JustifyContent::Start);
        self
    }

    /// Sets the element to justify flex items against the end of the container's main axis.
    /// [Docs](https://tailwindcss.com/docs/justify-content#end)
    fn justify_end(mut self) -> Self {
        self.style().justify_content = Some(JustifyContent::End);
        self
    }

    /// Sets the element to justify flex items along the center of the container's main axis.
    /// [Docs](https://tailwindcss.com/docs/justify-content#center)
    fn justify_center(mut self) -> Self {
        self.style().justify_content = Some(JustifyContent::Center);
        self
    }

    /// Sets the element to justify flex items along the container's main axis
    /// such that there is an equal amount of space between each item.
    /// [Docs](https://tailwindcss.com/docs/justify-content#space-between)
    fn justify_between(mut self) -> Self {
        self.style().justify_content = Some(JustifyContent::SpaceBetween);
        self
    }

    /// Sets the element to justify items along the container's main axis such
    /// that there is an equal amount of space on each side of each item.
    /// [Docs](https://tailwindcss.com/docs/justify-content#space-around)
    fn justify_around(mut self) -> Self {
        self.style().justify_content = Some(JustifyContent::SpaceAround);
        self
    }

    /// Sets the element to justify items along the container's main axis such
    /// that there is an equal amount of space around each item, but also
    /// accounting for the doubling of space you would normally see between
    /// each item when using justify-around.
    /// [Docs](https://tailwindcss.com/docs/justify-content#space-evenly)
    fn justify_evenly(mut self) -> Self {
        self.style().justify_content = Some(JustifyContent::SpaceEvenly);
        self
    }

    /// Sets the element to pack content items in their default position as if no align-content value was set.
    /// [Docs](https://tailwindcss.com/docs/align-content#normal)
    fn content_normal(mut self) -> Self {
        self.style().align_content = None;
        self
    }

    /// Sets the element to pack content items in the center of the container's cross axis.
    /// [Docs](https://tailwindcss.com/docs/align-content#center)
    fn content_center(mut self) -> Self {
        self.style().align_content = Some(AlignContent::Center);
        self
    }

    /// Sets the element to pack content items against the start of the container's cross axis.
    /// [Docs](https://tailwindcss.com/docs/align-content#start)
    fn content_start(mut self) -> Self {
        self.style().align_content = Some(AlignContent::FlexStart);
        self
    }

    /// Sets the element to pack content items against the end of the container's cross axis.
    /// [Docs](https://tailwindcss.com/docs/align-content#end)
    fn content_end(mut self) -> Self {
        self.style().align_content = Some(AlignContent::FlexEnd);
        self
    }

    /// Sets the element to pack content items along the container's cross axis
    /// such that there is an equal amount of space between each item.
    /// [Docs](https://tailwindcss.com/docs/align-content#space-between)
    fn content_between(mut self) -> Self {
        self.style().align_content = Some(AlignContent::SpaceBetween);
        self
    }

    /// Sets the element to pack content items along the container's cross axis
    /// such that there is an equal amount of space on each side of each item.
    /// [Docs](https://tailwindcss.com/docs/align-content#space-around)
    fn content_around(mut self) -> Self {
        self.style().align_content = Some(AlignContent::SpaceAround);
        self
    }

    /// Sets the element to pack content items along the container's cross axis
    /// such that there is an equal amount of space between each item.
    /// [Docs](https://tailwindcss.com/docs/align-content#space-evenly)
    fn content_evenly(mut self) -> Self {
        self.style().align_content = Some(AlignContent::SpaceEvenly);
        self
    }

    /// Sets the element to allow content items to fill the available space along the container's cross axis.
    /// [Docs](https://tailwindcss.com/docs/align-content#stretch)
    fn content_stretch(mut self) -> Self {
        self.style().align_content = Some(AlignContent::Stretch);
        self
    }

    /// Sets the aspect ratio of the element.
    /// [Docs](https://tailwindcss.com/docs/aspect-ratio)
    fn aspect_ratio(mut self, ratio: f32) -> Self {
        self.style().aspect_ratio = Some(ratio);
        self
    }

    /// Sets the aspect ratio of the element to 1/1 – equal width and height.
    /// [Docs](https://tailwindcss.com/docs/aspect-ratio)
    fn aspect_square(mut self) -> Self {
        self.style().aspect_ratio = Some(1.0);
        self
    }

    /// Sets the background color of the element.
    fn bg<F>(mut self, fill: F) -> Self
    where
        F: Into<Fill>,
        Self: Sized,
    {
        self.style().background = Some(fill.into());
        self
    }

    /// Returns a mutable reference to the text style that has been configured on this element.
    fn text_style(&mut self) -> &mut TextStyleRefinement {
        let style: &mut StyleRefinement = self.style();
        &mut style.text
    }

    /// Sets the text color of this element.
    ///
    /// This value cascades to its child elements.
    fn text_color(mut self, color: impl Into<Hsla>) -> Self {
        self.text_style().color = Some(color.into());
        self
    }

    /// Sets the font weight of this element
    ///
    /// This value cascades to its child elements.
    fn font_weight(mut self, weight: FontWeight) -> Self {
        self.text_style().font_weight = Some(weight);
        self
    }

    /// Sets the background color of this element.
    ///
    /// This value cascades to its child elements.
    fn text_bg(mut self, bg: impl Into<Hsla>) -> Self {
        self.text_style().background_color = Some(bg.into());
        self
    }

    /// Sets the text size of this element.
    ///
    /// This value cascades to its child elements.
    fn text_size(mut self, size: impl Into<AbsoluteLength>) -> Self {
        self.text_style().font_size = Some(size.into());
        self
    }

    /// Sets the text size to 'extra small'.
    /// [Docs](https://tailwindcss.com/docs/font-size#setting-the-font-size)
    fn text_xs(mut self) -> Self {
        self.text_style().font_size = Some(rems(0.75).into());
        self
    }

    /// Sets the text size to 'small'.
    /// [Docs](https://tailwindcss.com/docs/font-size#setting-the-font-size)
    fn text_sm(mut self) -> Self {
        self.text_style().font_size = Some(rems(0.875).into());
        self
    }

    /// Sets the text size to 'base'.
    /// [Docs](https://tailwindcss.com/docs/font-size#setting-the-font-size)
    fn text_base(mut self) -> Self {
        self.text_style().font_size = Some(rems(1.0).into());
        self
    }

    /// Sets the text size to 'large'.
    /// [Docs](https://tailwindcss.com/docs/font-size#setting-the-font-size)
    fn text_lg(mut self) -> Self {
        self.text_style().font_size = Some(rems(1.125).into());
        self
    }

    /// Sets the text size to 'extra large'.
    /// [Docs](https://tailwindcss.com/docs/font-size#setting-the-font-size)
    fn text_xl(mut self) -> Self {
        self.text_style().font_size = Some(rems(1.25).into());
        self
    }

    /// Sets the text size to 'extra extra large'.
    /// [Docs](https://tailwindcss.com/docs/font-size#setting-the-font-size)
    fn text_2xl(mut self) -> Self {
        self.text_style().font_size = Some(rems(1.5).into());
        self
    }

    /// Sets the text size to 'extra extra extra large'.
    /// [Docs](https://tailwindcss.com/docs/font-size#setting-the-font-size)
    fn text_3xl(mut self) -> Self {
        self.text_style().font_size = Some(rems(1.875).into());
        self
    }

    /// Sets the font style of the element to italic.
    /// [Docs](https://tailwindcss.com/docs/font-style#italicizing-text)
    fn italic(mut self) -> Self {
        self.text_style().font_style = Some(FontStyle::Italic);
        self
    }

    /// Sets the font style of the element to normal (not italic).
    /// [Docs](https://tailwindcss.com/docs/font-style#displaying-text-normally)
    fn not_italic(mut self) -> Self {
        self.text_style().font_style = Some(FontStyle::Normal);
        self
    }

    /// Sets the text decoration to underline.
    /// [Docs](https://tailwindcss.com/docs/text-decoration-line#underling-text)
    fn underline(mut self) -> Self {
        let style = self.text_style();
        style.underline = Some(UnderlineStyle {
            thickness: px(1.),
            ..Default::default()
        });
        self
    }

    /// Removes the text decoration on this element.
    ///
    /// This value cascades to its child elements.
    fn text_decoration_none(mut self) -> Self {
        self.text_style().underline = None;
        self
    }

    /// Sets the color for the underline on this element
    fn text_decoration_color(mut self, color: impl Into<Hsla>) -> Self {
        let style = self.text_style();
        let underline = style.underline.get_or_insert_with(Default::default);
        underline.color = Some(color.into());
        self
    }

    /// Sets the text decoration to be 0px thick.
    /// [Docs](https://tailwindcss.com/docs/text-decoration-thickness)
    fn text_decoration_0(mut self) -> Self {
        let style = self.text_style();
        let underline = style.underline.get_or_insert_with(Default::default);
        underline.thickness = px(0.);
        self
    }

    /// Sets the text decoration to be 1px thick.
    /// [Docs](https://tailwindcss.com/docs/text-decoration-thickness)
    fn text_decoration_1(mut self) -> Self {
        let style = self.text_style();
        let underline = style.underline.get_or_insert_with(Default::default);
        underline.thickness = px(1.);
        self
    }

    /// Sets the text decoration to be 2px thick.
    /// [Docs](https://tailwindcss.com/docs/text-decoration-thickness)
    fn text_decoration_2(mut self) -> Self {
        let style = self.text_style();
        let underline = style.underline.get_or_insert_with(Default::default);
        underline.thickness = px(2.);
        self
    }

    /// Sets the text decoration to be 4px thick.
    /// [Docs](https://tailwindcss.com/docs/text-decoration-thickness)
    fn text_decoration_4(mut self) -> Self {
        let style = self.text_style();
        let underline = style.underline.get_or_insert_with(Default::default);
        underline.thickness = px(4.);
        self
    }

    /// Sets the text decoration to be 8px thick.
    /// [Docs](https://tailwindcss.com/docs/text-decoration-thickness)
    fn text_decoration_8(mut self) -> Self {
        let style = self.text_style();
        let underline = style.underline.get_or_insert_with(Default::default);
        underline.thickness = px(8.);
        self
    }

    /// Sets the font family of this element and its children.
    fn font_family(mut self, family_name: impl Into<SharedString>) -> Self {
        self.text_style().font_family = Some(family_name.into());
        self
    }

    /// Sets the font features of this element and its children.
    fn font_features(mut self, features: FontFeatures) -> Self {
        self.text_style().font_features = Some(features);
        self
    }

    /// Sets the font of this element and its children.
    fn font(mut self, font: Font) -> Self {
        let Font {
            family,
            features,
            fallbacks,
            weight,
            style,
        } = font;

        let text_style = self.text_style();
        text_style.font_family = Some(family);
        text_style.font_features = Some(features);
        text_style.font_weight = Some(weight);
        text_style.font_style = Some(style);
        text_style.font_fallbacks = fallbacks;

        self
    }

    /// Sets the line height of this element and its children.
    fn line_height(mut self, line_height: impl Into<DefiniteLength>) -> Self {
        self.text_style().line_height = Some(line_height.into());
        self
    }

    /// Sets the opacity of this element and its children.
    fn opacity(mut self, opacity: f32) -> Self {
        self.style().opacity = Some(opacity);
        self
    }

    /// Draws a debug border around this element.
    #[cfg(debug_assertions)]
    fn debug(mut self) -> Self {
        self.style().debug = Some(true);
        self
    }

    /// Draws a debug border on all conforming elements below this element.
    #[cfg(debug_assertions)]
    fn debug_below(mut self) -> Self {
        self.style().debug_below = Some(true);
        self
    }
}
