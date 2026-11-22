use std::{
    any::{Any, TypeId, type_name},
    cell::{BorrowMutError, Ref, RefCell, RefMut},
    marker::PhantomData,
    mem,
    ops::{Deref, DerefMut},
    path::PathBuf,
    rc::{Rc, Weak},
    sync::{Arc, atomic::Ordering::SeqCst},
};

use crate::ui::anyhow::{Context as _, Result};
use crate::ui::futures::{FutureExt, channel::oneshot};
use crate::ui::slotmap::SlotMap;
use crate::ui::sync::RwLock;

pub(crate) use async_context::*;
use crate::ui::collections::{FxHashMap, FxHashSet, TypeIdHashMap, TypeIdHashSet, VecDeque};
use crate::ui::smallvec::SmallVec;
pub(crate) use context::*;
pub(crate) use entity_map::*;

use crate::ui::{
    Action, AnyView, AnyWindowHandle, AppContext, Arena, BackgroundExecutor, ClipboardItem,
    DispatchPhase, DisplayId, ExternalPaths, FocusHandle, FocusMap, ForegroundExecutor, Global,
    KeyBinding, Keymap, Keystroke, LayoutId, PathPromptOptions, Render, SubscriberSet,
    Subscription, Task, TextSystem, Window, WindowAppearance, WindowHandle, WindowId,
    WindowInvalidator, WindowsDisplay, WindowsPlatform,
};

mod async_context;
mod context;
mod entity_map;

/// Temporary(?) wrapper around [`RefCell<App>`] to help us debug any double borrows.
/// Strongly consider removing after stabilization.
#[doc(hidden)]
pub(crate) struct AppCell {
    app: RefCell<App>,
}

impl AppCell {
    #[doc(hidden)]
    #[track_caller]
    pub(crate) fn borrow(&self) -> AppRef<'_> {
        if option_env!("TRACK_THREAD_BORROWS").is_some() {
            let thread_id = std::thread::current().id();
            eprintln!("borrowed {thread_id:?}");
        }
        AppRef(self.app.borrow())
    }

    #[doc(hidden)]
    #[track_caller]
    pub(crate) fn borrow_mut(&self) -> AppRefMut<'_> {
        if option_env!("TRACK_THREAD_BORROWS").is_some() {
            let thread_id = std::thread::current().id();
            eprintln!("borrowed {thread_id:?}");
        }
        AppRefMut(self.app.borrow_mut())
    }

    #[doc(hidden)]
    #[track_caller]
    pub(crate) fn try_borrow_mut(&self) -> Result<AppRefMut<'_>, BorrowMutError> {
        if option_env!("TRACK_THREAD_BORROWS").is_some() {
            let thread_id = std::thread::current().id();
            eprintln!("borrowed {thread_id:?}");
        }
        Ok(AppRefMut(self.app.try_borrow_mut()?))
    }
}

#[doc(hidden)]
pub(crate) struct AppRef<'a>(Ref<'a, App>);

impl<'a> Deref for AppRef<'a> {
    type Target = Ref<'a, App>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for AppRef<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Drop for AppRef<'_> {
    fn drop(&mut self) {
        if option_env!("TRACK_THREAD_BORROWS").is_some() {
            let thread_id = std::thread::current().id();
            eprintln!("dropped borrow from {thread_id:?}");
        }
    }
}

#[doc(hidden)]
pub(crate) struct AppRefMut<'a>(RefMut<'a, App>);

impl<'a> Deref for AppRefMut<'a> {
    type Target = RefMut<'a, App>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for AppRefMut<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Drop for AppRefMut<'_> {
    fn drop(&mut self) {
        if option_env!("TRACK_THREAD_BORROWS").is_some() {
            let thread_id = std::thread::current().id();
            eprintln!("dropped {thread_id:?}");
        }
    }
}

/// A reference to a GPUI application, typically constructed in the `main` function of your app.
/// You won't interact with this type much outside of initial configuration and startup.
pub(crate) struct Application(Rc<AppCell>);

/// Represents an application before it is fully launched. Once your app is
/// configured, you'll start the app with `App::run`.
impl Application {
    /// Builds an app with the given Windows platform instance.
    pub(crate) fn with_platform(platform: Rc<WindowsPlatform>) -> Self {
        Self(App::new_app(platform))
    }

