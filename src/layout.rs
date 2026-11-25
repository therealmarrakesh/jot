//! A flexbox subset solved in physical pixels. Axis 0 is horizontal, axis 1 vertical.

use std::ops::Range;

use crate::geom::{Edges, Point, Rect, Size, point, size};

fn round_half_toward_zero(value: f32) -> f32 {
    (value.abs() - 0.5).ceil().copysign(value)
}

pub(crate) fn round_px(logical: f32, scale: f32) -> f32 {
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

/// Layout tree, laid out in pixels at `scale`; node ids are insertion order.
pub(crate) struct LayoutTree {
    pub(crate) scale: f32,
    nodes: Vec<Node>,
    kids: Vec<usize>,
}

impl LayoutTree {
    pub(crate) fn new(scale: f32) -> Self {
        Self {
            scale,
            nodes: Vec::new(),
            kids: Vec::new(),
        }
    }

    pub(crate) fn add(&mut self, style: FlexStyle, measured: bool) -> usize {
        self.nodes.push(Node {
            style: style.snap_for_scale(self.scale),
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
        measure: &mut impl FnMut(usize, Option<f32>, Option<f32>, AvailSize) -> Size,
    ) {
        let scale = self.scale;
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
    pub(crate) fn rect(&self, id: usize) -> Rect {
        let scale = self.scale;
        let (o, s) = (self.origin(id), self.nodes[id].size);
        let snap = |x: f32, y: f32| point(round_half_toward_zero(x), round_half_toward_zero(y));
        let px = Rect::from_corners(snap(o.x, o.y), snap(o.x + s.width, o.y + s.height));
        Rect::new(
            point(px.origin.x / scale, px.origin.y / scale),
            size(px.size.width / scale, px.size.height / scale),
        )
    }

    /// Moves `id` so its top-left lands on `logical`.
    pub(crate) fn set_origin(&mut self, id: usize, logical: Point) {
        let parent = self.nodes[id].parent.map_or(Point::ZERO, |p| self.origin(p));
        let px = |v: f32| round_px(v, self.scale);
        self.nodes[id].pos = point(px(logical.x), px(logical.y)) - parent;
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

    /// Sizes `id` under `req`; the layout pass also positions its children.
    fn solve(&mut self, id: usize, req: Req) -> Size {
        let (n, all_kids) = (&self.nodes[id], self.kids);
        let (style, measured, kids) = (n.style, n.measured, &all_kids[n.kids.clone()]);
        let b = [0, 1].map(|a| style.bounds(a, req.parent[a], req.pass != Pass::Content));
        let pb = [0, 1].map(|a| style.pb(a));
        let outer = [0, 1].map(|a| b[a].definite(req.known[a]));

        // Text leaves size to their measured content; boxes with nothing in flow to their padding.
        if measured || kids.iter().all(|&k| self.nodes[k].style.absolute) {
            let content = (measured && (outer.contains(&None) || req.pass == Pass::Layout)).then(|| {
                let inv = 1.0 / self.scale;
                let [kw, kh] = [0, 1].map(|a| outer[a].map(|v| (v - pb[a]).max(0.0) * inv));
                let [width, height] = [0, 1].map(|a| {
                    let space = req.avail[a].maybe_set(outer[a]).maybe_min(b[a].max);
                    space.maybe_max(b[a].min).maybe_sub(pb[a]).scale(inv)
                });
                let s = (self.measure)(id, kw, kh, AvailSize { width, height });
                [s.width, s.height].map(|v| (v.max(0.0) * self.scale).ceil())
            });
            let [w, h] = [0, 1].map(|a| {
                outer[a].unwrap_or_else(|| b[a].clamp(content.map_or(pb[a], |c| c[a] + pb[a])))
            });
            if req.pass == Pass::Layout {
                self.place_absolute(kids, size(w, h), &style);
            }
            return size(w, h);
        }

        let (m, c) = if style.row { (0, 1) } else { (1, 0) };
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
