mod bar;
mod model;
mod popup;
mod render;

pub(crate) use bar::{FocusRequest, MenuBar, MenuOutcome, RowPath};
pub(crate) use model::{Entry, MenuState};

pub(crate) const MENU_BAR_HEIGHT: f32 = 32.;
