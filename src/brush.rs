use super::*;
use gpui_kit::base::{Interpolate, Transition, transition};

const BRUSH_TRANSITION: Duration = Duration::from_millis(83);

#[derive(Clone, Copy)]
pub(crate) struct Brushes {
    pub rest: Rgba,
    pub hover: Rgba,
    pub pressed: Rgba,
}

impl Brushes {
    pub(crate) fn subtle(p: Palette) -> Self {
        Self {
            rest: transparent_black().into(),
            hover: rgba(p.subtle_secondary),
            pressed: rgba(p.subtle_tertiary),
        }
    }

    pub(crate) fn solid(color: Rgba) -> Self {
        Self {
            rest: color,
            hover: color,
            pressed: color,
        }
    }
}

#[derive(Clone, Copy, Default, PartialEq)]
struct Pointer {
    hovered: bool,
    pressed: bool,
}

#[derive(Clone, Copy, PartialEq)]
struct Brush(Rgba);

impl Interpolate for Brush {
    fn interpolate(&self, target: &Self, progress: f32) -> Self {
        let (from, to) = (self.0, target.0);
        let a = from.a + (to.a - from.a) * progress;
        let channel = |x: f32, y: f32| {
            let premultiplied = x * from.a + (y * to.a - x * from.a) * progress;
            if a > 0. {
                (premultiplied / a).clamp(0., 1.)
            } else {
                0.
            }
        };
        Brush(Rgba {
            r: channel(from.r, to.r),
            g: channel(from.g, to.g),
            b: channel(from.b, to.b),
            a,
        })
    }
}

pub(crate) struct Faded {
    element: Stateful<Div>,
    brushes: Brushes,
}

pub(crate) trait FadeBackground {
    fn fade_bg(self, brushes: Brushes) -> Faded;
}

impl FadeBackground for Stateful<Div> {
    fn fade_bg(self, brushes: Brushes) -> Faded {
        Faded {
            element: self,
            brushes,
        }
    }
}

fn update(pointer: &Entity<Pointer>, cx: &mut App, change: impl FnOnce(&mut Pointer)) {
    pointer.update(cx, |pointer, cx| {
        let before = *pointer;
        change(pointer);
        if *pointer != before {
            cx.notify();
        }
    });
}

fn sensor(pointer: Entity<Pointer>) -> Stateful<Div> {
    div()
        .id("brush-sensor")
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .on_hover({
            let pointer = pointer.clone();
            move |hovered, _, cx| update(&pointer, cx, |state| state.hovered = *hovered)
        })
        .capture_any_mouse_down({
            let pointer = pointer.clone();
            move |event, _, cx| {
                if event.button == MouseButton::Left {
                    update(&pointer, cx, |state| state.pressed = true);
                }
            }
        })
        .capture_any_mouse_up({
            let pointer = pointer.clone();
            move |event, _, cx| {
                if event.button == MouseButton::Left {
                    update(&pointer, cx, |state| state.pressed = false);
                }
            }
        })
        .on_mouse_up_out(MouseButton::Left, move |_, _, cx| {
            update(&pointer, cx, |state| state.pressed = false)
        })
}

impl Element for Faded {
    type RequestLayoutState = <Stateful<Div> as Element>::RequestLayoutState;
    type PrepaintState = <Stateful<Div> as Element>::PrepaintState;

    fn id(&self) -> Option<ElementId> {
        Element::id(&self.element)
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        self.element.source_location()
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let pointer = window.use_keyed_state("brush-pointer", cx, |_, _| Pointer::default());
        let state = *pointer.read(cx);
        let Brushes {
            rest,
            hover,
            pressed,
        } = self.brushes;
        let target = if state.pressed {
            pressed
        } else if state.hovered {
            hover
        } else {
            rest
        };
        let Brush(color) = transition(
            "brush",
            Brush(target),
            Transition::new(BRUSH_TRANSITION).easing(Easing::Linear),
            window,
            cx,
        );
        self.element.style().background = Some(Hsla::from(color).into());
        self.element.extend([sensor(pointer).into_any_element()]);
        self.element.request_layout(id, inspector_id, window, cx)
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.element
            .prepaint(id, inspector_id, bounds, request_layout, window, cx)
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.element.paint(
            id,
            inspector_id,
            bounds,
            request_layout,
            prepaint,
            window,
            cx,
        )
    }

    fn a11y_role(&self) -> Option<accesskit::Role> {
        self.element.a11y_role()
    }

    fn write_a11y_info(&self, node: &mut accesskit::Node) {
        self.element.write_a11y_info(node)
    }
}

impl IntoElement for Faded {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Styled for Faded {
    fn style(&mut self) -> &mut StyleRefinement {
        self.element.style()
    }
}

impl ParentElement for Faded {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.element.extend(elements)
    }
}

impl InteractiveElement for Faded {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.element.interactivity()
    }
}

impl StatefulInteractiveElement for Faded {}
