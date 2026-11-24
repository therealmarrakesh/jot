use std::collections::HashMap;
use std::hash::Hash;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crate::geom::{
    Axis, Corners, Edges, Point, Rect, Rgba, ScreenPoint, ScreenRect, Shadows, Size, black, point,
    size, transparent_black,
};
use crate::gfx::{
    Canvas, FontWeight, Gpu, Label, LabelLayout, PaintQuad, Scope, TextStyle, TextSystem,
    WindowSurface,
};
use crate::platform::{CursorStyle, MouseButton, NativeWindow, WindowControlArea, WindowId};
use crate::types::SharedStr;

pub(crate) const BRUSH_FADE_DURATION: Duration = Duration::from_millis(83);

#[derive(Copy, Clone, Debug)]
pub(crate) struct Brushes {
    pub(crate) rest: Rgba,
    pub(crate) hover: Rgba,
    pub(crate) pressed: Rgba,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct PointerState {
    pub(crate) pos: Point,
    pub(crate) pressed: bool,
}

#[derive(Clone, Default)]
struct TextOverrides {
    family: Option<SharedStr>,
    weight: Option<FontWeight>,
    size: Option<f32>,
    line_height: Option<f32>,
    color: Option<Rgba>,
    nowrap: Option<bool>,
    line_clamp: Option<usize>,
}

#[derive(Clone)]
struct ResolvedTextStyle {
    style: TextStyle,
    line_height: f32,
    color: Rgba,
    nowrap: bool,
    line_clamp: Option<usize>,
}

impl Default for ResolvedTextStyle {
    fn default() -> Self {
        Self {
            style: TextStyle::default(),
            line_height: 20.0,
            color: black(),
            nowrap: false,
            line_clamp: None,
        }
    }
}

impl ResolvedTextStyle {
    fn merge(&self, o: &TextOverrides) -> Self {
        let mut out = self.clone();
        if let Some(fam) = &o.family {
            out.style.font.family = fam.clone();
        }
        if let Some(w) = o.weight {
            out.style.font.weight = w;
        }
        if let Some(s) = o.size {
            out.style.set_size(s);
        }
        if let Some(lh) = o.line_height {
            out.line_height = lh;
        }
        if let Some(c) = o.color {
            out.color = c;
        }
        if let Some(nw) = o.nowrap {
            out.nowrap = nw;
        }
        if let Some(lc) = o.line_clamp {
            out.line_clamp = Some(lc);
        }
        out
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub(crate) enum Transform {
    #[default]
    None,
    Scale(f32, f32),
    ScaleFromPx {
        collapsed: f32,
        min_start: f32,
        t: f32,
    },
    RevealY {
        shown_ratio: f32,
        slide: bool,
    },
}

impl Transform {
    fn resolved_scale(self, sz: Size) -> Option<(f32, f32)> {
        match self {
            Self::Scale(sx, sy) => Some((sx, sy)),
            Self::ScaleFromPx {
                collapsed,
                min_start,
                t,
            } => {
                let axis = |dim: f32| {
                    let start = (collapsed / dim.max(collapsed)).min(min_start);
                    start + (1.0 - start) * t
                };
                Some((axis(sz.width), axis(sz.height)))
            }
            Self::None | Self::RevealY { .. } => None,
        }
    }
}

#[derive(Copy, Clone)]
struct Decor {
    bg: Rgba,
    border_color: Rgba,
    corners: Corners,
    shadows: Shadows,
    opacity: f32,
    transform: Transform,
    inert: bool,
}

impl Default for Decor {
    fn default() -> Self {
        Self {
            bg: transparent_black(),
            border_color: transparent_black(),
            corners: Corners::ZERO,
            shadows: Shadows::NONE,
            opacity: 1.0,
            transform: Transform::None,
            inert: false,
        }
    }
}

struct Interact<H> {
    hit: Option<H>,
    cursor: Option<CursorStyle>,
    window_area: Option<WindowControlArea>,
    tooltip: Option<SharedStr>,
}

impl<H> Default for Interact<H> {
    fn default() -> Self {
        Self {
            hit: None,
            cursor: None,
            window_area: None,
            tooltip: None,
        }
    }
}

enum NodePayload<H, C> {
    Box(Vec<Node<H, C>>),
    Text(SharedStr),
    Custom(C),
}

#[derive(Copy, Clone)]
pub(crate) enum Anchor<H> {
    Point(Point),
    Target {
        target: H,
        place: fn(target: Rect, popup: Size, viewport: Size) -> Point,
    },
}

impl<H: Copy> Anchor<H> {
    fn resolve(
        self,
        viewport: Size,
        popup: Size,
        mut lookup: impl FnMut(H) -> Option<Rect>,
    ) -> Point {
        let raw = match self {
            Self::Point(p) => p,
            Self::Target { target, place } => {
                let Some(rect) = lookup(target) else {
                    return Point::ZERO;
                };
                place(rect, popup, viewport)
            }
        };
        point(
            raw.x.min((viewport.width - popup.width).max(0.0)).max(0.0),
            raw.y.min((viewport.height - popup.height).max(0.0)).max(0.0),
        )
    }
}

pub(crate) struct Node<H, C> {
    flex: FlexStyle,
    decor: Decor,
    interact: Interact<H>,
    anchor: Option<Anchor<H>>,
    text: TextOverrides,
    payload: NodePayload<H, C>,
}

macro_rules! setters {
    ($($(#[$meta:meta])* $name:ident ($($arg:ident : $ty:ty),*) => |$s:ident| $body:block)*) => {
        $(
            $(#[$meta])*
            pub(crate) fn $name(mut self $(, $arg: $ty)*) -> Self {
                let $s = &mut self;
                $body
                self
            }
        )*
    };
}

impl<H, C> Node<H, C> {
    fn new_box(row: bool) -> Self {
        Self {
            flex: FlexStyle {
                row,
                ..Default::default()
            },
            decor: Decor::default(),
            interact: Interact::default(),
            anchor: None,
            text: TextOverrides::default(),
            payload: NodePayload::Box(Vec::new()),
        }
    }

    setters! {
        absolute() => |s| { s.flex.absolute = true; }
        w(v: f32) => |s| { s.flex.size[0] = Dim::Px(v); }
        h(v: f32) => |s| { s.flex.size[1] = Dim::Px(v); }
        w_full() => |s| { s.flex.size[0] = Dim::Full; }
        h_full() => |s| { s.flex.size[1] = Dim::Full; }
        min_w(v: f32) => |s| { s.flex.min[0] = Dim::Px(v); }
        min_h(v: f32) => |s| { s.flex.min[1] = Dim::Px(v); }
        min_h_full() => |s| { s.flex.min[1] = Dim::Full; }
        max_w(v: f32) => |s| { s.flex.max[0] = Dim::Px(v); }
        max_w_full() => |s| { s.flex.max[0] = Dim::Full; }
        flex_1() => |s| { s.flex.flex = Flex::Fill; }
        flex_none() => |s| { s.flex.flex = Flex::Fixed; }
        gap(v: f32) => |s| { s.flex.gap = v; }
        items_center() => |s| { s.flex.items_center = true; }
        justify_center() => |s| { s.flex.justify_center = true; }
        overflow_hidden() => |s| { s.flex.clip = [true, true]; }
        overflow_x_hidden() => |s| { s.flex.clip[0] = true; }
        p(v: f32) => |s| { s.flex.padding = Edges::all(v); }
        px(v: f32) => |s| { s.flex.padding.left = v; s.flex.padding.right = v; }
        py(v: f32) => |s| { s.flex.padding.top = v; s.flex.padding.bottom = v; }
        pt(v: f32) => |s| { s.flex.padding.top = v; }
        pr(v: f32) => |s| { s.flex.padding.right = v; }
        pb(v: f32) => |s| { s.flex.padding.bottom = v; }
        pl(v: f32) => |s| { s.flex.padding.left = v; }
        padding(e: Edges<f32>) => |s| { s.flex.padding = e; }
        mt(v: f32) => |s| { s.flex.margin.top = v; }
        mb(v: f32) => |s| { s.flex.margin.bottom = v; }
        ml(v: f32) => |s| { s.flex.margin.left = v; }
        top(v: f32) => |s| { s.flex.inset.top = Some(v); }
        right(v: f32) => |s| { s.flex.inset.right = Some(v); }
        bottom(v: f32) => |s| { s.flex.inset.bottom = Some(v); }
        left(v: f32) => |s| { s.flex.inset.left = Some(v); }
        bg(color: Rgba) => |s| { s.decor.bg = color; }
        border(width: f32, color: Rgba) => |s| {
            s.flex.border = Edges::all(width);
            s.decor.border_color = color;
        }
        border_widths(widths: Edges<f32>, color: Rgba) => |s| {
            s.flex.border = widths;
            s.decor.border_color = color;
        }
        border_t(w: f32) => |s| { s.flex.border.top = w; }
        border_r(w: f32) => |s| { s.flex.border.right = w; }
        border_b(w: f32) => |s| { s.flex.border.bottom = w; }
        border_l(w: f32) => |s| { s.flex.border.left = w; }
        border_color(color: Rgba) => |s| { s.decor.border_color = color; }
        rounded(r: f32) => |s| { s.decor.corners = Corners::all(r); }
        rounded_full() => |s| { s.decor.corners = Corners::all(9999.0); }
        rounded_t(r: f32) => |s| { s.decor.corners = Corners::top(r); }
        rounded_b(r: f32) => |s| { s.decor.corners = Corners::bottom(r); }
        shadows(shadows: Shadows) => |s| { s.decor.shadows = shadows; }
        opacity(opacity: f32) => |s| { s.decor.opacity = opacity; }
        scale(sx: f32, sy: f32) => |s| { s.decor.transform = Transform::Scale(sx, sy); }
        transform(t: Transform) => |s| { s.decor.transform = t; }
        inert() => |s| { s.decor.inert = true; }
        anchored(anchor: Anchor<H>) => |s| { s.anchor = Some(anchor); }
        font_semibold() => |s| { s.text.weight = Some(FontWeight::Semibold); }
        text_size(size: f32) => |s| { s.text.size = Some(size); }
        line_height(lh: f32) => |s| { s.text.line_height = Some(lh); }
        text_color(color: Rgba) => |s| { s.text.color = Some(color); }
        nowrap() => |s| { s.text.nowrap = Some(true); }
        line_clamp(lines: usize) => |s| { s.text.line_clamp = Some(lines); }
        hit(id: H) => |s| { s.interact.hit = Some(id); }
        cursor(cursor: CursorStyle) => |s| { s.interact.cursor = Some(cursor); }
        window_area(area: WindowControlArea) => |s| { s.interact.window_area = Some(area); }
    }

    pub(crate) fn size(self, v: f32) -> Self {
        self.w(v).h(v)
    }
    pub(crate) fn size_full(self) -> Self {
        self.w_full().h_full()
    }
    pub(crate) fn min_w_0(self) -> Self {
        self.min_w(0.0)
    }
    pub(crate) fn inset_0(self) -> Self {
        self.top(0.0).right(0.0).bottom(0.0).left(0.0)
    }
    pub(crate) fn font_family(mut self, family: impl Into<SharedStr>) -> Self {
        self.text.family = Some(family.into());
        self
    }
    pub(crate) fn tooltip(mut self, tip: impl Into<SharedStr>) -> Self {
        self.interact.tooltip = Some(tip.into());
        self
    }
    pub(crate) fn child(mut self, child: Node<H, C>) -> Self {
        if let NodePayload::Box(children) = &mut self.payload {
            children.push(child);
        }
        self
    }
    pub(crate) fn children(mut self, iter: impl IntoIterator<Item = Node<H, C>>) -> Self {
        if let NodePayload::Box(children) = &mut self.payload {
            children.extend(iter);
        }
        self
    }
    pub(crate) fn text_child(self, s: impl Into<SharedStr>) -> Self {
        self.child(text(s))
    }
    pub(crate) fn when(self, cond: bool, f: impl FnOnce(Self) -> Self) -> Self {
        if cond { f(self) } else { self }
    }
    pub(crate) fn when_some<T>(self, opt: Option<T>, f: impl FnOnce(Self, T) -> Self) -> Self {
        if let Some(v) = opt { f(self, v) } else { self }
    }
}

pub(crate) fn row<H, C>() -> Node<H, C> {
    Node::new_box(true)
}

pub(crate) fn box_<H, C>() -> Node<H, C> {
    Node::new_box(false)
}

pub(crate) fn text<H, C>(content: impl Into<SharedStr>) -> Node<H, C> {
    let mut node = Node::new_box(false);
    node.payload = NodePayload::Text(content.into());
    node
}

pub(crate) fn custom<H, C>(token: C) -> Node<H, C> {
    let mut node = Node::new_box(false);
    node.payload = NodePayload::Custom(token);
    node
}

enum FlatKind<C> {
    Box {
        children_start: usize,
        children_len: usize,
    },
    Text {
        content: SharedStr,
        style: ResolvedTextStyle,
        label: Option<Label>,
    },
    Custom(C),
}

struct FlatNode<H, C> {
    parent: Option<usize>,
    rect: Rect,
    border: Edges<f32>,
    clip: bool,
    decor: Decor,
    interact: Interact<H>,
    anchor: Option<Anchor<H>>,
    kind: FlatKind<C>,
}

#[derive(Copy, Clone, Debug)]
struct HitEntry {
    node_idx: usize,
    clipped_rect: Rect,
}

pub(crate) struct Frame<H, C> {
    nodes: Vec<FlatNode<H, C>>,
    child_order: Vec<usize>,
    root: Option<usize>,
    hits: Vec<HitEntry>,
    control_areas: Vec<(Rect, WindowControlArea)>,
}

impl<H, C> Default for Frame<H, C> {
    fn default() -> Self {
        Self {
            nodes: Vec::new(),
            child_order: Vec::new(),
            root: None,
            hits: Vec::new(),
            control_areas: Vec::new(),
        }
    }
}

impl<H: Copy + Eq, C: Copy> Frame<H, C> {
    pub(crate) fn custom_rects(&self) -> impl Iterator<Item = (C, Rect)> + '_ {
        self.nodes.iter().filter_map(|n| match n.kind {
            FlatKind::Custom(token) => Some((token, n.rect)),
            _ => None,
        })
    }

    pub(crate) fn layout(
        root_node: Node<H, C>,
        available: AvailSize,
        scale: f32,
        text_system: &TextSystem,
    ) -> Self {
        let mut engine = LayoutEngine::new();
        let mut frame = Frame::default();
        let default_text = ResolvedTextStyle::default();

        let root_idx = frame.flatten(root_node, None, &default_text, scale, &mut engine);
        frame.root = Some(root_idx);
        let root_lid = LayoutId(root_idx as u32);

        engine.compute(root_lid, available, scale, &mut |lid, kw, _kh, avail| {
            let FlatKind::Text {
                content,
                style,
                label,
            } = &mut frame.nodes[lid.0 as usize].kind
            else {
                return Size::ZERO;
            };
            let wrap_width = if style.nowrap {
                None
            } else {
                kw.or(match avail.width {
                    Space::Definite(w) => Some(w),
                    _ => None,
                })
            };
            let shaped = text_system.shape_label(
                content.clone(),
                &style.style,
                LabelLayout {
                    line_height: style.line_height,
                    wrap_width,
                    line_clamp: style.line_clamp,
                },
            );
            let sz = shaped.size();
            *label = Some(shaped);
            sz
        });

        let root_rect = engine.bounds(root_lid, scale);
        let viewport = match (available.width, available.height) {
            (Space::Definite(w), Space::Definite(h)) => size(w, h),
            _ => root_rect.size,
        };
        for idx in 0..frame.nodes.len() {
            let Some(anchor) = frame.nodes[idx].anchor else {
                continue;
            };
            let lid = LayoutId(idx as u32);
            let popup_size = engine.bounds(lid, scale).size;
            let origin = anchor.resolve(viewport, popup_size, |target| {
                frame
                    .nodes
                    .iter()
                    .enumerate()
                    .rev()
                    .find(|(_, n)| n.interact.hit == Some(target))
                    .map(|(i, _)| engine.bounds(LayoutId(i as u32), scale))
            });
            engine.set_origin(lid, origin, scale);
        }

        for (idx, node) in frame.nodes.iter_mut().enumerate() {
            node.rect = engine.bounds(LayoutId(idx as u32), scale);
        }

        let initial_clip = Rect::new(Point::ZERO, viewport);
        frame.collect_hits(root_idx, initial_clip);
        frame
    }

    pub(crate) fn measure(
        root_node: Node<H, C>,
        available: AvailSize,
        scale: f32,
        text_system: &TextSystem,
    ) -> Size {
        let frame = Self::layout(root_node, available, scale, text_system);
        frame
            .root
            .map_or(Size::ZERO, |idx| frame.nodes[idx].rect.size)
    }

    fn flatten(
        &mut self,
        node: Node<H, C>,
        parent: Option<usize>,
        inherited_text: &ResolvedTextStyle,
        scale: f32,
        engine: &mut LayoutEngine,
    ) -> usize {
        let resolved_text = inherited_text.merge(&node.text);
        let clip = node.flex.clip[0] || node.flex.clip[1];
        let border = snap_border_edges(node.flex.border, scale);
        let Node {
            mut flex,
            decor,
            interact,
            anchor,
            payload,
            ..
        } = node;
        if anchor.is_some() {
            flex.absolute = true;
        }
        let is_text = matches!(payload, NodePayload::Text(_));
        let flat_idx = self.nodes.len();
        let lid = engine.reserve_node(flex, scale, is_text);
        debug_assert_eq!(lid.0 as usize, flat_idx);

        self.nodes.push(FlatNode {
            parent,
            rect: Rect::ZERO,
            border,
            clip,
            decor,
            interact,
            anchor,
            kind: FlatKind::Box {
                children_start: 0,
                children_len: 0,
            },
        });

        let kind = match payload {
            NodePayload::Box(children) => {
                let mut child_flat_ids = Vec::with_capacity(children.len());
                let mut child_lids = Vec::with_capacity(children.len());
                for child in children {
                    let c_idx = self.flatten(child, Some(flat_idx), &resolved_text, scale, engine);
                    child_flat_ids.push(c_idx);
                    child_lids.push(LayoutId(c_idx as u32));
                }
                let start = self.child_order.len();
                self.child_order.extend_from_slice(&child_flat_ids);
                engine.set_children(lid, &child_lids);
                FlatKind::Box {
                    children_start: start,
                    children_len: child_flat_ids.len(),
                }
            }
            NodePayload::Text(content) => FlatKind::Text {
                content,
                style: resolved_text,
                label: None,
            },
            NodePayload::Custom(token) => FlatKind::Custom(token),
        };
        self.nodes[flat_idx].kind = kind;
        flat_idx
    }

    fn collect_hits(&mut self, idx: usize, clip: Rect) {
        let node = &self.nodes[idx];
        let scaled = node
            .decor
            .transform
            .resolved_scale(node.rect.size)
            .is_some_and(|(sx, sy)| (sx - 1.0).abs() > 1e-4 || (sy - 1.0).abs() > 1e-4);
        if node.decor.inert || scaled {
            return;
        }
        let clipped = node.rect.intersect(clip);
        let child_clip = if node.clip { clipped } else { clip };

        if !clipped.is_empty() {
            if let Some(area) = self.nodes[idx].interact.window_area {
                self.control_areas.push((clipped, area));
            } else if self.nodes[idx].interact.hit.is_some() {
                self.control_areas.push((clipped, WindowControlArea::Client));
            }
            if self.nodes[idx].interact.hit.is_some() {
                self.hits.push(HitEntry {
                    node_idx: idx,
                    clipped_rect: clipped,
                });
            }
        }

        if let FlatKind::Box {
            children_start,
            children_len,
        } = self.nodes[idx].kind
        {
            for offset in 0..children_len {
                self.collect_hits(self.child_order[children_start + offset], child_clip);
            }
        }
    }

    pub(crate) fn control_areas(&self) -> Vec<(Rect, WindowControlArea)> {
        self.control_areas.clone()
    }

    fn top_hit(&self, pos: Point) -> Option<&HitEntry> {
        self.hits
            .iter()
            .rev()
            .find(|entry| entry.clipped_rect.contains(pos))
    }

    pub(crate) fn hit_chain(&self, pos: Point) -> Vec<H> {
        let Some(top) = self.top_hit(pos) else {
            return Vec::new();
        };
        let mut chain = Vec::with_capacity(4);
        let mut cur = Some(top.node_idx);
        while let Some(idx) = cur {
            if let Some(hit) = self.nodes[idx].interact.hit {
                chain.push(hit);
            }
            cur = self.nodes[idx].parent;
        }
        chain
    }

    pub(crate) fn rect_of(&self, id: H) -> Option<Rect> {
        self.nodes
            .iter()
            .rev()
            .find(|node| node.interact.hit == Some(id))
            .map(|node| node.rect)
    }

    pub(crate) fn cursor_at(&self, pos: Point) -> CursorStyle {
        let Some(top) = self.top_hit(pos) else {
            return CursorStyle::Arrow;
        };
        let mut cur = Some(top.node_idx);
        while let Some(idx) = cur {
            if let Some(c) = self.nodes[idx].interact.cursor {
                return c;
            }
            cur = self.nodes[idx].parent;
        }
        CursorStyle::Arrow
    }

    pub(crate) fn tooltip_at(&self, pos: Point) -> Option<(H, SharedStr, Rect)> {
        let top = self.top_hit(pos)?;
        let mut cur = Some(top.node_idx);
        while let Some(idx) = cur {
            if let Some(tip) = &self.nodes[idx].interact.tooltip
                && let Some(hit) = self.nodes[idx].interact.hit
            {
                return Some((hit, tip.clone(), self.nodes[idx].rect));
            }
            cur = self.nodes[idx].parent;
        }
        None
    }

    pub(crate) fn paint(
        &self,
        canvas: &mut Canvas,
        draw_custom: &mut impl FnMut(C, Rect, &mut Canvas),
    ) {
        if let Some(root) = self.root {
            self.paint_node(root, canvas, draw_custom);
        }
    }

    fn paint_node(
        &self,
        idx: usize,
        canvas: &mut Canvas,
        draw_custom: &mut impl FnMut(C, Rect, &mut Canvas),
    ) {
        let node = &self.nodes[idx];
        let d = node.decor;
        let resolved_scale = d.transform.resolved_scale(node.rect.size);
        if d.opacity <= 0.0 || resolved_scale.is_some_and(|(sx, sy)| sx <= 1e-3 || sy <= 1e-3) {
            return;
        }
        let opacity = (d.opacity < 1.0 - 1e-4).then_some(d.opacity);
        let scale = resolved_scale.and_then(|(sx, sy)| {
            ((sx - 1.0).abs() > 1e-4 || (sy - 1.0).abs() > 1e-4)
                .then_some((node.rect.center(), sx, sy))
        });
        let (paint_rect, reveal_clip, reveal_translate) = match d.transform {
            Transform::RevealY { shown_ratio, slide } if shown_ratio < 1.0 - 1e-4 => {
                let shown_h = (node.rect.size.height * shown_ratio).max(0.0);
                let r = Rect::new(node.rect.origin, size(node.rect.size.width, shown_h));
                let delta = slide.then_some(point(0.0, -(node.rect.size.height - shown_h)));
                (r, Some(r.inset(node.border)), delta)
            }
            _ => (node.rect, None, None),
        };

        canvas.scoped(
            Scope {
                opacity,
                scale,
                ..Default::default()
            },
            |canvas| {
                if !d.shadows.is_empty() {
                    canvas.paint_shadows(paint_rect, d.corners, d.shadows);
                }
                if !d.bg.is_transparent()
                    || (!d.border_color.is_transparent() && node.border.any_nonzero())
                {
                    canvas.paint_quad(PaintQuad {
                        bounds: paint_rect,
                        corner_radii: d.corners,
                        background: d.bg,
                        border_widths: node.border,
                        border_color: d.border_color,
                    });
                }
                canvas.scoped(
                    Scope {
                        clip: reveal_clip.or_else(|| node.clip.then_some(node.rect)),
                        translate: reveal_translate,
                        ..Default::default()
                    },
                    |canvas| match &node.kind {
                        FlatKind::Box {
                            children_start,
                            children_len,
                        } => {
                            for offset in 0..*children_len {
                                self.paint_node(
                                    self.child_order[*children_start + offset],
                                    canvas,
                                    draw_custom,
                                );
                            }
                        }
                        FlatKind::Text { style, label, .. } => {
                            if let Some(label) = label {
                                canvas.draw_label(
                                    label,
                                    node.rect.origin,
                                    node.rect.size.width,
                                    style.color,
                                );
                            }
                        }
                        FlatKind::Custom(token) => {
                            draw_custom(*token, node.rect, canvas);
                        }
                    },
                );
            },
        );
    }
}

pub(crate) struct UiState<H: Copy + Eq + Hash, C: Copy, K: Copy + Eq + Hash> {
    frame: Frame<H, C>,
    pointer: Option<PointerState>,
    hovered_chain: Vec<H>,
    pressed: Option<H>,
    last_input_was_keyboard: bool,
    anims: AnimTracks<K>,
}

impl<H: Copy + Eq + Hash, C: Copy, K: Copy + Eq + Hash> Default for UiState<H, C, K> {
    fn default() -> Self {
        Self {
            frame: Frame::default(),
            pointer: None,
            hovered_chain: Vec::new(),
            pressed: None,
            last_input_was_keyboard: false,
            anims: AnimTracks::default(),
        }
    }
}

impl<H: Copy + Eq + Hash, C: Copy, K: Copy + Eq + Hash> UiState<H, C, K> {
    #[inline]
    pub(crate) fn frame(&self) -> &Frame<H, C> {
        &self.frame
    }

    #[inline]
    pub(crate) fn pointer(&self) -> Option<PointerState> {
        self.pointer
    }

    #[inline]
    pub(crate) fn last_input_was_keyboard(&self) -> bool {
        self.last_input_was_keyboard
    }

    #[inline]
    pub(crate) fn mark_keyboard_input(&mut self) {
        self.last_input_was_keyboard = true;
    }

    #[inline]
    pub(crate) fn mark_pointer_input(&mut self) {
        self.last_input_was_keyboard = false;
    }

    pub(crate) fn begin_frame(&mut self, now: Instant, reduce_motion: bool) {
        self.anims.begin_frame(now, reduce_motion);
    }

    pub(crate) fn finish_frame(&mut self, frame: Frame<H, C>) -> bool {
        self.frame = frame;
        let animating = self.anims.end_frame();
        let hover_shifted = self.pointer.is_some_and(|p| self.refresh_hover(p.pos));
        animating || hover_shifted
    }

    fn refresh_hover(&mut self, pos: Point) -> bool {
        let next = self.frame.hit_chain(pos);
        if next != self.hovered_chain {
            self.hovered_chain = next;
            true
        } else {
            false
        }
    }

    pub(crate) fn on_mouse_move(&mut self, pos: Point, pressed_any: bool) -> bool {
        self.pointer = Some(PointerState {
            pos,
            pressed: pressed_any,
        });
        self.refresh_hover(pos)
    }

    pub(crate) fn on_mouse_leave(&mut self) -> bool {
        self.pointer = None;
        self.clear_hover()
    }

    pub(crate) fn on_capture_lost(&mut self) -> bool {
        if let Some(p) = &mut self.pointer {
            p.pressed = false;
        }
        self.clear_hover()
    }

    fn clear_hover(&mut self) -> bool {
        let had = !self.hovered_chain.is_empty() || self.pressed.is_some();
        self.hovered_chain.clear();
        self.pressed = None;
        had
    }

    pub(crate) fn on_mouse_down(&mut self, pos: Point, button: MouseButton) -> &[H] {
        self.last_input_was_keyboard = false;
        self.pointer = Some(PointerState { pos, pressed: true });
        self.hovered_chain = self.frame.hit_chain(pos);
        if button == MouseButton::Left {
            self.pressed = self.hovered_chain.first().copied();
        }
        &self.hovered_chain
    }

    pub(crate) fn on_mouse_up(&mut self, pos: Point, button: MouseButton) -> (Option<H>, &[H]) {
        if let Some(p) = &mut self.pointer {
            p.pos = pos;
            p.pressed = false;
        }
        self.hovered_chain = self.frame.hit_chain(pos);
        let clicked = if button == MouseButton::Left {
            let was = self.pressed.take();
            let now = self.hovered_chain.first().copied();
            was.filter(|_| was == now)
        } else {
            None
        };
        (clicked, &self.hovered_chain)
    }

    #[inline]
    pub(crate) fn top_hit(&self) -> Option<H> {
        self.hovered_chain.first().copied()
    }

    #[inline]
    pub(crate) fn is_hovered(&self, hit: H) -> bool {
        self.hovered_chain.contains(&hit)
    }

    #[inline]
    pub(crate) fn is_pressed(&self, hit: H) -> bool {
        self.pressed == Some(hit) && self.hovered_chain.contains(&hit)
    }

    pub(crate) fn brush(&mut self, hit: H, brushes: Brushes, enabled: bool) -> Rgba
    where
        K: From<H>,
    {
        let target = if !enabled {
            brushes.rest
        } else if self.is_pressed(hit) {
            brushes.pressed
        } else if self.is_hovered(hit) {
            brushes.hover
        } else {
            brushes.rest
        };
        self.anims
            .animate_color(K::from(hit), target, BRUSH_FADE_DURATION, Easing::Linear)
    }

    pub(crate) fn progress(&mut self, key: K, active: bool, curve: Curve) -> AnimSample<f32> {
        self.anims.progress(key, active, curve)
    }

    pub(crate) fn animate_f32_from(
        &mut self,
        key: K,
        initial: f32,
        target: f32,
        duration: Duration,
        easing: Easing,
    ) -> AnimSample<f32> {
        self.anims
            .animate_f32_from(key, initial, target, duration, easing)
    }

    #[cfg(test)]
    pub(crate) fn track_count(&self) -> usize {
        self.anims.track_count()
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct OverlayAnchor {
    pub(crate) window: WindowId,
    pub(crate) origin: ScreenPoint,
    pub(crate) scale: f32,
}

#[derive(Default)]
pub(crate) struct OverlaySurface {
    live: Option<WindowSurface>,
    create_failed: bool,
}

impl OverlaySurface {
    pub(crate) fn show(&mut self, gpu: &Rc<Gpu>, rect: ScreenRect) {
        if self.live.is_none() {
            if self.create_failed {
                return;
            }
            match NativeWindow::create_overlay()
                .and_then(|window| WindowSurface::new(gpu.clone(), window))
            {
                Ok(live) => self.live = Some(live),
                Err(err) => {
                    self.create_failed = true;
                    crate::platform::debug_log(&format!("overlay create failed: {err}"));
                    return;
                }
            }
        }
        if let Some(live) = &self.live {
            let hwnd = live.window.hwnd();
            hwnd.place_topmost_noactivate(rect);
            hwnd.show_noactivate();
        }
    }

    pub(crate) fn hide(&self) {
        if let Some(live) = &self.live {
            live.window.hwnd().hide();
        }
    }

    pub(crate) fn paint_scene(
        &mut self,
        ts: &TextSystem,
        build: impl FnOnce(Size) -> Node<(), ()>,
    ) -> bool {
        let Some(live) = &mut self.live else {
            return false;
        };
        let Some((w, h, scale)) = live.window.sync_live_metrics() else {
            return false;
        };
        live.surface.resize(w, h, scale);
        let logical = live.window.logical_size();
        live.surface.draw(transparent_black(), |canvas| {
            let frame = Frame::layout(
                build(logical),
                AvailSize::definite(logical.width, logical.height),
                scale,
                ts,
            );
            frame.paint(canvas, &mut |_, _, _| {});
        })
    }
}

pub(crate) fn cubic_bezier(x1: f32, y1: f32, x2: f32, y2: f32, t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if t <= 0.0 {
        return 0.0;
    }
    if t >= 1.0 {
        return 1.0;
    }

    let cx = 3.0 * x1;
    let bx = 3.0 * (x2 - x1) - cx;
    let ax = 1.0 - cx - bx;

    let cy = 3.0 * y1;
    let by = 3.0 * (y2 - y1) - cy;
    let ay = 1.0 - cy - by;

    let eval_x = |s: f32| ((ax * s + bx) * s + cx) * s;
    let eval_y = |s: f32| ((ay * s + by) * s + cy) * s;
    let deriv_x = |s: f32| (3.0 * ax * s + 2.0 * bx) * s + cx;

    const EPS: f32 = 1e-6;
    let mut s = t;
    let mut solved = false;
    for _ in 0..8 {
        let residual = eval_x(s) - t;
        if residual.abs() < EPS {
            solved = true;
            break;
        }
        let slope = deriv_x(s);
        if slope.abs() < EPS {
            break;
        }
        s = (s - residual / slope).clamp(0.0, 1.0);
    }

    if !solved {
        let mut lo = 0.0f32;
        let mut hi = 1.0f32;
        s = t;
        for _ in 0..32 {
            let x = eval_x(s);
            if (x - t).abs() < EPS {
                break;
            }
            if x < t {
                lo = s;
            } else {
                hi = s;
            }
            s = 0.5 * (lo + hi);
        }
    }

    eval_y(s).clamp(0.0, 1.0)
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) enum Easing {
    Linear,
    #[default]
    EaseOutCubic,
    EaseInCubic,
    CubicBezier {
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
    },
}

impl Easing {
    pub(crate) const DECELERATE: Self = Self::CubicBezier {
        x1: 0.0,
        y1: 0.0,
        x2: 0.0,
        y2: 1.0,
    };

    #[inline]
    pub(crate) fn sample(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Self::Linear => t,
            Self::EaseOutCubic => 1.0 - (1.0 - t).powi(3),
            Self::EaseInCubic => t * t * t,
            Self::CubicBezier { x1, y1, x2, y2 } => cubic_bezier(x1, y1, x2, y2, t),
        }
    }
}

pub(crate) trait Lerp: Copy + PartialEq {
    fn lerp(self, target: Self, t: f32) -> Self;
}

impl Lerp for f32 {
    #[inline]
    fn lerp(self, target: Self, t: f32) -> Self {
        self + (target - self) * t
    }
}

impl Lerp for Rgba {
    #[inline]
    fn lerp(self, target: Self, t: f32) -> Self {
        let a = self.a + (target.a - self.a) * t;
        let channel = |x: f32, y: f32| {
            let premul = x * self.a + (y * target.a - x * self.a) * t;
            if a > 0.0 {
                (premul / a).clamp(0.0, 1.0)
            } else {
                0.0
            }
        };
        Rgba {
            r: channel(self.r, target.r),
            g: channel(self.g, target.g),
            b: channel(self.b, target.b),
            a,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Curve {
    pub(crate) duration: Duration,
    pub(crate) easing: Easing,
}

impl Curve {
    pub(crate) const fn new(duration: Duration, easing: Easing) -> Self {
        Self { duration, easing }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum Presence {
    #[default]
    Hidden,
    Open,
    Closing,
}

impl Presence {
    #[inline]
    pub(crate) fn is_visible(self) -> bool {
        !matches!(self, Self::Hidden)
    }

    #[inline]
    pub(crate) fn is_open(self) -> bool {
        matches!(self, Self::Open)
    }

    #[inline]
    pub(crate) fn is_closing(self) -> bool {
        matches!(self, Self::Closing)
    }
}

fn next_transition_serial() -> usize {
    static NEXT: AtomicUsize = AtomicUsize::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct Transition {
    serial: usize,
    phase: Presence,
}

impl Transition {
    pub(crate) fn open_new() -> Self {
        Self {
            serial: next_transition_serial(),
            phase: Presence::Open,
        }
    }

    pub(crate) fn open(&mut self) {
        if !self.phase.is_open() {
            self.serial = next_transition_serial();
            self.phase = Presence::Open;
        }
    }

    pub(crate) fn close(&mut self) -> bool {
        if !self.phase.is_open() {
            return false;
        }
        self.phase = Presence::Closing;
        true
    }

    pub(crate) fn finish(&mut self) {
        self.phase = Presence::Hidden;
    }

    #[inline]
    pub(crate) fn serial(self) -> usize {
        self.serial
    }

    #[inline]
    pub(crate) fn is_visible(self) -> bool {
        self.phase.is_visible()
    }

    #[inline]
    pub(crate) fn is_open(self) -> bool {
        self.phase.is_open()
    }

    #[inline]
    pub(crate) fn is_closing(self) -> bool {
        self.phase.is_closing()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AnimStatus {
    Idle,
    Running,
    Finished,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct AnimSample<T> {
    pub(crate) value: T,
    pub(crate) status: AnimStatus,
}

#[derive(Clone, Copy, Debug)]
struct AnimTrack<T> {
    from: T,
    target: T,
    started_at: Instant,
    reversing_scale: f32,
    duration: Duration,
    last_frame: u64,
}

impl<T: Lerp> AnimTrack<T> {
    fn sample_and_update(
        &mut self,
        target: T,
        duration: Duration,
        easing: Easing,
        now: Instant,
        reduce_motion: bool,
        frame: u64,
    ) -> AnimSample<T> {
        self.last_frame = frame;

        if reduce_motion || duration.is_zero() {
            self.from = target;
            self.target = target;
            self.started_at = now;
            self.reversing_scale = 1.0;
            self.duration = duration;
            return AnimSample {
                value: target,
                status: AnimStatus::Finished,
            };
        }

        let elapsed = now.saturating_duration_since(self.started_at);
        let (progress, status) = if self.duration.is_zero() || elapsed >= self.duration {
            (1.0, AnimStatus::Finished)
        } else {
            (
                elapsed.as_secs_f32() / self.duration.as_secs_f32(),
                AnimStatus::Running,
            )
        };
        let eased = easing.sample(progress);
        let current = self.from.lerp(self.target, eased);

        if self.target != target {
            let reversing = target == self.from;
            let reversing_scale = if reversing {
                (eased * self.reversing_scale + (1.0 - self.reversing_scale)).clamp(0.0, 1.0)
            } else {
                1.0
            };
            let next_duration = duration.mul_f32(reversing_scale);
            self.from = current;
            self.target = target;
            self.started_at = now;
            self.reversing_scale = reversing_scale;
            self.duration = next_duration;
            if next_duration.is_zero() {
                AnimSample {
                    value: target,
                    status: AnimStatus::Finished,
                }
            } else {
                AnimSample {
                    value: current,
                    status: AnimStatus::Running,
                }
            }
        } else if self.from == self.target {
            AnimSample {
                value: current,
                status: AnimStatus::Idle,
            }
        } else {
            AnimSample {
                value: current,
                status,
            }
        }
    }
}

pub(crate) struct AnimTracks<K: Copy + Eq + Hash> {
    frame: u64,
    now: Instant,
    reduce_motion: bool,
    running_this_frame: bool,
    scalars: HashMap<K, AnimTrack<f32>>,
    colors: HashMap<K, AnimTrack<Rgba>>,
}

impl<K: Copy + Eq + Hash> Default for AnimTracks<K> {
    fn default() -> Self {
        Self {
            frame: 0,
            now: Instant::now(),
            reduce_motion: false,
            running_this_frame: false,
            scalars: HashMap::new(),
            colors: HashMap::new(),
        }
    }
}

impl<K: Copy + Eq + Hash> AnimTracks<K> {
    pub(crate) fn begin_frame(&mut self, now: Instant, reduce_motion: bool) {
        self.frame = self.frame.wrapping_add(1);
        self.now = now;
        self.reduce_motion = reduce_motion;
        self.running_this_frame = false;
    }

    pub(crate) fn end_frame(&mut self) -> bool {
        let frame = self.frame;
        self.scalars.retain(|_, track| track.last_frame == frame);
        self.colors.retain(|_, track| track.last_frame == frame);
        self.running_this_frame
    }

    pub(crate) fn progress(&mut self, key: K, active: bool, curve: Curve) -> AnimSample<f32> {
        self.animate_f32_from(
            key,
            0.0,
            if active { 1.0 } else { 0.0 },
            curve.duration,
            curve.easing,
        )
    }

    pub(crate) fn animate_f32_from(
        &mut self,
        key: K,
        initial: f32,
        target: f32,
        duration: Duration,
        easing: Easing,
    ) -> AnimSample<f32> {
        let frame = self.frame;
        let now = self.now;
        let reduce_motion = self.reduce_motion;
        let track = self.scalars.entry(key).or_insert_with(|| AnimTrack {
            from: initial,
            target,
            started_at: now,
            reversing_scale: 1.0,
            duration,
            last_frame: frame,
        });
        let sample = track.sample_and_update(target, duration, easing, now, reduce_motion, frame);
        if sample.status == AnimStatus::Running {
            self.running_this_frame = true;
        }
        sample
    }

    pub(crate) fn animate_color(
        &mut self,
        key: K,
        target: Rgba,
        duration: Duration,
        easing: Easing,
    ) -> Rgba {
        let frame = self.frame;
        let now = self.now;
        let reduce_motion = self.reduce_motion;
        let track = self.colors.entry(key).or_insert_with(|| AnimTrack {
            from: target,
            target,
            started_at: now,
            reversing_scale: 1.0,
            duration,
            last_frame: frame,
        });
        let sample = track.sample_and_update(target, duration, easing, now, reduce_motion, frame);
        if sample.status == AnimStatus::Running {
            self.running_this_frame = true;
        }
        sample.value
    }

    #[cfg(test)]
    pub(crate) fn track_count(&self) -> usize {
        self.scalars.len() + self.colors.len()
    }
}

#[inline]
fn round_half_toward_zero(value: f32) -> f32 {
    (value.abs() - 0.5).ceil().copysign(value)
}

#[inline]
fn round_px(logical: f32, scale: f32) -> f32 {
    round_half_toward_zero(logical * scale)
}

#[inline]
fn round_stroke(logical: f32, scale: f32) -> f32 {
    if logical == 0.0 {
        0.0
    } else {
        round_px(logical.max(0.0), scale).max(1.0)
    }
}

#[inline]
pub(crate) fn snap_border_edges(border: Edges<f32>, scale: f32) -> Edges<f32> {
    let s = scale.max(0.5);
    Edges {
        top: round_stroke(border.top, s) / s,
        right: round_stroke(border.right, s) / s,
        bottom: round_stroke(border.bottom, s) / s,
        left: round_stroke(border.left, s) / s,
    }
}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub(crate) struct LayoutId(pub(crate) u32);

impl LayoutId {
    #[inline]
    fn idx(self) -> usize {
        self.0 as usize
    }
}

#[derive(Copy, Clone, Default, Debug, PartialEq)]
pub(crate) enum Space {
    Definite(f32),
    #[default]
    MinContent,
    MaxContent,
}

impl Space {
    #[inline]
    fn scale(self, factor: f32) -> Self {
        match self {
            Self::Definite(v) => Self::Definite(v * factor),
            other => other,
        }
    }

    #[inline]
    fn maybe_sub(self, delta: f32) -> Self {
        match self {
            Self::Definite(v) => Self::Definite((v - delta).max(0.0)),
            other => other,
        }
    }

    #[inline]
    fn maybe_min(self, limit: Option<f32>) -> Self {
        match (self, limit) {
            (Self::Definite(a), Some(m)) => Self::Definite(a.min(m)),
            (Self::MaxContent, Some(m)) => Self::Definite(m),
            _ => self,
        }
    }

    #[inline]
    fn maybe_max(self, floor: Option<f32>) -> Self {
        match (self, floor) {
            (Self::Definite(a), Some(m)) => Self::Definite(a.max(m)),
            _ => self,
        }
    }

    #[inline]
    fn maybe_set(self, known: Option<f32>) -> Self {
        known.map_or(self, Self::Definite)
    }

    #[inline]
    fn key(self) -> (u8, u32) {
        match self {
            Self::Definite(v) => (0, v.to_bits()),
            Self::MinContent => (1, 0),
            Self::MaxContent => (2, 0),
        }
    }
}

#[derive(Copy, Clone, Default, Debug, PartialEq)]
pub(crate) struct AvailSize {
    pub(crate) width: Space,
    pub(crate) height: Space,
}

impl AvailSize {
    pub(crate) const MIN: Self = Self {
        width: Space::MinContent,
        height: Space::MinContent,
    };

    pub(crate) const MAX: Self = Self {
        width: Space::MaxContent,
        height: Space::MaxContent,
    };

    #[inline]
    pub(crate) const fn definite(width: f32, height: f32) -> Self {
        Self {
            width: Space::Definite(width),
            height: Space::Definite(height),
        }
    }
}

impl From<Size> for AvailSize {
    #[inline]
    fn from(s: Size) -> Self {
        Self::definite(s.width, s.height)
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub(crate) enum Dim {
    #[default]
    Auto,
    Px(f32),
    Full,
}

impl Dim {
    #[inline]
    fn snap(self, scale: f32) -> Self {
        match self {
            Self::Px(v) => Self::Px(round_px(v, scale)),
            other => other,
        }
    }

    #[inline]
    fn resolve(self, base: Option<f32>) -> Option<f32> {
        match self {
            Self::Auto => None,
            Self::Px(v) => Some(v),
            Self::Full => base,
        }
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum Flex {
    #[default]
    Shrink,
    Fill,
    Fixed,
}

impl Flex {
    #[inline]
    fn can_grow(self) -> bool {
        matches!(self, Self::Fill)
    }

    #[inline]
    fn can_shrink(self) -> bool {
        !matches!(self, Self::Fixed)
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct FlexStyle {
    pub(crate) row: bool,
    pub(crate) absolute: bool,
    pub(crate) clip: [bool; 2],
    pub(crate) inset: Edges<Option<f32>>,
    pub(crate) size: [Dim; 2],
    pub(crate) min: [Dim; 2],
    pub(crate) max: [Dim; 2],
    pub(crate) margin: Edges<f32>,
    pub(crate) padding: Edges<f32>,
    pub(crate) border: Edges<f32>,
    pub(crate) gap: f32,
    pub(crate) items_center: bool,
    pub(crate) justify_center: bool,
    pub(crate) flex: Flex,
}

impl Default for FlexStyle {
    fn default() -> Self {
        Self {
            row: false,
            absolute: false,
            clip: [false; 2],
            inset: Edges::all(None),
            size: [Dim::Auto; 2],
            min: [Dim::Auto; 2],
            max: [Dim::Auto; 2],
            margin: Edges::ZERO,
            padding: Edges::ZERO,
            border: Edges::ZERO,
            gap: 0.0,
            items_center: false,
            justify_center: false,
            flex: Flex::Shrink,
        }
    }
}

impl FlexStyle {
    fn snap_for_scale(self, scale: f32) -> Self {
        let snap_f = |e: Edges<f32>| Edges {
            top: round_px(e.top, scale),
            right: round_px(e.right, scale),
            bottom: round_px(e.bottom, scale),
            left: round_px(e.left, scale),
        };
        let snap_opt = |e: Edges<Option<f32>>| Edges {
            top: e.top.map(|v| round_px(v, scale)),
            right: e.right.map(|v| round_px(v, scale)),
            bottom: e.bottom.map(|v| round_px(v, scale)),
            left: e.left.map(|v| round_px(v, scale)),
        };
        Self {
            inset: snap_opt(self.inset),
            size: [self.size[0].snap(scale), self.size[1].snap(scale)],
            min: [self.min[0].snap(scale), self.min[1].snap(scale)],
            max: [self.max[0].snap(scale), self.max[1].snap(scale)],
            margin: snap_f(self.margin),
            padding: snap_f(self.padding),
            border: Edges {
                top: round_stroke(self.border.top, scale),
                right: round_stroke(self.border.right, scale),
                bottom: round_stroke(self.border.bottom, scale),
                left: round_stroke(self.border.left, scale),
            },
            gap: round_px(self.gap, scale),
            ..self
        }
    }
}

#[inline]
fn edge_start(e: Edges<f32>, axis: usize) -> f32 {
    if axis == 0 { e.left } else { e.top }
}

#[inline]
fn edge_end(e: Edges<f32>, axis: usize) -> f32 {
    if axis == 0 { e.right } else { e.bottom }
}

#[inline]
fn edge_sum(e: Edges<f32>, axis: usize) -> f32 {
    if axis == 0 { e.horiz() } else { e.vert() }
}

#[inline]
fn pair<T>(main_axis: usize, main: T, cross: T) -> [T; 2] {
    if main_axis == 0 {
        [main, cross]
    } else {
        [cross, main]
    }
}

#[inline]
fn clamp_dim(val: f32, min: Option<f32>, max: Option<f32>) -> f32 {
    let v = max.map_or(val, |hi| val.min(hi));
    min.map_or(v, |lo| v.max(lo))
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
enum RunMode {
    PerformLayout,
    ComputeSize,
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
enum SizingMode {
    InherentSize,
    ContentSize,
}

#[derive(Copy, Clone)]
struct LayoutReq {
    known: [Option<f32>; 2],
    parent: [Option<f32>; 2],
    avail: [Space; 2],
    run_mode: RunMode,
    sizing_mode: SizingMode,
}

#[derive(Copy, Clone)]
struct AxisBounds {
    size: Option<f32>,
    min: Option<f32>,
    max: Option<f32>,
    floor: f32,
}

impl AxisBounds {
    #[inline]
    fn resolve(
        size: Dim,
        min: Dim,
        max: Dim,
        base: Option<f32>,
        pb: f32,
        inherent: bool,
    ) -> Self {
        let (size, min, max) = if inherent {
            (size.resolve(base), min.resolve(base), max.resolve(base))
        } else {
            (None, None, None)
        };
        Self {
            size,
            min,
            max,
            floor: min.map_or(pb, |m| m.max(pb)),
        }
    }

    #[inline]
    fn clamp(self, v: f32) -> f32 {
        clamp_dim(v, Some(self.floor), self.max)
    }

    #[inline]
    fn definite(self, known: Option<f32>) -> Option<f32> {
        known.or(self.size).map(|v| self.clamp(v))
    }
}

#[inline]
fn resolve_item_cross(
    cs: &FlexStyle,
    c: usize,
    pb_cross: f32,
    margin_cross: f32,
    items_center: bool,
    inner_cross: Option<f32>,
    stretch: bool,
) -> Option<f32> {
    let b = AxisBounds::resolve(cs.size[c], cs.min[c], cs.max[c], inner_cross, pb_cross, true);
    if let Some(s) = b.size {
        Some(b.clamp(s))
    } else if stretch
        && !items_center
        && let Some(ic) = inner_cross
    {
        Some(b.clamp((ic - margin_cross).max(0.0)))
    } else {
        None
    }
}

#[derive(Copy, Clone, Eq, PartialEq)]
struct SizeCacheKey {
    known: [u32; 2],
    avail: [(u8, u32); 2],
    parent: [u32; 2],
    sizing_mode: SizingMode,
}

#[inline]
fn opt_bits(v: Option<f32>) -> u32 {
    v.map_or(u32::MAX, f32::to_bits)
}

#[derive(Default)]
struct SizeCache {
    entries: [Option<(SizeCacheKey, Size)>; 8],
    next: usize,
}

impl SizeCache {
    fn get(&self, key: SizeCacheKey) -> Option<Size> {
        self.entries
            .iter()
            .flatten()
            .find_map(|&(k, v)| (k == key).then_some(v))
    }

    fn insert(&mut self, key: SizeCacheKey, value: Size) {
        for slot in &mut self.entries {
            if let Some((k, v)) = slot
                && *k == key
            {
                *v = value;
                return;
            }
        }
        self.entries[self.next] = Some((key, value));
        self.next = (self.next + 1) % self.entries.len();
    }
}

struct LayoutNode {
    style: FlexStyle,
    parent: Option<LayoutId>,
    children_start: u32,
    children_len: u32,
    measured: bool,
    location: Point,
    size: Size,
    abs_origin: Option<Point>,
    abs_bounds: Option<Rect>,
    cache: SizeCache,
}

#[derive(Default)]
pub(crate) struct LayoutEngine {
    nodes: Vec<LayoutNode>,
    children_arena: Vec<LayoutId>,
}

impl LayoutEngine {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn reserve_node(&mut self, style: FlexStyle, scale: f32, measured: bool) -> LayoutId {
        let id = LayoutId(self.nodes.len() as u32);
        self.nodes.push(LayoutNode {
            style: style.snap_for_scale(scale),
            parent: None,
            children_start: 0,
            children_len: 0,
            measured,
            location: Point::ZERO,
            size: Size::ZERO,
            abs_origin: None,
            abs_bounds: None,
            cache: SizeCache::default(),
        });
        id
    }

    pub(crate) fn set_children(&mut self, id: LayoutId, children: &[LayoutId]) {
        let start = self.children_arena.len() as u32;
        self.children_arena.extend_from_slice(children);
        for &child in children {
            self.nodes[child.idx()].parent = Some(id);
        }
        let node = &mut self.nodes[id.idx()];
        node.children_start = start;
        node.children_len = children.len() as u32;
    }

    #[cfg(test)]
    fn add_node(
        &mut self,
        style: FlexStyle,
        scale: f32,
        children: &[LayoutId],
        measured: bool,
    ) -> LayoutId {
        let id = self.reserve_node(style, scale, measured);
        self.set_children(id, children);
        id
    }

    pub(crate) fn compute(
        &mut self,
        root: LayoutId,
        available: AvailSize,
        scale: f32,
        measure: &mut impl FnMut(LayoutId, Option<f32>, Option<f32>, AvailSize) -> Size,
    ) {
        let avail = [available.width.scale(scale), available.height.scale(scale)];
        let parent = avail.map(|a| match a {
            Space::Definite(v) => Some(v),
            _ => None,
        });
        let req = LayoutReq {
            known: [None; 2],
            parent,
            avail,
            run_mode: RunMode::PerformLayout,
            sizing_mode: SizingMode::InherentSize,
        };
        let mut solver = Solver {
            nodes: &mut self.nodes,
            children: &self.children_arena,
            scale,
            measure,
        };
        let root_size = solver.compute_node(root, req);
        let node = &mut self.nodes[root.idx()];
        node.location = Point::ZERO;
        node.size = root_size;
    }

    pub(crate) fn bounds(&mut self, id: LayoutId, scale: f32) -> Rect {
        if let Some(b) = self.nodes[id.idx()].abs_bounds {
            return b;
        }
        let parent = self.nodes[id.idx()].parent;
        let location = self.nodes[id.idx()].location;
        let node_size = self.nodes[id.idx()].size;

        let abs_origin = match parent {
            Some(parent_id) => {
                self.bounds(parent_id, scale);
                self.nodes[parent_id.idx()]
                    .abs_origin
                    .unwrap_or(Point::ZERO)
                    + location
            }
            None => location,
        };
        self.nodes[id.idx()].abs_origin = Some(abs_origin);

        let snapped = Rect::from_corners(
            point(
                round_half_toward_zero(abs_origin.x),
                round_half_toward_zero(abs_origin.y),
            ),
            point(
                round_half_toward_zero(abs_origin.x + node_size.width),
                round_half_toward_zero(abs_origin.y + node_size.height),
            ),
        );
        let logical = Rect::new(
            point(snapped.origin.x / scale, snapped.origin.y / scale),
            size(snapped.size.width / scale, snapped.size.height / scale),
        );
        self.nodes[id.idx()].abs_bounds = Some(logical);
        logical
    }

    pub(crate) fn set_origin(&mut self, id: LayoutId, logical_origin: Point, scale: f32) {
        let parent_origin = match self.nodes[id.idx()].parent {
            Some(parent_id) => {
                self.bounds(parent_id, scale);
                self.nodes[parent_id.idx()]
                    .abs_origin
                    .unwrap_or(Point::ZERO)
            }
            None => Point::ZERO,
        };
        let desired = point(
            round_px(logical_origin.x, scale),
            round_px(logical_origin.y, scale),
        );
        self.nodes[id.idx()].location = desired - parent_origin;
        self.invalidate_bounds(id);
    }

    fn invalidate_bounds(&mut self, id: LayoutId) {
        let node = &mut self.nodes[id.idx()];
        node.abs_origin = None;
        node.abs_bounds = None;
        let start = node.children_start as usize;
        let end = start + node.children_len as usize;
        for i in start..end {
            self.invalidate_bounds(self.children_arena[i]);
        }
    }
}

struct Solver<'a, F> {
    nodes: &'a mut [LayoutNode],
    children: &'a [LayoutId],
    scale: f32,
    measure: &'a mut F,
}

impl<'a, F: FnMut(LayoutId, Option<f32>, Option<f32>, AvailSize) -> Size> Solver<'a, F> {
    fn child_slice(&self, id: LayoutId) -> &'a [LayoutId] {
        let node = &self.nodes[id.idx()];
        let start = node.children_start as usize;
        &self.children[start..start + node.children_len as usize]
    }

    fn compute_node(&mut self, id: LayoutId, req: LayoutReq) -> Size {
        if req.run_mode == RunMode::ComputeSize {
            let key = SizeCacheKey {
                known: req.known.map(opt_bits),
                avail: req.avail.map(Space::key),
                parent: req.parent.map(opt_bits),
                sizing_mode: req.sizing_mode,
            };
            if let Some(cached) = self.nodes[id.idx()].cache.get(key) {
                return cached;
            }
            let result = self.compute_uncached(id, req);
            self.nodes[id.idx()].cache.insert(key, result);
            return result;
        }
        let result = self.compute_uncached(id, req);
        self.nodes[id.idx()].size = result;
        result
    }

    fn compute_uncached(&mut self, id: LayoutId, req: LayoutReq) -> Size {
        let node = &self.nodes[id.idx()];
        if node.children_len == 0 || node.measured {
            self.compute_leaf(id, req)
        } else {
            self.compute_flex(id, req)
        }
    }

    fn compute_leaf(&mut self, id: LayoutId, req: LayoutReq) -> Size {
        let node = &self.nodes[id.idx()];
        let style = node.style;
        let measured = node.measured;
        let pb = style.padding.add(style.border);
        let pb_axes = [pb.horiz(), pb.vert()];
        let inherent = req.sizing_mode == SizingMode::InherentSize;
        let bounds = [0, 1].map(|a| {
            AxisBounds::resolve(
                style.size[a],
                style.min[a],
                style.max[a],
                req.parent[a],
                pb_axes[a],
                inherent,
            )
        });
        let known = [0, 1].map(|a| bounds[a].definite(req.known[a]));

        if measured && (known[0].is_none() || known[1].is_none() || req.run_mode == RunMode::PerformLayout) {
            let inv = 1.0 / self.scale;
            let known_inner = [0, 1].map(|a| known[a].map(|v| (v - pb_axes[a]).max(0.0) * inv));
            let avail_inner = [0, 1].map(|a| {
                req.avail[a]
                    .maybe_set(known[a])
                    .maybe_min(bounds[a].max)
                    .maybe_max(bounds[a].min)
                    .maybe_sub(pb_axes[a])
                    .scale(inv)
            });
            let logical = (self.measure)(
                id,
                known_inner[0],
                known_inner[1],
                AvailSize {
                    width: avail_inner[0],
                    height: avail_inner[1],
                },
            );
            let m = [
                (logical.width.max(0.0) * self.scale).ceil(),
                (logical.height.max(0.0) * self.scale).ceil(),
            ];
            return size(
                known[0].unwrap_or_else(|| bounds[0].clamp(m[0] + pb_axes[0])),
                known[1].unwrap_or_else(|| bounds[1].clamp(m[1] + pb_axes[1])),
            );
        }

        size(
            known[0].unwrap_or_else(|| bounds[0].clamp(pb_axes[0])),
            known[1].unwrap_or_else(|| bounds[1].clamp(pb_axes[1])),
        )
    }

    fn compute_flex(&mut self, id: LayoutId, req: LayoutReq) -> Size {
        let style = self.nodes[id.idx()].style;
        let (m, c) = if style.row { (0, 1) } else { (1, 0) };
        let pb = style.padding.add(style.border);
        let pb_main = edge_sum(pb, m);
        let pb_cross = edge_sum(pb, c);
        let inherent = req.sizing_mode == SizingMode::InherentSize;
        let b_main = AxisBounds::resolve(
            style.size[m],
            style.min[m],
            style.max[m],
            req.parent[m],
            pb_main,
            inherent,
        );
        let b_cross = AxisBounds::resolve(
            style.size[c],
            style.min[c],
            style.max[c],
            req.parent[c],
            pb_cross,
            inherent,
        );
        let node_main = b_main.definite(req.known[m]);
        let node_cross = b_cross.definite(req.known[c]);

        let children = self.child_slice(id);
        let in_flow_count = children
            .iter()
            .filter(|&&ch| !self.nodes[ch.idx()].style.absolute)
            .count();

        if in_flow_count == 0 {
            let final_main = node_main.unwrap_or_else(|| b_main.clamp(pb_main));
            let final_cross = node_cross.unwrap_or_else(|| b_cross.clamp(pb_cross));
            let s = pair(m, final_main, final_cross);
            let container_size = size(s[0], s[1]);
            if req.run_mode == RunMode::PerformLayout {
                self.layout_absolute_children(children, container_size, &style);
            }
            return container_size;
        }

        let inner_main_opt = node_main.map(|v| (v - pb_main).max(0.0));
        let inner_cross_opt = node_cross.map(|v| (v - pb_cross).max(0.0));
        let inner_parent = pair(m, inner_main_opt, inner_cross_opt);
        let gap_main = style.gap;

        let avail_main = req.avail[m]
            .maybe_set(node_main)
            .maybe_min(b_main.max)
            .maybe_sub(pb_main);
        let avail_cross = req.avail[c]
            .maybe_set(node_cross)
            .maybe_min(b_cross.max)
            .maybe_sub(pb_cross);

        struct FlexItem {
            id: LayoutId,
            style: FlexStyle,
            pb_main: f32,
            pb_cross: f32,
            flex_basis: f32,
            min_main: f32,
            max_main: Option<f32>,
            hypothetical_main: f32,
            target_main: f32,
            violation: f32,
            frozen: bool,
        }

        let mut items: Vec<FlexItem> = Vec::with_capacity(in_flow_count);
        for &child in children {
            let cs = self.nodes[child.idx()].style;
            if cs.absolute {
                continue;
            }
            let c_pb = cs.padding.add(cs.border);
            let c_pb_main = edge_sum(c_pb, m);
            let c_pb_cross = edge_sum(c_pb, c);

            let c_size_main = cs.size[m].resolve(inner_main_opt);
            let c_min_main = cs.min[m].resolve(inner_main_opt);
            let c_max_main = cs.max[m].resolve(inner_main_opt);
            let margin_cross = edge_sum(cs.margin, c);
            let pre_cross = resolve_item_cross(
                &cs,
                c,
                c_pb_cross,
                margin_cross,
                style.items_center,
                inner_cross_opt,
                true,
            );

            let explicit_basis = match cs.flex {
                Flex::Fill => inner_main_opt.map(|_| 0.0),
                Flex::Shrink | Flex::Fixed => c_size_main,
            };
            let (flex_basis, basis_from_content) = match explicit_basis {
                Some(b) => (b, false),
                None => {
                    let probe = if avail_main == Space::MinContent {
                        Space::MinContent
                    } else {
                        Space::MaxContent
                    };
                    let sz = self.compute_node(
                        child,
                        LayoutReq {
                            known: pair(m, None, pre_cross),
                            parent: inner_parent,
                            avail: pair(m, probe, avail_cross.maybe_sub(margin_cross)),
                            run_mode: RunMode::ComputeSize,
                            sizing_mode: SizingMode::InherentSize,
                        },
                    );
                    ([sz.width, sz.height][m], true)
                }
            };

            let effective_min_main = if let Some(min_m) = c_min_main {
                min_m.max(c_pb_main)
            } else if cs.clip[m]
                || c_max_main == Some(0.0)
                || c_size_main.is_some_and(|s| s == 0.0 || (!cs.flex.can_shrink() && flex_basis >= s))
                || (!cs.flex.can_shrink() && basis_from_content)
            {
                c_pb_main
            } else {
                let sz = self.compute_node(
                    child,
                    LayoutReq {
                        known: pair(m, None, pre_cross),
                        parent: inner_parent,
                        avail: pair(m, Space::MinContent, avail_cross.maybe_sub(margin_cross)),
                        run_mode: RunMode::ComputeSize,
                        sizing_mode: SizingMode::ContentSize,
                    },
                );
                let mut auto_min = [sz.width, sz.height][m];
                if let Some(s) = c_size_main {
                    auto_min = auto_min.min(s);
                }
                if let Some(mx) = c_max_main {
                    auto_min = auto_min.min(mx);
                }
                auto_min.max(c_pb_main)
            };

            let effective_max_main = c_max_main.map(|mx| mx.max(effective_min_main));
            let hypothetical_main =
                clamp_dim(flex_basis, Some(effective_min_main), effective_max_main);

            items.push(FlexItem {
                id: child,
                style: cs,
                pb_main: c_pb_main,
                pb_cross: c_pb_cross,
                flex_basis,
                min_main: effective_min_main,
                max_main: effective_max_main,
                hypothetical_main,
                target_main: hypothetical_main,
                violation: 0.0,
                frozen: false,
            });
        }

        let total_gaps = gap_main * items.len().saturating_sub(1) as f32;
        let total_hypothetical: f32 = total_gaps
            + items
                .iter()
                .map(|it| it.hypothetical_main + edge_sum(it.style.margin, m))
                .sum::<f32>();

        let (container_main, resolve_flex) = match node_main {
            Some(v) => (v, true),
            None => {
                let raw = total_hypothetical + pb_main;
                let clamped = b_main.clamp(raw);
                (clamped, (clamped - raw).abs() > 1e-4)
            }
        };
        let inner_main = (container_main - pb_main).max(0.0);

        if resolve_flex {
            let growing = total_hypothetical < inner_main;
            for item in &mut items {
                item.frozen = if growing {
                    !item.style.flex.can_grow() || item.flex_basis > item.hypothetical_main
                } else {
                    !item.style.flex.can_shrink() || item.flex_basis < item.hypothetical_main
                };
                item.target_main = if item.frozen {
                    item.hypothetical_main
                } else {
                    item.flex_basis
                };
            }

            loop {
                let mut unfrozen = 0usize;
                let mut used = total_gaps;
                for item in &items {
                    let base = if item.frozen {
                        item.target_main
                    } else {
                        unfrozen += 1;
                        item.flex_basis
                    };
                    used += base + edge_sum(item.style.margin, m);
                }
                if unfrozen == 0 {
                    break;
                }

                let remaining_free = inner_main - used;
                if growing && remaining_free > 0.0 {
                    let share = remaining_free / unfrozen as f32;
                    for item in items.iter_mut().filter(|it| !it.frozen) {
                        item.target_main = item.flex_basis + share;
                    }
                } else if !growing && remaining_free < 0.0 {
                    let mut scaled_sum: f32 = items
                        .iter()
                        .filter(|it| !it.frozen)
                        .map(|it| (it.flex_basis - it.pb_main).max(0.0))
                        .sum();
                    let use_raw = scaled_sum == 0.0;
                    if use_raw {
                        scaled_sum = items
                            .iter()
                            .filter(|it| !it.frozen)
                            .map(|it| it.flex_basis.max(1.0))
                            .sum();
                    }
                    if scaled_sum > 0.0 {
                        for item in items.iter_mut().filter(|it| !it.frozen) {
                            let factor = if use_raw {
                                item.flex_basis.max(1.0)
                            } else {
                                (item.flex_basis - item.pb_main).max(0.0)
                            };
                            item.target_main = item.flex_basis + remaining_free * (factor / scaled_sum);
                        }
                    }
                }

                let mut total_violation = 0.0f32;
                for item in &mut items {
                    if item.frozen {
                        item.violation = 0.0;
                    } else {
                        let clamped =
                            clamp_dim(item.target_main, Some(item.min_main), item.max_main);
                        item.violation = clamped - item.target_main;
                        item.target_main = clamped;
                        total_violation += item.violation;
                    }
                }

                if total_violation.abs() < 1e-4 {
                    break;
                }
                let mut froze_any = false;
                for item in items.iter_mut().filter(|it| !it.frozen) {
                    if (total_violation > 0.0 && item.violation > 0.0)
                        || (total_violation < 0.0 && item.violation < 0.0)
                    {
                        item.frozen = true;
                        froze_any = true;
                    }
                }
                if !froze_any {
                    break;
                }
            }
        }

        let parent_with_main = pair(m, Some(inner_main), inner_cross_opt);
        let container_cross = match node_cross {
            Some(v) => v,
            None => {
                let mut max_outer_cross = 0.0f32;
                for item in &items {
                    let margin_cross = edge_sum(item.style.margin, c);
                    let c_known_cross = resolve_item_cross(
                        &item.style,
                        c,
                        item.pb_cross,
                        margin_cross,
                        style.items_center,
                        inner_cross_opt,
                        false,
                    );
                    let sz = self.compute_node(
                        item.id,
                        LayoutReq {
                            known: pair(m, Some(item.target_main), c_known_cross),
                            parent: parent_with_main,
                            avail: pair(
                                m,
                                Space::Definite(item.target_main),
                                avail_cross.maybe_sub(margin_cross),
                            ),
                            run_mode: RunMode::ComputeSize,
                            sizing_mode: SizingMode::InherentSize,
                        },
                    );
                    max_outer_cross = max_outer_cross.max([sz.width, sz.height][c] + margin_cross);
                }
                b_cross.clamp(max_outer_cross + pb_cross)
            }
        };

        let s = pair(m, container_main, container_cross);
        let container_size = size(s[0], s[1]);
        if req.run_mode == RunMode::ComputeSize {
            return container_size;
        }

        let inner_cross = (container_cross - pb_cross).max(0.0);
        let final_parent = pair(m, Some(inner_main), Some(inner_cross));
        let mut final_sizes: Vec<Size> = Vec::with_capacity(items.len());
        let mut used_main = total_gaps;
        for item in &items {
            let margin_cross = edge_sum(item.style.margin, c);
            let final_cross_known = resolve_item_cross(
                &item.style,
                c,
                item.pb_cross,
                margin_cross,
                style.items_center,
                Some(inner_cross),
                true,
            );
            let child_size = self.compute_node(
                item.id,
                LayoutReq {
                    known: pair(m, Some(item.target_main), final_cross_known),
                    parent: final_parent,
                    avail: pair(
                        m,
                        Space::Definite(item.target_main),
                        Space::Definite((inner_cross - margin_cross).max(0.0)),
                    ),
                    run_mode: RunMode::PerformLayout,
                    sizing_mode: SizingMode::InherentSize,
                },
            );
            used_main += [child_size.width, child_size.height][m] + edge_sum(item.style.margin, m);
            final_sizes.push(child_size);
        }

        let main_offset = if style.justify_center {
            (inner_main - used_main) / 2.0
        } else {
            0.0
        };
        let mut main_cursor = edge_start(pb, m) + main_offset;
        let cross_origin = edge_start(pb, c);

        for (item, child_size) in items.iter().zip(final_sizes.iter()) {
            let cs = [child_size.width, child_size.height];
            let main_pos = main_cursor + edge_start(item.style.margin, m);
            let cross_pos = if style.items_center {
                cross_origin
                    + (inner_cross - (cs[c] + edge_sum(item.style.margin, c))) / 2.0
                    + edge_start(item.style.margin, c)
            } else {
                cross_origin + edge_start(item.style.margin, c)
            };
            let pos = pair(m, main_pos, cross_pos);
            let inset = item.style.inset;
            let rel_x = inset.left.or_else(|| inset.right.map(|r| -r)).unwrap_or(0.0);
            let rel_y = inset.top.or_else(|| inset.bottom.map(|b| -b)).unwrap_or(0.0);
            self.nodes[item.id.idx()].location = point(pos[0] + rel_x, pos[1] + rel_y);
            main_cursor += edge_start(item.style.margin, m)
                + cs[m]
                + edge_end(item.style.margin, m)
                + gap_main;
        }

        self.layout_absolute_children(children, container_size, &style);
        container_size
    }

    fn layout_absolute_children(
        &mut self,
        children: &[LayoutId],
        parent_size: Size,
        parent_style: &FlexStyle,
    ) {
        let border = parent_style.border;
        let padding = parent_style.padding;
        let cb_origin = point(border.left, border.top);
        let cb = [
            (parent_size.width - border.horiz()).max(0.0),
            (parent_size.height - border.vert()).max(0.0),
        ];
        let content = [
            (cb[0] - padding.horiz()).max(0.0),
            (cb[1] - padding.vert()).max(0.0),
        ];
        let (m, c) = if parent_style.row { (0, 1) } else { (1, 0) };

        for &child in children {
            let cs = self.nodes[child.idx()].style;
            if !cs.absolute {
                continue;
            }
            let c_pb = cs.padding.add(cs.border);
            let c_pb_axes = [c_pb.horiz(), c_pb.vert()];
            let insets = [
                (cs.inset.left, cs.inset.right),
                (cs.inset.top, cs.inset.bottom),
            ];
            let margin_axes = [cs.margin.horiz(), cs.margin.vert()];
            let bounds = [0, 1].map(|a| {
                AxisBounds::resolve(
                    cs.size[a],
                    cs.min[a],
                    cs.max[a],
                    Some(cb[a]),
                    c_pb_axes[a],
                    true,
                )
            });
            let known = [0, 1].map(|a| {
                bounds[a].definite(None).or_else(|| match insets[a] {
                    (Some(s), Some(e)) => {
                        Some(bounds[a].clamp((cb[a] - s - e - margin_axes[a]).max(0.0)))
                    }
                    _ => None,
                })
            });
            let avail = [0, 1].map(|a| {
                known[a]
                    .or(bounds[a].max)
                    .map_or(Space::MaxContent, Space::Definite)
            });

            let child_size = self.compute_node(
                child,
                LayoutReq {
                    known,
                    parent: [Some(cb[0]), Some(cb[1])],
                    avail,
                    run_mode: RunMode::PerformLayout,
                    sizing_mode: SizingMode::InherentSize,
                },
            );
            let child_axes = [child_size.width, child_size.height];

            let main_off = if parent_style.justify_center {
                (content[m] - (child_axes[m] + edge_sum(cs.margin, m))) / 2.0
            } else {
                0.0
            };
            let cross_off = if parent_style.items_center {
                (content[c] - (child_axes[c] + edge_sum(cs.margin, c))) / 2.0
            } else {
                0.0
            };
            let static_off = pair(m, main_off, cross_off);

            let pos = [0, 1].map(|a| {
                let origin = if a == 0 { cb_origin.x } else { cb_origin.y };
                let pad_s = edge_start(padding, a);
                let ms = edge_start(cs.margin, a);
                let me = edge_end(cs.margin, a);
                if let Some(s) = insets[a].0 {
                    origin + s + ms
                } else if let Some(e) = insets[a].1 {
                    origin + cb[a] - e - me - child_axes[a]
                } else {
                    origin + pad_s + static_off[a] + ms
                }
            });
            self.nodes[child.idx()].location = point(pos[0], pos[1]);
        }
    }
}

crate::types::id_type!(TabId);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum MenuKind {
    File,
    Edit,
    View,
}

impl MenuKind {
    pub(crate) const ALL: [Self; 3] = [Self::File, Self::Edit, Self::View];

    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::File => "File",
            Self::Edit => "Edit",
            Self::View => "View",
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum EditorSlot {
    Tab(TabId),
    FindQuery,
    FindReplace,
    GoToLine,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Focus {
    Editor(EditorSlot),
    MenuBar,
    DialogButton(usize),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum CustomDraw {
    Editor(EditorSlot),
    StatusSpacer,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum CaptionHit {
    Min,
    Max,
    Close,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum TabHit {
    Strip,
    List,
    Item(TabId),
    Close(TabId),
    New,
    ScrollLeft,
    ScrollRight,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum MenuHit {
    Bar,
    Trigger(MenuKind),
    Panel,
    Item { serial: usize, ix: usize },
    Command { serial: usize, ix: usize },
    SubmenuPanel,
    SubmenuItem { serial: usize, ix: usize },
}

impl MenuHit {
    #[inline]
    pub(crate) fn is_popup(self) -> bool {
        !matches!(self, Self::Bar | Self::Trigger(_))
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum FindHit {
    Card,
    ToggleReplace,
    Next,
    Prev,
    Options,
    Close,
    Replace,
    ReplaceAll,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum DialogHit {
    Backdrop,
    Card,
    Button(usize),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum HitId {
    Caption(CaptionHit),
    Tab(TabHit),
    Menu(MenuHit),
    Find(FindHit),
    Dialog(DialogHit),
    ThemeToggle,
    EditorArea,
    Editor(EditorSlot),
    ScrollTrack(EditorSlot, Axis),
    ScrollThumb(EditorSlot, Axis),
    FieldClear(EditorSlot),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum AnimKey {
    Hit(HitId),
    TabX(usize),
    TabWidth(usize),
    TabScroll,
    TabExtent,
    Transition { serial: usize, track: u8 },
    ScrollOpacity(EditorSlot),
    ScrollThickness(EditorSlot, Axis),
}

impl From<HitId> for AnimKey {
    #[inline]
    fn from(hit: HitId) -> Self {
        Self::Hit(hit)
    }
}

pub(crate) type AppNode = Node<HitId, CustomDraw>;
pub(crate) type AppFrame = Frame<HitId, CustomDraw>;
pub(crate) type AppUi = UiState<HitId, CustomDraw, AnimKey>;

#[cfg(test)]
mod tests {
    use crate::geom::{point, rect, rgb, white};

    use super::*;

    #[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
    enum TestHit {
        Strip,
        Tab(u64),
        TabClose(u64),
        NewTab,
        Backdrop,
        Button(usize),
    }

    #[test]
    fn scene_hit_chain_bubbles_nested_hits_and_clips_overflow() {
        let ts = TextSystem::new().unwrap();
        let tree: Node<TestHit, ()> = row()
            .size_full()
            .child(
                row()
                    .w(120.)
                    .h(32.)
                    .overflow_hidden()
                    .hit(TestHit::Strip)
                    .child(
                        row()
                            .w(100.)
                            .h(32.)
                            .flex_none()
                            .hit(TestHit::Tab(1))
                            .child(
                                box_()
                                    .absolute()
                                    .right(6.)
                                    .top(5.)
                                    .w(22.)
                                    .h(22.)
                                    .hit(TestHit::TabClose(1))
                                    .tooltip("Close tab"),
                            ),
                    )
                    .child(row().w(100.).h(32.).flex_none().hit(TestHit::Tab(2))),
            )
            .child(box_().w(40.).h(32.).hit(TestHit::NewTab).bg(rgb(0x123456)));

        let frame = Frame::layout(tree, AvailSize::definite(200., 32.), 1.0, &ts);

        assert_eq!(
            frame.hit_chain(point(80., 16.)),
            vec![TestHit::TabClose(1), TestHit::Tab(1), TestHit::Strip]
        );
        assert_eq!(
            frame.tooltip_at(point(80., 16.)).map(|(_, tip, _)| tip),
            Some(SharedStr::Static("Close tab"))
        );

        assert_eq!(frame.hit_chain(point(130., 16.)), vec![TestHit::NewTab]);

        let modal: Node<TestHit, ()> = box_()
            .size_full()
            .child(box_().w(100.).h(40.).hit(TestHit::NewTab))
            .child(
                box_()
                    .absolute()
                    .inset_0()
                    .hit(TestHit::Backdrop)
                    .child(box_().w(80.).h(32.).hit(TestHit::Button(0)).bg(white())),
            );
        let modal_frame = Frame::layout(modal, AvailSize::definite(200., 100.), 1.0, &ts);
        assert_eq!(
            modal_frame.hit_chain(point(10., 10.)),
            vec![TestHit::Button(0), TestHit::Backdrop]
        );
        assert_eq!(
            modal_frame.hit_chain(point(95., 10.)),
            vec![TestHit::Backdrop]
        );

        let passthrough: Node<TestHit, ()> = box_()
            .size_full()
            .child(box_().w(100.).h(40.).hit(TestHit::NewTab))
            .child(box_().absolute().inset_0());
        let pass_frame = Frame::layout(passthrough, AvailSize::definite(200., 100.), 1.0, &ts);
        assert_eq!(pass_frame.hit_chain(point(10., 10.)), vec![TestHit::NewTab]);
    }

    #[test]
    fn same_frame_anchor_and_measure_fit_work() {
        #[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
        enum AnchorHit {
            Trigger,
            Popup,
            Item,
            Submenu,
            FlyoutBtn,
            Flyout,
        }

        let ts = TextSystem::new().unwrap();
        let tree: Node<AnchorHit, ()> = box_()
            .size_full()
            .child(
                row()
                    .h(32.)
                    .pl(120.)
                    .child(box_().w(50.).h(32.).hit(AnchorHit::Trigger)),
            )
            .child(
                box_()
                    .absolute()
                    .left(140.)
                    .top(60.)
                    .w(30.)
                    .h(30.)
                    .hit(AnchorHit::FlyoutBtn),
            )
            .child(
                box_()
                    .w(120.)
                    .h(80.)
                    .hit(AnchorHit::Popup)
                    .anchored(Anchor::Target {
                        target: AnchorHit::Trigger,
                        place: |r, _, _| point(r.left(), r.bottom()),
                    })
                    .child(
                        box_()
                            .absolute()
                            .left(1.)
                            .top(20.)
                            .w(118.)
                            .h(30.)
                            .hit(AnchorHit::Item),
                    )
                    .child(
                        box_()
                            .w(100.)
                            .h(60.)
                            .hit(AnchorHit::Submenu)
                            .anchored(Anchor::Target {
                                target: AnchorHit::Item,
                                place: |r, popup, vp| {
                                    let right_x = r.right() - 3.;
                                    let x = if right_x + popup.width <= vp.width {
                                        right_x
                                    } else {
                                        r.left() - popup.width + 3.
                                    };
                                    point(x, r.top() - 3.)
                                },
                            }),
                    ),
            )
            .child(
                box_()
                    .w(80.)
                    .h(50.)
                    .hit(AnchorHit::Flyout)
                    .anchored(Anchor::Target {
                        target: AnchorHit::FlyoutBtn,
                        place: |r, popup, _| {
                            let x = r.center().x - (popup.width / 2.).floor();
                            let above = r.top() - (4. + popup.height);
                            point(x, if above >= 32. { above } else { r.bottom() + 4. })
                        },
                    }),
            );

        let frame = Frame::layout(tree, AvailSize::definite(300., 200.), 1.0, &ts);
        assert_eq!(frame.rect_of(AnchorHit::Popup).unwrap().origin, point(120., 32.));
        assert_eq!(frame.rect_of(AnchorHit::Submenu).unwrap().origin, point(24., 49.));
        assert_eq!(frame.rect_of(AnchorHit::Flyout).unwrap().origin, point(115., 94.));

        for scale in [1.0f32, 1.5] {
            let short: Node<(), ()> = box_()
                .max_w(320.)
                .border(1., black())
                .px(9.)
                .pt(6.)
                .pb(8.)
                .text_size(12.)
                .line_height(16.)
                .child(text("New tab"));
            let short_sz = Frame::measure(short, AvailSize::MAX, scale, &ts);
            assert!(short_sz.width > 30. && short_sz.width < 150.);
            assert!((short_sz.height - 32.).abs() <= 1.0);

            let long: Node<(), ()> = box_()
                .max_w(320.)
                .border(1., black())
                .px(9.)
                .pt(6.)
                .pb(8.)
                .text_size(12.)
                .line_height(16.)
                .child(text(
                    "This is a significantly longer tooltip message that must wrap cleanly across multiple lines inside the 320px cap",
                ));
            let long_sz = Frame::measure(long, AvailSize::MAX, scale, &ts);
            assert!(long_sz.width <= 320.5);
            assert!(long_sz.height >= 48. - 1.0);
        }
    }

    #[test]
    fn cubic_bezier_matches_engine_published_values() {
        for (t, expected) in [
            (0.0, 0.0),
            (0.2, 0.295),
            (0.5, 0.802),
            (0.8, 0.976),
            (1.0, 1.0),
        ] {
            let actual = cubic_bezier(0.25, 0.1, 0.25, 1., t);
            assert!(
                (actual - expected).abs() < 1e-3,
                "ease({t}) = {actual}, expected {expected}"
            );
        }

        for (t, expected) in [
            (0.05, 0.01136),
            (0.1, 0.03978),
            (0.25, 0.18235),
            (0.5, 0.5),
            (0.75, 0.81764),
            (0.95, 0.98863),
        ] {
            let actual = cubic_bezier(0.25, 0., 0.75, 1., t);
            assert!(
                (actual - expected).abs() < 3e-4,
                "curve({t}) = {actual}, expected {expected}"
            );
        }
    }

    #[test]
    fn cubic_bezier_with_thirds_x_maps_time_identically() {
        for step in 0..=100 {
            let t = step as f32 / 100.;
            let one_t = 1. - t;
            let expected = 3. * 0.72 * one_t * one_t * t + 3. * one_t * t * t + t * t * t;
            let actual = cubic_bezier(1. / 3., 0.72, 2. / 3., 1., t);
            assert!(
                (actual - expected).abs() < 1e-4,
                "ease({t}) = {actual}, expected {expected}"
            );
        }
    }

    #[test]
    fn cubic_bezier_matches_the_css_definition() {
        fn css_reference(x1: f64, y1: f64, x2: f64, y2: f64, t: f64) -> f64 {
            let sample = |p1: f64, p2: f64, s: f64| {
                3. * p1 * (1. - s) * (1. - s) * s + 3. * p2 * (1. - s) * s * s + s * s * s
            };
            let (mut low, mut high) = (0f64, 1f64);
            for _ in 0..64 {
                let mid = (low + high) / 2.;
                if sample(x1, x2, mid) < t {
                    low = mid;
                } else {
                    high = mid;
                }
            }
            sample(y1, y2, (low + high) / 2.)
        }

        for (x1, y1, x2, y2) in [
            (0.25, 0.1, 0.25, 1.),
            (1. / 3., 0.72, 2. / 3., 1.),
            (0.42, 0., 1., 1.),
            (0., 0., 0.58, 1.),
            (0.42, 0., 0.58, 1.),
            (1., 0., 0., 1.),
            (0., 0., 0., 1.),
            (1., 0.5, 1., 0.5),
        ] {
            for step in 0..=1000 {
                let t = step as f32 / 1000.;
                let y = cubic_bezier(x1, y1, x2, y2, t);
                let expected =
                    css_reference(x1 as f64, y1 as f64, x2 as f64, y2 as f64, t as f64) as f32;
                assert!(
                    (y - expected).abs() < 5e-4,
                    "cubic_bezier({x1}, {y1}, {x2}, {y2})({t}) = {y}, CSS = {expected}"
                );
                assert!((0.0..=1.0).contains(&y), "ease({t}) = {y} out of range");
            }
        }
    }

    #[test]
    fn cubic_bezier_is_monotonic_and_clamped() {
        assert_eq!(cubic_bezier(0.32, 0.72, 0., 1., -1.), 0.);
        assert_eq!(cubic_bezier(0.32, 0.72, 0., 1., 2.), 1.);

        let mut previous = 0.;
        for step in 0..=100 {
            let current = cubic_bezier(0.32, 0.72, 0., 1., step as f32 / 100.);
            assert!(current >= previous - 1e-4, "not monotonic at {step}");
            previous = current;
        }
    }

    #[test]
    fn anim_tracks_prune_unused_and_reverse_proportionally() {
        let t0 = Instant::now();
        let dur = Duration::from_millis(100);
        let mut tracks: AnimTracks<u32> = AnimTracks::default();

        tracks.begin_frame(t0, false);
        let s0 = tracks.animate_f32_from(1, 0.0, 100.0, dur, Easing::Linear);
        assert_eq!(s0.status, AnimStatus::Running);
        assert_eq!(s0.value, 0.0);
        assert!(tracks.end_frame());
        assert_eq!(tracks.track_count(), 1);

        let t_mid = t0 + Duration::from_millis(50);
        tracks.begin_frame(t_mid, false);
        let s1 = tracks.animate_f32_from(1, 0.0, 0.0, dur, Easing::Linear);
        assert_eq!(s1.status, AnimStatus::Running);
        assert!((s1.value - 50.0).abs() < 1e-3);
        assert!(tracks.end_frame());

        let t_done = t_mid + Duration::from_millis(50);
        tracks.begin_frame(t_done, false);
        let s2 = tracks.animate_f32_from(1, 0.0, 0.0, dur, Easing::Linear);
        assert_eq!(s2.status, AnimStatus::Finished);
        assert_eq!(s2.value, 0.0);
        assert!(!tracks.end_frame());

        tracks.begin_frame(t_done, false);
        assert!(!tracks.end_frame());
        assert_eq!(tracks.track_count(), 0);
    }

    fn st() -> FlexStyle {
        FlexStyle::default()
    }

    fn row_st() -> FlexStyle {
        FlexStyle {
            row: true,
            ..Default::default()
        }
    }

    fn full() -> FlexStyle {
        st().w_full().h_full()
    }

    impl FlexStyle {
        fn w(mut self, v: f32) -> Self { self.size[0] = Dim::Px(v); self }
        fn h(mut self, v: f32) -> Self { self.size[1] = Dim::Px(v); self }
        fn w_full(mut self) -> Self { self.size[0] = Dim::Full; self }
        fn h_full(mut self) -> Self { self.size[1] = Dim::Full; self }
        fn min_w(mut self, v: f32) -> Self { self.min[0] = Dim::Px(v); self }
        fn min_h(mut self, v: f32) -> Self { self.min[1] = Dim::Px(v); self }
        fn min_h_full(mut self) -> Self { self.min[1] = Dim::Full; self }
        fn max_w_full(mut self) -> Self { self.max[0] = Dim::Full; self }
        fn flex_1(mut self) -> Self { self.flex = Flex::Fill; self }
        fn flex_none(mut self) -> Self { self.flex = Flex::Fixed; self }
        fn gap(mut self, v: f32) -> Self { self.gap = v; self }
        fn items_center(mut self) -> Self { self.items_center = true; self }
        fn justify_center(mut self) -> Self { self.justify_center = true; self }
        fn overflow_hidden(mut self) -> Self { self.clip = [true, true]; self }
        fn overflow_x_hidden(mut self) -> Self { self.clip[0] = true; self }
        fn pad_all(mut self, v: f32) -> Self { self.padding = Edges::all(v); self }
        fn pad_xy(mut self, x: f32, y: f32) -> Self { self.padding = Edges::xy(x, y); self }
        fn pad(mut self, t: f32, r: f32, b: f32, l: f32) -> Self {
            self.padding = Edges { top: t, right: r, bottom: b, left: l };
            self
        }
        fn margin(mut self, t: f32, l: f32) -> Self {
            self.margin = Edges { top: t, left: l, ..Edges::ZERO };
            self
        }
        fn border_all(mut self, v: f32) -> Self { self.border = Edges::all(v); self }
        fn border(mut self, t: f32, r: f32, b: f32, l: f32) -> Self {
            self.border = Edges { top: t, right: r, bottom: b, left: l };
            self
        }
        fn abs(mut self) -> Self { self.absolute = true; self }
        fn inset_tl(mut self, t: f32, l: f32) -> Self {
            self.inset = Edges { top: Some(t), left: Some(l), ..Edges::all(None) };
            self
        }
        fn inset(mut self, top: Option<f32>, right: Option<f32>, bottom: Option<f32>, left: Option<f32>) -> Self {
            self.inset = Edges { top, right, bottom, left };
            self
        }
    }

    struct Tree {
        engine: LayoutEngine,
        scale: f32,
    }

    impl Tree {
        fn new(scale: f32) -> Self {
            Self { engine: LayoutEngine::new(), scale }
        }
        fn leaf(&mut self, style: FlexStyle) -> LayoutId {
            self.engine.add_node(style, self.scale, &[], false)
        }
        fn text(&mut self) -> LayoutId {
            self.engine.add_node(st(), self.scale, &[], true)
        }
        fn node(&mut self, style: FlexStyle, children: &[LayoutId]) -> LayoutId {
            self.engine.add_node(style, self.scale, children, false)
        }
        fn compute(&mut self, root: LayoutId, avail: AvailSize) {
            self.engine.compute(root, avail, self.scale, &mut |_, _, _, _| Size::ZERO);
        }
        fn compute_with(
            &mut self,
            root: LayoutId,
            avail: AvailSize,
            mut measure: impl FnMut(LayoutId, Option<f32>, Option<f32>, AvailSize) -> Size,
        ) {
            self.engine.compute(root, avail, self.scale, &mut measure);
        }
        fn bounds(&mut self, id: LayoutId) -> Rect {
            self.engine.bounds(id, self.scale)
        }
    }

    #[test]
    fn flex_col_window_chrome_and_flex_1_editor() {
        let mut t = Tree::new(1.0);
        let title = t.leaf(st().flex_none().h(40.));
        let menu = t.leaf(st().flex_none().h(32.));
        let editor = t.leaf(st().flex_1().min_h(60.));
        let status = t.leaf(st().flex_none().h(33.));
        let root = t.node(full(), &[title, menu, editor, status]);
        t.compute(root, AvailSize::definite(800., 600.));

        assert_eq!(t.bounds(title), rect(0., 0., 800., 40.));
        assert_eq!(t.bounds(menu), rect(0., 40., 800., 32.));
        assert_eq!(t.bounds(editor), rect(0., 72., 800., 495.));
        assert_eq!(t.bounds(status), rect(0., 567., 800., 33.));
    }

    #[test]
    fn dialog_wrapped_text_and_equal_buttons() {
        let mut t = Tree::new(1.0);
        let body_text = t.text();
        let content = t.node(st().pad_all(24.).border(0., 0., 1., 0.), &[body_text]);
        let btn = st().flex_1().min_w(0.).h(32.);
        let (b1, b2) = (t.leaf(btn), t.leaf(btn));
        let footer = t.node(row_st().pad_all(24.).gap(8.), &[b1, b2]);
        let col = t.node(st(), &[content, footer]);
        let frame = t.node(st().w(360.).max_w_full().border_all(1.), &[col]);
        let overlay = t.node(full().abs().inset_tl(0., 0.).items_center().justify_center(), &[frame]);
        let root = t.node(full(), &[overlay]);
        t.compute_with(root, AvailSize::definite(800., 600.), |id, kw, _, avail| {
            assert_eq!(id, body_text);
            let wrap = kw.or(match avail.width {
                Space::Definite(w) => Some(w),
                _ => None,
            });
            match wrap {
                Some(w) if w < 500. => size(w, (500.0 / w).ceil().max(1.0) * 20.),
                _ => size(500., 20.),
            }
        });

        assert_eq!(t.bounds(body_text).size, size(310., 40.));
        assert_eq!(t.bounds(content).size, size(358., 89.));
        assert_eq!(t.bounds(b1).size, size(151., 32.));
        assert_eq!(t.bounds(b2).size, size(151., 32.));
        let fb = t.bounds(frame);
        assert_eq!(fb.size, size(360., 171.));
        assert_eq!(fb.origin.x, 220.);
    }

    #[test]
    fn tab_label_min_w_0_truncates_wide_nowrap_text() {
        let mut t = Tree::new(1.0);
        let long_text = t.text();
        let label_box = t.node(st().flex_1().min_w(0.).overflow_hidden(), &[long_text]);
        let tab = t.node(row_st().items_center().w(180.).h(32.).pad(0., 36., 0., 12.), &[label_box]);
        t.compute_with(tab, AvailSize::definite(180., 32.), |_, _, _, _| size(400., 16.));

        assert_eq!(t.bounds(label_box), rect(12., 8., 132., 16.));
    }

    #[test]
    fn text_field_negative_insets_and_absolute_glyph() {
        let mut t = Tree::new(1.0);
        let bottom_edge = t.leaf(st().abs().inset(None, Some(-1.), Some(0.), Some(-1.)).h(2.));
        let glyph = t.leaf(st().abs().inset(Some(2.), Some(4.), Some(2.), None).w(24.));
        let field = t.node(st().w(200.).h(32.).border(1., 1., 0., 1.).pad(0., 0., 1., 0.), &[bottom_edge, glyph]);
        t.compute(field, AvailSize::definite(200., 32.));

        assert_eq!(t.bounds(bottom_edge), rect(0., 30., 200., 2.));
        assert_eq!(t.bounds(glyph), rect(171., 3., 24., 27.));
    }

    #[test]
    fn status_bar_min_content_root_measurement() {
        let mut t = Tree::new(1.0);
        let (t1, t2, t3) = (t.text(), t.text(), t.text());
        let div1 = t.leaf(st().flex_none().w(1.).h(14.));
        let spacer = t.leaf(st().flex_1().min_w(1.).h(14.));
        let row = t.node(row_st().items_center().gap(24.).h(32.).pad_xy(22., 0.), &[t1, div1, t2, spacer, t3]);
        let sep = t.leaf(st().h(1.));
        let status = t.node(st().flex_none(), &[sep, row]);
        t.compute_with(status, AvailSize::MIN, |id, _, _, _| {
            if id == t1 { size(60., 16.) } else if id == t2 { size(80., 16.) } else { size(40., 16.) }
        });

        assert_eq!(t.bounds(status).size, size(322., 33.));
    }

    #[test]
    fn menu_command_tile_items_center_and_relative_offset() {
        let mut t = Tree::new(1.0);
        let (icon, label_text) = (t.text(), t.text());
        let icon_box = t.node(st().h(16.), &[icon]);
        let label_box = t.node(st().margin(4., 0.).w(48.).h(16.), &[label_text]);
        let tile = t.node(st().items_center().w(60.).h(50.).pad(8., 0., 0., 0.), &[icon_box, label_box]);
        let nudged = t.leaf(st().inset_tl(2., 1.).w(10.).h(10.));
        let root = t.node(row_st(), &[tile, nudged]);
        t.compute_with(root, AvailSize::definite(200., 100.), |id, _, _, _| {
            if id == icon { size(16., 16.) } else { size(24., 14.) }
        });

        assert_eq!(t.bounds(icon_box).origin, point(22., 8.));
        assert_eq!(t.bounds(label_box), rect(6., 28., 48., 16.));
        assert_eq!(t.bounds(label_text).size.width, 48.);
        assert_eq!(t.bounds(nudged).origin, point(61., 2.));
    }

    #[test]
    fn fractional_dpi_edge_closure_between_flex_siblings() {
        let mut t = Tree::new(1.5);
        let child = st().flex_1().h(20.);
        let (a, b, c) = (t.leaf(child), t.leaf(child), t.leaf(child));
        let root = t.node(row_st().w(100.).h(20.), &[a, b, c]);
        t.compute(root, AvailSize::definite(100., 20.));

        let (ba, bb, bc) = (t.bounds(a), t.bounds(b), t.bounds(c));
        assert_eq!(ba.right(), bb.left());
        assert_eq!(bb.right(), bc.left());
        assert_eq!(bc.right(), 100.);
    }

    #[test]
    fn editor_percentage_chain_through_flex_1() {
        let mut t = Tree::new(1.0);
        let text_el = t.leaf(full().min_h(40.));
        let scrollbar = t.leaf(full().abs());
        let editor_state = t.node(st().flex_1().h_full().pad(10., 12., 10., 14.), &[text_el, scrollbar]);
        let inner = t.node(full(), &[editor_state]);
        let editor_area = t.node(st().flex_1().min_h(60.), &[inner]);
        let header = t.leaf(st().flex_none().h(100.));
        let root = t.node(full(), &[header, editor_area]);
        t.compute(root, AvailSize::definite(800., 500.));

        assert_eq!(t.bounds(text_el), rect(14., 110., 774., 380.));
        assert_eq!(t.bounds(scrollbar).size, size(800., 400.));
    }

    #[test]
    fn flex_shrink_respects_flex_none_and_min_size() {
        let mut t = Tree::new(1.0);
        let fixed = t.leaf(st().flex_none().w(100.).h(20.));
        let a = t.leaf(st().w(200.).min_w(80.).h(20.));
        let b = t.leaf(st().w(200.).min_w(0.).h(20.));
        let root = t.node(row_st().w(220.), &[fixed, a, b]);
        t.compute(root, AvailSize::definite(220., 20.));

        assert_eq!(t.bounds(fixed).size.width, 100.);
        assert_eq!(t.bounds(a).size.width, 80.);
        assert_eq!(t.bounds(b).size.width, 40.);
    }

    #[test]
    fn abspos_dropdown_inside_narrow_button_does_not_squish() {
        let mut t = Tree::new(1.0);
        let item = t.text();
        let dropdown = t.node(st().abs().inset_tl(32., 0.), &[item]);
        let button = t.node(st().w(40.).h(32.), &[dropdown]);
        t.compute_with(button, AvailSize::definite(40., 32.), |_, _, _, _| size(180., 28.));

        assert_eq!(t.bounds(dropdown), rect(0., 32., 180., 28.));
    }

    #[test]
    fn auto_height_flex_col_does_not_crush_in_short_window_and_respects_max_w_full() {
        let mut t = Tree::new(1.0);
        let content = t.leaf(st().h(120.));
        let footer = t.leaf(st().h(80.));
        let col = t.node(st(), &[content, footer]);
        let frame = t.node(st().w(360.).max_w_full().border_all(1.), &[col]);
        let overlay = t.node(full().abs().inset_tl(0., 0.).items_center().justify_center(), &[frame]);
        let root = t.node(full(), &[overlay]);
        t.compute(root, AvailSize::definite(250., 140.));

        assert_eq!(t.bounds(frame).size, size(250., 202.));
        assert_eq!(t.bounds(content).size, size(248., 120.));
        assert_eq!(t.bounds(footer).size, size(248., 80.));
    }

    #[test]
    fn single_line_text_field_min_h_full_centers_text_element() {
        let mut t = Tree::new(1.0);
        let text_el = t.leaf(st().w_full().h(20.));
        let input_state = t.node(row_st().flex_1().min_h_full().items_center().overflow_x_hidden(), &[text_el]);
        let inner = t.node(row_st().h_full().items_center().pad(0., 33., 0., 9.), &[input_state]);
        let field = t.node(st().w(200.).h(32.).border(1., 1., 0., 1.).pad(0., 0., 1., 0.), &[inner]);
        t.compute(field, AvailSize::definite(200., 32.));

        assert_eq!(t.bounds(input_state), rect(10., 1., 156., 30.));
        assert_eq!(t.bounds(text_el), rect(10., 6., 156., 20.));
    }

    #[test]
    fn directwrite_status_bar_shapes_all_labels() {
        use crate::gfx::{LabelLayout, TextStyle, TextSystem};
        use crate::types::SharedStr;

        let ts = TextSystem::new().unwrap();
        let style = TextStyle::regular("Segoe UI Variable Small", 12.0);
        let labels = ["Ln 1, Col 1", "0 characters", "100%", "Windows (CRLF)", "UTF-8"];

        let build = |t: &mut Tree| {
            let (t0, t2, t4, t6, t8) = (t.text(), t.text(), t.text(), t.text(), t.text());
            let div = st().flex_none().w(1.).h(16.);
            let (d1, d5, d7) = (t.leaf(div), t.leaf(div), t.leaf(div));
            let sp = t.leaf(st().flex_1().min_w(1.).h(16.));
            let row = t.node(
                row_st().items_center().gap(24.).h(31.).pad_xy(22., 0.),
                &[t0, d1, t2, sp, t4, d5, t6, d7, t8],
            );
            let sep = t.leaf(st().flex_none().h(1.));
            let bar = t.node(st().flex_none(), &[sep, row]);
            (bar, [t0, t2, t4, t6, t8], sp, d5)
        };

        let measure_labels = |ids: [LayoutId; 5]| {
            let (ts, style) = (&ts, &style);
            move |id: LayoutId, kw: Option<f32>, _: Option<f32>, avail: AvailSize| {
                let ix = ids.iter().position(|&x| x == id).unwrap();
                let wrap_width = kw.or(match avail.width {
                    Space::Definite(x) => Some(x),
                    _ => None,
                });
                ts.shape_label(
                    SharedStr::Static(labels[ix]),
                    style,
                    LabelLayout { line_height: 16.0, wrap_width, line_clamp: None },
                )
                .size()
            }
        };

        let mut probe = Tree::new(1.0);
        let (bar_probe, probe_ids, _, _) = build(&mut probe);
        probe.compute_with(bar_probe, AvailSize::MIN, measure_labels(probe_ids));
        let min_sz = probe.bounds(bar_probe).size;

        let mut win = Tree::new(1.0);
        let (bar_win, win_ids, sp, d5) = build(&mut win);
        let root = win.node(full(), &[bar_win]);
        win.compute_with(root, AvailSize::definite(min_sz.width, 200.), measure_labels(win_ids));

        let (spacer, zoom, next_div) = (win.bounds(sp), win.bounds(win_ids[2]), win.bounds(d5));
        assert!(spacer.size.width >= 1.0);
        assert!(zoom.size.height <= 16.0, "zoom must stay single-line, got {zoom:?}");
        assert!(zoom.right() < next_div.left());
    }
}
