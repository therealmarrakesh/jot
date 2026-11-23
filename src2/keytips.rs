//! Win32 transparent topmost overlay displaying Windows Alt/F10 KeyTip badges.

use crate::color::{rgb, transparent_black, white};
use crate::geom::{Rect, ScreenPoint, ScreenRect, point, size};
use crate::gfx::{Direct2DDevices, TextSystem};
use crate::platform::{
    NativeWindow, WindowAppearance, WindowBackdrop, WindowId, WindowKind, WindowOptions,
};
use crate::ui::{AvailSize, Frame, Node, box_, text};

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
    overlay: Option<NativeWindow>,
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
        if let Some(overlay) = &self.overlay {
            overlay.hwnd().hide();
        }
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

    pub(crate) fn recreate_renderer(&mut self, devices: Direct2DDevices) {
        if let Some(overlay) = &mut self.overlay {
            let _ = overlay.recreate_renderer(devices);
            if self.owner.is_some() {
                self.dirty = true;
            }
        }
    }

    pub(crate) fn sync(
        &mut self,
        owner: WindowId,
        origin: ScreenPoint,
        scale: f32,
        mut tips: Vec<KeyTip>,
        devices: &Direct2DDevices,
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
        if self.overlay.is_none() {
            let opts = WindowOptions {
                bounds: Some(Rect::new(point(0., 0.), size(1., 1.))),
                title: "".into(),
                hide_titlebar: true,
                focus: false,
                show: false,
                kind: WindowKind::Overlay,
                is_resizable: false,
                is_minimizable: false,
                display_id: None,
                backdrop: WindowBackdrop::Transparent,
                min_size: None,
            };
            let Ok(win) = NativeWindow::create(devices.clone(), &opts, appearance) else {
                return;
            };
            self.overlay = Some(win);
        }
        self.tips = tips;
        self.owner = Some(owner);
        self.rect = rect;
        self.dirty = true;
        if let Some(overlay) = &self.overlay {
            let hwnd = overlay.hwnd();
            hwnd.place_topmost_noactivate(rect);
            hwnd.show_noactivate();
        }
    }

    pub(crate) fn paint(&mut self, ts: &TextSystem) -> bool {
        if !self.dirty {
            return false;
        }
        self.dirty = false;
        if self.owner.is_none() || self.tips.is_empty() {
            return false;
        }
        let Some(overlay) = &mut self.overlay else {
            return false;
        };
        if !overlay.sync_live_metrics() {
            return false;
        }
        let logical = overlay.logical_size;
        let scale = overlay.scale_factor;
        let tree = render_tips(&self.tips);
        let frame = Frame::layout(
            tree,
            AvailSize::definite(logical.width, logical.height),
            scale,
            ts,
        );
        overlay.renderer.begin_frame(transparent_black());
        frame.paint(&mut overlay.renderer, &mut |_, _, _| {});
        overlay.renderer.end_frame().unwrap_or(false)
    }
}

fn render_tips(tips: &[KeyTip]) -> Node {
    box_().size_full().children(tips.iter().map(|tip| {
        box_()
            .absolute()
            .left(tip.bounds.origin.x)
            .top(tip.bounds.origin.y)
            .w(tip.bounds.size.width)
            .h(tip.bounds.size.height)
            .bg(rgb(FILL))
            .pl(PADDING)
            .pt(PADDING)
            .font_family(FONT)
            .text_size(FONT_SIZE)
            .line_height(LINE_HEIGHT)
            .text_color(white())
            .nowrap()
            .child(text(tip.key.to_string()))
    }))
}
