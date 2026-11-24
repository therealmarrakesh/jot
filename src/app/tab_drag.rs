use std::time::{Duration, Instant};

use crate::geom::{Point, ScreenPoint, point};
use crate::ids::TabId;
use crate::platform::{
    TimerHandle, WindowHwnd, WindowId, WindowPlacement, cursor_screen_pos,
    primary_mouse_button_down, release_mouse_capture, top_visible_window_at,
};
use crate::shell::AppEvent;
use crate::tabs::{CAPTION_WIDTH, STRIP_LEFT, STRIP_TOP, TAB_HEIGHT};

use super::App;

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

pub(crate) struct TabDragState {
    owner: WindowId,
    source: WindowId,
    tab: TabId,
    grab: Point,
    start: f32,
    press: ScreenPoint,
    phase: Phase,
    source_index: usize,
    source_placement: Option<WindowPlacement>,
    poll_timer: TimerHandle,
}

impl TabDragState {
    pub(crate) fn is_dragging(&self) -> bool {
        matches!(self.phase, Phase::Dragging)
    }

    pub(crate) fn is_in_move_loop(&self) -> bool {
        matches!(self.phase, Phase::Entering { .. } | Phase::Moving { .. })
    }

    pub(crate) fn owner(&self) -> WindowId {
        self.owner
    }

    pub(crate) fn tab(&self) -> TabId {
        self.tab
    }
}

impl App {
    pub(crate) fn is_tab_dragging(&self) -> bool {
        self.tab_drag
            .as_ref()
            .is_some_and(TabDragState::is_dragging)
    }

    pub(crate) fn sync_tab_drag(&mut self) {
        if let Some(drag) = &self.tab_drag
            && self
                .window(drag.owner())
                .is_none_or(|w| w.tab_index(drag.tab()).is_none())
        {
            self.tab_drag_end();
        }
    }

    fn mark_window_dirty(&mut self, id: WindowId) {
        if let Some(w) = self.window_mut(id) {
            w.dirty = true;
        }
    }

    pub(crate) fn tab_drag_press(
        &mut self,
        window_id: WindowId,
        tab: TabId,
        index: usize,
        grab: Point,
        position: Point,
    ) {
        let Some(hwnd) = self.window_hwnd(window_id) else {
            return;
        };
        self.tab_drag = Some(TabDragState {
            owner: window_id,
            source: window_id,
            tab,
            grab,
            start: position.x,
            press: to_screen(hwnd, position),
            phase: Phase::Pressed,
            source_index: index,
            source_placement: None,
            poll_timer: TimerHandle::default(),
        });
        self.mark_window_dirty(window_id);
    }

    fn tab_drag_end(&mut self) -> Option<TabDragState> {
        let drag = self.tab_drag.take();
        if let Some(d) = &drag {
            self.mark_window_dirty(d.owner);
            if d.source != d.owner {
                self.mark_window_dirty(d.source);
            }
        }
        drag
    }

    pub(crate) fn tab_drag_size_move_changed(&mut self, window_id: WindowId, entering: bool) {
        self.sync_tab_drag();
        let Some(drag) = self.tab_drag.as_mut() else {
            return;
        };
        if drag.owner != window_id {
            return;
        }
        if entering {
            if matches!(drag.phase, Phase::Entering { .. }) {
                drag.phase = Phase::Moving { attaching: false };
            }
        } else if matches!(drag.phase, Phase::Entering { .. } | Phase::Moving { .. }) {
            self.finish_move_loop();
        }
    }

