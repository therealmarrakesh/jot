//! Lean concrete `UiNode` tree, `CustomWidget` escape hatch, `CanvasNode`, `DeferredNode`,
//! `AnchoredNode`, hitboxes, and IME `InputHandler`.

use std::any::Any;
use std::ops::Range;

use crate::layout::{AvailableSpace, LayoutId};

use super::app::App;
use super::div::{Div, TextNode};
use super::geometry::{Bounds, Length, Pixels, Point, Size, point, px, relative};
use super::style::{Position, Style, StyleRefinement};
use super::types::SharedString;
use super::window::Window;

/// Local identifier for an element within its parent's element-ID path.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ElementId {
    Name(SharedString),
    NamedInteger(SharedString, u64),
    Integer(u64),
}

impl From<&'static str> for ElementId {
    fn from(s: &'static str) -> Self {
        Self::Name(SharedString::new_static(s))
    }
}

impl From<SharedString> for ElementId {
    fn from(s: SharedString) -> Self {
        Self::Name(s)
    }
}

impl From<String> for ElementId {
    fn from(s: String) -> Self {
        Self::Name(SharedString::from(s))
    }
}

impl From<usize> for ElementId {
    fn from(n: usize) -> Self {
        Self::Integer(n as u64)
    }
}

impl From<(&'static str, usize)> for ElementId {
    fn from((name, ix): (&'static str, usize)) -> Self {
        Self::NamedInteger(SharedString::new_static(name), ix as u64)
    }
}

/// Hierarchical path of `ElementId`s identifying persistent element state across frames.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct GlobalElementId(pub(crate) Vec<ElementId>);

/// Clipping rectangle pushed onto a window's clip stack.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct ContentMask {
    pub(crate) bounds: Bounds<Pixels>,
}

/// Phase of event propagation.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum DispatchPhase {
    Capture,
    Bubble,
}

impl DispatchPhase {
    #[inline]
    pub(crate) fn bubble(self) -> bool {
        matches!(self, Self::Bubble)
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum HitboxBehavior {
    #[default]
    Normal,
    BlockMouse,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct HitboxId(pub(crate) u64);

#[derive(Clone, Debug)]
pub(crate) struct Hitbox {
    pub(crate) id: HitboxId,
    pub(crate) bounds: Bounds<Pixels>,
    pub(crate) content_mask: Option<Bounds<Pixels>>,
    pub(crate) behavior: HitboxBehavior,
}

impl Hitbox {
    pub(crate) fn is_hovered(&self, window: &Window) -> bool {
        window.is_hitbox_hovered(self.id)
    }

    pub(crate) fn is_hovered_at(&self, pos: Point<Pixels>, window: &Window) -> bool {
        window.is_hitbox_hovered_at(self.id, pos)
    }

    pub(crate) fn contains(&self, p: &Point<Pixels>) -> bool {
        self.bounds.contains(p) && self.content_mask.is_none_or(|mask| mask.contains(p))
    }
}

/// Escape-hatch trait for custom 3-phase widgets (`Editor`, `Scrollbar`, `tooltip::Host`).
pub(crate) trait CustomWidget: 'static {
    fn id(&self) -> Option<ElementId> {
        None
    }

    fn request_layout(&mut self, window: &mut Window, cx: &mut App) -> LayoutId;

    fn prepaint(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App);

    fn paint(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App);
}

pub(crate) struct WidgetNode {
    widget: Box<dyn CustomWidget>,
    layout_id: Option<LayoutId>,
    bounds: Bounds<Pixels>,
}

/// Custom prepaint/paint callback leaf (`canvas(...)`).
type CanvasPrepaintFn = Box<dyn FnOnce(Bounds<Pixels>, &mut Window, &mut App) -> Box<dyn Any>>;
type CanvasPaintFn = Box<dyn FnOnce(Bounds<Pixels>, Box<dyn Any>, &mut Window, &mut App)>;

pub(crate) struct CanvasNode {
    style: StyleRefinement,
    prepaint: Option<CanvasPrepaintFn>,
    paint: Option<CanvasPaintFn>,
    layout_id: Option<LayoutId>,
    bounds: Bounds<Pixels>,
    state: Option<Box<dyn Any>>,
}

pub(crate) fn canvas<T: 'static>(
    prepaint: impl FnOnce(Bounds<Pixels>, &mut Window, &mut App) -> T + 'static,
    paint: impl FnOnce(Bounds<Pixels>, T, &mut Window, &mut App) + 'static,
) -> CanvasNode {
    CanvasNode {
        style: StyleRefinement::default(),
        prepaint: Some(Box::new(move |bounds, window, cx| {
            Box::new(prepaint(bounds, window, cx))
        })),
        paint: Some(Box::new(move |bounds, boxed, window, cx| {
            let state = *boxed
                .downcast::<T>()
                .expect("CanvasNode prepaint state type mismatch");
            paint(bounds, state, window, cx);
        })),
        layout_id: None,
        bounds: Bounds::default(),
        state: None,
    }
}

impl CanvasNode {
    pub(crate) fn absolute(mut self) -> Self {
        self.style.position = Some(Position::Absolute);
        self
    }

