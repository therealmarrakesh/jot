#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(feature = "bench")]
mod bench;
mod document;
mod files;
mod menu;

use gpui_kit::base::animation::cubic_bezier;
use gpui_kit::component::input::{self, InputEvent, RopeExt as _, Textarea, TextareaState};
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use menu::MenuBar;
use std::cell::Cell;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;
use windows_sys::Win32::Foundation::SYSTEMTIME;
use windows_sys::Win32::Globalization::{
    DATE_SHORTDATE, GetDateFormatEx, GetTimeFormatEx, TIME_NOSECONDS,
};
use windows_sys::Win32::System::SystemInformation::GetLocalTime;
use windows_sys::Win32::UI::WindowsAndMessaging::GetCaretBlinkTime;

const LAYER_ON_MICA: u32 = 0xffffffb3;
const STROKE: u32 = 0x0000000f;
const STROKE_ON_LAYER: u32 = 0xeaeaeab8;
const EDITOR_BACKGROUND: u32 = 0xf9f9f9;
const TEXT_PRIMARY: u32 = 0x000000bc;
const TEXT_SECONDARY: u32 = 0x0000007a;
const UI_FONT: &str = "Segoe UI Variable Small";
const MENU_FONT: &str = "Segoe UI Variable Text";
const SUBTLE_FILL_SECONDARY: u32 = 0x00000005;
const SUBTLE_FILL_TERTIARY: u32 = 0x00000003;

const CAPTION_GLYPH: u32 = 0x000000ff;
const CAPTION_GLYPH_INACTIVE: u32 = 0x0000005c;
const CAPTION_GLYPH_PRESSED: u32 = 0x0000009e;
const CAPTION_HOVER: u32 = 0x0000000a;
const CAPTION_PRESSED: u32 = 0x00000006;
const CLOSE_HOVER: u32 = 0xc42b1cff;
const CLOSE_PRESSED: u32 = 0xc42b1ce6;
const CLOSE_GLYPH_HOVER: u32 = 0xffffffff;
const CLOSE_GLYPH_PRESSED: u32 = 0xffffffb3;

fn separator() -> Div {
    div().flex_none().h(px(1.)).bg(rgba(STROKE_ON_LAYER))
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

    fn render(self, active: bool) -> impl IntoElement {
        let (hover_bg, pressed_bg, hover_fg, pressed_fg) = match self {
            Self::Close => (
                CLOSE_HOVER,
                CLOSE_PRESSED,
                CLOSE_GLYPH_HOVER,
                CLOSE_GLYPH_PRESSED,
            ),
            _ => (
                CAPTION_HOVER,
                CAPTION_PRESSED,
                CAPTION_GLYPH,
                CAPTION_GLYPH_PRESSED,
            ),
        };
        let fg = if active {
            CAPTION_GLYPH
        } else {
            CAPTION_GLYPH_INACTIVE
        };

        div()
            .id(self.id())
            .flex()
            .flex_none()
            .w(px(46.))
            .h(px(30.))
            .items_center()
            .justify_center()
            .font_family("Segoe Fluent Icons")
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
    ]
);

const WINDOW_WIDTH: f32 = 2213.;
const WINDOW_HEIGHT: f32 = 1107.;
const WINDOW_MAX_SCREEN_SHARE: f32 = 0.9;
const EDITOR_FONT_SIZE: f32 = 11. * 96. / 72.;
const EDITOR_LINE_HEIGHT: f32 = 20.;
const CARET_HEIGHT: f32 = 18.;
const CARET_COLOR: u32 = 0x060606;
const TAB_WIDTH: f32 = 240.;
const TAB_LABEL_INSET: f32 = 8.;
const TAB_LABEL_TRAILING: f32 = 44.;
const UNSAVED_DOT: f32 = 8.;
const CLOSE_BUTTON_TOP: f32 = 5.;
const CLOSE_BUTTON_RIGHT: f32 = 6.;
const PLUS_LINE: u32 = 0x000000a4;
const PLUS_CAP_LEAD: u32 = 0x00000029;
const PLUS_CAP_TRAIL: u32 = 0x0000004f;
const PLUS_CENTER: u32 = 0x000000d4;
const PLUS_SEGMENTS: [(f32, f32, f32, f32, u32); 9] = [
    (11., 12., 1., 1., PLUS_CAP_LEAD),
    (12., 12., 4., 1., PLUS_LINE),
    (16., 12., 1., 1., PLUS_CENTER),
    (17., 12., 4., 1., PLUS_LINE),
    (21., 12., 1., 1., PLUS_CAP_TRAIL),
    (16., 7., 1., 1., PLUS_CAP_LEAD),
    (16., 8., 1., 4., PLUS_LINE),
    (16., 13., 1., 4., PLUS_LINE),
    (16., 17., 1., 1., PLUS_CAP_TRAIL),
];
const ZOOM_STEP: f32 = 0.1;
const ZOOM_MIN: f32 = 0.1;
const ZOOM_MAX: f32 = 5.;

