use std::hash::Hash;
use std::time::{Duration, Instant};

use crate::color::{Rgba, black, transparent_black};
use crate::geom::{Corners, Edges, Point, Rect, Shadows, Size};
use crate::gfx::{
    D2DRenderer, Font, FontWeight, Label, LabelLayout, PaintQuad, TextAlign, TextStyle, TextSystem,
};
use crate::platform::{CursorStyle, MouseButton, WindowControlArea};
use crate::types::SharedStr;

use super::anim::{AnimSample, AnimTracks, Easing};
use super::layout::{
    AlignItems, AvailSize, AvailableSpace, Dim, FlexDirection, FlexStyle, JustifyContent,
    LayoutEngine, LayoutId, Overflow, Position, snap_border_edges,
};

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
    align: Option<TextAlign>,
    nowrap: Option<bool>,
    line_clamp: Option<usize>,
}

#[derive(Clone)]
struct ResolvedTextStyle {
    font: Font,
    size: f32,
    line_height: f32,
    color: Rgba,
    align: TextAlign,
    nowrap: bool,
    line_clamp: Option<usize>,
}

impl Default for ResolvedTextStyle {
    fn default() -> Self {
        Self {
            font: Font::default(),
            size: 14.0,
            line_height: 20.0,
            color: black(),
            align: TextAlign::Left,
            nowrap: false,
            line_clamp: None,
        }
    }
}

