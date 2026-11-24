use std::time::{Duration, Instant};

use crate::files::{FileOps, PromptId};
use crate::geom::{Point, ScreenPoint};
use crate::ids::TabId;
use crate::platform::{
    TimerHandle, WindowHandle, WindowId, WindowPlacement, cursor_screen_pos,
    primary_mouse_button_down, release_mouse_capture, top_visible_window_at,
};
use crate::shell::{AppEvent, Shell};

use super::windows::Windows;

pub(super) type ClosedWindow = (WindowId, Vec<PromptId>);

const DRAG_THRESHOLD: f32 = 10.;
const DETACH_MAGNETISM: f32 = 15.;
const MOVE_POLL: Duration = Duration::from_millis(16);
const MOVE_LOOP_START_TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Clone, Copy)]
enum Phase {
    Pressed,
    Dragging,
    Entering { since: Instant },
    Moving { attaching: bool },
}

pub(crate) struct TabDrag {
    owner: WindowId,
    source: WindowId,
    tab: TabId,
    grab: Point,
    start: f32,
    press: ScreenPoint,
    phase: Phase,
    source_index: usize,
    source_placement: Option<WindowPlacement>,
    poll_timer: Option<TimerHandle>,
}

impl TabDrag {
    pub(crate) fn start(
        window_id: WindowId,
        tab: TabId,
        index: usize,
        grab: Point,
        position: Point,
        windows: &mut Windows,
    ) -> Option<Self> {
        let win = windows.get_mut(window_id)?;
        let press = win.local_to_screen(position);
        win.mark_dirty();
        Some(Self {
            owner: window_id,
            source: window_id,
            tab,
            grab,
            start: position.x,
            press,
            phase: Phase::Pressed,
            source_index: index,
            source_placement: None,
            poll_timer: None,
        })
    }

    pub(crate) fn is_dragging(&self) -> bool {
        matches!(self.phase, Phase::Dragging)
    }

    pub(crate) fn is_in_move_loop(&self) -> bool {
        matches!(self.phase, Phase::Entering { .. } | Phase::Moving { .. })
    }

    pub(crate) fn owner(&self) -> WindowId {
        self.owner
    }

    pub(crate) fn source(&self) -> WindowId {
        self.source
    }

    pub(crate) fn is_alive(&self, windows: &Windows) -> bool {
        windows
            .get(self.owner)
            .is_some_and(|w| w.tabs().index_of(self.tab).is_some())
    }

    pub(crate) fn on_size_move_changed(
        &mut self,
        window_id: WindowId,
        entering: bool,
        windows: &mut Windows,
        file_ops: &FileOps,
        shell: &Shell,
        closed: &mut Vec<ClosedWindow>,
    ) -> bool {
        if self.owner != window_id {
            return false;
        }
        if entering {
            if matches!(self.phase, Phase::Entering { .. }) {
                self.phase = Phase::Moving { attaching: false };
            }
            false
        } else if self.is_in_move_loop() {
            self.finish_move_loop(windows, file_ops, shell, closed)
        } else {
            false
        }
    }

    pub(crate) fn on_mouse_move(
        &mut self,
        pressed_left: bool,
        windows: &mut Windows,
        file_ops: &FileOps,
        shell: &Shell,
        closed: &mut Vec<ClosedWindow>,
    ) -> bool {
        if !pressed_left {
            return self.on_release(windows);
        }
        let Some(scale) = windows.get(self.owner).map(|w| w.scale_factor()) else {
            return true;
        };
        let pt = cursor_screen_pos();
        match self.phase {
            Phase::Pressed => {
                let distance = ((pt.x - self.press.x) as f32).hypot((pt.y - self.press.y) as f32);
                if distance / scale <= DRAG_THRESHOLD {
                    return false;
                }
                self.phase = Phase::Dragging;
            }
            Phase::Dragging => {}
            Phase::Entering { .. } | Phase::Moving { .. } => return false,
        }

        let (alone, in_owner_strip) = windows
            .get(self.owner)
            .map_or((true, false), |w| {
                (w.tabs().len() == 1, w.strip_hit(pt, DETACH_MAGNETISM))
            });
        if !alone && in_owner_strip {
            if let Some(win) = windows.get_mut(self.owner) {
                let x = win.screen_to_local(pt).x;
                win.drag_tab(self.tab, x, self.grab.x, self.start);
            }
        } else if !alone
            && let Some(target_id) = strip_window(windows, file_ops, pt, None)
        {
            self.attach(target_id, pt, windows, shell, closed);
        } else {
            self.detach(alone, pt, windows, shell, closed);
        }
        windows.mark_dirty(self.owner);
        false
    }

