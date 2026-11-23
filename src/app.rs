//! Application root owning all open windows, shared theme/view settings, typed event/timer jobs,
//! cross-window tab dragging, and the `PlatformHandler` implementation.

use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

use crate::APP_NAME;
use crate::document::{self, Format};
use crate::editor::{EditorTimer, Rope};
use crate::geom::{Rect, Size, size};
use crate::gfx::{Direct2DDevices, TextSystem};
use crate::keytips::KeyTips;
use crate::platform::{
    Jobs, MouseButton, NativeWindow, Platform, PlatformEvent, PlatformHandler, SaveDialogResult,
    WindowAppearance, WindowBackdrop, WindowHwnd, WindowId, WindowKind, WindowOptions,
    WindowsDisplay, client_area_animations_enabled, system_accent_color,
};
use crate::tab_drag::TabDragState;
use crate::tabs::{TabData, TabId};
use crate::theme::Palette;
use crate::tooltip::Tooltips;
use crate::ui::EditorSlot;
use crate::window::{
    MainWindow, WINDOW_HEIGHT, WINDOW_MAX_SCREEN_SHARE, WINDOW_WIDTH, WindowAction,
};

/// Shared app-wide view preferences mirrored across all open windows.
#[derive(Copy, Clone, Debug)]
pub(crate) struct ViewSettings {
    pub(crate) status_bar: bool,
    pub(crate) word_wrap: bool,
}

impl Default for ViewSettings {
    fn default() -> Self {
        Self {
            status_bar: true,
            word_wrap: true,
        }
    }
}

/// Typed background/timer/cross-thread events delivered to `App::on_wake`.
pub(crate) enum AppEvent {
    EditorTimer {
        window: WindowId,
        slot: EditorSlot,
        kind: EditorTimer,
    },
    TabScrollRepeat {
        window: WindowId,
        step: f32,
    },
    MenuSubmenuTimer {
        window: WindowId,
        target: Option<usize>,
    },
    FindNoticeDismiss {
        window: WindowId,
    },
    TooltipShowTimer {
        window: WindowId,
        epoch: u64,
    },
    TooltipHideTimer,
    TabDragLift,
    TabDragPoll,
    HandoffPaths(Vec<PathBuf>),
    OpenDialogDone {
        window: WindowId,
        result: crate::types::Result<Option<Vec<PathBuf>>>,
    },
    FilesLoaded {
        window: WindowId,
        results: Vec<(PathBuf, Result<document::Loaded, document::LoadError>)>,
    },
    SaveDialogDone {
        window: WindowId,
        tab: TabId,
        format: Format,
        result: SaveDialogResult,
    },
    LossyCheckDone {
        window: WindowId,
        tab: TabId,
        snapshot: u64,
        text: Rope,
        path: PathBuf,
        format: Format,
        result: std::io::Result<bool>,
    },
    FileSaved {
        window: WindowId,
        tab: TabId,
        snapshot: u64,
        path: PathBuf,
        format: Format,
        result: std::io::Result<()>,
    },
    #[cfg(test)]
    TestCloseWindow(WindowId),
    #[cfg(test)]
    TestFlag(std::sync::Arc<std::sync::atomic::AtomicBool>),
}

pub(crate) struct App {
    pub(crate) platform: Rc<Platform>,
    pub(crate) text_system: TextSystem,
    pub(crate) jobs: Jobs<AppEvent>,
    appearance_override: Option<WindowAppearance>,
    appearance: WindowAppearance,
    pub(crate) palette: Palette,
    pub(crate) reduce_motion: bool,
    pub(crate) view: ViewSettings,
    pub(crate) windows: Vec<MainWindow>,
    pub(crate) active_window: Option<WindowId>,
    pub(crate) tab_drag: Option<TabDragState>,
    pub(crate) keytips: KeyTips,
    pub(crate) tooltips: Tooltips,
    #[cfg(test)]
    pub(crate) headless: bool,
    quit: bool,
}