struct ViewSettings {
    status_bar: bool,
    word_wrap: bool,
}

impl Global for ViewSettings {}

fn key_bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("ctrl-n", NewTab, None),
        KeyBinding::new("ctrl-shift-n", NewWindow, None),
        KeyBinding::new("ctrl-o", Open, None),
        KeyBinding::new("ctrl-s", Save, None),
        KeyBinding::new("ctrl-shift-s", SaveAs, None),
        KeyBinding::new("ctrl-alt-s", SaveAll, None),
        KeyBinding::new("ctrl-w", CloseTab, None),
        KeyBinding::new("ctrl-shift-w", CloseWindow, None),
        KeyBinding::new("ctrl-f", Find, None),
        KeyBinding::new("f3", FindNext, None),
        KeyBinding::new("shift-f3", FindPrevious, None),
        KeyBinding::new("ctrl-h", Replace, None),
        KeyBinding::new("ctrl-g", GoTo, None),
        KeyBinding::new("f5", TimeDate, None),
        KeyBinding::new("ctrl-=", ZoomIn, None),
        KeyBinding::new("ctrl--", ZoomOut, None),
        KeyBinding::new("ctrl-0", ResetZoom, None),
    ]
}

fn window_options(cx: &App) -> WindowOptions {
    let screen = cx
        .primary_display()
        .map(|display| display.visible_bounds().size)
        .unwrap_or(size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)));
    let window_size = size(
        px(WINDOW_WIDTH).min(screen.width * WINDOW_MAX_SCREEN_SHARE),
        px(WINDOW_HEIGHT).min(screen.height * WINDOW_MAX_SCREEN_SHARE),
    );

    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            window_size,
            cx,
        ))),
        titlebar: Some(TitlebarOptions {
            title: Some("Jot".into()),
            appears_transparent: true,
            traffic_light_position: None,
        }),
        window_background: WindowBackgroundAppearance::MicaAltBackdrop,
        ..Default::default()
    }
}

fn open_jot_window(cx: &mut App) -> (AnyWindowHandle, Entity<Jot>) {
    gpui_kit::open_window(window_options(cx), cx, |window, cx| {
        cx.new(|cx| Jot::new(window, cx))
    })
    .expect("Failed to open window")
}

fn caret_blink_period() -> Option<Duration> {
    let millis = unsafe { GetCaretBlinkTime() };
    (millis != 0 && millis != u32::MAX).then(|| Duration::from_millis(millis.into()))
}

