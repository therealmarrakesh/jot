#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod brush;
mod dialog;
mod document;
mod editor;
mod field;
mod files;
mod find;
mod instance;
mod keytips;
mod measure;
mod menu;
mod overlay;
mod scrollbar;
mod tab_drag;
mod tabs;
mod theme;
mod tooltip;
mod ui;
mod win32;

use brush::{BrushTransitionExt as _, Brushes, PressedExt as _};
use crate::ui::*;
use crate::editor::{EditorState, Rope, RopeExt as _};
use menu::MenuBar;
use std::cell::Cell;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;
use theme::{Palette, Surface, palette, theme_shadow};

const APP_NAME: &str = "Jot";
const NOTICES_FLAG: &str = "--notices";
const CONTEXT: &str = "Jot";

const WINDOW_WIDTH: f32 = 2213.;
const WINDOW_HEIGHT: f32 = 1107.;
const WINDOW_MAX_SCREEN_SHARE: f32 = 0.9;

const SMALL_FONT: &str = "Segoe UI Variable Small";
const TEXT_FONT: &str = "Segoe UI Variable Text";
const ICON_FONT: &str = "Segoe Fluent Icons";
const CLOSE_GLYPH: &str = "\u{E711}";

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
const EDITOR_PADDING_LEFT: f32 = 18.;
const EDITOR_PADDING_RIGHT: f32 = 12.;
const EDITOR_PADDING_TOP: f32 = 11.;
const EDITOR_PADDING_BOTTOM: f32 = 11.;
pub(crate) const EDITOR_PADDINGS: Edges<Pixels> = Edges {
    top: px(EDITOR_PADDING_TOP),
    right: px(EDITOR_PADDING_RIGHT),
    bottom: px(EDITOR_PADDING_BOTTOM),
    left: px(EDITOR_PADDING_LEFT),
};
const CARET_HEIGHT: f32 = 18.;
const ZOOM_STEP: f32 = 0.1;
const ZOOM_MIN: f32 = 0.1;
const ZOOM_MAX: f32 = 5.;

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
            .pressed(move |style| style.bg(rgba(pressed_bg)).text_color(rgba(pressed_fg)))
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
        .flex()
        .flex_col()
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

crate::ui::actions!(
    jot,
    [
        NewTab,
        NewWindow,
        Open,
        Save,
        SaveAs,
        SaveAll,
        CloseTab,
        NextTab,
        PrevTab,
        SelectLastTab,
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

#[derive(Clone, PartialEq, Eq)]
struct SelectTab(usize);
crate::ui::impl_action!(jot, SelectTab);

struct ViewSettings {
    status_bar: bool,
    word_wrap: bool,
}

impl Global for ViewSettings {}

fn key_bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("escape", Cancel, Some(CONTEXT)),
        KeyBinding::new("ctrl-n", NewTab, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-n", NewWindow, Some(CONTEXT)),
        KeyBinding::new("ctrl-o", Open, Some(CONTEXT)),
        KeyBinding::new("ctrl-s", Save, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-s", SaveAs, Some(CONTEXT)),
        KeyBinding::new("ctrl-alt-s", SaveAll, Some(CONTEXT)),
        KeyBinding::new("ctrl-w", CloseTab, Some(CONTEXT)),
        KeyBinding::new("ctrl-tab", NextTab, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-tab", PrevTab, Some(CONTEXT)),
        KeyBinding::new("ctrl-pagedown", NextTab, Some(CONTEXT)),
        KeyBinding::new("ctrl-pageup", PrevTab, Some(CONTEXT)),
        KeyBinding::new("ctrl-1", SelectTab(0), Some(CONTEXT)),
        KeyBinding::new("ctrl-2", SelectTab(1), Some(CONTEXT)),
        KeyBinding::new("ctrl-3", SelectTab(2), Some(CONTEXT)),
        KeyBinding::new("ctrl-4", SelectTab(3), Some(CONTEXT)),
        KeyBinding::new("ctrl-5", SelectTab(4), Some(CONTEXT)),
        KeyBinding::new("ctrl-6", SelectTab(5), Some(CONTEXT)),
        KeyBinding::new("ctrl-7", SelectTab(6), Some(CONTEXT)),
        KeyBinding::new("ctrl-8", SelectTab(7), Some(CONTEXT)),
        KeyBinding::new("ctrl-9", SelectLastTab, Some(CONTEXT)),
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
        KeyBinding::new("ctrl-+", ZoomIn, Some(CONTEXT)),
        KeyBinding::new("ctrl--", ZoomOut, Some(CONTEXT)),
        KeyBinding::new("ctrl-0", ResetZoom, Some(CONTEXT)),
    ]
}

fn window_options(window_size: Option<Size<Pixels>>, cx: &App) -> WindowOptions {
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
        }),
        window_background: WindowBackgroundAppearance::MicaAltBackdrop,
        ..Default::default()
    }
}