    pub(crate) fn tab_drag_moved(&mut self, pressed_left: bool) {
        self.sync_tab_drag();
        if !pressed_left {
            self.tab_drag_released();
            return;
        }
        let Some(mut drag) = self.tab_drag.take() else {
            return;
        };
        let Some(owner_hwnd) = self.window_hwnd(drag.owner) else {
            return;
        };
        let pt = cursor_screen_pos();
        match drag.phase {
            Phase::Pressed => {
                let distance = ((pt.x - drag.press.x) as f32).hypot((pt.y - drag.press.y) as f32);
                if distance / owner_hwnd.dpi_scale() <= DRAG_THRESHOLD {
                    self.tab_drag = Some(drag);
                    return;
                }
                drag.phase = Phase::Dragging;
            }
            Phase::Dragging => {}
            Phase::Entering { .. } | Phase::Moving { .. } => {
                self.tab_drag = Some(drag);
                return;
            }
        }

        let alone = self
            .window(drag.owner)
            .is_none_or(|w| w.tabs.len() == 1);
        if !alone && in_strip(owner_hwnd, pt) {
            let x = to_local(owner_hwnd, pt).x;
            let (tab, grab, start) = (drag.tab, drag.grab.x, drag.start);
            if let Some(win) = self.window_mut(drag.owner) {
                win.drag_tab(tab, x, grab, start);
            }
        } else if !alone && let Some(target_id) = self.strip_window(pt, None) {
            self.tab_drag_attach(&mut drag, target_id, pt);
        } else {
            self.tab_drag_detach(&mut drag, alone, pt);
        }
        let owner = drag.owner;
        self.tab_drag = Some(drag);
        self.mark_window_dirty(owner);
    }

    pub(crate) fn tab_drag_released(&mut self) {
        self.sync_tab_drag();
        if self
            .tab_drag
            .as_ref()
            .is_none_or(TabDragState::is_in_move_loop)
        {
            return;
        }
        let Some(drag) = self.tab_drag_end() else {
            return;
        };
        if drag.is_dragging() {
            if let Some(win) = self.window_mut(drag.owner) {
                win.release_tab();
            }
            if drag.owner != drag.source
                && let Some(hwnd) = self.window_hwnd(drag.owner)
            {
                hwnd.set_foreground();
            }
        }
    }

    pub(crate) fn tab_drag_cancel(&mut self) {
        self.sync_tab_drag();
        let Some(drag) = self.tab_drag_end() else {
            return;
        };
        release_mouse_capture();
        if matches!(drag.phase, Phase::Pressed) {
            return;
        }
        let tab = drag.tab;
        let index = drag.source_index;
        if drag.owner == drag.source {
            if drag.is_dragging() {
                if let Some(ix) = self.window_ix(drag.source) {
                    self.windows[ix].return_tab(tab, index, &self.shell.jobs);
                }
            } else if let Some(placement) = drag.source_placement
                && let Some(hwnd) = self.window_hwnd(drag.source)
            {
                hwnd.restore_placement(placement);
            }
            return;
        }

        let Some(data) = self
            .window_ix(drag.owner)
            .and_then(|ix| self.windows[ix].take_tab(tab, &self.shell.jobs))
        else {
            return;
        };
        self.sweep_closed_windows();

        if let Some(ix) = self.window_ix(drag.source) {
            let src_hwnd = self.windows[ix].native.hwnd();
            self.windows[ix].restore_tab(data, index, &self.shell.jobs);
            src_hwnd.set_foreground();
        } else {
            match self.open_window(Some(data), None) {
                Ok((_, hwnd)) => {
                    if let Some(placement) = drag.source_placement {
                        hwnd.restore_placement(placement);
                    }
                }
                Err(err) => {
                    if let Some(tab_data) = err.tab
                        && let Some(win) = self.windows.last_mut()
                    {
                        let ix = win.tabs.len();
                        win.restore_tab(tab_data, ix, &self.shell.jobs);
                    }
                }
            }
        }
    }

    fn tab_drag_attach(&mut self, drag: &mut TabDragState, target_id: WindowId, pt: ScreenPoint) {
        let Some(target_hwnd) = self.window_hwnd(target_id) else {
            return;
        };
        let tab = drag.tab;
        let prev_owner = drag.owner;
        let Some(data) = self
            .window_ix(prev_owner)
            .and_then(|ix| self.windows[ix].take_tab(tab, &self.shell.jobs))
        else {
            return;
        };
        self.sweep_closed_windows();

        let x = to_local(target_hwnd, pt).x;
        let grab = drag.grab.x;
        if let Some(ix) = self.window_ix(target_id) {
            self.windows[ix].insert_dragged_tab(data, x, grab, &self.shell.jobs);
        }
        drag.owner = target_id;
        drag.phase = Phase::Dragging;
        drag.poll_timer.cancel();
        target_hwnd.set_capture();
        self.mark_window_dirty(target_id);
        self.mark_window_dirty(prev_owner);
    }

