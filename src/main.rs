#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod brush;
mod dialog;
mod document;
mod field;
mod files;
mod find;
mod keytips;
mod measure;
mod menu;
mod overlay;
mod palette;
mod scrollbar;
mod tab_drag;
mod tabs;
mod tooltip;

use brush::{BrushTransitionExt as _, Brushes};
use gpui_kit::Edges;
use gpui_kit::base::Easing;
use gpui_kit::base::animation::cubic_bezier;
use gpui_kit::component::input::{
    self, InputEvent, PrepaintCaretGeometry, Rope, RopeExt as _, Textarea, TextareaState,
};
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use menu::MenuBar;
use palette::{
    NOISE_OPACITY, NOISE_SIZE, Palette, Surface, accent_color, acrylic_noise, palette, theme_shadow,
};
use std::cell::Cell;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;
use windows_sys::Win32::Foundation::{HWND, POINT, SYSTEMTIME};
use windows_sys::Win32::Globalization::{
    DATE_SHORTDATE, GetDateFormatEx, GetTimeFormatEx, TIME_NOSECONDS,
};
use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
use windows_sys::Win32::System::SystemInformation::GetLocalTime;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetCaretBlinkTime, GetCursorPos, SystemParametersInfoW, USER_DEFAULT_SCREEN_DPI,
};

const APP_NAME: &str = "Jot";
const NOTICES_FLAG: &str = "--notices";
const CONTEXT: &str = "Jot";

const WINDOW_WIDTH: f32 = 2213.;
const WINDOW_HEIGHT: f32 = 1107.;
const WINDOW_MAX_SCREEN_SHARE: f32 = 0.9;

const SMALL_FONT: &str = "Segoe UI Variable Small";
const TEXT_FONT: &str = "Segoe UI Variable Text";
const ICON_FONT: &str = "Segoe Fluent Icons";

const DECELERATE: Easing = Easing::CubicBezier {
    x1: 0.,
    y1: 0.,
    x2: 0.,
    y2: 1.,
};

const CAPTION_BUTTON_WIDTH: f32 = 46.;
const CLOSE_HOVER: u32 = 0xc42b1cff;
const CLOSE_PRESSED: u32 = 0xc42b1ce6;
const CLOSE_GLYPH_HOVER: u32 = 0xffffffff;
const CLOSE_GLYPH_PRESSED: u32 = 0xffffffb3;

const SEPARATOR_HEIGHT: f32 = 1.;
const MENU_BAR_HEIGHT: f32 = 32.;
const MENU_BAR_LEFT: f32 = 4.;
const STATUS_BAR_HEIGHT: f32 = 31.;
const STATUS_DIVIDER_WIDTH: f32 = 1.;
const STATUS_DIVIDER_HEIGHT: f32 = 16.;
const THEME_BUTTON_WIDTH: f32 = 30.;
const THEME_BUTTON_RIGHT: f32 = 8.;
const THEME_GLYPH_SIZE: f32 = 16.;
const MOON_GLYPH: &str = "\u{E708}";
const SUN_GLYPH: &str = "\u{E706}";

const EDITOR_FONT_SIZE: f32 = 11. * 96. / 72.;
const EDITOR_LINE_HEIGHT: f32 = 20.;
const EDITOR_PADDING_LEFT: f32 = 6.;
const EDITOR_PADDING_TOP: f32 = 7.;
const CARET_WIDTH: f32 = 1.;
const CARET_HEIGHT: f32 = 18.;
const ZOOM_STEP: f32 = 0.1;
const ZOOM_MIN: f32 = 0.1;
const ZOOM_MAX: f32 = 5.;

type CaretKey = (usize, Range<usize>, usize);

fn separator(p: Palette) -> Div {
    div()
        .flex_none()
        .h(px(SEPARATOR_HEIGHT))
        .bg(rgba(p.stroke_on_layer))
}

fn group_thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn character_count(selected: usize, total: usize) -> String {
    let noun = if total == 1 {
        "character"
    } else {
        "characters"
    };
    if selected > 0 {
        format!(
            "{} of {} {noun}",
            group_thousands(selected),
            group_thousands(total)
        )
    } else {
        format!("{} {noun}", group_thousands(total))
    }
}

#[derive(Clone, Copy)]
enum Caption {
    Minimize,
    Maximize,
    Restore,
    Close,
}

