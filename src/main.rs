#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Jot, a notepad. `main` starts one instance (or hands its files to the running one);
//! `App` holds the windows and what spans them, and takes the platform's events and frames.

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
#[cfg(test)]
mod tests;
mod text;
mod theme;
mod tooltip;
mod types;
mod ui;
mod widgets;
mod window;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::files::{FileEvents, FileIo, FileOps, FileRequest};
use crate::geom::Size;
use crate::gfx::{Gpu, TextSystem};
use crate::platform::{
    Arbitration, Key, MouseButton, Platform, PlatformEvent, PlatformHandler, WindowEvent,
    WindowHandle, WindowId, arbitrate_instance, debug_log, install_panic_hook, show_error_dialog,
};
use crate::shell::{Cx, Effect, Shell};
use crate::tabs::TabData;
use crate::tooltip::{KeyTips, Tooltips};
use crate::types::APP_NAME;
use crate::window::{
    InputMode, MainWindow, OpenWindowError, TabDrag, WindowLaunch, Windows, WindowsHost,
};

const NOTICES_FLAG: &str = "--notices";
const NOTICES: &str = include_str!(concat!(env!("OUT_DIR"), "/notices.txt"));
const NOTICES_FILE: &str = "THIRD-PARTY-NOTICES.txt";

// ---- 1. Starting up -------------------------------------------------------------------------

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
    let started = Platform::new(lock).and_then(|platform| {
        let gpu = Gpu::new()?;
        Ok((Rc::new(platform), gpu, TextSystem::new()?))
    });
    let (platform, gpu, text_system) = match started {
        Ok(started) => started,
        Err(err) => {
            show_error_dialog(APP_NAME, &format!("Jot couldn't start.\n\n{err}"));
            return;
        }
    };
    let app = Rc::new(RefCell::new(App::new(platform, gpu, text_system)));
    Platform::run(app, move |app| {
        if let Err(message) = app.launch(paths, false) {
            show_error_dialog(
                APP_NAME,
                &format!("Jot couldn't open a window.\n\n{message}"),
            );
            app.request_quit();
        }
    });
}

// ---- 2. The app -----------------------------------------------------------------------------

