use crate::ui::smallvec::SmallVec;

use crate::ui::{
    Anchor, AnyElement, App, Axis, Bounds, Display, Element, GlobalElementId, IntoElement,
    LayoutId, ParentElement, Pixels, Point, Position, Style, Window, point,
};

/// The state that the anchored element uses to track its children.
pub(crate) struct AnchoredState {
    child_layout_ids: SmallVec<[LayoutId; 4]>,
}

/// An anchored element that avoids overflowing the window bounds.
pub(crate) struct Anchored {
    children: SmallVec<[AnyElement; 2]>,
    anchor_position: Option<Point<Pixels>>,
}

/// Creates an element that avoids overflowing the window bounds.
pub(crate) fn anchored() -> Anchored {
    Anchored {
        children: SmallVec::new(),
        anchor_position: None,
    }
}

impl Anchored {
    /// Sets the position in window coordinates
    /// (otherwise the location the anchored element is rendered is used).
    pub(crate) fn position(mut self, anchor: Point<Pixels>) -> Self {
        self.anchor_position = Some(anchor);
        self
    }
}

impl ParentElement for Anchored {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements)
    }
}

impl Element for Anchored {
    type RequestLayoutState = AnchoredState;
    type PrepaintState = ();

    fn id(&self) -> Option<crate::ui::ElementId> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (crate::ui::LayoutId, Self::RequestLayoutState) {
        let child_layout_ids = self
            .children
            .iter_mut()
            .map(|child| child.request_layout(window, cx))
            .collect::<SmallVec<_>>();

        let anchored_style = Style {
            position: Position::Absolute,
            display: Display::Flex,
            ..Style::default()
        };

        let layout_id = window.request_layout(anchored_style, child_layout_ids.iter().copied(), cx);

        (layout_id, AnchoredState { child_layout_ids })
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) {
        if request_layout.child_layout_ids.is_empty() {
            return;
        }

        let children_bounds = request_layout
            .child_layout_ids
            .iter()
            .map(|id| window.layout_bounds(*id))
            .reduce(|acc, bounds| acc.union(&bounds))
            .unwrap();

        let origin = self.anchor_position.unwrap_or(bounds.origin);
        let mut anchor = Anchor::TopLeft;
        let mut desired = Bounds::from_anchor_and_size(anchor, origin, children_bounds.size);

        let limits = Bounds {
            origin: Point::default(),
            size: window.viewport_size(),
        };

        if desired.left() < limits.left() || desired.right() > limits.right() {
            let switched = Bounds::from_anchor_and_size(
                anchor.other_side_along(Axis::Horizontal),
                origin,
                children_bounds.size,
            );
            if !(switched.left() < limits.left() || switched.right() > limits.right()) {
                anchor = anchor.other_side_along(Axis::Horizontal);
                desired = switched;
            }
        }

        if desired.top() < limits.top() || desired.bottom() > limits.bottom() {
            let switched = Bounds::from_anchor_and_size(
                anchor.other_side_along(Axis::Vertical),
                origin,
                children_bounds.size,
            );
            if !(switched.top() < limits.top() || switched.bottom() > limits.bottom()) {
                desired = switched;
            }
        }

        if desired.right() > limits.right() {
            desired.origin.x -= desired.right() - limits.right();
        }
        if desired.left() < limits.left() {
            desired.origin.x = limits.origin.x;
        }

        if desired.bottom() > limits.bottom() {
            desired.origin.y -= desired.bottom() - limits.bottom();
        }
        if desired.top() < limits.top() {
            desired.origin.y = limits.origin.y;
        }

        let offset = desired.origin - bounds.origin;
        let offset = point(offset.x.round(), offset.y.round());

        window.with_element_offset(offset, |window| {
            for child in &mut self.children {
                child.prepaint(window, cx);
            }
        })
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _bounds: crate::ui::Bounds<crate::ui::Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        for child in &mut self.children {
            child.paint(window, cx);
        }
    }
}

impl IntoElement for Anchored {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}
