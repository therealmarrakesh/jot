mod arena_scope;
mod dispatch;
mod element_id;
mod focus;
mod frame;
mod handle;
mod hitbox;
mod paint;

pub(crate) use arena_scope::*;
pub(crate) use dispatch::*;
pub(crate) use element_id::*;
pub(crate) use focus::*;
pub(crate) use frame::*;
pub(crate) use handle::*;
pub(crate) use hitbox::*;

use std::{
    any::TypeId,
    cell::{Cell, RefCell},
    mem,
    ops::DerefMut,
    rc::Rc,
    sync::Arc,
    time::Instant,
};

use crate::ui::{
    AnyView, App, AppContext, AsyncWindowContext, Bounds, Capslock, Context, DispatchTree,
    DisplayId, Entity, EntityId, GlobalElementId, IntoElement, LayoutEngine, Modifiers, Pixels,
    Point, Refineable, Size, SubscriberSet, Subscription, Task, TextStyle, TextStyleRefinement,
    WindowAppearance, WindowBounds, WindowOptions, WindowParams, WindowTextSystem,
    anyhow::Result,
    gpui_util::{ResultExt, measure},
    point, px, size,
};
use crate::win32::{DirectXAtlas, WindowsWindow};

/// Default window size used when no explicit size is provided.
pub(crate) const DEFAULT_WINDOW_SIZE: Size<Pixels> = size(px(1536.), px(1095.));

type AnyObserver = Box<dyn FnMut(&mut Window, &mut App) -> bool + 'static>;
type FrameCallback = Box<dyn FnOnce(&mut Window, &mut App)>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub(super) enum InputModality {
    Mouse,
    Keyboard,
}

#[derive(Clone, Debug, Default)]
pub(super) struct ModifierState {
    modifiers: Modifiers,
    saw_other_input: bool,
}

/// Holds the state for a specific window.
pub(crate) struct Window {
    pub(crate) handle: AnyWindowHandle,
    pub(crate) invalidator: WindowInvalidator,
    pub(crate) removed: bool,
    pub(crate) platform_window: WindowsWindow,
    pub(super) sprite_atlas: Arc<DirectXAtlas>,
    text_system: WindowTextSystem,
    pub(crate) viewport_size: Size<Pixels>,
    pub(super) layout_engine: Option<LayoutEngine>,
    pub(crate) root: Option<AnyView>,
    pub(crate) element_id_stack: ElementIdStack,
    pub(crate) text_style_stack: Vec<TextStyleRefinement>,
    pub(crate) rendered_entity_stack: Vec<EntityId>,
    pub(crate) element_offset_stack: Vec<Point<Pixels>>,
    pub(crate) element_opacity: f32,
    pub(crate) content_mask_stack: Vec<ContentMask<Pixels>>,
    pub(crate) rendered_frame: Frame,
    pub(crate) next_frame: Frame,
    pub(super) next_hitbox_id: HitboxId,
    pub(crate) next_frame_callbacks: Rc<RefCell<Vec<FrameCallback>>>,
    focus_listeners: SubscriberSet<(), AnyWindowFocusListener>,
    pub(super) default_prevented: bool,
    pub(super) mouse_position: Point<Pixels>,
    pub(super) mouse_hit_test: HitTest,
    pub(super) modifiers: Modifiers,
    pub(super) capslock: Capslock,
    scale_factor: f32,
    pub(crate) bounds_observers: SubscriberSet<(), AnyObserver>,
    appearance: WindowAppearance,
    pub(crate) appearance_observers: SubscriberSet<(), AnyObserver>,
    active: Rc<Cell<bool>>,
    hovered: Rc<Cell<bool>>,
    pub(crate) needs_present: Rc<Cell<bool>>,
    pub(crate) input_rate_tracker: Rc<RefCell<InputRateTracker>>,
    pub(super) last_input_modality: InputModality,
    pub(crate) refreshing: bool,
    pub(crate) activation_observers: SubscriberSet<(), AnyObserver>,
    pub(crate) focus: Option<FocusId>,
    pub(crate) focus_generation: u64,
    pub(super) pending_modifier: ModifierState,
}

