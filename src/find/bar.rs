use std::time::Duration;

use crate::command::AppCommand;
use crate::editor::{Editor, FieldOptions};
use crate::geom::{Point, Rect, Size, point, size};
use crate::gfx::TextSystem;
use crate::ids::{AnimKey, EditorSlot, Focus, HitId, Node, UiState};
use crate::platform::{CursorStyle, Key, KeyEvent, TimerHandle};
use crate::shell::{WindowTimer, WindowTimers};
use crate::theme::{
    BODY_SIZE, FIELD_CARET, OVERLAY_RADIUS, Palette, TEXT_FONT, theme_shadow,
};
use crate::types::SharedStr;
use crate::ui::{Anchor, AnimStatus, Curve, Easing, Transform, Transition, box_, row, text};
use crate::widgets::{button, icon_button, text_field};

use super::matcher::Matcher;

const EXPAND_CURVE: Curve = Curve::new(
    Duration::from_millis(300),
    Easing::CubicBezier {
        x1: 0.1,
        y1: 0.9,
        x2: 0.2,
        y2: 1.,
    },
);
const CONTRACT_CURVE: Curve = Curve::new(
    Duration::from_millis(200),
    Easing::CubicBezier {
        x1: 0.7,
        y1: 0.,
        x2: 1.,
        y2: 0.5,
    },
);
const CARD_Z: f32 = 32.;
const NOTICE_Z: f32 = 16.;
const COLLAPSED_SIZE: f32 = 20.;
const EXPAND_START_SCALE: f32 = 0.01;
const NOTICE_DURATION: Duration = Duration::from_secs(3);
const MAX_SEED_QUERY_CHARS: usize = 256;
const CARD_WIDTH: f32 = 860.;
const CARD_TOP: f32 = 12.;
const CARD_SHADOW_CLIP_TOP: f32 = 6.;
const CARD_SHADOW_CLIP_BOTTOM: f32 = 14.;
const CARD_SIDE_MARGIN: f32 = 12.;
const CARD_PADDING: f32 = 8.;
const ROW_GAP: f32 = 8.;
const ICON_GAP: f32 = 10.;
const ICON_BUTTON_SIZE: f32 = 30.;
const ACTION_GAP: f32 = 10.;
const ACTION_WIDTH: f32 = 90.;
const ICON_SIZE: f32 = 16.;
const CHEVRON_SIZE: f32 = 14.;
const NOTICE_GAP: f32 = 8.;
const NOTICE_PADDING_X: f32 = 18.;
const NOTICE_PADDING_TOP: f32 = 16.;
const NOTICE_PADDING_BOTTOM: f32 = 15.;
const NOTICE_LINE: f32 = 20.;
const BORDER: f32 = 1.;
const CHEVRON_UP_GLYPH: &str = "\u{E70E}";
const CHEVRON_DOWN_GLYPH: &str = "\u{E70D}";
const ARROW_DOWN_GLYPH: &str = "\u{E74B}";
const ARROW_UP_GLYPH: &str = "\u{E74A}";
const OPTIONS_GLYPH: &str = "\u{E9E9}";
const CLOSE_GLYPH: &str = "\u{E8BB}";

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum FindKey {
    PassThrough,
    Focus(Focus),
    Command(AppCommand),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum FindOutcome {
    EmptyQuery,
    Found { wrapped: bool },
    NotFound,
    Replaced(usize),
}

pub(crate) struct FindBar {
    transition: Transition,
    replace: bool,
    match_case: bool,
    wrap_around: bool,
    pub(crate) query: Editor,
    pub(crate) replacement: Editor,
    notice: Option<SharedStr>,
    notice_timer: Option<TimerHandle>,
    matcher: Option<Matcher>,
    buffer: String,
}

impl Default for FindBar {
    fn default() -> Self {
        Self::new()
    }
}

impl FindBar {
    pub(crate) fn new() -> Self {
        let query = Editor::field(FieldOptions {
            placeholder: "Find".into(),
            caret_height: FIELD_CARET,
            validate: None,
        });
        let replacement = Editor::field(FieldOptions {
            placeholder: "Replace".into(),
            caret_height: FIELD_CARET,
            validate: None,
        });
        Self {
            transition: Transition::default(),
            replace: false,
            match_case: false,
            wrap_around: true,
            query,
            replacement,
            notice: None,
            notice_timer: None,
            matcher: None,
            buffer: String::new(),
        }
    }

    pub(crate) fn is_open(&self) -> bool {
        self.transition.is_open()
    }

    pub(crate) fn is_replace(&self) -> bool {
        self.replace
    }

    pub(crate) fn match_case(&self) -> bool {
        self.match_case
    }

    pub(crate) fn wrap_around(&self) -> bool {
        self.wrap_around
    }

    pub(crate) fn toggle_replace(&mut self) {
        self.replace = !self.replace;
    }

    pub(crate) fn toggle_match_case(&mut self) {
        self.match_case = !self.match_case;
    }

    pub(crate) fn toggle_wrap_around(&mut self) {
        self.wrap_around = !self.wrap_around;
    }

    pub(crate) fn on_key(&self, ev: KeyEvent, focus: Focus) -> FindKey {
        if ev.modifiers.ctrl || ev.modifiers.alt || ev.modifiers.win {
            return FindKey::PassThrough;
        }
        if ev.key == Key::Tab && self.replace {
            return match focus {
                Focus::Editor(EditorSlot::FindQuery) => {
                    FindKey::Focus(Focus::Editor(EditorSlot::FindReplace))
                }
                Focus::Editor(EditorSlot::FindReplace) => {
                    FindKey::Focus(Focus::Editor(EditorSlot::FindQuery))
                }
                _ => FindKey::PassThrough,
            };
        }
        if ev.key == Key::Enter {
            return match (focus, ev.modifiers.shift) {
                (Focus::Editor(EditorSlot::FindQuery), false) => {
                    FindKey::Command(AppCommand::FindNext)
                }
                (Focus::Editor(EditorSlot::FindQuery), true) => {
                    FindKey::Command(AppCommand::FindPrevious)
                }
                (Focus::Editor(EditorSlot::FindReplace), false) => {
                    FindKey::Command(AppCommand::ReplaceNext)
                }
                _ => FindKey::PassThrough,
            };
        }
        FindKey::PassThrough
    }

    pub(crate) fn clear_notice(&mut self) {
        self.notice = None;
        self.notice_timer = None;
    }

    fn show_notice(&mut self, notice: impl Into<SharedStr>, timers: WindowTimers<'_>) {
        if !self.is_open() {
            return;
        }
        self.notice = Some(notice.into());
        self.notice_timer = Some(timers.after(NOTICE_DURATION, WindowTimer::FindNotice));
    }

    pub(crate) fn on_notice_dismiss(&mut self) -> bool {
        self.notice.take().is_some()
    }

    pub(crate) fn open(&mut self, replace: bool, selected: &str, ts: &TextSystem) {
        if !selected.is_empty()
            && !selected.contains('\n')
            && selected.chars().take(MAX_SEED_QUERY_CHARS + 1).count() <= MAX_SEED_QUERY_CHARS
        {
            self.query.set_value(selected, ts);
        }
        self.transition.open();
        self.replace = replace;
        self.query.select_all();
    }

    pub(crate) fn close(&mut self) -> bool {
        if !self.transition.close() {
            return false;
        }
        self.clear_notice();
        true
    }

    fn prepare(&mut self) -> Option<String> {
        let query = self.query.model().value();
        if query.is_empty() {
            return None;
        }
        let stale = self
            .matcher
            .as_ref()
            .is_none_or(|m| m.query != query || m.match_case != self.match_case);
        if stale {
            self.matcher = Matcher::new(&query, self.match_case);
        }
        self.matcher.as_ref().map(|_| query)
    }

    pub(crate) fn step(
        &mut self,
        editor: &mut Editor,
        forward: bool,
        timers: WindowTimers<'_>,
    ) -> FindOutcome {
        let Some(query) = self.prepare() else {
            return FindOutcome::EmptyQuery;
        };
        let text = editor.model().text().clone();
        let selection = editor.model().selected_range();
        let wrap = self.wrap_around;
        let found = self
            .matcher
            .as_ref()
            .and_then(|m| m.search(&text, selection, forward, wrap, &mut self.buffer));
        match found {
            Some((range, wrapped)) => {
                editor.set_selected_range(range);
                if wrapped {
                    let msg = if forward {
                        "Found next from the top"
                    } else {
                        "Found previous from the bottom"
                    };
                    self.show_notice(msg, timers);
                } else {
                    self.clear_notice();
                }
                FindOutcome::Found { wrapped }
            }
            None => {
                self.show_notice(not_found(&query), timers);
                FindOutcome::NotFound
            }
        }
    }

    pub(crate) fn replace_next(
        &mut self,
        editor: &mut Editor,
        ts: &TextSystem,
        timers: WindowTimers<'_>,
    ) -> FindOutcome {
        if self.prepare().is_none() {
            return FindOutcome::EmptyQuery;
        }
        let sel_len = editor.model().selected_range().len();
        let is_match = self.matcher.as_ref().is_some_and(|m| {
            sel_len > 0
                && sel_len <= m.max_match_bytes()
                && m.is_match(&editor.model().selected_text().to_string())
        });
        if is_match {
            let replacement = self.replacement.model().value();
            editor.replace(&replacement, ts);
        }
        let out = self.step(editor, true, timers);
        if is_match {
            FindOutcome::Replaced(1)
        } else {
            out
        }
    }

    pub(crate) fn replace_all(
        &mut self,
        editor: &mut Editor,
        ts: &TextSystem,
        timers: WindowTimers<'_>,
    ) -> FindOutcome {
        let Some(query) = self.prepare() else {
            return FindOutcome::EmptyQuery;
        };
        let text = editor.model().text().clone();
        let ranges = match self.matcher.as_ref() {
            Some(m) => m.all(&text, &mut self.buffer),
            None => return FindOutcome::EmptyQuery,
        };
        if ranges.is_empty() {
            self.show_notice(not_found(&query), timers);
            return FindOutcome::NotFound;
        }
        let count = ranges.len();
        let replacement = self.replacement.model().value();
        editor.replace_all(&ranges, &replacement, ts);
        FindOutcome::Replaced(count)
    }

    fn sample(&mut self, ui: &mut UiState) -> Option<Transform> {
        if !self.transition.is_visible() {
            return None;
        }
        let serial = self.transition.serial();
        let closing = self.transition.is_closing();
        let shown = ui
            .progress(AnimKey::Transition { serial, track: 0 }, true, EXPAND_CURVE)
            .value;
        let hidden = ui.progress(
            AnimKey::Transition { serial, track: 1 },
            closing,
            CONTRACT_CURVE,
        );
        if closing && hidden.status == AnimStatus::Finished {
            self.transition.finish();
            return None;
        }
        Some(Transform::ScaleFromPx {
            collapsed: COLLAPSED_SIZE,
            min_start: if closing { 1.0 } else { EXPAND_START_SCALE },
            t: if closing { 1.0 - hidden.value } else { shown },
        })
    }

    pub(crate) fn render(
        &mut self,
        focused_slot: Option<EditorSlot>,
        p: Palette,
        ui: &mut UiState,
    ) -> Option<Node> {
        let transform = self.sample(ui)?;
        let notice_node = self
            .notice
            .clone()
            .filter(|_| self.transition.is_open())
            .map(|msg| render_notice(p, msg));
        let card = self
            .render_card(p, focused_slot, ui)
            .transform(transform)
            .when(self.transition.is_closing(), Node::inert);

        Some(
            box_()
                .absolute()
                .inset_0()
                .child(
                    row()
                        .absolute()
                        .top(CARD_TOP - CARD_SHADOW_CLIP_TOP)
                        .left(0.)
                        .right(0.)
                        .px(CARD_SIDE_MARGIN)
                        .pt(CARD_SHADOW_CLIP_TOP)
                        .pb(CARD_SHADOW_CLIP_BOTTOM)
                        .overflow_hidden()
                        .justify_center()
                        .child(card),
                )
                .when_some(notice_node, Node::child),
        )
    }

    fn render_card(&self, p: Palette, focused_slot: Option<EditorSlot>, ui: &mut UiState) -> Node {
        let query_row = self.render_query_row(p, focused_slot, ui);
        let replace_row = self
            .replace
            .then(|| self.render_replace_row(p, focused_slot, ui));

        box_()
            .w(CARD_WIDTH)
            .max_w_full()
            .border(BORDER, p.surface_stroke)
            .rounded(OVERLAY_RADIUS)
            .shadows(theme_shadow(p.dark, CARD_Z))
            .child(
                box_()
                    .hit(HitId::FindCard)
                    .cursor(CursorStyle::Arrow)
                    .w_full()
                    .p(CARD_PADDING)
                    .rounded(OVERLAY_RADIUS - BORDER)
                    .bg(p.card_surface.panel)
                    .child(query_row)
                    .when_some(replace_row, Node::child),
            )
    }

    fn render_query_row(
        &self,
        p: Palette,
        focused_slot: Option<EditorSlot>,
        ui: &mut UiState,
    ) -> Node {
        let dims = size(ICON_BUTTON_SIZE, ICON_BUTTON_SIZE);
        let btn = |hit, tip, glyph, sz, ui: &mut UiState| {
            icon_button(p, hit, tip, (glyph, sz), dims, true, ui)
        };
        let replace = self.replace;
        let expand_btn = btn(
            HitId::FindToggleReplace,
            if replace {
                "Close replace options"
            } else {
                "Open replace options"
            },
            if replace {
                CHEVRON_DOWN_GLYPH
            } else {
                CHEVRON_UP_GLYPH
            },
            CHEVRON_SIZE,
            ui,
        );
        let query_field = text_field(
            p,
            p.card_surface,
            EditorSlot::FindQuery,
            &self.query,
            focused_slot == Some(EditorSlot::FindQuery),
            true,
            ui,
        );
        let next_btn = btn(HitId::FindNext, "Search down", ARROW_DOWN_GLYPH, ICON_SIZE, ui);
        let prev_btn = btn(HitId::FindPrev, "Search up", ARROW_UP_GLYPH, ICON_SIZE, ui);
        let opts_btn = btn(HitId::FindOptions, "More options", OPTIONS_GLYPH, ICON_SIZE, ui);
        let close_btn = btn(
            HitId::FindClose,
            "Exit Find and Replace",
            CLOSE_GLYPH,
            ICON_SIZE,
            ui,
        );

        row()
            .items_center()
            .gap(ICON_GAP)
            .child(expand_btn)
            .child(box_().flex_1().min_w_0().child(query_field))
            .child(next_btn)
            .child(prev_btn)
            .child(opts_btn)
            .child(close_btn)
    }

    fn render_replace_row(
        &self,
        p: Palette,
        focused_slot: Option<EditorSlot>,
        ui: &mut UiState,
    ) -> Node {
        let replace_field = text_field(
            p,
            p.card_surface,
            EditorSlot::FindReplace,
            &self.replacement,
            focused_slot == Some(EditorSlot::FindReplace),
            false,
            ui,
        );
        let replace_btn = button(
            p,
            p.card_surface,
            HitId::FindReplace,
            "Replace",
            false,
            false,
            ui,
        );
        let replace_all_btn = button(
            p,
            p.card_surface,
            HitId::FindReplaceAll,
            "Replace all",
            false,
            false,
            ui,
        );

        row()
            .items_center()
            .mt(ROW_GAP)
            .child(box_().flex_none().w(ICON_BUTTON_SIZE + ICON_GAP))
            .child(box_().flex_1().min_w_0().child(replace_field))
            .child(
                box_()
                    .flex_none()
                    .ml(ACTION_GAP)
                    .w(ACTION_WIDTH)
                    .child(replace_btn),
            )
            .child(
                box_()
                    .flex_none()
                    .ml(ACTION_GAP)
                    .w(ACTION_WIDTH)
                    .child(replace_all_btn),
            )
    }
}

fn not_found(query: &str) -> String {
    format!("Cannot find \"{query}\"")
}

fn place_notice(card: Rect, popup: Size, _viewport: Size) -> Point {
    point(
        card.center().x - (popup.width / 2.).floor(),
        card.bottom() + BORDER + NOTICE_GAP,
    )
}

fn render_notice(p: Palette, msg: SharedStr) -> Node {
    box_()
        .anchored(Anchor::Target {
            target: HitId::FindCard,
            place: place_notice,
        })
        .border(BORDER, p.flyout_stroke)
        .rounded(OVERLAY_RADIUS)
        .shadows(theme_shadow(p.dark, NOTICE_Z))
        .child(
            box_()
                .px(NOTICE_PADDING_X)
                .pt(NOTICE_PADDING_TOP)
                .pb(NOTICE_PADDING_BOTTOM)
                .rounded(OVERLAY_RADIUS - BORDER)
                .bg(p.notice_fill)
                .font_family(TEXT_FONT)
                .text_size(BODY_SIZE)
                .line_height(NOTICE_LINE)
                .text_color(p.text_primary)
                .child(text(msg)),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    use crate::editor::{DocOptions, Rope};
    use crate::geom::Edges;
    use crate::platform::Jobs;
    use crate::shell::AppEvent;

    #[test]
    fn find_bar_step_wrap_replace_next_and_replace_all_return_outcomes() {
        let ts = TextSystem::new();
        let jobs = Jobs::new();
        let timers = WindowTimers::for_test(&jobs);
        let mut bar = FindBar::new();
        let mut ed = Editor::document(
            Rope::from("foo bar foo baz"),
            DocOptions {
                soft_wrap: true,
                paddings: Edges::ZERO,
            },
        );

        bar.open(true, "foo", &ts);
        assert!(bar.is_open());
        assert_eq!(bar.query.model().value(), "foo");

        assert_eq!(bar.step(&mut ed, true, timers), FindOutcome::Found { wrapped: false });
        assert_eq!(ed.model().selected_range(), 0..3);

        assert_eq!(bar.step(&mut ed, true, timers), FindOutcome::Found { wrapped: false });
        assert_eq!(ed.model().selected_range(), 8..11);

        assert_eq!(bar.step(&mut ed, true, timers), FindOutcome::Found { wrapped: true });
        assert_eq!(ed.model().selected_range(), 0..3);
        assert!(matches!(
            jobs.pop_at(Instant::now() + NOTICE_DURATION + Duration::from_secs(1)),
            Some(AppEvent::Window {
                timer: WindowTimer::FindNotice,
                ..
            })
        ));

        bar.replacement.set_value("qux", &ts);
        assert_eq!(bar.replace_next(&mut ed, &ts, timers), FindOutcome::Replaced(1));
        assert_eq!(ed.model().value(), "qux bar foo baz");
        assert_eq!(ed.model().selected_range(), 8..11);

        bar.query.set_value("o", &ts);
        assert_eq!(bar.replace_all(&mut ed, &ts, timers), FindOutcome::Replaced(2));
        assert_eq!(ed.model().value(), "qux bar fquxqux baz");

        bar.query.set_value("missing", &ts);
        assert_eq!(bar.step(&mut ed, true, timers), FindOutcome::NotFound);
        assert!(bar.on_notice_dismiss());
        assert!(!bar.on_notice_dismiss());
    }
}
