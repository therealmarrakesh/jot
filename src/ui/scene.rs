use std::time::{Duration, Instant};

use crate::color::{Rgba, black, rgba, transparent_black};
use crate::geom::{Corners, Edges, Point, Rect, Shadows, Size};
use crate::gfx::{
    D2DRenderer, Font, FontWeight, PaintQuad, TextAlign, TextSystem, WrappedLine,
};
use crate::platform::{CursorStyle, MouseButton, WindowControlArea};
use crate::theme::Palette;
use crate::types::SharedStr;

use super::anim::{AnimSample, AnimTracks, Easing};
use super::layout::{
    AlignItems, AvailSize, AvailableSpace, Dim, FlexDirection, FlexStyle, JustifyContent,
    LayoutEngine, LayoutId, Overflow, Position, snap_border_edges,
};

pub(crate) const BRUSH_FADE_DURATION: Duration = Duration::from_millis(83);

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum EditorSlot {
    Tab(u64),
    FindQuery,
    FindReplace,
    GoToLine,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum CustomDraw {
    Editor(EditorSlot),
    Scrollbar,
    StatusSpacer,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum HitId {
    CaptionMin,
    CaptionMax,
    CaptionClose,
    TabStrip,
    TabList,
    Tab(u64),
    TabClose(u64),
    NewTab,
    TabScrollLeft,
    TabScrollRight,
    MenuBar,
    MenuTrigger(usize),
    MenuPanel,
    MenuItem(usize),
    MenuCommand(usize),
    SubmenuPanel,
    SubmenuItem(usize),
    ThemeToggle,
    EditorArea,
    Editor(EditorSlot),
    FieldClear(EditorSlot),
    FindCard,
    FindToggleReplace,
    FindNext,
    FindPrev,
    FindOptions,
    FindClose,
    FindReplace,
    FindReplaceAll,
    DialogBackdrop,
    DialogCard,
    DialogButton(usize),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum AnimKey {
    Hit(HitId),
    TabX(usize),
    TabWidth(usize),
    TabScroll,
    TabExtent,
    MenuUnroll(usize),
    MenuAppear(usize),
    MenuFade(usize),
    MenuHover(usize, usize),
    DialogShow(usize),
    DialogFadeIn(usize),
    DialogHide(usize),
    DialogFadeOut(usize),
    FindShow(usize),
    FindHide(usize),
    TooltipFade,
}

#[derive(Copy, Clone, Debug)]
pub(crate) struct Brushes {
    pub(crate) rest: Rgba,
    pub(crate) hover: Rgba,
    pub(crate) pressed: Rgba,
}

impl Brushes {
    pub(crate) fn subtle(p: Palette) -> Self {
        Self {
            rest: transparent_black(),
            hover: rgba(p.subtle_secondary),
            pressed: rgba(p.subtle_tertiary),
        }
    }
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

enum NodePayload {
    Box(Vec<Node>),
    Text(SharedStr),
    Custom(CustomDraw),
}

pub(crate) struct Node {
    flex: FlexStyle,
    bg: Rgba,
    border_color: Rgba,
    corners: Corners,
    shadows: Shadows,
    opacity: f32,
    text: TextOverrides,
    hit: Option<HitId>,
    cursor: Option<CursorStyle>,
    window_area: Option<WindowControlArea>,
    tooltip: Option<SharedStr>,
    payload: NodePayload,
}

impl Node {
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
            text: TextOverrides::default(),
            hit: None,
            cursor: None,
            window_area: None,
            tooltip: None,
            payload: NodePayload::Box(Vec::new()),
        }
    }

    pub(crate) fn row(mut self) -> Self {
        self.flex.direction = FlexDirection::Row;
        self
    }
    pub(crate) fn absolute(mut self) -> Self {
        self.flex.position = Position::Absolute;
        self
    }
    pub(crate) fn relative(mut self) -> Self {
        self.flex.position = Position::Relative;
        self
    }

    pub(crate) fn w(mut self, v: f32) -> Self {
        self.flex.width = Dim::Px(v);
        self
    }
    pub(crate) fn h(mut self, v: f32) -> Self {
        self.flex.height = Dim::Px(v);
        self
    }
    pub(crate) fn w_pct(mut self, f: f32) -> Self {
        self.flex.width = Dim::Pct(f);
        self
    }
    pub(crate) fn h_pct(mut self, f: f32) -> Self {
        self.flex.height = Dim::Pct(f);
        self
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
    pub(crate) fn min_w(mut self, v: f32) -> Self {
        self.flex.min_width = Dim::Px(v);
        self
    }
    pub(crate) fn min_w_0(self) -> Self {
        self.min_w(0.0)
    }
    pub(crate) fn min_h(mut self, v: f32) -> Self {
        self.flex.min_height = Dim::Px(v);
        self
    }
    pub(crate) fn min_h_full(mut self) -> Self {
        self.flex.min_height = Dim::Pct(1.0);
        self
    }
    pub(crate) fn max_w_full(mut self) -> Self {
        self.flex.max_width = Dim::Pct(1.0);
        self
    }

    pub(crate) fn flex_1(mut self) -> Self {
        self.flex.flex_grow = 1.0;
        self.flex.flex_shrink = 1.0;
        self.flex.flex_basis = Dim::Pct(0.0);
        self
    }
    pub(crate) fn flex_none(mut self) -> Self {
        self.flex.flex_grow = 0.0;
        self.flex.flex_shrink = 0.0;
        self
    }
    pub(crate) fn gap(mut self, v: f32) -> Self {
        self.flex.gap = v;
        self
    }

    pub(crate) fn items_center(mut self) -> Self {
        self.flex.align_items = Some(AlignItems::Center);
        self
    }
    pub(crate) fn self_start(mut self) -> Self {
        self.flex.align_self = Some(AlignItems::Start);
        self
    }
    pub(crate) fn justify_center(mut self) -> Self {
        self.flex.justify_content = Some(JustifyContent::Center);
        self
    }

    pub(crate) fn overflow_hidden(mut self) -> Self {
        self.flex.overflow_x = Overflow::Hidden;
        self.flex.overflow_y = Overflow::Hidden;
        self
    }
    pub(crate) fn overflow_x_hidden(mut self) -> Self {
        self.flex.overflow_x = Overflow::Hidden;
        self
    }

    pub(crate) fn p(mut self, v: f32) -> Self {
        self.flex.padding = Edges::all(Dim::Px(v));
        self
    }
    pub(crate) fn px(mut self, v: f32) -> Self {
        self.flex.padding.left = Dim::Px(v);
        self.flex.padding.right = Dim::Px(v);
        self
    }
    pub(crate) fn py(mut self, v: f32) -> Self {
        self.flex.padding.top = Dim::Px(v);
        self.flex.padding.bottom = Dim::Px(v);
        self
    }
    pub(crate) fn pt(mut self, v: f32) -> Self {
        self.flex.padding.top = Dim::Px(v);
        self
    }
    pub(crate) fn pr(mut self, v: f32) -> Self {
        self.flex.padding.right = Dim::Px(v);
        self
    }
    pub(crate) fn pb(mut self, v: f32) -> Self {
        self.flex.padding.bottom = Dim::Px(v);
        self
    }
    pub(crate) fn pl(mut self, v: f32) -> Self {
        self.flex.padding.left = Dim::Px(v);
        self
    }
    pub(crate) fn padding(mut self, e: Edges<f32>) -> Self {
        self.flex.padding = Edges {
            top: Dim::Px(e.top),
            right: Dim::Px(e.right),
            bottom: Dim::Px(e.bottom),
            left: Dim::Px(e.left),
        };
        self
    }

    pub(crate) fn mt(mut self, v: f32) -> Self {
        self.flex.margin.top = Dim::Px(v);
        self
    }
    pub(crate) fn mb(mut self, v: f32) -> Self {
        self.flex.margin.bottom = Dim::Px(v);
        self
    }
    pub(crate) fn ml(mut self, v: f32) -> Self {
        self.flex.margin.left = Dim::Px(v);
        self
    }

    pub(crate) fn top(mut self, v: f32) -> Self {
        self.flex.inset.top = Dim::Px(v);
        self
    }
    pub(crate) fn right(mut self, v: f32) -> Self {
        self.flex.inset.right = Dim::Px(v);
        self
    }
    pub(crate) fn bottom(mut self, v: f32) -> Self {
        self.flex.inset.bottom = Dim::Px(v);
        self
    }
    pub(crate) fn left(mut self, v: f32) -> Self {
        self.flex.inset.left = Dim::Px(v);
        self
    }
    #[cfg(test)]
    pub(crate) fn inset_0(self) -> Self {
        self.top(0.0).right(0.0).bottom(0.0).left(0.0)
    }

    pub(crate) fn bg(mut self, color: Rgba) -> Self {
        self.bg = color;
        self
    }
    pub(crate) fn border(mut self, width: f32, color: Rgba) -> Self {
        self.flex.border = Edges::all(width);
        self.border_color = color;
        self
    }
    pub(crate) fn border_widths(mut self, widths: Edges<f32>, color: Rgba) -> Self {
        self.flex.border = widths;
        self.border_color = color;
        self
    }
    pub(crate) fn border_t(mut self, w: f32) -> Self {
        self.flex.border.top = w;
        self
    }
    pub(crate) fn border_r(mut self, w: f32) -> Self {
        self.flex.border.right = w;
        self
    }
    pub(crate) fn border_b(mut self, w: f32) -> Self {
        self.flex.border.bottom = w;
        self
    }
    pub(crate) fn border_l(mut self, w: f32) -> Self {
        self.flex.border.left = w;
        self
    }
    pub(crate) fn border_color(mut self, color: Rgba) -> Self {
        self.border_color = color;
        self
    }
    pub(crate) fn rounded(mut self, r: f32) -> Self {
        self.corners = Corners::all(r);
        self
    }
    pub(crate) fn rounded_full(mut self) -> Self {
        self.corners = Corners::all(9999.0);
        self
    }
    pub(crate) fn rounded_t(mut self, r: f32) -> Self {
        self.corners = Corners::top(r);
        self
    }
    pub(crate) fn rounded_b(mut self, r: f32) -> Self {
        self.corners = Corners::bottom(r);
        self
    }
    pub(crate) fn shadows(mut self, shadows: Shadows) -> Self {
        self.shadows = shadows;
        self
    }
    pub(crate) fn opacity(mut self, opacity: f32) -> Self {
        self.opacity = opacity;
        self
    }

    pub(crate) fn font_family(mut self, family: impl Into<SharedStr>) -> Self {
        self.text.family = Some(family.into());
        self
    }
    pub(crate) fn font_semibold(mut self) -> Self {
        self.text.weight = Some(FontWeight::SEMIBOLD);
        self
    }
    pub(crate) fn text_size(mut self, size: f32) -> Self {
        self.text.size = Some(size);
        self
    }
    pub(crate) fn line_height(mut self, lh: f32) -> Self {
        self.text.line_height = Some(lh);
        self
    }
    pub(crate) fn text_color(mut self, color: Rgba) -> Self {
        self.text.color = Some(color);
        self
    }
    pub(crate) fn text_center(mut self) -> Self {
        self.text.align = Some(TextAlign::Center);
        self
    }
    pub(crate) fn nowrap(mut self) -> Self {
        self.text.nowrap = Some(true);
        self
    }
    pub(crate) fn line_clamp(mut self, lines: usize) -> Self {
        self.text.line_clamp = Some(lines);
        self
    }

    pub(crate) fn hit(mut self, id: HitId) -> Self {
        self.hit = Some(id);
        self
    }
    pub(crate) fn cursor(mut self, cursor: CursorStyle) -> Self {
        self.cursor = Some(cursor);
        self
    }
    pub(crate) fn window_area(mut self, area: WindowControlArea) -> Self {
        self.window_area = Some(area);
        self
    }
    pub(crate) fn tooltip(mut self, tip: impl Into<SharedStr>) -> Self {
        self.tooltip = Some(tip.into());
        self
    }

    pub(crate) fn child(mut self, child: Node) -> Self {
        if let NodePayload::Box(children) = &mut self.payload {
            children.push(child);
        }
        self
    }
    pub(crate) fn children(mut self, iter: impl IntoIterator<Item = Node>) -> Self {
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

    pub(crate) fn strip_hits(mut self) -> Self {
        self.hit = None;
        self.cursor = None;
        self.window_area = None;
        self.tooltip = None;
        if let NodePayload::Box(children) = &mut self.payload {
            for child in children {
                let taken = std::mem::replace(child, box_());
                *child = taken.strip_hits();
            }
        }
        self
    }
}

pub(crate) fn row() -> Node {
    Node::new_box(FlexDirection::Row)
}

pub(crate) fn col() -> Node {
    Node::new_box(FlexDirection::Column)
}

pub(crate) fn box_() -> Node {
    Node::new_box(FlexDirection::Column)
}

pub(crate) fn text(content: impl Into<SharedStr>) -> Node {
    let mut node = Node::new_box(FlexDirection::Column);
    node.payload = NodePayload::Text(content.into());
    node
}

pub(crate) fn custom(token: CustomDraw) -> Node {
    let mut node = Node::new_box(FlexDirection::Column);
    node.payload = NodePayload::Custom(token);
    node
}

enum FlatKind {
    Box {
        children_start: usize,
        children_len: usize,
    },
    Text {
        content: SharedStr,
        style: ResolvedTextStyle,
        lines: Vec<WrappedLine>,
    },
    Custom(CustomDraw),
}

struct FlatNode {
    parent: Option<usize>,
    layout_id: LayoutId,
    rect: Rect,
    bg: Rgba,
    border: Edges<f32>,
    border_color: Rgba,
    corners: Corners,
    shadows: Shadows,
    opacity: f32,
    clip: bool,
    hit: Option<HitId>,
    cursor: Option<CursorStyle>,
    window_area: Option<WindowControlArea>,
    tooltip: Option<SharedStr>,
    kind: FlatKind,
}

#[derive(Copy, Clone, Debug)]
struct HitEntry {
    node_idx: usize,
    clipped_rect: Rect,
}

#[derive(Default)]
pub(crate) struct Frame {
    nodes: Vec<FlatNode>,
    child_order: Vec<usize>,
    root: Option<usize>,
    hits: Vec<HitEntry>,
    control_areas: Vec<(Rect, Option<WindowControlArea>)>,
}

impl Frame {
    pub(crate) fn layout(
        root_node: Node,
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
                lines,
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
            *lines = text_system.shape_wrapped(
                content.clone(),
                style.size,
                &style.font,
                wrap_width,
                Some(style.line_height),
                style.line_clamp,
                style.align,
            );
            let mut sz = Size::ZERO;
            for line in lines.iter() {
                let ls = line.size(style.line_height);
                sz.height += ls.height;
                sz.width = sz.width.max(ls.width).ceil();
            }
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

    pub(crate) fn measure_min_size(root_node: Node, scale: f32, text_system: &TextSystem) -> Size {
        let frame = Self::layout(root_node, AvailSize::MIN, scale, text_system);
        frame
            .root
            .map_or(Size::ZERO, |idx| frame.nodes[idx].rect.size)
    }

    fn flatten(
        &mut self,
        node: Node,
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

        match node.payload {
            NodePayload::Box(children) => {
                self.nodes.push(FlatNode {
                    parent,
                    layout_id: LayoutId(0),
                    rect: Rect::ZERO,
                    bg: node.bg,
                    border,
                    border_color: node.border_color,
                    corners: node.corners,
                    shadows: node.shadows,
                    opacity: node.opacity,
                    clip,
                    hit: node.hit,
                    cursor: node.cursor,
                    window_area: node.window_area,
                    tooltip: node.tooltip,
                    kind: FlatKind::Box {
                        children_start: 0,
                        children_len: 0,
                    },
                });
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
                let lid = engine.add_node(node.flex, scale, &child_layout_ids, false);
                if layout_to_flat.len() <= lid.0 as usize {
                    layout_to_flat.resize(lid.0 as usize + 1, 0);
                }
                layout_to_flat[lid.0 as usize] = flat_idx;
                let flat = &mut self.nodes[flat_idx];
                flat.layout_id = lid;
                flat.kind = FlatKind::Box {
                    children_start: start,
                    children_len: child_flat_ids.len(),
                };
            }
            NodePayload::Text(content) => {
                let lid = engine.add_node(node.flex, scale, &[], true);
                if layout_to_flat.len() <= lid.0 as usize {
                    layout_to_flat.resize(lid.0 as usize + 1, 0);
                }
                layout_to_flat[lid.0 as usize] = flat_idx;
                self.nodes.push(FlatNode {
                    parent,
                    layout_id: lid,
                    rect: Rect::ZERO,
                    bg: node.bg,
                    border,
                    border_color: node.border_color,
                    corners: node.corners,
                    shadows: node.shadows,
                    opacity: node.opacity,
                    clip,
                    hit: node.hit,
                    cursor: node.cursor,
                    window_area: node.window_area,
                    tooltip: node.tooltip,
                    kind: FlatKind::Text {
                        content,
                        style: resolved_text,
                        lines: Vec::new(),
                    },
                });
            }
            NodePayload::Custom(token) => {
                let lid = engine.add_node(node.flex, scale, &[], false);
                if layout_to_flat.len() <= lid.0 as usize {
                    layout_to_flat.resize(lid.0 as usize + 1, 0);
                }
                layout_to_flat[lid.0 as usize] = flat_idx;
                self.nodes.push(FlatNode {
                    parent,
                    layout_id: lid,
                    rect: Rect::ZERO,
                    bg: node.bg,
                    border,
                    border_color: node.border_color,
                    corners: node.corners,
                    shadows: node.shadows,
                    opacity: node.opacity,
                    clip,
                    hit: node.hit,
                    cursor: node.cursor,
                    window_area: node.window_area,
                    tooltip: node.tooltip,
                    kind: FlatKind::Custom(token),
                });
            }
        }
        flat_idx
    }

    fn collect_hits(&mut self, idx: usize, clip: Rect) {
        let rect = self.nodes[idx].rect;
        let clipped = rect.intersect(clip);
        let child_clip = if self.nodes[idx].clip { clipped } else { clip };

        if !clipped.is_empty() {
            if let Some(area) = self.nodes[idx].window_area {
                self.control_areas.push((clipped, Some(area)));
            } else if self.nodes[idx].hit.is_some() {
                self.control_areas.push((clipped, None));
            }
        }

        if self.nodes[idx].hit.is_some() && !clipped.is_empty() {
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

    pub(crate) fn hit_chain(&self, pos: Point) -> Vec<HitId> {
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

    pub(crate) fn rect_of(&self, id: HitId) -> Option<Rect> {
        self.nodes
            .iter()
            .rev()
            .find(|node| node.hit == Some(id))
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

    pub(crate) fn tooltip_at(&self, pos: Point) -> Option<(HitId, SharedStr, Rect)> {
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
        draw_custom: &mut impl FnMut(CustomDraw, Rect, &mut D2DRenderer),
    ) {
        if let Some(root) = self.root {
            self.paint_node(root, renderer, draw_custom);
        }
    }

    fn paint_node(
        &self,
        idx: usize,
        renderer: &mut D2DRenderer,
        draw_custom: &mut impl FnMut(CustomDraw, Rect, &mut D2DRenderer),
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
            FlatKind::Text {
                style,
                lines,
                ..
            } => {
                let mut y = node.rect.origin.y;
                for line in lines {
                    line.paint(
                        Point::new(node.rect.origin.x, y),
                        style.line_height,
                        style.align,
                        Some(node.rect.size.width),
                        style.color,
                        renderer,
                    );
                    y += style.line_height * line.line_count.max(1) as f32;
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

pub(crate) struct UiState {
    pub(crate) frame: Frame,
    hovered_chain: Vec<HitId>,
    pressed: Option<HitId>,
    pub(crate) last_input_was_keyboard: bool,
    pub(crate) anims: AnimTracks<AnimKey>,
    pub(crate) now: Instant,
    pub(crate) reduce_motion: bool,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            frame: Frame::default(),
            hovered_chain: Vec::new(),
            pressed: None,
            last_input_was_keyboard: false,
            anims: AnimTracks::default(),
            now: Instant::now(),
            reduce_motion: false,
        }
    }
}

impl UiState {
    pub(crate) fn begin_frame(&mut self, now: Instant, reduce_motion: bool) {
        self.now = now;
        self.reduce_motion = reduce_motion;
        self.anims.begin_frame();
    }

    pub(crate) fn finish_frame(&mut self, frame: Frame) -> bool {
        self.frame = frame;
        self.anims.end_frame()
    }

    pub(crate) fn update_hover(&mut self, pos: Point) -> bool {
        let next = self.frame.hit_chain(pos);
        if next != self.hovered_chain {
            self.hovered_chain = next;
            true
        } else {
            false
        }
    }

    pub(crate) fn clear_hover(&mut self) -> bool {
        let had = !self.hovered_chain.is_empty() || self.pressed.is_some();
        self.hovered_chain.clear();
        self.pressed = None;
        had
    }

    pub(crate) fn on_mouse_down(&mut self, pos: Point, button: MouseButton) -> Option<HitId> {
        self.last_input_was_keyboard = false;
        self.hovered_chain = self.frame.hit_chain(pos);
        let top = self.hovered_chain.first().copied();
        if button == MouseButton::Left {
            self.pressed = top;
        }
        top
    }

    pub(crate) fn on_mouse_up(&mut self, pos: Point, button: MouseButton) -> Option<HitId> {
        self.hovered_chain = self.frame.hit_chain(pos);
        if button == MouseButton::Left {
            let was = self.pressed.take();
            let now = self.hovered_chain.first().copied();
            if was.is_some() && was == now {
                return was;
            }
        }
        None
    }

    #[inline]
    pub(crate) fn top_hit(&self) -> Option<HitId> {
        self.hovered_chain.first().copied()
    }

    #[inline]
    pub(crate) fn is_hovered(&self, hit: HitId) -> bool {
        self.hovered_chain.contains(&hit)
    }

    #[inline]
    pub(crate) fn is_pressed(&self, hit: HitId) -> bool {
        self.pressed == Some(hit) && self.hovered_chain.contains(&hit)
    }

    pub(crate) fn brush(&mut self, hit: HitId, brushes: Brushes, enabled: bool) -> Rgba {
        self.brush_keyed(AnimKey::Hit(hit), hit, brushes, enabled)
    }

    pub(crate) fn brush_keyed(
        &mut self,
        key: AnimKey,
        hit: HitId,
        brushes: Brushes,
        enabled: bool,
    ) -> Rgba {
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
            key,
            target,
            BRUSH_FADE_DURATION,
            Easing::Linear,
            self.now,
            self.reduce_motion,
        )
    }

    pub(crate) fn animate_f32(
        &mut self,
        key: AnimKey,
        target: f32,
        duration: Duration,
        easing: Easing,
    ) -> f32 {
        self.anims
            .animate_f32(key, target, duration, easing, self.now, self.reduce_motion)
    }

    pub(crate) fn animate_f32_from(
        &mut self,
        key: AnimKey,
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::{rgb, white};
    use crate::geom::point;

    #[test]
    fn scene_hit_chain_bubbles_nested_hits_and_clips_overflow() {
        let ts = TextSystem::new();
        let tree = row()
            .size_full()
            .child(
                row()
                    .w(120.)
                    .h(32.)
                    .overflow_hidden()
                    .hit(HitId::TabStrip)
                    .child(
                        row()
                            .w(100.)
                            .h(32.)
                            .flex_none()
                            .hit(HitId::Tab(1))
                            .child(
                                box_()
                                    .absolute()
                                    .right(6.)
                                    .top(5.)
                                    .w(22.)
                                    .h(22.)
                                    .hit(HitId::TabClose(1))
                                    .tooltip("Close tab"),
                            ),
                    )
                    .child(
                        row().w(100.).h(32.).flex_none().hit(HitId::Tab(2)),
                    ),
            )
            .child(box_().w(40.).h(32.).hit(HitId::NewTab).bg(rgb(0x123456)));

        let frame = Frame::layout(tree, AvailSize::definite(200., 32.), 1.0, &ts);

        assert_eq!(
            frame.hit_chain(point(80., 16.)),
            vec![HitId::TabClose(1), HitId::Tab(1), HitId::TabStrip]
        );
        assert_eq!(
            frame.tooltip_at(point(80., 16.)).map(|(_, tip, _)| tip),
            Some(SharedStr::new_static("Close tab"))
        );

        assert_eq!(frame.hit_chain(point(130., 16.)), vec![HitId::NewTab]);

        let modal = col()
            .size_full()
            .child(box_().w(100.).h(40.).hit(HitId::NewTab))
            .child(
                box_()
                    .absolute()
                    .inset_0()
                    .hit(HitId::DialogBackdrop)
                    .child(
                        box_()
                            .w(80.)
                            .h(32.)
                            .hit(HitId::DialogButton(0))
                            .bg(white()),
                    ),
            );
        let modal_frame = Frame::layout(modal, AvailSize::definite(200., 100.), 1.0, &ts);
        assert_eq!(
            modal_frame.hit_chain(point(10., 10.)),
            vec![HitId::DialogButton(0), HitId::DialogBackdrop]
        );
        assert_eq!(
            modal_frame.hit_chain(point(95., 10.)),
            vec![HitId::DialogBackdrop]
        );
    }
}
