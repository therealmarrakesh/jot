use crate::ui::{
    AnyView, AnyWindowHandle, AppContext, AsyncApp, EntityId, FocusHandle, Global, Subscription,
    Task, WeakEntity, Window,
};
use crate::ui::anyhow::Result;
use std::{
    any::TypeId,
    borrow::{Borrow, BorrowMut},
    ops,
};

use super::{App, AsyncWindowContext, Entity};

/// The app context, with specialized behavior for the given entity.
pub(crate) struct Context<'a, T> {
    app: &'a mut App,
    entity_state: WeakEntity<T>,
}

impl<'a, T> ops::Deref for Context<'a, T> {
    type Target = App;

    fn deref(&self) -> &Self::Target {
        self.app
    }
}

impl<'a, T> ops::DerefMut for Context<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.app
    }
}

impl<'a, T: 'static> Context<'a, T> {
    pub(crate) fn new_context(app: &'a mut App, entity_state: WeakEntity<T>) -> Self {
        Self { app, entity_state }
    }

    /// Returns a handle to the entity belonging to this context.
    pub(crate) fn entity(&self) -> Entity<T> {
        self.weak_entity()
            .upgrade()
            .expect("The entity must be alive if we have a entity context")
    }

    /// Returns a weak handle to the entity belonging to this context.
    pub(crate) fn weak_entity(&self) -> WeakEntity<T> {
        self.entity_state.clone()
    }

    /// Arranges for the given function to be called whenever [`Context::notify`] is
    /// called with the given entity.
    pub(crate) fn observe<W>(
        &mut self,
        entity: &Entity<W>,
        mut on_notify: impl FnMut(&mut T, Entity<W>, &mut Context<T>) + 'static,
    ) -> Subscription
    where
        T: 'static,
        W: 'static,
    {
        let observer = self.weak_entity();
        self.app.observe_internal(entity, move |entity, cx| {
            invoke_observer(&observer, entity, cx, &mut on_notify)
        })
    }

    /// Tell GPUI that this entity has changed and observers of it should be notified.
    pub(crate) fn notify(&mut self) {
        self.app.notify(self.entity_state.entity_id);
    }

    /// Spawn the future returned by the given function.
    /// The function is provided a weak handle to the entity owned by this context and a context that can be held across await points.
    /// The returned task must be held or detached.
    #[track_caller]
    #[inline(always)]
    pub(crate) fn spawn<AsyncFn, R>(&self, f: AsyncFn) -> Task<R>
    where
        T: 'static,
        AsyncFn: AsyncFnOnce(WeakEntity<T>, &mut AsyncApp) -> R + 'static,
        R: 'static,
    {
        let this = self.weak_entity();
        self.app.spawn(async move |cx| f(this, cx).await)
    }

    /// Convenience method for accessing view state in an event callback.
    #[inline(always)]
    pub(crate) fn listener<E: ?Sized>(
        &self,
        listener: impl Fn(&mut T, &E, &mut Window, &mut Context<T>) + 'static,
    ) -> impl Fn(&E, &mut Window, &mut App) + 'static {
        let view = self.entity().downgrade();
        move |event: &E, window: &mut Window, cx: &mut App| {
            invoke_listener(&view, window, cx, &|view, window, cx| {
                listener(view, event, window, cx);
            });
        }
    }

    /// Register a callback to be invoked when the window is resized.
    pub(crate) fn observe_window_bounds(
        &self,
        window: &mut Window,
        mut callback: impl FnMut(&mut T, &mut Window, &mut Context<T>) + 'static,
    ) -> Subscription {
        let view = self.weak_entity();
        let (subscription, activate) = window.bounds_observers.insert(
            (),
            Box::new(move |window, cx| {
                view.update(cx, |view, cx| callback(view, window, cx))
                    .is_ok()
            }),
        );
        activate();
        subscription
    }

    /// Register a callback to be invoked when the window is activated or deactivated.
    pub(crate) fn observe_window_activation(
        &self,
        window: &mut Window,
        mut callback: impl FnMut(&mut T, &mut Window, &mut Context<T>) + 'static,
    ) -> Subscription {
        let view = self.weak_entity();
        let (subscription, activate) = window.activation_observers.insert(
            (),
            Box::new(move |window, cx| {
                view.update(cx, |view, cx| callback(view, window, cx))
                    .is_ok()
            }),
        );
        activate();
        subscription
    }

    /// Registers a callback to be invoked when the window appearance changes.
    pub(crate) fn observe_window_appearance(
        &self,
        window: &mut Window,
        mut callback: impl FnMut(&mut T, &mut Window, &mut Context<T>) + 'static,
    ) -> Subscription {
        let view = self.weak_entity();
        let (subscription, activate) = window.appearance_observers.insert(
            (),
            Box::new(move |window, cx| {
                view.update(cx, |view, cx| callback(view, window, cx))
                    .is_ok()
            }),
        );
        activate();
        subscription
    }

    /// Register a listener to be called when the given focus handle receives focus.
    /// Returns a subscription and persists until the subscription is dropped.
    pub(crate) fn on_focus(
        &mut self,
        handle: &FocusHandle,
        window: &mut Window,
        mut listener: impl FnMut(&mut T, &mut Window, &mut Context<T>) + 'static,
    ) -> Subscription {
        let view = self.weak_entity();
        let focus_id = handle.id;
        let (subscription, activate) =
            window.new_focus_listener(Box::new(move |event, window, cx| {
                view.update(cx, |view, cx| {
                    if event.previous_focus_path.last() != Some(&focus_id)
                        && event.current_focus_path.last() == Some(&focus_id)
                    {
                        listener(view, window, cx)
                    }
                })
                .is_ok()
            }));
        self.defer(|_| activate());
        subscription
    }

    /// Register a listener to be called when the given focus handle loses focus.
    /// Returns a subscription and persists until the subscription is dropped.
    pub(crate) fn on_blur(
        &mut self,
        handle: &FocusHandle,
        window: &mut Window,
        mut listener: impl FnMut(&mut T, &mut Window, &mut Context<T>) + 'static,
    ) -> Subscription {
        let view = self.weak_entity();
        let focus_id = handle.id;
        let (subscription, activate) =
            window.new_focus_listener(Box::new(move |event, window, cx| {
                view.update(cx, |view, cx| {
                    if event.previous_focus_path.last() == Some(&focus_id)
                        && event.current_focus_path.last() != Some(&focus_id)
                    {
                        listener(view, window, cx)
                    }
                })
                .is_ok()
            }));
        self.defer(|_| activate());
        subscription
    }

    /// Schedule a future to be run asynchronously.
    /// The given callback is invoked with a [`WeakEntity<V>`] to avoid leaking the entity for a long-running process.
    /// It's also given an [`AsyncWindowContext`], which can be used to access the state of the entity across await points.
    /// The returned future will be polled on the main thread.
    #[track_caller]
    #[inline(always)]
    pub(crate) fn spawn_in<AsyncFn, R>(&self, window: &Window, f: AsyncFn) -> Task<R>
    where
        R: 'static,
        AsyncFn: AsyncFnOnce(WeakEntity<T>, &mut AsyncWindowContext) -> R + 'static,
    {
        let view = self.weak_entity();
        window.spawn(self, async move |cx| f(view, cx).await)
    }

    /// Register a callback to be invoked when the given global state changes.
    pub(crate) fn observe_global_in<G: Global>(
        &mut self,
        window: &Window,
        mut f: impl FnMut(&mut T, &mut Window, &mut Context<T>) + 'static,
    ) -> Subscription {
        let window_handle = window.handle;
        let view = self.weak_entity();
        let (subscription, activate) = self.global_observers.insert(
            TypeId::of::<G>(),
            Box::new(move |cx| {
                if view.upgrade().is_none() {
                    return false;
                }
                let Ok(entity_alive) = window_handle.update(cx, |_, window, cx| {
                    view.update(cx, |view, cx| f(view, window, cx)).is_ok()
                }) else {
                    return true;
                };
                entity_alive
            }),
        );
        self.defer(move |_| activate());
        subscription
    }
}

