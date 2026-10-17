#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use gpui_kit::component::global_state::GlobalState;
use gpui_kit::component::input::{self, Textarea, TextareaState};
use gpui_kit::component::menu::AppMenuBar;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

const LAYER_ON_MICA: u32 = 0xffffffb3;
const STROKE: u32 = 0x0000000f;
const STROKE_ON_LAYER: u32 = 0xeaeaeab8;
const EDITOR_BACKGROUND: u32 = 0xf9f9f9;

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
const ZOOM_STEP: f32 = 0.1;
const ZOOM_MIN: f32 = 0.1;
const ZOOM_MAX: f32 = 5.;

struct ViewSettings {
    status_bar: bool,
    word_wrap: bool,
}

impl Global for ViewSettings {}

fn app_menus(view: &ViewSettings) -> Vec<Menu> {
    vec![
        Menu::new("File").items([
            MenuItem::action("New tab", NewTab),
            MenuItem::action("New window", NewWindow),
            MenuItem::action("Open", Open),
            MenuItem::action("Save", Save),
            MenuItem::action("Save as", SaveAs),
            MenuItem::action("Save all", SaveAll),
            MenuItem::separator(),
            MenuItem::action("Close tab", CloseTab),
            MenuItem::action("Close window", CloseWindow),
            MenuItem::action("Exit", Exit),
        ]),
        Menu::new("Edit").items([
            MenuItem::action("Undo", input::Undo),
            MenuItem::separator(),
            MenuItem::action("Cut", input::Cut),
            MenuItem::action("Copy", input::Copy),
            MenuItem::action("Paste", input::Paste),
            MenuItem::action("Delete", input::Delete),
            MenuItem::separator(),
            MenuItem::action("Find", Find),
            MenuItem::action("Find next", FindNext),
            MenuItem::action("Find previous", FindPrevious),
            MenuItem::action("Replace", Replace),
            MenuItem::action("Go to", GoTo),
            MenuItem::separator(),
            MenuItem::action("Select all", input::SelectAll),
            MenuItem::action("Time/Date", TimeDate),
        ]),
        Menu::new("View").items([
            MenuItem::submenu(Menu::new("Zoom").items([
                MenuItem::action("Zoom in", ZoomIn),
                MenuItem::action("Zoom out", ZoomOut),
                MenuItem::action("Restore default zoom", ResetZoom),
            ])),
            MenuItem::action("Status bar", ToggleStatusBar).checked(view.status_bar),
            MenuItem::action("Word wrap", ToggleWordWrap).checked(view.word_wrap),
        ]),
    ]
}

fn publish_menus(cx: &mut App) {
    let menus = app_menus(cx.global::<ViewSettings>())
        .into_iter()
        .map(Menu::owned)
        .collect();
    GlobalState::global_mut(cx).set_app_menus(menus);
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

pub struct Jot {
    editor: Entity<TextareaState>,
    menu_bar: Entity<AppMenuBar>,
    zoom: f32,
    _subscriptions: Vec<Subscription>,
}

impl Jot {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let word_wrap = cx.global::<ViewSettings>().word_wrap;
        let editor = cx.new(|cx| TextareaState::new(window, cx).soft_wrap(word_wrap));
        editor.focus_handle(cx).focus(window, cx);
        let menu_bar = AppMenuBar::new(cx);

        let _subscriptions = vec![
            cx.observe_window_activation(window, |_, _, cx| cx.notify()),
            cx.observe_global_in::<ViewSettings>(window, |this, window, cx| {
                let word_wrap = cx.global::<ViewSettings>().word_wrap;
                this.editor
                    .update(cx, |editor, cx| editor.set_soft_wrap(word_wrap, window, cx));
                this.menu_bar.update(cx, |menu_bar, cx| menu_bar.reload(cx));
                cx.notify();
            }),
        ];

        Self {
            editor,
            menu_bar,
            zoom: 1.,
            _subscriptions,
        }
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

    fn editor(&self) -> impl IntoElement {
        div()
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
                    .text_size(px(EDITOR_FONT_SIZE * self.zoom)),
            )
    }

    fn status_bar(&self) -> impl IntoElement {
        div()
            .flex_none()
            .child(separator())
            .child(div().h(px(31.)).bg(rgba(LAYER_ON_MICA)))
    }
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
            .child(self.editor())
            .when(status_bar, |this| this.child(self.status_bar()))
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

            cx.set_global(ViewSettings {
                status_bar: true,
                word_wrap: true,
            });
            publish_menus(cx);
            cx.bind_keys(key_bindings());

            cx.on_action(|_: &Exit, cx| cx.quit());
            cx.on_action(|_: &NewWindow, cx| open_jot_window(cx));
            cx.on_action(|_: &ToggleStatusBar, cx| {
                cx.update_global::<ViewSettings, _>(|view, _| view.status_bar = !view.status_bar);
                publish_menus(cx);
            });
            cx.on_action(|_: &ToggleWordWrap, cx| {
                cx.update_global::<ViewSettings, _>(|view, _| view.word_wrap = !view.word_wrap);
                publish_menus(cx);
            });

            open_jot_window(cx);
        });
}