fn time_date() -> String {
    let mut now: SYSTEMTIME = unsafe { std::mem::zeroed() };
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

fn caret_key(editor: &TextareaState) -> (usize, Range<usize>, usize) {
    (
        editor.cursor(),
        editor.selected_range(),
        editor.text().len(),
    )
}

struct Tab {
    editor: Entity<TextareaState>,
    zoom: f32,
    path: Option<PathBuf>,
    format: document::Format,
    dirty: bool,
    _observer: Subscription,
    _changes: Subscription,
}

pub struct Jot {
    tabs: Vec<Tab>,
    active: usize,
    hovered_tab: Option<usize>,
    close_confirmed: bool,
    menu_bar: Entity<MenuBar>,
    caret_visible: bool,
    caret_key: (usize, Range<usize>, usize),
    _caret_blink: Task<()>,
    _subscriptions: Vec<Subscription>,
    #[cfg(feature = "bench")]
    painted: bench::FrameProbe,
}

impl Jot {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let tab = Self::create_tab(None, window, cx);
        let editor = tab.editor.clone();
        let menu_bar = cx.new(|cx| MenuBar::new(editor, cx));

        let _subscriptions = vec![
            cx.observe_window_activation(window, |this, window, cx| {
                if window.is_window_active() {
                    this.restart_caret_blink(cx);
                } else {
                    this._caret_blink = Task::ready(());
                    cx.notify();
                }
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
            close_confirmed: false,
            menu_bar,
            caret_visible: true,
            caret_key: (0, 0..0, 0),
            _caret_blink: Task::ready(()),
            _subscriptions,
            #[cfg(feature = "bench")]
            painted: Rc::new(Cell::new(None)),
        };
        jot.activate_tab(0, window, cx);
        let this = cx.weak_entity();
        window.on_window_should_close(cx, move |window, cx| {
            this.update(cx, |jot, cx| jot.request_close_window(window, cx))
                .unwrap_or(true)
        });
        #[cfg(feature = "bench")]
        bench::schedule(jot.painted.clone(), window, cx);
        jot
    }

    fn create_tab(
        content: Option<(PathBuf, document::Loaded)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Tab {
        let word_wrap = cx.global::<ViewSettings>().word_wrap;
        let (path, text, format) = match content {
            Some((path, loaded)) => (Some(path), loaded.text, loaded.format),
            None => (None, String::new(), document::Format::default()),
        };
        let editor = cx.new(|cx| {
            TextareaState::new(window, cx)
                .soft_wrap(word_wrap)
                .default_value(text)
        });
        let _changes = cx.subscribe(&editor, |this, editor, event: &InputEvent, cx| {
            if let InputEvent::Change = event
                && let Some(tab) = this.tabs.iter_mut().find(|tab| tab.editor == editor)
                && !tab.dirty
            {
                tab.dirty = true;
                cx.notify();
            }
        });
        let _observer = cx.observe(&editor, |this, editor, cx| {
            if &editor != this.active_editor() {
                return;
            }
            let key = caret_key(editor.read(cx));
            if key != this.caret_key {
                this.caret_key = key;
                this.restart_caret_blink(cx);
            }
        });
        Tab {
            editor,
            zoom: 1.,
            path,
            format,
            dirty: false,
            _observer,
            _changes,
        }
    }

    fn active_editor(&self) -> &Entity<TextareaState> {
        &self.tabs[self.active].editor
    }

    fn zoom(&self) -> f32 {
        self.tabs[self.active].zoom
    }

    fn add_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let tab = Self::create_tab(None, window, cx);
        self.tabs.push(tab);
        self.activate_tab(self.tabs.len() - 1, window, cx);
    }

    fn activate_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.active = ix;
        let editor = self.active_editor().clone();
        editor.focus_handle(cx).focus(window, cx);
        self.menu_bar
            .update(cx, |menu_bar, _| menu_bar.editor = editor.clone());
        self.caret_key = caret_key(editor.read(cx));
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
        let height = px((CARET_HEIGHT * self.zoom()).round());
        let shown = self.caret_visible
            && window.is_window_active()
            && editor.focus_handle(cx).is_focused(window);

        canvas(
            |_, _, _| {},
            move |_, _, window, cx| {
                let editor = editor.read(cx);
                if !shown || !editor.selected_range().is_empty() {
                    return;
                }
                let Some(geometry) = editor.prepaint_caret_geometry() else {
                    return;
                };
                let origin = geometry.cursor_bounds().origin;
                let caret = Bounds::new(
                    point(origin.x, origin.y + geometry.scroll_offset().y),
                    size(px(1.), height),
                );
                window.with_content_mask(
                    Some(ContentMask {
                        bounds: geometry.input_bounds(),
                    }),
                    |window| window.paint_quad(fill(caret, rgb(CARET_COLOR))),
                );
            },
        )
        .absolute()
        .size_full()
    }

    fn insert_time_date(&mut self, _: &TimeDate, window: &mut Window, cx: &mut Context<Self>) {
        let stamp = time_date();
        self.active_editor()
            .clone()
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

    fn remove_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.tabs.len() == 1 {
            window.remove_window();
            return;
        }
        self.tabs.remove(ix);
        self.hovered_tab = None;
        if ix < self.active || self.active == self.tabs.len() {
            self.active -= 1;
        }
        self.activate_tab(self.active, window, cx);
    }

    fn caption_buttons(&self, window: &Window) -> impl IntoElement {
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
                    .child(Caption::Minimize.render(active))
                    .child(maximize.render(active))
                    .child(Caption::Close.render(active)),
            )
            .child(div().flex_1().window_control_area(WindowControlArea::Drag))
    }

    fn tab(&self, ix: usize, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let active = ix == self.active;
        let separator = !active && ix + 1 != self.active && ix + 1 < self.tabs.len();
        let dirty = self.tabs[ix].dirty;
        let show_close = self.hovered_tab == Some(ix) || (active && !dirty);
        let show_dot = dirty && !show_close;
        let inset = if active { 1. } else { 0. };
        let name = self.tab_name(ix);

        div()
            .id(("tab", ix))
            .relative()
            .flex_none()
            .w(px(TAB_WIDTH))
            .h(px(32.))
            .when(active, |this| {
                this.border_t_1()
                    .border_l_1()
                    .border_r_1()
                    .border_color(rgba(STROKE))
                    .rounded_t(px(8.))
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| this.activate_tab(ix, window, cx)),
            )
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered {
                    this.hovered_tab = Some(ix);
                } else if this.hovered_tab == Some(ix) {
                    this.hovered_tab = None;
                }
                cx.notify();
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .size_full()
                    .pl(px(TAB_LABEL_INSET))
                    .pr(px(TAB_LABEL_TRAILING))
                    .font_family(UI_FONT)
                    .text_size(px(12.))
                    .map(|this| {
                        if active {
                            this.rounded_t(px(7.))
                                .bg(rgba(LAYER_ON_MICA))
                                .text_color(rgba(TEXT_PRIMARY))
                                .font_weight(FontWeight::SEMIBOLD)
                        } else {
                            this.rounded_t(px(8.))
                                .text_color(rgba(TEXT_SECONDARY))
                                .hover(|style| style.bg(rgba(SUBTLE_FILL_SECONDARY)))
                        }
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(name),
                    ),
            )
            .when(show_dot, |this| {
                this.child(
                    div()
                        .absolute()
                        .top(px(CLOSE_BUTTON_TOP - inset))
                        .right(px(CLOSE_BUTTON_RIGHT - inset))
                        .flex()
                        .items_center()
                        .justify_center()
                        .w(px(32.))
                        .h(px(24.))
                        .child(
                            div()
                                .size(px(UNSAVED_DOT))
                                .rounded_full()
                                .bg(rgba(TEXT_PRIMARY)),
                        ),
                )
            })
            .when(separator, |this| {
                this.child(
                    div()
                        .absolute()
                        .right_0()
                        .top(px(8.))
                        .w(px(1.))
                        .h(px(16.))
                        .bg(rgba(STROKE)),
                )
            })
            .when(show_close, |this| {
                this.child(
                    div()
                        .id(("close-tab", ix))
                        .absolute()
                        .top(px(CLOSE_BUTTON_TOP - inset))
                        .right(px(CLOSE_BUTTON_RIGHT - inset))
                        .flex()
                        .items_center()
                        .justify_center()
                        .w(px(32.))
                        .h(px(24.))
                        .rounded(px(4.))
                        .font_family("Segoe Fluent Icons")
                        .text_size(px(12.))
                        .text_color(rgba(TEXT_PRIMARY))
                        .hover(|style| style.bg(rgba(SUBTLE_FILL_SECONDARY)))
                        .active(|style| style.bg(rgba(SUBTLE_FILL_TERTIARY)))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(
                            cx.listener(move |this, _, window, cx| this.close_tab(ix, window, cx)),
                        )
                        .child("\u{E711}"),
                )
            })
    }

    fn new_tab_button(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        div()
            .id("new-tab")
            .flex_none()
            .ml(px(5.))
            .mt(px(5.))
            .w(px(32.))
            .h(px(24.))
            .rounded(px(4.))
            .hover(|style| style.bg(rgba(SUBTLE_FILL_SECONDARY)))
            .active(|style| style.bg(rgba(SUBTLE_FILL_TERTIARY)))
            .on_click(cx.listener(|this, _, window, cx| this.add_tab(window, cx)))
            .child(
                canvas(
                    |_, _, _| {},
                    |bounds, _, window, _| {
                        for (x, y, w, h, color) in PLUS_SEGMENTS {
                            window.paint_quad(fill(
                                Bounds::new(
                                    bounds.origin + point(px(x), px(y)),
                                    size(px(w), px(h)),
                                ),
                                rgba(color),
                            ));
                        }
                    },
                )
                .size_full(),
            )
    }

    fn title_bar(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tabs: Vec<_> = (0..self.tabs.len()).map(|ix| self.tab(ix, cx)).collect();

        div()
            .flex()
            .flex_none()
            .h(px(42.))
            .child(
                div()
                    .flex_none()
                    .w(px(9.))
                    .h_full()
                    .window_control_area(WindowControlArea::Drag),
            )
            .child(
                div()
                    .v_flex()
                    .flex_none()
                    .child(
                        div()
                            .h(px(10.))
                            .window_control_area(WindowControlArea::Drag),
                    )
                    .child(
                        div()
                            .flex()
                            .h(px(32.))
                            .children(tabs)
                            .child(self.new_tab_button(cx)),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .window_control_area(WindowControlArea::Drag),
            )
            .child(self.caption_buttons(window))
    }

    fn menu_bar(&self) -> impl IntoElement {
        div()
            .flex_none()
            .child(
                div()
                    .h(px(32.))
                    .pl(px(4.))
                    .bg(rgba(LAYER_ON_MICA))
                    .child(self.menu_bar.clone()),
            )
            .child(separator())
    }

    fn editor(&self, window: &Window, cx: &App) -> impl IntoElement {
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .pl(px(6.))
            .pt(px(7.))
            .bg(rgb(EDITOR_BACKGROUND))
            .child(
                Textarea::new(self.active_editor())
                    .appearance(false)
                    .h_full()
                    .font_family("Consolas")
                    .text_size(px(EDITOR_FONT_SIZE * self.zoom()))
                    .line_height(px((EDITOR_LINE_HEIGHT * self.zoom()).round())),
            )
            .child(self.caret(window, cx))
    }

    fn status_bar(&self, cx: &App) -> impl IntoElement {
        let editor = self.active_editor().read(cx);
        let text = editor.text();
        let cursor = editor.cursor();
        let point = text.offset_to_point(cursor);
        let column = text.byte_to_char_idx(cursor) - text.byte_to_char_idx(cursor - point.column);
        let selection = editor.selected_range();
        let selected =
            text.byte_to_char_idx(selection.end) - text.byte_to_char_idx(selection.start);
        let total = text.len_chars();

        div().flex_none().child(separator()).child(
            div()
                .flex()
                .items_center()
                .h(px(31.))
                .bg(rgba(LAYER_ON_MICA))
                .font_family(UI_FONT)
                .text_size(px(12.))
                .text_color(rgba(TEXT_SECONDARY))
                .child(div().flex_none().w(px(96.)).pl(px(22.)).child(format!(
                    "Ln {}, Col {}",
                    point.row + 1,
                    column + 1
                )))
                .child(status_divider())
                .child(div().pl(px(8.)).child(character_count(selected, total)))
                .child(div().flex_1())
                .child(status_cell(
                    68.,
                    format!("{}%", (self.zoom() * 100.).round()),
                ))
                .child(status_cell(144., "UTF-8")),
        )
    }
}

fn status_divider() -> Div {
    div().flex_none().w(px(1.)).h(px(16.)).bg(rgba(STROKE))
}

fn status_cell(width: f32, label: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .w(px(width))
        .child(status_divider())
        .child(div().pl(px(12.)).child(label.into()))
}

impl Render for Jot {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let status_bar = cx.global::<ViewSettings>().status_bar;

        let root = div()
            .v_flex()
            .size_full()
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
            .child(self.title_bar(window, cx))
            .child(self.menu_bar())
            .child(self.editor(window, cx))
            .when(status_bar, |this| this.child(self.status_bar(cx)));
        #[cfg(feature = "bench")]
        let root = root.child(bench::probe(&self.painted));
        root
    }
}

