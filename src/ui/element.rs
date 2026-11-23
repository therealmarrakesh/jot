//! Clean-room element traits, IDs, `AnyElement`, `canvas`, `deferred`, `anchored`,
//! `ScrollHandle`, hitboxes, and IME `EntityInputHandler`.

use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;

use crate::layout::{AvailableSpace, LayoutId};

use super::app::{App, Context};
use super::entity::{Entity, EntityId};
use super::geometry::{Bounds, Pixels, Point, Size, point, px};
use super::style::{Style, StyleRefinement};
use super::styled::Styled;
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

impl From<EntityId> for ElementId {
    fn from(id: EntityId) -> Self {
        Self::Integer(id.0)
    }
}

impl From<(&'static str, usize)> for ElementId {
    fn from((name, ix): (&'static str, usize)) -> Self {
        Self::NamedInteger(SharedString::new_static(name), ix as u64)
    }
}

impl From<(&'static str, EntityId)> for ElementId {
    fn from((name, id): (&'static str, EntityId)) -> Self {
        Self::NamedInteger(SharedString::new_static(name), id.0)
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
        self.bounds.contains(p)
            && self
                .content_mask
                .is_none_or(|mask| mask.contains(p))
    }
}

/// Shared 2D scroll offset handle used by `EditorState` and `Scrollbar`.
#[derive(Clone, Debug, Default)]
pub(crate) struct ScrollHandle(Rc<Cell<Point<Pixels>>>);

impl ScrollHandle {
    pub(crate) fn new() -> Self {
        Self(Rc::new(Cell::new(Point::default())))
    }

    #[inline]
    pub(crate) fn offset(&self) -> Point<Pixels> {
        self.0.get()
    }

    #[inline]
    pub(crate) fn set_offset(&self, offset: Point<Pixels>) {
        self.0.set(offset);
    }
}

/// Core 3-phase UI element protocol (`request_layout` → `prepaint` → `paint`).
pub(crate) trait Element: 'static + IntoElement {
    type RequestLayoutState: 'static;
    type PrepaintState: 'static;

    fn id(&self) -> Option<ElementId>;

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState);

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState;

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    );
}

pub(crate) trait IntoElement: Sized {
    type Element: Element;
    fn into_element(self) -> Self::Element;
    fn into_any_element(self) -> AnyElement {
        AnyElement::new(self.into_element())
    }
}

pub(crate) trait Render: 'static + Sized {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement;
}

pub(crate) trait ParentElement: Sized {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>);

    fn child(mut self, child: impl IntoElement) -> Self {
        self.extend(std::iter::once(child.into_any_element()));
        self
    }

    fn children(mut self, children: impl IntoIterator<Item = impl IntoElement>) -> Self {
        self.extend(children.into_iter().map(IntoElement::into_any_element));
        self
    }
}

trait ElementObject {
    fn request_layout(&mut self, window: &mut Window, cx: &mut App) -> LayoutId;
    fn prepaint(&mut self, window: &mut Window, cx: &mut App);
    fn paint(&mut self, window: &mut Window, cx: &mut App);
    fn layout_as_root(
        &mut self,
        available: Size<AvailableSpace>,
        window: &mut Window,
        cx: &mut App,
    ) -> Size<Pixels>;
    fn prepaint_as_root(
        &mut self,
        origin: Point<Pixels>,
        available: Size<AvailableSpace>,
        window: &mut Window,
        cx: &mut App,
    );
}

struct DrawableElement<E: Element> {
    element: E,
    layout_id: Option<LayoutId>,
    bounds: Bounds<Pixels>,
    request_state: Option<E::RequestLayoutState>,
    prepaint_state: Option<E::PrepaintState>,
}

impl<E: Element> ElementObject for DrawableElement<E> {
    fn request_layout(&mut self, window: &mut Window, cx: &mut App) -> LayoutId {
        let (layout_id, state) = if let Some(id) = self.element.id() {
            window.with_id(id, |gid, window| {
                self.element.request_layout(Some(gid), window, cx)
            })
        } else {
            self.element.request_layout(None, window, cx)
        };
        self.layout_id = Some(layout_id);
        self.request_state = Some(state);
        layout_id
    }

