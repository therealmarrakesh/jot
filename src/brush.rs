use super::*;
use gpui_kit::base::{Interpolate, Transition, transition};

const FADE_DURATION: Duration = Duration::from_millis(83);

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

#[derive(Clone, Copy, PartialEq)]
struct Premultiplied(Rgba);

impl Interpolate for Premultiplied {
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
        Premultiplied(Rgba {
            r: channel(from.r, to.r),
            g: channel(from.g, to.g),
            b: channel(from.b, to.b),
            a,
        })
    }
}

#[derive(Clone, Default)]
pub(crate) struct Pressed(Rc<Cell<bool>>);

impl Pressed {
    fn set(&self, pressed: bool, window: &mut Window) {
        if self.0.replace(pressed) != pressed {
            window.refresh();
        }
    }
}

pub(crate) struct BrushTransition {
    element: Stateful<Div>,
    brushes: Brushes,
    tooltip: Option<SharedString>,
}

impl BrushTransition {
    pub(crate) fn tooltip(mut self, text: impl Into<SharedString>) -> Self {
        self.tooltip = Some(text.into());
        self
    }
}

pub(crate) trait PressedExt: StatefulInteractiveElement + Sized {
    fn pressed(self, f: impl Fn(StyleRefinement) -> StyleRefinement + 'static) -> Self {
        self.capture_any_mouse_down(|event, window, _| {
            if event.button != MouseButton::Left {
                window.prevent_default();
            }
        })
        .active(f)
    }
}

impl<T: StatefulInteractiveElement> PressedExt for T {}

pub(crate) trait BrushTransitionExt {
    fn brush_transition(self, brushes: Brushes) -> BrushTransition;
}

impl BrushTransitionExt for Stateful<Div> {
    fn brush_transition(self, brushes: Brushes) -> BrushTransition {
        BrushTransition {
            element: self,
            brushes,
            tooltip: None,
        }
    }
}

impl Element for BrushTransition {
    type RequestLayoutState = <Stateful<Div> as Element>::RequestLayoutState;
    type PrepaintState = (Pressed, <Stateful<Div> as Element>::PrepaintState);

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
        let pressed = id.map_or_else(Pressed::default, |id| {
            window.with_element_state(id, |pressed: Option<Pressed>, _| {
                let pressed = pressed.unwrap_or_default();
                (pressed.clone(), pressed)
            })
        });
        let interactivity = self.element.interactivity();
        interactivity.capture_any_mouse_down({
            let pressed = pressed.clone();
            move |event, window, _| {
                if event.button == MouseButton::Left {
                    pressed.set(true, window);
                }
            }
        });
        interactivity.capture_any_mouse_up({
            let pressed = pressed.clone();
            move |event, window, _| {
                if event.button == MouseButton::Left {
                    pressed.set(false, window);
                }
            }
        });
        interactivity.on_mouse_up_out(MouseButton::Left, {
            let pressed = pressed.clone();
            move |_, window, _| pressed.set(false, window)
        });
        if let Some((tip_id, text)) = Element::id(&self.element).zip(self.tooltip.take()) {
            tooltip::register(tip_id, text, bounds, window, cx);
        }
        let prepaint = self
            .element
            .prepaint(id, inspector_id, bounds, request_layout, window, cx);
        (pressed, prepaint)
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        (pressed, hitbox): &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let hovered = hitbox
            .as_ref()
            .is_some_and(|hitbox| hitbox.is_hovered(window));
        let Brushes {
            rest,
            hover,
            pressed: down,
        } = self.brushes;
        let target = if pressed.0.get() {
            down
        } else if hovered {
            hover
        } else {
            rest
        };
        let Premultiplied(color) = transition(
            "brush",
            Premultiplied(target),
            Transition::new(FADE_DURATION).easing(Easing::Linear),
            window,
            cx,
        );
        self.element.style().background = Some(color.into());
        if let Some(hitbox) = hitbox.clone() {
            window.on_mouse_event(move |_: &MouseMoveEvent, phase, window, _| {
                if phase == DispatchPhase::Capture && hitbox.is_hovered(window) != hovered {
                    window.refresh();
                }
            });
        }
        self.element
            .paint(id, inspector_id, bounds, request_layout, hitbox, window, cx)
    }

    fn a11y_role(&self) -> Option<accesskit::Role> {
        self.element.a11y_role()
    }

    fn write_a11y_info(&self, node: &mut accesskit::Node) {
        self.element.write_a11y_info(node)
    }

    fn a11y_synthetic_children(
        &mut self,
        (_, prepaint): &mut Self::PrepaintState,
        builder: &mut A11ySubtreeBuilder,
    ) {
        Element::a11y_synthetic_children(&mut self.element, prepaint, builder)
    }
}

impl IntoElement for BrushTransition {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Styled for BrushTransition {
    fn style(&mut self) -> &mut StyleRefinement {
        self.element.style()
    }
}

impl ParentElement for BrushTransition {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.element.extend(elements)
    }
}

impl InteractiveElement for BrushTransition {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.element.interactivity()
    }
}

impl StatefulInteractiveElement for BrushTransition {}
