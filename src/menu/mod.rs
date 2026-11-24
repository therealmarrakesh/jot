mod bar;
mod model;
mod popup;
mod render;

pub(crate) use bar::{FocusRequest, MenuBar, MenuOutcome, SubmenuTimer};
pub(crate) use model::{Entry, MenuKind, MenuState};

pub(crate) const MENU_BAR_HEIGHT: f32 = 32.;
