use std::{
    any::TypeId,
    fmt::Debug,
    hash::{Hash, Hasher},
    marker::PhantomData,
};

use crate::ui::{
    AnyView, App, AppContext, Context, Render, Window,
    anyhow::{Context as _, Result, anyhow},
};

crate::ui::new_key_type! {
    /// A unique identifier for a window.
    pub(crate) struct WindowId;
}

impl WindowId {
    pub(crate) fn as_u64(&self) -> u64 {
        self.0.as_ffi()
    }
}

impl From<u64> for WindowId {
    fn from(value: u64) -> Self {
        WindowId(crate::ui::slotmap::KeyData::from_ffi(value))
    }
}

/// Typed handle to a window with root view `V`.
pub(crate) struct WindowHandle<V> {
    pub(crate) any_handle: AnyWindowHandle,
    state_type: PhantomData<fn(V) -> V>,
}

impl<V> std::ops::Deref for WindowHandle<V> {
    type Target = AnyWindowHandle;
    fn deref(&self) -> &Self::Target {
        &self.any_handle
    }
}

impl<V> std::ops::DerefMut for WindowHandle<V> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.any_handle
    }
}

impl<V> Debug for WindowHandle<V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WindowHandle")
            .field("any_handle", &self.any_handle.id.as_u64())
            .finish()
    }
}

impl<V: 'static + Render> WindowHandle<V> {
    pub(crate) fn new(id: WindowId) -> Self {
        WindowHandle {
            any_handle: AnyWindowHandle {
                id,
                state_type: TypeId::of::<V>(),
            },
            state_type: PhantomData,
        }
    }

    pub(crate) fn update<C, R>(
        &self,
        cx: &mut C,
        update: impl FnOnce(&mut V, &mut Window, &mut Context<V>) -> R,
    ) -> Result<R>
    where
        C: AppContext,
    {
        cx.update_window(self.any_handle, |root_view, window, cx| {
            let view = root_view
                .downcast::<V>()
                .map_err(|_| anyhow!("the type of the window's root view has changed"))?;

            Ok(view.update(cx, |view, cx| update(view, window, cx)))
        })?
    }

    pub(crate) fn read<'a>(&self, cx: &'a App) -> Result<&'a V> {
        let view = cx
            .windows
            .get(self.id)
            .and_then(|window| {
                window
                    .as_deref()
                    .and_then(|window| window.root.clone())
                    .map(|root_view| root_view.downcast::<V>())
            })
            .context("window not found")?
            .map_err(|_| anyhow!("the type of the window's root view has changed"))?;

        Ok(view.read(cx))
    }
}

impl<V> Copy for WindowHandle<V> {}

impl<V> Clone for WindowHandle<V> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<V> PartialEq for WindowHandle<V> {
    fn eq(&self, other: &Self) -> bool {
        self.any_handle == other.any_handle
    }
}

impl<V> Eq for WindowHandle<V> {}

impl<V> Hash for WindowHandle<V> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.any_handle.hash(state);
    }
}

impl<V: 'static> From<WindowHandle<V>> for AnyWindowHandle {
    fn from(val: WindowHandle<V>) -> Self {
        val.any_handle
    }
}

/// Type-erased handle to any open window.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) struct AnyWindowHandle {
    pub(crate) id: WindowId,
    state_type: TypeId,
}

impl AnyWindowHandle {
    pub(crate) fn window_id(&self) -> WindowId {
        self.id
    }

    pub(crate) fn downcast<T: 'static>(&self) -> Option<WindowHandle<T>> {
        if TypeId::of::<T>() == self.state_type {
            Some(WindowHandle {
                any_handle: *self,
                state_type: PhantomData,
            })
        } else {
            None
        }
    }

    pub(crate) fn update<C, R>(
        self,
        cx: &mut C,
        update: impl FnOnce(AnyView, &mut Window, &mut App) -> R,
    ) -> Result<R>
    where
        C: AppContext,
    {
        cx.update_window(self, update)
    }
}
