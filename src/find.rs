use super::*;
use field::{BODY_SIZE, CONTROL_HEIGHT, CONTROL_RADIUS, FieldStyle, OVERLAY_RADIUS};
use gpui_kit::base::{MotionStatus, Presence, Transition, transition_with_status};
use gpui_kit::component::input::InputState;
use menu::Entry;
use regex::bytes::{Regex, RegexBuilder};

const QUERY_CONTEXT: &str = "JotFindQuery";
const REPLACEMENT_CONTEXT: &str = "JotFindReplacement";
const EXPAND_DURATION: Duration = Duration::from_millis(300);
const CONTRACT_DURATION: Duration = Duration::from_millis(200);
const CARD_Z: f32 = 32.;
const NOTICE_Z: f32 = 16.;
const COLLAPSED_SIZE: f32 = 20.;
const EXPAND_START_SCALE: f32 = 0.01;
const NOTICE_DURATION: Duration = Duration::from_secs(3);
const BLOCK_SIZE: usize = 256 * 1024;
const MAX_UTF8_LEN: usize = 4;
const MERGE_GAP: usize = 512;
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
const MIN_VISIBLE_SCALE: f32 = 0.05;
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
const CLOSE_GLYPH: &str = "\u{E711}";

pub(crate) fn key_bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("enter", FindNext, Some(QUERY_CONTEXT)),
        KeyBinding::new("shift-enter", FindPrevious, Some(QUERY_CONTEXT)),
        KeyBinding::new("enter", ReplaceNext, Some(REPLACEMENT_CONTEXT)),
    ]
}

struct Matcher {
    query: String,
    match_case: bool,
    pattern: Regex,
    overlap: usize,
    block: usize,
}

impl Matcher {
    fn new(query: &str, match_case: bool) -> Option<Self> {
        let pattern = RegexBuilder::new(&regex::escape(query))
            .case_insensitive(!match_case)
            .build()
            .ok()?;
        let overlap = query.chars().count() * MAX_UTF8_LEN;
        Some(Self {
            query: query.to_string(),
            match_case,
            pattern,
            overlap,
            block: BLOCK_SIZE.max(4 * overlap),
        })
    }

    fn fill(text: &Rope, range: Range<usize>, buffer: &mut Vec<u8>) -> usize {
        let start = text.floor_char_boundary(range.start);
        let end = text.ceil_char_boundary(range.end.min(text.len()));
        buffer.clear();
        for chunk in text.slice(start..end).chunks() {
            buffer.extend_from_slice(chunk.as_bytes());
        }
        start
    }

    fn is_match(&self, text: &str) -> bool {
        self.pattern
            .find(text.as_bytes())
            .is_some_and(|found| found.range() == (0..text.len()))
    }

    fn next(
        &self,
        text: &Rope,
        from: usize,
        limit: usize,
        buffer: &mut Vec<u8>,
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
            let safe = if end == len {
                limit
            } else {
                (end - self.overlap).min(limit)
            };
            if let Some(found) = self.pattern.find_at(buffer, pos - start) {
                let range = start + found.start()..start + found.end();
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

    fn previous(
        &self,
        text: &Rope,
        before: usize,
        floor: usize,
        buffer: &mut Vec<u8>,
    ) -> Option<Range<usize>> {
        let floor = text.floor_char_boundary(floor.min(text.len()));
        let mut end = text.floor_char_boundary(before.min(text.len()));
        loop {
            let begin = end.saturating_sub(self.block + self.overlap).max(floor);
            let start = Self::fill(text, begin..end, buffer);
            let limit = (end - start).min(buffer.len());
            if let Some(found) = self.pattern.find_iter(&buffer[..limit]).last() {
                let mut range = found.range();
                while let Some(later) = self.pattern.find_at(&buffer[..limit], range.start + 1) {
                    range = later.range();
                }
                return Some(start + range.start..start + range.end);
            }
            if begin <= floor {
                return None;
            }
            end = text.floor_char_boundary(begin + self.overlap);
        }
    }

    fn all(&self, text: &Rope, buffer: &mut Vec<u8>) -> Vec<Range<usize>> {
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
            let mut resume = safe;
            for found in self.pattern.find_iter(&buffer[pos - start..]) {
                let range = pos + found.start()..pos + found.end();
                if range.start >= safe {
                    break;
                }
                resume = resume.max(range.end);
                ranges.push(range);
            }
            if end == len {
                return ranges;
            }
            pos = resume;
        }
    }

    fn search(
        &self,
        text: &Rope,
        selection: Range<usize>,
        forward: bool,
        wrap: bool,
        buffer: &mut Vec<u8>,
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
            let floor = selection.start.saturating_sub(self.overlap);
            self.previous(text, text.len(), floor, buffer)
                .map(|range| (range, true))
        }
    }
}