pub(crate) struct App {
    pub(crate) windows: Windows,
    tab_drag: Option<TabDrag>,
    file_ops: FileOps,
    file_events: FileEvents,
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
        Self {
            windows: Windows::new(launch),
            tab_drag: None,
            file_ops: FileOps::default(),
            file_events: FileEvents::new(platform.waker()),
            keytips: KeyTips::default(),
            tooltips: Tooltips::default(),
            quit: false,
            shell: Shell::new(gpu, text_system),
            platform,
        }
    }

    /// Opens launch files: in a new window if none (or no files), else the active one, raised.
    pub(crate) fn launch(
        &mut self,
        paths: Vec<PathBuf>,
        bring_to_front: bool,
    ) -> Result<(), String> {
        if paths.is_empty() || self.windows.is_empty() {
            let (window, hwnd) = self.open_window(None, None).map_err(|e| e.message)?;
            if bring_to_front {
                hwnd.bring_to_front();
            }
            if !paths.is_empty() {
                self.open_paths(window, paths);
            }
        } else if let Some(window) = self.windows.fallback(None)
            && self.open_paths(window, paths)
            && bring_to_front
            && let Some(hwnd) = self.windows.hwnd(window)
        {
            hwnd.bring_to_front();
        }
        Ok(())
    }

    pub(crate) fn open_window(
        &mut self,
        initial: Option<TabData>,
        window_size: Option<Size>,
    ) -> Result<(WindowId, WindowHandle), Box<OpenWindowError>> {
        self.windows
            .open_at(initial, window_size, None, &self.shell)
    }

    fn request_quit(&mut self) {
        if !self.quit {
            self.quit = true;
            self.platform.post_quit();
        }
    }

    fn open_paths(&mut self, window: WindowId, paths: Vec<PathBuf>) -> bool {
        self.run_file_ops(|ops, host, io| {
            ops.request(FileRequest::OpenPaths { window, paths }, host, io);
        })
    }

    /// Runs `f` on the file workflows and their windows, then starts its file work; whether any.
    fn run_file_ops(
        &mut self,
        f: impl FnOnce(&mut FileOps, &mut WindowsHost<'_>, &mut Vec<FileIo>),
    ) -> bool {
        let mut io = Vec::new();
        let mut host = self.windows.host(&self.shell);
        f(&mut self.file_ops, &mut host, &mut io);
        let started = !io.is_empty();
        self.file_ops.spawn(io, &host, &self.file_events);
        started
    }

    /// Drops closed windows (and a drag they took), blocks shutdown on unsaved work, may quit.
    pub(crate) fn sweep_closed_windows(&mut self) {
        for (window, prompts) in self.windows.sweep_closed() {
            self.keytips.hide_owner(window);
            self.tooltips.window_closed(window);
            self.file_ops.on_window_closed(window, &prompts);
        }
        if let Some(drag) = self.tab_drag.take_if(|d| !d.is_alive(&self.windows)) {
            drag.end(&mut self.windows);
        }
        let unsaved = self
            .windows
            .any_dirty()
            .then_some("You have unsaved changes.");
        self.platform.set_block_shutdown(unsaved);
        if self.windows.is_empty() && !self.quit && self.file_ops.writes.in_flight() == 0 {
            self.request_quit();
        }
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
                    self.windows.apply_theme(self.shell.dark());
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

    fn evaluate_tooltip_for(&mut self, ix: usize) {
        let probe = self.windows[ix].tooltip_probe();
        self.tooltips.evaluate(probe, &self.shell);
    }
}

// ---- 3. Window events -----------------------------------------------------------------------

impl App {
    /// Does the app's part of a window event (tooltips, key tips, tab drags, files) and passes
    /// the event to the window; whether it was used.
    fn on_window_event(&mut self, id: WindowId, event: WindowEvent) -> bool {
        match &event {
            WindowEvent::Resized | WindowEvent::Moved => self.keytips.hide_owner(id),
            WindowEvent::Activated(true) => self.windows.set_active(id),
            WindowEvent::Activated(false) => {
                self.keytips.hide_owner(id);
                self.tooltips.window_deactivated(id);
            }
            WindowEvent::MouseMove { pressed_button, .. } => {
                let left = *pressed_button == Some(MouseButton::Left);
                self.drive_drag(|drag, windows, ops, shell| {
                    drag.on_mouse_move(left, windows, ops, shell)
                });
            }
            WindowEvent::MouseLeave => self.tooltips.on_mouse_leave(id),
            WindowEvent::MouseDown { .. } => self.tooltips.on_mouse_down(id),
            WindowEvent::MouseUp { button, .. } if *button == MouseButton::Left => {
                self.drive_drag(|drag, windows, _, _| drag.on_release(windows));
            }
            WindowEvent::KeyDown(key) => {
                self.tooltips.on_key_down();
                // Escape cancels a tab drag, unless a dialog or menu has the keyboard.
                let window = self.windows.get(id);
                let window_wants_it = window
                    .is_some_and(|w| matches!(w.mode(), InputMode::Dialog | InputMode::Popup));
                if self.tab_drag.as_ref().is_some_and(TabDrag::is_dragging)
                    && key.key == Key::Escape
                    && key.modifiers.none()
                    && !window_wants_it
                {
                    if let Some(drag) = self.tab_drag.take() {
                        drag.cancel(&mut self.windows, &self.shell);
                    }
                    return true;
                }
            }
            &WindowEvent::SizeMoveChanged(entering) => {
                self.drive_drag(|drag, windows, ops, shell| {
                    drag.on_size_move_changed(id, entering, windows, ops, shell)
                });
                return false;
            }
            WindowEvent::CloseRequested => {
                self.run_file_ops(|ops, host, io| {
                    ops.request(FileRequest::CloseWindow { window: id }, host, io);
                });
                return false;
            }
            _ => {}
        }
        if let WindowEvent::FilesDropped(paths) = event {
            if self.windows.get(id).is_some() {
                self.open_paths(id, paths);
            }
            return false;
        }
        let (moved, capture_lost) = (
            matches!(event, WindowEvent::MouseMove { .. }),
            matches!(event, WindowEvent::CaptureLost),
        );
        let handled = self.with_window(id, |w, cx| w.on_event(event, cx)) == Some(true);
        if moved && let Some(ix) = self.windows.ix(id) {
            self.evaluate_tooltip_for(ix);
        }
        if capture_lost {
            self.drive_drag(|drag, windows, _, _| drag.on_capture_lost(id, windows));
        }
        handled
    }

    fn with_window<R>(
        &mut self,
        window_id: WindowId,
        f: impl FnOnce(&mut MainWindow, &mut Cx<'_>) -> R,
    ) -> Option<R> {
        let win = self.windows.get_mut(window_id)?;
        let mut fx = Vec::new();
        let out = f(win, &mut self.shell.cx(&mut fx));
        self.run_effects(fx);
        Some(out)
    }
}

// ---- 4. Tab drags ---------------------------------------------------------------------------

impl App {
    /// Runs `f` on the tab drag, if one is going on, and ends the drag if `f` says to.
    fn drive_drag(&mut self, f: impl FnOnce(&mut TabDrag, &mut Windows, &FileOps, &Shell) -> bool) {
        let (windows, ops, shell) = (&mut self.windows, &self.file_ops, &self.shell);
        if let Some(drag) = self.tab_drag.take_if(|d| f(d, windows, ops, shell)) {
            drag.end(windows);
        }
    }
}

// ---- 5. The platform's calls ----------------------------------------------------------------

impl PlatformHandler for App {
    fn on_event(&mut self, event: PlatformEvent) -> bool {
        let mut handled = false;
        match event {
            PlatformEvent::Window(id, event) => handled = self.on_window_event(id, event),
            PlatformEvent::SettingsChanged(settings) => {
                self.shell.apply_settings(settings);
                self.windows.apply_theme(self.shell.dark());
            }
            PlatformEvent::ModifiersChanged(modifiers) => {
                self.tooltips.on_modifiers_changed(modifiers);
            }
            PlatformEvent::Launch(paths) => {
                let _ = self.launch(paths, true);
            }
            PlatformEvent::SessionEnding => self.file_ops.writes.wait(Duration::from_secs(5)),
            PlatformEvent::Wake => {
                while let Some(event) = self.file_events.next() {
                    self.run_file_ops(|ops, host, io| ops.on_event(event, host, io));
                    self.sweep_closed_windows();
                }
            }
        }
        self.sweep_closed_windows();
        handled
    }

    /// Acts on what's due, paints what needs it, and says when to come back.
    fn drive_frames(&mut self) {
        self.sweep_closed_windows();
        if self.quit {
            return;
        }
        let now = Instant::now();
        for w in self.windows.iter_mut() {
            w.tick(now);
        }
        if let Some((target, _)) = self.tooltips.opening.filter(|&(_, at)| at <= now) {
            let probe = self
                .windows
                .get(target.window)
                .map(MainWindow::tooltip_probe);
            self.tooltips.on_show_timer(target, probe, &self.shell);
        }
        if self
            .tab_drag
            .as_ref()
            .and_then(|d| d.poll_at)
            .is_some_and(|at| at <= now)
        {
            self.drive_drag(TabDrag::on_poll);
            self.sweep_closed_windows();
        }
        let generation = self.shell.gpu.generation();
        let (mut any_vsync, mut painted_any) = (false, false);
        for ix in 0..self.windows.len() {
            if !self.windows[ix].dirty {
                continue;
            }
            painted_any = true;
            let w = &mut self.windows[ix];
            any_vsync |= w.render_frame(now, &self.shell);
            let (anchor, tips) = w.key_tip_request(&self.shell);
            let pointer = w.ui.pointer.is_some();
            self.keytips.sync(anchor, tips, &self.shell);
            if pointer {
                self.evaluate_tooltip_for(ix);
            }
        }
        if painted_any {
            self.shell.text_system.finish_frame();
        }
        any_vsync |= self.keytips.paint(&self.shell);
        any_vsync |= self.tooltips.paint(now, &self.shell);
        if self.shell.gpu.generation() != generation {
            self.windows.mark_all_dirty();
            any_vsync = true;
        }
        if any_vsync {
            self.platform.request_frame_tick();
        }
        let windows = self.windows.iter().filter_map(MainWindow::deadline);
        let tooltip = self.tooltips.opening.map(|(_, at)| at);
        let drag = self.tab_drag.as_ref().and_then(|drag| drag.poll_at);
        self.platform
            .wake_at(windows.chain(tooltip).chain(drag).min());
    }
}
