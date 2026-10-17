#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::*;
use gpui_kit::*;

pub struct Jot {
    input: Entity<InputState>,
    _subscription: Subscription,
}

impl Jot {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Jot something down..."));

        let _subscription = cx.subscribe_in(&input, window, |_, _, event, _, cx| {
            if let InputEvent::Change = event {
                cx.notify();
            }
        });

        input.focus_handle(cx).focus(window, cx);

        Self { input, _subscription }
    }
}

impl Render for Jot {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chars = self.input.read(cx).value().chars().count();

        div()
            .v_flex()
            .gap_2()
            .size_full()
            .p_4()
            .justify_center()
            .child(Input::new(&self.input))
            .child(format!("{chars} characters"))
    }
}

fn main() {
    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);

        gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
            cx.new(|cx| Jot::new(window, cx))
        })
        .expect("Failed to open window");
    });
}
