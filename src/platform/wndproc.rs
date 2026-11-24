use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;

use windows::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
    Graphics::Gdi::{ScreenToClient, ValidateRect},
    System::Shutdown::{ShutdownBlockReasonCreate, ShutdownBlockReasonDestroy},
    UI::{
        Controls::WM_MOUSELEAVE,
        HiDpi::GetDpiForWindow,
        Input::{
            Ime::{
                GCS_COMPSTR, GCS_CURSORPOS, GCS_RESULTSTR, ImmGetCompositionStringW, ImmGetContext,
                ImmReleaseContext,
            },
            KeyboardAndMouse::{
                ReleaseCapture, SetCapture, TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT,
                TrackMouseEvent,
            },
        },
        Shell::{DragFinish, DragQueryFileW, HDROP},
        WindowsAndMessaging::{
            DefWindowProcW, GetCursorPos, GetWindowRect, HTCAPTION, HTCLIENT, HTCLOSE, HTMAXBUTTON,
            HTMINBUTTON, HTTOP, HTTOPLEFT, HTTOPRIGHT, IsZoomed, KillTimer, MINMAXINFO,
            NCCALCSIZE_PARAMS, PostMessageW, SC_CLOSE, SC_MAXIMIZE, SC_MINIMIZE, SC_RESTORE,
            SWP_NOACTIVATE, SWP_NOZORDER, SetTimer, SetWindowPos, USER_DEFAULT_SCREEN_DPI,
            USER_TIMER_MINIMUM, WM_ACTIVATE, WM_CAPTURECHANGED, WM_CHAR, WM_CLOSE, WM_DPICHANGED,
            WM_DROPFILES, WM_ENDSESSION, WM_ENTERMENULOOP, WM_ENTERSIZEMOVE, WM_ERASEBKGND,
            WM_EXITMENULOOP, WM_EXITSIZEMOVE, WM_GETMINMAXINFO, WM_IME_COMPOSITION,
            WM_IME_ENDCOMPOSITION, WM_IME_STARTCOMPOSITION, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDBLCLK,
            WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEHWHEEL,
            WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_MOVE, WM_NCCALCSIZE, WM_NCHITTEST, WM_NCLBUTTONDBLCLK,
            WM_NCLBUTTONDOWN, WM_NCLBUTTONUP, WM_NCMOUSELEAVE, WM_NCMOUSEMOVE, WM_PAINT,
            WM_QUERYENDSESSION, WM_RBUTTONDBLCLK, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SETCURSOR,
            WM_SETTINGCHANGE, WM_SIZE, WM_SYSCHAR, WM_SYSCOMMAND, WM_SYSKEYDOWN, WM_SYSKEYUP,
            WM_THEMECHANGED, WM_TIMER, WindowFromPoint,
        },
    },
};
use windows::core::{HSTRING, PCWSTR};

use crate::geom::{Point, point};

use super::ffi_guard;
use super::{PlatformEvent, WindowControlArea, hit_test_control_areas};
use super::{
    Key, KeyEvent, MouseButton, current_modifiers, is_syskey_passthrough, read_ime_string,
    set_ime_candidate_pos, translate_vk,
};
use super::pump::{self, dispatch, lookup_window, service};
use super::settings::SystemSettings;
use super::window::{
    MouseButtons, SIZE_MOVE_TIMER_ID, apply_cursor, frame_thickness_for_dpi, query_physical_size,
};

fn client_point_from_lparam(lparam: LPARAM, scale: f32) -> (i32, i32, Point) {
    let x = (lparam.0 & 0xFFFF) as i16 as i32;
    let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
    (x, y, point(x as f32 / scale, y as f32 / scale))
}

fn screen_to_client_point(hwnd: HWND, lparam: LPARAM, scale: f32) -> Point {
    let mut pt = POINT {
        x: (lparam.0 & 0xFFFF) as i16 as i32,
        y: ((lparam.0 >> 16) & 0xFFFF) as i16 as i32,
    };
    let _ = unsafe { ScreenToClient(hwnd, &mut pt) };
    point(pt.x as f32 / scale, pt.y as f32 / scale)
}

fn cursor_is_over_hwnd(hwnd: HWND) -> bool {
    let mut pt = POINT::default();
    unsafe { GetCursorPos(&mut pt).is_ok() && WindowFromPoint(pt) == hwnd }
}