pub(crate) struct FindBar {
    visible: bool,
    closing: bool,
    replace: bool,
    serial: usize,
    match_case: bool,
    wrap_around: bool,
    focus: FocusHandle,
    query: Entity<InputState>,
    replacement: Entity<InputState>,
    notice: Option<SharedString>,
    notice_timer: Task<()>,
    matcher: Option<Matcher>,
    buffer: Vec<u8>,
    options_bounds: Rc<Cell<Bounds<Pixels>>>,
    _observers: [Subscription; 2],
}

impl FindBar {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Jot>) -> Self {
        let query = cx.new(|cx| InputState::new(window, cx));
        let replacement = cx.new(|cx| InputState::new(window, cx));
        let _observers = [
            Jot::watch_field(&query, cx),
            Jot::watch_field(&replacement, cx),
        ];
        Self {
            visible: false,
            closing: false,
            replace: false,
            serial: 0,
            match_case: false,
            wrap_around: true,
            focus: cx.focus_handle(),
            query,
            replacement,
            notice: None,
            notice_timer: Task::ready(()),
            matcher: None,
            buffer: Vec::new(),
            options_bounds: Rc::new(Cell::new(Bounds::default())),
            _observers,
        }
    }

    fn prepare(&mut self, cx: &App) -> Option<String> {
        let query = self.query.read(cx).value().to_string();
        if query.is_empty() {
            return None;
        }
        let stale = self
            .matcher
            .as_ref()
            .is_none_or(|matcher| matcher.query != query || matcher.match_case != self.match_case);
        if stale {
            self.matcher = Matcher::new(&query, self.match_case);
        }
        self.matcher.as_ref().map(|_| query)
    }

    fn is_open(&self) -> bool {
        self.visible && !self.closing
    }

    fn card_size(&self, window: &Window) -> (f32, f32) {
        let rows = if self.replace { 2. } else { 1. };
        let height =
            2. * BORDER + 2. * CARD_PADDING + rows * CONTROL_HEIGHT + (rows - 1.) * ROW_GAP;
        let available = window.viewport_size().width.as_f32() - 2. * CARD_SIDE_MARGIN;
        (CARD_WIDTH.min(available).max(0.), height)
    }

    fn sample(&mut self, window: &mut Window, cx: &mut App) -> Option<Point<f32>> {
        if !self.visible {
            return None;
        }
        let shown = Presence::new(ElementId::from(("jot-find-show", self.serial)), true)
            .transition(
                Transition::new(EXPAND_DURATION).easing(Easing::CubicBezier {
                    x1: 0.1,
                    y1: 0.9,
                    x2: 0.2,
                    y2: 1.,
                }),
            )
            .sample(window, cx)
            .progress;
        let hidden = transition_with_status(
            ElementId::from(("jot-find-hide", self.serial)),
            if self.closing { 1. } else { 0. },
            Transition::new(CONTRACT_DURATION).easing(Easing::CubicBezier {
                x1: 0.7,
                y1: 0.,
                x2: 1.,
                y2: 0.5,
            }),
            window,
            cx,
        );
        let (width, height) = self.card_size(window);
        let axis = |size: f32| {
            let collapsed = COLLAPSED_SIZE / size.max(COLLAPSED_SIZE);
            if self.closing {
                1. - (1. - collapsed) * hidden.value
            } else {
                let start = collapsed.min(EXPAND_START_SCALE);
                start + (1. - start) * shown
            }
        };
        let scale = point(axis(width), axis(height));
        if self.closing && hidden.status == MotionStatus::Finished {
            self.visible = false;
            return None;
        }
        Some(scale)
    }
}

impl Jot {
    pub(crate) fn find(&mut self, _: &Find, window: &mut Window, cx: &mut Context<Self>) {
        if self.has_text(cx) {
            self.open_find(false, window, cx);
        }
    }

    pub(crate) fn replace(&mut self, _: &Replace, window: &mut Window, cx: &mut Context<Self>) {
        if self.has_text(cx) {
            self.open_find(true, window, cx);
        }
    }

    pub(crate) fn find_next(&mut self, _: &FindNext, window: &mut Window, cx: &mut Context<Self>) {
        if self.has_text(cx) {
            self.find_step(true, window, cx);
        }
    }

    pub(crate) fn find_previous(
        &mut self,
        _: &FindPrevious,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.has_text(cx) {
            self.find_step(false, window, cx);
        }
    }

