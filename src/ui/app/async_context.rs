use crate::ui::{
    AnyView, AnyWindowHandle, App, AppCell, AppContext, BackgroundExecutor, Entity, EntityId,
    ForegroundExecutor, Result, Window,
};
use crate::ui::anyhow::Context as _;
use std::rc::Weak;

use super::Context;

/// An async-friendly version of [App] with a static lifetime so it can be held across `await` points in async code.
#[derive(Clone)]
pub(crate) struct AsyncApp {
    pub(crate) app: Weak<AppCell>,
    pub(crate) background_executor: BackgroundExecutor,
    pub(crate) foreground_executor: ForegroundExecutor,
}

impl AsyncApp {
    fn app(&self) -> std::rc::Rc<AppCell> {
        self.app
            .upgrade()
            .expect("app was released before async operation completed")
    }
}

impl AppContext for AsyncApp {
    fn new<T: 'static>(&mut self, build_entity: impl FnOnce(&mut Context<T>) -> T) -> Entity<T> {
        let app = self.app();
        let mut app = app.borrow_mut();
        app.new(build_entity)
    }

    #[inline(always)]
    fn update_entity<T: 'static, R>(
        &mut self,
        handle: &Entity<T>,
        update: impl FnOnce(&mut T, &mut Context<T>) -> R,
    ) -> R {
        let app = self.app();
        let mut app = app.borrow_mut();
        app.update_entity(handle, update)
    }

    #[inline(always)]
    fn update_window<T, F>(&mut self, window: AnyWindowHandle, f: F) -> Result<T>
    where
        F: FnOnce(AnyView, &mut Window, &mut App) -> T,
    {
        let app = self.app.upgrade().context("app was released")?;
        let mut lock = app.try_borrow_mut()?;
        lock.update_window(window, f)
    }

    #[inline(always)]
    fn with_window<R>(
        &mut self,
        entity_id: EntityId,
        f: impl FnOnce(&mut Window, &mut App) -> R,
    ) -> Option<R> {
        let app = self.app.upgrade()?;
        let mut lock = app.try_borrow_mut().ok()?;
        lock.with_window(entity_id, f)
    }
}

impl AsyncApp {
    /// Get an executor which can be used to spawn futures in the background.
    pub(crate) fn background_executor(&self) -> &BackgroundExecutor {
        &self.background_executor
    }

    /// Get an executor which can be used to spawn futures in the foreground.
    pub(crate) fn foreground_executor(&self) -> &ForegroundExecutor {
        &self.foreground_executor
    }

    /// Invoke the given function in the context of the app, then flush any effects produced during its invocation.
    #[inline(always)]
    pub(crate) fn update<R>(&self, f: impl FnOnce(&mut App) -> R) -> R {
        let app = self.app();
        let mut lock = app.borrow_mut();
        lock.update(f)
    }
}

/// A cloneable, owned handle to the application context,
/// composed with the window associated with the current task.
#[derive(Clone)]
pub(crate) struct AsyncWindowContext {
    app: AsyncApp,
    window: AnyWindowHandle,
}

impl std::ops::Deref for AsyncWindowContext {
    type Target = AsyncApp;
    fn deref(&self) -> &Self::Target {
        &self.app
    }
}

impl std::ops::DerefMut for AsyncWindowContext {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.app
    }
}

impl AsyncWindowContext {
    pub(crate) fn new_context(app: AsyncApp, window: AnyWindowHandle) -> Self {
        Self { app, window }
    }

    /// A convenience method for [`App::update_window`].
    #[inline(always)]
    pub(crate) fn update<R>(&mut self, update: impl FnOnce(&mut Window, &mut App) -> R) -> Result<R> {
        self.app
            .update_window(self.window, |_, window, cx| update(window, cx))
    }
}

impl AppContext for AsyncWindowContext {
    fn new<T>(&mut self, build_entity: impl FnOnce(&mut Context<T>) -> T) -> Entity<T>
    where
        T: 'static,
    {
        let mut build_entity = Some(build_entity);
        match self.app.update_window(self.window, |_, _, cx| {
            cx.new(
                build_entity
                    .take()
                    .expect("build_entity is taken exactly once"),
            )
        }) {
            Ok(entity) => entity,
            Err(_) => self.app.new(
                build_entity
                    .take()
                    .expect("update_window returned Err without invoking the closure"),
            ),
        }
    }

    #[inline(always)]
    fn update_entity<T: 'static, R>(
        &mut self,
        handle: &Entity<T>,
        update: impl FnOnce(&mut T, &mut Context<T>) -> R,
    ) -> R {
        self.app.update_entity(handle, update)
    }

    #[inline(always)]
    fn update_window<T, F>(&mut self, window: AnyWindowHandle, update: F) -> Result<T>
    where
        F: FnOnce(AnyView, &mut Window, &mut App) -> T,
    {
        self.app.update_window(window, update)
    }

    #[inline(always)]
    fn with_window<R>(
        &mut self,
        entity_id: EntityId,
        f: impl FnOnce(&mut Window, &mut App) -> R,
    ) -> Option<R> {
        self.app.with_window(entity_id, f)
    }
}