fn is_relevant_setting_change(lparam: LPARAM) -> bool {
    if lparam.0 == 0 {
        return true;
    }
    let ptr = lparam.0 as *const u16;
    let pcw = PCWSTR(ptr);
    let wide = unsafe { pcw.as_wide() };
    let name = String::from_utf16_lossy(wide);
    name.eq_ignore_ascii_case("ImmersiveColorSet")
        || name.eq_ignore_ascii_case("WindowMetrics")
        || name.eq_ignore_ascii_case("HighContrast")
}

fn update_ime_pos(hwnd: HWND) {
    if let Some(s) = lookup_window(hwnd)
        && let Some(rect) = s.ime_caret.get()
    {
        set_ime_candidate_pos(hwnd, rect, s.scale());
    }
}

fn release_capture_suppressed(hwnd: HWND, release: impl FnOnce()) {
    let shared = lookup_window(hwnd);
    if let Some(s) = &shared {
        s.input.borrow_mut().releasing_capture = true;
    }
    release();
    if let Some(s) = &shared {
        s.input.borrow_mut().releasing_capture = false;
    }
}

fn handle_mouse_move(hwnd: HWND, lparam: LPARAM, live_buttons: Option<MouseButtons>) {
    let Some(s) = lookup_window(hwnd) else {
        return;
    };
    if s.in_size_move.get() {
        return;
    }
    let non_client = live_buttons.is_none();
    let scale = s.scale();
    let (need_track, ev) = {
        let mut input = s.input.borrow_mut();
        let (need_track, position, pressed_button) = match live_buttons {
            None => (
                !input.tracking_nc_leave,
                screen_to_client_point(hwnd, lparam, scale),
                input.buttons.primary(),
            ),
            Some(live) => {
                input.buttons = live;
                (
                    !input.tracking_leave,
                    client_point_from_lparam(lparam, scale).2,
                    live.primary(),
                )
            }
        };
        input.hovered = true;
        (
            need_track,
            PlatformEvent::MouseMove {
                window_id: s.id,
                position,
                pressed_button,
            },
        )
    };
    if need_track {
        let mut tme = TRACKMOUSEEVENT {
            cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
            dwFlags: if non_client {
                TME_LEAVE | TME_NONCLIENT
            } else {
                TME_LEAVE
            },
            hwndTrack: hwnd,
            dwHoverTime: 0,
        };
        if unsafe { TrackMouseEvent(&mut tme) }.is_ok() {
            let mut input = s.input.borrow_mut();
            if non_client {
                input.tracking_nc_leave = true;
            } else {
                input.tracking_leave = true;
            }
        }
    }
    dispatch(ev);
}

fn handle_mouse_leave(hwnd: HWND, non_client: bool) {
    let Some(s) = lookup_window(hwnd) else {
        return;
    };
    let check_leave = {
        let mut input = s.input.borrow_mut();
        if non_client {
            input.tracking_nc_leave = false;
            input.nc_button_down = None;
            !input.tracking_leave
        } else {
            input.tracking_leave = false;
            !input.tracking_nc_leave
        }
    };
    if check_leave && !cursor_is_over_hwnd(hwnd) {
        s.input.borrow_mut().hovered = false;
        dispatch(PlatformEvent::MouseLeave { window_id: s.id });
    }
}

