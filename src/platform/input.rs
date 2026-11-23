//! Typed keyboard/mouse/IME input types, Win32 virtual-key translation, and multi-click tracking.

use windows::Win32::{
    Foundation::{HWND, POINT, RECT},
    UI::{
        Input::{
            Ime::{
                CANDIDATEFORM, CFS_CANDIDATEPOS, CFS_POINT, COMPOSITIONFORM, CPS_COMPLETE, HIMC,
                IME_COMPOSITION_STRING, ImmGetCompositionStringW, ImmGetContext, ImmNotifyIME,
                ImmReleaseContext, ImmSetCandidateWindow, ImmSetCompositionWindow,
                NI_COMPOSITIONSTR,
            },
            KeyboardAndMouse::{
                GetDoubleClickTime, GetKeyState, VK_ADD, VK_APPS, VK_BACK, VK_CONTROL, VK_DELETE,
                VK_DOWN, VK_END, VK_ESCAPE, VK_F1, VK_F4, VK_F12, VK_HOME, VK_INSERT,
                VK_LCONTROL, VK_LEFT, VK_LWIN, VK_MENU, VK_NEXT, VK_NUMPAD0, VK_NUMPAD9,
                VK_OEM_4, VK_OEM_6, VK_OEM_MINUS, VK_OEM_PLUS, VK_PRIOR, VK_RCONTROL, VK_RETURN,
                VK_RIGHT, VK_RMENU, VK_RWIN, VK_SHIFT, VK_SPACE, VK_SUBTRACT, VK_TAB, VK_UP,
                VIRTUAL_KEY,
            },
        },
        WindowsAndMessaging::{GetMessageTime, GetSystemMetrics, SM_CXDOUBLECLK, SM_CYDOUBLECLK},
    },
};

use crate::geom::Rect;

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct Modifiers {
    pub(crate) ctrl: bool,
    pub(crate) alt: bool,
    pub(crate) shift: bool,
    pub(crate) win: bool,
    /// True when Windows reports AltGr (right Alt + synthetic left Ctrl, K-4).
    pub(crate) alt_gr: bool,
}

impl Modifiers {
    #[inline]
    pub(crate) const fn none(self) -> bool {
        !self.ctrl && !self.alt && !self.shift && !self.win
    }

