use crate::color::{rgb, transparent_black};
use crate::geom::{Rect, ScreenPoint, ScreenRect, point, size};
use crate::gfx::{Direct2DDevices, TextSystem};
use crate::platform::{
    NativeWindow, WindowAppearance, WindowBackdrop, WindowId, WindowKind, WindowOptions,
};
use crate::theme::Palette;
use crate::ui::{AvailSize, Frame, Node, box_, text};

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
    create_failed: bool,
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
        self.create_failed = false;
        if let Some(overlay) = &mut self.overlay {
            if let Err(err) = overlay.recreate_renderer(devices) {
                crate::platform::debug_log(&format!("keytips recreate_renderer failed: {err}"));
            }
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
            if self.create_failed {
                return;
            }
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
            match NativeWindow::create(devices.clone(), &opts, appearance) {
                Ok(win) => self.overlay = Some(win),
                Err(err) => {
                    self.create_failed = true;
                    crate::platform::debug_log(&format!("keytips overlay create failed: {err}"));
                    return;
                }
            }
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

    pub(crate) fn paint(&mut self, p: Palette, ts: &TextSystem) -> bool {
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
        let tree = render_tips(&self.tips, p);
        let frame = Frame::layout(
            tree,
            AvailSize::definite(logical.width, logical.height),
            scale,
            ts,
        );
        match overlay.renderer_mut() {
            Some(renderer) => {
                renderer.begin_frame(transparent_black());
                frame.paint(renderer, &mut |_, _, _| {});
                match renderer.end_frame() {
                    Ok(lost) => lost,
                    Err(err) => {
                        crate::platform::debug_log(&format!("keytips end_frame failed: {err}"));
                        true
                    }
                }
            }
            None => true,
        }
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
