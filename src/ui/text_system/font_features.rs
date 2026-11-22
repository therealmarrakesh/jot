use std::sync::Arc;

/// The OpenType features that can be configured for a given font.
#[derive(Default, Clone, Eq, PartialEq, Hash)]
pub(crate) struct FontFeatures(pub(crate) Arc<Vec<(String, u32)>>);

impl FontFeatures {
    /// Get the tag name list of the font OpenType features
    /// only enabled or disabled features are returned
    pub(crate) fn tag_value_list(&self) -> &[(String, u32)] {
        self.0.as_slice()
    }
}

impl std::fmt::Debug for FontFeatures {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug = f.debug_struct("FontFeatures");
        for (tag, value) in self.tag_value_list() {
            debug.field(tag, value);
        }

        debug.finish()
    }
}