fn open_jot_window(
    initial: Option<TabData>,
    window_size: Option<Size<Pixels>>,
    cx: &mut App,
) -> (WindowHandle<Jot>, Entity<Jot>) {
    let mut created = None;
    let handle = cx
        .open_window(window_options(window_size, cx), |window, cx| {
            let view = cx.new(|cx| Jot::new(initial, window, cx));
            created = Some(view.clone());
            view
        })
        .expect("Failed to open window");
    (handle, created.expect("Failed to create Jot view"))
}

fn round_half_up(value: f32) -> f32 {
    (value + 0.5).floor()
}

pub(crate) fn text_width(window: &Window, text: &str, family: &'static str, size: f32) -> f32 {
    let run = TextRun {
        len: text.len(),
        font: font(family),
        color: black(),
        underline: None,
    };
    window
        .text_system()
        .layout_line(text, px(size), &[run])
        .width
        .as_f32()
        .ceil()
}

struct TabData {
    editor: Entity<EditorState>,
    zoom: f32,
    path: Option<PathBuf>,
    format: document::Format,
}

impl TabData {
    pub(crate) fn is_dirty(&self, cx: &App) -> bool {
        let editor = self.editor.read(cx);
        if self.path.is_some() {
            !editor.is_clean()
        } else {
            !editor.is_clean() && editor.text().len() > 0
        }
    }
}