    fn tab_drag_detach(&mut self, drag: &mut TabDragState, alone: bool, pt: ScreenPoint) {
        let tab = drag.tab;
        if !alone {
            let source_win = drag.owner;
            let taken = self.window_ix(source_win).and_then(|ix| {
                let native = &self.windows[ix].native;
                let sz = if native.is_maximized() {
                    native
                        .hwnd()
                        .restored_logical_size()
                        .unwrap_or_else(|| native.logical_size())
                } else {
                    native.logical_size()
                };
                self.windows[ix]
                    .take_tab(tab, &self.shell.jobs)
                    .map(|data| (data, sz))
            });
            let Some((data, sz)) = taken else {
                return;
            };
            self.sweep_closed_windows();
            let (window, _hwnd) = match self.open_window_at(Some(data), Some(sz), Some(pt)) {
                Ok(created) => created,
                Err(err) => {
                    if let Some(tab_data) = err.tab {
                        if let Some(ix) = self.window_ix(source_win) {
                            self.windows[ix].restore_tab(
                                tab_data,
                                drag.source_index,
                                &self.shell.jobs,
                            );
                        } else if let Some(win) = self.windows.last_mut() {
                            let ix = win.tabs.len();
                            win.restore_tab(tab_data, ix, &self.shell.jobs);
                        }
                    }
                    return;
                }
            };
            self.mark_window_dirty(source_win);
            drag.owner = window;
        } else if let Some(hwnd) = self.window_hwnd(drag.owner) {
            if drag.owner == drag.source && drag.source_placement.is_none() {
                drag.source_placement = hwnd.saved_placement();
            }
            if hwnd.is_maximized() {
                hwnd.restore();
            }
            if let Some(win) = self.window_mut(drag.owner) {
                win.release_tab();
            }
        } else {
            return;
        }
        self.run_move_loop(drag, pt);
    }

    fn run_move_loop(&self, drag: &mut TabDragState, pt: ScreenPoint) {
        let Some(hwnd) = self.window_hwnd(drag.owner) else {
            return;
        };
        place_window(
            hwnd,
            pt,
            point(STRIP_LEFT + drag.grab.x, STRIP_TOP + drag.grab.y),
        );
        self.start_move_loop(drag, pt);
    }

    fn start_move_loop(&self, drag: &mut TabDragState, pt: ScreenPoint) {
        let Some(hwnd) = self.window_hwnd(drag.owner) else {
            return;
        };
        hwnd.begin_system_move(pt);
        drag.phase = Phase::Entering {
            since: Instant::now(),
        };
        drag.poll_timer = self
            .shell
            .jobs
            .every(MOVE_POLL, MOVE_POLL, || AppEvent::TabDragPoll);
    }

    pub(crate) fn tab_drag_poll(&mut self) {
        self.sync_tab_drag();
        let Some(mut drag) = self.tab_drag.take() else {
            return;
        };
        let Some(hwnd) = self.window_hwnd(drag.owner) else {
            return;
        };
        let attaching = match drag.phase {
            Phase::Entering { .. } => false,
            Phase::Moving { attaching } => attaching,
            Phase::Pressed | Phase::Dragging => {
                self.tab_drag = Some(drag);
                return;
            }
        };
        let pt = cursor_screen_pos();
        if hwnd.in_move_loop() {
            let attaching = attaching || self.strip_window(pt, Some(drag.owner)).is_some();
            if attaching && !matches!(drag.phase, Phase::Moving { attaching: true }) {
                hwnd.cancel_mode();
            }
            drag.phase = Phase::Moving { attaching };
            self.tab_drag = Some(drag);
            return;
        }
        if let Phase::Entering { since } = drag.phase
            && since.elapsed() < MOVE_LOOP_START_TIMEOUT
        {
            self.tab_drag = Some(drag);
            return;
        }
        self.tab_drag = Some(drag);
        self.finish_move_loop();
    }

