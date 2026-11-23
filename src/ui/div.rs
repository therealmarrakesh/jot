//! Single concrete fluent `Div` builder (combining layout, visual style, hover/active/pressed
//! transitions, tooltip registration, bounds observation, and input handlers) and `TextNode`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use crate::layout::{AvailableSpace, LayoutId};

use super::anim::{Easing, Lerp, animate};
use super::app::App;
use super::color::{Rgba, rgba, transparent_black};
use super::element::{
    ContentMask, DispatchPhase, ElementId, GlobalElementId, Hitbox, HitboxBehavior, UiNode,
};
use super::focus::FocusHandle;
use super::geometry::{Bounds, Pixels, Size, point, px, size};
use super::graphics::d2d_renderer::PaintQuad;
use super::graphics::dwrite::{TextAlign, WrappedLine};
use super::keymap::{Command, KeyContext};
use super::platform::{ExternalPaths, WindowControlArea};
use super::style::{Overflow, Style, StyleRefinement, TextOverflow, WhiteSpace};
use super::types::SharedString;
use super::window::{
    ClickEvent, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ScrollWheelEvent, Window,
};

const BRUSH_FADE_DURATION: Duration = Duration::from_millis(83);

type MouseDownHandler = Box<dyn FnMut(&MouseDownEvent, &mut Window, &mut App)>;
type MouseUpHandler = Box<dyn FnMut(&MouseUpEvent, &mut Window, &mut App)>;
type ClickHandler = Box<dyn FnMut(&ClickEvent, &mut Window, &mut App)>;
type HoverHandler = Box<dyn FnMut(&bool, &mut Window, &mut App)>;
type ScrollWheelHandler = Box<dyn FnMut(&ScrollWheelEvent, &mut Window, &mut App)>;
type CommandHandler = Box<dyn FnMut(Command, &mut Window, &mut App) -> bool>;
type DropHandler = Box<dyn FnMut(&ExternalPaths, &mut Window, &mut App)>;
type BoundsHandler = Box<dyn FnOnce(Bounds<Pixels>, &mut Window, &mut App)>;

/// Rest / hover / pressed fill colors animated with premultiplied RGBA interpolation.
#[derive(Clone, Copy)]
pub(crate) struct Brushes {
    pub(crate) rest: Rgba,
    pub(crate) hover: Rgba,
    pub(crate) pressed: Rgba,
}

