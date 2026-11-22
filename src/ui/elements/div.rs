//! Div is the central, reusable container element for building UI trees.

use crate::ui::Refineable;
use crate::ui::collections::HashMap;
use crate::ui::smallvec::SmallVec;
use crate::ui::{
    Action, AnyElement, App, Bounds, ClickEvent, DispatchPhase, Display, Element, ElementId,
    ExternalPaths, FocusHandle, Global, GlobalElementId, Hitbox, HitboxBehavior, HitboxId,
    IntoElement, KeyContext, KeyDownEvent, KeyUpEvent, LayoutId, MouseButton, MouseClickEvent,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Point,
    ScrollWheelEvent, SharedString, Size, Style, StyleRefinement, Styled, Visibility, Window,
    WindowControlArea, point,
};
use std::{
    any::{Any, TypeId},
    cell::{Cell, RefCell},
    mem,
    rc::Rc,
};

/// The styling information for a given group.
pub(crate) struct GroupStyle {
    /// The identifier for this group.
    pub(crate) group: SharedString,

    /// The specific style refinement that this group would apply
    /// to its children.
    pub(crate) style: Box<StyleRefinement>,
}

impl Interactivity {
    /// Create a default `Interactivity`.
    pub(crate) fn new() -> Interactivity {
        Interactivity::default()
    }

    /// Bind the given callback to the mouse down event for the given mouse button, during the bubble phase.
    pub(crate) fn on_mouse_down(
        &mut self,
        button: MouseButton,
        listener: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) {
        self.mouse_down_listeners
            .push(Box::new(move |event, phase, hitbox, window, cx| {
                if phase == DispatchPhase::Bubble
                    && event.button == button
                    && hitbox.is_hovered(window)
                {
                    (listener)(event, window, cx)
                }
            }));
    }

    /// Bind the given callback to the mouse down event for any button, during the capture phase.
    pub(crate) fn capture_any_mouse_down(
        &mut self,
        listener: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) {
        self.mouse_down_listeners
            .push(Box::new(move |event, phase, hitbox, window, cx| {
                if phase == DispatchPhase::Capture && hitbox.is_hovered(window) {
                    (listener)(event, window, cx)
                }
            }));
    }

    /// Bind the given callback to the mouse up event for the given button, during the bubble phase.
    pub(crate) fn on_mouse_up(
        &mut self,
        button: MouseButton,
        listener: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
    ) {
        self.mouse_up_listeners
            .push(Box::new(move |event, phase, hitbox, window, cx| {
                if phase == DispatchPhase::Bubble
                    && event.button == button
                    && hitbox.is_hovered(window)
                {
                    (listener)(event, window, cx)
                }
            }));
    }

    /// Bind the given callback to the mouse up event for any button, during the capture phase.
    pub(crate) fn capture_any_mouse_up(
        &mut self,
        listener: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
    ) {
        self.mouse_up_listeners
            .push(Box::new(move |event, phase, hitbox, window, cx| {
                if phase == DispatchPhase::Capture && hitbox.is_hovered(window) {
                    (listener)(event, window, cx)
                }
            }));
    }

    /// Bind the given callback to the mouse down event, on any button, during the capture phase,
    /// when the mouse is outside of the bounds of this element.
    pub(crate) fn on_mouse_down_out(
        &mut self,
        listener: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) {
        self.mouse_down_listeners
            .push(Box::new(move |event, phase, hitbox, window, cx| {
                if phase == DispatchPhase::Capture && !hitbox.contains(&window.mouse_position()) {
                    (listener)(event, window, cx)
                }
            }));
    }

    /// Bind the given callback to the mouse up event, for the given button, during the capture phase,
    /// when the mouse is outside of the bounds of this element.
    pub(crate) fn on_mouse_up_out(
        &mut self,
        button: MouseButton,
        listener: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
    ) {
        self.mouse_up_listeners
            .push(Box::new(move |event, phase, hitbox, window, cx| {
                if phase == DispatchPhase::Capture
                    && event.button == button
                    && !hitbox.is_hovered(window)
                {
                    (listener)(event, window, cx);
                }
            }));
    }

    /// Bind the given callback to scroll wheel events during the bubble phase.
    pub(crate) fn on_scroll_wheel(
        &mut self,
        listener: impl Fn(&ScrollWheelEvent, &mut Window, &mut App) + 'static,
    ) {
        self.scroll_wheel_listeners
            .push(Box::new(move |event, phase, hitbox, window, cx| {
                if phase == DispatchPhase::Bubble && hitbox.should_handle_scroll(window) {
                    (listener)(event, window, cx);
                }
            }));
    }

