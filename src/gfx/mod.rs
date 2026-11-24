#![allow(unsafe_code)]

pub(crate) mod d2d;
mod line_breaks;
pub(crate) mod text;

pub(crate) use d2d::{Canvas, Gpu, PaintQuad, Scope, WindowSurface, fill};
pub(crate) use text::{
    FontFace, FontWeight, Label, LabelLayout, ShapedRow, TextStyle, TextSystem,
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

impl<K: Eq + Hash + Clone, V> FrameCache<K, V> {
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
        if let Some(entry) = self.entries.get_mut(&key) {
            self.weight = self.weight.saturating_sub(entry.2) + item_weight;
            *entry = (value, self.frame, item_weight);
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
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            if let Some((_, _, w)) = self.entries.remove(&oldest) {
                self.weight = self.weight.saturating_sub(w);
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