    /// Start the application. The provided callback will be called once the
    /// app is fully launched.
    pub(crate) fn run<F>(self, on_finish_launching: F)
    where
        F: 'static + FnOnce(&mut App),
    {
        let this = self.0.clone();
        let platform = self.0.borrow().platform.clone();
        platform.run(Box::new(move || {
            let cx = &mut *this.borrow_mut();
            on_finish_launching(cx);
        }));
    }
}

type Handler = Box<dyn FnMut(&mut App) -> bool + 'static>;
pub(crate) type KeystrokeObserver =
    Box<dyn FnMut(&KeystrokeEvent, &mut Window, &mut App) -> bool + 'static>;
type WindowClosedHandler = Box<dyn FnMut(&mut App, WindowId)>;

/// Contains the state of the full application, and passed as a reference to a variety of callbacks.
/// Other [Context] derefs to this type.
/// You need a reference to an `App` to access the state of a [Entity].
pub(crate) struct App {
    pub(crate) this: Weak<AppCell>,
    pub(crate) platform: Rc<WindowsPlatform>,
    text_system: Arc<TextSystem>,

    pub(crate) active_file_drop: Option<ExternalPaths>,
    pub(crate) background_executor: BackgroundExecutor,
    pub(crate) foreground_executor: ForegroundExecutor,
    pub(crate) entities: EntityMap,
    pub(crate) windows: SlotMap<WindowId, Option<Box<Window>>>,
    pub(crate) window_handles: FxHashMap<WindowId, AnyWindowHandle>,
    pub(crate) focus_handles: Arc<FocusMap>,
    pub(crate) keymap: Rc<RefCell<Keymap>>,
    pub(crate) global_action_listeners:
        TypeIdHashMap<Vec<Rc<dyn Fn(&dyn Any, DispatchPhase, &mut Self)>>>,
    pending_effects: VecDeque<Effect>,

    pub(crate) observers: SubscriberSet<EntityId, Handler>,
    pub(crate) keystroke_interceptors: SubscriberSet<(), KeystrokeObserver>,
    pub(crate) global_observers: SubscriberSet<TypeId, Handler>,
    pub(crate) window_closed_observers: SubscriberSet<(), WindowClosedHandler>,

    pub(crate) element_arena: RefCell<Arena>,

    pub(crate) globals_by_type: TypeIdHashMap<Box<dyn Any>>,

    pub(crate) pending_notifications: FxHashSet<EntityId>,
    pub(crate) pending_global_notifications: TypeIdHashSet,
    pub(crate) layout_id_buffer: Vec<LayoutId>,
    pub(crate) propagate_event: bool,
    pub(crate) window_invalidators_by_entity:
        FxHashMap<EntityId, FxHashMap<WindowId, WindowInvalidator>>,
    pub(crate) tracked_entities: FxHashMap<WindowId, FxHashSet<EntityId>>,
    pub(crate) current_window_by_entity: FxHashMap<EntityId, WindowId>,

    pub(crate) window_update_stack: Vec<WindowId>,
    pub(crate) reduce_motion: bool,
    pending_updates: usize,
}

impl App {
    #[allow(clippy::new_ret_no_self)]
    pub(crate) fn new_app(platform: Rc<WindowsPlatform>) -> Rc<AppCell> {
        let background_executor = platform.background_executor();
        let foreground_executor = platform.foreground_executor();
        assert!(
            background_executor.is_main_thread(),
            "must construct App on main thread"
        );

        let text_system = Arc::new(TextSystem::new(platform.text_system()));
        let entities = EntityMap::new();

        let app = Rc::new_cyclic(|this| AppCell {
            app: RefCell::new(App {
                this: this.clone(),
                platform: platform.clone(),
                text_system,
                pending_updates: 0,
                active_file_drop: None,
                background_executor,
                foreground_executor,
                globals_by_type: Default::default(),
                entities,
                windows: SlotMap::with_key(),
                window_update_stack: Vec::new(),
                window_handles: FxHashMap::default(),
                focus_handles: Arc::new(RwLock::new(SlotMap::with_key())),
                keymap: Rc::new(RefCell::new(Keymap::default())),
                global_action_listeners: Default::default(),
                pending_effects: VecDeque::new(),
                pending_notifications: FxHashSet::default(),
                pending_global_notifications: Default::default(),
                observers: SubscriberSet::new(),
                tracked_entities: FxHashMap::default(),
                window_invalidators_by_entity: FxHashMap::default(),
                current_window_by_entity: FxHashMap::default(),
                keystroke_interceptors: SubscriberSet::new(),
                global_observers: SubscriberSet::new(),
                window_closed_observers: SubscriberSet::new(),
                layout_id_buffer: Default::default(),
                propagate_event: true,
                reduce_motion: false,
                element_arena: RefCell::new(Arena::new(1024 * 1024)),
            }),
        });

        platform.on_quit(Box::new({
            let cx = Rc::downgrade(&app);
            move || {
                let Some(cx) = cx.upgrade() else {
                    return true;
                };
                match cx.try_borrow_mut() {
                    Ok(mut cx) => {
                        cx.shutdown();
                        true
                    }
                    Err(_) => false,
                }
            }
        }));

        app
    }

