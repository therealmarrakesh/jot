#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use gpui_kit::component::input::{Textarea, TextareaState};
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

pub struct Jot {
    editor: Entity<TextareaState>,
    _activation: Subscription,
}

impl Jot {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let editor = cx.new(|cx| TextareaState::new(window, cx));
        editor.focus_handle(cx).focus(window, cx);
        let _activation = cx.observe_window_activation(window, |_, _, cx| cx.notify());
        Self {
            editor,
            _activation,
        }
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
                            .left(px(47.))
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
            .child(div().h(px(32.)).bg(rgba(LAYER_ON_MICA)))
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
                    .text_size(px(11. * 96. / 72.)),
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
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .v_flex()
            .size_full()
            .child(self.title_bar(window))
            .child(self.menu_bar())
            .child(self.editor())
            .child(self.status_bar())
    }
}

fn main() {
    gpui_kit::application().run(move |cx| {
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

        let options = WindowOptions {
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
        };

        gpui_kit::open_window(options, cx, |window, cx| cx.new(|cx| Jot::new(window, cx)))
            .expect("Failed to open window");
    });
}
