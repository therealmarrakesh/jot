use std::{
    cell::{Cell, RefCell},
    thread::AccessError,
};

use crate::ui::{App, Arena};

thread_local! {
    /// Fallback arena used when no app-specific arena is active.
    pub(crate) static ELEMENT_ARENA: RefCell<Arena> = RefCell::new(Arena::new(1024 * 1024));

    /// Points to the current App's element arena during draw operations.
    static CURRENT_ELEMENT_ARENA: Cell<Option<*const RefCell<Arena>>> = const { Cell::new(None) };
}

/// Whether a window draw is currently in progress on this thread.
pub(super) fn draw_in_progress() -> bool {
    CURRENT_ELEMENT_ARENA.with(|current| current.get().is_some())
}

/// Allocates an element in the current arena (or falls back to `ELEMENT_ARENA`).
#[inline(always)]
pub(crate) fn with_element_arena<R>(callback: impl FnOnce(&mut Arena) -> R) -> R {
    let mut callback = Some(callback);
    let mut result = None;
    let access = with_element_arena_erased(&mut |arena| {
        result = Some(callback.take().expect("arena callback runs once")(arena));
    });
    drop(callback);
    access.expect("cannot access a Thread Local Storage value during or after destruction");
    result.expect("arena callback produces a result")
}

/// Scope guard that sets `CURRENT_ELEMENT_ARENA` for a draw and tracks nesting depth.
pub(crate) struct ElementArenaScope {
    entered: *const RefCell<Arena>,
    previous: Option<*const RefCell<Arena>>,
    exited: bool,
}

impl ElementArenaScope {
    pub(crate) fn enter(arena: &RefCell<Arena>) -> Self {
        arena.borrow_mut().begin_scope();
        let previous = CURRENT_ELEMENT_ARENA.with(|current| {
            let prev = current.get();
            current.set(Some(arena as *const RefCell<Arena>));
            prev
        });
        Self {
            entered: arena as *const RefCell<Arena>,
            previous,
            exited: false,
        }
    }

    pub(crate) fn exit(mut self, arena: &RefCell<Arena>) -> ArenaClearNeeded {
        assert!(
            std::ptr::eq(self.entered, arena),
            "ElementArenaScope::exit called with a different arena than was entered"
        );
        self.exited = true;
        ArenaClearNeeded::new(arena)
    }
}

impl Drop for ElementArenaScope {
    fn drop(&mut self) {
        CURRENT_ELEMENT_ARENA.with(|current| {
            current.set(self.previous);
        });
        // SAFETY: `entered` came from `&RefCell<Arena>` in `enter`, owned by the `App`
        // outliving this draw guard on both normal and unwinding paths.
        unsafe { &*self.entered }.borrow_mut().end_scope();
        if !self.exited && !std::thread::panicking() {
            debug_assert!(false, "ElementArenaScope dropped without calling exit()");
        }
    }
}

/// Token indicating the element arena was drawn into and must be cleared before the next draw.
#[must_use]
pub(crate) struct ArenaClearNeeded {
    arena: *const RefCell<Arena>,
}

impl ArenaClearNeeded {
    fn new(arena: &RefCell<Arena>) -> Self {
        Self {
            arena: arena as *const RefCell<Arena>,
        }
    }

    pub(crate) fn clear(self, cx: &mut App) {
        assert!(
            std::ptr::eq(self.arena, &cx.element_arena),
            "ArenaClearNeeded::clear called with a different App than the draw ran against"
        );
        cx.element_arena.borrow_mut().clear();
    }
}

#[inline(never)]
fn with_element_arena_erased(callback: &mut dyn FnMut(&mut Arena)) -> Result<(), AccessError> {
    if let Some(arena_pointer) = CURRENT_ELEMENT_ARENA.try_with(Cell::get)? {
        // SAFETY: Valid for the duration of the draw that set it.
        let arena_cell = unsafe { &*arena_pointer };
        callback(&mut arena_cell.borrow_mut());
        Ok(())
    } else {
        ELEMENT_ARENA.try_with(|arena| callback(&mut arena.borrow_mut()))
    }
}
