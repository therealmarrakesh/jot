#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use gpui_kit::base::actions::{SelectLeft, SelectRight};
use gpui_kit::component::input::{self, RopeExt as _, Textarea, TextareaState};
use gpui_kit::component::menu::PopupMenu;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::ops::Range;
use std::time::Duration;
use windows_sys::Win32::UI::WindowsAndMessaging::GetCaretBlinkTime;

const LAYER_ON_MICA: u32 = 0xffffffb3;
const STROKE: u32 = 0x0000000f;
const STROKE_ON_LAYER: u32 = 0xeaeaeab8;
const EDITOR_BACKGROUND: u32 = 0xf9f9f9;
const TEXT_PRIMARY: u32 = 0x000000bc;
const TEXT_SECONDARY: u32 = 0x0000007a;
const UI_FONT: &str = "Segoe UI Variable Small";
const MENU_FONT: &str = "Segoe UI Variable Text";
const MENU_ITEM_ACTIVE: u32 = 0x00000005;

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

const EDITOR_FONT_SIZE: f32 = 11. * 96. / 72.;
const EDITOR_LINE_HEIGHT: f32 = 20.;
const CARET_HEIGHT: f32 = 18.;
const CARET_COLOR: u32 = 0x060606;
const ZOOM_STEP: f32 = 0.1;
const ZOOM_MIN: f32 = 0.1;
const ZOOM_MAX: f32 = 5.;

struct ViewSettings {
    status_bar: bool,
    word_wrap: bool,
}

impl Global for ViewSettings {}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MenuKind {
    File,
    Edit,
    View,
}

impl MenuKind {
    const ALL: [Self; 3] = [Self::File, Self::Edit, Self::View];

    fn title(self) -> &'static str {
        match self {
            Self::File => "File",
            Self::Edit => "Edit",
            Self::View => "View",
        }
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|kind| *kind == self).unwrap_or(0)
    }

    fn build(self, menu: PopupMenu, window: &mut Window, cx: &mut Context<PopupMenu>) -> PopupMenu {
        match self {
            Self::File => menu
                .menu("New tab", Box::new(NewTab))
                .menu("New window", Box::new(NewWindow))
                .menu("Open", Box::new(Open))
                .menu("Save", Box::new(Save))
                .menu("Save as", Box::new(SaveAs))
                .menu("Save all", Box::new(SaveAll))
                .separator()
                .menu("Close tab", Box::new(CloseTab))
                .menu("Close window", Box::new(CloseWindow))
                .menu("Exit", Box::new(Exit)),
            Self::Edit => menu
                .menu("Undo", Box::new(input::Undo))
                .separator()
                .menu("Cut", Box::new(input::Cut))
                .menu("Copy", Box::new(input::Copy))
                .menu("Paste", Box::new(input::Paste))
                .menu("Delete", Box::new(input::Delete))
                .separator()
                .menu("Find", Box::new(Find))
                .menu("Find next", Box::new(FindNext))
                .menu("Find previous", Box::new(FindPrevious))
                .menu("Replace", Box::new(Replace))
                .menu("Go to", Box::new(GoTo))
                .separator()
                .menu("Select all", Box::new(input::SelectAll))
                .menu("Time/Date", Box::new(TimeDate)),
            Self::View => {
                let view = cx.global::<ViewSettings>();
                let (status_bar, word_wrap) = (view.status_bar, view.word_wrap);
                menu.submenu("Zoom", window, cx, |menu, _, _| {
                    menu.menu("Zoom in", Box::new(ZoomIn))
                        .menu("Zoom out", Box::new(ZoomOut))
                        .menu("Restore default zoom", Box::new(ResetZoom))
                })
                .menu_with_check("Status bar", status_bar, Box::new(ToggleStatusBar))
                .menu_with_check("Word wrap", word_wrap, Box::new(ToggleWordWrap))
            }
        }
    }
}

struct MenuBar {
    action_context: FocusHandle,
    open: Option<(MenuKind, Entity<PopupMenu>)>,
    _dismiss: Option<Subscription>,
}

impl MenuBar {
    fn new(action_context: FocusHandle) -> Self {
        Self {
            action_context,
            open: None,
            _dismiss: None,
        }
    }

    fn open_kind(&self) -> Option<MenuKind> {
        self.open.as_ref().map(|(kind, _)| *kind)
    }