    pub(crate) fn on_release(&mut self, windows: &mut Windows) -> bool {
        if self.is_in_move_loop() {
            return false;
        }
        if self.is_dragging() {
            if let Some(win) = windows.get_mut(self.owner) {
                win.release_tab();
            }
            if self.owner != self.source
                && let Some(hwnd) = windows.hwnd(self.owner)
            {
                hwnd.set_foreground();
            }
        }
        true
    }

    pub(crate) fn on_capture_lost(&mut self, window_id: WindowId, windows: &mut Windows) -> bool {
        if self.owner == window_id && self.is_dragging() {
            self.on_release(windows)
        } else {
            false
        }
    }

    pub(crate) fn cancel(
        &mut self,
        windows: &mut Windows,
        shell: &Shell,
        closed: &mut Vec<ClosedWindow>,
    ) {
        self.poll_timer = None;
        release_mouse_capture();
        if matches!(self.phase, Phase::Pressed) {
            return;
        }
        let tab = self.tab;
        let index = self.source_index;
        if self.owner == self.source {
            if self.is_dragging() {
                if let Some(win) = windows.get_mut(self.source) {
                    win.return_tab(tab, index, shell);
                }
            } else if let Some(placement) = self.source_placement
                && let Some(hwnd) = windows.hwnd(self.source)
            {
                hwnd.restore_placement(placement);
            }
            return;
        }

        let Some(data) = windows
            .get_mut(self.owner)
            .and_then(|w| w.take_tab(tab, shell))
        else {
            return;
        };
        closed.extend(windows.sweep_closed());

        if let Some(src) = windows.get_mut(self.source) {
            let src_hwnd = src.hwnd();
            src.restore_tab(data, index, shell);
            src_hwnd.set_foreground();
        } else {
            match windows.open(Some(data), None, shell) {
                Ok((_, hwnd)) => {
                    if let Some(placement) = self.source_placement {
                        hwnd.restore_placement(placement);
                    }
                }
                Err(err) => {
                    if let Some(tab_data) = err.tab {
                        windows.rescue_tab(None, 0, tab_data, shell);
                    }
                }
            }
        }
    }

    pub(crate) fn on_poll(
        &mut self,
        windows: &mut Windows,
        file_ops: &FileOps,
        shell: &Shell,
        closed: &mut Vec<ClosedWindow>,
    ) -> bool {
        let Some(hwnd) = windows.hwnd(self.owner) else {
            return true;
        };
        let attaching = match self.phase {
            Phase::Entering { .. } => false,
            Phase::Moving { attaching } => attaching,
            Phase::Pressed | Phase::Dragging => return false,
        };
        let pt = cursor_screen_pos();
        if hwnd.in_move_loop() {
            let attaching = attaching || strip_window(windows, file_ops, pt, Some(self.owner)).is_some();
            if attaching && !matches!(self.phase, Phase::Moving { attaching: true }) {
                hwnd.cancel_mode();
            }
            self.phase = Phase::Moving { attaching };
            return false;
        }
        if let Phase::Entering { since } = self.phase
            && since.elapsed() < MOVE_LOOP_START_TIMEOUT
        {
            return false;
        }
        self.finish_move_loop(windows, file_ops, shell, closed)
    }

    fn finish_move_loop(
        &mut self,
        windows: &mut Windows,
        file_ops: &FileOps,
        shell: &Shell,
        closed: &mut Vec<ClosedWindow>,
    ) -> bool {
        self.poll_timer = None;
        let Some(hwnd) = windows.hwnd(self.owner) else {
            return true;
        };
        let attaching = matches!(self.phase, Phase::Moving { attaching: true });
        let pt = cursor_screen_pos();
        let pressed = primary_mouse_button_down();
        if let Some(target_id) = strip_window(windows, file_ops, pt, Some(self.owner)) {
            self.attach(target_id, pt, windows, shell, closed);
            if !pressed {
                return self.on_release(windows);
            }
            false
        } else if pressed && attaching {
            self.run_move_loop(pt, windows, shell);
            windows.mark_dirty(self.owner);
            false
        } else if pressed {
            self.cancel(windows, shell, closed);
            true
        } else {
            windows.mark_dirty(self.owner);
            hwnd.set_foreground();
            true
        }
    }

