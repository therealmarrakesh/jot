#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod color;
mod command;
mod dialog;
mod document;
mod editor;
mod files;
mod find;
mod geom;
mod gfx;
mod ids;
mod keytips;
mod menu;
mod platform;
mod shell;
mod tabs;
mod theme;
mod tooltip;
mod types;
mod ui;
mod widgets;
mod window;

#[cfg(test)]
mod tests;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use app::App;
use gfx::{Gpu, TextSystem};
use platform::{Arbitration, Platform, arbitrate_instance, diag};

pub(crate) const APP_NAME: &str = "Jot";
const NOTICES_FLAG: &str = "--notices";
const NOTICES: &str = include_str!(concat!(env!("OUT_DIR"), "/notices.txt"));
const NOTICES_FILE: &str = "THIRD-PARTY-NOTICES.txt";

fn write_notices() -> std::io::Result<PathBuf> {
    let dir = std::env::temp_dir().join(APP_NAME);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(NOTICES_FILE);
    std::fs::write(&path, NOTICES)?;
    Ok(path)
}

fn main() {
    diag::install_panic_hook();

    let (flags, mut paths): (Vec<_>, Vec<_>) = std::env::args_os()
        .skip(1)
        .map(PathBuf::from)
        .partition(|arg| arg.as_os_str() == NOTICES_FLAG);
    if !flags.is_empty() {
        match write_notices() {
            Ok(path) => paths.push(path),
            Err(err) => diag::debug_log(&format!("Failed to write notices file: {err}")),
        }
    }

    let lock = match arbitrate_instance(&paths) {
        Arbitration::HandedOff => return,
        Arbitration::Primary(lock) => lock,
    };

    let platform = match Platform::new(lock) {
        Ok(p) => Rc::new(p),
        Err(err) => {
            diag::show_error_dialog(APP_NAME, &format!("Jot couldn't start.\n\n{err}"));
            return;
        }
    };
    let gpu = match Gpu::new() {
        Ok(g) => g,
        Err(err) => {
            diag::show_error_dialog(APP_NAME, &format!("Jot couldn't start.\n\n{err}"));
            return;
        }
    };
    let text_system = match TextSystem::new() {
        Ok(ts) => ts,
        Err(err) => {
            diag::show_error_dialog(APP_NAME, &format!("Jot couldn't start.\n\n{err}"));
            return;
        }
    };
    let app = Rc::new(RefCell::new(App::new(platform, gpu, text_system)));

    Platform::run(app, move |app| {
        if let Err(message) = app.launch(paths, false) {
            diag::show_error_dialog(APP_NAME, &format!("Jot couldn't open a window.\n\n{message}"));
            app.request_quit();
        }
    });
}