impl Brushes {
    pub(crate) fn subtle(p: crate::theme::Palette) -> Self {
        Self {
            rest: transparent_black(),
            hover: rgba(p.subtle_secondary),
            pressed: rgba(p.subtle_tertiary),
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
struct Premultiplied(Rgba);

impl Lerp for Premultiplied {
    fn lerp(&self, target: &Self, t: f32) -> Self {
        let (from, to) = (self.0, target.0);
        let a = from.a + (to.a - from.a) * t;
        let channel = |x: f32, y: f32| {
            let premul = x * from.a + (y * to.a - x * from.a) * t;
            if a > 0.0 {
                (premul / a).clamp(0.0, 1.0)
            } else {
                0.0
            }
        };
        Premultiplied(Rgba {
            r: channel(from.r, to.r),
            g: channel(from.g, to.g),
            b: channel(from.b, to.b),
            a,
        })
    }
}

#[derive(Clone, Default)]
struct InteractiveState {
    clicked: Rc<Cell<bool>>,
    pending_click: Rc<Cell<bool>>,
    aux_clicked: Rc<Cell<Option<MouseButton>>>,
    hovered: Rc<Cell<bool>>,
    brush_pressed: Rc<Cell<bool>>,
}

/// Core layout, visual, and interactive container node (`div()`).
#[derive(Default)]
pub(crate) struct Div {
    element_id: Option<ElementId>,
    base_style: StyleRefinement,
    hover_style: Option<StyleRefinement>,
    active_style: Option<StyleRefinement>,
    group: Option<SharedString>,
    group_hover_style: Option<(SharedString, StyleRefinement)>,
    brushes: Option<Brushes>,
    tooltip: Option<SharedString>,
    disabled: bool,
    on_bounds: Option<BoundsHandler>,
    focus_handle: Option<FocusHandle>,
    key_context: Option<KeyContext>,
    window_control: Option<WindowControlArea>,
    occlude_mouse: bool,
    mouse_down_listeners: Vec<(Option<MouseButton>, MouseDownHandler)>,
    mouse_down_out_listeners: Vec<MouseDownHandler>,
    capture_mouse_down_listeners: Vec<MouseDownHandler>,
    mouse_up_listeners: Vec<(Option<MouseButton>, MouseUpHandler)>,
    mouse_up_out_listeners: Vec<(Option<MouseButton>, MouseUpHandler)>,
    click_listeners: Vec<ClickHandler>,
    aux_click_listeners: Vec<ClickHandler>,
    hover_listeners: Vec<HoverHandler>,
    scroll_wheel_listeners: Vec<ScrollWheelHandler>,
    command_listeners: Vec<CommandHandler>,
    drop_listeners: Vec<DropHandler>,
    children: Vec<UiNode>,
    // Per-frame cached layout/prepaint state
    layout_id: Option<LayoutId>,
    bounds: Bounds<Pixels>,
    computed_style: Style,
    hitbox: Option<Hitbox>,
}

pub(crate) fn div() -> Div {
    Div::default()
}

crate::__impl_visual_style_builders!(Div, |self| &mut self.base_style);
crate::__impl_layout_style_builders!(Div, |self| &mut self.base_style);

impl Div {
    // ---- Conditional / fluent composition helpers ----

    #[inline]
    pub(crate) fn map<U>(self, f: impl FnOnce(Self) -> U) -> U {
        f(self)
    }

    #[inline]
    pub(crate) fn when(self, condition: bool, then: impl FnOnce(Self) -> Self) -> Self {
        if condition { then(self) } else { self }
    }

    #[inline]
    pub(crate) fn when_some<T>(self, option: Option<T>, then: impl FnOnce(Self, T) -> Self) -> Self {
        if let Some(value) = option {
            then(self, value)
        } else {
            self
        }
    }

    // ---- Children ----

    pub(crate) fn child(mut self, child: impl Into<UiNode>) -> Self {
        self.children.push(child.into());
        self
    }

    pub(crate) fn children(mut self, children: impl IntoIterator<Item = impl Into<UiNode>>) -> Self {
        self.children.extend(children.into_iter().map(Into::into));
        self
    }

    // ---- Identity, hover/active/brush transitions, tooltips, bounds ----

    pub(crate) fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.element_id = Some(id.into());
        self
    }

    pub(crate) fn group(mut self, name: impl Into<SharedString>) -> Self {
        self.group = Some(name.into());
        self
    }

    pub(crate) fn group_hover(
        mut self,
        group: impl Into<SharedString>,
        f: impl FnOnce(StyleRefinement) -> StyleRefinement,
    ) -> Self {
        let style = f(StyleRefinement::default());
        self.group_hover_style = Some((group.into(), style));
        self
    }

    pub(crate) fn hover(mut self, f: impl FnOnce(StyleRefinement) -> StyleRefinement) -> Self {
        let style = f(StyleRefinement::default());
        self.hover_style
            .get_or_insert_with(StyleRefinement::default)
            .refine(&style);
        self
    }

    pub(crate) fn active(mut self, f: impl FnOnce(StyleRefinement) -> StyleRefinement) -> Self {
        let style = f(StyleRefinement::default());
        self.active_style
            .get_or_insert_with(StyleRefinement::default)
            .refine(&style);
        self
    }

    /// Applies an active style strictly on primary (left) button presses.
    pub(crate) fn pressed(self, f: impl FnOnce(StyleRefinement) -> StyleRefinement) -> Self {
        self.capture_any_mouse_down(|event, window, _| {
            if event.button != MouseButton::Left {
                window.prevent_default();
            }
        })
        .active(f)
    }

    /// Smoothly animates background fills between `rest`, `hover`, and `pressed` states.
    pub(crate) fn brush_transition(mut self, brushes: Brushes) -> Self {
        self.brushes = Some(brushes);
        self
    }

    /// Registers a native tooltip anchored to this `Div`'s laid-out bounds (requires `.id(...)`).
    pub(crate) fn tooltip(mut self, text: impl Into<SharedString>) -> Self {
        self.tooltip = Some(text.into());
        self
    }

    /// Suppresses brush transitions and tooltip registration while disabled.
    pub(crate) fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Observes this `Div`'s laid-out bounds during `prepaint`.
    pub(crate) fn on_bounds(
        mut self,
        f: impl FnOnce(Bounds<Pixels>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_bounds = Some(Box::new(f));
        self
    }

    // ---- Focus, key contexts, window chrome, occlusion ----

    pub(crate) fn track_focus(mut self, handle: &FocusHandle) -> Self {
        self.focus_handle = Some(handle.clone());
        self
    }

    pub(crate) fn key_context(mut self, context: &str) -> Self {
        self.key_context = Some(KeyContext::parse(context));
        self
    }

    pub(crate) fn window_control_area(mut self, area: WindowControlArea) -> Self {
        self.window_control = Some(area);
        self
    }

    pub(crate) fn occlude(mut self) -> Self {
        self.occlude_mouse = true;
        self
    }

    // ---- Event handlers ----

    pub(crate) fn on_mouse_down(
        mut self,
        button: MouseButton,
        f: impl FnMut(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.mouse_down_listeners.push((Some(button), Box::new(f)));
        self
    }

    pub(crate) fn on_mouse_down_out(
        mut self,
        f: impl FnMut(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.mouse_down_out_listeners.push(Box::new(f));
        self
    }

    pub(crate) fn capture_any_mouse_down(
        mut self,
        f: impl FnMut(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.capture_mouse_down_listeners.push(Box::new(f));
        self
    }

    pub(crate) fn on_mouse_up(
        mut self,
        button: MouseButton,
        f: impl FnMut(&MouseUpEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.mouse_up_listeners.push((Some(button), Box::new(f)));
        self
    }

    pub(crate) fn on_mouse_up_out(
        mut self,
        button: MouseButton,
        f: impl FnMut(&MouseUpEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.mouse_up_out_listeners
            .push((Some(button), Box::new(f)));
        self
    }

    pub(crate) fn on_click(
        mut self,
        f: impl FnMut(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.click_listeners.push(Box::new(f));
        self
    }

    pub(crate) fn on_aux_click(
        mut self,
        f: impl FnMut(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.aux_click_listeners.push(Box::new(f));
        self
    }

    pub(crate) fn on_hover(
        mut self,
        f: impl FnMut(&bool, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.hover_listeners.push(Box::new(f));
        self
    }

    pub(crate) fn on_scroll_wheel(
        mut self,
        f: impl FnMut(&ScrollWheelEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.scroll_wheel_listeners.push(Box::new(f));
        self
    }

    pub(crate) fn on_command(
        mut self,
        f: impl FnMut(Command, &mut Window, &mut App) -> bool + 'static,
    ) -> Self {
        self.command_listeners.push(Box::new(f));
        self
    }

    pub(crate) fn on_drop(
        mut self,
        f: impl FnMut(&ExternalPaths, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.drop_listeners.push(Box::new(f));
        self
    }

    // ---- Internal 3-phase lifecycle called by `UiNode` ----

    #[inline]
    pub(super) fn element_id(&self) -> Option<ElementId> {
        self.element_id.clone()
    }

    #[inline]
    pub(super) fn layout_id(&self) -> Option<LayoutId> {
        self.layout_id
    }

    #[inline]
    pub(super) fn set_layout_id(&mut self, id: LayoutId) {
        self.layout_id = Some(id);
    }

    #[inline]
    pub(super) fn bounds(&self) -> Bounds<Pixels> {
        self.bounds
    }

    #[inline]
    pub(super) fn set_bounds(&mut self, bounds: Bounds<Pixels>) {
        self.bounds = bounds;
    }

    pub(super) fn request_layout(&mut self, window: &mut Window, cx: &mut App) -> LayoutId {
        let mut style = Style::default();
        style.refine(&self.base_style);

        let child_ids: Vec<LayoutId> = window.with_text_style(&style.text, |window| {
            self.children
                .iter_mut()
                .map(|child| child.request_layout(window, cx))
                .collect()
        });
        let lid = window.request_layout(style.clone(), child_ids, cx);
        self.computed_style = style;
        lid
    }

    pub(super) fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(on_bounds) = self.on_bounds.take() {
            on_bounds(bounds, window, cx);
        }

        if !self.disabled
            && let (Some(tip_id), Some(text)) = (self.element_id.clone(), self.tooltip.take())
        {
            crate::tooltip::register(tip_id, text, bounds, window, cx);
        }

        let needs_hitbox = self.occlude_mouse
            || self.window_control.is_some()
            || self.focus_handle.is_some()
            || self.hover_style.is_some()
            || self.active_style.is_some()
            || self.group.is_some()
            || self.group_hover_style.is_some()
            || (self.brushes.is_some() && !self.disabled)
            || self.base_style.cursor.is_some()
            || !self.mouse_down_listeners.is_empty()
            || !self.mouse_up_listeners.is_empty()
            || !self.capture_mouse_down_listeners.is_empty()
            || !self.click_listeners.is_empty()
            || !self.aux_click_listeners.is_empty()
            || !self.hover_listeners.is_empty()
            || !self.scroll_wheel_listeners.is_empty()
            || !self.drop_listeners.is_empty();

        let hitbox = if needs_hitbox {
            let behavior = if self.occlude_mouse {
                HitboxBehavior::BlockMouse
            } else {
                HitboxBehavior::Normal
            };
            Some(window.insert_hitbox(bounds, behavior))
        } else {
            None
        };

        if let Some(area) = self.window_control {
            window.insert_window_control(bounds, area);
        }

        if let (Some(group), Some(hb)) = (&self.group, &hitbox) {
            window.register_group_hitbox(group.clone(), hb.id);
        }

        let clip = (self.computed_style.overflow.x == Overflow::Hidden
            || self.computed_style.overflow.y == Overflow::Hidden)
            .then_some(ContentMask { bounds });

        let focus_id = self.focus_handle.as_ref().map(FocusHandle::id);
        let key_ctx = self.key_context.clone();
        let text_ref = self.computed_style.text.clone();

        window.with_focus_node(focus_id, key_ctx, |window| {
            window.with_text_style(&text_ref, |window| {
                window.with_content_mask(clip, |window| {
                    for child in &mut self.children {
                        child.prepaint(window, cx);
                    }
                });
            });
        });

        self.hitbox = hitbox;
    }

    pub(super) fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let state = id.map(|gid| {
            window.with_element_state(gid, |prev: Option<InteractiveState>, _| {
                let s = prev.unwrap_or_default();
                (s.clone(), s)
            })
        });

        let mut resolved = self.computed_style.clone();
        resolved.refine(&self.base_style);

        let hovered = self.hitbox.as_ref().is_some_and(|hb| hb.is_hovered(window));
        if hovered && let Some(hs) = &self.hover_style {
            resolved.refine(hs);
        }
        if let Some((group, ghs)) = &self.group_hover_style
            && window.is_group_hovered(group)
        {
            resolved.refine(ghs);
        }
        let is_active = state.as_ref().is_some_and(|s| s.clicked.get());
        if is_active && let Some(as_) = &self.active_style {
            resolved.refine(as_);
        }

        if let Some(brushes) = self.brushes {
            let brush_hovered = !self.disabled && hovered;
            let brush_down = !self.disabled && state.as_ref().is_some_and(|s| s.brush_pressed.get());
            let target = if brush_down {
                brushes.pressed
            } else if brush_hovered {
                brushes.hover
            } else {
                brushes.rest
            };
            let Premultiplied(color) = animate(
                "brush",
                Premultiplied(target),
                BRUSH_FADE_DURATION,
                Easing::Linear,
                window,
                cx,
            );
            resolved.background = Some(color);

            if !self.disabled
                && let Some(hb) = self.hitbox.clone()
            {
                if let Some(st) = &state {
                    let pressed_down = st.brush_pressed.clone();
                    let hb_down = hb.clone();
                    window.on_mouse_event(move |ev: &MouseDownEvent, phase, window, _| {
                        if phase == DispatchPhase::Capture
                            && ev.button == MouseButton::Left
                            && hb_down.is_hovered(window)
                            && !pressed_down.replace(true)
                        {
                            window.refresh();
                        }
                    });
                    let pressed_up = st.brush_pressed.clone();
                    let hb_up = hb.clone();
                    window.on_mouse_event(move |ev: &MouseUpEvent, phase, window, _| {
                        if phase == DispatchPhase::Capture
                            && ev.button == MouseButton::Left
                            && hb_up.is_hovered(window)
                            && pressed_up.replace(false)
                        {
                            window.refresh();
                        }
                    });
                    let pressed_out = st.brush_pressed.clone();
                    let hb_out = hb.clone();
                    window.on_mouse_event(move |ev: &MouseUpEvent, phase, window, _| {
                        if phase == DispatchPhase::Bubble
                            && ev.button == MouseButton::Left
                            && !hb_out.is_hovered(window)
                            && pressed_out.replace(false)
                        {
                            window.refresh();
                        }
                    });
                }
                window.on_mouse_event(move |_: &MouseMoveEvent, phase, window, _| {
                    if phase == DispatchPhase::Capture && hb.is_hovered(window) != brush_hovered {
                        window.refresh();
                    }
                });
            }
        }

        if let (Some(cursor), Some(hb)) = (resolved.cursor, self.hitbox.as_ref()) {
            window.set_cursor_style(cursor, hb);
        }

        let focus_id = self.focus_handle.as_ref().map(FocusHandle::id);

        // Register event listeners in pre-order (parent before children) so Capture phase runs
        // outermost-to-innermost and Bubble phase (reverse) runs innermost-to-outermost.
        let hb_clone = self.hitbox.clone();
        let owner_focus = focus_id.or_else(|| window.current_focus_id());

        for handler in self.command_listeners.drain(..) {
            window.register_command_listener(owner_focus, handler);
        }
        for handler in self.drop_listeners.drain(..) {
            window.register_drop_listener(hb_clone.clone(), handler);
        }

        if let Some(hb) = hb_clone {
            let has_hover_fx = self.hover_style.is_some()
                || self.group.is_some()
                || !self.hover_listeners.is_empty();
            if has_hover_fx {
                let mut listeners: Vec<HoverHandler> = std::mem::take(&mut self.hover_listeners);
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

            for mut cb in self.capture_mouse_down_listeners.drain(..) {
                let hb_c = hb.clone();
                window.on_mouse_event(move |ev: &MouseDownEvent, phase, window, cx| {
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
            if let Some(handle) = self.focus_handle.clone() {
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
            for (btn, mut cb) in self.mouse_down_listeners.drain(..).rev() {
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

            if let Some(st) = state.clone()
                && (self.active_style.is_some() || !self.click_listeners.is_empty())
            {
                let has_active = self.active_style.is_some();
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
                let click_cbs = Rc::new(RefCell::new(std::mem::take(&mut self.click_listeners)));
                let mouse_click_cbs = click_cbs.clone();
                window.on_mouse_event(move |ev: &MouseUpEvent, phase, window, cx| {
                    if phase == DispatchPhase::Bubble && ev.button == MouseButton::Left {
                        let was_active = clicked_up.replace(false);
                        let was_pending = pending_up.replace(false);
                        if was_active || was_pending {
                            window.refresh();
                            if was_pending && hb_up.is_hovered(window) {
                                let click = ClickEvent { button: ev.button };
                                for cb in mouse_click_cbs.borrow_mut().iter_mut() {
                                    cb(&click, window, cx);
                                }
                            }
                        }
                    }
                });
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

            if let Some(st) = state
                && !self.aux_click_listeners.is_empty()
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
                let mut aux_cbs: Vec<ClickHandler> = std::mem::take(&mut self.aux_click_listeners);
                window.on_mouse_event(move |ev: &MouseUpEvent, phase, window, cx| {
                    if phase == DispatchPhase::Bubble && ev.button != MouseButton::Left {
                        let was = aux_up.replace(None);
                        if was == Some(ev.button)
                            && hb_up.is_hovered(window)
                            && !window.default_prevented()
                        {
                            let click = ClickEvent { button: ev.button };
                            for cb in &mut aux_cbs {
                                cb(&click, window, cx);
                            }
                        }
                    }
                });
            }

            for (btn, mut cb) in self.mouse_up_listeners.drain(..).rev() {
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
            for (btn, mut cb) in self.mouse_up_out_listeners.drain(..).rev() {
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
            for mut cb in self.scroll_wheel_listeners.drain(..).rev() {
                let hb_l = hb.clone();
                window.on_scroll_wheel_event(move |ev: &ScrollWheelEvent, phase, window, cx| {
                    if phase == DispatchPhase::Bubble && hb_l.contains(&ev.position) {
                        cb(ev, window, cx);
                    }
                });
            }
        }

        for mut cb in self.mouse_down_out_listeners.drain(..) {
            window.on_mouse_event(move |ev: &MouseDownEvent, phase, window, cx| {
                if phase == DispatchPhase::Capture && !bounds.contains(&ev.position) {
                    cb(ev, window, cx);
                }
            });
        }

        // Paint visual box: drop shadows -> background -> children -> border.
        let opacity = resolved.opacity.unwrap_or(1.0);
        let paint_contents = |window: &mut Window, children: &mut [UiNode], cx: &mut App| {
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
                    border_color: Rgba::default(),
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
                    background: Rgba::default(),
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

// ---------------------------------------------------------------------------
// Leaf text node (`String`, `&'static str`, `SharedString`)
// ---------------------------------------------------------------------------

struct MeasuredText {
    wrap_width: Option<Pixels>,
    trim_width: Option<Pixels>,
    lines: Vec<WrappedLine>,
    size: Size<Pixels>,
}

pub(crate) struct TextNode {
    text: SharedString,
    measured: Rc<RefCell<Option<MeasuredText>>>,
    lines: Vec<WrappedLine>,
    line_height: Pixels,
    align: TextAlign,
    layout_id: Option<LayoutId>,
    bounds: Bounds<Pixels>,
}

impl TextNode {
    pub(crate) fn new(text: SharedString) -> Self {
        Self {
            text,
            measured: Rc::new(RefCell::new(None)),
            lines: Vec::new(),
            line_height: px(18.0),
            align: TextAlign::Left,
            layout_id: None,
            bounds: Bounds::default(),
        }
    }

    #[inline]
    pub(super) fn layout_id(&self) -> Option<LayoutId> {
        self.layout_id
    }

    #[inline]
    pub(super) fn set_layout_id(&mut self, id: LayoutId) {
        self.layout_id = Some(id);
    }

    #[inline]
    pub(super) fn bounds(&self) -> Bounds<Pixels> {
        self.bounds
    }

    #[inline]
    pub(super) fn set_bounds(&mut self, bounds: Bounds<Pixels>) {
        self.bounds = bounds;
    }

    pub(super) fn request_layout(&mut self, window: &mut Window, cx: &mut App) -> LayoutId {
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

        window.request_measured_layout(
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
        )
    }

    pub(super) fn prepaint(&mut self, bounds: Bounds<Pixels>, window: &mut Window, _cx: &mut App) {
        let text_style = window.text_style();
        self.line_height = text_style.line_height_in_pixels();
        self.align = text_style.text_align;
        let needs_trim = text_style.text_overflow == Some(TextOverflow::Truncate)
            || text_style.line_clamp.is_some();
        let bound_w = bounds.size.width.max(px(1.0));
        if let Some(prev) = self.measured.borrow_mut().take() {
            if needs_trim && prev.trim_width.is_none() {
                self.lines = window.text_system().shape_text_trimmed(
                    self.text.clone(),
                    text_style.font_size,
                    &text_style.font(),
                    text_style.color,
                    None,
                    prev.wrap_width,
                    text_style.line_clamp,
                    Some(bound_w),
                    text_style.text_align,
                );
            } else {
                self.lines = prev.lines;
            }
            return;
        }
        let wrap_width = (text_style.white_space != WhiteSpace::Nowrap).then_some(bound_w);
        let trim_width = needs_trim.then_some(bound_w);
        self.lines = window.text_system().shape_text_trimmed(
            self.text.clone(),
            text_style.font_size,
            &text_style.font(),
            text_style.color,
            None,
            wrap_width,
            text_style.line_clamp,
            trim_width,
            text_style.text_align,
        );
    }

    pub(super) fn paint(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
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
