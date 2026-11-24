use std::ops::Range;
use std::time::Duration;

use crate::app::AppEvent;
use crate::command::AppCommand;
use crate::editor::{Editor, Rope};
use crate::geom::{Point, point};
use crate::gfx::TextSystem;
use crate::ids::{AnimKey, CustomDraw, EditorSlot, HitId, Node, UiState};
use crate::menu::Entry;
use crate::platform::{CursorStyle, Jobs, TimerHandle, WindowId};
use crate::tabs::CLOSE_GLYPH;
use crate::theme::{Palette, theme_shadow};
use crate::types::SharedStr;
use crate::ui::controls::{
    BODY_LINE, BODY_SIZE, ButtonStyle, CONTROL_HEIGHT, CONTROL_RADIUS, FIELD_CARET,
    IconButtonStyle, OVERLAY_RADIUS, TEXT_FONT, TextFieldStyle, button, icon_button,
    text_field_frame,
};
use crate::ui::{AnimStatus, Easing, Presence, box_, col, row, text};

const EXPAND_DURATION: Duration = Duration::from_millis(300);
const CONTRACT_DURATION: Duration = Duration::from_millis(200);
const CARD_Z: f32 = 32.;
const NOTICE_Z: f32 = 16.;
const COLLAPSED_SIZE: f32 = 20.;
const EXPAND_START_SCALE: f32 = 0.01;
const NOTICE_DURATION: Duration = Duration::from_secs(3);
const BLOCK_SIZE: usize = 256 * 1024;
const MAX_UTF8_LEN: usize = 4;
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

#[inline]
fn fold_char(c: char) -> char {
    if c.is_ascii() {
        return c.to_ascii_lowercase();
    }
    match c {
        '\u{017F}' => 's',
        '\u{03C2}' => '\u{03C3}',
        '\u{03D0}' => '\u{03B2}',
        '\u{03D1}' => '\u{03B8}',
        '\u{03D5}' => '\u{03C6}',
        '\u{03D6}' => '\u{03C0}',
        '\u{03F0}' => '\u{03BA}',
        '\u{03F1}' => '\u{03C1}',
        '\u{03F5}' => '\u{03B5}',
        '\u{1E9B}' => '\u{1E61}',
        '\u{2126}' => '\u{03C9}',
        '\u{212A}' => 'k',
        '\u{212B}' => '\u{00E5}',
        _ => {
            let mut lower = c.to_lowercase();
            let first = lower.next().unwrap_or(c);
            if lower.next().is_none() { first } else { c }
        }
    }
}

#[inline]
fn match_rest_folded(haystack: &str, start: usize, rest: &[char]) -> Option<usize> {
    if rest.is_empty() {
        return Some(start);
    }
    let mut chars = haystack[start..].char_indices();
    for &expected in rest {
        let (_, ch) = chars.next()?;
        if fold_char(ch) != expected {
            return None;
        }
    }
    let end = chars
        .next()
        .map_or(haystack.len(), |(rel, _)| start + rel);
    Some(end)
}

pub(crate) struct Matcher {
    query: String,
    match_case: bool,
    folded: Vec<char>,
    overlap: usize,
    block: usize,
}

impl Matcher {
    pub(crate) fn new(query: &str, match_case: bool) -> Option<Self> {
        if query.is_empty() {
            return None;
        }
        let folded = if match_case {
            Vec::new()
        } else {
            query.chars().map(fold_char).collect()
        };
        let overlap = query.chars().count() * MAX_UTF8_LEN;
        Some(Self {
            query: query.to_string(),
            match_case,
            folded,
            overlap,
            block: BLOCK_SIZE.max(4 * overlap),
        })
    }

    fn fill(text: &Rope, range: Range<usize>, buffer: &mut String) -> usize {
        let start = text.floor_char_boundary(range.start);
        let end = text.ceil_char_boundary(range.end.min(text.len()));
        buffer.clear();
        for chunk in text.slice(start..end).chunks() {
            buffer.push_str(chunk);
        }
        start
    }

