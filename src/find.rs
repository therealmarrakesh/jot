use std::ops::Range;
use std::time::{Duration, Instant};

use ropey::Rope;

use crate::command::AppCommand;
use crate::editor::{Editor, FieldOptions};
use crate::geom::{Point, Rect, Size, point, size};
use crate::gfx::TextSystem;
use crate::platform::{CursorStyle, Key, KeyEvent};
use crate::theme::{BODY_SIZE, BORDER, OVERLAY_RADIUS, Palette, TEXT_FONT};
use crate::types::{SharedStr, take_due};
use crate::ui::{
    Anchor, AnimStatus, AppNode, AppUi, Curve, Easing, EditorSlot, FindHit, Focus, HitId,
    Transform, Transition, box_, row, text,
};
use crate::widgets::{button, card, icon_button, text_field};

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

impl FindHit {
    /// What clicking the find bar's button runs.
    pub(crate) fn command(self) -> Option<AppCommand> {
        Some(match self {
            Self::Card => return None,
            Self::ToggleReplace => AppCommand::ToggleReplace,
            Self::Next => AppCommand::FindNext,
            Self::Prev => AppCommand::FindPrevious,
            Self::Options => AppCommand::FindOptions,
            Self::Close => AppCommand::Cancel,
            Self::Replace => AppCommand::ReplaceNext,
            Self::ReplaceAll => AppCommand::ReplaceAll,
        })
    }
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
    pub(crate) replace: bool,
    pub(crate) match_case: bool,
    pub(crate) wrap_around: bool,
    pub(crate) query: Editor,
    pub(crate) replacement: Editor,
    notice: Option<SharedStr>,
    /// When the notice goes away.
    notice_until: Option<Instant>,
    matcher: Option<Matcher>,
    buffer: String,
}

