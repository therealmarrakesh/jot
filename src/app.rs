use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

use crate::APP_NAME;
use crate::document::{self, Format};
use crate::editor::{EditorTimer, Rope};
use crate::files::{self, WorkflowGen};
use crate::geom::{Rect, ScreenPoint, Size, point, size};
use crate::gfx::{Direct2DDevices, TextSystem};
use crate::keytips::KeyTips;
use crate::platform::{
    HandoffCommand, Jobs, MouseButton, NativeWindow, Platform, PlatformEvent, PlatformHandler,
    SaveDialogResult, WindowAppearance, WindowBackdrop, WindowHwnd, WindowId, WindowKind,
    WindowOptions, WindowsDisplay, cursor_screen_pos, system_settings,
};
use crate::tab_drag::TabDragState;
use crate::tabs::{TabData, TabId};
use crate::theme::Palette;
use crate::tooltip::Tooltips;
use crate::ui::{EditorSlot, HitId};
use crate::window::{
    MainWindow, WINDOW_HEIGHT, WINDOW_MAX_SCREEN_SHARE, WINDOW_WIDTH, WindowAction,
};

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
        hit: HitId,
    },
    TooltipHideTimer,
    TabDragPoll,
    OpenDialogDone {
        window: WindowId,
        result: crate::types::Result<Option<Vec<PathBuf>>>,
    },
    FilesLoaded {
        window: WindowId,
        results: Vec<(PathBuf, String, Result<document::Loaded, document::LoadError>)>,
    },
    SaveDialogDone {
        window: WindowId,
        wf_gen: WorkflowGen,
        tab: TabId,
        format: Format,
        result: SaveDialogResult,
    },
    LossyCheckDone {
        window: WindowId,
        wf_gen: WorkflowGen,
        tab: TabId,
        snapshot: u64,
        text: Rope,
        path: PathBuf,
        format: Format,
        result: std::io::Result<bool>,
    },
    FileSaved {
        window: WindowId,
        wf_gen: WorkflowGen,
        tab: TabId,
        snapshot: u64,
        path: PathBuf,
        path_key: String,
        format: Format,
        result: std::io::Result<()>,
    },
    #[cfg(test)]
    TestCloseWindow(WindowId),
    #[cfg(test)]
    TestFlag(std::sync::Arc<std::sync::atomic::AtomicBool>),
}

pub(crate) struct OpenWindowError {
    pub(crate) tab: Option<TabData>,
    pub(crate) message: String,
}

impl std::fmt::Debug for OpenWindowError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenWindowError")
            .field("has_tab", &self.tab.is_some())
            .field("message", &self.message)
            .finish()
    }
}

pub(crate) struct App {
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
    in_flight_loads: HashSet<String>,
    cascade_step: u32,
    #[cfg(test)]
    pub(crate) headless: bool,
    quit: bool,
    // Declared last so windows, overlays, and COM objects drop before COM uninitialize.
    pub(crate) platform: Rc<Platform>,
}

impl App {
    pub(crate) fn new(platform: Rc<Platform>) -> Self {
        let jobs = Jobs::new();
        jobs.attach_platform(&platform);
        let settings = system_settings();
        let appearance = settings.appearance;
        let dark = matches!(appearance, WindowAppearance::Dark);
        let palette = if let Some(hc) = settings.high_contrast {
            Palette::from_high_contrast(hc)
        } else {
            Palette::for_appearance(dark, settings.accent(dark))
        };
        let reduce_motion = settings.reduce_motion;

        Self {
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
            in_flight_loads: HashSet::new(),
            cascade_step: 0,
            #[cfg(test)]
            headless: false,
            quit: false,
            platform,
        }
    }

    pub(crate) fn find_tab_owner(&self, tab: TabId) -> Option<(usize, usize)> {
        self.windows
            .iter()
            .enumerate()
            .find_map(|(wix, w)| w.tab_index(tab).map(|tix| (wix, tix)))
    }

    pub(crate) fn find_open_tab_by_key(&self, key: &str) -> Option<(usize, usize)> {
        self.windows
            .iter()
            .enumerate()
            .find_map(|(wix, w)| w.find_open_tab_by_key(key).map(|tix| (wix, tix)))
    }

