mod tab_drag;
mod windows;

use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::files::{FileIo, FileOps, FileRequest, spawn_file_io};
use crate::geom::Size;
use crate::gfx::Gpu;
use crate::ids::EditorSlot;
use crate::keytips::KeyTips;
use crate::platform::{
    Key, MouseButton, Platform, PlatformEvent, PlatformHandler, WindowHwnd, WindowId,
};
use crate::shell::{AppEvent, Cx, Effect, Shell, WindowTimer};
use crate::tabs::TabData;
use crate::tooltip::{TooltipTarget, Tooltips};
use crate::window::{InputMode, MainWindow};

use tab_drag::TabDrag;
use windows::{OpenWindowError, Windows};

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
    pub(crate) fn new(platform: Rc<Platform>, gpu: Rc<Gpu>) -> Self {
        let shell = Shell::new(&platform, gpu);
        let tooltips = Tooltips::new(shell.settings.mouse_hover_time);
        Self {
            windows: Windows::new(),
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
        f: impl FnOnce(&mut FileOps, &mut windows::WindowsHost<'_>, &mut Vec<FileIo>),
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
                self.file_ops.tracker(),
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
                win.mark_dirty();
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
            self.windows.mark_dirty(drag.owner());
            if drag.source() != drag.owner() {
                self.windows.mark_dirty(drag.source());
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
        self.windows.mark_dirty(drag.owner());
        if drag.source() != drag.owner() {
            self.windows.mark_dirty(drag.source());
        }
        self.notify_closed(closed);
    }

    pub(crate) fn open_window(
        &mut self,
        initial: Option<TabData>,
        window_size: Option<Size>,
    ) -> std::result::Result<(WindowId, WindowHwnd), Box<OpenWindowError>> {
        self.windows.open(initial, window_size, &self.shell)
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
        tab: crate::ids::TabId,
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
                self.tooltips.set_hover_time(settings.mouse_hover_time);
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
                #[cfg(test)]
                AppEvent::TestCloseWindow(id) => {
                    if let Some(w) = self.windows.get_mut(id) {
                        w.request_close();
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
            if self.windows[ix].is_dirty() {
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
