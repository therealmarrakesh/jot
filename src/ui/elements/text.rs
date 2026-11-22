use crate::ui::{
    App, Bounds, Element, ElementId, GlobalElementId, IntoElement, LayoutId, Pixels, SharedString,
    Size, WhiteSpace, Window, WrappedLine,
};
use crate::ui::anyhow::Context as _;
use crate::ui::gpui_util::ResultExt;
use crate::ui::smallvec::SmallVec;
use std::{borrow::Cow, cell::RefCell, rc::Rc};

impl Element for &'static str {
    type RequestLayoutState = TextLayout;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        window: &mut Window,
        _cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let state = TextLayout::default();
        let layout_id = state.layout(SharedString::from(*self), window);
        (layout_id, state)
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        bounds: Bounds<Pixels>,
        text_layout: &mut Self::RequestLayoutState,
        _window: &mut Window,
        _cx: &mut App,
    ) {
        text_layout.prepaint(bounds, self)
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _bounds: Bounds<Pixels>,
        text_layout: &mut TextLayout,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        text_layout.paint(self, window, cx)
    }
}

impl IntoElement for &'static str {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl IntoElement for String {
    type Element = SharedString;

    fn into_element(self) -> Self::Element {
        self.into()
    }
}

impl IntoElement for Cow<'static, str> {
    type Element = SharedString;

    fn into_element(self) -> Self::Element {
        self.into()
    }
}

impl Element for SharedString {
    type RequestLayoutState = TextLayout;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        window: &mut Window,
        _cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let state = TextLayout::default();
        let layout_id = state.layout(self.clone(), window);
        (layout_id, state)
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        bounds: Bounds<Pixels>,
        text_layout: &mut Self::RequestLayoutState,
        _window: &mut Window,
        _cx: &mut App,
    ) {
        text_layout.prepaint(bounds, self.as_ref())
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _bounds: Bounds<Pixels>,
        text_layout: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        text_layout.paint(self.as_ref(), window, cx)
    }
}

impl IntoElement for SharedString {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

/// The Layout for TextElement.
#[derive(Default, Clone)]
pub(crate) struct TextLayout(Rc<RefCell<Option<TextLayoutInner>>>);

struct TextLayoutInner {
    lines: SmallVec<[WrappedLine; 1]>,
    line_height: Pixels,
    wrap_width: Option<Pixels>,
    size: Option<Size<Pixels>>,
    bounds: Option<Bounds<Pixels>>,
}

impl TextLayout {
    fn layout(&self, text: SharedString, window: &mut Window) -> LayoutId {
        let text_style = window.text_style();
        let font_size = text_style.font_size.to_pixels(window.rem_size());
        let line_height = window.pixel_snap(
            text_style
                .line_height
                .to_pixels(font_size.into(), window.rem_size()),
        );
        let runs = vec![text_style.to_run(text.len())];

        window.request_measured_layout(Default::default(), {
            let element_state = self.clone();

            move |known_dimensions, available_space, window, _cx| {
                let wrap_width = if text_style.white_space == WhiteSpace::Normal {
                    known_dimensions.width.or(match available_space.width {
                        crate::ui::AvailableSpace::Definite(x) => Some(x),
                        _ => None,
                    })
                } else {
                    None
                };

                if let Some(text_layout) = element_state.0.borrow().as_ref()
                    && let Some(size) = text_layout.size
                    && (wrap_width.is_none() || wrap_width == text_layout.wrap_width)
                {
                    return size;
                }

                let Some(lines) = window
                    .text_system()
                    .shape_text(
                        text.clone(),
                        font_size,
                        &runs,
                        wrap_width,
                        text_style.line_clamp,
                    )
                    .log_err()
                else {
                    element_state.0.borrow_mut().replace(TextLayoutInner {
                        lines: Default::default(),
                        line_height,
                        wrap_width,
                        size: Some(Size::default()),
                        bounds: None,
                    });
                    return Size::default();
                };

                let mut size: Size<Pixels> = Size::default();
                for line in &lines {
                    let line_size = line.size(line_height);
                    size.height += line_size.height;
                    size.width = size.width.max(line_size.width).ceil();
                }

                element_state.0.borrow_mut().replace(TextLayoutInner {
                    lines,
                    line_height,
                    wrap_width,
                    size: Some(size),
                    bounds: None,
                });

                size
            }
        })
    }

    fn prepaint(&self, bounds: Bounds<Pixels>, text: &str) {
        let mut element_state = self.0.borrow_mut();
        let element_state = element_state
            .as_mut()
            .with_context(|| format!("measurement has not been performed on {text}"))
            .unwrap();
        element_state.bounds = Some(bounds);
    }

    fn paint(&self, text: &str, window: &mut Window, cx: &mut App) {
        let element_state = self.0.borrow();
        let element_state = element_state
            .as_ref()
            .with_context(|| format!("measurement has not been performed on {text}"))
            .unwrap();
        let bounds = element_state
            .bounds
            .with_context(|| format!("prepaint has not been performed on {text}"))
            .unwrap();

        let line_height = element_state.line_height;
        let mut line_origin = bounds.origin;
        let text_style = window.text_style();
        for line in &element_state.lines {
            line.paint_background(
                line_origin,
                line_height,
                text_style.text_align,
                Some(bounds),
                window,
                cx,
            )
            .log_err();
            line.paint(
                line_origin,
                line_height,
                text_style.text_align,
                Some(bounds),
                window,
                cx,
            )
            .log_err();
            line_origin.y += line.size(line_height).height;
        }
    }
}
