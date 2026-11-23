//! Native CSS Border-Box Block & Single-Line Flexbox layout engine for Jot.
//!
//! Replaces `taffy` with a compact arena-backed engine covering the exact CSS
//! layout subset Jot uses: `Display::Block`, single-line `Display::Flex`
//! (`Row` / `Column`), `Position::Relative` / `Position::Absolute`, `inset`,
//! `margin`, `padding`, `border`, `gap`, `size` / `min_size` / `max_size`,
//! `flex_grow` / `flex_shrink` / `flex_basis`, `overflow: hidden` automatic
//! min-size suppression, `align_items` / `align_self` / `justify_content`,
//! measured leaf nodes (`TextLayout`), and two-phase device-pixel snapping.

use std::fmt::Debug;

use crate::ui::smallvec::SmallVec;
use crate::ui::{
    AbsoluteLength, AlignItems, AlignSelf, App, Bounds, DefiniteLength, Display, Edges,
    FlexDirection, JustifyContent, Length, Overflow, Pixels, Point, Position, Size, Style, Window,
    size,
    util::{
        ceil_to_device_pixel, round_half_toward_zero, round_stroke_to_device_pixel,
        round_to_device_pixel,
    },
};

type MeasureFn =
    dyn FnMut(Size<Option<Pixels>>, Size<AvailableSpace>, &mut Window, &mut App) -> Size<Pixels>;

enum MeasureSlot {
    App(Box<MeasureFn>),
    #[cfg(test)]
    Pure(Box<dyn FnMut(Size<Option<Pixels>>, Size<AvailableSpace>) -> Size<Pixels>>),
}

enum MeasureEnv<'a> {
    App(&'a mut Window, &'a mut App),
    #[cfg(test)]
    Pure,
}

/// Unique dense handle for a layout node within the current frame's [`LayoutEngine`].
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub(crate) struct LayoutId(u32);

impl LayoutId {
    #[inline]
    fn idx(self) -> usize {
        self.0 as usize
    }
}

/// The space available for an element to be laid out in (logical pixels).
#[derive(Copy, Clone, Default, Debug, Eq, PartialEq)]
pub(crate) enum AvailableSpace {
    Definite(Pixels),
    #[default]
    MinContent,
    MaxContent,
}

impl AvailableSpace {
    pub(crate) const fn min_size() -> Size<Self> {
        Size {
            width: Self::MinContent,
            height: Self::MinContent,
        }
    }
}

impl From<Pixels> for AvailableSpace {
    fn from(pixels: Pixels) -> Self {
        AvailableSpace::Definite(pixels)
    }
}

impl From<Size<Pixels>> for Size<AvailableSpace> {
    fn from(size: Size<Pixels>) -> Self {
        Size {
            width: AvailableSpace::Definite(size.width),
            height: AvailableSpace::Definite(size.height),
        }
    }
}

/// Device-pixel dimension resolved from [`Length`] / [`DefiniteLength`] during `request_layout`.
#[derive(Copy, Clone, Debug, PartialEq)]
enum Dim {
    Auto,
    Length(f32),
    Percent(f32),
}

impl Dim {
    #[inline]
    fn from_length(len: Length, rem_size: Pixels, scale_factor: f32) -> Self {
        match len {
            Length::Auto => Dim::Auto,
            Length::Definite(d) => Self::from_definite(d, rem_size, scale_factor),
        }
    }

    #[inline]
    fn from_definite(len: DefiniteLength, rem_size: Pixels, scale_factor: f32) -> Self {
        match len {
            DefiniteLength::Absolute(a) => {
                Dim::Length(round_to_device_pixel(a.to_pixels(rem_size).0, scale_factor))
            }
            DefiniteLength::Fraction(f) => Dim::Percent(f),
        }
    }

    #[inline]
    fn is_auto(self) -> bool {
        matches!(self, Dim::Auto)
    }

    #[inline]
    fn resolve(self, base: Option<f32>) -> Option<f32> {
        match self {
            Dim::Auto => None,
            Dim::Length(v) => Some(v),
            Dim::Percent(p) => base.map(|b| b * p),
        }
    }

    #[inline]
    fn resolve_or_zero(self, base: Option<f32>) -> f32 {
        self.resolve(base).unwrap_or(0.0)
    }
}

#[derive(Copy, Clone, Debug)]
struct EdgeDim {
    top: Dim,
    right: Dim,
    bottom: Dim,
    left: Dim,
}

impl EdgeDim {
    fn from_lengths(edges: &Edges<Length>, rem_size: Pixels, scale_factor: f32) -> Self {
        Self {
            top: Dim::from_length(edges.top, rem_size, scale_factor),
            right: Dim::from_length(edges.right, rem_size, scale_factor),
            bottom: Dim::from_length(edges.bottom, rem_size, scale_factor),
            left: Dim::from_length(edges.left, rem_size, scale_factor),
        }
    }

    fn from_definites(edges: &Edges<DefiniteLength>, rem_size: Pixels, scale_factor: f32) -> Self {
        Self {
            top: Dim::from_definite(edges.top, rem_size, scale_factor),
            right: Dim::from_definite(edges.right, rem_size, scale_factor),
            bottom: Dim::from_definite(edges.bottom, rem_size, scale_factor),
            left: Dim::from_definite(edges.left, rem_size, scale_factor),
        }
    }

    #[inline]
    fn resolve_or_zero(self, base: Option<f32>) -> EdgeF32 {
        EdgeF32 {
            top: self.top.resolve_or_zero(base),
            right: self.right.resolve_or_zero(base),
            bottom: self.bottom.resolve_or_zero(base),
            left: self.left.resolve_or_zero(base),
        }
    }
}

#[derive(Copy, Clone, Debug, Default)]
struct EdgeF32 {
    top: f32,
    right: f32,
    bottom: f32,
    left: f32,
}

impl EdgeF32 {
    fn from_borders(edges: &Edges<AbsoluteLength>, rem_size: Pixels, scale_factor: f32) -> Self {
        let snap = |w: AbsoluteLength| {
            round_stroke_to_device_pixel(w.to_pixels(rem_size).0, scale_factor)
        };
        Self {
            top: snap(edges.top),
            right: snap(edges.right),
            bottom: snap(edges.bottom),
            left: snap(edges.left),
        }
    }

    #[inline]
    fn horiz(self) -> f32 {
        self.left + self.right
    }

    #[inline]
    fn vert(self) -> f32 {
        self.top + self.bottom
    }

    #[inline]
    fn main_start(self, row: bool) -> f32 {
        if row { self.left } else { self.top }
    }

    #[inline]
    fn main_end(self, row: bool) -> f32 {
        if row { self.right } else { self.bottom }
    }

    #[inline]
    fn main_sum(self, row: bool) -> f32 {
        if row { self.horiz() } else { self.vert() }
    }

    #[inline]
    fn cross_start(self, row: bool) -> f32 {
        if row { self.top } else { self.left }
    }

    #[inline]
    fn cross_sum(self, row: bool) -> f32 {
        if row { self.vert() } else { self.horiz() }
    }

    #[inline]
    fn add(self, other: Self) -> Self {
        Self {
            top: self.top + other.top,
            right: self.right + other.right,
            bottom: self.bottom + other.bottom,
            left: self.left + other.left,
        }
    }
}

#[derive(Copy, Clone, Debug)]
struct SizeDim {
    width: Dim,
    height: Dim,
}

impl SizeDim {
    fn from_lengths(s: &Size<Length>, rem_size: Pixels, scale_factor: f32) -> Self {
        Self {
            width: Dim::from_length(s.width, rem_size, scale_factor),
            height: Dim::from_length(s.height, rem_size, scale_factor),
        }
    }

    fn from_definites(s: &Size<DefiniteLength>, rem_size: Pixels, scale_factor: f32) -> Self {
        Self {
            width: Dim::from_definite(s.width, rem_size, scale_factor),
            height: Dim::from_definite(s.height, rem_size, scale_factor),
        }
    }

    #[inline]
    fn main(self, row: bool) -> Dim {
        if row { self.width } else { self.height }
    }

    #[inline]
    fn cross(self, row: bool) -> Dim {
        if row { self.height } else { self.width }
    }
}

#[derive(Copy, Clone, Debug)]
struct NodeStyle {
    display: Display,
    position: Position,
    overflow_x: Overflow,
    overflow_y: Overflow,
    inset: EdgeDim,
    size: SizeDim,
    min_size: SizeDim,
    max_size: SizeDim,
    margin: EdgeDim,
    padding: EdgeDim,
    border: EdgeF32,
    gap: SizeDim,
    align_items: Option<AlignItems>,
    align_self: Option<AlignSelf>,
    justify_content: Option<JustifyContent>,
    flex_direction: FlexDirection,
    flex_basis: Dim,
    flex_grow: f32,
    flex_shrink: f32,
}