fn main() {
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);

            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();

            let theme = Theme::global_mut(cx);
            theme.background = transparent_black();
            theme.tokens.background = transparent_black().into();
            theme.caret = hsla(0., 0., 0., f32::EPSILON);
            theme.tokens.caret = hsla(0., 0., 0., f32::EPSILON).into();
            cx.set_text_rendering_mode(TextRenderingMode::Grayscale);

            cx.set_global(ViewSettings {
                status_bar: true,
                #[cfg(not(feature = "bench"))]
                word_wrap: true,
                #[cfg(feature = "bench")]
                word_wrap: bench::word_wrap(),
            });
            cx.bind_keys(key_bindings());
            cx.bind_keys(menu::key_bindings());

            cx.on_action(|_: &Exit, cx| {
                for window in cx.windows() {
                    window
                        .update(cx, |_, window, cx| {
                            window.dispatch_action(Box::new(CloseWindow), cx)
                        })
                        .ok();
                }
            });
            cx.on_action(|_: &NewWindow, cx| {
                open_jot_window(cx);
            });
            cx.on_action(|_: &ToggleStatusBar, cx| {
                cx.update_global::<ViewSettings, _>(|view, _| view.status_bar = !view.status_bar);
            });
            cx.on_action(|_: &ToggleWordWrap, cx| {
                cx.update_global::<ViewSettings, _>(|view, _| view.word_wrap = !view.word_wrap);
            });

            let (window, jot) = open_jot_window(cx);
            let paths: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
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
