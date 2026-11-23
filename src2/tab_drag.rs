//! Cross-window tab drag, reorder, detach into new window, attach into another window's tab
//! strip, native Win32 modal move-loop polling, and Escape cancellation.

use std::time::{Duration, Instant};

use crate::app::{App, AppEvent};
use crate::geom::{Point, ScreenPoint, point};
use crate::platform::{
    TimerHandle, WindowHwnd, WindowId, WindowPlacement, cursor_screen_pos,
    primary_mouse_button_down, release_mouse_capture, top_visible_window_at,
};
use crate::tabs::{CAPTION_WIDTH, STRIP_LEFT, STRIP_TOP, TAB_HEIGHT, TabId};

const DRAG_THRESHOLD: f32 = 10.;
const DETACH_MAGNETISM: f32 = 15.;
const MOVE_POLL: Duration = Duration::from_millis(10);
const MOVE_LOOP_START_TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Clone, Copy)]
pub(crate) struct Owner {
    pub(crate) window: WindowId,
    pub(crate) hwnd: WindowHwnd,
}

#[derive(Clone, Copy)]
enum Phase {
    Pressed,
    Dragging,
    Entering { since: Instant },
    Moving { attaching: bool },
}

pub(crate) struct TabDragState {
    owner: Owner,
    source: Owner,
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
}

impl App {
    pub(crate) fn is_tab_dragging(&self) -> bool {
        self.tab_drag
            .as_ref()
            .is_some_and(TabDragState::is_dragging)
    }

    pub(crate) fn tab_drag_press(
        &mut self,
        window_id: WindowId,
        tab: TabId,
        index: usize,
        grab: Point,
        position: Point,
        alone: bool,
    ) {
        let Some(hwnd) = self.window_hwnd(window_id) else {
            return;
        };
        let owner = Owner {
            window: window_id,
            hwnd,
        };
        if alone && !hwnd.is_maximized() {
            self.jobs.poster().post(AppEvent::TabDragLift);
        }
        self.tab_drag = Some(TabDragState {
            owner,
            source: owner,
            tab,
            grab,
            start: position.x,
            press: to_screen(hwnd, position),
            phase: Phase::Pressed,
            source_index: index,
            source_placement: None,
            poll_timer: TimerHandle::default(),
        });
        self.mark_all_dirty();
    }

    pub(crate) fn tab_drag_forget(&mut self, tab: TabId) {
        if self.tab_drag.as_ref().is_some_and(|d| d.tab == tab) {
            self.tab_drag_end();
        }
    }

    fn tab_drag_end(&mut self) -> Option<TabDragState> {
        let drag = self.tab_drag.take();
        self.mark_all_dirty();
        drag
    }

    pub(crate) fn tab_drag_lift(&mut self) {
        let Some(mut drag) = self.tab_drag.take() else {
            return;
        };
        let pt = drag.press;
        drag.source_placement = drag.owner.hwnd.saved_placement();
        self.start_move_loop(&mut drag, pt);
        self.tab_drag = Some(drag);
        self.mark_all_dirty();
    }