    pub(crate) fn toggle_match_case(
        &mut self,
        _: &ToggleMatchCase,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.find.match_case = !self.find.match_case;
        self.refocus_find(window, cx);
    }

    pub(crate) fn toggle_wrap_around(
        &mut self,
        _: &ToggleWrapAround,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.find.wrap_around = !self.find.wrap_around;
        self.refocus_find(window, cx);
    }

    pub(crate) fn replace_next(
        &mut self,
        _: &ReplaceNext,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.find.prepare(cx).is_none() {
            return;
        }
        let editor = self.active_editor();
        let selected = editor.read(cx).selected_text().to_string();
        let is_match = self
            .find
            .matcher
            .as_ref()
            .is_some_and(|matcher| matcher.is_match(&selected));
        if is_match {
            let replacement = self.find.replacement.read(cx).value();
            editor.update(cx, |editor, cx| editor.replace(replacement, window, cx));
        }
        self.find_step(true, window, cx);
    }

    pub(crate) fn replace_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(query) = self.find.prepare(cx) else {
            return;
        };
        let editor = self.active_editor().clone();
        let (text, cursor) = {
            let editor = editor.read(cx);
            (editor.text().clone(), editor.cursor())
        };
        let find = &mut self.find;
        let ranges = match find.matcher.as_ref() {
            Some(matcher) => matcher.all(&text, &mut find.buffer),
            None => return,
        };
        if ranges.is_empty() {
            self.show_notice(not_found(&query), cx);
            return;
        }
        let replacement = self.find.replacement.read(cx).value().to_string();
        let before = ranges.iter().take_while(|range| range.end <= cursor);
        let removed: usize = before.clone().map(|range| range.len()).sum();
        let added = before.count() * replacement.len();
        let edits = merge_edits(&text, ranges, &replacement);
        editor.update(cx, |editor, cx| {
            editor.replace_ranges(&edits, window, cx);
            let text = editor.text();
            let cursor = text.floor_char_boundary((cursor - removed + added).min(text.len()));
            editor.set_selected_range(cursor..cursor, cx);
        });
        self.refocus_find(window, cx);
    }

    fn refocus_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.find.is_open() {
            let target = if self.find.replace
                && self
                    .find
                    .replacement
                    .focus_handle(cx)
                    .contains_focused(window, cx)
            {
                &self.find.replacement
            } else {
                &self.find.query
            };
            target.update(cx, |input, cx| input.focus(window, cx));
        }
        cx.notify();
    }

    fn open_find(&mut self, replace: bool, window: &mut Window, cx: &mut Context<Self>) {
        let selected = self.active_editor().read(cx).selected_text().to_string();
        if !selected.is_empty() && !selected.contains('\n') {
            self.find
                .query
                .update(cx, |input, cx| input.set_value(selected, window, cx));
        }
        if !self.find.is_open() {
            self.find.serial += 1;
            self.find.visible = true;
            self.find.closing = false;
        }
        self.find.replace = replace;
        self.find.query.update(cx, |input, cx| {
            input.select_all(window, cx);
            input.focus(window, cx);
        });
        self.sync_find_selection(cx);
        cx.notify();
    }

    fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.find.is_open() {
            return;
        }
        self.find.closing = true;
        self.find.notice = None;
        self.find.notice_timer = Task::ready(());
        let editor = self.active_editor();
        editor.update(cx, |editor, cx| editor.set_selection_focus(None, cx));
        editor.focus_handle(cx).focus(window, cx);
        self.restart_caret_blink(cx);
    }

    pub(crate) fn sync_find_selection(&mut self, cx: &mut Context<Self>) {
        let focus = self.find.is_open().then(|| self.find.focus.clone());
        for (ix, tab) in self.tabs.iter().enumerate() {
            let focus = (ix == self.active).then(|| focus.clone()).flatten();
            tab.editor
                .update(cx, |editor, cx| editor.set_selection_focus(focus, cx));
        }
    }

    fn find_step(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(query) = self.find.prepare(cx) else {
            self.open_find(false, window, cx);
            return;
        };
        let editor = self.active_editor().clone();
        let (text, selection) = {
            let editor = editor.read(cx);
            (editor.text().clone(), editor.selected_range())
        };
        let wrap = self.find.wrap_around;
        let find = &mut self.find;
        let found = find
            .matcher
            .as_ref()
            .and_then(|matcher| matcher.search(&text, selection, forward, wrap, &mut find.buffer));
        match found {
            Some((range, wrapped)) => {
                editor.update(cx, |editor, cx| editor.set_selected_range(range, cx));
                if wrapped {
                    let notice = if forward {
                        "Found next from the top"
                    } else {
                        "Found previous from the bottom"
                    };
                    self.show_notice(notice, cx);
                } else {
                    self.find.notice = None;
                }
            }
            None => self.show_notice(not_found(&query), cx),
        }
        if !self.find.is_open() {
            editor.focus_handle(cx).focus(window, cx);
        }
        cx.notify();
    }

    fn show_notice(&mut self, notice: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.find.notice = Some(notice.into());
        self.find.notice_timer = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(NOTICE_DURATION).await;
            this.update(cx, |this, cx| {
                this.find.notice = None;
                cx.notify();
            })
            .ok();
        });
        cx.notify();
    }

    fn open_find_options(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let target = self.find.options_bounds.get();
        let entries = vec![
            Entry::Toggle("Match case", self.find.match_case, || {
                Box::new(ToggleMatchCase)
            }),
            Entry::Toggle("Wrap around", self.find.wrap_around, || {
                Box::new(ToggleWrapAround)
            }),
        ];
        let return_focus = self.find.query.focus_handle(cx);
        self.menu_bar.update(cx, |menu_bar, cx| {
            menu_bar.open_flyout_above(entries, target, return_focus, window, cx)
        });
    }

    pub(crate) fn render_find(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let p = palette(cx);
        let scale = self.find.sample(window, cx)?;
        let (width, height) = self.find.card_size(window);
        let notice = self
            .find
            .notice
            .clone()
            .filter(|_| !self.find.closing)
            .map(|text| notice(p, text, CARD_TOP + height + NOTICE_GAP));

        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .child(
                    div()
                        .absolute()
                        .top(px(CARD_TOP - CARD_SHADOW_CLIP_TOP))
                        .left_0()
                        .right_0()
                        .h(px(height + CARD_SHADOW_CLIP_TOP + CARD_SHADOW_CLIP_BOTTOM))
                        .pt(px(CARD_SHADOW_CLIP_TOP))
                        .pb(px(CARD_SHADOW_CLIP_BOTTOM))
                        .overflow_hidden()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(self.find_card(p, width, height, scale, window, cx)),
                )
                .children(notice),
        )
    }

    fn find_card(
        &self,
        p: Palette,
        width: f32,
        height: f32,
        scale: Point<f32>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let s = scale.x;
        let u = |v: f32| px(v * s);
        let card = div()
            .w(px(width * scale.x))
            .h(px(height * scale.y))
            .border_1()
            .border_color(rgba(p.surface_stroke))
            .rounded(px(OVERLAY_RADIUS * scale.x.min(scale.y)))
            .shadow(theme_shadow(p.dark, CARD_Z, s));
        if s < MIN_VISIBLE_SCALE {
            return card;
        }
        let buttons = field::ButtonStyle {
            surface: p.card_surface,
            accent: accent_color(cx),
            scale: s,
        };
        let query_row = self.find_query_row(p, s, window, cx);
        let replace_row = self
            .find
            .replace
            .then(|| self.find_replace_row(p, buttons, window, cx));
        card.child(
            div()
                .id("find-bar")
                .track_focus(&self.find.focus)
                .size_full()
                .flex()
                .flex_col()
                .justify_center()
                .p(u(CARD_PADDING))
                .rounded(u(OVERLAY_RADIUS - BORDER))
                .bg(rgb(p.card_fill))
                .occlude()
                .cursor_default()
                .on_action(
                    cx.listener(|this, _: &input::Escape, window, cx| this.close_find(window, cx)),
                )
                .child(query_row)
                .children(replace_row),
        )
    }

    fn find_query_row(&self, p: Palette, s: f32, window: &Window, cx: &mut Context<Self>) -> Div {
        let u = |v: f32| px(v * s);
        let options_bounds = self.find.options_bounds.clone();
        let replace = self.find.replace;
        div()
            .flex()
            .items_center()
            .gap(u(ICON_GAP))
            .child(tooltip::owner(
                "find-expand",
                if replace {
                    "Close replace options"
                } else {
                    "Open replace options"
                },
                icon_button(
                    p,
                    "find-expand",
                    if replace {
                        CHEVRON_DOWN_GLYPH
                    } else {
                        CHEVRON_UP_GLYPH
                    },
                    CHEVRON_SIZE,
                    s,
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.find.replace = !this.find.replace;
                    this.refocus_find(window, cx);
                })),
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .key_context(QUERY_CONTEXT)
                    .child(self.text_field(
                        &self.find.query,
                        FieldStyle {
                            placeholder: Some("Find"),
                            search_icon: true,
                        },
                        p.card_surface,
                        s,
                        window,
                        cx,
                    )),
            )
            .child(tooltip::owner(
                "find-next",
                "Search down",
                icon_button(p, "find-next", ARROW_DOWN_GLYPH, ICON_SIZE, s)
                    .on_click(cx.listener(|this, _, window, cx| this.find_step(true, window, cx))),
            ))
            .child(tooltip::owner(
                "find-previous",
                "Search up",
                icon_button(p, "find-previous", ARROW_UP_GLYPH, ICON_SIZE, s)
                    .on_click(cx.listener(|this, _, window, cx| this.find_step(false, window, cx))),
            ))
            .child(tooltip::owner(
                "find-options",
                "More options",
                icon_button(p, "find-options", OPTIONS_GLYPH, ICON_SIZE, s)
                    .on_click(cx.listener(|this, _, window, cx| this.open_find_options(window, cx)))
                    .on_prepaint(move |bounds, _, _| options_bounds.set(bounds)),
            ))
            .child(tooltip::owner(
                "find-close",
                "Exit Find and Replace",
                icon_button(p, "find-close", CLOSE_GLYPH, ICON_SIZE, s)
                    .on_click(cx.listener(|this, _, window, cx| this.close_find(window, cx))),
            ))
    }

    fn find_replace_row(
        &self,
        p: Palette,
        buttons: field::ButtonStyle,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let s = buttons.scale;
        let u = |v: f32| px(v * s);
        let action = |button| {
            div()
                .flex_none()
                .ml(u(ACTION_GAP))
                .w(u(ACTION_WIDTH))
                .child(button)
        };
        div()
            .flex()
            .items_center()
            .mt(u(ROW_GAP))
            .child(div().flex_none().w(u(ICON_BUTTON_SIZE + ICON_GAP)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .key_context(REPLACEMENT_CONTEXT)
                    .child(self.text_field(
                        &self.find.replacement,
                        FieldStyle {
                            placeholder: Some("Replace"),
                            search_icon: false,
                        },
                        p.card_surface,
                        s,
                        window,
                        cx,
                    )),
            )
            .child(action(
                field::button(p, buttons, "find-replace", "Replace", false, None, window).on_click(
                    cx.listener(|this, _, window, cx| this.replace_next(&ReplaceNext, window, cx)),
                ),
            ))
            .child(action(
                field::button(
                    p,
                    buttons,
                    "find-replace-all",
                    "Replace all",
                    false,
                    None,
                    window,
                )
                .on_click(cx.listener(|this, _, window, cx| this.replace_all(window, cx))),
            ))
    }
}

