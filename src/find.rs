//! Find and replace: the bar over the document, with its query and replacement fields, and the
//! search behind it.
//!
//! The bar grows in from its corner and shrinks away; a notice under it says when a search
//! wrapped or found nothing. Searching reads the document a block at a time (overlapping so a
//! match across blocks isn't missed), matching exactly or with simple case folding.

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

// How the bar grows in and shrinks away.
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
const COLLAPSED_SIZE: f32 = 20.;
const EXPAND_START_SCALE: f32 = 0.01;
const NOTICE_DURATION: Duration = Duration::from_secs(3);
/// The longest selection that seeds the query.
const MAX_SEED_QUERY_CHARS: usize = 256;

// The card and its notice.
const CARD_Z: f32 = 32.;
const NOTICE_Z: f32 = 16.;
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

// ---- 1. The bar -----------------------------------------------------------------------------

/// What a key in the bar does: nothing here, move focus, or run a command.
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
    /// The matcher for the last query searched, and the text it searches a block at a time.
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

    /// Opens the bar (with the replace row or without), its query seeded with the selection if
    /// that's one short line, and selected.
    pub(crate) fn open(&mut self, replace: bool, selected: &str, ts: &TextSystem) {
        let short = selected.chars().take(MAX_SEED_QUERY_CHARS + 1).count() <= MAX_SEED_QUERY_CHARS;
        if !selected.is_empty() && !selected.contains('\n') && short {
            self.query.set_value(selected, ts);
        }
        self.transition.open();
        self.replace = replace;
        self.query.select_all();
    }

    /// Starts closing the bar; whether it was open.
    pub(crate) fn close(&mut self) -> bool {
        let closed = self.transition.close();
        if closed {
            self.clear_notice();
        }
        closed
    }

    /// Tab moves between the fields while replace shows; Enter searches down, Shift+Enter up,
    /// and in the replacement field replaces.
    pub(crate) fn on_key(&self, ev: KeyEvent, focus: Focus) -> FindKey {
        let m = ev.modifiers;
        if m.ctrl || m.alt || m.win {
            return FindKey::PassThrough;
        }
        let (query, replacement) = (
            Focus::Editor(EditorSlot::FindQuery),
            Focus::Editor(EditorSlot::FindReplace),
        );
        match (ev.key, focus) {
            (Key::Tab, f) if self.replace && f == query => FindKey::Focus(replacement),
            (Key::Tab, f) if self.replace && f == replacement => FindKey::Focus(query),
            (Key::Enter, f) if f == query && !m.shift => FindKey::Command(AppCommand::FindNext),
            (Key::Enter, f) if f == query => FindKey::Command(AppCommand::FindPrevious),
            (Key::Enter, f) if f == replacement && !m.shift => {
                FindKey::Command(AppCommand::ReplaceNext)
            }
            _ => FindKey::PassThrough,
        }
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

    pub(crate) fn clear_notice(&mut self) {
        self.notice = None;
        self.notice_until = None;
    }

    /// Shows `notice` under the bar for a while, if the bar is open.
    fn show_notice(&mut self, notice: impl Into<SharedStr>) {
        if self.is_open() {
            self.notice = Some(notice.into());
            self.notice_until = Some(Instant::now() + NOTICE_DURATION);
        }
    }

    /// Dismisses the notice once it's due; whether that changed anything.
    pub(crate) fn tick(&mut self, now: Instant) -> bool {
        take_due(&mut self.notice_until, now) && self.notice.take().is_some()
    }

    pub(crate) fn deadline(&self) -> Option<Instant> {
        self.notice_until
    }
}

// ---- 2. Searching and replacing -------------------------------------------------------------

impl FindBar {
    /// The query, with the matcher brought up to date for it; `None` while it's empty.
    fn prepare(&mut self) -> Option<String> {
        let query = self.query.model().text().to_string();
        let current = self.matcher.as_ref();
        if current.is_none_or(|m| m.query != query || m.match_case != self.match_case) {
            self.matcher = Matcher::new(&query, self.match_case);
        }
        self.matcher.as_ref().map(|_| query)
    }

    /// Selects the next match down or up from the selection (wrapping, if that's on), noting a
    /// wrap or a miss under the bar.
    pub(crate) fn step(&mut self, editor: &mut Editor, forward: bool) -> FindOutcome {
        let Some(query) = self.prepare() else {
            return FindOutcome::EmptyQuery;
        };
        let (text, selection) = (editor.model().text(), editor.model().selection().range());
        let matcher = self.matcher.as_ref().expect("prepared");
        let found = matcher.search(text, selection, forward, self.wrap_around, &mut self.buffer);
        let Some((range, wrapped)) = found else {
            self.show_notice(format!("Cannot find \"{query}\""));
            return FindOutcome::NotFound;
        };
        editor.set_selected_range(range);
        match (wrapped, forward) {
            (false, _) => self.clear_notice(),
            (true, true) => self.show_notice("Found next from the top"),
            (true, false) => self.show_notice("Found previous from the bottom"),
        }
        FindOutcome::Found { wrapped }
    }

