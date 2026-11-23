//! Clean-room `Div`, `Stateful<Div>`, `Interactivity`, `InteractiveElement`,
//! `StatefulInteractiveElement`, text elements, and `Entity<V>` rendering.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::layout::{AvailableSpace, LayoutId};

use super::app::App;
use super::element::{
    AnyElement, ContentMask, DispatchPhase, Element, ElementId, GlobalElementId, Hitbox,
    HitboxBehavior, IntoElement, ParentElement, Render,
};
use super::entity::Entity;
use super::focus::FocusHandle;
use super::geometry::{Bounds, Pixels, Point, Size, point, px, size};
use super::graphics::d2d_renderer::PaintQuad;
use super::graphics::dwrite::WrappedLine;
use super::keymap::{Command, KeyContext};
use super::platform::{ExternalPaths, WindowControlArea};
use super::style::{Overflow, Style, StyleRefinement, TextOverflow, WhiteSpace};
use super::styled::Styled;
use super::types::SharedString;
use super::types::smallvec::SmallVec;
use super::window::{
    ClickEvent, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ScrollWheelEvent, Window,
};

type MouseDownHandler = Box<dyn FnMut(&MouseDownEvent, &mut Window, &mut App)>;
type MouseUpHandler = Box<dyn FnMut(&MouseUpEvent, &mut Window, &mut App)>;
type MouseMoveHandler = Box<dyn FnMut(&MouseMoveEvent, &mut Window, &mut App)>;
type ClickHandler = Box<dyn FnMut(&ClickEvent, &mut Window, &mut App)>;
type HoverHandler = Box<dyn FnMut(&bool, &mut Window, &mut App)>;
type ScrollWheelHandler = Box<dyn FnMut(&ScrollWheelEvent, &mut Window, &mut App)>;
type KeyDownHandler = Box<dyn FnMut(&KeyDownEvent, &mut Window, &mut App)>;
type DropHandler = Box<dyn FnMut(&ExternalPaths, &mut Window, &mut App)>;
type CommandHandler = Box<dyn FnMut(Command, &mut Window, &mut App) -> bool>;

#[derive(Default)]
pub(crate) struct Interactivity {
    pub(crate) element_id: Option<ElementId>,
    pub(crate) base_style: StyleRefinement,
    pub(crate) hover_style: Option<StyleRefinement>,
    pub(crate) active_style: Option<StyleRefinement>,
    pub(crate) group: Option<SharedString>,
    pub(crate) group_hover_style: Option<(SharedString, StyleRefinement)>,
    pub(crate) focus_handle: Option<FocusHandle>,
    pub(crate) key_context: Option<KeyContext>,
    pub(crate) window_control: Option<WindowControlArea>,
    pub(crate) occlude_mouse: bool,
    pub(crate) mouse_down_listeners: Vec<(Option<MouseButton>, MouseDownHandler)>,
    pub(crate) mouse_down_out_listeners: Vec<MouseDownHandler>,
    pub(crate) capture_mouse_down_listeners: Vec<MouseDownHandler>,
    pub(crate) mouse_up_listeners: Vec<(Option<MouseButton>, MouseUpHandler)>,
    pub(crate) mouse_up_out_listeners: Vec<(Option<MouseButton>, MouseUpHandler)>,
    pub(crate) capture_mouse_up_listeners: Vec<MouseUpHandler>,
    pub(crate) mouse_move_listeners: Vec<MouseMoveHandler>,
    pub(crate) click_listeners: Vec<ClickHandler>,
    pub(crate) aux_click_listeners: Vec<ClickHandler>,
    pub(crate) hover_listeners: Vec<HoverHandler>,
    pub(crate) scroll_wheel_listeners: Vec<ScrollWheelHandler>,
    pub(crate) key_down_listeners: Vec<KeyDownHandler>,
    pub(crate) command_listeners: Vec<CommandHandler>,
    pub(crate) drop_listeners: Vec<DropHandler>,
}