    pub(crate) fn size_full(mut self) -> Self {
        self.style.size.width = Some(Length::Definite(relative(1.0)));
        self.style.size.height = Some(Length::Definite(relative(1.0)));
        self
    }

    pub(crate) fn flex_1(mut self) -> Self {
        self.style.flex_grow = Some(1.0);
        self.style.flex_shrink = Some(1.0);
        self.style.flex_basis = Some(Length::Definite(relative(0.0)));
        self
    }

    pub(crate) fn min_w(mut self, width: impl Into<Length>) -> Self {
        self.style.min_size.width = Some(width.into());
        self
    }

    pub(crate) fn h(mut self, height: impl Into<Length>) -> Self {
        self.style.size.height = Some(height.into());
        self
    }
}

/// Defers prepaint & paint of `child` to run after normal content in ascending `priority` order.
pub(crate) struct DeferredNode {
    priority: usize,
    child: Option<Box<UiNode>>,
}

pub(crate) fn deferred(child: impl Into<UiNode>) -> DeferredNode {
    DeferredNode {
        priority: 0,
        child: Some(Box::new(child.into())),
    }
}

impl DeferredNode {
    pub(crate) fn with_priority(mut self, priority: usize) -> Self {
        self.priority = priority;
        self
    }
}

/// Positions a popup child at an explicit or layout-derived anchor, clamped to the viewport.
pub(crate) struct AnchoredNode {
    anchor_position: Option<Point<Pixels>>,
    child_layout_id: Option<LayoutId>,
    layout_id: Option<LayoutId>,
    child: Option<Box<UiNode>>,
}

pub(crate) fn anchored() -> AnchoredNode {
    AnchoredNode {
        anchor_position: None,
        child_layout_id: None,
        layout_id: None,
        child: None,
    }
}

impl AnchoredNode {
    pub(crate) fn position(mut self, pos: Point<Pixels>) -> Self {
        self.anchor_position = Some(pos);
        self
    }

    pub(crate) fn child(mut self, child: impl Into<UiNode>) -> Self {
        self.child = Some(Box::new(child.into()));
        self
    }
}

/// Concrete retained-per-frame UI tree node (replaces `Element` / `IntoElement` / `AnyElement`).
pub(crate) enum UiNode {
    Div(Box<Div>),
    Text(TextNode),
    Canvas(Box<CanvasNode>),
    Deferred(DeferredNode),
    Anchored(AnchoredNode),
    Widget(WidgetNode),
}

impl UiNode {
    pub(crate) fn widget(widget: impl CustomWidget) -> Self {
        Self::Widget(WidgetNode {
            widget: Box::new(widget),
            layout_id: None,
            bounds: Bounds::default(),
        })
    }

    pub(crate) fn request_layout(&mut self, window: &mut Window, cx: &mut App) -> LayoutId {
        match self {
            Self::Div(div) => {
                let lid = if let Some(id) = div.element_id() {
                    window.with_id(id, |_, window| div.request_layout(window, cx))
                } else {
                    div.request_layout(window, cx)
                };
                div.set_layout_id(lid);
                lid
            }
            Self::Text(text) => {
                let lid = text.request_layout(window, cx);
                text.set_layout_id(lid);
                lid
            }
            Self::Canvas(canvas) => {
                let mut style = Style::default();
                style.refine(&canvas.style);
                let lid = window.request_layout(style, [], cx);
                canvas.layout_id = Some(lid);
                lid
            }
            Self::Deferred(def) => def
                .child
                .as_mut()
                .expect("deferred child missing")
                .request_layout(window, cx),
            Self::Anchored(anc) => {
                let child_lid = anc.child.as_mut().map(|c| c.request_layout(window, cx));
                let style = Style {
                    position: Position::Absolute,
                    display: super::style::Display::Flex,
                    ..Style::default()
                };
                let lid = window.request_layout(style, child_lid, cx);
                anc.child_layout_id = child_lid;
                anc.layout_id = Some(lid);
                lid
            }
            Self::Widget(node) => {
                let lid = if let Some(id) = node.widget.id() {
                    window.with_id(id, |_, window| node.widget.request_layout(window, cx))
                } else {
                    node.widget.request_layout(window, cx)
                };
                node.layout_id = Some(lid);
                lid
            }
        }
    }