impl ResolvedTextStyle {
    fn merge(&self, o: &TextOverrides) -> Self {
        let mut out = self.clone();
        if let Some(fam) = &o.family {
            out.font.family = fam.clone();
        }
        if let Some(w) = o.weight {
            out.font.weight = w;
        }
        if let Some(s) = o.size {
            out.size = s;
        }
        if let Some(lh) = o.line_height {
            out.line_height = lh;
        }
        if let Some(c) = o.color {
            out.color = c;
        }
        if let Some(a) = o.align {
            out.align = a;
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

enum NodePayload<H, C> {
    Box(Vec<Node<H, C>>),
    Text(SharedStr),
    Custom(C),
}

pub(crate) struct Node<H, C> {
    flex: FlexStyle,
    bg: Rgba,
    border_color: Rgba,
    corners: Corners,
    shadows: Shadows,
    opacity: f32,
    inert: bool,
    text: TextOverrides,
    hit: Option<H>,
    cursor: Option<CursorStyle>,
    window_area: Option<WindowControlArea>,
    tooltip: Option<SharedStr>,
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
    fn new_box(direction: FlexDirection) -> Self {
        Self {
            flex: FlexStyle {
                direction,
                ..Default::default()
            },
            bg: transparent_black(),
            border_color: transparent_black(),
            corners: Corners::ZERO,
            shadows: Shadows::NONE,
            opacity: 1.0,
            inert: false,
            text: TextOverrides::default(),
            hit: None,
            cursor: None,
            window_area: None,
            tooltip: None,
            payload: NodePayload::Box(Vec::new()),
        }
    }

    setters! {
        row() => |s| { s.flex.direction = FlexDirection::Row; }
        absolute() => |s| { s.flex.position = Position::Absolute; }
        relative() => |s| { s.flex.position = Position::Relative; }
        w(v: f32) => |s| { s.flex.width = Dim::Px(v); }
        h(v: f32) => |s| { s.flex.height = Dim::Px(v); }
        w_pct(f: f32) => |s| { s.flex.width = Dim::Pct(f); }
        h_pct(f: f32) => |s| { s.flex.height = Dim::Pct(f); }
        min_w(v: f32) => |s| { s.flex.min_width = Dim::Px(v); }
        min_h(v: f32) => |s| { s.flex.min_height = Dim::Px(v); }
        min_h_full() => |s| { s.flex.min_height = Dim::Pct(1.0); }
        max_w_full() => |s| { s.flex.max_width = Dim::Pct(1.0); }
        flex_1() => |s| {
            s.flex.flex_grow = 1.0;
            s.flex.flex_shrink = 1.0;
            s.flex.flex_basis = Dim::Pct(0.0);
        }
        flex_none() => |s| {
            s.flex.flex_grow = 0.0;
            s.flex.flex_shrink = 0.0;
        }
        gap(v: f32) => |s| { s.flex.gap = v; }
        items_center() => |s| { s.flex.align_items = Some(AlignItems::Center); }
        self_start() => |s| { s.flex.align_self = Some(AlignItems::Start); }
        justify_center() => |s| { s.flex.justify_content = Some(JustifyContent::Center); }
        overflow_hidden() => |s| {
            s.flex.overflow_x = Overflow::Hidden;
            s.flex.overflow_y = Overflow::Hidden;
        }
        overflow_x_hidden() => |s| { s.flex.overflow_x = Overflow::Hidden; }
        p(v: f32) => |s| { s.flex.padding = Edges::all(Dim::Px(v)); }
        px(v: f32) => |s| {
            s.flex.padding.left = Dim::Px(v);
            s.flex.padding.right = Dim::Px(v);
        }
        py(v: f32) => |s| {
            s.flex.padding.top = Dim::Px(v);
            s.flex.padding.bottom = Dim::Px(v);
        }
        pt(v: f32) => |s| { s.flex.padding.top = Dim::Px(v); }
        pr(v: f32) => |s| { s.flex.padding.right = Dim::Px(v); }
        pb(v: f32) => |s| { s.flex.padding.bottom = Dim::Px(v); }
        pl(v: f32) => |s| { s.flex.padding.left = Dim::Px(v); }
        padding(e: Edges<f32>) => |s| {
            s.flex.padding = Edges {
                top: Dim::Px(e.top),
                right: Dim::Px(e.right),
                bottom: Dim::Px(e.bottom),
                left: Dim::Px(e.left),
            };
        }
        mt(v: f32) => |s| { s.flex.margin.top = Dim::Px(v); }
        mb(v: f32) => |s| { s.flex.margin.bottom = Dim::Px(v); }
        ml(v: f32) => |s| { s.flex.margin.left = Dim::Px(v); }
        top(v: f32) => |s| { s.flex.inset.top = Dim::Px(v); }
        right(v: f32) => |s| { s.flex.inset.right = Dim::Px(v); }
        bottom(v: f32) => |s| { s.flex.inset.bottom = Dim::Px(v); }
        left(v: f32) => |s| { s.flex.inset.left = Dim::Px(v); }
        bg(color: Rgba) => |s| { s.bg = color; }
        border(width: f32, color: Rgba) => |s| {
            s.flex.border = Edges::all(width);
            s.border_color = color;
        }
        border_widths(widths: Edges<f32>, color: Rgba) => |s| {
            s.flex.border = widths;
            s.border_color = color;
        }
        border_t(w: f32) => |s| { s.flex.border.top = w; }
        border_r(w: f32) => |s| { s.flex.border.right = w; }
        border_b(w: f32) => |s| { s.flex.border.bottom = w; }
        border_l(w: f32) => |s| { s.flex.border.left = w; }
        border_color(color: Rgba) => |s| { s.border_color = color; }
        rounded(r: f32) => |s| { s.corners = Corners::all(r); }
        rounded_full() => |s| { s.corners = Corners::all(9999.0); }
        rounded_t(r: f32) => |s| { s.corners = Corners::top(r); }
        rounded_b(r: f32) => |s| { s.corners = Corners::bottom(r); }
        shadows(shadows: Shadows) => |s| { s.shadows = shadows; }
        opacity(opacity: f32) => |s| { s.opacity = opacity; }
        inert() => |s| { s.inert = true; }
        font_semibold() => |s| { s.text.weight = Some(FontWeight::SEMIBOLD); }
        text_size(size: f32) => |s| { s.text.size = Some(size); }
        line_height(lh: f32) => |s| { s.text.line_height = Some(lh); }
        text_color(color: Rgba) => |s| { s.text.color = Some(color); }
        text_center() => |s| { s.text.align = Some(TextAlign::Center); }
        nowrap() => |s| { s.text.nowrap = Some(true); }
        line_clamp(lines: usize) => |s| { s.text.line_clamp = Some(lines); }
        hit(id: H) => |s| { s.hit = Some(id); }
        cursor(cursor: CursorStyle) => |s| { s.cursor = Some(cursor); }
        window_area(area: WindowControlArea) => |s| { s.window_area = Some(area); }
    }

    pub(crate) fn w_full(self) -> Self {
        self.w_pct(1.0)
    }
    pub(crate) fn h_full(self) -> Self {
        self.h_pct(1.0)
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
    #[cfg(test)]
    pub(crate) fn inset_0(self) -> Self {
        self.top(0.0).right(0.0).bottom(0.0).left(0.0)
    }
    pub(crate) fn font_family(mut self, family: impl Into<SharedStr>) -> Self {
        self.text.family = Some(family.into());
        self
    }
    pub(crate) fn tooltip(mut self, tip: impl Into<SharedStr>) -> Self {
        self.tooltip = Some(tip.into());
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
    Node::new_box(FlexDirection::Row)
}

pub(crate) fn col<H, C>() -> Node<H, C> {
    Node::new_box(FlexDirection::Column)
}

pub(crate) fn box_<H, C>() -> Node<H, C> {
    Node::new_box(FlexDirection::Column)
}

pub(crate) fn text<H, C>(content: impl Into<SharedStr>) -> Node<H, C> {
    let mut node = Node::new_box(FlexDirection::Column);
    node.payload = NodePayload::Text(content.into());
    node
}

pub(crate) fn custom<H, C>(token: C) -> Node<H, C> {
    let mut node = Node::new_box(FlexDirection::Column);
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
    layout_id: LayoutId,
    rect: Rect,
    bg: Rgba,
    border: Edges<f32>,
    border_color: Rgba,
    corners: Corners,
    shadows: Shadows,
    opacity: f32,
    inert: bool,
    clip: bool,
    hit: Option<H>,
    cursor: Option<CursorStyle>,
    window_area: Option<WindowControlArea>,
    tooltip: Option<SharedStr>,
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
    control_areas: Vec<(Rect, Option<WindowControlArea>)>,
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
    pub(crate) fn layout(
        root_node: Node<H, C>,
        available: AvailSize,
        scale: f32,
        text_system: &TextSystem,
    ) -> Self {
        let mut engine = LayoutEngine::new();
        let mut frame = Frame::default();
        let mut layout_to_flat: Vec<usize> = Vec::new();
        let default_text = ResolvedTextStyle::default();

        let root_idx = frame.flatten(
            root_node,
            None,
            &default_text,
            scale,
            &mut engine,
            &mut layout_to_flat,
        );
        frame.root = Some(root_idx);
        let root_layout_id = frame.nodes[root_idx].layout_id;

        engine.compute(root_layout_id, available, scale, &mut |lid, kw, _kh, avail| {
            let flat_idx = layout_to_flat[lid.0 as usize];
            let FlatKind::Text {
                content,
                style,
                label,
            } = &mut frame.nodes[flat_idx].kind
            else {
                return Size::ZERO;
            };
            let wrap_width = if style.nowrap {
                None
            } else {
                kw.or(match avail.width {
                    AvailableSpace::Definite(w) => Some(w),
                    _ => None,
                })
            };
            let shaped = text_system.shape_label(
                content.clone(),
                &TextStyle::new(style.font.clone(), style.size),
                LabelLayout {
                    line_height: style.line_height,
                    wrap_width,
                    line_clamp: style.line_clamp,
                    align: style.align,
                },
            );
            let sz = shaped.size();
            *label = Some(shaped);
            sz
        });

        for node in &mut frame.nodes {
            node.rect = engine.bounds(node.layout_id, scale);
        }

        let initial_clip = match (available.width, available.height) {
            (AvailableSpace::Definite(w), AvailableSpace::Definite(h)) => {
                Rect::new(Point::ZERO, Size::new(w, h))
            }
            _ => frame.nodes[root_idx].rect,
        };
        frame.collect_hits(root_idx, initial_clip);
        frame
    }

    pub(crate) fn measure_min_size(
        root_node: Node<H, C>,
        scale: f32,
        text_system: &TextSystem,
    ) -> Size {
        let frame = Self::layout(root_node, AvailSize::MIN, scale, text_system);
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
        layout_to_flat: &mut Vec<usize>,
    ) -> usize {
        let resolved_text = inherited_text.merge(&node.text);
        let clip = node.flex.overflow_x == Overflow::Hidden
            || node.flex.overflow_y == Overflow::Hidden;
        let border = snap_border_edges(node.flex.border, scale);
        let flat_idx = self.nodes.len();
        let Node {
            flex,
            bg,
            border_color,
            corners,
            shadows,
            opacity,
            inert,
            hit,
            cursor,
            window_area,
            tooltip,
            payload,
            ..
        } = node;

        self.nodes.push(FlatNode {
            parent,
            layout_id: LayoutId(0),
            rect: Rect::ZERO,
            bg,
            border,
            border_color,
            corners,
            shadows,
            opacity,
            inert,
            clip,
            hit,
            cursor,
            window_area,
            tooltip,
            kind: FlatKind::Box {
                children_start: 0,
                children_len: 0,
            },
        });

        let (lid, kind) = match payload {
            NodePayload::Box(children) => {
                let mut child_flat_ids = Vec::with_capacity(children.len());
                let mut child_layout_ids = Vec::with_capacity(children.len());
                for child in children {
                    let c_idx = self.flatten(
                        child,
                        Some(flat_idx),
                        &resolved_text,
                        scale,
                        engine,
                        layout_to_flat,
                    );
                    child_flat_ids.push(c_idx);
                    child_layout_ids.push(self.nodes[c_idx].layout_id);
                }
                let start = self.child_order.len();
                self.child_order.extend_from_slice(&child_flat_ids);
                let lid = engine.add_node(flex, scale, &child_layout_ids, false);
                (
                    lid,
                    FlatKind::Box {
                        children_start: start,
                        children_len: child_flat_ids.len(),
                    },
                )
            }
            NodePayload::Text(content) => (
                engine.add_node(flex, scale, &[], true),
                FlatKind::Text {
                    content,
                    style: resolved_text,
                    label: None,
                },
            ),
            NodePayload::Custom(token) => (
                engine.add_node(flex, scale, &[], false),
                FlatKind::Custom(token),
            ),
        };

        if layout_to_flat.len() <= lid.0 as usize {
            layout_to_flat.resize(lid.0 as usize + 1, 0);
        }
        layout_to_flat[lid.0 as usize] = flat_idx;
        self.nodes[flat_idx].layout_id = lid;
        self.nodes[flat_idx].kind = kind;
        flat_idx
    }

    fn collect_hits(&mut self, idx: usize, clip: Rect) {
        if self.nodes[idx].inert {
            return;
        }
        let rect = self.nodes[idx].rect;
        let clipped = rect.intersect(clip);
        let child_clip = if self.nodes[idx].clip { clipped } else { clip };

        if !clipped.is_empty() {
            if let Some(area) = self.nodes[idx].window_area {
                self.control_areas.push((clipped, Some(area)));
            } else if self.nodes[idx].hit.is_some() {
                self.control_areas.push((clipped, None));
            }
            self.hits.push(HitEntry {
                node_idx: idx,
                clipped_rect: clipped,
            });
        }

        if let FlatKind::Box {
            children_start,
            children_len,
        } = self.nodes[idx].kind
        {
            for offset in 0..children_len {
                let child_idx = self.child_order[children_start + offset];
                self.collect_hits(child_idx, child_clip);
            }
        }
    }

    pub(crate) fn control_areas(&self) -> Vec<(Rect, Option<WindowControlArea>)> {
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
            if let Some(hit) = self.nodes[idx].hit {
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
            .find(|node| node.hit == Some(id))
            .map(|node| node.rect)
    }

    #[cfg(test)]
    pub(crate) fn find_rect(&self, pred: impl Fn(H) -> bool) -> Option<Rect> {
        self.nodes
            .iter()
            .rev()
            .find(|node| node.hit.is_some_and(&pred))
            .map(|node| node.rect)
    }

    pub(crate) fn cursor_at(&self, pos: Point) -> CursorStyle {
        let Some(top) = self.top_hit(pos) else {
            return CursorStyle::Arrow;
        };
        let mut cur = Some(top.node_idx);
        while let Some(idx) = cur {
            if let Some(c) = self.nodes[idx].cursor {
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
            if let Some(tip) = &self.nodes[idx].tooltip
                && let Some(hit) = self.nodes[idx].hit
            {
                return Some((hit, tip.clone(), self.nodes[idx].rect));
            }
            cur = self.nodes[idx].parent;
        }
        None
    }

    pub(crate) fn paint(
        &self,
        renderer: &mut D2DRenderer,
        draw_custom: &mut impl FnMut(C, Rect, &mut D2DRenderer),
    ) {
        if let Some(root) = self.root {
            self.paint_node(root, renderer, draw_custom);
        }
    }

    fn paint_node(
        &self,
        idx: usize,
        renderer: &mut D2DRenderer,
        draw_custom: &mut impl FnMut(C, Rect, &mut D2DRenderer),
    ) {
        let node = &self.nodes[idx];
        if node.opacity <= 0.0 {
            return;
        }
        let pushed_layer = node.opacity < 1.0 - 1e-4;
        if pushed_layer {
            renderer.push_opacity_layer(node.opacity);
        }

        if !node.shadows.is_empty() {
            renderer.paint_shadows(node.rect, node.corners, node.shadows);
        }

        if !node.bg.is_transparent()
            || (!node.border_color.is_transparent() && node.border.any_nonzero())
        {
            renderer.paint_quad(PaintQuad {
                bounds: node.rect,
                corner_radii: node.corners,
                background: node.bg,
                border_widths: node.border,
                border_color: node.border_color,
            });
        }

        let pushed_clip = node.clip;
        if pushed_clip {
            renderer.push_clip(node.rect);
        }

        match &node.kind {
            FlatKind::Box {
                children_start,
                children_len,
            } => {
                for offset in 0..*children_len {
                    let child_idx = self.child_order[*children_start + offset];
                    self.paint_node(child_idx, renderer, draw_custom);
                }
            }
            FlatKind::Text { style, label, .. } => {
                if let Some(label) = label {
                    renderer.draw_label(label, node.rect.origin, node.rect.size.width, style.color);
                }
            }
            FlatKind::Custom(token) => {
                draw_custom(*token, node.rect, renderer);
            }
        }

        if pushed_clip {
            renderer.pop_clip();
        }
        if pushed_layer {
            renderer.pop_layer();
        }
    }
}

pub(crate) struct UiState<H: Copy + Eq + Hash, C: Copy, K: Copy + Eq + Hash> {
    frame: Frame<H, C>,
    pointer: Option<PointerState>,
    hovered_chain: Vec<H>,
    pressed: Option<H>,
    last_input_was_keyboard: bool,
    anims: AnimTracks<K>,
    now: Instant,
    reduce_motion: bool,
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
            now: Instant::now(),
            reduce_motion: false,
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

    pub(crate) fn begin_frame(&mut self, now: Instant, reduce_motion: bool) {
        self.now = now;
        self.reduce_motion = reduce_motion;
        self.anims.begin_frame();
    }

    pub(crate) fn finish_frame(&mut self, frame: Frame<H, C>) -> bool {
        self.frame = frame;
        let animating = self.anims.end_frame();
        let hover_shifted = self
            .pointer
            .is_some_and(|p| self.refresh_hover(p.pos));
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
        self.anims.animate_color(
            K::from(hit),
            target,
            BRUSH_FADE_DURATION,
            Easing::Linear,
            self.now,
            self.reduce_motion,
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
        self.anims.animate_f32_from(
            key,
            initial,
            target,
            duration,
            easing,
            self.now,
            self.reduce_motion,
        )
    }

    #[cfg(test)]
    pub(crate) fn track_count(&self) -> usize {
        self.anims.track_count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::{rgb, white};
    use crate::geom::point;

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
        let ts = TextSystem::new();
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
                    .child(
                        row().w(100.).h(32.).flex_none().hit(TestHit::Tab(2)),
                    ),
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

        let modal: Node<TestHit, ()> = col()
            .size_full()
            .child(box_().w(100.).h(40.).hit(TestHit::NewTab))
            .child(
                box_()
                    .absolute()
                    .inset_0()
                    .hit(TestHit::Backdrop)
                    .child(
                        box_()
                            .w(80.)
                            .h(32.)
                            .hit(TestHit::Button(0))
                            .bg(white()),
                    ),
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
    }
}
