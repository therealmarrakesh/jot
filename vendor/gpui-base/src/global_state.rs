use std::rc::{Rc, Weak};
use std::time::Instant;

use gpui::{App, Global};

/// Holds the deferred interaction context open for as long as it is alive.
pub struct DeferredPopover(#[allow(dead_code)] Rc<()>);

/// Application-wide state shared by Base behaviors.
pub struct GlobalState {
    deferred_popovers: Vec<Weak<()>>,
    suppress_text_selection: bool,
    last_touch: Option<Instant>,
}

impl Global for GlobalState {}

impl GlobalState {
    fn new() -> Self {
        Self {
            deferred_popovers: Vec::new(),
            suppress_text_selection: false,
            last_touch: None,
        }
    }

    pub fn note_touch(cx: &mut App) {
        Self::init(cx);
        Self::global_mut(cx).last_touch = Some(Instant::now());
    }

    pub fn is_touch_press(cx: &App) -> bool {
        cx.try_global::<Self>()
            .and_then(|state| state.last_touch)
            .is_some_and(|at| at.elapsed() < std::time::Duration::from_secs(1))
    }

    pub fn init(cx: &mut App) {
        if !cx.has_global::<Self>() {
            cx.set_global(Self::new());
        }
    }

    pub fn suppress_text_selection(cx: &mut App) {
        Self::global_mut(cx).suppress_text_selection = true;
    }

    pub fn global(cx: &App) -> &Self {
        cx.global::<Self>()
    }

    pub fn global_mut(cx: &mut App) -> &mut Self {
        cx.global_mut::<Self>()
    }

    pub fn is_in_deferred_context(cx: &App) -> bool {
        Self::global(cx)
            .deferred_popovers
            .iter()
            .any(|popover| popover.strong_count() > 0)
    }

    pub fn register_deferred_popover(cx: &mut App) -> DeferredPopover {
        let token = Rc::new(());
        let state = Self::global_mut(cx);
        state
            .deferred_popovers
            .retain(|popover| popover.strong_count() > 0);
        state.deferred_popovers.push(Rc::downgrade(&token));
        DeferredPopover(token)
    }
}
