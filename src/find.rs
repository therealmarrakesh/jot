use super::*;
use field::{BODY_SIZE, CONTROL_HEIGHT, CONTROL_RADIUS, FieldStyle};
use gpui_kit::base::{MotionStatus, Presence, Transition, transition_with_status};
use gpui_kit::component::input::InputState;
use menu::Entry;
use regex::bytes::{Regex, RegexBuilder};

const QUERY_CONTEXT: &str = "JotFindQuery";
const REPLACEMENT_CONTEXT: &str = "JotFindReplacement";
const SHOW_DURATION: Duration = Duration::from_millis(167);
const HIDE_DURATION: Duration = Duration::from_millis(200);
const NOTICE_DURATION: Duration = Duration::from_secs(3);
const BLOCK_SIZE: usize = 256 * 1024;
const MAX_UTF8_LEN: usize = 4;
const MERGE_GAP: usize = 512;
const CARD_WIDTH: f32 = 860.;
const CARD_TOP: f32 = 12.;
const CARD_SIDE_MARGIN: f32 = 12.;
const CARD_RADIUS: f32 = 8.;
const CARD_PADDING: f32 = 8.;
const ROW_GAP: f32 = 8.;
const ICON_GAP: f32 = 8.;
const ACTION_GAP: f32 = 10.;
const ACTION_WIDTH: f32 = 90.;
const ICON_SIZE: f32 = 16.;
const CHEVRON_SIZE: f32 = 13.;
const MIN_VISIBLE_SCALE: f32 = 0.05;
const NOTICE_GAP: f32 = 8.;
const NOTICE_PADDING_X: f32 = 18.;
const NOTICE_PADDING_TOP: f32 = 16.;
const NOTICE_PADDING_BOTTOM: f32 = 15.;
const NOTICE_LINE: f32 = 20.;
const CHEVRON_UP: &str = "\u{E70E}";
const CHEVRON_DOWN: &str = "\u{E70D}";
const ARROW_DOWN: &str = "\u{E74B}";
const ARROW_UP: &str = "\u{E74A}";
const OPTIONS: &str = "\u{E9E9}";
const CLOSE: &str = "\u{E711}";

pub fn key_bindings() -> Vec<KeyBinding> {
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
            wrap.then(|| self.next(text, 0, selection.end, buffer))
                .flatten()
                .map(|range| (range, true))
        } else {
            if let Some(range) = self.previous(text, selection.start, 0, buffer) {
                return Some((range, false));
            }
            let floor = selection.start.saturating_sub(self.overlap);
            wrap.then(|| self.previous(text, text.len(), floor, buffer))
                .flatten()
                .map(|range| (range, true))
        }
    }
}

pub(crate) struct FindBar {
    visible: bool,
    closing: bool,
    replace: bool,
    serial: usize,
    scale: f32,
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
            scale: 0.,
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

    pub(crate) fn is_open(&self) -> bool {
        self.visible && !self.closing
    }

    fn sample(&mut self, window: &mut Window, cx: &mut App) {
        if !self.visible {
            return;
        }
        let shown = Presence::new(ElementId::from(("jot-find-show", self.serial)), true)
            .transition(Transition::new(SHOW_DURATION).ease(cubic_bezier(0., 0., 0., 1.)))
            .sample(window, cx)
            .progress;
        let hidden = transition_with_status(
            ElementId::from(("jot-find-hide", self.serial)),
            if self.closing { 1. } else { 0. },
            Transition::new(HIDE_DURATION).ease(cubic_bezier(1., 0., 1., 1.)),
            window,
            cx,
        );
        self.scale = if self.closing {
            1. - hidden.value
        } else {
            shown
        };
        if self.closing && hidden.status == MotionStatus::Finished {
            self.visible = false;
        }
    }
}

impl Jot {
    pub(crate) fn find(&mut self, _: &Find, window: &mut Window, cx: &mut Context<Self>) {
        self.open_find(false, window, cx);
    }