    /// Replaces the selection if it's a match, then finds the next one.
    pub(crate) fn replace_next(&mut self, editor: &mut Editor, ts: &TextSystem) -> FindOutcome {
        if self.prepare().is_none() {
            return FindOutcome::EmptyQuery;
        }
        let matcher = self.matcher.as_ref().expect("prepared");
        let model = editor.model();
        let selection = model.selection().range();
        let is_match = !selection.is_empty()
            && selection.len() <= matcher.max_match_bytes()
            && matcher.is_match(&model.text().slice(selection).to_string());
        if is_match {
            let replacement = self.replacement.model().text().to_string();
            editor.replace(&replacement, ts);
        }
        let found = self.step(editor, true);
        if is_match {
            FindOutcome::Replaced(1)
        } else {
            found
        }
    }

    /// Replaces every match, as one edit.
    pub(crate) fn replace_all(&mut self, editor: &mut Editor, ts: &TextSystem) -> FindOutcome {
        let Some(query) = self.prepare() else {
            return FindOutcome::EmptyQuery;
        };
        let matcher = self.matcher.as_ref().expect("prepared");
        let ranges = matcher.all(editor.model().text(), &mut self.buffer);
        if ranges.is_empty() {
            self.show_notice(format!("Cannot find \"{query}\""));
            return FindOutcome::NotFound;
        }
        let replacement = self.replacement.model().text().to_string();
        editor.replace_all(&ranges, &replacement, ts);
        FindOutcome::Replaced(ranges.len())
    }
}

// ---- 3. Drawing -----------------------------------------------------------------------------

impl FindBar {
    /// How far the bar has grown in or shrunk away, or `None` once it's gone.
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

    /// The bar, centred at the top of the document with room for its shadow, and its notice.
    /// Kept out of line: inlined into the window's frame it duplicates kilobytes of builders.
    #[inline(never)]
    pub(crate) fn render(
        &mut self,
        focused_slot: Option<EditorSlot>,
        p: Palette,
        ui: &mut AppUi,
    ) -> Option<AppNode> {
        let transform = self.sample(ui)?;
        let notice = self.notice.clone().filter(|_| self.transition.is_open());
        let card = self
            .render_card(p, focused_slot, ui)
            .transform(transform)
            .when(self.transition.is_closing(), AppNode::inert);
        let lane = row()
            .absolute()
            .top(CARD_TOP - CARD_SHADOW_CLIP_TOP)
            .left(0.)
            .right(0.)
            .px(CARD_SIDE_MARGIN)
            .pt(CARD_SHADOW_CLIP_TOP)
            .pb(CARD_SHADOW_CLIP_BOTTOM)
            .overflow_hidden()
            .justify_center()
            .child(card);
        let notice = notice.map(|msg| render_notice(p, msg));
        Some(
            box_()
                .absolute()
                .inset_0()
                .child(lane)
                .when_some(notice, AppNode::child),
        )
    }