    fn fallback_window_ix(&self, preferred: WindowId) -> Option<usize> {
        self.window_ix(preferred)
            .or_else(|| self.active_window.and_then(|id| self.window_ix(id)))
            .or_else(|| (!self.windows.is_empty()).then_some(self.windows.len() - 1))
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

    fn window_options(
        &mut self,
        window_size: Option<Size>,
        target_point: Option<ScreenPoint>,
    ) -> WindowOptions {
        let display = if let Some(pt) = target_point {
            WindowsDisplay::from_point(pt).or_else(WindowsDisplay::primary)
        } else if let Some(origin) = self
            .active_window
            .and_then(|id| self.window(id))
            .map(|w| w.native.hwnd().client_origin())
        {
            WindowsDisplay::from_point(origin).or_else(WindowsDisplay::primary)
        } else {
            #[cfg(test)]
            let fallback = if self.headless {
                WindowsDisplay::primary()
            } else {
                WindowsDisplay::from_point(cursor_screen_pos()).or_else(WindowsDisplay::primary)
            };
            #[cfg(not(test))]
            let fallback =
                WindowsDisplay::from_point(cursor_screen_pos()).or_else(WindowsDisplay::primary);
            fallback
        };
        let visible = display.map(|d| d.visible_bounds());
        let screen = visible.map_or(size(WINDOW_WIDTH, WINDOW_HEIGHT), |v| v.size);
        let target_size = window_size.unwrap_or_else(|| {
            size(
                WINDOW_WIDTH.min(screen.width * WINDOW_MAX_SCREEN_SHARE),
                WINDOW_HEIGHT.min(screen.height * WINDOW_MAX_SCREEN_SHARE),
            )
        });
        let mut bounds = display.map_or_else(
            || Rect::new(crate::geom::Point::ZERO, target_size),
            |d| d.centered_rect(target_size),
        );
        if target_point.is_none() && !self.windows.is_empty() && window_size.is_none() {
            let step = (self.cascade_step % 8) as f32 * 28.0;
            self.cascade_step = self.cascade_step.wrapping_add(1);
            if let Some(vis) = visible {
                let max_x = (vis.right() - target_size.width).max(vis.left());
                let max_y = (vis.bottom() - target_size.height).max(vis.top());
                bounds.origin = point(
                    (bounds.origin.x + step).clamp(vis.left(), max_x),
                    (bounds.origin.y + step).clamp(vis.top(), max_y),
                );
            } else {
                bounds.origin = point(bounds.origin.x + step, bounds.origin.y + step);
            }
        }
        #[cfg(test)]
        let (focus, show) = (!self.headless, !self.headless);
        #[cfg(not(test))]
        let (focus, show) = (true, true);
        WindowOptions {
            bounds: Some(bounds),
            title: APP_NAME.into(),
            focus,
            show,
            kind: WindowKind::Normal,
            is_resizable: true,
            is_minimizable: true,
            display_id: display.map(|d| d.id()),
            backdrop: WindowBackdrop::MicaAlt,
            min_size: None,
        }
    }

    pub(crate) fn open_window(
        &mut self,
        initial: Option<TabData>,
        window_size: Option<Size>,
    ) -> std::result::Result<(WindowId, WindowHwnd), Box<OpenWindowError>> {
        self.open_window_at(initial, window_size, None)
    }

    pub(crate) fn open_window_at(
        &mut self,
        initial: Option<TabData>,
        window_size: Option<Size>,
        target_point: Option<ScreenPoint>,
    ) -> std::result::Result<(WindowId, WindowHwnd), Box<OpenWindowError>> {
        let opts = self.window_options(window_size, target_point);
        let native =
            match NativeWindow::create(self.platform.devices(), &opts, self.appearance) {
                Ok(n) => n,
                Err(err) => {
                    let message = err.to_string();
                    crate::diag::debug_log(&format!("NativeWindow::create failed: {message}"));
                    return Err(Box::new(OpenWindowError {
                        tab: initial,
                        message,
                    }));
                }
            };
        let id = native.id;
        let hwnd = native.hwnd();
        let win = MainWindow::new(native, initial, self.view.word_wrap, &self.jobs);
        self.windows.push(win);
        self.active_window = Some(id);
        Ok((id, hwnd))
    }

    pub(crate) fn request_quit(&mut self) {
        if !self.quit {
            self.quit = true;
            self.platform.post_quit();
        }
    }

    pub(crate) fn open_paths_in_window(&mut self, window_id: WindowId, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        let mut to_load: Vec<(PathBuf, String)> = Vec::new();
        let mut last_existing: Option<(usize, usize)> = None;
        for raw in paths {
            let path = std::path::absolute(&raw).unwrap_or(raw);
            let key = document::quick_path_key(&path);
            if let Some(found) = self.find_open_tab_by_key(&key) {
                last_existing = Some(found);
                continue;
            }
            if self.in_flight_loads.contains(&key)
                || to_load.iter().any(|(_, queued_key)| queued_key == &key)
            {
                continue;
            }
            self.in_flight_loads.insert(key.clone());
            to_load.push((path, key));
        }
        if to_load.is_empty() {
            if let Some((wix, tix)) = last_existing {
                let hwnd = self.windows[wix].native.hwnd();
                self.windows[wix].activate_tab(tix, &self.jobs);
                hwnd.bring_to_front();
            }
            return;
        }
        files::spawn_load_files(window_id, to_load, &self.jobs);
    }

    pub(crate) fn open_handoff_paths(&mut self, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        if paths.len() == 1 {
            let key = document::quick_path_key(&paths[0]);
            if let Some((wix, tix)) = self.find_open_tab_by_key(&key) {
                let hwnd = self.windows[wix].native.hwnd();
                self.windows[wix].activate_tab(tix, &self.jobs);
                hwnd.bring_to_front();
                return;
            }
        }

        let target_id = self
            .active_window
            .filter(|&id| self.window(id).is_some())
            .or_else(|| self.windows.last().map(MainWindow::id));

        if let Some(id) = target_id {
            if let Some(ix) = self.window_ix(id) {
                let hwnd = self.windows[ix].native.hwnd();
                self.open_paths_in_window(id, paths);
                hwnd.bring_to_front();
            }
        } else if let Ok((id, _)) = self.open_window(None, None) {
            self.open_paths_in_window(id, paths);
        }
    }

    fn sync_theme(&mut self) {
        let settings = system_settings();
        self.reduce_motion = settings.reduce_motion;
        let next = self.appearance_override.unwrap_or(settings.appearance);
        let dark = matches!(next, WindowAppearance::Dark);
        self.appearance = next;
        self.palette = if let Some(hc) = settings.high_contrast {
            Palette::from_high_contrast(hc)
        } else {
            Palette::for_appearance(dark, settings.accent(dark))
        };
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
            } => {
                self.tab_drag_press(window_id, tab, index, grab, position);
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
        if self.windows.iter().any(|w| w.pending_close) {
            let closed: Vec<WindowId> = self
                .windows
                .iter()
                .filter(|w| w.pending_close)
                .map(MainWindow::id)
                .collect();
            for wid in closed {
                self.keytips.window_closed(wid);
                self.tooltips.window_closed(wid, &self.jobs);
                self.tab_drag_window_closed(wid);
            }
            self.windows.retain(|w| !w.pending_close);
            if let Some(active) = self.active_window
                && self.window(active).is_none()
            {
                self.active_window = self.windows.last().map(MainWindow::id);
            }
        }
        let dirty_any = self.windows.iter().any(MainWindow::has_dirty_tabs);
        let block = dirty_any.then_some("You have unsaved changes.");
        for w in &self.windows {
            w.native.set_block_shutdown(block);
        }
        if self.windows.is_empty() && !self.quit && crate::files::in_flight_writes() == 0 {
            self.request_quit();
        }
    }

    fn recover_device_loss(&mut self) {
        match Direct2DDevices::new() {
            Ok(new_devices) => {
                self.platform.replace_devices(new_devices.clone());
                for w in &mut self.windows {
                    if let Err(err) = w.native.recreate_renderer(new_devices.clone()) {
                        crate::diag::debug_log(&format!(
                            "recreate_renderer failed for window {:?}: {err}",
                            w.id()
                        ));
                    }
                    w.dirty = true;
                }
                self.keytips.recreate_renderer(new_devices.clone());
                self.tooltips.recreate_renderer(new_devices);
            }
            Err(err) => {
                crate::diag::debug_log(&format!("Direct2DDevices recovery failed: {err}"));
            }
        }
    }

    fn evaluate_tooltip_for(&mut self, ix: usize, pressed_any: bool) {
        let w = &self.windows[ix];
        let wid = w.id();
        let hovered = w.pointer_hovered;
        let pos = w.last_mouse_pos;
        let origin = w.native.hwnd().client_origin();
        let scale = w.native.scale_factor();
        let devices = self.platform.devices();
        self.tooltips.evaluate(
            wid,
            hovered,
            pressed_any,
            pos,
            origin,
            scale,
            &w.ui.frame,
            &devices,
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
            Err(err) => {
                crate::diag::debug_log(&format!("render_frame error: {err}"));
                (false, true)
            }
        };
        let wid = self.windows[ix].id();
        if self.windows[ix].menu_bar.access_active() || self.keytips.is_owner(wid) {
            let tips = self.windows[ix].key_tips(&self.text_system);
            let origin = self.windows[ix].native.hwnd().client_origin();
            let scale = self.windows[ix].native.scale_factor();
            let devices = self.platform.devices();
            self.keytips
                .sync(wid, origin, scale, tips, &devices, self.appearance);
        }
        if self.windows[ix].pointer_hovered {
            let pressed = self.windows[ix].pointer_pressed;
            self.evaluate_tooltip_for(ix, pressed);
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
            PlatformEvent::Moved { window_id } => {
                if let Some(ix) = self.window_ix(window_id) {
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
            PlatformEvent::PaintRequested { window_id } => {
                if let Some(ix) = self.window_ix(window_id)
                    && !self.windows[ix].has_painted
                {
                    self.windows[ix].dirty = true;
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
                page_scroll,
                modifiers,
            } => {
                if let Some(ix) = self.window_ix(window_id) {
                    self.windows[ix].on_scroll_wheel(position, delta_lines, page_scroll, modifiers);
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
            PlatformEvent::KeyUp { window_id, event } => {
                let view = self.view;
                let dragging = self.is_tab_dragging();
                let res = self.window_ix(window_id).map(|ix| {
                    self.windows[ix].on_key_up(
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
            PlatformEvent::CaptureLost { window_id } => {
                if let Some(ix) = self.window_ix(window_id) {
                    self.windows[ix].on_capture_lost();
                }
                if self
                    .tab_drag
                    .as_ref()
                    .is_some_and(|d| d.owner() == window_id && d.is_dragging())
                {
                    self.tab_drag_released();
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
                if self.window(window_id).is_some() {
                    self.open_paths_in_window(window_id, paths);
                }
            }
            PlatformEvent::Handoff(cmd) => match cmd {
                HandoffCommand::OpenPaths(paths) => self.open_handoff_paths(paths),
                HandoffCommand::NewWindow => {
                    if let Ok((_, hwnd)) = self.open_window(None, None) {
                        hwnd.bring_to_front();
                    }
                }
            },
            PlatformEvent::SizeMoveChanged {
                window_id,
                entering,
            } => {
                self.tab_drag_size_move_changed(window_id, entering);
            }
            PlatformEvent::CloseRequested { window_id } => {
                if let Some(ix) = self.window_ix(window_id) {
                    self.windows[ix].request_close_window(&self.jobs);
                }
            }
            PlatformEvent::SessionEnding { ending } => {
                if ending {
                    crate::files::wait_for_in_flight_writes(std::time::Duration::from_secs(5));
                }
            }
        }
        self.sweep_closed_windows();
        handled
    }

    fn on_wake(&mut self) {
        while let Some(ev) = self.jobs.pop() {
            match ev {
                AppEvent::EditorTimer { window, slot, kind } => {
                    let target_ix = match slot {
                        EditorSlot::Tab(raw) => self
                            .find_tab_owner(TabId(raw))
                            .map(|(wix, _)| wix)
                            .or_else(|| self.window_ix(window)),
                        _ => self.window_ix(window),
                    };
                    if let Some(ix) = target_ix {
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
                AppEvent::TooltipShowTimer { window, hit } => {
                    let state = self.window_ix(window).map(|ix| {
                        let w = &self.windows[ix];
                        (
                            w.pointer_hovered,
                            w.last_mouse_pos,
                            w.native.hwnd().client_origin(),
                            w.native.scale_factor(),
                            &w.ui.frame,
                        )
                    });
                    let devices = self.platform.devices();
                    self.tooltips.on_show_timer(
                        window,
                        hit,
                        state,
                        &devices,
                        self.appearance,
                        &self.text_system,
                        &self.jobs,
                    );
                }
                AppEvent::TooltipHideTimer => {
                    self.tooltips.on_hide_timer();
                }
                AppEvent::TabDragPoll => self.tab_drag_poll(),
                AppEvent::OpenDialogDone { window, result } => {
                    if let Some(ix) = self.window_ix(window) {
                        self.windows[ix].open_dialog_active = false;
                    }
                    let Some(target_ix) = self.fallback_window_ix(window) else {
                        continue;
                    };
                    let target_id = self.windows[target_ix].id();
                    match result {
                        Ok(Some(paths)) => self.open_paths_in_window(target_id, paths),
                        Ok(None) => {}
                        Err(err) => {
                            self.windows[target_ix].show_file_prompt(
                                files::dialog_error_prompt("Can't open file", err.to_string()),
                                &self.jobs,
                            );
                        }
                    }
                }
                AppEvent::FilesLoaded { window, results } => {
                    let wrap = self.view.word_wrap;
                    let mut forgotten = Vec::new();
                    for (path, quick_key, res) in results {
                        self.in_flight_loads.remove(&quick_key);
                        let dup = res
                            .as_ref()
                            .ok()
                            .and_then(|loaded| self.find_open_tab_by_key(&loaded.path_key))
                            .or_else(|| self.find_open_tab_by_key(&quick_key));
                        if let Some((wix, tix)) = dup {
                            let hwnd = self.windows[wix].native.hwnd();
                            self.windows[wix].activate_tab(tix, &self.jobs);
                            hwnd.bring_to_front();
                            continue;
                        }
                        if let Some(ix) = self.fallback_window_ix(window)
                            && let Some(id) =
                                self.windows[ix].adopt_loaded_file(path, res, wrap, &self.jobs)
                        {
                            forgotten.push(id);
                        }
                    }
                    for id in forgotten {
                        self.tab_drag_forget(id);
                    }
                }
                AppEvent::SaveDialogDone {
                    window,
                    wf_gen,
                    tab,
                    format,
                    result,
                } => match files::resolve_save_dialog(format, result) {
                    Ok(Some((path, next_format))) => {
                        let target_key = document::quick_path_key(&path);
                        let conflict = self
                            .find_open_tab_by_key(&target_key)
                            .is_some_and(|(wix, tix)| self.windows[wix].tabs[tix].data.id != tab);
                        if conflict {
                            let prompt_ix = self
                                .find_tab_owner(tab)
                                .map(|(wix, _)| wix)
                                .or_else(|| self.fallback_window_ix(window));
                            if let Some(ix) = prompt_ix {
                                self.windows[ix].show_file_prompt(
                                    files::already_open_save_prompt(&path),
                                    &self.jobs,
                                );
                            }
                            if let Some(ix) = self.window_ix(window) {
                                self.windows[ix].abort_file_workflow_if(wf_gen);
                            }
                        } else if let Some((owner_wix, owner_tix)) = self.find_tab_owner(tab) {
                            self.windows[owner_wix].begin_write_tab_for_workflow(
                                owner_tix,
                                window,
                                wf_gen,
                                path,
                                next_format,
                                &self.jobs,
                            );
                        } else if let Some(ix) = self.window_ix(window) {
                            self.windows[ix].abort_file_workflow_if(wf_gen);
                        }
                    }
                    Ok(None) => {
                        if let Some(ix) = self.window_ix(window) {
                            self.windows[ix].abort_file_workflow_if(wf_gen);
                        }
                    }
                    Err(msg) => {
                        let prompt_ix = self
                            .find_tab_owner(tab)
                            .map(|(wix, _)| wix)
                            .or_else(|| self.fallback_window_ix(window));
                        if let Some(ix) = prompt_ix {
                            self.windows[ix].show_file_prompt(
                                files::dialog_error_prompt("Can't save file", msg),
                                &self.jobs,
                            );
                        }
                        if let Some(ix) = self.window_ix(window) {
                            self.windows[ix].abort_file_workflow_if(wf_gen);
                        }
                    }
                },
                AppEvent::LossyCheckDone {
                    window,
                    wf_gen,
                    tab,
                    snapshot,
                    text,
                    path,
                    format,
                    result,
                } => {
                    let owner_wix = self
                        .find_tab_owner(tab)
                        .map(|(wix, _)| wix)
                        .or_else(|| self.fallback_window_ix(window));
                    match result {
                        Err(err) => {
                            if let Some((wix, _)) = self.find_tab_owner(tab) {
                                let _ = self.windows[wix].clear_tab_save_in_flight(tab);
                            }
                            if let Some(ix) = owner_wix {
                                self.windows[ix].show_file_prompt(
                                    files::save_error_prompt(&path, &err),
                                    &self.jobs,
                                );
                            }
                            if let Some(ix) = self.window_ix(window) {
                                self.windows[ix].abort_file_workflow_if(wf_gen);
                            }
                        }
                        Ok(true) => {
                            if let Some(ix) = owner_wix {
                                self.windows[ix].show_file_prompt(
                                    files::FilePrompt::ConfirmLossyAnsi {
                                        wf_gen,
                                        tab,
                                        snapshot,
                                        text,
                                        path,
                                        format,
                                    },
                                    &self.jobs,
                                );
                            } else if let Some(ix) = self.window_ix(window) {
                                self.windows[ix].abort_file_workflow_if(wf_gen);
                            }
                        }
                        Ok(false) => {
                            files::spawn_write_file(
                                window, wf_gen, tab, snapshot, text, path, format, &self.jobs,
                            );
                        }
                    }
                }
                AppEvent::FileSaved {
                    window,
                    wf_gen,
                    tab,
                    snapshot,
                    path,
                    path_key,
                    format,
                    result,
                } => match result {
                    Ok(()) => {
                        let mut retriggered_gen = None;
                        if let Some((owner_wix, _)) = self.find_tab_owner(tab) {
                            retriggered_gen = self.windows[owner_wix].apply_tab_saved(
                                tab,
                                snapshot,
                                path,
                                path_key,
                                format,
                                window,
                                &self.jobs,
                            );
                        }
                        if retriggered_gen != Some(wf_gen)
                            && let Some(origin_ix) = self.window_ix(window)
                            && let Some(id) = self.windows[origin_ix]
                                .advance_file_workflow_if(wf_gen, &self.jobs)
                        {
                            self.tab_drag_forget(id);
                        }
                    }
                    Err(err) => {
                        if let Some((owner_wix, _)) = self.find_tab_owner(tab) {
                            let _ = self.windows[owner_wix].clear_tab_save_in_flight(tab);
                        }
                        let prompt_ix = self
                            .find_tab_owner(tab)
                            .map(|(wix, _)| wix)
                            .or_else(|| self.fallback_window_ix(window));
                        if let Some(ix) = prompt_ix {
                            self.windows[ix].show_file_prompt(
                                files::save_error_prompt(&path, &err),
                                &self.jobs,
                            );
                        }
                        if let Some(origin_ix) = self.window_ix(window) {
                            self.windows[origin_ix].abort_file_workflow_if(wf_gen);
                        }
                    }
                },
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
        any_lost |= self.keytips.paint(self.palette, &self.text_system);
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
}