fn not_found(query: &str) -> String {
    format!("Cannot find \"{query}\"")
}

fn notice(p: Palette, text: SharedString, top: f32) -> Div {
    div()
        .absolute()
        .top(px(top))
        .left_0()
        .right_0()
        .flex()
        .justify_center()
        .child(
            div()
                .border_1()
                .border_color(rgba(p.flyout_stroke))
                .rounded(px(OVERLAY_RADIUS))
                .shadow(theme_shadow(p.dark, NOTICE_Z, 1.))
                .child(
                    div()
                        .px(px(NOTICE_PADDING_X))
                        .pt(px(NOTICE_PADDING_TOP))
                        .pb(px(NOTICE_PADDING_BOTTOM))
                        .rounded(px(OVERLAY_RADIUS - BORDER))
                        .bg(rgb(p.notice_fill))
                        .font_family(TEXT_FONT)
                        .text_size(px(BODY_SIZE))
                        .line_height(px(NOTICE_LINE))
                        .text_color(rgba(p.text_primary))
                        .child(text),
                ),
        )
}

fn icon_button(
    p: Palette,
    id: &'static str,
    glyph: &'static str,
    size: f32,
    s: f32,
) -> brush::BrushTransition {
    let u = |v: f32| px(v * s);
    field::icon_button(
        p,
        id,
        glyph,
        u(size),
        gpui::size(u(ICON_BUTTON_SIZE), u(ICON_BUTTON_SIZE)),
        u(CONTROL_RADIUS),
    )
}

fn merge_edits(
    text: &Rope,
    ranges: Vec<Range<usize>>,
    replacement: &str,
) -> Vec<(Range<usize>, String)> {
    let mut edits: Vec<(Range<usize>, String)> = Vec::new();
    for range in ranges {
        match edits.last_mut() {
            Some((last, merged)) if range.start - last.end <= MERGE_GAP => {
                for chunk in text.slice(last.end..range.start).chunks() {
                    merged.push_str(chunk);
                }
                merged.push_str(replacement);
                last.end = range.end;
            }
            _ => edits.push((range, replacement.to_string())),
        }
    }
    edits
}