impl Interactivity {
    pub(crate) fn capture_any_mouse_down(
        &mut self,
        f: impl FnMut(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) {
        self.capture_mouse_down_listeners.push(Box::new(f));
    }

    pub(crate) fn capture_any_mouse_up(
        &mut self,
        f: impl FnMut(&MouseUpEvent, &mut Window, &mut App) + 'static,
    ) {
        self.capture_mouse_up_listeners.push(Box::new(f));
    }

    pub(crate) fn on_mouse_up_out(
        &mut self,
        button: MouseButton,
        f: impl FnMut(&MouseUpEvent, &mut Window, &mut App) + 'static,
    ) {
        self.mouse_up_out_listeners.push((Some(button), Box::new(f)));
    }
}

pub(crate) trait InteractiveElement: Sized {
    fn interactivity(&mut self) -> &mut Interactivity;

    fn id(mut self, id: impl Into<ElementId>) -> Stateful<Self> {
        self.interactivity().element_id = Some(id.into());
        Stateful(self)
    }

    fn group(mut self, name: impl Into<SharedString>) -> Self {
        self.interactivity().group = Some(name.into());
        self
    }

    fn group_hover(
        mut self,
        group: impl Into<SharedString>,
        f: impl FnOnce(StyleRefinement) -> StyleRefinement,
    ) -> Self {
        let style = f(StyleRefinement::default());
        self.interactivity().group_hover_style = Some((group.into(), style));
        self
    }

    fn hover(mut self, f: impl FnOnce(StyleRefinement) -> StyleRefinement) -> Self {
        let style = f(StyleRefinement::default());
        self.interactivity()
            .hover_style
            .get_or_insert_with(StyleRefinement::default)
            .refine(&style);
        self
    }

    fn track_focus(mut self, handle: &FocusHandle) -> Self {
        self.interactivity().focus_handle = Some(handle.clone());
        self
    }

    fn key_context(mut self, context: &str) -> Self {
        self.interactivity().key_context = Some(KeyContext::parse(context));
        self
    }

    fn window_control_area(mut self, area: WindowControlArea) -> Self {
        self.interactivity().window_control = Some(area);
        self
    }

    fn occlude(mut self) -> Self {
        self.interactivity().occlude_mouse = true;
        self
    }

