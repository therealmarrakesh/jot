mod anim;
mod layout;

use std::hash::Hash;
use std::rc::Rc;
use std::time::{Duration, Instant};

use anim::AnimSample;
use layout::{Dim, Flex, FlexStyle, LayoutEngine, LayoutId};

use crate::color::{Rgba, black, transparent_black};
use crate::geom::{
    Corners, Edges, Point, Rect, ScreenPoint, ScreenRect, Shadows, Size, point, size,
};
use crate::gfx::{
    Canvas, FontWeight, Gpu, Label, LabelLayout, PaintQuad, Scope, TextStyle, TextSystem,
    WindowSurface,
};
use crate::platform::{CursorStyle, MouseButton, NativeWindow, WindowControlArea, WindowId};
use crate::types::SharedStr;

pub(crate) use anim::{AnimStatus, AnimTracks, Curve, Easing, Transition};
pub(crate) use layout::{AvailSize, Space, snap_border_edges};

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
            (Space::Definite(w), Space::Definite(h)) => Size::new(w, h),
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

#[cfg(test)]
mod tests {
    use crate::color::{rgb, white};
    use crate::geom::point;

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
            Some(SharedStr::new_static("Close tab"))
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
}