fn default_bounds(display_id: Option<DisplayId>, cx: &mut App) -> WindowBounds {
    let active_window_bounds = cx
        .active_window()
        .and_then(|w| w.update(cx, |_, window, _| window.window_bounds()).ok());

    const CASCADE_OFFSET: f32 = 25.0;

    let display = display_id
        .map(|id| cx.find_display(id))
        .unwrap_or_else(|| cx.primary_display());

    let default_placement = || Bounds::new(point(px(0.), px(0.)), DEFAULT_WINDOW_SIZE);

    let display_bounds = display
        .as_ref()
        .map(|d| d.visible_bounds())
        .unwrap_or_else(default_placement);

    let (
        Bounds {
            origin: base_origin,
            size: base_size,
        },
        window_bounds_ctor,
    ): (_, fn(Bounds<Pixels>) -> WindowBounds) = match active_window_bounds {
        Some(bounds) => match bounds {
            WindowBounds::Windowed(bounds) => (bounds, WindowBounds::Windowed),
            WindowBounds::Maximized(bounds) => (bounds, WindowBounds::Maximized),
        },
        None => (
            display
                .as_ref()
                .map(|d| d.default_bounds())
                .unwrap_or_else(default_placement),
            WindowBounds::Windowed,
        ),
    };

    let cascade_offset = point(px(CASCADE_OFFSET), px(CASCADE_OFFSET));
    let proposed_origin = base_origin + cascade_offset;
    let proposed_bounds = Bounds::new(proposed_origin, base_size);

    let display_right = display_bounds.origin.x + display_bounds.size.width;
    let display_bottom = display_bounds.origin.y + display_bounds.size.height;
    let window_right = proposed_bounds.origin.x + proposed_bounds.size.width;
    let window_bottom = proposed_bounds.origin.y + proposed_bounds.size.height;

    let fits_horizontally = window_right <= display_right;
    let fits_vertically = window_bottom <= display_bottom;

    let final_origin = match (fits_horizontally, fits_vertically) {
        (true, true) => proposed_origin,
        (false, true) => point(display_bounds.origin.x, base_origin.y),
        (true, false) => point(base_origin.x, display_bounds.origin.y),
        (false, false) => display_bounds.origin,
    };
    window_bounds_ctor(Bounds::new(final_origin, base_size))
}