    fn find_at(&self, haystack: &str, at: usize) -> Option<Range<usize>> {
        if at >= haystack.len() {
            return None;
        }
        let at = haystack.ceil_char_boundary(at);
        if self.match_case {
            return haystack[at..]
                .find(&self.query)
                .map(|off| (at + off)..(at + off + self.query.len()));
        }
        let (&first, rest) = self.folded.split_first()?;
        if first.is_ascii() {
            let lower = first as u8;
            let upper = lower.to_ascii_uppercase();
            let check_non_ascii = lower == b'k' || lower == b's';
            let bytes = haystack.as_bytes();
            let mut i = at;
            while i < bytes.len() {
                let b = bytes[i];
                if b < 0x80 {
                    if (b == lower || b == upper)
                        && let Some(end) = match_rest_folded(haystack, i + 1, rest)
                    {
                        return Some(i..end);
                    }
                    i += 1;
                } else {
                    let ch = haystack[i..].chars().next()?;
                    let next = i + ch.len_utf8();
                    if check_non_ascii
                        && fold_char(ch) == first
                        && let Some(end) = match_rest_folded(haystack, next, rest)
                    {
                        return Some(i..end);
                    }
                    i = next;
                }
            }
            None
        } else {
            for (rel, ch) in haystack[at..].char_indices() {
                if fold_char(ch) == first {
                    let start = at + rel;
                    if let Some(end) = match_rest_folded(haystack, start + ch.len_utf8(), rest) {
                        return Some(start..end);
                    }
                }
            }
            None
        }
    }

    pub(crate) fn is_match(&self, text: &str) -> bool {
        if self.match_case {
            text == self.query
        } else {
            text.chars()
                .map(fold_char)
                .eq(self.folded.iter().copied())
        }
    }

    pub(crate) fn max_match_bytes(&self) -> usize {
        if self.match_case {
            self.query.len()
        } else {
            self.folded.len().saturating_mul(MAX_UTF8_LEN)
        }
    }

    pub(crate) fn next(
        &self,
        text: &Rope,
        from: usize,
        limit: usize,
        buffer: &mut String,
    ) -> Option<Range<usize>> {
        let len = text.len();
        let limit = limit.min(len);
        let mut pos = from.min(len);
        loop {
            if pos >= limit {
                return None;
            }
            let end = (pos + self.block + self.overlap)
                .min(len)
                .min(limit + self.overlap);
            let start = Self::fill(text, pos..end, buffer);
            let haystack = buffer.as_str();
            let safe = if end == len {
                limit
            } else {
                (end - self.overlap).min(limit)
            };
            if let Some(found) = self.find_at(haystack, pos - start) {
                let range = start + found.start..start + found.end;
                if range.start < safe {
                    return Some(range);
                }
            }
            if end == len {
                return None;
            }
            pos = end - self.overlap;
        }
    }

    pub(crate) fn previous(
        &self,
        text: &Rope,
        before: usize,
        floor: usize,
        buffer: &mut String,
    ) -> Option<Range<usize>> {
        let len = text.len();
        let floor = floor.min(len);
        let mut before = before.min(len);
        loop {
            let begin = before.saturating_sub(self.block).max(floor);
            let start = Self::fill(text, begin..(before + self.overlap).min(len), buffer);
            let haystack = buffer.as_str();
            let limit = before - start;
            let mut found = None;
            let mut cursor = 0;
            while let Some(hit) = self.find_at(haystack, cursor) {
                if hit.start >= limit {
                    break;
                }
                cursor = hit.end;
                found = Some(hit);
            }
            while let Some(later) = found
                .as_ref()
                .and_then(|range| self.find_at(haystack, range.start + 1))
                .filter(|later| later.start < limit)
            {
                found = Some(later);
            }
            if let Some(range) = found {
                return Some(start + range.start..start + range.end);
            }
            if begin <= floor {
                return None;
            }
            before = begin;
        }
    }

    pub(crate) fn all(&self, text: &Rope, buffer: &mut String) -> Vec<Range<usize>> {
        let len = text.len();
        let mut ranges = Vec::new();
        let mut pos = 0;
        loop {
            let end = (pos + self.block + self.overlap).min(len);
            let start = Self::fill(text, pos..end, buffer);
            let haystack = buffer.as_str();
            let safe = if end == len {
                usize::MAX
            } else {
                end - self.overlap
            };
            let mut resume = safe;
            let mut cursor = pos - start;
            while let Some(found) = self.find_at(haystack, cursor) {
                let range = start + found.start..start + found.end;
                if range.start >= safe {
                    break;
                }
                resume = resume.max(range.end);
                ranges.push(range);
                cursor = found.end;
            }
            if end == len {
                return ranges;
            }
            pos = resume;
        }
    }