    fn attach(
        &mut self,
        target_id: WindowId,
        pt: ScreenPoint,
        windows: &mut Windows,
        shell: &Shell,
        closed: &mut Vec<ClosedWindow>,
    ) {
        let Some(target_hwnd) = windows.hwnd(target_id) else {
            return;
        };
        let prev_owner = self.owner;
        let Some(data) = windows
            .get_mut(prev_owner)
            .and_then(|w| w.take_tab(self.tab, shell))
        else {
            return;
        };
        closed.extend(windows.sweep_closed());

        if let Some(target) = windows.get_mut(target_id) {
            let x = target.screen_to_local(pt).x;
            target.insert_dragged_tab(data, x, self.grab.x, shell);
        }
        self.owner = target_id;
        self.phase = Phase::Dragging;
        self.poll_timer = None;
        target_hwnd.set_capture();
        windows.mark_dirty(target_id);
        windows.mark_dirty(prev_owner);
    }

    fn detach(
        &mut self,
        alone: bool,
        pt: ScreenPoint,
        windows: &mut Windows,
        shell: &Shell,
        closed: &mut Vec<ClosedWindow>,
    ) {
        if !alone {
            let source_win = self.owner;
            let taken = windows.get_mut(source_win).and_then(|w| {
                let sz = w.restored_logical_size();
                w.take_tab(self.tab, shell).map(|data| (data, sz))
            });
            let Some((data, sz)) = taken else {
                return;
            };
            closed.extend(windows.sweep_closed());
            let (window, _) = match windows.open_at(Some(data), Some(sz), Some(pt), shell) {
                Ok(created) => created,
                Err(err) => {
                    if let Some(tab_data) = err.tab {
                        windows.rescue_tab(Some(source_win), self.source_index, tab_data, shell);
                    }
                    return;
                }
            };
            windows.mark_dirty(source_win);
            self.owner = window;
        } else if let Some(win) = windows.get_mut(self.owner) {
            let hwnd = win.hwnd();
            if self.owner == self.source && self.source_placement.is_none() {
                self.source_placement = hwnd.saved_placement();
            }
            if hwnd.is_maximized() {
                hwnd.restore();
            }
            win.release_tab();
        } else {
            return;
        }
        self.run_move_loop(pt, windows, shell);
    }

    fn run_move_loop(&mut self, pt: ScreenPoint, windows: &Windows, shell: &Shell) {
        let Some(win) = windows.get(self.owner) else {
            return;
        };
        let hwnd = win.hwnd();
        place_window(hwnd, pt, win.strip_grab_offset(self.grab));
        hwnd.begin_system_move(pt);
        self.phase = Phase::Entering {
            since: Instant::now(),
        };
        self.poll_timer = Some(
            shell
                .jobs
                .every(MOVE_POLL, MOVE_POLL, || AppEvent::TabDragPoll),
        );
    }

    #[cfg(test)]
    pub(crate) fn detach_for_test(
        &mut self,
        windows: &mut Windows,
        shell: &Shell,
        closed: &mut Vec<ClosedWindow>,
    ) {
        self.phase = Phase::Dragging;
        let pt = ScreenPoint {
            x: self.press.x + 120,
            y: self.press.y + 200,
        };
        self.detach(false, pt, windows, shell, closed);
        windows.mark_dirty(self.owner);
    }
}

fn strip_window(
    windows: &Windows,
    file_ops: &FileOps,
    pt: ScreenPoint,
    exclude: Option<WindowId>,
) -> Option<WindowId> {
    let exclude_hwnd = exclude.and_then(|id| windows.hwnd(id));
    let hwnd = top_visible_window_at(pt, exclude_hwnd)?;
    windows.iter().find_map(|w| {
        let accepts = w.hwnd() == hwnd
            && w.accepts_tab_drop()
            && !file_ops.has_active_workflow_for(w.id())
            && w.strip_hit(pt, DETACH_MAGNETISM);
        accepts.then_some(w.id())
    })
}

fn place_window(hwnd: WindowHandle, pt: ScreenPoint, offset: Point) {
    let scale = hwnd.dpi_scale();
    let origin = hwnd.client_origin();
    let frame = hwnd.window_rect();
    let x = pt.x - (offset.x * scale).round() as i32 - (origin.x - frame.left);
    let y = pt.y - (offset.y * scale).round() as i32 - (origin.y - frame.top);
    hwnd.move_noactivate(x, y);
}
