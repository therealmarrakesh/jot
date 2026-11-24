use std::rc::Rc;

use crate::color::transparent_black;
use crate::geom::{Rect, ScreenRect, Size, point, size};
use crate::gfx::{D2DRenderer, Gpu, Surface};
use crate::platform::{NativeWindow, WindowAppearance, WindowBackdrop, WindowKind, WindowOptions};

struct LiveOverlay {
    // Surface drops before NativeWindow so DComp/swapchain release before DestroyWindow.
    surface: Surface,
    window: NativeWindow,
}

#[derive(Default)]
pub(crate) struct OverlaySurface {
    live: Option<LiveOverlay>,
    create_failed: bool,
}

impl OverlaySurface {
    pub(crate) fn show(&mut self, gpu: &Rc<Gpu>, rect: ScreenRect, appearance: WindowAppearance) {
        if self.live.is_none() {
            if self.create_failed {
                return;
            }
            let opts = WindowOptions {
                bounds: Some(Rect::new(point(0., 0.), size(1., 1.))),
                title: "".into(),
                focus: false,
                show: false,
                kind: WindowKind::Overlay,
                is_resizable: false,
                is_minimizable: false,
                display_id: None,
                backdrop: WindowBackdrop::Transparent,
                min_size: None,
            };
            match NativeWindow::create(&opts, appearance).and_then(|window| {
                let (w, h) = window.physical_size();
                let surface =
                    Surface::new(gpu.clone(), window.hwnd().raw(), w, h, window.scale_factor())?;
                Ok(LiveOverlay { surface, window })
            }) {
                Ok(live) => self.live = Some(live),
                Err(err) => {
                    self.create_failed = true;
                    crate::diag::debug_log(&format!("overlay create failed: {err}"));
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

    pub(crate) fn paint(&mut self, render: impl FnOnce(Size, f32, &mut D2DRenderer)) -> bool {
        let Some(live) = &mut self.live else {
            return false;
        };
        let Some((w, h, scale)) = live.window.sync_live_metrics() else {
            return false;
        };
        live.surface.resize(w, h, scale);
        let logical = live.window.logical_size();
        live.surface.draw(transparent_black(), |renderer| {
            render(logical, scale, renderer);
        })
    }
}