    fn prepaint(&mut self, window: &mut Window, cx: &mut App) {
        let layout_id = self.layout_id.expect("prepaint called before request_layout");
        let bounds = window.layout_bounds(layout_id);
        self.bounds = bounds;
        let req = self.request_state.as_mut().unwrap();
        let pre = if let Some(id) = self.element.id() {
            #[cfg(test)]
            window.record_test_element_bounds(id.clone(), bounds);
            window.with_id(id, |gid, window| {
                self.element.prepaint(Some(gid), bounds, req, window, cx)
            })
        } else {
            self.element.prepaint(None, bounds, req, window, cx)
        };
        self.prepaint_state = Some(pre);
    }

    fn paint(&mut self, window: &mut Window, cx: &mut App) {
        let req = self.request_state.as_mut().unwrap();
        let pre = self.prepaint_state.as_mut().unwrap();
        if let Some(id) = self.element.id() {
            window.with_id(id, |gid, window| {
                self.element
                    .paint(Some(gid), self.bounds, req, pre, window, cx);
            });
        } else {
            self.element.paint(None, self.bounds, req, pre, window, cx);
        }
    }

    fn layout_as_root(
        &mut self,
        available: Size<AvailableSpace>,
        window: &mut Window,
        cx: &mut App,
    ) -> Size<Pixels> {
        let lid = self.request_layout(window, cx);
        window.compute_layout(lid, available, cx);
        window.layout_bounds(lid).size
    }

    fn prepaint_as_root(
        &mut self,
        origin: Point<Pixels>,
        available: Size<AvailableSpace>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let lid = self.request_layout(window, cx);
        window.compute_layout(lid, available, cx);
        window.with_element_offset(origin, |window| {
            self.prepaint(window, cx);
        });
    }
}

/// Boxed type-erased UI element (100% safe Rust; no unsafe element arena).
pub(crate) struct AnyElement(Box<dyn ElementObject>);

impl AnyElement {
    pub(crate) fn new<E: Element>(element: E) -> Self {
        Self(Box::new(DrawableElement {
            element,
            layout_id: None,
            bounds: Bounds::default(),
            request_state: None,
            prepaint_state: None,
        }))
    }

    pub(crate) fn request_layout(&mut self, window: &mut Window, cx: &mut App) -> LayoutId {
        self.0.request_layout(window, cx)
    }

    pub(crate) fn prepaint(&mut self, window: &mut Window, cx: &mut App) {
        self.0.prepaint(window, cx);
    }

    pub(crate) fn paint(&mut self, window: &mut Window, cx: &mut App) {
        self.0.paint(window, cx);
    }

    pub(crate) fn layout_as_root(
        &mut self,
        available: Size<AvailableSpace>,
        window: &mut Window,
        cx: &mut App,
    ) -> Size<Pixels> {
        self.0.layout_as_root(available, window, cx)
    }

    pub(crate) fn prepaint_as_root(
        &mut self,
        origin: Point<Pixels>,
        available: Size<AvailableSpace>,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.0.prepaint_as_root(origin, available, window, cx);
    }
}

impl Element for AnyElement {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.0.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.0.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.0.paint(window, cx);
    }
}

impl IntoElement for AnyElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
    fn into_any_element(self) -> AnyElement {
        self
    }
}

// ---------------------------------------------------------------------------
// IME / text input protocol
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UTF16Selection {
    pub(crate) range: Range<usize>,
}

pub(crate) trait EntityInputHandler: 'static + Sized {
    fn selected_text_range(
        &mut self,
        ignore_disabled_input: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<UTF16Selection>;

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    );

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    );

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>>;
}

