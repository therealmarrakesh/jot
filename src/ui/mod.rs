//! Trimmed Windows-only GPUI core for Jot.
#![allow(missing_docs)]

mod action;
pub(crate) mod anyhow;
mod app;

mod arena;
mod bounds_tree;
pub(crate) mod collections;
mod color;
mod common_actions;
mod element;
mod elements;
mod executor;
pub(crate) mod futures;
pub(crate) mod gpui_util;
mod shared_string;
pub(crate) mod slotmap;
pub(crate) mod smallvec;
mod geometry;
mod global;
mod input;
mod interactive;
mod key_dispatch;
mod keymap;
mod layout;
pub(crate) mod motion;
mod platform;
pub(crate) mod reduce_motion;
mod refineable;
mod scene;
mod style;
mod styled;
mod subscription;
pub(crate) mod sync;
mod text_system;
mod util;
mod view;
mod window;

mod seal {
    pub(crate) trait Sealed {}
}

pub(crate) use action::*;
pub(crate) use crate::ui::anyhow::Result;
pub(crate) use app::*;
pub(crate) use arena::*;
pub(crate) use color::*;
pub(crate) use common_actions::*;
pub(crate) use element::*;
pub(crate) use elements::*;
pub(crate) use executor::*;
pub(crate) use geometry::*;
pub(crate) use global::*;
pub(crate) use gpui_util::{debug_panic, maybe};
pub(crate) use motion::{Easing, Interpolate, MotionStatus, transition, transition_from};
pub(crate) use shared_string::*;
pub(crate) use input::*;
pub(crate) use interactive::*;
use key_dispatch::*;
pub(crate) use keymap::*;
pub(crate) use platform::*;
pub(crate) use crate::win32::{
    SaveChoice, SavePathOptions, WindowsDisplay, WindowsPlatform, prompt_for_save_path,
    set_window_min_size, system_parameters_u32,
};
pub(crate) use slotmap::new_key_type;
pub(crate) use refineable::*;
pub(crate) use scene::*;
pub(crate) use style::*;
pub(crate) use styled::*;
pub(crate) use subscription::*;
use layout::LayoutEngine;
pub(crate) use layout::{AvailableSpace, LayoutId};
pub(crate) use text_system::*;
pub(crate) use util::FluentBuilder;
pub(crate) use view::*;
pub(crate) use window::*;

pub(crate) trait AppContext {
    #[expect(
        clippy::wrong_self_convention,
        reason = "`App::new` is an ubiquitous function for creating entities"
    )]
    #[allow(clippy::new_ret_no_self)]
    fn new<T: 'static>(&mut self, build_entity: impl FnOnce(&mut Context<T>) -> T) -> Entity<T>;
    fn update_entity<T, R>(
        &mut self,
        handle: &Entity<T>,
        update: impl FnOnce(&mut T, &mut Context<T>) -> R,
    ) -> R
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
}

pub(crate) trait BorrowAppContext {
    fn update_global<G, R>(&mut self, f: impl FnOnce(&mut G, &mut Self) -> R) -> R
    where
        G: Global;
}

impl<C> BorrowAppContext for C
where
    C: std::borrow::BorrowMut<App>,
{
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
}