impl Caption {
    fn id(self) -> &'static str {
        match self {
            Self::Minimize => "caption-minimize",
            Self::Maximize => "caption-maximize",
            Self::Restore => "caption-restore",
            Self::Close => "caption-close",
        }
    }

    fn glyph(self) -> &'static str {
        match self {
            Self::Minimize => "\u{E921}",
            Self::Maximize => "\u{E922}",
            Self::Restore => "\u{E923}",
            Self::Close => "\u{E8BB}",
        }
    }

    fn area(self) -> WindowControlArea {
        match self {
            Self::Minimize => WindowControlArea::Min,
            Self::Maximize | Self::Restore => WindowControlArea::Max,
            Self::Close => WindowControlArea::Close,
        }
    }

    fn render(self, active: bool, p: Palette) -> impl IntoElement {
        let (hover_bg, pressed_bg, hover_fg, pressed_fg) = match self {
            Self::Close => (
                CLOSE_HOVER,
                CLOSE_PRESSED,
                CLOSE_GLYPH_HOVER,
                CLOSE_GLYPH_PRESSED,
            ),
            _ => (
                p.layer_hover,
                p.subtle_tertiary,
                p.caption_glyph,
                p.caption_glyph_pressed,
            ),
        };
        let fg = if active {
            p.caption_glyph
        } else {
            p.caption_glyph_inactive
        };

        div()
            .id(self.id())
            .flex()
            .flex_none()
            .w(px(CAPTION_BUTTON_WIDTH))
            .h(px(30.))
            .items_center()
            .justify_center()
            .font_family(ICON_FONT)
            .text_size(px(10.))
            .text_color(rgba(fg))
            .window_control_area(self.area())
            .hover(move |style| style.bg(rgba(hover_bg)).text_color(rgba(hover_fg)))
            .active(move |style| style.bg(rgba(pressed_bg)).text_color(rgba(pressed_fg)))
            .child(
                div()
                    .relative()
                    .when(matches!(self, Self::Minimize), |this| {
                        this.left(px(1.)).top(px(1.))
                    })
                    .child(self.glyph()),
            )
    }
}

fn caption_buttons(window: &Window, p: Palette) -> impl IntoElement {
    let active = window.is_window_active();
    let maximize = if window.is_maximized() {
        Caption::Restore
    } else {
        Caption::Maximize
    };

    div()
        .v_flex()
        .flex_none()
        .h_full()
        .child(
            div()
                .flex()
                .child(Caption::Minimize.render(active, p))
                .child(maximize.render(active, p))
                .child(Caption::Close.render(active, p)),
        )
        .child(div().flex_1().window_control_area(WindowControlArea::Drag))
}

gpui_kit::actions!(
    jot,
    [
        NewTab,
        NewWindow,
        Open,
        Save,
        SaveAs,
        SaveAll,
        CloseTab,
        CloseWindow,
        Exit,
        Find,
        FindNext,
        FindPrevious,
        Replace,
        GoTo,
        TimeDate,
        ZoomIn,
        ZoomOut,
        ResetZoom,
        ToggleStatusBar,
        ToggleWordWrap,
        ToggleMatchCase,
        ToggleWrapAround,
        ReplaceNext,
        ShowContextMenu,
        ToggleTheme,
    ]
);

struct ViewSettings {
    status_bar: bool,
    word_wrap: bool,
}

impl Global for ViewSettings {}

