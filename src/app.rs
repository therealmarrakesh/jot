use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

use crate::APP_NAME;
use crate::document::{self, DocPath, Format, PathKey};
use crate::editor::{EditorTimer, Rope};
use crate::files::{self, WorkflowGen};
use crate::geom::{Point, Rect, ScreenPoint, Size, point, size};
use crate::gfx::{Gpu, TextSystem};
use crate::ids::{EditorSlot, HitId};
use crate::keytips::KeyTips;
use crate::platform::{
    HandoffCommand, Jobs, Key, MouseButton, NativeWindow, Platform, PlatformEvent, PlatformHandler,
    SaveDialogResult, WindowAppearance, WindowBackdrop, WindowHwnd, WindowId, WindowKind,
    WindowOptions, WindowsDisplay, cursor_screen_pos, system_settings,
};
use crate::tab_drag::TabDragState;
use crate::tabs::{TabData, TabId};
use crate::theme::Palette;
use crate::tooltip::{TooltipTarget, Tooltips};
use crate::window::{MainWindow, WINDOW_HEIGHT, WINDOW_MAX_SCREEN_SHARE, WINDOW_WIDTH};

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
        results: Vec<(DocPath, Result<document::Loaded, document::LoadError>)>,
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
        doc_path: DocPath,
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

pub(crate) enum Effect {
    NewWindow,
    Exit,
    ToggleStatusBar,
    ToggleTheme,
    ToggleWordWrap,
    StartTabDrag {
        window: WindowId,
        tab: TabId,
        index: usize,
        grab: Point,
        position: Point,
    },
}

pub(crate) struct Cx<'a> {
    pub(crate) ts: &'a TextSystem,
    pub(crate) jobs: &'a Jobs<AppEvent>,
    pub(crate) view: ViewSettings,
    pub(crate) palette: Palette,
    pub(crate) reduce_motion: bool,
    fx: &'a mut Vec<Effect>,
}

impl Cx<'_> {
    #[inline]
    pub(crate) fn emit(&mut self, effect: Effect) {
        self.fx.push(effect);
    }
}

pub(crate) struct Shell {
    pub(crate) text_system: TextSystem,
    pub(crate) jobs: Jobs<AppEvent>,
    appearance_override: Option<WindowAppearance>,
    appearance: WindowAppearance,
    pub(crate) palette: Palette,
    pub(crate) reduce_motion: bool,
    pub(crate) view: ViewSettings,
    pub(crate) gpu: Rc<Gpu>,
}

impl Shell {
    fn cx<'a>(&'a self, fx: &'a mut Vec<Effect>) -> Cx<'a> {
        Cx {
            ts: &self.text_system,
            jobs: &self.jobs,
            view: self.view,
            palette: self.palette,
            reduce_motion: self.reduce_motion,
            fx,
        }
    }
}

pub(crate) struct App {
    pub(crate) windows: Vec<MainWindow>,
    pub(crate) active_window: Option<WindowId>,
    pub(crate) tab_drag: Option<TabDragState>,
    pub(crate) keytips: KeyTips,
    pub(crate) tooltips: Tooltips,
    in_flight_loads: HashSet<PathKey>,
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
        let jobs = Jobs::new();
        jobs.attach_platform(&platform);
        let settings = system_settings();
        let appearance = settings.appearance;
        let dark = matches!(appearance, WindowAppearance::Dark);
        let palette = Palette::resolve(dark, settings.accent(dark), settings.high_contrast);
        let reduce_motion = settings.reduce_motion;