impl Window {
    pub(crate) fn new(
        handle: AnyWindowHandle,
        options: WindowOptions,
        cx: &mut App,
    ) -> Result<Self> {
        let WindowOptions {
            window_bounds,
            titlebar,
            focus,
            show,
            kind,
            is_movable,
            inactive_frame_interval,
            is_resizable,
            is_minimizable,
            display_id,
            window_background,
            window_min_size,
        } = options;

        let window_bounds = window_bounds.unwrap_or_else(|| default_bounds(display_id, cx));
        let platform_window = cx.platform.open_window(
            handle,
            WindowParams {
                bounds: window_bounds.get_bounds(),
                titlebar,
                kind,
                is_movable,
                is_resizable,
                is_minimizable,
                focus,
                show,
                display_id,
                window_min_size,
            },
        )?;

        let sprite_atlas = platform_window.sprite_atlas();
        let mouse_position = platform_window.mouse_position();
        let modifiers = platform_window.modifiers();
        let capslock = platform_window.capslock();
        let content_size = platform_window.content_size();
        let scale_factor = platform_window.scale_factor();
        let appearance = platform_window.appearance();
        let text_system = WindowTextSystem::new(cx.text_system().clone());
        let invalidator = WindowInvalidator::new(handle.window_id());
        let active = Rc::new(Cell::new(platform_window.is_active()));
        let hovered = Rc::new(Cell::new(platform_window.is_hovered()));
        let needs_present = Rc::new(Cell::new(false));
        let next_frame_callbacks: Rc<RefCell<Vec<FrameCallback>>> = Default::default();
        let input_rate_tracker = Rc::new(RefCell::new(InputRateTracker::default()));
        let last_frame_time = Rc::new(Cell::new(None));

        platform_window.set_background_appearance(window_background);

        if matches!(window_bounds, WindowBounds::Maximized(_)) {
            platform_window.zoom();
        }

        platform_window.on_close(Box::new({
            let mut cx = cx.to_async();
            move || {
                let _ = handle.update(&mut cx, |_, window, _| window.remove_window());
            }
        }));
        platform_window.on_request_frame(Box::new({
            let mut cx = cx.to_async();
            let invalidator = invalidator.clone();
            let active = active.clone();
            let needs_present = needs_present.clone();
            let next_frame_callbacks = next_frame_callbacks.clone();
            let input_rate_tracker = input_rate_tracker.clone();
            let mut deferred_force_render = false;
            move |request_frame_options| {
                if draw_in_progress() {
                    deferred_force_render |= request_frame_options.force_render;
                    return;
                }
                let force_render =
                    mem::take(&mut deferred_force_render) || request_frame_options.force_render;

                let min_frame_interval = if request_frame_options.require_presentation
                    || (!request_frame_options.force_render
                        && next_frame_callbacks.borrow().is_empty())
                {
                    None
                } else if !active.get() && !input_rate_tracker.borrow_mut().is_high_rate() {
                    inactive_frame_interval
                } else {
                    None
                };

                let now = Instant::now();
                if let Some(min_interval) = min_frame_interval
                    && let Some(last_frame) = last_frame_time.get()
                    && now.duration_since(last_frame) < min_interval
                {
                    deferred_force_render |= force_render;
                    return;
                }
                last_frame_time.set(Some(now));

                let pending_next_frame_callbacks = next_frame_callbacks.take();
                if !pending_next_frame_callbacks.is_empty() {
                    handle
                        .update(&mut cx, |_, window, cx| {
                            for callback in pending_next_frame_callbacks {
                                callback(window, cx);
                            }
                        })
                        .log_err();
                }

                let needs_present = request_frame_options.require_presentation
                    || needs_present.get()
                    || input_rate_tracker.borrow_mut().is_high_rate();

                let should_draw = invalidator.is_dirty() || force_render;
                if should_draw {
                    measure("frame duration", || {
                        handle
                            .update(&mut cx, |_, window, cx| {
                                if force_render {
                                    window.refresh();
                                }
                                let arena_clear_needed = window.draw(cx);
                                window.present();
                                arena_clear_needed.clear(cx);
                            })
                            .log_err();
                    })
                } else if needs_present {
                    handle
                        .update(&mut cx, |_, window, _| window.present())
                        .log_err();
                }
            }
        }));
        platform_window.on_resize(Box::new({
            let mut cx = cx.to_async();
            move |_, _| {
                handle
                    .update(&mut cx, |_, window, cx| window.bounds_changed(cx))
                    .log_err();
            }
        }));
        platform_window.on_moved(Box::new({
            let mut cx = cx.to_async();
            move || {
                handle
                    .update(&mut cx, |_, window, cx| window.bounds_changed(cx))
                    .log_err();
            }
        }));
        platform_window.on_appearance_changed(Box::new({
            let cx = cx.to_async();
            let foreground_executor = cx.foreground_executor().clone();
            move || {
                let mut cx = cx.clone();
                foreground_executor
                    .spawn(async move {
                        handle
                            .update(&mut cx, |_, window, cx| window.appearance_changed(cx))
                            .log_err();
                    })
                    .detach();
            }
        }));
        platform_window.on_active_status_change(Box::new({
            let mut cx = cx.to_async();
            move |active| {
                handle
                    .update(&mut cx, |_, window, cx| {
                        window.active.set(active);
                        window.modifiers = window.platform_window.modifiers();
                        window.capslock = window.platform_window.capslock();
                        window
                            .activation_observers
                            .clone()
                            .retain(&(), |callback| callback(window, cx));

                        window.bounds_changed(cx);
                        window.refresh();
                    })
                    .log_err();
            }
        }));
        platform_window.on_hover_status_change(Box::new({
            let mut cx = cx.to_async();
            move |active| {
                handle
                    .update(&mut cx, |_, window, _| {
                        window.hovered.set(active);
                        window.refresh();
                    })
                    .log_err();
            }
        }));
        platform_window.on_input({
            let mut cx = cx.to_async();
            Box::new(move |event| {
                handle
                    .update(&mut cx, |_, window, cx| window.dispatch_event(event, cx))
                    .log_err()
                    .unwrap_or(DispatchEventResult::default())
            })
        });
        platform_window.on_hit_test_window_control({
            let mut cx = cx.to_async();
            Box::new(move || {
                handle
                    .update(&mut cx, |_, window, _cx| {
                        for (area, hitbox) in &window.rendered_frame.window_control_hitboxes {
                            if window.mouse_hit_test.ids.contains(&hitbox.id) {
                                return Some(*area);
                            }
                        }
                        None
                    })
                    .log_err()
                    .unwrap_or(None)
            })
        });

        Ok(Window {
            handle,
            invalidator,
            removed: false,
            platform_window,
            sprite_atlas,
            text_system,
            viewport_size: content_size,
            layout_engine: Some(LayoutEngine::new()),
            root: None,
            element_id_stack: ElementIdStack::default(),
            text_style_stack: Vec::new(),
            rendered_entity_stack: Vec::new(),
            element_offset_stack: Vec::new(),
            content_mask_stack: Vec::new(),
            element_opacity: 1.0,
            rendered_frame: Frame::new(DispatchTree::new(cx.keymap.clone())),
            next_frame: Frame::new(DispatchTree::new(cx.keymap.clone())),
            next_frame_callbacks,
            next_hitbox_id: HitboxId(0),
            focus_listeners: SubscriberSet::new(),
            default_prevented: true,
            mouse_position,
            mouse_hit_test: HitTest::default(),
            modifiers,
            capslock,
            scale_factor,
            bounds_observers: SubscriberSet::new(),
            appearance,
            appearance_observers: SubscriberSet::new(),
            active,
            hovered,
            needs_present,
            input_rate_tracker,
            last_input_modality: InputModality::Mouse,
            refreshing: false,
            activation_observers: SubscriberSet::new(),
            focus: None,
            focus_generation: 0,
            pending_modifier: ModifierState::default(),
        })
    }