struct Tab {
    data: TabData,
    slot: tabs::SlotKey,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct StatusMetrics {
    line: usize,
    column: usize,
    selected: usize,
    total: usize,
    zoom_pct: u32,
    format: document::Format,
}

#[derive(Clone, Copy, PartialEq)]
struct MinSizeKey {
    status: Option<StatusMetrics>,
    zoom_pct: u32,
    menu_width: f32,
    scale: f32,
}

#[derive(Clone, Copy)]
struct MinSizeCache {
    key: MinSizeKey,
    size: Size<Pixels>,
}

struct Jot {
    tabs: Vec<Tab>,
    active: usize,
    strip: tabs::TabStrip,
    hwnd: WindowHwnd,
    menu_bar: MenuBar,
    dialogs: dialog::Dialogs,
    find: find::FindBar,
    min_size: Rc<Cell<Option<MinSizeCache>>>,
    _subscriptions: Vec<Subscription>,
}

impl Jot {
    fn new(initial: Option<TabData>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let tab = match initial {
            Some(data) => Self::adopt_tab(data, window, cx),
            None => Self::create_tab(None, window, cx),
        };
        let menu_bar = MenuBar::new(window, cx);
        let dialogs = dialog::Dialogs::new(window, cx);
        let find = find::FindBar::new(window, cx);

        let access_keys = {
            let jot = cx.weak_entity();
            let handle = window.window_handle();
            cx.intercept_keystrokes(move |event, window, cx| {
                if window.window_handle() != handle {
                    return;
                }
                if jot
                    .update(cx, |jot, cx| {
                        !jot.dialogs.is_open() && jot.menu_key_down(&event.keystroke, window, cx)
                    })
                    .unwrap_or(false)
                {
                    cx.stop_propagation();
                }
            })
        };
        let _subscriptions = vec![
            access_keys,
            cx.observe_window_appearance(window, |_, window, cx| {
                theme::apply(window.appearance(), cx);
                cx.notify();
            }),
            cx.observe_window_activation(window, |this, window, cx| {
                if !window.is_window_active() {
                    this.menu_cancel(window, cx);
                    tooltip::window_deactivated(window.window_handle(), cx);
                    cx.notify();
                }
            }),
            cx.observe_global_in::<ViewSettings>(window, |this, window, cx| {
                let word_wrap = cx.global::<ViewSettings>().word_wrap;
                for tab in &this.tabs {
                    tab.data
                        .editor
                        .update(cx, |editor, cx| editor.set_soft_wrap(word_wrap, window, cx));
                }
                cx.notify();
            }),
        ];

        let mut jot = Self {
            tabs: vec![tab],
            active: 0,
            strip: tabs::TabStrip::new(),
            hwnd: window.hwnd(),
            menu_bar,
            dialogs,
            find,
            min_size: Rc::new(Cell::new(None)),
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

    fn active_editor(&self) -> &Entity<EditorState> {
        &self.tabs[self.active].data.editor
    }

    fn has_text(&self, cx: &App) -> bool {
        self.active_editor().read(cx).text().len() > 0
    }

    fn zoom(&self) -> f32 {
        self.tabs[self.active].data.zoom
    }

    fn line_height(&self) -> f32 {
        (EDITOR_LINE_HEIGHT * self.zoom()).round()
    }

    fn editor_min_height(&self) -> f32 {
        EDITOR_PADDING_TOP + EDITOR_PADDING_BOTTOM + self.line_height()
    }

    fn status_metrics(&self, cx: &App) -> StatusMetrics {
        let editor = self.active_editor().read(cx);
        let text = editor.text();
        let cursor = editor.cursor();
        let point = text.offset_to_point(cursor);
        let column = text.byte_to_char_idx(cursor) - text.byte_to_char_idx(cursor - point.column);
        let selection = editor.selected_range();
        let selected =
            text.byte_to_char_idx(selection.end) - text.byte_to_char_idx(selection.start);
        StatusMetrics {
            line: point.row + 1,
            column: column + 1,
            selected,
            total: text.len_chars(),
            zoom_pct: (self.zoom() * 100.).round() as u32,
            format: self.tabs[self.active].data.format,
        }
    }

    fn min_size_probe(
        &self,
        metrics: Option<StatusMetrics>,
        window: &Window,
        cx: &App,
    ) -> Option<impl IntoElement + use<>> {
        let zoom_pct = metrics.map_or((self.zoom() * 100.).round() as u32, |m| m.zoom_pct);
        let scale = window.scale_factor();
        let menu_width = self.menu_bar.width();
        let key = MinSizeKey {
            status: metrics,
            zoom_pct,
            menu_width,
            scale,
        };
        if self
            .min_size
            .get()
            .is_some_and(|cached| cached.key == key)
        {
            return None;
        }
        let hwnd = self.hwnd;
        let menu_areas = self.menu_bar.hit_areas();
        let cache = self.min_size.clone();
        let status = metrics.map(|metrics| self.status_bar(metrics, cx).into_any_element());
        let rows = tabs::STRIP_TOP
            + tabs::TAB_HEIGHT
            + MENU_BAR_HEIGHT
            + SEPARATOR_HEIGHT
            + self.editor_min_height();
        Some(
            canvas(
                move |_, window, cx| {
                    let status = status.map(|mut status| {
                        status.layout_as_root(AvailableSpace::min_size(), window, cx)
                    });
                    let measured_menu = menu::bar_width(&menu_areas);
                    let menu_row =
                        MENU_BAR_LEFT + measured_menu + THEME_BUTTON_WIDTH + THEME_BUTTON_RIGHT;
                    let width = status
                        .map_or(0., |status| status.width.as_f32())
                        .max(menu_row)
                        .max(tabs::TITLE_BAR_MIN_WIDTH);
                    let height = rows + status.map_or(0., |status| status.height.as_f32());
                    let next = size(px(width), px(height));
                    let changed = cache.get().is_none_or(|cached| cached.size != next);
                    cache.set(Some(MinSizeCache {
                        key: MinSizeKey {
                            menu_width: measured_menu,
                            ..key
                        },
                        size: next,
                    }));
                    if changed {
                        crate::ui::set_window_min_size(hwnd, Some(next));
                    }
                },
                |_, _, _, _| {},
            )
            .absolute(),
        )
    }

    fn caret_height(&self) -> Pixels {
        px((CARET_HEIGHT * self.zoom()).round())
    }

    fn activate_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.active = ix;
        self.reveal_tab(ix);
        self.find.clear_notice();
        if self.find.is_open() && !self.has_text(cx) {
            self.close_find(window, cx);
        }
        self.active_editor().focus_handle(cx).focus(window, cx);
        self.sync_selection_focus(cx);
        self.refresh_title(window);
        cx.notify();
    }

    pub(crate) fn sync_selection_focus(&mut self, cx: &mut Context<Self>) {
        let focus = if self.menu_bar.is_open() {
            Some(self.menu_bar.focus_handle().clone())
        } else {
            self.find.selection_focus()
        };
        for (ix, tab) in self.tabs.iter().enumerate() {
            let focus = (ix == self.active).then(|| focus.clone()).flatten();
            tab.data
                .editor
                .update(cx, |editor, cx| editor.set_selection_focus(focus, cx));
        }
    }

    fn new_tab(&mut self, _: &NewTab, window: &mut Window, cx: &mut Context<Self>) {
        self.add_tab(window, cx);
    }

    fn insert_time_date(&mut self, _: &TimeDate, window: &mut Window, cx: &mut Context<Self>) {
        let stamp = crate::win32::format_local_time_date();
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
        self.tabs[self.active].data.zoom =
            ((zoom / ZOOM_STEP).round() * ZOOM_STEP).clamp(ZOOM_MIN, ZOOM_MAX);
        cx.notify();
    }

    fn cancel(&mut self, _: &Cancel, window: &mut Window, cx: &mut Context<Self>) {
        if tab_drag::dragging(cx) {
            cx.defer(tab_drag::cancel);
        } else if self.find.is_open() {
            self.close_find(window, cx);
        } else {
            cx.propagate();
        }
    }

    fn close_window(&mut self, _: &CloseWindow, window: &mut Window, cx: &mut Context<Self>) {
        if self.request_close_window(window, cx) {
            window.remove_window();
        }
    }

    fn close_active_tab(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        self.close_tab(self.active, window, cx);
    }

    fn next_tab(&mut self, _: &NextTab, window: &mut Window, cx: &mut Context<Self>) {
        if self.dialogs.is_open() || self.tabs.len() <= 1 {
            return;
        }
        let next = (self.active + 1) % self.tabs.len();
        self.activate_tab(next, window, cx);
    }

    fn prev_tab(&mut self, _: &PrevTab, window: &mut Window, cx: &mut Context<Self>) {
        if self.dialogs.is_open() || self.tabs.len() <= 1 {
            return;
        }
        let prev = (self.active + self.tabs.len() - 1) % self.tabs.len();
        self.activate_tab(prev, window, cx);
    }

    fn select_tab(&mut self, action: &SelectTab, window: &mut Window, cx: &mut Context<Self>) {
        if self.dialogs.is_open() || action.0 >= self.tabs.len() || action.0 == self.active {
            return;
        }
        self.activate_tab(action.0, window, cx);
    }

    fn select_last_tab(&mut self, _: &SelectLastTab, window: &mut Window, cx: &mut Context<Self>) {
        let last = self.tabs.len().saturating_sub(1);
        if self.dialogs.is_open() || last == self.active {
            return;
        }
        self.activate_tab(last, window, cx);
    }

    fn title_bar(
        &self,
        strip: &tabs::StripFrame,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
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
                    .flex()
                    .flex_col()
                    .flex_none()
                    .child(
                        div()
                            .h(px(tabs::STRIP_TOP))
                            .window_control_area(WindowControlArea::Drag),
                    )
                    .child(self.tab_strip(strip, cx)),
            )
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .window_control_area(WindowControlArea::Drag),
            )
            .child(caption_buttons(window, p))
    }

