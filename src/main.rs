#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod command;
mod dialog;
mod document;
mod editor;
mod field;
mod files;
mod find;
mod instance;
mod keytips;
mod layout;
mod menu;
mod overlay;
mod scrollbar;
mod tab_drag;
mod tabs;
mod theme;
mod tooltip;
mod ui;
mod win32;

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

    fn render(self, active: bool, p: Palette) -> Div {
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

fn caption_buttons(window: &Window, p: Palette) -> Div {
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

pub(crate) struct ViewSettings {
    pub(crate) status_bar: bool,
    pub(crate) word_wrap: bool,
}

impl Default for ViewSettings {
    fn default() -> Self {
        Self {
            status_bar: true,
            word_wrap: true,
        }
    }
}

/// Typed app-wide shared state owned directly by `ui::App` (replaces dynamic `Global` maps).
#[derive(Default)]
pub(crate) struct AppShared {
    pub(crate) palette: Palette,
    pub(crate) view: ViewSettings,
    pub(crate) tooltips: tooltip::Tooltips,
    pub(crate) keytips: keytips::KeyTips,
    pub(crate) tab_drag: Option<tab_drag::TabDrag>,
}

fn apply_word_wrap_to_windows(word_wrap: bool, cx: &mut App) {
    for handle in cx.windows() {
        if let Some(window) = handle.downcast::<Jot>() {
            let _ = window.update(cx, |jot, window, cx| {
                jot.set_word_wrap(word_wrap, window, cx);
            });
        }
    }
}

fn key_bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("escape", Command::Cancel, Some(CONTEXT)),
        KeyBinding::new("ctrl-n", Command::NewTab, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-n", Command::NewWindow, Some(CONTEXT)),
        KeyBinding::new("ctrl-o", Command::Open, Some(CONTEXT)),
        KeyBinding::new("ctrl-s", Command::Save, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-s", Command::SaveAs, Some(CONTEXT)),
        KeyBinding::new("ctrl-alt-s", Command::SaveAll, Some(CONTEXT)),
        KeyBinding::new("ctrl-w", Command::CloseTab, Some(CONTEXT)),
        KeyBinding::new("ctrl-tab", Command::NextTab, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-tab", Command::PrevTab, Some(CONTEXT)),
        KeyBinding::new("ctrl-pagedown", Command::NextTab, Some(CONTEXT)),
        KeyBinding::new("ctrl-pageup", Command::PrevTab, Some(CONTEXT)),
        KeyBinding::new("ctrl-1", Command::SelectTab(0), Some(CONTEXT)),
        KeyBinding::new("ctrl-2", Command::SelectTab(1), Some(CONTEXT)),
        KeyBinding::new("ctrl-3", Command::SelectTab(2), Some(CONTEXT)),
        KeyBinding::new("ctrl-4", Command::SelectTab(3), Some(CONTEXT)),
        KeyBinding::new("ctrl-5", Command::SelectTab(4), Some(CONTEXT)),
        KeyBinding::new("ctrl-6", Command::SelectTab(5), Some(CONTEXT)),
        KeyBinding::new("ctrl-7", Command::SelectTab(6), Some(CONTEXT)),
        KeyBinding::new("ctrl-8", Command::SelectTab(7), Some(CONTEXT)),
        KeyBinding::new("ctrl-9", Command::SelectLastTab, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-w", Command::CloseWindow, Some(CONTEXT)),
        KeyBinding::new("ctrl-f", Command::Find, Some(CONTEXT)),
        KeyBinding::new("f3", Command::FindNext, Some(CONTEXT)),
        KeyBinding::new("shift-f3", Command::FindPrevious, Some(CONTEXT)),
        KeyBinding::new("ctrl-h", Command::Replace, Some(CONTEXT)),
        KeyBinding::new("ctrl-g", Command::GoTo, Some(CONTEXT)),
        KeyBinding::new("f5", Command::TimeDate, Some(CONTEXT)),
        KeyBinding::new("shift-f10", Command::ShowContextMenu, Some(CONTEXT)),
        KeyBinding::new("menu", Command::ShowContextMenu, Some(CONTEXT)),
        KeyBinding::new("ctrl-=", Command::ZoomIn, Some(CONTEXT)),
        KeyBinding::new("ctrl-+", Command::ZoomIn, Some(CONTEXT)),
        KeyBinding::new("ctrl--", Command::ZoomOut, Some(CONTEXT)),
        KeyBinding::new("ctrl-0", Command::ResetZoom, Some(CONTEXT)),
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

    fn set_word_wrap(&mut self, word_wrap: bool, window: &mut Window, cx: &mut Context<Self>) {
        for tab in &self.tabs {
            tab.data
                .editor
                .update(cx, |editor, cx| editor.set_soft_wrap(word_wrap, window, cx));
        }
        cx.notify();
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
    ) -> Option<CanvasNode> {
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
        let status = metrics.map(|metrics| UiNode::from(self.status_bar(metrics, cx)));
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
                    let measured_menu = menu_areas.bar_width();
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

    fn handle_command(
        &mut self,
        command: Command,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        match command {
            Command::Cancel => return self.cancel(window, cx),
            Command::ShowContextMenu => return self.show_context_menu(window, cx),
            Command::ZoomIn => self.set_zoom(self.zoom() + ZOOM_STEP, cx),
            Command::ZoomOut => self.set_zoom(self.zoom() - ZOOM_STEP, cx),
            Command::ResetZoom => self.set_zoom(1., cx),
            Command::CloseWindow => {
                if self.request_close_window(window, cx) {
                    window.remove_window();
                }
            }
            Command::NewTab => self.add_tab(window, cx),
            Command::CloseTab => self.close_tab(self.active, window, cx),
            Command::NextTab => self.next_tab(window, cx),
            Command::PrevTab => self.prev_tab(window, cx),
            Command::SelectTab(ix) => self.select_tab(ix, window, cx),
            Command::SelectLastTab => self.select_last_tab(window, cx),
            Command::Open => self.open(window, cx),
            Command::Save => self.save(window, cx),
            Command::SaveAs => self.save_as(window, cx),
            Command::SaveAll => self.save_all(window, cx),
            Command::TimeDate => self.insert_time_date(window, cx),
            Command::GoTo => self.go_to(window, cx),
            Command::Find => self.find(window, cx),
            Command::Replace => self.replace(window, cx),
            Command::FindNext => self.find_next(window, cx),
            Command::FindPrevious => self.find_previous(window, cx),
            Command::ToggleMatchCase => self.toggle_match_case(window, cx),
            Command::ToggleWrapAround => self.toggle_wrap_around(window, cx),
            Command::ReplaceNext => self.replace_next(window, cx),
            _ => return false,
        }
        true
    }

    fn insert_time_date(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let stamp = crate::win32::format_local_time_date();
        self.active_editor()
            .update(cx, |editor, cx| editor.replace(stamp, window, cx));
    }

    fn set_zoom(&mut self, zoom: f32, cx: &mut Context<Self>) {
        self.tabs[self.active].data.zoom =
            ((zoom / ZOOM_STEP).round() * ZOOM_STEP).clamp(ZOOM_MIN, ZOOM_MAX);
        cx.notify();
    }

    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if tab_drag::dragging(cx) {
            cx.defer(tab_drag::cancel);
            true
        } else if self.find.is_open() {
            self.close_find(window, cx);
            true
        } else {
            false
        }
    }

    fn next_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dialogs.is_open() || self.tabs.len() <= 1 {
            return;
        }
        let next = (self.active + 1) % self.tabs.len();
        self.activate_tab(next, window, cx);
    }

    fn prev_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dialogs.is_open() || self.tabs.len() <= 1 {
            return;
        }
        let prev = (self.active + self.tabs.len() - 1) % self.tabs.len();
        self.activate_tab(prev, window, cx);
    }

    fn select_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.dialogs.is_open() || ix >= self.tabs.len() || ix == self.active {
            return;
        }
        self.activate_tab(ix, window, cx);
    }

    fn select_last_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
    ) -> Div {
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

    fn render_menu_bar(&self, menu: Div, p: Palette) -> Div {
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
                            window.dispatch_command(Command::ToggleTheme, cx);
                        }),
                    ),
            )
            .child(separator(p))
    }

    fn show_context_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let editor = self.active_editor().clone();
        if !editor.focus_handle(cx).is_focused(window) {
            return false;
        }
        let Some(caret) = editor.read(cx).caret_bounds() else {
            return true;
        };
        let anchor = point(caret.right(), caret.bottom());
        self.open_context_menu(anchor, window, cx);
        true
    }

    fn editor(&self, find: Option<Div>, cx: &mut Context<Self>) -> Div {
        let p = palette(cx);
        let editor = self.active_editor().clone();
        let caret_height = self.caret_height();
        let caret_hidden = self.menu_bar.access_active();
        editor.update(cx, |state, _| {
            state.set_caret_height(caret_height);
            state.set_caret_hidden(caret_hidden);
        });
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

    fn status_bar(&self, metrics: StatusMetrics, cx: &App) -> Div {
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
                .whitespace_nowrap()
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

fn status_spacer(p: Palette) -> CanvasNode {
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl Into<UiNode> {
        let p = palette(cx);
        let status_metrics = cx
            .shared
            .view
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
            .on_command(cx.command_listener(Self::handle_command))
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
        cx.sync_system_reduce_motion();
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
        cx.bind_keys(key_bindings());
        cx.bind_keys(menu::key_bindings());
        cx.bind_keys(dialog::key_bindings());
        cx.bind_keys(find::key_bindings());

        cx.on_command(|command, cx| {
            match command {
                Command::Exit => cx.defer(|cx| {
                    for window in cx.windows() {
                        window
                            .update(cx, |_, window, cx| {
                                window.dispatch_command(Command::CloseWindow, cx);
                            })
                            .ok();
                    }
                }),
                Command::NewWindow => {
                    open_jot_window(None, None, cx);
                }
                Command::ToggleStatusBar => {
                    cx.shared.view.status_bar = !cx.shared.view.status_bar;
                    cx.refresh_windows();
                }
                Command::ToggleTheme => {
                    cx.set_window_appearance(Some(if palette(cx).dark {
                        WindowAppearance::Light
                    } else {
                        WindowAppearance::Dark
                    }));
                }
                Command::ToggleWordWrap => {
                    cx.shared.view.word_wrap = !cx.shared.view.word_wrap;
                    let word_wrap = cx.shared.view.word_wrap;
                    cx.defer(move |cx| apply_word_wrap_to_windows(word_wrap, cx));
                    cx.refresh_windows();
                }
                _ => return false,
            }
            true
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

#[cfg(test)]
mod ui_regression_tests {
    use super::*;

    struct TestHarness {
        app_cell: Rc<crate::ui::AppCell>,
        handle: WindowHandle<Jot>,
    }

    impl TestHarness {
        fn new() -> Self {
            let platform = Rc::new(
                crate::ui::WindowsPlatform::new().expect("WindowsPlatform::new failed"),
            );
            let app_cell = crate::ui::App::new_rc(platform);
            let handle = {
                let mut app = app_cell.inner.borrow_mut();
                // Settle transitions in 1 frame so hit-tests are deterministic without sleeps.
                app.set_reduce_motion(true);
                editor::init(&mut app);
                theme::apply(app.window_appearance(), &mut app);
                app.bind_keys(key_bindings());
                app.bind_keys(menu::key_bindings());
                app.bind_keys(dialog::key_bindings());
                app.bind_keys(find::key_bindings());
                app.on_command(|command, cx| {
                    match command {
                        Command::NewWindow => {
                            let mut opts = window_options(Some(size(px(800.), px(600.))), cx);
                            opts.show = false;
                            opts.focus = false;
                            let _ = cx.open_window(opts, |window, cx| {
                                cx.new(|cx| Jot::new(None, window, cx))
                            });
                        }
                        Command::ToggleStatusBar => {
                            cx.shared.view.status_bar = !cx.shared.view.status_bar;
                            cx.refresh_windows();
                        }
                        Command::ToggleTheme => {
                            cx.set_window_appearance(Some(if palette(cx).dark {
                                WindowAppearance::Light
                            } else {
                                WindowAppearance::Dark
                            }));
                        }
                        Command::ToggleWordWrap => {
                            cx.shared.view.word_wrap = !cx.shared.view.word_wrap;
                            let word_wrap = cx.shared.view.word_wrap;
                            cx.defer(move |cx| apply_word_wrap_to_windows(word_wrap, cx));
                            cx.refresh_windows();
                        }
                        _ => return false,
                    }
                    true
                });

                let mut opts = window_options(Some(size(px(1024.), px(700.))), &app);
                opts.show = false;
                opts.focus = false;
                let handle = app
                    .open_window(opts, |window, cx| cx.new(|cx| Jot::new(None, window, cx)))
                    .expect("open_window failed");
                app.flush_effects();
                handle
            };
            let mut harness = Self { app_cell, handle };
            harness.draw();
            harness
        }

        fn draw(&mut self) {
            for _ in 0..2 {
                {
                    let mut app = self.app_cell.inner.borrow_mut();
                    let _ = app.update_window_impl(self.handle.id, |root, window, cx| {
                        window.draw_frame(&root, cx);
                    });
                    app.flush_effects();
                }
                let fg = self.app_cell.inner.borrow().foreground.clone();
                fg.drain_ready();
                {
                    let mut app = self.app_cell.inner.borrow_mut();
                    app.flush_effects();
                }
            }
        }

        fn window_open(&self) -> bool {
            let app = self.app_cell.inner.borrow();
            app.windows
                .get(&self.handle.id)
                .and_then(Option::as_ref)
                .is_some()
        }

        fn read_jot<R>(&self, f: impl FnOnce(&Jot, &App) -> R) -> R {
            let app = self.app_cell.inner.borrow();
            let jot = self.handle.read(&app).expect("Jot window should be open");
            f(jot, &app)
        }

        fn element_bounds(&self, id: impl Into<ElementId>) -> Option<Bounds<Pixels>> {
            let id = id.into();
            let app = self.app_cell.inner.borrow();
            let win = app.windows.get(&self.handle.id)?.as_ref()?;
            win.test_element_bounds(&id)
        }

        fn move_mouse(&mut self, pos: Point<Pixels>) {
            {
                let mut app = self.app_cell.inner.borrow_mut();
                let _ = app.update_window_impl(self.handle.id, |_, window, cx| {
                    window.dispatch_mouse_move(
                        MouseMoveEvent {
                            position: pos,
                            pressed_button: None,
                            modifiers: Modifiers::default(),
                        },
                        cx,
                    );
                });
            }
            self.draw();
        }

        fn click_point_count(
            &mut self,
            pos: Point<Pixels>,
            button: MouseButton,
            click_count: usize,
        ) {
            self.move_mouse(pos);
            {
                let mut app = self.app_cell.inner.borrow_mut();
                let _ = app.update_window_impl(self.handle.id, |_, window, cx| {
                    window.dispatch_mouse_down(
                        MouseDownEvent {
                            button,
                            position: pos,
                            modifiers: Modifiers::default(),
                            click_count,
                        },
                        cx,
                    );
                });
            }
            self.draw();
            {
                let mut app = self.app_cell.inner.borrow_mut();
                let _ = app.update_window_impl(self.handle.id, |_, window, cx| {
                    window.dispatch_mouse_up(
                        MouseUpEvent {
                            button,
                            position: pos,
                            modifiers: Modifiers::default(),
                            click_count,
                        },
                        cx,
                    );
                });
            }
            self.draw();
        }

        fn click_point(&mut self, pos: Point<Pixels>, button: MouseButton) {
            self.click_point_count(pos, button, 1);
        }

        fn drag_points(&mut self, from: Point<Pixels>, to: Point<Pixels>) {
            self.move_mouse(from);
            {
                let mut app = self.app_cell.inner.borrow_mut();
                let _ = app.update_window_impl(self.handle.id, |_, window, cx| {
                    window.dispatch_mouse_down(
                        MouseDownEvent {
                            button: MouseButton::Left,
                            position: from,
                            modifiers: Modifiers::default(),
                            click_count: 1,
                        },
                        cx,
                    );
                });
            }
            self.draw();
            {
                let mut app = self.app_cell.inner.borrow_mut();
                let _ = app.update_window_impl(self.handle.id, |_, window, cx| {
                    window.dispatch_mouse_move(
                        MouseMoveEvent {
                            position: to,
                            pressed_button: Some(MouseButton::Left),
                            modifiers: Modifiers::default(),
                        },
                        cx,
                    );
                });
            }
            self.draw();
            {
                let mut app = self.app_cell.inner.borrow_mut();
                let _ = app.update_window_impl(self.handle.id, |_, window, cx| {
                    window.dispatch_mouse_up(
                        MouseUpEvent {
                            button: MouseButton::Left,
                            position: to,
                            modifiers: Modifiers::default(),
                            click_count: 1,
                        },
                        cx,
                    );
                });
            }
            self.draw();
        }

        fn click_id(&mut self, id: impl Into<ElementId>) {
            let id = id.into();
            let bounds = self
                .element_bounds(id.clone())
                .unwrap_or_else(|| panic!("element {id:?} was not laid out"));
            assert!(
                !bounds.is_empty(),
                "element {id:?} laid out with empty bounds {bounds:?}"
            );
            self.click_point(bounds.center(), MouseButton::Left);
        }

        fn middle_click_id(&mut self, id: impl Into<ElementId>) {
            let id = id.into();
            let bounds = self
                .element_bounds(id.clone())
                .unwrap_or_else(|| panic!("element {id:?} was not laid out"));
            self.click_point(bounds.center(), MouseButton::Middle);
        }

        fn hover_id(&mut self, id: impl Into<ElementId>) {
            let id = id.into();
            let bounds = self
                .element_bounds(id.clone())
                .unwrap_or_else(|| panic!("element {id:?} was not laid out"));
            self.move_mouse(bounds.center());
        }

        fn press_key(&mut self, spec: &str) {
            let keystroke = Keystroke::parse(spec);
            {
                let mut app = self.app_cell.inner.borrow_mut();
                let _ = app.update_window_impl(self.handle.id, |_, window, cx| {
                    window.dispatch_key_down(
                        KeyDownEvent {
                            keystroke,
                            is_held: false,
                        },
                        cx,
                    );
                });
            }
            self.draw();
        }

        fn type_text(&mut self, text: &str) {
            {
                let mut app = self.app_cell.inner.borrow_mut();
                let _ = app.update_window_impl(self.handle.id, |_, window, cx| {
                    window.dispatch_text_input(text, cx);
                });
            }
            self.draw();
        }

        fn scroll_wheel(&mut self, pos: Point<Pixels>, lines_y: f32, modifiers: Modifiers) {
            {
                let mut app = self.app_cell.inner.borrow_mut();
                let _ = app.update_window_impl(self.handle.id, |_, window, cx| {
                    window.dispatch_scroll_wheel(
                        ScrollWheelEvent {
                            position: pos,
                            delta: ScrollDelta::Lines(point(0.0, lines_y)),
                            modifiers,
                        },
                        cx,
                    );
                });
            }
            self.draw();
        }

        fn close_if_open(&mut self) {
            let mut app = self.app_cell.inner.borrow_mut();
            app.remove_window_by_id(self.handle.id);
        }
    }

    impl Drop for TestHarness {
        fn drop(&mut self) {
            self.close_if_open();
        }
    }

    #[test]
    fn tab_close_button_new_tab_hover_and_middle_click() {
        let mut h = TestHarness::new();

        // Add two tabs via the real '+' button.
        h.click_id("new-tab");
        h.click_id("new-tab");
        let ids: Vec<EntityId> = h.read_jot(|j, _| {
            assert_eq!(j.tabs.len(), 3);
            assert_eq!(j.active, 2);
            j.tabs.iter().map(|t| t.data.editor.entity_id()).collect()
        });

        // Click tab 0 to switch to it, then click its 'x' close button.
        h.click_id(("tab", ids[0]));
        h.read_jot(|j, _| assert_eq!(j.active, 0));
        h.click_id(("close-tab", ids[0]));
        h.read_jot(|j, cx| {
            assert_eq!(j.tabs.len(), 2, "clicking active tab 'x' must close tab");
            assert!(j.tab_index(ids[0]).is_none());
            assert!(
                !tab_drag::active(cx),
                "tab close button must stop_propagation so parent tab drag does not start"
            );
        });

        // Hover inactive tab (ids[2]) so its 'x' appears, then click 'x' to close it.
        h.hover_id(("tab", ids[2]));
        assert!(
            h.element_bounds(("close-tab", ids[2])).is_some(),
            "hovering inactive tab must reveal its 'x' button"
        );
        h.click_id(("close-tab", ids[2]));
        h.read_jot(|j, _| {
            assert_eq!(j.tabs.len(), 1, "clicking hovered inactive tab 'x' must close tab");
            assert_eq!(j.tabs[0].data.editor.entity_id(), ids[1]);
        });

        // Add a tab and close it with middle-click on its tab face.
        h.click_id("new-tab");
        let second_id = h.read_jot(|j, _| {
            assert_eq!(j.tabs.len(), 2);
            j.tabs[1].data.editor.entity_id()
        });
        h.middle_click_id(("tab-face", second_id));
        h.read_jot(|j, _| assert_eq!(j.tabs.len(), 1, "middle-clicking tab must close it"));

        // Dirty the sole remaining tab and click 'x' -> discard prompt opens -> click "Don't save" -> closes window.
        h.type_text("unsaved text");
        let last_id = h.read_jot(|j, _| j.tabs[0].data.editor.entity_id());
        h.hover_id(("tab", last_id));
        h.click_id(("close-tab", last_id));
        h.read_jot(|j, _| {
            assert!(
                j.dialogs.is_open(),
                "closing dirty tab via 'x' must open save confirmation dialog"
            );
        });
        h.click_id(("button", 1)); // "Don't save"
        assert!(
            !h.window_open(),
            "confirming Don't save on last tab must close the window"
        );

        // Also verify closing a clean sole tab via 'x' immediately closes the window.
        let mut h2 = TestHarness::new();
        let sole_id = h2.read_jot(|j, _| j.tabs[0].data.editor.entity_id());
        h2.click_id(("close-tab", sole_id));
        assert!(
            !h2.window_open(),
            "clicking 'x' on sole clean tab must close the window"
        );
    }

    #[test]
    fn menus_shortcuts_toggles_submenus_and_context_menu() {
        let mut h = TestHarness::new();

        // Verify shortcut hints resolve even while AppCell is mutably borrowed.
        {
            let mut app = h.app_cell.inner.borrow_mut();
            app.update_window_impl(h.handle.id, |root, window, cx| {
                let jot = root.downcast::<Jot>().unwrap();
                let focus = jot.read(cx).active_editor().focus_handle(cx);
                assert!(window.shortcut_for_command_in(Command::Save, &focus).is_some());
                assert!(window.shortcut_for_command_in(Command::NewTab, &focus).is_some());
            })
            .unwrap();
        }

        // Open File menu -> click "New tab" (row 0).
        h.click_id("File");
        h.read_jot(|j, _| assert!(j.menu_bar.is_open()));
        h.click_id(("menu-row", 0));
        h.read_jot(|j, _| {
            assert_eq!(j.tabs.len(), 2, "File -> New tab must add a tab");
            assert!(!j.menu_bar.is_open(), "menu must close after activation");
        });

        // Open View menu -> open Zoom submenu (row 0) -> click "Zoom in" (submenu row 0).
        h.click_id("View");
        h.click_id(("menu-row", 0));
        h.click_id(("menu-row", 0));
        h.read_jot(|j, _| {
            assert_eq!(
                (j.zoom() * 100.).round() as u32,
                110,
                "View -> Zoom -> Zoom in must zoom to 110%"
            );
        });

        // Open View menu -> toggle Word wrap (row 2) and Status bar (row 1).
        h.click_id("View");
        h.click_id(("menu-row", 2));
        h.read_jot(|_, cx| assert!(!cx.shared.view.word_wrap));

        h.click_id("View");
        h.click_id(("menu-row", 1));
        h.read_jot(|_, cx| assert!(!cx.shared.view.status_bar));

        // Open File menu -> hover Edit trigger -> switches open menu; click editor -> closes menu.
        h.click_id("File");
        h.hover_id("Edit");
        h.read_jot(|j, _| assert!(j.menu_bar.is_open()));
        let editor_bounds = h.element_bounds("editor-area").unwrap();
        h.click_point(editor_bounds.center(), MouseButton::Left);
        h.read_jot(|j, _| assert!(!j.menu_bar.is_open(), "clicking outside menu must close it"));

        // Alt activates access key mode; pressing 'f' opens File menu; Down+Escape navigates & closes.
        h.press_key("alt");
        h.read_jot(|j, _| assert!(j.menu_bar.access_active()));
        h.press_key("f");
        h.read_jot(|j, _| assert!(j.menu_bar.is_open(), "Alt then F must open File menu"));
        h.press_key("down");
        h.press_key("escape");
        h.press_key("escape");
        h.read_jot(|j, _| {
            assert!(!j.menu_bar.is_open());
            assert!(!j.menu_bar.access_active());
        });

        // Right-click on selected text opens context menu with Delete enabled; clicking Delete erases it.
        h.type_text("delete me");
        h.press_key("ctrl-a");
        let on_text = point(
            editor_bounds.left() + px(EDITOR_PADDING_LEFT + 12.0),
            editor_bounds.top() + px(EDITOR_PADDING_TOP + 10.0),
        );
        h.click_point(on_text, MouseButton::Right);
        h.read_jot(|j, _| assert!(j.menu_bar.is_open(), "right-click must open context menu"));
        h.click_id(("menu-command", 4)); // Delete command button
        h.read_jot(|j, cx| {
            assert!(!j.menu_bar.is_open());
            assert_eq!(
                j.active_editor().read(cx).text().len(),
                0,
                "context menu Delete command must erase selected text"
            );
        });
    }

    #[test]
    fn find_replace_bar_clear_button_and_key_contexts() {
        let mut h = TestHarness::new();
        h.type_text("alpha beta alpha gamma");

        // Ctrl+F opens Find bar and focuses query input.
        h.press_key("ctrl-f");
        h.read_jot(|j, _| assert!(j.find.is_open()));

        // Type query, press Enter -> FindNext selects first "alpha"; Enter again -> second "alpha";
        // Shift+Enter -> back to first "alpha".
        h.type_text("alpha");
        h.press_key("enter");
        h.read_jot(|j, cx| {
            assert_eq!(j.active_editor().read(cx).selected_range(), 0..5);
        });
        h.press_key("enter");
        h.read_jot(|j, cx| {
            assert_eq!(j.active_editor().read(cx).selected_range(), 11..16);
        });
        h.press_key("shift-enter");
        h.read_jot(|j, cx| {
            assert_eq!(j.active_editor().read(cx).selected_range(), 0..5);
        });

        // Click the inline clear 'x' button inside the Find query field (has on_mouse_down
        // prevent_default + stop_propagation AND on_click) -> clears the query field!
        let clear_bounds = {
            let app = h.app_cell.inner.borrow();
            let win = app.windows.get(&h.handle.id).unwrap().as_ref().unwrap();
            win.find_test_element_bounds(|id| {
                matches!(id, ElementId::NamedInteger(name, _) if name.as_str() == "field-clear")
            })
            .expect("find query clear 'x' button must be laid out when query is non-empty")
        };
        h.click_point(clear_bounds.center(), MouseButton::Left);
        {
            let app = h.app_cell.inner.borrow();
            let win = app.windows.get(&h.handle.id).unwrap().as_ref().unwrap();
            assert!(
                win.find_test_element_bounds(|id| {
                    matches!(id, ElementId::NamedInteger(name, _) if name.as_str() == "field-clear")
                })
                .is_none(),
                "clicking field-clear 'x' must empty query so clear button disappears"
            );
        }

        // Re-type query, open Find options flyout -> toggle Match case on & off.
        h.type_text("ALPHA");
        h.click_id("find-options");
        h.read_jot(|j, _| assert!(j.menu_bar.is_open(), "find-options must open flyout"));
        h.click_id(("menu-row", 0)); // Match case toggle ON
        h.click_id("find-options");
        h.click_id(("menu-row", 0)); // Match case toggle OFF

        // Click "find-expand" -> click replacement text field (tests click-to-focus via
        // `track_focus`!) -> type "OMEGA" -> click find-next -> press Enter (`JotFindReplacement`
        // -> ReplaceNext!) -> then click "Replace all"!
        h.click_id("find-expand");
        let replace_btn_bounds = h
            .element_bounds("find-replace")
            .expect("Replace button must be laid out after expanding replace row");
        let query_clear_bounds = {
            let app = h.app_cell.inner.borrow();
            let win = app.windows.get(&h.handle.id).unwrap().as_ref().unwrap();
            win.find_test_element_bounds(|id| {
                matches!(id, ElementId::NamedInteger(name, _) if name.as_str() == "field-clear")
            })
            .expect("query field clear button must be laid out")
        };
        // Replacement input sits directly to the left of the "Replace" button, horizontally aligned
        // with the query row above it.
        let replacement_input_pt = point(
            query_clear_bounds.left() - px(40.0),
            replace_btn_bounds.center().y,
        );
        h.click_point(replacement_input_pt, MouseButton::Left);
        h.type_text("OMEGA");
        // Verify typing went into the replacement field (not the query field or main editor).
        h.read_jot(|j, cx| {
            assert_eq!(j.active_editor().read(cx).value().as_str(), "alpha beta alpha gamma");
        });
        // Click "find-next" (icon_button calls prevent_default so focus stays on replacement input!)
        // and press Enter -> ReplaceNext via JotFindReplacement key context!
        h.click_id("find-next");
        h.press_key("enter");
        h.read_jot(|j, cx| {
            assert_eq!(
                j.active_editor().read(cx).value().as_str(),
                "alpha beta OMEGA gamma",
                "Enter in replacement field must replace current match via JotFindReplacement"
            );
        });
        h.click_id("find-replace-all");
        h.read_jot(|j, cx| {
            assert_eq!(
                j.active_editor().read(cx).value().as_str(),
                "OMEGA beta OMEGA gamma",
                "Replace all must replace remaining matches"
            );
        });

        // Clicking inside the main editor while Find bar is open must transfer focus back to the
        // main editor so subsequent typing edits the document!
        let ed_bounds = h.element_bounds("editor-area").unwrap();
        let ed_click = point(
            ed_bounds.left() + px(EDITOR_PADDING_LEFT + 5.0),
            ed_bounds.bottom() - px(40.0),
        );
        h.click_point(ed_click, MouseButton::Left);
        h.type_text("!");
        h.read_jot(|j, cx| {
            assert_eq!(
                j.active_editor().read(cx).value().as_str(),
                "OMEGA beta OMEGA gamma!",
                "clicking main editor while Find bar is open must focus main editor"
            );
        });

        // Click "find-close" 'x' button -> closes Find bar cleanly.
        h.click_id("find-close");
        h.read_jot(|j, _| assert!(!j.find.is_open(), "find-close 'x' must close Find bar"));
    }

    #[test]
    fn go_to_dialog_navigation_and_out_of_range_flow() {
        let mut h = TestHarness::new();
        h.type_text("one");
        h.press_key("enter");
        h.type_text("two");
        h.press_key("enter");
        h.type_text("three");
        h.press_key("enter");
        h.type_text("four");

        // Ctrl+G -> type "2" -> Enter jumps cursor to line 2 (row 1).
        h.press_key("ctrl-g");
        h.read_jot(|j, _| assert!(j.dialogs.is_open()));
        h.type_text("2");
        h.press_key("enter");
        h.read_jot(|j, cx| {
            assert!(!j.dialogs.is_open());
            let ed = j.active_editor().read(cx);
            assert_eq!(ed.text().offset_to_point(ed.cursor()).row, 1);
        });

        // Out-of-range line -> shows error dialog -> OK returns to Go To -> Escape closes.
        h.press_key("ctrl-g");
        h.type_text("999");
        h.press_key("enter");
        h.read_jot(|j, _| assert!(j.dialogs.is_open()));
        h.click_id(("button", 0)); // OK -> returns to Go To
        h.read_jot(|j, _| assert!(j.dialogs.is_open()));
        h.press_key("tab"); // cycle focus to button
        h.press_key("shift-tab"); // cycle back to input
        h.press_key("escape");
        h.read_jot(|j, _| assert!(!j.dialogs.is_open()));
    }

    #[test]
    fn theme_zoom_editing_undo_scrollbar_and_caption_hit_tests() {
        let mut h = TestHarness::new();

        // Theme button click toggles dark/light and back.
        let initial_dark = h.read_jot(|_, cx| palette(cx).dark);
        h.click_id("theme");
        h.read_jot(|_, cx| assert_eq!(palette(cx).dark, !initial_dark));
        h.click_id("theme");
        h.read_jot(|_, cx| assert_eq!(palette(cx).dark, initial_dark));

        // Zoom via keyboard and Ctrl+wheel.
        h.press_key("ctrl-=");
        h.read_jot(|j, _| assert_eq!((j.zoom() * 100.).round() as u32, 110));
        h.press_key("ctrl--");
        h.read_jot(|j, _| assert_eq!((j.zoom() * 100.).round() as u32, 100));
        let editor_center = h.element_bounds("editor-area").unwrap().center();
        h.scroll_wheel(editor_center, 3.0, Modifiers::control());
        h.read_jot(|j, _| assert_eq!((j.zoom() * 100.).round() as u32, 110));
        h.press_key("ctrl-0");
        h.read_jot(|j, _| assert_eq!((j.zoom() * 100.).round() as u32, 100));

        // Typing, Select All, Undo, Redo, Backspace.
        h.type_text("hello world");
        h.press_key("ctrl-a");
        h.read_jot(|j, cx| assert_eq!(j.active_editor().read(cx).selected_range(), 0..11));
        h.press_key("backspace");
        h.read_jot(|j, cx| assert_eq!(j.active_editor().read(cx).text().len(), 0));
        h.press_key("ctrl-z");
        h.read_jot(|j, cx| assert_eq!(j.active_editor().read(cx).value().as_str(), "hello world"));
        h.press_key("ctrl-y");
        h.read_jot(|j, cx| assert_eq!(j.active_editor().read(cx).text().len(), 0));

        // Scroll wheel scrolls multi-line document, activates Scrollbar, and scrollbar track
        // click scrolls without moving the editor caret/selection.
        let big: String = (0..120).map(|i| format!("Line {i}\n")).collect();
        h.type_text(&big);
        h.press_key("ctrl-home");
        h.scroll_wheel(editor_center, -6.0, Modifiers::default());
        let after_wheel_y = h.read_jot(|j, cx| j.active_editor().read(cx).scroll_handle.offset().y);
        assert!(
            after_wheel_y < px(0.0),
            "scrolling down with mouse wheel must scroll editor viewport"
        );
        let ed_bounds = h.element_bounds("editor-area").unwrap();
        let track_pt = point(ed_bounds.right() - px(6.0), ed_bounds.bottom() - px(40.0));
        h.click_point(track_pt, MouseButton::Left);
        h.read_jot(|j, cx| {
            let ed = j.active_editor().read(cx);
            assert!(
                ed.scroll_handle.offset().y < after_wheel_y,
                "clicking vertical scrollbar track must jump scroll offset further down"
            );
            assert!(
                ed.selected_range().is_empty(),
                "clicking scrollbar must stop_propagation and not select editor text"
            );
        });

        // Double-click word selection, triple-click line selection, and drag selection.
        h.press_key("ctrl-a");
        h.press_key("backspace");
        h.type_text("alpha beta gamma");
        let first_char = point(
            ed_bounds.left() + px(EDITOR_PADDING_LEFT + 12.0),
            ed_bounds.top() + px(EDITOR_PADDING_TOP + 10.0),
        );
        h.click_point_count(first_char, MouseButton::Left, 2);
        h.read_jot(|j, cx| {
            assert_eq!(
                j.active_editor().read(cx).selected_range(),
                0..5,
                "double-click must select word"
            );
        });
        h.click_point_count(first_char, MouseButton::Left, 3);
        h.read_jot(|j, cx| {
            assert_eq!(
                j.active_editor().read(cx).selected_range(),
                0..16,
                "triple-click must select full line"
            );
        });
        let drag_start = point(
            ed_bounds.left() + px(EDITOR_PADDING_LEFT + 1.0),
            ed_bounds.top() + px(EDITOR_PADDING_TOP + 10.0),
        );
        let drag_end = point(
            ed_bounds.left() + px(EDITOR_PADDING_LEFT + 70.0),
            ed_bounds.top() + px(EDITOR_PADDING_TOP + 10.0),
        );
        h.drag_points(drag_start, drag_end);
        h.read_jot(|j, cx| {
            let sel = j.active_editor().read(cx).selected_range();
            assert!(!sel.is_empty(), "dragging in editor must create a text selection");
        });

        // Caption button hit-testing resolves Min / Max / Close areas cleanly.
        let close_pt = h.element_bounds("caption-close").unwrap().center();
        let max_pt = h.element_bounds("caption-maximize").unwrap().center();
        let min_pt = h.element_bounds("caption-minimize").unwrap().center();
        let app = h.app_cell.inner.borrow();
        let win = app.windows.get(&h.handle.id).unwrap().as_ref().unwrap();
        assert_eq!(win.hit_test_control_area(close_pt), Some(WindowControlArea::Close));
        assert_eq!(win.hit_test_control_area(max_pt), Some(WindowControlArea::Max));
        assert_eq!(win.hit_test_control_area(min_pt), Some(WindowControlArea::Min));
    }

    #[test]
    fn dirty_window_close_file_drop_and_multi_window() {
        let mut h = TestHarness::new();

        // File drop opens a real file from disk into the blank initial tab.
        let tmp = std::env::temp_dir().join(format!("jot-drop-test-{}.txt", std::process::id()));
        std::fs::write(&tmp, "dropped content 123").unwrap();
        let ed_center = h.element_bounds("editor-area").unwrap().center();
        {
            let mut app = h.app_cell.inner.borrow_mut();
            let _ = app.update_window_impl(h.handle.id, |_, window, cx| {
                window.dispatch_file_drop(ExternalPaths(vec![tmp.clone()]), ed_center, cx);
            });
        }
        // Pump foreground until background load delivers the file.
        for _ in 0..40 {
            h.draw();
            if h.read_jot(|j, cx| j.active_editor().read(cx).value().as_str() == "dropped content 123") {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        h.read_jot(|j, cx| {
            assert_eq!(j.active_editor().read(cx).value().as_str(), "dropped content 123");
            assert!(j.tabs[0].data.path.is_some());
        });
        let _ = std::fs::remove_file(&tmp);

        // Multi-window via Ctrl+Shift+N creates a second window cleanly.
        h.press_key("ctrl-shift-n");
        let extra_id = {
            let app = h.app_cell.inner.borrow();
            assert_eq!(app.window_order.len(), 2, "Ctrl+Shift+N must open a 2nd window");
            app.window_order[1]
        };
        {
            let mut app = h.app_cell.inner.borrow_mut();
            app.remove_window_by_id(extra_id);
            assert_eq!(app.window_order.len(), 1);
        }

        // Dirty window close prompt: Cancel keeps window open, Don't save closes it.
        h.type_text("!");
        let allow_close = {
            let mut app = h.app_cell.inner.borrow_mut();
            app.update_window_impl(h.handle.id, |_, window, cx| window.check_should_close(cx))
                .unwrap()
        };
        assert!(!allow_close, "dirty window must intercept close and show prompt");
        h.draw();
        h.read_jot(|j, _| assert!(j.dialogs.is_open()));
        h.click_id(("button", 2)); // Cancel
        assert!(h.window_open(), "Cancel must keep window open");

        let allow_close = {
            let mut app = h.app_cell.inner.borrow_mut();
            app.update_window_impl(h.handle.id, |_, window, cx| window.check_should_close(cx))
                .unwrap()
        };
        assert!(!allow_close);
        h.draw();
        h.click_id(("button", 1)); // Don't save
        assert!(!h.window_open(), "Don't save must close the window");
    }

    #[test]
    fn overlay_window_resizes_from_1x1_and_renders() {
        let h = TestHarness::new();
        let overlay = {
            let mut app = h.app_cell.inner.borrow_mut();
            overlay::open(|_| TooltipTestView, &mut app).expect("overlay::open failed")
        };
        overlay::place(
            overlay.hwnd,
            ScreenRect {
                left: 10,
                top: 10,
                right: 170,
                bottom: 50,
            },
        );
        {
            let mut app = h.app_cell.inner.borrow_mut();
            app.update_window_impl(overlay.handle.window_id(), |root, window, cx| {
                window.draw_frame(&root, cx);
                assert!(
                    window.viewport_size().width > px(10.0)
                        && window.viewport_size().height > px(10.0),
                    "overlay must sync resized client rect before drawing"
                );
            })
            .unwrap();
            app.remove_window_by_id(overlay.handle.window_id());
        }
    }

    struct TooltipTestView;
    impl Render for TooltipTestView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl Into<UiNode> {
            div().size_full().bg(rgba(0x202020ff)).child("Tip")
        }
    }

    #[test]
    fn status_bar_min_width_and_no_vertical_char_wrapping() {
        use windows::Win32::{
            Foundation::{LPARAM, WPARAM},
            UI::WindowsAndMessaging::{MINMAXINFO, SendMessageW, WM_GETMINMAXINFO},
        };

        let mut h = TestHarness::new();
        h.type_text("a");

        let (hwnd, min_size) = h.read_jot(|j, _| {
            let cached = j
                .min_size
                .get()
                .expect("min_size_probe must populate min_size cache on first draw");
            (j.hwnd, cached.size)
        });
        assert!(
            min_size.width >= px(420.0),
            "status bar min width must reflect unwrapped single-line labels, got {:?}",
            min_size.width
        );

        // Real WM_GETMINMAXINFO on the window HWND must enforce ptMinTrackSize >= min_size.
        let mut mmi = MINMAXINFO::default();
        unsafe {
            let _ = SendMessageW(
                hwnd.raw(),
                WM_GETMINMAXINFO,
                Some(WPARAM(0)),
                Some(LPARAM((&raw mut mmi) as isize)),
            );
        }
        let scale = {
            let app = h.app_cell.inner.borrow();
            app.windows
                .get(&h.handle.id)
                .unwrap()
                .as_ref()
                .unwrap()
                .scale_factor()
        };
        assert!(
            mmi.ptMinTrackSize.x >= (min_size.width.as_f32() * scale).ceil() as i32,
            "WM_GETMINMAXINFO ptMinTrackSize.x ({}) must enforce min_size.width ({:?})",
            mmi.ptMinTrackSize.x,
            min_size.width
        );

        // Even when laid out at exact min width OR forced narrower than min width (e.g. toggling
        // Status Bar on while squished), every status bar label must remain single-line.
        let mut app = h.app_cell.inner.borrow_mut();
        app.update_window_impl(h.handle.id, |_, window, cx| {
            let p = palette(cx);
            for test_width in [min_size.width, px(220.0)] {
                let mut bar: UiNode = div()
                    .w(test_width)
                    .flex()
                    .items_center()
                    .gap(px(24.))
                    .h(px(STATUS_BAR_HEIGHT))
                    .px(px(22.))
                    .font_family(SMALL_FONT)
                    .text_size(px(12.))
                    .text_color(rgba(p.text_secondary))
                    .child(div().id("sb-pos").child("Ln 1, Col 1"))
                    .child(status_divider(p))
                    .child(div().id("sb-chars").child("1 character"))
                    .child(status_spacer(p))
                    .child(div().id("sb-zoom").child("100%"))
                    .child(status_divider(p))
                    .child(div().id("sb-eol").child("Windows (CRLF)"))
                    .child(status_divider(p))
                    .child(div().id("sb-enc").child("UTF-8"))
                    .into();
                let root_lid = bar.request_layout(window, cx);
                window.compute_layout(
                    root_lid,
                    size(
                        AvailableSpace::Definite(test_width),
                        AvailableSpace::Definite(px(STATUS_BAR_HEIGHT)),
                    ),
                    cx,
                );
                bar.prepaint(window, cx);
                for label_id in ["sb-pos", "sb-chars", "sb-zoom", "sb-eol", "sb-enc"] {
                    let b = window
                        .frame_element_bounds(&ElementId::from(label_id))
                        .expect("status bar label bounds missing");
                    assert!(
                        b.size.height <= px(STATUS_BAR_HEIGHT),
                        "label {label_id} wrapped vertically at width {test_width:?}: bounds={b:?}"
                    );
                    assert!(
                        b.size.width >= px(25.0),
                        "label {label_id} squished below single-line width at {test_width:?}: bounds={b:?}"
                    );
                }
            }

            // Constrained definite-width text (e.g. dialog body) must still wrap across lines.
            let mut dialog_body: UiNode = div()
                .flex()
                .flex_col()
                .w(px(180.0))
                .font_family(TEXT_FONT)
                .text_size(px(14.0))
                .line_height(px(20.0))
                .child(
                    div()
                        .id("dlg-msg")
                        .child("Do you want to save changes to a very long untitled document?"),
                )
                .into();
            let dlid = dialog_body.request_layout(window, cx);
            window.compute_layout(
                dlid,
                size(AvailableSpace::Definite(px(180.0)), AvailableSpace::MaxContent),
                cx,
            );
            dialog_body.prepaint(window, cx);
            let msg_bounds = window
                .frame_element_bounds(&ElementId::from("dlg-msg"))
                .expect("dialog msg bounds missing");
            assert!(
                msg_bounds.size.height >= px(40.0),
                "constrained dialog text must still wrap across multiple lines: bounds={msg_bounds:?}"
            );
        })
        .unwrap();
    }

    #[test]
    fn hwnd_registry_cleanup_reentrant_queue_and_modal_size_move_timer() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        use windows::Win32::{
            Foundation::{LPARAM, WPARAM},
            UI::WindowsAndMessaging::{
                SendMessageW, WM_ACTIVATE, WM_ENTERSIZEMOVE, WM_EXITSIZEMOVE, WM_TIMER,
            },
        };

        let h = TestHarness::new();
        let hwnd = h.read_jot(|j, _| j.hwnd);
        assert!(
            crate::ui::platform::window::has_hwnd_entry(hwnd),
            "open window must be registered in HWND_REGISTRY"
        );

        // Bind ACTIVE_APP for direct SendMessageW testing, then restore on exit.
        crate::ui::platform::ACTIVE_APP.with(|s| *s.borrow_mut() = Some(h.app_cell.clone()));

        // 1. Re-entrant Win32 message while AppCell is mutably borrowed must be queued, not lost.
        {
            let _borrow = h.app_cell.inner.borrow_mut();
            unsafe {
                let _ = SendMessageW(
                    hwnd.raw(),
                    WM_ACTIVATE,
                    Some(WPARAM(1)),
                    Some(LPARAM(0)),
                );
            }
        }
        crate::ui::platform::drive_frames(&h.app_cell);
        {
            let app = h.app_cell.inner.borrow();
            let win = app.windows.get(&h.handle.id).unwrap().as_ref().unwrap();
            assert!(
                win.is_window_active(),
                "queued re-entrant WM_ACTIVATE must apply once drive_frames drains it"
            );
        }

        // 2. Modal size/move loop timer pumps foreground tasks while dragging edges.
        let ran = Arc::new(AtomicBool::new(false));
        {
            let ran_flag = ran.clone();
            let app = h.app_cell.inner.borrow();
            app.foreground
                .spawn(async move {
                    ran_flag.store(true, Ordering::SeqCst);
                })
                .detach();
        }
        unsafe {
            let _ = SendMessageW(
                hwnd.raw(),
                WM_ENTERSIZEMOVE,
                Some(WPARAM(0)),
                Some(LPARAM(0)),
            );
        }
        {
            let app = h.app_cell.inner.borrow();
            let win = app.windows.get(&h.handle.id).unwrap().as_ref().unwrap();
            assert!(win.native.in_size_move);
        }
        unsafe {
            let _ = SendMessageW(hwnd.raw(), WM_TIMER, Some(WPARAM(1)), Some(LPARAM(0)));
        }
        assert!(
            ran.load(Ordering::SeqCst),
            "WM_TIMER during modal size/move must drain ready foreground tasks"
        );
        unsafe {
            let _ = SendMessageW(
                hwnd.raw(),
                WM_EXITSIZEMOVE,
                Some(WPARAM(0)),
                Some(LPARAM(0)),
            );
        }
        {
            let app = h.app_cell.inner.borrow();
            let win = app.windows.get(&h.handle.id).unwrap().as_ref().unwrap();
            assert!(!win.native.in_size_move);
        }

        // 3. Closing window cleans up HWND_REGISTRY atomically.
        {
            let mut app = h.app_cell.inner.borrow_mut();
            app.remove_window_by_id(h.handle.id);
        }
        crate::ui::platform::ACTIVE_APP.with(|s| *s.borrow_mut() = None);
        assert!(
            !crate::ui::platform::window::has_hwnd_entry(hwnd),
            "closing window must remove entry from HWND_REGISTRY"
        );
    }

    #[test]
    fn real_win32_message_loop_wakes_timers_and_quits_cleanly() {
        let platform = Rc::new(
            crate::ui::WindowsPlatform::new().expect("WindowsPlatform::new failed"),
        );
        Application::with_platform(platform).run(|cx| {
            cx.sync_system_reduce_motion();
            editor::init(cx);
            theme::apply(cx.window_appearance(), cx);
            let mut opts = window_options(None, cx);
            opts.show = false;
            opts.focus = false;
            let handle = cx
                .open_window(opts, |window, cx| cx.new(|cx| Jot::new(None, window, cx)))
                .expect("open_window in Application::run failed");
            cx.on_window_closed(|cx, _| cx.quit()).detach();
            cx.spawn(async move |cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(25))
                    .await;
                cx.update(|cx| {
                    cx.remove_window_by_id(handle.id);
                });
            })
            .detach();
        });
    }

    #[test]
    fn window_anim_tracks_prune_unused_tracks_and_word_wrap_updates_tabs() {
        let mut h = TestHarness::new();
        let baseline_tracks = {
            let app = h.app_cell.inner.borrow();
            app.windows
                .get(&h.handle.id)
                .and_then(Option::as_ref)
                .unwrap()
                .anim_track_count()
        };

        // Opening Go To dialog creates its per-window animation tracks (zero Entities).
        h.press_key("ctrl-g");
        let open_tracks = {
            let app = h.app_cell.inner.borrow();
            app.windows
                .get(&h.handle.id)
                .and_then(Option::as_ref)
                .unwrap()
                .anim_track_count()
        };
        assert!(
            open_tracks > baseline_tracks,
            "dialog open must allocate anim tracks on Window ({open_tracks} > {baseline_tracks})"
        );

        // Closing the dialog prunes its animation tracks on the following settled frame.
        h.press_key("escape");
        h.draw();
        let closed_tracks = {
            let app = h.app_cell.inner.borrow();
            app.windows
                .get(&h.handle.id)
                .and_then(Option::as_ref)
                .unwrap()
                .anim_track_count()
        };
        assert_eq!(
            closed_tracks, baseline_tracks,
            "closing dialog must prune unused anim tracks back to baseline"
        );

        // Toggling word wrap via typed AppShared updates all open tabs directly.
        h.press_key("ctrl-n");
        h.read_jot(|j, cx| {
            assert_eq!(j.tabs.len(), 2);
            assert!(cx.shared.view.word_wrap);
            for tab in &j.tabs {
                assert!(tab.data.editor.read(cx).soft_wrap_enabled());
            }
        });
        h.click_id("View");
        h.click_id(("menu-row", 2));
        h.read_jot(|j, cx| {
            assert!(!cx.shared.view.word_wrap);
            for tab in &j.tabs {
                assert!(!tab.data.editor.read(cx).soft_wrap_enabled());
            }
        });
    }
}
