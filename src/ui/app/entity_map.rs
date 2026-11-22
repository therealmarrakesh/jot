use crate::ui::{App, AppContext, Window, seal::Sealed};
use crate::ui::anyhow::{Context as _, Result};
use crate::ui::collections::FxHashSet;
use crate::ui::sync::RwLock;
use crate::ui::slotmap::{KeyData, SecondaryMap, SlotMap};
use std::{
    any::{Any, TypeId, type_name},
    cell::RefCell,
    cmp::Ordering,
    fmt::{self, Display},
    hash::{Hash, Hasher},
    marker::PhantomData,
    sync::{
        Arc, Weak,
        atomic::{AtomicUsize, Ordering::SeqCst},
    },
    thread::panicking,
};

use super::Context;
use crate::ui::util::atomic_incr_if_not_zero;

crate::ui::new_key_type! {
    /// A unique identifier for a entity across the application.
    pub(crate) struct EntityId;
}

impl From<u64> for EntityId {
    fn from(value: u64) -> Self {
        Self(KeyData::from_ffi(value))
    }
}

impl EntityId {
    /// Converts this entity id to a [u64]
    pub(crate) fn as_u64(self) -> u64 {
        self.0.as_ffi()
    }
}

impl Display for EntityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_u64())
    }
}

pub(crate) struct EntityMap {
    entities: SecondaryMap<EntityId, Box<dyn Any>>,
    pub(crate) accessed_entities: RefCell<FxHashSet<EntityId>>,
    ref_counts: Arc<RwLock<EntityRefCounts>>,
}

#[doc(hidden)]
pub(crate) struct EntityRefCounts {
    counts: SlotMap<EntityId, AtomicUsize>,
    dropped_entity_ids: Vec<EntityId>,
}

pub(super) struct LeaseInner {
    pub(super) entity: Option<Box<dyn Any>>,
}

impl EntityMap {
    pub(crate) fn new() -> Self {
        Self {
            entities: SecondaryMap::new(),
            accessed_entities: RefCell::new(FxHashSet::default()),
            ref_counts: Arc::new(RwLock::new(EntityRefCounts {
                counts: SlotMap::with_key(),
                dropped_entity_ids: Vec::new(),
            })),
        }
    }

    /// Reserve a slot for an entity, which you can subsequently use with `insert`.
    pub(crate) fn reserve<T: 'static>(&self) -> Slot<T> {
        let id = self.ref_counts.write().counts.insert(1.into());
        Slot(Entity::new(id, Arc::downgrade(&self.ref_counts)))
    }

    /// Insert an entity into a slot obtained by calling `reserve`.
    pub(crate) fn insert<T>(&mut self, slot: Slot<T>, entity: T) -> Entity<T>
    where
        T: 'static,
    {
        let accessed_entities = self.accessed_entities.get_mut();
        accessed_entities.insert(slot.entity_id);

        let handle = slot.0;
        self.entities.insert(handle.entity_id, Box::new(entity));
        handle
    }

    #[inline(always)]
    pub(crate) fn read<T: 'static>(&self, entity: &Entity<T>) -> &T {
        self.assert_valid_context(entity);
        self.read_inner(entity.entity_id)
            .and_then(|entity| entity.downcast_ref())
            .unwrap_or_else(|| double_lease_panic("read", type_name::<T>()))
    }

    #[track_caller]
    pub(super) fn lease_erased(&mut self, pointer: &AnyEntity, entity_type: &str) -> LeaseInner {
        self.assert_valid_context(pointer);
        let entity = Some(
            self.lease_inner(pointer.entity_id)
                .unwrap_or_else(|| double_lease_panic("update", entity_type)),
        );
        LeaseInner { entity }
    }

    pub(super) fn end_lease_erased(&mut self, entity_id: EntityId, mut lease: LeaseInner) {
        self.end_lease_inner(entity_id, lease.entity.take().unwrap());
    }

    fn assert_valid_context(&self, entity: &AnyEntity) {
        debug_assert!(
            Weak::ptr_eq(&entity.entity_map, &Arc::downgrade(&self.ref_counts)),
            "used a entity with the wrong context"
        );
    }

    pub(crate) fn clear_accessed(&mut self) {
        self.accessed_entities.get_mut().clear();
    }

    pub(crate) fn take_dropped(&mut self) -> Vec<(EntityId, Box<dyn Any>)> {
        let ref_counts = &mut *self.ref_counts.write();
        let dropped_entity_ids = ref_counts.dropped_entity_ids.drain(..);
        let accessed_entities = self.accessed_entities.get_mut();

        dropped_entity_ids
            .filter_map(|entity_id| {
                let count = ref_counts.counts.remove(entity_id).unwrap();
                debug_assert_eq!(
                    count.load(SeqCst),
                    0,
                    "dropped an entity that was referenced"
                );
                accessed_entities.remove(&entity_id);
                // If the EntityId was allocated with `Context::reserve`,
                // the entity may not have been inserted.
                Some((entity_id, self.entities.remove(entity_id)?))
            })
            .collect()
    }

    #[inline(never)]
    fn read_inner(&self, entity_id: EntityId) -> Option<&dyn Any> {
        let mut accessed_entities = self.accessed_entities.borrow_mut();
        accessed_entities.insert(entity_id);
        self.entities.get(entity_id).map(Box::as_ref)
    }

    #[inline(never)]
    fn lease_inner(&mut self, entity_id: EntityId) -> Option<Box<dyn Any>> {
        self.accessed_entities.get_mut().insert(entity_id);
        self.entities.remove(entity_id)
    }

    #[inline(never)]
    fn end_lease_inner(&mut self, entity_id: EntityId, entity: Box<dyn Any>) {
        self.entities.insert(entity_id, entity);
    }
}

