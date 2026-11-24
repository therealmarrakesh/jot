#![allow(unsafe_code)]

pub(crate) mod d2d;
pub(crate) mod text;
pub(crate) mod wrap;

pub(crate) use d2d::{Canvas, Gpu, PaintQuad, Scope, Surface, fill};
pub(crate) use text::{
    FontFace, FontWeight, Label, LabelLayout, ShapedRow, TextAlign, TextStyle, TextSystem,
};

use std::collections::HashMap;
use std::hash::Hash;

pub(super) struct FrameCache<K, V> {
    entries: HashMap<K, (V, u64, usize)>,
    frame: u64,
    max_entries: usize,
    max_weight: usize,
    weight: usize,
    keep_frames: u64,
}

impl<K: Eq + Hash, V> FrameCache<K, V> {
    pub(super) fn new(max_entries: usize, max_weight: usize, keep_frames: u64) -> Self {
        Self {
            entries: HashMap::new(),
            frame: 0,
            max_entries,
            max_weight,
            weight: 0,
            keep_frames,
        }
    }

    pub(super) fn get(&mut self, key: &K) -> Option<&V> {
        let entry = self.entries.get_mut(key)?;
        entry.1 = self.frame;
        Some(&entry.0)
    }

    pub(super) fn insert(&mut self, key: K, value: V, item_weight: usize) {
        if self.entries.contains_key(&key) {
            if let Some((_, _, old_w)) = self.entries.insert(key, (value, self.frame, item_weight)) {
                self.weight = self.weight.saturating_sub(old_w) + item_weight;
            }
            return;
        }
        while self.entries.len() >= self.max_entries
            || (self.max_weight > 0
                && !self.entries.is_empty()
                && self.weight.saturating_add(item_weight) > self.max_weight)
        {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, (_, last, _))| *last)
                .map(|(k, _)| k as *const K)
            else {
                break;
            };
            // Find and remove oldest without requiring K: Clone.
            let mut drop_idx = None;
            for (k, (_, last, _)) in &self.entries {
                if std::ptr::eq(k, oldest) {
                    drop_idx = Some(*last);
                    break;
                }
            }
            let Some(cutoff) = drop_idx else { break };
            let mut removed_one = false;
            self.entries.retain(|_, (_, last, w)| {
                if !removed_one && *last == cutoff {
                    removed_one = true;
                    self.weight = self.weight.saturating_sub(*w);
                    false
                } else {
                    true
                }
            });
            if !removed_one {
                break;
            }
        }
        self.weight += item_weight;
        self.entries.insert(key, (value, self.frame, item_weight));
    }

    pub(super) fn advance_frame(&mut self) {
        self.frame = self.frame.wrapping_add(1);
        let frame = self.frame;
        let keep = self.keep_frames;
        let weight = &mut self.weight;
        self.entries.retain(|_, (_, last, w)| {
            let alive = frame.wrapping_sub(*last) <= keep;
            if !alive {
                *weight = weight.saturating_sub(*w);
            }
            alive
        });
    }

    pub(super) fn clear(&mut self) {
        self.entries.clear();
        self.weight = 0;
    }

    #[cfg(test)]
    pub(super) fn age_counts(&self) -> (usize, usize) {
        let mut curr = 0;
        let mut prev = 0;
        for (_, last, _) in self.entries.values() {
            if *last == self.frame {
                curr += 1;
            } else {
                prev += 1;
            }
        }
        (curr, prev)
    }
}
