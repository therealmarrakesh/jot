mod tab_drag;

use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::APP_NAME;
use crate::document::{self, DocPath, Format, PathKey};
use crate::editor::Rope;
use crate::files::{
    DocHost, FileIo, FileOps, FileRequest, Prompt, TabSnapshot, spawn_file_io,
};
use crate::geom::{Point, Rect, ScreenPoint, Size, point, size};
use crate::gfx::Gpu;
use crate::ids::{EditorSlot, TabId};
use crate::keytips::KeyTips;
use crate::platform::{
    HandoffCommand, Key, MainWindowConfig, MouseButton, NativeWindow, Platform, PlatformEvent,
    PlatformHandler, WindowHwnd, WindowId, WindowsDisplay, cursor_screen_pos, diag,
};
use crate::shell::{AppEvent, Cx, Effect, Shell};
use crate::tabs::TabData;
use crate::tooltip::{TooltipTarget, Tooltips};
use crate::window::{InputMode, MainWindow, WINDOW_HEIGHT, WINDOW_MAX_SCREEN_SHARE, WINDOW_WIDTH};

use tab_drag::TabDragState;

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

struct AppDocHost<'a> {
    windows: &'a mut Vec<MainWindow>,
    active_window: Option<WindowId>,
    shell: &'a Shell,
}

impl AppDocHost<'_> {
    fn window_ix(&self, id: WindowId) -> Option<usize> {
        self.windows.iter().position(|w| w.id() == id)
    }

    fn find_tab(&self, tab: TabId) -> Option<(usize, usize)> {
        self.windows
            .iter()
            .enumerate()
            .find_map(|(wix, w)| w.tab_index(tab).map(|tix| (wix, tix)))
    }
}

impl DocHost for AppDocHost<'_> {
    fn tab_info(&self, tab: TabId) -> Option<TabSnapshot> {
        let (wix, tix) = self.find_tab(tab)?;
        let w = &self.windows[wix];
        let data = &w.tabs[tix].data;
        Some(TabSnapshot {
            window: w.id(),
            name: w.tab_name(tix),
            path: data.path().map(PathBuf::from),
            format: data.format,
            dirty: data.is_dirty(),
            revision: data.editor.revision(),
        })
    }

    fn window_dirty_tabs(&self, window: WindowId) -> Vec<TabId> {
        self.window_ix(window)
            .map(|ix| self.windows[ix].dirty_tabs())
            .unwrap_or_default()
    }

    fn active_tab_path(&self, window: WindowId) -> Option<PathBuf> {
        let ix = self.window_ix(window)?;
        self.windows[ix].tabs.active().path().map(PathBuf::from)
    }

    fn fallback_window(&self, preferred: WindowId) -> Option<WindowId> {
        self.window_ix(preferred)
            .or_else(|| self.active_window.and_then(|id| self.window_ix(id)))
            .or_else(|| (!self.windows.is_empty()).then_some(self.windows.len() - 1))
            .map(|ix| self.windows[ix].id())
    }

    fn find_tab_by_key(&self, key: &PathKey) -> Option<(WindowId, TabId)> {
        self.windows.iter().find_map(|w| {
            let tix = w.find_open_tab_by_key(key)?;
            Some((w.id(), w.tabs[tix].data.id))
        })
    }

    fn activate_tab(&mut self, window: WindowId, tab: TabId, bring_to_front: bool) {
        let Some(wix) = self.window_ix(window) else {
            return;
        };
        let Some(tix) = self.windows[wix].tab_index(tab) else {
            return;
        };
        let hwnd = self.windows[wix].native.hwnd();
        self.windows[wix].activate_tab(tix, &self.shell.jobs);
        if bring_to_front {
            hwnd.bring_to_front();
        }
    }

    fn begin_save(&mut self, tab: TabId) -> Option<(u64, Rope)> {
        let (wix, _) = self.find_tab(tab)?;
        let ed = self.windows[wix].tabs.editor_mut(tab)?;
        let snapshot = ed.begin_save();
        let text = ed.text().clone();
        Some((snapshot, text))
    }

    fn mark_saved(&mut self, tab: TabId, snapshot: u64, doc_path: DocPath, format: Format) {
        if let Some((wix, _)) = self.find_tab(tab) {
            self.windows[wix].mark_tab_saved(tab, snapshot, doc_path, format);
        }
    }

    fn adopt_loaded(&mut self, window: WindowId, loaded: document::Loaded) {
        if let Some(wix) = self.window_ix(window) {
            self.windows[wix].adopt_loaded_file(
                loaded,
                self.shell.view.word_wrap,
                &self.shell.jobs,
            );
        }
    }

    fn remove_tab(&mut self, tab: TabId) {
        if let Some((wix, tix)) = self.find_tab(tab) {
            self.windows[wix].remove_tab(tix, &self.shell.jobs);
        }
    }

    fn mark_window_close(&mut self, window: WindowId) {
        if let Some(wix) = self.window_ix(window) {
            self.windows[wix].pending_close = true;
        }
    }

    fn show_prompt(&mut self, window: WindowId, prompt: Prompt) {
        if let Some(wix) = self.window_ix(window) {
            self.windows[wix].show_prompt(prompt, &self.shell.jobs);
        }
    }
}

