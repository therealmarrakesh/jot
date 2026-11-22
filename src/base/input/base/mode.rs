use crate::base::input::TabSize;

/// How the input lays its text out: rows and tab width.
#[derive(Clone)]
pub(crate) enum LayoutMode {
    PlainText { tab: TabSize, rows: usize },
}

impl Default for LayoutMode {
    fn default() -> Self {
        LayoutMode::plain_text()
    }
}

#[allow(unused)]
impl LayoutMode {
    pub(super) fn plain_text() -> Self {
        LayoutMode::PlainText {
            tab: TabSize::default(),
            rows: 1,
        }
    }

    pub(super) fn rows(&self) -> usize {
        let LayoutMode::PlainText { rows, .. } = self;
        (*rows).max(1)
    }
}
