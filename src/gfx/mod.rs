#![allow(unsafe_code)]

pub(crate) mod d2d;
pub(crate) mod text;
pub(crate) mod wrap;

pub(crate) use d2d::{D2DRenderer, Gpu, PaintQuad, Surface, fill};
pub(crate) use text::{
    Font, FontWeight, Label, LabelLayout, ShapedRow, TextAlign, TextStyle, TextSystem, font,
};
pub(crate) use wrap::{AdvanceTable, break_opportunities};
