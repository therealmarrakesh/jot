use crate::smallvec::SmallVec;
use gpui::Uuid;
use gpui::gpui_util::ResultExt;
use windows::{
    Win32::{
        Foundation::*,
        Graphics::Gdi::*,
        UI::{
            HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI},
            WindowsAndMessaging::USER_DEFAULT_SCREEN_DPI,
        },
    },
    core::*,
};

use crate::platform::win32::logical_point;
use gpui::{
    Bounds, DEFAULT_WINDOW_SIZE, DevicePixels, DisplayId, Pixels, point, refresh_interval_from_hz,
    size,
};

#[derive(Debug, Clone, Copy)]
pub struct WindowsDisplay {
    pub(crate) handle: HMONITOR,
    pub(crate) display_id: DisplayId,
    scale_factor: f32,
    bounds: Bounds<Pixels>,
    visible_bounds: Bounds<Pixels>,
    physical_bounds: Bounds<DevicePixels>,
    uuid: Uuid,
}

// The `HMONITOR` is thread-safe.
unsafe impl Send for WindowsDisplay {}
unsafe impl Sync for WindowsDisplay {}

impl WindowsDisplay {
    pub(crate) fn new(display_id: DisplayId) -> Option<Self> {
        let handle = HMONITOR(u64::from(display_id) as _);
        let info = get_monitor_info(handle).log_err()?;
        let monitor_size = info.monitorInfo.rcMonitor;
        let work_area = info.monitorInfo.rcWork;
        let uuid = generate_uuid(&info.szDevice);
        let scale_factor = get_scale_factor_for_monitor(handle).log_err()?;
        let physical_size = size(
            (monitor_size.right - monitor_size.left).into(),
            (monitor_size.bottom - monitor_size.top).into(),
        );

        Some(WindowsDisplay {
            handle,
            display_id,
            scale_factor,
            bounds: Bounds {
                origin: logical_point(
                    monitor_size.left as f32,
                    monitor_size.top as f32,
                    scale_factor,
                ),
                size: physical_size.to_pixels(scale_factor),
            },
            visible_bounds: Bounds {
                origin: logical_point(work_area.left as f32, work_area.top as f32, scale_factor),
                size: size(
                    (work_area.right - work_area.left) as f32 / scale_factor,
                    (work_area.bottom - work_area.top) as f32 / scale_factor,
                )
                .map(gpui::px),
            },
            physical_bounds: Bounds {
                origin: point(monitor_size.left.into(), monitor_size.top.into()),
                size: physical_size,
            },
            uuid,
        })
    }

    pub(crate) fn display_id_for_monitor(monitor: HMONITOR) -> DisplayId {
        DisplayId::new(monitor.0 as u64)
    }

    pub fn primary_monitor() -> Option<Self> {
        const POINT_ZERO: POINT = POINT { x: 0, y: 0 };
        let monitor = unsafe { MonitorFromPoint(POINT_ZERO, MONITOR_DEFAULTTOPRIMARY) };
        if monitor.is_invalid() {
            crate::log::error!(
                "can not find the primary monitor: {}",
                std::io::Error::last_os_error()
            );
            return None;
        }
        WindowsDisplay::new(Self::display_id_for_monitor(monitor))
    }

    pub(crate) fn scale_factor(&self) -> f32 {
        self.scale_factor
    }

    pub fn check_given_bounds(&self, bounds: Bounds<Pixels>) -> bool {
        let center = bounds.center();
        let center = POINT {
            x: (center.x.as_f32() * self.scale_factor) as i32,
            y: (center.y.as_f32() * self.scale_factor) as i32,
        };
        let monitor = unsafe { MonitorFromPoint(center, MONITOR_DEFAULTTONULL) };
        if monitor.is_invalid() {
            false
        } else {
            let Some(display) = WindowsDisplay::new(Self::display_id_for_monitor(monitor)) else {
                return false;
            };
            display.uuid == self.uuid
        }
    }