impl NodeStyle {
    fn from_style(style: &Style, rem_size: Pixels, scale_factor: f32) -> Self {
        Self {
            display: style.display,
            position: style.position,
            overflow_x: style.overflow.x,
            overflow_y: style.overflow.y,
            inset: EdgeDim::from_lengths(&style.inset, rem_size, scale_factor),
            size: SizeDim::from_lengths(&style.size, rem_size, scale_factor),
            min_size: SizeDim::from_lengths(&style.min_size, rem_size, scale_factor),
            max_size: SizeDim::from_lengths(&style.max_size, rem_size, scale_factor),
            margin: EdgeDim::from_lengths(&style.margin, rem_size, scale_factor),
            padding: EdgeDim::from_definites(&style.padding, rem_size, scale_factor),
            border: EdgeF32::from_borders(&style.border_widths, rem_size, scale_factor),
            gap: SizeDim::from_definites(&style.gap, rem_size, scale_factor),
            align_items: style.align_items,
            align_self: style.align_self,
            justify_content: style.justify_content,
            flex_direction: style.flex_direction,
            flex_basis: Dim::from_length(style.flex_basis, rem_size, scale_factor),
            flex_grow: style.flex_grow,
            flex_shrink: style.flex_shrink,
        }
    }

    #[inline]
    fn overflow_main(&self, row: bool) -> Overflow {
        if row { self.overflow_x } else { self.overflow_y }
    }
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
    fn from_logical(space: AvailableSpace, scale_factor: f32) -> Self {
        match space {
            AvailableSpace::Definite(p) => Avail::Definite(p.0 * scale_factor),
            AvailableSpace::MinContent => Avail::MinContent,
            AvailableSpace::MaxContent => Avail::MaxContent,
        }
    }

    #[inline]
    fn to_logical(self, scale_factor: f32) -> AvailableSpace {
        match self {
            Avail::Definite(v) => AvailableSpace::Definite(Pixels(v / scale_factor)),
            Avail::MinContent => AvailableSpace::MinContent,
            Avail::MaxContent => AvailableSpace::MaxContent,
        }
    }

    #[inline]
    fn maybe_sub(self, delta: f32) -> Self {
        match self {
            Avail::Definite(v) => Avail::Definite((v - delta).max(0.0)),
            other => other,
        }
    }

    #[inline]
    fn maybe_min(self, limit: Option<f32>) -> Self {
        match (self, limit) {
            (Avail::Definite(a), Some(m)) => Avail::Definite(a.min(m)),
            (Avail::MaxContent, Some(m)) => Avail::Definite(m),
            _ => self,
        }
    }

    #[inline]
    fn maybe_max(self, floor: Option<f32>) -> Self {
        match (self, floor) {
            (Avail::Definite(a), Some(m)) => Avail::Definite(a.max(m)),
            _ => self,
        }
    }

    #[inline]
    fn maybe_set(self, known: Option<f32>) -> Self {
        match known {
            Some(v) => Avail::Definite(v),
            None => self,
        }
    }

    #[inline]
    fn to_key(self) -> (u8, u32) {
        match self {
            Avail::Definite(v) => (0, v.to_bits()),
            Avail::MinContent => (1, 0),
            Avail::MaxContent => (2, 0),
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
        None => u32::MAX, // canonical NaN sentinel, never collides with valid finite f32
    }
}

#[derive(Default)]
struct SizeCache {
    entries: [Option<(SizeCacheKey, Size<f32>)>; 4],
    next: usize,
}

impl SizeCache {
    fn get(&self, key: SizeCacheKey) -> Option<Size<f32>> {
        for (k, v) in self.entries.iter().flatten() {
            if *k == key {
                return Some(*v);
            }
        }
        None
    }

    fn insert(&mut self, key: SizeCacheKey, value: Size<f32>) {
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

    fn clear(&mut self) {
        self.entries = [None; 4];
        self.next = 0;
    }
}

struct Node {
    style: NodeStyle,
    parent: Option<LayoutId>,
    children_start: u32,
    children_len: u32,
    measure: Option<MeasureSlot>,
    location: Point<f32>,
    size: Size<f32>,
    abs_origin: Option<Point<f32>>,
    abs_bounds: Option<Bounds<Pixels>>,
    computed: bool,
    cache: SizeCache,
}

pub(crate) struct LayoutEngine {
    nodes: Vec<Node>,
    children_arena: Vec<LayoutId>,
    invalidate_scratch: Vec<LayoutId>,
}

impl LayoutEngine {
    pub(crate) fn new() -> Self {
        Self {
            nodes: Vec::new(),
            children_arena: Vec::new(),
            invalidate_scratch: Vec::new(),
        }
    }

    pub(crate) fn clear(&mut self) {
        self.nodes.clear();
        self.children_arena.clear();
    }

    pub(crate) fn request_layout(
        &mut self,
        style: Style,
        rem_size: Pixels,
        scale_factor: f32,
        children: &[LayoutId],
    ) -> LayoutId {
        let id = LayoutId(self.nodes.len() as u32);
        let children_start = self.children_arena.len() as u32;
        let children_len = children.len() as u32;
        self.children_arena.extend_from_slice(children);
        for &child in children {
            self.nodes[child.idx()].parent = Some(id);
        }
        self.nodes.push(Node {
            style: NodeStyle::from_style(&style, rem_size, scale_factor),
            parent: None,
            children_start,
            children_len,
            measure: None,
            location: Point::default(),
            size: Size::default(),
            abs_origin: None,
            abs_bounds: None,
            computed: false,
            cache: SizeCache::default(),
        });
        id
    }

    pub(crate) fn request_measured_layout(
        &mut self,
        style: Style,
        rem_size: Pixels,
        scale_factor: f32,
        measure: impl FnMut(
            Size<Option<Pixels>>,
            Size<AvailableSpace>,
            &mut Window,
            &mut App,
        ) -> Size<Pixels>
        + 'static,
    ) -> LayoutId {
        let id = LayoutId(self.nodes.len() as u32);
        self.nodes.push(Node {
            style: NodeStyle::from_style(&style, rem_size, scale_factor),
            parent: None,
            children_start: 0,
            children_len: 0,
            measure: Some(MeasureSlot::App(Box::new(measure))),
            location: Point::default(),
            size: Size::default(),
            abs_origin: None,
            abs_bounds: None,
            computed: false,
            cache: SizeCache::default(),
        });
        id
    }

    /// Treats any `auto` dimension of the given node's style as filling `size`.
    pub(crate) fn stretch_auto_size_to_fill(
        &mut self,
        id: LayoutId,
        size: Size<Pixels>,
        scale_factor: f32,
    ) {
        let style = &mut self.nodes[id.idx()].style;
        if style.size.width.is_auto() {
            style.size.width = Dim::Length(round_to_device_pixel(size.width.0, scale_factor));
        }
        if style.size.height.is_auto() {
            style.size.height = Dim::Length(round_to_device_pixel(size.height.0, scale_factor));
        }
    }

    pub(crate) fn compute_layout(
        &mut self,
        id: LayoutId,
        available_space: Size<AvailableSpace>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let scale_factor = window.scale_factor();
        let mut env = MeasureEnv::App(window, cx);
        self.compute_root(id, available_space, scale_factor, &mut env);
    }

    fn compute_root(
        &mut self,
        id: LayoutId,
        available_space: Size<AvailableSpace>,
        scale_factor: f32,
        env: &mut MeasureEnv<'_>,
    ) {
        if self.nodes[id.idx()].computed {
            self.invalidate_scratch.clear();
            self.invalidate_scratch.push(id);
            while let Some(cur) = self.invalidate_scratch.pop() {
                let node = &mut self.nodes[cur.idx()];
                node.abs_origin = None;
                node.abs_bounds = None;
                node.cache.clear();
                let start = node.children_start as usize;
                let end = start + node.children_len as usize;
                self.invalidate_scratch
                    .extend_from_slice(&self.children_arena[start..end]);
            }
        }
        self.nodes[id.idx()].computed = true;

        let avail = Size {
            width: Avail::from_logical(available_space.width, scale_factor),
            height: Avail::from_logical(available_space.height, scale_factor),
        };
        let parent_size = Size {
            width: match avail.width {
                Avail::Definite(w) => Some(w),
                _ => None,
            },
            height: match avail.height {
                Avail::Definite(h) => Some(h),
                _ => None,
            },
        };
        let root_size = self.compute_node(
            id,
            Size {
                width: None,
                height: None,
            },
            parent_size,
            avail,
            RunMode::PerformLayout,
            SizingMode::InherentSize,
            scale_factor,
            env,
        );
        let root = &mut self.nodes[id.idx()];
        root.location = Point::default();
        root.size = root_size;
    }