    /// Quit the application gracefully.
    pub(crate) fn shutdown(&mut self) {
        self.windows.clear();
        self.window_handles.clear();
        self.flush_effects();
    }

    /// Gracefully quit the application via the platform's standard routine.
    pub(crate) fn quit(&self) {
        self.platform.quit();
    }

    /// Returns whether non-essential animations (e.g. loading spinners) should
    /// be rendered in a static state instead of animating.
    pub(crate) fn reduce_motion(&self) -> bool {
        self.reduce_motion
    }

    /// Sets whether non-essential animations (e.g. loading spinners) should be
    /// rendered in a static state instead of animating.
    pub(crate) fn set_reduce_motion(&mut self, reduce_motion: bool) {
        if self.reduce_motion != reduce_motion {
            self.reduce_motion = reduce_motion;
            self.refresh_windows();
        }
    }

    /// Schedules all windows in the application to be redrawn. This can be called
    /// multiple times in an update cycle and still result in a single redraw.
    pub(crate) fn refresh_windows(&mut self) {
        self.pending_effects.push_back(Effect::RefreshWindows);
    }

    #[inline(always)]
    pub(crate) fn update<R>(&mut self, update: impl FnOnce(&mut Self) -> R) -> R {
        self.start_update();
        let result = update(self);
        self.finish_update();
        result
    }

    pub(crate) fn start_update(&mut self) {
        self.pending_updates += 1;
    }

    #[inline(never)]
    pub(crate) fn finish_update(&mut self) {
        if self.pending_updates == 1 {
            self.flush_effects();
        }
        self.pending_updates -= 1;
    }

    /// Arrange a callback to be invoked when the given entity calls `notify` on its respective context.
    pub(crate) fn observe<W>(
        &mut self,
        entity: &Entity<W>,
        mut on_notify: impl FnMut(Entity<W>, &mut App) + 'static,
    ) -> Subscription
    where
        W: 'static,
    {
        self.observe_internal(entity, move |e, cx| {
            on_notify(e, cx);
            true
        })
    }

    pub(crate) fn record_entities_accessed(
        &mut self,
        window_handle: AnyWindowHandle,
        invalidator: WindowInvalidator,
        entities: &FxHashSet<EntityId>,
    ) {
        let mut tracked_entities =
            std::mem::take(self.tracked_entities.entry(window_handle.id).or_default());
        for entity in tracked_entities.iter() {
            self.window_invalidators_by_entity
                .entry(*entity)
                .and_modify(|windows| {
                    windows.remove(&window_handle.id);
                });
        }
        for entity in entities.iter() {
            self.window_invalidators_by_entity
                .entry(*entity)
                .or_default()
                .insert(window_handle.id, invalidator.clone());
            self.current_window_by_entity
                .insert(*entity, window_handle.id);
        }
        tracked_entities.clear();
        tracked_entities.extend(entities.iter().copied());
        self.tracked_entities
            .insert(window_handle.id, tracked_entities);
    }

    pub(crate) fn new_observer(&mut self, key: EntityId, value: Handler) -> Subscription {
        let (subscription, activate) = self.observers.insert(key, value);
        self.defer(move |_| activate());
        subscription
    }

    pub(crate) fn observe_internal<W>(
        &mut self,
        entity: &Entity<W>,
        mut on_notify: impl FnMut(Entity<W>, &mut App) -> bool + 'static,
    ) -> Subscription
    where
        W: 'static,
    {
        let entity_id = entity.entity_id();
        let handle = entity.downgrade();
        self.new_observer(
            entity_id,
            Box::new(move |cx| {
                if let Some(entity) = handle.upgrade() {
                    on_notify(entity, cx)
                } else {
                    false
                }
            }),
        )
    }

    /// Returns handles to all open windows in the application.
    /// Each handle could be downcast to a handle typed for the root view of that window.
    /// To find all windows of a given type, you could filter on
    pub(crate) fn windows(&self) -> Vec<AnyWindowHandle> {
        self.windows
            .keys()
            .flat_map(|window_id| self.window_handles.get(&window_id).copied())
            .collect()
    }

    /// Returns a handle to the window that is currently focused at the platform level, if one exists.
    pub(crate) fn active_window(&self) -> Option<AnyWindowHandle> {
        self.platform.active_window()
    }

