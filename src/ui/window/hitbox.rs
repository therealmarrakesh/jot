use std::fmt::Debug;

use crate::ui::{Bounds, CursorStyle, Pixels, Point, Window, smallvec::SmallVec};

#[derive(Clone)]
pub(crate) struct CursorStyleRequest {
    pub(crate) hitbox_id: HitboxId,
    pub(crate) style: CursorStyle,
}

#[derive(Default, Eq, PartialEq)]
pub(crate) struct HitTest {
    pub(crate) ids: SmallVec<[HitboxId; 8]>,
    pub(crate) hover_hitbox_count: usize,
}

/// Window control hit-test region mapped to native Win32 non-client caption/buttons.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WindowControlArea {
    Drag,
    Close,
    Max,
    Min,
}

/// Identifier for a [`Hitbox`].
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub(crate) struct HitboxId(pub(super) u64);

impl HitboxId {
    pub(crate) fn is_hovered(self, window: &Window) -> bool {
        if window.last_input_was_keyboard() {
            return false;
        }
        self.hit_test(window)
    }

    pub(crate) fn is_hovered_ignoring_last_input(self, window: &Window) -> bool {
        self.hit_test(window)
    }

    fn hit_test(self, window: &Window) -> bool {
        let hit_test = &window.mouse_hit_test;
        for id in hit_test.ids.iter().take(hit_test.hover_hitbox_count) {
            if self == *id {
                return true;
            }
        }
        false
    }

    pub(crate) fn should_handle_scroll(self, window: &Window) -> bool {
        window.mouse_hit_test.ids.contains(&self)
    }

    pub(super) fn next(self) -> HitboxId {
        HitboxId(self.0.wrapping_add(1))
    }
}

/// Rectangular hit-test region registered during prepaint.
#[derive(Clone, Debug)]
pub(crate) struct Hitbox {
    pub(crate) id: HitboxId,
    pub(crate) bounds: Bounds<Pixels>,
    pub(crate) content_mask: ContentMask<Pixels>,
    pub(crate) behavior: HitboxBehavior,
}

impl std::ops::Deref for Hitbox {
    type Target = Bounds<Pixels>;
    fn deref(&self) -> &Self::Target {
        &self.bounds
    }
}

impl Hitbox {
    pub(crate) fn is_hovered(&self, window: &Window) -> bool {
        self.id.is_hovered(window)
    }

    pub(crate) fn is_hovered_at(&self, position: Point<Pixels>, window: &Window) -> bool {
        let hit_test = window.rendered_frame.hit_test(position);
        hit_test
            .ids
            .iter()
            .take(hit_test.hover_hitbox_count)
            .any(|id| self.id == *id)
    }

    pub(crate) fn should_handle_scroll(&self, window: &Window) -> bool {
        self.id.should_handle_scroll(window)
    }
}

/// How a hitbox affects mouse hit-testing for hitboxes behind it.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum HitboxBehavior {
    #[default]
    Normal,
    BlockMouse,
    BlockMouseExceptScroll,
}

/// Rectangular clip region applied to painted primitives and hitboxes.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub(crate) struct ContentMask<P: Clone + Debug + Default + PartialEq> {
    pub(crate) bounds: Bounds<P>,
}

impl ContentMask<Pixels> {
    pub(crate) fn intersect(&self, other: &Self) -> Self {
        ContentMask {
            bounds: self.bounds.intersect(&other.bounds),
        }
    }
}
