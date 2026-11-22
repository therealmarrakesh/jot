//! Minimal generational `SlotMap` and `SecondaryMap` (replaces the external `slotmap` crate).

use std::marker::PhantomData;

/// Compact 64-bit generational key: lower 32 bits = slot index, upper 32 bits = version.
/// Live slots always have odd versions (`version & 1 == 1`), ensuring `as_ffi() != 0`.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) struct KeyData {
    idx: u32,
    version: u32,
}

impl KeyData {
    #[inline]
    pub(crate) const fn as_ffi(self) -> u64 {
        ((self.version as u64) << 32) | (self.idx as u64)
    }

    #[inline]
    pub(crate) const fn from_ffi(value: u64) -> Self {
        Self {
            idx: value as u32,
            version: (value >> 32) as u32,
        }
    }

    #[inline]
    pub(crate) const fn is_null(self) -> bool {
        self.version == 0
    }
}

pub(crate) trait Key: Copy + Clone + Default + Eq + PartialEq + Ord + PartialOrd + std::hash::Hash {
    fn data(&self) -> KeyData;
    fn from_data(data: KeyData) -> Self;
}

macro_rules! new_key_type {
    (
        $(#[$meta:meta])*
        $vis:vis struct $name:ident;
    ) => {
        $(#[$meta])*
        #[derive(Copy, Clone, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Hash)]
        #[repr(transparent)]
        $vis struct $name($crate::ui::slotmap::KeyData);

        impl $crate::ui::slotmap::Key for $name {
            #[inline]
            fn data(&self) -> $crate::ui::slotmap::KeyData {
                self.0
            }

            #[inline]
            fn from_data(data: $crate::ui::slotmap::KeyData) -> Self {
                Self(data)
            }
        }
    };
}

pub(crate) use new_key_type;

struct Slot<V> {
    version: u32,
    value: Option<V>,
}

pub(crate) struct SlotMap<K: Key, V> {
    slots: Vec<Slot<V>>,
    free_list: Vec<u32>,
    len: usize,
    _key: PhantomData<fn() -> K>,
}

impl<K: Key, V> Default for SlotMap<K, V> {
    fn default() -> Self {
        Self::with_key()
    }
}

impl<K: Key, V> SlotMap<K, V> {
    pub(crate) fn with_key() -> Self {
        Self {
            slots: Vec::new(),
            free_list: Vec::new(),
            len: 0,
            _key: PhantomData,
        }
    }

    #[inline]
    pub(crate) fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub(crate) fn insert(&mut self, value: V) -> K {
        self.len += 1;
        if let Some(idx) = self.free_list.pop() {
            let slot = &mut self.slots[idx as usize];
            debug_assert!(slot.value.is_none());
            slot.version = slot.version.wrapping_add(1) | 1;
            slot.value = Some(value);
            K::from_data(KeyData {
                idx,
                version: slot.version,
            })
        } else {
            let idx = self.slots.len() as u32;
            self.slots.push(Slot {
                version: 1,
                value: Some(value),
            });
            K::from_data(KeyData { idx, version: 1 })
        }
    }

    pub(crate) fn get(&self, key: K) -> Option<&V> {
        let kd = key.data();
        let slot = self.slots.get(kd.idx as usize)?;
        if slot.version == kd.version {
            slot.value.as_ref()
        } else {
            None
        }
    }

    pub(crate) fn get_mut(&mut self, key: K) -> Option<&mut V> {
        let kd = key.data();
        let slot = self.slots.get_mut(kd.idx as usize)?;
        if slot.version == kd.version {
            slot.value.as_mut()
        } else {
            None
        }
    }

    pub(crate) fn remove(&mut self, key: K) -> Option<V> {
        let kd = key.data();
        let slot = self.slots.get_mut(kd.idx as usize)?;
        if slot.version == kd.version {
            let val = slot.value.take()?;
            slot.version = slot.version.wrapping_add(1);
            self.free_list.push(kd.idx);
            self.len -= 1;
            Some(val)
        } else {
            None
        }
    }

    pub(crate) fn clear(&mut self) {
        for (idx, slot) in self.slots.iter_mut().enumerate() {
            if slot.value.take().is_some() {
                slot.version = slot.version.wrapping_add(1);
                self.free_list.push(idx as u32);
            }
        }
        self.len = 0;
    }

    pub(crate) fn retain(&mut self, mut f: impl FnMut(K, &mut V) -> bool) {
        for (idx, slot) in self.slots.iter_mut().enumerate() {
            let key = K::from_data(KeyData {
                idx: idx as u32,
                version: slot.version,
            });
            let remove = match slot.value.as_mut() {
                Some(v) => !f(key, v),
                None => false,
            };
            if remove {
                slot.value = None;
                slot.version = slot.version.wrapping_add(1);
                self.free_list.push(idx as u32);
                self.len -= 1;
            }
        }
    }

    pub(crate) fn values_mut(&mut self) -> impl Iterator<Item = &mut V> {
        self.slots.iter_mut().filter_map(|s| s.value.as_mut())
    }

    pub(crate) fn keys(&self) -> impl Iterator<Item = K> + '_ {
        self.slots.iter().enumerate().filter_map(|(idx, s)| {
            s.value.as_ref().map(|_| {
                K::from_data(KeyData {
                    idx: idx as u32,
                    version: s.version,
                })
            })
        })
    }
}

pub(crate) struct SecondaryMap<K: Key, V> {
    slots: Vec<Slot<V>>,
    _key: PhantomData<fn() -> K>,
}

impl<K: Key, V> Default for SecondaryMap<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: Key, V> SecondaryMap<K, V> {
    pub(crate) fn new() -> Self {
        Self {
            slots: Vec::new(),
            _key: PhantomData,
        }
    }

    pub(crate) fn insert(&mut self, key: K, value: V) -> Option<V> {
        let kd = key.data();
        if kd.is_null() {
            return None;
        }
        let idx = kd.idx as usize;
        if idx >= self.slots.len() {
            self.slots.resize_with(idx + 1, || Slot {
                version: 0,
                value: None,
            });
        }
        let slot = &mut self.slots[idx];
        if slot.version == kd.version {
            slot.value.replace(value)
        } else {
            slot.version = kd.version;
            slot.value = Some(value);
            None
        }
    }

    pub(crate) fn get(&self, key: K) -> Option<&V> {
        let kd = key.data();
        let slot = self.slots.get(kd.idx as usize)?;
        if slot.version == kd.version {
            slot.value.as_ref()
        } else {
            None
        }
    }

    pub(crate) fn remove(&mut self, key: K) -> Option<V> {
        let kd = key.data();
        let slot = self.slots.get_mut(kd.idx as usize)?;
        if slot.version == kd.version {
            slot.version = 0;
            slot.value.take()
        } else {
            None
        }
    }
}