pub(crate) struct App {
    pub(crate) windows: Vec<MainWindow>,
    pub(crate) active_window: Option<WindowId>,
    pub(crate) tab_drag: Option<TabDragState>,
    pub(crate) file_ops: FileOps,
    pub(crate) keytips: KeyTips,
    pub(crate) tooltips: Tooltips,
    cascade_step: u32,
    #[cfg(test)]
    pub(crate) headless: bool,
    quit: bool,
    // Declared after windows/overlays so surfaces drop before GPU devices, and before platform so COM drops last.
    pub(crate) shell: Shell,
    pub(crate) platform: Rc<Platform>,
}

impl App {
    pub(crate) fn new(platform: Rc<Platform>, gpu: Rc<Gpu>) -> Self {
        let shell = Shell::new(&platform, gpu);
        Self {
            windows: Vec::new(),
            active_window: None,
            tab_drag: None,
            file_ops: FileOps::default(),
            keytips: KeyTips::default(),
            tooltips: Tooltips::default(),
            cascade_step: 0,
            #[cfg(test)]
            headless: false,
            quit: false,
            shell,
            platform,
        }
    }

    fn run_file_ops(&mut self, f: impl FnOnce(&mut FileOps, &mut AppDocHost<'_>, &mut Vec<FileIo>)) {
        let mut io = Vec::new();
        {
            let mut host = AppDocHost {
                windows: &mut self.windows,
                active_window: self.active_window,
                shell: &self.shell,
            };
            f(&mut self.file_ops, &mut host, &mut io);
        }
        for cmd in io {
            let owner_hwnd = match &cmd {
                FileIo::OpenDialog { window, .. } => self.window_hwnd(*window),
                FileIo::SaveDialog { owner, .. } => self.window_hwnd(*owner),
                FileIo::LoadFiles { .. } | FileIo::WriteFile { .. } => None,
            };
            spawn_file_io(
                cmd,
                owner_hwnd,
                self.file_ops.tracker(),
                &self.shell.jobs,
                AppEvent::File,
            );
        }
    }

    fn deliver<R>(
        &mut self,
        window_id: WindowId,
        mark_dirty: bool,
        f: impl FnOnce(&mut MainWindow, &mut Cx<'_>) -> R,
    ) -> Option<R> {
        let ix = self.window_ix(window_id)?;
        let mut fx = Vec::new();
        let out = {
            let mut cx = self.shell.cx(&mut fx);
            let win = &mut self.windows[ix];
            let r = f(win, &mut cx);
            if mark_dirty {
                win.dirty = true;
            }
            r
        };
        self.run_effects(fx);
        Some(out)
    }

    fn run_effects(&mut self, fx: Vec<Effect>) {
        for effect in fx {
            match effect {
                Effect::NewWindow => {
                    let _ = self.open_window(None, None);
                }
                Effect::Exit => {
                    let ids: Vec<WindowId> = self.windows.iter().map(MainWindow::id).collect();
                    for window in ids {
                        self.run_file_ops(|ops, host, io| {
                            ops.request(FileRequest::CloseWindow { window }, host, io);
                        });
                    }
                    self.sweep_closed_windows();
                }
                Effect::ToggleStatusBar => {
                    self.shell.view.status_bar = !self.shell.view.status_bar;
                    self.mark_all_dirty();
                }
                Effect::ToggleTheme => {
                    self.shell.toggle_theme();
                    self.apply_theme_to_windows();
                }
                Effect::ToggleWordWrap => {
                    self.shell.view.word_wrap = !self.shell.view.word_wrap;
                    let wrap = self.shell.view.word_wrap;
                    for w in &mut self.windows {
                        w.set_word_wrap(wrap, &self.shell.text_system);
                    }
                }
                Effect::StartTabDrag {
                    window,
                    tab,
                    index,
                    grab,
                    position,
                } => {
                    self.tab_drag_press(window, tab, index, grab, position);
                }
                Effect::File(req) => {
                    self.run_file_ops(|ops, host, io| ops.request(req, host, io));
                }
                Effect::PromptResponse { id, button } => {
                    self.run_file_ops(|ops, host, io| ops.on_prompt(id, button, host, io));
                }
            }
        }
    }

    pub(crate) fn find_tab_owner(&self, tab: TabId) -> Option<(usize, usize)> {
        self.windows
            .iter()
            .enumerate()
            .find_map(|(wix, w)| w.tab_index(tab).map(|tix| (wix, tix)))
    }

    pub(crate) fn find_open_tab_by_key(&self, key: &PathKey) -> Option<(usize, usize)> {
        self.windows
            .iter()
            .enumerate()
            .find_map(|(wix, w)| w.find_open_tab_by_key(key).map(|tix| (wix, tix)))
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

    fn window_config(
        &mut self,
        window_size: Option<Size>,
        target_point: Option<ScreenPoint>,
    ) -> MainWindowConfig {
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
            || Rect::new(Point::ZERO, target_size),
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
        MainWindowConfig {
            bounds: Some(bounds),
            title: APP_NAME.into(),
            display_id: display.map(|d| d.id()),
            focus,
            show,
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
        let config = self.window_config(window_size, target_point);
        let (native, surface) =
            match NativeWindow::create_main(&config, self.shell.appearance()).and_then(|n| {
                let (w, h) = n.physical_size();
                let s = crate::gfx::Surface::new(
                    self.shell.gpu.clone(),
                    n.hwnd().raw(),
                    w,
                    h,
                    n.scale_factor(),
                )?;
                Ok((n, s))
            }) {
                Ok(created) => created,
                Err(err) => {
                    let message = err.to_string();
                    diag::debug_log(&format!("window/surface create failed: {message}"));
                    return Err(Box::new(OpenWindowError {
                        tab: initial,
                        message,
                    }));
                }
            };
        let id = native.id;
        let hwnd = native.hwnd();
        let win = MainWindow::new(
            native,
            surface,
            initial,
            self.shell.view.word_wrap,
            self.shell.settings,
            &self.shell.jobs,
        );
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

    pub(crate) fn open_paths_in_window(&mut self, window: WindowId, paths: Vec<PathBuf>) {
        self.run_file_ops(|ops, host, io| {
            ops.request(FileRequest::OpenPaths { window, paths }, host, io);
        });
    }

    pub(crate) fn open_handoff_paths(&mut self, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        if paths.len() == 1 {
            let key = PathKey::quick(&paths[0]);
            if let Some((wix, tix)) = self.find_open_tab_by_key(&key) {
                let hwnd = self.windows[wix].native.hwnd();
                self.windows[wix].activate_tab(tix, &self.shell.jobs);
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
        self.shell.sync_theme();
        self.apply_theme_to_windows();
    }

    fn apply_theme_to_windows(&mut self) {
        let appearance = self.shell.appearance();
        let settings = self.shell.settings;
        for w in &mut self.windows {
            w.native.set_appearance(appearance);
            w.set_blink_timing(settings.caret_blink_interval, settings.caret_blink_timeout);
            w.dirty = true;
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
                self.keytips.hide_owner(wid);
                self.tooltips.window_closed(wid, &self.shell.jobs);
                self.file_ops.on_window_closed(wid);
            }
            self.windows.retain(|w| !w.pending_close);
            if let Some(active) = self.active_window
                && self.window(active).is_none()
            {
                self.active_window = self.windows.last().map(MainWindow::id);
            }
        }
        self.sync_tab_drag();
        let dirty_any = self.windows.iter().any(MainWindow::has_dirty_tabs);
        let block = dirty_any.then_some("You have unsaved changes.");
        for w in &self.windows {
            w.native.set_block_shutdown(block);
        }
        if self.windows.is_empty() && !self.quit && self.file_ops.in_flight_writes() == 0 {
            self.request_quit();
        }
    }

    fn evaluate_tooltip_for(&mut self, ix: usize) {
        let probe = self.windows[ix].tooltip_probe();
        self.tooltips.evaluate(
            probe,
            &self.shell.gpu,
            &self.shell.text_system,
            &self.shell.jobs,
        );
    }

    fn paint_window_ix(&mut self, ix: usize, now: Instant) -> bool {
        let mut fx = Vec::new();
        let needs_vsync = {
            let cx = self.shell.cx(&mut fx);
            self.windows[ix].render_frame(now, &cx)
        };
        let wid = self.windows[ix].id();
        if self.windows[ix].menu_bar.access_active() || self.keytips.is_owner(wid) {
            let tips = self.windows[ix].key_tips(&self.shell.text_system);
            let origin = self.windows[ix].native.hwnd().client_origin();
            let scale = self.windows[ix].native.scale_factor();
            self.keytips
                .sync(wid, origin, scale, tips, &self.shell.gpu);
        }
        if self.windows[ix].ui.pointer().is_some() {
            self.evaluate_tooltip_for(ix);
        }
        needs_vsync
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
                self.deliver(window_id, true, |w, cx| {
                    w.surface.resize(physical_width, physical_height, scale);
                    w.cancel_menu(cx.jobs);
                });
                self.keytips.hide_owner(window_id);
            }
            PlatformEvent::Moved { window_id } => {
                self.deliver(window_id, false, |w, cx| {
                    w.cancel_menu(cx.jobs);
                });
                self.keytips.hide_owner(window_id);
            }
            PlatformEvent::Activated { window_id, active } => {
                if active {
                    self.active_window = Some(window_id);
                } else {
                    self.keytips.hide_owner(window_id);
                    self.tooltips
                        .window_deactivated(window_id, &self.shell.jobs);
                }
                self.deliver(window_id, true, |w, cx| w.on_activated(active, cx.jobs));
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
                self.deliver(window_id, false, |w, cx| {
                    w.on_mouse_move(position, pressed_button, cx);
                });
                if let Some(ix) = self.window_ix(window_id) {
                    self.evaluate_tooltip_for(ix);
                }
            }
            PlatformEvent::MouseLeave { window_id } => {
                self.deliver(window_id, false, |w, _| w.on_mouse_leave());
                self.tooltips.on_mouse_leave(window_id, &self.shell.jobs);
            }
            PlatformEvent::MouseDown {
                window_id,
                button,
                position,
                modifiers,
                click_count,
            } => {
                self.tooltips.on_mouse_down(window_id, &self.shell.jobs);
                self.deliver(window_id, true, |w, cx| {
                    w.on_mouse_down(button, position, modifiers, click_count, cx);
                });
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
                self.deliver(window_id, true, |w, cx| {
                    w.on_mouse_up(button, position, cx);
                });
            }
            PlatformEvent::ScrollWheel {
                window_id,
                position,
                delta_lines,
                page_scroll,
                modifiers,
            } => {
                self.deliver(window_id, false, |w, _| {
                    w.on_scroll_wheel(position, delta_lines, page_scroll, modifiers);
                });
            }
            PlatformEvent::KeyDown { window_id, event } => {
                self.tooltips.on_key_down();
                if self.is_tab_dragging()
                    && event.key == Key::Escape
                    && event.modifiers.none()
                    && !self
                        .window(window_id)
                        .is_some_and(|w| matches!(w.mode(), InputMode::Dialog | InputMode::Popup))
                {
                    self.tab_drag_cancel();
                    handled = true;
                } else if let Some(was_handled) =
                    self.deliver(window_id, true, |w, cx| w.on_key_down(event, cx))
                {
                    handled = was_handled;
                }
            }
            PlatformEvent::KeyUp { window_id, event } => {
                if let Some(was_handled) =
                    self.deliver(window_id, false, |w, cx| w.on_key_up(event, cx))
                {
                    handled = was_handled;
                }
            }
            PlatformEvent::CaptureLost { window_id } => {
                self.deliver(window_id, false, |w, _| w.on_capture_lost());
                if self
                    .tab_drag
                    .as_ref()
                    .is_some_and(|d| d.owner() == window_id && d.is_dragging())
                {
                    self.tab_drag_released();
                }
            }
            PlatformEvent::ModifiersChanged { modifiers } => {
                self.tooltips
                    .on_modifiers_changed(modifiers, &self.shell.jobs);
            }
            PlatformEvent::TextInput { window_id, text } => {
                self.deliver(window_id, true, |w, cx| w.on_text_input(&text, cx));
            }
            PlatformEvent::ImeComposition {
                window_id,
                text,
                cursor,
            } => {
                self.deliver(window_id, true, |w, cx| {
                    w.on_ime_composition(&text, cursor, cx);
                });
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
                self.run_file_ops(|ops, host, io| {
                    ops.request(FileRequest::CloseWindow { window: window_id }, host, io);
                });
            }
            PlatformEvent::SessionEnding { ending } => {
                if ending {
                    self.file_ops.wait_for_writes(Duration::from_secs(5));
                }
            }
        }
        self.sweep_closed_windows();
        handled
    }

    fn on_wake(&mut self) {
        while let Some(ev) = self.shell.jobs.pop() {
            match ev {
                AppEvent::EditorTimer { window, slot, kind } => {
                    let target_ix = match slot {
                        EditorSlot::Tab(id) => self
                            .find_tab_owner(id)
                            .map(|(wix, _)| wix)
                            .or_else(|| self.window_ix(window)),
                        _ => self.window_ix(window),
                    };
                    if let Some(ix) = target_ix {
                        self.windows[ix].on_editor_timer(slot, kind, &self.shell.jobs);
                    }
                }
                AppEvent::TabScrollRepeat { window, step } => {
                    if let Some(ix) = self.window_ix(window) {
                        self.windows[ix].on_tab_scroll_repeat(step);
                    }
                }
                AppEvent::MenuSubmenuTimer { window, target } => {
                    if let Some(ix) = self.window_ix(window) {
                        self.windows[ix].on_menu_submenu_timer(target, &self.shell.text_system);
                    }
                }
                AppEvent::FindNoticeDismiss { window } => {
                    if let Some(ix) = self.window_ix(window) {
                        self.windows[ix].on_find_notice_dismiss();
                    }
                }
                AppEvent::TooltipShowTimer { window, hit } => {
                    let probe = self
                        .window_ix(window)
                        .map(|ix| self.windows[ix].tooltip_probe());
                    self.tooltips.on_show_timer(
                        TooltipTarget { window, hit },
                        probe,
                        &self.shell.gpu,
                        &self.shell.text_system,
                        &self.shell.jobs,
                    );
                }
                AppEvent::TooltipHideTimer => {
                    self.tooltips.on_hide_timer();
                }
                AppEvent::TabDragPoll => self.tab_drag_poll(),
                AppEvent::File(fe) => {
                    self.run_file_ops(|ops, host, io| ops.on_event(fe, host, io));
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

    fn drive_frames(&mut self) {
        self.sweep_closed_windows();
        if self.quit {
            return;
        }
        let now = Instant::now();
        let gen_before = self.shell.gpu.generation();
        let mut any_vsync = false;
        let mut painted_any = false;
        for ix in 0..self.windows.len() {
            if self.windows[ix].dirty {
                painted_any = true;
                any_vsync |= self.paint_window_ix(ix, now);
            }
        }
        if painted_any {
            self.shell.text_system.finish_frame();
        }
        any_vsync |= self
            .keytips
            .paint(self.shell.palette, &self.shell.text_system);
        any_vsync |= self.tooltips.paint(
            self.shell.palette,
            self.shell.settings.reduce_motion,
            now,
            &self.shell.text_system,
        );
        if self.shell.gpu.generation() != gen_before {
            self.mark_all_dirty();
            any_vsync = true;
        }
        if any_vsync {
            self.platform.request_frame_tick();
        }
    }
}