    fn render_menu_bar(&self, menu: impl IntoElement, p: Palette) -> impl IntoElement {
        div()
            .flex_none()
            .child(
                div()
                    .flex()
                    .h(px(MENU_BAR_HEIGHT))
                    .pl(px(MENU_BAR_LEFT))
                    .pr(px(THEME_BUTTON_RIGHT))
                    .bg(rgba(p.layer))
                    .child(menu)
                    .child(div().flex_1())
                    .child(
                        field::icon_button(
                            p,
                            "theme",
                            "Switch theme",
                            if p.dark { SUN_GLYPH } else { MOON_GLYPH },
                            field::IconButtonStyle {
                                glyph_size: px(THEME_GLYPH_SIZE),
                                size: size(px(THEME_BUTTON_WIDTH), px(MENU_BAR_HEIGHT)),
                                radius: px(field::CONTROL_RADIUS),
                            },
                            true,
                        )
                        .flex_none()
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(ToggleTheme), cx);
                        }),
                    ),
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
        let Some(caret) = editor.read(cx).caret_bounds() else {
            return;
        };
        let anchor = point(caret.right(), caret.bottom());
        self.open_context_menu(anchor, window, cx);
    }

    fn editor(
        &self,
        find: Option<impl IntoElement>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let p = palette(cx);
        let editor = self.active_editor().clone();
        {
            let state = editor.read(cx);
            state.caret_height.set(self.caret_height());
            state.caret_hidden.set(self.menu_bar.access_active());
        }
        div()
            .id("editor-area")
            .relative()
            .flex_1()
            .min_h(px(self.editor_min_height()))
            .bg(rgb(p.editor))
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _, cx| {
                if !event.modifiers.control
                    || this.dialogs.is_open()
                    || this.menu_bar.is_open()
                {
                    return;
                }
                let dy = match event.delta {
                    ScrollDelta::Lines(lines) => lines.y,
                    ScrollDelta::Pixels(pixels) => pixels.y.as_f32(),
                };
                if dy > 0. {
                    this.set_zoom(this.zoom() + ZOOM_STEP, cx);
                } else if dy < 0. {
                    this.set_zoom(this.zoom() - ZOOM_STEP, cx);
                }
                cx.stop_propagation();
            }))
            .child(
                div()
                    .relative()
                    .size_full()
                    .overflow_hidden()
                    .child(
                        div()
                            .size_full()
                            .font_family("Consolas")
                            .text_size(px(EDITOR_FONT_SIZE * self.zoom()))
                            .line_height(px(self.line_height()))
                            .text_color(rgba(p.text_primary))
                            .child(editor),
                    ),
            )
            .children(find)
    }

    fn status_bar(&self, metrics: StatusMetrics, cx: &App) -> impl IntoElement {
        let p = palette(cx);

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
                .child(format!("Ln {}, Col {}", metrics.line, metrics.column))
                .child(status_divider(p))
                .child(character_count(metrics.selected, metrics.total))
                .child(status_spacer(p))
                .child(format!("{}%", metrics.zoom_pct))
                .child(status_divider(p))
                .child(metrics.format.line_ending.label())
                .child(status_divider(p))
                .child(metrics.format.encoding.label()),
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
        let status_metrics = cx
            .global::<ViewSettings>()
            .status_bar
            .then(|| self.status_metrics(cx));
        let strip = self.sample_tabs(window, cx);
        let menu = self.render_menu(window, cx);
        let dialogs = self.render_dialogs(window, cx);
        let min_size = self.min_size_probe(status_metrics, window, cx);
        let find = self.render_find(window, cx);

        let root = div()
            .relative()
            .size_full()
            .capture_any_mouse_down(cx.listener(|this, _, _, cx| {
                this.menu_pointer_down(cx);
            }))
            .on_drop(cx.listener(Self::open_dropped))
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::show_context_menu))
            .on_action(cx.listener(Self::zoom_in))
            .on_action(cx.listener(Self::zoom_out))
            .on_action(cx.listener(Self::reset_zoom))
            .on_action(cx.listener(Self::close_window))
            .on_action(cx.listener(Self::new_tab))
            .on_action(cx.listener(Self::close_active_tab))
            .on_action(cx.listener(Self::next_tab))
            .on_action(cx.listener(Self::prev_tab))
            .on_action(cx.listener(Self::select_tab))
            .on_action(cx.listener(Self::select_last_tab))
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
                    .flex()
                    .flex_col()
                    .size_full()
                    .child(self.title_bar(&strip, window, cx))
                    .child(self.render_menu_bar(menu, p))
                    .child(self.editor(find, cx))
                    .children(status_metrics.map(|metrics| self.status_bar(metrics, cx))),
            )
            .children(dialogs)
            .children(min_size);
        tooltip::host(root)
    }
}