impl App {
    pub(crate) fn new(platform: Rc<Platform>) -> Self {
        let jobs = Jobs::new();
        jobs.set_wake_callback(platform.wake_callback());
        let appearance = platform.system_appearance();
        let dark = matches!(appearance, WindowAppearance::Dark);
        let palette = Palette::for_appearance(dark, system_accent_color(dark));
        let reduce_motion = client_area_animations_enabled().is_some_and(|enabled| !enabled);

        Self {
            platform,
            text_system: TextSystem::new(),
            jobs,
            appearance_override: None,
            appearance,
            palette,
            reduce_motion,
            view: ViewSettings::default(),
            windows: Vec::new(),
            active_window: None,
            tab_drag: None,
            keytips: KeyTips::default(),
            tooltips: Tooltips::default(),
            #[cfg(test)]
            headless: false,
            quit: false,
        }
    }

    pub(crate) fn window_ix(&self, id: WindowId) -> Option<usize> {
        self.windows.iter().position(|w| w.id() == id)
    }

    pub(crate) fn window(&self, id: WindowId) -> Option<&MainWindow> {
        self.windows.iter().find(|w| w.id() == id)
    }

    pub(crate) fn window_mut(&mut self, id: WindowId) -> Option<&mut MainWindow> {
        self.windows.iter_mut().find(|w| w.id() == id)
    }

    pub(crate) fn window_hwnd(&self, id: WindowId) -> Option<WindowHwnd> {
        self.window(id).map(|w| w.native.hwnd())
    }

    pub(crate) fn mark_all_dirty(&mut self) {
        for w in &mut self.windows {
            w.dirty = true;
        }
    }

    fn window_options(&self, window_size: Option<Size>) -> WindowOptions {
        let display = WindowsDisplay::primary();
        let screen = display
            .map_or(size(WINDOW_WIDTH, WINDOW_HEIGHT), |d| d.visible_bounds().size);
        let target_size = window_size.unwrap_or_else(|| {
            size(
                WINDOW_WIDTH.min(screen.width * WINDOW_MAX_SCREEN_SHARE),
                WINDOW_HEIGHT.min(screen.height * WINDOW_MAX_SCREEN_SHARE),
            )
        });
        let bounds = display.map_or_else(
            || Rect::new(crate::geom::Point::ZERO, target_size),
            |d| d.centered_rect(target_size),
        );
        #[cfg(test)]
        let (focus, show) = (!self.headless, !self.headless);
        #[cfg(not(test))]
        let (focus, show) = (true, true);
        WindowOptions {
            bounds: Some(bounds),
            title: APP_NAME.into(),
            hide_titlebar: true,
            focus,
            show,
            kind: WindowKind::Normal,
            is_resizable: true,
            is_minimizable: true,
            display_id: None,
            backdrop: WindowBackdrop::MicaAlt,
            min_size: None,
        }
    }

    pub(crate) fn open_window(
        &mut self,
        initial: Option<TabData>,
        window_size: Option<Size>,
    ) -> Option<(WindowId, WindowHwnd)> {
        let opts = self.window_options(window_size);
        let native =
            NativeWindow::create(self.platform.devices.clone(), &opts, self.appearance).ok()?;
        let id = native.id;
        let hwnd = native.hwnd();
        let win = MainWindow::new(native, initial, self.view.word_wrap, &self.jobs);
        self.windows.push(win);
        self.active_window = Some(id);
        Some((id, hwnd))
    }

    pub(crate) fn open_handoff_paths(&mut self, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        if paths.len() == 1 {
            let target = std::path::absolute(&paths[0]).unwrap_or_else(|_| paths[0].clone());
            for ix in 0..self.windows.len() {
                if let Some(tab_ix) = self.windows[ix].find_open_tab(&target) {
                    let hwnd = self.windows[ix].native.hwnd();
                    self.windows[ix].activate_tab(tab_ix, &self.jobs);
                    hwnd.bring_to_front();
                    return;
                }
            }
        }

        let target_id = self
            .active_window
            .filter(|&id| self.window(id).is_some())
            .or_else(|| self.windows.last().map(MainWindow::id));

        if let Some(id) = target_id {
            if let Some(ix) = self.window_ix(id) {
                let hwnd = self.windows[ix].native.hwnd();
                self.windows[ix].open_paths(paths, &self.jobs);
                hwnd.bring_to_front();
            }
        } else if let Some((id, _)) = self.open_window(None, None)
            && let Some(ix) = self.window_ix(id)
        {
            self.windows[ix].open_paths(paths, &self.jobs);
        }
    }

