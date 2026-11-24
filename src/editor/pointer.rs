use crate::geom::Point;
use crate::platform::{Modifiers, MouseButton};

use super::blink::AutoScroll;
use super::{Editor, EditorTimers};

impl Editor {
    pub(super) fn hit_test_point(&self, pos: Point) -> (usize, bool) {
        let Some(frame) = self.fresh_frame() else {
            return (0, false);
        };
        frame.hit_test(
            pos,
            self.scroll.x,
            self.scroll.y,
            self.model.text().len(),
            self.model.is_multi_line(),
        )
    }

    pub(crate) fn on_mouse_down(
        &mut self,
        button: MouseButton,
        position: Point,
        modifiers: Modifiers,
        click_count: usize,
        timers: &mut dyn EditorTimers,
    ) {
        self.model.commit_composition();
        self.model.break_coalescing();
        self.selecting = true;
        let (offset, aff) = self.hit_test_point(position);

        if button == MouseButton::Left && click_count >= 3 {
            self.select_line_at(offset);
            return;
        }
        if button == MouseButton::Left && click_count == 2 {
            self.select_word_at(offset);
            return;
        }
        if button == MouseButton::Right {
            if !self.model.selection().contains(offset) {
                self.move_to_with_affinity(offset, None, aff, false, Some(timers));
            }
            if let Some(doc) = &mut self.doc {
                doc.pending_context_menu = Some((position, offset));
            }
            return;
        }
        if modifiers.shift {
            self.select_to_with_affinity(offset, aff);
            self.request_reveal(self.model.cursor(), aff, None, false);
        } else {
            self.move_to_with_affinity(offset, None, aff, false, Some(timers));
        }
    }

    pub(crate) fn on_mouse_move(
        &mut self,
        position: Point,
        dragging_left: bool,
        focused: bool,
        timers: &mut dyn EditorTimers,
    ) -> bool {
        if dragging_left
            && self.selecting
            && focused
            && self.model.text().len() > 0
            && self.fresh_frame().is_some()
        {
            let (offset, aff) = self.hit_test_point(position);
            self.select_to_with_affinity(offset, aff);
            if let Some(doc) = &mut self.doc {
                doc.auto_scroll.last_drag_pos = Some(position);
                let delta = AutoScroll::compute_delta(position.y, self.input_bounds);
                doc.auto_scroll.update(delta.map(|d| -d), timers);
            }
            self.request_reveal(self.model.cursor(), aff, None, false);
            return true;
        }
        false
    }

    pub(crate) fn on_mouse_up(
        &mut self,
        button: MouseButton,
        timers: &mut dyn EditorTimers,
    ) -> Option<Point> {
        let mut open_context_menu = None;
        if button == MouseButton::Right
            && let Some(doc) = &mut self.doc
            && let Some((pos, offset)) = doc.pending_context_menu.take()
        {
            if !self.model.selection().contains(offset) {
                self.move_to_with_affinity(offset, None, false, false, Some(timers));
            }
            open_context_menu = Some(pos);
        }
        self.reset_pointer_selection_state();
        open_context_menu
    }
}
