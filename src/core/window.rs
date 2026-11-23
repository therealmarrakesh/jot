//! Clean-room `Window` state, 3-pass layout/prepaint/paint orchestration, hitboxes, focus tree,
//! element state cache, and event/action dispatch.

use std::any::{Any, TypeId};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use windows::Win32::Graphics::DirectWrite::IDWriteTextLayout;

use crate::layout::{AvailableSpace, LayoutEngine, LayoutId};
use crate::win32::WindowHwnd;

use super::app::{
    AnyWindowHandle, App, AsyncWindowContext, Context, Subscription,
};
use super::color::{Rgba, transparent_black};
use super::element::{
    AnyElement, ContentMask, DispatchPhase, ElementId, ElementInputHandler, EntityInputHandler,
    ErasedInputHandler, GlobalElementId, Hitbox, HitboxBehavior, HitboxId,
};
use super::entity::{AnyView, Entity, EntityId};
use super::focus::{FocusHandle, FocusId};
use super::geometry::{Bounds, Corners, Pixels, Point, Size, point, px};
use super::graphics::d2d_renderer::{BoxShadow, D2DRenderer, PaintQuad};
use super::graphics::dwrite::TextSystem;
use super::keymap::{Action, KeyBinding, KeyContext, Keystroke, Modifiers};
use super::platform::window::NativeWindow;
use super::platform::{
    CursorStyle, ExternalPaths, WindowAppearance, WindowBounds, WindowControlArea, WindowOptions,
};
use super::runtime::Task;
use super::style::{Style, TextStyle, TextStyleRefinement};
use super::types::SharedString;
use super::types::anyhow::Result;

static NEXT_WINDOW_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct WindowId(pub(crate) u64);

impl WindowId {
    pub(crate) fn next() -> Self {
        Self(NEXT_WINDOW_ID.fetch_add(1, Ordering::Relaxed))
    }
}

