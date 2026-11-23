//! Clean-room `Window` lifecycle, 3-pass layout/prepaint/paint orchestration, and observers.

mod dispatch;
mod events;
mod frame;

pub(crate) use events::*;

use std::any::{Any, TypeId};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::layout::LayoutEngine;
use crate::win32::WindowHwnd;

use super::app::App;
use super::color::transparent_black;
use super::element::{ElementId, GlobalElementId, HitboxBehavior, UiNode};
use super::focus::{FocusHandle, FocusId};
use super::geometry::{Bounds, Pixels, Point, Size, point, px};
use super::graphics::d2d_renderer::D2DRenderer;
use super::graphics::dwrite::TextSystem;
use super::keymap::KeyContext;
use super::platform::window::NativeWindow;
use super::platform::{CursorStyle, WindowAppearance, WindowBounds, WindowOptions};
use super::style::TextStyle;
use super::types::UiResult;

use frame::{DeferredDraw, FrameState};

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

type ShouldCloseCallback = Box<dyn FnMut(&mut Window, &mut App) -> bool>;
type KeystrokeInterceptor = Box<dyn FnMut(&KeyDownEvent, &mut Window, &mut App) -> bool>;
type WindowCallback = Box<dyn FnMut(&mut Window, &mut App)>;
type FocusChangeCallback = Box<dyn FnMut(Option<FocusId>, Option<FocusId>, &mut Window, &mut App)>;

pub(crate) struct Window {
    id: WindowId,
    pub(crate) native: NativeWindow,
    renderer: D2DRenderer,
    text_system: Rc<TextSystem>,
    keymap: Rc<RefCell<super::keymap::Keymap>>,
    appearance: WindowAppearance,
    layout_engine: LayoutEngine,
    frame: FrameState,
    prev_frame: FrameState,
    element_states: HashMap<(GlobalElementId, TypeId), Box<dyn Any>>,
    next_element_states: HashMap<(GlobalElementId, TypeId), Box<dyn Any>>,
    anim_tracks: HashMap<(GlobalElementId, TypeId), Box<dyn Any>>,
    next_anim_tracks: HashMap<(GlobalElementId, TypeId), Box<dyn Any>>,
    id_stack: Vec<ElementId>,
    text_style_stack: Vec<TextStyle>,
    content_mask_stack: Vec<Bounds<Pixels>>,
    element_offset_stack: Vec<Point<Pixels>>,
    focus_stack: Vec<FocusId>,
    key_context_stack: Vec<KeyContext>,
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
    keystroke_interceptor: Option<KeystrokeInterceptor>,
    activation_observers: Vec<WindowCallback>,
    appearance_observers: Vec<WindowCallback>,
    bounds_observers: Vec<WindowCallback>,
    focus_change_observers: Vec<FocusChangeCallback>,
}