    /// Bind the given callback to an action dispatch during the bubble phase.
    #[track_caller]
    #[inline(always)]
    pub(crate) fn on_action<A: Action>(&mut self, listener: impl Fn(&A, &mut Window, &mut App) + 'static) {
        self.action_listeners.push((
            TypeId::of::<A>(),
            Box::new(move |action, phase, window, cx| {
                let action = action.downcast_ref().unwrap();
                if phase == DispatchPhase::Bubble {
                    (listener)(action, window, cx)
                }
            }),
        ));
    }

    /// Bind the given callback to drop events of the given type.
    pub(crate) fn on_drop<T: 'static>(&mut self, listener: impl Fn(&T, &mut Window, &mut App) + 'static) {
        self.drop_listeners.push((
            TypeId::of::<T>(),
            Box::new(move |dragged_value, window, cx| {
                listener(dragged_value.downcast_ref().unwrap(), window, cx);
            }),
        ));
    }

    /// Bind the given callback to click events of this element.
    #[inline(always)]
    pub(crate) fn on_click(&mut self, listener: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static)
    where
        Self: Sized,
    {
        self.click_listeners.push(Rc::new(listener));
    }

    /// Bind the given callback to non-primary click events of this element.
    pub(crate) fn on_aux_click(&mut self, listener: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static)
    where
        Self: Sized,
    {
        self.aux_click_listeners.push(Rc::new(listener));
    }

    /// Bind the given callback on the hover start and end events of this element.
    pub(crate) fn on_hover(&mut self, listener: impl Fn(&bool, &mut Window, &mut App) + 'static)
    where
        Self: Sized,
    {
        debug_assert!(
            self.hover_listener.is_none(),
            "calling on_hover more than once on the same element is not supported"
        );
        self.hover_listener = Some(Box::new(listener));
    }

    /// Block the mouse from all interactions with elements behind this element's hitbox.
    pub(crate) fn occlude_mouse(&mut self) {
        self.hitbox_behavior = HitboxBehavior::BlockMouse;
    }

    /// Set the bounds of this element as a window control area for the platform window.
    pub(crate) fn window_control_area(&mut self, area: WindowControlArea) {
        self.window_control = Some(area);
    }
}

/// A trait for elements that want to use the standard GPUI event handlers that don't
/// require any state.
pub(crate) trait InteractiveElement: Sized {
    /// Retrieve the interactivity state associated with this element.
    fn interactivity(&mut self) -> &mut Interactivity;

    /// Assign this element to a group of elements that can be styled together.
    fn group(mut self, group: impl Into<SharedString>) -> Self {
        self.interactivity().group = Some(group.into());
        self
    }

    /// Assign this element an ID, so that it can be used with interactivity.
    fn id(mut self, id: impl Into<ElementId>) -> Stateful<Self> {
        self.interactivity().element_id = Some(id.into());
        Stateful { element: self }
    }

    /// Track the focus state of the given focus handle on this element.
    fn track_focus(mut self, focus_handle: &FocusHandle) -> Self {
        self.interactivity().tracked_focus_handle = Some(focus_handle.clone());
        self
    }

    /// Set the keymap context for this element.
    fn key_context(mut self, key_context: impl Into<KeyContext>) -> Self {
        self.interactivity().key_context = Some(key_context.into());
        self
    }

    /// Apply the given style to this element when the mouse hovers over it.
    fn hover(mut self, f: impl FnOnce(StyleRefinement) -> StyleRefinement) -> Self {
        debug_assert!(
            self.interactivity().hover_style.is_none(),
            "hover style already set"
        );
        self.interactivity().hover_style = Some(Box::new(f(StyleRefinement::default())));
        self
    }

    /// Apply the given style to this element when the mouse hovers over a group member.
    fn group_hover(
        mut self,
        group_name: impl Into<SharedString>,
        f: impl FnOnce(StyleRefinement) -> StyleRefinement,
    ) -> Self {
        self.interactivity().group_hover_style = Some(GroupStyle {
            group: group_name.into(),
            style: Box::new(f(StyleRefinement::default())),
        });
        self
    }

    /// Bind the given callback to the mouse down event for the given mouse button.
    #[inline(always)]
    fn on_mouse_down(
        mut self,
        button: MouseButton,
        listener: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.interactivity().on_mouse_down(button, listener);
        self
    }

    /// Bind the given callback to the mouse down event for any button, during the capture phase.
    fn capture_any_mouse_down(
        mut self,
        listener: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.interactivity().capture_any_mouse_down(listener);
        self
    }

    /// Bind the given callback to the mouse up event for the given button, during the bubble phase.
    fn on_mouse_up(
        mut self,
        button: MouseButton,
        listener: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.interactivity().on_mouse_up(button, listener);
        self
    }

    /// Bind the given callback to the mouse down event, on any button, during the capture phase,
    /// when the mouse is outside of the bounds of this element.
    fn on_mouse_down_out(
        mut self,
        listener: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.interactivity().on_mouse_down_out(listener);
        self
    }