fn open_handoff_paths(paths: Vec<PathBuf>, cx: &mut App) {
    if paths.is_empty() {
        return;
    }
    // If a single path is already open in any Jot window, focus that window & tab directly.
    if paths.len() == 1 {
        let target = std::path::absolute(&paths[0]).unwrap_or_else(|_| paths[0].clone());
        for handle in cx.windows() {
            let Some(window) = handle.downcast::<Jot>() else {
                continue;
            };
            let existing = window
                .read(cx)
                .ok()
                .and_then(|jot| jot.find_open_tab(&target).map(|ix| (ix, jot.hwnd)));
            if let Some((ix, hwnd)) = existing {
                let _ = window.update(cx, |jot, window, cx| {
                    jot.activate_tab(ix, window, cx);
                });
                hwnd.bring_to_front();
                return;
            }
        }
    }

    let target_window = cx
        .active_window()
        .and_then(|handle| handle.downcast::<Jot>())
        .or_else(|| {
            cx.windows()
                .into_iter()
                .rev()
                .find_map(|handle| handle.downcast::<Jot>())
        });

    if let Some(window) = target_window {
        let mut hwnd = WindowHwnd::default();
        let _ = window.update(cx, |jot, window, cx| {
            hwnd = jot.hwnd;
            jot.open_paths(paths, window, cx);
        });
        hwnd.bring_to_front();
    } else {
        let (window, _) = open_jot_window(None, None, cx);
        let _ = window.update(cx, |jot, window, cx| {
            jot.open_paths(paths, window, cx);
        });
    }
}

