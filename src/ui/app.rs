//! Clean-room `App` owning windows, concrete `WindowContent` roots, shared app state, keymap,
//! and jobs/timers directly without an entity store or `Context<T>` wrappers.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use crate::win32::WindowHwnd;

use super::focus::{FocusHandle, FocusId};
use super::graphics::dwrite::TextSystem;
use super::jobs::{Jobs, TimerHandle, UiPoster};
use super::keymap::{Command, KeyBinding, Keymap};
use super::platform::dialogs::SavePathResult;
use super::platform::{
    DisplayId, PathPromptOptions, SavePathOptions, WindowAppearance, WindowOptions,
    WindowsDisplay, WindowsPlatform,
};
use super::types::{UiError, UiResult};
use super::window::{Window, WindowId};

type GlobalCommandHandler = Box<dyn FnMut(Command, &mut App) -> bool>;
type WindowClosedCallback = Box<dyn FnMut(&mut App, WindowId)>;
type DeferredAppCallback = Box<dyn FnOnce(&mut App)>;

pub(crate) struct AppCell {
    pub(crate) inner: RefCell<App>,
}

/// Central application state owning all windows, typed window roots, shared state, keymap, and jobs.
pub(crate) struct App {
    pub(crate) platform: Rc<WindowsPlatform>,
    pub(crate) text_system: Rc<TextSystem>,
    pub(crate) jobs: Jobs,
    pub(crate) shared: crate::AppShared,
    pub(crate) keymap: Rc<RefCell<Keymap>>,
    pub(crate) reduce_motion: bool,
    system_reduce_motion_applied: Option<bool>,
    pub(crate) appearance_override: Option<WindowAppearance>,
    pub(crate) windows: HashMap<WindowId, Option<Box<Window>>>,
    contents: HashMap<WindowId, Option<crate::WindowContent>>,
    pub(crate) window_order: Vec<WindowId>,
    pub(crate) active_window: Option<WindowId>,
    pending_dirty_windows: HashSet<WindowId>,
    pending_focus_changes: VecDeque<(WindowId, Option<FocusId>, Option<FocusId>)>,
    pub(crate) deferred: VecDeque<DeferredAppCallback>,
    update_depth: usize,
    pub(crate) flushing: bool,
    global_commands: Vec<GlobalCommandHandler>,
    window_closed_listeners: Vec<WindowClosedCallback>,
    pub(crate) quit_requested: bool,
    pub(crate) stop_propagation: bool,
}

impl App {
    pub(crate) fn new_rc(platform: Rc<WindowsPlatform>) -> Rc<AppCell> {
        let text_system = Rc::new(TextSystem::new());
        Rc::new(AppCell {
            inner: RefCell::new(App {
                platform,
                text_system,
                jobs: Jobs::new(),
                shared: crate::AppShared::default(),
                keymap: Rc::new(RefCell::new(Keymap::default())),
                reduce_motion: false,
                system_reduce_motion_applied: None,
                appearance_override: None,
                windows: HashMap::new(),
                contents: HashMap::new(),
                window_order: Vec::new(),
                active_window: None,
                pending_dirty_windows: HashSet::new(),
                pending_focus_changes: VecDeque::new(),
                deferred: VecDeque::new(),
                update_depth: 0,
                flushing: false,
                global_commands: Vec::new(),
                window_closed_listeners: Vec::new(),
                quit_requested: false,
                stop_propagation: false,
            }),
        })
    }

    pub(crate) fn ui_poster(&self) -> UiPoster {
        self.jobs.poster()
    }

    pub(crate) fn drain_jobs(&mut self) {
        Jobs::drain_into(self);
    }

    pub(crate) fn after(
        &self,
        delay: Duration,
        on_fire: impl FnOnce(&mut App) + 'static,
    ) -> TimerHandle {
        self.jobs.after(delay, on_fire)
    }

    pub(crate) fn after_in(
        &self,
        window_id: WindowId,
        delay: Duration,
        on_fire: impl FnOnce(&mut Window, &mut App) + 'static,
    ) -> TimerHandle {
        self.jobs.after(delay, move |cx| {
            let _ = cx.update_window(window_id, on_fire);
        })
    }

    pub(crate) fn every(
        &self,
        delay: Duration,
        interval: Duration,
        on_tick: impl FnMut(&mut App) -> bool + 'static,
    ) -> TimerHandle {
        self.jobs.every(delay, interval, on_tick)
    }

