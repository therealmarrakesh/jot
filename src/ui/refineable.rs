//! Hierarchical partial-override trait (`Refineable`), in-housed from `gpui-pre-refineable`.

/// A trait for types that can be refined with partial updates.
pub(crate) trait Refineable: Clone {
    type Refinement: Refineable<Refinement = Self::Refinement> + IsEmpty + Default;

    fn refine(&mut self, refinement: &Self::Refinement);
}

pub(crate) trait IsEmpty {
    fn is_empty(&self) -> bool;
}