    pub(crate) fn new_focus_listener(
        &self,
        value: AnyWindowFocusListener,
    ) -> (Subscription, impl FnOnce() + use<>) {
        self.focus_listeners.insert((), value)
    }

    pub(crate) fn window_handle(&self) -> AnyWindowHandle {
        self.handle
    }

    pub(crate) fn raw_hwnd(&self) -> Option<isize> {
        self.platform_window.raw_hwnd()
    }

    pub(crate) fn refresh(&mut self) {
        if self.invalidator.not_drawing() {
            self.refreshing = true;
            self.invalidator.set_dirty(true);
        }
    }

    pub(crate) fn remove_window(&mut self) {
        self.removed = true;
    }

    pub(crate) fn focused(&self, cx: &App) -> Option<FocusHandle> {
        self.focus
            .and_then(|id| FocusHandle::for_id(id, &cx.focus_handles))
    }

    pub(crate) fn focus(&mut self, handle: &FocusHandle, _cx: &mut App) {
        if self.focus == Some(handle.id) {
            return;
        }

        self.focus = Some(handle.id);
        self.focus_generation = self.focus_generation.wrapping_add(1);
        self.refresh();
    }

    pub(crate) fn blur(&mut self, _cx: &mut App) {
        if self.focus.is_some() {
            self.focus_generation = self.focus_generation.wrapping_add(1);
        }
        self.focus = None;
        self.refresh();
    }

    pub(crate) fn text_system(&self) -> &WindowTextSystem {
        &self.text_system
    }

    pub(crate) fn text_style(&self) -> TextStyle {
        let mut style = TextStyle::default();
        for refinement in &self.text_style_stack {
            style.refine(refinement);
        }
        style
    }

    pub(crate) fn is_maximized(&self) -> bool {
        self.platform_window.is_maximized()
    }

    pub(crate) fn window_bounds(&self) -> WindowBounds {
        self.platform_window.window_bounds()
    }

    pub(crate) fn defer(&self, cx: &mut App, f: impl FnOnce(&mut Window, &mut App) + 'static) {
        let handle = self.handle;
        cx.defer(move |cx| {
            handle.update(cx, |_, window, cx| f(window, cx)).ok();
        });
    }