    /// Opens a new window with the given option and the root view returned by the given function.
    /// The function is invoked with a `Window`, which can be used to interact with window-specific
    /// functionality.
    pub(crate) fn open_window<V: 'static + Render>(
        &mut self,
        options: crate::ui::WindowOptions,
        build_root_view: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
    ) -> crate::ui::anyhow::Result<WindowHandle<V>> {
        self.update(|cx| {
            let id = cx.windows.insert(None);
            let handle = WindowHandle::new(id);
            match Window::new(handle.into(), options, cx) {
                Ok(mut window) => {
                    cx.window_update_stack.push(id);
                    let root_view = build_root_view(&mut window, cx);
                    cx.window_update_stack.pop();
                    window.root.replace(root_view.into());
                    window.defer(cx, |window: &mut Window, cx| window.appearance_changed(cx));

                    // allow a window to draw at least once before returning
                    // this didn't cause any issues on non windows platforms as it seems we always won the race to on_request_frame
                    // on windows we quite frequently lose the race and return a window that has never rendered, which leads to a crash
                    // where DispatchTree::root_node_id asserts on empty nodes
                    let clear = window.draw(cx);
                    clear.clear(cx);

                    cx.window_handles.insert(id, window.handle);
                    cx.windows.get_mut(id).unwrap().replace(Box::new(window));
                    Ok(handle)
                }
                Err(e) => {
                    cx.windows.remove(id);
                    Err(e)
                }
            }
        })
    }

    /// Instructs the platform to activate the application by bringing it to the foreground.
    pub(crate) fn activate(&self, ignoring_other_apps: bool) {
        self.platform.activate(ignoring_other_apps);
    }

    /// Returns the list of currently active displays.
    pub(crate) fn displays(&self) -> Vec<WindowsDisplay> {
        self.platform.displays()
    }

    /// Returns the primary display that will be used for new windows.
    pub(crate) fn primary_display(&self) -> Option<WindowsDisplay> {
        self.platform.primary_display()
    }

    /// Returns the display with the given ID, if one exists.
    pub(crate) fn find_display(&self, id: DisplayId) -> Option<WindowsDisplay> {
        self.displays()
            .into_iter()
            .find(|display| display.id() == id)
    }

    /// Returns the appearance of the application's windows.
    pub(crate) fn window_appearance(&self) -> WindowAppearance {
        self.platform.window_appearance()
    }

    /// Overrides the appearance (light/dark) applied to the app's windows.
    pub(crate) fn set_window_appearance(&self, appearance: Option<WindowAppearance>) {
        self.platform.set_window_appearance(appearance);
    }

    /// Reads data from the platform clipboard.
    pub(crate) fn read_from_clipboard(&self) -> Option<ClipboardItem> {
        self.platform.read_from_clipboard()
    }

    /// Writes data to the platform clipboard.
    pub(crate) fn write_to_clipboard(&self, item: ClipboardItem) {
        self.platform.write_to_clipboard(item)
    }

    /// Displays a platform modal for selecting paths.
    pub(crate) fn prompt_for_paths(
        &self,
        options: PathPromptOptions,
    ) -> oneshot::Receiver<Result<Option<Vec<PathBuf>>>> {
        self.platform.prompt_for_paths(options)
    }

    pub(crate) fn push_effect(&mut self, effect: Effect) {
        match &effect {
            Effect::Notify { emitter } => {
                if !self.pending_notifications.insert(*emitter) {
                    return;
                }
            }
            Effect::NotifyGlobalObservers { global_type }
                if !self.pending_global_notifications.insert(*global_type) =>
            {
                return;
            }
            _ => {}
        };

        self.pending_effects.push_back(effect);
    }

    /// Called at the end of [`App::update`] to complete any side effects
    /// such as notifying observers, etc. Effects can themselves
    /// cause effects, so we continue looping until all effects are processed.
    fn flush_effects(&mut self) {
        loop {
            self.release_dropped_entities();
            self.release_dropped_focus_handles();
            if let Some(effect) = self.pending_effects.pop_front() {
                match effect {
                    Effect::Notify { emitter } => {
                        self.apply_notify_effect(emitter);
                    }

                    Effect::RefreshWindows => {
                        self.apply_refresh_effect();
                    }

                    Effect::NotifyGlobalObservers { global_type } => {
                        self.apply_notify_global_observers_effect(global_type);
                    }

                    Effect::Defer { callback } => {
                        self.apply_defer_effect(callback);
                    }
                }
            } else if self.pending_effects.is_empty() {
                break;
            }
        }
    }

