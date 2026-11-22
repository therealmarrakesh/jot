/// Display mapping system for Input (soft-wrapping).
mod display_map;
mod inline_line;
pub(crate) use inline_line::InputLine;
mod text_wrapper;
mod wrap_map;

pub use self::display_map::{DisplayMap, WrappingIndent};
pub(crate) use self::text_wrapper::LineLayout;

/// Position in the buffer (logical text).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BufferPoint {
    pub line: usize,
    pub col: usize,
}

impl BufferPoint {
    pub fn new(line: usize, col: usize) -> Self {
        Self { line, col }
    }
}

/// Position after soft-wrapping (internal).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct WrapPoint {
    pub row: usize,
    pub col: usize,
}

impl WrapPoint {
    pub(super) fn new(row: usize, col: usize) -> Self {
        Self { row, col }
    }
}

/// Final display position (after soft-wrapping).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DisplayPoint {
    pub row: usize,
    pub col: usize,
}

impl DisplayPoint {
    pub fn new(row: usize, col: usize) -> Self {
        Self { row, col }
    }
}