    fn sync_theme(&mut self) {
        self.reduce_motion = client_area_animations_enabled().is_some_and(|enabled| !enabled);
        let next = self
            .appearance_override
            .unwrap_or_else(|| self.platform.system_appearance());
        let dark = matches!(next, WindowAppearance::Dark);
        self.appearance = next;
        self.palette = Palette::for_appearance(dark, system_accent_color(dark));
        for w in &mut self.windows {
            w.native.set_appearance(next);
            w.dirty = true;
        }
    }

    fn handle_window_action(&mut self, window_id: WindowId, action: WindowAction) {
        match action {
            WindowAction::NewWindow => {
                let _ = self.open_window(None, None);
            }
            WindowAction::Exit => {
                for ix in 0..self.windows.len() {
                    self.windows[ix].request_close_window(&self.jobs);
                }
                self.sweep_closed_windows();
            }
            WindowAction::ToggleStatusBar => {
                self.view.status_bar = !self.view.status_bar;
                self.mark_all_dirty();
            }
            WindowAction::ToggleTheme => {
                let next = if self.palette.dark {
                    WindowAppearance::Light
                } else {
                    WindowAppearance::Dark
                };
                self.appearance_override = Some(next);
                self.sync_theme();
            }
            WindowAction::ToggleWordWrap => {
                self.view.word_wrap = !self.view.word_wrap;
                let wrap = self.view.word_wrap;
                for w in &mut self.windows {
                    w.set_word_wrap(wrap, &self.text_system);
                }
            }
            WindowAction::StartTabDrag {
                tab,
                index,
                grab,
                position,
                alone,
            } => {
                self.tab_drag_press(window_id, tab, index, grab, position, alone);
            }
            WindowAction::CancelTabDrag => {
                self.tab_drag_cancel();
            }
            WindowAction::ForgetTabDrag(tab) => {
                self.tab_drag_forget(tab);
            }
        }
    }

    pub(crate) fn sweep_closed_windows(&mut self) {
        if !self.windows.iter().any(|w| w.pending_close) {
            return;
        }
        for w in &self.windows {
            if w.pending_close {
                self.keytips.window_closed(w.id());
                self.tooltips.window_closed(w.id(), &self.jobs);
            }
        }
        self.windows.retain(|w| !w.pending_close);
        if let Some(active) = self.active_window
            && self.window(active).is_none()
        {
            self.active_window = self.windows.last().map(MainWindow::id);
        }
        if self.windows.is_empty() && !self.quit {
            self.quit = true;
            self.platform.post_quit();
        }
    }

    fn recover_device_loss(&mut self) {
        if let Ok(new_devices) = Direct2DDevices::new() {
            for w in &mut self.windows {
                let _ = w.native.recreate_renderer(new_devices.clone());
                w.dirty = true;
            }
            self.keytips.recreate_renderer(new_devices.clone());
            self.tooltips.recreate_renderer(new_devices);
        }
    }

    fn evaluate_tooltip_for(&mut self, ix: usize, pressed_any: bool) {
        let w = &self.windows[ix];
        let wid = w.id();
        let hovered = w.pointer_hovered;
        let pos = w.last_mouse_pos;
        let origin = w.native.hwnd().client_origin();
        let scale = w.native.scale_factor;
        self.tooltips.evaluate(
            wid,
            hovered,
            pressed_any,
            pos,
            origin,
            scale,
            &w.ui.frame,
            &self.platform.devices,
            self.appearance,
            &self.text_system,
            &self.jobs,
        );
    }

