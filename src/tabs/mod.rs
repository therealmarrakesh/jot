mod model;
mod set;
mod view;

pub(crate) use model::TabData;
pub(crate) use set::{
    LIST_PAD, MIN_STRIP_WIDTH, Removal, SCROLL_REPEAT_DELAY, SCROLL_REPEAT_INTERVAL, SCROLL_STEP,
    STRIP_END_RESERVE, TAB_HEIGHT, TabSet, WHEEL_SHARE,
};
pub(crate) use view::StripFrame;