impl Window {
    pub(crate) fn new(id: WindowId, options: WindowOptions, cx: &App) -> UiResult<Self> {
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
            text_system: cx.text_system.clone(),
            keymap: cx.keymap.clone(),
            appearance,
            layout_engine: LayoutEngine::new(),
            frame: FrameState::default(),
            prev_frame: FrameState::default(),
            element_states: HashMap::new(),
            next_element_states: HashMap::new(),
            anim_tracks: HashMap::new(),
            next_anim_tracks: HashMap::new(),
            id_stack: Vec::new(),
            text_style_stack: vec![TextStyle::default()],
            content_mask_stack: Vec::new(),
            element_offset_stack: vec![Point::default()],
            focus_stack: Vec::new(),
            key_context_stack: Vec::new(),
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
            keystroke_interceptor: None,
            activation_observers: Vec::new(),
            appearance_observers: Vec::new(),
            bounds_observers: Vec::new(),
            focus_change_observers: Vec::new(),
        })
    }

    pub(crate) fn finish_open(&mut self) {
        self.native.show(self.show_on_open, self.focus_on_open);
    }

    pub(crate) fn window_id(&self) -> WindowId {
        self.id
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
            let _ = cx.update_window(id, f);
        });
    }

    pub(crate) fn on_window_should_close(
        &mut self,
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

    pub(crate) fn set_keystroke_interceptor(
        &mut self,
        f: impl FnMut(&KeyDownEvent, &mut Window, &mut App) -> bool + 'static,
    ) {
        self.keystroke_interceptor = Some(Box::new(f));
    }

    // ---- Observers ----

    pub(crate) fn on_activation_change(
        &mut self,
        f: impl FnMut(&mut Window, &mut App) + 'static,
    ) {
        self.activation_observers.push(Box::new(f));
    }

    pub(crate) fn on_appearance_change(
        &mut self,
        f: impl FnMut(&mut Window, &mut App) + 'static,
    ) {
        self.appearance_observers.push(Box::new(f));
    }

    pub(crate) fn on_bounds_change(
        &mut self,
        f: impl FnMut(&mut Window, &mut App) + 'static,
    ) {
        self.bounds_observers.push(Box::new(f));
    }

    pub(crate) fn on_focus_change(
        &mut self,
        f: impl FnMut(Option<FocusId>, Option<FocusId>, &mut Window, &mut App) + 'static,
    ) {
        self.focus_change_observers.push(Box::new(f));
    }

    pub(crate) fn notify_activation_changed(&mut self, cx: &mut App) {
        let mut obs = std::mem::take(&mut self.activation_observers);
        for cb in &mut obs {
            cb(self, cx);
        }
        obs.append(&mut self.activation_observers);
        self.activation_observers = obs;
        self.dirty = true;
    }

    pub(crate) fn apply_appearance(&mut self, appearance: WindowAppearance, cx: &mut App) {
        self.appearance = appearance;
        self.native.set_appearance(appearance);
        let mut obs = std::mem::take(&mut self.appearance_observers);
        for cb in &mut obs {
            cb(self, cx);
        }
        obs.append(&mut self.appearance_observers);
        self.appearance_observers = obs;
        self.dirty = true;
    }

    pub(crate) fn notify_bounds_changed(&mut self, cx: &mut App) {
        let mut obs = std::mem::take(&mut self.bounds_observers);
        for cb in &mut obs {
            cb(self, cx);
        }
        obs.append(&mut self.bounds_observers);
        self.bounds_observers = obs;
    }

    pub(crate) fn fire_focus_change(
        &mut self,
        old_focus: Option<FocusId>,
        new_focus: Option<FocusId>,
        cx: &mut App,
    ) {
        let mut obs = std::mem::take(&mut self.focus_change_observers);
        for cb in &mut obs {
            cb(old_focus, new_focus, self, cx);
        }
        obs.append(&mut self.focus_change_observers);
        self.focus_change_observers = obs;
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

    /// Resets per-frame stacks and syncs client metrics; returns `false` for minimized windows.
    pub(crate) fn prepare_frame(&mut self) -> bool {
        self.dirty = false;
        self.animation_requested = false;
        let Some((pw, ph, scale)) = self.native.live_client_metrics() else {
            return false;
        };
        let _ = self.renderer.resize(pw, ph, scale);
        self.layout_engine.clear();
        self.frame = FrameState::default();
        self.next_element_states.clear();
        self.next_anim_tracks.clear();
        self.deferred_draws.clear();
        self.id_stack.clear();
        self.text_style_stack.clear();
        self.text_style_stack.push(TextStyle::default());
        self.content_mask_stack.clear();
        self.element_offset_stack.clear();
        self.element_offset_stack.push(Point::default());
        self.focus_stack.clear();
        self.key_context_stack.clear();
        true
    }

    /// Runs the 3-pass layout -> prepaint -> Direct2D paint cycle for `root_elem`.
    pub(crate) fn finish_frame(&mut self, mut root_elem: UiNode, cx: &mut App) {
        // 1. Layout
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

        self.text_system.finish_frame();
        self.element_states = std::mem::take(&mut self.next_element_states);
        self.anim_tracks = std::mem::take(&mut self.next_anim_tracks);
        self.prev_frame = std::mem::take(&mut self.frame);
        self.update_cursor_for_position();
    }

    #[cfg(test)]
    pub(crate) fn anim_track_count(&self) -> usize {
        self.anim_tracks.len()
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
}