    fn open_menu(&mut self, kind: MenuKind, window: &mut Window, cx: &mut Context<Self>) {
        let action_context = self.action_context.clone();
        let popup = PopupMenu::build(window, cx, move |menu, window, cx| {
            kind.build(menu.action_context(action_context), window, cx)
        });
        popup.focus_handle(cx).focus(window, cx);
        self._dismiss = Some(cx.subscribe_in(
            &popup,
            window,
            |this, popup, _: &DismissEvent, window, cx| {
                if this.open.as_ref().is_some_and(|(_, open)| open == popup) {
                    this.close_menu(window, cx);
                }
            },
        ));
        self.open = Some((kind, popup));
        cx.notify();
    }

    fn close_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open = None;
        self._dismiss = None;
        self.action_context.focus(window, cx);
        cx.notify();
    }

    fn step(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(kind) = self.open_kind() else {
            return;
        };
        let count = MenuKind::ALL.len() as isize;
        let next = (kind.index() as isize + delta).rem_euclid(count) as usize;
        self.open_menu(MenuKind::ALL[next], window, cx);
    }

    fn trigger(&self, kind: MenuKind, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let popup = self
            .open
            .as_ref()
            .filter(|(open, _)| *open == kind)
            .map(|(_, popup)| popup.clone());

        div()
            .id(kind.title())
            .relative()
            .flex()
            .items_center()
            .h_full()
            .px(px(10.))
            .rounded(px(4.))
            .map(|this| {
                if popup.is_some() {
                    this.bg(rgba(MENU_ITEM_ACTIVE))
                } else {
                    this.hover(|style| style.bg(rgba(MENU_ITEM_ACTIVE)))
                }
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                    if this.open_kind() == Some(kind) {
                        this.close_menu(window, cx);
                    } else {
                        this.open_menu(kind, window, cx);
                    }
                }),
            )
            .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                if *hovered && this.open_kind().is_some_and(|open| open != kind) {
                    this.open_menu(kind, window, cx);
                }
            }))
            .child(div().relative().top(px(1.)).child(kind.title()))
            .when_some(popup, |this, popup| {
                this.child(
                    div()
                        .absolute()
                        .left_0()
                        .top(px(32.))
                        .child(deferred(anchored().child(popup))),
                )
            })
    }
}

impl Render for MenuBar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .h_full()
            .gap(px(8.))
            .font_family(MENU_FONT)
            .text_size(px(14.))
            .text_color(rgba(TEXT_PRIMARY))
            .on_action(cx.listener(|this, _: &SelectLeft, window, cx| this.step(-1, window, cx)))
            .on_action(cx.listener(|this, _: &SelectRight, window, cx| this.step(1, window, cx)))
            .children(MenuKind::ALL.map(|kind| self.trigger(kind, cx)))
    }
}

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
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(2213.), px(1107.)),
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

fn open_jot_window(cx: &mut App) {
    gpui_kit::open_window(window_options(cx), cx, |window, cx| {
        cx.new(|cx| Jot::new(window, cx))
    })
    .expect("Failed to open window");
}

fn caret_blink_period() -> Option<Duration> {
    let millis = unsafe { GetCaretBlinkTime() };
    (millis != 0 && millis != u32::MAX).then(|| Duration::from_millis(millis.into()))
}

fn caret_key(editor: &TextareaState) -> (usize, Range<usize>, usize) {
    (
        editor.cursor(),
        editor.selected_range(),
        editor.text().len(),
    )
}

