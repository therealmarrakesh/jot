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
        let mut tree = LayoutTree::default();
        let mut frame = Frame::default();
        let default_text = ResolvedTextStyle::default();

        let root_idx = frame.flatten(root_node, None, &default_text, scale, &mut tree);
        frame.root = Some(root_idx);

        tree.solve(root_idx, available, scale, &mut |idx, kw, _kh, avail| {
            let FlatKind::Text {
                content,
                style,
                label,
            } = &mut frame.nodes[idx].kind
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

        let root_rect = tree.rect(root_idx, scale);
        let viewport = match (available.width, available.height) {
            (Space::Definite(w), Space::Definite(h)) => size(w, h),
            _ => root_rect.size,
        };
        for idx in 0..frame.nodes.len() {
            let Some(anchor) = frame.nodes[idx].anchor else {
                continue;
            };
            let popup_size = tree.rect(idx, scale).size;
            let origin = anchor.resolve(viewport, popup_size, |target| {
                frame
                    .nodes
                    .iter()
                    .enumerate()
                    .rev()
                    .find(|(_, n)| n.interact.hit == Some(target))
                    .map(|(i, _)| tree.rect(i, scale))
            });
            tree.set_origin(idx, origin, scale);
        }

        for (idx, node) in frame.nodes.iter_mut().enumerate() {
            node.rect = tree.rect(idx, scale);
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
        tree: &mut LayoutTree,
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
        tree.add(flex, scale, is_text);

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
                let child_flat_ids: Vec<usize> = children
                    .into_iter()
                    .map(|child| self.flatten(child, Some(flat_idx), &resolved_text, scale, tree))
                    .collect();
                let start = self.child_order.len();
                self.child_order.extend_from_slice(&child_flat_ids);
                tree.set_children(flat_idx, &child_flat_ids);
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
    pub(crate) last_input_was_keyboard: bool,
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
    pub(crate) fn frame(&self) -> &Frame<H, C> {
        &self.frame
    }

    pub(crate) fn pointer(&self) -> Option<PointerState> {
        self.pointer
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

    pub(crate) fn top_hit(&self) -> Option<H> {
        self.hovered_chain.first().copied()
    }

    pub(crate) fn is_hovered(&self, hit: H) -> bool {
        self.hovered_chain.contains(&hit)
    }

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
    fn lerp(self, target: Self, t: f32) -> Self {
        self + (target - self) * t
    }
}

impl Lerp for Rgba {
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
        if !self.is_open() {
            self.serial = next_transition_serial();
            self.phase = Presence::Open;
        }
    }

    pub(crate) fn close(&mut self) -> bool {
        if !self.is_open() {
            return false;
        }
        self.phase = Presence::Closing;
        true
    }

    pub(crate) fn finish(&mut self) {
        self.phase = Presence::Hidden;
    }

    pub(crate) fn serial(self) -> usize {
        self.serial
    }

    pub(crate) fn is_visible(self) -> bool {
        self.phase != Presence::Hidden
    }

    pub(crate) fn is_open(self) -> bool {
        self.phase == Presence::Open
    }

    pub(crate) fn is_closing(self) -> bool {
        self.phase == Presence::Closing
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
        self.animate(|t| &mut t.scalars, key, initial, target, duration, easing)
    }

    pub(crate) fn animate_color(
        &mut self,
        key: K,
        target: Rgba,
        duration: Duration,
        easing: Easing,
    ) -> Rgba {
        self.animate(|t| &mut t.colors, key, target, target, duration, easing).value
    }

    fn animate<T: Lerp>(
        &mut self,
        tracks: impl FnOnce(&mut Self) -> &mut HashMap<K, AnimTrack<T>>,
        key: K,
        from: T,
        target: T,
        duration: Duration,
        easing: Easing,
    ) -> AnimSample<T> {
        let (frame, now, reduce_motion) = (self.frame, self.now, self.reduce_motion);
        let track = tracks(self).entry(key).or_insert_with(|| AnimTrack {
            from,
            target,
            started_at: now,
            reversing_scale: 1.0,
            duration,
            last_frame: frame,
        });
        let sample = track.sample_and_update(target, duration, easing, now, reduce_motion, frame);
        self.running_this_frame |= sample.status == AnimStatus::Running;
        sample
    }

    #[cfg(test)]
    pub(crate) fn track_count(&self) -> usize {
        self.scalars.len() + self.colors.len()
    }
}

pub(crate) use layout::{AvailSize, Dim, Flex, FlexStyle, LayoutTree, Space, snap_border_edges};

/// A flexbox subset solved in physical pixels. Axis 0 is horizontal, axis 1 vertical.
mod layout {
    use std::ops::Range;

    use crate::geom::{Edges, Point, Rect, Size, point, size};

    fn round_half_toward_zero(value: f32) -> f32 {
        (value.abs() - 0.5).ceil().copysign(value)
    }

    fn round_px(logical: f32, scale: f32) -> f32 {
        round_half_toward_zero(logical * scale)
    }

    fn round_stroke(logical: f32, scale: f32) -> f32 {
        if logical == 0.0 {
            0.0
        } else {
            round_px(logical.max(0.0), scale).max(1.0)
        }
    }

    pub(crate) fn snap_border_edges(border: Edges<f32>, scale: f32) -> Edges<f32> {
        let s = scale.max(0.5);
        border.map(|v| round_stroke(v, s) / s)
    }

    #[derive(Copy, Clone, Default, Debug, PartialEq)]
    pub(crate) enum Space {
        Definite(f32),
        #[default]
        MinContent,
        MaxContent,
    }

    impl Space {
        fn scale(self, factor: f32) -> Self {
            match self {
                Self::Definite(v) => Self::Definite(v * factor),
                other => other,
            }
        }

        fn maybe_sub(self, delta: f32) -> Self {
            match self {
                Self::Definite(v) => Self::Definite((v - delta).max(0.0)),
                other => other,
            }
        }

        fn maybe_min(self, limit: Option<f32>) -> Self {
            match (self, limit) {
                (Self::Definite(a), Some(m)) => Self::Definite(a.min(m)),
                (Self::MaxContent, Some(m)) => Self::Definite(m),
                _ => self,
            }
        }

        fn maybe_max(self, floor: Option<f32>) -> Self {
            match (self, floor) {
                (Self::Definite(a), Some(m)) => Self::Definite(a.max(m)),
                _ => self,
            }
        }

        fn maybe_set(self, known: Option<f32>) -> Self {
            known.map_or(self, Self::Definite)
        }

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

        pub(crate) const fn definite(width: f32, height: f32) -> Self {
            Self {
                width: Space::Definite(width),
                height: Space::Definite(height),
            }
        }
    }

    impl From<Size> for AvailSize {
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
        fn snap(self, scale: f32) -> Self {
            match self {
                Self::Px(v) => Self::Px(round_px(v, scale)),
                other => other,
            }
        }

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
        fn can_grow(self) -> bool {
            matches!(self, Self::Fill)
        }

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

    #[derive(Copy, Clone, PartialEq, Eq)]
    enum Pass {
        /// Final pass: sizes and positions are stored on the nodes.
        Layout,
        /// Outer size under the node's own size constraints.
        Size,
        /// Min-content size, ignoring the node's own size constraints.
        Content,
    }

    #[derive(Copy, Clone)]
    struct Req {
        known: [Option<f32>; 2],
        parent: [Option<f32>; 2],
        avail: [Space; 2],
        pass: Pass,
    }

    type CacheKey = (Pass, [u32; 2], [u32; 2], [(u8, u32); 2]);

    impl Req {
        fn key(&self) -> CacheKey {
            let bits = |v: [Option<f32>; 2]| v.map(|v| v.map_or(u32::MAX, f32::to_bits));
            (self.pass, bits(self.known), bits(self.parent), self.avail.map(Space::key))
        }
    }

    /// Resolved size constraints along one axis; `floor` also covers padding and border.
    #[derive(Copy, Clone)]
    struct Bounds {
        size: Option<f32>,
        min: Option<f32>,
        max: Option<f32>,
        floor: f32,
    }

    impl Bounds {
        #[inline]
        fn clamp(self, v: f32) -> f32 {
            clamp(v, Some(self.floor), self.max)
        }

        fn definite(self, known: Option<f32>) -> Option<f32> {
            known.or(self.size).map(|v| self.clamp(v))
        }
    }

    #[inline]
    fn clamp(v: f32, min: Option<f32>, max: Option<f32>) -> f32 {
        let v = max.map_or(v, |hi| v.min(hi));
        min.map_or(v, |lo| v.max(lo))
    }

    /// Places a `(main, cross)` pair on absolute axes.
    fn axes<T>(main: usize, along_main: T, along_cross: T) -> [T; 2] {
        if main == 0 { [along_main, along_cross] } else { [along_cross, along_main] }
    }

    impl FlexStyle {
        fn snap_for_scale(self, scale: f32) -> Self {
            let px = |v: f32| round_px(v, scale);
            Self {
                inset: self.inset.map(|v| v.map(px)),
                size: self.size.map(|d| d.snap(scale)),
                min: self.min.map(|d| d.snap(scale)),
                max: self.max.map(|d| d.snap(scale)),
                margin: self.margin.map(px),
                padding: self.padding.map(px),
                border: self.border.map(|v| round_stroke(v, scale)),
                gap: round_px(self.gap, scale),
                ..self
            }
        }

        fn pb(&self, axis: usize) -> f32 {
            self.padding.add(self.border).along(axis)
        }

        fn bounds(&self, axis: usize, base: Option<f32>, inherent: bool) -> Bounds {
            let resolve = |d: Dim| if inherent { d.resolve(base) } else { None };
            let (min, pb) = (resolve(self.min[axis]), self.pb(axis));
            Bounds {
                size: resolve(self.size[axis]),
                min,
                max: resolve(self.max[axis]),
                floor: min.map_or(pb, |m| m.max(pb)),
            }
        }

        /// Cross size of a flex item: its own size, or the line stretched to fit.
        fn cross_size(&self, c: usize, line: Option<f32>, centered: bool, stretch: bool) -> Option<f32> {
            let b = self.bounds(c, line, true);
            match (b.size, line) {
                (Some(s), _) => Some(b.clamp(s)),
                (None, Some(l)) if stretch && !centered => {
                    Some(b.clamp((l - self.margin.along(c)).max(0.0)))
                }
                _ => None,
            }
        }
    }

    struct Node {
        style: FlexStyle,
        parent: Option<usize>,
        kids: Range<usize>,
        measured: bool,
        pos: Point,
        size: Size,
        cache: [Option<(CacheKey, Size)>; 8],
        next: usize,
    }

    /// Layout tree; node ids are insertion order.
    #[derive(Default)]
    pub(crate) struct LayoutTree {
        nodes: Vec<Node>,
        kids: Vec<usize>,
    }

    impl LayoutTree {
        pub(crate) fn add(&mut self, style: FlexStyle, scale: f32, measured: bool) -> usize {
            self.nodes.push(Node {
                style: style.snap_for_scale(scale),
                parent: None,
                kids: 0..0,
                measured,
                pos: Point::ZERO,
                size: Size::ZERO,
                cache: Default::default(),
                next: 0,
            });
            self.nodes.len() - 1
        }

        pub(crate) fn set_children(&mut self, id: usize, kids: &[usize]) {
            let start = self.kids.len();
            self.kids.extend_from_slice(kids);
            for &k in kids {
                self.nodes[k].parent = Some(id);
            }
            self.nodes[id].kids = start..self.kids.len();
        }

        /// Lays out the tree under `root`; `measure` sizes text leaves in logical units.
        pub(crate) fn solve(
            &mut self,
            root: usize,
            avail: AvailSize,
            scale: f32,
            measure: &mut impl FnMut(usize, Option<f32>, Option<f32>, AvailSize) -> Size,
        ) {
            let avail = [avail.width, avail.height].map(|s| s.scale(scale));
            let parent = avail.map(|s| match s {
                Space::Definite(v) => Some(v),
                _ => None,
            });
            let req = Req { known: [None; 2], parent, avail, pass: Pass::Layout };
            Solver { nodes: &mut self.nodes, kids: &self.kids, scale, measure }.node(root, req);
            self.nodes[root].pos = Point::ZERO;
        }

        fn origin(&self, id: usize) -> Point {
            let n = &self.nodes[id];
            n.parent.map_or(n.pos, |p| self.origin(p) + n.pos)
        }

        /// Pixel-snapped bounds in logical units.
        pub(crate) fn rect(&self, id: usize, scale: f32) -> Rect {
            let (o, s) = (self.origin(id), self.nodes[id].size);
            let snap = |x: f32, y: f32| point(round_half_toward_zero(x), round_half_toward_zero(y));
            let px = Rect::from_corners(snap(o.x, o.y), snap(o.x + s.width, o.y + s.height));
            Rect::new(
                point(px.origin.x / scale, px.origin.y / scale),
                size(px.size.width / scale, px.size.height / scale),
            )
        }

        /// Moves `id` so its top-left lands on `logical`.
        pub(crate) fn set_origin(&mut self, id: usize, logical: Point, scale: f32) {
            let parent = self.nodes[id].parent.map_or(Point::ZERO, |p| self.origin(p));
            self.nodes[id].pos = point(round_px(logical.x, scale), round_px(logical.y, scale)) - parent;
        }
    }

    struct Item {
        id: usize,
        style: FlexStyle,
        basis: f32,
        min: f32,
        max: Option<f32>,
        hypothetical: f32,
        target: f32,
        violation: f32,
        frozen: bool,
    }

    struct Solver<'a, F> {
        nodes: &'a mut [Node],
        kids: &'a [usize],
        scale: f32,
        measure: &'a mut F,
    }

    impl<'a, F: FnMut(usize, Option<f32>, Option<f32>, AvailSize) -> Size> Solver<'a, F> {
        fn node(&mut self, id: usize, req: Req) -> Size {
            if req.pass == Pass::Layout {
                let s = self.solve(id, req);
                self.nodes[id].size = s;
                return s;
            }
            let key = req.key();
            if let Some(&(_, s)) = self.nodes[id].cache.iter().flatten().find(|(k, _)| *k == key) {
                return s;
            }
            let s = self.solve(id, req);
            let n = &mut self.nodes[id];
            n.cache[n.next] = Some((key, s));
            n.next = (n.next + 1) % n.cache.len();
            s
        }

        fn solve(&mut self, id: usize, req: Req) -> Size {
            let n = &self.nodes[id];
            if n.kids.is_empty() || n.measured { self.leaf(id, req) } else { self.flex(id, req) }
        }

        fn kids_of(&self, id: usize) -> &'a [usize] {
            &self.kids[self.nodes[id].kids.clone()]
        }

        fn leaf(&mut self, id: usize, req: Req) -> Size {
            let (style, measured) = (self.nodes[id].style, self.nodes[id].measured);
            let b = [0, 1].map(|a| style.bounds(a, req.parent[a], req.pass != Pass::Content));
            let pb = [0, 1].map(|a| style.pb(a));
            let known = [0, 1].map(|a| b[a].definite(req.known[a]));
            let content = (measured && (known.contains(&None) || req.pass == Pass::Layout)).then(|| {
                let inv = 1.0 / self.scale;
                let [kw, kh] = [0, 1].map(|a| known[a].map(|v| (v - pb[a]).max(0.0) * inv));
                let [width, height] = [0, 1].map(|a| {
                    let space = req.avail[a].maybe_set(known[a]).maybe_min(b[a].max);
                    space.maybe_max(b[a].min).maybe_sub(pb[a]).scale(inv)
                });
                let s = (self.measure)(id, kw, kh, AvailSize { width, height });
                [s.width, s.height].map(|v| (v.max(0.0) * self.scale).ceil())
            });
            let [w, h] = [0, 1].map(|a| {
                known[a].unwrap_or_else(|| b[a].clamp(content.map_or(pb[a], |c| c[a] + pb[a])))
            });
            size(w, h)
        }

        fn flex(&mut self, id: usize, req: Req) -> Size {
            let style = self.nodes[id].style;
            let (m, c) = if style.row { (0, 1) } else { (1, 0) };
            let b = [0, 1].map(|a| style.bounds(a, req.parent[a], req.pass != Pass::Content));
            let pb = [0, 1].map(|a| style.pb(a));
            let outer = [0, 1].map(|a| b[a].definite(req.known[a]));
            let kids = self.kids_of(id);
            if kids.iter().all(|&k| self.nodes[k].style.absolute) {
                let [w, h] = [0, 1].map(|a| outer[a].unwrap_or_else(|| b[a].clamp(pb[a])));
                if req.pass == Pass::Layout {
                    self.place_absolute(kids, size(w, h), &style);
                }
                return size(w, h);
            }

            let inner = [0, 1].map(|a| outer[a].map(|v| (v - pb[a]).max(0.0)));
            let avail = [0, 1].map(|a| {
                req.avail[a].maybe_set(outer[a]).maybe_min(b[a].max).maybe_sub(pb[a])
            });
            let mut items = self.items(kids, &style, m, inner, avail);
            let gaps = style.gap * items.len().saturating_sub(1) as f32;
            let hypothetical =
                gaps + items.iter().map(|it| it.hypothetical + it.style.margin.along(m)).sum::<f32>();
            let (main, flexible) = match outer[m] {
                Some(v) => (v, true),
                None => {
                    let raw = hypothetical + pb[m];
                    let v = b[m].clamp(raw);
                    (v, (v - raw).abs() > 1e-4)
                }
            };
            let inner_main = (main - pb[m]).max(0.0);
            if flexible {
                resolve_lengths(&mut items, m, inner_main, gaps, hypothetical < inner_main);
            }

            let cross = match outer[c] {
                Some(v) => v,
                None => {
                    let parent = axes(m, Some(inner_main), inner[c]);
                    let mut tallest = 0.0f32;
                    for it in &items {
                        let margin = it.style.margin.along(c);
                        let cross = it.style.cross_size(c, inner[c], style.items_center, false);
                        let req = Req {
                            known: axes(m, Some(it.target), cross),
                            parent,
                            avail: axes(m, Space::Definite(it.target), avail[c].maybe_sub(margin)),
                            pass: Pass::Size,
                        };
                        tallest = tallest.max(self.node(it.id, req).along(c) + margin);
                    }
                    b[c].clamp(tallest + pb[c])
                }
            };
            let [w, h] = axes(m, main, cross);
            if req.pass == Pass::Layout {
                self.place_items(&items, &style, m, inner_main, (cross - pb[c]).max(0.0), gaps);
                self.place_absolute(kids, size(w, h), &style);
            }
            size(w, h)
        }

        /// Flex basis, min/max and hypothetical main size of every in-flow child.
        fn items(
            &mut self,
            kids: &[usize],
            style: &FlexStyle,
            m: usize,
            inner: [Option<f32>; 2],
            avail: [Space; 2],
        ) -> Vec<Item> {
            let c = 1 - m;
            let mut items = Vec::with_capacity(kids.len());
            for &id in kids {
                let cs = self.nodes[id].style;
                if cs.absolute {
                    continue;
                }
                let [own, lo, hi] = [cs.size[m], cs.min[m], cs.max[m]].map(|d| d.resolve(inner[m]));
                let cross = cs.cross_size(c, inner[c], style.items_center, true);
                let probe = |space, pass| Req {
                    known: axes(m, None, cross),
                    parent: inner,
                    avail: axes(m, space, avail[c].maybe_sub(cs.margin.along(c))),
                    pass,
                };
                let explicit = if cs.flex == Flex::Fill { inner[m].map(|_| 0.0) } else { own };
                let (basis, from_content) = match explicit {
                    Some(v) => (v, false),
                    None => {
                        let space = if avail[m] == Space::MinContent {
                            Space::MinContent
                        } else {
                            Space::MaxContent
                        };
                        (self.node(id, probe(space, Pass::Size)).along(m), true)
                    }
                };
                let (pb, rigid) = (cs.pb(m), !cs.flex.can_shrink());
                let min = match lo {
                    Some(v) => v.max(pb),
                    None if cs.clip[m]
                        || hi == Some(0.0)
                        || own.is_some_and(|s| s == 0.0 || (rigid && basis >= s))
                        || (rigid && from_content) =>
                    {
                        pb
                    }
                    None => {
                        let content = self.node(id, probe(Space::MinContent, Pass::Content)).along(m);
                        [own, hi].into_iter().flatten().fold(content, f32::min).max(pb)
                    }
                };
                let max = hi.map(|v| v.max(min));
                let hypothetical = clamp(basis, Some(min), max);
                items.push(Item {
                    id,
                    style: cs,
                    basis,
                    min,
                    max,
                    hypothetical,
                    target: hypothetical,
                    violation: 0.0,
                    frozen: false,
                });
            }
            items
        }

        /// Final pass over in-flow children: size each one and place it along the line.
        fn place_items(
            &mut self,
            items: &[Item],
            style: &FlexStyle,
            m: usize,
            inner_main: f32,
            inner_cross: f32,
            gaps: f32,
        ) {
            let c = 1 - m;
            let parent = axes(m, Some(inner_main), Some(inner_cross));
            let mut used = gaps;
            let sizes: Vec<Size> = items
                .iter()
                .map(|it| {
                    let margin = it.style.margin;
                    let cross = it.style.cross_size(c, Some(inner_cross), style.items_center, true);
                    let line = Space::Definite((inner_cross - margin.along(c)).max(0.0));
                    let req = Req {
                        known: axes(m, Some(it.target), cross),
                        parent,
                        avail: axes(m, Space::Definite(it.target), line),
                        pass: Pass::Layout,
                    };
                    let s = self.node(it.id, req);
                    used += s.along(m) + margin.along(m);
                    s
                })
                .collect();

            let edge = style.padding.add(style.border);
            let offset = if style.justify_center { (inner_main - used) / 2.0 } else { 0.0 };
            let mut cursor = edge.start(m) + offset;
            for (it, s) in items.iter().zip(sizes) {
                let (margin, inset) = (it.style.margin, it.style.inset);
                let cross_pos = if style.items_center {
                    let slack = (inner_cross - (s.along(c) + margin.along(c))) / 2.0;
                    edge.start(c) + slack + margin.start(c)
                } else {
                    edge.start(c) + margin.start(c)
                };
                let [x, y] = axes(m, cursor + margin.start(m), cross_pos);
                let shift = |a: usize| inset.start(a).or_else(|| inset.end(a).map(|e| -e)).unwrap_or(0.0);
                self.nodes[it.id].pos = point(x + shift(0), y + shift(1));
                cursor += margin.start(m) + s.along(m) + margin.end(m) + style.gap;
            }
        }

        /// Absolutely positioned children, placed against the padding box of `outer`.
        fn place_absolute(&mut self, kids: &[usize], outer: Size, style: &FlexStyle) {
            let (border, padding) = (style.border, style.padding);
            let main = if style.row { 0 } else { 1 };
            let block = [0, 1].map(|a| (outer.along(a) - border.along(a)).max(0.0));
            let content = [0, 1].map(|a| (block[a] - padding.along(a)).max(0.0));
            for &id in kids {
                let cs = self.nodes[id].style;
                if !cs.absolute {
                    continue;
                }
                let (inset, margin) = (cs.inset, cs.margin);
                let b = [0, 1].map(|a| cs.bounds(a, Some(block[a]), true));
                let known = [0, 1].map(|a| {
                    b[a].definite(None).or_else(|| match (inset.start(a), inset.end(a)) {
                        (Some(s), Some(e)) => Some(b[a].clamp((block[a] - s - e - margin.along(a)).max(0.0))),
                        _ => None,
                    })
                });
                let avail = [0, 1].map(|a| known[a].or(b[a].max).map_or(Space::MaxContent, Space::Definite));
                let req = Req { known, parent: block.map(Some), avail, pass: Pass::Layout };
                let s = self.node(id, req);
                let center = |a: usize, on: bool| {
                    if on { (content[a] - (s.along(a) + margin.along(a))) / 2.0 } else { 0.0 }
                };
                let offset = axes(main, center(main, style.justify_center), center(1 - main, style.items_center));
                let [x, y] = [0, 1].map(|a| match (inset.start(a), inset.end(a)) {
                    (Some(st), _) => border.start(a) + st + margin.start(a),
                    (None, Some(e)) => border.start(a) + block[a] - e - margin.end(a) - s.along(a),
                    (None, None) => border.start(a) + padding.start(a) + offset[a] + margin.start(a),
                });
                self.nodes[id].pos = point(x, y);
            }
        }
    }

    /// CSS flexible-length resolution: grow or shrink unfrozen items, freezing those that hit
    /// their min/max, until the line fits.
    fn resolve_lengths(items: &mut [Item], m: usize, inner: f32, gaps: f32, growing: bool) {
        for it in items.iter_mut() {
            it.frozen = if growing {
                !it.style.flex.can_grow() || it.basis > it.hypothetical
            } else {
                !it.style.flex.can_shrink() || it.basis < it.hypothetical
            };
            it.target = if it.frozen { it.hypothetical } else { it.basis };
        }
        loop {
            let mut unfrozen = 0usize;
            let mut used = gaps;
            for it in items.iter() {
                let base = if it.frozen {
                    it.target
                } else {
                    unfrozen += 1;
                    it.basis
                };
                used += base + it.style.margin.along(m);
            }
            if unfrozen == 0 {
                break;
            }
            let free = inner - used;
            if growing && free > 0.0 {
                let share = free / unfrozen as f32;
                for it in items.iter_mut().filter(|it| !it.frozen) {
                    it.target = it.basis + share;
                }
            } else if !growing && free < 0.0 {
                // Shrink in proportion to the size above padding and border. Items with none
                // already sit at their minimum, so an all-zero line has nothing to shrink.
                let scaled = |it: &Item| (it.basis - it.style.pb(m)).max(0.0);
                let total: f32 = items.iter().filter(|it| !it.frozen).map(scaled).sum();
                if total > 0.0 {
                    for it in items.iter_mut().filter(|it| !it.frozen) {
                        it.target = it.basis + free * (scaled(it) / total);
                    }
                }
            }
            let mut violation = 0.0f32;
            for it in items.iter_mut() {
                if it.frozen {
                    it.violation = 0.0;
                } else {
                    let clamped = clamp(it.target, Some(it.min), it.max);
                    it.violation = clamped - it.target;
                    it.target = clamped;
                    violation += it.violation;
                }
            }
            if violation.abs() < 1e-4 {
                break;
            }
            let mut froze = false;
            for it in items.iter_mut().filter(|it| !it.frozen) {
                if (violation > 0.0 && it.violation > 0.0) || (violation < 0.0 && it.violation < 0.0) {
                    it.frozen = true;
                    froze = true;
                }
            }
            if !froze {
                break;
            }
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
        tree: LayoutTree,
        scale: f32,
    }

    impl Tree {
        fn new(scale: f32) -> Self {
            Self { tree: LayoutTree::default(), scale }
        }
        fn leaf(&mut self, style: FlexStyle) -> usize {
            self.tree.add(style, self.scale, false)
        }
        fn text(&mut self) -> usize {
            self.tree.add(st(), self.scale, true)
        }
        fn node(&mut self, style: FlexStyle, children: &[usize]) -> usize {
            let id = self.tree.add(style, self.scale, false);
            self.tree.set_children(id, children);
            id
        }
        fn compute(&mut self, root: usize, avail: AvailSize) {
            self.tree.solve(root, avail, self.scale, &mut |_, _, _, _| Size::ZERO);
        }
        fn compute_with(
            &mut self,
            root: usize,
            avail: AvailSize,
            mut measure: impl FnMut(usize, Option<f32>, Option<f32>, AvailSize) -> Size,
        ) {
            self.tree.solve(root, avail, self.scale, &mut measure);
        }
        fn bounds(&mut self, id: usize) -> Rect {
            self.tree.rect(id, self.scale)
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

        let measure_labels = |ids: [usize; 5]| {
            let (ts, style) = (&ts, &style);
            move |id: usize, kw: Option<f32>, _: Option<f32>, avail: AvailSize| {
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