    /// The card: the query row and, with replace on, the replacement row.
    fn render_card(&self, p: Palette, focused: Option<EditorSlot>, ui: &mut AppUi) -> AppNode {
        let query_row = self.query_row(p, focused, ui);
        let replace_row = self.replace.then(|| self.replace_row(p, focused, ui));
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

    /// The replace toggle, the query field, the search buttons, options and close.
    fn query_row(&self, p: Palette, focused: Option<EditorSlot>, ui: &mut AppUi) -> AppNode {
        let dims = size(ICON_BUTTON_SIZE, ICON_BUTTON_SIZE);
        let mut icon = |hit, tip, glyph, glyph_size| {
            icon_button(
                p,
                HitId::Find(hit),
                tip,
                (glyph, glyph_size),
                dims,
                true,
                ui,
            )
        };
        let (toggle_tip, toggle_glyph) = if self.replace {
            ("Close replace options", CHEVRON_DOWN_GLYPH)
        } else {
            ("Open replace options", CHEVRON_UP_GLYPH)
        };
        let toggle = icon(
            FindHit::ToggleReplace,
            toggle_tip,
            toggle_glyph,
            CHEVRON_SIZE,
        );
        let focused = focused == Some(EditorSlot::FindQuery);
        let field = text_field(p, p.card_surface, &self.query, focused, true, ui);
        let mut icon = |hit, tip, glyph| {
            icon_button(p, HitId::Find(hit), tip, (glyph, ICON_SIZE), dims, true, ui)
        };
        let next = icon(FindHit::Next, "Search down", ARROW_DOWN_GLYPH);
        let prev = icon(FindHit::Prev, "Search up", ARROW_UP_GLYPH);
        let options = icon(FindHit::Options, "More options", OPTIONS_GLYPH);
        let close = icon(FindHit::Close, "Exit Find and Replace", CLOSE_GLYPH);
        row()
            .items_center()
            .gap(ICON_GAP)
            .child(toggle)
            .child(box_().flex_1().min_w_0().child(field))
            .child(next)
            .child(prev)
            .child(options)
            .child(close)
    }

    /// The replacement field under the query's, and the replace buttons.
    fn replace_row(&self, p: Palette, focused: Option<EditorSlot>, ui: &mut AppUi) -> AppNode {
        let surface = p.card_surface;
        let focused = focused == Some(EditorSlot::FindReplace);
        let field = text_field(p, surface, &self.replacement, focused, false, ui);
        let mut action = |hit, label| {
            let button = button(p, surface, HitId::Find(hit), label, false, false, ui);
            box_()
                .flex_none()
                .ml(ACTION_GAP)
                .w(ACTION_WIDTH)
                .child(button)
        };
        let replace = action(FindHit::Replace, "Replace");
        let replace_all = action(FindHit::ReplaceAll, "Replace all");
        row()
            .items_center()
            .mt(ROW_GAP)
            .child(box_().flex_none().w(ICON_BUTTON_SIZE + ICON_GAP))
            .child(box_().flex_1().min_w_0().child(field))
            .child(replace)
            .child(replace_all)
    }
}

/// The notice, centred under the card.
fn render_notice(p: Palette, msg: SharedStr) -> AppNode {
    fn place(card: Rect, popup: Size, _viewport: Size) -> Point {
        let x = card.center().x - (popup.width / 2.).floor();
        point(x, card.bottom() + BORDER + NOTICE_GAP)
    }
    let face = box_()
        .px(NOTICE_PADDING_X)
        .pt(NOTICE_PADDING_TOP)
        .pb(NOTICE_PADDING_BOTTOM)
        .font_family(TEXT_FONT)
        .text_size(BODY_SIZE)
        .line_height(NOTICE_LINE)
        .text_color(p.text_primary)
        .child(text(msg));
    let anchor = Anchor::Target {
        target: HitId::Find(FindHit::Card),
        place,
    };
    card(
        p,
        p.flyout_stroke,
        p.notice_fill,
        OVERLAY_RADIUS,
        NOTICE_Z,
        face,
    )
    .anchored(anchor)
}

// ---- 4. Matching ----------------------------------------------------------------------------

/// How much of the document is searched at a time, and the most bytes a character takes.
const BLOCK_SIZE: usize = 256 * 1024;
const MAX_UTF8_LEN: usize = 4;

/// `c` folded for a case-insensitive match: lower case where that's one character, with the
/// characters that fold to another's lower case (long s, final sigma, Kelvin, ...) mapped to it.
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

/// Where a folded match of `rest` starting at `start` in `haystack` ends, if it does.
fn match_rest_folded(haystack: &str, start: usize, rest: &[char]) -> Option<usize> {
    let mut chars = haystack[start..].char_indices();
    for &expected in rest {
        let (_, ch) = chars.next()?;
        if fold_char(ch) != expected {
            return None;
        }
    }
    Some(chars.next().map_or(haystack.len(), |(at, _)| start + at))
}

/// A query to search for, exactly or folded.
pub(crate) struct Matcher {
    query: String,
    match_case: bool,
    /// The query folded (for a case-insensitive search).
    folded: Vec<char>,
    /// How far blocks overlap (the longest a match can be), and how big they are.
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

    /// Copies `range` of `text`, widened to whole characters, into `buffer`; where it starts.
    fn fill(text: &Rope, range: Range<usize>, buffer: &mut String) -> usize {
        let start = text.floor_char_boundary(range.start);
        let end = text.ceil_char_boundary(range.end.min(text.len()));
        buffer.clear();
        buffer.extend(text.slice(start..end).chunks());
        start
    }

