#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod command;
mod dialog;
mod document;
mod editor;
mod files;
mod find;
mod geom;
mod gfx;
mod layout;
mod lines;
mod menu;
mod platform;
mod shell;
mod tabs;
mod text;
mod theme;
mod tooltip;
mod types;
mod ui;
mod widgets;
mod window;
#[cfg(test)]
mod tests;

use std::cell::RefCell;
use std::ops::{Deref, DerefMut};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::document::{DocPath, Format, PathKey};
use crate::text::Rope;
use crate::files::{
    DocHost, FileIo, FileOps, FileRequest, Prompt, PromptId, TabSnapshot, spawn_file_io,
};
use crate::geom::{Point, Rect, ScreenPoint, Size, point, size};
use crate::gfx::{Gpu, TextSystem, WindowSurface};
use crate::platform::{
    Arbitration, Key, MainWindowConfig, MouseButton, NativeWindow, Platform, PlatformEvent,
    PlatformHandler, TimerHandle, WindowAppearance, WindowHandle, WindowId, WindowPlacement,
    WindowsDisplay, arbitrate_instance, cursor_screen_pos, debug_log, install_panic_hook,
    primary_mouse_button_down, release_mouse_capture, show_error_dialog, top_visible_window_at,
};
use crate::shell::{AppEvent, Cx, Effect, Shell, WindowTimer};
use crate::tabs::TabData;
use crate::tooltip::{KeyTips, TooltipTarget, Tooltips};
use crate::types::APP_NAME;
use crate::ui::{EditorSlot, TabId};
use crate::window::{InputMode, MainWindow, WINDOW_HEIGHT, WINDOW_MAX_SCREEN_SHARE, WINDOW_WIDTH};

const NOTICES_FLAG: &str = "--notices";
const NOTICES: &str = include_str!(concat!(env!("OUT_DIR"), "/notices.txt"));
const NOTICES_FILE: &str = "THIRD-PARTY-NOTICES.txt";

fn write_notices() -> std::io::Result<PathBuf> {
    let dir = std::env::temp_dir().join(APP_NAME);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(NOTICES_FILE);
    std::fs::write(&path, NOTICES)?;
    Ok(path)
}

fn main() {
    install_panic_hook();

    let (flags, mut paths): (Vec<_>, Vec<_>) = std::env::args_os()
        .skip(1)
        .map(PathBuf::from)
        .partition(|arg| arg.as_os_str() == NOTICES_FLAG);
    if !flags.is_empty() {
        match write_notices() {
            Ok(path) => paths.push(path),
            Err(err) => debug_log(&format!("Failed to write notices file: {err}")),
        }
    }

    let lock = match arbitrate_instance(&paths) {
        Arbitration::HandedOff => return,
        Arbitration::Primary(lock) => lock,
    };

    let platform = match Platform::new(lock) {
        Ok(p) => Rc::new(p),
        Err(err) => {
            show_error_dialog(APP_NAME, &format!("Jot couldn't start.\n\n{err}"));
            return;
        }
    };
    let (gpu, text_system) = match Gpu::new().and_then(|g| Ok((g, TextSystem::new()?))) {
        Ok(devices) => devices,
        Err(err) => {
            show_error_dialog(APP_NAME, &format!("Jot couldn't start.\n\n{err}"));
            return;
        }
    };
    let app = Rc::new(RefCell::new(App::new(platform, gpu, text_system)));

    Platform::run(app, move |app| {
        if let Err(message) = app.launch(paths, false) {
            show_error_dialog(APP_NAME, &format!("Jot couldn't open a window.\n\n{message}"));
            app.request_quit();
        }
    });
}

pub(crate) struct App {
    pub(crate) windows: Windows,
    tab_drag: Option<TabDrag>,
    pub(crate) file_ops: FileOps,
    keytips: KeyTips,
    tooltips: Tooltips,
    quit: bool,
    // Declared after windows/overlays so surfaces drop before GPU devices, and before platform so COM drops last.
    pub(crate) shell: Shell,
    platform: Rc<Platform>,
}

impl App {
    pub(crate) fn new(platform: Rc<Platform>, gpu: Rc<Gpu>, text_system: TextSystem) -> Self {
        Self::with_launch(platform, gpu, text_system, WindowLaunch::default())
    }

