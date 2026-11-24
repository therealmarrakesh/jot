use std::time::Duration;

use crate::editor::Editor;
use crate::geom::{Point, point};
use crate::gfx::TextSystem;
use crate::ids::{AnimKey, CustomDraw, EditorSlot, HitId, Node, UiState};
use crate::platform::{CursorStyle, TimerHandle};
use crate::theme::{Palette, theme_shadow};
use crate::types::SharedStr;
use crate::ui::controls::{
    BODY_LINE, BODY_SIZE, ButtonStyle, CONTROL_HEIGHT, CONTROL_RADIUS, FIELD_CARET,
    IconButtonStyle, OVERLAY_RADIUS, TEXT_FONT, TextFieldStyle, button, icon_button,
    text_field_frame,
};
use crate::ui::{AnimStatus, Easing, Presence, box_, row, text};

use super::matcher::Matcher;

const EXPAND_DURATION: Duration = Duration::from_millis(300);
const CONTRACT_DURATION: Duration = Duration::from_millis(200);
const CARD_Z: f32 = 32.;
const NOTICE_Z: f32 = 16.;
const COLLAPSED_SIZE: f32 = 20.;
const EXPAND_START_SCALE: f32 = 0.01;
pub(crate) const NOTICE_DURATION: Duration = Duration::from_secs(3);
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

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct FindOutcome {
    pub(crate) searched: bool,
    pub(crate) replaced: bool,
    pub(crate) arm_notice: bool,
}

