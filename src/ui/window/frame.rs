use std::{
    any::{Any, TypeId},
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant},
};

use crate::ui::{
    AnyElement, App, Bounds, ContentMask, Corners, CursorStyle, DispatchNodeId, DispatchTree,
    Edges, Effect, EntityId, FocusId, GlobalElementId, Pixels, PlatformInputHandler, Point, Rgba,
    Scene, TextStyleRefinement, Window, WindowId, collections::FxHashMap, smallvec::SmallVec,
    transparent_black,
};

use super::{
    AnyMouseListener, CursorStyleRequest, ElementIdStack, HitTest, Hitbox, HitboxBehavior,
    WindowControlArea,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DrawPhase {
    None,
    Prepaint,
    Paint,
    Focus,
}

struct WindowInvalidatorInner {
    dirty: bool,
    draw_phase: DrawPhase,
    update_count: usize,
}

#[derive(Clone)]
pub(crate) struct WindowInvalidator {
    inner: Rc<RefCell<WindowInvalidatorInner>>,
}

impl WindowInvalidator {
    pub(crate) fn new(_window_id: WindowId) -> Self {
        WindowInvalidator {
            inner: Rc::new(RefCell::new(WindowInvalidatorInner {
                dirty: true,
                draw_phase: DrawPhase::None,
                update_count: 0,
            })),
        }
    }

    pub(crate) fn invalidate_view(&self, entity: EntityId, cx: &mut App) -> bool {
        let mut inner = self.inner.borrow_mut();
        inner.update_count += 1;
        if inner.draw_phase == DrawPhase::None {
            inner.dirty = true;
            drop(inner);
            cx.push_effect(Effect::Notify { emitter: entity });
            true
        } else {
            false
        }
    }

    pub(crate) fn is_dirty(&self) -> bool {
        self.inner.borrow().dirty
    }

    pub(crate) fn set_dirty(&self, dirty: bool) {
        let mut inner = self.inner.borrow_mut();
        inner.dirty = dirty;
        if dirty {
            inner.update_count += 1;
        }
    }

    pub(crate) fn set_phase(&self, phase: DrawPhase) {
        self.inner.borrow_mut().draw_phase = phase
    }

    pub(crate) fn update_count(&self) -> usize {
        self.inner.borrow().update_count
    }

    pub(crate) fn not_drawing(&self) -> bool {
        self.inner.borrow().draw_phase == DrawPhase::None
    }

    #[track_caller]
    pub(crate) fn debug_assert_paint(&self) {
        debug_assert!(
            matches!(self.inner.borrow().draw_phase, DrawPhase::Paint),
            "this method can only be called during paint"
        );
    }

    #[track_caller]
    pub(crate) fn debug_assert_prepaint(&self) {
        debug_assert!(
            matches!(self.inner.borrow().draw_phase, DrawPhase::Prepaint),
            "this method can only be called during request_layout, or prepaint"
        );
    }

    #[track_caller]
    pub(crate) fn debug_assert_paint_or_prepaint(&self) {
        debug_assert!(
            matches!(
                self.inner.borrow().draw_phase,
                DrawPhase::Paint | DrawPhase::Prepaint
            ),
            "this method can only be called during request_layout, prepaint, or paint"
        );
    }
}

/// Tracks input event timestamps to determine if input is arriving at a high rate.
#[derive(Clone, Debug)]
pub(crate) struct InputRateTracker {
    timestamps: Vec<Instant>,
    window: Duration,
    inputs_per_second: u32,
    sustain_until: Instant,
    sustain_duration: Duration,
}

impl Default for InputRateTracker {
    fn default() -> Self {
        Self {
            timestamps: Vec::new(),
            window: Duration::from_millis(100),
            inputs_per_second: 60,
            sustain_until: Instant::now(),
            sustain_duration: Duration::from_secs(1),
        }
    }
}

impl InputRateTracker {
    pub(crate) fn record_input(&mut self) {
        let now = Instant::now();
        self.timestamps.push(now);
        self.prune_old_timestamps(now);

        let min_events = self.inputs_per_second as u128 * self.window.as_millis() / 1000;
        if self.timestamps.len() as u128 >= min_events {
            self.sustain_until = now + self.sustain_duration;
        }
    }

    pub(crate) fn is_high_rate(&self) -> bool {
        Instant::now() < self.sustain_until
    }

    fn prune_old_timestamps(&mut self, now: Instant) {
        self.timestamps
            .retain(|&t| now.duration_since(t) <= self.window);
    }
}

pub(crate) struct ElementStateBox {
    pub(crate) inner: Box<dyn Any>,
    #[cfg(debug_assertions)]
    pub(crate) type_name: &'static str,
}

pub(crate) struct DeferredDraw {
    pub(super) current_view: EntityId,
    pub(super) priority: usize,
    pub(super) parent_node: DispatchNodeId,
    pub(super) element_id_stack: ElementIdStack,
    pub(super) text_style_stack: Vec<TextStyleRefinement>,
    pub(super) content_mask: Option<ContentMask<Pixels>>,
    pub(super) element: Option<AnyElement>,
    pub(super) absolute_offset: Point<Pixels>,
}

pub(crate) struct Frame {
    pub(crate) focus: Option<FocusId>,
    pub(crate) window_active: bool,
    pub(crate) element_states: FxHashMap<(GlobalElementId, TypeId), ElementStateBox>,
    pub(super) accessed_element_states: Vec<(GlobalElementId, TypeId)>,
    pub(crate) mouse_listeners: Vec<Option<AnyMouseListener>>,
    pub(crate) dispatch_tree: DispatchTree,
    pub(crate) scene: Scene,
    pub(crate) hitboxes: Vec<Hitbox>,
    pub(crate) window_control_hitboxes: Vec<(WindowControlArea, Hitbox)>,
    pub(crate) deferred_draws: Vec<DeferredDraw>,
    pub(crate) input_handlers: Vec<Option<PlatformInputHandler>>,
    pub(crate) cursor_styles: Vec<CursorStyleRequest>,
}

impl Frame {
    pub(crate) fn new(dispatch_tree: DispatchTree) -> Self {
        Frame {
            focus: None,
            window_active: false,
            element_states: FxHashMap::default(),
            accessed_element_states: Vec::new(),
            mouse_listeners: Vec::new(),
            dispatch_tree,
            scene: Scene::default(),
            hitboxes: Vec::new(),
            window_control_hitboxes: Vec::new(),
            deferred_draws: Vec::new(),
            input_handlers: Vec::new(),
            cursor_styles: Vec::new(),
        }
    }

    pub(crate) fn clear(&mut self) {
        self.element_states.clear();
        self.accessed_element_states.clear();
        self.mouse_listeners.clear();
        self.dispatch_tree.clear();
        self.scene.clear();
        self.input_handlers.clear();
        self.cursor_styles.clear();
        self.hitboxes.clear();
        self.window_control_hitboxes.clear();
        self.deferred_draws.clear();
        self.focus = None;
    }

    pub(crate) fn cursor_style(&self, window: &Window) -> Option<CursorStyle> {
        for request in self.cursor_styles.iter().rev() {
            if request.hitbox_id.is_hovered_ignoring_last_input(window) {
                return Some(request.style);
            }
        }
        None
    }

    pub(crate) fn hit_test(&self, position: Point<Pixels>) -> HitTest {
        let mut set_hover_hitbox_count = false;
        let mut hit_test = HitTest::default();
        for hitbox in self.hitboxes.iter().rev() {
            let bounds = hitbox.bounds.intersect(&hitbox.content_mask.bounds);
            if bounds.contains(&position) {
                hit_test.ids.push(hitbox.id);
                if !set_hover_hitbox_count
                    && hitbox.behavior == HitboxBehavior::BlockMouseExceptScroll
                {
                    hit_test.hover_hitbox_count = hit_test.ids.len();
                    set_hover_hitbox_count = true;
                }
                if hitbox.behavior == HitboxBehavior::BlockMouse {
                    break;
                }
            }
        }
        if !set_hover_hitbox_count {
            hit_test.hover_hitbox_count = hit_test.ids.len();
        }
        hit_test
    }

    pub(crate) fn focus_path(&self) -> SmallVec<[FocusId; 8]> {
        self.focus
            .map(|focus_id| self.dispatch_tree.focus_path(focus_id))
            .unwrap_or_default()
    }

    pub(crate) fn finish(&mut self, prev_frame: &mut Self) {
        for element_state_key in &self.accessed_element_states {
            if let Some((element_state_key, element_state)) =
                prev_frame.element_states.remove_entry(element_state_key)
            {
                self.element_states.insert(element_state_key, element_state);
            }
        }

        self.scene.finish();
    }
}

/// Rectangle primitive passed to [`Window::paint_quad`].
#[derive(Clone)]
pub(crate) struct PaintQuad {
    pub(crate) bounds: Bounds<Pixels>,
    pub(crate) corner_radii: Corners<Pixels>,
    pub(crate) background: Rgba,
    pub(crate) border_widths: Edges<Pixels>,
    pub(crate) border_color: Rgba,
}

impl PaintQuad {
    pub(crate) fn corner_radii(self, corner_radii: impl Into<Corners<Pixels>>) -> Self {
        PaintQuad {
            corner_radii: corner_radii.into(),
            ..self
        }
    }
}

pub(crate) fn quad(
    bounds: Bounds<Pixels>,
    corner_radii: impl Into<Corners<Pixels>>,
    background: Rgba,
    border_widths: impl Into<Edges<Pixels>>,
    border_color: Rgba,
) -> PaintQuad {
    PaintQuad {
        bounds,
        corner_radii: corner_radii.into(),
        background,
        border_widths: border_widths.into(),
        border_color,
    }
}

pub(crate) fn fill(bounds: impl Into<Bounds<Pixels>>, background: Rgba) -> PaintQuad {
    PaintQuad {
        bounds: bounds.into(),
        corner_radii: (0.).into(),
        background,
        border_widths: (0.).into(),
        border_color: transparent_black(),
    }
}