#[track_caller]
fn double_lease_panic(operation: &str, entity_type: &str) -> ! {
    panic!("cannot {operation} {entity_type} while it is already being updated")
}

impl Drop for LeaseInner {
    fn drop(&mut self) {
        if self.entity.is_some() && !panicking() {
            panic!("Leases must be ended with EntityMap::end_lease")
        }
    }
}

pub(crate) struct Slot<T>(Entity<T>);

impl<T> std::ops::Deref for Slot<T> {
    type Target = Entity<T>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> std::ops::DerefMut for Slot<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// A dynamically typed reference to a entity, which can be downcast into a `Entity<T>`.
pub(crate) struct AnyEntity {
    pub(crate) entity_id: EntityId,
    pub(crate) entity_type: TypeId,
    entity_map: Weak<RwLock<EntityRefCounts>>,
}

impl AnyEntity {
    fn new(
        id: EntityId,
        entity_type: TypeId,
        entity_map: Weak<RwLock<EntityRefCounts>>,
    ) -> Self {
        Self {
            entity_id: id,
            entity_type,
            entity_map,
        }
    }

    /// Returns the id associated with this entity.
    #[inline]
    pub(crate) fn entity_id(&self) -> EntityId {
        self.entity_id
    }

    /// Converts this entity handle into a weak variant, which does not prevent it from being released.
    pub(crate) fn downgrade(&self) -> AnyWeakEntity {
        AnyWeakEntity {
            entity_id: self.entity_id,
            entity_type: self.entity_type,
            entity_ref_counts: self.entity_map.clone(),
        }
    }

    /// Converts this entity handle into a strongly-typed entity handle of the given type.
    /// If this entity handle is not of the specified type, returns itself as an error variant.
    pub(crate) fn downcast<T: 'static>(self) -> Result<Entity<T>, AnyEntity> {
        if TypeId::of::<T>() == self.entity_type {
            Ok(Entity {
                any_entity: self,
                entity_type: PhantomData,
            })
        } else {
            Err(self)
        }
    }
}

impl Clone for AnyEntity {
    fn clone(&self) -> Self {
        if let Some(entity_map) = self.entity_map.upgrade() {
            let entity_map = entity_map.read();
            let count = entity_map
                .counts
                .get(self.entity_id)
                .expect("detected over-release of a entity");
            let prev_count = count.fetch_add(1, SeqCst);
            assert_ne!(prev_count, 0, "Detected over-release of a entity.");
        }

        Self {
            entity_id: self.entity_id,
            entity_type: self.entity_type,
            entity_map: self.entity_map.clone(),
        }
    }
}

impl Drop for AnyEntity {
    fn drop(&mut self) {
        if let Some(entity_map) = self.entity_map.upgrade() {
            let prev_count = {
                let entity_map = entity_map.read();
                let count = entity_map
                    .counts
                    .get(self.entity_id)
                    .expect("detected over-release of a handle.");
                let prev_count = count.fetch_sub(1, SeqCst);
                assert_ne!(prev_count, 0, "Detected over-release of a entity.");
                prev_count
            };
            if prev_count == 1 {
                // We were the last reference to this entity, so we can remove it.
                entity_map.write().dropped_entity_ids.push(self.entity_id);
            }
        }
    }
}

