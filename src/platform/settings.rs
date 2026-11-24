use std::time::Duration;

use windows::Win32::{
    Globalization::{DATE_SHORTDATE, GetDateFormatEx, GetTimeFormatEx, TIME_NOSECONDS},
    Graphics::Gdi::{
        COLOR_BTNFACE, COLOR_BTNTEXT, COLOR_GRAYTEXT, COLOR_HIGHLIGHT, COLOR_HIGHLIGHTTEXT,
        COLOR_HOTLIGHT, COLOR_WINDOW, COLOR_WINDOWFRAME, COLOR_WINDOWTEXT, GetSysColor,
        SYS_COLOR_INDEX,
    },
    System::{
        Registry::{HKEY_CURRENT_USER, RRF_RT_REG_BINARY, RRF_RT_REG_DWORD, RegGetValueW},
        SystemInformation::GetLocalTime,
    },
    UI::{
        Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW},
        WindowsAndMessaging::{
            GetCaretBlinkTime, SPI_GETCARETWIDTH, SPI_GETCLIENTAREAANIMATION, SPI_GETHIGHCONTRAST,
            SPI_GETMENUSHOWDELAY, SPI_GETMOUSEHOVERTIME, SPI_GETWHEELSCROLLCHARS,
            SPI_GETWHEELSCROLLLINES, SYSTEM_PARAMETERS_INFO_ACTION,
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
        },
    },
};
use windows::core::w;

use super::window::WindowAppearance;

const DEFAULT_WHEEL_LINES: u32 = 3;
const DEFAULT_WHEEL_CHARS: u32 = 3;
const DEFAULT_HOVER_TIME_MS: u32 = 400;
const DEFAULT_MENU_SHOW_DELAY_MS: u32 = 400;

#[derive(Copy, Clone, Debug)]
pub(crate) struct HighContrastColors {
    pub(crate) window: u32,
    pub(crate) window_text: u32,
    pub(crate) highlight: u32,
    pub(crate) highlight_text: u32,
    pub(crate) gray_text: u32,
    pub(crate) btn_face: u32,
    pub(crate) btn_text: u32,
    pub(crate) window_frame: u32,
    pub(crate) hotlight: u32,
}

#[derive(Copy, Clone, Debug)]
pub(crate) struct SystemSettings {
    pub(crate) appearance: WindowAppearance,
    pub(crate) accent_light: Option<u32>,
    pub(crate) accent_dark: Option<u32>,
    pub(crate) high_contrast: Option<HighContrastColors>,
    pub(crate) reduce_motion: bool,
    pub(crate) caret_blink_interval: Option<Duration>,
    pub(crate) caret_blink_timeout: Option<Duration>,
    pub(crate) caret_width_px: u32,
    pub(crate) mouse_hover_time: Duration,
    pub(crate) menu_show_delay: Duration,
    pub(crate) wheel_scroll_lines: u32,
    pub(crate) wheel_scroll_chars: u32,
}

#[cfg(test)]
impl Default for SystemSettings {
    fn default() -> Self {
        Self {
            appearance: WindowAppearance::Light,
            accent_light: None,
            accent_dark: None,
            high_contrast: None,
            reduce_motion: false,
            caret_blink_interval: None,
            caret_blink_timeout: None,
            caret_width_px: 1,
            mouse_hover_time: Duration::from_millis(u64::from(DEFAULT_HOVER_TIME_MS)),
            menu_show_delay: Duration::from_millis(u64::from(DEFAULT_MENU_SHOW_DELAY_MS)),
            wheel_scroll_lines: DEFAULT_WHEEL_LINES,
            wheel_scroll_chars: DEFAULT_WHEEL_CHARS,
        }
    }
}

impl SystemSettings {
    pub(crate) fn query() -> Self {
        let appearance = query_system_appearance();
        let (accent_light, accent_dark) = query_system_accents();
        let reduce_motion = system_parameters_u32(SPI_GETCLIENTAREAANIMATION.0)
            .is_some_and(|enabled| enabled == 0);
        let hover_ms =
            system_parameters_u32(SPI_GETMOUSEHOVERTIME.0).unwrap_or(DEFAULT_HOVER_TIME_MS);
        let menu_ms =
            system_parameters_u32(SPI_GETMENUSHOWDELAY.0).unwrap_or(DEFAULT_MENU_SHOW_DELAY_MS);
        Self {
            appearance,
            accent_light,
            accent_dark,
            high_contrast: query_high_contrast_colors(),
            reduce_motion,
            caret_blink_interval: query_caret_blink_interval(),
            caret_blink_timeout: query_caret_blink_timeout(),
            caret_width_px: system_parameters_u32(SPI_GETCARETWIDTH.0)
                .unwrap_or(1)
                .clamp(1, 20),
            mouse_hover_time: Duration::from_millis(u64::from(hover_ms)),
            menu_show_delay: Duration::from_millis(u64::from(menu_ms)),
            wheel_scroll_lines: system_parameters_u32(SPI_GETWHEELSCROLLLINES.0)
                .unwrap_or(DEFAULT_WHEEL_LINES),
            wheel_scroll_chars: system_parameters_u32(SPI_GETWHEELSCROLLCHARS.0)
                .unwrap_or(DEFAULT_WHEEL_CHARS),
        }
    }

