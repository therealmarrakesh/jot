//! Clean-room entity storage and strongly-typed `Entity<T>` / `WeakEntity<T>` / `AnyView` handles.
//!
//! Zero `unsafe` and zero `RefCell` per entity: `EntityStore` owns `Option<Box<dyn Any>>` values
//! directly inside `App`, so `Entity::read(&App) -> &T` borrows straight from `&App`, while
//! `Entity::update(&mut App, ...)` temporarily takes the `Box` out to guarantee aliasing safety.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicU64, Ordering};

use super::app::{App, Context};
use super::element::{Render, UiNode};
use super::types::{UiError, UiResult};
use super::window::Window;

static NEXT_ENTITY_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct EntityId(pub(crate) u64);

impl EntityId {
    pub(crate) fn next() -> Self {
        Self(NEXT_ENTITY_ID.fetch_add(1, Ordering::Relaxed))
    }
}

pub(crate) struct StoredEntity {
    pub(crate) value: Option<Box<dyn Any>>,
    pub(crate) live: Weak<()>,
}

#[derive(Default)]
pub(crate) struct EntityStore {
    entries: HashMap<EntityId, StoredEntity>,
}

impl EntityStore {
    pub(crate) fn reserve(&mut self) -> (EntityId, Rc<()>) {
        let id = EntityId::next();
        let token = Rc::new(());
        self.entries.insert(
            id,
            StoredEntity {
                value: None,
                live: Rc::downgrade(&token),
            },
        );
        (id, token)
    }

    pub(crate) fn insert_value<T: 'static>(&mut self, id: EntityId, value: T) {
        if let Some(entry) = self.entries.get_mut(&id) {
            entry.value = Some(Box::new(value));
        }
    }

    pub(crate) fn read<T: 'static>(&self, id: EntityId) -> &T {
        self.entries
            .get(&id)
            .and_then(|e| e.value.as_deref())
            .and_then(|any| any.downcast_ref::<T>())
            .unwrap_or_else(|| {
                panic!(
                    "failed to read entity {:?} ({}): dropped or mutably leased",
                    id,
                    std::any::type_name::<T>()
                )
            })
    }

    pub(crate) fn take_box(&mut self, id: EntityId) -> Option<Box<dyn Any>> {
        self.entries.get_mut(&id)?.value.take()
    }

    pub(crate) fn put_box(&mut self, id: EntityId, value: Box<dyn Any>) {
        if let Some(entry) = self.entries.get_mut(&id) {
            entry.value = Some(value);
        }
    }

    pub(crate) fn prune_dropped(&mut self) {
        self.entries.retain(|_, entry| entry.live.strong_count() > 0);
    }
}

/// Strong typed handle to a state object stored inside `App`.
pub(crate) struct Entity<T> {
    pub(crate) id: EntityId,
    pub(crate) token: Rc<()>,
    _marker: PhantomData<fn() -> T>,
}

impl<T> Clone for Entity<T> {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            token: self.token.clone(),
            _marker: PhantomData,
        }
    }
}

impl<T> PartialEq for Entity<T> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}
impl<T> Eq for Entity<T> {}

impl<T> Hash for Entity<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

impl<T> fmt::Debug for Entity<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Entity").field(&self.id.0).finish()
    }
}

/// Weak handle to an `Entity<T>`.
pub(crate) struct WeakEntity<T> {
    pub(crate) id: EntityId,
    pub(crate) token: Weak<()>,
    _marker: PhantomData<fn() -> T>,
}

impl<T> Clone for WeakEntity<T> {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            token: self.token.clone(),
            _marker: PhantomData,
        }
    }
}

impl<T: 'static> Entity<T> {
    pub(crate) fn from_parts(id: EntityId, token: Rc<()>) -> Self {
        Self {
            id,
            token,
            _marker: PhantomData,
        }
    }

    #[inline]
    pub(crate) fn entity_id(&self) -> EntityId {
        self.id
    }

    #[inline]
    pub(crate) fn downgrade(&self) -> WeakEntity<T> {
        WeakEntity {
            id: self.id,
            token: Rc::downgrade(&self.token),
            _marker: PhantomData,
        }
    }

    pub(crate) fn read<'a>(&self, cx: &'a App) -> &'a T {
        cx.entities.read::<T>(self.id)
    }

    pub(crate) fn update<C: EntityUpdateContext, R>(
        &self,
        cx: &mut C,
        f: impl FnOnce(&mut T, &mut Context<T>) -> R,
    ) -> R {
        cx.with_app_mut(|app| app.update_entity_impl(self, f))
    }
}

pub(crate) trait EntityUpdateContext {
    fn with_app_mut<R>(&mut self, f: impl FnOnce(&mut App) -> R) -> R;
}

impl EntityUpdateContext for App {
    fn with_app_mut<R>(&mut self, f: impl FnOnce(&mut App) -> R) -> R {
        f(self)
    }
}

impl<U> EntityUpdateContext for Context<'_, U> {
    fn with_app_mut<R>(&mut self, f: impl FnOnce(&mut App) -> R) -> R {
        f(self.app)
    }
}

impl<T: 'static> WeakEntity<T> {
    pub(crate) fn upgrade(&self) -> Option<Entity<T>> {
        self.token
            .upgrade()
            .map(|token| Entity::from_parts(self.id, token))
    }

    pub(crate) fn update<C: EntityUpdateContext, R>(
        &self,
        cx: &mut C,
        f: impl FnOnce(&mut T, &mut Context<T>) -> R,
    ) -> UiResult<R> {
        let entity = self
            .upgrade()
            .ok_or_else(|| UiError::new("entity was dropped"))?;
        Ok(cx.with_app_mut(|app| entity.update(app, f)))
    }
}

/// Type-erased renderable view handle (e.g. a window's root view).
#[derive(Clone)]
pub(crate) struct AnyView {
    id: EntityId,
    token: Rc<()>,
    type_id: TypeId,
    render_fn: fn(EntityId, &Rc<()>, &mut Window, &mut App) -> UiNode,
}

impl AnyView {
    pub(crate) fn from_entity<V: Render>(entity: Entity<V>) -> Self {
        fn render_erased<V: Render>(
            id: EntityId,
            token: &Rc<()>,
            window: &mut Window,
            cx: &mut App,
        ) -> UiNode {
            let entity = Entity::<V>::from_parts(id, token.clone());
            entity.update(cx, |view, cx| view.render(window, cx).into())
        }
        Self {
            id: entity.id,
            token: entity.token,
            type_id: TypeId::of::<V>(),
            render_fn: render_erased::<V>,
        }
    }

    #[inline]
    pub(crate) fn entity_id(&self) -> EntityId {
        self.id
    }

    pub(crate) fn downcast<V: 'static>(&self) -> Option<Entity<V>> {
        (self.type_id == TypeId::of::<V>()).then(|| Entity::from_parts(self.id, self.token.clone()))
    }

    pub(crate) fn render(&self, window: &mut Window, cx: &mut App) -> UiNode {
        (self.render_fn)(self.id, &self.token, window, cx)
    }
}