    pub(crate) fn to_async(&self, cx: &App) -> AsyncWindowContext {
        AsyncWindowContext::new_context(cx.to_async(), self.handle)
    }

    pub(crate) fn on_next_frame(&self, callback: impl FnOnce(&mut Window, &mut App) + 'static) {
        RefCell::borrow_mut(&self.next_frame_callbacks).push(Box::new(callback));
    }

    pub(crate) fn request_animation_frame(&self) {
        let entity = self.current_view();
        self.on_next_frame(move |_, cx| cx.notify(entity));
    }

    #[track_caller]
    pub(crate) fn spawn<AsyncFn, R>(&self, cx: &App, f: AsyncFn) -> Task<R>
    where
        R: 'static,
        AsyncFn: AsyncFnOnce(&mut AsyncWindowContext) -> R + 'static,
    {
        let handle = self.handle;
        cx.spawn(async move |app| {
            let mut async_window_cx = AsyncWindowContext::new_context(app.clone(), handle);
            f(&mut async_window_cx).await
        })
    }

    pub(crate) fn bounds_changed(&mut self, cx: &mut App) {
        self.scale_factor = self.platform_window.scale_factor();
        self.viewport_size = self.platform_window.content_size();
        self.mouse_position = self.platform_window.mouse_position();

        self.refresh();

        self.bounds_observers
            .clone()
            .retain(&(), |callback| callback(self, cx));
    }

    pub(crate) fn bounds(&self) -> Bounds<Pixels> {
        self.platform_window.bounds()
    }

    pub(crate) fn appearance_changed(&mut self, cx: &mut App) {
        self.appearance = self.platform_window.appearance();

        self.appearance_observers
            .clone()
            .retain(&(), |callback| callback(self, cx));
    }

    pub(crate) fn appearance(&self) -> WindowAppearance {
        self.appearance
    }

    pub(crate) fn viewport_size(&self) -> Size<Pixels> {
        self.viewport_size
    }

    pub(crate) fn is_window_active(&self) -> bool {
        self.active.get()
    }

    pub(crate) fn is_window_hovered(&self) -> bool {
        self.hovered.get()
    }

    pub(crate) fn set_window_title(&mut self, title: &str) {
        self.platform_window.set_title(title);
    }

    pub(crate) fn scale_factor(&self) -> f32 {
        self.scale_factor
    }

    pub(crate) fn with_global_id<R>(
        &mut self,
        element_id: ElementId,
        f: impl FnOnce(&GlobalElementId, &mut Self) -> R,
    ) -> R {
        self.with_id(element_id, |this| {
            let global_id = this.element_id_stack.global_id();
            f(&global_id, this)
        })
    }

    #[inline]
    pub(crate) fn with_id<R>(
        &mut self,
        element_id: impl Into<ElementId>,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        self.element_id_stack.push(element_id.into());
        let result = f(self);
        self.element_id_stack.pop();
        result
    }

    pub(crate) fn line_height(&self) -> Pixels {
        self.text_style().line_height_in_pixels()
    }

    pub(crate) fn prevent_default(&mut self) {
        self.default_prevented = true;
    }

    pub(crate) fn default_prevented(&self) -> bool {
        self.default_prevented
    }

    pub(crate) fn mouse_position(&self) -> Point<Pixels> {
        self.mouse_position
    }

    pub(crate) fn last_input_was_keyboard(&self) -> bool {
        self.last_input_modality == InputModality::Keyboard
    }

    pub(crate) fn current_view(&self) -> EntityId {
        self.invalidator.debug_assert_paint_or_prepaint();
        self.rendered_entity_stack.last().copied().unwrap()
    }

    #[inline]
    pub(crate) fn with_rendered_view<R>(
        &mut self,
        id: EntityId,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        self.rendered_entity_stack.push(id);
        let result = f(self);
        self.rendered_entity_stack.pop();
        result
    }