    /// Repeatedly called during `flush_effects` to release any entities whose
    /// reference count has become zero.
    fn release_dropped_entities(&mut self) {
        loop {
            let dropped = self.entities.take_dropped();
            if dropped.is_empty() {
                break;
            }

            for (entity_id, _entity) in dropped {
                self.observers.remove(&entity_id);
                self.window_invalidators_by_entity.remove(&entity_id);
                self.current_window_by_entity.remove(&entity_id);
            }
        }
    }

    /// Repeatedly called during `flush_effects` to handle a focused handle being dropped.
    fn release_dropped_focus_handles(&mut self) {
        self.focus_handles
            .clone()
            .write()
            .retain(|handle_id, focus| {
                if focus.ref_count.load(SeqCst) == 0 {
                    for window_handle in self.windows() {
                        window_handle
                            .update(self, |_, window, cx| {
                                if window.focus == Some(handle_id) {
                                    window.blur(cx);
                                }
                            })
                            .unwrap();
                    }
                    false
                } else {
                    true
                }
            });
    }

    fn apply_notify_effect(&mut self, emitter: EntityId) {
        self.pending_notifications.remove(&emitter);

        self.observers
            .clone()
            .retain(&emitter, |handler| handler(self));
    }

    fn apply_refresh_effect(&mut self) {
        for window in self.windows.values_mut() {
            if let Some(window) = window.as_deref_mut() {
                window.refreshing = true;
                window.invalidator.set_dirty(true);
            }
        }
    }

    fn apply_notify_global_observers_effect(&mut self, type_id: TypeId) {
        self.pending_global_notifications.remove(&type_id);
        self.global_observers
            .clone()
            .retain(&type_id, |observer| observer(self));
    }

    fn apply_defer_effect(&mut self, callback: Box<dyn FnOnce(&mut Self) + 'static>) {
        callback(self);
    }

    /// Run `f` against the entity's *current* window — the most recently
    /// rendered window that referenced the entity, or its creation window if
    /// it has yet to be rendered. Returns `None` if the entity has no
    /// current window, or if that window has been closed, or if it is
    /// already on the update stack.
    #[inline(always)]
    pub(crate) fn with_window<R>(
        &mut self,
        entity_id: EntityId,
        f: impl FnOnce(&mut Window, &mut App) -> R,
    ) -> Option<R> {
        let window_id = *self.current_window_by_entity.get(&entity_id)?;
        self.update_window_id(window_id, |_, window, cx| f(window, cx))
            .ok()
    }

    #[inline(always)]
    pub(crate) fn update_window_id<T, F>(&mut self, id: WindowId, update: F) -> Result<T>
    where
        F: FnOnce(AnyView, &mut Window, &mut App) -> T,
    {
        let mut update = Some(update);
        let mut result = None;
        self.update_window_erased(id, &mut |arguments| {
            if let Some((root_view, window, cx)) = arguments {
                result = Some(update.take().unwrap()(root_view, window, cx));
            } else {
                drop(update.take());
                drop(result.take());
            }
        });
        result.context("window not found")
    }

    /// Creates an `AsyncApp`, which can be cloned and has a static lifetime
    /// so it can be held across `await` points.
    pub(crate) fn to_async(&self) -> AsyncApp {
        AsyncApp {
            app: self.this.clone(),
            background_executor: self.background_executor.clone(),
            foreground_executor: self.foreground_executor.clone(),
        }
    }

    /// Obtains a reference to the executor, which can be used to spawn futures.
    pub(crate) fn background_executor(&self) -> &BackgroundExecutor {
        &self.background_executor
    }

    /// Obtains a reference to the executor, which can be used to spawn futures.
    pub(crate) fn foreground_executor(&self) -> &ForegroundExecutor {
        &self.foreground_executor
    }

    /// Spawns the future returned by the given function on the main thread. The closure will be invoked
    /// with [AsyncApp], which allows the application state to be accessed across await points.
    #[track_caller]
    #[inline(always)]
    pub(crate) fn spawn<AsyncFn, R>(&self, f: AsyncFn) -> Task<R>
    where
        AsyncFn: AsyncFnOnce(&mut AsyncApp) -> R + 'static,
        R: 'static,
    {
        let mut cx = self.to_async();

        self.foreground_executor
            .spawn(async move { f(&mut cx).await }.boxed_local())
    }

    /// Schedules the given function to be run at the end of the current effect cycle, allowing entities
    /// that are currently on the stack to be returned to the app.
    pub(crate) fn defer(&mut self, f: impl FnOnce(&mut App) + 'static) {
        self.push_effect(Effect::Defer {
            callback: Box::new(f),
        });
    }

