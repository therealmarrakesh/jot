use std::{borrow::Cow, mem};

use crate::ui::{
    AnyElement, App, AvailableSpace, Bounds, BoxShadow, ContentMask, Corners, CursorStyle, Edges,
    FontId, GlyphId, IsZero, LayoutId, MonochromeSprite, Pixels, Point, PolychromeSprite, Quad,
    RenderGlyphParams, Rgba, SUBPIXEL_VARIANTS_X, SUBPIXEL_VARIANTS_Y, ScaledPixels, Shadow, Size,
    Style, TextStyleRefinement, UnderlineStyle,
    anyhow::Result,
    point, px, size,
    smallvec::SmallVec,
    util::{
        ceil_to_device_pixel, floor_to_device_pixel, round_half_toward_zero,
        round_stroke_to_device_pixel, round_to_device_pixel,
    },
};

use super::{
    CursorStyleRequest, DeferredDraw, Hitbox, HitboxBehavior, PaintQuad, Window, WindowControlArea,
    fill,
};

impl Window {
    /// Rounds a logical value to the nearest device pixel.
    #[inline]
    pub(crate) fn pixel_snap(&self, value: Pixels) -> Pixels {
        px(round_to_device_pixel(value.0, self.scale_factor()) / self.scale_factor())
    }

    /// Snaps a point's coordinates to the nearest device pixel.
    #[inline]
    pub(crate) fn pixel_snap_point(&self, position: Point<Pixels>) -> Point<Pixels> {
        position.map(|c| self.pixel_snap(c))
    }

    #[inline]
    fn snap_bounds(&self, bounds: Bounds<Pixels>) -> Bounds<ScaledPixels> {
        let scale_factor = self.scale_factor();
        let left = round_to_device_pixel(bounds.left().0, scale_factor);
        let top = round_to_device_pixel(bounds.top().0, scale_factor);
        let right = round_to_device_pixel(bounds.right().0, scale_factor).max(left);
        let bottom = round_to_device_pixel(bounds.bottom().0, scale_factor).max(top);
        Bounds::from_corners(
            point(ScaledPixels(left), ScaledPixels(top)),
            point(ScaledPixels(right), ScaledPixels(bottom)),
        )
    }

    #[inline]
    fn snap_stroke(&self, value: Pixels) -> ScaledPixels {
        ScaledPixels(round_stroke_to_device_pixel(value.0, self.scale_factor()))
    }

    #[inline]
    fn snap_border_widths(&self, edges: Edges<Pixels>) -> Edges<ScaledPixels> {
        edges.map(|e| self.snap_stroke(*e))
    }

    #[inline]
    fn cover_bounds(&self, bounds: Bounds<Pixels>) -> Bounds<ScaledPixels> {
        let scale_factor = self.scale_factor();
        let left = floor_to_device_pixel(bounds.left().0, scale_factor);
        let top = floor_to_device_pixel(bounds.top().0, scale_factor);
        let right = ceil_to_device_pixel(bounds.right().0, scale_factor).max(left);
        let bottom = ceil_to_device_pixel(bounds.bottom().0, scale_factor).max(top);
        Bounds::from_corners(
            point(ScaledPixels(left), ScaledPixels(top)),
            point(ScaledPixels(right), ScaledPixels(bottom)),
        )
    }

    #[inline]
    fn snapped_content_mask(&self) -> ContentMask<ScaledPixels> {
        ContentMask {
            bounds: self.cover_bounds(self.content_mask().bounds),
        }
    }