    #[inline]
    pub(crate) fn accent(self, dark: bool) -> Option<u32> {
        if dark {
            self.accent_dark
        } else {
            self.accent_light
        }
    }

    #[inline]
    pub(crate) fn wheel_scroll_amount(self, horizontal: bool) -> u32 {
        if horizontal {
            self.wheel_scroll_chars
        } else {
            self.wheel_scroll_lines
        }
    }
}

fn system_parameters_u32(action: u32) -> Option<u32> {
    let mut value = 0u32;
    unsafe {
        SystemParametersInfoW(
            SYSTEM_PARAMETERS_INFO_ACTION(action),
            0,
            Some((&raw mut value).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    }
    .is_ok()
    .then_some(value)
}

fn query_system_appearance() -> WindowAppearance {
    let mut value = 1u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut value).cast()),
            Some(&mut size),
        )
    };
    if status.is_ok() && value == 0 {
        WindowAppearance::Dark
    } else {
        WindowAppearance::Light
    }
}

fn query_system_accents() -> (Option<u32>, Option<u32>) {
    let mut shades = [0u8; 32];
    let mut len = shades.len() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Accent"),
            w!("AccentPalette"),
            RRF_RT_REG_BINARY,
            None,
            Some(shades.as_mut_ptr().cast()),
            Some(&mut len),
        )
    };
    if status.0 != 0 {
        return (None, None);
    }
    let read = |at: usize| {
        (len as usize >= at + 3)
            .then(|| u32::from_be_bytes([0, shades[at], shades[at + 1], shades[at + 2]]))
    };
    (read(16), read(4))
}

#[inline]
fn colorref_to_rgb24(colorref: u32) -> u32 {
    let r = colorref & 0xFF;
    let g = (colorref >> 8) & 0xFF;
    let b = (colorref >> 16) & 0xFF;
    (r << 16) | (g << 8) | b
}

fn query_high_contrast_colors() -> Option<HighContrastColors> {
    let mut hc = HIGHCONTRASTW {
        cbSize: size_of::<HIGHCONTRASTW>() as u32,
        ..Default::default()
    };
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            hc.cbSize,
            Some((&raw mut hc).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    }
    .is_ok();
    if !ok || (hc.dwFlags & HCF_HIGHCONTRASTON).0 == 0 {
        return None;
    }
    let sys = |idx: SYS_COLOR_INDEX| colorref_to_rgb24(unsafe { GetSysColor(idx) });
    Some(HighContrastColors {
        window: sys(COLOR_WINDOW),
        window_text: sys(COLOR_WINDOWTEXT),
        highlight: sys(COLOR_HIGHLIGHT),
        highlight_text: sys(COLOR_HIGHLIGHTTEXT),
        gray_text: sys(COLOR_GRAYTEXT),
        btn_face: sys(COLOR_BTNFACE),
        btn_text: sys(COLOR_BTNTEXT),
        window_frame: sys(COLOR_WINDOWFRAME),
        hotlight: sys(COLOR_HOTLIGHT),
    })
}

fn query_caret_blink_interval() -> Option<Duration> {
    let millis = unsafe { GetCaretBlinkTime() };
    (millis != 0 && millis != u32::MAX).then(|| Duration::from_millis(millis.into()))
}

fn query_caret_blink_timeout() -> Option<Duration> {
    const SPI_GETCARETTIMEOUT_ACTION: u32 = 0x2022;
    let millis = system_parameters_u32(SPI_GETCARETTIMEOUT_ACTION).unwrap_or(5_000);
    (millis != 0 && millis != u32::MAX).then(|| Duration::from_millis(millis.into()))
}

pub(crate) fn format_local_time_date() -> String {
    let now = unsafe { GetLocalTime() };
    let mut time = [0u16; 128];
    let mut date = [0u16; 128];
    let (time_len, date_len) = unsafe {
        (
            GetTimeFormatEx(None, TIME_NOSECONDS, Some(&now), None, Some(&mut time)),
            GetDateFormatEx(
                None,
                DATE_SHORTDATE,
                Some(&now),
                None,
                Some(&mut date),
                None,
            ),
        )
    };
    let text =
        |buffer: &[u16], len: i32| String::from_utf16_lossy(&buffer[..(len.max(1) - 1) as usize]);
    format!("{} {}", text(&time, time_len), text(&date, date_len))
}