fn dispatch_key_message(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    is_down: bool,
) -> LRESULT {
    let vk = (wparam.0 & 0xFFFF) as u16;
    let modifiers = current_modifiers();
    let mut handled = false;
    if let Some(s) = lookup_window(hwnd) {
        let mods_changed = {
            let mut input = s.input.borrow_mut();
            input.suppress_next_char = false;
            (modifiers != input.last_modifiers).then(|| {
                input.last_modifiers = modifiers;
                PlatformEvent::ModifiersChanged { modifiers }
            })
        };
        if let Some(ev) = mods_changed {
            dispatch(ev);
        }
        if let Some(key) = translate_vk(vk, modifiers) {
            let mut stroke_mods = modifiers;
            if key == Key::Alt {
                stroke_mods.alt = false;
            }
            let event = KeyEvent {
                key,
                modifiers: stroke_mods,
            };
            if is_down {
                let is_held = (lparam.0 & (1 << 30)) != 0;
                if is_held && matches!(key, Key::Alt | Key::F(10)) {
                    handled = true;
                } else {
                    handled = dispatch(PlatformEvent::KeyDown {
                        window_id: s.id,
                        event,
                    });
                    if handled {
                        s.input.borrow_mut().suppress_next_char = true;
                    }
                }
            } else {
                dispatch(PlatformEvent::KeyUp {
                    window_id: s.id,
                    event,
                });
            }
        }
    }
    if (is_down && handled)
        || (msg == WM_SYSKEYDOWN && !is_syskey_passthrough(vk))
        || msg == WM_SYSKEYUP
    {
        return LRESULT(0);
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

pub(super) unsafe extern "system" fn window_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    ffi_guard("window_wnd_proc", LRESULT(0), || {
        window_wnd_proc_inner(hwnd, msg, wparam, lparam)
    })
}

fn window_wnd_proc_inner(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_ERASEBKGND => LRESULT(1),

        WM_PAINT => {
            let _ = unsafe { ValidateRect(Some(hwnd), None) };
            if let Some(s) = lookup_window(hwnd) {
                dispatch(PlatformEvent::PaintRequested { window_id: s.id });
            }
            LRESULT(0)
        }

        WM_NCCALCSIZE if wparam.0 != 0 => {
            let params = unsafe { &mut *(lparam.0 as *mut NCCALCSIZE_PARAMS) };
            let saved_top = params.rgrc[0].top;
            let result = unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
            params.rgrc[0].top = saved_top;
            if unsafe { IsZoomed(hwnd) }.as_bool() {
                let dpi = unsafe { GetDpiForWindow(hwnd) }.max(USER_DEFAULT_SCREEN_DPI);
                let (_, by) = frame_thickness_for_dpi(dpi);
                params.rgrc[0].top += by;
            }
            result
        }

        WM_GETMINMAXINFO => {
            if let Some(s) = lookup_window(hwnd)
                && let Some(min_size) = s.chrome.borrow().min_size
            {
                let dpi = unsafe { GetDpiForWindow(hwnd) }.max(USER_DEFAULT_SCREEN_DPI);
                let scale = dpi as f32 / USER_DEFAULT_SCREEN_DPI as f32;
                let (bx, by) = frame_thickness_for_dpi(dpi);
                let mmi = unsafe { &mut *(lparam.0 as *mut MINMAXINFO) };
                mmi.ptMinTrackSize.x = (min_size.width * scale).ceil() as i32 + bx * 2;
                mmi.ptMinTrackSize.y = (min_size.height * scale).ceil() as i32 + by;
                return LRESULT(0);
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }

        WM_NCHITTEST => {
            let def_hit = unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
            if def_hit.0 != HTCLIENT as isize {
                return def_hit;
            }
            let hit = lookup_window(hwnd).map(|s| {
                let mut rc = RECT::default();
                let _ = unsafe { GetWindowRect(hwnd, &mut rc) };
                let sx = (lparam.0 & 0xFFFF) as i16 as i32;
                let sy = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
                if !unsafe { IsZoomed(hwnd) }.as_bool() {
                    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(USER_DEFAULT_SCREEN_DPI);
                    let (bx, by) = frame_thickness_for_dpi(dpi);
                    if sy < rc.top + by {
                        if sx < rc.left + bx * 2 {
                            return HTTOPLEFT;
                        }
                        if sx >= rc.right - bx * 2 {
                            return HTTOPRIGHT;
                        }
                        return HTTOP;
                    }
                }
                let local = screen_to_client_point(hwnd, lparam, s.scale());
                match hit_test_control_areas(&s.chrome.borrow().control_areas, local) {
                    WindowControlArea::Close => HTCLOSE,
                    WindowControlArea::Max => HTMAXBUTTON,
                    WindowControlArea::Min => HTMINBUTTON,
                    WindowControlArea::Drag => HTCAPTION,
                    WindowControlArea::Client => HTCLIENT,
                }
            });
            LRESULT(hit.unwrap_or(HTCLIENT) as isize)
        }

        WM_SETCURSOR if (lparam.0 & 0xFFFF) as u32 == HTCLIENT => {
            if let Some(s) = lookup_window(hwnd) {
                apply_cursor(s.chrome.borrow().cursor);
            }
            LRESULT(1)
        }

        WM_NCMOUSEMOVE => {
            handle_mouse_move(hwnd, lparam, None);
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }

        WM_NCMOUSELEAVE => {
            handle_mouse_leave(hwnd, true);
            LRESULT(0)
        }

        WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK => {
            let area = wparam.0 as u32;
            if msg == WM_NCLBUTTONDBLCLK && area == HTCAPTION {
                return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
            }
            if matches!(area, HTMINBUTTON | HTMAXBUTTON | HTCLOSE | HTCAPTION) {
                if let Some(s) = lookup_window(hwnd) {
                    {
                        let mut input = s.input.borrow_mut();
                        if area != HTCAPTION {
                            input.nc_button_down = Some(area);
                        }
                        input.hovered = true;
                    }
                    let position = screen_to_client_point(hwnd, lparam, s.scale());
                    dispatch(PlatformEvent::MouseDown {
                        window_id: s.id,
                        button: MouseButton::Left,
                        position,
                        modifiers: current_modifiers(),
                        click_count: 1,
                    });
                }
                if area != HTCAPTION {
                    return LRESULT(0);
                }
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }

        WM_NCLBUTTONUP => {
            let area = wparam.0 as u32;
            if matches!(area, HTMINBUTTON | HTMAXBUTTON | HTCLOSE) {
                if let Some(s) = lookup_window(hwnd) {
                    let was = s.input.borrow_mut().nc_button_down.take();
                    let position = screen_to_client_point(hwnd, lparam, s.scale());
                    dispatch(PlatformEvent::MouseUp {
                        window_id: s.id,
                        button: MouseButton::Left,
                        position,
                    });
                    if was == Some(area) {
                        let cmd = match area {
                            HTMINBUTTON => SC_MINIMIZE,
                            HTMAXBUTTON => {
                                if unsafe { IsZoomed(hwnd) }.as_bool() {
                                    SC_RESTORE
                                } else {
                                    SC_MAXIMIZE
                                }
                            }
                            HTCLOSE => SC_CLOSE,
                            _ => 0,
                        };
                        if cmd != 0 {
                            let _ = unsafe {
                                PostMessageW(
                                    Some(hwnd),
                                    WM_SYSCOMMAND,
                                    WPARAM(cmd as usize),
                                    LPARAM(0),
                                )
                            };
                        }
                    }
                }
                return LRESULT(0);
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }

        WM_MOUSEMOVE => {
            handle_mouse_move(hwnd, lparam, Some(MouseButtons::from_wparam(wparam)));
            LRESULT(0)
        }

        WM_MOUSELEAVE => {
            handle_mouse_leave(hwnd, false);
            LRESULT(0)
        }

        WM_CAPTURECHANGED => {
            if let Some(s) = lookup_window(hwnd) {
                let emit = {
                    let mut input = s.input.borrow_mut();
                    if input.releasing_capture {
                        false
                    } else {
                        input.buttons.clear();
                        true
                    }
                };
                if emit {
                    dispatch(PlatformEvent::CaptureLost { window_id: s.id });
                }
            }
            LRESULT(0)
        }

        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK | WM_RBUTTONDOWN | WM_RBUTTONDBLCLK | WM_MBUTTONDOWN => {
            let button = match msg {
                WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => MouseButton::Left,
                WM_RBUTTONDOWN | WM_RBUTTONDBLCLK => MouseButton::Right,
                _ => MouseButton::Middle,
            };
            unsafe {
                SetCapture(hwnd);
            }
            if let Some(s) = lookup_window(hwnd) {
                let (x, y, position) = client_point_from_lparam(lparam, s.scale());
                let click_count = {
                    let mut input = s.input.borrow_mut();
                    input.buttons.press(button);
                    input.clicks.record(button, x, y)
                };
                dispatch(PlatformEvent::MouseDown {
                    window_id: s.id,
                    button,
                    position,
                    modifiers: current_modifiers(),
                    click_count,
                });
            }
            LRESULT(0)
        }

        WM_LBUTTONUP | WM_RBUTTONUP | WM_MBUTTONUP => {
            let button = match msg {
                WM_LBUTTONUP => MouseButton::Left,
                WM_RBUTTONUP => MouseButton::Right,
                _ => MouseButton::Middle,
            };
            let (release_cap, ev) = if let Some(s) = lookup_window(hwnd) {
                let release_cap = {
                    let mut input = s.input.borrow_mut();
                    input.buttons.release(button);
                    input.buttons.is_empty()
                };
                let (_, _, position) = client_point_from_lparam(lparam, s.scale());
                (
                    release_cap,
                    Some(PlatformEvent::MouseUp {
                        window_id: s.id,
                        button,
                        position,
                    }),
                )
            } else {
                (true, None)
            };
            if release_cap {
                release_capture_suppressed(hwnd, || {
                    let _ = unsafe { ReleaseCapture() };
                });
            }
            if let Some(ev) = ev {
                dispatch(ev);
            }
            LRESULT(0)
        }

        WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
            let modifiers = current_modifiers();
            let horizontal = msg == WM_MOUSEHWHEEL || modifiers.shift;
            let raw_delta = ((wparam.0 >> 16) & 0xFFFF) as i16 as f32;
            let notches = raw_delta / 120.0;
            let raw_amount = pump::cached_settings().wheel_scroll_amount(horizontal);
            // WHEEL_PAGESCROLL is u32::MAX.
            let page_scroll = !horizontal && raw_amount == u32::MAX;
            let amount = if page_scroll { 1.0 } else { raw_amount.max(1) as f32 };
            let delta_lines = if horizontal {
                point(
                    if msg == WM_MOUSEHWHEEL {
                        -notches * amount
                    } else {
                        notches * amount
                    },
                    0.0,
                )
            } else {
                point(0.0, notches * amount)
            };
            if let Some(s) = lookup_window(hwnd) {
                let position = screen_to_client_point(hwnd, lparam, s.scale());
                dispatch(PlatformEvent::ScrollWheel {
                    window_id: s.id,
                    position,
                    delta_lines,
                    page_scroll,
                    modifiers,
                });
            }
            LRESULT(0)
        }

        WM_KEYDOWN | WM_SYSKEYDOWN => dispatch_key_message(hwnd, msg, wparam, lparam, true),

        WM_SYSCHAR => {
            // Forward Alt+Space to DefWindowProcW so the native system menu opens.
            if (wparam.0 & 0xFFFF) == b' ' as usize {
                return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
            }
            LRESULT(0)
        }

        WM_KEYUP | WM_SYSKEYUP => dispatch_key_message(hwnd, msg, wparam, lparam, false),

        WM_CHAR => {
            let code = (wparam.0 & 0xFFFF) as u16;
            if let Some(s) = lookup_window(hwnd) {
                let text = {
                    let mut input = s.input.borrow_mut();
                    if std::mem::take(&mut input.suppress_next_char) {
                        None
                    } else if (0xD800..=0xDBFF).contains(&code) {
                        input.pending_surrogate = Some(code);
                        None
                    } else if (0xDC00..=0xDFFF).contains(&code) {
                        input
                            .pending_surrogate
                            .take()
                            .and_then(|high| String::from_utf16(&[high, code]).ok())
                    } else {
                        input.pending_surrogate = None;
                        (code >= 0x20 && code != 0x7F)
                            .then(|| char::from_u32(u32::from(code)).map(|c| c.to_string()))
                            .flatten()
                    }
                };
                if let Some(text) = text {
                    dispatch(PlatformEvent::TextInput {
                        window_id: s.id,
                        text,
                    });
                }
            }
            LRESULT(0)
        }

        WM_IME_STARTCOMPOSITION => {
            update_ime_pos(hwnd);
            LRESULT(0)
        }

        WM_IME_COMPOSITION => {
            let flags = lparam.0 as u32;
            if let Some(s) = lookup_window(hwnd) {
                let himc = unsafe { ImmGetContext(hwnd) };
                if !himc.is_invalid() {
                    if (flags & GCS_RESULTSTR.0) != 0
                        && let Some(text) = read_ime_string(himc, GCS_RESULTSTR)
                    {
                        dispatch(PlatformEvent::TextInput {
                            window_id: s.id,
                            text,
                        });
                    }
                    if (flags & GCS_COMPSTR.0) != 0 {
                        let text = read_ime_string(himc, GCS_COMPSTR).unwrap_or_default();
                        let cursor = unsafe {
                            ImmGetCompositionStringW(himc, GCS_CURSORPOS, None, 0)
                        }
                        .max(0) as usize;
                        dispatch(PlatformEvent::ImeComposition {
                            window_id: s.id,
                            text,
                            cursor,
                        });
                    }
                    let _ = unsafe { ImmReleaseContext(hwnd, himc) };
                    update_ime_pos(hwnd);
                }
            }
            LRESULT(0)
        }

        WM_IME_ENDCOMPOSITION => {
            if let Some(s) = lookup_window(hwnd) {
                dispatch(PlatformEvent::ImeComposition {
                    window_id: s.id,
                    text: String::new(),
                    cursor: 0,
                });
            }
            LRESULT(0)
        }

        WM_SIZE => {
            let w = (lparam.0 & 0xFFFF) as u32;
            let h = ((lparam.0 >> 16) & 0xFFFF) as u32;
            if w > 0
                && h > 0
                && let Some(s) = lookup_window(hwnd)
            {
                let scale = s.scale();
                s.set_metrics(w, h, scale);
                dispatch(PlatformEvent::Resized {
                    window_id: s.id,
                    physical_width: w,
                    physical_height: h,
                    scale,
                });
            }
            LRESULT(0)
        }

        WM_MOVE => {
            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            if let Some(s) = lookup_window(hwnd)
                && s.last_origin_px.replace(Some((x, y))) != Some((x, y))
            {
                dispatch(PlatformEvent::Moved { window_id: s.id });
            }
            LRESULT(0)
        }

        WM_DPICHANGED => {
            let new_dpi = (wparam.0 & 0xFFFF) as u32;
            let scale =
                new_dpi.max(USER_DEFAULT_SCREEN_DPI) as f32 / USER_DEFAULT_SCREEN_DPI as f32;
            // Update scale before SetWindowPos so nested WM_SIZE sees the new DPI.
            if let Some(s) = lookup_window(hwnd) {
                let (pw, ph) = s.metrics.get().physical;
                s.set_metrics(pw, ph, scale);
                let suggested = unsafe { &*(lparam.0 as *const RECT) };
                unsafe {
                    let _ = SetWindowPos(
                        hwnd,
                        None,
                        suggested.left,
                        suggested.top,
                        suggested.right - suggested.left,
                        suggested.bottom - suggested.top,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                }
                let (w, h) = query_physical_size(hwnd);
                s.set_metrics(w, h, scale);
                dispatch(PlatformEvent::Resized {
                    window_id: s.id,
                    physical_width: w,
                    physical_height: h,
                    scale,
                });
            }
            LRESULT(0)
        }

        WM_ACTIVATE => {
            let active = (wparam.0 & 0xFFFF) != 0;
            if let Some(s) = lookup_window(hwnd) {
                s.active.set(active);
                dispatch(PlatformEvent::Activated {
                    window_id: s.id,
                    active,
                });
            }
            LRESULT(0)
        }

        WM_SETTINGCHANGE | WM_THEMECHANGED => {
            if lookup_window(hwnd).is_some()
                && (msg == WM_THEMECHANGED || is_relevant_setting_change(lparam))
            {
                let fresh = SystemSettings::query();
                pump::set_cached_settings(fresh);
                dispatch(PlatformEvent::SettingsChanged(fresh));
            }
            LRESULT(0)
        }

        WM_ENTERSIZEMOVE | WM_ENTERMENULOOP => {
            if let Some(s) = lookup_window(hwnd) {
                s.in_size_move.set(true);
                let was_hovered = std::mem::take(&mut s.input.borrow_mut().hovered);
                if was_hovered {
                    dispatch(PlatformEvent::MouseLeave { window_id: s.id });
                }
                dispatch(PlatformEvent::SizeMoveChanged {
                    window_id: s.id,
                    entering: true,
                });
            }
            unsafe {
                let _ = SetTimer(Some(hwnd), SIZE_MOVE_TIMER_ID, USER_TIMER_MINIMUM, None);
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
        }

        WM_TIMER if wparam.0 == SIZE_MOVE_TIMER_ID => {
            pump::mark_wake_pending();
            service();
            LRESULT(0)
        }

        WM_EXITSIZEMOVE | WM_EXITMENULOOP => {
            let _ = unsafe { KillTimer(Some(hwnd), SIZE_MOVE_TIMER_ID) };
            if let Some(s) = lookup_window(hwnd) {
                s.in_size_move.set(false);
                dispatch(PlatformEvent::SizeMoveChanged {
                    window_id: s.id,
                    entering: false,
                });
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }

        WM_QUERYENDSESSION => {
            if let Some(reason) = pump::shutdown_block() {
                let wide = HSTRING::from(reason);
                let _ = unsafe { ShutdownBlockReasonCreate(hwnd, &wide) };
                if let Some(s) = lookup_window(hwnd) {
                    dispatch(PlatformEvent::CloseRequested { window_id: s.id });
                }
                LRESULT(0)
            } else {
                let _ = unsafe { ShutdownBlockReasonDestroy(hwnd) };
                LRESULT(1)
            }
        }

        WM_ENDSESSION => {
            let ending = wparam.0 != 0;
            if !ending {
                let _ = unsafe { ShutdownBlockReasonDestroy(hwnd) };
            }
            dispatch(PlatformEvent::SessionEnding { ending });
            LRESULT(0)
        }

        WM_DROPFILES => {
            let hdrop = HDROP(wparam.0 as *mut _);
            let paths = collect_drop_paths(hdrop);
            unsafe { DragFinish(hdrop) };
            if !paths.is_empty()
                && let Some(s) = lookup_window(hwnd)
            {
                dispatch(PlatformEvent::FilesDropped {
                    window_id: s.id,
                    paths,
                });
            }
            LRESULT(0)
        }

        WM_CLOSE => {
            if let Some(s) = lookup_window(hwnd) {
                dispatch(PlatformEvent::CloseRequested { window_id: s.id });
            }
            LRESULT(0)
        }

        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn collect_drop_paths(hdrop: HDROP) -> Vec<PathBuf> {
    let file_count = unsafe { DragQueryFileW(hdrop, 0xFFFF_FFFF, None) };
    let mut paths = Vec::with_capacity(file_count as usize);
    for ix in 0..file_count {
        let len = unsafe { DragQueryFileW(hdrop, ix, None) } as usize;
        if len == 0 {
            continue;
        }
        let mut buffer = vec![0u16; len + 1];
        let written = unsafe { DragQueryFileW(hdrop, ix, Some(buffer.as_mut_slice())) } as usize;
        if written > 0 {
            paths.push(PathBuf::from(OsString::from_wide(&buffer[..written])));
        }
    }
    paths
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use windows::Win32::UI::WindowsAndMessaging::SendMessageW;

    use super::*;
    use crate::platform::{MainWindowConfig, PlatformHandler, WindowAppearance};
    use crate::platform::pump::Platform;
    use crate::platform::window::NativeWindow;

    #[test]
    fn release_capture_suppresses_synchronous_capture_lost_but_external_emits() {
        #[derive(Default)]
        struct CaptureProbe {
            lost: usize,
        }

        impl PlatformHandler for CaptureProbe {
            fn on_event(&mut self, event: PlatformEvent) -> bool {
                if matches!(event, PlatformEvent::CaptureLost { .. }) {
                    self.lost += 1;
                }
                false
            }

            fn on_wake(&mut self) {}
            fn drive_frames(&mut self) {}
        }

        let _platform = Platform::new(None).expect("Platform::new failed");
        let cfg = MainWindowConfig {
            show: false,
            focus: false,
            ..Default::default()
        };
        let win = NativeWindow::create_main(&cfg, WindowAppearance::Light).expect("window");
        let hwnd = win.hwnd().raw();
        let probe = Rc::new(RefCell::new(CaptureProbe::default()));

        Platform::with_test_handler(probe.clone(), || {
            release_capture_suppressed(hwnd, || unsafe {
                let _ = SendMessageW(hwnd, WM_CAPTURECHANGED, Some(WPARAM(0)), Some(LPARAM(0)));
            });
            assert_eq!(
                probe.borrow().lost,
                0,
                "synchronous WM_CAPTURECHANGED during own ReleaseCapture must be suppressed"
            );

            unsafe {
                let _ = SendMessageW(hwnd, WM_CAPTURECHANGED, Some(WPARAM(0)), Some(LPARAM(0)));
            }
            assert_eq!(
                probe.borrow().lost,
                1,
                "external WM_CAPTURECHANGED must emit CaptureLost"
            );
        });
    }
}
