use std::rc::Rc;

use crate::color::transparent_black;
use crate::geom::{ScreenPoint, ScreenRect, Size};
use crate::gfx::{Gpu, TextSystem, WindowSurface};
use crate::platform::{NativeWindow, WindowId};

use super::{AvailSize, Frame, Node};

#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct OverlayAnchor {
    pub(crate) window: WindowId,
    pub(crate) origin: ScreenPoint,
    pub(crate) scale: f32,
}

#[derive(Default)]
pub(crate) struct OverlaySurface {
    live: Option<WindowSurface>,
    create_failed: bool,
}

impl OverlaySurface {
    pub(crate) fn show(&mut self, gpu: &Rc<Gpu>, rect: ScreenRect) {
        if self.live.is_none() {
            if self.create_failed {
                return;
            }
            match NativeWindow::create_overlay()
                .and_then(|window| WindowSurface::new(gpu.clone(), window))
            {
                Ok(live) => self.live = Some(live),
                Err(err) => {
                    self.create_failed = true;
                    crate::platform::diag::debug_log(&format!("overlay create failed: {err}"));
                    return;
                }
            }
        }
        if let Some(live) = &self.live {
            let hwnd = live.window.hwnd();
            hwnd.place_topmost_noactivate(rect);
            hwnd.show_noactivate();
        }
    }

    pub(crate) fn hide(&self) {
        if let Some(live) = &self.live {
            live.window.hwnd().hide();
        }
    }

    pub(crate) fn paint_scene(
        &mut self,
        ts: &TextSystem,
        build: impl FnOnce(Size) -> Node<(), ()>,
    ) -> bool {
        let Some(live) = &mut self.live else {
            return false;
        };
        let Some((w, h, scale)) = live.window.sync_live_metrics() else {
            return false;
        };
        live.surface.resize(w, h, scale);
        let logical = live.window.logical_size();
        live.surface.draw(transparent_black(), |canvas| {
            let frame = Frame::layout(
                build(logical),
                AvailSize::definite(logical.width, logical.height),
                scale,
                ts,
            );
            frame.paint(canvas, &mut |_, _, _| {});
        })
    }
}