    /// Accessor for the text system.
    pub(crate) fn text_system(&self) -> &Arc<TextSystem> {
        &self.text_system
    }

    /// Check whether a global of the given type has been assigned.
    pub(crate) fn has_global<G: Global>(&self) -> bool {
        self.globals_by_type.contains_key(&TypeId::of::<G>())
    }

    /// Access the global of the given type. Panics if a global for that type has not been assigned.
    #[track_caller]
    pub(crate) fn global<G: Global>(&self) -> &G {
        self.globals_by_type
            .get(&TypeId::of::<G>())
            .map(|any_state| any_state.downcast_ref::<G>().unwrap())
            .unwrap_or_else(|| panic!("no state of type {} exists", type_name::<G>()))
    }

    /// Access the global of the given type if a value has been assigned.
    pub(crate) fn try_global<G: Global>(&self) -> Option<&G> {
        self.globals_by_type
            .get(&TypeId::of::<G>())
            .map(|any_state| any_state.downcast_ref::<G>().unwrap())
    }

    /// Access the global of the given type mutably. Panics if a global for that type has not been assigned.
    #[track_caller]
    pub(crate) fn global_mut<G: Global>(&mut self) -> &mut G {
        let global_type = TypeId::of::<G>();
        self.push_effect(Effect::NotifyGlobalObservers { global_type });
        self.globals_by_type
            .get_mut(&global_type)
            .and_then(|any_state| any_state.downcast_mut::<G>())
            .unwrap_or_else(|| panic!("no state of type {} exists", type_name::<G>()))
    }

    /// Access the global of the given type mutably. A default value is assigned if a global of this type has not
    /// yet been assigned.
    pub(crate) fn default_global<G: Global + Default>(&mut self) -> &mut G {
        let global_type = TypeId::of::<G>();
        self.push_effect(Effect::NotifyGlobalObservers { global_type });
        self.globals_by_type
            .entry(global_type)
            .or_insert_with(|| Box::<G>::default())
            .downcast_mut::<G>()
            .unwrap()
    }

    /// Sets the value of the global of the given type.
    pub(crate) fn set_global<G: Global>(&mut self, global: G) {
        let global_type = TypeId::of::<G>();
        self.push_effect(Effect::NotifyGlobalObservers { global_type });
        self.globals_by_type.insert(global_type, Box::new(global));
    }

    /// Remove the global of the given type from the app context. Does not notify global observers.
    pub(crate) fn remove_global<G: Global>(&mut self) -> G {
        let global_type = TypeId::of::<G>();
        self.push_effect(Effect::NotifyGlobalObservers { global_type });
        *self
            .globals_by_type
            .remove(&global_type)
            .unwrap_or_else(|| panic!("no global added for {}", type_name::<G>()))
            .downcast()
            .unwrap()
    }

    /// Move the global of the given type to the stack.
    #[track_caller]
    pub(crate) fn lease_global<G: Global>(&mut self) -> GlobalLease<G> {
        GlobalLease::new(
            self.globals_by_type
                .remove(&TypeId::of::<G>())
                .with_context(|| format!("no global registered of type {}", type_name::<G>()))
                .unwrap(),
        )
    }

    /// Restore the global of the given type after it is moved to the stack.
    pub(crate) fn end_global_lease<G: Global>(&mut self, lease: GlobalLease<G>) {
        let global_type = TypeId::of::<G>();

        self.push_effect(Effect::NotifyGlobalObservers { global_type });
        self.globals_by_type.insert(global_type, lease.global);
    }

    /// Register a callback to be invoked when a keystroke is received by the application
    /// in any window. Note that this fires _before_ all other action and event mechanisms have resolved.
    /// `cx.stop_propagation` calls within interceptors will prevent action dispatch.
    pub(crate) fn intercept_keystrokes(
        &mut self,
        mut f: impl FnMut(&KeystrokeEvent, &mut Window, &mut App) + 'static,
    ) -> Subscription {
        fn inner(
            keystroke_interceptors: &SubscriberSet<(), KeystrokeObserver>,
            handler: KeystrokeObserver,
        ) -> Subscription {
            let (subscription, activate) = keystroke_interceptors.insert((), handler);
            activate();
            subscription
        }

        inner(
            &self.keystroke_interceptors,
            Box::new(move |event, window, cx| {
                f(event, window, cx);
                true
            }),
        )
    }

    /// Register key bindings.
    pub(crate) fn bind_keys(&mut self, bindings: impl IntoIterator<Item = KeyBinding>) {
        self.keymap.borrow_mut().add_bindings(bindings);
        self.pending_effects.push_back(Effect::RefreshWindows);
    }