    fn on_mouse_down(
        mut self,
        button: MouseButton,
        f: impl FnMut(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.interactivity()
            .mouse_down_listeners
            .push((Some(button), Box::new(f)));
        self
    }

    fn on_mouse_down_out(
        mut self,
        f: impl FnMut(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.interactivity()
            .mouse_down_out_listeners
            .push(Box::new(f));
        self
    }

    fn capture_any_mouse_down(
        mut self,
        f: impl FnMut(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.interactivity()
            .capture_mouse_down_listeners
            .push(Box::new(f));
        self
    }

    fn on_mouse_up(
        mut self,
        button: MouseButton,
        f: impl FnMut(&MouseUpEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.interactivity()
            .mouse_up_listeners
            .push((Some(button), Box::new(f)));
        self
    }

    fn on_mouse_up_out(
        mut self,
        button: MouseButton,
        f: impl FnMut(&MouseUpEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.interactivity()
            .mouse_up_out_listeners
            .push((Some(button), Box::new(f)));
        self
    }

    fn on_scroll_wheel(
        mut self,
        f: impl FnMut(&ScrollWheelEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.interactivity()
            .scroll_wheel_listeners
            .push(Box::new(f));
        self
    }

    fn on_command(
        mut self,
        f: impl FnMut(Command, &mut Window, &mut App) -> bool + 'static,
    ) -> Self {
        self.interactivity().command_listeners.push(Box::new(f));
        self
    }

    fn on_drop(
        mut self,
        f: impl FnMut(&ExternalPaths, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.interactivity().drop_listeners.push(Box::new(f));
        self
    }
}

pub(crate) trait StatefulInteractiveElement: InteractiveElement {
    fn active(mut self, f: impl FnOnce(StyleRefinement) -> StyleRefinement) -> Self {
        let style = f(StyleRefinement::default());
        self.interactivity()
            .active_style
            .get_or_insert_with(StyleRefinement::default)
            .refine(&style);
        self
    }

    fn on_click(
        mut self,
        f: impl FnMut(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.interactivity().click_listeners.push(Box::new(f));
        self
    }

    fn on_aux_click(
        mut self,
        f: impl FnMut(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.interactivity().aux_click_listeners.push(Box::new(f));
        self
    }

    fn on_hover(mut self, f: impl FnMut(&bool, &mut Window, &mut App) + 'static) -> Self {
        self.interactivity().hover_listeners.push(Box::new(f));
        self
    }
}

/// Core layout & visual container element (`div()`).
#[derive(Default)]
pub(crate) struct Div {
    interactivity: Interactivity,
    children: Vec<AnyElement>,
}

pub(crate) fn div() -> Div {
    Div::default()
}

impl Styled for Div {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.interactivity.base_style
    }
}

impl ParentElement for Div {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl InteractiveElement for Div {
    fn interactivity(&mut self) -> &mut Interactivity {
        &mut self.interactivity
    }
}

/// Wrapper marking an element as having an explicit `ElementId` (enabling `on_click`, `active`,
/// `on_hover`).
pub(crate) struct Stateful<E>(E);

impl<E: Styled> Styled for Stateful<E> {
    fn style(&mut self) -> &mut StyleRefinement {
        self.0.style()
    }
}

impl<E: ParentElement> ParentElement for Stateful<E> {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.0.extend(elements);
    }
}

impl<E: InteractiveElement> InteractiveElement for Stateful<E> {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.0.interactivity()
    }
}

impl<E: InteractiveElement> StatefulInteractiveElement for Stateful<E> {}

#[derive(Clone, Default)]
struct InteractiveState {
    clicked: Rc<Cell<bool>>,
    pending_click: Rc<Cell<bool>>,
    aux_clicked: Rc<Cell<Option<MouseButton>>>,
    hovered: Rc<Cell<bool>>,
}

impl Element for Div {
    type RequestLayoutState = Style;
    type PrepaintState = Option<Hitbox>;

    fn id(&self) -> Option<ElementId> {
        self.interactivity.element_id.clone()
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Style) {
        let mut style = Style::default();
        style.refine(&self.interactivity.base_style);

        let child_ids: Vec<LayoutId> = window.with_text_style(&style.text, |window| {
            self.children
                .iter_mut()
                .map(|child| child.request_layout(window, cx))
                .collect()
        });
        let lid = window.request_layout(style.clone(), child_ids, cx);
        (lid, style)
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        bounds: Bounds<Pixels>,
        style: &mut Style,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Hitbox> {
        let needs_hitbox = self.interactivity.occlude_mouse
            || self.interactivity.window_control.is_some()
            || self.interactivity.focus_handle.is_some()
            || self.interactivity.hover_style.is_some()
            || self.interactivity.active_style.is_some()
            || self.interactivity.group.is_some()
            || self.interactivity.group_hover_style.is_some()
            || self.interactivity.base_style.cursor.is_some()
            || !self.interactivity.mouse_down_listeners.is_empty()
            || !self.interactivity.mouse_up_listeners.is_empty()
            || !self.interactivity.mouse_move_listeners.is_empty()
            || !self.interactivity.capture_mouse_down_listeners.is_empty()
            || !self.interactivity.capture_mouse_up_listeners.is_empty()
            || !self.interactivity.click_listeners.is_empty()
            || !self.interactivity.aux_click_listeners.is_empty()
            || !self.interactivity.hover_listeners.is_empty()
            || !self.interactivity.scroll_wheel_listeners.is_empty()
            || !self.interactivity.drop_listeners.is_empty();

        let hitbox = if needs_hitbox {
            let behavior = if self.interactivity.occlude_mouse {
                HitboxBehavior::BlockMouse
            } else {
                HitboxBehavior::Normal
            };
            Some(window.insert_hitbox(bounds, behavior))
        } else {
            None
        };

        if let Some(area) = self.interactivity.window_control {
            window.insert_window_control(bounds, area);
        }

        if let (Some(group), Some(hb)) = (&self.interactivity.group, &hitbox) {
            window.register_group_hitbox(group.clone(), hb.id);
        }

        let clip = (style.overflow.x == Overflow::Hidden || style.overflow.y == Overflow::Hidden)
            .then_some(ContentMask { bounds });

        let focus_id = self.interactivity.focus_handle.as_ref().map(FocusHandle::id);
        let key_ctx = self.interactivity.key_context.clone();
        let text_ref = style.text.clone();

        window.with_focus_node(focus_id, key_ctx, |window| {
            window.with_text_style(&text_ref, |window| {
                window.with_content_mask(clip, |window| {
                    for child in &mut self.children {
                        child.prepaint(window, cx);
                    }
                });
            });
        });

        hitbox
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        bounds: Bounds<Pixels>,
        style: &mut Style,
        hitbox: &mut Option<Hitbox>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let state = id.map(|gid| {
            window.with_element_state(gid, |prev: Option<InteractiveState>, _| {
                let s = prev.unwrap_or_default();
                (s.clone(), s)
            })
        });

        let mut resolved = style.clone();
        resolved.refine(&self.interactivity.base_style);

        let hovered = hitbox.as_ref().is_some_and(|hb| hb.is_hovered(window));
        if hovered && let Some(hs) = &self.interactivity.hover_style {
            resolved.refine(hs);
        }
        if let Some((group, ghs)) = &self.interactivity.group_hover_style
            && window.is_group_hovered(group)
        {
            resolved.refine(ghs);
        }
        let is_active = state.as_ref().is_some_and(|s| s.clicked.get());
        if is_active && let Some(as_) = &self.interactivity.active_style {
            resolved.refine(as_);
        }

        if let (Some(cursor), Some(hb)) = (resolved.cursor, hitbox.as_ref()) {
            window.set_cursor_style(cursor, hb);
        }

        let focus_id = self.interactivity.focus_handle.as_ref().map(FocusHandle::id);

        // Register event listeners in pre-order (parent before children) so Capture phase runs
        // outermost-to-innermost and Bubble phase (reverse) runs innermost-to-outermost.
        let hb_clone = hitbox.clone();
        let owner_focus = focus_id.or_else(|| window.current_focus_id());

        for handler in self.interactivity.command_listeners.drain(..) {
            window.register_command_listener(owner_focus, handler);
        }
        for handler in self.interactivity.key_down_listeners.drain(..) {
            window.register_key_down_listener(owner_focus, handler);
        }
        for handler in self.interactivity.drop_listeners.drain(..) {
            let hb = hb_clone.clone();
            window.register_drop_listener(hb, handler);
        }

        // Hover / active state transitions & callbacks
        if let Some(hb) = hb_clone.clone() {
            let has_hover_fx = self.interactivity.hover_style.is_some()
                || self.interactivity.group.is_some()
                || !self.interactivity.hover_listeners.is_empty();
            if has_hover_fx {
                let mut listeners: Vec<HoverHandler> =
                    std::mem::take(&mut self.interactivity.hover_listeners);
                let prev_hovered = state.as_ref().map(|s| s.hovered.clone());
                let hb_move = hb.clone();
                window.on_mouse_event(move |_: &MouseMoveEvent, phase, window, cx| {
                    if phase != DispatchPhase::Bubble {
                        return;
                    }
                    let now = hb_move.is_hovered(window);
                    let changed = match &prev_hovered {
                        Some(cell) => cell.replace(now) != now,
                        None => now != hovered,
                    };
                    if changed {
                        for cb in &mut listeners {
                            cb(&now, window, cx);
                        }
                        window.refresh();
                    }
                });
            }

            // Capture mouse down/up listeners (Capture phase runs forward).
            for mut cb in self.interactivity.capture_mouse_down_listeners.drain(..) {
                let hb_c = hb.clone();
                window.on_mouse_event(move |ev: &MouseDownEvent, phase, window, cx| {
                    if phase == DispatchPhase::Capture && hb_c.is_hovered(window) {
                        cb(ev, window, cx);
                    }
                });
            }
            for mut cb in self.interactivity.capture_mouse_up_listeners.drain(..) {
                let hb_c = hb.clone();
                window.on_mouse_event(move |ev: &MouseUpEvent, phase, window, cx| {
                    if phase == DispatchPhase::Capture && hb_c.is_hovered(window) {
                        cb(ev, window, cx);
                    }
                });
            }

            // Bubble phase runs in reverse registration order, so within a single Div:
            //   1. Register `track_focus` auto-focus FIRST so Bubble runs it LAST (after user
            //      `on_mouse_down` handlers, which may call `window.prevent_default()` to suppress
            //      focus transfer, and preventing ancestor `track_focus` containers from stealing
            //      focus once this element claims it).
            //   2. Register user `on_mouse_down` BEFORE internal `click`/`aux_click` MouseDown
            //      trackers so Bubble runs the trackers first — an element's own
            //      `.on_mouse_down(|.., cx| cx.stop_propagation())` stops parent elements without
            //      suppressing its own `.on_click(...)`!
            //   3. Register internal `click`/`aux_click` MouseUp handlers BEFORE user `on_mouse_up`
            //      so Bubble runs `on_mouse_up` before `on_click`.
            if let Some(handle) = self.interactivity.focus_handle.clone() {
                let hb_f = hb.clone();
                window.on_mouse_event(move |_: &MouseDownEvent, phase, window, cx| {
                    if phase == DispatchPhase::Bubble
                        && hb_f.is_hovered(window)
                        && !window.default_prevented()
                    {
                        handle.focus(window, cx);
                        window.prevent_default();
                    }
                });
            }
            for (btn, mut cb) in self.interactivity.mouse_down_listeners.drain(..).rev() {
                let hb_l = hb.clone();
                window.on_mouse_event(move |ev: &MouseDownEvent, phase, window, cx| {
                    if phase == DispatchPhase::Bubble
                        && btn.is_none_or(|b| b == ev.button)
                        && hb_l.is_hovered(window)
                    {
                        cb(ev, window, cx);
                    }
                });
            }

            // Active visual state & primary click tracking
            if let Some(st) = state.clone()
                && (self.interactivity.active_style.is_some()
                    || !self.interactivity.click_listeners.is_empty())
            {
                let has_active = self.interactivity.active_style.is_some();
                let clicked_down = st.clicked.clone();
                let pending_down = st.pending_click.clone();
                let hb_down = hb.clone();
                window.on_mouse_event(move |ev: &MouseDownEvent, phase, window, _| {
                    if phase == DispatchPhase::Bubble
                        && ev.button == MouseButton::Left
                        && hb_down.is_hovered(window)
                    {
                        pending_down.set(true);
                        if has_active && !window.default_prevented() {
                            clicked_down.set(true);
                        }
                        window.refresh();
                    }
                });
                let clicked_up = st.clicked.clone();
                let pending_up = st.pending_click.clone();
                let hb_up = hb.clone();
                let click_cbs = std::rc::Rc::new(std::cell::RefCell::new(std::mem::take(
                    &mut self.interactivity.click_listeners,
                )));
                let mouse_click_cbs = click_cbs.clone();
                window.on_mouse_event(move |ev: &MouseUpEvent, phase, window, cx| {
                    if phase == DispatchPhase::Bubble && ev.button == MouseButton::Left {
                        let was_active = clicked_up.replace(false);
                        let was_pending = pending_up.replace(false);
                        if was_active || was_pending {
                            window.refresh();
                            if was_pending && hb_up.is_hovered(window) {
                                let click = ClickEvent {
                                    button: ev.button,
                                    position: ev.position,
                                    modifiers: ev.modifiers,
                                    click_count: ev.click_count,
                                };
                                for cb in mouse_click_cbs.borrow_mut().iter_mut() {
                                    cb(&click, window, cx);
                                }
                            }
                        }
                    }
                });
                // Keyboard activation (Enter / Space) when this clickable element has focus
                if let Some(fid) = focus_id
                    && window.focused_id() == Some(fid)
                    && !click_cbs.borrow().is_empty()
                {
                    window.register_key_down_listener(
                        Some(fid),
                        Box::new(move |ev: &KeyDownEvent, window, cx| {
                            if !ev.keystroke.modifiers.modified()
                                && matches!(ev.keystroke.key.as_str(), "enter" | "space")
                            {
                                let click = ClickEvent {
                                    button: MouseButton::Left,
                                    position: Point::default(),
                                    modifiers: ev.keystroke.modifiers,
                                    click_count: 1,
                                };
                                for cb in click_cbs.borrow_mut().iter_mut() {
                                    cb(&click, window, cx);
                                }
                                cx.stop_propagation();
                            }
                        }),
                    );
                }
            }

            // Auxiliary click tracking (middle/right click)
            if let Some(st) = state.clone()
                && !self.interactivity.aux_click_listeners.is_empty()
            {
                let aux_down = st.aux_clicked.clone();
                let hb_down = hb.clone();
                window.on_mouse_event(move |ev: &MouseDownEvent, phase, window, _| {
                    if phase == DispatchPhase::Bubble
                        && ev.button != MouseButton::Left
                        && hb_down.is_hovered(window)
                        && !window.default_prevented()
                    {
                        aux_down.set(Some(ev.button));
                    }
                });
                let aux_up = st.aux_clicked.clone();
                let hb_up = hb.clone();
                let mut aux_cbs: Vec<ClickHandler> =
                    std::mem::take(&mut self.interactivity.aux_click_listeners);
                window.on_mouse_event(move |ev: &MouseUpEvent, phase, window, cx| {
                    if phase == DispatchPhase::Bubble && ev.button != MouseButton::Left {
                        let was = aux_up.replace(None);
                        if was == Some(ev.button)
                            && hb_up.is_hovered(window)
                            && !window.default_prevented()
                        {
                            let click = ClickEvent {
                                button: ev.button,
                                position: ev.position,
                                modifiers: ev.modifiers,
                                click_count: ev.click_count,
                            };
                            for cb in &mut aux_cbs {
                                cb(&click, window, cx);
                            }
                        }
                    }
                });
            }

            // Bubble mouse up / move / scroll listeners
            for (btn, mut cb) in self.interactivity.mouse_up_listeners.drain(..).rev() {
                let hb_l = hb.clone();
                window.on_mouse_event(move |ev: &MouseUpEvent, phase, window, cx| {
                    if phase == DispatchPhase::Bubble
                        && btn.is_none_or(|b| b == ev.button)
                        && hb_l.is_hovered(window)
                    {
                        cb(ev, window, cx);
                    }
                });
            }
            for (btn, mut cb) in self.interactivity.mouse_up_out_listeners.drain(..).rev() {
                let hb_l = hb.clone();
                window.on_mouse_event(move |ev: &MouseUpEvent, phase, window, cx| {
                    if phase == DispatchPhase::Bubble
                        && btn.is_none_or(|b| b == ev.button)
                        && !hb_l.is_hovered(window)
                    {
                        cb(ev, window, cx);
                    }
                });
            }
            for mut cb in self.interactivity.mouse_move_listeners.drain(..).rev() {
                let hb_l = hb.clone();
                window.on_mouse_event(move |ev: &MouseMoveEvent, phase, window, cx| {
                    if phase == DispatchPhase::Bubble && hb_l.is_hovered(window) {
                        cb(ev, window, cx);
                    }
                });
            }
            for mut cb in self.interactivity.scroll_wheel_listeners.drain(..).rev() {
                let hb_l = hb.clone();
                window.on_scroll_wheel_event(move |ev: &ScrollWheelEvent, phase, window, cx| {
                    if phase == DispatchPhase::Bubble && hb_l.contains(&ev.position) {
                        cb(ev, window, cx);
                    }
                });
            }
        }

        // Mouse-down-out listeners fire even without hover on `bounds`.
        for mut cb in self.interactivity.mouse_down_out_listeners.drain(..) {
            window.on_mouse_event(move |ev: &MouseDownEvent, phase, window, cx| {
                if phase == DispatchPhase::Capture && !bounds.contains(&ev.position) {
                    cb(ev, window, cx);
                }
            });
        }

        // Paint visual box: drop shadows -> background -> children -> border (inside optional opacity layer).
        let opacity = resolved.opacity.unwrap_or(1.0);
        let paint_contents = |window: &mut Window,
                              children: &mut [AnyElement],
                              cx: &mut App| {
            if !resolved.box_shadow.is_empty() {
                window.paint_shadows(bounds, resolved.corner_radii, &resolved.box_shadow);
            }
            if let Some(bg) = resolved.background
                && !bg.is_transparent()
            {
                window.paint_quad(PaintQuad {
                    bounds,
                    corner_radii: resolved.corner_radii,
                    background: bg,
                    border_widths: super::geometry::Edges::default(),
                    border_color: super::color::Rgba::default(),
                });
            }
            let clip =
                (resolved.overflow.x == Overflow::Hidden || resolved.overflow.y == Overflow::Hidden)
                    .then_some(ContentMask { bounds });
            window.with_paint_focus(focus_id, |window| {
                window.with_text_style(&resolved.text, |window| {
                    window.with_content_mask(clip, |window| {
                        for child in children {
                            child.paint(window, cx);
                        }
                    });
                });
            });
            if let Some(border_color) = resolved.border_color
                && !border_color.is_transparent()
                && (resolved.border_widths.top > px(0.0)
                    || resolved.border_widths.right > px(0.0)
                    || resolved.border_widths.bottom > px(0.0)
                    || resolved.border_widths.left > px(0.0))
            {
                window.paint_quad(PaintQuad {
                    bounds,
                    corner_radii: resolved.corner_radii,
                    background: super::color::Rgba::default(),
                    border_widths: resolved.border_widths,
                    border_color,
                });
            }
        };

        if opacity < 1.0 {
            window.with_opacity(bounds, opacity, |window| {
                paint_contents(window, &mut self.children, cx);
            });
        } else {
            paint_contents(window, &mut self.children, cx);
        }
    }
}

impl IntoElement for Div {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl<E: Element> Element for Stateful<E> {
    type RequestLayoutState = E::RequestLayoutState;
    type PrepaintState = E::PrepaintState;

    fn id(&self) -> Option<ElementId> {
        self.0.id()
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        self.0.request_layout(id, window, cx)
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        bounds: Bounds<Pixels>,
        req: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.0.prepaint(id, bounds, req, window, cx)
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        bounds: Bounds<Pixels>,
        req: &mut Self::RequestLayoutState,
        pre: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.0.paint(id, bounds, req, pre, window, cx)
    }
}

impl<E: Element> IntoElement for Stateful<E> {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

// ---------------------------------------------------------------------------
// Leaf text element (`String`, `&'static str`, `SharedString`)
// ---------------------------------------------------------------------------

struct MeasuredText {
    wrap_width: Option<Pixels>,
    trim_width: Option<Pixels>,
    lines: SmallVec<[WrappedLine; 1]>,
    size: Size<Pixels>,
}

pub(crate) struct TextLeaf {
    text: SharedString,
    measured: Rc<RefCell<Option<MeasuredText>>>,
    lines: Vec<WrappedLine>,
    line_height: Pixels,
    align: super::graphics::dwrite::TextAlign,
}

impl Element for TextLeaf {
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
        let text_style = window.text_style();
        let line_height = text_style.line_height_in_pixels();
        self.line_height = line_height;
        self.align = text_style.text_align;
        let text = self.text.clone();
        let font = text_style.font();
        let font_size = text_style.font_size;
        let color = text_style.color;
        let nowrap = text_style.white_space == WhiteSpace::Nowrap;
        let truncate = text_style.text_overflow == Some(TextOverflow::Truncate);
        let line_clamp = text_style.line_clamp;
        let align = text_style.text_align;
        let measured = self.measured.clone();
        *measured.borrow_mut() = None;

        let lid = window.request_measured_layout(
            Style::default(),
            move |known, available, window, _cx| {
                let wrap_width = if nowrap {
                    None
                } else {
                    known.width.or(match available.width {
                        AvailableSpace::Definite(w) => Some(w),
                        AvailableSpace::MinContent | AvailableSpace::MaxContent => None,
                    })
                };
                let trim_width = if truncate || line_clamp.is_some() {
                    wrap_width
                } else {
                    None
                };
                if let Some(prev) = measured.borrow().as_ref()
                    && (wrap_width.is_none()
                        || (wrap_width == prev.wrap_width && trim_width == prev.trim_width)
                        || (prev.wrap_width.is_none()
                            && prev.trim_width.is_none()
                            && trim_width.is_none()
                            && wrap_width >= Some(prev.size.width)))
                {
                    let out_w = if truncate && matches!(available.width, AvailableSpace::MinContent)
                    {
                        Pixels::ZERO
                    } else {
                        prev.size.width
                    };
                    return size(out_w, prev.size.height);
                }
                let shaped = window.text_system().shape_text_trimmed(
                    text.clone(),
                    font_size,
                    &font,
                    color,
                    None,
                    wrap_width,
                    line_clamp,
                    trim_width,
                    align,
                );
                let mut total_w = Pixels::ZERO;
                let mut total_h = Pixels::ZERO;
                for l in &shaped {
                    let sz = l.size(line_height);
                    total_w = total_w.max(sz.width);
                    total_h += sz.height;
                }
                let cached_size = size(total_w.ceil(), total_h.max(line_height));
                *measured.borrow_mut() = Some(MeasuredText {
                    wrap_width,
                    trim_width,
                    lines: shaped,
                    size: cached_size,
                });
                let out_w = if truncate && matches!(available.width, AvailableSpace::MinContent) {
                    Pixels::ZERO
                } else {
                    cached_size.width
                };
                size(out_w, cached_size.height)
            },
            cx,
        );
        (lid, ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        _cx: &mut App,
    ) {
        let text_style = window.text_style();
        self.line_height = text_style.line_height_in_pixels();
        self.align = text_style.text_align;
        let needs_trim = text_style.text_overflow == Some(TextOverflow::Truncate)
            || text_style.line_clamp.is_some();
        let bound_w = bounds.size.width.max(px(1.0));
        if let Some(prev) = self.measured.borrow_mut().take() {
            if needs_trim && prev.trim_width.is_none() {
                self.lines = window
                    .text_system()
                    .shape_text_trimmed(
                        self.text.clone(),
                        text_style.font_size,
                        &text_style.font(),
                        text_style.color,
                        None,
                        prev.wrap_width,
                        text_style.line_clamp,
                        Some(bound_w),
                        text_style.text_align,
                    )
                    .into_vec();
            } else {
                self.lines = prev.lines.into_vec();
            }
            return;
        }
        let wrap_width = (text_style.white_space != WhiteSpace::Nowrap).then_some(bound_w);
        let trim_width = needs_trim.then_some(bound_w);
        self.lines = window
            .text_system()
            .shape_text_trimmed(
                self.text.clone(),
                text_style.font_size,
                &text_style.font(),
                text_style.color,
                None,
                wrap_width,
                text_style.line_clamp,
                trim_width,
                text_style.text_align,
            )
            .into_vec();
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let live_color = window.text_style().color;
        let mut y = bounds.origin.y;
        for line in &self.lines {
            let _ = line.paint_with_color(
                point(bounds.origin.x, y),
                self.line_height,
                self.align,
                Some(bounds.size.width),
                live_color,
                window,
                cx,
            );
            y += line.size(self.line_height).height;
        }
    }
}

impl IntoElement for TextLeaf {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl IntoElement for SharedString {
    type Element = TextLeaf;
    fn into_element(self) -> TextLeaf {
        TextLeaf {
            text: self,
            measured: Rc::new(RefCell::new(None)),
            lines: Vec::new(),
            line_height: px(18.0),
            align: super::graphics::dwrite::TextAlign::Left,
        }
    }
}

impl IntoElement for &'static str {
    type Element = TextLeaf;
    fn into_element(self) -> TextLeaf {
        SharedString::new_static(self).into_element()
    }
}

impl IntoElement for String {
    type Element = TextLeaf;
    fn into_element(self) -> TextLeaf {
        SharedString::from(self).into_element()
    }
}

// ---------------------------------------------------------------------------
// `Entity<V: Render>` as an `IntoElement`
// ---------------------------------------------------------------------------

pub(crate) struct EntityElement<V: Render> {
    entity: Entity<V>,
    rendered: Option<AnyElement>,
}

impl<V: Render> Element for EntityElement<V> {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        Some(ElementId::from(self.entity.entity_id()))
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut elem = window.with_current_view(self.entity.entity_id(), |window| {
            self.entity
                .update(cx, |view, cx| view.render(window, cx).into_any_element())
        });
        let lid = elem.request_layout(window, cx);
        self.rendered = Some(elem);
        (lid, ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(elem) = self.rendered.as_mut() {
            window.with_current_view(self.entity.entity_id(), |window| {
                elem.prepaint(window, cx);
            });
        }
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
        if let Some(elem) = self.rendered.as_mut() {
            window.with_current_view(self.entity.entity_id(), |window| {
                elem.paint(window, cx);
            });
        }
    }
}

impl<V: Render> IntoElement for EntityElement<V> {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl<V: Render> IntoElement for Entity<V> {
    type Element = EntityElement<V>;
    fn into_element(self) -> EntityElement<V> {
        EntityElement {
            entity: self,
            rendered: None,
        }
    }
}