    fn paint_window_ix(&mut self, ix: usize, now: Instant) -> (bool, bool) {
        let p = self.palette;
        let view = self.view;
        let reduce_motion = self.reduce_motion;
        let res = match self.windows[ix].render_frame(
            p,
            view,
            reduce_motion,
            now,
            &self.text_system,
            &self.jobs,
        ) {
            Ok(res) => (res.needs_vsync, res.device_lost),
            Err(_) => (false, false),
        };
        let wid = self.windows[ix].id();
        if self.windows[ix].menu_bar.access_active() || self.keytips.is_owner(wid) {
            let tips = self.windows[ix].key_tips(&self.text_system);
            let origin = self.windows[ix].native.hwnd().client_origin();
            let scale = self.windows[ix].native.scale_factor;
            self.keytips.sync(
                wid,
                origin,
                scale,
                tips,
                &self.platform.devices,
                self.appearance,
            );
        }
        if self.windows[ix].pointer_hovered {
            self.evaluate_tooltip_for(ix, false);
        }
        res
    }
}

impl PlatformHandler for App {
    fn on_event(&mut self, event: PlatformEvent) -> bool {
        let mut handled = false;
        match event {
            PlatformEvent::Resized {
                window_id,
                physical_width,
                physical_height,
                scale,
            }
            | PlatformEvent::DpiChanged {
                window_id,
                physical_width,
                physical_height,
                scale,
            } => {
                if let Some(ix) = self.window_ix(window_id) {
                    self.windows[ix]
                        .native
                        .sync_size(physical_width, physical_height, scale);
                    self.windows[ix].cancel_menu(&self.jobs);
                    self.windows[ix].dirty = true;
                }
                self.keytips.hide_owner(window_id);
            }
            PlatformEvent::Moved { window_id, origin } => {
                if let Some(ix) = self.window_ix(window_id)
                    && self.windows[ix].native.origin != origin
                {
                    self.windows[ix].native.origin = origin;
                    self.windows[ix].cancel_menu(&self.jobs);
                    self.keytips.hide_owner(window_id);
                }
            }
            PlatformEvent::Activated { window_id, active } => {
                if active {
                    self.active_window = Some(window_id);
                } else {
                    self.keytips.hide_owner(window_id);
                    self.tooltips.window_deactivated(window_id, &self.jobs);
                }
                if let Some(ix) = self.window_ix(window_id) {
                    self.windows[ix].on_activated(active, &self.jobs);
                }
            }
            PlatformEvent::ThemeChanged => {
                self.sync_theme();
            }
            PlatformEvent::MouseMove {
                window_id,
                position,
                pressed_button,
            } => {
                if self.tab_drag.is_some() {
                    self.tab_drag_moved(pressed_button == Some(MouseButton::Left));
                }
                let view = self.view;
                if let Some(ix) = self.window_ix(window_id) {
                    self.windows[ix].on_mouse_move(
                        position,
                        pressed_button,
                        view,
                        &self.text_system,
                        &self.jobs,
                    );
                    self.evaluate_tooltip_for(ix, pressed_button.is_some());
                }
            }
            PlatformEvent::MouseLeave { window_id } => {
                if let Some(ix) = self.window_ix(window_id) {
                    self.windows[ix].on_mouse_leave();
                }
                self.tooltips.on_mouse_leave(window_id, &self.jobs);
            }
            PlatformEvent::MouseDown {
                window_id,
                button,
                position,
                modifiers,
                click_count,
            } => {
                self.tooltips.on_mouse_down(window_id, &self.jobs);
                let view = self.view;
                let action = self.window_ix(window_id).and_then(|ix| {
                    self.windows[ix].on_mouse_down(
                        button,
                        position,
                        modifiers,
                        click_count,
                        view,
                        &self.text_system,
                        &self.jobs,
                    )
                });
                if let Some(action) = action {
                    self.handle_window_action(window_id, action);
                }
            }
            PlatformEvent::MouseUp {
                window_id,
                button,
                position,
            } => {
                self.tooltips.on_mouse_up();
                if button == MouseButton::Left && self.tab_drag.is_some() {
                    self.tab_drag_released();
                }
                let view = self.view;
                let action = self.window_ix(window_id).and_then(|ix| {
                    self.windows[ix].on_mouse_up(
                        button,
                        position,
                        view,
                        &self.text_system,
                        &self.jobs,
                    )
                });
                if let Some(action) = action {
                    self.handle_window_action(window_id, action);
                }
            }
            PlatformEvent::ScrollWheel {
                window_id,
                position,
                delta_lines,
                modifiers,
            } => {
                if let Some(ix) = self.window_ix(window_id) {
                    self.windows[ix].on_scroll_wheel(position, delta_lines, modifiers);
                }
            }
            PlatformEvent::KeyDown { window_id, event } => {
                self.tooltips.on_key_down();
                let view = self.view;
                let dragging = self.is_tab_dragging();
                let res = self.window_ix(window_id).map(|ix| {
                    self.windows[ix].on_key_down(
                        event,
                        view,
                        dragging,
                        &self.text_system,
                        &self.jobs,
                    )
                });
                if let Some((was_handled, action)) = res {
                    handled = was_handled;
                    if let Some(action) = action {
                        self.handle_window_action(window_id, action);
                    }
                }
            }
            PlatformEvent::ModifiersChanged { modifiers } => {
                self.tooltips.on_modifiers_changed(modifiers, &self.jobs);
            }
            PlatformEvent::TextInput { window_id, text } => {
                if let Some(ix) = self.window_ix(window_id) {
                    self.windows[ix].on_text_input(&text, &self.text_system, &self.jobs);
                }
            }
            PlatformEvent::ImeComposition {
                window_id,
                text,
                cursor,
            } => {
                if let Some(ix) = self.window_ix(window_id) {
                    self.windows[ix].on_ime_composition(&text, cursor, &self.text_system);
                }
            }
            PlatformEvent::FilesDropped { window_id, paths } => {
                if let Some(ix) = self.window_ix(window_id) {
                    self.windows[ix].open_paths(paths, &self.jobs);
                }
            }
            PlatformEvent::SizeMoveChanged {
                window_id,
                entering,
            } => {
                if let Some(ix) = self.window_ix(window_id) {
                    self.windows[ix].native.in_size_move = entering;
                }
            }
            PlatformEvent::CloseRequested { window_id } => {
                if let Some(ix) = self.window_ix(window_id) {
                    self.windows[ix].request_close_window(&self.jobs);
                }
            }
        }
        self.sweep_closed_windows();
        handled
    }