    /// Register a global handler for actions invoked via the keyboard. These handlers are run at
    /// the end of the bubble phase for actions, and so will only be invoked if there are no other
    /// handlers or if they called `cx.propagate()`.
    pub(crate) fn on_action<A: Action>(
        &mut self,
        listener: impl Fn(&A, &mut Self) + 'static,
    ) -> &mut Self {
        self.global_action_listeners
            .entry(TypeId::of::<A>())
            .or_default()
            .push(Rc::new(move |action, phase, cx| {
                if phase == DispatchPhase::Bubble {
                    let action = action.downcast_ref().unwrap();
                    listener(action, cx)
                }
            }));
        self
    }

    /// Event handlers propagate events by default. Call this method to stop dispatching to
    /// event handlers with a lower z-index (mouse) or higher in the tree (keyboard). This is
    /// the opposite of [`Self::propagate`]. It's also possible to cancel a call to [`Self::propagate`] by
    /// calling this method before effects are flushed.
    pub(crate) fn stop_propagation(&mut self) {
        self.propagate_event = false;
    }

    /// Action handlers stop propagation by default during the bubble phase of action dispatch
    /// dispatching to action handlers higher in the element tree. This is the opposite of
    /// [`Self::stop_propagation`]. It's also possible to cancel a call to [`Self::stop_propagation`] by calling
    /// this method before effects are flushed.
    pub(crate) fn propagate(&mut self) {
        self.propagate_event = true;
    }

    /// Register a callback to be invoked when a window is closed
    /// The window is no longer accessible at the point this callback is invoked.
    pub(crate) fn on_window_closed(
        &self,
        on_closed: impl FnMut(&mut App, WindowId) + 'static,
    ) -> Subscription {
        let (subscription, activate) = self.window_closed_observers.insert((), Box::new(on_closed));
        activate();
        subscription
    }

    /// Obtain a new [`FocusHandle`], which allows you to track and manipulate the keyboard focus
    /// for elements rendered within this window.
    #[track_caller]
    pub(crate) fn focus_handle(&self) -> FocusHandle {
        FocusHandle::new(&self.focus_handles)
    }

    /// Tell GPUI that an entity has changed and observers of it should be notified.
    pub(crate) fn notify(&mut self, entity_id: EntityId) {
        let window_invalidators = mem::take(
            self.window_invalidators_by_entity
                .entry(entity_id)
                .or_default(),
        );

        // `window_invalidators_by_entity` is monotonic, so an entry alone
        // doesn't mean the window is currently rendering the entity. Filter
        // through `tracked_entities` to keep invalidation tight to windows
        // that actually display this entity right now.
        let live_invalidators: SmallVec<[WindowInvalidator; 2]> = window_invalidators
            .iter()
            .filter(|(window_id, _)| {
                self.tracked_entities
                    .get(window_id)
                    .is_some_and(|set| set.contains(&entity_id))
            })
            .map(|(_, invalidator)| invalidator.clone())
            .collect();

        if live_invalidators.is_empty() {
            if self.pending_notifications.insert(entity_id) {
                self.pending_effects
                    .push_back(Effect::Notify { emitter: entity_id });
            }
        } else {
            for invalidator in &live_invalidators {
                invalidator.invalidate_view(entity_id, self);
            }
        }

        self.window_invalidators_by_entity
            .insert(entity_id, window_invalidators);
    }

    #[inline(never)]
    fn update_window_erased(
        &mut self,
        window_id: WindowId,
        update: &mut dyn FnMut(Option<(AnyView, &mut Window, &mut App)>),
    ) {
        self.update(|cx| {
            let Some(mut window) = cx.windows.get_mut(window_id).and_then(Option::take) else {
                update(None);
                return;
            };

            let root_view = window.root.clone().unwrap();

            cx.window_update_stack.push(window.handle.id);
            update(Some((root_view, &mut window, cx)));
            fn trail(window_id: WindowId, window: Box<Window>, cx: &mut App) -> Option<()> {
                cx.window_update_stack.pop();

                if window.removed {
                    cx.window_handles.remove(&window_id);
                    cx.windows.remove(window_id);
                    if let Some(tracked) = cx.tracked_entities.remove(&window_id) {
                        for entity_id in tracked {
                            if let Some(windows) =
                                cx.window_invalidators_by_entity.get_mut(&entity_id)
                            {
                                windows.remove(&window_id);
                            }
                            if cx.current_window_by_entity.get(&entity_id) == Some(&window_id) {
                                cx.current_window_by_entity.remove(&entity_id);
                            }
                        }
                    }

                    cx.window_closed_observers.clone().retain(&(), |callback| {
                        callback(cx, window_id);
                        true
                    });

                    if cx.windows.is_empty() {
                        cx.quit();
                    }
                } else {
                    cx.windows.get_mut(window_id)?.replace(window);
                }
                Some(())
            }
            if trail(window_id, window, cx).is_none() {
                update(None);
            }
        });
    }

