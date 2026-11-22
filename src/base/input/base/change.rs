use crate::base::input::Selection;

/// One text replacement, in the coordinates of the document immediately before
/// the replacement was applied.
#[derive(Debug, PartialEq, Clone)]
pub(super) struct Change {
    pub(crate) old_range: Selection,
    pub(crate) old_text: String,
    pub(crate) new_range: Selection,
    pub(crate) new_text: String,
}

impl Change {
    pub(super) fn new(
        old_range: impl Into<Selection>,
        old_text: &str,
        new_range: impl Into<Selection>,
        new_text: &str,
    ) -> Self {
        Self {
            old_range: old_range.into(),
            old_text: old_text.to_string(),
            new_range: new_range.into(),
            new_text: new_text.to_string(),
        }
    }
}
