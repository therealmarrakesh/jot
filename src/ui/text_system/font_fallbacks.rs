use std::sync::Arc;

/// The fallback fonts that can be configured for a given font.
/// Fallback fonts family names are stored here.
#[derive(Default, Clone, Eq, PartialEq, Hash, Debug)]
pub(crate) struct FontFallbacks(pub(crate) Arc<Vec<String>>);

impl FontFallbacks {
    /// Get the fallback fonts family names
    pub(crate) fn fallback_list(&self) -> &[String] {
        self.0.as_slice()
    }
}