pub(crate) struct FindBar {
    presence: Presence,
    pub(crate) replace: bool,
    serial: usize,
    pub(crate) match_case: bool,
    pub(crate) wrap_around: bool,
    pub(crate) query: Editor,
    pub(crate) replacement: Editor,
    notice: Option<SharedStr>,
    notice_timer: TimerHandle,
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
        let query = Editor::single_line()
            .with_caret_height(FIELD_CARET)
            .with_placeholder("Find");
        let replacement = Editor::single_line()
            .with_caret_height(FIELD_CARET)
            .with_placeholder("Replace");
        Self {
            presence: Presence::Hidden,
            replace: false,
            serial: 0,
            match_case: false,
            wrap_around: true,
            query,
            replacement,
            notice: None,
            notice_timer: TimerHandle::default(),
            matcher: None,
            buffer: String::new(),
        }
    }

    pub(crate) fn is_open(&self) -> bool {
        self.presence.is_open()
    }

    pub(crate) fn clear_notice(&mut self) {
        self.notice = None;
        self.notice_timer.cancel();
    }

    pub(crate) fn set_notice_timer(&mut self, timer: TimerHandle) {
        self.notice_timer = timer;
    }

    fn show_notice(&mut self, notice: impl Into<SharedStr>) -> bool {
        if !self.is_open() {
            return false;
        }
        self.notice = Some(notice.into());
        true
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
        if !self.is_open() {
            self.serial += 1;
            self.presence = Presence::Open;
        }
        self.replace = replace;
        self.query.select_all();
    }

    pub(crate) fn close(&mut self) -> bool {
        if !self.is_open() {
            return false;
        }
        self.presence = Presence::Closing;
        self.clear_notice();
        true
    }

    fn prepare(&mut self) -> Option<String> {
        let query = self.query.value().to_string();
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

    pub(crate) fn step(&mut self, editor: &mut Editor, forward: bool) -> FindOutcome {
        let Some(query) = self.prepare() else {
            return FindOutcome::default();
        };
        let text = editor.text().clone();
        let selection = editor.selected_range();
        let wrap = self.wrap_around;
        let found = self
            .matcher
            .as_ref()
            .and_then(|m| m.search(&text, selection, forward, wrap, &mut self.buffer));
        let arm_notice = match found {
            Some((range, wrapped)) => {
                editor.set_selected_range(range);
                if wrapped {
                    let msg = if forward {
                        "Found next from the top"
                    } else {
                        "Found previous from the bottom"
                    };
                    self.show_notice(msg)
                } else {
                    self.clear_notice();
                    false
                }
            }
            None => self.show_notice(not_found(&query)),
        };
        FindOutcome {
            searched: true,
            replaced: false,
            arm_notice,
        }
    }

    pub(crate) fn replace_next(&mut self, editor: &mut Editor, ts: &TextSystem) -> FindOutcome {
        if self.prepare().is_none() {
            return FindOutcome::default();
        }
        let sel_len = editor.selected_range().len();
        let is_match = self.matcher.as_ref().is_some_and(|m| {
            sel_len > 0
                && sel_len <= m.max_match_bytes()
                && m.is_match(&editor.selected_text().to_string())
        });
        if is_match {
            let replacement = self.replacement.value();
            editor.replace(replacement.as_str(), ts);
        }
        let mut out = self.step(editor, true);
        out.replaced = is_match;
        out
    }

    pub(crate) fn replace_all(&mut self, editor: &mut Editor, ts: &TextSystem) -> FindOutcome {
        let Some(query) = self.prepare() else {
            return FindOutcome::default();
        };
        let text = editor.text().clone();
        let ranges = match self.matcher.as_ref() {
            Some(m) => m.all(&text, &mut self.buffer),
            None => return FindOutcome::default(),
        };
        if ranges.is_empty() {
            let arm_notice = self.show_notice(not_found(&query));
            return FindOutcome {
                searched: true,
                replaced: false,
                arm_notice,
            };
        }
        let replacement = self.replacement.value();
        editor.replace_all(&ranges, replacement.as_str(), ts);
        FindOutcome {
            searched: true,
            replaced: true,
            arm_notice: false,
        }
    }

    fn card_size(&self, viewport_width: f32) -> (f32, f32) {
        let rows = if self.replace { 2. } else { 1. };
        let height =
            2. * BORDER + 2. * CARD_PADDING + rows * CONTROL_HEIGHT + (rows - 1.) * ROW_GAP;
        let available = viewport_width - 2. * CARD_SIDE_MARGIN;
        (CARD_WIDTH.min(available).max(0.), height)
    }

    fn sample(&mut self, viewport_width: f32, ui: &mut UiState) -> Option<Point> {
        if !self.presence.is_visible() {
            return None;
        }
        let closing = self.presence.is_closing();
        let shown = ui
            .animate_f32_from(
                AnimKey::FindShow(self.serial),
                0.0,
                1.0,
                EXPAND_DURATION,
                Easing::CubicBezier {
                    x1: 0.1,
                    y1: 0.9,
                    x2: 0.2,
                    y2: 1.,
                },
            )
            .value;
        let hidden = ui.animate_f32_from(
            AnimKey::FindHide(self.serial),
            0.0,
            if closing { 1. } else { 0. },
            CONTRACT_DURATION,
            Easing::CubicBezier {
                x1: 0.7,
                y1: 0.,
                x2: 1.,
                y2: 0.5,
            },
        );
        let (width, height) = self.card_size(viewport_width);
        let axis = |sz: f32| {
            let collapsed = COLLAPSED_SIZE / sz.max(COLLAPSED_SIZE);
            if closing {
                1. - (1. - collapsed) * hidden.value
            } else {
                let start = collapsed.min(EXPAND_START_SCALE);
                start + (1. - start) * shown
            }
        };
        let scale = point(axis(width), axis(height));
        if closing && hidden.status == AnimStatus::Finished {
            self.presence = Presence::Hidden;
            return None;
        }
        Some(scale)
    }

    pub(crate) fn render(
        &mut self,
        viewport_width: f32,
        focused_slot: Option<EditorSlot>,
        p: Palette,
        ui: &mut UiState,
    ) -> Option<Node> {
        let scale = self.sample(viewport_width, ui)?;
        let (width, height) = self.card_size(viewport_width);
        let notice_node = self
            .notice
            .clone()
            .filter(|_| self.presence.is_open())
            .map(|msg| render_notice(p, msg, CARD_TOP + height + NOTICE_GAP));
        let card = self
            .render_card(p, width, height, focused_slot, ui)
            .scale(scale.x, scale.y)
            .when(self.presence.is_closing(), Node::inert);

        Some(
            box_()
                .absolute()
                .top(0.)
                .left(0.)
                .right(0.)
                .child(
                    row()
                        .absolute()
                        .top(CARD_TOP - CARD_SHADOW_CLIP_TOP)
                        .left(0.)
                        .right(0.)
                        .h(height + CARD_SHADOW_CLIP_TOP + CARD_SHADOW_CLIP_BOTTOM)
                        .pt(CARD_SHADOW_CLIP_TOP)
                        .pb(CARD_SHADOW_CLIP_BOTTOM)
                        .overflow_hidden()
                        .items_center()
                        .justify_center()
                        .child(card),
                )
                .when_some(notice_node, |this, n| this.child(n)),
        )
    }

    fn render_card(
        &self,
        p: Palette,
        width: f32,
        height: f32,
        focused_slot: Option<EditorSlot>,
        ui: &mut UiState,
    ) -> Node {
        let buttons = ButtonStyle {
            surface: p.card_surface,
            accent: p.accent,
        };
        let query_row = self.render_query_row(p, focused_slot, ui);
        let replace_row = self
            .replace
            .then(|| self.render_replace_row(p, buttons, focused_slot, ui));

        box_()
            .w(width)
            .h(height)
            .border(BORDER, p.surface_stroke)
            .rounded(OVERLAY_RADIUS)
            .shadows(theme_shadow(p.dark, CARD_Z))
            .child(
                box_()
                    .hit(HitId::FindCard)
                    .cursor(CursorStyle::Arrow)
                    .size_full()
                    .justify_center()
                    .p(CARD_PADDING)
                    .rounded(OVERLAY_RADIUS - BORDER)
                    .bg(p.card_surface.panel)
                    .child(query_row)
                    .when_some(replace_row, |this, r| this.child(r)),
            )
    }

    fn render_field(
        editor: &Editor,
        slot: EditorSlot,
        focused_slot: Option<EditorSlot>,
        search_icon: bool,
        p: Palette,
        ui: &mut UiState,
    ) -> Node {
        let view = editor.view(HitId::Editor(slot), CustomDraw::Editor(slot), BODY_LINE + 2.);
        text_field_frame(
            p,
            HitId::Editor(slot),
            HitId::FieldClear(slot),
            TextFieldStyle {
                surface: p.card_surface,
                focused: focused_slot == Some(slot),
                empty: editor.text().len() == 0,
                search_icon,
            },
            view,
            ui,
        )
    }

    fn render_query_row(
        &self,
        p: Palette,
        focused_slot: Option<EditorSlot>,
        ui: &mut UiState,
    ) -> Node {
        let replace = self.replace;
        let expand_btn = find_icon_button(
            p,
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
        let query_field =
            Self::render_field(&self.query, EditorSlot::FindQuery, focused_slot, true, p, ui);
        let next_btn = find_icon_button(
            p,
            HitId::FindNext,
            "Search down",
            ARROW_DOWN_GLYPH,
            ICON_SIZE,
            ui,
        );
        let prev_btn = find_icon_button(
            p,
            HitId::FindPrev,
            "Search up",
            ARROW_UP_GLYPH,
            ICON_SIZE,
            ui,
        );
        let opts_btn = find_icon_button(
            p,
            HitId::FindOptions,
            "More options",
            OPTIONS_GLYPH,
            ICON_SIZE,
            ui,
        );
        let close_btn = find_icon_button(
            p,
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
        buttons: ButtonStyle,
        focused_slot: Option<EditorSlot>,
        ui: &mut UiState,
    ) -> Node {
        let replace_field = Self::render_field(
            &self.replacement,
            EditorSlot::FindReplace,
            focused_slot,
            false,
            p,
            ui,
        );
        let replace_btn = button(p, buttons, HitId::FindReplace, "Replace", false, false, ui);
        let replace_all_btn = button(
            p,
            buttons,
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

fn render_notice(p: Palette, msg: SharedStr, top: f32) -> Node {
    row()
        .absolute()
        .top(top)
        .left(0.)
        .right(0.)
        .justify_center()
        .child(
            box_()
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
                ),
        )
}

fn find_icon_button(
    p: Palette,
    hit: HitId,
    tip: &'static str,
    glyph: &'static str,
    glyph_size: f32,
    ui: &mut UiState,
) -> Node {
    icon_button(
        p,
        hit,
        tip,
        glyph,
        IconButtonStyle {
            glyph_size,
            width: ICON_BUTTON_SIZE,
            height: ICON_BUTTON_SIZE,
            radius: CONTROL_RADIUS,
        },
        true,
        ui,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::Rope;

    #[test]
    fn find_bar_step_wrap_replace_next_and_replace_all_return_outcomes() {
        let ts = TextSystem::new();
        let mut bar = FindBar::new();
        let mut ed = Editor::multi_line().with_rope(Rope::from("foo bar foo baz"));

        bar.open(true, "foo", &ts);
        assert!(bar.is_open());
        assert_eq!(bar.query.value().as_str(), "foo");

        let s1 = bar.step(&mut ed, true);
        assert_eq!(s1, FindOutcome { searched: true, replaced: false, arm_notice: false });
        assert_eq!(ed.selected_range(), 0..3);

        let s2 = bar.step(&mut ed, true);
        assert_eq!(s2, FindOutcome { searched: true, replaced: false, arm_notice: false });
        assert_eq!(ed.selected_range(), 8..11);

        let s3 = bar.step(&mut ed, true);
        assert_eq!(s3, FindOutcome { searched: true, replaced: false, arm_notice: true });
        assert_eq!(ed.selected_range(), 0..3);

        bar.replacement.set_value("qux", &ts);
        let r1 = bar.replace_next(&mut ed, &ts);
        assert!(r1.replaced);
        assert_eq!(ed.value().as_str(), "qux bar foo baz");
        assert_eq!(ed.selected_range(), 8..11);

        bar.query.set_value("o", &ts);
        let ra = bar.replace_all(&mut ed, &ts);
        assert_eq!(ra, FindOutcome { searched: true, replaced: true, arm_notice: false });
        assert_eq!(ed.value().as_str(), "qux bar fquxqux baz");

        bar.query.set_value("missing", &ts);
        let miss = bar.step(&mut ed, true);
        assert_eq!(miss, FindOutcome { searched: true, replaced: false, arm_notice: true });
        assert!(bar.on_notice_dismiss());
        assert!(!bar.on_notice_dismiss());
    }
}