impl From<u64> for WindowId {
    #[inline]
    fn from(id: u64) -> Self {
        Self(id)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum MouseButton {
    Left,
    Right,
    Middle,
}

#[derive(Clone, Debug)]
pub(crate) struct MouseDownEvent {
    pub(crate) button: MouseButton,
    pub(crate) position: Point<Pixels>,
    pub(crate) modifiers: Modifiers,
    pub(crate) click_count: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct MouseUpEvent {
    pub(crate) button: MouseButton,
    pub(crate) position: Point<Pixels>,
    pub(crate) modifiers: Modifiers,
    pub(crate) click_count: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct MouseMoveEvent {
    pub(crate) position: Point<Pixels>,
    pub(crate) pressed_button: Option<MouseButton>,
    #[allow(dead_code)]
    pub(crate) modifiers: Modifiers,
}

impl MouseMoveEvent {
    #[inline]
    pub(crate) fn dragging(&self) -> bool {
        self.pressed_button.is_some()
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ClickEvent {
    pub(crate) button: MouseButton,
    #[allow(dead_code)]
    pub(crate) position: Point<Pixels>,
    #[allow(dead_code)]
    pub(crate) modifiers: Modifiers,
    #[allow(dead_code)]
    pub(crate) click_count: usize,
}

impl ClickEvent {
    #[inline]
    pub(crate) fn is_middle_click(&self) -> bool {
        self.button == MouseButton::Middle
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) enum ScrollDelta {
    Lines(Point<f32>),
    #[allow(dead_code)]
    Pixels(Point<Pixels>),
}

impl ScrollDelta {
    pub(crate) fn pixel_delta(self, line_height: Pixels) -> Point<Pixels> {
        match self {
            Self::Lines(l) => point(line_height * l.x, line_height * l.y),
            Self::Pixels(p) => p,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ScrollWheelEvent {
    pub(crate) position: Point<Pixels>,
    pub(crate) delta: ScrollDelta,
    pub(crate) modifiers: Modifiers,
}

#[derive(Clone, Debug)]
pub(crate) struct KeyDownEvent {
    pub(crate) keystroke: Keystroke,
    #[allow(dead_code)]
    pub(crate) is_held: bool,
}

#[derive(Copy, Clone, Debug)]
pub(crate) struct ModifiersChangedEvent {
    pub(crate) modifiers: Modifiers,
}

pub(crate) trait MouseEvent: 'static {}
impl MouseEvent for MouseDownEvent {}
impl MouseEvent for MouseUpEvent {}
impl MouseEvent for MouseMoveEvent {}
impl MouseEvent for ScrollWheelEvent {}

type AnyMouseListener = Box<dyn FnMut(&dyn Any, DispatchPhase, &mut Window, &mut App)>;
type AnyScrollListener = Box<dyn FnMut(&ScrollWheelEvent, DispatchPhase, &mut Window, &mut App)>;
type AnyKeyListener = Box<dyn FnMut(&KeyDownEvent, DispatchPhase, &mut Window, &mut App)>;
type AnyModListener = Box<dyn FnMut(&ModifiersChangedEvent, &mut Window, &mut App)>;
type ActionHandlerBox = Box<dyn FnMut(&dyn Action, &mut Window, &mut App)>;
type KeyDownHandlerBox = Box<dyn FnMut(&KeyDownEvent, &mut Window, &mut App)>;
type DropHandlerBox = Box<dyn FnMut(&ExternalPaths, &mut Window, &mut App)>;
type ShouldCloseCallback = Box<dyn FnMut(&mut Window, &mut App) -> bool>;
type FocusedActionListener = (Option<FocusId>, TypeId, ActionHandlerBox);
type FocusedKeyListener = (Option<FocusId>, KeyDownHandlerBox);
type WindowCallback = (Rc<Cell<bool>>, Box<dyn FnMut(&mut Window, &mut App) -> bool>);
type FocusCallback = (FocusId, Rc<Cell<bool>>, Box<dyn FnMut(&mut Window, &mut App) -> bool>);
type DropListener = (Option<Hitbox>, DropHandlerBox);

struct DeferredDraw {
    priority: usize,
    order: usize,
    id_stack: Vec<ElementId>,
    text_style: TextStyle,
    element_offset: Point<Pixels>,
    key_context_stack: Vec<KeyContext>,
    focus_stack: Vec<FocusId>,
    element: AnyElement,
}

struct FocusNode {
    parent: Option<FocusId>,
    key_contexts: Vec<KeyContext>,
}

struct FrameState {
    hitboxes: Vec<Hitbox>,
    next_hitbox_id: u64,
    hovered_hitboxes: HashSet<HitboxId>,
    group_hitboxes: HashMap<SharedString, Vec<HitboxId>>,
    window_controls: Vec<(Bounds<Pixels>, WindowControlArea)>,
    cursor_regions: Vec<(Hitbox, CursorStyle)>,
    mouse_listeners: Vec<(TypeId, AnyMouseListener)>,
    scroll_listeners: Vec<AnyScrollListener>,
    key_listeners: Vec<AnyKeyListener>,
    mod_listeners: Vec<AnyModListener>,
    action_listeners: Vec<FocusedActionListener>,
    key_down_listeners: Vec<FocusedKeyListener>,
    drop_listeners: Vec<DropListener>,
    focus_nodes: HashMap<FocusId, FocusNode>,
    root_key_contexts: Vec<KeyContext>,
    input_handler: Option<(FocusId, Box<dyn ErasedInputHandler>)>,
    #[cfg(test)]
    element_bounds: HashMap<ElementId, Bounds<Pixels>>,
}

impl Default for FrameState {
    fn default() -> Self {
        Self {
            hitboxes: Vec::new(),
            next_hitbox_id: 1,
            hovered_hitboxes: HashSet::new(),
            group_hitboxes: HashMap::new(),
            window_controls: Vec::new(),
            cursor_regions: Vec::new(),
            mouse_listeners: Vec::new(),
            scroll_listeners: Vec::new(),
            key_listeners: Vec::new(),
            mod_listeners: Vec::new(),
            action_listeners: Vec::new(),
            key_down_listeners: Vec::new(),
            drop_listeners: Vec::new(),
            focus_nodes: HashMap::new(),
            root_key_contexts: Vec::new(),
            input_handler: None,
            #[cfg(test)]
            element_bounds: HashMap::new(),
        }
    }
}

pub(crate) struct Window {
    id: WindowId,
    pub(crate) native: NativeWindow,
    renderer: D2DRenderer,
    root_view: Option<AnyView>,
    root_type: TypeId,
    text_system: Rc<TextSystem>,
    keymap: Rc<RefCell<super::keymap::Keymap>>,
    appearance: WindowAppearance,
    layout_engine: LayoutEngine,
    frame: FrameState,
    prev_frame: FrameState,
    element_states: HashMap<(GlobalElementId, TypeId), Box<dyn Any>>,
    next_element_states: HashMap<(GlobalElementId, TypeId), Box<dyn Any>>,
    id_stack: Vec<ElementId>,
    text_style_stack: Vec<TextStyle>,
    content_mask_stack: Vec<Bounds<Pixels>>,
    element_offset_stack: Vec<Point<Pixels>>,
    focus_stack: Vec<FocusId>,
    key_context_stack: Vec<KeyContext>,
    view_stack: Vec<EntityId>,
    deferred_draws: Vec<DeferredDraw>,
    focused_handle: Option<FocusHandle>,
    in_paint: bool,
    mouse_position: Point<Pixels>,
    last_input_was_keyboard: bool,
    default_prevented: bool,
    dirty: bool,
    animation_requested: bool,
    removed: bool,
    show_on_open: bool,
    focus_on_open: bool,
    should_close: Option<ShouldCloseCallback>,
    activation_observers: Vec<WindowCallback>,
    appearance_observers: Vec<WindowCallback>,
    bounds_observers: Vec<WindowCallback>,
    focus_in_observers: Vec<FocusCallback>,
    focus_out_observers: Vec<FocusCallback>,
}

impl Window {
    pub(crate) fn new(id: WindowId, options: WindowOptions, cx: &App) -> Result<Self> {
        let appearance = cx.window_appearance();
        let native = NativeWindow::create(id, &options, appearance)?;
        let (pw, ph) = native.physical_size();
        let renderer = D2DRenderer::new(
            cx.platform.devices.clone(),
            native.hwnd,
            pw,
            ph,
            native.scale_factor,
        )?;
        Ok(Self {
            id,
            native,
            renderer,
            root_view: None,
            root_type: TypeId::of::<()>(),
            text_system: cx.text_system.clone(),
            keymap: cx.keymap.clone(),
            appearance,
            layout_engine: LayoutEngine::new(),
            frame: FrameState::default(),
            prev_frame: FrameState::default(),
            element_states: HashMap::new(),
            next_element_states: HashMap::new(),
            id_stack: Vec::new(),
            text_style_stack: vec![TextStyle::default()],
            content_mask_stack: Vec::new(),
            element_offset_stack: vec![Point::default()],
            focus_stack: Vec::new(),
            key_context_stack: Vec::new(),
            view_stack: Vec::new(),
            deferred_draws: Vec::new(),
            focused_handle: None,
            in_paint: false,
            mouse_position: Point::default(),
            last_input_was_keyboard: false,
            default_prevented: false,
            dirty: true,
            animation_requested: false,
            removed: false,
            show_on_open: options.show,
            focus_on_open: options.focus,
            should_close: None,
            activation_observers: Vec::new(),
            appearance_observers: Vec::new(),
            bounds_observers: Vec::new(),
            focus_in_observers: Vec::new(),
            focus_out_observers: Vec::new(),
        })
    }

    pub(crate) fn set_root_view(&mut self, view: AnyView) {
        self.root_view = Some(view);
    }

    pub(crate) fn finish_open(&mut self) {
        self.native.show(self.show_on_open, self.focus_on_open);
    }

    pub(crate) fn root_view(&self) -> Option<&AnyView> {
        self.root_view.as_ref()
    }

    pub(crate) fn window_id(&self) -> WindowId {
        self.id
    }

    pub(crate) fn window_handle(&self) -> AnyWindowHandle {
        AnyWindowHandle {
            id: self.id,
            root_type: self.root_type,
        }
    }

    pub(crate) fn set_root_type<V: 'static>(&mut self) {
        self.root_type = TypeId::of::<V>();
    }

    pub(crate) fn hwnd(&self) -> WindowHwnd {
        self.native.window_hwnd()
    }

    pub(crate) fn scale_factor(&self) -> f32 {
        self.native.scale_factor
    }

    pub(crate) fn viewport_size(&self) -> Size<Pixels> {
        self.native.logical_size
    }

    pub(crate) fn bounds(&self) -> Bounds<Pixels> {
        self.native.bounds()
    }

    pub(crate) fn window_bounds(&self) -> WindowBounds {
        if self.native.is_maximized() {
            WindowBounds::Maximized(self.native.bounds())
        } else {
            WindowBounds::Windowed(self.native.bounds())
        }
    }

    pub(crate) fn is_maximized(&self) -> bool {
        self.native.is_maximized()
    }

    pub(crate) fn is_window_active(&self) -> bool {
        self.native.active
    }

    pub(crate) fn is_window_hovered(&self) -> bool {
        self.native.hovered
    }

    pub(crate) fn appearance(&self) -> WindowAppearance {
        self.appearance
    }

    pub(crate) fn mouse_position(&self) -> Point<Pixels> {
        self.mouse_position
    }

    pub(crate) fn last_input_was_keyboard(&self) -> bool {
        self.last_input_was_keyboard
    }

    pub(crate) fn set_window_title(&self, title: &str) {
        self.native.set_title(title);
    }

    pub(crate) fn text_system(&self) -> &Rc<TextSystem> {
        &self.text_system
    }

    pub(crate) fn text_style(&self) -> TextStyle {
        self.text_style_stack.last().cloned().unwrap_or_default()
    }

    pub(crate) fn line_height(&self) -> Pixels {
        self.text_style().line_height_in_pixels()
    }

    pub(crate) fn current_view(&self) -> EntityId {
        self.view_stack
            .last()
            .copied()
            .or_else(|| self.root_view.as_ref().map(AnyView::entity_id))
            .unwrap_or(EntityId(0))
    }

    pub(crate) fn pixel_snap_point(&self, p: Point<Pixels>) -> Point<Pixels> {
        let s = self.native.scale_factor;
        point(
            px((p.x.as_f32() * s).round() / s),
            px((p.y.as_f32() * s).round() / s),
        )
    }

    pub(crate) fn refresh(&mut self) {
        self.dirty = true;
    }

    pub(crate) fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    pub(crate) fn request_animation_frame(&mut self) {
        self.animation_requested = true;
        self.dirty = true;
    }

    pub(crate) fn needs_redraw(&self) -> bool {
        self.dirty || self.animation_requested
    }

    pub(crate) fn prevent_default(&mut self) {
        self.default_prevented = true;
    }

    pub(crate) fn default_prevented(&self) -> bool {
        self.default_prevented
    }

    pub(crate) fn remove_window(&mut self) {
        self.removed = true;
    }

    pub(crate) fn take_removed_flag(&mut self) -> bool {
        std::mem::take(&mut self.removed)
    }

    pub(crate) fn destroy_native(&mut self) {
        self.native.destroy();
    }

    pub(crate) fn defer(
        &mut self,
        cx: &mut App,
        f: impl FnOnce(&mut Window, &mut App) + 'static,
    ) {
        let id = self.id;
        cx.defer(move |cx| {
            let _ = cx.update_window_impl(id, |_, window, cx| f(window, cx));
        });
    }

    pub(crate) fn on_window_should_close(
        &mut self,
        _cx: &mut App,
        f: impl FnMut(&mut Window, &mut App) -> bool + 'static,
    ) {
        self.should_close = Some(Box::new(f));
    }

    pub(crate) fn check_should_close(&mut self, cx: &mut App) -> bool {
        if let Some(mut cb) = self.should_close.take() {
            let allow = cb(self, cx);
            self.should_close = Some(cb);
            allow
        } else {
            true
        }
    }

    pub(crate) fn spawn<AsyncFn, R>(&self, cx: &App, f: AsyncFn) -> Task<R>
    where
        AsyncFn: AsyncFnOnce(&mut AsyncWindowContext) -> R + 'static,
        R: 'static,
    {
        let mut async_win = AsyncWindowContext {
            app: cx.self_weak.clone(),
            window_id: self.id,
            background: cx.background.clone(),
        };
        cx.foreground.spawn(async move { f(&mut async_win).await })
    }

    pub(crate) fn listener_for<T: 'static, E: ?Sized>(
        &self,
        entity: &Entity<T>,
        mut f: impl FnMut(&mut T, &E, &mut Window, &mut Context<T>) + 'static,
    ) -> impl FnMut(&E, &mut Window, &mut App) + 'static {
        let weak = entity.downgrade();
        move |event, window, app| {
            let _ = weak.update(app, |this, cx| f(this, event, window, cx));
        }
    }

    pub(crate) fn use_state<T: 'static>(
        &mut self,
        cx: &mut App,
        init: impl FnOnce(&mut Window, &mut Context<T>) -> T,
    ) -> Entity<T> {
        let gid = GlobalElementId(self.id_stack.clone());
        self.with_element_state(&gid, |prev: Option<Entity<T>>, window| {
            let entity = prev.unwrap_or_else(|| cx.new(|cx| init(window, cx)));
            (entity.clone(), entity)
        })
    }

    pub(crate) fn use_keyed_state<T: 'static>(
        &mut self,
        id: impl Into<ElementId>,
        cx: &mut App,
        init: impl FnOnce(&mut Window, &mut Context<T>) -> T,
    ) -> Entity<T> {
        self.with_id(id.into(), |gid, window| {
            window.with_element_state(gid, |prev: Option<Entity<T>>, window| {
                let entity = prev.unwrap_or_else(|| cx.new(|cx| init(window, cx)));
                (entity.clone(), entity)
            })
        })
    }

    pub(crate) fn with_element_state<S: 'static, R>(
        &mut self,
        id: &GlobalElementId,
        f: impl FnOnce(Option<S>, &mut Window) -> (R, S),
    ) -> R {
        let key = (id.clone(), TypeId::of::<S>());
        let prev = self
            .element_states
            .remove(&key)
            .or_else(|| self.next_element_states.remove(&key))
            .and_then(|boxed| boxed.downcast::<S>().ok())
            .map(|b| *b);
        let (result, next) = f(prev, self);
        self.next_element_states.insert(key, Box::new(next));
        result
    }

    #[cfg(test)]
    pub(crate) fn record_test_element_bounds(&mut self, id: ElementId, bounds: Bounds<Pixels>) {
        self.frame.element_bounds.insert(id, bounds);
    }

    #[cfg(test)]
    pub(crate) fn test_element_bounds(&self, id: &ElementId) -> Option<Bounds<Pixels>> {
        self.prev_frame.element_bounds.get(id).copied()
    }

    #[cfg(test)]
    pub(crate) fn frame_element_bounds(&self, id: &ElementId) -> Option<Bounds<Pixels>> {
        self.frame.element_bounds.get(id).copied()
    }

    #[cfg(test)]
    pub(crate) fn find_test_element_bounds(
        &self,
        pred: impl Fn(&ElementId) -> bool,
    ) -> Option<Bounds<Pixels>> {
        self.prev_frame
            .element_bounds
            .iter()
            .find(|(id, _)| pred(id))
            .map(|(_, &bounds)| bounds)
    }

    // ---- Scoped stacks ----

    pub(crate) fn with_id<R>(
        &mut self,
        id: ElementId,
        f: impl FnOnce(&GlobalElementId, &mut Window) -> R,
    ) -> R {
        self.id_stack.push(id);
        let gid = GlobalElementId(self.id_stack.clone());
        let res = f(&gid, self);
        self.id_stack.pop();
        res
    }

    pub(crate) fn with_current_view<R>(
        &mut self,
        view_id: EntityId,
        f: impl FnOnce(&mut Window) -> R,
    ) -> R {
        self.view_stack.push(view_id);
        let res = f(self);
        self.view_stack.pop();
        res
    }

    pub(crate) fn with_text_style<R>(
        &mut self,
        refinement: &TextStyleRefinement,
        f: impl FnOnce(&mut Window) -> R,
    ) -> R {
        let mut next = self.text_style();
        next.refine(refinement);
        self.text_style_stack.push(next);
        let res = f(self);
        self.text_style_stack.pop();
        res
    }

    pub(crate) fn with_element_offset<R>(
        &mut self,
        offset: Point<Pixels>,
        f: impl FnOnce(&mut Window) -> R,
    ) -> R {
        let cur = self.element_offset_stack.last().copied().unwrap_or_default();
        self.element_offset_stack.push(cur + offset);
        let res = f(self);
        self.element_offset_stack.pop();
        res
    }

    pub(crate) fn with_content_mask<R>(
        &mut self,
        mask: Option<ContentMask>,
        f: impl FnOnce(&mut Window) -> R,
    ) -> R {
        let Some(mask) = mask else {
            return f(self);
        };
        let effective = match self.content_mask_stack.last() {
            Some(prev) => prev.intersect(&mask.bounds),
            None => mask.bounds,
        };
        self.content_mask_stack.push(effective);
        let clip_pushed = self.in_paint;
        if clip_pushed {
            self.renderer.push_clip(effective);
        }
        let res = f(self);
        if clip_pushed {
            self.renderer.pop_clip();
        }
        self.content_mask_stack.pop();
        res
    }

    pub(crate) fn with_opacity<R>(
        &mut self,
        bounds: Bounds<Pixels>,
        opacity: f32,
        f: impl FnOnce(&mut Window) -> R,
    ) -> R {
        if !self.in_paint {
            return f(self);
        }
        self.renderer.push_opacity_layer(bounds, opacity);
        let res = f(self);
        self.renderer.pop_layer();
        res
    }

    pub(crate) fn with_paint_focus<R>(
        &mut self,
        focus_id: Option<FocusId>,
        f: impl FnOnce(&mut Window) -> R,
    ) -> R {
        if let Some(fid) = focus_id {
            self.focus_stack.push(fid);
            let res = f(self);
            self.focus_stack.pop();
            res
        } else {
            f(self)
        }
    }

    #[inline]
    pub(crate) fn current_focus_id(&self) -> Option<FocusId> {
        self.focus_stack.last().copied()
    }

    pub(crate) fn with_focus_node<R>(
        &mut self,
        focus_id: Option<FocusId>,
        key_context: Option<KeyContext>,
        f: impl FnOnce(&mut Window) -> R,
    ) -> R {
        let pushed_ctx = key_context.is_some();
        if let Some(ctx) = key_context {
            if focus_id.is_none() && self.focus_stack.is_empty() {
                self.frame.root_key_contexts.push(ctx.clone());
            }
            self.key_context_stack.push(ctx);
        }
        let res = if let Some(fid) = focus_id {
            let parent = self.focus_stack.last().copied();
            self.frame.focus_nodes.insert(
                fid,
                FocusNode {
                    parent,
                    key_contexts: self.key_context_stack.clone(),
                },
            );
            self.focus_stack.push(fid);
            let res = f(self);
            self.focus_stack.pop();
            res
        } else {
            f(self)
        };
        if pushed_ctx {
            self.key_context_stack.pop();
        }
        res
    }

    // ---- Layout bridge ----

    pub(crate) fn request_layout(
        &mut self,
        style: Style,
        children: impl IntoIterator<Item = LayoutId>,
        _cx: &mut App,
    ) -> LayoutId {
        let ids: Vec<LayoutId> = children.into_iter().collect();
        self.layout_engine
            .request_layout(style, self.native.scale_factor, &ids)
    }

    pub(crate) fn request_measured_layout(
        &mut self,
        style: Style,
        measure: impl FnMut(Size<Option<Pixels>>, Size<AvailableSpace>, &mut Window, &mut App) -> Size<Pixels>
            + 'static,
        _cx: &mut App,
    ) -> LayoutId {
        self.layout_engine
            .request_measured_layout(style, self.native.scale_factor, measure)
    }

    pub(crate) fn compute_layout(
        &mut self,
        root: LayoutId,
        available: Size<AvailableSpace>,
        cx: &mut App,
    ) {
        let mut engine = std::mem::replace(&mut self.layout_engine, LayoutEngine::new());
        engine.compute_layout(root, available, self, cx);
        self.layout_engine = engine;
    }

    pub(crate) fn layout_bounds(&mut self, id: LayoutId) -> Bounds<Pixels> {
        let offset = self.element_offset_stack.last().copied().unwrap_or_default();
        let mut b = self.layout_engine.layout_bounds(id, self.native.scale_factor);
        b.origin += offset;
        b
    }

    // ---- Hitboxes, controls, paint primitives ----

    pub(crate) fn insert_hitbox(
        &mut self,
        bounds: Bounds<Pixels>,
        behavior: HitboxBehavior,
    ) -> Hitbox {
        let id = HitboxId(self.frame.next_hitbox_id);
        self.frame.next_hitbox_id += 1;
        let hb = Hitbox {
            id,
            bounds,
            content_mask: self.content_mask_stack.last().copied(),
            behavior,
        };
        self.frame.hitboxes.push(hb.clone());
        hb
    }

    pub(crate) fn is_hitbox_hovered(&self, id: HitboxId) -> bool {
        if self.in_paint {
            self.frame.hovered_hitboxes.contains(&id)
        } else {
            self.prev_frame.hovered_hitboxes.contains(&id)
        }
    }

    pub(crate) fn is_hitbox_hovered_at(&self, id: HitboxId, pos: Point<Pixels>) -> bool {
        let frame = if self.in_paint {
            &self.frame
        } else {
            &self.prev_frame
        };
        for hb in frame.hitboxes.iter().rev() {
            if hb.contains(&pos) {
                if hb.id == id {
                    return true;
                }
                if matches!(hb.behavior, HitboxBehavior::BlockMouse) {
                    return false;
                }
            }
        }
        false
    }

    pub(crate) fn register_group_hitbox(&mut self, group: SharedString, id: HitboxId) {
        self.frame.group_hitboxes.entry(group).or_default().push(id);
    }

    pub(crate) fn is_group_hovered(&self, group: &str) -> bool {
        let f = if self.in_paint {
            &self.frame
        } else {
            &self.prev_frame
        };
        f.group_hitboxes
            .get(group)
            .is_some_and(|ids| ids.iter().any(|id| f.hovered_hitboxes.contains(id)))
    }

    pub(crate) fn insert_window_control(
        &mut self,
        bounds: Bounds<Pixels>,
        area: WindowControlArea,
    ) {
        self.frame.window_controls.push((bounds, area));
    }

    pub(crate) fn hit_test_control_area(&self, pos: Point<Pixels>) -> Option<WindowControlArea> {
        // Check in reverse paint order so caption buttons beat the underlying drag bar.
        for &(bounds, area) in self.prev_frame.window_controls.iter().rev() {
            if bounds.contains(&pos) {
                return Some(area);
            }
        }
        None
    }

    pub(crate) fn set_cursor_style(&mut self, style: CursorStyle, hitbox: &Hitbox) {
        self.frame.cursor_regions.push((hitbox.clone(), style));
    }

    pub(crate) fn paint_quad(&mut self, quad: PaintQuad) {
        self.renderer.paint_quad(quad);
    }

    pub(crate) fn paint_shadows(
        &mut self,
        bounds: Bounds<Pixels>,
        radii: Corners<Pixels>,
        shadows: &[BoxShadow],
    ) {
        self.renderer.paint_shadows(bounds, radii, shadows);
    }

    pub(crate) fn paint_layer<R>(
        &mut self,
        bounds: Bounds<Pixels>,
        f: impl FnOnce(&mut Window) -> R,
    ) -> R {
        if !self.in_paint {
            return f(self);
        }
        self.renderer.push_clip(bounds);
        let res = f(self);
        self.renderer.pop_clip();
        res
    }

    pub(crate) fn paint_dwrite_layout(
        &mut self,
        origin: Point<Pixels>,
        layout: &IDWriteTextLayout,
        default_color: Rgba,
        run_colors: &[(u32, u32, Rgba)],
    ) {
        self.renderer
            .paint_text_layout(origin, layout, default_color, run_colors);
    }

    pub(crate) fn defer_draw(&mut self, element: AnyElement, priority: usize) {
        let order = self.deferred_draws.len();
        self.deferred_draws.push(DeferredDraw {
            priority,
            order,
            id_stack: self.id_stack.clone(),
            text_style: self.text_style(),
            element_offset: self.element_offset_stack.last().copied().unwrap_or_default(),
            key_context_stack: self.key_context_stack.clone(),
            focus_stack: self.focus_stack.clone(),
            element,
        });
    }

    pub(crate) fn handle_input<V: EntityInputHandler>(
        &mut self,
        focus: &FocusHandle,
        handler: ElementInputHandler<V>,
        _cx: &mut App,
    ) {
        if self.focused_id() == Some(focus.id()) {
            self.frame.input_handler = Some((focus.id(), Box::new(handler)));
        }
    }

    // ---- Listener registration during paint ----

    pub(crate) fn on_mouse_event<E: MouseEvent>(
        &mut self,
        mut f: impl FnMut(&E, DispatchPhase, &mut Window, &mut App) + 'static,
    ) {
        self.frame.mouse_listeners.push((
            TypeId::of::<E>(),
            Box::new(move |ev, phase, window, cx| {
                if let Some(typed) = ev.downcast_ref::<E>() {
                    f(typed, phase, window, cx);
                }
            }),
        ));
    }

    pub(crate) fn on_scroll_wheel_event(
        &mut self,
        f: impl FnMut(&ScrollWheelEvent, DispatchPhase, &mut Window, &mut App) + 'static,
    ) {
        self.frame.scroll_listeners.push(Box::new(f));
    }

    pub(crate) fn on_key_event<E: 'static>(
        &mut self,
        mut f: impl FnMut(&E, DispatchPhase, &mut Window, &mut App) + 'static,
    ) {
        if TypeId::of::<E>() == TypeId::of::<KeyDownEvent>() {
            self.frame
                .key_listeners
                .push(Box::new(move |ev, phase, window, cx| {
                    if let Some(typed) = (ev as &dyn Any).downcast_ref::<E>() {
                        f(typed, phase, window, cx);
                    }
                }));
        }
    }

    pub(crate) fn on_modifiers_changed(
        &mut self,
        f: impl FnMut(&ModifiersChangedEvent, &mut Window, &mut App) + 'static,
    ) {
        self.frame.mod_listeners.push(Box::new(f));
    }

    pub(crate) fn register_action_listener(
        &mut self,
        focus: Option<FocusId>,
        type_id: TypeId,
        handler: ActionHandlerBox,
    ) {
        self.frame.action_listeners.push((focus, type_id, handler));
    }

    pub(crate) fn register_key_down_listener(
        &mut self,
        focus: Option<FocusId>,
        handler: KeyDownHandlerBox,
    ) {
        self.frame.key_down_listeners.push((focus, handler));
    }

    pub(crate) fn register_drop_listener(
        &mut self,
        hitbox: Option<Hitbox>,
        handler: DropHandlerBox,
    ) {
        self.frame.drop_listeners.push((hitbox, handler));
    }

    // ---- Focus & key contexts ----

    pub(crate) fn focused_id(&self) -> Option<FocusId> {
        self.focused_handle.as_ref().map(FocusHandle::id)
    }

    pub(crate) fn focused(&self, _cx: &App) -> Option<FocusHandle> {
        self.focused_handle.clone()
    }

    pub(crate) fn focus_handle(&mut self, handle: FocusHandle, cx: &mut App) {
        let id = handle.id();
        let prev = self.focused_id();
        if prev == Some(id) {
            return;
        }
        self.focused_handle = Some(handle);
        self.dirty = true;
        if let Some(old) = prev {
            cx.queue_focus_event(self.id, old, false);
        }
        cx.queue_focus_event(self.id, id, true);
    }

    pub(crate) fn focus_contains(&self, ancestor: FocusId) -> bool {
        let mut cur = self.focused_id();
        while let Some(id) = cur {
            if id == ancestor {
                return true;
            }
            cur = self
                .prev_frame
                .focus_nodes
                .get(&id)
                .or_else(|| self.frame.focus_nodes.get(&id))
                .and_then(|n| n.parent);
        }
        false
    }

    fn key_contexts_for(&self, focus: Option<FocusId>) -> Vec<KeyContext> {
        let frame = if !self.prev_frame.focus_nodes.is_empty() {
            &self.prev_frame
        } else {
            &self.frame
        };
        if let Some(id) = focus
            && let Some(node) = frame.focus_nodes.get(&id)
        {
            return node.key_contexts.clone();
        }
        frame.root_key_contexts.clone()
    }

    pub(crate) fn highest_precedence_binding_for_action_in(
        &self,
        action: &dyn Action,
        focus: &FocusHandle,
    ) -> Option<KeyBinding> {
        let contexts = self.key_contexts_for(Some(focus.id()));
        self.keymap
            .borrow()
            .highest_precedence_for_action(action, &contexts)
    }

    pub(crate) fn dispatch_action(&mut self, action: Box<dyn Action>, cx: &mut App) -> bool {
        self.dispatch_action_internal(self.focused_id(), action, cx)
    }

    pub(crate) fn dispatch_action_on(
        &mut self,
        target: FocusId,
        action: Box<dyn Action>,
        cx: &mut App,
    ) {
        self.dispatch_action_internal(Some(target), action, cx);
    }

    fn dispatch_action_internal(
        &mut self,
        start: Option<FocusId>,
        action: Box<dyn Action>,
        cx: &mut App,
    ) -> bool {
        let type_id = action.as_any().type_id();
        let mut focus_chain: Vec<Option<FocusId>> = Vec::new();
        let mut cur = start;
        while let Some(id) = cur {
            focus_chain.push(Some(id));
            cur = self
                .prev_frame
                .focus_nodes
                .get(&id)
                .and_then(|n| n.parent);
        }
        focus_chain.push(None);

        let mut listeners = std::mem::take(&mut self.prev_frame.action_listeners);
        let mut handled = false;
        'outer: for target in &focus_chain {
            for (owner, tid, handler) in listeners.iter_mut().rev() {
                if owner == target && *tid == type_id {
                    cx.propagate_action = false;
                    handler(action.as_ref(), self, cx);
                    if !cx.propagate_action {
                        handled = true;
                        break 'outer;
                    }
                }
            }
        }
        self.prev_frame.action_listeners = listeners;

        if !handled {
            handled = cx.dispatch_global_action(action.as_ref());
        }
        self.dirty = true;
        handled
    }

    // ---- Observers ----

    pub(crate) fn on_activation_change(
        &mut self,
        f: impl FnMut(&mut Window, &mut App) -> bool + 'static,
    ) -> Subscription {
        let active = Rc::new(Cell::new(true));
        self.activation_observers.push((active.clone(), Box::new(f)));
        Subscription::new(move || active.set(false))
    }

    pub(crate) fn on_appearance_change(
        &mut self,
        f: impl FnMut(&mut Window, &mut App) -> bool + 'static,
    ) -> Subscription {
        let active = Rc::new(Cell::new(true));
        self.appearance_observers.push((active.clone(), Box::new(f)));
        Subscription::new(move || active.set(false))
    }

    pub(crate) fn on_bounds_change(
        &mut self,
        f: impl FnMut(&mut Window, &mut App) -> bool + 'static,
    ) -> Subscription {
        let active = Rc::new(Cell::new(true));
        self.bounds_observers.push((active.clone(), Box::new(f)));
        Subscription::new(move || active.set(false))
    }

    pub(crate) fn on_focus_in(
        &mut self,
        id: FocusId,
        f: impl FnMut(&mut Window, &mut App) -> bool + 'static,
    ) -> Subscription {
        let active = Rc::new(Cell::new(true));
        self.focus_in_observers
            .push((id, active.clone(), Box::new(f)));
        Subscription::new(move || active.set(false))
    }

    pub(crate) fn on_focus_out(
        &mut self,
        id: FocusId,
        f: impl FnMut(&mut Window, &mut App) -> bool + 'static,
    ) -> Subscription {
        let active = Rc::new(Cell::new(true));
        self.focus_out_observers
            .push((id, active.clone(), Box::new(f)));
        Subscription::new(move || active.set(false))
    }

    pub(crate) fn notify_activation_changed(&mut self, cx: &mut App) {
        let mut obs = std::mem::take(&mut self.activation_observers);
        obs.retain_mut(|(a, cb)| a.get() && cb(self, cx));
        obs.append(&mut self.activation_observers);
        self.activation_observers = obs;
        self.dirty = true;
    }

    pub(crate) fn apply_appearance(&mut self, appearance: WindowAppearance, cx: &mut App) {
        self.appearance = appearance;
        self.native.set_appearance(appearance);
        let mut obs = std::mem::take(&mut self.appearance_observers);
        obs.retain_mut(|(a, cb)| a.get() && cb(self, cx));
        obs.append(&mut self.appearance_observers);
        self.appearance_observers = obs;
        self.dirty = true;
    }

    pub(crate) fn notify_bounds_changed(&mut self, cx: &mut App) {
        let mut obs = std::mem::take(&mut self.bounds_observers);
        obs.retain_mut(|(a, cb)| a.get() && cb(self, cx));
        obs.append(&mut self.bounds_observers);
        self.bounds_observers = obs;
    }

    pub(crate) fn fire_focus_in(&mut self, id: FocusId, cx: &mut App) {
        let mut obs = std::mem::take(&mut self.focus_in_observers);
        obs.retain_mut(|(target, a, cb)| a.get() && (*target != id || cb(self, cx)));
        obs.append(&mut self.focus_in_observers);
        self.focus_in_observers = obs;
    }

    pub(crate) fn fire_focus_out(&mut self, id: FocusId, cx: &mut App) {
        let mut obs = std::mem::take(&mut self.focus_out_observers);
        obs.retain_mut(|(target, a, cb)| a.get() && (*target != id || cb(self, cx)));
        obs.append(&mut self.focus_out_observers);
        self.focus_out_observers = obs;
    }

    // ---- Frame lifecycle ----

    pub(crate) fn on_resize(
        &mut self,
        physical_width: u32,
        physical_height: u32,
        scale_factor: f32,
        cx: &mut App,
    ) {
        let _ = self
            .renderer
            .resize(physical_width, physical_height, scale_factor);
        self.notify_bounds_changed(cx);
        self.dirty = true;
    }

    pub(crate) fn draw_frame(&mut self, root: &AnyView, cx: &mut App) {
        self.dirty = false;
        self.animation_requested = false;
        // Reconcile live Win32 client rect & DPI (catches `SetWindowPos` / `overlay::place`
        // resizes that fired while `AppCell` was mutably borrowed, and skips minimized 0x0 frames).
        let Some((pw, ph, scale)) = self.native.live_client_metrics() else {
            return;
        };
        let _ = self.renderer.resize(pw, ph, scale);
        self.layout_engine.clear();
        self.frame = FrameState::default();
        self.next_element_states.clear();
        self.deferred_draws.clear();
        self.id_stack.clear();
        self.text_style_stack.clear();
        self.text_style_stack.push(TextStyle::default());
        self.content_mask_stack.clear();
        self.element_offset_stack.clear();
        self.element_offset_stack.push(Point::default());
        self.focus_stack.clear();
        self.key_context_stack.clear();
        self.view_stack.clear();
        self.view_stack.push(root.entity_id());

        // 1. Render + Layout
        let mut root_elem = root.render(self, cx);
        let root_layout_id = root_elem.request_layout(self, cx);
        let viewport = self.viewport_size();
        self.layout_engine
            .stretch_auto_size_to_fill(root_layout_id, viewport, self.native.scale_factor);
        self.compute_layout(root_layout_id, viewport.into(), cx);

        // 2. Prepaint (main tree + deferred popups sorted by priority)
        root_elem.prepaint(self, cx);
        let mut deferred = std::mem::take(&mut self.deferred_draws);
        deferred.sort_by_key(|d| (d.priority, d.order));
        for entry in &mut deferred {
            let saved_ids = std::mem::replace(&mut self.id_stack, entry.id_stack.clone());
            let saved_ctx =
                std::mem::replace(&mut self.key_context_stack, entry.key_context_stack.clone());
            let saved_focus = std::mem::replace(&mut self.focus_stack, entry.focus_stack.clone());
            self.text_style_stack.push(entry.text_style.clone());
            self.element_offset_stack.push(entry.element_offset);
            entry.element.prepaint(self, cx);
            self.element_offset_stack.pop();
            self.text_style_stack.pop();
            self.focus_stack = saved_focus;
            self.key_context_stack = saved_ctx;
            self.id_stack = saved_ids;
        }

        // Compute hovered hitboxes for the freshly prepainted frame before paint so hover
        // styles render accurately on the very first frame.
        Self::update_hovered_hitboxes_in(self.native.hovered, self.mouse_position, &mut self.frame);

        // 3. Paint onto Direct2D 1.1 target bitmap
        self.focus_stack.clear();
        self.content_mask_stack.clear();
        self.text_style_stack.clear();
        self.text_style_stack.push(TextStyle::default());
        self.renderer.begin_frame(transparent_black());
        self.in_paint = true;
        root_elem.paint(self, cx);
        for entry in &mut deferred {
            let saved_ids = std::mem::replace(&mut self.id_stack, entry.id_stack.clone());
            let saved_focus = std::mem::replace(&mut self.focus_stack, entry.focus_stack.clone());
            self.text_style_stack.push(entry.text_style.clone());
            entry.element.paint(self, cx);
            self.text_style_stack.pop();
            self.focus_stack = saved_focus;
            self.id_stack = saved_ids;
        }
        self.in_paint = false;
        let device_lost = self.renderer.end_frame().unwrap_or(false);
        if device_lost {
            let (pw, ph) = self.native.physical_size();
            if let Ok(devices) = super::graphics::d2d_renderer::Direct2DDevices::new()
                && let Ok(rebuilt) = D2DRenderer::new(
                    devices,
                    self.native.hwnd,
                    pw,
                    ph,
                    self.native.scale_factor,
                )
            {
                self.renderer = rebuilt;
                self.dirty = true;
            }
        }

        self.element_states = std::mem::take(&mut self.next_element_states);
        self.prev_frame = std::mem::take(&mut self.frame);
        self.update_cursor_for_position();
    }

    fn update_hovered_hitboxes_in(hovered: bool, pos: Point<Pixels>, frame: &mut FrameState) {
        frame.hovered_hitboxes.clear();
        if !hovered {
            return;
        }
        for hb in frame.hitboxes.iter().rev() {
            if hb.contains(&pos) {
                frame.hovered_hitboxes.insert(hb.id);
                if matches!(hb.behavior, HitboxBehavior::BlockMouse) {
                    break;
                }
            }
        }
    }

    fn refresh_hovered(&mut self) {
        Self::update_hovered_hitboxes_in(
            self.native.hovered,
            self.mouse_position,
            &mut self.prev_frame,
        );
        self.update_cursor_for_position();
    }

    fn update_cursor_for_position(&mut self) {
        let mut cursor = CursorStyle::Arrow;
        for (hb, style) in self.prev_frame.cursor_regions.iter().rev() {
            if self.prev_frame.hovered_hitboxes.contains(&hb.id) {
                cursor = *style;
                break;
            }
        }
        if cursor != self.native.cursor {
            self.native.set_cursor(cursor);
        }
    }

    // ---- Input dispatch ----

    fn dispatch_mouse_event_typed<E: MouseEvent>(&mut self, event: E, cx: &mut App) {
        self.default_prevented = false;
        let type_id = TypeId::of::<E>();
        let mut listeners = std::mem::take(&mut self.prev_frame.mouse_listeners);
        cx.stop_propagation = false;
        for (tid, cb) in &mut listeners {
            if *tid == type_id {
                cb(&event, DispatchPhase::Capture, self, cx);
                if cx.stop_propagation {
                    break;
                }
            }
        }
        if !cx.stop_propagation {
            for (tid, cb) in listeners.iter_mut().rev() {
                if *tid == type_id {
                    cb(&event, DispatchPhase::Bubble, self, cx);
                    if cx.stop_propagation {
                        break;
                    }
                }
            }
        }
        cx.stop_propagation = false;
        self.prev_frame.mouse_listeners = listeners;
    }

    pub(crate) fn dispatch_mouse_move(&mut self, event: MouseMoveEvent, cx: &mut App) {
        self.mouse_position = event.position;
        let vp = self.viewport_size();
        if event.position.x >= px(0.0)
            && event.position.y >= px(0.0)
            && event.position.x < vp.width
            && event.position.y < vp.height
        {
            self.native.hovered = true;
        }
        self.refresh_hovered();
        self.dispatch_mouse_event_typed(event, cx);
    }

    pub(crate) fn on_mouse_leave(&mut self, cx: &mut App) {
        self.native.hovered = false;
        self.prev_frame.hovered_hitboxes.clear();
        self.dispatch_mouse_event_typed(
            MouseMoveEvent {
                position: point(px(-1e6), px(-1e6)),
                pressed_button: self.native.pressed_button,
                modifiers: super::platform::window::current_modifiers(),
            },
            cx,
        );
        self.dirty = true;
    }

    pub(crate) fn dispatch_mouse_down(&mut self, event: MouseDownEvent, cx: &mut App) {
        self.last_input_was_keyboard = false;
        self.native.hovered = true;
        self.mouse_position = event.position;
        self.refresh_hovered();
        self.dispatch_mouse_event_typed(event, cx);
        self.dirty = true;
    }

    pub(crate) fn dispatch_mouse_up(&mut self, event: MouseUpEvent, cx: &mut App) {
        self.mouse_position = event.position;
        self.refresh_hovered();
        self.dispatch_mouse_event_typed(event, cx);
        self.dirty = true;
    }

    pub(crate) fn dispatch_scroll_wheel(&mut self, event: ScrollWheelEvent, cx: &mut App) {
        self.mouse_position = event.position;
        let mut listeners = std::mem::take(&mut self.prev_frame.scroll_listeners);
        cx.stop_propagation = false;
        for cb in &mut listeners {
            cb(&event, DispatchPhase::Capture, self, cx);
            if cx.stop_propagation {
                break;
            }
        }
        if !cx.stop_propagation {
            for cb in listeners.iter_mut().rev() {
                cb(&event, DispatchPhase::Bubble, self, cx);
                if cx.stop_propagation {
                    break;
                }
            }
        }
        cx.stop_propagation = false;
        self.prev_frame.scroll_listeners = listeners;
        // Also deliver to `window.on_mouse_event::<ScrollWheelEvent>` listeners (e.g. Scrollbar).
        self.dispatch_mouse_event_typed(event, cx);
    }

    pub(crate) fn dispatch_modifiers_changed(
        &mut self,
        event: ModifiersChangedEvent,
        cx: &mut App,
    ) {
        let mut listeners = std::mem::take(&mut self.prev_frame.mod_listeners);
        for cb in &mut listeners {
            cb(&event, self, cx);
        }
        self.prev_frame.mod_listeners = listeners;
    }

    pub(crate) fn dispatch_key_down(&mut self, event: KeyDownEvent, cx: &mut App) -> bool {
        self.last_input_was_keyboard = true;
        if cx.run_keystroke_interceptors(&event, self) {
            return true;
        }

        // Capture key listeners
        let mut key_listeners = std::mem::take(&mut self.prev_frame.key_listeners);
        cx.stop_propagation = false;
        for cb in &mut key_listeners {
            cb(&event, DispatchPhase::Capture, self, cx);
            if cx.stop_propagation {
                break;
            }
        }
        self.prev_frame.key_listeners = key_listeners;
        if cx.stop_propagation {
            cx.stop_propagation = false;
            return true;
        }

        // Keymap action lookup along active focus chain
        let contexts = self.key_contexts_for(self.focused_id());
        let actions = self
            .keymap
            .borrow()
            .match_keystroke(&event.keystroke, &contexts);
        for action in actions {
            if self.dispatch_action(action, cx) {
                return true;
            }
        }

        // Focused element `on_key_down` listeners
        let mut kd_listeners = std::mem::take(&mut self.prev_frame.key_down_listeners);
        let mut handled = false;
        cx.stop_propagation = false;
        for (owner, cb) in kd_listeners.iter_mut().rev() {
            if owner.is_none_or(|fid| self.focus_contains(fid)) {
                cb(&event, self, cx);
                if cx.stop_propagation {
                    handled = true;
                    break;
                }
            }
        }
        cx.stop_propagation = false;
        self.prev_frame.key_down_listeners = kd_listeners;
        handled
    }

    pub(crate) fn dispatch_text_input(&mut self, text: &str, cx: &mut App) {
        if let Some((fid, mut handler)) = self.prev_frame.input_handler.take() {
            handler.replace_text_in_range(None, text, self, cx);
            self.prev_frame.input_handler = Some((fid, handler));
            self.dirty = true;
        }
    }

    pub(crate) fn dispatch_ime_composition(&mut self, text: &str, cursor_utf16: usize, cx: &mut App) {
        if let Some((fid, mut handler)) = self.prev_frame.input_handler.take() {
            handler.replace_and_mark_text_in_range(
                None,
                text,
                Some(cursor_utf16..cursor_utf16),
                self,
                cx,
            );
            self.prev_frame.input_handler = Some((fid, handler));
            self.dirty = true;
        }
    }

    pub(crate) fn query_ime_caret_bounds(&mut self, cx: &mut App) -> Option<Bounds<Pixels>> {
        let (fid, mut handler) = self.prev_frame.input_handler.take()?;
        let sel = handler.selected_text_range(self, cx);
        let bounds = sel.and_then(|s| handler.bounds_for_range(s.range, self, cx));
        self.prev_frame.input_handler = Some((fid, handler));
        bounds
    }

    pub(crate) fn dispatch_file_drop(
        &mut self,
        paths: ExternalPaths,
        pos: Point<Pixels>,
        cx: &mut App,
    ) {
        let mut listeners = std::mem::take(&mut self.prev_frame.drop_listeners);
        for (hb, cb) in listeners.iter_mut().rev() {
            if hb.as_ref().is_none_or(|h| h.contains(&pos)) {
                cb(&paths, self, cx);
                break;
            }
        }
        self.prev_frame.drop_listeners = listeners;
    }
}