fn key_bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("ctrl-n", NewTab, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-n", NewWindow, Some(CONTEXT)),
        KeyBinding::new("ctrl-o", Open, Some(CONTEXT)),
        KeyBinding::new("ctrl-s", Save, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-s", SaveAs, Some(CONTEXT)),
        KeyBinding::new("ctrl-alt-s", SaveAll, Some(CONTEXT)),
        KeyBinding::new("ctrl-w", CloseTab, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-w", CloseWindow, Some(CONTEXT)),
        KeyBinding::new("ctrl-f", Find, Some(CONTEXT)),
        KeyBinding::new("f3", FindNext, Some(CONTEXT)),
        KeyBinding::new("shift-f3", FindPrevious, Some(CONTEXT)),
        KeyBinding::new("ctrl-h", Replace, Some(CONTEXT)),
        KeyBinding::new("ctrl-g", GoTo, Some(CONTEXT)),
        KeyBinding::new("f5", TimeDate, Some(CONTEXT)),
        KeyBinding::new("shift-f10", ShowContextMenu, Some(CONTEXT)),
        KeyBinding::new("menu", ShowContextMenu, Some(CONTEXT)),
        KeyBinding::new("ctrl-=", ZoomIn, Some(CONTEXT)),
        KeyBinding::new("ctrl--", ZoomOut, Some(CONTEXT)),
        KeyBinding::new("ctrl-0", ResetZoom, Some(CONTEXT)),
    ]
}

fn window_options(window_size: Option<gpui_kit::Size<Pixels>>, cx: &App) -> WindowOptions {
    let screen = cx
        .primary_display()
        .map_or(size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)), |display| {
            display.visible_bounds().size
        });
    let window_size = window_size.unwrap_or(size(
        px(WINDOW_WIDTH).min(screen.width * WINDOW_MAX_SCREEN_SHARE),
        px(WINDOW_HEIGHT).min(screen.height * WINDOW_MAX_SCREEN_SHARE),
    ));

    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            window_size,
            cx,
        ))),
        titlebar: Some(TitlebarOptions {
            title: Some(APP_NAME.into()),
            appears_transparent: true,
            traffic_light_position: None,
        }),
        window_background: WindowBackgroundAppearance::MicaAltBackdrop,
        ..Default::default()
    }
}

fn open_jot_window(
    initial: Option<TabData>,
    window_size: Option<gpui_kit::Size<Pixels>>,
    cx: &mut App,
) -> (AnyWindowHandle, Entity<Jot>) {
    gpui_kit::open_window(window_options(window_size, cx), cx, |window, cx| {
        cx.new(|cx| Jot::new(initial, window, cx))
    })
    .expect("Failed to open window")
}

fn caret_blink_period() -> Option<Duration> {
    let millis = unsafe { GetCaretBlinkTime() };
    (millis != 0 && millis != u32::MAX).then(|| Duration::from_millis(millis.into()))
}

fn round_half_up(value: f32) -> f32 {
    (value + 0.5).floor()
}

fn window_hwnd(window: &Window) -> isize {
    match raw_window_handle::HasWindowHandle::window_handle(window).map(|handle| handle.as_raw()) {
        Ok(raw_window_handle::RawWindowHandle::Win32(handle)) => handle.hwnd.get(),
        _ => 0,
    }
}

fn cursor() -> POINT {
    let mut point = POINT::default();
    unsafe { GetCursorPos(&mut point) };
    point
}

fn client_origin(hwnd: isize) -> POINT {
    let mut origin = POINT::default();
    unsafe { ClientToScreen(hwnd as HWND, &mut origin) };
    origin
}

fn system_u32(action: u32) -> Option<u32> {
    let mut value = 0u32;
    (unsafe { SystemParametersInfoW(action, 0, (&raw mut value).cast(), 0) } != 0).then_some(value)
}

fn time_date() -> String {
    let mut now = SYSTEMTIME::default();
    let mut time = [0u16; 128];
    let mut date = [0u16; 128];
    let (time_len, date_len) = unsafe {
        GetLocalTime(&mut now);
        (
            GetTimeFormatEx(
                std::ptr::null(),
                TIME_NOSECONDS,
                &now,
                std::ptr::null(),
                time.as_mut_ptr(),
                time.len() as i32,
            ),
            GetDateFormatEx(
                std::ptr::null(),
                DATE_SHORTDATE,
                &now,
                std::ptr::null(),
                date.as_mut_ptr(),
                date.len() as i32,
                std::ptr::null(),
            ),
        )
    };
    let text =
        |buffer: &[u16], len: i32| String::from_utf16_lossy(&buffer[..(len.max(1) - 1) as usize]);
    format!("{} {}", text(&time, time_len), text(&date, date_len))
}

fn caret_canvas(
    geometry: impl Fn(&App) -> Option<PrepaintCaretGeometry> + 'static,
    height: Pixels,
    shown: bool,
) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |_, _, window, cx| {
            if !shown {
                return;
            }
            let Some(geometry) = geometry(cx) else {
                return;
            };
            let caret = Bounds::new(
                caret_origin(&geometry, window),
                size(px(CARET_WIDTH), height),
            );
            window.with_content_mask(
                Some(ContentMask {
                    bounds: geometry.input_bounds(),
                }),
                |window| window.paint_quad(fill(caret, rgb(palette(cx).caret))),
            );
        },
    )
    .absolute()
    .size_full()
}