    fn query_ime_caret(&self, window_id: WindowId) -> Option<(Rect, f32)> {
        self.window(window_id)?.ime_caret_bounds()
    }

    fn on_wake(&mut self) {
        while let Some(ev) = self.jobs.pop() {
            match ev {
                AppEvent::EditorTimer { window, slot, kind } => {
                    if let Some(ix) = self.window_ix(window) {
                        self.windows[ix].on_editor_timer(slot, kind, &self.jobs);
                    }
                }
                AppEvent::TabScrollRepeat { window, step } => {
                    if let Some(ix) = self.window_ix(window) {
                        self.windows[ix].on_tab_scroll_repeat(step);
                    }
                }
                AppEvent::MenuSubmenuTimer { window, target } => {
                    if let Some(ix) = self.window_ix(window) {
                        self.windows[ix].on_menu_submenu_timer(target, &self.text_system);
                    }
                }
                AppEvent::FindNoticeDismiss { window } => {
                    if let Some(ix) = self.window_ix(window) {
                        self.windows[ix].on_find_notice_dismiss();
                    }
                }
                AppEvent::TooltipShowTimer { window, epoch } => {
                    let state = self.window_ix(window).map(|ix| {
                        let w = &self.windows[ix];
                        (
                            w.pointer_hovered,
                            w.last_mouse_pos,
                            w.native.hwnd().client_origin(),
                            w.native.scale_factor,
                            &w.ui.frame,
                        )
                    });
                    self.tooltips.on_show_timer(
                        window,
                        epoch,
                        state,
                        &self.platform.devices,
                        self.appearance,
                        &self.text_system,
                        &self.jobs,
                    );
                }
                AppEvent::TooltipHideTimer => {
                    self.tooltips.on_hide_timer();
                }
                AppEvent::TabDragLift => self.tab_drag_lift(),
                AppEvent::TabDragPoll => self.tab_drag_poll(),
                AppEvent::HandoffPaths(paths) => self.open_handoff_paths(paths),
                AppEvent::OpenDialogDone { window, result } => {
                    if let Ok(Some(paths)) = result
                        && let Some(ix) = self.window_ix(window)
                    {
                        self.windows[ix].open_paths(paths, &self.jobs);
                    }
                }
                AppEvent::FilesLoaded { window, results } => {
                    let wrap = self.view.word_wrap;
                    let forgotten = self
                        .window_ix(window)
                        .map(|ix| self.windows[ix].on_files_loaded(results, wrap, &self.jobs))
                        .unwrap_or_default();
                    for id in forgotten {
                        self.tab_drag_forget(id);
                    }
                }
                AppEvent::SaveDialogDone {
                    window,
                    tab,
                    format,
                    result,
                } => {
                    if let Some(ix) = self.window_ix(window) {
                        self.windows[ix].on_save_dialog_done(tab, format, result, &self.jobs);
                    }
                }
                AppEvent::LossyCheckDone {
                    window,
                    tab,
                    snapshot,
                    text,
                    path,
                    format,
                    result,
                } => {
                    if let Some(ix) = self.window_ix(window) {
                        self.windows[ix].on_lossy_check_done(
                            tab, snapshot, text, path, format, result, &self.jobs,
                        );
                    }
                }
                AppEvent::FileSaved {
                    window,
                    tab,
                    snapshot,
                    path,
                    format,
                    result,
                } => {
                    let forgotten = self.window_ix(window).and_then(|ix| {
                        self.windows[ix]
                            .on_file_saved(tab, snapshot, path, format, result, &self.jobs)
                    });
                    if let Some(id) = forgotten {
                        self.tab_drag_forget(id);
                    }
                }
                #[cfg(test)]
                AppEvent::TestCloseWindow(id) => {
                    if let Some(ix) = self.window_ix(id) {
                        self.windows[ix].pending_close = true;
                    }
                }
                #[cfg(test)]
                AppEvent::TestFlag(flag) => {
                    flag.store(true, std::sync::atomic::Ordering::SeqCst);
                }
            }
            self.sweep_closed_windows();
        }
    }

