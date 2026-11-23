use super::*;

const FILL: u32 = 0x767676;
const PADDING: f32 = 4.;
const MARGIN: f32 = 1.;
const FONT: &str = "Segoe UI";
const FONT_SIZE: f32 = 12.;
const LINE_HEIGHT: f32 = 16.;

#[derive(Clone, Copy)]
pub(crate) enum Placement {
    Below,
    Left,
}

#[derive(Clone, PartialEq)]
pub(crate) struct KeyTip {
    key: char,
    bounds: Bounds<Pixels>,
}

struct KeyTipView {
    tips: Vec<KeyTip>,
}

impl Render for KeyTipView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl Into<UiNode> {
        div().size_full().children(self.tips.iter().map(|tip| {
            div()
                .absolute()
                .left(tip.bounds.origin.x)
                .top(tip.bounds.origin.y)
                .w(tip.bounds.size.width)
                .h(tip.bounds.size.height)
                .bg(rgb(FILL))
                .pl(px(PADDING))
                .pt(px(PADDING))
                .font_family(FONT)
                .text_size(px(FONT_SIZE))
                .line_height(px(LINE_HEIGHT))
                .text_color(white())
                .whitespace_nowrap()
                .child(tip.key.to_string())
        }))
    }
}

#[derive(Default)]
pub(crate) struct KeyTips {
    overlay: Option<overlay::Overlay<KeyTipView>>,
    owner: Option<AnyWindowHandle>,
    rect: ScreenRect,
}

impl KeyTips {
    fn hide(&mut self) {
        self.owner = None;
        self.rect = ScreenRect::default();
        if let Some(overlay) = &self.overlay {
            overlay::hide(overlay.hwnd);
        }
    }
}

pub(crate) fn layout(
    window: &Window,
    key: char,
    anchor: Bounds<Pixels>,
    placement: Placement,
) -> KeyTip {
    let width = text_width(window, &key.to_string(), FONT, FONT_SIZE);
    let fill = size(width + 2. * PADDING, LINE_HEIGHT + 2. * PADDING);
    let (outer_width, outer_height) = (fill.width + 2. * MARGIN, fill.height + 2. * MARGIN);
    let (x, y) = match placement {
        Placement::Below => (
            round_half_up(anchor.center().x.as_f32() - outer_width / 2.),
            anchor.bottom().as_f32(),
        ),
        Placement::Left => (
            anchor.left().as_f32() - outer_width,
            round_half_up(
                anchor.top().as_f32() + (anchor.size.height.as_f32() - outer_height) / 2.,
            ),
        ),
    };
    KeyTip {
        key,
        bounds: Bounds::new(
            point(px(x + MARGIN), px(y + MARGIN)),
            size(px(fill.width), px(fill.height)),
        ),
    }
}

pub(crate) fn show(
    owner: AnyWindowHandle,
    origin: ScreenPoint,
    scale: f32,
    mut tips: Vec<KeyTip>,
    cx: &mut App,
) {
    let Some(union) = tips.iter().map(|tip| tip.bounds).reduce(|a, b| a.union(&b)) else {
        if cx.shared.keytips.owner == Some(owner) {
            cx.shared.keytips.hide();
        }
        return;
    };
    for tip in &mut tips {
        tip.bounds.origin -= union.origin;
    }
    let rect = overlay::screen_rect(union, scale, origin);
    let unchanged = cx.shared.keytips.owner == Some(owner) && cx.shared.keytips.rect == rect;
    let current = cx.shared.keytips.overlay.clone();
    if unchanged
        && current
            .as_ref()
            .is_some_and(|overlay| overlay.view.read(cx).tips == tips)
    {
        return;
    }
    let overlay = match current {
        Some(overlay) => overlay,
        None => {
            let Some(overlay) = overlay::open(|_| KeyTipView { tips: Vec::new() }, cx) else {
                return;
            };
            cx.shared.keytips.overlay = Some(overlay.clone());
            overlay
        }
    };
    overlay.view.update(cx, |view, cx| {
        view.tips = tips;
        cx.notify();
    });
    overlay::place(overlay.hwnd, rect);
    overlay::show(overlay.hwnd);
    cx.shared.keytips.owner = Some(owner);
    cx.shared.keytips.rect = rect;
}

pub(crate) fn is_owner(owner: AnyWindowHandle, cx: &App) -> bool {
    cx.shared.keytips.owner == Some(owner)
}

pub(crate) fn hide_owner(owner: AnyWindowHandle, cx: &mut App) {
    if cx.shared.keytips.owner == Some(owner) {
        cx.shared.keytips.hide();
    }
}

pub(crate) fn window_closed(window: WindowId, cx: &mut App) {
    if cx
        .shared
        .keytips
        .owner
        .is_some_and(|owner| owner.window_id() == window)
    {
        cx.shared.keytips.hide();
    }
}