impl<T> From<Entity<T>> for AnyEntity {
    #[inline]
    fn from(entity: Entity<T>) -> Self {
        entity.any_entity
    }
}

impl Hash for AnyEntity {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.entity_id.hash(state);
    }
}

impl PartialEq for AnyEntity {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.entity_id == other.entity_id
    }
}

impl Eq for AnyEntity {}

impl Ord for AnyEntity {
    #[inline]
    fn cmp(&self, other: &Self) -> Ordering {
        self.entity_id.cmp(&other.entity_id)
    }
}

impl PartialOrd for AnyEntity {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl std::fmt::Debug for AnyEntity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AnyEntity")
            .field("entity_id", &self.entity_id.as_u64())
            .finish()
    }
}

/// A strong, well-typed reference to a struct which is managed
/// by GPUI
pub(crate) struct Entity<T> {
    pub(crate) any_entity: AnyEntity,
    pub(crate) entity_type: PhantomData<fn(T) -> T>,
}

impl<T> std::ops::Deref for Entity<T> {
    type Target = AnyEntity;
    fn deref(&self) -> &Self::Target {
        &self.any_entity
    }
}

impl<T> std::ops::DerefMut for Entity<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.any_entity
    }
}

impl<T> Sealed for Entity<T> {}

impl<T: 'static> Entity<T> {
    #[inline]
    fn new(id: EntityId, entity_map: Weak<RwLock<EntityRefCounts>>) -> Self
    where
        T: 'static,
    {
        Self {
            any_entity: AnyEntity::new(id, TypeId::of::<T>(), entity_map),
            entity_type: PhantomData,
        }
    }

    /// Get the entity ID associated with this entity
    #[inline]
    pub(crate) fn entity_id(&self) -> EntityId {
        self.any_entity.entity_id
    }

    /// Downgrade this entity pointer to a non-retaining weak pointer
    #[inline]
    pub(crate) fn downgrade(&self) -> WeakEntity<T> {
        WeakEntity {
            any_entity: self.any_entity.downgrade(),
            entity_type: self.entity_type,
        }
    }

    /// Convert this into a dynamically typed entity.
    #[inline]
    pub(crate) fn into_any(self) -> AnyEntity {
        self.any_entity
    }

    /// Grab a reference to this entity from the context.
    #[inline]
    pub(crate) fn read<'a>(&self, cx: &'a App) -> &'a T {
        cx.entities.read(self)
    }

    /// Updates the entity referenced by this handle with the given function.
    #[inline]
    pub(crate) fn update<R, C: AppContext>(
        &self,
        cx: &mut C,
        update: impl FnOnce(&mut T, &mut Context<T>) -> R,
    ) -> R {
        cx.update_entity(self, update)
    }
}

impl<T> Clone for Entity<T> {
    #[inline]
    fn clone(&self) -> Self {
        Self {
            any_entity: self.any_entity.clone(),
            entity_type: self.entity_type,
        }
    }
}

impl<T> std::fmt::Debug for Entity<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Entity")
            .field("entity_id", &self.any_entity.entity_id)
            .field("entity_type", &type_name::<T>())
            .finish()
    }
}

impl<T> Hash for Entity<T> {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.any_entity.hash(state);
    }
}

impl<T> PartialEq for Entity<T> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.any_entity == other.any_entity
    }
}

impl<T> Eq for Entity<T> {}

impl<T> PartialEq<WeakEntity<T>> for Entity<T> {
    #[inline]
    fn eq(&self, other: &WeakEntity<T>) -> bool {
        self.any_entity.entity_id() == other.entity_id()
    }
}

impl<T: 'static> Ord for Entity<T> {
    #[inline]
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.entity_id().cmp(&other.entity_id())
    }
}

impl<T: 'static> PartialOrd for Entity<T> {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// A type erased, weak reference to a entity.
#[derive(Clone)]
pub(crate) struct AnyWeakEntity {
    pub(crate) entity_id: EntityId,
    entity_type: TypeId,
    entity_ref_counts: Weak<RwLock<EntityRefCounts>>,
}

impl AnyWeakEntity {
    /// Get the entity ID associated with this weak reference.
    #[inline]
    pub(crate) fn entity_id(&self) -> EntityId {
        self.entity_id
    }