    pub(super) fn prepaint_deferred_draws(&mut self, cx: &mut App) {
        assert_eq!(self.element_id_stack.len(), 0);

        let mut round_start = 0;
        let mut depth = 0;
        loop {
            let round_end = self.next_frame.deferred_draws.len();
            if round_start == round_end {
                break;
            }
            assert!(depth < 10, "Exceeded maximum (10) deferred depth");
            depth += 1;

            let mut traversal_order = (round_start..round_end).collect::<SmallVec<[usize; 8]>>();
            traversal_order.sort_by_key(|ix| self.next_frame.deferred_draws[*ix].priority);

            for deferred_draw_ix in traversal_order {
                let (mut element, parent_node, current_view, absolute_offset) = {
                    let deferred_draw = &mut self.next_frame.deferred_draws[deferred_draw_ix];
                    self.element_id_stack
                        .clone_from(&deferred_draw.element_id_stack);
                    self.text_style_stack
                        .clone_from(&deferred_draw.text_style_stack);
                    (
                        deferred_draw.element.take().unwrap(),
                        deferred_draw.parent_node,
                        deferred_draw.current_view,
                        deferred_draw.absolute_offset,
                    )
                };
                self.next_frame.dispatch_tree.set_active_node(parent_node);

                self.with_rendered_view(current_view, |window| {
                    window.with_absolute_element_offset(absolute_offset, |window| {
                        element.prepaint(window, cx);
                    });
                });
                self.next_frame.deferred_draws[deferred_draw_ix].element = Some(element);
            }

            self.element_id_stack.clear();
            self.text_style_stack.clear();
            round_start = round_end;
        }
    }

    pub(super) fn paint_deferred_draws(&mut self, cx: &mut App) {
        assert_eq!(self.element_id_stack.len(), 0);

        if self.next_frame.deferred_draws.is_empty() {
            return;
        }

        let traversal_order = self.deferred_draw_traversal_order();
        let mut deferred_draws = mem::take(&mut self.next_frame.deferred_draws);
        for deferred_draw_ix in traversal_order {
            let deferred_draw = &mut deferred_draws[deferred_draw_ix];
            self.element_id_stack
                .clone_from(&deferred_draw.element_id_stack);
            self.next_frame
                .dispatch_tree
                .set_active_node(deferred_draw.parent_node);

            let content_mask = deferred_draw.content_mask;
            let element = deferred_draw.element.as_mut().unwrap();
            self.with_rendered_view(deferred_draw.current_view, |window| {
                window.with_content_mask(content_mask, |window| {
                    element.paint(window, cx);
                })
            });
        }
        self.next_frame.deferred_draws = deferred_draws;
        self.element_id_stack.clear();
    }

    fn deferred_draw_traversal_order(&mut self) -> SmallVec<[usize; 8]> {
        let deferred_count = self.next_frame.deferred_draws.len();
        let mut sorted_indices = (0..deferred_count).collect::<SmallVec<[_; 8]>>();
        sorted_indices.sort_by_key(|ix| self.next_frame.deferred_draws[*ix].priority);
        sorted_indices
    }

    pub(crate) fn with_text_style<F, R>(&mut self, style: Option<TextStyleRefinement>, f: F) -> R
    where
        F: FnOnce(&mut Self) -> R,
    {
        self.invalidator.debug_assert_paint_or_prepaint();
        if let Some(style) = style {
            self.text_style_stack.push(style);
            let result = f(self);
            self.text_style_stack.pop();
            result
        } else {
            f(self)
        }
    }

    pub(crate) fn set_cursor_style(&mut self, style: CursorStyle, hitbox: &Hitbox) {
        self.invalidator.debug_assert_paint();
        self.next_frame.cursor_styles.push(CursorStyleRequest {
            hitbox_id: hitbox.id,
            style,
        });
    }

    #[inline]
    pub(crate) fn with_content_mask<R>(
        &mut self,
        mask: Option<ContentMask<Pixels>>,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        self.invalidator.debug_assert_paint_or_prepaint();
        if let Some(mask) = mask {
            let mask = mask.intersect(&self.content_mask());
            self.content_mask_stack.push(mask);
            let result = f(self);
            self.content_mask_stack.pop();
            result
        } else {
            f(self)
        }
    }

