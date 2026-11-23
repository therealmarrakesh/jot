//! Clean-room keyboard focus handles and focus-tree hierarchy.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};

use super::app::App;
use super::keymap::Command;
use super::window::Window;

static NEXT_FOCUS_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct FocusId(pub(crate) u64);

struct FocusInner {
    id: FocusId,
}

/// Strong handle identifying a focusable node in a window.
#[derive(Clone)]
pub(crate) struct FocusHandle {
    inner: Arc<FocusInner>,
}

/// Weak reference to a `FocusHandle`.
#[derive(Clone, Default)]
pub(crate) struct WeakFocusHandle {
    inner: Weak<FocusInner>,
}

impl PartialEq for FocusHandle {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.id() == other.id()
    }
}

impl Eq for FocusHandle {}

impl std::fmt::Debug for FocusHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("FocusHandle").field(&self.id().0).finish()
    }
}

impl FocusHandle {
    pub(crate) fn new() -> Self {
        let id = FocusId(NEXT_FOCUS_ID.fetch_add(1, Ordering::Relaxed));
        Self {
            inner: Arc::new(FocusInner { id }),
        }
    }

    #[inline]
    pub(crate) fn id(&self) -> FocusId {
        self.inner.id
    }

    #[inline]
    pub(crate) fn downgrade(&self) -> WeakFocusHandle {
        WeakFocusHandle {
            inner: Arc::downgrade(&self.inner),
        }
    }

    pub(crate) fn focus(&self, window: &mut Window, cx: &mut App) {
        window.focus_handle(self.clone(), cx);
    }

    pub(crate) fn is_focused(&self, window: &Window) -> bool {
        window.focused_id() == Some(self.id())
    }

    pub(crate) fn contains_focused(&self, window: &Window, _cx: &App) -> bool {
        window.focus_contains(self.id())
    }

    pub(crate) fn dispatch_command(&self, command: Command, window: &mut Window, cx: &mut App) {
        window.dispatch_command_on(self.id(), command, cx);
    }
}

impl WeakFocusHandle {
    #[inline]
    pub(crate) fn upgrade(&self) -> Option<FocusHandle> {
        self.inner.upgrade().map(|inner| FocusHandle { inner })
    }
}