    // Two-phase device-pixel snapping:
    // 1. Pre-layout: authored lengths & borders are snapped in `NodeStyle::from_style`,
    //    and measured leaf sizes are ceiled in `snap_measured_size_to_device_pixels`.
    // 2. Post-layout: each element's absolute border-box edges are snapped via
    //    `round_half_toward_zero` in device-pixel space for global edge closure.
    pub(crate) fn layout_bounds(&mut self, id: LayoutId, scale_factor: f32) -> Bounds<Pixels> {
        if let Some(bounds) = self.nodes[id.idx()].abs_bounds {
            return bounds;
        }

        let parent = self.nodes[id.idx()].parent;
        let location = self.nodes[id.idx()].location;
        let node_size = self.nodes[id.idx()].size;

        let abs_origin = match parent {
            Some(parent_id) => {
                self.layout_bounds(parent_id, scale_factor);
                let parent_origin = self.nodes[parent_id.idx()]
                    .abs_origin
                    .expect("parent absolute outer origin should be cached");
                parent_origin + location
            }
            None => location,
        };
        self.nodes[id.idx()].abs_origin = Some(abs_origin);

        let abs_far = abs_origin + Point::new(node_size.width, node_size.height);
        let snapped = Bounds::from_corners(
            abs_origin.map(round_half_toward_zero),
            abs_far.map(round_half_toward_zero),
        );
        let bounds = (snapped / scale_factor).map(Pixels);
        self.nodes[id.idx()].abs_bounds = Some(bounds);
        bounds
    }

    fn compute_node(
        &mut self,
        id: LayoutId,
        known: Size<Option<f32>>,
        parent_size: Size<Option<f32>>,
        avail: Size<Avail>,
        run_mode: RunMode,
        sizing_mode: SizingMode,
        scale_factor: f32,
        env: &mut MeasureEnv<'_>,
    ) -> Size<f32> {
        if run_mode == RunMode::ComputeSize {
            let key = SizeCacheKey {
                known_w: opt_f32_bits(known.width),
                known_h: opt_f32_bits(known.height),
                avail_w: avail.width.to_key(),
                avail_h: avail.height.to_key(),
                parent_w: opt_f32_bits(parent_size.width),
                parent_h: opt_f32_bits(parent_size.height),
                sizing_mode,
            };
            if let Some(cached) = self.nodes[id.idx()].cache.get(key) {
                return cached;
            }
            let result = self.compute_node_uncached(
                id,
                known,
                parent_size,
                avail,
                run_mode,
                sizing_mode,
                scale_factor,
                env,
            );
            self.nodes[id.idx()].cache.insert(key, result);
            return result;
        }

        let result = self.compute_node_uncached(
            id,
            known,
            parent_size,
            avail,
            run_mode,
            sizing_mode,
            scale_factor,
            env,
        );
        self.nodes[id.idx()].size = result;
        result
    }

    fn compute_node_uncached(
        &mut self,
        id: LayoutId,
        known: Size<Option<f32>>,
        parent_size: Size<Option<f32>>,
        avail: Size<Avail>,
        run_mode: RunMode,
        sizing_mode: SizingMode,
        scale_factor: f32,
        env: &mut MeasureEnv<'_>,
    ) -> Size<f32> {
        let (has_children, has_measure, display) = {
            let node = &self.nodes[id.idx()];
            (
                node.children_len > 0,
                node.measure.is_some(),
                node.style.display,
            )
        };

        if !has_children || has_measure {
            return self.compute_leaf(
                id,
                known,
                parent_size,
                avail,
                run_mode,
                sizing_mode,
                scale_factor,
                env,
            );
        }

        match display {
            Display::Block => self.compute_block(
                id,
                known,
                parent_size,
                avail,
                run_mode,
                sizing_mode,
                scale_factor,
                env,
            ),
            Display::Flex => self.compute_flex(
                id,
                known,
                parent_size,
                avail,
                run_mode,
                sizing_mode,
                scale_factor,
                env,
            ),
        }
    }

    fn compute_leaf(
        &mut self,
        id: LayoutId,
        known: Size<Option<f32>>,
        parent_size: Size<Option<f32>>,
        avail: Size<Avail>,
        run_mode: RunMode,
        sizing_mode: SizingMode,
        scale_factor: f32,
        env: &mut MeasureEnv<'_>,
    ) -> Size<f32> {
        let style = self.nodes[id.idx()].style;
        let padding = style.padding.resolve_or_zero(parent_size.width);
        let pb = padding.add(style.border);
        let pb_w = pb.horiz();
        let pb_h = pb.vert();

        let (style_w, style_h, min_w, max_w, min_h, max_h) = match sizing_mode {
            SizingMode::InherentSize => (
                style.size.width.resolve(parent_size.width),
                style.size.height.resolve(parent_size.height),
                style.min_size.width.resolve(parent_size.width),
                style.max_size.width.resolve(parent_size.width),
                style.min_size.height.resolve(parent_size.height),
                style.max_size.height.resolve(parent_size.height),
            ),
            SizingMode::ContentSize => (None, None, None, None, None, None),
        };

        let floor_w = min_w.map_or(pb_w, |m| m.max(pb_w));
        let floor_h = min_h.map_or(pb_h, |m| m.max(pb_h));
        let node_w = known.width.or(style_w).map(|w| clamp_dim(w, Some(floor_w), max_w));
        let node_h = known.height.or(style_h).map(|h| clamp_dim(h, Some(floor_h), max_h));

        let has_measure = self.nodes[id.idx()].measure.is_some();
        if has_measure && (node_w.is_none() || node_h.is_none() || run_mode == RunMode::PerformLayout)
        {
            let known_inner = Size {
                width: node_w.map(|w| Pixels((w - pb_w).max(0.0) / scale_factor)),
                height: node_h.map(|h| Pixels((h - pb_h).max(0.0) / scale_factor)),
            };
            let avail_inner = Size {
                width: avail
                    .width
                    .maybe_set(node_w)
                    .maybe_min(max_w)
                    .maybe_max(min_w)
                    .maybe_sub(pb_w)
                    .to_logical(scale_factor),
                height: avail
                    .height
                    .maybe_set(node_h)
                    .maybe_min(max_h)
                    .maybe_max(min_h)
                    .maybe_sub(pb_h)
                    .to_logical(scale_factor),
            };
            let mut slot = self.nodes[id.idx()].measure.take().unwrap();
            let measured_logical = match (&mut slot, env) {
                (MeasureSlot::App(f), MeasureEnv::App(window, cx)) => {
                    f(known_inner, avail_inner, window, cx)
                }
                #[cfg(test)]
                (MeasureSlot::Pure(f), MeasureEnv::Pure) => f(known_inner, avail_inner),
                #[cfg(test)]
                _ => unreachable!("mismatched measure environment"),
            };
            self.nodes[id.idx()].measure = Some(slot);

            let measured = snap_measured_size_to_device_pixels(measured_logical, scale_factor);
            return size(
                node_w.unwrap_or_else(|| clamp_dim(measured.width + pb_w, Some(floor_w), max_w)),
                node_h.unwrap_or_else(|| clamp_dim(measured.height + pb_h, Some(floor_h), max_h)),
            );
        }

        size(
            node_w.unwrap_or_else(|| clamp_dim(pb_w, Some(floor_w), max_w)),
            node_h.unwrap_or_else(|| clamp_dim(pb_h, Some(floor_h), max_h)),
        )
    }