pub(crate) fn text_width(window: &Window, text: &str, family: &'static str, size: f32) -> f32 {
    let run = TextRun {
        len: text.len(),
        font: font(family),
        color: gpui_kit::black(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    window
        .text_system()
        .shape_line(SharedString::new(text), px(size), &[run], None)
        .width
        .as_f32()
        .ceil()
}

fn caret_origin(geometry: &PrepaintCaretGeometry, window: &Window) -> Point<Pixels> {
    let origin = geometry.cursor_bounds().origin;
    window.pixel_snap_point(point(origin.x, origin.y + geometry.scroll_offset().y))
}

fn caret_key(editor: &TextareaState) -> CaretKey {
    (
        editor.cursor(),
        editor.selected_range(),
        editor.text().len(),
    )
}

struct TabData {
    editor: Entity<TextareaState>,
    zoom: f32,
    path: Option<PathBuf>,
    format: document::Format,
    dirty: bool,
}

struct Tab {
    editor: Entity<TextareaState>,
    zoom: f32,
    path: Option<PathBuf>,
    format: document::Format,
    dirty: bool,
    slot: tabs::Slot,
    _observer: Subscription,
    _changes: Subscription,
}

impl Tab {
    fn into_data(self) -> TabData {
        TabData {
            editor: self.editor,
            zoom: self.zoom,
            path: self.path,
            format: self.format,
            dirty: self.dirty,
        }
    }
}

struct Jot {
    tabs: Vec<Tab>,
    active: usize,
    hovered_tab: Option<EntityId>,
    strip: tabs::TabStrip,
    scroll: scrollbar::ScrollChrome,
    hwnd: isize,
    close_confirmed: bool,
    menu_bar: Entity<MenuBar>,
    dialogs: dialog::Dialogs,
    find: find::FindBar,
    field_caret: Option<(EntityId, usize, Range<usize>)>,
    caret_visible: bool,
    caret_key: CaretKey,
    _caret_blink: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl Jot {
    fn new(initial: Option<TabData>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let tab = match initial {
            Some(data) => Self::adopt_tab(data, window, cx),
            None => Self::create_tab(None, window, cx),
        };
        let editor = tab.editor.clone();
        let menu_bar = cx.new(|cx| MenuBar::new(editor, cx));
        let menu_events = cx.subscribe(&menu_bar, |this, menu_bar, event, cx| match event {
            menu::MenuEvent::Opened => {
                let focus = menu_bar.read(cx).focus_handle().clone();
                this.active_editor()
                    .update(cx, |editor, cx| editor.set_selection_focus(Some(focus), cx));
            }
            menu::MenuEvent::Closed => this.sync_find_selection(cx),
        });
        let dialogs = dialog::Dialogs::new(window, cx);
        let find = find::FindBar::new(window, cx);

        let access_keys = {
            let menu_bar = menu_bar.downgrade();
            let handle = window.window_handle();
            cx.intercept_keystrokes(move |event, window, cx| {
                if window.window_handle() != handle {
                    return;
                }
                if menu_bar
                    .update(cx, |menu_bar, cx| {
                        menu_bar.key_down(&event.keystroke, window, cx)
                    })
                    .unwrap_or(false)
                {
                    cx.stop_propagation();
                }
            })
        };
        let _subscriptions = vec![
            menu_events,
            access_keys,
            cx.observe_window_appearance(window, |_, window, cx| {
                palette::apply(window.appearance(), cx);
                cx.notify();
            }),
            cx.observe_window_activation(window, |this, window, cx| {
                if window.is_window_active() {
                    this.restart_caret_blink(cx);
                } else {
                    this._caret_blink = Task::ready(());
                    this.menu_bar
                        .update(cx, |menu_bar, cx| menu_bar.exit_access(cx));
                    cx.notify();
                }
            }),
            cx.observe_window_bounds(window, |this, _, cx| {
                this.menu_bar
                    .update(cx, |menu_bar, cx| menu_bar.exit_access(cx));
            }),
            cx.observe_global_in::<ViewSettings>(window, |this, window, cx| {
                let word_wrap = cx.global::<ViewSettings>().word_wrap;
                for tab in &this.tabs {
                    tab.editor
                        .update(cx, |editor, cx| editor.set_soft_wrap(word_wrap, window, cx));
                }
                cx.notify();
            }),
        ];

        let mut jot = Self {
            tabs: vec![tab],
            active: 0,
            hovered_tab: None,
            strip: tabs::TabStrip::new(),
            scroll: scrollbar::ScrollChrome::new(),
            hwnd: window_hwnd(window),
            close_confirmed: false,
            menu_bar,
            dialogs,
            find,
            field_caret: None,
            caret_visible: true,
            caret_key: (0, 0..0, 0),
            _caret_blink: Task::ready(()),
            _subscriptions,
        };
        jot.activate_tab(0, window, cx);
        let this = cx.weak_entity();
        window.on_window_should_close(cx, move |window, cx| {
            this.update(cx, |jot, cx| jot.request_close_window(window, cx))
                .unwrap_or(true)
        });
        jot
    }

    fn active_editor(&self) -> &Entity<TextareaState> {
        &self.tabs[self.active].editor
    }

    fn has_text(&self, cx: &App) -> bool {
        self.active_editor().read(cx).text().len() > 0
    }

    fn zoom(&self) -> f32 {
        self.tabs[self.active].zoom
    }

    fn line_height(&self) -> f32 {
        (EDITOR_LINE_HEIGHT * self.zoom()).round()
    }

    fn editor_min_height(&self) -> f32 {
        let inset = EDITOR_PADDING_TOP + gpui_kit::component::Size::default().input_py().as_f32();
        2. * inset + self.line_height()
    }

    fn min_size_probe(&self, status_bar: bool, cx: &App) -> impl IntoElement + use<> {
        let hwnd = self.hwnd;
        let menu_bar = self.menu_bar.clone();
        let status = status_bar.then(|| self.status_bar(cx).into_any_element());
        let rows = tabs::STRIP_TOP
            + tabs::TAB_HEIGHT
            + MENU_BAR_HEIGHT
            + SEPARATOR_HEIGHT
            + self.editor_min_height();
        canvas(
            move |_, window, cx| {
                let status = status.map(|mut status| {
                    status.layout_as_root(AvailableSpace::min_size(), window, cx)
                });
                let menu_row = MENU_BAR_LEFT
                    + menu_bar.read(cx).width()
                    + THEME_BUTTON_WIDTH
                    + THEME_BUTTON_RIGHT;
                let width = status
                    .map_or(0., |status| status.width.as_f32())
                    .max(menu_row)
                    .max(tabs::TITLE_BAR_MIN_WIDTH);
                let height = rows + status.map_or(0., |status| status.height.as_f32());
                gpui_windows::set_window_min_size(hwnd, Some(size(px(width), px(height))));
            },
            |_, _, _, _| {},
        )
        .absolute()
    }

    fn caret_height(&self) -> Pixels {
        px((CARET_HEIGHT * self.zoom()).round())
    }

    fn activate_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.active = ix;
        self.reveal_tab(ix, cx);
        let editor = self.active_editor().clone();
        editor.focus_handle(cx).focus(window, cx);
        self.menu_bar
            .update(cx, |menu_bar, _| menu_bar.editor = editor.clone());
        self.caret_key = caret_key(editor.read(cx));
        self.sync_find_selection(cx);
        self.restart_caret_blink(cx);
        self.refresh_title(window);
    }

    fn new_tab(&mut self, _: &NewTab, window: &mut Window, cx: &mut Context<Self>) {
        self.add_tab(window, cx);
    }

    fn restart_caret_blink(&mut self, cx: &mut Context<Self>) {
        self.caret_visible = true;
        cx.notify();
        let Some(period) = caret_blink_period() else {
            self._caret_blink = Task::ready(());
            return;
        };
        self._caret_blink = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(period).await;
                let toggled = this.update(cx, |this, cx| {
                    this.caret_visible = !this.caret_visible;
                    cx.notify();
                });
                if toggled.is_err() {
                    break;
                }
            }
        });
    }

    fn caret(&self, window: &Window, cx: &App) -> impl IntoElement {
        let editor = self.active_editor().clone();
        let height = self.caret_height();
        let shown = self.caret_visible
            && !self.menu_bar.read(cx).access_active()
            && window.is_window_active()
            && editor.focus_handle(cx).is_focused(window);

        caret_canvas(
            move |cx| {
                let editor = editor.read(cx);
                editor
                    .selected_range()
                    .is_empty()
                    .then(|| editor.prepaint_caret_geometry())
                    .flatten()
            },
            height,
            shown,
        )
    }

    fn insert_time_date(&mut self, _: &TimeDate, window: &mut Window, cx: &mut Context<Self>) {
        let stamp = time_date();
        self.active_editor()
            .update(cx, |editor, cx| editor.replace(stamp, window, cx));
    }

    fn zoom_in(&mut self, _: &ZoomIn, _: &mut Window, cx: &mut Context<Self>) {
        self.set_zoom(self.zoom() + ZOOM_STEP, cx);
    }

    fn zoom_out(&mut self, _: &ZoomOut, _: &mut Window, cx: &mut Context<Self>) {
        self.set_zoom(self.zoom() - ZOOM_STEP, cx);
    }

    fn reset_zoom(&mut self, _: &ResetZoom, _: &mut Window, cx: &mut Context<Self>) {
        self.set_zoom(1., cx);
    }

    fn set_zoom(&mut self, zoom: f32, cx: &mut Context<Self>) {
        self.tabs[self.active].zoom =
            ((zoom / ZOOM_STEP).round() * ZOOM_STEP).clamp(ZOOM_MIN, ZOOM_MAX);
        cx.notify();
    }

    fn close_window(&mut self, _: &CloseWindow, window: &mut Window, cx: &mut Context<Self>) {
        if self.request_close_window(window, cx) {
            window.remove_window();
        }
    }

    fn close_active_tab(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        self.close_tab(self.active, window, cx);
    }

    fn title_bar(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);

        div()
            .flex()
            .flex_none()
            .h(px(tabs::STRIP_TOP + tabs::TAB_HEIGHT))
            .child(
                div()
                    .flex_none()
                    .w(px(tabs::HEADER_WIDTH))
                    .h_full()
                    .window_control_area(WindowControlArea::Drag),
            )
            .child(
                div()
                    .v_flex()
                    .flex_none()
                    .child(
                        div()
                            .h(px(tabs::STRIP_TOP))
                            .window_control_area(WindowControlArea::Drag),
                    )
                    .child(self.tab_strip(cx)),
            )
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .window_control_area(WindowControlArea::Drag),
            )
            .child(caption_buttons(window, p))
    }

    fn render_menu_bar(&self, p: Palette) -> impl IntoElement {
        div()
            .flex_none()
            .child(
                div()
                    .flex()
                    .h(px(MENU_BAR_HEIGHT))
                    .pl(px(MENU_BAR_LEFT))
                    .pr(px(THEME_BUTTON_RIGHT))
                    .bg(rgba(p.layer))
                    .child(self.menu_bar.clone())
                    .child(div().flex_1())
                    .child(tooltip::owner(
                        "theme",
                        "Switch theme",
                        field::icon_button(
                            p,
                            "theme",
                            if p.dark { SUN_GLYPH } else { MOON_GLYPH },
                            px(THEME_GLYPH_SIZE),
                            size(px(THEME_BUTTON_WIDTH), px(MENU_BAR_HEIGHT)),
                            px(field::CONTROL_RADIUS),
                        )
                        .flex_none()
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(ToggleTheme), cx);
                        }),
                    )),
            )
            .child(separator(p))
    }

    fn show_context_menu(
        &mut self,
        _: &ShowContextMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let editor = self.active_editor().clone();
        if !editor.focus_handle(cx).is_focused(window) {
            cx.propagate();
            return;
        }
        let Some(geometry) = editor.read(cx).prepaint_caret_geometry() else {
            return;
        };
        let caret = caret_origin(&geometry, window);
        let anchor = point(caret.x + px(CARET_WIDTH), caret.y + self.caret_height());
        self.open_context_menu(anchor, window, cx);
    }

    fn open_context_menu(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.menu_bar.update(cx, |menu_bar, cx| {
            menu_bar.open_context_menu(position, window, cx);
        });
    }

    fn editor(
        &self,
        find: Option<impl IntoElement>,
        scrollbars: impl IntoElement,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let p = palette(cx);
        let this = cx.weak_entity();
        div()
            .id("editor-area")
            .relative()
            .flex_1()
            .min_h(px(self.editor_min_height()))
            .pl(px(EDITOR_PADDING_LEFT))
            .pt(px(EDITOR_PADDING_TOP))
            .bg(rgb(p.editor))
            .on_mouse_move(cx.listener(|this, _, _, cx| this.scroll_activity(cx)))
            .on_hover(cx.listener(|this, _, _, cx| this.scroll_activity(cx)))
            .on_prepaint({
                let area = self.scroll.area.clone();
                move |bounds, _, _| area.set(bounds)
            })
            .child(
                div()
                    .relative()
                    .size_full()
                    .overflow_hidden()
                    .child(
                        Textarea::new(self.active_editor())
                            .context_menu(move |menu, window, cx| {
                                let position = window.mouse_position();
                                this.update(cx, |jot, cx| {
                                    jot.open_context_menu(position, window, cx)
                                })
                                .ok();
                                menu
                            })
                            .appearance(false)
                            .h_full()
                            .font_family("Consolas")
                            .text_size(px(EDITOR_FONT_SIZE * self.zoom()))
                            .line_height(px(self.line_height())),
                    )
                    .child(self.caret(window, cx)),
            )
            .child(scrollbars)
            .children(find)
    }

    fn status_bar(&self, cx: &App) -> impl IntoElement {
        let p = palette(cx);
        let editor = self.active_editor().read(cx);
        let text = editor.text();
        let cursor = editor.cursor();
        let point = text.offset_to_point(cursor);
        let column = text.byte_to_char_idx(cursor) - text.byte_to_char_idx(cursor - point.column);
        let selection = editor.selected_range();
        let selected =
            text.byte_to_char_idx(selection.end) - text.byte_to_char_idx(selection.start);
        let total = text.len_chars();

        div().flex_none().child(separator(p)).child(
            div()
                .flex()
                .items_center()
                .gap(px(24.))
                .h(px(STATUS_BAR_HEIGHT))
                .px(px(22.))
                .bg(rgba(p.layer))
                .font_family(SMALL_FONT)
                .text_size(px(12.))
                .text_color(rgba(p.text_secondary))
                .child(format!("Ln {}, Col {}", point.row + 1, column + 1))
                .child(status_divider(p))
                .child(character_count(selected, total))
                .child(status_spacer(p))
                .child(format!("{}%", (self.zoom() * 100.).round()))
                .child(status_divider(p))
                .child(self.tabs[self.active].format.line_ending.label())
                .child(status_divider(p))
                .child(self.tabs[self.active].format.encoding.label()),
        )
    }
}