fn main() {
    let (flags, mut paths): (Vec<_>, Vec<_>) = std::env::args_os()
        .skip(1)
        .map(PathBuf::from)
        .partition(|arg| arg.as_os_str() == NOTICES_FLAG);
    if !flags.is_empty() {
        paths.extend(document::write_notices().ok());
    }
    if !paths.is_empty() && instance::try_handoff(&paths) {
        return;
    }

    let platform = Rc::new(
        crate::ui::WindowsPlatform::new().expect("Failed to initialize Windows platform"),
    );
    Application::with_platform(platform).run(move |cx| {
        ui::reduce_motion::init(cx);
        editor::init(cx);

        let (handoff_tx, mut handoff_rx) = crate::ui::futures::channel::mpsc::unbounded::<Vec<PathBuf>>();
        instance::start_listener(move |incoming| {
            let _ = handoff_tx.unbounded_send(incoming);
        });
        cx.spawn(async move |cx| {
            while let Some(incoming) = handoff_rx.next().await {
                cx.update(|cx| open_handoff_paths(incoming, cx));
            }
        })
        .detach();

        cx.on_window_closed(|cx, window| {
            tooltip::window_closed(window, cx);
            keytips::window_closed(window, cx);
            if cx
                .windows()
                .into_iter()
                .all(|window| window.downcast::<Jot>().is_none())
            {
                cx.quit();
            }
        })
        .detach();

        theme::apply(cx.window_appearance(), cx);

        cx.set_global(ViewSettings {
            status_bar: true,
            word_wrap: true,
        });
        cx.bind_keys(key_bindings());
        cx.bind_keys(menu::key_bindings());
        cx.bind_keys(dialog::key_bindings());
        cx.bind_keys(find::key_bindings());

        cx.on_action(|_: &Exit, cx| {
            cx.defer(|cx| {
                for window in cx.windows() {
                    window
                        .update(cx, |_, window, cx| {
                            window.dispatch_action(Box::new(CloseWindow), cx);
                        })
                        .ok();
                }
            });
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

        let (window, _) = open_jot_window(None, None, cx);
        if !paths.is_empty() {
            window
                .update(cx, |jot, window, cx| {
                    jot.open_paths(paths, window, cx);
                })
                .ok();
        }
    });
}

#[cfg(test)]
mod large_file_smoke {
    use super::*;
    use crate::editor::{TextWrapper, WrapDisplayPoint};
    use std::io::Write as _;
    use std::time::Instant;

    fn run_profile(target_mb: usize) {
        let dir = std::env::temp_dir().join(format!("jot-bench-{target_mb}mb-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("big.txt");

        let target_bytes = target_mb * 1024 * 1024;
        {
            let mut w = std::io::BufWriter::with_capacity(256 * 1024, std::fs::File::create(&file).unwrap());
            let mut written = 0usize;
            let mut line_ix = 0usize;
            while written < target_bytes {
                let line = if line_ix.is_multiple_of(50) {
                    format!("Line {line_ix:07}: long wrapped paragraph with repeated words alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi omicron pi rho sigma tau upsilon phi chi psi omega end.\r\n")
                } else {
                    format!("Line {line_ix:07}: standard 80-col plain text log entry for Jot benchmark.\r\n")
                };
                w.write_all(line.as_bytes()).unwrap();
                written += line.len();
                line_ix += 1;
            }
            w.write_all(b"FINAL_SENTINEL_NEEDLE_42\r\n").unwrap();
            w.flush().unwrap();
        }

        let disk_len = std::fs::metadata(&file).unwrap().len();
        println!("\n=== Large-File Smoke Profile ({target_mb} MB, {disk_len} bytes on disk) ===");

        let t0 = Instant::now();
        let loaded = document::load(&file).unwrap();
        let load_ms = t0.elapsed().as_secs_f64() * 1000.0;
        let line_count = loaded.text.len_lines(ropey::LineType::LF);
        println!("load:                 {load_ms:8.2} ms  (rope {} bytes, {} lines)", loaded.text.len(), line_count);

        // TextWrapper: no-wrap
        let mut wrapper_nowrap = TextWrapper::new(font("Consolas"), px(14.0), None);
        let t0 = Instant::now();
        wrapper_nowrap.update_for_bench(&loaded.text, &(0..0), &loaded.text, 0, &mut |_, _, _| {});
        let nowrap_ms = t0.elapsed().as_secs_f64() * 1000.0;
        println!("wrap (no-wrap):       {nowrap_ms:8.2} ms  ({} display rows)", wrapper_nowrap.len());

        // TextWrapper: word-wrap at 900px (~110 ASCII chars per row, ascii_fit_len = 110)
        let mut wrapper_wrap = TextWrapper::new(font("Consolas"), px(14.0), Some(px(900.0)));
        let t0 = Instant::now();
        wrapper_wrap.update_for_bench(&loaded.text, &(0..0), &loaded.text, 110, &mut |line, _, out| {
            let mut i = 110;
            while i < line.len() {
                let b = line.floor_char_boundary(i);
                if b > 0 && b < line.len() {
                    out.push(crate::ui::Boundary { ix: b, next_indent: 0 });
                }
                i += 110;
            }
        });
        let wrap_ms = t0.elapsed().as_secs_f64() * 1000.0;
        println!("wrap (word-wrap):     {wrap_ms:8.2} ms  ({} display rows)", wrapper_wrap.len());

        // Incremental 1-line edit in the middle of the file
        let mid = loaded.text.len() / 2;
        let mut edited = loaded.text.clone();
        edited.insert(mid, "X");
        let insert_rope = crate::editor::Rope::from("X");
        let t0 = Instant::now();
        wrapper_wrap.update_for_bench(&edited, &(mid..mid), &insert_rope, 110, &mut |_, _, _| {});
        let edit_ms = t0.elapsed().as_secs_f64() * 1000.0;
        println!("incremental edit:     {edit_ms:8.2} ms");

        // Display point queries (scrolling to middle & end)
        let t0 = Instant::now();
        for i in 0..1000 {
            let row = (i * (wrapper_wrap.len() / 1000)).min(wrapper_wrap.len().saturating_sub(1));
            let off = wrapper_wrap.display_point_to_offset(WrapDisplayPoint::new(row, 0, 0));
            let _ = wrapper_wrap.offset_to_display_point(off);
        }
        let query_ms = t0.elapsed().as_secs_f64() * 1000.0;
        println!("1000 scroll queries:  {query_ms:8.2} ms");

        // Find (case-sensitive & case-insensitive full scan to sentinel at end)
        let mut buf = Vec::new();
        let m_cs = find::Matcher::new("FINAL_SENTINEL_NEEDLE_42", true).unwrap();
        let t0 = Instant::now();
        let hit = m_cs.next(&loaded.text, 0, usize::MAX, &mut buf);
        let find_cs_ms = t0.elapsed().as_secs_f64() * 1000.0;
        assert!(hit.is_some());
        println!("find next (case):     {find_cs_ms:8.2} ms");

        let m_ci = find::Matcher::new("final_sentinel_needle_42", false).unwrap();
        let t0 = Instant::now();
        let hit_ci = m_ci.next(&loaded.text, 0, usize::MAX, &mut buf);
        let find_ci_ms = t0.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(hit_ci, hit);
        println!("find next (nocase):   {find_ci_ms:8.2} ms");

        let m_all = find::Matcher::new("omicron", false).unwrap();
        let t0 = Instant::now();
        let all_hits = m_all.all(&loaded.text, &mut buf);
        let find_all_ms = t0.elapsed().as_secs_f64() * 1000.0;
        println!("find all (nocase):    {find_all_ms:8.2} ms  ({} matches)", all_hits.len());

        // Streaming save (atomic ReplaceFileW)
        let t0 = Instant::now();
        document::write_rope(&file, &loaded.text, loaded.format).unwrap();
        let save_ms = t0.elapsed().as_secs_f64() * 1000.0;
        println!("save (streaming):     {save_ms:8.2} ms");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    #[ignore]
    fn large_file_smoke_profile() {
        run_profile(50);
        run_profile(100);
    }
}