pub struct Jot {
    editor: Entity<TextareaState>,
    menu_bar: Entity<MenuBar>,
    zoom: f32,
    caret_visible: bool,
    caret_key: (usize, Range<usize>, usize),
    _caret_blink: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl Jot {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let word_wrap = cx.global::<ViewSettings>().word_wrap;
        let editor = cx.new(|cx| TextareaState::new(window, cx).soft_wrap(word_wrap));
        editor.focus_handle(cx).focus(window, cx);
        let action_context = editor.focus_handle(cx);
        let menu_bar = cx.new(|_| MenuBar::new(action_context));

        let _subscriptions = vec![
            cx.observe_window_activation(window, |this, window, cx| {
                if window.is_window_active() {
                    this.restart_caret_blink(cx);
                } else {
                    this._caret_blink = Task::ready(());
                    cx.notify();
                }
            }),
            cx.observe(&editor, |this, editor, cx| {
                let key = caret_key(editor.read(cx));
                if key != this.caret_key {
                    this.caret_key = key;
                    this.restart_caret_blink(cx);
                }
            }),
            cx.observe_global_in::<ViewSettings>(window, |this, window, cx| {
                let word_wrap = cx.global::<ViewSettings>().word_wrap;
                this.editor
                    .update(cx, |editor, cx| editor.set_soft_wrap(word_wrap, window, cx));
                cx.notify();
            }),
        ];

        let caret_key = caret_key(editor.read(cx));
        let mut jot = Self {
            editor,
            menu_bar,
            zoom: 1.,
            caret_visible: true,
            caret_key,
            _caret_blink: Task::ready(()),
            _subscriptions,
        };
        jot.restart_caret_blink(cx);
        jot
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
        let editor = self.editor.clone();
        let height = px((CARET_HEIGHT * self.zoom).round());
        let shown = self.caret_visible
            && window.is_window_active()
            && self.editor.focus_handle(cx).is_focused(window);

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

    fn zoom_in(&mut self, _: &ZoomIn, _: &mut Window, cx: &mut Context<Self>) {
        self.set_zoom(self.zoom + ZOOM_STEP, cx);
    }

    fn zoom_out(&mut self, _: &ZoomOut, _: &mut Window, cx: &mut Context<Self>) {
        self.set_zoom(self.zoom - ZOOM_STEP, cx);
    }

    fn reset_zoom(&mut self, _: &ResetZoom, _: &mut Window, cx: &mut Context<Self>) {
        self.set_zoom(1., cx);
    }

    fn set_zoom(&mut self, zoom: f32, cx: &mut Context<Self>) {
        self.zoom = ((zoom / ZOOM_STEP).round() * ZOOM_STEP).clamp(ZOOM_MIN, ZOOM_MAX);
        cx.notify();
    }

    fn close_window(&mut self, _: &CloseWindow, window: &mut Window, _: &mut Context<Self>) {
        window.remove_window();
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

    fn title_bar(&self, window: &Window) -> impl IntoElement {
        div()
            .flex()
            .flex_none()
            .h(px(42.))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .h_full()
                    .window_control_area(WindowControlArea::Drag)
                    .child(
                        div()
                            .absolute()
                            .left(px(9.))
                            .top(px(10.))
                            .w(px(240.))
                            .h(px(32.))
                            .border_t_1()
                            .border_l_1()
                            .border_r_1()
                            .border_color(rgba(STROKE))
                            .rounded_t(px(8.))
                            .child(div().size_full().rounded_t(px(7.)).bg(rgba(LAYER_ON_MICA))),
                    ),
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
                Textarea::new(&self.editor)
                    .appearance(false)
                    .h_full()
                    .font_family("Consolas")
                    .text_size(px(EDITOR_FONT_SIZE * self.zoom))
                    .line_height(px((EDITOR_LINE_HEIGHT * self.zoom).round())),
            )
            .child(self.caret(window, cx))
    }

    fn status_bar(&self, cx: &App) -> impl IntoElement {
        let editor = self.editor.read(cx);
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
                .child(status_cell(68., format!("{}%", (self.zoom * 100.).round())))
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

        div()
            .v_flex()
            .size_full()
            .on_action(cx.listener(Self::zoom_in))
            .on_action(cx.listener(Self::zoom_out))
            .on_action(cx.listener(Self::reset_zoom))
            .on_action(cx.listener(Self::close_window))
            .child(self.title_bar(window))
            .child(self.menu_bar())
            .child(self.editor(window, cx))
            .when(status_bar, |this| this.child(self.status_bar(cx)))
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
                word_wrap: true,
            });
            cx.bind_keys(key_bindings());

            cx.on_action(|_: &Exit, cx| cx.quit());
            cx.on_action(|_: &NewWindow, cx| open_jot_window(cx));
            cx.on_action(|_: &ToggleStatusBar, cx| {
                cx.update_global::<ViewSettings, _>(|view, _| view.status_bar = !view.status_bar);
            });
            cx.on_action(|_: &ToggleWordWrap, cx| {
                cx.update_global::<ViewSettings, _>(|view, _| view.word_wrap = !view.word_wrap);
            });

            open_jot_window(cx);
        });
}