    fn compute_block(
        &mut self,
        id: LayoutId,
        known: Size<Option<f32>>,
        parent_size: Size<Option<f32>>,
        avail: Size<Avail>,
        run_mode: RunMode,
        sizing_mode: SizingMode,
        scale_factor: f32,
        env: &mut MeasureEnv<'_>,
    ) -> Size<f32> {
        let style = self.nodes[id.idx()].style;
        let padding = style.padding.resolve_or_zero(parent_size.width);
        let border = style.border;
        let pb = padding.add(border);
        let pb_w = pb.horiz();
        let pb_h = pb.vert();

        let (style_w, style_h, min_w, max_w, min_h, max_h) = match sizing_mode {
            SizingMode::InherentSize => (
                style.size.width.resolve(parent_size.width),
                style.size.height.resolve(parent_size.height),
                style.min_size.width.resolve(parent_size.width),
                style.max_size.width.resolve(parent_size.width),
                style.min_size.height.resolve(parent_size.height),
                style.max_size.height.resolve(parent_size.height),
            ),
            SizingMode::ContentSize => (None, None, None, None, None, None),
        };

        let floor_w = min_w.map_or(pb_w, |m| m.max(pb_w));
        let floor_h = min_h.map_or(pb_h, |m| m.max(pb_h));
        let known_w = known
            .width
            .or(style_w)
            .map(|w| clamp_dim(w, Some(floor_w), max_w));
        let known_h = known
            .height
            .or(style_h)
            .map(|h| clamp_dim(h, Some(floor_h), max_h));

        let children: SmallVec<[LayoutId; 8]> = {
            let node = &self.nodes[id.idx()];
            let start = node.children_start as usize;
            let end = start + node.children_len as usize;
            SmallVec::from_slice(&self.children_arena[start..end])
        };

        // 1. Determine container width.
        let container_w = match known_w {
            Some(w) => w,
            None => {
                let child_avail_w = avail.width.maybe_min(max_w).maybe_max(min_w).maybe_sub(pb_w);
                let child_avail_h = avail
                    .height
                    .maybe_set(known_h)
                    .maybe_min(max_h)
                    .maybe_sub(pb_h);
                let mut max_child_w = 0.0f32;
                for &child in &children {
                    let cs = self.nodes[child.idx()].style;
                    if cs.position == Position::Absolute {
                        continue;
                    }
                    let cm = cs.margin.resolve_or_zero(None);
                    let child_size = self.compute_node(
                        child,
                        Size {
                            width: None,
                            height: None,
                        },
                        Size {
                            width: None,
                            height: known_h.map(|h| (h - pb_h).max(0.0)),
                        },
                        Size {
                            width: child_avail_w.maybe_sub(cm.horiz()),
                            height: child_avail_h,
                        },
                        RunMode::ComputeSize,
                        SizingMode::InherentSize,
                        scale_factor,
                        env,
                    );
                    max_child_w = max_child_w.max(child_size.width + cm.horiz());
                }
                clamp_dim(max_child_w + pb_w, Some(floor_w), max_w)
            }
        };

        let inner_w = (container_w - pb_w).max(0.0);
        let inner_h_opt = known_h.map(|h| (h - pb_h).max(0.0));
        let child_avail_h = avail
            .height
            .maybe_set(known_h)
            .maybe_min(max_h)
            .maybe_sub(pb_h);

        // 2. Stack in-flow children vertically.
        let mut cursor_y = border.top + padding.top;
        for &child in &children {
            let cs = self.nodes[child.idx()].style;
            if cs.position == Position::Absolute {
                continue;
            }
            let cm = cs.margin.resolve_or_zero(Some(inner_w));
            let c_min_w = cs.min_size.width.resolve(Some(inner_w));
            let c_max_w = cs.max_size.width.resolve(Some(inner_w));
            let c_min_h = cs.min_size.height.resolve(inner_h_opt);
            let c_max_h = cs.max_size.height.resolve(inner_h_opt);
            let c_pb = cs.padding.resolve_or_zero(Some(inner_w)).add(cs.border);

            let c_w = cs
                .size
                .width
                .resolve(Some(inner_w))
                .unwrap_or((inner_w - cm.horiz()).max(0.0));
            let c_known_w = Some(clamp_dim(
                c_w,
                Some(c_min_w.map_or(c_pb.horiz(), |m| m.max(c_pb.horiz()))),
                c_max_w,
            ));
            let c_known_h = cs.size.height.resolve(inner_h_opt).map(|h| {
                clamp_dim(
                    h,
                    Some(c_min_h.map_or(c_pb.vert(), |m| m.max(c_pb.vert()))),
                    c_max_h,
                )
            });

            let child_size = self.compute_node(
                child,
                Size {
                    width: c_known_w,
                    height: c_known_h,
                },
                Size {
                    width: Some(inner_w),
                    height: inner_h_opt,
                },
                Size {
                    width: Avail::Definite((inner_w - cm.horiz()).max(0.0)),
                    height: child_avail_h,
                },
                run_mode,
                SizingMode::InherentSize,
                scale_factor,
                env,
            );

            if run_mode == RunMode::PerformLayout {
                let rel = relative_inset_offset(cs.inset, Some(inner_w), inner_h_opt);
                self.nodes[child.idx()].location = Point::new(
                    border.left + padding.left + cm.left + rel.x,
                    cursor_y + cm.top + rel.y,
                );
            }
            cursor_y += cm.top + child_size.height + cm.bottom;
        }

        let content_h = cursor_y - border.top - padding.top;
        let container_h =
            known_h.unwrap_or_else(|| clamp_dim(content_h + pb_h, Some(floor_h), max_h));

        if run_mode == RunMode::PerformLayout {
            self.layout_absolute_children(
                &children,
                size(container_w, container_h),
                border,
                padding,
                None,
                scale_factor,
                env,
            );
        }

        size(container_w, container_h)
    }

    fn compute_flex(
        &mut self,
        id: LayoutId,
        known: Size<Option<f32>>,
        parent_size: Size<Option<f32>>,
        avail: Size<Avail>,
        run_mode: RunMode,
        sizing_mode: SizingMode,
        scale_factor: f32,
        env: &mut MeasureEnv<'_>,
    ) -> Size<f32> {
        let style = self.nodes[id.idx()].style;
        let row = style.flex_direction == FlexDirection::Row;
        let padding = style.padding.resolve_or_zero(parent_size.width);
        let border = style.border;
        let pb = padding.add(border);
        let pb_main = pb.main_sum(row);
        let pb_cross = pb.cross_sum(row);

        let (style_w, style_h, min_w, max_w, min_h, max_h) = match sizing_mode {
            SizingMode::InherentSize => (
                style.size.width.resolve(parent_size.width),
                style.size.height.resolve(parent_size.height),
                style.min_size.width.resolve(parent_size.width),
                style.max_size.width.resolve(parent_size.width),
                style.min_size.height.resolve(parent_size.height),
                style.max_size.height.resolve(parent_size.height),
            ),
            SizingMode::ContentSize => (None, None, None, None, None, None),
        };

        let (style_main, style_cross) = if row { (style_w, style_h) } else { (style_h, style_w) };
        let (min_main, min_cross) = if row { (min_w, min_h) } else { (min_h, min_w) };
        let (max_main, max_cross) = if row { (max_w, max_h) } else { (max_h, max_w) };
        let (known_main, known_cross) = if row {
            (known.width, known.height)
        } else {
            (known.height, known.width)
        };
        let (avail_main_in, avail_cross_in) = if row {
            (avail.width, avail.height)
        } else {
            (avail.height, avail.width)
        };

        let floor_main = min_main.map_or(pb_main, |m| m.max(pb_main));
        let floor_cross = min_cross.map_or(pb_cross, |m| m.max(pb_cross));
        let node_main = known_main
            .or(style_main)
            .map(|v| clamp_dim(v, Some(floor_main), max_main));
        let node_cross = known_cross
            .or(style_cross)
            .map(|v| clamp_dim(v, Some(floor_cross), max_cross));

        let children: SmallVec<[LayoutId; 8]> = {
            let node = &self.nodes[id.idx()];
            let start = node.children_start as usize;
            let end = start + node.children_len as usize;
            SmallVec::from_slice(&self.children_arena[start..end])
        };

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
            if run_mode == RunMode::PerformLayout {
                self.layout_absolute_children(
                    &children,
                    container_size,
                    border,
                    padding,
                    Some((row, style.justify_content, style.align_items)),
                    scale_factor,
                    env,
                );
            }
            return container_size;
        }

        let inner_main_opt = node_main.map(|v| (v - pb_main).max(0.0));
        let inner_cross_opt = node_cross.map(|v| (v - pb_cross).max(0.0));
        let inner_size_opt = size_from(inner_main_opt, inner_cross_opt, row);
        let gap_main = style.gap.main(row).resolve_or_zero(inner_main_opt);

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
            style: NodeStyle,
            margin: EdgeF32,
            pb_main: f32,
            pb_cross: f32,
            align: Option<AlignItems>,
            flex_basis: f32,
            min_main: f32,
            max_main: Option<f32>,
            hypothetical_main: f32,
            target_main: f32,
            frozen: bool,
        }

