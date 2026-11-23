//! Clean-room `App`, `Context<T>`, `AsyncApp`, `AsyncWindowContext`, globals, observers,
//! subscriptions, and window handles.

use std::any::{Any, TypeId};
use std::borrow::{Borrow, BorrowMut};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use std::path::PathBuf;
use std::rc::{Rc, Weak};

use super::element::Render;
use super::entity::{AnyView, Entity, EntityId, EntityStore, WeakEntity};
use super::focus::FocusHandle;
use super::graphics::dwrite::TextSystem;
use super::keymap::{Command, KeyBinding, Keymap};
use super::platform::{
    DisplayId, PathPromptOptions, WindowAppearance, WindowOptions, WindowsDisplay, WindowsPlatform,
};
use super::runtime::{BackgroundExecutor, ForegroundExecutor, Task, futures::channel::oneshot};
use super::types::anyhow::{Context as _, Result};
use super::window::{KeyDownEvent, Window, WindowId};

/// Marker trait for singleton state stored in `App`.
pub(crate) trait Global: 'static {}

/// RAII handle that unregisters an observer/callback when dropped unless `.detach()`ed.
pub(crate) struct Subscription {
    unsubscribe: Option<Box<dyn FnOnce()>>,
}

impl Subscription {
    pub(crate) fn new(unsubscribe: impl FnOnce() + 'static) -> Self {
        Self {
            unsubscribe: Some(Box::new(unsubscribe)),
        }
    }

    pub(crate) fn detach(mut self) {
        self.unsubscribe = None;
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        if let Some(f) = self.unsubscribe.take() {
            f();
        }
    }
}

struct CallbackSlot<F: ?Sized> {
    active: Rc<Cell<bool>>,
    callback: F,
}

/// Type-erased handle to an open window.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct AnyWindowHandle {
    pub(crate) id: WindowId,
    pub(crate) root_type: TypeId,
}

impl AnyWindowHandle {
    pub(crate) fn window_id(self) -> WindowId {
        self.id
    }

    pub(crate) fn downcast<V: 'static>(self) -> Option<WindowHandle<V>> {
        (self.root_type == TypeId::of::<V>()).then_some(WindowHandle {
            id: self.id,
            _marker: PhantomData,
        })
    }

    pub(crate) fn update<R>(
        self,
        cx: &mut App,
        f: impl FnOnce(AnyView, &mut Window, &mut App) -> R,
    ) -> Result<R> {
        cx.update_window_impl(self.id, f)
    }
}

/// Strongly-typed handle to an open window whose root view is `V`.
pub(crate) struct WindowHandle<V> {
    pub(crate) id: WindowId,
    _marker: PhantomData<fn() -> V>,
}

impl<V> Copy for WindowHandle<V> {}
impl<V> Clone for WindowHandle<V> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<V> PartialEq for WindowHandle<V> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}
impl<V> Eq for WindowHandle<V> {}

impl<V: 'static> From<WindowHandle<V>> for AnyWindowHandle {
    fn from(h: WindowHandle<V>) -> Self {
        Self {
            id: h.id,
            root_type: TypeId::of::<V>(),
        }
    }
}

impl<V: 'static> WindowHandle<V> {
    #[cfg(test)]
    pub(crate) fn new(id: WindowId) -> Self {
        Self {
            id,
            _marker: PhantomData,
        }
    }

    pub(crate) fn read(self, cx: &App) -> Result<&V> {
        let view = cx
            .window_root(self.id)
            .context("window is closed")?
            .downcast::<V>()
            .context("window root type mismatch")?;
        Ok(view.read(cx))
    }

    pub(crate) fn update<R>(
        self,
        cx: &mut App,
        f: impl FnOnce(&mut V, &mut Window, &mut Context<V>) -> R,
    ) -> Result<R> {
        cx.update_window_impl(self.id, |root, window, app| {
            let entity = root
                .downcast::<V>()
                .expect("WindowHandle root type mismatch");
            entity.update(app, |this, cx| f(this, window, cx))
        })
    }
}

type EntityObserver = Box<dyn FnMut(&mut App) -> bool>;
type GlobalObserver = Box<dyn FnMut(&mut App) -> bool>;
type KeystrokeInterceptor = Box<dyn FnMut(&KeyDownEvent, &mut Window, &mut App) -> bool>;
type GlobalCommandHandler = Box<dyn FnMut(Command, &mut App) -> bool>;
type WindowClosedCallback = Box<dyn FnMut(&mut App, WindowId)>;
type DeferredAppCallback = Box<dyn FnOnce(&mut App)>;