    /// Bind the given callback to the mouse up event, for the given button, during the capture phase,
    /// when the mouse is outside of the bounds of this element.
    fn on_mouse_up_out(
        mut self,
        button: MouseButton,
        listener: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.interactivity().on_mouse_up_out(button, listener);
        self
    }

    /// Bind the given callback to scroll wheel events during the bubble phase.
    fn on_scroll_wheel(
        mut self,
        listener: impl Fn(&ScrollWheelEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.interactivity().on_scroll_wheel(listener);
        self
    }

    /// Bind the given callback to an action dispatch during the bubble phase.
    #[track_caller]
    #[inline(always)]
    fn on_action<A: Action>(
        mut self,
        listener: impl Fn(&A, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.interactivity().on_action(listener);
        self
    }

    /// Bind the given callback to drop events of the given type.
    fn on_drop<T: 'static>(
        mut self,
        listener: impl Fn(&T, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.interactivity().on_drop(listener);
        self
    }

    /// Block the mouse from all interactions with elements behind this element's hitbox.
    fn occlude(mut self) -> Self {
        self.interactivity().occlude_mouse();
        self
    }

    /// Set the bounds of this element as a window control area for the platform window.
    fn window_control_area(mut self, area: WindowControlArea) -> Self {
        self.interactivity().window_control_area(area);
        self
    }
}

/// A trait for elements that want to use stateful GPUI interactivity features.
pub(crate) trait StatefulInteractiveElement: InteractiveElement {
    /// Set the given styles to be applied when this element is active.
    fn active(mut self, f: impl FnOnce(StyleRefinement) -> StyleRefinement) -> Self
    where
        Self: Sized,
    {
        self.interactivity().active_style = Some(Box::new(f(StyleRefinement::default())));
        self
    }

    /// Bind the given callback to click events of this element.
    #[inline(always)]
    fn on_click(mut self, listener: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self
    where
        Self: Sized,
    {
        self.interactivity().on_click(listener);
        self
    }

    /// Bind the given callback to non-primary click events of this element.
    fn on_aux_click(
        mut self,
        listener: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self
    where
        Self: Sized,
    {
        self.interactivity().on_aux_click(listener);
        self
    }

    /// Bind the given callback on the hover start and end events of this element.
    fn on_hover(mut self, listener: impl Fn(&bool, &mut Window, &mut App) + 'static) -> Self
    where
        Self: Sized,
    {
        self.interactivity().on_hover(listener);
        self
    }
}

pub(crate) type MouseDownListener =
    Box<dyn Fn(&MouseDownEvent, DispatchPhase, &Hitbox, &mut Window, &mut App) + 'static>;
pub(crate) type MouseUpListener =
    Box<dyn Fn(&MouseUpEvent, DispatchPhase, &Hitbox, &mut Window, &mut App) + 'static>;
pub(crate) type ScrollWheelListener =
    Box<dyn Fn(&ScrollWheelEvent, DispatchPhase, &Hitbox, &mut Window, &mut App) + 'static>;
pub(crate) type ClickListener = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;
type DropListener = Box<dyn Fn(&dyn Any, &mut Window, &mut App) + 'static>;
pub(crate) type ActionListener =
    Box<dyn Fn(&dyn Any, DispatchPhase, &mut Window, &mut App) + 'static>;

/// Construct a new [`Div`] element.
#[track_caller]
pub(crate) fn div() -> Div {
    Div {
        interactivity: Interactivity::new(),
        children: SmallVec::default(),
    }
}

/// A [`Div`] element, the container element for building UIs.
pub(crate) struct Div {
    interactivity: Interactivity,
    children: SmallVec<[AnyElement; 2]>,
}

/// A frame state for a `Div` element, holding layout IDs for its children.
pub(crate) struct DivFrameState {
    child_layout_ids: SmallVec<[LayoutId; 2]>,
}

impl Styled for Div {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.interactivity.base_style
    }
}

impl InteractiveElement for Div {
    fn interactivity(&mut self) -> &mut Interactivity {
        &mut self.interactivity
    }
}

impl ParentElement for Div {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl Element for Div {
    type RequestLayoutState = DivFrameState;
    type PrepaintState = Option<Hitbox>;

    fn id(&self) -> Option<ElementId> {
        self.interactivity.element_id.clone()
    }

    fn request_layout(
        &mut self,
        global_id: Option<&GlobalElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut child_layout_ids = SmallVec::new();

        let layout_id = self.interactivity.request_layout(
            global_id,
            window,
            cx,
            |style, window, cx| {
                window.with_text_style(style.text_style().cloned(), |window| {
                    child_layout_ids = self
                        .children
                        .iter_mut()
                        .map(|child| child.request_layout(window, cx))
                        .collect::<SmallVec<_>>();
                    window.request_layout(style, child_layout_ids.iter().copied(), cx)
                })
            },
        );

        (layout_id, DivFrameState { child_layout_ids })
    }

    fn prepaint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Hitbox> {
        let mut child_min = point(Pixels::MAX, Pixels::MAX);
        let mut child_max = Point::default();
        let content_size = if request_layout.child_layout_ids.is_empty() {
            bounds.size
        } else {
            for child_layout_id in &request_layout.child_layout_ids {
                let child_bounds = window.layout_bounds(*child_layout_id);
                child_min = child_min.min(&child_bounds.origin);
                child_max = child_max.max(&child_bounds.bottom_right());
            }
            (child_max - child_min).into()
        };

        self.interactivity.prepaint(
            global_id,
            bounds,
            content_size,
            window,
            cx,
            |style, scroll_offset, hitbox, window, cx| {
                if style.display == Display::None {
                    return hitbox;
                }

                window.with_element_offset(scroll_offset, |window| {
                    for child in &mut self.children {
                        child.prepaint(window, cx);
                    }
                });

                hitbox
            },
        )
    }

    fn paint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        hitbox: &mut Option<Hitbox>,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.interactivity.paint(
            global_id,
            bounds,
            hitbox.as_ref(),
            window,
            cx,
            |style, window, cx| {
                if style.display == Display::None {
                    return;
                }

                for child in &mut self.children {
                    child.paint(window, cx);
                }
            },
        );
    }
}

impl IntoElement for Div {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

/// The interactivity struct powering `Div`.
#[derive(Default)]
pub(crate) struct Interactivity {
    pub(crate) element_id: Option<ElementId>,
    pub(crate) active: Option<bool>,
    pub(crate) hovered: Option<bool>,
    pub(crate) content_size: Size<Pixels>,
    pub(crate) key_context: Option<KeyContext>,
    pub(crate) tracked_focus_handle: Option<FocusHandle>,
    pub(crate) group: Option<SharedString>,
    pub(crate) base_style: Box<StyleRefinement>,
    pub(crate) hover_style: Option<Box<StyleRefinement>>,
    pub(crate) group_hover_style: Option<GroupStyle>,
    pub(crate) active_style: Option<Box<StyleRefinement>>,
    pub(crate) mouse_down_listeners: Vec<MouseDownListener>,
    pub(crate) mouse_up_listeners: Vec<MouseUpListener>,
    pub(crate) scroll_wheel_listeners: Vec<ScrollWheelListener>,
    pub(crate) action_listeners: Vec<(TypeId, ActionListener)>,
    pub(crate) drop_listeners: Vec<(TypeId, DropListener)>,
    pub(crate) click_listeners: Vec<ClickListener>,
    pub(crate) aux_click_listeners: Vec<ClickListener>,
    pub(crate) hover_listener: Option<Box<dyn Fn(&bool, &mut Window, &mut App)>>,
    pub(crate) window_control: Option<WindowControlArea>,
    pub(crate) hitbox_behavior: HitboxBehavior,
}

impl Interactivity {
    /// Layout this element according to this interactivity state's configured styles.
    pub(crate) fn request_layout(
        &mut self,
        global_id: Option<&GlobalElementId>,
        window: &mut Window,
        cx: &mut App,
        f: impl FnOnce(Style, &mut Window, &mut App) -> LayoutId,
    ) -> LayoutId {
        window.with_optional_element_state::<InteractiveElementState, _>(
            global_id,
            |element_state, window| {
                let mut element_state =
                    element_state.map(|element_state| element_state.unwrap_or_default());
                let style = self.compute_style_internal(None, element_state.as_mut(), window, cx);
                let layout_id = f(style, window, cx);
                (layout_id, element_state)
            },
        )
    }

    /// Commit the bounds of this element according to this interactivity state's configured styles.
    pub(crate) fn prepaint<R>(
        &mut self,
        global_id: Option<&GlobalElementId>,
        bounds: Bounds<Pixels>,
        content_size: Size<Pixels>,
        window: &mut Window,
        cx: &mut App,
        f: impl FnOnce(&Style, Point<Pixels>, Option<Hitbox>, &mut Window, &mut App) -> R,
    ) -> R {
        self.content_size = content_size;

        if let Some(focus_handle) = self.tracked_focus_handle.as_ref() {
            window.set_focus_handle(focus_handle, cx);
        }
        window.with_optional_element_state::<InteractiveElementState, _>(
            global_id,
            |element_state, window| {
                let mut element_state =
                    element_state.map(|element_state| element_state.unwrap_or_default());
                let style = self.compute_style_internal(None, element_state.as_mut(), window, cx);

                if let Some(element_state) = element_state.as_mut() {
                    if let Some(clicked_state) = element_state.clicked_state.as_ref() {
                        self.active = Some(clicked_state.get());
                    }
                    if self.hover_style.is_some() || self.group_hover_style.is_some() {
                        element_state
                            .hover_state
                            .get_or_insert_with(Default::default);
                    }
                }

                window.with_text_style(style.text_style().cloned(), |window| {
                    window.with_content_mask(
                        style.overflow_mask(bounds, window.rem_size()),
                        |window| {
                            let hitbox = if self.should_insert_hitbox(&style) {
                                Some(window.insert_hitbox(bounds, self.hitbox_behavior))
                            } else {
                                None
                            };

                            let result = f(&style, Point::default(), hitbox, window, cx);
                            (result, element_state)
                        },
                    )
                })
            },
        )
    }

    fn should_insert_hitbox(&self, style: &Style) -> bool {
        self.hitbox_behavior != HitboxBehavior::Normal
            || self.window_control.is_some()
            || style.mouse_cursor.is_some()
            || self.group.is_some()
            || self.tracked_focus_handle.is_some()
            || self.hover_style.is_some()
            || self.group_hover_style.is_some()
            || self.hover_listener.is_some()
            || !self.mouse_up_listeners.is_empty()
            || !self.mouse_down_listeners.is_empty()
            || !self.click_listeners.is_empty()
            || !self.aux_click_listeners.is_empty()
            || !self.scroll_wheel_listeners.is_empty()
            || !self.drop_listeners.is_empty()
    }

    /// Paint this element according to this interactivity state's configured styles
    /// and bind the element's mouse and keyboard events.
    pub(crate) fn paint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        bounds: Bounds<Pixels>,
        hitbox: Option<&Hitbox>,
        window: &mut Window,
        cx: &mut App,
        f: impl FnOnce(&Style, &mut Window, &mut App),
    ) {
        self.hovered = hitbox.map(|hitbox| hitbox.is_hovered(window));
        window.with_optional_element_state::<InteractiveElementState, _>(
            global_id,
            |element_state, window| {
                let mut element_state =
                    element_state.map(|element_state| element_state.unwrap_or_default());

                let style = self.compute_style_internal(hitbox, element_state.as_mut(), window, cx);

                self.paint_hover_group_handler(window, cx);

                if style.visibility == Visibility::Hidden {
                    return ((), element_state);
                }

                window.with_element_opacity(style.opacity, |window| {
                    style.paint(bounds, window, cx, |window: &mut Window, cx: &mut App| {
                        window.with_text_style(style.text_style().cloned(), |window| {
                            window.with_content_mask(
                                style.overflow_mask(bounds, window.rem_size()),
                                |window| {
                                    if let Some(hitbox) = hitbox {
                                        if let Some(mouse_cursor) = style.mouse_cursor {
                                            window.set_cursor_style(mouse_cursor, hitbox);
                                        }

                                        if let Some(group) = self.group.clone() {
                                            GroupHitboxes::push(group, hitbox.id, cx);
                                        }

                                        if let Some(area) = self.window_control {
                                            window.insert_window_control_hitbox(
                                                area,
                                                hitbox.clone(),
                                            );
                                        }

                                        self.paint_mouse_listeners(
                                            hitbox,
                                            element_state.as_mut(),
                                            window,
                                            cx,
                                        );
                                    }

                                    self.paint_keyboard_listeners(window);

                                    f(&style, window, cx);

                                    if hitbox.is_some()
                                        && let Some(group) = self.group.as_ref()
                                    {
                                        GroupHitboxes::pop(group, cx);
                                    }
                                },
                            );
                        });
                    });
                });

                ((), element_state)
            },
        );
    }

    fn paint_mouse_listeners(
        &mut self,
        hitbox: &Hitbox,
        element_state: Option<&mut InteractiveElementState>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let is_focused = self
            .tracked_focus_handle
            .as_ref()
            .is_some_and(|handle| handle.is_focused(window));

        if let Some(focus_handle) = self.tracked_focus_handle.clone() {
            let hitbox = hitbox.clone();
            window.on_mouse_event(move |_: &MouseDownEvent, phase, window, cx| {
                if phase == DispatchPhase::Bubble
                    && hitbox.is_hovered(window)
                    && !window.default_prevented()
                {
                    window.focus(&focus_handle, cx);
                    window.prevent_default();
                }
            });
        }

        for listener in self.mouse_down_listeners.drain(..) {
            let hitbox = hitbox.clone();
            window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                listener(event, phase, &hitbox, window, cx);
            })
        }

        for listener in self.mouse_up_listeners.drain(..) {
            let hitbox = hitbox.clone();
            window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                listener(event, phase, &hitbox, window, cx);
            })
        }

        for listener in self.scroll_wheel_listeners.drain(..) {
            let hitbox = hitbox.clone();
            window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, cx| {
                listener(event, phase, &hitbox, window, cx);
            })
        }

