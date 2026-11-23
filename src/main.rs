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
mod instance;
mod keytips;
mod menu;
mod platform;
mod scrollbar;
mod tab_drag;
mod tabs;
mod theme;
mod tooltip;
mod types;
mod ui;
mod window;

#[cfg(test)]
mod tests;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use app::{App, AppEvent};
use platform::Platform;

pub(crate) const APP_NAME: &str = "Jot";
const NOTICES_FLAG: &str = "--notices";

fn main() {
    let (flags, mut paths): (Vec<_>, Vec<_>) = std::env::args_os()
        .skip(1)
        .map(PathBuf::from)
        .partition(|arg| arg.as_os_str() == NOTICES_FLAG);
    if !flags.is_empty() {
        paths.extend(document::write_notices().ok());
    }
    if !paths.is_empty() && instance::try_handoff(&paths) {
        return;
    }

    let platform = Rc::new(Platform::new().expect("Failed to initialize Windows platform"));
    let app = Rc::new(RefCell::new(App::new(platform)));

    let poster = app.borrow().jobs.poster();
    instance::start_listener(move |incoming| {
        poster.post(AppEvent::HandoffPaths(incoming));
    });

    Platform::run(app, move |app| {
        if let Some((id, _)) = app.open_window(None, None)
            && !paths.is_empty()
            && let Some(ix) = app.window_ix(id)
        {
            app.windows[ix].open_paths(paths, &app.jobs);
        }
    });
}