fn status_divider(p: Palette) -> Div {
    div()
        .flex_none()
        .w(px(STATUS_DIVIDER_WIDTH))
        .h(px(STATUS_DIVIDER_HEIGHT))
        .bg(rgba(p.stroke))
}

fn status_spacer(p: Palette) -> Canvas<()> {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let width = bounds.size.width.as_f32();
            if width < STATUS_DIVIDER_WIDTH + 0.5 {
                let x = bounds.origin.x + px(((width - STATUS_DIVIDER_WIDTH) / 2.).round());
                let line = Bounds::new(
                    point(x, bounds.origin.y),
                    size(px(STATUS_DIVIDER_WIDTH), bounds.size.height),
                );
                window.paint_quad(fill(line, rgba(p.stroke)));
            }
        },
    )
    .flex_1()
    .min_w(px(STATUS_DIVIDER_WIDTH))
    .h(px(STATUS_DIVIDER_HEIGHT))
}

impl Render for Jot {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let status_bar = cx.global::<ViewSettings>().status_bar;
        self.sample_tabs(window);
        self.sample_scrollbars(cx);
        let scrollbars = self.scrollbars(window, cx);
        let dialogs = self.render_dialogs(window, cx);
        let min_size = self.min_size_probe(status_bar, cx);
        let find = self.render_find(window, cx);

