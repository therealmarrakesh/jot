//! Per-frame retained state, layout bridge, scoped stacks, hitboxes, paint hooks, and listener
//! registration for `Window`.

use std::any::{Any, TypeId};
use std::collections::{HashMap, HashSet};

use windows::Win32::Graphics::DirectWrite::IDWriteTextLayout;

use crate::layout::{AvailableSpace, LayoutEngine, LayoutId};
use crate::ui::app::App;
use crate::ui::color::Rgba;
use crate::ui::element::{
    ContentMask, DispatchPhase, ElementId, GlobalElementId, Hitbox, HitboxBehavior, HitboxId,
    InputHandler, UiNode,
};
use crate::ui::entity::EntityId;
use crate::ui::focus::{FocusHandle, FocusId};
use crate::ui::geometry::{Bounds, Corners, Pixels, Point, Size};
use crate::ui::graphics::d2d_renderer::{BoxShadow, PaintQuad};
use crate::ui::keymap::{Command, KeyBinding, KeyContext};
use crate::ui::platform::{CursorStyle, ExternalPaths, WindowControlArea};
use crate::ui::style::{Style, TextStyle, TextStyleRefinement};
use crate::ui::types::SharedString;

use super::events::{KeyDownEvent, ModifiersChangedEvent, MouseEvent, ScrollWheelEvent};
use super::Window;

pub(super) type AnyMouseListener = Box<dyn FnMut(&dyn Any, DispatchPhase, &mut Window, &mut App)>;
pub(super) type AnyScrollListener =
    Box<dyn FnMut(&ScrollWheelEvent, DispatchPhase, &mut Window, &mut App)>;
pub(super) type AnyKeyListener =
    Box<dyn FnMut(&KeyDownEvent, DispatchPhase, &mut Window, &mut App)>;
pub(super) type AnyModListener = Box<dyn FnMut(&ModifiersChangedEvent, &mut Window, &mut App)>;
pub(super) type CommandHandlerBox = Box<dyn FnMut(Command, &mut Window, &mut App) -> bool>;
pub(super) type KeyDownHandlerBox = Box<dyn FnMut(&KeyDownEvent, &mut Window, &mut App)>;
pub(super) type DropHandlerBox = Box<dyn FnMut(&ExternalPaths, &mut Window, &mut App)>;
pub(super) type FocusedCommandListener = (Option<FocusId>, CommandHandlerBox);
pub(super) type FocusedKeyListener = (Option<FocusId>, KeyDownHandlerBox);
pub(super) type DropListener = (Option<Hitbox>, DropHandlerBox);

pub(super) struct DeferredDraw {
    pub(super) priority: usize,
    pub(super) order: usize,
    pub(super) id_stack: Vec<ElementId>,
    pub(super) text_style: TextStyle,
    pub(super) element_offset: Point<Pixels>,
    pub(super) key_context_stack: Vec<KeyContext>,
    pub(super) focus_stack: Vec<FocusId>,
    pub(super) element: UiNode,
}

pub(super) struct FocusNode {
    pub(super) parent: Option<FocusId>,
    pub(super) key_contexts: Vec<KeyContext>,
}

pub(super) struct FrameState {
    pub(super) hitboxes: Vec<Hitbox>,
    pub(super) next_hitbox_id: u64,
    pub(super) hovered_hitboxes: HashSet<HitboxId>,
    pub(super) group_hitboxes: HashMap<SharedString, Vec<HitboxId>>,
    pub(super) window_controls: Vec<(Bounds<Pixels>, WindowControlArea)>,
    pub(super) cursor_regions: Vec<(Hitbox, CursorStyle)>,
    pub(super) mouse_listeners: Vec<(TypeId, AnyMouseListener)>,
    pub(super) scroll_listeners: Vec<AnyScrollListener>,
    pub(super) key_listeners: Vec<AnyKeyListener>,
    pub(super) mod_listeners: Vec<AnyModListener>,
    pub(super) command_listeners: Vec<FocusedCommandListener>,
    pub(super) key_down_listeners: Vec<FocusedKeyListener>,
    pub(super) drop_listeners: Vec<DropListener>,
    pub(super) focus_nodes: HashMap<FocusId, FocusNode>,
    pub(super) root_key_contexts: Vec<KeyContext>,
    pub(super) input_handler: Option<(FocusId, Box<dyn InputHandler>)>,
    #[cfg(test)]
    pub(super) element_bounds: HashMap<ElementId, Bounds<Pixels>>,
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
            command_listeners: Vec::new(),
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

impl Window {
    // ---- Persistent per-element state ----

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

    pub(crate) fn with_anim_track<S: 'static, R>(
        &mut self,
        id: ElementId,
        f: impl FnOnce(Option<S>) -> (R, S),
    ) -> R {
        self.with_id(id, |gid, window| {
            let key = (gid.clone(), TypeId::of::<S>());
            let prev = window
                .anim_tracks
                .remove(&key)
                .or_else(|| window.next_anim_tracks.remove(&key))
                .and_then(|boxed| boxed.downcast::<S>().ok())
                .map(|b| *b);
            let (result, next) = f(prev);
            window.next_anim_tracks.insert(key, Box::new(next));
            result
        })
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

    pub(crate) fn with_content_mask<R>(
        &mut self,
        mask: Option<ContentMask>,
        f: impl FnOnce(&mut Window) -> R,
    ) -> R {
        let Some(mask) = mask else {
            return f(self);
        };
        let clipped = match self.content_mask_stack.last().copied() {
            Some(prev) => prev.intersect(&mask.bounds),
            None => mask.bounds,
        };
        self.content_mask_stack.push(clipped);
        if self.in_paint {
            self.renderer.push_clip(clipped);
        }
        let res = f(self);
        if self.in_paint {
            self.renderer.pop_clip();
        }
        self.content_mask_stack.pop();
        res
    }

    pub(crate) fn with_element_offset<R>(
        &mut self,
        offset: Point<Pixels>,
        f: impl FnOnce(&mut Window) -> R,
    ) -> R {
        let prev = self.element_offset_stack.last().copied().unwrap_or_default();
        self.element_offset_stack.push(prev + offset);
        let res = f(self);
        self.element_offset_stack.pop();
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

    pub(crate) fn defer_draw(&mut self, element: UiNode, priority: usize) {
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

    pub(crate) fn handle_input(&mut self, focus: &FocusHandle, handler: impl InputHandler) {
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

    pub(crate) fn register_command_listener(
        &mut self,
        focus: Option<FocusId>,
        handler: CommandHandlerBox,
    ) {
        self.frame.command_listeners.push((focus, handler));
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

    pub(crate) fn focus_handle(&mut self, handle: FocusHandle, cx: &mut App) {
        let id = handle.id();
        let prev = self.focused_id();
        if prev == Some(id) {
            return;
        }
        self.focused_handle = Some(handle);
        self.dirty = true;
        cx.queue_focus_change(self.id, prev, Some(id));
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

    pub(super) fn key_contexts_for(&self, focus: Option<FocusId>) -> Vec<KeyContext> {
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

    pub(crate) fn shortcut_for_command_in(
        &self,
        command: Command,
        focus: &FocusHandle,
    ) -> Option<KeyBinding> {
        let contexts = self.key_contexts_for(Some(focus.id()));
        self.keymap.borrow().shortcut_for(command, &contexts)
    }
}
