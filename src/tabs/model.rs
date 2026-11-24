use std::path::Path;

use crate::document::{self, DocPath, Format, PathKey};
use crate::editor::{Editor, Rope};
use crate::geom::Edges;
use crate::ids::TabId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SlotKey(pub(super) usize);

pub(crate) struct TabData {
    pub(crate) id: TabId,
    pub(crate) editor: Editor,
    pub(crate) zoom: f32,
    pub(crate) doc_path: Option<DocPath>,
    pub(crate) format: Format,
}

impl TabData {
    #[inline]
    pub(crate) fn path(&self) -> Option<&Path> {
        self.doc_path.as_ref().map(DocPath::as_path)
    }

    #[inline]
    pub(crate) fn path_key(&self) -> Option<&PathKey> {
        self.doc_path.as_ref().map(|p| &p.key)
    }

    pub(crate) fn is_dirty(&self) -> bool {
        if self.doc_path.is_some() {
            !self.editor.is_clean()
        } else {
            !self.editor.is_clean() && self.editor.text().len() > 0
        }
    }

    pub(crate) fn create(
        content: Option<document::Loaded>,
        word_wrap: bool,
        paddings: Edges<f32>,
    ) -> Self {
        let (doc_path, text, format) = match content {
            Some(loaded) => (Some(loaded.doc_path), loaded.text, loaded.format),
            None => (None, Rope::new(), Format::default()),
        };
        let editor = Editor::multi_line()
            .with_soft_wrap(word_wrap)
            .with_rope(text)
            .with_paddings(paddings);
        Self {
            id: TabId::next(),
            editor,
            zoom: 1.,
            doc_path,
            format,
        }
    }
}

pub(crate) struct Tab {
    pub(crate) data: TabData,
    pub(crate) slot: SlotKey,
}

impl Tab {
    pub(crate) fn new(data: TabData, slot: SlotKey) -> Self {
        Self { data, slot }
    }
}