pub(crate) struct AppCell {
    pub(crate) inner: RefCell<App>,
}

/// Central application state owning all entities, globals, windows, keymap, and executors.
pub(crate) struct App {
    pub(crate) self_weak: Weak<AppCell>,
    pub(crate) platform: Rc<WindowsPlatform>,
    pub(crate) text_system: Rc<TextSystem>,
    pub(crate) foreground: ForegroundExecutor,
    pub(crate) background: BackgroundExecutor,
    pub(crate) entities: EntityStore,
    pub(crate) globals: HashMap<TypeId, Option<Box<dyn Any>>>,
    pub(crate) keymap: Rc<RefCell<Keymap>>,
    pub(crate) reduce_motion: bool,
    pub(crate) appearance_override: Option<WindowAppearance>,
    pub(crate) windows: HashMap<WindowId, Option<Box<Window>>>,
    pub(crate) window_order: Vec<WindowId>,
    pub(crate) active_window: Option<WindowId>,
    pub(crate) dirty_entities: HashSet<EntityId>,
    dirty_globals: Vec<TypeId>,
    pending_focus_events: VecDeque<(WindowId, super::focus::FocusId, bool)>,
    pub(crate) deferred: VecDeque<DeferredAppCallback>,
    update_depth: usize,
    pub(crate) flushing: bool,
    entity_observers: HashMap<EntityId, Vec<CallbackSlot<EntityObserver>>>,
    global_observers: HashMap<TypeId, Vec<CallbackSlot<GlobalObserver>>>,
    keystroke_interceptors: Vec<CallbackSlot<KeystrokeInterceptor>>,
    global_commands: Vec<GlobalCommandHandler>,
    window_closed_listeners: Vec<CallbackSlot<WindowClosedCallback>>,
    pub(crate) quit_requested: bool,
    pub(crate) stop_propagation: bool,
}

impl App {
    pub(crate) fn new_rc(platform: Rc<WindowsPlatform>) -> Rc<AppCell> {
        let foreground = ForegroundExecutor::new();
        let background = BackgroundExecutor::new();
        let text_system = Rc::new(TextSystem::new());
        Rc::new_cyclic(|weak| AppCell {
            inner: RefCell::new(App {
                self_weak: weak.clone(),
                platform,
                text_system,
                foreground,
                background,
                entities: EntityStore::default(),
                globals: HashMap::new(),
                keymap: Rc::new(RefCell::new(Keymap::default())),
                reduce_motion: false,
                appearance_override: None,
                windows: HashMap::new(),
                window_order: Vec::new(),
                active_window: None,
                dirty_entities: HashSet::new(),
                dirty_globals: Vec::new(),
                pending_focus_events: VecDeque::new(),
                deferred: VecDeque::new(),
                update_depth: 0,
                flushing: false,
                entity_observers: HashMap::new(),
                global_observers: HashMap::new(),
                keystroke_interceptors: Vec::new(),
                global_commands: Vec::new(),
                window_closed_listeners: Vec::new(),
                quit_requested: false,
                stop_propagation: false,
            }),
        })
    }

    pub(crate) fn async_app(&self) -> AsyncApp {
        AsyncApp {
            app: self.self_weak.clone(),
            background: self.background.clone(),
        }
    }

    pub(crate) fn foreground_executor(&self) -> &ForegroundExecutor {
        &self.foreground
    }

    pub(crate) fn background_executor(&self) -> &BackgroundExecutor {
        &self.background
    }

    pub(crate) fn text_system(&self) -> &Rc<TextSystem> {
        &self.text_system
    }

    pub(crate) fn focus_handle(&self) -> FocusHandle {
        FocusHandle::new()
    }

    // ---- Entities ----