    fn finish_move_loop(&mut self) {
        let Some(mut drag) = self.tab_drag.take() else {
            return;
        };
        drag.poll_timer.cancel();
        let Some(hwnd) = self.window_hwnd(drag.owner) else {
            return;
        };
        let attaching = matches!(drag.phase, Phase::Moving { attaching: true });
        let pt = cursor_screen_pos();
        let pressed = primary_mouse_button_down();
        if let Some(target_id) = self.strip_window(pt, Some(drag.owner)) {
            self.tab_drag_attach(&mut drag, target_id, pt);
            self.tab_drag = Some(drag);
            if !pressed {
                self.tab_drag_released();
            }
        } else if pressed && attaching {
            self.run_move_loop(&mut drag, pt);
            let owner = drag.owner;
            self.tab_drag = Some(drag);
            self.mark_window_dirty(owner);
        } else if pressed {
            self.tab_drag = Some(drag);
            self.tab_drag_cancel();
        } else {
            let owner = drag.owner;
            self.mark_window_dirty(owner);
            hwnd.set_foreground();
        }
    }

    fn strip_window(&self, pt: ScreenPoint, exclude: Option<WindowId>) -> Option<WindowId> {
        let exclude_hwnd = exclude.and_then(|id| self.window_hwnd(id));
        let hwnd = top_visible_window_at(pt, exclude_hwnd)?;
        if !in_strip(hwnd, pt) {
            return None;
        }
        self.windows.iter().find_map(|w| {
            let w_hwnd = w.native.hwnd();
            let accepts_attach = !w.dialogs.is_open()
                && !self.file_ops.has_active_workflow_for(w.id())
                && !w.pending_close
                && w_hwnd.is_enabled();
            (w_hwnd == hwnd && accepts_attach).then_some(w.native.id)
        })
    }

    #[cfg(test)]
    pub(crate) fn tab_drag_detach_for_test(&mut self) {
        let Some(mut drag) = self.tab_drag.take() else {
            return;
        };
        drag.phase = Phase::Dragging;
        let pt = ScreenPoint {
            x: drag.press.x + 120,
            y: drag.press.y + 200,
        };
        self.tab_drag_detach(&mut drag, false, pt);
        let owner = drag.owner;
        self.tab_drag = Some(drag);
        self.mark_window_dirty(owner);
    }
}

fn to_local(hwnd: WindowHwnd, pt: ScreenPoint) -> Point {
    let origin = hwnd.client_origin();
    let scale = hwnd.dpi_scale();
    point(
        (pt.x - origin.x) as f32 / scale,
        (pt.y - origin.y) as f32 / scale,
    )
}

fn to_screen(hwnd: WindowHwnd, pos: Point) -> ScreenPoint {
    let origin = hwnd.client_origin();
    let scale = hwnd.dpi_scale();
    ScreenPoint {
        x: origin.x + (pos.x * scale).round() as i32,
        y: origin.y + (pos.y * scale).round() as i32,
    }
}

fn in_strip(hwnd: WindowHwnd, pt: ScreenPoint) -> bool {
    let client = hwnd.client_rect();
    let width = client.right as f32 / hwnd.dpi_scale();
    let pos = to_local(hwnd, pt);
    (STRIP_LEFT..width - CAPTION_WIDTH).contains(&pos.x)
        && (STRIP_TOP - DETACH_MAGNETISM..STRIP_TOP + TAB_HEIGHT + DETACH_MAGNETISM)
            .contains(&pos.y)
}

fn place_window(hwnd: WindowHwnd, pt: ScreenPoint, offset: Point) {
    let scale = hwnd.dpi_scale();
    let origin = hwnd.client_origin();
    let frame = hwnd.window_rect();
    let x = pt.x - (offset.x * scale).round() as i32 - (origin.x - frame.left);
    let y = pt.y - (offset.y * scale).round() as i32 - (origin.y - frame.top);
    hwnd.move_noactivate(x, y);
}
