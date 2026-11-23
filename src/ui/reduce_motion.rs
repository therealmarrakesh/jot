//! Honors Windows' client-area animation preference.

use crate::ui::{App, Global};

#[derive(Default)]
struct SystemReduceMotion {
    applied: Option<bool>,
}

impl Global for SystemReduceMotion {}

pub(crate) fn init(cx: &mut App) {
    let Some(animations_enabled) = crate::win32::client_area_animations_enabled() else {
        return;
    };
    let reduce = !animations_enabled;
    let applied = cx
        .try_global::<SystemReduceMotion>()
        .and_then(|state| state.applied);
    if cx.reduce_motion() != applied.unwrap_or(false) {
        return;
    }
    cx.set_reduce_motion(reduce);
    cx.default_global::<SystemReduceMotion>().applied = Some(reduce);
}