    pub(crate) fn replace(&mut self, _: &Replace, window: &mut Window, cx: &mut Context<Self>) {
        self.open_find(true, window, cx);
    }

    pub(crate) fn find_next(&mut self, _: &FindNext, window: &mut Window, cx: &mut Context<Self>) {
        self.find_step(true, window, cx);
    }

    pub(crate) fn find_previous(
        &mut self,
        _: &FindPrevious,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.find_step(false, window, cx);
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
        let editor = self.active_editor().clone();
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

    pub(crate) fn replace_all(
        &mut self,
        _: &ReplaceAll,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
            self.show_notice(format!("Cannot find \"{query}\""), cx);
            return;
        }
        let replacement = self.find.replacement.read(cx).value().to_string();
        let shift: isize = ranges
            .iter()
            .take_while(|range| range.end <= cursor)
            .map(|range| replacement.len() as isize - range.len() as isize)
            .sum();
        let edits = merge_edits(&text, ranges, &replacement);
        editor.update(cx, |editor, cx| {
            editor.replace_ranges(&edits, window, cx);
            let text = editor.text();
            let cursor = text.floor_char_boundary(
                (cursor as isize + shift).clamp(0, text.len() as isize) as usize,
            );
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
                self.find.replacement.clone()
            } else {
                self.find.query.clone()
            };
            target.update(cx, |input, cx| input.focus(window, cx));
        }
        cx.notify();
    }