        if self.hover_style.is_some() || self.base_style.mouse_cursor.is_some() {
            let hitbox = hitbox.clone();
            let hover_state = self.hover_style.as_ref().and_then(|_| {
                element_state
                    .as_ref()
                    .and_then(|state| state.hover_state.as_ref())
                    .cloned()
            });
            let current_view = window.current_view();

            window.on_mouse_event(move |_: &MouseMoveEvent, phase, window, cx| {
                let hovered = hitbox.is_hovered(window);
                let was_hovered = hover_state
                    .as_ref()
                    .is_some_and(|state| state.borrow().element);
                if phase == DispatchPhase::Capture
                    && hovered != was_hovered
                    && let Some(hover_state) = &hover_state
                {
                    hover_state.borrow_mut().element = hovered;
                    cx.notify(current_view);
                }
            });
        }

        if let Some(group_hover) = self.group_hover_style.as_ref()
            && let Some(group_hitbox_id) = GroupHitboxes::get(&group_hover.group, cx)
        {
            let hover_state = element_state
                .as_ref()
                .and_then(|element| element.hover_state.as_ref())
                .cloned();
            let current_view = window.current_view();

            window.on_mouse_event(move |_: &MouseMoveEvent, phase, window, cx| {
                let group_hovered = group_hitbox_id.is_hovered(window);
                let was_group_hovered = hover_state
                    .as_ref()
                    .is_some_and(|state| state.borrow().group);
                if phase == DispatchPhase::Capture
                    && group_hovered != was_group_hovered
                    && let Some(hover_state) = &hover_state
                {
                    hover_state.borrow_mut().group = group_hovered;
                    cx.notify(current_view);
                }
            });
        }