    pub(crate) fn with_element_offset<R>(
        &mut self,
        offset: Point<Pixels>,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        self.invalidator.debug_assert_prepaint();

        if offset.is_zero() {
            return f(self);
        };

        let abs_offset = self.element_offset() + offset;
        self.with_absolute_element_offset(abs_offset, f)
    }

    pub(crate) fn with_absolute_element_offset<R>(
        &mut self,
        offset: Point<Pixels>,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        self.invalidator.debug_assert_prepaint();
        self.element_offset_stack.push(offset);
        let result = f(self);
        self.element_offset_stack.pop();
        result
    }

    pub(crate) fn with_element_opacity<R>(
        &mut self,
        opacity: Option<f32>,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        self.invalidator.debug_assert_paint_or_prepaint();

        let Some(opacity) = opacity else {
            return f(self);
        };

        let previous_opacity = self.element_opacity;
        self.element_opacity = previous_opacity * opacity;
        let result = f(self);
        self.element_opacity = previous_opacity;
        result
    }

    pub(crate) fn element_offset(&self) -> Point<Pixels> {
        self.invalidator.debug_assert_prepaint();
        self.element_offset_stack
            .last()
            .copied()
            .unwrap_or_default()
    }

    #[inline]
    pub(crate) fn element_opacity(&self) -> f32 {
        self.invalidator.debug_assert_paint_or_prepaint();
        self.element_opacity
    }

    pub(crate) fn content_mask(&self) -> ContentMask<Pixels> {
        self.invalidator.debug_assert_paint_or_prepaint();
        self.content_mask_stack
            .last()
            .cloned()
            .unwrap_or_else(|| ContentMask {
                bounds: Bounds {
                    origin: Point::default(),
                    size: self.viewport_size,
                },
            })
    }

    pub(crate) fn defer_draw(
        &mut self,
        element: AnyElement,
        absolute_offset: Point<Pixels>,
        priority: usize,
        content_mask: Option<ContentMask<Pixels>>,
    ) {
        self.invalidator.debug_assert_prepaint();
        let parent_node = self.next_frame.dispatch_tree.active_node_id().unwrap();
        self.next_frame.deferred_draws.push(DeferredDraw {
            current_view: self.current_view(),
            parent_node,
            element_id_stack: self.element_id_stack.clone(),
            text_style_stack: self.text_style_stack.clone(),
            content_mask,
            priority,
            element: Some(element),
            absolute_offset,
        });
    }

    pub(crate) fn paint_layer<R>(&mut self, bounds: Bounds<Pixels>, f: impl FnOnce(&mut Self) -> R) -> R {
        self.invalidator.debug_assert_paint();

        let content_mask = self.content_mask();
        let clipped_bounds = bounds.intersect(&content_mask.bounds);
        if !clipped_bounds.is_empty() {
            self.next_frame
                .scene
                .push_layer(self.cover_bounds(clipped_bounds));
        }

        let result = f(self);

        if !clipped_bounds.is_empty() {
            self.next_frame.scene.pop_layer();
        }

        result
    }

    pub(crate) fn paint_drop_shadows(
        &mut self,
        bounds: Bounds<Pixels>,
        corner_radii: Corners<Pixels>,
        shadows: &[BoxShadow],
    ) {
        self.invalidator.debug_assert_paint();

        let scale_factor = self.scale_factor();
        let content_mask = self.snapped_content_mask();
        let opacity = self.element_opacity();
        let element_bounds = self.cover_bounds(bounds);
        let element_corner_radii = corner_radii.scale(scale_factor);
        for shadow in shadows {
            if shadow.inset {
                continue;
            }
            let shadow_bounds = (bounds + shadow.offset).dilate(shadow.spread_radius);
            self.next_frame.scene.insert_primitive(Shadow {
                order: 0,
                blur_radius: shadow.blur_radius.scale(scale_factor),
                bounds: self.cover_bounds(shadow_bounds),
                content_mask,
                corner_radii: corner_radii.scale(scale_factor),
                color: shadow.color.opacity(opacity),
                element_bounds,
                element_corner_radii,
                inset: 0,
                pad: 0,
            });
        }
    }

