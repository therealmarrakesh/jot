mod bar;
mod matcher;

pub(crate) use bar::{FindBar, FindOutcome, NOTICE_DURATION};
#[cfg(test)]
pub(crate) use matcher::Matcher;