        let drop_listeners = mem::take(&mut self.drop_listeners);
        let click_listeners = mem::take(&mut self.click_listeners);
        let aux_click_listeners = mem::take(&mut self.aux_click_listeners);

        if !drop_listeners.is_empty() {
            let hitbox = hitbox.clone();
            window.on_mouse_event({
                move |_: &MouseUpEvent, phase, window, cx| {
                    if cx.active_file_drop.is_some()
                        && phase == DispatchPhase::Bubble
                        && hitbox.is_hovered(window)
                    {
                        for (drop_state_type, listener) in &drop_listeners {
                            if *drop_state_type == TypeId::of::<ExternalPaths>()
                                && let Some(paths) = cx.active_file_drop.take()
                            {
                                listener(&paths, window, cx);
                                window.refresh();
                                cx.stop_propagation();
                            }
                        }
                    }
                }
            });
        }

        if let Some(element_state) = element_state {
            if !click_listeners.is_empty() || !aux_click_listeners.is_empty() {
                let pending_mouse_down = element_state
                    .pending_mouse_down
                    .get_or_insert_with(Default::default)
                    .clone();

                let pending_keyboard_down = element_state
                    .pending_keyboard_down
                    .get_or_insert_with(Default::default)
                    .clone();

                window.on_mouse_event({
                    let pending_mouse_down = pending_mouse_down.clone();
                    let hitbox = hitbox.clone();
                    let has_aux_click_listeners = !aux_click_listeners.is_empty();
                    move |event: &MouseDownEvent, phase, window, _cx| {
                        if phase == DispatchPhase::Bubble
                            && (event.button == MouseButton::Left || has_aux_click_listeners)
                            && hitbox.is_hovered(window)
                        {
                            *pending_mouse_down.borrow_mut() = Some(event.clone());
                            window.refresh();
                        }
                    }
                });

                if is_focused {
                    window.on_key_event({
                        let pending_keyboard_down = pending_keyboard_down.clone();
                        move |event: &KeyDownEvent, phase, window, _cx| {
                            if phase.bubble() && !window.default_prevented() {
                                let stroke = &event.keystroke;
                                let is_activation_key = (stroke.key.eq("enter")
                                    || stroke.key.eq("space"))
                                    && !stroke.modifiers.modified();
                                pending_keyboard_down.set(
                                    is_activation_key.then_some(window.focus_generation),
                                );
                            }
                        }
                    });

                    window.on_key_event({
                        let click_listeners = click_listeners.clone();
                        move |event: &KeyUpEvent, phase, window, cx| {
                            if phase.bubble() && !window.default_prevented() {
                                let stroke = &event.keystroke;
                                let is_activation_key = (stroke.key.eq("enter")
                                    || stroke.key.eq("space"))
                                    && !stroke.modifiers.modified();

                                if is_activation_key {
                                    let pending = pending_keyboard_down.take();
                                    if pending != Some(window.focus_generation) {
                                        return;
                                    }

                                    let click_event = ClickEvent::Keyboard;
                                    for listener in &click_listeners {
                                        listener(&click_event, window, cx);
                                    }
                                } else {
                                    pending_keyboard_down.set(None);
                                }
                            }
                        }
                    });
                }

                window.on_mouse_event({
                    let mut captured_mouse_down = None;
                    let hitbox = hitbox.clone();
                    move |event: &MouseUpEvent, phase, window, cx| match phase {
                        DispatchPhase::Capture => {
                            let mut pending_mouse_down = pending_mouse_down.borrow_mut();
                            if pending_mouse_down.is_some() && hitbox.is_hovered(window) {
                                captured_mouse_down = pending_mouse_down.take();
                                window.refresh();
                            } else if pending_mouse_down.is_some() {
                                pending_mouse_down.take();
                                window.refresh();
                            }
                        }
                        DispatchPhase::Bubble => {
                            if let Some(mouse_down) = captured_mouse_down.take() {
                                let btn = mouse_down.button;
                                let mouse_click = ClickEvent::Mouse(MouseClickEvent {
                                    down: mouse_down,
                                    up: event.clone(),
                                });

                                match btn {
                                    MouseButton::Left => {
                                        for listener in &click_listeners {
                                            listener(&mouse_click, window, cx);
                                        }
                                    }
                                    _ => {
                                        for listener in &aux_click_listeners {
                                            listener(&mouse_click, window, cx);
                                        }
                                    }
                                }
                            }
                        }
                    }
                });
            }

            if let Some(hover_listener) = self.hover_listener.take() {
                let was_hovered = element_state
                    .hover_listener_state
                    .get_or_insert_with(Default::default)
                    .clone();
                let has_mouse_down = element_state
                    .pending_mouse_down
                    .get_or_insert_with(Default::default)
                    .clone();
                let hover_listener = Rc::new(hover_listener);
                let hover_listener_state = was_hovered.clone();
                let update_hover = move |is_hovered: bool, window: &mut Window, cx: &mut App| {
                    if is_hovered != hover_listener_state.get() {
                        hover_listener_state.set(is_hovered);
                        hover_listener(&is_hovered, window, cx);
                    }
                };

                if has_mouse_down.borrow().is_none() {
                    let is_hovered = hitbox.is_hovered(window);
                    if is_hovered != was_hovered.get() {
                        let update_hover = update_hover.clone();
                        window.defer(cx, move |window, cx| {
                            update_hover(is_hovered, window, cx);
                        });
                    }
                }

                window.on_mouse_event({
                    let update_hover = update_hover.clone();
                    let hitbox = hitbox.clone();
                    move |_: &MouseMoveEvent, phase, window, cx| {
                        if phase == DispatchPhase::Bubble {
                            let is_hovered =
                                has_mouse_down.borrow().is_none() && hitbox.is_hovered(window);
                            update_hover(is_hovered, window, cx);
                        }
                    }
                });
            }

            let active_state = element_state
                .clicked_state
                .get_or_insert_with(Default::default)
                .clone();

            {
                let active_state = active_state.clone();
                window.on_mouse_event(move |_: &MouseUpEvent, phase, window, _cx| {
                    if phase == DispatchPhase::Capture && active_state.get() {
                        active_state.set(false);
                        window.refresh();
                    }
                });
            }

            {
                let hitbox = hitbox.clone();
                window.on_mouse_event(move |_: &MouseDownEvent, phase, window, _cx| {
                    if phase == DispatchPhase::Bubble
                        && !window.default_prevented()
                        && hitbox.is_hovered(window)
                    {
                        active_state.set(true);
                        window.refresh();
                    }
                });
            }
        }
    }

    fn paint_keyboard_listeners(&mut self, window: &mut Window) {
        let action_listeners = mem::take(&mut self.action_listeners);
        if let Some(context) = self.key_context.clone() {
            window.set_key_context(context);
        }

        for (action_type, listener) in action_listeners {
            window.on_action(action_type, listener)
        }
    }

    fn paint_hover_group_handler(&self, window: &mut Window, cx: &mut App) {
        let group_hitbox = self
            .group_hover_style
            .as_ref()
            .and_then(|group_hover| GroupHitboxes::get(&group_hover.group, cx));

        if let Some(group_hitbox) = group_hitbox {
            let was_hovered = group_hitbox.is_hovered(window);
            let current_view = window.current_view();
            window.on_mouse_event(move |_: &MouseMoveEvent, phase, window, cx| {
                let hovered = group_hitbox.is_hovered(window);
                if phase == DispatchPhase::Capture && hovered != was_hovered {
                    cx.notify(current_view);
                }
            });
        }
    }

    fn compute_style_internal(
        &self,
        hitbox: Option<&Hitbox>,
        element_state: Option<&mut InteractiveElementState>,
        window: &mut Window,
        cx: &mut App,
    ) -> Style {
        let mut style = Style::default();
        style.refine(&self.base_style);

        if let Some(group_hover) = self.group_hover_style.as_ref() {
            let is_group_hovered =
                if let Some(group_hitbox_id) = GroupHitboxes::get(&group_hover.group, cx) {
                    group_hitbox_id.is_hovered(window)
                } else if let Some(element_state) = element_state.as_ref() {
                    element_state
                        .hover_state
                        .as_ref()
                        .is_some_and(|state| state.borrow().group)
                } else {
                    false
                };

            if is_group_hovered {
                style.refine(&group_hover.style);
            }
        }

        if let Some(hover_style) = self.hover_style.as_ref() {
            let is_hovered = if let Some(hitbox) = hitbox {
                hitbox.is_hovered(window)
            } else if let Some(element_state) = element_state.as_ref() {
                element_state
                    .hover_state
                    .as_ref()
                    .is_some_and(|state| state.borrow().element)
            } else {
                false
            };

            if is_hovered {
                style.refine(hover_style);
            }
        }

        if let Some(element_state) = element_state {
            let clicked = element_state
                .clicked_state
                .get_or_insert_with(Default::default)
                .get();
            if let Some(active_style) = self.active_style.as_ref()
                && clicked
            {
                style.refine(active_style);
            }
        }

        style
    }
}