    #[allow(clippy::wrong_self_convention, clippy::new_ret_no_self)]
    pub(crate) fn new<T: 'static>(&mut self, build: impl FnOnce(&mut Context<T>) -> T) -> Entity<T> {
        let (id, token) = self.entities.reserve();
        self.update_depth += 1;
        let value = {
            let mut cx = Context {
                app: self,
                entity_id: id,
                token: token.clone(),
                _marker: PhantomData,
            };
            build(&mut cx)
        };
        self.entities.insert_value(id, value);
        self.update_depth -= 1;
        self.flush_effects();
        Entity::from_parts(id, token)
    }

    pub(crate) fn update_entity_impl<T: 'static, R>(
        &mut self,
        entity: &Entity<T>,
        f: impl FnOnce(&mut T, &mut Context<T>) -> R,
    ) -> R {
        let mut boxed = self.entities.take_box(entity.id).unwrap_or_else(|| {
            panic!(
                "re-entrant mutable borrow or dropped entity {:?} ({})",
                entity.id,
                std::any::type_name::<T>()
            )
        });
        let value = boxed
            .downcast_mut::<T>()
            .expect("Entity<T> slot type mismatch");
        self.update_depth += 1;
        let result = {
            let mut cx = Context {
                app: self,
                entity_id: entity.id,
                token: entity.token.clone(),
                _marker: PhantomData,
            };
            f(value, &mut cx)
        };
        self.entities.put_box(entity.id, boxed);
        self.update_depth -= 1;
        self.flush_effects();
        result
    }

    // ---- Globals ----

    pub(crate) fn has_global<G: Global>(&self) -> bool {
        self.globals
            .get(&TypeId::of::<G>())
            .is_some_and(Option::is_some)
    }

    pub(crate) fn global<G: Global>(&self) -> &G {
        self.try_global::<G>()
            .unwrap_or_else(|| panic!("missing global {}", std::any::type_name::<G>()))
    }

    pub(crate) fn try_global<G: Global>(&self) -> Option<&G> {
        self.globals
            .get(&TypeId::of::<G>())
            .and_then(|opt| opt.as_deref())
            .and_then(|any| any.downcast_ref::<G>())
    }

    pub(crate) fn global_mut<G: Global>(&mut self) -> &mut G {
        self.globals
            .get_mut(&TypeId::of::<G>())
            .and_then(|opt| opt.as_deref_mut())
            .and_then(|any| any.downcast_mut::<G>())
            .unwrap_or_else(|| panic!("missing global {}", std::any::type_name::<G>()))
    }

    pub(crate) fn default_global<G: Global + Default>(&mut self) -> &mut G {
        let entry = self
            .globals
            .entry(TypeId::of::<G>())
            .or_insert_with(|| Some(Box::new(G::default())));
        if entry.is_none() {
            *entry = Some(Box::new(G::default()));
        }
        entry
            .as_deref_mut()
            .unwrap()
            .downcast_mut::<G>()
            .unwrap()
    }

    pub(crate) fn set_global<G: Global>(&mut self, global: G) {
        self.globals
            .insert(TypeId::of::<G>(), Some(Box::new(global)));
        self.queue_global_notify::<G>();
        self.flush_effects();
    }

    pub(crate) fn remove_global<G: Global>(&mut self) -> G {
        let boxed = self
            .globals
            .remove(&TypeId::of::<G>())
            .flatten()
            .expect("remove_global called for missing global");
        *boxed.downcast::<G>().unwrap()
    }

    pub(crate) fn update_global<G: Global, R>(
        &mut self,
        f: impl FnOnce(&mut G, &mut App) -> R,
    ) -> R {
        let mut boxed = self
            .globals
            .get_mut(&TypeId::of::<G>())
            .and_then(Option::take)
            .unwrap_or_else(|| panic!("missing or leased global {}", std::any::type_name::<G>()));
        let g = boxed.downcast_mut::<G>().unwrap();
        self.update_depth += 1;
        let result = f(g, self);
        self.globals.insert(TypeId::of::<G>(), Some(boxed));
        self.update_depth -= 1;
        self.queue_global_notify::<G>();
        self.flush_effects();
        result
    }

    fn queue_global_notify<G: Global>(&mut self) {
        let type_id = TypeId::of::<G>();
        if !self.dirty_globals.contains(&type_id) {
            self.dirty_globals.push(type_id);
        }
        self.refresh_windows();
    }

    pub(crate) fn queue_focus_event(
        &mut self,
        window_id: WindowId,
        focus_id: super::focus::FocusId,
        focused_in: bool,
    ) {
        self.pending_focus_events
            .push_back((window_id, focus_id, focused_in));
    }

    // ---- Notifications & deferred work ----

    pub(crate) fn notify(&mut self, entity_id: EntityId) {
        self.dirty_entities.insert(entity_id);
        self.refresh_windows();
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

            while let Some((wid, fid, focused_in)) = self.pending_focus_events.pop_front() {
                ran_any = true;
                let _ = self.update_window_impl(wid, |_, window, cx| {
                    if focused_in {
                        window.fire_focus_in(fid, cx);
                    } else {
                        window.fire_focus_out(fid, cx);
                    }
                });
            }

            let globals: Vec<TypeId> = std::mem::take(&mut self.dirty_globals);
            for type_id in globals {
                if let Some(mut observers) = self.global_observers.remove(&type_id) {
                    ran_any = true;
                    observers.retain_mut(|slot| slot.active.get() && (slot.callback)(self));
                    let existing = self.global_observers.entry(type_id).or_default();
                    observers.append(existing);
                    *existing = observers;
                }
            }

            let dirty: Vec<EntityId> = self.dirty_entities.drain().collect();
            for id in dirty {
                if let Some(mut observers) = self.entity_observers.remove(&id) {
                    ran_any = true;
                    observers.retain_mut(|slot| slot.active.get() && (slot.callback)(self));
                    let entry = self.entity_observers.entry(id).or_default();
                    observers.append(entry);
                    *entry = observers;
                }
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

    // ---- Keymap, global actions, interceptors ----

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

    pub(crate) fn intercept_keystrokes(
        &mut self,
        mut f: impl FnMut(&KeyDownEvent, &mut Window, &mut App) + 'static,
    ) -> Subscription {
        let active = Rc::new(Cell::new(true));
        self.keystroke_interceptors.push(CallbackSlot {
            active: active.clone(),
            callback: Box::new(move |event, window, cx| {
                f(event, window, cx);
                true
            }),
        });
        Subscription::new(move || active.set(false))
    }

    pub(crate) fn run_keystroke_interceptors(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
    ) -> bool {
        let mut interceptors = std::mem::take(&mut self.keystroke_interceptors);
        self.stop_propagation = false;
        let mut stopped = false;
        interceptors.retain_mut(|slot| {
            if !slot.active.get() {
                return false;
            }
            if !stopped {
                (slot.callback)(event, window, self);
                if self.stop_propagation {
                    stopped = true;
                }
            }
            true
        });
        interceptors.append(&mut self.keystroke_interceptors);
        self.keystroke_interceptors = interceptors;
        self.stop_propagation = false;
        stopped
    }

    pub(crate) fn stop_propagation(&mut self) {
        self.stop_propagation = true;
    }

    // ---- Motion, appearance, clipboard, displays, dialogs ----

    pub(crate) fn reduce_motion(&self) -> bool {
        self.reduce_motion
    }

    pub(crate) fn set_reduce_motion(&mut self, reduce: bool) {
        self.reduce_motion = reduce;
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
                    let _ = cx.update_window_impl(id, |_, window, cx| {
                        window.apply_appearance(resolved, cx);
                    });
                }
            });
            return;
        }
        let ids: Vec<WindowId> = self.window_order.clone();
        for id in ids {
            let _ = self.update_window_impl(id, |_, window, cx| {
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
    ) -> oneshot::Receiver<Result<Option<Vec<PathBuf>>>> {
        super::platform::dialogs::prompt_for_paths(options)
    }

    // ---- Windows ----

    pub(crate) fn open_window<V: Render>(
        &mut self,
        options: WindowOptions,
        build_root: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
    ) -> Result<WindowHandle<V>> {
        let id = WindowId::next();
        let will_activate = options.show && options.focus;
        let mut window = Box::new(Window::new(id, options, self)?);
        window.set_root_type::<V>();
        self.update_depth += 1;
        let root_entity = build_root(&mut window, self);
        let root_view = AnyView::from_entity(root_entity);
        window.set_root_view(root_view);
        self.windows.insert(id, Some(window));
        self.window_order.push(id);
        if will_activate {
            self.active_window = Some(id);
        }
        self.update_depth -= 1;
        self.flush_effects();
        if self.update_depth == 0 {
            let _ = self.update_window_impl(id, |root, window, cx| {
                window.draw_frame(&root, cx);
            });
        }
        if let Some(Some(window)) = self.windows.get_mut(&id) {
            window.finish_open();
        }
        Ok(WindowHandle {
            id,
            _marker: PhantomData,
        })
    }

    pub(crate) fn windows(&self) -> Vec<AnyWindowHandle> {
        self.window_order
            .iter()
            .filter_map(|&id| {
                let win = self.windows.get(&id)?.as_ref()?;
                Some(win.window_handle())
            })
            .collect()
    }

    pub(crate) fn active_window(&self) -> Option<AnyWindowHandle> {
        let id = self.active_window?;
        self.windows
            .get(&id)?
            .as_ref()
            .map(|w| w.window_handle())
    }

    pub(crate) fn window_root(&self, id: WindowId) -> Option<AnyView> {
        self.windows.get(&id)?.as_ref()?.root_view().cloned()
    }

    pub(crate) fn update_window_impl<R>(
        &mut self,
        id: WindowId,
        f: impl FnOnce(AnyView, &mut Window, &mut App) -> R,
    ) -> Result<R> {
        let mut window = self
            .windows
            .get_mut(&id)
            .and_then(Option::take)
            .context("window is closed or currently borrowed")?;
        let root = window.root_view().cloned().context("window has no root view")?;
        self.update_depth += 1;
        let result = f(root, &mut window, self);
        let removed = window.take_removed_flag();
        if !removed {
            if let Some(slot) = self.windows.get_mut(&id) {
                *slot = Some(window);
            }
            self.update_depth -= 1;
        } else {
            self.windows.remove(&id);
            self.window_order.retain(|&w| w != id);
            if self.active_window == Some(id) {
                self.active_window = None;
            }
            window.destroy_native();
            self.update_depth -= 1;
            self.fire_window_closed(id);
        }
        self.flush_effects();
        Ok(result)
    }

    pub(crate) fn remove_window_by_id(&mut self, id: WindowId) {
        if let Some(Some(mut window)) = self.windows.remove(&id) {
            self.window_order.retain(|&w| w != id);
            if self.active_window == Some(id) {
                self.active_window = None;
            }
            window.destroy_native();
            self.fire_window_closed(id);
        }
    }

    pub(crate) fn refresh_windows(&mut self) {
        for window in self.windows.values_mut().flatten() {
            window.mark_dirty();
        }
    }

    pub(crate) fn on_window_closed(
        &mut self,
        f: impl FnMut(&mut App, WindowId) + 'static,
    ) -> Subscription {
        let active = Rc::new(Cell::new(true));
        self.window_closed_listeners.push(CallbackSlot {
            active: active.clone(),
            callback: Box::new(f),
        });
        Subscription::new(move || active.set(false))
    }

    pub(crate) fn fire_window_closed(&mut self, id: WindowId) {
        let mut listeners = std::mem::take(&mut self.window_closed_listeners);
        listeners.retain_mut(|slot| {
            if !slot.active.get() {
                return false;
            }
            (slot.callback)(self, id);
            true
        });
        listeners.append(&mut self.window_closed_listeners);
        self.window_closed_listeners = listeners;
    }

    pub(crate) fn spawn<AsyncFn, R>(&self, f: AsyncFn) -> Task<R>
    where
        AsyncFn: AsyncFnOnce(&mut AsyncApp) -> R + 'static,
        R: 'static,
    {
        let mut async_app = self.async_app();
        self.foreground.spawn(async move { f(&mut async_app).await })
    }

    pub(crate) fn quit(&mut self) {
        self.quit_requested = true;
        self.platform.post_quit();
    }

    pub(crate) fn add_entity_observer(
        &mut self,
        entity_id: EntityId,
        active: Rc<Cell<bool>>,
        callback: EntityObserver,
    ) {
        self.entity_observers
            .entry(entity_id)
            .or_default()
            .push(CallbackSlot { active, callback });
    }

    pub(crate) fn add_global_observer<G: Global>(
        &mut self,
        active: Rc<Cell<bool>>,
        callback: GlobalObserver,
    ) {
        self.global_observers
            .entry(TypeId::of::<G>())
            .or_default()
            .push(CallbackSlot { active, callback });
    }
}

/// Entity-scoped mutable context passed to `Render`, actions, and `Entity::update`.
pub(crate) struct Context<'a, T> {
    pub(crate) app: &'a mut App,
    pub(crate) entity_id: EntityId,
    pub(crate) token: Rc<()>,
    _marker: PhantomData<fn() -> T>,
}