impl<T> AppContext for Context<'_, T> {
    #[inline]
    fn new<U: 'static>(&mut self, build_entity: impl FnOnce(&mut Context<U>) -> U) -> Entity<U> {
        self.app.new(build_entity)
    }

    #[inline]
    fn update_entity<U: 'static, R>(
        &mut self,
        handle: &Entity<U>,
        update: impl FnOnce(&mut U, &mut Context<U>) -> R,
    ) -> R {
        self.app.update_entity(handle, update)
    }

    #[inline]
    fn update_window<R, F>(&mut self, window: AnyWindowHandle, update: F) -> Result<R>
    where
        F: FnOnce(AnyView, &mut Window, &mut App) -> R,
    {
        self.app.update_window(window, update)
    }

    #[inline]
    fn with_window<R>(
        &mut self,
        entity_id: EntityId,
        f: impl FnOnce(&mut Window, &mut App) -> R,
    ) -> Option<R> {
        self.app.with_window(entity_id, f)
    }
}

impl<T> Borrow<App> for Context<'_, T> {
    fn borrow(&self) -> &App {
        self.app
    }
}

impl<T> BorrowMut<App> for Context<'_, T> {
    fn borrow_mut(&mut self) -> &mut App {
        self.app
    }
}

#[inline(never)]
fn invoke_observer<T: 'static, W: 'static>(
    observer: &WeakEntity<T>,
    observed: Entity<W>,
    cx: &mut App,
    on_notify: &mut dyn FnMut(&mut T, Entity<W>, &mut Context<T>),
) -> bool {
    if let Some(observer) = observer.upgrade() {
        observer.update(cx, |observer, cx| on_notify(observer, observed, cx));
        true
    } else {
        false
    }
}

#[inline(never)]
fn invoke_listener<T: 'static>(
    view: &WeakEntity<T>,
    window: &mut Window,
    cx: &mut App,
    listener: &dyn Fn(&mut T, &mut Window, &mut Context<T>),
) {
    view.update(cx, |view, cx| listener(view, window, cx)).ok();
}