    pub(crate) fn with_launch(
        platform: Rc<Platform>,
        gpu: Rc<Gpu>,
        text_system: TextSystem,
        launch: WindowLaunch,
    ) -> Self {
        let shell = Shell::new(&platform, gpu, text_system);
        let mut tooltips = Tooltips::default();
        tooltips.hover_time = shell.settings.mouse_hover_time;
        Self {
            windows: Windows::new(launch),
            tab_drag: None,
            file_ops: FileOps::default(),
            keytips: KeyTips::default(),
            tooltips,
            quit: false,
            shell,
            platform,
        }
    }

    fn run_file_ops(
        &mut self,
        f: impl FnOnce(&mut FileOps, &mut WindowsHost<'_>, &mut Vec<FileIo>),
    ) {
        let mut io = Vec::new();
        {
            let mut host = self.windows.host(&self.shell);
            f(&mut self.file_ops, &mut host, &mut io);
        }
        for cmd in io {
            let owner_hwnd = match &cmd {
                FileIo::OpenDialog { window, .. } => self.windows.hwnd(*window),
                FileIo::SaveDialog { owner, .. } => self.windows.hwnd(*owner),
                FileIo::LoadFiles { .. } | FileIo::WriteFile { .. } => None,
            };
            spawn_file_io(
                cmd,
                owner_hwnd,
                &self.file_ops.writes,
                &self.shell.jobs,
                AppEvent::File,
            );
        }
    }

    fn with_window<R>(
        &mut self,
        window_id: WindowId,
        f: impl FnOnce(&mut MainWindow, &mut Cx<'_>) -> R,
    ) -> Option<R> {
        self.with_window_inner(window_id, true, f)
    }

    fn with_window_quiet<R>(
        &mut self,
        window_id: WindowId,
        f: impl FnOnce(&mut MainWindow, &mut Cx<'_>) -> R,
    ) -> Option<R> {
        self.with_window_inner(window_id, false, f)
    }

    fn with_window_inner<R>(
        &mut self,
        window_id: WindowId,
        mark_dirty: bool,
        f: impl FnOnce(&mut MainWindow, &mut Cx<'_>) -> R,
    ) -> Option<R> {
        let win = self.windows.get_mut(window_id)?;
        let mut fx = Vec::new();
        let out = {
            let mut cx = self.shell.cx(&mut fx);
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
                    for window in self.windows.ids() {
                        self.run_file_ops(|ops, host, io| {
                            ops.request(FileRequest::CloseWindow { window }, host, io);
                        });
                    }
                    self.sweep_closed_windows();
                }
                Effect::ToggleStatusBar => {
                    self.shell.view.status_bar = !self.shell.view.status_bar;
                    self.windows.mark_all_dirty();
                }
                Effect::ToggleTheme => {
                    self.shell.toggle_theme();
                    self.windows.apply_theme(self.shell.appearance());
                }
                Effect::ToggleWordWrap => {
                    self.shell.view.word_wrap = !self.shell.view.word_wrap;
                    self.windows.set_word_wrap(&self.shell);
                }
                Effect::StartTabDrag {
                    window,
                    tab,
                    index,
                    grab,
                    position,
                } => {
                    self.tab_drag =
                        TabDrag::start(window, tab, index, grab, position, &mut self.windows);
                }
                Effect::File(req) => {
                    self.run_file_ops(|ops, host, io| ops.request(req, host, io));
                }
                Effect::PromptResponse { id, answer } => {
                    self.run_file_ops(|ops, host, io| ops.on_prompt(id, answer, host, io));
                }
            }
        }
    }

    fn end_tab_drag(&mut self) {
        if let Some(drag) = self.tab_drag.take() {
            self.windows.mark_dirty(drag.owner);
            if drag.source != drag.owner {
                self.windows.mark_dirty(drag.source);
            }
        }
    }

    fn notify_closed(&mut self, closed: Vec<(WindowId, Vec<crate::files::PromptId>)>) {
        for (wid, prompts) in closed {
            self.keytips.hide_owner(wid);
            self.tooltips.window_closed(wid, &self.shell.jobs);
            self.file_ops.on_window_closed(wid, &prompts);
        }
    }

    fn drive_drag(
        &mut self,
        f: impl FnOnce(
            &mut TabDrag,
            &mut Windows,
            &FileOps,
            &Shell,
            &mut Vec<(WindowId, Vec<crate::files::PromptId>)>,
        ) -> bool,
    ) {
        let Some(drag) = &mut self.tab_drag else {
            return;
        };
        let mut closed = Vec::new();
        let ended = f(
            drag,
            &mut self.windows,
            &self.file_ops,
            &self.shell,
            &mut closed,
        );
        if ended {
            self.end_tab_drag();
        }
        self.notify_closed(closed);
    }

    fn cancel_tab_drag(&mut self) {
        let Some(mut drag) = self.tab_drag.take() else {
            return;
        };
        let mut closed = Vec::new();
        drag.cancel(&mut self.windows, &self.shell, &mut closed);
        self.windows.mark_dirty(drag.owner);
        if drag.source != drag.owner {
            self.windows.mark_dirty(drag.source);
        }
        self.notify_closed(closed);
    }

    pub(crate) fn open_window(
        &mut self,
        initial: Option<TabData>,
        window_size: Option<Size>,
    ) -> std::result::Result<(WindowId, WindowHandle), Box<OpenWindowError>> {
        self.windows.open_at(initial, window_size, None, &self.shell)
    }

    pub(crate) fn request_quit(&mut self) {
        if !self.quit {
            self.quit = true;
            self.platform.post_quit();
        }
    }

    pub(crate) fn launch(
        &mut self,
        paths: Vec<PathBuf>,
        bring_to_front: bool,
    ) -> std::result::Result<(), String> {
        if paths.is_empty() || self.windows.is_empty() {
            let (window, hwnd) = self.open_window(None, None).map_err(|e| e.message)?;
            if bring_to_front {
                hwnd.bring_to_front();
            }
            if !paths.is_empty() {
                self.open_paths_in_window(window, paths);
            }
            return Ok(());
        }
        if let Some(window) = self.windows.fallback(None) {
            let mut loaded_any = false;
            self.run_file_ops(|ops, host, io| {
                let before = io.len();
                ops.request(FileRequest::OpenPaths { window, paths }, host, io);
                loaded_any = io.len() > before;
            });
            if bring_to_front
                && loaded_any
                && let Some(hwnd) = self.windows.hwnd(window)
            {
                hwnd.bring_to_front();
            }
        }
        Ok(())
    }

    fn open_paths_in_window(&mut self, window: WindowId, paths: Vec<PathBuf>) {
        self.run_file_ops(|ops, host, io| {
            ops.request(FileRequest::OpenPaths { window, paths }, host, io);
        });
    }

    pub(crate) fn sweep_closed_windows(&mut self) {
        let closed = self.windows.sweep_closed();
        self.notify_closed(closed);
        if self
            .tab_drag
            .as_ref()
            .is_some_and(|d| !d.is_alive(&self.windows))
        {
            self.end_tab_drag();
        }
        self.platform.set_block_shutdown(
            self.windows
                .any_dirty()
                .then_some("You have unsaved changes."),
        );
        if self.windows.is_empty() && !self.quit && self.file_ops.writes.in_flight() == 0 {
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
        let needs_vsync = self.windows[ix].render_frame(now, &self.shell);
        let wid = self.windows[ix].id();
        if self.windows[ix].key_tips_active() || self.keytips.is_owner(wid) {
            let (anchor, tips) = self.windows[ix].key_tip_request(&self.shell);
            self.keytips.sync(anchor, tips, &self.shell.gpu);
        }
        if self.windows[ix].has_pointer() {
            self.evaluate_tooltip_for(ix);
        }
        needs_vsync
    }

    #[cfg(test)]
    pub(crate) fn is_tab_drag_active(&self) -> bool {
        self.tab_drag.is_some()
    }

    #[cfg(test)]
    pub(crate) fn tab_drag_start_and_detach_for_test(
        &mut self,
        window: WindowId,
        tab: crate::ui::TabId,
        index: usize,
        grab: crate::geom::Point,
        position: crate::geom::Point,
    ) {
        self.tab_drag = TabDrag::start(window, tab, index, grab, position, &mut self.windows);
        self.drive_drag(|drag, windows, _, shell, closed| {
            drag.detach_for_test(windows, shell, closed);
            false
        });
    }

    #[cfg(test)]
    pub(crate) fn tab_drag_poll_for_test(&mut self) {
        self.drive_drag(|drag, windows, ops, shell, closed| {
            drag.on_poll(windows, ops, shell, closed)
        });
    }

    #[cfg(test)]
    pub(crate) fn tab_drag_cancel_for_test(&mut self) {
        self.cancel_tab_drag();
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
                self.with_window(window_id, |w, cx| {
                    w.on_resized(physical_width, physical_height, scale, cx.shell);
                });
                self.keytips.hide_owner(window_id);
            }
            PlatformEvent::Moved { window_id } => {
                self.with_window_quiet(window_id, |w, cx| {
                    w.cancel_menu(cx.shell);
                });
                self.keytips.hide_owner(window_id);
            }
            PlatformEvent::Activated { window_id, active } => {
                if active {
                    self.windows.set_active(window_id);
                } else {
                    self.keytips.hide_owner(window_id);
                    self.tooltips
                        .window_deactivated(window_id, &self.shell.jobs);
                }
                self.with_window(window_id, |w, cx| w.on_activated(active, cx.shell));
            }
            PlatformEvent::PaintRequested { window_id } => {
                if let Some(w) = self.windows.get_mut(window_id) {
                    w.on_paint_requested();
                }
            }
            PlatformEvent::SettingsChanged(settings) => {
                self.shell.apply_settings(settings);
                self.tooltips.hover_time = settings.mouse_hover_time;
                self.windows.apply_theme(self.shell.appearance());
            }
            PlatformEvent::MouseMove {
                window_id,
                position,
                pressed_button,
            } => {
                let left = pressed_button == Some(MouseButton::Left);
                self.drive_drag(|drag, windows, ops, shell, closed| {
                    drag.on_mouse_move(left, windows, ops, shell, closed)
                });
                self.with_window_quiet(window_id, |w, cx| {
                    w.on_mouse_move(position, pressed_button, cx);
                });
                if let Some(ix) = self.windows.ix(window_id) {
                    self.evaluate_tooltip_for(ix);
                }
            }
            PlatformEvent::MouseLeave { window_id } => {
                self.with_window_quiet(window_id, |w, _| w.on_mouse_leave());
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
                self.with_window(window_id, |w, cx| {
                    w.on_mouse_down(button, position, modifiers, click_count, cx);
                });
            }
            PlatformEvent::MouseUp {
                window_id,
                button,
                position,
            } => {
                self.tooltips.on_mouse_up();
                if button == MouseButton::Left {
                    self.drive_drag(|drag, windows, _, _, _| drag.on_release(windows));
                }
                self.with_window(window_id, |w, cx| {
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
                self.with_window_quiet(window_id, |w, _| {
                    w.on_scroll_wheel(position, delta_lines, page_scroll, modifiers);
                });
            }
            PlatformEvent::KeyDown { window_id, event } => {
                self.tooltips.on_key_down();
                if self.tab_drag.as_ref().is_some_and(TabDrag::is_dragging)
                    && event.key == Key::Escape
                    && event.modifiers.none()
                    && !self
                        .windows
                        .get(window_id)
                        .is_some_and(|w| matches!(w.mode(), InputMode::Dialog | InputMode::Popup))
                {
                    self.cancel_tab_drag();
                    handled = true;
                } else if let Some(was_handled) =
                    self.with_window(window_id, |w, cx| w.on_key_down(event, cx))
                {
                    handled = was_handled;
                }
            }
            PlatformEvent::KeyUp { window_id, event } => {
                if let Some(was_handled) =
                    self.with_window_quiet(window_id, |w, cx| w.on_key_up(event, cx))
                {
                    handled = was_handled;
                }
            }
            PlatformEvent::CaptureLost { window_id } => {
                self.with_window_quiet(window_id, |w, _| w.on_capture_lost());
                self.drive_drag(|drag, windows, _, _, _| drag.on_capture_lost(window_id, windows));
            }
            PlatformEvent::ModifiersChanged { modifiers } => {
                self.tooltips
                    .on_modifiers_changed(modifiers, &self.shell.jobs);
            }
            PlatformEvent::TextInput { window_id, text } => {
                self.with_window(window_id, |w, cx| w.on_text_input(&text, cx));
            }
            PlatformEvent::ImeComposition {
                window_id,
                text,
                cursor,
            } => {
                self.with_window(window_id, |w, cx| {
                    w.on_ime_composition(&text, cursor, cx);
                });
            }
            PlatformEvent::FilesDropped { window_id, paths } => {
                if self.windows.get(window_id).is_some() {
                    self.open_paths_in_window(window_id, paths);
                }
            }
            PlatformEvent::Launch { paths } => {
                let _ = self.launch(paths, true);
            }
            PlatformEvent::SizeMoveChanged {
                window_id,
                entering,
            } => {
                self.drive_drag(|drag, windows, ops, shell, closed| {
                    drag.on_size_move_changed(window_id, entering, windows, ops, shell, closed)
                });
            }
            PlatformEvent::CloseRequested { window_id } => {
                self.run_file_ops(|ops, host, io| {
                    ops.request(FileRequest::CloseWindow { window: window_id }, host, io);
                });
            }
            PlatformEvent::SessionEnding { ending } => {
                if ending {
                    self.file_ops.writes.wait(Duration::from_secs(5));
                }
            }
        }
        self.sweep_closed_windows();
        handled
    }

    fn on_wake(&mut self) {
        while let Some(ev) = self.shell.jobs.pop() {
            match ev {
                AppEvent::Window { window, timer } => {
                    let target = match timer {
                        WindowTimer::Editor {
                            slot: EditorSlot::Tab(id),
                            ..
                        } => self
                            .windows
                            .owner_of(id)
                            .map(|(ix, _)| self.windows[ix].id())
                            .unwrap_or(window),
                        _ => window,
                    };
                    self.with_window_quiet(target, |w, cx| w.on_timer(timer, cx.shell));
                }
                AppEvent::TooltipShowTimer { window, hit } => {
                    let probe = self.windows.get(window).map(MainWindow::tooltip_probe);
                    self.tooltips.on_show_timer(
                        TooltipTarget { window, hit },
                        probe,
                        &self.shell.gpu,
                        &self.shell.text_system,
                        &self.shell.jobs,
                    );
                }
                AppEvent::TabDragPoll => {
                    self.drive_drag(|drag, windows, ops, shell, closed| {
                        drag.on_poll(windows, ops, shell, closed)
                    });
                }
                AppEvent::File(fe) => {
                    self.run_file_ops(|ops, host, io| ops.on_event(fe, host, io));
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
            self.windows.mark_all_dirty();
            any_vsync = true;
        }
        if any_vsync {
            self.platform.request_frame_tick();
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub(crate) struct WindowLaunch {
    pub(crate) show: bool,
    pub(crate) focus: bool,
    pub(crate) use_cursor_display: bool,
}

impl Default for WindowLaunch {
    fn default() -> Self {
        Self {
            show: true,
            focus: true,
            use_cursor_display: true,
        }
    }
}

impl WindowLaunch {
    #[cfg(test)]
    pub(crate) const HIDDEN: Self = Self {
        show: false,
        focus: false,
        use_cursor_display: false,
    };
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

pub(crate) struct Windows {
    list: Vec<MainWindow>,
    active: Option<WindowId>,
    cascade_step: u32,
    launch: WindowLaunch,
}

impl Windows {
    pub(crate) fn new(launch: WindowLaunch) -> Self {
        Self {
            list: Vec::new(),
            active: None,
            cascade_step: 0,
            launch,
        }
    }

    pub(crate) fn set_active(&mut self, id: WindowId) {
        self.active = Some(id);
    }

    pub(crate) fn ix(&self, id: WindowId) -> Option<usize> {
        self.list.iter().position(|w| w.id() == id)
    }

    pub(crate) fn get(&self, id: WindowId) -> Option<&MainWindow> {
        self.list.iter().find(|w| w.id() == id)
    }

    pub(crate) fn get_mut(&mut self, id: WindowId) -> Option<&mut MainWindow> {
        self.list.iter_mut().find(|w| w.id() == id)
    }

    pub(crate) fn hwnd(&self, id: WindowId) -> Option<WindowHandle> {
        self.get(id).map(MainWindow::hwnd)
    }

    pub(crate) fn ids(&self) -> Vec<WindowId> {
        self.list.iter().map(MainWindow::id).collect()
    }

    pub(crate) fn owner_of(&self, tab: TabId) -> Option<(usize, usize)> {
        self.list
            .iter()
            .enumerate()
            .find_map(|(wix, w)| w.tabs().index_of(tab).map(|tix| (wix, tix)))
    }

    pub(crate) fn find_by_key(&self, key: &PathKey) -> Option<(WindowId, TabId)> {
        self.list.iter().find_map(|w| {
            let tix = w.tabs().find_by_key(key)?;
            Some((w.id(), w.tabs()[tix].data.id))
        })
    }

    pub(crate) fn fallback(&self, preferred: Option<WindowId>) -> Option<WindowId> {
        preferred
            .and_then(|id| self.ix(id))
            .or_else(|| self.active.and_then(|id| self.ix(id)))
            .or_else(|| (!self.list.is_empty()).then_some(self.list.len() - 1))
            .map(|ix| self.list[ix].id())
    }

    pub(crate) fn mark_dirty(&mut self, id: WindowId) {
        if let Some(w) = self.get_mut(id) {
            w.dirty = true;
        }
    }

    pub(crate) fn mark_all_dirty(&mut self) {
        for w in &mut self.list {
            w.dirty = true;
        }
    }

    pub(crate) fn apply_theme(&mut self, appearance: WindowAppearance) {
        for w in &mut self.list {
            w.apply_theme(appearance);
        }
    }

    pub(crate) fn set_word_wrap(&mut self, shell: &Shell) {
        for w in &mut self.list {
            w.set_word_wrap(shell);
        }
    }

    pub(crate) fn any_dirty(&self) -> bool {
        self.list.iter().any(|w| w.tabs().has_dirty())
    }

    pub(crate) fn rescue_tab(
        &mut self,
        preferred: Option<WindowId>,
        index: usize,
        tab: TabData,
        shell: &Shell,
    ) {
        if let Some(id) = preferred
            && let Some(win) = self.get_mut(id)
        {
            win.restore_tab(tab, index, shell);
        } else if let Some(win) = self.list.last_mut() {
            let ix = win.tabs().len();
            win.restore_tab(tab, ix, shell);
        }
    }

    pub(crate) fn sweep_closed(&mut self) -> Vec<(WindowId, Vec<crate::files::PromptId>)> {
        if !self.list.iter().any(|w| w.pending_close) {
            return Vec::new();
        }
        let closed: Vec<(WindowId, Vec<crate::files::PromptId>)> = self
            .list
            .iter_mut()
            .filter(|w| w.pending_close)
            .map(|w| (w.id(), w.drain_cancelled_prompts()))
            .collect();
        self.list.retain(|w| !w.pending_close);
        if let Some(active) = self.active
            && self.get(active).is_none()
        {
            self.active = self.list.last().map(MainWindow::id);
        }
        closed
    }

    fn window_config(
        &mut self,
        window_size: Option<Size>,
        target_point: Option<ScreenPoint>,
    ) -> MainWindowConfig {
        let display = target_point
            .or_else(|| self.active.and_then(|id| self.get(id)).map(|w| w.hwnd().client_origin()))
            .or_else(|| self.launch.use_cursor_display.then(cursor_screen_pos))
            .and_then(WindowsDisplay::from_point)
            .or_else(WindowsDisplay::primary);
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
        if target_point.is_none() && !self.list.is_empty() && window_size.is_none() {
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
        MainWindowConfig {
            bounds: Some(bounds),
            title: APP_NAME.into(),
            display_id: display.map(|d| d.id()),
            focus: self.launch.focus,
            show: self.launch.show,
        }
    }

    pub(crate) fn open_at(
        &mut self,
        initial: Option<TabData>,
        window_size: Option<Size>,
        target_point: Option<ScreenPoint>,
        shell: &Shell,
    ) -> std::result::Result<(WindowId, WindowHandle), Box<OpenWindowError>> {
        let config = self.window_config(window_size, target_point);
        let win_surface = match NativeWindow::create_main(&config, shell.appearance())
            .and_then(|n| WindowSurface::new(shell.gpu.clone(), n))
        {
            Ok(created) => created,
            Err(err) => {
                let message = err.to_string();
                debug_log(&format!("window/surface create failed: {message}"));
                return Err(Box::new(OpenWindowError {
                    tab: initial,
                    message,
                }));
            }
        };
        let id = win_surface.window.id;
        let hwnd = win_surface.window.hwnd();
        let win = MainWindow::new(win_surface, initial, shell);
        self.list.push(win);
        self.active = Some(id);
        Ok((id, hwnd))
    }

    pub(crate) fn host<'a>(&'a mut self, shell: &'a Shell) -> WindowsHost<'a> {
        WindowsHost {
            windows: self,
            shell,
        }
    }
}

impl Deref for Windows {
    type Target = [MainWindow];

    fn deref(&self) -> &[MainWindow] {
        &self.list
    }
}

impl DerefMut for Windows {
    fn deref_mut(&mut self) -> &mut [MainWindow] {
        &mut self.list
    }
}

pub(crate) struct WindowsHost<'a> {
    windows: &'a mut Windows,
    shell: &'a Shell,
}

impl DocHost for WindowsHost<'_> {
    fn tab_info(&self, tab: TabId) -> Option<TabSnapshot> {
        let (wix, tix) = self.windows.owner_of(tab)?;
        let w = &self.windows[wix];
        let tabs = w.tabs();
        let data = &tabs[tix].data;
        Some(TabSnapshot {
            window: w.id(),
            name: tabs.tab_name(tix),
            path: data.path().map(PathBuf::from),
            format: data.format,
            dirty: data.is_dirty(),
            revision: data.editor.model().revision(),
        })
    }

    fn window_dirty_tabs(&self, window: WindowId) -> Vec<TabId> {
        self.windows
            .get(window)
            .map(|w| w.tabs().dirty_tabs())
            .unwrap_or_default()
    }

    fn active_tab_path(&self, window: WindowId) -> Option<PathBuf> {
        self.windows
            .get(window)?
            .tabs()
            .active()
            .path()
            .map(PathBuf::from)
    }

    fn fallback_window(&self, preferred: WindowId) -> Option<WindowId> {
        self.windows.fallback(Some(preferred))
    }

    fn find_tab_by_key(&self, key: &PathKey) -> Option<(WindowId, TabId)> {
        self.windows.find_by_key(key)
    }

    fn activate_tab(&mut self, window: WindowId, tab: TabId, bring_to_front: bool) {
        let Some(w) = self.windows.get_mut(window) else {
            return;
        };
        let Some(tix) = w.tabs().index_of(tab) else {
            return;
        };
        let hwnd = w.hwnd();
        w.activate_tab(tix, self.shell);
        if bring_to_front {
            hwnd.bring_to_front();
        }
    }

    fn begin_save(&mut self, tab: TabId) -> Option<(u64, Rope)> {
        let (wix, _) = self.windows.owner_of(tab)?;
        self.windows[wix].begin_tab_save(tab)
    }

    fn mark_saved(&mut self, tab: TabId, snapshot: u64, doc_path: DocPath, format: Format) {
        if let Some((wix, _)) = self.windows.owner_of(tab) {
            self.windows[wix].mark_tab_saved(tab, snapshot, doc_path, format);
        }
    }

    fn adopt_loaded(&mut self, window: WindowId, loaded: document::Loaded) {
        if let Some(w) = self.windows.get_mut(window) {
            w.adopt_loaded_file(loaded, self.shell);
        }
    }

    fn remove_tab(&mut self, tab: TabId) {
        if let Some((wix, tix)) = self.windows.owner_of(tab) {
            self.windows[wix].remove_tab(tix, self.shell);
        }
    }

    fn mark_window_close(&mut self, window: WindowId) {
        if let Some(w) = self.windows.get_mut(window) {
            w.pending_close = true;
        }
    }

    fn show_prompt(&mut self, window: WindowId, prompt: Prompt) {
        if let Some(w) = self.windows.get_mut(window) {
            w.show_prompt(prompt, self.shell);
        }
    }
}

type ClosedWindow = (WindowId, Vec<PromptId>);

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
        win.dirty = true;
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
            match windows.open_at(Some(data), None, None, shell) {
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