impl FindBar {
    pub(crate) fn new() -> Self {
        let field = |slot, placeholder: &'static str| {
            Editor::field(FieldOptions {
                slot,
                placeholder: placeholder.into(),
                validate: None,
            })
        };
        Self {
            transition: Transition::default(),
            replace: false,
            match_case: false,
            wrap_around: true,
            query: field(EditorSlot::FindQuery, "Find"),
            replacement: field(EditorSlot::FindReplace, "Replace"),
            notice: None,
            notice_until: None,
            matcher: None,
            buffer: String::new(),
        }
    }

    pub(crate) fn is_open(&self) -> bool {
        self.transition.is_open()
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
        self.notice_until = None;
    }

    fn show_notice(&mut self, notice: impl Into<SharedStr>) {
        if !self.is_open() {
            return;
        }
        self.notice = Some(notice.into());
        self.notice_until = Some(Instant::now() + NOTICE_DURATION);
    }

    pub(crate) fn deadline(&self) -> Option<Instant> {
        self.notice_until
    }

    /// Flips one of the bar's options: `ToggleReplace`, `ToggleMatchCase` or `ToggleWrapAround`.
    pub(crate) fn toggle(&mut self, option: AppCommand) {
        let flag = match option {
            AppCommand::ToggleReplace => &mut self.replace,
            AppCommand::ToggleMatchCase => &mut self.match_case,
            AppCommand::ToggleWrapAround => &mut self.wrap_around,
            _ => return,
        };
        *flag = !*flag;
    }

    /// The field focus goes to in the bar, given where it is: the replacement field if that's
    /// showing and has it, else the query.
    pub(crate) fn field_for(&self, focus: Focus) -> EditorSlot {
        if self.replace && focus == Focus::Editor(EditorSlot::FindReplace) {
            EditorSlot::FindReplace
        } else {
            EditorSlot::FindQuery
        }
    }

    /// Dismisses the notice once it's due; whether that changed anything.
    pub(crate) fn tick(&mut self, now: Instant) -> bool {
        take_due(&mut self.notice_until, now) && self.notice.take().is_some()
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
        let query = self.query.model().text().to_string();
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
            return FindOutcome::EmptyQuery;
        };
        let text = editor.model().text().clone();
        let selection = editor.model().selection().range();
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
                    self.show_notice(msg);
                } else {
                    self.clear_notice();
                }
                FindOutcome::Found { wrapped }
            }
            None => {
                self.show_notice(not_found(&query));
                FindOutcome::NotFound
            }
        }
    }

    pub(crate) fn replace_next(&mut self, editor: &mut Editor, ts: &TextSystem) -> FindOutcome {
        if self.prepare().is_none() {
            return FindOutcome::EmptyQuery;
        }
        let sel_len = editor.model().selection().range().len();
        let is_match = self.matcher.as_ref().is_some_and(|m| {
            sel_len > 0
                && sel_len <= m.max_match_bytes()
                && m.is_match(&editor.model().text().slice(editor.model().selection().range()).to_string())
        });
        if is_match {
            let replacement = self.replacement.model().text().to_string();
            editor.replace(&replacement, ts);
        }
        let out = self.step(editor, true);
        if is_match {
            FindOutcome::Replaced(1)
        } else {
            out
        }
    }

    pub(crate) fn replace_all(&mut self, editor: &mut Editor, ts: &TextSystem) -> FindOutcome {
        let Some(query) = self.prepare() else {
            return FindOutcome::EmptyQuery;
        };
        let text = editor.model().text().clone();
        let ranges = match self.matcher.as_ref() {
            Some(m) => m.all(&text, &mut self.buffer),
            None => return FindOutcome::EmptyQuery,
        };
        if ranges.is_empty() {
            self.show_notice(not_found(&query));
            return FindOutcome::NotFound;
        }
        let count = ranges.len();
        let replacement = self.replacement.model().text().to_string();
        editor.replace_all(&ranges, &replacement, ts);
        FindOutcome::Replaced(count)
    }

    fn sample(&mut self, ui: &mut AppUi) -> Option<Transform> {
        if !self.transition.is_visible() {
            return None;
        }
        let t = self.transition;
        let closing = t.is_closing();
        let shown = t.progress(ui, 0, true, EXPAND_CURVE).value;
        let hidden = t.progress(ui, 1, closing, CONTRACT_CURVE);
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
        ui: &mut AppUi,
    ) -> Option<AppNode> {
        let transform = self.sample(ui)?;
        let notice_node = self
            .notice
            .clone()
            .filter(|_| self.transition.is_open())
            .map(|msg| render_notice(p, msg));
        let card = self
            .render_card(p, focused_slot, ui)
            .transform(transform)
            .when(self.transition.is_closing(), AppNode::inert);

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
                .when_some(notice_node, AppNode::child),
        )
    }

    fn render_card(&self, p: Palette, focused_slot: Option<EditorSlot>, ui: &mut AppUi) -> AppNode {
        let query_row = self.render_query_row(p, focused_slot, ui);
        let replace_row = self
            .replace
            .then(|| self.render_replace_row(p, focused_slot, ui));

        let face = box_()
            .hit(HitId::Find(FindHit::Card))
            .cursor(CursorStyle::Arrow)
            .w_full()
            .p(CARD_PADDING)
            .child(query_row)
            .when_some(replace_row, AppNode::child);
        let fill = p.card_surface.panel;
        card(p, p.surface_stroke, fill, OVERLAY_RADIUS, CARD_Z, face)
            .w(CARD_WIDTH)
            .max_w_full()
    }

    fn render_query_row(
        &self,
        p: Palette,
        focused_slot: Option<EditorSlot>,
        ui: &mut AppUi,
    ) -> AppNode {
        let dims = size(ICON_BUTTON_SIZE, ICON_BUTTON_SIZE);
        let btn = |hit, tip, glyph, sz, ui: &mut AppUi| {
            icon_button(p, HitId::Find(hit), tip, (glyph, sz), dims, true, ui)
        };
        let replace = self.replace;
        let expand_btn = btn(
            FindHit::ToggleReplace,
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
        let next_btn = btn(FindHit::Next, "Search down", ARROW_DOWN_GLYPH, ICON_SIZE, ui);
        let prev_btn = btn(FindHit::Prev, "Search up", ARROW_UP_GLYPH, ICON_SIZE, ui);
        let opts_btn = btn(FindHit::Options, "More options", OPTIONS_GLYPH, ICON_SIZE, ui);
        let close_btn = btn(
            FindHit::Close,
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
        ui: &mut AppUi,
    ) -> AppNode {
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
            HitId::Find(FindHit::Replace),
            "Replace",
            false,
            false,
            ui,
        );
        let replace_all_btn = button(
            p,
            p.card_surface,
            HitId::Find(FindHit::ReplaceAll),
            "Replace all",
            false,
            false,
            ui,
        );

        let action = |btn| box_().flex_none().ml(ACTION_GAP).w(ACTION_WIDTH).child(btn);
        row()
            .items_center()
            .mt(ROW_GAP)
            .child(box_().flex_none().w(ICON_BUTTON_SIZE + ICON_GAP))
            .child(box_().flex_1().min_w_0().child(replace_field))
            .child(action(replace_btn))
            .child(action(replace_all_btn))
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

fn render_notice(p: Palette, msg: SharedStr) -> AppNode {
    let face = box_()
        .px(NOTICE_PADDING_X)
        .pt(NOTICE_PADDING_TOP)
        .pb(NOTICE_PADDING_BOTTOM)
        .font_family(TEXT_FONT)
        .text_size(BODY_SIZE)
        .line_height(NOTICE_LINE)
        .text_color(p.text_primary)
        .child(text(msg));
    let (stroke, fill) = (p.flyout_stroke, p.notice_fill);
    card(p, stroke, fill, OVERLAY_RADIUS, NOTICE_Z, face).anchored(Anchor::Target {
        target: HitId::Find(FindHit::Card),
        place: place_notice,
    })
}

const BLOCK_SIZE: usize = 256 * 1024;
const MAX_UTF8_LEN: usize = 4;

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

#[cfg(test)]
mod tests {

    use crate::editor::DocOptions;
    use crate::text::Rope;
    use crate::geom::Edges;

    use super::*;

    #[test]
    fn find_bar_step_wrap_replace_next_and_replace_all_return_outcomes() {
        let ts = TextSystem::new().unwrap();
        let mut bar = FindBar::new();
        let mut ed = Editor::document(
            Rope::from("foo bar foo baz"),
            DocOptions {
                tab: crate::ui::TabId::next(),
                soft_wrap: true,
                paddings: Edges::ZERO,
            },
        );

        bar.open(true, "foo", &ts);
        assert!(bar.is_open());
        assert_eq!(bar.query.model().text().to_string(), "foo");

        assert_eq!(
            bar.step(&mut ed, true),
            FindOutcome::Found { wrapped: false }
        );
        assert_eq!(ed.model().selection().range(), 0..3);

        assert_eq!(
            bar.step(&mut ed, true),
            FindOutcome::Found { wrapped: false }
        );
        assert_eq!(ed.model().selection().range(), 8..11);

        assert_eq!(
            bar.step(&mut ed, true),
            FindOutcome::Found { wrapped: true }
        );
        assert_eq!(ed.model().selection().range(), 0..3);
        let due = bar.notice_until.expect("a wrapped search shows a notice");
        assert!(!bar.tick(due - Duration::from_millis(1)));
        assert!(bar.tick(due));
        assert!(bar.notice.is_none() && bar.notice_until.is_none());

        bar.replacement.set_value("qux", &ts);
        assert_eq!(bar.replace_next(&mut ed, &ts), FindOutcome::Replaced(1));
        assert_eq!(ed.model().text().to_string(), "qux bar foo baz");
        assert_eq!(ed.model().selection().range(), 8..11);

        bar.query.set_value("o", &ts);
        assert_eq!(bar.replace_all(&mut ed, &ts), FindOutcome::Replaced(2));
        assert_eq!(ed.model().text().to_string(), "qux bar fquxqux baz");

        bar.query.set_value("missing", &ts);
        assert_eq!(bar.step(&mut ed, true), FindOutcome::NotFound);
        let due = bar.notice_until.expect("a failed search shows a notice");
        assert!(bar.tick(due));
        assert!(!bar.tick(due));
    }

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
        assert_eq!(
            matcher.previous(&text, text.len(), 0, &mut buffer),
            Some(3..6)
        );
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
}
