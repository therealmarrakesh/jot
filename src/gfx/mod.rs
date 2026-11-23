//! Direct3D 11 + DirectComposition + Direct2D 1.1 rendering and DirectWrite text shaping.

pub(crate) mod d2d;
pub(crate) mod text;
pub(crate) mod wrap;

pub(crate) use d2d::{D2DRenderer, Direct2DDevices, PaintQuad, fill};
pub(crate) use text::{
    Font, FontWeight, ShapedLine, TextAlign, TextRun, TextSystem, UnderlineStyle, WrappedLine,
    font,
};
pub(crate) use wrap::{Boundary, MAX_WRAP_INDENT};
#[cfg(test)]
pub(crate) use wrap::line_break_opportunities;