/// The per-frame state of an interactive element.
#[derive(Default)]
pub(crate) struct InteractiveElementState {
    pub(crate) clicked_state: Option<Rc<Cell<bool>>>,
    pub(crate) hover_state: Option<Rc<RefCell<ElementHoverState>>>,
    pub(crate) hover_listener_state: Option<Rc<Cell<bool>>>,
    pub(crate) pending_mouse_down: Option<Rc<RefCell<Option<MouseDownEvent>>>>,
    pub(crate) pending_keyboard_down: Option<Rc<Cell<Option<u64>>>>,
}

/// Whether or not the element or a group that contains it is hovered.
#[derive(Copy, Clone, Default, Eq, PartialEq)]
pub(crate) struct ElementHoverState {
    pub(crate) group: bool,
    pub(crate) element: bool,
}

#[derive(Default)]
pub(crate) struct GroupHitboxes(HashMap<SharedString, SmallVec<[HitboxId; 1]>>);

impl Global for GroupHitboxes {}

impl GroupHitboxes {
    pub(crate) fn get(name: &SharedString, cx: &mut App) -> Option<HitboxId> {
        cx.default_global::<Self>()
            .0
            .get(name)
            .and_then(|bounds_stack| bounds_stack.last())
            .cloned()
    }

    pub(crate) fn push(name: SharedString, hitbox_id: HitboxId, cx: &mut App) {
        cx.default_global::<Self>()
            .0
            .entry(name)
            .or_default()
            .push(hitbox_id);
    }

