//! Honors Windows' client-area animation preference.

use gpui::{App, Global};
use windows::Win32::UI::WindowsAndMessaging::{
    SPI_GETCLIENTAREAANIMATION, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
};

#[derive(Default)]
struct SystemReduceMotion {
    applied: Option<bool>,
}

impl Global for SystemReduceMotion {}

pub(crate) fn init(cx: &mut App) {
    let mut animations_enabled = 0i32;
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some((&raw mut animations_enabled).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    }
    .is_ok();
    let Some(reduce) = ok.then_some(animations_enabled == 0) else {
        return;
    };
    let applied = cx
        .try_global::<SystemReduceMotion>()
        .and_then(|state| state.applied);
    if cx.reduce_motion() != applied.unwrap_or(false) {
        return;
    }
    cx.set_reduce_motion(reduce);
    cx.default_global::<SystemReduceMotion>().applied = Some(reduce);
}
