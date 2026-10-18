use super::*;
use std::time::Instant;

const SETTLE: Duration = Duration::from_millis(1500);
const POLL: Duration = Duration::from_millis(1);
const TYPING_SAMPLES: usize = 20;

pub(crate) type FrameProbe = Rc<Cell<Option<Instant>>>;

pub(crate) fn probe(painted: &FrameProbe) -> impl IntoElement {
    let painted = painted.clone();
    canvas(
        |_, _, _| {},
        move |_, _, _, _| painted.set(Some(Instant::now())),
    )
    .absolute()
    .size_0()
}

pub(crate) fn word_wrap() -> bool {
    std::env::var("JOT_BENCH_WRAP").map_or(true, |value| value != "0")
}

pub(crate) fn schedule(painted: FrameProbe, window: &mut Window, cx: &mut Context<Jot>) {
    let Ok(path) = std::env::var("JOT_BENCH_FILE") else {
        return;
    };

    cx.spawn_in(window, async move |this, cx| {
        cx.background_executor().timer(SETTLE).await;

        let started = Instant::now();
        let text = std::fs::read_to_string(&path).expect("benchmark file");
        let read = started.elapsed();
        let bytes = text.len();

        let real_paste = std::env::var("JOT_BENCH_MODE").is_ok_and(|mode| mode == "paste");
        let mut saved_clipboard = None;
        let mut clipboard_write = Duration::ZERO;
        if real_paste {
            cx.update(|_, cx| {
                saved_clipboard = cx.read_from_clipboard().and_then(|item| item.text());
                let started = Instant::now();
                cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                clipboard_write = started.elapsed();
            })
            .ok();
        }

        let started = Instant::now();
        this.update_in(cx, |jot, window, cx| {
            let editor = jot.active_editor();
            if real_paste {
                let focus = editor.focus_handle(cx);
                focus.dispatch_action(&input::Paste, window, cx);
            } else {
                editor.update(cx, |editor, cx| editor.replace(text, window, cx));
            }
            window.refresh();
        })
        .ok();
        let insert = started.elapsed();
        let paint = wait_for_paint(&painted, started, cx).await;

        if let Some(saved) = saved_clipboard {
            cx.update(|_, cx| cx.write_to_clipboard(ClipboardItem::new_string(saved)))
                .ok();
        }

        let mut typing = Vec::with_capacity(TYPING_SAMPLES);
        for _ in 0..TYPING_SAMPLES {
            let started = Instant::now();
            this.update_in(cx, |jot, window, cx| {
                jot.active_editor()
                    .update(cx, |editor, cx| editor.insert("x", window, cx));
                window.refresh();
            })
            .ok();
            typing.push(wait_for_paint(&painted, started, cx).await);
        }
        typing.sort();

        println!(
            "file {path}\nmode {}\nbytes {bytes}\nread_ms {:.1}\nclipboard_write_ms {:.1}\ninsert_ms {:.1}\nfirst_paint_ms {:.1}\ntyping_median_ms {:.1}\ntyping_max_ms {:.1}",
            if real_paste { "paste" } else { "replace" },
            ms(read),
            ms(clipboard_write),
            ms(insert),
            ms(paint),
            ms(typing[typing.len() / 2]),
            ms(typing[typing.len() - 1]),
        );

        cx.background_executor().timer(SETTLE).await;
        cx.update(|_, cx| cx.quit()).ok();
    })
    .detach();
}

async fn wait_for_paint(
    painted: &FrameProbe,
    since: Instant,
    cx: &mut AsyncWindowContext,
) -> Duration {
    loop {
        if let Some(at) = painted.get().filter(|at| *at > since) {
            return at - since;
        }
        cx.background_executor().timer(POLL).await;
    }
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.
}