impl<T> Deref for Context<'_, T> {
    type Target = App;

    #[inline]
    fn deref(&self) -> &App {
        self.app
    }
}

impl<T> DerefMut for Context<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut App {
        self.app
    }
}

impl<T> Borrow<App> for Context<'_, T> {
    #[inline]
    fn borrow(&self) -> &App {
        self.app
    }
}

impl<T> BorrowMut<App> for Context<'_, T> {
    #[inline]
    fn borrow_mut(&mut self) -> &mut App {
        self.app
    }
}

impl<T: 'static> Context<'_, T> {
    #[inline]
    pub(crate) fn entity(&self) -> Entity<T> {
        Entity::from_parts(self.entity_id, self.token.clone())
    }

    #[inline]
    pub(crate) fn weak_entity(&self) -> WeakEntity<T> {
        self.entity().downgrade()
    }

    pub(crate) fn notify(&mut self) {
        self.app.notify(self.entity_id);
    }

    pub(crate) fn listener<E: ?Sized>(
        &self,
        mut f: impl FnMut(&mut T, &E, &mut Window, &mut Context<T>) + 'static,
    ) -> impl FnMut(&E, &mut Window, &mut App) + 'static {
        let weak = self.weak_entity();
        move |event, window, app| {
            let _ = weak.update(app, |this, cx| f(this, event, window, cx));
        }
    }

    pub(crate) fn command_listener(
        &self,
        mut f: impl FnMut(&mut T, Command, &mut Window, &mut Context<T>) -> bool + 'static,
    ) -> impl FnMut(Command, &mut Window, &mut App) -> bool + 'static {
        let weak = self.weak_entity();
        move |command, window, app| {
            weak.update(app, |this, cx| f(this, command, window, cx))
                .unwrap_or(false)
        }
    }

    pub(crate) fn spawn<AsyncFn, R>(&self, f: AsyncFn) -> Task<R>
    where
        AsyncFn: AsyncFnOnce(WeakEntity<T>, &mut AsyncApp) -> R + 'static,
        R: 'static,
    {
        let weak = self.weak_entity();
        let mut async_app = self.app.async_app();
        self.app
            .foreground
            .spawn(async move { f(weak, &mut async_app).await })
    }

    pub(crate) fn spawn_in<AsyncFn, R>(&self, window: &Window, f: AsyncFn) -> Task<R>
    where
        AsyncFn: AsyncFnOnce(WeakEntity<T>, &mut AsyncWindowContext) -> R + 'static,
        R: 'static,
    {
        let weak = self.weak_entity();
        let mut async_win = AsyncWindowContext {
            app: self.app.self_weak.clone(),
            window_id: window.window_id(),
            background: self.app.background.clone(),
        };
        self.app
            .foreground
            .spawn(async move { f(weak, &mut async_win).await })
    }

    pub(crate) fn observe<Observed: 'static>(
        &mut self,
        observed: &Entity<Observed>,
        mut f: impl FnMut(&mut T, Entity<Observed>, &mut Context<T>) + 'static,
    ) -> Subscription {
        let observer = self.weak_entity();
        let observed_weak = observed.downgrade();
        let active = Rc::new(Cell::new(true));
        self.app.add_entity_observer(
            observed.entity_id(),
            active.clone(),
            Box::new(move |app| {
                let (Some(obs), Some(target)) = (observer.upgrade(), observed_weak.upgrade()) else {
                    return false;
                };
                obs.update(app, |this, cx| f(this, target, cx));
                true
            }),
        );
        Subscription::new(move || active.set(false))
    }

    pub(crate) fn observe_global_in<G: Global>(
        &mut self,
        window: &Window,
        mut f: impl FnMut(&mut T, &mut Window, &mut Context<T>) + 'static,
    ) -> Subscription {
        let observer = self.weak_entity();
        let window_id = window.window_id();
        let active = Rc::new(Cell::new(true));
        self.app.add_global_observer::<G>(
            active.clone(),
            Box::new(move |app| {
                let Some(obs) = observer.upgrade() else {
                    return false;
                };
                app.update_window_impl(window_id, |_, window, app| {
                    obs.update(app, |this, cx| f(this, window, cx));
                })
                .is_ok()
            }),
        );
        Subscription::new(move || active.set(false))
    }

    pub(crate) fn observe_window_activation(
        &mut self,
        window: &mut Window,
        mut f: impl FnMut(&mut T, &mut Window, &mut Context<T>) + 'static,
    ) -> Subscription {
        let observer = self.weak_entity();
        window.on_activation_change(move |window, app| {
            let Some(obs) = observer.upgrade() else {
                return false;
            };
            obs.update(app, |this, cx| f(this, window, cx));
            true
        })
    }

    pub(crate) fn observe_window_appearance(
        &mut self,
        window: &mut Window,
        mut f: impl FnMut(&mut T, &mut Window, &mut Context<T>) + 'static,
    ) -> Subscription {
        let observer = self.weak_entity();
        window.on_appearance_change(move |window, app| {
            let Some(obs) = observer.upgrade() else {
                return false;
            };
            obs.update(app, |this, cx| f(this, window, cx));
            true
        })
    }

    pub(crate) fn observe_window_bounds(
        &mut self,
        window: &mut Window,
        mut f: impl FnMut(&mut T, &mut Window, &mut Context<T>) + 'static,
    ) -> Subscription {
        let observer = self.weak_entity();
        window.on_bounds_change(move |window, app| {
            let Some(obs) = observer.upgrade() else {
                return false;
            };
            obs.update(app, |this, cx| f(this, window, cx));
            true
        })
    }

    pub(crate) fn on_focus(
        &mut self,
        focus: &FocusHandle,
        window: &mut Window,
        mut f: impl FnMut(&mut T, &mut Window, &mut Context<T>) + 'static,
    ) -> Subscription {
        let observer = self.weak_entity();
        window.on_focus_in(focus.id(), move |window, app| {
            let Some(obs) = observer.upgrade() else {
                return false;
            };
            obs.update(app, |this, cx| f(this, window, cx));
            true
        })
    }

    pub(crate) fn on_blur(
        &mut self,
        focus: &FocusHandle,
        window: &mut Window,
        mut f: impl FnMut(&mut T, &mut Window, &mut Context<T>) + 'static,
    ) -> Subscription {
        let observer = self.weak_entity();
        window.on_focus_out(focus.id(), move |window, app| {
            let Some(obs) = observer.upgrade() else {
                return false;
            };
            obs.update(app, |this, cx| f(this, window, cx));
            true
        })
    }
}