    pub fn displays() -> Vec<WindowsDisplay> {
        available_monitors()
            .into_iter()
            .filter_map(|handle| WindowsDisplay::new(Self::display_id_for_monitor(handle)))
            .collect()
    }

    pub fn physical_bounds(&self) -> Bounds<DevicePixels> {
        self.physical_bounds
    }

    pub fn id(&self) -> DisplayId {
        self.display_id
    }

    pub fn uuid(&self) -> crate::anyhow::Result<Uuid> {
        Ok(self.uuid)
    }

    pub fn bounds(&self) -> Bounds<Pixels> {
        self.bounds
    }

    pub fn visible_bounds(&self) -> Bounds<Pixels> {
        self.visible_bounds
    }

    pub fn default_bounds(&self) -> Bounds<Pixels> {
        let bounds = self.bounds();
        let center = bounds.center();
        let clipped_window_size = DEFAULT_WINDOW_SIZE.min(&bounds.size);

        let offset = clipped_window_size / 2.0;
        let origin = point(center.x - offset.width, center.y - offset.height);
        Bounds::new(origin, clipped_window_size)
    }

    pub fn refresh_interval(&self) -> Option<std::time::Duration> {
        get_monitor_info(self.handle)
            .log_err()
            .and_then(|info| refresh_interval_for_device(&info.szDevice))
    }
}

fn refresh_interval_for_device(device_name: &[u16; 32]) -> Option<std::time::Duration> {
    let mut mode = DEVMODEW {
        dmSize: std::mem::size_of::<DEVMODEW>() as u16,
        ..Default::default()
    };
    let found = unsafe {
        EnumDisplaySettingsW(
            PCWSTR(device_name.as_ptr()),
            ENUM_CURRENT_SETTINGS,
            &mut mode,
        )
    };
    if !found.as_bool() || mode.dmDisplayFrequency <= 1 {
        return None;
    }
    refresh_interval_from_hz(f64::from(mode.dmDisplayFrequency))
}

fn available_monitors() -> SmallVec<[HMONITOR; 4]> {
    let mut monitors: SmallVec<[HMONITOR; 4]> = SmallVec::new();
    unsafe {
        EnumDisplayMonitors(
            None,
            None,
            Some(monitor_enum_proc),
            LPARAM(&mut monitors as *mut _ as _),
        )
        .ok()
        .log_err();
    }
    monitors
}

unsafe extern "system" fn monitor_enum_proc(
    hmonitor: HMONITOR,
    _hdc: HDC,
    _place: *mut RECT,
    data: LPARAM,
) -> BOOL {
    let monitors = data.0 as *mut SmallVec<[HMONITOR; 4]>;
    unsafe { (*monitors).push(hmonitor) };
    BOOL(1)
}

fn get_monitor_info(hmonitor: HMONITOR) -> crate::anyhow::Result<MONITORINFOEXW> {
    let mut monitor_info: MONITORINFOEXW = unsafe { std::mem::zeroed() };
    monitor_info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    let status = unsafe {
        GetMonitorInfoW(
            hmonitor,
            &mut monitor_info as *mut MONITORINFOEXW as *mut MONITORINFO,
        )
    };
    if status.as_bool() {
        Ok(monitor_info)
    } else {
        Err(crate::anyhow::anyhow!(std::io::Error::last_os_error()))
    }
}

fn generate_uuid(device_name: &[u16]) -> Uuid {
    let name: Vec<u8> = device_name
        .iter()
        .flat_map(|&a| a.to_be_bytes())
        .collect();
    Uuid::from_hash_bytes(&name)
}

fn get_scale_factor_for_monitor(monitor: HMONITOR) -> Result<f32> {
    let mut dpi_x = 0;
    let mut dpi_y = 0;
    unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) }?;
    assert_eq!(dpi_x, dpi_y);
    Ok(dpi_x as f32 / USER_DEFAULT_SCREEN_DPI as f32)
}