    #[cfg(test)]
    pub(crate) fn spawn_bg<R: Send + 'static>(
        &self,
        work: impl FnOnce() -> R + Send + 'static,
        on_done: impl FnOnce(R, &mut App) + 'static,
    ) {
        self.jobs.spawn_bg(work, on_done);
    }

    pub(crate) fn spawn_bg_in<R: Send + 'static>(
        &self,
        window_id: WindowId,
        work: impl FnOnce() -> R + Send + 'static,
        on_done: impl FnOnce(R, &mut Window, &mut App) + 'static,
    ) {
        self.jobs.spawn_bg(work, move |result, cx| {
            let _ = cx.update_window(window_id, |window, cx| on_done(result, window, cx));
        });
    }

    pub(crate) fn text_system(&self) -> &Rc<TextSystem> {
        &self.text_system
    }

    pub(crate) fn focus_handle(&self) -> FocusHandle {
        FocusHandle::new()
    }

    pub(crate) fn queue_focus_change(
        &mut self,
        window_id: WindowId,
        old_focus: Option<FocusId>,
        new_focus: Option<FocusId>,
    ) {
        self.pending_focus_changes
            .push_back((window_id, old_focus, new_focus));
    }

    pub(crate) fn defer(&mut self, f: impl FnOnce(&mut App) + 'static) {
        self.deferred.push_back(Box::new(f));
    }

    pub(crate) fn flush_effects(&mut self) {
        if self.flushing || self.update_depth > 0 {
            return;
        }
        self.flushing = true;
        loop {
            let mut ran_any = false;

            while let Some((wid, old_focus, new_focus)) = self.pending_focus_changes.pop_front() {
                ran_any = true;
                let _ = self.update_window(wid, |window, cx| {
                    window.fire_focus_change(old_focus, new_focus, cx);
                });
            }

            if let Some(deferred) = self.deferred.pop_front() {
                deferred(self);
                continue;
            }
            if !ran_any {
                break;
            }
        }
        self.flushing = false;
    }

    // ---- Keymap & global commands ----

    pub(crate) fn bind_keys(&mut self, bindings: impl IntoIterator<Item = KeyBinding>) {
        RefCell::borrow_mut(&self.keymap).add_bindings(bindings);
    }

    pub(crate) fn on_command(&mut self, f: impl FnMut(Command, &mut App) -> bool + 'static) {
        self.global_commands.push(Box::new(f));
    }

    pub(crate) fn dispatch_global_command(&mut self, command: Command) -> bool {
        let mut handlers = std::mem::take(&mut self.global_commands);
        let mut handled = false;
        for handler in handlers.iter_mut().rev() {
            if handler(command, self) {
                handled = true;
                break;
            }
        }
        handlers.append(&mut self.global_commands);
        self.global_commands = handlers;
        handled
    }

    pub(crate) fn stop_propagation(&mut self) {
        self.stop_propagation = true;
    }

    // ---- Motion, appearance, clipboard, displays, dialogs ----

    pub(crate) fn reduce_motion(&self) -> bool {
        self.reduce_motion
    }

    #[cfg(test)]
    pub(crate) fn set_reduce_motion(&mut self, reduce: bool) {
        self.reduce_motion = reduce;
    }

    /// Synchronizes `reduce_motion` with Windows' `SPI_GETCLIENTAREAANIMATION` preference unless
    /// an explicit caller override (such as tests) changed `reduce_motion` directly.
    pub(crate) fn sync_system_reduce_motion(&mut self) {
        let Some(animations_enabled) = crate::win32::client_area_animations_enabled() else {
            return;
        };
        let reduce = !animations_enabled;
        if self.reduce_motion != self.system_reduce_motion_applied.unwrap_or(false) {
            return;
        }
        self.reduce_motion = reduce;
        self.system_reduce_motion_applied = Some(reduce);
    }

    pub(crate) fn window_appearance(&self) -> WindowAppearance {
        self.appearance_override
            .unwrap_or_else(|| self.platform.system_appearance())
    }

    pub(crate) fn set_window_appearance(&mut self, appearance: Option<WindowAppearance>) {
        self.appearance_override = appearance;
        let resolved = self.window_appearance();
        if self.update_depth > 0 {
            self.defer(move |cx| {
                let ids: Vec<WindowId> = cx.window_order.clone();
                for id in ids {
                    let _ = cx.update_window(id, |window, cx| {
                        window.apply_appearance(resolved, cx);
                    });
                }
            });
            return;
        }
        let ids: Vec<WindowId> = self.window_order.clone();
        for id in ids {
            let _ = self.update_window(id, |window, cx| {
                window.apply_appearance(resolved, cx);
            });
        }
    }

    pub(crate) fn write_clipboard_text(&self, text: &str) {
        crate::win32::write_clipboard_text(text);
    }

    pub(crate) fn read_clipboard_text(&self) -> Option<String> {
        crate::win32::read_clipboard_text()
    }

    pub(crate) fn primary_display(&self) -> Option<WindowsDisplay> {
        WindowsDisplay::primary()
    }

    pub(crate) fn find_display(&self, id: DisplayId) -> Option<WindowsDisplay> {
        WindowsDisplay::from_id(id)
    }

    pub(crate) fn prompt_for_paths(
        &self,
        options: PathPromptOptions,
        on_done: impl FnOnce(UiResult<Option<Vec<PathBuf>>>, &mut App) + 'static,
    ) {
        self.jobs.spawn_dedicated(
            "jot-open-dialog",
            move || super::platform::dialogs::run_open_dialog(options),
            on_done,
        );
    }

    pub(crate) fn prompt_for_save_path(
        &self,
        owner: WindowHwnd,
        options: SavePathOptions,
        on_done: impl FnOnce(SavePathResult, &mut App) + 'static,
    ) {
        let owner_raw = owner.raw().0 as isize;
        self.jobs.spawn_dedicated(
            "jot-save-dialog",
            move || super::platform::dialogs::run_save_dialog(owner_raw, options),
            on_done,
        );
    }

    // ---- Windows & typed root content ----

    pub(crate) fn open_window(
        &mut self,
        options: WindowOptions,
        build: impl FnOnce(&mut Window, &mut App) -> crate::WindowContent,
    ) -> UiResult<WindowId> {
        let id = WindowId::next();
        let will_activate = options.show && options.focus;
        let mut window = Box::new(Window::new(id, options, self)?);
        self.update_depth += 1;
        let content = build(&mut window, self);
        self.windows.insert(id, Some(window));
        self.contents.insert(id, Some(content));
        self.window_order.push(id);
        if will_activate {
            self.active_window = Some(id);
        }
        self.update_depth -= 1;
        self.flush_effects();
        if self.update_depth == 0 {
            self.draw_window(id);
        }
        if let Some(Some(window)) = self.windows.get_mut(&id) {
            window.finish_open();
        }
        Ok(id)
    }

    pub(crate) fn main_windows(&self) -> Vec<WindowId> {
        self.window_order
            .iter()
            .copied()
            .filter(|id| {
                matches!(
                    self.contents.get(id),
                    Some(Some(crate::WindowContent::Main(_)))
                )
            })
            .collect()
    }

    pub(crate) fn active_window(&self) -> Option<WindowId> {
        let id = self.active_window?;
        self.windows.get(&id)?.as_ref().map(|_| id)
    }

    pub(crate) fn jot(&self, id: WindowId) -> Option<&crate::Jot> {
        match self.contents.get(&id)?.as_ref()? {
            crate::WindowContent::Main(jot) => Some(jot),
            _ => None,
        }
    }

    pub(crate) fn with_jot<R>(
        &mut self,
        id: WindowId,
        f: impl FnOnce(&mut crate::Jot, &mut App) -> R,
    ) -> Option<R> {
        let slot = self.contents.get_mut(&id)?;
        if !matches!(slot, Some(crate::WindowContent::Main(_))) {
            return None;
        }
        let mut content = slot.take()?;
        self.update_depth += 1;
        let result = match &mut content {
            crate::WindowContent::Main(jot) => f(jot, self),
            _ => unreachable!(),
        };
        if let Some(slot) = self.contents.get_mut(&id) {
            *slot = Some(content);
        }
        self.update_depth -= 1;
        self.flush_effects();
        Some(result)
    }

    pub(crate) fn update_jot_window<R>(
        &mut self,
        id: WindowId,
        f: impl FnOnce(&mut crate::Jot, &mut Window, &mut App) -> R,
    ) -> Option<R> {
        self.update_window(id, |window, cx| cx.with_jot(id, |jot, cx| f(jot, window, cx)))
            .ok()
            .flatten()
    }

    pub(crate) fn with_tooltip<R>(
        &mut self,
        id: WindowId,
        f: impl FnOnce(&mut crate::tooltip::TooltipView, &mut App) -> R,
    ) -> Option<R> {
        let slot = self.contents.get_mut(&id)?;
        if !matches!(slot, Some(crate::WindowContent::Tooltip(_))) {
            return None;
        }
        let mut content = slot.take()?;
        self.update_depth += 1;
        let result = match &mut content {
            crate::WindowContent::Tooltip(view) => f(view, self),
            _ => unreachable!(),
        };
        if let Some(slot) = self.contents.get_mut(&id) {
            *slot = Some(content);
        }
        self.update_depth -= 1;
        self.flush_effects();
        Some(result)
    }

    pub(crate) fn keytips_view(&self, id: WindowId) -> Option<&crate::keytips::KeyTipView> {
        match self.contents.get(&id)?.as_ref()? {
            crate::WindowContent::KeyTips(view) => Some(view),
            _ => None,
        }
    }

    pub(crate) fn with_keytips<R>(
        &mut self,
        id: WindowId,
        f: impl FnOnce(&mut crate::keytips::KeyTipView, &mut App) -> R,
    ) -> Option<R> {
        let slot = self.contents.get_mut(&id)?;
        if !matches!(slot, Some(crate::WindowContent::KeyTips(_))) {
            return None;
        }
        let mut content = slot.take()?;
        self.update_depth += 1;
        let result = match &mut content {
            crate::WindowContent::KeyTips(view) => f(view, self),
            _ => unreachable!(),
        };
        if let Some(slot) = self.contents.get_mut(&id) {
            *slot = Some(content);
        }
        self.update_depth -= 1;
        self.flush_effects();
        Some(result)
    }

    fn finish_window_lease(&mut self, id: WindowId, mut window: Box<Window>) {
        let removed = window.take_removed_flag();
        if !removed {
            if self.pending_dirty_windows.remove(&id) {
                window.mark_dirty();
            }
            if let Some(slot) = self.windows.get_mut(&id) {
                *slot = Some(window);
            }
            self.update_depth -= 1;
        } else {
            self.windows.remove(&id);
            self.contents.remove(&id);
            self.pending_dirty_windows.remove(&id);
            self.window_order.retain(|&w| w != id);
            if self.active_window == Some(id) {
                self.active_window = None;
            }
            window.destroy_native();
            self.update_depth -= 1;
            self.fire_window_closed(id);
        }
        self.flush_effects();
    }

    pub(crate) fn update_window<R>(
        &mut self,
        id: WindowId,
        f: impl FnOnce(&mut Window, &mut App) -> R,
    ) -> UiResult<R> {
        let mut window = self
            .windows
            .get_mut(&id)
            .and_then(Option::take)
            .ok_or_else(|| UiError::new("window is closed or currently borrowed"))?;
        self.update_depth += 1;
        let result = f(&mut window, self);
        self.finish_window_lease(id, window);
        Ok(result)
    }

    pub(crate) fn draw_window(&mut self, id: WindowId) {
        let Some(mut window) = self.windows.get_mut(&id).and_then(Option::take) else {
            return;
        };
        self.update_depth += 1;
        if window.prepare_frame()
            && let Some(mut content) = self.contents.get_mut(&id).and_then(Option::take)
        {
            let root = content.render(&mut window, self);
            // Return content before layout/prepaint/paint so custom widgets can borrow `Jot`
            // directly from `App` without any entity store or `RefCell`.
            if let Some(slot) = self.contents.get_mut(&id) {
                *slot = Some(content);
            }
            window.finish_frame(root, self);
        }
        self.finish_window_lease(id, window);
    }

    pub(crate) fn remove_window_by_id(&mut self, id: WindowId) {
        if let Some(Some(mut window)) = self.windows.remove(&id) {
            self.contents.remove(&id);
            self.pending_dirty_windows.remove(&id);
            self.window_order.retain(|&w| w != id);
            if self.active_window == Some(id) {
                self.active_window = None;
            }
            window.destroy_native();
            self.fire_window_closed(id);
        }
    }

    pub(crate) fn refresh_window(&mut self, id: WindowId) {
        if let Some(Some(window)) = self.windows.get_mut(&id) {
            window.mark_dirty();
        } else if self.windows.contains_key(&id) {
            self.pending_dirty_windows.insert(id);
        }
    }

    pub(crate) fn refresh_windows(&mut self) {
        for (&id, slot) in &mut self.windows {
            if let Some(window) = slot.as_mut() {
                window.mark_dirty();
            } else {
                self.pending_dirty_windows.insert(id);
            }
        }
    }

    pub(crate) fn on_window_closed(&mut self, f: impl FnMut(&mut App, WindowId) + 'static) {
        self.window_closed_listeners.push(Box::new(f));
    }

    pub(crate) fn fire_window_closed(&mut self, id: WindowId) {
        let mut listeners = std::mem::take(&mut self.window_closed_listeners);
        for cb in &mut listeners {
            cb(self, id);
        }
        listeners.append(&mut self.window_closed_listeners);
        self.window_closed_listeners = listeners;
    }

    pub(crate) fn quit(&mut self) {
        self.quit_requested = true;
        self.platform.post_quit();
    }
}

/// Top-level application runner owning the platform and message loop.
pub(crate) struct Application {
    platform: Rc<WindowsPlatform>,
}

impl Application {
    pub(crate) fn with_platform(platform: Rc<WindowsPlatform>) -> Self {
        Self { platform }
    }

    pub(crate) fn run(self, on_ready: impl FnOnce(&mut App) + 'static) {
        let app_cell = App::new_rc(self.platform.clone());
        self.platform.run(app_cell, on_ready);
    }
}