    /// The first match in `haystack` from `at`.
    fn find_at(&self, haystack: &str, at: usize) -> Option<Range<usize>> {
        if at >= haystack.len() {
            return None;
        }
        let at = haystack.ceil_char_boundary(at);
        if self.match_case {
            let found = haystack[at..].find(&self.query)?;
            return Some(at + found..at + found + self.query.len());
        }
        let (&first, rest) = self.folded.split_first()?;
        if !first.is_ascii() {
            return haystack[at..].char_indices().find_map(|(off, ch)| {
                if fold_char(ch) != first {
                    return None;
                }
                let start = at + off;
                match_rest_folded(haystack, start + ch.len_utf8(), rest).map(|end| start..end)
            });
        }
        // An ASCII first character is looked for byte by byte; only k and s also fold from
        // characters outside ASCII (Kelvin, long s).
        let (lower, upper) = (first as u8, first.to_ascii_uppercase() as u8);
        let folds_from_wide = lower == b'k' || lower == b's';
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
                continue;
            }
            let ch = haystack[i..].chars().next()?;
            let next = i + ch.len_utf8();
            if folds_from_wide
                && fold_char(ch) == first
                && let Some(end) = match_rest_folded(haystack, next, rest)
            {
                return Some(i..end);
            }
            i = next;
        }
        None
    }

    /// Whether `text` is, whole, a match.
    fn is_match(&self, text: &str) -> bool {
        if self.match_case {
            text == self.query
        } else {
            text.chars().map(fold_char).eq(self.folded.iter().copied())
        }
    }

    /// The most bytes a match can take.
    fn max_match_bytes(&self) -> usize {
        if self.match_case {
            self.query.len()
        } else {
            self.folded.len().saturating_mul(MAX_UTF8_LEN)
        }
    }

    /// The first match starting in `from..limit`.
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
        while pos < limit {
            let end = (pos + self.block + self.overlap)
                .min(len)
                .min(limit + self.overlap);
            let start = Self::fill(text, pos..end, buffer);
            // A match starting in the overlap is found whole in the next block.
            let safe = if end == len {
                limit
            } else {
                (end - self.overlap).min(limit)
            };
            if let Some(found) = self.find_at(buffer, pos - start)
                && start + found.start < safe
            {
                return Some(start + found.start..start + found.end);
            }
            if end == len {
                return None;
            }
            pos = end - self.overlap;
        }
        None
    }

    /// The last match starting in `floor..before` (the rightmost, overlapping ones included).
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
            let limit = before - start;
            // The last of the matches that don't overlap, then any later one overlapping it.
            let mut found = None;
            let mut cursor = 0;
            while let Some(hit) = self.find_at(buffer, cursor).filter(|hit| hit.start < limit) {
                cursor = hit.end;
                found = Some(hit);
            }
            while let Some(later) = found
                .as_ref()
                .and_then(|hit| self.find_at(buffer, hit.start + 1))
                .filter(|later| later.start < limit)
            {
                found = Some(later);
            }
            if let Some(hit) = found {
                return Some(start + hit.start..start + hit.end);
            }
            if begin <= floor {
                return None;
            }
            before = begin;
        }
    }

    /// Every match, none overlapping, in order.
    pub(crate) fn all(&self, text: &Rope, buffer: &mut String) -> Vec<Range<usize>> {
        let len = text.len();
        let mut ranges = Vec::new();
        let mut pos = 0;
        loop {
            let end = (pos + self.block + self.overlap).min(len);
            let start = Self::fill(text, pos..end, buffer);
            let safe = if end == len {
                usize::MAX
            } else {
                end - self.overlap
            };
            // The next block starts past the last match taken, or at the overlap.
            let mut resume = safe;
            let mut cursor = pos - start;
            while let Some(found) = self.find_at(buffer, cursor) {
                let range = start + found.start..start + found.end;
                if range.start >= safe {
                    break;
                }
                resume = resume.max(range.end);
                cursor = found.end;
                ranges.push(range);
            }
            if end == len {
                return ranges;
            }
            pos = resume;
        }
    }

    /// The next match down from the selection's end (or up from its start), then, with `wrap`,
    /// from the other end of the document; and whether it wrapped.
    pub(crate) fn search(
        &self,
        text: &Rope,
        selection: Range<usize>,
        forward: bool,
        wrap: bool,
        buffer: &mut String,
    ) -> Option<(Range<usize>, bool)> {
        let first = if forward {
            self.next(text, selection.end, usize::MAX, buffer)
        } else {
            self.previous(text, selection.start, 0, buffer)
        };
        if let Some(range) = first {
            return Some((range, false));
        }
        if !wrap {
            return None;
        }
        let wrapped = if forward {
            self.next(text, 0, selection.end, buffer)
        } else {
            self.previous(text, text.len(), selection.start, buffer)
        };
        wrapped.map(|range| (range, true))
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