    fn on_paint(&mut self, window_id: WindowId) {
        if let Some(ix) = self.windows.iter().position(|w| w.id() == window_id) {
            let (needs_vsync, lost) = self.paint_window_ix(ix, Instant::now());
            if lost {
                self.recover_device_loss();
            }
            if needs_vsync {
                self.platform.request_frame_tick();
            }
        }
    }

    fn drive_frames(&mut self) {
        self.sweep_closed_windows();
        if self.quit {
            return;
        }
        let now = Instant::now();
        let mut any_vsync = false;
        let mut any_lost = false;
        let mut painted_any = false;
        for ix in 0..self.windows.len() {
            if self.windows[ix].dirty {
                painted_any = true;
                let (vsync, lost) = self.paint_window_ix(ix, now);
                any_vsync |= vsync;
                any_lost |= lost;
            }
        }
        if painted_any {
            self.text_system.finish_frame();
        }
        any_lost |= self.keytips.paint(&self.text_system);
        let (tt_vsync, tt_lost) =
            self.tooltips
                .paint(self.palette, self.reduce_motion, now, &self.text_system);
        any_vsync |= tt_vsync;
        any_lost |= tt_lost;
        if any_lost {
            self.recover_device_loss();
        }
        if any_vsync {
            self.platform.request_frame_tick();
        }
    }

    fn should_quit(&self) -> bool {
        self.quit
    }
}
