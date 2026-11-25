//! The retained UI. Renderers build a tree of `AppNode`s each frame; `AppFrame::layout` lays it
//! out and keeps it, in tree order, for painting and for pointer queries. `AppUi` is a window's
//! pointer, hover and press state over its last frame, with the animations its widgets run.
//! The hit, drawing and animation ids the whole app uses are at the end.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crate::geom::{
    Axis, Corners, Edges, Point, Rect, Rgba, Shadows, Size, black, point, size, transparent_black,
};
use crate::gfx::{Canvas, FontWeight, Label, LabelLayout, PaintQuad, Scope, TextStyle, TextSystem};
use crate::layout::{AvailSize, Dim, Flex, FlexStyle, LayoutTree, Space, snap_border_edges};
use crate::platform::{CursorStyle, MouseButton, WindowControlArea};
use crate::types::SharedStr;

// ---- 1. Building nodes ----------------------------------------------------------------------

/// Text properties a node sets for itself and its descendants; unset ones are inherited.
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

/// The text properties in effect at a node.
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
        if let Some(family) = &o.family {
            out.style.font.family = family.clone();
        }
        if let Some(weight) = o.weight {
            out.style.font.weight = weight;
        }
        if let Some(size) = o.size {
            out.style.set_size(size);
        }
        out.line_height = o.line_height.unwrap_or(out.line_height);
        out.color = o.color.unwrap_or(out.color);
        out.nowrap = o.nowrap.unwrap_or(out.nowrap);
        out.line_clamp = o.line_clamp.or(out.line_clamp);
        out
    }
}

/// How a node is drawn transformed: scaled about its centre, or revealed from the top.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub(crate) enum Transform {
    #[default]
    None,
    Scale(f32, f32),
    /// Grows from `collapsed` pixels (but at least `min_start` of its size) to full size as `t`
    /// goes from 0 to 1.
    ScaleFromPx {
        collapsed: f32,
        min_start: f32,
        t: f32,
    },
    /// Shows the top `shown_ratio` of the node, sliding its content down with it if `slide`.
    RevealY {
        shown_ratio: f32,
        slide: bool,
    },
}

impl Transform {
    /// The scale this applies to a node of size `sz`, if it scales it at all.
    fn scale(self, sz: Size) -> Option<(f32, f32)> {
        let scale = match self {
            Self::Scale(sx, sy) => (sx, sy),
            Self::ScaleFromPx {
                collapsed,
                min_start,
                t,
            } => {
                let axis = |dim: f32| {
                    let start = (collapsed / dim.max(collapsed)).min(min_start);
                    start + (1.0 - start) * t
                };
                (axis(sz.width), axis(sz.height))
            }
            Self::None | Self::RevealY { .. } => return None,
        };
        Some(scale).filter(|(sx, sy)| (sx - 1.0).abs() > 1e-4 || (sy - 1.0).abs() > 1e-4)
    }
}