    #[inline]
    pub(crate) const fn ctrl_only(self) -> bool {
        self.ctrl && !self.alt && !self.shift && !self.win && !self.alt_gr
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Key {
    Char(char),
    Backspace,
    Tab,
    Enter,
    Escape,
    Space,
    PageUp,
    PageDown,
    End,
    Home,
    Left,
    Up,
    Right,
    Down,
    Insert,
    Delete,
    Alt,
    Menu,
    F(u8),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct KeyEvent {
    pub(crate) key: Key,
    pub(crate) modifiers: Modifiers,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum MouseButton {
    Left,
    Right,
    Middle,
}

#[derive(Default)]
pub(super) struct ClickTracker {
    last_button: Option<MouseButton>,
    last_pos: (i32, i32),
    last_time_ms: Option<u32>,
    count: usize,
}

impl ClickTracker {
    pub(super) fn record(&mut self, button: MouseButton, x: i32, y: i32) -> usize {
        // Use Win32 message timestamp (`GetMessageTime`) and half system double-click rect (K-9).
        // SAFETY: Querying Win32 message timestamp and double-click metrics requires no preconditions.
        let now_ms = unsafe { GetMessageTime() } as u32;
        // SAFETY: `GetDoubleClickTime` is a pure OS metric query.
        let dbl_ms = unsafe { GetDoubleClickTime() };
        // SAFETY: `GetSystemMetrics` is a pure OS metric query.
        let tol_x = (unsafe { GetSystemMetrics(SM_CXDOUBLECLK) } / 2).max(2);
        // SAFETY: `GetSystemMetrics` is a pure OS metric query.
        let tol_y = (unsafe { GetSystemMetrics(SM_CYDOUBLECLK) } / 2).max(2);
        let same_spot = self.last_button == Some(button)
            && self
                .last_time_ms
                .is_some_and(|t| now_ms.wrapping_sub(t) <= dbl_ms)
            && (x - self.last_pos.0).abs() <= tol_x
            && (y - self.last_pos.1).abs() <= tol_y;
        self.count = if same_spot { self.count + 1 } else { 1 };
        self.last_button = Some(button);
        self.last_pos = (x, y);
        self.last_time_ms = Some(now_ms);
        self.count
    }
}

pub(crate) fn current_modifiers() -> Modifiers {
    // SAFETY: `GetKeyState` reads synchronous key state on the calling UI thread.
    let down = |vk: VIRTUAL_KEY| (unsafe { GetKeyState(i32::from(vk.0)) } as u16 & 0x8000) != 0;
    let lctrl = down(VK_LCONTROL);
    let rctrl = down(VK_RCONTROL);
    let ralt = down(VK_RMENU);
    // Windows synthesizes VK_LCONTROL + VK_RMENU for AltGr on international keyboards (K-4).
    let alt_gr = ralt && lctrl && !rctrl;
    Modifiers {
        ctrl: down(VK_CONTROL),
        alt: down(VK_MENU),
        shift: down(VK_SHIFT),
        win: down(VK_LWIN) || down(VK_RWIN),
        alt_gr,
    }
}

pub(super) fn is_syskey_passthrough(vk: u16) -> bool {
    vk == VK_F4.0 || vk == VK_SPACE.0
}

pub(super) fn translate_vk(vk: u16, modifiers: Modifiers) -> Option<Key> {
    let v = VIRTUAL_KEY(vk);
    let key = match v {
        VK_BACK => Key::Backspace,
        VK_TAB => Key::Tab,
        VK_RETURN => Key::Enter,
        VK_ESCAPE => Key::Escape,
        VK_SPACE => Key::Space,
        VK_PRIOR => Key::PageUp,
        VK_NEXT => Key::PageDown,
        VK_END => Key::End,
        VK_HOME => Key::Home,
        VK_LEFT => Key::Left,
        VK_UP => Key::Up,
        VK_RIGHT => Key::Right,
        VK_DOWN => Key::Down,
        VK_INSERT => Key::Insert,
        VK_DELETE => Key::Delete,
        VK_MENU => Key::Alt,
        VK_APPS => Key::Menu,
        VK_OEM_PLUS => Key::Char(if modifiers.shift { '+' } else { '=' }),
        VK_ADD => Key::Char('+'),
        VK_OEM_MINUS | VK_SUBTRACT => Key::Char('-'),
        VK_OEM_4 => Key::Char('['),
        VK_OEM_6 => Key::Char(']'),
        _ if (VK_F1.0..=VK_F12.0).contains(&vk) => Key::F((vk - VK_F1.0 + 1) as u8),
        _ if (VK_NUMPAD0.0..=VK_NUMPAD9.0).contains(&vk) => {
            Key::Char((b'0' + (vk - VK_NUMPAD0.0) as u8) as char)
        }
        _ if (b'0' as u16..=b'9' as u16).contains(&vk) => Key::Char(vk as u8 as char),
        _ if (b'A' as u16..=b'Z' as u16).contains(&vk) => {
            Key::Char((vk as u8 as char).to_ascii_lowercase())
        }
        _ => return None,
    };
    Some(key)
}

pub(super) fn read_ime_string(himc: HIMC, flag: IME_COMPOSITION_STRING) -> Option<String> {
    // SAFETY: `himc` is a valid IMC handle acquired by caller; querying byte length with `None`.
    let bytes = unsafe { ImmGetCompositionStringW(himc, flag, None, 0) };
    if bytes <= 0 {
        return None;
    }
    let len = (bytes as usize) / 2;
    let mut buf = vec![0u16; len];
    // SAFETY: `buf` has `len * 2 >= bytes` writable bytes for `ImmGetCompositionStringW`.
    let written = unsafe {
        ImmGetCompositionStringW(himc, flag, Some(buf.as_mut_ptr().cast()), bytes as u32)
    };
    if written <= 0 {
        return None;
    }
    String::from_utf16(&buf[..(written as usize) / 2]).ok()
}

pub(super) fn set_ime_candidate_pos(hwnd: HWND, caret_rect: Rect, scale: f32) {
    let pt = POINT {
        x: (caret_rect.origin.x * scale).round() as i32,
        y: (caret_rect.bottom() * scale).round() as i32,
    };
    // SAFETY: `hwnd` is an active UI-thread window; invalid HIMC is checked immediately below.
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
    // SAFETY: `himc` is valid and `comp_form` / `cand_form` live for the call; released right after.
    unsafe {
        let _ = ImmSetCompositionWindow(himc, &comp_form);
        let _ = ImmSetCandidateWindow(himc, &cand_form);
        let _ = ImmReleaseContext(hwnd, himc);
    }
}

/// Asks Windows IME to commit any active composition string before focus/tab/pointer transitions (E-2).
pub(crate) fn complete_ime_composition(hwnd: HWND) {
    // SAFETY: `ImmGetContext` is called on the UI thread owning `hwnd`; invalid HIMC is checked.
    let himc = unsafe { ImmGetContext(hwnd) };
    if himc.is_invalid() {
        return;
    }
    // SAFETY: `himc` is a valid IMC handle paired with `ImmReleaseContext` below.
    unsafe {
        let _ = ImmNotifyIME(himc, NI_COMPOSITIONSTR, CPS_COMPLETE, 0);
        let _ = ImmReleaseContext(hwnd, himc);
    }
}