/// Async handle to `App` usable inside foreground `cx.spawn` futures.
#[derive(Clone)]
pub(crate) struct AsyncApp {
    pub(crate) app: Weak<AppCell>,
    pub(crate) background: BackgroundExecutor,
}

impl AsyncApp {
    pub(crate) fn background_executor(&self) -> &BackgroundExecutor {
        &self.background
    }

    pub(crate) fn update<R>(&self, f: impl FnOnce(&mut App) -> R) -> R {
        let cell = self.app.upgrade().expect("Application dropped");
        let mut app = cell.inner.borrow_mut();
        let result = f(&mut app);
        app.flush_effects();
        result
    }
}

/// Async handle bound to a specific window, usable inside `cx.spawn_in` futures.
#[derive(Clone)]
pub(crate) struct AsyncWindowContext {
    pub(crate) app: Weak<AppCell>,
    pub(crate) window_id: WindowId,
    pub(crate) background: BackgroundExecutor,
}

impl AsyncWindowContext {
    pub(crate) fn background_executor(&self) -> &BackgroundExecutor {
        &self.background
    }

    pub(crate) fn update<R>(&self, f: impl FnOnce(&mut Window, &mut App) -> R) -> Result<R> {
        let cell = self.app.upgrade().context("Application dropped")?;
        let mut app = cell.inner.borrow_mut();
        app.update_window_impl(self.window_id, |_, window, app| f(window, app))
    }
}

pub(crate) trait AppContext {
    #[allow(clippy::wrong_self_convention, clippy::new_ret_no_self)]
    fn new<T: 'static>(&mut self, build: impl FnOnce(&mut Context<T>) -> T) -> Entity<T>;
}

impl AppContext for App {
    fn new<T: 'static>(&mut self, build: impl FnOnce(&mut Context<T>) -> T) -> Entity<T> {
        App::new(self, build)
    }
}

impl<V: 'static> AppContext for Context<'_, V> {
    fn new<T: 'static>(&mut self, build: impl FnOnce(&mut Context<T>) -> T) -> Entity<T> {
        App::new(self.app, build)
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