        let mut items: SmallVec<[FlexItem; 8]> = SmallVec::with_capacity(in_flow_count);
        for &child in &children {
            let cs = self.nodes[child.idx()].style;
            if cs.position == Position::Absolute {
                continue;
            }
            let margin = cs.margin.resolve_or_zero(inner_size_opt.width);
            let c_pb = cs.padding.resolve_or_zero(inner_size_opt.width).add(cs.border);
            let c_pb_main = c_pb.main_sum(row);
            let c_pb_cross = c_pb.cross_sum(row);
            let align = cs.align_self.or(style.align_items);

            let c_size_main = cs.size.main(row).resolve(inner_main_opt);
            let c_size_cross = cs.size.cross(row).resolve(inner_cross_opt);
            let c_min_main = cs.min_size.main(row).resolve(inner_main_opt);
            let c_max_main = cs.max_size.main(row).resolve(inner_main_opt);
            let c_min_cross = cs.min_size.cross(row).resolve(inner_cross_opt);
            let c_max_cross = cs.max_size.cross(row).resolve(inner_cross_opt);
            let c_floor_cross = c_min_cross.map_or(c_pb_cross, |m| m.max(c_pb_cross));

            // Pre-compute definite or stretched cross size so wrapped text inside a
            // flex column measures its true height at the stretched column width.
            let pre_cross = if let Some(s) = c_size_cross {
                Some(clamp_dim(s, Some(c_floor_cross), c_max_cross))
            } else if align.is_none()
                && let Some(ic) = inner_cross_opt
            {
                Some(clamp_dim(
                    (ic - margin.cross_sum(row)).max(0.0),
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
                    let measured = self.compute_node(
                        child,
                        size_from(None, pre_cross, row),
                        inner_size_opt,
                        size_from(probe_main, avail_cross.maybe_sub(margin.cross_sum(row)), row),
                        RunMode::ComputeSize,
                        SizingMode::InherentSize,
                        scale_factor,
                        env,
                    );
                    (main_of(measured, row), true)
                }
            };

            let effective_min_main = if let Some(min_m) = c_min_main {
                min_m.max(c_pb_main)
            } else if cs.overflow_main(row) == Overflow::Hidden
                || (cs.flex_shrink == 0.0 && basis_from_content)
            {
                c_pb_main
            } else {
                let measured_min = self.compute_node(
                    child,
                    size_from(None, pre_cross, row),
                    inner_size_opt,
                    size_from(
                        Avail::MinContent,
                        avail_cross.maybe_sub(margin.cross_sum(row)),
                        row,
                    ),
                    RunMode::ComputeSize,
                    SizingMode::ContentSize,
                    scale_factor,
                    env,
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
                frozen: false,
            });
        }

        // Determine container main size.
        let total_gaps = gap_main * items.len().saturating_sub(1) as f32;
        let total_hypothetical: f32 = total_gaps
            + items
                .iter()
                .map(|it| it.hypothetical_main + it.margin.main_sum(row))
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

        // CSS Flexbox §9.7: resolve flexible lengths.
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
                        }) + it.margin.main_sum(row)
                    })
                    .sum::<f32>();

            loop {
                let mut unfrozen_flex_sum = 0.0f32;
                let mut used = total_gaps;
                let mut any_unfrozen = false;
                for item in &items {
                    if item.frozen {
                        used += item.target_main + item.margin.main_sum(row);
                    } else {
                        any_unfrozen = true;
                        used += item.flex_basis + item.margin.main_sum(row);
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
                let mut violations: SmallVec<[f32; 8]> = SmallVec::with_capacity(items.len());
                for item in &mut items {
                    if item.frozen {
                        violations.push(0.0);
                    } else {
                        let clamped =
                            clamp_dim(item.target_main, Some(item.min_main), item.max_main);
                        let delta = clamped - item.target_main;
                        item.target_main = clamped;
                        total_violation += delta;
                        violations.push(delta);
                    }
                }

                if total_violation.abs() < 1e-4 {
                    break;
                }
                let mut froze_any = false;
                for (item, &v) in items.iter_mut().zip(violations.iter()) {
                    if !item.frozen
                        && ((total_violation > 0.0 && v > 0.0)
                            || (total_violation < 0.0 && v < 0.0))
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

        // Determine container cross size.
        let inner_size_with_main = size_from(Some(inner_main), inner_cross_opt, row);
        let container_cross = match node_cross {
            Some(c) => c,
            None => {
                let mut max_outer_cross = 0.0f32;
                for item in &items {
                    let c_min_cross = item.style.min_size.cross(row).resolve(inner_cross_opt);
                    let c_max_cross = item.style.max_size.cross(row).resolve(inner_cross_opt);
                    let c_floor_cross = c_min_cross.map_or(item.pb_cross, |m| m.max(item.pb_cross));
                    let c_known_cross = item
                        .style
                        .size
                        .cross(row)
                        .resolve(inner_cross_opt)
                        .map(|s| clamp_dim(s, Some(c_floor_cross), c_max_cross));
                    let measured = self.compute_node(
                        item.id,
                        size_from(Some(item.target_main), c_known_cross, row),
                        inner_size_with_main,
                        size_from(
                            Avail::Definite(item.target_main),
                            avail_cross.maybe_sub(item.margin.cross_sum(row)),
                            row,
                        ),
                        RunMode::ComputeSize,
                        SizingMode::InherentSize,
                        scale_factor,
                        env,
                    );
                    max_outer_cross =
                        max_outer_cross.max(cross_of(measured, row) + item.margin.cross_sum(row));
                }
                clamp_dim(max_outer_cross + pb_cross, Some(floor_cross), max_cross)
            }
        };

        let container_size = size_from(container_main, container_cross, row);
        if run_mode == RunMode::ComputeSize {
            return container_size;
        }

        // Final PerformLayout pass & positioning of all in-flow children.
        let inner_cross = (container_cross - pb_cross).max(0.0);
        let final_inner_size = size_from(Some(inner_main), Some(inner_cross), row);

        let mut final_sizes: SmallVec<[Size<f32>; 8]> = SmallVec::with_capacity(items.len());
        let mut used_main = total_gaps;
        for item in &items {
            let c_min_cross = item.style.min_size.cross(row).resolve(Some(inner_cross));
            let c_max_cross = item.style.max_size.cross(row).resolve(Some(inner_cross));
            let c_floor_cross = c_min_cross.map_or(item.pb_cross, |m| m.max(item.pb_cross));

            let final_cross_known = if let Some(s) = item.style.size.cross(row).resolve(Some(inner_cross)) {
                Some(clamp_dim(s, Some(c_floor_cross), c_max_cross))
            } else if item.align.is_none() {
                Some(clamp_dim(
                    (inner_cross - item.margin.cross_sum(row)).max(0.0),
                    Some(c_floor_cross),
                    c_max_cross,
                ))
            } else {
                None
            };

            let child_size = self.compute_node(
                item.id,
                size_from(Some(item.target_main), final_cross_known, row),
                final_inner_size,
                size_from(
                    Avail::Definite(item.target_main),
                    Avail::Definite((inner_cross - item.margin.cross_sum(row)).max(0.0)),
                    row,
                ),
                RunMode::PerformLayout,
                SizingMode::InherentSize,
                scale_factor,
                env,
            );
            used_main += main_of(child_size, row) + item.margin.main_sum(row);
            final_sizes.push(child_size);
        }

        let main_offset = match style.justify_content {
            Some(JustifyContent::Center) => (inner_main - used_main) / 2.0,
            None => 0.0,
        };
        let mut main_cursor = border.main_start(row) + padding.main_start(row) + main_offset;
        let cross_origin = border.cross_start(row) + padding.cross_start(row);

        for (item, child_size) in items.iter().zip(final_sizes.iter()) {
            let c_main = main_of(*child_size, row);
            let c_cross = cross_of(*child_size, row);
            let main_pos = main_cursor + item.margin.main_start(row);
            let cross_pos = match item.align {
                Some(AlignItems::Center) => {
                    cross_origin
                        + (inner_cross - (c_cross + item.margin.cross_sum(row))) / 2.0
                        + item.margin.cross_start(row)
                }
                Some(AlignItems::Start) | None => cross_origin + item.margin.cross_start(row),
            };
            let rel = relative_inset_offset(
                item.style.inset,
                final_inner_size.width,
                final_inner_size.height,
            );
            let base = point_from(main_pos, cross_pos, row);
            self.nodes[item.id.idx()].location = Point::new(base.x + rel.x, base.y + rel.y);
            main_cursor +=
                item.margin.main_start(row) + c_main + item.margin.main_end(row) + gap_main;
        }

        self.layout_absolute_children(
            &children,
            container_size,
            border,
            padding,
            Some((row, style.justify_content, style.align_items)),
            scale_factor,
            env,
        );

        container_size
    }

    fn layout_absolute_children(
        &mut self,
        children: &[LayoutId],
        parent_size: Size<f32>,
        parent_border: EdgeF32,
        parent_padding: EdgeF32,
        flex_align: Option<(bool, Option<JustifyContent>, Option<AlignItems>)>,
        scale_factor: f32,
        env: &mut MeasureEnv<'_>,
    ) {
        // CSS containing block for `position: absolute` is the parent's padding box.
        let cb_origin = Point::new(parent_border.left, parent_border.top);
        let cb_w = (parent_size.width - parent_border.horiz()).max(0.0);
        let cb_h = (parent_size.height - parent_border.vert()).max(0.0);
        let cb_size = Size {
            width: Some(cb_w),
            height: Some(cb_h),
        };
        let content_w = (cb_w - parent_padding.horiz()).max(0.0);
        let content_h = (cb_h - parent_padding.vert()).max(0.0);

        for &child in children {
            let cs = self.nodes[child.idx()].style;
            if cs.position != Position::Absolute {
                continue;
            }
            let inset_l = cs.inset.left.resolve(Some(cb_w));
            let inset_r = cs.inset.right.resolve(Some(cb_w));
            let inset_t = cs.inset.top.resolve(Some(cb_h));
            let inset_b = cs.inset.bottom.resolve(Some(cb_h));
            let m = cs.margin.resolve_or_zero(Some(cb_w));
            let c_pb = cs.padding.resolve_or_zero(Some(cb_w)).add(cs.border);

            let c_min_w = cs.min_size.width.resolve(Some(cb_w));
            let c_max_w = cs.max_size.width.resolve(Some(cb_w));
            let c_min_h = cs.min_size.height.resolve(Some(cb_h));
            let c_max_h = cs.max_size.height.resolve(Some(cb_h));
            let floor_w = c_min_w.map_or(c_pb.horiz(), |v| v.max(c_pb.horiz()));
            let floor_h = c_min_h.map_or(c_pb.vert(), |v| v.max(c_pb.vert()));

            let known_w = if let Some(w) = cs.size.width.resolve(Some(cb_w)) {
                Some(clamp_dim(w, Some(floor_w), c_max_w))
            } else if let (Some(l), Some(r)) = (inset_l, inset_r) {
                Some(clamp_dim((cb_w - l - r - m.horiz()).max(0.0), Some(floor_w), c_max_w))
            } else {
                None
            };
            let known_h = if let Some(h) = cs.size.height.resolve(Some(cb_h)) {
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
                Size {
                    width: known_w,
                    height: known_h,
                },
                cb_size,
                Size {
                    width: avail_w,
                    height: avail_h,
                },
                RunMode::PerformLayout,
                SizingMode::InherentSize,
                scale_factor,
                env,
            );

            let (static_x, static_y) = match flex_align {
                Some((row, justify, align_items)) => {
                    let align = cs.align_self.or(align_items);
                    let main_off = match justify {
                        Some(JustifyContent::Center) => {
                            let avail_m = if row { content_w } else { content_h };
                            let child_m = main_of(child_size, row) + m.main_sum(row);
                            (avail_m - child_m) / 2.0
                        }
                        None => 0.0,
                    };
                    let cross_off = match align {
                        Some(AlignItems::Center) => {
                            let avail_c = if row { content_h } else { content_w };
                            let child_c = cross_of(child_size, row) + m.cross_sum(row);
                            (avail_c - child_c) / 2.0
                        }
                        Some(AlignItems::Start) | None => 0.0,
                    };
                    if row {
                        (main_off, cross_off)
                    } else {
                        (cross_off, main_off)
                    }
                }
                None => (0.0, 0.0),
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

            self.nodes[child.idx()].location = Point::new(x, y);
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
fn main_of<T: Copy + Debug + Default + PartialEq>(s: Size<T>, row: bool) -> T {
    if row { s.width } else { s.height }
}

#[inline]
fn cross_of<T: Copy + Debug + Default + PartialEq>(s: Size<T>, row: bool) -> T {
    if row { s.height } else { s.width }
}

#[inline]
fn size_from<T: Copy + Debug + Default + PartialEq>(main: T, cross: T, row: bool) -> Size<T> {
    if row {
        Size {
            width: main,
            height: cross,
        }
    } else {
        Size {
            width: cross,
            height: main,
        }
    }
}

#[inline]
fn point_from<T: Copy + Debug + Default + PartialEq>(main: T, cross: T, row: bool) -> Point<T> {
    if row {
        Point { x: main, y: cross }
    } else {
        Point { x: cross, y: main }
    }
}

#[inline]
fn relative_inset_offset(inset: EdgeDim, parent_w: Option<f32>, parent_h: Option<f32>) -> Point<f32> {
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
    Point::new(x, y)
}

fn snap_measured_size_to_device_pixels(size: Size<Pixels>, scale_factor: f32) -> Size<f32> {
    size.map(|d| ceil_to_device_pixel(d.0.max(0.0), scale_factor))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::{Styled as _, StyleRefinement, px, relative};

    impl LayoutEngine {
        fn req(&mut self, refine: StyleRefinement, children: &[LayoutId], scale: f32) -> LayoutId {
            let mut style = Style::default();
            crate::ui::Refineable::refine(&mut style, &refine);
            self.request_layout(style, px(16.), scale, children)
        }

        fn req_measured(
            &mut self,
            refine: StyleRefinement,
            scale: f32,
            f: impl FnMut(Size<Option<Pixels>>, Size<AvailableSpace>) -> Size<Pixels> + 'static,
        ) -> LayoutId {
            let mut style = Style::default();
            crate::ui::Refineable::refine(&mut style, &refine);
            let id = LayoutId(self.nodes.len() as u32);
            self.nodes.push(Node {
                style: NodeStyle::from_style(&style, px(16.), scale),
                parent: None,
                children_start: 0,
                children_len: 0,
                measure: Some(MeasureSlot::Pure(Box::new(f))),
                location: Point::default(),
                size: Size::default(),
                abs_origin: None,
                abs_bounds: None,
                computed: false,
                cache: SizeCache::default(),
            });
            id
        }

        fn run(&mut self, root: LayoutId, avail: Size<AvailableSpace>, scale: f32) {
            let mut env = MeasureEnv::Pure;
            self.compute_root(root, avail, scale, &mut env);
        }
    }

    fn s() -> StyleRefinement {
        StyleRefinement::default()
    }

    #[test]
    fn flex_col_window_chrome_and_flex_1_editor() {
        let mut engine = LayoutEngine::new();
        let title = engine.req(s().flex_none().h(px(40.)), &[], 1.0);
        let menu = engine.req(s().flex_none().h(px(32.)), &[], 1.0);
        let editor = engine.req(s().flex_1().min_h(px(60.)), &[], 1.0);
        let status = engine.req(s().flex_none().h(px(33.)), &[], 1.0);
        let root = engine.req(
            s().size_full().flex().flex_col(),
            &[title, menu, editor, status],
            1.0,
        );
        engine.run(root, size(px(800.), px(600.)).into(), 1.0);

        assert_eq!(
            engine.layout_bounds(title, 1.0),
            Bounds::new(Point::new(px(0.), px(0.)), size(px(800.), px(40.)))
        );
        assert_eq!(
            engine.layout_bounds(menu, 1.0),
            Bounds::new(Point::new(px(0.), px(40.)), size(px(800.), px(32.)))
        );
        assert_eq!(
            engine.layout_bounds(editor, 1.0),
            Bounds::new(Point::new(px(0.), px(72.)), size(px(800.), px(495.)))
        );
        assert_eq!(
            engine.layout_bounds(status, 1.0),
            Bounds::new(Point::new(px(0.), px(567.)), size(px(800.), px(33.)))
        );
    }

    #[test]
    fn dialog_wrapped_text_and_equal_buttons() {
        // Mirrors dialog_frame in src/dialog.rs: Block outer w(360) + border_1 ->
        // flex_col -> Block content (p(24), wrapped text) + Flex buttons (p(24), gap(8), 2x flex_1 min_w_0).
        let mut engine = LayoutEngine::new();
        // Simulated text: 500px unwrapped, wraps into 20px rows at wrap_width.
        let body_text = engine.req_measured(s(), 1.0, |known, avail| {
            let wrap = known.width.or(match avail.width {
                AvailableSpace::Definite(w) => Some(w),
                _ => None,
            });
            match wrap {
                Some(w) if w < px(500.) => {
                    let lines = (500.0 / w.0).ceil().max(1.0);
                    size(w, px(lines * 20.))
                }
                _ => size(px(500.), px(20.)),
            }
        });
        let content = engine.req(s().p(px(24.)).border_b_1(), &[body_text], 1.0);
        let b1 = engine.req(s().flex_1().min_w_0().h(px(32.)), &[], 1.0);
        let b2 = engine.req(s().flex_1().min_w_0().h(px(32.)), &[], 1.0);
        let footer = engine.req(s().flex().p(px(24.)).gap(px(8.)), &[b1, b2], 1.0);
        let col = engine.req(s().flex().flex_col(), &[content, footer], 1.0);
        let frame = engine.req(s().w(px(360.)).max_w_full().border_1(), &[col], 1.0);
        let overlay = engine.req(
            s().absolute().top_0().left_0().size_full().flex().items_center().justify_center(),
            &[frame],
            1.0,
        );
        let root = engine.req(s().size_full(), &[overlay], 1.0);
        engine.run(root, size(px(800.), px(600.)).into(), 1.0);

        // Inner content width = 360 - 2 (border) - 48 (padding) = 310 -> 2 lines -> 40px tall.
        let tb = engine.layout_bounds(body_text, 1.0);
        assert_eq!(tb.size, size(px(310.), px(40.)));
        // Content height = 40 + 48 padding + 1 bottom border = 89.
        assert_eq!(engine.layout_bounds(content, 1.0).size, size(px(358.), px(89.)));
        // Footer buttons: (358 - 48 - 8) / 2 = 151px each!
        assert_eq!(engine.layout_bounds(b1, 1.0).size, size(px(151.), px(32.)));
        assert_eq!(engine.layout_bounds(b2, 1.0).size, size(px(151.), px(32.)));
        // Frame height = 89 + (32 + 48) + 2 = 171, centered in 800x600 -> (220, 214..215).
        let fb = engine.layout_bounds(frame, 1.0);
        assert_eq!(fb.size, size(px(360.), px(171.)));
        assert_eq!(fb.origin.x, px(220.));
    }

    #[test]
    fn tab_label_min_w_0_truncates_wide_nowrap_text() {
        let mut engine = LayoutEngine::new();
        let long_text = engine.req_measured(s(), 1.0, |_, _| size(px(400.), px(16.)));
        let label_box = engine.req(s().flex_1().min_w_0().overflow_hidden(), &[long_text], 1.0);
        let tab = engine.req(
            s().flex().items_center().w(px(180.)).h(px(32.)).pl(px(12.)).pr(px(36.)),
            &[label_box],
            1.0,
        );
        engine.run(tab, size(px(180.), px(32.)).into(), 1.0);

        let lb = engine.layout_bounds(label_box, 1.0);
        assert_eq!(lb.origin, Point::new(px(12.), px(8.)));
        assert_eq!(lb.size, size(px(132.), px(16.)));
    }

    #[test]
    fn text_field_negative_insets_and_absolute_glyph() {
        // Mirrors src/field.rs text_field + bottom_edge + field_glyph.
        let mut engine = LayoutEngine::new();
        let bottom_edge = engine.req(
            s().absolute().left(px(-1.)).right(px(-1.)).bottom_0().h(px(2.)),
            &[],
            1.0,
        );
        let glyph = engine.req(
            s().absolute().top(px(2.)).bottom(px(2.)).right(px(4.)).w(px(24.)),
            &[],
            1.0,
        );
        let field = engine.req(
            s().relative()
                .w(px(200.))
                .h(px(32.))
                .border_t_1()
                .border_l_1()
                .border_r_1()
                .pb(px(1.)),
            &[bottom_edge, glyph],
            1.0,
        );
        engine.run(field, size(px(200.), px(32.)).into(), 1.0);

        // Padding box = x: 1..199 (w=198), y: 1..32 (h=31).
        // bottom_edge: left=-1 -> x=0, w=200; bottom=0 -> y=30, h=2.
        assert_eq!(
            engine.layout_bounds(bottom_edge, 1.0),
            Bounds::new(Point::new(px(0.), px(30.)), size(px(200.), px(2.)))
        );
        // glyph: top=2 -> y=3, h=31-4=27; right=4 -> x=199-4-24=171, w=24.
        assert_eq!(
            engine.layout_bounds(glyph, 1.0),
            Bounds::new(Point::new(px(171.), px(3.)), size(px(24.), px(27.)))
        );
    }

    #[test]
    fn status_bar_min_content_root_measurement() {
        // Mirrors src/main.rs:626 status.layout_as_root(AvailableSpace::min_size(), ...).
        let mut engine = LayoutEngine::new();
        let t1 = engine.req_measured(s(), 1.0, |_, _| size(px(60.), px(16.)));
        let div1 = engine.req(s().flex_none().w(px(1.)).h(px(14.)), &[], 1.0);
        let t2 = engine.req_measured(s(), 1.0, |_, _| size(px(80.), px(16.)));
        let spacer = engine.req(s().flex_1().min_w(px(1.)).h(px(14.)), &[], 1.0);
        let t3 = engine.req_measured(s(), 1.0, |_, _| size(px(40.), px(16.)));
        let row = engine.req(
            s().flex().items_center().gap(px(24.)).h(px(32.)).px(px(22.)),
            &[t1, div1, t2, spacer, t3],
            1.0,
        );
        let sep = engine.req(s().h(px(1.)), &[], 1.0);
        let status = engine.req(s().flex_none(), &[sep, row], 1.0);
        engine.run(status, AvailableSpace::min_size(), 1.0);

        // Expected width: 22*2 + (60 + 1 + 80 + 1 + 40) + 4*24 = 44 + 182 + 96 = 322.
        // Expected height: 1 + 32 = 33.
        assert_eq!(engine.layout_bounds(status, 1.0).size, size(px(322.), px(33.)));
    }

    #[test]
    fn menu_command_tile_self_start_text_center_and_relative_offset() {
        let mut engine = LayoutEngine::new();
        let icon = engine.req_measured(s(), 1.0, |_, _| size(px(16.), px(16.)));
        let icon_box = engine.req(s().h(px(16.)), &[icon], 1.0);
        let label_text = engine.req_measured(s(), 1.0, |_, _| size(px(24.), px(14.)));
        let label_box = engine.req(
            s().self_start().ml(px(6.)).w(px(48.)).mt(px(4.)).h(px(16.)),
            &[label_text],
            1.0,
        );
        let tile = engine.req(
            s().flex().flex_col().items_center().w(px(60.)).h(px(50.)).pt(px(8.)),
            &[icon_box, label_box],
            1.0,
        );
        let nudged = engine.req(s().relative().left(px(1.)).top(px(2.)).w(px(10.)).h(px(10.)), &[], 1.0);
        let root = engine.req(s().flex(), &[tile, nudged], 1.0);
        engine.run(root, size(px(200.), px(100.)).into(), 1.0);

        // icon_box centered inside 60px -> x = (60 - 16)/2 = 22, y = 8.
        assert_eq!(engine.layout_bounds(icon_box, 1.0).origin, Point::new(px(22.), px(8.)));
        // label_box self_start + ml(6) -> x = 6, y = 8 + 16 + 4 = 28, width = 48.
        assert_eq!(
            engine.layout_bounds(label_box, 1.0),
            Bounds::new(Point::new(px(6.), px(28.)), size(px(48.), px(16.)))
        );
        // label_text stretched to label_box width 48 so text_center() centers inside 48px!
        assert_eq!(engine.layout_bounds(label_text, 1.0).size.width, px(48.));
        // nudged sits at x = 60 + 1 = 61, y = 0 + 2 = 2.
        assert_eq!(engine.layout_bounds(nudged, 1.0).origin, Point::new(px(61.), px(2.)));
    }

    #[test]
    fn fractional_dpi_edge_closure_between_flex_siblings() {
        // At 150% scale, 3 equal flex_1 siblings in 100 logical px (= 150 dp) close seamlessly.
        let scale = 1.5;
        let mut engine = LayoutEngine::new();
        let a = engine.req(s().flex_1().h(px(20.)), &[], scale);
        let b = engine.req(s().flex_1().h(px(20.)), &[], scale);
        let c = engine.req(s().flex_1().h(px(20.)), &[], scale);
        let root = engine.req(s().w(px(100.)).h(px(20.)).flex(), &[a, b, c], scale);
        engine.run(root, size(px(100.), px(20.)).into(), scale);

        let ba = engine.layout_bounds(a, scale);
        let bb = engine.layout_bounds(b, scale);
        let bc = engine.layout_bounds(c, scale);
        assert_eq!(ba.right(), bb.left());
        assert_eq!(bb.right(), bc.left());
        assert_eq!(bc.right(), px(100.));
    }

    #[test]
    fn editor_percentage_chain_through_flex_1() {
        // Mirrors main.rs editor-area (flex_1) -> Block size_full -> Block size_full ->
        // EditorState (h_full, padding) -> TextElement (100%x100%, min_h) + EditorScrollbar (abs 100%x100%).
        let mut engine = LayoutEngine::new();
        let text_el = engine.req(s().w_full().h_full().min_h(px(40.)), &[], 1.0);
        let scrollbar = engine.req(s().absolute().size_full(), &[], 1.0);
        let editor_state = engine.req(
            s().flex_1().h_full().pt(px(10.)).pr(px(12.)).pb(px(10.)).pl(px(14.)),
            &[text_el, scrollbar],
            1.0,
        );
        let inner = engine.req(s().size_full(), &[editor_state], 1.0);
        let editor_area = engine.req(s().flex_1().min_h(px(60.)), &[inner], 1.0);
        let header = engine.req(s().flex_none().h(px(100.)), &[], 1.0);
        let root = engine.req(s().size_full().flex().flex_col(), &[header, editor_area], 1.0);
        engine.run(root, size(px(800.), px(500.)).into(), 1.0);

        assert_eq!(
            engine.layout_bounds(text_el, 1.0),
            Bounds::new(Point::new(px(14.), px(110.)), size(px(774.), px(380.)))
        );
        // EditorScrollbar fills padding box (800x400), including editor padding.
        assert_eq!(
            engine.layout_bounds(scrollbar, 1.0).size,
            size(px(800.), px(400.))
        );
    }

    #[test]
    fn flex_shrink_respects_flex_none_and_min_size() {
        let mut engine = LayoutEngine::new();
        let fixed = engine.req(s().flex_none().w(px(100.)).h(px(20.)), &[], 1.0);
        let a = engine.req(s().w(px(200.)).min_w(px(80.)).h(px(20.)), &[], 1.0);
        let b = engine.req(s().w(px(200.)).min_w_0().h(px(20.)), &[], 1.0);
        let root = engine.req(s().w(px(220.)).flex(), &[fixed, a, b], 1.0);
        engine.run(root, size(px(220.), px(20.)).into(), 1.0);

        // fixed stays 100. Remaining 120 split between a (min 80) and b (min 0):
        // a freezes at 80, b gets 40!
        assert_eq!(engine.layout_bounds(fixed, 1.0).size.width, px(100.));
        assert_eq!(engine.layout_bounds(a, 1.0).size.width, px(80.));
        assert_eq!(engine.layout_bounds(b, 1.0).size.width, px(40.));
    }

    #[test]
    fn abspos_dropdown_inside_narrow_button_does_not_squish() {
        let mut engine = LayoutEngine::new();
        let item = engine.req_measured(s(), 1.0, |_, _| size(px(180.), px(28.)));
        let dropdown = engine.req(s().absolute().left_0().top(px(32.)).flex().flex_col(), &[item], 1.0);
        let button = engine.req(s().relative().w(px(40.)).h(px(32.)), &[dropdown], 1.0);
        engine.run(button, size(px(40.), px(32.)).into(), 1.0);

        assert_eq!(
            engine.layout_bounds(dropdown, 1.0),
            Bounds::new(Point::new(px(0.), px(32.)), size(px(180.), px(28.)))
        );
    }

    #[test]
    fn stretch_auto_size_to_fill_and_recompute_invalidates_cache() {
        let mut engine = LayoutEngine::new();
        let child = engine.req(s().w(relative(0.5)).h(px(20.)), &[], 1.0);
        let root = engine.req(s().flex(), &[child], 1.0);
        engine.stretch_auto_size_to_fill(root, size(px(200.), px(100.)), 1.0);
        engine.run(root, size(px(200.), px(100.)).into(), 1.0);
        assert_eq!(engine.layout_bounds(child, 1.0).size.width, px(100.));

        // Re-running layout on the same root after changing root size updates cached bounds.
        engine.nodes[root.idx()].style.size.width = Dim::Length(400.0);
        engine.run(root, size(px(400.), px(100.)).into(), 1.0);
        assert_eq!(engine.layout_bounds(child, 1.0).size.width, px(200.));
    }

    #[test]
    fn auto_height_flex_col_does_not_crush_in_short_window_and_respects_max_w_full() {
        let mut engine = LayoutEngine::new();
        let content = engine.req(s().h(px(120.)), &[], 1.0);
        let footer = engine.req(s().h(px(80.)), &[], 1.0);
        let col = engine.req(s().flex().flex_col(), &[content, footer], 1.0);
        let frame = engine.req(s().w(px(360.)).max_w_full().border_1(), &[col], 1.0);
        let overlay = engine.req(
            s().absolute().top_0().left_0().size_full().flex().items_center().justify_center(),
            &[frame],
            1.0,
        );
        let root = engine.req(s().size_full(), &[overlay], 1.0);
        // Narrow and short window: width shrinks via max_w_full() to 250, height stays natural 202.
        engine.run(root, size(px(250.), px(140.)).into(), 1.0);

        assert_eq!(engine.layout_bounds(frame, 1.0).size, size(px(250.), px(202.)));
        assert_eq!(engine.layout_bounds(content, 1.0).size, size(px(248.), px(120.)));
        assert_eq!(engine.layout_bounds(footer, 1.0).size, size(px(248.), px(80.)));
    }

    #[test]
    fn single_line_text_field_min_h_full_centers_text_element() {
        // Mirrors src/field.rs text_field + src/base/input/base/state.rs single-line EditorState.
        let mut engine = LayoutEngine::new();
        let text_el = engine.req(s().w_full().h(px(20.)), &[], 1.0);
        let input_state = engine.req(
            s().flex_1().min_h_full().flex().items_center().overflow_x_hidden(),
            &[text_el],
            1.0,
        );
        let inner = engine.req(
            s().h_full().flex().items_center().pl(px(9.)).pr(px(33.)),
            &[input_state],
            1.0,
        );
        let field = engine.req(
            s().relative().w(px(200.)).h(px(32.)).border_t_1().border_l_1().border_r_1().pb(px(1.)),
            &[inner],
            1.0,
        );
        engine.run(field, size(px(200.), px(32.)).into(), 1.0);

        // inner padding box of field: x = 1..199 (198), y = 1..31 (30).
        // input_state: x = 1 + 9 = 10, w = 198 - 42 = 156, h = 30 (via min_h_full).
        // text_el: vertically centered inside 30 -> y = 1 + (30 - 20)/2 = 6, size = 156x20.
        assert_eq!(
            engine.layout_bounds(input_state, 1.0),
            Bounds::new(Point::new(px(10.), px(1.)), size(px(156.), px(30.)))
        );
        assert_eq!(
            engine.layout_bounds(text_el, 1.0),
            Bounds::new(Point::new(px(10.), px(6.)), size(px(156.), px(20.)))
        );
    }

    #[test]
    fn directwrite_status_bar_shapes_all_labels_including_null_glyph_offsets() {
        let dw = std::sync::Arc::new(crate::win32::DirectWriteTextSystem::new(None).unwrap());
        let ts = std::sync::Arc::new(crate::ui::TextSystem::new(dw.clone()));
        let wts = std::rc::Rc::new(crate::ui::WindowTextSystem::new(ts.clone()));
        let font = crate::ui::font("Segoe UI Variable Small");
        for label in ["Ln 1, Col 1", "0 characters", "100%", "42", "Windows (CRLF)", "UTF-8"] {
            let fid = ts.resolve_font(&font);
            let layout = dw.layout_line(
                label,
                px(12.),
                &[crate::ui::FontRun {
                    len: label.len(),
                    font_id: fid,
                }],
            );
            assert!(
                layout.width > px(0.) && !layout.runs.is_empty(),
                "expected non-empty layout for {label:?}, got width={:?}, runs={}",
                layout.width,
                layout.runs.len()
            );
        }

        let make_status = |engine: &mut LayoutEngine, scale: f32| {
            let text_node = |engine: &mut LayoutEngine, text: &'static str| {
                let wts = wts.clone();
                let font = font.clone();
                let cache: std::rc::Rc<
                    std::cell::RefCell<Option<(Option<Pixels>, Size<Pixels>)>>,
                > = Default::default();
                engine.req_measured(s(), scale, move |known, avail| {
                    let wrap_width = known.width.or(match avail.width {
                        AvailableSpace::Definite(x) => Some(x),
                        _ => None,
                    });
                    if let Some((cached_w, cached_sz)) = *cache.borrow()
                        && (wrap_width.is_none() || wrap_width == cached_w)
                    {
                        return cached_sz;
                    }
                    let lines = wts.shape_text(
                        text.into(),
                        px(12.),
                        &font,
                        crate::ui::black(),
                        None,
                        wrap_width,
                        None,
                    );
                    let mut sz: Size<Pixels> = Size::default();
                    for line in &lines {
                        let ls = line.size(px(16.));
                        sz.height += ls.height;
                        sz.width = sz.width.max(ls.width).ceil();
                    }
                    *cache.borrow_mut() = Some((wrap_width, sz));
                    sz
                })
            };
            let t0 = text_node(engine, "Ln 1, Col 1");
            let d1 = engine.req(s().flex_none().w(px(1.)).h(px(16.)), &[], scale);
            let t2 = text_node(engine, "0 characters");
            let sp = engine.req(s().flex_1().min_w(px(1.)).h(px(16.)), &[], scale);
            let t4 = text_node(engine, "100%");
            let d5 = engine.req(s().flex_none().w(px(1.)).h(px(16.)), &[], scale);
            let t6 = text_node(engine, "Windows (CRLF)");
            let d7 = engine.req(s().flex_none().w(px(1.)).h(px(16.)), &[], scale);
            let t8 = text_node(engine, "UTF-8");
            let row = engine.req(
                s().flex()
                    .items_center()
                    .gap(px(24.))
                    .h(px(31.))
                    .px(px(22.)),
                &[t0, d1, t2, sp, t4, d5, t6, d7, t8],
                scale,
            );
            let sep = engine.req(s().flex_none().h(px(1.)), &[], scale);
            let bar = engine.req(s().flex_none(), &[sep, row], scale);
            (bar, [t0, d1, t2, sp, t4, d5, t6, d7, t8])
        };

        let mut probe = LayoutEngine::new();
        let (bar_probe, _) = make_status(&mut probe, 1.0);
        probe.run(bar_probe, AvailableSpace::min_size(), 1.0);
        let min_sz = probe.layout_bounds(bar_probe, 1.0).size;

        let mut win = LayoutEngine::new();
        let (bar_win, kids) = make_status(&mut win, 1.0);
        let root = win.req(s().size_full().flex().flex_col(), &[bar_win], 1.0);
        win.run(root, size(min_sz.width, px(200.)).into(), 1.0);

        let spacer = win.layout_bounds(kids[3], 1.0);
        let zoom = win.layout_bounds(kids[4], 1.0);
        let next_div = win.layout_bounds(kids[5], 1.0);
        assert_eq!(spacer.size.width, px(1.));
        assert!(zoom.size.width > px(20.), "100% width was {:?}", zoom.size.width);
        assert_eq!(zoom.left(), spacer.right() + px(24.));
        assert_eq!(next_div.left(), zoom.right() + px(24.));
    }
}