    pub(crate) fn pop(name: &SharedString, cx: &mut App) {
        cx.default_global::<Self>().0.get_mut(name).unwrap().pop();
    }
}

/// A wrapper around an element that can store state, produced after assigning an ElementId.
pub(crate) struct Stateful<E> {
    pub(crate) element: E,
}

impl<E> Styled for Stateful<E>
where
    E: Styled,
{
    fn style(&mut self) -> &mut StyleRefinement {
        self.element.style()
    }
}

impl<E> StatefulInteractiveElement for Stateful<E>
where
    E: Element,
    Self: InteractiveElement,
{
}

impl<E> InteractiveElement for Stateful<E>
where
    E: InteractiveElement,
{
    fn interactivity(&mut self) -> &mut Interactivity {
        self.element.interactivity()
    }
}

impl<E> Element for Stateful<E>
where
    E: Element,
{
    type RequestLayoutState = E::RequestLayoutState;
    type PrepaintState = E::PrepaintState;

    fn id(&self) -> Option<ElementId> {
        self.element.id()
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        self.element.request_layout(id, window, cx)
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        bounds: Bounds<Pixels>,
        state: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> E::PrepaintState {
        self.element.prepaint(id, bounds, state, window, cx)
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.element
            .paint(id, bounds, request_layout, prepaint, window, cx);
    }
}

impl<E> IntoElement for Stateful<E>
where
    E: Element,
{
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl<E> ParentElement for Stateful<E>
where
    E: ParentElement,
{
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.element.extend(elements)
    }
}

/// Shared scroll offset handle.
#[derive(Clone, Debug, Default)]
pub(crate) struct ScrollHandle(Rc<Cell<Point<Pixels>>>);

impl ScrollHandle {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn offset(&self) -> Point<Pixels> {
        self.0.get()
    }

    pub(crate) fn set_offset(&self, position: Point<Pixels>) {
        self.0.set(position);
    }
}
