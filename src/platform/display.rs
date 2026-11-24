use windows::Win32::{
    Foundation::POINT,
    Graphics::Gdi::{
        GetMonitorInfoW, HMONITOR, MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTOPRIMARY, MONITORINFO,
        MonitorFromPoint,
    },
    UI::{
        HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI},
        WindowsAndMessaging::USER_DEFAULT_SCREEN_DPI,
    },
};

use crate::geom::{Rect, ScreenPoint, Size, point, size};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct DisplayId(pub(crate) usize);

#[derive(Copy, Clone, Debug)]
pub(crate) struct WindowsDisplay {
    monitor: HMONITOR,
}

impl WindowsDisplay {
    pub(crate) fn primary() -> Option<Self> {
        let monitor = unsafe { MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY) };
        (!monitor.is_invalid()).then_some(Self { monitor })
    }

    pub(crate) fn from_point(pt: ScreenPoint) -> Option<Self> {
        let monitor = unsafe {
            MonitorFromPoint(
                POINT { x: pt.x, y: pt.y },
                MONITOR_DEFAULTTONEAREST,
            )
        };
        (!monitor.is_invalid()).then_some(Self { monitor })
    }

    pub(crate) fn id(&self) -> DisplayId {
        DisplayId(self.monitor.0 as usize)
    }

    pub(crate) fn from_id(id: DisplayId) -> Option<Self> {
        let monitor = HMONITOR(id.0 as *mut _);
        (!monitor.is_invalid()).then_some(Self { monitor })
    }

    pub(crate) fn scale_factor(&self) -> f32 {
        let mut dpi_x = USER_DEFAULT_SCREEN_DPI;
        let mut dpi_y = USER_DEFAULT_SCREEN_DPI;
        let _ =
            unsafe { GetDpiForMonitor(self.monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) };
        (dpi_x.max(USER_DEFAULT_SCREEN_DPI) as f32) / (USER_DEFAULT_SCREEN_DPI as f32)
    }

    pub(crate) fn visible_bounds(&self) -> Rect {
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !unsafe { GetMonitorInfoW(self.monitor, &mut info) }.as_bool() {
            return Rect::new(point(0.0, 0.0), size(1280.0, 800.0));
        }
        let scale = self.scale_factor();
        let rc = info.rcWork;
        Rect::new(
            point(rc.left as f32 / scale, rc.top as f32 / scale),
            size(
                (rc.right - rc.left) as f32 / scale,
                (rc.bottom - rc.top) as f32 / scale,
            ),
        )
    }

    pub(crate) fn centered_rect(&self, target: Size) -> Rect {
        let vis = self.visible_bounds();
        Rect::new(
            point(
                vis.origin.x + ((vis.size.width - target.width) * 0.5).max(0.0),
                vis.origin.y + ((vis.size.height - target.height) * 0.5).max(0.0),
            ),
            target,
        )
    }
}
