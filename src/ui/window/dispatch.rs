//! Action, pointer, scroll, keyboard, IME, and file-drop dispatch for `Window`.

use std::any::TypeId;

use crate::ui::app::App;
use crate::ui::element::DispatchPhase;
use crate::ui::focus::FocusId;
use crate::ui::geometry::{Bounds, Pixels, Point, point, px};
use crate::ui::keymap::Action;
use crate::ui::platform::ExternalPaths;

use super::events::{
    KeyDownEvent, ModifiersChangedEvent, MouseDownEvent, MouseEvent, MouseMoveEvent, MouseUpEvent,
    ScrollWheelEvent,
};
use super::Window;

impl Window {
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
                modifiers: crate::ui::platform::window::current_modifiers(),
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