        Self {
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
            shell: Shell {
                text_system: TextSystem::new(),
                jobs,
                appearance_override: None,
                appearance,
                palette,
                reduce_motion,
                view: ViewSettings::default(),
                gpu,
            },
            platform,
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
                    for ix in 0..self.windows.len() {
                        self.windows[ix].request_close_window(&self.shell.jobs);
                    }
                    self.sweep_closed_windows();
                }
                Effect::ToggleStatusBar => {
                    self.shell.view.status_bar = !self.shell.view.status_bar;
                    self.mark_all_dirty();
                }
                Effect::ToggleTheme => {
                    let next = if self.shell.palette.dark {
                        WindowAppearance::Light
                    } else {
                        WindowAppearance::Dark
                    };
                    self.shell.appearance_override = Some(next);
                    self.sync_theme();
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
        let (native, surface) = match NativeWindow::create(&opts, self.shell.appearance).and_then(|n| {
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
                crate::diag::debug_log(&format!("window/surface create failed: {message}"));
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

    pub(crate) fn open_paths_in_window(&mut self, window_id: WindowId, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        let mut to_load: Vec<DocPath> = Vec::new();
        let mut last_existing: Option<(usize, usize)> = None;
        for raw in paths {
            let path = std::path::absolute(&raw).unwrap_or(raw);
            let doc_path = DocPath::quick(path);
            if let Some(found) = self.find_open_tab_by_key(&doc_path.key) {
                last_existing = Some(found);
                continue;
            }
            if self.in_flight_loads.contains(&doc_path.key)
                || to_load.iter().any(|queued| queued.key == doc_path.key)
            {
                continue;
            }
            self.in_flight_loads.insert(doc_path.key.clone());
            to_load.push(doc_path);
        }
        if to_load.is_empty() {
            if let Some((wix, tix)) = last_existing {
                let hwnd = self.windows[wix].native.hwnd();
                self.windows[wix].activate_tab(tix, &self.shell.jobs);
                hwnd.bring_to_front();
            }
            return;
        }
        files::spawn_load_files(window_id, to_load, &self.shell.jobs);
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
        let settings = system_settings();
        self.shell.reduce_motion = settings.reduce_motion;
        let next = self
            .shell
            .appearance_override
            .unwrap_or(settings.appearance);
        let dark = matches!(next, WindowAppearance::Dark);
        self.shell.appearance = next;
        self.shell.palette = Palette::resolve(dark, settings.accent(dark), settings.high_contrast);
        for w in &mut self.windows {
            w.native.set_appearance(next);
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
                self.keytips.window_closed(wid);
                self.tooltips.window_closed(wid, &self.shell.jobs);
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
        if self.windows.is_empty() && !self.quit && crate::files::in_flight_writes() == 0 {
            self.request_quit();
        }
    }

    fn evaluate_tooltip_for(&mut self, ix: usize) {
        let probe = self.windows[ix].tooltip_probe();
        self.tooltips.evaluate(
            probe,
            &self.shell.gpu,
            self.shell.appearance,
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
            self.keytips.sync(
                wid,
                origin,
                scale,
                tips,
                &self.shell.gpu,
                self.shell.appearance,
            );
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
                        .is_some_and(|w| w.dialogs.is_open() || w.menu_bar.is_open())
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
                self.deliver(window_id, false, |w, cx| w.request_close_window(cx.jobs));
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
                        self.shell.appearance,
                        &self.shell.text_system,
                        &self.shell.jobs,
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
                                &self.shell.jobs,
                            );
                        }
                    }
                }
                AppEvent::FilesLoaded { window, results } => {
                    let wrap = self.shell.view.word_wrap;
                    for (doc_path, res) in results {
                        self.in_flight_loads.remove(&doc_path.key);
                        let dup = res
                            .as_ref()
                            .ok()
                            .and_then(|loaded| self.find_open_tab_by_key(&loaded.doc_path.key))
                            .or_else(|| self.find_open_tab_by_key(&doc_path.key));
                        if let Some((wix, tix)) = dup {
                            let hwnd = self.windows[wix].native.hwnd();
                            self.windows[wix].activate_tab(tix, &self.shell.jobs);
                            hwnd.bring_to_front();
                            continue;
                        }
                        if let Some(ix) = self.fallback_window_ix(window) {
                            self.windows[ix].adopt_loaded_file(
                                doc_path.path,
                                res,
                                wrap,
                                &self.shell.jobs,
                            );
                        }
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
                        let target_key = PathKey::quick(&path);
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
                                    &self.shell.jobs,
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
                                &self.shell.jobs,
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
                                &self.shell.jobs,
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
                                    &self.shell.jobs,
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
                                    &self.shell.jobs,
                                );
                            } else if let Some(ix) = self.window_ix(window) {
                                self.windows[ix].abort_file_workflow_if(wf_gen);
                            }
                        }
                        Ok(false) => {
                            files::spawn_write_file(
                                window,
                                wf_gen,
                                tab,
                                snapshot,
                                text,
                                path,
                                format,
                                &self.shell.jobs,
                            );
                        }
                    }
                }
                AppEvent::FileSaved {
                    window,
                    wf_gen,
                    tab,
                    snapshot,
                    doc_path,
                    format,
                    result,
                } => match result {
                    Ok(()) => {
                        let mut retriggered_gen = None;
                        if let Some((owner_wix, _)) = self.find_tab_owner(tab) {
                            retriggered_gen = self.windows[owner_wix].apply_tab_saved(
                                tab,
                                snapshot,
                                doc_path,
                                format,
                                window,
                                &self.shell.jobs,
                            );
                        }
                        if retriggered_gen != Some(wf_gen)
                            && let Some(origin_ix) = self.window_ix(window)
                        {
                            self.windows[origin_ix]
                                .advance_file_workflow_if(wf_gen, &self.shell.jobs);
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
                                files::save_error_prompt(doc_path.as_path(), &err),
                                &self.shell.jobs,
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
            self.shell.reduce_motion,
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
