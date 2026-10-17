use super::*;
use std::time::Instant;
use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
};
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, HWND_TOPMOST, SPI_GETMESSAGEDURATION, SPI_GETMOUSEHOVERTIME, SW_HIDE,
    SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SetWindowPos, ShowWindow, SystemParametersInfoW,
};

const PADDING_X: f32 = 9.;
const PADDING_TOP: f32 = 6.;
const PADDING_BOTTOM: f32 = 8.;
const MAX_WIDTH: f32 = 320.;
const FONT_SIZE: f32 = 12.;
const LINE_HEIGHT: f32 = 16.;
const RADIUS: f32 = 4.;
const MARGIN: f32 = 16.;
const POINTER_OFFSET: f32 = 20.;
const DEFAULT_HOVER_TIME: u32 = 400;
const DEFAULT_SHOW_SECONDS: u32 = 5;
const RESHOW_WINDOW: Duration = Duration::from_millis(200);

pub(crate) struct TooltipView {
    text: SharedString,
}

impl Render for TooltipView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        div().size_full().p(px(MARGIN)).child(
            div()
                .size_full()
                .rounded(px(RADIUS))
                .border_1()
                .border_color(rgba(p.tooltip_border))
                .bg(rgb(p.tooltip_fill))
                .shadow(vec![BoxShadow {
                    color: hsla(0., 0., 0., p.shadow),
                    offset: point(px(0.), px(4.)),
                    blur_radius: px(8.),
                    spread_radius: px(0.),
                    inset: false,
                }])
                .pl(px(PADDING_X - 1.))
                .pr(px(PADDING_X - 1.))
                .pt(px(PADDING_TOP - 1.))
                .font_family(UI_FONT)
                .text_size(px(FONT_SIZE))
                .line_height(px(LINE_HEIGHT))
                .text_color(rgba(p.text_primary))
                .child(self.text.clone()),
        )
    }
}

struct Owner {
    id: ElementId,
    depth: u8,
    text: SharedString,
}

struct Popup {
    view: Entity<TooltipView>,
    handle: AnyWindowHandle,
    hwnd: isize,
}

#[derive(Default)]
struct Tooltips {
    popup: Option<Popup>,
    hovered: Vec<Owner>,
    current: Option<ElementId>,
    shown: bool,
    hidden_at: Option<Instant>,
    pending: Option<Task<()>>,
}

impl Global for Tooltips {}

pub(crate) fn hover(
    id: impl Into<ElementId>,
    depth: u8,
    text: impl Into<SharedString>,
    hovered: bool,
    cx: &mut App,
) {
    let id = id.into();
    let tips = cx.default_global::<Tooltips>();
    tips.hovered.retain(|owner| owner.id != id);
    if hovered {
        tips.hovered.push(Owner {
            id,
            depth,
            text: text.into(),
        });
    }
    let candidate = tips
        .hovered
        .iter()
        .rev()
        .max_by_key(|owner| owner.depth)
        .map(|owner| (owner.id.clone(), owner.text.clone()));
    if candidate.as_ref().map(|(id, _)| id) == tips.current.as_ref() {
        return;
    }
    hide(cx);
    let Some((id, text)) = candidate else {
        cx.default_global::<Tooltips>().current = None;
        return;
    };
    let tips = cx.default_global::<Tooltips>();
    let reshow = tips
        .hidden_at
        .is_some_and(|hidden| hidden.elapsed() < RESHOW_WINDOW);
    tips.current = Some(id.clone());
    let delay = show_delay(reshow);
    let task = cx.spawn(async move |cx| {
        cx.background_executor().timer(delay).await;
        cx.update(|cx| {
            if cx.default_global::<Tooltips>().current.as_ref() == Some(&id) {
                show(text, cx);
            }
        });
    });
    cx.default_global::<Tooltips>().pending = Some(task);
}

pub(crate) fn dismiss(cx: &mut App) {
    hide(cx);
}

pub(crate) fn is_popup(handle: AnyWindowHandle, cx: &App) -> bool {
    cx.try_global::<Tooltips>()
        .and_then(|tips| tips.popup.as_ref())
        .is_some_and(|popup| popup.handle == handle)
}

fn system_value(action: u32, fallback: u32) -> u32 {
    let mut value = 0u32;
    let ok = unsafe { SystemParametersInfoW(action, 0, (&mut value as *mut u32).cast(), 0) };
    if ok == 0 { fallback } else { value }
}

fn show_delay(reshow: bool) -> Duration {
    let hover = system_value(SPI_GETMOUSEHOVERTIME, DEFAULT_HOVER_TIME) as f32;
    let factor = if reshow { 1.5 } else { 2. };
    Duration::from_millis((hover * factor) as u64)
}

