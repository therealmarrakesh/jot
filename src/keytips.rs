use std::rc::Rc;

use crate::color::rgb;
use crate::geom::{Rect, ScreenPoint, ScreenRect, point, size};
use crate::gfx::{Gpu, TextSystem};
use crate::platform::{WindowAppearance, WindowId};
use crate::theme::Palette;
use crate::ui::{AvailSize, Frame, Node, OverlaySurface, box_, text};

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
    pub(crate) key: char,
    pub(crate) bounds: Rect,
}

#[inline]
pub(crate) fn round_half_up(value: f32) -> f32 {
    (value + 0.5).floor()
}

pub(crate) fn layout_tip(
    ts: &TextSystem,
    key: char,
    anchor: Rect,
    placement: Placement,
) -> KeyTip {
    let mut buf = [0u8; 4];
    let width = ts.text_width(key.encode_utf8(&mut buf), FONT, FONT_SIZE);
    let fill = size(width + 2. * PADDING, LINE_HEIGHT + 2. * PADDING);
    let (outer_w, outer_h) = (fill.width + 2. * MARGIN, fill.height + 2. * MARGIN);
    let (x, y) = match placement {
        Placement::Below => (
            round_half_up(anchor.center().x - outer_w / 2.),
            anchor.bottom(),
        ),
        Placement::Left => (
            anchor.left() - outer_w,
            round_half_up(anchor.top() + (anchor.size.height - outer_h) / 2.),
        ),
    };
    KeyTip {
        key,
        bounds: Rect::new(point(x + MARGIN, y + MARGIN), fill),
    }
}

#[derive(Default)]
pub(crate) struct KeyTips {
    overlay: OverlaySurface,
    owner: Option<WindowId>,
    rect: ScreenRect,
    tips: Vec<KeyTip>,
    dirty: bool,
}

impl KeyTips {
    pub(crate) fn is_owner(&self, owner: WindowId) -> bool {
        self.owner == Some(owner)
    }

    pub(crate) fn hide(&mut self) {
        self.owner = None;
        self.rect = ScreenRect::default();
        self.tips.clear();
        self.dirty = false;
        self.overlay.hide();
    }

    pub(crate) fn hide_owner(&mut self, owner: WindowId) {
        if self.owner == Some(owner) {
            self.hide();
        }
    }

    pub(crate) fn window_closed(&mut self, window: WindowId) {
        if self.owner == Some(window) {
            self.hide();
        }
    }

    pub(crate) fn sync(
        &mut self,
        owner: WindowId,
        origin: ScreenPoint,
        scale: f32,
        mut tips: Vec<KeyTip>,
        gpu: &Rc<Gpu>,
        appearance: WindowAppearance,
    ) {
        let Some(union) = tips.iter().map(|t| t.bounds).reduce(Rect::union) else {
            self.hide_owner(owner);
            return;
        };
        for tip in &mut tips {
            tip.bounds.origin -= union.origin;
        }
        let rect = union.to_screen(scale, origin);
        if self.owner == Some(owner) && self.rect == rect && self.tips == tips {
            return;
        }
        self.tips = tips;
        self.owner = Some(owner);
        self.rect = rect;
        self.dirty = true;
        self.overlay.show(gpu, rect, appearance);
    }

    pub(crate) fn paint(&mut self, p: Palette, ts: &TextSystem) -> bool {
        if !self.dirty {
            return false;
        }
        self.dirty = false;
        if self.owner.is_none() || self.tips.is_empty() {
            return false;
        }
        let tips = &self.tips;
        let needs_retry = self.overlay.paint(|logical, scale, renderer| {
            let tree = render_tips(tips, p);
            let frame = Frame::layout(
                tree,
                AvailSize::definite(logical.width, logical.height),
                scale,
                ts,
            );
            frame.paint(renderer, &mut |_, _, _| {});
        });
        if needs_retry {
            self.dirty = true;
        }
        needs_retry
    }
}

fn render_tips(tips: &[KeyTip], p: Palette) -> Node {
    box_().size_full().children(tips.iter().map(|tip| {
        box_()
            .absolute()
            .left(tip.bounds.origin.x)
            .top(tip.bounds.origin.y)
            .w(tip.bounds.size.width)
            .h(tip.bounds.size.height)
            .bg(rgb(p.keytip_fill))
            .pl(PADDING)
            .pt(PADDING)
            .font_family(FONT)
            .text_size(FONT_SIZE)
            .line_height(LINE_HEIGHT)
            .text_color(rgb(p.keytip_text))
            .nowrap()
            .child(text(tip.key.to_string()))
    }))
}