pub(crate) trait ErasedInputHandler {
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

pub(crate) struct ElementInputHandler<V: EntityInputHandler> {
    bounds: Bounds<Pixels>,
    entity: Entity<V>,
}

impl<V: EntityInputHandler> ElementInputHandler<V> {
    pub(crate) fn new(bounds: Bounds<Pixels>, entity: Entity<V>) -> Self {
        Self { bounds, entity }
    }
}

impl<V: EntityInputHandler> ErasedInputHandler for ElementInputHandler<V> {
    fn selected_text_range(&mut self, window: &mut Window, cx: &mut App) -> Option<UTF16Selection> {
        self.entity
            .update(cx, |this, cx| this.selected_text_range(false, window, cx))
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.entity.update(cx, |this, cx| {
            this.replace_text_in_range(range_utf16, new_text, window, cx)
        });
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.entity.update(cx, |this, cx| {
            this.replace_and_mark_text_in_range(
                range_utf16,
                new_text,
                new_selected_range_utf16,
                window,
                cx,
            )
        });
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Bounds<Pixels>> {
        let bounds = self.bounds;
        self.entity
            .update(cx, |this, cx| this.bounds_for_range(range_utf16, bounds, window, cx))
    }
}

// ---------------------------------------------------------------------------
// `canvas(...)`, `deferred(...)`, `anchored()`
// ---------------------------------------------------------------------------

type CanvasPrepaint<T> = Box<dyn FnOnce(Bounds<Pixels>, &mut Window, &mut App) -> T>;
type CanvasPaint<T> = Box<dyn FnOnce(Bounds<Pixels>, T, &mut Window, &mut App)>;

pub(crate) struct Canvas<T: 'static> {
    prepaint: Option<CanvasPrepaint<T>>,
    paint: Option<CanvasPaint<T>>,
    style: StyleRefinement,
}

pub(crate) fn canvas<T: 'static>(
    prepaint: impl FnOnce(Bounds<Pixels>, &mut Window, &mut App) -> T + 'static,
    paint: impl FnOnce(Bounds<Pixels>, T, &mut Window, &mut App) + 'static,
) -> Canvas<T> {
    Canvas {
        prepaint: Some(Box::new(prepaint)),
        paint: Some(Box::new(paint)),
        style: StyleRefinement::default(),
    }
}

impl<T: 'static> Styled for Canvas<T> {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl<T: 'static> Element for Canvas<T> {
    type RequestLayoutState = ();
    type PrepaintState = Option<T>;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.refine(&self.style);
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Option<T> {
        self.prepaint.take().map(|f| f(bounds, window, cx))
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        prepaint: &mut Option<T>,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let (Some(f), Some(state)) = (self.paint.take(), prepaint.take()) {
            f(bounds, state, window, cx);
        }
    }
}

impl<T: 'static> IntoElement for Canvas<T> {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

/// Defers prepaint & paint of `child` to run after normal content in ascending `priority` order.
pub(crate) struct Deferred {
    child: Option<AnyElement>,
    priority: usize,
}

pub(crate) fn deferred(child: impl IntoElement) -> Deferred {
    Deferred {
        child: Some(child.into_any_element()),
        priority: 0,
    }
}

impl Deferred {
    pub(crate) fn with_priority(mut self, priority: usize) -> Self {
        self.priority = priority;
        self
    }
}

impl Element for Deferred {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let lid = self
            .child
            .as_mut()
            .expect("deferred child missing")
            .request_layout(window, cx);
        (lid, ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        _cx: &mut App,
    ) {
        if let Some(child) = self.child.take() {
            window.defer_draw(child, self.priority);
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        _: &mut Window,
        _: &mut App,
    ) {
    }
}

impl IntoElement for Deferred {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

/// Positions a popup child at an explicit or layout-derived anchor, clamped to the viewport.
pub(crate) struct Anchored {
    child: Option<AnyElement>,
    anchor_position: Option<Point<Pixels>>,
}

pub(crate) fn anchored() -> Anchored {
    Anchored {
        child: None,
        anchor_position: None,
    }
}

impl Anchored {
    pub(crate) fn position(mut self, pos: Point<Pixels>) -> Self {
        self.anchor_position = Some(pos);
        self
    }

    pub(crate) fn child(mut self, child: impl IntoElement) -> Self {
        self.child = Some(child.into_any_element());
        self
    }
}

impl Element for Anchored {
    type RequestLayoutState = Option<LayoutId>;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Option<LayoutId>) {
        let child_lid = self.child.as_mut().map(|c| c.request_layout(window, cx));
        let style = Style {
            position: super::style::Position::Absolute,
            display: super::style::Display::Flex,
            ..Style::default()
        };
        let lid = window.request_layout(style, child_lid, cx);
        (lid, child_lid)
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        bounds: Bounds<Pixels>,
        child_lid: &mut Option<LayoutId>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let (Some(child), Some(cid)) = (self.child.as_mut(), *child_lid) else {
            return;
        };
        let child_bounds = window.layout_bounds(cid);
        let viewport = window.viewport_size();
        let desired = self.anchor_position.unwrap_or(bounds.origin);
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

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Bounds<Pixels>,
        _: &mut Option<LayoutId>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(child) = self.child.as_mut() {
            child.paint(window, cx);
        }
    }
}

impl IntoElement for Anchored {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