    fn open_find(&mut self, replace: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.accent = field::accent_color(palette(cx));
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
            self.find.scale = 0.;
        }
        self.find.replace = replace;
        self.find.query.update(cx, |input, cx| {
            input.select_all(window, cx);
            input.focus(window, cx);
        });
        self.sync_find_selection(cx);
        cx.notify();
    }

    pub(crate) fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.find.is_open() {
            return;
        }
        self.find.closing = true;
        self.find.notice = None;
        self.find.notice_timer = Task::ready(());
        let editor = self.active_editor().clone();
        editor.update(cx, |editor, cx| editor.set_selection_focus(None, cx));
        editor.focus_handle(cx).focus(window, cx);
        self.restart_caret_blink(cx);
        cx.notify();
    }

    pub(crate) fn sync_find_selection(&mut self, cx: &mut Context<Self>) {
        let focus = self.find.is_open().then(|| self.find.focus.clone());
        let active = self.active_editor().clone();
        for tab in &self.tabs {
            let focus = (tab.editor == active).then(|| focus.clone()).flatten();
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
            None => self.show_notice(format!("Cannot find \"{query}\""), cx),
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
        self.find.sample(window, cx);
        if !self.find.visible {
            return None;
        }
        let s = self.find.scale;
        let rows = if self.find.replace { 2. } else { 1. };
        let height = 2. + 2. * CARD_PADDING + rows * CONTROL_HEIGHT + (rows - 1.) * ROW_GAP;
        let available = window.viewport_size().width.as_f32() - 2. * CARD_SIDE_MARGIN;
        let width = CARD_WIDTH.min(available).max(0.);
        let notice = self
            .find
            .notice
            .clone()
            .filter(|_| !self.find.closing)
            .map(|notice| self.notice(p, notice, CARD_TOP + height + NOTICE_GAP));

        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .child(
                    div()
                        .absolute()
                        .top(px(CARD_TOP))
                        .left_0()
                        .right_0()
                        .h(px(height))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(self.find_card(width, height, s, window, cx)),
                )
                .children(notice),
        )
    }

    fn notice(&self, p: Palette, text: SharedString, top: f32) -> Div {
        div()
            .absolute()
            .top(px(top))
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .child(
                div()
                    .px(px(NOTICE_PADDING_X))
                    .pt(px(NOTICE_PADDING_TOP))
                    .pb(px(NOTICE_PADDING_BOTTOM))
                    .bg(rgb(p.notice_fill))
                    .border_1()
                    .border_color(rgb(p.notice_border))
                    .rounded(px(CARD_RADIUS))
                    .shadow(vec![BoxShadow {
                        color: hsla(0., 0., 0., p.shadow),
                        offset: point(px(0.), px(4.)),
                        blur_radius: px(8.),
                        spread_radius: px(0.),
                        inset: false,
                    }])
                    .font_family(MENU_FONT)
                    .text_size(px(BODY_SIZE))
                    .line_height(px(NOTICE_LINE))
                    .text_color(rgba(p.text_primary))
                    .child(text),
            )
    }

    fn icon_button(
        &self,
        p: Palette,
        id: &'static str,
        tip: &'static str,
        glyph: &'static str,
        size: f32,
        s: f32,
    ) -> Stateful<Div> {
        let u = |v: f32| px(v * s);
        div()
            .id(id)
            .on_hover(move |hovered, _, cx| tooltip::hover(id, 0, tip, *hovered, cx))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .size(u(CONTROL_HEIGHT))
            .rounded(u(CONTROL_RADIUS))
            .cursor_default()
            .font_family("Segoe Fluent Icons")
            .text_size(u(size))
            .text_color(rgba(p.text_primary))
            .hover(move |style| style.bg(rgba(p.subtle_secondary)))
            .active(move |style| style.bg(rgba(p.subtle_tertiary)))
            .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
            .child(glyph)
    }

    fn find_card(
        &self,
        width: f32,
        height: f32,
        s: f32,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let p = palette(cx);
        let u = |v: f32| px(v * s);
        let card = div()
            .w(u(width))
            .h(u(height))
            .border_1()
            .border_color(rgba(p.surface_stroke))
            .rounded(u(CARD_RADIUS))
            .shadow(vec![BoxShadow {
                color: hsla(0., 0., 0., p.shadow),
                offset: point(px(0.), u(8.)),
                blur_radius: u(16.),
                spread_radius: px(0.),
                inset: false,
            }]);
        if s < MIN_VISIBLE_SCALE {
            return card;
        }
        let options_bounds = self.find.options_bounds.clone();
        let replace = self.find.replace;
        let query_row = div()
            .flex()
            .items_center()
            .gap(u(ICON_GAP))
            .child(
                self.icon_button(
                    p,
                    "find-expand",
                    if replace {
                        "Close replace options"
                    } else {
                        "Open replace options"
                    },
                    if replace { CHEVRON_DOWN } else { CHEVRON_UP },
                    CHEVRON_SIZE,
                    s,
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.find.replace = !this.find.replace;
                    this.refocus_find(window, cx);
                })),
            )
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
            .child(
                self.icon_button(p, "find-next", "Search down", ARROW_DOWN, ICON_SIZE, s)
                    .on_click(cx.listener(|this, _, window, cx| this.find_step(true, window, cx))),
            )
            .child(
                self.icon_button(p, "find-previous", "Search up", ARROW_UP, ICON_SIZE, s)
                    .on_click(cx.listener(|this, _, window, cx| this.find_step(false, window, cx))),
            )
            .child(
                self.icon_button(p, "find-options", "More options", OPTIONS, ICON_SIZE, s)
                    .on_click(cx.listener(|this, _, window, cx| this.open_find_options(window, cx)))
                    .child(
                        canvas(
                            move |bounds, _, _| options_bounds.set(bounds),
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_full(),
                    ),
            )
            .child(
                self.icon_button(
                    p,
                    "find-close",
                    "Exit Find and Replace",
                    CLOSE,
                    ICON_SIZE,
                    s,
                )
                .on_click(cx.listener(|this, _, window, cx| this.close_find(window, cx))),
            );
        let replace_row = replace.then(|| {
            div()
                .flex()
                .items_center()
                .mt(u(ROW_GAP))
                .child(div().flex_none().w(u(CONTROL_HEIGHT + ICON_GAP)))
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
                .child(
                    div()
                        .flex_none()
                        .ml(u(ACTION_GAP))
                        .w(u(ACTION_WIDTH))
                        .child(
                            field::button(
                                p,
                                "find-replace",
                                "Replace",
                                false,
                                false,
                                p.card_surface,
                                self.accent,
                                s,
                            )
                            .on_click(cx.listener(
                                |this, _, window, cx| this.replace_next(&ReplaceNext, window, cx),
                            )),
                        ),
                )
                .child(
                    div()
                        .flex_none()
                        .ml(u(ACTION_GAP))
                        .w(u(ACTION_WIDTH))
                        .child(
                            field::button(
                                p,
                                "find-replace-all",
                                "Replace all",
                                false,
                                false,
                                p.card_surface,
                                self.accent,
                                s,
                            )
                            .on_click(cx.listener(
                                |this, _, window, cx| this.replace_all(&ReplaceAll, window, cx),
                            )),
                        ),
                )
        });
        card.child(
            div()
                .id("find-bar")
                .track_focus(&self.find.focus)
                .size_full()
                .p(u(CARD_PADDING))
                .rounded(u(CARD_RADIUS - 1.))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(seed: u64, len: usize) -> String {
        let alphabet = [
            "a", "b", "A", "B", "é", "É", "ß", "😀", "\n", " ", "ab", "K",
        ];
        let mut state = seed;
        (0..len)
            .map(|_| {
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                alphabet[(state >> 33) as usize % alphabet.len()]
            })
            .collect()
    }

    fn expected_next(pattern: &Regex, text: &str, from: usize) -> Option<Range<usize>> {
        pattern
            .find_at(text.as_bytes(), from)
            .map(|found| found.range())
    }

    fn expected_previous(pattern: &Regex, text: &str, before: usize) -> Option<Range<usize>> {
        let haystack = &text.as_bytes()[..before];
        (0..before)
            .rev()
            .find_map(|start| {
                pattern
                    .find_at(haystack, start)
                    .filter(|found| found.start() == start)
            })
            .map(|found| found.range())
    }

    #[core::prelude::v1::test]
    fn block_search_matches_whole_text_search() {
        let queries = ["ab", "a", "éé", "😀a", "B\nA", "k", "ss"];
        let mut buffer = Vec::new();
        for seed in 0..6 {
            let source = sample(seed, 600);
            let rope = Rope::from(source.as_str());
            for query in queries {
                for match_case in [false, true] {
                    for block in [1, 5, 64, BLOCK_SIZE] {
                        let mut matcher = Matcher::new(query, match_case).unwrap();
                        matcher.block = block.max(1);
                        let expected: Vec<_> = matcher
                            .pattern
                            .find_iter(source.as_bytes())
                            .map(|found| found.range())
                            .collect();
                        assert_eq!(matcher.all(&rope, &mut buffer), expected);
                        let first = expected_next(&matcher.pattern, &source, 0);
                        let last = expected_previous(&matcher.pattern, &source, source.len());
                        for offset in (0..=source.len())
                            .filter(|i| source.is_char_boundary(*i))
                            .step_by(11)
                        {
                            assert_eq!(
                                matcher.next(&rope, offset, usize::MAX, &mut buffer),
                                expected_next(&matcher.pattern, &source, offset),
                                "next {query:?} case={match_case} block={block} from={offset}"
                            );
                            assert_eq!(
                                matcher.previous(&rope, offset, 0, &mut buffer),
                                expected_previous(&matcher.pattern, &source, offset),
                                "previous {query:?} case={match_case} block={block} before={offset}"
                            );
                            assert_eq!(
                                matcher.next(&rope, 0, offset, &mut buffer),
                                first.clone().filter(|range| range.start < offset),
                                "wrapped next {query:?} case={match_case} block={block} limit={offset}"
                            );
                            assert_eq!(
                                matcher.previous(&rope, source.len(), offset, &mut buffer),
                                last.clone().filter(|range| range.start >= offset),
                                "wrapped previous {query:?} case={match_case} block={block} floor={offset}"
                            );
                        }
                    }
                }
            }
        }
    }
}