    #[inline(never)]
    fn update_entity_erased(
        &mut self,
        handle: &AnyEntity,
        entity_type: &str,
        update: &mut dyn FnMut(&mut dyn Any, &mut App),
    ) {
        self.update(|cx| {
            let mut lease = cx.entities.lease_erased(handle, entity_type);
            update(lease.entity.as_deref_mut().unwrap(), cx);
            cx.entities.end_lease_erased(handle.entity_id, lease);
        });
    }
}

impl AppContext for App {
    /// Builds an entity that is owned by the application.
    ///
    /// The given function will be invoked with a [`Context`] and must return an object representing the entity. An
    /// [`Entity`] handle will be returned, which can be used to access the entity in a context.
    fn new<T: 'static>(&mut self, build_entity: impl FnOnce(&mut Context<T>) -> T) -> Entity<T> {
        self.update(|cx| {
            let slot = cx.entities.reserve();
            let entity_id = slot.entity_id();
            let entity = build_entity(&mut Context::new_context(cx, slot.downgrade()));

            if let Some(&window_id) = cx.window_update_stack.last() {
                cx.current_window_by_entity.insert(entity_id, window_id);
            }

            cx.entities.insert(slot, entity)
        })
    }

    /// Updates the entity referenced by the given handle. The function is passed a mutable reference to the
    /// entity along with a `Context` for the entity.
    #[inline(always)]
    fn update_entity<T: 'static, R>(
        &mut self,
        handle: &Entity<T>,
        update: impl FnOnce(&mut T, &mut Context<T>) -> R,
    ) -> R {
        let mut update = Some(update);
        let mut result = None;
        self.update_entity_erased(handle, type_name::<T>(), &mut |entity, cx| {
            let value = update.take().unwrap()(
                entity.downcast_mut::<T>().unwrap(),
                &mut Context::new_context(cx, handle.downgrade()),
            );
            result = Some(value);
        });
        result.unwrap()
    }

    fn update_window<T, F>(&mut self, handle: AnyWindowHandle, update: F) -> Result<T>
    where
        F: FnOnce(AnyView, &mut Window, &mut App) -> T,
    {
        self.update_window_id(handle.id, update)
    }

    fn with_window<R>(
        &mut self,
        entity_id: EntityId,
        f: impl FnOnce(&mut Window, &mut App) -> R,
    ) -> Option<R> {
        App::with_window(self, entity_id, f)
    }
}

/// These effects are processed at the end of each application update cycle.
pub(crate) enum Effect {
    Notify {
        emitter: EntityId,
    },
    RefreshWindows,
    NotifyGlobalObservers {
        global_type: TypeId,
    },
    Defer {
        callback: Box<dyn FnOnce(&mut App) + 'static>,
    },
}

impl std::fmt::Debug for Effect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Effect::Notify { emitter } => write!(f, "Notify({})", emitter),
            Effect::RefreshWindows => write!(f, "RefreshWindows"),
            Effect::NotifyGlobalObservers { global_type } => {
                write!(f, "NotifyGlobalObservers({:?})", global_type)
            }
            Effect::Defer { .. } => write!(f, "Defer(..)"),
        }
    }
}

/// Wraps a global variable value during `update_global` while the value has been moved to the stack.
pub(crate) struct GlobalLease<G: Global> {
    global: Box<dyn Any>,
    global_type: PhantomData<G>,
}

impl<G: Global> GlobalLease<G> {
    fn new(global: Box<dyn Any>) -> Self {
        GlobalLease {
            global,
            global_type: PhantomData,
        }
    }
}

impl<G: Global> Deref for GlobalLease<G> {
    type Target = G;

    fn deref(&self) -> &Self::Target {
        self.global.downcast_ref().unwrap()
    }
}

impl<G: Global> DerefMut for GlobalLease<G> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.global.downcast_mut().unwrap()
    }
}

/// Whether a keystroke should prefer character input or key bindings.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InputPreference {
    /// Prefer typing text over triggering key bindings.
    CharacterInput,
    /// Dispatch key bindings normally, if any match.
    KeyBindings,
}

/// A keystroke event.
#[derive(Debug)]
pub(crate) struct KeystrokeEvent {
    /// The keystroke that occurred
    pub(crate) keystroke: Keystroke,
}