/// How a node looks, apart from its layout.
#[derive(Copy, Clone)]
struct Decor {
    bg: Rgba,
    border_color: Rgba,
    corners: Corners,
    shadows: Shadows,
    opacity: f32,
    transform: Transform,
    /// Takes no pointer input, nor does anything inside it.
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

/// What a node means to the pointer and to Windows.
#[derive(Default)]
struct Interact {
    hit: Option<HitId>,
    cursor: Option<CursorStyle>,
    window_area: Option<WindowControlArea>,
    tooltip: Option<SharedStr>,
}

/// Where an anchored node goes once everything is laid out: at a point, or placed against the
/// node with hit `target`. Either way it's kept inside the viewport.
#[derive(Copy, Clone)]
pub(crate) enum Anchor {
    Point(Point),
    Target {
        target: HitId,
        place: fn(target: Rect, popup: Size, viewport: Size) -> Point,
    },
}

impl Anchor {
    fn resolve(
        self,
        viewport: Size,
        popup: Size,
        target_rect: impl FnOnce(HitId) -> Option<Rect>,
    ) -> Point {
        let at = match self {
            Self::Point(p) => p,
            Self::Target { target, place } => match target_rect(target) {
                Some(rect) => place(rect, popup, viewport),
                None => return Point::ZERO,
            },
        };
        point(
            at.x.min((viewport.width - popup.width).max(0.0)).max(0.0),
            at.y.min((viewport.height - popup.height).max(0.0)).max(0.0),
        )
    }
}

enum Payload {
    Box(Vec<AppNode>),
    Text(SharedStr),
    Custom(CustomDraw),
}

/// A node of the tree a renderer builds: a box of children, a text, or something the app draws
/// itself, with its layout, look and pointer meaning set through the builder methods.
pub(crate) struct AppNode {
    flex: FlexStyle,
    decor: Decor,
    interact: Interact,
    anchor: Option<Anchor>,
    text: TextOverrides,
    payload: Payload,
}

macro_rules! setters {
    ($($name:ident ($($arg:ident : $ty:ty),*) => |$s:ident| $body:block)*) => {
        $(
            pub(crate) fn $name(mut self $(, $arg: $ty)*) -> Self {
                let $s = &mut self;
                $body
                self
            }
        )*
    };
}

impl AppNode {
    fn new(row: bool, payload: Payload) -> Self {
        Self {
            flex: FlexStyle {
                row,
                ..Default::default()
            },
            decor: Decor::default(),
            interact: Interact::default(),
            anchor: None,
            text: TextOverrides::default(),
            payload,
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
        anchored(anchor: Anchor) => |s| { s.anchor = Some(anchor); }
        font_semibold() => |s| { s.text.weight = Some(FontWeight::Semibold); }
        text_size(size: f32) => |s| { s.text.size = Some(size); }
        line_height(lh: f32) => |s| { s.text.line_height = Some(lh); }
        text_color(color: Rgba) => |s| { s.text.color = Some(color); }
        nowrap() => |s| { s.text.nowrap = Some(true); }
        line_clamp(lines: usize) => |s| { s.text.line_clamp = Some(lines); }
        hit(id: HitId) => |s| { s.interact.hit = Some(id); }
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
    /// Adds a child (a text or custom node has none, and ignores it).
    pub(crate) fn child(mut self, child: AppNode) -> Self {
        if let Payload::Box(list) = &mut self.payload {
            list.push(child);
        }
        self
    }
    pub(crate) fn children(mut self, children: impl IntoIterator<Item = AppNode>) -> Self {
        if let Payload::Box(list) = &mut self.payload {
            list.extend(children);
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

/// A box laying its children out in a row.
pub(crate) fn row() -> AppNode {
    AppNode::new(true, Payload::Box(Vec::new()))
}

/// A box laying its children out in a column.
pub(crate) fn box_() -> AppNode {
    AppNode::new(false, Payload::Box(Vec::new()))
}

pub(crate) fn text(content: impl Into<SharedStr>) -> AppNode {
    AppNode::new(false, Payload::Text(content.into()))
}

/// A node the app draws itself (`AppFrame::paint` hands it back with its rect).
pub(crate) fn custom(token: CustomDraw) -> AppNode {
    AppNode::new(false, Payload::Custom(token))
}

// ---- 2. The laid-out frame ------------------------------------------------------------------

/// What a laid-out node holds: children (the nodes after it in its subtree), a shaped text, or
/// something the app draws itself.
enum Content {
    Box,
    Text {
        content: SharedStr,
        style: ResolvedTextStyle,
        label: Option<Label>,
    },
    Custom(CustomDraw),
}

/// A laid-out node. Nodes are kept in tree order: a node's subtree is the nodes after it, up to
/// `end`.
struct FrameNode {
    parent: Option<usize>,
    end: usize,
    rect: Rect,
    border: Edges<f32>,
    clip: bool,
    decor: Decor,
    interact: Interact,
    content: Content,
}

/// A laid-out tree, kept for painting and for pointer queries until the next frame.
#[derive(Default)]
pub(crate) struct AppFrame {
    nodes: Vec<FrameNode>,
    /// The nodes that take the pointer, in tree order, each with where it does: its rect within
    /// its ancestors' clips. Nodes inert or scaling in or out, and all inside them, take none.
    hits: Vec<(usize, Rect)>,
    /// What Windows should treat each part of the window as, in tree order.
    pub(crate) control_areas: Vec<(Rect, WindowControlArea)>,
}

impl AppFrame {
    /// Lays `root` out in `available` space at `scale`, shaping its texts.
    pub(crate) fn layout(
        root: AppNode,
        available: AvailSize,
        scale: f32,
        text_system: &TextSystem,
    ) -> Self {
        let mut frame = Self::default();
        let mut tree = LayoutTree::new(scale);
        let mut anchors = Vec::new();
        frame.add(
            root,
            None,
            &ResolvedTextStyle::default(),
            &mut tree,
            &mut anchors,
        );

        tree.solve(0, available, &mut |idx, known_width, _, avail| {
            let Content::Text {
                content,
                style,
                label,
            } = &mut frame.nodes[idx].content
            else {
                return Size::ZERO;
            };
            let wrap_width = match avail.width {
                _ if style.nowrap => None,
                Space::Definite(w) => known_width.or(Some(w)),
                _ => known_width,
            };
            let layout = LabelLayout {
                line_height: style.line_height,
                wrap_width,
                line_clamp: style.line_clamp,
            };
            let shaped = text_system.shape_label(content.clone(), &style.style, layout);
            let size = shaped.size();
            *label = Some(shaped);
            size
        });

        let viewport = match (available.width, available.height) {
            (Space::Definite(w), Space::Definite(h)) => size(w, h),
            _ => tree.rect(0).size,
        };
        // Anchored nodes go where they're anchored, in tree order: a later one can be anchored
        // to an earlier one where it landed.
        for (idx, anchor) in anchors {
            let popup = tree.rect(idx).size;
            let origin = anchor.resolve(viewport, popup, |target| {
                let target = frame
                    .nodes
                    .iter()
                    .rposition(|n| n.interact.hit == Some(target));
                target.map(|t| tree.rect(t))
            });
            tree.set_origin(idx, origin);
        }
        for (idx, node) in frame.nodes.iter_mut().enumerate() {
            node.rect = tree.rect(idx);
        }
        frame.place_hits(Rect::new(Point::ZERO, viewport));
        frame
    }

    /// The size `root` takes laid out in `available` space.
    pub(crate) fn measure(
        root: AppNode,
        available: AvailSize,
        scale: f32,
        text_system: &TextSystem,
    ) -> Size {
        let frame = Self::layout(root, available, scale, text_system);
        frame
            .nodes
            .first()
            .map_or(Size::ZERO, |root| root.rect.size)
    }

    /// Adds `node` and its subtree, in tree order, to the frame and to the layout tree.
    fn add(
        &mut self,
        node: AppNode,
        parent: Option<usize>,
        inherited: &ResolvedTextStyle,
        tree: &mut LayoutTree,
        anchors: &mut Vec<(usize, Anchor)>,
    ) -> usize {
        let idx = self.nodes.len();
        let style = inherited.merge(&node.text);
        let AppNode {
            mut flex,
            decor,
            interact,
            anchor,
            payload,
            ..
        } = node;
        if let Some(anchor) = anchor {
            flex.absolute = true;
            anchors.push((idx, anchor));
        }
        self.nodes.push(FrameNode {
            parent,
            end: idx + 1,
            rect: Rect::ZERO,
            border: snap_border_edges(flex.border, tree.scale),
            clip: flex.clip[0] || flex.clip[1],
            decor,
            interact,
            content: Content::Box,
        });
        tree.add(flex, matches!(payload, Payload::Text(_)));
        match payload {
            Payload::Box(children) => {
                let ids: Vec<usize> = children
                    .into_iter()
                    .map(|child| self.add(child, Some(idx), &style, tree, anchors))
                    .collect();
                tree.set_children(idx, &ids);
            }
            Payload::Text(content) => {
                self.nodes[idx].content = Content::Text {
                    content,
                    style,
                    label: None,
                };
            }
            Payload::Custom(token) => self.nodes[idx].content = Content::Custom(token),
        }
        self.nodes[idx].end = self.nodes.len();
        idx
    }

    /// Works out where each node takes the pointer, and what Windows should treat each part of
    /// the window as.
    fn place_hits(&mut self, viewport: Rect) {
        // For each node, the clip its children are in, or none if nothing below it takes input.
        let mut inner: Vec<Option<Rect>> = Vec::with_capacity(self.nodes.len());
        for (idx, node) in self.nodes.iter().enumerate() {
            let outer = node.parent.map_or(Some(viewport), |p| inner[p]);
            let open = !node.decor.inert && node.decor.transform.scale(node.rect.size).is_none();
            let Some(outer) = outer.filter(|_| open) else {
                inner.push(None);
                continue;
            };
            let clipped = node.rect.intersect(outer);
            inner.push(Some(if node.clip { clipped } else { outer }));
            if clipped.is_empty() {
                continue;
            }
            let area = node.interact.window_area;
            if let Some(area) = area.or(node.interact.hit.map(|_| WindowControlArea::Client)) {
                self.control_areas.push((clipped, area));
            }
            if node.interact.hit.is_some() {
                self.hits.push((idx, clipped));
            }
        }
    }

    /// The children of node `idx`, in order.
    fn children(&self, idx: usize) -> impl Iterator<Item = usize> + '_ {
        let (mut next, end) = (idx + 1, self.nodes[idx].end);
        std::iter::from_fn(move || {
            let child = (next < end).then_some(next)?;
            next = self.nodes[child].end;
            Some(child)
        })
    }

    /// The topmost node taking the pointer at `pos`, then its ancestors.
    fn chain_at(&self, pos: Point) -> impl Iterator<Item = &FrameNode> {
        let top = self.hits.iter().rev().find(|(_, rect)| rect.contains(pos));
        let mut next = top.map(|&(idx, _)| idx);
        std::iter::from_fn(move || {
            let node = &self.nodes[next?];
            next = node.parent;
            Some(node)
        })
    }

    /// The hits under `pos`, innermost first.
    pub(crate) fn hit_chain(&self, pos: Point) -> Vec<HitId> {
        self.chain_at(pos).filter_map(|n| n.interact.hit).collect()
    }

    pub(crate) fn cursor_at(&self, pos: Point) -> CursorStyle {
        self.chain_at(pos)
            .find_map(|n| n.interact.cursor)
            .unwrap_or(CursorStyle::Arrow)
    }

    /// The innermost hit under `pos` with a tooltip: its hit, the tip, and its rect.
    pub(crate) fn tooltip_at(&self, pos: Point) -> Option<(HitId, SharedStr, Rect)> {
        self.chain_at(pos)
            .find_map(|n| Some((n.interact.hit?, n.interact.tooltip.clone()?, n.rect)))
    }

    /// Where the last node with hit `id` was laid out.
    pub(crate) fn rect_of(&self, id: HitId) -> Option<Rect> {
        let node = self
            .nodes
            .iter()
            .rev()
            .find(|n| n.interact.hit == Some(id))?;
        Some(node.rect)
    }

    /// The nodes the app draws itself, with their rects.
    pub(crate) fn custom_rects(&self) -> impl Iterator<Item = (CustomDraw, Rect)> + '_ {
        self.nodes.iter().filter_map(|n| match n.content {
            Content::Custom(token) => Some((token, n.rect)),
            _ => None,
        })
    }

    /// Paints the frame, handing each custom node to `draw_custom` in its place.
    pub(crate) fn paint(
        &self,
        canvas: &mut Canvas,
        draw_custom: &mut impl FnMut(CustomDraw, Rect, &mut Canvas),
    ) {
        if !self.nodes.is_empty() {
            self.paint_node(0, canvas, draw_custom);
        }
    }

    fn paint_node(
        &self,
        idx: usize,
        canvas: &mut Canvas,
        draw_custom: &mut impl FnMut(CustomDraw, Rect, &mut Canvas),
    ) {
        let node = &self.nodes[idx];
        let (d, rect) = (node.decor, node.rect);
        let scale = d.transform.scale(rect.size);
        if d.opacity <= 0.0 || scale.is_some_and(|(sx, sy)| sx <= 1e-3 || sy <= 1e-3) {
            return;
        }
        // A reveal paints the top of the node, its content clipped to it (and slid down with it).
        let (shape, reveal_clip, slide) = match d.transform {
            Transform::RevealY { shown_ratio, slide } if shown_ratio < 1.0 - 1e-4 => {
                let shown = Rect::new(
                    rect.origin,
                    size(rect.size.width, (rect.size.height * shown_ratio).max(0.0)),
                );
                let slide = slide.then_some(point(0.0, -(rect.size.height - shown.size.height)));
                (shown, Some(shown.inset(node.border)), slide)
            }
            _ => (rect, None, None),
        };
        let outer = Scope {
            opacity: (d.opacity < 1.0 - 1e-4).then_some(d.opacity),
            scale: scale.map(|(sx, sy)| (rect.center(), sx, sy)),
            ..Default::default()
        };
        canvas.scoped(outer, |canvas| {
            if !d.shadows.is_empty() {
                canvas.paint_shadows(shape, d.corners, d.shadows);
            }
            let bordered = !d.border_color.is_transparent() && node.border.any_nonzero();
            if !d.bg.is_transparent() || bordered {
                canvas.paint_quad(PaintQuad {
                    bounds: shape,
                    corner_radii: d.corners,
                    background: d.bg,
                    border_widths: node.border,
                    border_color: d.border_color,
                });
            }
            let inner = Scope {
                clip: reveal_clip.or(node.clip.then_some(rect)),
                translate: slide,
                ..Default::default()
            };
            canvas.scoped(inner, |canvas| match &node.content {
                Content::Box => {
                    for child in self.children(idx) {
                        self.paint_node(child, canvas, draw_custom);
                    }
                }
                Content::Text { style, label, .. } => {
                    if let Some(label) = label {
                        canvas.draw_label(label, rect.origin, rect.size.width, style.color);
                    }
                }
                Content::Custom(token) => draw_custom(*token, rect, canvas),
            });
        });
    }
}

// ---- 3. Pointer, hover and press ------------------------------------------------------------

#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct PointerState {
    pub(crate) pos: Point,
    pub(crate) pressed: bool,
}

/// A control's fill at rest, hovered and pressed.
#[derive(Copy, Clone, Debug)]
pub(crate) struct Brushes {
    pub(crate) rest: Rgba,
    pub(crate) hover: Rgba,
    pub(crate) pressed: Rgba,
}

const BRUSH_FADE: Curve = Curve::new(Duration::from_millis(83), Easing::Linear);

/// A window's interaction state: the last frame laid out, where the pointer is and what it's
/// over or pressing, and the animations its widgets run.
#[derive(Default)]
pub(crate) struct AppUi {
    pub(crate) frame: AppFrame,
    pub(crate) pointer: Option<PointerState>,
    /// The hits under the pointer, innermost first.
    hovered_chain: Vec<HitId>,
    /// The innermost hit the left button went down on.
    pressed: Option<HitId>,
    pub(crate) last_input_was_keyboard: bool,
    pub(crate) anims: AnimTracks<AnimKey>,
}

impl AppUi {
    pub(crate) fn begin_frame(&mut self, now: Instant, reduce_motion: bool) {
        self.anims.begin_frame(now, reduce_motion);
    }

    /// Keeps the frame just laid out; whether another should follow, for a running animation
    /// or because what's under the pointer moved.
    pub(crate) fn finish_frame(&mut self, frame: AppFrame) -> bool {
        self.frame = frame;
        let animating = self.anims.end_frame();
        let hover_moved = self.pointer.is_some_and(|p| self.hover(p.pos));
        animating || hover_moved
    }

    /// Re-reads what's under `pos`; whether it changed.
    fn hover(&mut self, pos: Point) -> bool {
        let chain = self.frame.hit_chain(pos);
        let changed = chain != self.hovered_chain;
        self.hovered_chain = chain;
        changed
    }

    pub(crate) fn on_mouse_move(&mut self, pos: Point, pressed_any: bool) -> bool {
        self.pointer = Some(PointerState {
            pos,
            pressed: pressed_any,
        });
        self.hover(pos)
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

    /// A button went down at `pos`: the hits under it, innermost first.
    pub(crate) fn on_mouse_down(&mut self, pos: Point, button: MouseButton) -> &[HitId] {
        self.last_input_was_keyboard = false;
        self.pointer = Some(PointerState { pos, pressed: true });
        self.hovered_chain = self.frame.hit_chain(pos);
        if button == MouseButton::Left {
            self.pressed = self.hovered_chain.first().copied();
        }
        &self.hovered_chain
    }

    /// A button came up at `pos`: the hit clicked (pressed and released on it with the left
    /// button), and the hits under it.
    pub(crate) fn on_mouse_up(
        &mut self,
        pos: Point,
        button: MouseButton,
    ) -> (Option<HitId>, &[HitId]) {
        if let Some(p) = &mut self.pointer {
            p.pos = pos;
            p.pressed = false;
        }
        self.hovered_chain = self.frame.hit_chain(pos);
        let clicked = if button == MouseButton::Left {
            let pressed = self.pressed.take();
            pressed.filter(|_| pressed == self.hovered_chain.first().copied())
        } else {
            None
        };
        (clicked, &self.hovered_chain)
    }

    pub(crate) fn top_hit(&self) -> Option<HitId> {
        self.hovered_chain.first().copied()
    }

    pub(crate) fn is_hovered(&self, hit: HitId) -> bool {
        self.hovered_chain.contains(&hit)
    }

    /// Whether `hit` is pressed with the pointer still over it.
    pub(crate) fn is_pressed(&self, hit: HitId) -> bool {
        self.pressed == Some(hit) && self.hovered_chain.contains(&hit)
    }

    /// The fill of control `hit` now, fading between `brushes` as it's hovered and pressed.
    pub(crate) fn brush(&mut self, hit: HitId, brushes: Brushes, enabled: bool) -> Rgba {
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
            .animate_color(AnimKey::Hit(hit), target, BRUSH_FADE)
    }
}

// ---- 4. Animation ---------------------------------------------------------------------------

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

/// The CSS cubic Bézier timing function through (x1, y1) and (x2, y2) at time `t`: Newton's
/// method for the curve parameter, falling back to bisection where it stalls.
fn cubic_bezier(x1: f32, y1: f32, x2: f32, y2: f32, t: f32) -> f32 {
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
    let x_at = |s: f32| ((ax * s + bx) * s + cx) * s;
    let y_at = |s: f32| ((ay * s + by) * s + cy) * s;
    let dx_at = |s: f32| (3.0 * ax * s + 2.0 * bx) * s + cx;

    const EPS: f32 = 1e-6;
    let mut s = t;
    let mut solved = false;
    for _ in 0..8 {
        let residual = x_at(s) - t;
        if residual.abs() < EPS {
            solved = true;
            break;
        }
        let slope = dx_at(s);
        if slope.abs() < EPS {
            break;
        }
        s = (s - residual / slope).clamp(0.0, 1.0);
    }
    if !solved {
        let (mut lo, mut hi) = (0.0f32, 1.0f32);
        s = t;
        for _ in 0..32 {
            let x = x_at(s);
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
    y_at(s).clamp(0.0, 1.0)
}

/// How long an animation runs, and how it eases.
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

/// What can be animated: blended `t` of the way from one value to another.
trait Lerp: Copy + PartialEq {
    fn lerp(self, target: Self, t: f32) -> Self;
}

impl Lerp for f32 {
    fn lerp(self, target: Self, t: f32) -> Self {
        self + (target - self) * t
    }
}

impl Lerp for Rgba {
    /// Blends premultiplied, so a fade to or from transparent doesn't pass through black.
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AnimStatus {
    /// Settled where it was asked to be.
    Idle,
    Running,
    /// Reached its target this frame.
    Finished,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct AnimSample<T> {
    pub(crate) value: T,
    pub(crate) status: AnimStatus,
}

/// One animated value: running from `from` to `target` since `started_at`, over `duration`.
#[derive(Clone, Copy, Debug)]
struct AnimTrack<T> {
    from: T,
    target: T,
    started_at: Instant,
    /// How much of a full run the current one is, after turning back partway.
    reversing_scale: f32,
    duration: Duration,
    /// The last frame that asked for it (tracks not asked for in a frame are dropped).
    last_frame: u64,
}

impl<T: Lerp> AnimTrack<T> {
    fn new(
        from: T,
        target: T,
        now: Instant,
        duration: Duration,
        reversing_scale: f32,
        frame: u64,
    ) -> Self {
        Self {
            from,
            target,
            started_at: now,
            reversing_scale,
            duration,
            last_frame: frame,
        }
    }

    /// Samples the track at `now` as it heads for `target` along `curve`, restarting it toward
    /// a new target from where it is (turning back costs only the way it came).
    fn step(
        &mut self,
        target: T,
        curve: Curve,
        now: Instant,
        reduce_motion: bool,
        frame: u64,
    ) -> AnimSample<T> {
        let sample = |value, status| AnimSample { value, status };
        if reduce_motion || curve.duration.is_zero() {
            *self = Self::new(target, target, now, curve.duration, 1.0, frame);
            return sample(target, AnimStatus::Finished);
        }
        self.last_frame = frame;
        let elapsed = now.saturating_duration_since(self.started_at);
        let (progress, status) = if self.duration.is_zero() || elapsed >= self.duration {
            (1.0, AnimStatus::Finished)
        } else {
            (
                elapsed.as_secs_f32() / self.duration.as_secs_f32(),
                AnimStatus::Running,
            )
        };
        let eased = curve.easing.sample(progress);
        let current = self.from.lerp(self.target, eased);
        if target == self.target {
            let settled = self.from == self.target;
            return sample(current, if settled { AnimStatus::Idle } else { status });
        }
        let reversing_scale = if target == self.from {
            (eased * self.reversing_scale + (1.0 - self.reversing_scale)).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let duration = curve.duration.mul_f32(reversing_scale);
        *self = Self::new(current, target, now, duration, reversing_scale, frame);
        if duration.is_zero() {
            sample(target, AnimStatus::Finished)
        } else {
            sample(current, AnimStatus::Running)
        }
    }
}

/// A set of animated values keyed by `K`, sampled once per frame. A track not asked for in a
/// frame is dropped at its end.
pub(crate) struct AnimTracks<K: Copy + Eq + Hash> {
    frame: u64,
    now: Instant,
    reduce_motion: bool,
    running: bool,
    scalars: HashMap<K, AnimTrack<f32>>,
    colors: HashMap<K, AnimTrack<Rgba>>,
}

impl<K: Copy + Eq + Hash> Default for AnimTracks<K> {
    fn default() -> Self {
        Self {
            frame: 0,
            now: Instant::now(),
            reduce_motion: false,
            running: false,
            scalars: HashMap::new(),
            colors: HashMap::new(),
        }
    }
}

impl<K: Copy + Eq + Hash> AnimTracks<K> {
    pub(crate) fn begin_frame(&mut self, now: Instant, reduce_motion: bool) {
        self.frame = self.frame.wrapping_add(1);
        (self.now, self.reduce_motion, self.running) = (now, reduce_motion, false);
    }

    /// Drops the tracks this frame didn't ask for; whether any is still running.
    pub(crate) fn end_frame(&mut self) -> bool {
        let frame = self.frame;
        self.scalars.retain(|_, track| track.last_frame == frame);
        self.colors.retain(|_, track| track.last_frame == frame);
        self.running
    }

    /// Runs from 0 toward 1 while `active`, and back toward 0 when not.
    pub(crate) fn progress(&mut self, key: K, active: bool, curve: Curve) -> AnimSample<f32> {
        self.animate_f32_from(key, 0.0, if active { 1.0 } else { 0.0 }, curve)
    }

    /// Animates toward `target`, starting from `initial` the first time `key` is seen.
    pub(crate) fn animate_f32_from(
        &mut self,
        key: K,
        initial: f32,
        target: f32,
        curve: Curve,
    ) -> AnimSample<f32> {
        let (frame, now) = (self.frame, self.now);
        let track = self
            .scalars
            .entry(key)
            .or_insert_with(|| AnimTrack::new(initial, target, now, curve.duration, 1.0, frame));
        let sample = track.step(target, curve, now, self.reduce_motion, frame);
        self.running |= sample.status == AnimStatus::Running;
        sample
    }

    /// Fades toward `target`, starting there the first time `key` is seen.
    fn animate_color(&mut self, key: K, target: Rgba, curve: Curve) -> Rgba {
        let (frame, now) = (self.frame, self.now);
        let track = self
            .colors
            .entry(key)
            .or_insert_with(|| AnimTrack::new(target, target, now, curve.duration, 1.0, frame));
        let sample = track.step(target, curve, now, self.reduce_motion, frame);
        self.running |= sample.status == AnimStatus::Running;
        sample.value
    }

    #[cfg(test)]
    pub(crate) fn track_count(&self) -> usize {
        self.scalars.len() + self.colors.len()
    }
}

/// Whether something that animates in and out (a dialog, a menu panel, the find bar) is showing.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Presence {
    #[default]
    Hidden,
    Open,
    Closing,
}

/// One showing of something that animates in and out. Each opening gets a new serial, so its
/// animation tracks start fresh.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct Transition {
    serial: usize,
    phase: Presence,
}

impl Transition {
    pub(crate) fn open_new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(1);
        Self {
            serial: NEXT.fetch_add(1, Ordering::Relaxed),
            phase: Presence::Open,
        }
    }

    /// Opens it, as a new showing, unless it's open already.
    pub(crate) fn open(&mut self) {
        if !self.is_open() {
            *self = Self::open_new();
        }
    }

    /// Starts closing it; whether it was open.
    pub(crate) fn close(&mut self) -> bool {
        let was_open = self.is_open();
        if was_open {
            self.phase = Presence::Closing;
        }
        was_open
    }

    pub(crate) fn finish(&mut self) {
        self.phase = Presence::Hidden;
    }

    pub(crate) fn serial(self) -> usize {
        self.serial
    }

    /// Animation `track` of this showing, running while `active`.
    pub(crate) fn progress(
        self,
        ui: &mut AppUi,
        track: u8,
        active: bool,
        curve: Curve,
    ) -> AnimSample<f32> {
        let key = AnimKey::Transition {
            serial: self.serial,
            track,
        };
        ui.anims.progress(key, active, curve)
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

// ---- 5. The app's ids -----------------------------------------------------------------------

crate::types::id_type!(TabId);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum MenuKind {
    File,
    Edit,
    View,
}

/// Which editor of a window: a tab's document, or one of the fields.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum EditorSlot {
    Tab(TabId),
    FindQuery,
    FindReplace,
    GoToLine,
}

/// What has a window's keyboard focus.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Focus {
    Editor(EditorSlot),
    MenuBar,
    DialogButton(usize),
}

/// What a custom node is: the window draws these itself.
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
    /// Row `ix` of the open panel with transition `serial` (button `col` of a button row).
    Row {
        serial: usize,
        ix: usize,
        col: usize,
    },
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

/// What a node is to the pointer: every hit-testable part of a window.
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
    FieldClear(EditorSlot),
}

/// What an animation track belongs to.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum AnimKey {
    /// A control's brush fade.
    Hit(HitId),
    TabX(usize),
    TabWidth(usize),
    TabScroll,
    TabExtent,
    /// Track `track` of a showing of something that animates in and out.
    Transition {
        serial: usize,
        track: u8,
    },
    ScrollOpacity(EditorSlot),
    ScrollThickness(EditorSlot, Axis),
}

#[cfg(test)]
mod tests {
    use crate::geom::{point, rgb, white};

    use super::*;

    const STRIP: HitId = HitId::Tab(TabHit::Strip);
    const NEW_TAB: HitId = HitId::Tab(TabHit::New);
    const BACKDROP: HitId = HitId::Dialog(DialogHit::Backdrop);
    fn tab(id: u64) -> HitId {
        HitId::Tab(TabHit::Item(TabId(id)))
    }
    fn tab_close(id: u64) -> HitId {
        HitId::Tab(TabHit::Close(TabId(id)))
    }
    fn button(ix: usize) -> HitId {
        HitId::Dialog(DialogHit::Button(ix))
    }

    #[test]
    fn scene_hit_chain_bubbles_nested_hits_and_clips_overflow() {
        let ts = TextSystem::new().unwrap();
        let tree: AppNode = row()
            .size_full()
            .child(
                row()
                    .w(120.)
                    .h(32.)
                    .overflow_hidden()
                    .hit(STRIP)
                    .child(
                        row().w(100.).h(32.).flex_none().hit(tab(1)).child(
                            box_()
                                .absolute()
                                .right(6.)
                                .top(5.)
                                .w(22.)
                                .h(22.)
                                .hit(tab_close(1))
                                .tooltip("Close tab"),
                        ),
                    )
                    .child(row().w(100.).h(32.).flex_none().hit(tab(2))),
            )
            .child(box_().w(40.).h(32.).hit(NEW_TAB).bg(rgb(0x123456)));

        let frame = AppFrame::layout(tree, AvailSize::definite(200., 32.), 1.0, &ts);

        assert_eq!(
            frame.hit_chain(point(80., 16.)),
            vec![tab_close(1), tab(1), STRIP]
        );
        assert_eq!(
            frame.tooltip_at(point(80., 16.)).map(|(_, tip, _)| tip),
            Some(SharedStr::Static("Close tab"))
        );

        assert_eq!(frame.hit_chain(point(130., 16.)), vec![NEW_TAB]);

        let modal: AppNode = box_()
            .size_full()
            .child(box_().w(100.).h(40.).hit(NEW_TAB))
            .child(
                box_()
                    .absolute()
                    .inset_0()
                    .hit(BACKDROP)
                    .child(box_().w(80.).h(32.).hit(button(0)).bg(white())),
            );
        let modal_frame = AppFrame::layout(modal, AvailSize::definite(200., 100.), 1.0, &ts);
        assert_eq!(
            modal_frame.hit_chain(point(10., 10.)),
            vec![button(0), BACKDROP]
        );
        assert_eq!(modal_frame.hit_chain(point(95., 10.)), vec![BACKDROP]);

        let passthrough: AppNode = box_()
            .size_full()
            .child(box_().w(100.).h(40.).hit(NEW_TAB))
            .child(box_().absolute().inset_0());
        let pass_frame = AppFrame::layout(passthrough, AvailSize::definite(200., 100.), 1.0, &ts);
        assert_eq!(pass_frame.hit_chain(point(10., 10.)), vec![NEW_TAB]);
    }

    #[test]
    fn same_frame_anchor_and_measure_fit_work() {
        let menu_row = |serial| {
            HitId::Menu(MenuHit::Row {
                serial,
                ix: 0,
                col: 0,
            })
        };
        let (trigger, popup) = (
            HitId::Menu(MenuHit::Trigger(MenuKind::File)),
            HitId::Menu(MenuHit::Panel),
        );
        let (item, submenu) = (menu_row(1), menu_row(2));
        let (flyout_btn, flyout) = (HitId::Find(FindHit::Options), HitId::Find(FindHit::Card));

        let ts = TextSystem::new().unwrap();
        let tree: AppNode = box_()
            .size_full()
            .child(
                row()
                    .h(32.)
                    .pl(120.)
                    .child(box_().w(50.).h(32.).hit(trigger)),
            )
            .child(
                box_()
                    .absolute()
                    .left(140.)
                    .top(60.)
                    .w(30.)
                    .h(30.)
                    .hit(flyout_btn),
            )
            .child(
                box_()
                    .w(120.)
                    .h(80.)
                    .hit(popup)
                    .anchored(Anchor::Target {
                        target: trigger,
                        place: |r, _, _| point(r.left(), r.bottom()),
                    })
                    .child(box_().absolute().left(1.).top(20.).w(118.).h(30.).hit(item))
                    .child(box_().w(100.).h(60.).hit(submenu).anchored(Anchor::Target {
                        target: item,
                        place: |r, popup, vp| {
                            let right_x = r.right() - 3.;
                            let x = if right_x + popup.width <= vp.width {
                                right_x
                            } else {
                                r.left() - popup.width + 3.
                            };
                            point(x, r.top() - 3.)
                        },
                    })),
            )
            .child(box_().w(80.).h(50.).hit(flyout).anchored(Anchor::Target {
                target: flyout_btn,
                place: |r, popup, _| {
                    let x = r.center().x - (popup.width / 2.).floor();
                    let above = r.top() - (4. + popup.height);
                    point(x, if above >= 32. { above } else { r.bottom() + 4. })
                },
            }));

        let frame = AppFrame::layout(tree, AvailSize::definite(300., 200.), 1.0, &ts);
        assert_eq!(frame.rect_of(popup).unwrap().origin, point(120., 32.));
        assert_eq!(frame.rect_of(submenu).unwrap().origin, point(24., 49.));
        assert_eq!(frame.rect_of(flyout).unwrap().origin, point(115., 94.));

        for scale in [1.0f32, 1.5] {
            let short: AppNode = box_()
                .max_w(320.)
                .border(1., black())
                .px(9.)
                .pt(6.)
                .pb(8.)
                .text_size(12.)
                .line_height(16.)
                .child(text("New tab"));
            let short_sz = AppFrame::measure(short, AvailSize::MAX, scale, &ts);
            assert!(short_sz.width > 30. && short_sz.width < 150.);
            assert!((short_sz.height - 32.).abs() <= 1.0);

            let long: AppNode = box_()
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
            let long_sz = AppFrame::measure(long, AvailSize::MAX, scale, &ts);
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
        let s0 = tracks.animate_f32_from(1, 0.0, 100.0, Curve::new(dur, Easing::Linear));
        assert_eq!(s0.status, AnimStatus::Running);
        assert_eq!(s0.value, 0.0);
        assert!(tracks.end_frame());
        assert_eq!(tracks.track_count(), 1);

        let t_mid = t0 + Duration::from_millis(50);
        tracks.begin_frame(t_mid, false);
        let s1 = tracks.animate_f32_from(1, 0.0, 0.0, Curve::new(dur, Easing::Linear));
        assert_eq!(s1.status, AnimStatus::Running);
        assert!((s1.value - 50.0).abs() < 1e-3);
        assert!(tracks.end_frame());

        let t_done = t_mid + Duration::from_millis(50);
        tracks.begin_frame(t_done, false);
        let s2 = tracks.animate_f32_from(1, 0.0, 0.0, Curve::new(dur, Easing::Linear));
        assert_eq!(s2.status, AnimStatus::Finished);
        assert_eq!(s2.value, 0.0);
        assert!(!tracks.end_frame());

        tracks.begin_frame(t_done, false);
        assert!(!tracks.end_frame());
        assert_eq!(tracks.track_count(), 0);
    }
}
