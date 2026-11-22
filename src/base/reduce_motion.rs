//! Honors Windows' client-area animation preference.

use gpui::{App, Global};
use windows::Win32::UI::WindowsAndMessaging::SPI_GETCLIENTAREAANIMATION;

#[derive(Default)]
struct SystemReduceMotion {
    applied: Option<bool>,
}

impl Global for SystemReduceMotion {}

pub(crate) fn init(cx: &mut App) {
    let Some(animations_enabled) = gpui::system_parameters_u32(SPI_GETCLIENTAREAANIMATION.0) else {
        return;
    };
    let reduce = animations_enabled == 0;
    let applied = cx
        .try_global::<SystemReduceMotion>()
        .and_then(|state| state.applied);
    if cx.reduce_motion() != applied.unwrap_or(false) {
        return;
    }
    cx.set_reduce_motion(reduce);
    cx.default_global::<SystemReduceMotion>().applied = Some(reduce);
}
