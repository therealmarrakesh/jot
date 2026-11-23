mod clipboard;
mod os;
mod pipe;

#[cfg(not(feature = "d2d"))]
mod dialog;
#[cfg(not(feature = "d2d"))]
mod direct_manipulation;
#[cfg(not(feature = "d2d"))]
mod direct_write;
#[cfg(not(feature = "d2d"))]
mod directx_atlas;
#[cfg(not(feature = "d2d"))]
mod directx_devices;
#[cfg(not(feature = "d2d"))]
mod directx_renderer;
#[cfg(not(feature = "d2d"))]
mod dispatcher;
#[cfg(not(feature = "d2d"))]
mod display;
#[cfg(not(feature = "d2d"))]
mod events;
#[cfg(not(feature = "d2d"))]
mod keyboard;
#[cfg(not(feature = "d2d"))]
mod platform;
#[cfg(not(feature = "d2d"))]
mod system_settings;
#[cfg(not(feature = "d2d"))]
mod util;
#[cfg(not(feature = "d2d"))]
mod vsync;
#[cfg(not(feature = "d2d"))]
mod window;
#[cfg(not(feature = "d2d"))]
mod wrapper;

pub(crate) use clipboard::*;
pub(crate) use os::*;
pub(crate) use pipe::*;

#[cfg(not(feature = "d2d"))]
pub(crate) use direct_write::*;
#[cfg(not(feature = "d2d"))]
pub(crate) use directx_atlas::*;
#[cfg(not(feature = "d2d"))]
pub(crate) use directx_devices::*;
#[cfg(not(feature = "d2d"))]
pub(crate) use directx_renderer::*;
#[cfg(not(feature = "d2d"))]
pub(crate) use dispatcher::*;
#[cfg(not(feature = "d2d"))]
pub(crate) use events::*;
#[cfg(not(feature = "d2d"))]
pub(crate) use keyboard::*;
#[cfg(not(feature = "d2d"))]
pub(crate) use platform::*;
#[cfg(not(feature = "d2d"))]
pub(crate) use system_settings::*;
#[cfg(not(feature = "d2d"))]
pub(crate) use util::*;
#[cfg(not(feature = "d2d"))]
pub(crate) use vsync::*;
#[cfg(not(feature = "d2d"))]
pub(crate) use window::*;
#[cfg(not(feature = "d2d"))]
pub(crate) use wrapper::*;

#[cfg(not(feature = "d2d"))]
pub(crate) use display::WindowsDisplay;
#[cfg(not(feature = "d2d"))]
pub(crate) use platform::{SaveChoice, SavePathOptions, WindowsPlatform, prompt_for_save_path};
#[cfg(not(feature = "d2d"))]
pub(crate) use window::set_window_min_size;

#[cfg(feature = "d2d")]
pub(crate) use crate::ui::win32_bridge::*;

#[cfg(not(feature = "d2d"))]
pub(crate) use windows::Win32::Foundation::HWND;