    /// Upgrade this weak entity reference to a strong reference.
    pub(crate) fn upgrade(&self) -> Option<AnyEntity> {
        let ref_counts = &self.entity_ref_counts.upgrade()?;
        let ref_counts = ref_counts.read();
        let ref_count = ref_counts.counts.get(self.entity_id)?;

        if atomic_incr_if_not_zero(ref_count) == 0 {
            // entity_id is in dropped_entity_ids
            return None;
        }
        drop(ref_counts);

        Some(AnyEntity {
            entity_id: self.entity_id,
            entity_type: self.entity_type,
            entity_map: self.entity_ref_counts.clone(),
        })
    }
}

impl std::fmt::Debug for AnyWeakEntity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("entity_id", &self.entity_id)
            .field("entity_type", &self.entity_type)
            .finish()
    }
}

impl<T> From<WeakEntity<T>> for AnyWeakEntity {
    #[inline]
    fn from(entity: WeakEntity<T>) -> Self {
        entity.any_entity
    }
}

impl Hash for AnyWeakEntity {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.entity_id.hash(state);
    }
}

impl PartialEq for AnyWeakEntity {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.entity_id == other.entity_id
    }
}

impl Eq for AnyWeakEntity {}

impl Ord for AnyWeakEntity {
    #[inline]
    fn cmp(&self, other: &Self) -> Ordering {
        self.entity_id.cmp(&other.entity_id)
    }
}

impl PartialOrd for AnyWeakEntity {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// A weak reference to a entity of the given type.
pub(crate) struct WeakEntity<T> {
    any_entity: AnyWeakEntity,
    entity_type: PhantomData<fn(T) -> T>,
}

impl<T> std::ops::Deref for WeakEntity<T> {
    type Target = AnyWeakEntity;
    fn deref(&self) -> &Self::Target {
        &self.any_entity
    }
}

impl<T> std::ops::DerefMut for WeakEntity<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.any_entity
    }
}

impl<T> std::fmt::Debug for WeakEntity<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("entity_id", &self.any_entity.entity_id)
            .field("entity_type", &type_name::<T>())
            .finish()
    }
}

impl<T> Clone for WeakEntity<T> {
    fn clone(&self) -> Self {
        Self {
            any_entity: self.any_entity.clone(),
            entity_type: self.entity_type,
        }
    }
}

impl<T: 'static> WeakEntity<T> {
    /// Upgrade this weak entity reference into a strong entity reference
    pub(crate) fn upgrade(&self) -> Option<Entity<T>> {
        Some(Entity {
            any_entity: self.any_entity.upgrade()?,
            entity_type: self.entity_type,
        })
    }

    /// Updates the entity referenced by this handle with the given function if
    /// the referenced entity still exists. Returns an error if the entity has
    /// been released.
    #[inline(always)]
    pub(crate) fn update<C, R>(
        &self,
        cx: &mut C,
        update: impl FnOnce(&mut T, &mut Context<T>) -> R,
    ) -> Result<R>
    where
        C: AppContext,
    {
        let entity = self.upgrade().context("entity released")?;
        Ok(cx.update_entity(&entity, update))
    }

    /// Updates the entity referenced by this handle with the given function if
    /// the referenced entity still exists, within a visual context that has a window.
    /// Returns an error if the entity has been released.
    #[inline(always)]
    pub(crate) fn update_in<C, R>(
        &self,
        cx: &mut C,
        update: impl FnOnce(&mut T, &mut Window, &mut Context<T>) -> R,
    ) -> Result<R>
    where
        C: AppContext,
    {
        let entity = self.upgrade().context("entity released")?;
        cx.with_window(entity.entity_id(), |window, app| {
            entity.update(app, |entity, cx| update(entity, window, cx))
        })
        .context("entity has no current window")
    }
}

impl<T> Hash for WeakEntity<T> {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.any_entity.hash(state);
    }
}

impl<T> PartialEq for WeakEntity<T> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.any_entity == other.any_entity
    }
}

impl<T> Eq for WeakEntity<T> {}

impl<T> PartialEq<Entity<T>> for WeakEntity<T> {
    #[inline]
    fn eq(&self, other: &Entity<T>) -> bool {
        self.entity_id() == other.any_entity.entity_id()
    }
}

impl<T: 'static> Ord for WeakEntity<T> {
    #[inline]
    fn cmp(&self, other: &Self) -> Ordering {
        self.entity_id().cmp(&other.entity_id())
    }
}

impl<T: 'static> PartialOrd for WeakEntity<T> {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