    pub(crate) fn prepaint(&mut self, window: &mut Window, cx: &mut App) {
        match self {
            Self::Div(div) => {
                let lid = div.layout_id().expect("prepaint called before request_layout");
                let bounds = window.layout_bounds(lid);
                div.set_bounds(bounds);
                if let Some(id) = div.element_id() {
                    #[cfg(test)]
                    window.record_test_element_bounds(id.clone(), bounds);
                    window.with_id(id, |gid, window| div.prepaint(Some(gid), bounds, window, cx));
                } else {
                    div.prepaint(None, bounds, window, cx);
                }
            }
            Self::Text(text) => {
                let lid = text.layout_id().expect("prepaint called before request_layout");
                let bounds = window.layout_bounds(lid);
                text.set_bounds(bounds);
                text.prepaint(bounds, window, cx);
            }
            Self::Canvas(canvas) => {
                let lid = canvas
                    .layout_id
                    .expect("prepaint called before request_layout");
                let bounds = window.layout_bounds(lid);
                canvas.bounds = bounds;
                canvas.state = canvas.prepaint.take().map(|f| f(bounds, window, cx));
            }
            Self::Deferred(def) => {
                if let Some(child) = def.child.take() {
                    window.defer_draw(*child, def.priority);
                }
            }
            Self::Anchored(anc) => {
                let lid = anc
                    .layout_id
                    .expect("prepaint called before request_layout");
                let bounds = window.layout_bounds(lid);
                let (Some(child), Some(cid)) = (anc.child.as_mut(), anc.child_layout_id) else {
                    return;
                };
                let child_bounds = window.layout_bounds(cid);
                let viewport = window.viewport_size();
                let desired = anc.anchor_position.unwrap_or(bounds.origin);
                let clamped = point(
                    desired
                        .x
                        .min((viewport.width - child_bounds.size.width).max(px(0.0)))
                        .max(px(0.0)),
                    desired
                        .y
                        .min((viewport.height - child_bounds.size.height).max(px(0.0)))
                        .max(px(0.0)),
                );
                let offset = clamped - child_bounds.origin;
                window.with_element_offset(offset, |window| {
                    child.prepaint(window, cx);
                });
            }
            Self::Widget(node) => {
                let lid = node
                    .layout_id
                    .expect("prepaint called before request_layout");
                let bounds = window.layout_bounds(lid);
                node.bounds = bounds;
                if let Some(id) = node.widget.id() {
                    #[cfg(test)]
                    window.record_test_element_bounds(id.clone(), bounds);
                    window.with_id(id, |_, window| node.widget.prepaint(bounds, window, cx));
                } else {
                    node.widget.prepaint(bounds, window, cx);
                }
            }
        }
    }

    pub(crate) fn paint(&mut self, window: &mut Window, cx: &mut App) {
        match self {
            Self::Div(div) => {
                let bounds = div.bounds();
                if let Some(id) = div.element_id() {
                    window.with_id(id, |gid, window| div.paint(Some(gid), bounds, window, cx));
                } else {
                    div.paint(None, bounds, window, cx);
                }
            }
            Self::Text(text) => {
                let bounds = text.bounds();
                text.paint(bounds, window, cx);
            }
            Self::Canvas(canvas) => {
                if let (Some(f), Some(state)) = (canvas.paint.take(), canvas.state.take()) {
                    f(canvas.bounds, state, window, cx);
                }
            }
            Self::Deferred(_) => {}
            Self::Anchored(anc) => {
                if let Some(child) = anc.child.as_mut() {
                    child.paint(window, cx);
                }
            }
            Self::Widget(node) => {
                let bounds = node.bounds;
                if let Some(id) = node.widget.id() {
                    window.with_id(id, |_, window| node.widget.paint(bounds, window, cx));
                } else {
                    node.widget.paint(bounds, window, cx);
                }
            }
        }
    }

    pub(crate) fn layout_as_root(
        &mut self,
        available: Size<AvailableSpace>,
        window: &mut Window,
        cx: &mut App,
    ) -> Size<Pixels> {
        let lid = self.request_layout(window, cx);
        window.compute_layout(lid, available, cx);
        window.layout_bounds(lid).size
    }
}

impl From<Div> for UiNode {
    #[inline]
    fn from(div: Div) -> Self {
        Self::Div(Box::new(div))
    }
}

impl From<TextNode> for UiNode {
    #[inline]
    fn from(text: TextNode) -> Self {
        Self::Text(text)
    }
}

impl From<SharedString> for UiNode {
    #[inline]
    fn from(text: SharedString) -> Self {
        Self::Text(TextNode::new(text))
    }
}

impl From<&'static str> for UiNode {
    #[inline]
    fn from(text: &'static str) -> Self {
        Self::Text(TextNode::new(SharedString::new_static(text)))
    }
}

impl From<String> for UiNode {
    #[inline]
    fn from(text: String) -> Self {
        Self::Text(TextNode::new(SharedString::from(text)))
    }
}

impl From<CanvasNode> for UiNode {
    #[inline]
    fn from(canvas: CanvasNode) -> Self {
        Self::Canvas(Box::new(canvas))
    }
}

impl From<DeferredNode> for UiNode {
    #[inline]
    fn from(def: DeferredNode) -> Self {
        Self::Deferred(def)
    }
}

impl From<AnchoredNode> for UiNode {
    #[inline]
    fn from(anc: AnchoredNode) -> Self {
        Self::Anchored(anc)
    }
}

// ---------------------------------------------------------------------------
// IME / text input protocol
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UTF16Selection {
    pub(crate) range: Range<usize>,
}

pub(crate) trait InputHandler: 'static {
    fn selected_text_range(&mut self, window: &mut Window, cx: &mut App) -> Option<UTF16Selection>;
    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        window: &mut Window,
        cx: &mut App,
    );
    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut App,
    );
    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Bounds<Pixels>>;
}
