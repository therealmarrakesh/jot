#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod color;
mod command;
mod diag;
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
use instance::Arbitration;
use platform::Platform;

pub(crate) const APP_NAME: &str = "Jot";
const NOTICES_FLAG: &str = "--notices";

fn main() {
    diag::install_panic_hook();

    let (flags, mut paths): (Vec<_>, Vec<_>) = std::env::args_os()
        .skip(1)
        .map(PathBuf::from)
        .partition(|arg| arg.as_os_str() == NOTICES_FLAG);
    if !flags.is_empty() {
        match document::write_notices() {
            Ok(path) => paths.push(path),
            Err(err) => diag::debug_log(&format!("Failed to write notices file: {err}")),
        }
    }

    let server = match instance::arbitrate(&paths) {
        Arbitration::HandedOff => return,
        Arbitration::Primary(server) => server,
    };

    let platform = match Platform::new() {
        Ok(p) => Rc::new(p),
        Err(err) => {
            diag::show_error_dialog(APP_NAME, &format!("Jot couldn't start.\n\n{err}"));
            return;
        }
    };
    let app = Rc::new(RefCell::new(App::new(platform)));

    if let Some(ref server) = server {
        let poster = app.borrow().jobs.poster();
        app.borrow_mut().set_pipe_accepting(server.accepting_flag());
        server.attach_handler(move |cmd| {
            poster.post(AppEvent::Handoff(cmd));
        });
    }

    Platform::run(app, move |app| match app.open_window(None, None) {
        Ok((id, _)) => {
            if !paths.is_empty() {
                app.open_paths_in_window(id, paths);
            }
        }
        Err(err) => {
            diag::show_error_dialog(
                APP_NAME,
                &format!("Jot couldn't open a window.\n\n{}", err.message),
            );
            app.request_quit();
        }
    });
    drop(server);
}
