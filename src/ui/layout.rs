use crate::geom::{Edges, Point, Rect, Size, point, size};

#[inline]
fn round_half_toward_zero(value: f32) -> f32 {
    (value.abs() - 0.5).ceil().copysign(value)
}

#[inline]
fn round_to_device_pixel(logical: f32, scale_factor: f32) -> f32 {
    round_half_toward_zero(logical * scale_factor)
}

#[inline]
fn round_stroke_to_device_pixel(logical: f32, scale_factor: f32) -> f32 {
    if logical == 0.0 {
        0.0
    } else {
        round_to_device_pixel(logical.max(0.0), scale_factor).max(1.0)
    }
}

#[inline]
pub(crate) fn snap_border_edges(border: Edges<f32>, scale_factor: f32) -> Edges<f32> {
    let s = scale_factor.max(0.5);
    Edges {
        top: round_stroke_to_device_pixel(border.top, s) / s,
        right: round_stroke_to_device_pixel(border.right, s) / s,
        bottom: round_stroke_to_device_pixel(border.bottom, s) / s,
        left: round_stroke_to_device_pixel(border.left, s) / s,
    }
}

#[inline]
fn ceil_to_device_pixel(logical: f32, scale_factor: f32) -> f32 {
    (logical * scale_factor).ceil()
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
pub(crate) enum AvailableSpace {
    Definite(f32),
    #[default]
    MinContent,
    MaxContent,
}

#[derive(Copy, Clone, Default, Debug, PartialEq)]
pub(crate) struct AvailSize {
    pub(crate) width: AvailableSpace,
    pub(crate) height: AvailableSpace,
}

impl AvailSize {
    pub(crate) const MIN: Self = Self {
        width: AvailableSpace::MinContent,
        height: AvailableSpace::MinContent,
    };

    #[inline]
    pub(crate) const fn definite(width: f32, height: f32) -> Self {
        Self {
            width: AvailableSpace::Definite(width),
            height: AvailableSpace::Definite(height),
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
    Pct(f32),
}

impl Dim {
    #[inline]
    fn snap(self, scale_factor: f32) -> Self {
        match self {
            Self::Auto => Self::Auto,
            Self::Px(v) => Self::Px(round_to_device_pixel(v, scale_factor)),
            Self::Pct(p) => Self::Pct(p),
        }
    }

    #[inline]
    fn resolve(self, base: Option<f32>) -> Option<f32> {
        match self {
            Self::Auto => None,
            Self::Px(v) => Some(v),
            Self::Pct(p) => base.map(|b| b * p),
        }
    }

    #[inline]
    fn resolve_or_zero(self, base: Option<f32>) -> f32 {
        self.resolve(base).unwrap_or(0.0)
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum FlexDirection {
    Row,
    #[default]
    Column,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum Position {
    #[default]
    Relative,
    Absolute,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum Overflow {
    #[default]
    Visible,
    Hidden,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum AlignItems {
    Start,
    Center,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum JustifyContent {
    Center,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct FlexStyle {
    pub(crate) direction: FlexDirection,
    pub(crate) position: Position,
    pub(crate) overflow_x: Overflow,
    pub(crate) overflow_y: Overflow,
    pub(crate) inset: Edges<Dim>,
    pub(crate) width: Dim,
    pub(crate) height: Dim,
    pub(crate) min_width: Dim,
    pub(crate) min_height: Dim,
    pub(crate) max_width: Dim,
    pub(crate) max_height: Dim,
    pub(crate) margin: Edges<Dim>,
    pub(crate) padding: Edges<Dim>,
    pub(crate) border: Edges<f32>,
    pub(crate) gap: f32,
    pub(crate) align_items: Option<AlignItems>,
    pub(crate) align_self: Option<AlignItems>,
    pub(crate) justify_content: Option<JustifyContent>,
    pub(crate) flex_basis: Dim,
    pub(crate) flex_grow: f32,
    pub(crate) flex_shrink: f32,
}

impl Default for FlexStyle {
    fn default() -> Self {
        Self {
            direction: FlexDirection::Column,
            position: Position::Relative,
            overflow_x: Overflow::Visible,
            overflow_y: Overflow::Visible,
            inset: Edges::all(Dim::Auto),
            width: Dim::Auto,
            height: Dim::Auto,
            min_width: Dim::Auto,
            min_height: Dim::Auto,
            max_width: Dim::Auto,
            max_height: Dim::Auto,
            margin: Edges::all(Dim::Px(0.0)),
            padding: Edges::all(Dim::Px(0.0)),
            border: Edges::ZERO,
            gap: 0.0,
            align_items: None,
            align_self: None,
            justify_content: None,
            flex_basis: Dim::Auto,
            flex_grow: 0.0,
            flex_shrink: 1.0,
        }
    }
}

impl FlexStyle {
    fn snap_for_scale(self, scale: f32) -> Self {
        let snap_edges = |e: Edges<Dim>| Edges {
            top: e.top.snap(scale),
            right: e.right.snap(scale),
            bottom: e.bottom.snap(scale),
            left: e.left.snap(scale),
        };
        Self {
            inset: snap_edges(self.inset),
            width: self.width.snap(scale),
            height: self.height.snap(scale),
            min_width: self.min_width.snap(scale),
            min_height: self.min_height.snap(scale),
            max_width: self.max_width.snap(scale),
            max_height: self.max_height.snap(scale),
            margin: snap_edges(self.margin),
            padding: snap_edges(self.padding),
            border: Edges {
                top: round_stroke_to_device_pixel(self.border.top, scale),
                right: round_stroke_to_device_pixel(self.border.right, scale),
                bottom: round_stroke_to_device_pixel(self.border.bottom, scale),
                left: round_stroke_to_device_pixel(self.border.left, scale),
            },
            gap: round_to_device_pixel(self.gap, scale),
            flex_basis: self.flex_basis.snap(scale),
            ..self
        }
    }

    #[inline]
    fn size_main(&self, row: bool) -> Dim {
        if row { self.width } else { self.height }
    }

    #[inline]
    fn size_cross(&self, row: bool) -> Dim {
        if row { self.height } else { self.width }
    }

    #[inline]
    fn min_main(&self, row: bool) -> Dim {
        if row { self.min_width } else { self.min_height }
    }

    #[inline]
    fn min_cross(&self, row: bool) -> Dim {
        if row { self.min_height } else { self.min_width }
    }

    #[inline]
    fn max_main(&self, row: bool) -> Dim {
        if row { self.max_width } else { self.max_height }
    }

    #[inline]
    fn max_cross(&self, row: bool) -> Dim {
        if row { self.max_height } else { self.max_width }
    }

    #[inline]
    fn overflow_main(&self, row: bool) -> Overflow {
        if row { self.overflow_x } else { self.overflow_y }
    }
}

#[inline]
fn resolve_edges_or_zero(e: Edges<Dim>, base: Option<f32>) -> Edges<f32> {
    Edges {
        top: e.top.resolve_or_zero(base),
        right: e.right.resolve_or_zero(base),
        bottom: e.bottom.resolve_or_zero(base),
        left: e.left.resolve_or_zero(base),
    }
}

#[inline]
fn edge_main_start(e: Edges<f32>, row: bool) -> f32 {
    if row { e.left } else { e.top }
}

#[inline]
fn edge_main_end(e: Edges<f32>, row: bool) -> f32 {
    if row { e.right } else { e.bottom }
}

#[inline]
fn edge_main_sum(e: Edges<f32>, row: bool) -> f32 {
    if row { e.horiz() } else { e.vert() }
}

#[inline]
fn edge_cross_start(e: Edges<f32>, row: bool) -> f32 {
    if row { e.top } else { e.left }
}

#[inline]
fn edge_cross_sum(e: Edges<f32>, row: bool) -> f32 {
    if row { e.vert() } else { e.horiz() }
}

#[derive(Copy, Clone, Debug, Default, PartialEq)]
enum Avail {
    Definite(f32),
    MinContent,
    #[default]
    MaxContent,
}

impl Avail {
    #[inline]
    fn from_logical(space: AvailableSpace, scale: f32) -> Self {
        match space {
            AvailableSpace::Definite(v) => Self::Definite(v * scale),
            AvailableSpace::MinContent => Self::MinContent,
            AvailableSpace::MaxContent => Self::MaxContent,
        }
    }

    #[inline]
    fn to_logical(self, scale: f32) -> AvailableSpace {
        match self {
            Self::Definite(v) => AvailableSpace::Definite(v / scale),
            Self::MinContent => AvailableSpace::MinContent,
            Self::MaxContent => AvailableSpace::MaxContent,
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
        match known {
            Some(v) => Self::Definite(v),
            None => self,
        }
    }

    #[inline]
    fn to_key(self) -> (u8, u32) {
        match self {
            Self::Definite(v) => (0, v.to_bits()),
            Self::MinContent => (1, 0),
            Self::MaxContent => (2, 0),
        }
    }
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
    known_w: Option<f32>,
    known_h: Option<f32>,
    parent_w: Option<f32>,
    parent_h: Option<f32>,
    avail_w: Avail,
    avail_h: Avail,
    run_mode: RunMode,
    sizing_mode: SizingMode,
}

#[derive(Copy, Clone, Eq, PartialEq)]
struct SizeCacheKey {
    known_w: u32,
    known_h: u32,
    avail_w: (u8, u32),
    avail_h: (u8, u32),
    parent_w: u32,
    parent_h: u32,
    sizing_mode: SizingMode,
}

#[inline]
fn opt_f32_bits(v: Option<f32>) -> u32 {
    match v {
        Some(x) => x.to_bits(),
        None => u32::MAX,
    }
}

#[derive(Default)]
struct SizeCache {
    entries: [Option<(SizeCacheKey, Size)>; 8],
    next: usize,
}

impl SizeCache {
    fn get(&self, key: SizeCacheKey) -> Option<Size> {
        for (k, v) in self.entries.iter().flatten() {
            if *k == key {
                return Some(*v);
            }
        }
        None
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

struct Node {
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
    nodes: Vec<Node>,
    children_arena: Vec<LayoutId>,
}

impl LayoutEngine {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn add_node(
        &mut self,
        style: FlexStyle,
        scale: f32,
        children: &[LayoutId],
        measured: bool,
    ) -> LayoutId {
        let id = LayoutId(self.nodes.len() as u32);
        let children_start = self.children_arena.len() as u32;
        let children_len = children.len() as u32;
        self.children_arena.extend_from_slice(children);
        for &child in children {
            self.nodes[child.idx()].parent = Some(id);
        }
        self.nodes.push(Node {
            style: style.snap_for_scale(scale),
            parent: None,
            children_start,
            children_len,
            measured,
            location: Point::ZERO,
            size: Size::ZERO,
            abs_origin: None,
            abs_bounds: None,
            cache: SizeCache::default(),
        });
        id
    }

    pub(crate) fn compute(
        &mut self,
        root: LayoutId,
        available: AvailSize,
        scale: f32,
        measure: &mut impl FnMut(LayoutId, Option<f32>, Option<f32>, AvailSize) -> Size,
    ) {
        let avail_w = Avail::from_logical(available.width, scale);
        let avail_h = Avail::from_logical(available.height, scale);
        let req = LayoutReq {
            known_w: None,
            known_h: None,
            parent_w: match avail_w {
                Avail::Definite(w) => Some(w),
                _ => None,
            },
            parent_h: match avail_h {
                Avail::Definite(h) => Some(h),
                _ => None,
            },
            avail_w,
            avail_h,
            run_mode: RunMode::PerformLayout,
            sizing_mode: SizingMode::InherentSize,
        };
        let mut solver = Solver {
            nodes: &mut self.nodes,
            children: &self.children_arena,
            scale,
        };
        let root_size = solver.compute_node(root, req, measure);
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
                let parent_origin = self.nodes[parent_id.idx()]
                    .abs_origin
                    .unwrap_or(Point::ZERO);
                parent_origin + location
            }
            None => location,
        };
        self.nodes[id.idx()].abs_origin = Some(abs_origin);

        let abs_far = point(
            abs_origin.x + node_size.width,
            abs_origin.y + node_size.height,
        );
        let snapped = Rect::from_corners(
            point(
                round_half_toward_zero(abs_origin.x),
                round_half_toward_zero(abs_origin.y),
            ),
            point(
                round_half_toward_zero(abs_far.x),
                round_half_toward_zero(abs_far.y),
            ),
        );
        let logical = Rect::new(
            point(snapped.origin.x / scale, snapped.origin.y / scale),
            size(snapped.size.width / scale, snapped.size.height / scale),
        );
        self.nodes[id.idx()].abs_bounds = Some(logical);
        logical
    }
}

struct Solver<'a> {
    nodes: &'a mut [Node],
    children: &'a [LayoutId],
    scale: f32,
}

impl<'a> Solver<'a> {
    fn child_slice(&self, id: LayoutId) -> &'a [LayoutId] {
        let node = &self.nodes[id.idx()];
        let start = node.children_start as usize;
        let end = start + node.children_len as usize;
        &self.children[start..end]
    }

    fn compute_node(
        &mut self,
        id: LayoutId,
        req: LayoutReq,
        measure: &mut impl FnMut(LayoutId, Option<f32>, Option<f32>, AvailSize) -> Size,
    ) -> Size {
        if req.run_mode == RunMode::ComputeSize {
            let key = SizeCacheKey {
                known_w: opt_f32_bits(req.known_w),
                known_h: opt_f32_bits(req.known_h),
                avail_w: req.avail_w.to_key(),
                avail_h: req.avail_h.to_key(),
                parent_w: opt_f32_bits(req.parent_w),
                parent_h: opt_f32_bits(req.parent_h),
                sizing_mode: req.sizing_mode,
            };
            if let Some(cached) = self.nodes[id.idx()].cache.get(key) {
                return cached;
            }
            let result = self.compute_uncached(id, req, measure);
            self.nodes[id.idx()].cache.insert(key, result);
            return result;
        }
        let result = self.compute_uncached(id, req, measure);
        self.nodes[id.idx()].size = result;
        result
    }

    fn compute_uncached(
        &mut self,
        id: LayoutId,
        req: LayoutReq,
        measure: &mut impl FnMut(LayoutId, Option<f32>, Option<f32>, AvailSize) -> Size,
    ) -> Size {
        let node = &self.nodes[id.idx()];
        if node.children_len == 0 || node.measured {
            self.compute_leaf(id, req, measure)
        } else {
            self.compute_flex(id, req, measure)
        }
    }

    fn compute_leaf(
        &mut self,
        id: LayoutId,
        req: LayoutReq,
        measure: &mut impl FnMut(LayoutId, Option<f32>, Option<f32>, AvailSize) -> Size,
    ) -> Size {
        let style = self.nodes[id.idx()].style;
        let measured = self.nodes[id.idx()].measured;
        let padding = resolve_edges_or_zero(style.padding, req.parent_w);
        let pb = padding.add(style.border);
        let pb_w = pb.horiz();
        let pb_h = pb.vert();

        let (style_w, style_h, min_w, max_w, min_h, max_h) = match req.sizing_mode {
            SizingMode::InherentSize => (
                style.width.resolve(req.parent_w),
                style.height.resolve(req.parent_h),
                style.min_width.resolve(req.parent_w),
                style.max_width.resolve(req.parent_w),
                style.min_height.resolve(req.parent_h),
                style.max_height.resolve(req.parent_h),
            ),
            SizingMode::ContentSize => (None, None, None, None, None, None),
        };

        let floor_w = min_w.map_or(pb_w, |m| m.max(pb_w));
        let floor_h = min_h.map_or(pb_h, |m| m.max(pb_h));
        let node_w = req
            .known_w
            .or(style_w)
            .map(|w| clamp_dim(w, Some(floor_w), max_w));
        let node_h = req
            .known_h
            .or(style_h)
            .map(|h| clamp_dim(h, Some(floor_h), max_h));

        if measured && (node_w.is_none() || node_h.is_none() || req.run_mode == RunMode::PerformLayout)
        {
            let known_inner_w = node_w.map(|w| (w - pb_w).max(0.0) / self.scale);
            let known_inner_h = node_h.map(|h| (h - pb_h).max(0.0) / self.scale);
            let avail_inner = AvailSize {
                width: req
                    .avail_w
                    .maybe_set(node_w)
                    .maybe_min(max_w)
                    .maybe_max(min_w)
                    .maybe_sub(pb_w)
                    .to_logical(self.scale),
                height: req
                    .avail_h
                    .maybe_set(node_h)
                    .maybe_min(max_h)
                    .maybe_max(min_h)
                    .maybe_sub(pb_h)
                    .to_logical(self.scale),
            };
            let logical = measure(id, known_inner_w, known_inner_h, avail_inner);
            let mw = ceil_to_device_pixel(logical.width.max(0.0), self.scale);
            let mh = ceil_to_device_pixel(logical.height.max(0.0), self.scale);
            return size(
                node_w.unwrap_or_else(|| clamp_dim(mw + pb_w, Some(floor_w), max_w)),
                node_h.unwrap_or_else(|| clamp_dim(mh + pb_h, Some(floor_h), max_h)),
            );
        }

        size(
            node_w.unwrap_or_else(|| clamp_dim(pb_w, Some(floor_w), max_w)),
            node_h.unwrap_or_else(|| clamp_dim(pb_h, Some(floor_h), max_h)),
        )
    }

    fn compute_flex(
        &mut self,
        id: LayoutId,
        req: LayoutReq,
        measure: &mut impl FnMut(LayoutId, Option<f32>, Option<f32>, AvailSize) -> Size,
    ) -> Size {
        let style = self.nodes[id.idx()].style;
        let row = style.direction == FlexDirection::Row;
        let padding = resolve_edges_or_zero(style.padding, req.parent_w);
        let border = style.border;
        let pb = padding.add(border);
        let pb_main = edge_main_sum(pb, row);
        let pb_cross = edge_cross_sum(pb, row);

        let (style_w, style_h, min_w, max_w, min_h, max_h) = match req.sizing_mode {
            SizingMode::InherentSize => (
                style.width.resolve(req.parent_w),
                style.height.resolve(req.parent_h),
                style.min_width.resolve(req.parent_w),
                style.max_width.resolve(req.parent_w),
                style.min_height.resolve(req.parent_h),
                style.max_height.resolve(req.parent_h),
            ),
            SizingMode::ContentSize => (None, None, None, None, None, None),
        };

        let (style_main, style_cross) = if row { (style_w, style_h) } else { (style_h, style_w) };
        let (min_main, min_cross) = if row { (min_w, min_h) } else { (min_h, min_w) };
        let (max_main, max_cross) = if row { (max_w, max_h) } else { (max_h, max_w) };
        let (known_main, known_cross) = if row {
            (req.known_w, req.known_h)
        } else {
            (req.known_h, req.known_w)
        };
        let (avail_main_in, avail_cross_in) = if row {
            (req.avail_w, req.avail_h)
        } else {
            (req.avail_h, req.avail_w)
        };

        let floor_main = min_main.map_or(pb_main, |m| m.max(pb_main));
        let floor_cross = min_cross.map_or(pb_cross, |m| m.max(pb_cross));
        let node_main = known_main
            .or(style_main)
            .map(|v| clamp_dim(v, Some(floor_main), max_main));
        let node_cross = known_cross
            .or(style_cross)
            .map(|v| clamp_dim(v, Some(floor_cross), max_cross));

        let children = self.child_slice(id);
        let in_flow_count = children
            .iter()
            .filter(|&&c| self.nodes[c.idx()].style.position != Position::Absolute)
            .count();

        if in_flow_count == 0 {
            let final_main =
                node_main.unwrap_or_else(|| clamp_dim(pb_main, Some(floor_main), max_main));
            let final_cross =
                node_cross.unwrap_or_else(|| clamp_dim(pb_cross, Some(floor_cross), max_cross));
            let container_size = size_from(final_main, final_cross, row);
            if req.run_mode == RunMode::PerformLayout {
                self.layout_absolute_children(
                    children,
                    container_size,
                    border,
                    padding,
                    (row, style.justify_content, style.align_items),
                    measure,
                );
            }
            return container_size;
        }

        let inner_main_opt = node_main.map(|v| (v - pb_main).max(0.0));
        let inner_cross_opt = node_cross.map(|v| (v - pb_cross).max(0.0));
        let (inner_w_opt, inner_h_opt) = pair_from(inner_main_opt, inner_cross_opt, row);
        let gap_main = style.gap;

        let avail_main = avail_main_in
            .maybe_set(node_main)
            .maybe_min(max_main)
            .maybe_sub(pb_main);
        let avail_cross = avail_cross_in
            .maybe_set(node_cross)
            .maybe_min(max_cross)
            .maybe_sub(pb_cross);

        struct FlexItem {
            id: LayoutId,
            style: FlexStyle,
            margin: Edges<f32>,
            pb_main: f32,
            pb_cross: f32,
            align: Option<AlignItems>,
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
            if cs.position == Position::Absolute {
                continue;
            }
            let margin = resolve_edges_or_zero(cs.margin, inner_w_opt);
            let c_pb = resolve_edges_or_zero(cs.padding, inner_w_opt).add(cs.border);
            let c_pb_main = edge_main_sum(c_pb, row);
            let c_pb_cross = edge_cross_sum(c_pb, row);
            let align = cs.align_self.or(style.align_items);

            let c_size_main = cs.size_main(row).resolve(inner_main_opt);
            let c_size_cross = cs.size_cross(row).resolve(inner_cross_opt);
            let c_min_main = cs.min_main(row).resolve(inner_main_opt);
            let c_max_main = cs.max_main(row).resolve(inner_main_opt);
            let c_min_cross = cs.min_cross(row).resolve(inner_cross_opt);
            let c_max_cross = cs.max_cross(row).resolve(inner_cross_opt);
            let c_floor_cross = c_min_cross.map_or(c_pb_cross, |m| m.max(c_pb_cross));

            let pre_cross = if let Some(s) = c_size_cross {
                Some(clamp_dim(s, Some(c_floor_cross), c_max_cross))
            } else if align.is_none()
                && let Some(ic) = inner_cross_opt
            {
                Some(clamp_dim(
                    (ic - edge_cross_sum(margin, row)).max(0.0),
                    Some(c_floor_cross),
                    c_max_cross,
                ))
            } else {
                None
            };

            let explicit_basis = match cs.flex_basis {
                Dim::Auto => c_size_main,
                other => other.resolve(inner_main_opt),
            };
            let (flex_basis, basis_from_content) = match explicit_basis {
                Some(b) => (b, false),
                None => {
                    let probe_main = if avail_main == Avail::MinContent {
                        Avail::MinContent
                    } else {
                        Avail::MaxContent
                    };
                    let (kw, kh) = pair_from(None, pre_cross, row);
                    let (aw, ah) =
                        pair_from(probe_main, avail_cross.maybe_sub(edge_cross_sum(margin, row)), row);
                    let measured = self.compute_node(
                        child,
                        LayoutReq {
                            known_w: kw,
                            known_h: kh,
                            parent_w: inner_w_opt,
                            parent_h: inner_h_opt,
                            avail_w: aw,
                            avail_h: ah,
                            run_mode: RunMode::ComputeSize,
                            sizing_mode: SizingMode::InherentSize,
                        },
                        measure,
                    );
                    (main_of(measured, row), true)
                }
            };

            let effective_min_main = if let Some(min_m) = c_min_main {
                min_m.max(c_pb_main)
            } else if cs.overflow_main(row) == Overflow::Hidden
                || c_max_main == Some(0.0)
                || c_size_main.is_some_and(|s| s == 0.0 || (cs.flex_shrink == 0.0 && flex_basis >= s))
                || (cs.flex_shrink == 0.0 && basis_from_content)
            {
                c_pb_main
            } else {
                let (kw, kh) = pair_from(None, pre_cross, row);
                let (aw, ah) = pair_from(
                    Avail::MinContent,
                    avail_cross.maybe_sub(edge_cross_sum(margin, row)),
                    row,
                );
                let measured_min = self.compute_node(
                    child,
                    LayoutReq {
                        known_w: kw,
                        known_h: kh,
                        parent_w: inner_w_opt,
                        parent_h: inner_h_opt,
                        avail_w: aw,
                        avail_h: ah,
                        run_mode: RunMode::ComputeSize,
                        sizing_mode: SizingMode::ContentSize,
                    },
                    measure,
                );
                let mut auto_min = main_of(measured_min, row);
                if let Some(s) = c_size_main {
                    auto_min = auto_min.min(s);
                }
                if let Some(m) = c_max_main {
                    auto_min = auto_min.min(m);
                }
                auto_min.max(c_pb_main)
            };

            let effective_max_main = c_max_main.map(|m| m.max(effective_min_main));
            let hypothetical_main =
                clamp_dim(flex_basis, Some(effective_min_main), effective_max_main);

            items.push(FlexItem {
                id: child,
                style: cs,
                margin,
                pb_main: c_pb_main,
                pb_cross: c_pb_cross,
                align,
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
                .map(|it| it.hypothetical_main + edge_main_sum(it.margin, row))
                .sum::<f32>();

        let (container_main, resolve_flex) = match node_main {
            Some(m) => (m, true),
            None => {
                let raw = total_hypothetical + pb_main;
                let clamped = clamp_dim(raw, Some(floor_main), max_main);
                let constrained = (clamped - raw).abs() > 1e-4;
                (clamped, constrained)
            }
        };
        let inner_main = (container_main - pb_main).max(0.0);

        if resolve_flex {
            let growing = total_hypothetical < inner_main;
            for item in &mut items {
                let freeze = if growing {
                    item.style.flex_grow == 0.0 || item.flex_basis > item.hypothetical_main
                } else {
                    item.style.flex_shrink == 0.0 || item.flex_basis < item.hypothetical_main
                };
                item.frozen = freeze;
                item.target_main = if freeze {
                    item.hypothetical_main
                } else {
                    item.flex_basis
                };
            }

            let initial_free_space = inner_main
                - total_gaps
                - items
                    .iter()
                    .map(|it| {
                        (if it.frozen {
                            it.target_main
                        } else {
                            it.flex_basis
                        }) + edge_main_sum(it.margin, row)
                    })
                    .sum::<f32>();

            loop {
                let mut unfrozen_flex_sum = 0.0f32;
                let mut used = total_gaps;
                let mut any_unfrozen = false;
                for item in &items {
                    if item.frozen {
                        used += item.target_main + edge_main_sum(item.margin, row);
                    } else {
                        any_unfrozen = true;
                        used += item.flex_basis + edge_main_sum(item.margin, row);
                        unfrozen_flex_sum += if growing {
                            item.style.flex_grow
                        } else {
                            item.style.flex_shrink
                        };
                    }
                }
                if !any_unfrozen {
                    break;
                }

                let mut remaining_free = inner_main - used;
                if unfrozen_flex_sum < 1.0 {
                    let scaled = initial_free_space * unfrozen_flex_sum;
                    if scaled.abs() < remaining_free.abs() {
                        remaining_free = scaled;
                    }
                }

                if growing && unfrozen_flex_sum > 0.0 && remaining_free > 0.0 {
                    for item in &mut items {
                        if !item.frozen {
                            item.target_main = item.flex_basis
                                + remaining_free * (item.style.flex_grow / unfrozen_flex_sum);
                        }
                    }
                } else if !growing && remaining_free < 0.0 {
                    let mut scaled_sum: f32 = items
                        .iter()
                        .filter(|it| !it.frozen)
                        .map(|it| it.style.flex_shrink * (it.flex_basis - it.pb_main).max(0.0))
                        .sum();
                    let use_raw_basis = scaled_sum == 0.0;
                    if use_raw_basis {
                        scaled_sum = items
                            .iter()
                            .filter(|it| !it.frozen)
                            .map(|it| it.style.flex_shrink * it.flex_basis.max(1.0))
                            .sum();
                    }
                    if scaled_sum > 0.0 {
                        for item in &mut items {
                            if !item.frozen {
                                let factor = if use_raw_basis {
                                    item.style.flex_shrink * item.flex_basis.max(1.0)
                                } else {
                                    item.style.flex_shrink
                                        * (item.flex_basis - item.pb_main).max(0.0)
                                };
                                item.target_main =
                                    item.flex_basis + remaining_free * (factor / scaled_sum);
                            }
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
                        let delta = clamped - item.target_main;
                        item.target_main = clamped;
                        item.violation = delta;
                        total_violation += delta;
                    }
                }

                if total_violation.abs() < 1e-4 {
                    break;
                }
                let mut froze_any = false;
                for item in &mut items {
                    if !item.frozen
                        && ((total_violation > 0.0 && item.violation > 0.0)
                            || (total_violation < 0.0 && item.violation < 0.0))
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

        let (pw_with_main, ph_with_main) = pair_from(Some(inner_main), inner_cross_opt, row);
        let container_cross = match node_cross {
            Some(c) => c,
            None => {
                let mut max_outer_cross = 0.0f32;
                for item in &items {
                    let c_min_cross = item.style.min_cross(row).resolve(inner_cross_opt);
                    let c_max_cross = item.style.max_cross(row).resolve(inner_cross_opt);
                    let c_floor_cross = c_min_cross.map_or(item.pb_cross, |m| m.max(item.pb_cross));
                    let c_known_cross = item
                        .style
                        .size_cross(row)
                        .resolve(inner_cross_opt)
                        .map(|s| clamp_dim(s, Some(c_floor_cross), c_max_cross));
                    let (kw, kh) = pair_from(Some(item.target_main), c_known_cross, row);
                    let (aw, ah) = pair_from(
                        Avail::Definite(item.target_main),
                        avail_cross.maybe_sub(edge_cross_sum(item.margin, row)),
                        row,
                    );
                    let measured = self.compute_node(
                        item.id,
                        LayoutReq {
                            known_w: kw,
                            known_h: kh,
                            parent_w: pw_with_main,
                            parent_h: ph_with_main,
                            avail_w: aw,
                            avail_h: ah,
                            run_mode: RunMode::ComputeSize,
                            sizing_mode: SizingMode::InherentSize,
                        },
                        measure,
                    );
                    max_outer_cross = max_outer_cross
                        .max(cross_of(measured, row) + edge_cross_sum(item.margin, row));
                }
                clamp_dim(max_outer_cross + pb_cross, Some(floor_cross), max_cross)
            }
        };

        let container_size = size_from(container_main, container_cross, row);
        if req.run_mode == RunMode::ComputeSize {
            return container_size;
        }

        let inner_cross = (container_cross - pb_cross).max(0.0);
        let (final_pw, final_ph) = pair_from(Some(inner_main), Some(inner_cross), row);

        let mut final_sizes: Vec<Size> = Vec::with_capacity(items.len());
        let mut used_main = total_gaps;
        for item in &items {
            let c_min_cross = item.style.min_cross(row).resolve(Some(inner_cross));
            let c_max_cross = item.style.max_cross(row).resolve(Some(inner_cross));
            let c_floor_cross = c_min_cross.map_or(item.pb_cross, |m| m.max(item.pb_cross));

            let final_cross_known = if let Some(s) = item.style.size_cross(row).resolve(Some(inner_cross))
            {
                Some(clamp_dim(s, Some(c_floor_cross), c_max_cross))
            } else if item.align.is_none() {
                Some(clamp_dim(
                    (inner_cross - edge_cross_sum(item.margin, row)).max(0.0),
                    Some(c_floor_cross),
                    c_max_cross,
                ))
            } else {
                None
            };

            let (kw, kh) = pair_from(Some(item.target_main), final_cross_known, row);
            let (aw, ah) = pair_from(
                Avail::Definite(item.target_main),
                Avail::Definite((inner_cross - edge_cross_sum(item.margin, row)).max(0.0)),
                row,
            );
            let child_size = self.compute_node(
                item.id,
                LayoutReq {
                    known_w: kw,
                    known_h: kh,
                    parent_w: final_pw,
                    parent_h: final_ph,
                    avail_w: aw,
                    avail_h: ah,
                    run_mode: RunMode::PerformLayout,
                    sizing_mode: SizingMode::InherentSize,
                },
                measure,
            );
            used_main += main_of(child_size, row) + edge_main_sum(item.margin, row);
            final_sizes.push(child_size);
        }

        let main_offset = match style.justify_content {
            Some(JustifyContent::Center) => (inner_main - used_main) / 2.0,
            None => 0.0,
        };
        let mut main_cursor =
            edge_main_start(border, row) + edge_main_start(padding, row) + main_offset;
        let cross_origin = edge_cross_start(border, row) + edge_cross_start(padding, row);

        for (item, child_size) in items.iter().zip(final_sizes.iter()) {
            let c_main = main_of(*child_size, row);
            let c_cross = cross_of(*child_size, row);
            let main_pos = main_cursor + edge_main_start(item.margin, row);
            let cross_pos = match item.align {
                Some(AlignItems::Center) => {
                    cross_origin
                        + (inner_cross - (c_cross + edge_cross_sum(item.margin, row))) / 2.0
                        + edge_cross_start(item.margin, row)
                }
                Some(AlignItems::Start) | None => cross_origin + edge_cross_start(item.margin, row),
            };
            let rel = relative_inset_offset(item.style.inset, final_pw, final_ph);
            let base = point_from(main_pos, cross_pos, row);
            self.nodes[item.id.idx()].location = point(base.x + rel.x, base.y + rel.y);
            main_cursor += edge_main_start(item.margin, row)
                + c_main
                + edge_main_end(item.margin, row)
                + gap_main;
        }

        self.layout_absolute_children(
            children,
            container_size,
            border,
            padding,
            (row, style.justify_content, style.align_items),
            measure,
        );

        container_size
    }

    fn layout_absolute_children(
        &mut self,
        children: &[LayoutId],
        parent_size: Size,
        parent_border: Edges<f32>,
        parent_padding: Edges<f32>,
        flex_align: (bool, Option<JustifyContent>, Option<AlignItems>),
        measure: &mut impl FnMut(LayoutId, Option<f32>, Option<f32>, AvailSize) -> Size,
    ) {
        let cb_origin = point(parent_border.left, parent_border.top);
        let cb_w = (parent_size.width - parent_border.horiz()).max(0.0);
        let cb_h = (parent_size.height - parent_border.vert()).max(0.0);
        let content_w = (cb_w - parent_padding.horiz()).max(0.0);
        let content_h = (cb_h - parent_padding.vert()).max(0.0);
        let (row, justify, align_items) = flex_align;

        for &child in children {
            let cs = self.nodes[child.idx()].style;
            if cs.position != Position::Absolute {
                continue;
            }
            let inset_l = cs.inset.left.resolve(Some(cb_w));
            let inset_r = cs.inset.right.resolve(Some(cb_w));
            let inset_t = cs.inset.top.resolve(Some(cb_h));
            let inset_b = cs.inset.bottom.resolve(Some(cb_h));
            let m = resolve_edges_or_zero(cs.margin, Some(cb_w));
            let c_pb = resolve_edges_or_zero(cs.padding, Some(cb_w)).add(cs.border);

            let c_min_w = cs.min_width.resolve(Some(cb_w));
            let c_max_w = cs.max_width.resolve(Some(cb_w));
            let c_min_h = cs.min_height.resolve(Some(cb_h));
            let c_max_h = cs.max_height.resolve(Some(cb_h));
            let floor_w = c_min_w.map_or(c_pb.horiz(), |v| v.max(c_pb.horiz()));
            let floor_h = c_min_h.map_or(c_pb.vert(), |v| v.max(c_pb.vert()));

            let known_w = if let Some(w) = cs.width.resolve(Some(cb_w)) {
                Some(clamp_dim(w, Some(floor_w), c_max_w))
            } else if let (Some(l), Some(r)) = (inset_l, inset_r) {
                Some(clamp_dim((cb_w - l - r - m.horiz()).max(0.0), Some(floor_w), c_max_w))
            } else {
                None
            };
            let known_h = if let Some(h) = cs.height.resolve(Some(cb_h)) {
                Some(clamp_dim(h, Some(floor_h), c_max_h))
            } else if let (Some(t), Some(b)) = (inset_t, inset_b) {
                Some(clamp_dim((cb_h - t - b - m.vert()).max(0.0), Some(floor_h), c_max_h))
            } else {
                None
            };

            let avail_w = known_w
                .or(c_max_w)
                .map_or(Avail::MaxContent, Avail::Definite);
            let avail_h = known_h
                .or(c_max_h)
                .map_or(Avail::MaxContent, Avail::Definite);

            let child_size = self.compute_node(
                child,
                LayoutReq {
                    known_w,
                    known_h,
                    parent_w: Some(cb_w),
                    parent_h: Some(cb_h),
                    avail_w,
                    avail_h,
                    run_mode: RunMode::PerformLayout,
                    sizing_mode: SizingMode::InherentSize,
                },
                measure,
            );

            let align = cs.align_self.or(align_items);
            let main_off = match justify {
                Some(JustifyContent::Center) => {
                    let avail_m = if row { content_w } else { content_h };
                    let child_m = main_of(child_size, row) + edge_main_sum(m, row);
                    (avail_m - child_m) / 2.0
                }
                None => 0.0,
            };
            let cross_off = match align {
                Some(AlignItems::Center) => {
                    let avail_c = if row { content_h } else { content_w };
                    let child_c = cross_of(child_size, row) + edge_cross_sum(m, row);
                    (avail_c - child_c) / 2.0
                }
                Some(AlignItems::Start) | None => 0.0,
            };
            let (static_x, static_y) = if row {
                (main_off, cross_off)
            } else {
                (cross_off, main_off)
            };

            let x = if let Some(l) = inset_l {
                cb_origin.x + l + m.left
            } else if let Some(r) = inset_r {
                cb_origin.x + cb_w - r - m.right - child_size.width
            } else {
                cb_origin.x + parent_padding.left + static_x + m.left
            };
            let y = if let Some(t) = inset_t {
                cb_origin.y + t + m.top
            } else if let Some(b) = inset_b {
                cb_origin.y + cb_h - b - m.bottom - child_size.height
            } else {
                cb_origin.y + parent_padding.top + static_y + m.top
            };

            self.nodes[child.idx()].location = point(x, y);
        }
    }
}

#[inline]
fn clamp_dim(val: f32, min: Option<f32>, max: Option<f32>) -> f32 {
    let v = match max {
        Some(hi) => val.min(hi),
        None => val,
    };
    match min {
        Some(lo) => v.max(lo),
        None => v,
    }
}

#[inline]
fn main_of(s: Size, row: bool) -> f32 {
    if row { s.width } else { s.height }
}

#[inline]
fn cross_of(s: Size, row: bool) -> f32 {
    if row { s.height } else { s.width }
}

#[inline]
fn size_from(main: f32, cross: f32, row: bool) -> Size {
    if row {
        size(main, cross)
    } else {
        size(cross, main)
    }
}

#[inline]
fn pair_from<T>(main: T, cross: T, row: bool) -> (T, T) {
    if row { (main, cross) } else { (cross, main) }
}

#[inline]
fn point_from(main: f32, cross: f32, row: bool) -> Point {
    if row {
        point(main, cross)
    } else {
        point(cross, main)
    }
}

#[inline]
fn relative_inset_offset(inset: Edges<Dim>, parent_w: Option<f32>, parent_h: Option<f32>) -> Point {
    let x = if let Some(l) = inset.left.resolve(parent_w) {
        l
    } else if let Some(r) = inset.right.resolve(parent_w) {
        -r
    } else {
        0.0
    };
    let y = if let Some(t) = inset.top.resolve(parent_h) {
        t
    } else if let Some(b) = inset.bottom.resolve(parent_h) {
        -b
    } else {
        0.0
    };
    point(x, y)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::rect;

    fn st() -> FlexStyle {
        FlexStyle::default()
    }

    fn row_st() -> FlexStyle {
        FlexStyle {
            direction: FlexDirection::Row,
            ..Default::default()
        }
    }

    fn full() -> FlexStyle {
        FlexStyle {
            width: Dim::Pct(1.0),
            height: Dim::Pct(1.0),
            ..Default::default()
        }
    }

    fn flex_1(mut s: FlexStyle) -> FlexStyle {
        s.flex_grow = 1.0;
        s.flex_shrink = 1.0;
        s.flex_basis = Dim::Pct(0.0);
        s
    }

    fn flex_none(mut s: FlexStyle) -> FlexStyle {
        s.flex_grow = 0.0;
        s.flex_shrink = 0.0;
        s
    }

    fn no_measure(_: LayoutId, _: Option<f32>, _: Option<f32>, _: AvailSize) -> Size {
        Size::ZERO
    }

    #[test]
    fn flex_col_window_chrome_and_flex_1_editor() {
        let mut engine = LayoutEngine::new();
        let title = engine.add_node(flex_none(FlexStyle { height: Dim::Px(40.), ..st() }), 1.0, &[], false);
        let menu = engine.add_node(flex_none(FlexStyle { height: Dim::Px(32.), ..st() }), 1.0, &[], false);
        let editor = engine.add_node(flex_1(FlexStyle { min_height: Dim::Px(60.), ..st() }), 1.0, &[], false);
        let status = engine.add_node(flex_none(FlexStyle { height: Dim::Px(33.), ..st() }), 1.0, &[], false);
        let root = engine.add_node(full(), 1.0, &[title, menu, editor, status], false);
        engine.compute(root, AvailSize::definite(800., 600.), 1.0, &mut no_measure);

        assert_eq!(engine.bounds(title, 1.0), rect(0., 0., 800., 40.));
        assert_eq!(engine.bounds(menu, 1.0), rect(0., 40., 800., 32.));
        assert_eq!(engine.bounds(editor, 1.0), rect(0., 72., 800., 495.));
        assert_eq!(engine.bounds(status, 1.0), rect(0., 567., 800., 33.));
    }

    #[test]
    fn dialog_wrapped_text_and_equal_buttons() {
        let mut engine = LayoutEngine::new();
        let body_text = engine.add_node(st(), 1.0, &[], true);
        let content = engine.add_node(
            FlexStyle {
                padding: Edges::all(Dim::Px(24.)),
                border: Edges { bottom: 1., ..Edges::ZERO },
                ..st()
            },
            1.0,
            &[body_text],
            false,
        );
        let btn_st = flex_1(FlexStyle {
            min_width: Dim::Px(0.),
            height: Dim::Px(32.),
            ..st()
        });
        let b1 = engine.add_node(btn_st, 1.0, &[], false);
        let b2 = engine.add_node(btn_st, 1.0, &[], false);
        let footer = engine.add_node(
            FlexStyle {
                padding: Edges::all(Dim::Px(24.)),
                gap: 8.,
                ..row_st()
            },
            1.0,
            &[b1, b2],
            false,
        );
        let col = engine.add_node(st(), 1.0, &[content, footer], false);
        let frame = engine.add_node(
            FlexStyle {
                width: Dim::Px(360.),
                max_width: Dim::Pct(1.0),
                border: Edges::all(1.),
                ..st()
            },
            1.0,
            &[col],
            false,
        );
        let overlay = engine.add_node(
            FlexStyle {
                position: Position::Absolute,
                inset: Edges { top: Dim::Px(0.), left: Dim::Px(0.), ..Edges::all(Dim::Auto) },
                align_items: Some(AlignItems::Center),
                justify_content: Some(JustifyContent::Center),
                ..full()
            },
            1.0,
            &[frame],
            false,
        );
        let root = engine.add_node(full(), 1.0, &[overlay], false);
        engine.compute(root, AvailSize::definite(800., 600.), 1.0, &mut |id, kw, _, avail| {
            assert_eq!(id, body_text);
            let wrap = kw.or(match avail.width {
                AvailableSpace::Definite(w) => Some(w),
                _ => None,
            });
            match wrap {
                Some(w) if w < 500. => {
                    let lines = (500.0 / w).ceil().max(1.0);
                    size(w, lines * 20.)
                }
                _ => size(500., 20.),
            }
        });

        assert_eq!(engine.bounds(body_text, 1.0).size, size(310., 40.));
        assert_eq!(engine.bounds(content, 1.0).size, size(358., 89.));
        assert_eq!(engine.bounds(b1, 1.0).size, size(151., 32.));
        assert_eq!(engine.bounds(b2, 1.0).size, size(151., 32.));
        let fb = engine.bounds(frame, 1.0);
        assert_eq!(fb.size, size(360., 171.));
        assert_eq!(fb.origin.x, 220.);
    }

    #[test]
    fn tab_label_min_w_0_truncates_wide_nowrap_text() {
        let mut engine = LayoutEngine::new();
        let long_text = engine.add_node(st(), 1.0, &[], true);
        let label_box = engine.add_node(
            flex_1(FlexStyle {
                min_width: Dim::Px(0.),
                overflow_x: Overflow::Hidden,
                overflow_y: Overflow::Hidden,
                ..st()
            }),
            1.0,
            &[long_text],
            false,
        );
        let tab = engine.add_node(
            FlexStyle {
                align_items: Some(AlignItems::Center),
                width: Dim::Px(180.),
                height: Dim::Px(32.),
                padding: Edges { left: Dim::Px(12.), right: Dim::Px(36.), ..Edges::all(Dim::Px(0.)) },
                ..row_st()
            },
            1.0,
            &[label_box],
            false,
        );
        engine.compute(tab, AvailSize::definite(180., 32.), 1.0, &mut |_, _, _, _| {
            size(400., 16.)
        });

        assert_eq!(engine.bounds(label_box, 1.0), rect(12., 8., 132., 16.));
    }

    #[test]
    fn text_field_negative_insets_and_absolute_glyph() {
        let mut engine = LayoutEngine::new();
        let bottom_edge = engine.add_node(
            FlexStyle {
                position: Position::Absolute,
                inset: Edges {
                    left: Dim::Px(-1.),
                    right: Dim::Px(-1.),
                    bottom: Dim::Px(0.),
                    top: Dim::Auto,
                },
                height: Dim::Px(2.),
                ..st()
            },
            1.0,
            &[],
            false,
        );
        let glyph = engine.add_node(
            FlexStyle {
                position: Position::Absolute,
                inset: Edges {
                    top: Dim::Px(2.),
                    bottom: Dim::Px(2.),
                    right: Dim::Px(4.),
                    left: Dim::Auto,
                },
                width: Dim::Px(24.),
                ..st()
            },
            1.0,
            &[],
            false,
        );
        let field = engine.add_node(
            FlexStyle {
                width: Dim::Px(200.),
                height: Dim::Px(32.),
                border: Edges { top: 1., left: 1., right: 1., bottom: 0. },
                padding: Edges { bottom: Dim::Px(1.), ..Edges::all(Dim::Px(0.)) },
                ..st()
            },
            1.0,
            &[bottom_edge, glyph],
            false,
        );
        engine.compute(field, AvailSize::definite(200., 32.), 1.0, &mut no_measure);

        assert_eq!(engine.bounds(bottom_edge, 1.0), rect(0., 30., 200., 2.));
        assert_eq!(engine.bounds(glyph, 1.0), rect(171., 3., 24., 27.));
    }

    #[test]
    fn status_bar_min_content_root_measurement() {
        let mut engine = LayoutEngine::new();
        let t1 = engine.add_node(st(), 1.0, &[], true);
        let div1 = engine.add_node(flex_none(FlexStyle { width: Dim::Px(1.), height: Dim::Px(14.), ..st() }), 1.0, &[], false);
        let t2 = engine.add_node(st(), 1.0, &[], true);
        let spacer = engine.add_node(flex_1(FlexStyle { min_width: Dim::Px(1.), height: Dim::Px(14.), ..st() }), 1.0, &[], false);
        let t3 = engine.add_node(st(), 1.0, &[], true);
        let row = engine.add_node(
            FlexStyle {
                align_items: Some(AlignItems::Center),
                gap: 24.,
                height: Dim::Px(32.),
                padding: Edges::xy(Dim::Px(22.), Dim::Px(0.)),
                ..row_st()
            },
            1.0,
            &[t1, div1, t2, spacer, t3],
            false,
        );
        let sep = engine.add_node(FlexStyle { height: Dim::Px(1.), ..st() }, 1.0, &[], false);
        let status = engine.add_node(flex_none(st()), 1.0, &[sep, row], false);
        engine.compute(status, AvailSize::MIN, 1.0, &mut |id, _, _, _| {
            if id == t1 {
                size(60., 16.)
            } else if id == t2 {
                size(80., 16.)
            } else {
                size(40., 16.)
            }
        });

        assert_eq!(engine.bounds(status, 1.0).size, size(322., 33.));
    }

    #[test]
    fn menu_command_tile_self_start_text_center_and_relative_offset() {
        let mut engine = LayoutEngine::new();
        let icon = engine.add_node(st(), 1.0, &[], true);
        let icon_box = engine.add_node(FlexStyle { height: Dim::Px(16.), ..st() }, 1.0, &[icon], false);
        let label_text = engine.add_node(st(), 1.0, &[], true);
        let label_box = engine.add_node(
            FlexStyle {
                align_self: Some(AlignItems::Start),
                margin: Edges { left: Dim::Px(6.), top: Dim::Px(4.), ..Edges::all(Dim::Px(0.)) },
                width: Dim::Px(48.),
                height: Dim::Px(16.),
                ..st()
            },
            1.0,
            &[label_text],
            false,
        );
        let tile = engine.add_node(
            FlexStyle {
                align_items: Some(AlignItems::Center),
                width: Dim::Px(60.),
                height: Dim::Px(50.),
                padding: Edges { top: Dim::Px(8.), ..Edges::all(Dim::Px(0.)) },
                ..st()
            },
            1.0,
            &[icon_box, label_box],
            false,
        );
        let nudged = engine.add_node(
            FlexStyle {
                inset: Edges { left: Dim::Px(1.), top: Dim::Px(2.), ..Edges::all(Dim::Auto) },
                width: Dim::Px(10.),
                height: Dim::Px(10.),
                ..st()
            },
            1.0,
            &[],
            false,
        );
        let root = engine.add_node(row_st(), 1.0, &[tile, nudged], false);
        engine.compute(root, AvailSize::definite(200., 100.), 1.0, &mut |id, _, _, _| {
            if id == icon { size(16., 16.) } else { size(24., 14.) }
        });

        assert_eq!(engine.bounds(icon_box, 1.0).origin, point(22., 8.));
        assert_eq!(engine.bounds(label_box, 1.0), rect(6., 28., 48., 16.));
        assert_eq!(engine.bounds(label_text, 1.0).size.width, 48.);
        assert_eq!(engine.bounds(nudged, 1.0).origin, point(61., 2.));
    }

    #[test]
    fn fractional_dpi_edge_closure_between_flex_siblings() {
        let scale = 1.5;
        let mut engine = LayoutEngine::new();
        let child = flex_1(FlexStyle { height: Dim::Px(20.), ..st() });
        let a = engine.add_node(child, scale, &[], false);
        let b = engine.add_node(child, scale, &[], false);
        let c = engine.add_node(child, scale, &[], false);
        let root = engine.add_node(
            FlexStyle { width: Dim::Px(100.), height: Dim::Px(20.), ..row_st() },
            scale,
            &[a, b, c],
            false,
        );
        engine.compute(root, AvailSize::definite(100., 20.), scale, &mut no_measure);

        let ba = engine.bounds(a, scale);
        let bb = engine.bounds(b, scale);
        let bc = engine.bounds(c, scale);
        assert_eq!(ba.right(), bb.left());
        assert_eq!(bb.right(), bc.left());
        assert_eq!(bc.right(), 100.);
    }

    #[test]
    fn editor_percentage_chain_through_flex_1() {
        let mut engine = LayoutEngine::new();
        let text_el = engine.add_node(FlexStyle { min_height: Dim::Px(40.), ..full() }, 1.0, &[], false);
        let scrollbar = engine.add_node(FlexStyle { position: Position::Absolute, ..full() }, 1.0, &[], false);
        let editor_state = engine.add_node(
            flex_1(FlexStyle {
                height: Dim::Pct(1.0),
                padding: Edges {
                    top: Dim::Px(10.),
                    right: Dim::Px(12.),
                    bottom: Dim::Px(10.),
                    left: Dim::Px(14.),
                },
                ..st()
            }),
            1.0,
            &[text_el, scrollbar],
            false,
        );
        let inner = engine.add_node(full(), 1.0, &[editor_state], false);
        let editor_area = engine.add_node(flex_1(FlexStyle { min_height: Dim::Px(60.), ..st() }), 1.0, &[inner], false);
        let header = engine.add_node(flex_none(FlexStyle { height: Dim::Px(100.), ..st() }), 1.0, &[], false);
        let root = engine.add_node(full(), 1.0, &[header, editor_area], false);
        engine.compute(root, AvailSize::definite(800., 500.), 1.0, &mut no_measure);

        assert_eq!(engine.bounds(text_el, 1.0), rect(14., 110., 774., 380.));
        assert_eq!(engine.bounds(scrollbar, 1.0).size, size(800., 400.));
    }

    #[test]
    fn flex_shrink_respects_flex_none_and_min_size() {
        let mut engine = LayoutEngine::new();
        let fixed = engine.add_node(flex_none(FlexStyle { width: Dim::Px(100.), height: Dim::Px(20.), ..st() }), 1.0, &[], false);
        let a = engine.add_node(FlexStyle { width: Dim::Px(200.), min_width: Dim::Px(80.), height: Dim::Px(20.), ..st() }, 1.0, &[], false);
        let b = engine.add_node(FlexStyle { width: Dim::Px(200.), min_width: Dim::Px(0.), height: Dim::Px(20.), ..st() }, 1.0, &[], false);
        let root = engine.add_node(FlexStyle { width: Dim::Px(220.), ..row_st() }, 1.0, &[fixed, a, b], false);
        engine.compute(root, AvailSize::definite(220., 20.), 1.0, &mut no_measure);

        assert_eq!(engine.bounds(fixed, 1.0).size.width, 100.);
        assert_eq!(engine.bounds(a, 1.0).size.width, 80.);
        assert_eq!(engine.bounds(b, 1.0).size.width, 40.);
    }

    #[test]
    fn abspos_dropdown_inside_narrow_button_does_not_squish() {
        let mut engine = LayoutEngine::new();
        let item = engine.add_node(st(), 1.0, &[], true);
        let dropdown = engine.add_node(
            FlexStyle {
                position: Position::Absolute,
                inset: Edges { left: Dim::Px(0.), top: Dim::Px(32.), ..Edges::all(Dim::Auto) },
                ..st()
            },
            1.0,
            &[item],
            false,
        );
        let button = engine.add_node(FlexStyle { width: Dim::Px(40.), height: Dim::Px(32.), ..st() }, 1.0, &[dropdown], false);
        engine.compute(button, AvailSize::definite(40., 32.), 1.0, &mut |_, _, _, _| {
            size(180., 28.)
        });

        assert_eq!(engine.bounds(dropdown, 1.0), rect(0., 32., 180., 28.));
    }

    #[test]
    fn auto_height_flex_col_does_not_crush_in_short_window_and_respects_max_w_full() {
        let mut engine = LayoutEngine::new();
        let content = engine.add_node(FlexStyle { height: Dim::Px(120.), ..st() }, 1.0, &[], false);
        let footer = engine.add_node(FlexStyle { height: Dim::Px(80.), ..st() }, 1.0, &[], false);
        let col = engine.add_node(st(), 1.0, &[content, footer], false);
        let frame = engine.add_node(
            FlexStyle {
                width: Dim::Px(360.),
                max_width: Dim::Pct(1.0),
                border: Edges::all(1.),
                ..st()
            },
            1.0,
            &[col],
            false,
        );
        let overlay = engine.add_node(
            FlexStyle {
                position: Position::Absolute,
                inset: Edges { top: Dim::Px(0.), left: Dim::Px(0.), ..Edges::all(Dim::Auto) },
                align_items: Some(AlignItems::Center),
                justify_content: Some(JustifyContent::Center),
                ..full()
            },
            1.0,
            &[frame],
            false,
        );
        let root = engine.add_node(full(), 1.0, &[overlay], false);
        engine.compute(root, AvailSize::definite(250., 140.), 1.0, &mut no_measure);

        assert_eq!(engine.bounds(frame, 1.0).size, size(250., 202.));
        assert_eq!(engine.bounds(content, 1.0).size, size(248., 120.));
        assert_eq!(engine.bounds(footer, 1.0).size, size(248., 80.));
    }

    #[test]
    fn single_line_text_field_min_h_full_centers_text_element() {
        let mut engine = LayoutEngine::new();
        let text_el = engine.add_node(FlexStyle { width: Dim::Pct(1.0), height: Dim::Px(20.), ..st() }, 1.0, &[], false);
        let input_state = engine.add_node(
            flex_1(FlexStyle {
                min_height: Dim::Pct(1.0),
                align_items: Some(AlignItems::Center),
                overflow_x: Overflow::Hidden,
                ..row_st()
            }),
            1.0,
            &[text_el],
            false,
        );
        let inner = engine.add_node(
            FlexStyle {
                height: Dim::Pct(1.0),
                align_items: Some(AlignItems::Center),
                padding: Edges { left: Dim::Px(9.), right: Dim::Px(33.), ..Edges::all(Dim::Px(0.)) },
                ..row_st()
            },
            1.0,
            &[input_state],
            false,
        );
        let field = engine.add_node(
            FlexStyle {
                width: Dim::Px(200.),
                height: Dim::Px(32.),
                border: Edges { top: 1., left: 1., right: 1., bottom: 0. },
                padding: Edges { bottom: Dim::Px(1.), ..Edges::all(Dim::Px(0.)) },
                ..st()
            },
            1.0,
            &[inner],
            false,
        );
        engine.compute(field, AvailSize::definite(200., 32.), 1.0, &mut no_measure);

        assert_eq!(engine.bounds(input_state, 1.0), rect(10., 1., 156., 30.));
        assert_eq!(engine.bounds(text_el, 1.0), rect(10., 6., 156., 20.));
    }

    #[test]
    fn directwrite_status_bar_shapes_all_labels() {
        use crate::gfx::{TextAlign, TextSystem, font};
        use crate::types::SharedStr;

        let ts = TextSystem::new();
        let f = font("Segoe UI Variable Small");
        let labels = ["Ln 1, Col 1", "0 characters", "100%", "Windows (CRLF)", "UTF-8"];

        let build = |engine: &mut LayoutEngine, scale: f32| {
            let t0 = engine.add_node(st(), scale, &[], true);
            let d1 = engine.add_node(flex_none(FlexStyle { width: Dim::Px(1.), height: Dim::Px(16.), ..st() }), scale, &[], false);
            let t2 = engine.add_node(st(), scale, &[], true);
            let sp = engine.add_node(flex_1(FlexStyle { min_width: Dim::Px(1.), height: Dim::Px(16.), ..st() }), scale, &[], false);
            let t4 = engine.add_node(st(), scale, &[], true);
            let d5 = engine.add_node(flex_none(FlexStyle { width: Dim::Px(1.), height: Dim::Px(16.), ..st() }), scale, &[], false);
            let t6 = engine.add_node(st(), scale, &[], true);
            let d7 = engine.add_node(flex_none(FlexStyle { width: Dim::Px(1.), height: Dim::Px(16.), ..st() }), scale, &[], false);
            let t8 = engine.add_node(st(), scale, &[], true);
            let row = engine.add_node(
                FlexStyle {
                    align_items: Some(AlignItems::Center),
                    gap: 24.,
                    height: Dim::Px(31.),
                    padding: Edges::xy(Dim::Px(22.), Dim::Px(0.)),
                    ..row_st()
                },
                scale,
                &[t0, d1, t2, sp, t4, d5, t6, d7, t8],
                false,
            );
            let sep = engine.add_node(flex_none(FlexStyle { height: Dim::Px(1.), ..st() }), scale, &[], false);
            let bar = engine.add_node(flex_none(st()), scale, &[sep, row], false);
            (bar, [t0, t2, t4, t6, t8], sp, d5)
        };

        let measure_labels = |ids: [LayoutId; 5]| {
            let ts = &ts;
            let f = &f;
            move |id: LayoutId, kw: Option<f32>, _: Option<f32>, avail: AvailSize| {
                let ix = ids.iter().position(|&x| x == id).unwrap();
                let wrap_width = kw.or(match avail.width {
                    AvailableSpace::Definite(x) => Some(x),
                    _ => None,
                });
                let lines = ts.shape_wrapped(
                    SharedStr::new_static(labels[ix]),
                    12.0,
                    f,
                    wrap_width,
                    None,
                    None,
                    TextAlign::Left,
                );
                let mut sz = Size::ZERO;
                for line in &lines {
                    let ls = line.size(16.0);
                    sz.height += ls.height;
                    sz.width = sz.width.max(ls.width).ceil();
                }
                sz
            }
        };

        let mut probe = LayoutEngine::new();
        let (bar_probe, probe_ids, _, _) = build(&mut probe, 1.0);
        probe.compute(bar_probe, AvailSize::MIN, 1.0, &mut measure_labels(probe_ids));
        let min_sz = probe.bounds(bar_probe, 1.0).size;

        let mut win = LayoutEngine::new();
        let (bar_win, win_ids, sp, d5) = build(&mut win, 1.0);
        let root = win.add_node(full(), 1.0, &[bar_win], false);
        win.compute(root, AvailSize::definite(min_sz.width, 200.), 1.0, &mut measure_labels(win_ids));

        let spacer = win.bounds(sp, 1.0);
        let zoom = win.bounds(win_ids[2], 1.0);
        let next_div = win.bounds(d5, 1.0);
        assert!(spacer.size.width >= 1.0);
        assert!(zoom.size.height <= 16.0, "zoom must stay single-line, got {:?}", zoom);
        assert!(zoom.right() < next_div.left());
    }
}