    pub(crate) fn paint_inset_shadows(
        &mut self,
        bounds: Bounds<Pixels>,
        corner_radii: Corners<Pixels>,
        shadows: &[BoxShadow],
    ) {
        self.invalidator.debug_assert_paint();

        let scale_factor = self.scale_factor();
        let content_mask = self.snapped_content_mask();
        let opacity = self.element_opacity();
        let element_bounds = self.cover_bounds(bounds);
        let element_corner_radii = corner_radii.scale(scale_factor);
        for shadow in shadows {
            if !shadow.inset {
                continue;
            }
            let hole = (bounds + shadow.offset).dilate(-shadow.spread_radius);
            let zero = Pixels::ZERO;
            let hole_corner_radii = Corners {
                top_left: (corner_radii.top_left - shadow.spread_radius).max(zero),
                top_right: (corner_radii.top_right - shadow.spread_radius).max(zero),
                bottom_right: (corner_radii.bottom_right - shadow.spread_radius).max(zero),
                bottom_left: (corner_radii.bottom_left - shadow.spread_radius).max(zero),
            };
            self.next_frame.scene.insert_primitive(Shadow {
                order: 0,
                blur_radius: shadow.blur_radius.scale(scale_factor),
                bounds: self.cover_bounds(hole),
                content_mask,
                corner_radii: hole_corner_radii.scale(scale_factor),
                color: shadow.color.opacity(opacity),
                element_bounds,
                element_corner_radii,
                inset: 1,
                pad: 0,
            });
        }
    }

    fn largest_border_interior(quad: &Quad) -> Bounds<ScaledPixels> {
        let radii = &quad.corner_radii;
        let widths = &quad.border_widths;
        let edge_radii = Edges {
            top: radii.top_left.max(radii.top_right),
            right: radii.top_right.max(radii.bottom_right),
            bottom: radii.bottom_left.max(radii.bottom_right),
            left: radii.top_left.max(radii.bottom_left),
        };

        let antialias_inset = point(ScaledPixels(1.0), ScaledPixels(1.0));
        let inset_bounds = |top_left_inset, bottom_right_inset| {
            Bounds::from_corners(
                quad.bounds.origin + top_left_inset + antialias_inset,
                quad.bounds.bottom_right() - bottom_right_inset - antialias_inset,
            )
        };

        let horizontal_band = inset_bounds(
            point(widths.left, widths.top.max(edge_radii.top)),
            point(widths.right, widths.bottom.max(edge_radii.bottom)),
        );
        let vertical_band = inset_bounds(
            point(widths.left.max(edge_radii.left), widths.top),
            point(widths.right.max(edge_radii.right), widths.bottom),
        );

        let area = |bounds: &Bounds<ScaledPixels>| {
            bounds.size.width.0.max(0.) * bounds.size.height.0.max(0.)
        };
        if area(&horizontal_band) >= area(&vertical_band) {
            horizontal_band
        } else {
            vertical_band
        }
    }

