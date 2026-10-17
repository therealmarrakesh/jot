use super::*;
use windows_sys::Win32::Foundation::POINT;

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

pub(crate) struct KeyTipView {
    tips: Vec<KeyTip>,
}

impl Render for KeyTipView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
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
                .text_color(gpui_kit::white())
                .whitespace_nowrap()
                .child(tip.key.to_string())
        }))
    }
}

#[derive(Default)]
struct KeyTips {
    overlay: Option<overlay::Overlay<KeyTipView>>,
    owner: Option<AnyWindowHandle>,
    shown: Vec<KeyTip>,
    rect: [i32; 4],
}

impl Global for KeyTips {}

pub(crate) fn round_half_up(value: f32) -> f32 {
    (value + 0.5).floor()
}

pub(crate) fn layout(
    window: &Window,
    key: char,
    anchor: Bounds<Pixels>,
    placement: Placement,
) -> KeyTip {
    let text = key.to_string();
    let run = TextRun {
        len: text.len(),
        font: font(FONT),
        color: gpui_kit::black(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let width = window
        .text_system()
        .shape_line(text.into(), px(FONT_SIZE), &[run], None)
        .width
        .as_f32()
        .ceil();
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
    origin: POINT,
    scale: f32,
    tips: Vec<KeyTip>,
    cx: &mut App,
) {
    let state = cx.default_global::<KeyTips>();
    let Some(union) = tips.iter().map(|tip| tip.bounds).reduce(|a, b| a.union(&b)) else {
        if state.owner == Some(owner) {
            state.owner = None;
            state.shown.clear();
            if let Some(overlay) = &state.overlay {
                overlay::hide(overlay.hwnd);
            }
        }
        return;
    };
    let tips: Vec<KeyTip> = tips
        .into_iter()
        .map(|tip| KeyTip {
            key: tip.key,
            bounds: Bounds::new(tip.bounds.origin - union.origin, tip.bounds.size),
        })
        .collect();
    let physical = |value: Pixels| (value.as_f32() * scale).round() as i32;
    let rect = [
        origin.x + physical(union.origin.x),
        origin.y + physical(union.origin.y),
        physical(union.size.width),
        physical(union.size.height),
    ];
    if state.owner == Some(owner) && state.shown == tips && state.rect == rect {
        return;
    }
    let overlay = match state.overlay.clone() {
        Some(overlay) => overlay,
        None => {
            let Some(overlay) = overlay::open(true, |_| KeyTipView { tips: Vec::new() }, cx) else {
                return;
            };
            cx.default_global::<KeyTips>().overlay = Some(overlay.clone());
            overlay
        }
    };
    overlay.view.update(cx, |view, cx| {
        view.tips = tips.clone();
        cx.notify();
    });
    overlay::clear_backdrop(overlay.hwnd);
    overlay::place(overlay.hwnd, rect[0], rect[1], rect[2], rect[3]);
    overlay::show(overlay.hwnd);
    let state = cx.default_global::<KeyTips>();
    state.owner = Some(owner);
    state.shown = tips;
    state.rect = rect;
}
