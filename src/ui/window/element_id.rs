use std::{fmt::Display, sync::Arc};

use crate::ui::{EntityId, FocusHandle, FocusId, GlobalElementId, SharedString, smallvec::SmallVec};

/// An identifier for an element in the UI tree.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub(crate) enum ElementId {
    View(EntityId),
    Integer(u64),
    Name(SharedString),
    FocusHandle(FocusId),
    NamedInteger(SharedString, u64),
    CodeLocation(core::panic::Location<'static>),
    NamedChild(Arc<ElementId>, SharedString),
}

impl Display for ElementId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ElementId::View(entity_id) => write!(f, "view-{entity_id}"),
            ElementId::Integer(ix) => write!(f, "{ix}"),
            ElementId::Name(name) => write!(f, "{name}"),
            ElementId::FocusHandle(_) => write!(f, "FocusHandle"),
            ElementId::NamedInteger(s, i) => write!(f, "{s}-{i}"),
            ElementId::CodeLocation(location) => write!(f, "{location}"),
            ElementId::NamedChild(id, name) => write!(f, "{id}-{name}"),
        }
    }
}

impl From<u64> for ElementId {
    fn from(id: u64) -> Self {
        ElementId::Integer(id)
    }
}

impl From<usize> for ElementId {
    fn from(id: usize) -> Self {
        ElementId::Integer(id as u64)
    }
}

impl From<i32> for ElementId {
    fn from(id: i32) -> Self {
        Self::Integer(id as u64)
    }
}

impl From<SharedString> for ElementId {
    fn from(name: SharedString) -> Self {
        ElementId::Name(name)
    }
}

impl From<String> for ElementId {
    fn from(name: String) -> Self {
        ElementId::Name(name.into())
    }
}

impl From<Arc<str>> for ElementId {
    fn from(name: Arc<str>) -> Self {
        ElementId::Name(name.into())
    }
}

impl From<&'static str> for ElementId {
    fn from(name: &'static str) -> Self {
        ElementId::Name(SharedString::new_static(name))
    }
}

impl<'a> From<&'a FocusHandle> for ElementId {
    fn from(handle: &'a FocusHandle) -> Self {
        ElementId::FocusHandle(handle.id)
    }
}

impl From<(&'static str, EntityId)> for ElementId {
    fn from((name, id): (&'static str, EntityId)) -> Self {
        ElementId::NamedInteger(SharedString::new_static(name), id.as_u64())
    }
}

impl From<(&'static str, usize)> for ElementId {
    fn from((name, id): (&'static str, usize)) -> Self {
        ElementId::NamedInteger(SharedString::new_static(name), id as u64)
    }
}

impl From<(SharedString, usize)> for ElementId {
    fn from((name, id): (SharedString, usize)) -> Self {
        ElementId::NamedInteger(name, id as u64)
    }
}

impl From<(&'static str, u64)> for ElementId {
    fn from((name, id): (&'static str, u64)) -> Self {
        ElementId::NamedInteger(SharedString::new_static(name), id)
    }
}

impl From<(&'static str, u32)> for ElementId {
    fn from((name, id): (&'static str, u32)) -> Self {
        ElementId::NamedInteger(SharedString::new_static(name), u64::from(id))
    }
}

impl<T: Into<SharedString>> From<(ElementId, T)> for ElementId {
    fn from((id, name): (ElementId, T)) -> Self {
        ElementId::NamedChild(Arc::new(id), name.into())
    }
}

/// Active element ID path with incremental prefix hashing.
#[derive(Clone, Default)]
pub(crate) struct ElementIdStack {
    ids: SmallVec<[ElementId; 32]>,
    hashes: SmallVec<[u64; 32]>,
}

impl ElementIdStack {
    pub(crate) fn push(&mut self, id: ElementId) {
        self.hashes.push(crate::ui::extend_path_hash(self.hash(), &id));
        self.ids.push(id);
    }

    pub(crate) fn pop(&mut self) -> Option<ElementId> {
        self.hashes.pop();
        self.ids.pop()
    }

    pub(crate) fn clear(&mut self) {
        self.ids.clear();
        self.hashes.clear();
    }

    fn hash(&self) -> u64 {
        self.hashes
            .last()
            .copied()
            .unwrap_or(crate::ui::EMPTY_PATH_HASH)
    }

    pub(crate) fn global_id(&self) -> GlobalElementId {
        GlobalElementId::with_hash(&self.ids, self.hash())
    }
}

impl std::ops::Deref for ElementIdStack {
    type Target = [ElementId];

    fn deref(&self) -> &Self::Target {
        &self.ids
    }
}
