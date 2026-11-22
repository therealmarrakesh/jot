//! Honors the operating system's reduced-motion preference.

use gpui::{App, Global};

#[derive(Default)]
struct SystemReduceMotion {
    applied: Option<bool>,
}

impl Global for SystemReduceMotion {}

pub(crate) fn init(cx: &mut App) {
    apply_system_reduce_motion(cx);
}

pub fn apply_system_reduce_motion(cx: &mut App) {
    if is_test_scheduler(cx) {
        return;
    }
    apply_preference(platform_read(), cx);
}

fn is_test_scheduler(cx: &App) -> bool {
    cx.background_executor().is_test()
}

fn apply_preference(preference: Option<bool>, cx: &mut App) {
    let Some(reduce) = preference else {
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

#[cfg(target_os = "windows")]
fn platform_read() -> Option<bool> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SPI_GETCLIENTAREAANIMATION, SystemParametersInfoW,
    };

    let mut animations_enabled = 0i32;
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            (&raw mut animations_enabled).cast(),
            0,
        )
    } != 0;
    ok.then_some(animations_enabled == 0)
}

#[cfg(not(target_os = "windows"))]
fn platform_read() -> Option<bool> {
    None
}