    pub(crate) fn paint_quad(&mut self, quad: PaintQuad) {
        self.invalidator.debug_assert_paint();

        let opacity = self.element_opacity();
        let snapped_bounds = self.snap_bounds(quad.bounds);
        let snapped_border_widths = self.snap_border_widths(quad.border_widths);
        let quad = Quad {
            order: 0,
            bounds: snapped_bounds,
            content_mask: self.snapped_content_mask(),
            background: quad.background.opacity(opacity),
            border_color: quad.border_color.opacity(opacity),
            corner_radii: quad.corner_radii.scale(self.scale_factor()),
            border_widths: snapped_border_widths,
            pad: 0,
        };

        if !quad.background.is_transparent() {
            self.next_frame.scene.insert_primitive(quad);
            return;
        }

        // Splitting a border-only quad around its empty interior avoids shading
        // every transparent pixel inside large outlines.
        let outer_bounds = quad.bounds;
        let inner_bounds = Self::largest_border_interior(&quad);

        if inner_bounds.is_empty() {
            self.next_frame.scene.insert_primitive(quad);
            return;
        }

        let strips = [
            Bounds::from_corners(
                outer_bounds.origin,
                point(outer_bounds.right(), inner_bounds.top()),
            ),
            Bounds::from_corners(
                point(outer_bounds.left(), inner_bounds.bottom()),
                outer_bounds.bottom_right(),
            ),
            Bounds::from_corners(
                point(outer_bounds.left(), inner_bounds.top()),
                inner_bounds.bottom_left(),
            ),
            Bounds::from_corners(
                inner_bounds.top_right(),
                point(outer_bounds.right(), inner_bounds.bottom()),
            ),
        ];

        for strip in strips {
            let content_mask_bounds = quad.content_mask.bounds.intersect(&strip);
            if !content_mask_bounds.is_empty() {
                self.next_frame.scene.insert_primitive(Quad {
                    content_mask: ContentMask {
                        bounds: content_mask_bounds,
                    },
                    ..quad
                });
            }
        }
    }

    pub(crate) fn paint_underline(
        &mut self,
        origin: Point<Pixels>,
        width: Pixels,
        style: &UnderlineStyle,
    ) {
        self.paint_quad(fill(
            Bounds {
                origin,
                size: size(width, style.thickness),
            },
            style.color.unwrap_or_default(),
        ));
    }

    pub(crate) fn paint_glyph(
        &mut self,
        origin: Point<Pixels>,
        font_id: FontId,
        glyph_id: GlyphId,
        font_size: Pixels,
        color: Rgba,
    ) -> Result<()> {
        self.invalidator.debug_assert_paint();

        let element_opacity = self.element_opacity();
        let scale_factor = self.scale_factor();
        let glyph_origin = origin.scale(scale_factor);

        let quantized_origin = Point::new(
            round_half_toward_zero(glyph_origin.x.0 * SUBPIXEL_VARIANTS_X as f32)
                / SUBPIXEL_VARIANTS_X as f32,
            round_half_toward_zero(glyph_origin.y.0 * SUBPIXEL_VARIANTS_Y as f32)
                / SUBPIXEL_VARIANTS_Y as f32,
        );
        let subpixel_variant = Point::new(
            (quantized_origin.x.fract() * SUBPIXEL_VARIANTS_X as f32) as u8,
            (quantized_origin.y.fract() * SUBPIXEL_VARIANTS_Y as f32) as u8,
        );
        let integer_origin = quantized_origin.map(|c| ScaledPixels(c.trunc()));
        let params = RenderGlyphParams {
            font_id,
            glyph_id,
            font_size,
            subpixel_variant,
            scale_factor,
            is_emoji: false,
            dilation: 0,
        };

        let raster_bounds = self.text_system().raster_bounds(&params)?;
        if !raster_bounds.is_zero() {
            let tile = self
                .sprite_atlas
                .get_or_insert_with(params.clone(), || {
                    let (size, bytes) = self.text_system().rasterize_glyph(&params)?;
                    Ok(Some((size, Cow::Owned(bytes))))
                })?
                .expect("Callback above only errors or returns Some");
            let bounds = Bounds {
                origin: integer_origin + raster_bounds.origin.map(Into::into),
                size: tile.bounds.size.map(Into::into),
            };
            let content_mask = self.snapped_content_mask();

            self.next_frame.scene.insert_primitive(MonochromeSprite {
                order: 0,
                pad: 0,
                bounds,
                content_mask,
                color: color.opacity(element_opacity),
                tile,
            });
        }
        Ok(())
    }