        let root = div()
            .relative()
            .size_full()
            .on_mouse_move(|_, _, cx| tooltip::pointer_moved(cx))
            .on_modifiers_changed(|event, _, cx| tooltip::modifiers_changed(&event.modifiers, cx))
            .capture_key_down(|event: &KeyDownEvent, _, cx| {
                tooltip::key_down(cx);
                if event.keystroke.key == "escape" && tab_drag::dragging(cx) {
                    cx.stop_propagation();
                    cx.defer(tab_drag::cancel);
                }
            })
            .capture_any_mouse_down(cx.listener(|this, _, _, cx| {
                this.menu_bar
                    .update(cx, |menu_bar, cx| menu_bar.pointer_down(cx));
            }))
            .on_drop(cx.listener(Self::open_dropped))
            .on_action(cx.listener(Self::show_context_menu))
            .on_action(cx.listener(Self::zoom_in))
            .on_action(cx.listener(Self::zoom_out))
            .on_action(cx.listener(Self::reset_zoom))
            .on_action(cx.listener(Self::close_window))
            .on_action(cx.listener(Self::new_tab))
            .on_action(cx.listener(Self::close_active_tab))
            .on_action(cx.listener(Self::open))
            .on_action(cx.listener(Self::save))
            .on_action(cx.listener(Self::save_as))
            .on_action(cx.listener(Self::save_all))
            .on_action(cx.listener(Self::insert_time_date))
            .on_action(cx.listener(Self::go_to))
            .on_action(cx.listener(Self::find))
            .on_action(cx.listener(Self::replace))
            .on_action(cx.listener(Self::find_next))
            .on_action(cx.listener(Self::find_previous))
            .on_action(cx.listener(Self::toggle_match_case))
            .on_action(cx.listener(Self::toggle_wrap_around))
            .on_action(cx.listener(Self::replace_next))
            .child(
                div()
                    .key_context(CONTEXT)
                    .v_flex()
                    .size_full()
                    .child(self.title_bar(window, cx))
                    .child(self.render_menu_bar(p))
                    .child(self.editor(find, scrollbars, window, cx))
                    .when(status_bar, |this| this.child(self.status_bar(cx))),
            )
            .children(dialogs)
            .child(min_size);
        tooltip::host(root)
    }
}