    pub(crate) fn draw(&mut self, cx: &mut App) -> ArenaClearNeeded {
        let arena_scope = ElementArenaScope::enter(&cx.element_arena);

        cx.entities.clear_accessed();
        debug_assert!(self.rendered_entity_stack.is_empty());
        self.invalidator.set_dirty(false);

        if let Some(input_handler) = self.platform_window.take_input_handler() {
            if let Some(slot) = self
                .rendered_frame
                .input_handlers
                .iter_mut()
                .rev()
                .find(|h| h.is_none())
            {
                *slot = Some(input_handler);
            } else {
                self.rendered_frame.input_handlers.push(Some(input_handler));
            }
        }
        self.draw_roots(cx);
        self.next_frame.window_active = self.active.get();

        if let Some(input_handler) = self
            .next_frame
            .input_handlers
            .iter_mut()
            .rev()
            .find_map(|h| h.take())
        {
            self.platform_window.set_input_handler(input_handler);
        }

        self.layout_engine.as_mut().unwrap().clear();
        self.text_system().finish_frame();
        self.next_frame.finish(&mut self.rendered_frame);

        self.invalidator.set_phase(DrawPhase::Focus);
        let previous_focus_path = self.rendered_frame.focus_path();
        let previous_window_active = self.rendered_frame.window_active;
        mem::swap(&mut self.rendered_frame, &mut self.next_frame);
        self.next_frame.clear();
        let current_focus_path = self.rendered_frame.focus_path();
        let current_window_active = self.rendered_frame.window_active;
        let focus_before_listeners = self.focus;

        if previous_focus_path != current_focus_path
            || previous_window_active != current_window_active
        {
            let event = WindowFocusEvent {
                previous_focus_path: if previous_window_active {
                    previous_focus_path
                } else {
                    Default::default()
                },
                current_focus_path: if current_window_active {
                    current_focus_path
                } else {
                    Default::default()
                },
            };
            self.focus_listeners
                .clone()
                .retain(&(), |listener| listener(&event, self, cx));
        }

        debug_assert!(self.rendered_entity_stack.is_empty());
        self.record_entities_accessed(cx);
        self.reset_cursor_style(cx);
        self.refreshing = false;
        self.invalidator.set_phase(DrawPhase::None);
        if self.focus != focus_before_listeners {
            self.refresh();
        }
        self.needs_present.set(true);

        arena_scope.exit(&cx.element_arena)
    }

    fn record_entities_accessed(&mut self, cx: &mut App) {
        let mut entities_ref = cx.entities.accessed_entities.get_mut();
        let mut entities = mem::take(entities_ref.deref_mut());
        let handle = self.handle;
        cx.record_entities_accessed(handle, self.invalidator.clone(), &entities);
        let mut entities_ref = cx.entities.accessed_entities.get_mut();
        mem::swap(&mut entities, entities_ref.deref_mut());
    }

    fn present(&mut self) {
        self.platform_window.draw(&self.rendered_frame.scene);
        self.needs_present.set(false);
    }

    fn draw_roots(&mut self, cx: &mut App) {
        self.invalidator.set_phase(DrawPhase::Prepaint);

        let root_size = self.viewport_size;
        let scale_factor = self.scale_factor();
        let mut root_element = self.root.as_ref().unwrap().clone().into_any_element();
        let root_layout_id = root_element.request_layout(self, cx);
        self.layout_engine
            .as_mut()
            .unwrap()
            .stretch_auto_size_to_fill(root_layout_id, root_size, scale_factor);
        root_element.prepaint_as_root(Point::default(), root_size.into(), self, cx);

        self.prepaint_deferred_draws(cx);

        self.mouse_hit_test = self.next_frame.hit_test(self.mouse_position);

        self.invalidator.set_phase(DrawPhase::Paint);
        root_element.paint(self, cx);

        self.paint_deferred_draws(cx);
    }

