//! Win32 virtual-key translation, modifier queries, multi-click tracking, and IME helpers.

use std::time::Instant;

use windows::Win32::{
    Foundation::{HWND, POINT, RECT},
    UI::{
        Input::{
            Ime::{
                CANDIDATEFORM, CFS_CANDIDATEPOS, CFS_POINT, COMPOSITIONFORM, HIMC,
                IME_COMPOSITION_STRING, ImmGetCompositionStringW, ImmGetContext,
                ImmReleaseContext, ImmSetCandidateWindow, ImmSetCompositionWindow,
            },
            KeyboardAndMouse::{
                GetDoubleClickTime, GetKeyState, VK_ADD, VK_APPS, VK_BACK, VK_CONTROL, VK_DELETE,
                VK_DOWN, VK_END, VK_ESCAPE, VK_F1, VK_F4, VK_F12, VK_HOME, VK_LEFT, VK_LWIN,
                VK_MENU, VK_NEXT, VK_NUMPAD0, VK_NUMPAD9, VK_OEM_4, VK_OEM_6, VK_OEM_MINUS,
                VK_OEM_PLUS, VK_PRIOR, VK_RETURN, VK_RIGHT, VK_RWIN, VK_SHIFT, VK_SPACE,
                VK_SUBTRACT, VK_TAB, VK_UP, VIRTUAL_KEY,
            },
        },
        WindowsAndMessaging::{GetSystemMetrics, SM_CXDOUBLECLK, SM_CYDOUBLECLK},
    },
};

use crate::ui::keymap::Modifiers;
use crate::ui::window::MouseButton;

#[derive(Default)]
pub(super) struct ClickTracker {
    last_button: Option<MouseButton>,
    last_pos: (i32, i32),
    last_time: Option<Instant>,
    count: usize,
}

impl ClickTracker {
    pub(super) fn record(&mut self, button: MouseButton, x: i32, y: i32) -> usize {
        let now = Instant::now();
        let dbl_ms = unsafe { GetDoubleClickTime() } as u128;
        let tol_x = unsafe { GetSystemMetrics(SM_CXDOUBLECLK) } / 2;
        let tol_y = unsafe { GetSystemMetrics(SM_CYDOUBLECLK) } / 2;
        let same_spot = self.last_button == Some(button)
            && self
                .last_time
                .is_some_and(|t| now.duration_since(t).as_millis() <= dbl_ms)
            && (x - self.last_pos.0).abs() <= tol_x.max(4)
            && (y - self.last_pos.1).abs() <= tol_y.max(4);
        self.count = if same_spot { self.count + 1 } else { 1 };
        self.last_button = Some(button);
        self.last_pos = (x, y);
        self.last_time = Some(now);
        self.count
    }
}

pub(crate) fn current_modifiers() -> Modifiers {
    let down = |vk: VIRTUAL_KEY| (unsafe { GetKeyState(i32::from(vk.0)) } as u16 & 0x8000) != 0;
    Modifiers {
        control: down(VK_CONTROL),
        alt: down(VK_MENU),
        shift: down(VK_SHIFT),
        platform: down(VK_LWIN) || down(VK_RWIN),
        function: false,
    }
}

pub(super) fn is_syskey_passthrough(vk: u16) -> bool {
    vk == VK_F4.0 || vk == VK_SPACE.0
}

pub(super) fn translate_vk(vk: u16, modifiers: Modifiers) -> Option<String> {
    let v = VIRTUAL_KEY(vk);
    let s = match v {
        VK_BACK => "backspace",
        VK_TAB => "tab",
        VK_RETURN => "enter",
        VK_ESCAPE => "escape",
        VK_SPACE => "space",
        VK_PRIOR => "pageup",
        VK_NEXT => "pagedown",
        VK_END => "end",
        VK_HOME => "home",
        VK_LEFT => "left",
        VK_UP => "up",
        VK_RIGHT => "right",
        VK_DOWN => "down",
        VK_DELETE => "delete",
        VK_MENU => "alt",
        VK_APPS => "menu",
        VK_OEM_PLUS => {
            if modifiers.shift {
                "+"
            } else {
                "="
            }
        }
        VK_ADD => "+",
        VK_OEM_MINUS | VK_SUBTRACT => "-",
        VK_OEM_4 => "[",
        VK_OEM_6 => "]",
        _ if (VK_F1.0..=VK_F12.0).contains(&vk) => {
            return Some(format!("f{}", vk - VK_F1.0 + 1));
        }
        _ if (VK_NUMPAD0.0..=VK_NUMPAD9.0).contains(&vk) => {
            return Some(((b'0' + (vk - VK_NUMPAD0.0) as u8) as char).to_string());
        }
        _ if (b'0' as u16..=b'9' as u16).contains(&vk) => {
            return Some((vk as u8 as char).to_string());
        }
        _ if (b'A' as u16..=b'Z' as u16).contains(&vk) => {
            return Some(((vk as u8 as char).to_ascii_lowercase()).to_string());
        }
        _ => return None,
    };
    Some(s.to_owned())
}

pub(super) fn read_ime_string(himc: HIMC, flag: IME_COMPOSITION_STRING) -> Option<String> {
    let bytes = unsafe { ImmGetCompositionStringW(himc, flag, None, 0) };
    if bytes <= 0 {
        return None;
    }
    let len = (bytes as usize) / 2;
    let mut buf = vec![0u16; len];
    let written = unsafe {
        ImmGetCompositionStringW(himc, flag, Some(buf.as_mut_ptr().cast()), bytes as u32)
    };
    if written <= 0 {
        return None;
    }
    String::from_utf16(&buf[..(written as usize) / 2]).ok()
}

pub(super) fn update_ime_candidate_pos(
    hwnd: HWND,
    window: &mut crate::ui::window::Window,
    cx: &mut crate::ui::app::App,
) {
    let Some(rect) = window.query_ime_caret_bounds(cx) else {
        return;
    };
    let scale = window.native.scale_factor;
    let pt = POINT {
        x: (rect.origin.x.as_f32() * scale).round() as i32,
        y: (rect.bottom().as_f32() * scale).round() as i32,
    };
    let himc = unsafe { ImmGetContext(hwnd) };
    if himc.is_invalid() {
        return;
    }
    let comp_form = COMPOSITIONFORM {
        dwStyle: CFS_POINT,
        ptCurrentPos: pt,
        rcArea: RECT::default(),
    };
    let cand_form = CANDIDATEFORM {
        dwIndex: 0,
        dwStyle: CFS_CANDIDATEPOS,
        ptCurrentPos: pt,
        rcArea: RECT::default(),
    };
    unsafe {
        let _ = ImmSetCompositionWindow(himc, &comp_form);
        let _ = ImmSetCandidateWindow(himc, &cand_form);
        let _ = ImmReleaseContext(hwnd, himc);
    }
}