    pub(crate) fn tab_drag_moved(&mut self, pressed_left: bool) {
        if !pressed_left {
            self.tab_drag_released();
            return;
        }
        let Some(mut drag) = self.tab_drag.take() else {
            return;
        };
        let pt = cursor_screen_pos();
        match drag.phase {
            Phase::Pressed => {
                let distance = ((pt.x - drag.press.x) as f32).hypot((pt.y - drag.press.y) as f32);
                if distance / drag.owner.hwnd.dpi_scale() <= DRAG_THRESHOLD {
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
            .window(drag.owner.window)
            .is_none_or(|w| w.tabs.len() == 1);
        if !alone && in_strip(drag.owner.hwnd, pt) {
            let x = to_local(drag.owner.hwnd, pt).x;
            let (tab, grab, start) = (drag.tab, drag.grab.x, drag.start);
            if let Some(win) = self.window_mut(drag.owner.window) {
                win.drag_tab(tab, x, grab, start);
            }
        } else if !alone && let Some(target) = self.strip_window(pt, None) {
            self.tab_drag_attach(&mut drag, target, pt);
        } else {
            self.tab_drag_detach(&mut drag, alone, pt);
        }
        self.tab_drag = Some(drag);
        self.mark_all_dirty();
    }

    pub(crate) fn tab_drag_released(&mut self) {
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
            if let Some(win) = self.window_mut(drag.owner.window) {
                win.release_tab();
            }
            if drag.owner.hwnd != drag.source.hwnd {
                drag.owner.hwnd.set_foreground();
            }
        }
    }

    pub(crate) fn tab_drag_cancel(&mut self) {
        let Some(drag) = self.tab_drag_end() else {
            return;
        };
        release_mouse_capture();
        if matches!(drag.phase, Phase::Pressed) {
            return;
        }
        let tab = drag.tab;
        let index = drag.source_index;
        if drag.owner.hwnd == drag.source.hwnd {
            if drag.is_dragging() {
                if let Some(ix) = self.window_ix(drag.source.window) {
                    self.windows[ix].return_tab(tab, index, &self.jobs);
                }
            } else if let Some(placement) = drag.source_placement {
                drag.source.hwnd.restore_placement(placement);
            }
            return;
        }

        let Some(data) = self
            .window_ix(drag.owner.window)
            .and_then(|ix| self.windows[ix].take_tab(tab, &self.jobs))
        else {
            return;
        };
        self.sweep_closed_windows();

        if let Some(ix) = self.window_ix(drag.source.window) {
            self.windows[ix].restore_tab(data, index, &self.jobs);
            drag.source.hwnd.set_foreground();
        } else if let Some((_, hwnd)) = self.open_window(Some(data), None)
            && let Some(placement) = drag.source_placement
        {
            hwnd.restore_placement(placement);
        }
    }

    fn tab_drag_attach(&mut self, drag: &mut TabDragState, target: Owner, pt: ScreenPoint) {
        let tab = drag.tab;
        let Some(data) = self
            .window_ix(drag.owner.window)
            .and_then(|ix| self.windows[ix].take_tab(tab, &self.jobs))
        else {
            return;
        };
        self.sweep_closed_windows();

        let x = to_local(target.hwnd, pt).x;
        let grab = drag.grab.x;
        if let Some(ix) = self.window_ix(target.window) {
            self.windows[ix].insert_dragged_tab(data, x, grab, &self.jobs);
        }
        target.hwnd.set_capture();
        drag.owner = target;
        drag.phase = Phase::Dragging;
        drag.poll_timer.cancel();
    }

    fn tab_drag_detach(&mut self, drag: &mut TabDragState, alone: bool, pt: ScreenPoint) {
        let tab = drag.tab;
        if !alone {
            let taken = self.window_ix(drag.owner.window).and_then(|ix| {
                let sz = self.windows[ix].native.logical_size;
                self.windows[ix]
                    .take_tab(tab, &self.jobs)
                    .map(|data| (data, sz))
            });
            let Some((data, sz)) = taken else {
                return;
            };
            self.sweep_closed_windows();
            let Some((window, hwnd)) = self.open_window(Some(data), Some(sz)) else {
                return;
            };
            drag.owner = Owner { window, hwnd };
        } else {
            let hwnd = drag.owner.hwnd;
            if hwnd == drag.source.hwnd && drag.source_placement.is_none() {
                drag.source_placement = hwnd.saved_placement();
            }
            if hwnd.is_maximized() {
                hwnd.restore();
            }
            if let Some(win) = self.window_mut(drag.owner.window) {
                win.release_tab();
            }
        }
        self.run_move_loop(drag, pt);
    }

    fn run_move_loop(&self, drag: &mut TabDragState, pt: ScreenPoint) {
        place_window(
            drag.owner.hwnd,
            pt,
            point(STRIP_LEFT + drag.grab.x, STRIP_TOP + drag.grab.y),
        );
        self.start_move_loop(drag, pt);
    }

    fn start_move_loop(&self, drag: &mut TabDragState, pt: ScreenPoint) {
        drag.owner.hwnd.begin_system_move(pt);
        drag.phase = Phase::Entering {
            since: Instant::now(),
        };
        drag.poll_timer = self.jobs.every(MOVE_POLL, MOVE_POLL, || AppEvent::TabDragPoll);
    }

    pub(crate) fn tab_drag_poll(&mut self) {
        let Some(mut drag) = self.tab_drag.take() else {
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
        let hwnd = drag.owner.hwnd;
        let pt = cursor_screen_pos();
        if hwnd.in_move_loop() {
            let attaching = attaching || self.strip_window(pt, Some(hwnd)).is_some();
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

        drag.poll_timer.cancel();
        let pressed = primary_mouse_button_down();
        if let Some(target) = self.strip_window(pt, Some(hwnd)) {
            self.tab_drag_attach(&mut drag, target, pt);
            self.tab_drag = Some(drag);
            self.mark_all_dirty();
            if !pressed {
                self.tab_drag_released();
            }
        } else if pressed && attaching {
            self.run_move_loop(&mut drag, pt);
            self.tab_drag = Some(drag);
            self.mark_all_dirty();
        } else if pressed {
            self.tab_drag = Some(drag);
            self.tab_drag_cancel();
        } else {
            self.mark_all_dirty();
            hwnd.set_foreground();
        }
    }

    fn strip_window(&self, pt: ScreenPoint, exclude: Option<WindowHwnd>) -> Option<Owner> {
        let hwnd = top_visible_window_at(pt, exclude)?;
        if !in_strip(hwnd, pt) {
            return None;
        }
        self.windows.iter().find_map(|w| {
            let w_hwnd = w.native.hwnd();
            (w_hwnd == hwnd).then_some(Owner {
                window: w.native.id,
                hwnd: w_hwnd,
            })
        })
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