    pub(crate) fn use_keyed_state<S: 'static>(
        &mut self,
        key: impl Into<ElementId>,
        cx: &mut App,
        init: impl FnOnce(&mut Self, &mut Context<S>) -> S,
    ) -> Entity<S> {
        let current_view = self.current_view();
        self.with_global_id(key.into(), |global_id, window| {
            window.with_element_state(global_id, |state: Option<Entity<S>>, window| {
                if let Some(state) = state {
                    (state.clone(), state)
                } else {
                    let new_state = cx.new(|cx| init(window, cx));
                    Self::observe_keyed_state(&new_state, current_view, cx);
                    (new_state.clone(), new_state)
                }
            })
        })
    }

    #[track_caller]
    pub(crate) fn use_state<S: 'static>(
        &mut self,
        cx: &mut App,
        init: impl FnOnce(&mut Self, &mut Context<S>) -> S,
    ) -> Entity<S> {
        self.use_keyed_state(
            ElementId::CodeLocation(*core::panic::Location::caller()),
            cx,
            init,
        )
    }

    #[inline(always)]
    pub(crate) fn with_element_state<S, R>(
        &mut self,
        global_id: &GlobalElementId,
        f: impl FnOnce(Option<S>, &mut Self) -> (R, S),
    ) -> R
    where
        S: 'static,
    {
        self.invalidator.debug_assert_paint_or_prepaint();

        let (key, state) = self.take_element_state(global_id, TypeId::of::<S>());

        if let Some(any) = state {
            let ElementStateBox {
                inner,
                #[cfg(debug_assertions)]
                type_name,
            } = any;
            let mut state_box = inner
                .downcast::<Option<S>>()
                .map_err(|_| {
                    #[cfg(debug_assertions)]
                    {
                        crate::ui::anyhow::anyhow!(
                            "invalid element state type for id, requested {:?}, actual: {:?}",
                            std::any::type_name::<S>(),
                            type_name
                        )
                    }

                    #[cfg(not(debug_assertions))]
                    {
                        crate::ui::anyhow::anyhow!(
                            "invalid element state type for id, requested {:?}",
                            std::any::type_name::<S>(),
                        )
                    }
                })
                .unwrap();

            let state = state_box.take().expect(
                "reentrant call to with_element_state for the same state type and element id",
            );
            let (result, state) = f(Some(state), self);
            state_box.replace(state);
            self.insert_element_state(
                key,
                ElementStateBox {
                    inner: state_box,
                    #[cfg(debug_assertions)]
                    type_name,
                },
            );
            result
        } else {
            let (result, state) = f(None, self);
            self.insert_element_state(
                key,
                ElementStateBox {
                    inner: Box::new(Some(state)),
                    #[cfg(debug_assertions)]
                    type_name: std::any::type_name::<S>(),
                },
            );
            result
        }
    }

    pub(crate) fn with_optional_element_state<S, R>(
        &mut self,
        global_id: Option<&GlobalElementId>,
        f: impl FnOnce(Option<Option<S>>, &mut Self) -> (R, Option<S>),
    ) -> R
    where
        S: 'static,
    {
        self.invalidator.debug_assert_paint_or_prepaint();

        if let Some(global_id) = global_id {
            self.with_element_state(global_id, |state, cx| {
                let (result, state) = f(Some(state), cx);
                let state =
                    state.expect("you must return some state when you pass some element id");
                (result, state)
            })
        } else {
            let (result, state) = f(None, self);
            debug_assert!(
                state.is_none(),
                "you must not return an element state when passing None for the global id"
            );
            result
        }
    }

    #[inline(never)]
    fn take_element_state(
        &mut self,
        global_id: &GlobalElementId,
        state_type: TypeId,
    ) -> ((GlobalElementId, TypeId), Option<ElementStateBox>) {
        let key = (global_id.clone(), state_type);
        self.next_frame.accessed_element_states.push(key.clone());
        let state = self
            .next_frame
            .element_states
            .remove(&key)
            .or_else(|| self.rendered_frame.element_states.remove(&key));
        (key, state)
    }

    #[inline(never)]
    fn insert_element_state(
        &mut self,
        key: (GlobalElementId, TypeId),
        state: ElementStateBox,
    ) -> Option<ElementStateBox> {
        self.next_frame.element_states.insert(key, state)
    }

    #[inline(never)]
    fn observe_keyed_state<S: 'static>(state: &Entity<S>, current_view: EntityId, cx: &mut App) {
        cx.observe(state, move |_, cx| {
            cx.notify(current_view);
        })
        .detach();
    }
}
