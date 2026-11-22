//! A visual row whose offsets remain in source UTF-8 bytes.
use crate::ui::{App, Pixels, Point, ShapedLine, TextAlign, Window, px};

pub(crate) struct InputLine {
    pub(crate) len: usize,
    pub(crate) width: Pixels,
    content: Content,
}

enum Content {
    Text(Box<ShapedLine>),
    Unshaped,
}

impl From<ShapedLine> for InputLine {
    fn from(line: ShapedLine) -> Self {
        Self {
            len: line.len,
            width: line.width,
            content: Content::Text(Box::new(line)),
        }
    }
}

impl InputLine {
    pub(crate) fn unshaped(len: usize) -> Self {
        Self {
            len,
            width: px(0.),
            content: Content::Unshaped,
        }
    }

    pub(crate) fn is_shaped(&self) -> bool {
        !matches!(self.content, Content::Unshaped)
    }

    pub(crate) fn x_for_index(&self, ix: usize) -> Pixels {
        match &self.content {
            Content::Text(line) => line.x_for_index(ix),
            Content::Unshaped => px(0.),
        }
    }

    pub(crate) fn closest_index_for_x(&self, x: Pixels) -> usize {
        match &self.content {
            Content::Text(line) => line.closest_index_for_x(x),
            Content::Unshaped => 0,
        }
    }

    pub(crate) fn paint(
        &self,
        pos: Point<Pixels>,
        height: Pixels,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Content::Text(line) = &self.content {
            let _ = line.paint(pos, height, TextAlign::Left, None, window, cx);
        }
    }
}
