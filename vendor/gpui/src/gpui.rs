//! Trimmed Windows-only GPUI core for Jot.
#![allow(missing_docs)]
#![allow(clippy::type_complexity)]
#![allow(clippy::collapsible_else_if)]
#![allow(unused_mut)]
#![allow(dead_code)]

extern crate self as gpui;
#[macro_use]
mod action;
mod app;

mod arena;
mod assets;
mod bounds_tree;
mod color;
pub mod colors;
mod element;
mod elements;
mod executor;
mod platform_scheduler;
pub(crate) use platform_scheduler::PlatformScheduler;
pub mod scheduler;
mod shared_string;
mod geometry;
mod gestures;
mod global;
mod input;
mod inspector;
mod interactive;
mod key_dispatch;
mod keymap;
mod path_builder;
mod platform;
pub mod prelude;
#[expect(missing_docs)]
pub mod queue;
mod scene;
mod style;
mod styled;
mod subscription;
mod tab_stop;
mod taffy;
mod text_system;
mod util;
mod view;
mod window;

#[doc(hidden)]
pub mod private {
    pub use anyhow;
    pub use rand;
}

mod seal {
    pub trait Sealed {}
}

pub use action::*;
pub use anyhow::Result;
pub use app::*;
pub(crate) use arena::*;
pub use assets::*;
pub use color::*;
pub use ctor::ctor;
pub use element::*;
pub use elements::*;
pub use executor::*;
pub use geometry::*;
pub use gestures::*;
pub use global::*;
pub use gpui_macros::{AppContext, IntoElement, Render, VisualContext};
pub use gpui_util::arc_cow::ArcCow;
pub use shared_string::*;
pub use input::*;
pub use inspector::*;
pub use interactive::*;
use key_dispatch::*;
pub use keymap::*;
pub use path_builder::*;
pub use platform::*;
pub use queue::{PriorityQueueReceiver, PriorityQueueSender};
pub use refineable::*;
pub use scene::*;
use std::{any::Any, future::Future};
pub use style::*;
pub use styled::*;
pub use subscription::*;
pub(crate) use tab_stop::*;
use taffy::TaffyLayoutEngine;
pub use taffy::{AvailableSpace, LayoutId};
pub use text_system::*;
pub use util::{FutureExt, Timeout};
pub use view::*;
pub use window::*;

pub use pollster::block_on;

pub trait AppContext {
    #[expect(
        clippy::wrong_self_convention,
        reason = "`App::new` is an ubiquitous function for creating entities"
    )]
    fn new<T: 'static>(&mut self, build_entity: impl FnOnce(&mut Context<T>) -> T) -> Entity<T>;
    fn reserve_entity<T: 'static>(&mut self) -> Reservation<T>;
    fn insert_entity<T: 'static>(
        &mut self,
        reservation: Reservation<T>,
        build_entity: impl FnOnce(&mut Context<T>) -> T,
    ) -> Entity<T>;
    fn update_entity<T, R>(
        &mut self,
        handle: &Entity<T>,
        update: impl FnOnce(&mut T, &mut Context<T>) -> R,
    ) -> R
    where
        T: 'static;
    fn as_mut<'a, T>(&'a mut self, handle: &Entity<T>) -> GpuiBorrow<'a, T>
    where
        T: 'static;
    fn read_entity<T, R>(&self, handle: &Entity<T>, read: impl FnOnce(&T, &App) -> R) -> R
    where
        T: 'static;
    fn update_window<T, F>(&mut self, window: AnyWindowHandle, f: F) -> Result<T>
    where
        F: FnOnce(AnyView, &mut Window, &mut App) -> T;
    fn with_window<R>(
        &mut self,
        entity_id: EntityId,
        f: impl FnOnce(&mut Window, &mut App) -> R,
    ) -> Option<R>;
    fn read_window<T, R>(
        &self,
        window: &WindowHandle<T>,
        read: impl FnOnce(Entity<T>, &App) -> R,
    ) -> Result<R>
    where
        T: 'static;
    fn background_spawn<R>(&self, future: impl Future<Output = R> + Send + 'static) -> Task<R>
    where
        R: Send + 'static;
    fn read_global<G, R>(&self, callback: impl FnOnce(&G, &App) -> R) -> R
    where
        G: Global;
}

pub struct Reservation<T>(pub(crate) Slot<T>);

impl<T: 'static> Reservation<T> {
    pub fn entity_id(&self) -> EntityId {
        self.0.entity_id()
    }
}

pub trait VisualContext: AppContext {
    type Result<T>;

    fn window_handle(&self) -> AnyWindowHandle;
    fn update_window_entity<T: 'static, R>(
        &mut self,
        entity: &Entity<T>,
        update: impl FnOnce(&mut T, &mut Window, &mut Context<T>) -> R,
    ) -> Self::Result<R>;
    fn new_window_entity<T: 'static>(
        &mut self,
        build_entity: impl FnOnce(&mut Window, &mut Context<T>) -> T,
    ) -> Self::Result<Entity<T>>;
    fn replace_root_view<V>(
        &mut self,
        build_view: impl FnOnce(&mut Window, &mut Context<V>) -> V,
    ) -> Self::Result<Entity<V>>
    where
        V: 'static + Render;
    fn focus<V>(&mut self, entity: &Entity<V>) -> Self::Result<()>
    where
        V: Focusable;
}

pub trait EventEmitter<E: Any>: 'static {}

pub trait BorrowAppContext {
    fn set_global<T: Global>(&mut self, global: T);
    fn update_global<G, R>(&mut self, f: impl FnOnce(&mut G, &mut Self) -> R) -> R
    where
        G: Global;
    fn update_default_global<G, R>(&mut self, f: impl FnOnce(&mut G, &mut Self) -> R) -> R
    where
        G: Global + Default;
}

impl<C> BorrowAppContext for C
where
    C: std::borrow::BorrowMut<App>,
{
    fn set_global<G: Global>(&mut self, global: G) {
        self.borrow_mut().set_global(global)
    }

    #[track_caller]
    fn update_global<G, R>(&mut self, f: impl FnOnce(&mut G, &mut Self) -> R) -> R
    where
        G: Global,
    {
        let mut global = self.borrow_mut().lease_global::<G>();
        let result = f(&mut global, self);
        self.borrow_mut().end_global_lease(global);
        result
    }

    fn update_default_global<G, R>(&mut self, f: impl FnOnce(&mut G, &mut Self) -> R) -> R
    where
        G: Global + Default,
    {
        self.borrow_mut().default_global::<G>();
        self.update_global(f)
    }
}

#[derive(Default, Debug, Clone)]
pub struct GpuSpecs {
    pub is_software_emulated: bool,
    pub device_name: String,
    pub driver_name: String,
    pub driver_info: String,
}