    pub(crate) fn paint_emoji(
        &mut self,
        origin: Point<Pixels>,
        font_id: FontId,
        glyph_id: GlyphId,
        font_size: Pixels,
    ) -> Result<()> {
        self.invalidator.debug_assert_paint();

        let scale_factor = self.scale_factor();
        let glyph_origin = origin.scale(scale_factor);
        let integer_origin = glyph_origin.map(|c| ScaledPixels(round_half_toward_zero(c.0)));
        let params = RenderGlyphParams {
            font_id,
            glyph_id,
            font_size,
            subpixel_variant: Default::default(),
            scale_factor,
            is_emoji: true,
            dilation: 0,
        };

        let raster_bounds = self.text_system().raster_bounds(&params)?;
        if !raster_bounds.is_zero() {
            let tile = self
                .sprite_atlas
                .get_or_insert_with(params.clone(), || {
                    let (size, bytes) = self.text_system().rasterize_glyph(&params)?;
                    Ok(Some((size, Cow::Owned(bytes))))
                })?
                .expect("Callback above only errors or returns Some");

            let bounds = Bounds {
                origin: integer_origin + raster_bounds.origin.map(Into::into),
                size: tile.bounds.size.map(Into::into),
            };
            let content_mask = self.snapped_content_mask();
            let opacity = self.element_opacity();

            self.next_frame.scene.insert_primitive(PolychromeSprite {
                order: 0,
                pad: 0,
                grayscale: false.into(),
                bounds,
                corner_radii: Default::default(),
                content_mask,
                tile,
                opacity,
            });
        }
        Ok(())
    }

    #[must_use]
    pub(crate) fn request_layout(
        &mut self,
        style: Style,
        children: impl IntoIterator<Item = LayoutId>,
        cx: &mut App,
    ) -> LayoutId {
        self.invalidator.debug_assert_prepaint();

        cx.layout_id_buffer.clear();
        cx.layout_id_buffer.extend(children);
        let scale_factor = self.scale_factor();

        self.layout_engine
            .as_mut()
            .unwrap()
            .request_layout(style, scale_factor, &cx.layout_id_buffer)
    }

    pub(crate) fn request_measured_layout<F>(&mut self, style: Style, measure: F) -> LayoutId
    where
        F: Fn(Size<Option<Pixels>>, Size<AvailableSpace>, &mut Window, &mut App) -> Size<Pixels>
            + 'static,
    {
        self.invalidator.debug_assert_prepaint();

        let scale_factor = self.scale_factor();
        self.layout_engine
            .as_mut()
            .unwrap()
            .request_measured_layout(style, scale_factor, measure)
    }

    pub(crate) fn compute_layout(
        &mut self,
        layout_id: LayoutId,
        available_space: Size<AvailableSpace>,
        cx: &mut App,
    ) {
        self.invalidator.debug_assert_prepaint();

        let mut layout_engine = self.layout_engine.take().unwrap();
        layout_engine.compute_layout(layout_id, available_space, self, cx);
        self.layout_engine = Some(layout_engine);
    }

    pub(crate) fn layout_bounds(&mut self, layout_id: LayoutId) -> Bounds<Pixels> {
        self.invalidator.debug_assert_prepaint();

        let scale_factor = self.scale_factor();
        let mut bounds = self
            .layout_engine
            .as_mut()
            .unwrap()
            .layout_bounds(layout_id, scale_factor);
        let snapped_offset = self.pixel_snap_point(self.element_offset());
        bounds.origin += snapped_offset;
        bounds
    }

    pub(crate) fn insert_hitbox(&mut self, bounds: Bounds<Pixels>, behavior: HitboxBehavior) -> Hitbox {
        self.invalidator.debug_assert_prepaint();

        let content_mask = self.content_mask();
        let id = self.next_hitbox_id;
        self.next_hitbox_id = self.next_hitbox_id.next();
        let hitbox = Hitbox {
            id,
            bounds,
            content_mask,
            behavior,
        };
        self.next_frame.hitboxes.push(hitbox.clone());
        hitbox
    }

    pub(crate) fn insert_window_control_hitbox(&mut self, area: WindowControlArea, hitbox: Hitbox) {
        self.invalidator.debug_assert_paint();
        self.next_frame.window_control_hitboxes.push((area, hitbox));
    }
}
