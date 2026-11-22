use std::hash::{BuildHasher, Hasher};

pub use std::collections::*;

/// Minimal FxHash implementation (compatible with `rustc_hash::FxHasher`).
#[derive(Clone, Default)]
pub struct FxHasher {
    hash: usize,
}

#[cfg(target_pointer_width = "64")]
const K: usize = 0x517cc1b727220a95;
#[cfg(target_pointer_width = "32")]
const K: usize = 0x9e3779b9;

impl FxHasher {
    #[inline]
    pub const fn with_seed(seed: usize) -> Self {
        Self { hash: seed }
    }

    #[inline]
    fn add_to_hash(&mut self, i: usize) {
        self.hash = self.hash.rotate_left(5).bitxor(i).wrapping_mul(K);
    }
}

trait BitXorExt {
    fn bitxor(self, rhs: Self) -> Self;
}

impl BitXorExt for usize {
    #[inline]
    fn bitxor(self, rhs: Self) -> Self {
        self ^ rhs
    }
}

impl Hasher for FxHasher {
    #[inline]
    fn write(&mut self, mut bytes: &[u8]) {
        #[cfg(target_pointer_width = "64")]
        while bytes.len() >= 8 {
            let chunk = u64::from_ne_bytes(bytes[..8].try_into().unwrap());
            self.add_to_hash(chunk as usize);
            bytes = &bytes[8..];
        }
        while bytes.len() >= 4 {
            let chunk = u32::from_ne_bytes(bytes[..4].try_into().unwrap());
            self.add_to_hash(chunk as usize);
            bytes = &bytes[4..];
        }
        if bytes.len() >= 2 {
            let chunk = u16::from_ne_bytes(bytes[..2].try_into().unwrap());
            self.add_to_hash(chunk as usize);
            bytes = &bytes[2..];
        }
        if let Some(&b) = bytes.first() {
            self.add_to_hash(b as usize);
        }
    }

    #[inline]
    fn write_u8(&mut self, i: u8) {
        self.add_to_hash(i as usize);
    }

    #[inline]
    fn write_u16(&mut self, i: u16) {
        self.add_to_hash(i as usize);
    }

    #[inline]
    fn write_u32(&mut self, i: u32) {
        self.add_to_hash(i as usize);
    }

    #[inline]
    fn write_u64(&mut self, i: u64) {
        #[cfg(target_pointer_width = "64")]
        self.add_to_hash(i as usize);
        #[cfg(target_pointer_width = "32")]
        {
            self.add_to_hash(i as usize);
            self.add_to_hash((i >> 32) as usize);
        }
    }

    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.add_to_hash(i);
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.hash as u64
    }
}

#[derive(Clone, Copy, Default)]
pub struct FxBuildHasher;

impl BuildHasher for FxBuildHasher {
    type Hasher = FxHasher;

    #[inline]
    fn build_hasher(&self) -> Self::Hasher {
        FxHasher::default()
    }
}

pub type FxHashMap<K, V> = std::collections::HashMap<K, V, FxBuildHasher>;
pub type FxHashSet<T> = std::collections::HashSet<T, FxBuildHasher>;
pub type HashMap<K, V> = FxHashMap<K, V>;
pub type HashSet<T> = FxHashSet<T>;
pub type TypeIdHashMap<V> =
    std::collections::HashMap<std::any::TypeId, V, crate::gpui_util::TypeIdHashBuilder>;
pub type TypeIdHashSet =
    std::collections::HashSet<std::any::TypeId, crate::gpui_util::TypeIdHashBuilder>;