fn main() {
    gpui_kit::application().run(|cx| {
        gpui_kit::init(cx);

        cx.on_window_closed(|cx, window| {
            tooltip::window_closed(window, cx);
            if cx
                .windows()
                .into_iter()
                .all(|window| overlay::is_overlay(window, cx))
            {
                cx.quit();
            }
        })
        .detach();

        palette::apply(cx.window_appearance(), cx);
        cx.set_text_rendering_mode(TextRenderingMode::Grayscale);

        cx.set_global(ViewSettings {
            status_bar: true,
            word_wrap: true,
        });
        cx.bind_keys(key_bindings());
        cx.bind_keys(menu::key_bindings());
        cx.bind_keys(dialog::key_bindings());
        cx.bind_keys(find::key_bindings());

        cx.on_action(|_: &Exit, cx| {
            for window in cx.windows() {
                window
                    .update(cx, |_, window, cx| {
                        window.dispatch_action(Box::new(CloseWindow), cx);
                    })
                    .ok();
            }
        });
        cx.on_action(|_: &NewWindow, cx| {
            open_jot_window(None, None, cx);
        });
        cx.on_action(|_: &ToggleStatusBar, cx| {
            cx.update_global::<ViewSettings, _>(|view, _| view.status_bar = !view.status_bar);
        });
        cx.on_action(|_: &ToggleTheme, cx| {
            cx.set_window_appearance(Some(if palette(cx).dark {
                WindowAppearance::Light
            } else {
                WindowAppearance::Dark
            }));
        });
        cx.on_action(|_: &ToggleWordWrap, cx| {
            cx.update_global::<ViewSettings, _>(|view, _| view.word_wrap = !view.word_wrap);
        });

        let (window, jot) = open_jot_window(None, None, cx);
        tooltip::prepare(cx);
        let (flags, mut paths): (Vec<_>, Vec<_>) = std::env::args_os()
            .skip(1)
            .map(PathBuf::from)
            .partition(|arg| arg.as_os_str() == NOTICES_FLAG);
        if !flags.is_empty() {
            paths.extend(document::write_notices().ok());
        }
        if !paths.is_empty() {
            window
                .update(cx, |_, window, cx| {
                    jot.update(cx, |jot, cx| {
                        for path in paths {
                            jot.open_path(path, window, cx);
                        }
                    })
                })
                .ok();
        }
    });
}