    pub(crate) fn search(
        &self,
        text: &Rope,
        selection: Range<usize>,
        forward: bool,
        wrap: bool,
        buffer: &mut String,
    ) -> Option<(Range<usize>, bool)> {
        if forward {
            if let Some(range) = self.next(text, selection.end, usize::MAX, buffer) {
                return Some((range, false));
            }
            if !wrap {
                return None;
            }
            self.next(text, 0, selection.end, buffer)
                .map(|range| (range, true))
        } else {
            if let Some(range) = self.previous(text, selection.start, 0, buffer) {
                return Some((range, false));
            }
            if !wrap {
                return None;
            }
            self.previous(text, text.len(), selection.start, buffer)
                .map(|range| (range, true))
        }
    }
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

    pub(crate) fn show_notice(
        &mut self,
        notice: impl Into<SharedStr>,
        window: WindowId,
        jobs: &Jobs<AppEvent>,
    ) {
        if !self.is_open() {
            return;
        }
        self.notice = Some(notice.into());
        self.notice_timer = jobs.after(NOTICE_DURATION, AppEvent::FindNoticeDismiss { window });
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

    pub(crate) fn options_entries(&self) -> Vec<Entry> {
        vec![
            Entry::toggle("Match case", self.match_case, AppCommand::ToggleMatchCase),
            Entry::toggle("Wrap around", self.wrap_around, AppCommand::ToggleWrapAround),
        ]
    }

    pub(crate) fn prepare(&mut self) -> Option<String> {
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

    pub(crate) fn step(
        &mut self,
        editor: &mut Editor,
        forward: bool,
        window: WindowId,
        jobs: &Jobs<AppEvent>,
    ) -> bool {
        let Some(query) = self.prepare() else {
            return false;
        };
        let text = editor.text().clone();
        let selection = editor.selected_range();
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
                    self.show_notice(msg, window, jobs);
                } else {
                    self.notice = None;
                }
            }
            None => self.show_notice(not_found(&query), window, jobs),
        }
        true
    }

    pub(crate) fn replace_next(
        &mut self,
        editor: &mut Editor,
        window: WindowId,
        ts: &TextSystem,
        jobs: &Jobs<AppEvent>,
    ) {
        if self.prepare().is_none() {
            return;
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
        let _ = self.step(editor, true, window, jobs);
    }

    pub(crate) fn replace_all(
        &mut self,
        editor: &mut Editor,
        window: WindowId,
        ts: &TextSystem,
        jobs: &Jobs<AppEvent>,
    ) -> bool {
        let Some(query) = self.prepare() else {
            return false;
        };
        let text = editor.text().clone();
        let cursor = editor.cursor();
        let ranges = match self.matcher.as_ref() {
            Some(m) => m.all(&text, &mut self.buffer),
            None => return false,
        };
        if ranges.is_empty() {
            self.show_notice(not_found(&query), window, jobs);
            return false;
        }
        let replacement = self.replacement.value().to_string();
        let next_cursor = cursor_after_replace_all(cursor, &ranges, replacement.len());
        let rebuilt = rebuild_replaced_rope(&text, &ranges, &replacement);
        editor.replace_all_bulk(rebuilt, next_cursor, ts);
        true
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
                col()
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
        let query_slot = EditorSlot::FindQuery;
        let query_editor = self.query.view(
            HitId::Editor(query_slot),
            CustomDraw::Editor(query_slot),
            None,
            BODY_LINE + 2.,
        );
        let query_field = text_field_frame(
            p,
            HitId::Editor(query_slot),
            HitId::FieldClear(query_slot),
            TextFieldStyle {
                surface: p.card_surface,
                focused: focused_slot == Some(query_slot),
                empty: self.query.text().len() == 0,
                search_icon: true,
            },
            query_editor,
            ui,
        );
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
        let replace_slot = EditorSlot::FindReplace;
        let replace_editor = self.replacement.view(
            HitId::Editor(replace_slot),
            CustomDraw::Editor(replace_slot),
            None,
            BODY_LINE + 2.,
        );
        let replace_field = text_field_frame(
            p,
            HitId::Editor(replace_slot),
            HitId::FieldClear(replace_slot),
            TextFieldStyle {
                surface: p.card_surface,
                focused: focused_slot == Some(replace_slot),
                empty: self.replacement.text().len() == 0,
                search_icon: false,
            },
            replace_editor,
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

fn cursor_after_replace_all(
    cursor: usize,
    ranges: &[Range<usize>],
    replacement_len: usize,
) -> usize {
    let mut offset = cursor;
    for range in ranges {
        if range.end <= cursor {
            offset = offset - range.len() + replacement_len;
        } else if range.start < cursor {
            let inside = cursor - range.start;
            offset = offset - inside + inside.min(replacement_len);
            break;
        } else {
            break;
        }
    }
    offset
}

fn rebuild_replaced_rope(text: &Rope, ranges: &[Range<usize>], replacement: &str) -> Rope {
    let mut builder = ropey::RopeBuilder::new();
    let mut cursor = 0usize;
    for range in ranges {
        if range.start > cursor {
            for chunk in text.slice(cursor..range.start).chunks() {
                builder.append(chunk);
            }
        }
        if !replacement.is_empty() {
            builder.append(replacement);
        }
        cursor = range.end;
    }
    if cursor < text.len() {
        for chunk in text.slice(cursor..text.len()).chunks() {
            builder.append(chunk);
        }
    }
    builder.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunked_matcher(query: &str, match_case: bool, block: usize) -> Matcher {
        let mut matcher = Matcher::new(query, match_case).unwrap();
        matcher.block = block;
        matcher
    }

    #[test]
    fn chunked_search_matches_across_block_boundaries() {
        let text = Rope::from("alpha beta alpha gamma alpha");
        let matcher = chunked_matcher("alpha", true, 4);
        let mut buffer = String::new();

        assert_eq!(matcher.all(&text, &mut buffer), vec![0..5, 11..16, 23..28]);
        assert_eq!(
            matcher.search(&text, 0..0, true, false, &mut buffer),
            Some((0..5, false))
        );
        assert_eq!(
            matcher.search(&text, 23..28, true, true, &mut buffer),
            Some((0..5, true))
        );
        assert_eq!(
            matcher.search(&text, 23..28, true, false, &mut buffer),
            None
        );
        assert_eq!(
            matcher.search(&text, 11..16, false, false, &mut buffer),
            Some((0..5, false))
        );
        assert_eq!(
            matcher.search(&text, 0..5, false, true, &mut buffer),
            Some((23..28, true))
        );
    }

    #[test]
    fn case_insensitive_unicode_folding_matches_across_chunk_boundary() {
        let text = Rope::from("abc\u{212A}def");
        let matcher = chunked_matcher("k", false, 2);
        let mut buffer = String::new();

        assert_eq!(matcher.all(&text, &mut buffer), vec![3..6]);
        assert_eq!(matcher.next(&text, 0, usize::MAX, &mut buffer), Some(3..6));
        assert_eq!(matcher.previous(&text, text.len(), 0, &mut buffer), Some(3..6));
    }

    #[test]
    fn previous_finds_rightmost_overlapping_match() {
        let text = Rope::from("aaaa");
        let matcher = chunked_matcher("aa", true, 2);
        let mut buffer = String::new();

        assert_eq!(matcher.previous(&text, 2, 0, &mut buffer), Some(1..3));
        assert_eq!(matcher.previous(&text, 1, 0, &mut buffer), Some(0..2));
        assert_eq!(matcher.previous(&text, 0, 0, &mut buffer), None);
    }

    #[test]
    fn cursor_after_replace_all_handles_inside_and_around_matches() {
        let ranges = vec![0..3, 8..11];
        assert_eq!(cursor_after_replace_all(0, &ranges, 1), 0);
        assert_eq!(cursor_after_replace_all(2, &ranges, 1), 1);
        assert_eq!(cursor_after_replace_all(2, &ranges, 0), 0);
        assert_eq!(cursor_after_replace_all(2, &ranges, 5), 2);
        assert_eq!(cursor_after_replace_all(3, &ranges, 1), 1);
        assert_eq!(cursor_after_replace_all(5, &ranges, 1), 3);
        assert_eq!(cursor_after_replace_all(10, &ranges, 1), 7);
        assert_eq!(cursor_after_replace_all(10, &ranges, 0), 5);
        assert_eq!(cursor_after_replace_all(11, &ranges, 1), 7);
    }

    #[test]
    fn rebuild_replaced_rope_streams_non_overlapping_ranges() {
        let text = Rope::from("aa bb cc aa dd aa");
        let rebuilt = rebuild_replaced_rope(&text, &[0..2, 9..11, 15..17], "XYZ");
        assert_eq!(rebuilt.to_string(), "XYZ bb cc XYZ dd XYZ");
    }
}