fn hide(cx: &mut App) {
    let tips = cx.default_global::<Tooltips>();
    tips.pending = None;
    if tips.shown {
        tips.shown = false;
        tips.hidden_at = Some(Instant::now());
        if let Some(popup) = &tips.popup {
            unsafe { ShowWindow(popup.hwnd as HWND, SW_HIDE) };
        }
    }
}

fn popup(cx: &mut App) -> Option<(Entity<TooltipView>, AnyWindowHandle, isize)> {
    if let Some(popup) = &cx.default_global::<Tooltips>().popup {
        return Some((popup.view.clone(), popup.handle, popup.hwnd));
    }
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
            point(px(0.), px(0.)),
            size(px(1.), px(1.)),
        ))),
        titlebar: None,
        focus: false,
        show: false,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        window_background: WindowBackgroundAppearance::Transparent,
        ..Default::default()
    };
    let mut created = None;
    let handle = cx
        .open_window(options, |window, cx| {
            let view = cx.new(|_| TooltipView { text: "".into() });
            created = Some((view.clone(), detach::hwnd(window)));
            view
        })
        .ok()?;
    let (view, hwnd) = created?;
    cx.default_global::<Tooltips>().popup = Some(Popup {
        view: view.clone(),
        handle: handle.into(),
        hwnd,
    });
    Some((view, handle.into(), hwnd))
}

fn measure(text: &SharedString, window: &mut Window) -> (f32, f32) {
    let font = gpui_kit::font(UI_FONT);
    let runs = [TextRun {
        len: text.len(),
        font,
        color: gpui_kit::black(),
        background_color: None,
        underline: None,
        strikethrough: None,
    }];
    let wrap = MAX_WIDTH - 2. * PADDING_X;
    let Ok(lines) =
        window
            .text_system()
            .shape_text(text.clone(), px(FONT_SIZE), &runs, Some(px(wrap)), None)
    else {
        return (MAX_WIDTH, PADDING_TOP + LINE_HEIGHT + PADDING_BOTTOM);
    };
    let count: usize = lines
        .iter()
        .map(|line| line.wrap_boundaries().len() + 1)
        .sum();
    let widest = lines
        .iter()
        .map(|line| {
            if line.wrap_boundaries().is_empty() {
                line.width().as_f32()
            } else {
                wrap
            }
        })
        .fold(0., f32::max);
    (
        (widest.ceil() + 2. * PADDING_X).min(MAX_WIDTH),
        PADDING_TOP + count as f32 * LINE_HEIGHT + PADDING_BOTTOM,
    )
}

fn show(text: SharedString, cx: &mut App) {
    let Some((view, handle, hwnd)) = popup(cx) else {
        return;
    };
    let mut cursor = POINT { x: 0, y: 0 };
    unsafe { GetCursorPos(&mut cursor) };
    let monitor = unsafe { MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST) };
    let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
    info.cbSize = size_of::<MONITORINFO>() as u32;
    unsafe { GetMonitorInfoW(monitor, &mut info) };
    let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
    unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) };
    let scale = dpi_x as f32 / 96.;
    let work: RECT = info.rcWork;
    unsafe {
        SetWindowPos(
            hwnd as HWND,
            HWND_TOPMOST,
            cursor.x,
            cursor.y,
            1,
            1,
            SWP_NOACTIVATE,
        )
    };
    let Ok((width, height)) = handle.update(cx, |_, window, cx| {
        let measured = measure(&text, window);
        view.update(cx, |view, cx| {
            view.text = text.clone();
            cx.notify();
        });
        measured
    }) else {
        return;
    };
    let physical = |v: f32| (v * scale).round() as i32;
    let (w, h) = (physical(width), physical(height));
    let gap = physical(POINTER_OFFSET);
    let left = (cursor.x - w / 2).clamp(work.left, (work.right - w).max(work.left));
    let mut top = cursor.y - gap - h;
    if top < work.top {
        top = cursor.y + gap;
    }
    let margin = physical(MARGIN);
    unsafe {
        SetWindowPos(
            hwnd as HWND,
            HWND_TOPMOST,
            left - margin,
            top - margin,
            w + 2 * margin,
            h + 2 * margin,
            SWP_NOACTIVATE,
        );
        ShowWindow(hwnd as HWND, SW_SHOWNOACTIVATE);
    }
    let tips = cx.default_global::<Tooltips>();
    tips.shown = true;
    let seconds = system_value(SPI_GETMESSAGEDURATION, DEFAULT_SHOW_SECONDS);
    let current = tips.current.clone();
    let task = cx.spawn(async move |cx| {
        cx.background_executor()
            .timer(Duration::from_secs(seconds.into()))
            .await;
        cx.update(|cx| {
            if cx.default_global::<Tooltips>().current == current {
                hide(cx);
            }
        });
    });
    cx.default_global::<Tooltips>().pending = Some(task);
}
