use super::*;
use gpui_kit::base::actions::{
    Cancel, Confirm, SelectDown, SelectFirst, SelectLast, SelectLeft, SelectRight, SelectUp,
};
use gpui_kit::base::{MotionStatus, Presence, Transition, transition_with_status};

const CONTEXT: &str = "JotMenu";
const OPEN_DURATION: Duration = Duration::from_millis(250);
const CLOSE_DURATION: Duration = Duration::from_millis(83);
const CLOSED_RATIO: f32 = 0.5;
const MIN_WIDTH: f32 = 232.;
const FILL: u32 = 0xfcfcfc;
const BORDER: u32 = 0xdcdcdc;
const SEPARATOR_LINE: u32 = 0xededed;
const ROW_COMMAND: f32 = 33.;
const ROW_COMPACT: f32 = 32.;
const ROW_SEPARATOR: f32 = 3.;
const PADDING_Y: f32 = 2.;
const ROW_INSET: f32 = 4.;
const ROW_MARGIN_Y: f32 = 2.;
const ROW_PADDING: f32 = 11.;
const CHECK_COLUMN: f32 = 28.;
const SHORTCUT_GAP: f32 = 24.;
const CHEVRON_WIDTH: f32 = 12.;
const LABEL_SIZE: f32 = 14.;
const SHORTCUT_SIZE: f32 = 12.;
const GLYPH_SIZE: f32 = 12.;
const SUBMENU_OVERLAP: f32 = 4.;

pub fn key_bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("up", SelectUp, Some(CONTEXT)),
        KeyBinding::new("down", SelectDown, Some(CONTEXT)),
        KeyBinding::new("home", SelectFirst, Some(CONTEXT)),
        KeyBinding::new("end", SelectLast, Some(CONTEXT)),
        KeyBinding::new("enter", Confirm { secondary: false }, Some(CONTEXT)),
        KeyBinding::new("escape", Cancel, Some(CONTEXT)),
        KeyBinding::new("left", SelectLeft, Some(CONTEXT)),
        KeyBinding::new("right", SelectRight, Some(CONTEXT)),
    ]
}

fn open_transition() -> Transition {
    Transition::new(OPEN_DURATION).ease(cubic_bezier(0., 0., 0., 1.))
}

fn close_transition() -> Transition {
    Transition::new(CLOSE_DURATION).ease(|t| t)
}

#[derive(Clone)]
enum Entry {
    Command(&'static str, fn() -> Box<dyn Action>),
    Toggle(&'static str, bool, fn() -> Box<dyn Action>),
    Submenu(&'static str, Vec<Entry>),
    Separator,
}

impl Entry {
    fn height(&self) -> f32 {
        match self {
            Self::Command(..) => ROW_COMMAND,
            Self::Toggle(..) | Self::Submenu(..) => ROW_COMPACT,
            Self::Separator => ROW_SEPARATOR,
        }
    }

    fn selectable(&self) -> bool {
        !matches!(self, Self::Separator)
    }

    fn action(&self) -> Option<Box<dyn Action>> {
        match self {
            Self::Command(_, action) | Self::Toggle(_, _, action) => Some(action()),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MenuKind {
    File,
    Edit,
    View,
}

impl MenuKind {
    const ALL: [Self; 3] = [Self::File, Self::Edit, Self::View];

    fn title(self) -> &'static str {
        match self {
            Self::File => "File",
            Self::Edit => "Edit",
            Self::View => "View",
        }
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|kind| *kind == self).unwrap_or(0)
    }

    fn entries(self, cx: &App) -> Vec<Entry> {
        use Entry::*;
        match self {
            Self::File => vec![
                Command("New tab", || Box::new(NewTab)),
                Command("New window", || Box::new(NewWindow)),
                Command("Open", || Box::new(Open)),
                Command("Save", || Box::new(Save)),
                Command("Save as", || Box::new(SaveAs)),
                Command("Save all", || Box::new(SaveAll)),
                Separator,
                Command("Close tab", || Box::new(CloseTab)),
                Command("Close window", || Box::new(CloseWindow)),
                Command("Exit", || Box::new(Exit)),
            ],
            Self::Edit => vec![
                Command("Undo", || Box::new(input::Undo)),
                Separator,
                Command("Cut", || Box::new(input::Cut)),
                Command("Copy", || Box::new(input::Copy)),
                Command("Paste", || Box::new(input::Paste)),
                Command("Delete", || Box::new(input::Delete)),
                Separator,
                Command("Find", || Box::new(Find)),
                Command("Find next", || Box::new(FindNext)),
                Command("Find previous", || Box::new(FindPrevious)),
                Command("Replace", || Box::new(Replace)),
                Command("Go to", || Box::new(GoTo)),
                Separator,
                Command("Select all", || Box::new(input::SelectAll)),
                Command("Time/Date", || Box::new(TimeDate)),
            ],
            Self::View => {
                let view = cx.global::<ViewSettings>();
                vec![
                    Submenu(
                        "Zoom",
                        vec![
                            Command("Zoom in", || Box::new(ZoomIn)),
                            Command("Zoom out", || Box::new(ZoomOut)),
                            Command("Restore default zoom", || Box::new(ResetZoom)),
                        ],
                    ),
                    Toggle("Status bar", view.status_bar, || Box::new(ToggleStatusBar)),
                    Toggle("Word wrap", view.word_wrap, || Box::new(ToggleWordWrap)),
                ]
            }
        }
    }
}

struct Panel {
    entries: Vec<Entry>,
    shortcuts: Vec<Option<SharedString>>,
    width: f32,
    highlighted: Option<usize>,
}

impl Panel {
    fn new(entries: Vec<Entry>, action_context: &FocusHandle, window: &Window) -> Self {
        let shortcuts: Vec<_> = entries
            .iter()
            .map(|entry| {
                entry
                    .action()
                    .and_then(|action| shortcut_text(action.as_ref(), action_context, window))
            })
            .collect();
        let checks = entries
            .iter()
            .any(|entry| matches!(entry, Entry::Toggle(..)));
        let content = entries
            .iter()
            .zip(&shortcuts)
            .map(|(entry, shortcut)| {
                let label = match entry {
                    Entry::Command(label, _)
                    | Entry::Toggle(label, ..)
                    | Entry::Submenu(label, _) => text_width(window, label, MENU_FONT, LABEL_SIZE),
                    Entry::Separator => 0.,
                };
                let trailing = match (entry, shortcut) {
                    (Entry::Submenu(..), _) => SHORTCUT_GAP + CHEVRON_WIDTH,
                    (_, Some(shortcut)) => {
                        SHORTCUT_GAP + text_width(window, shortcut, UI_FONT, SHORTCUT_SIZE)
                    }
                    _ => 0.,
                };
                label + trailing
            })
            .fold(0., f32::max);
        let check = if checks { CHECK_COLUMN } else { 0. };
        let width = (2. * (1. + ROW_INSET + ROW_PADDING) + check + content).ceil();

        Self {
            entries,
            shortcuts,
            width: width.max(MIN_WIDTH),
            highlighted: None,
        }
    }

    fn height(&self) -> f32 {
        2. + 2. * PADDING_Y + self.entries.iter().map(Entry::height).sum::<f32>()
    }

    fn row_top(&self, ix: usize) -> f32 {
        1. + PADDING_Y + self.entries[..ix].iter().map(Entry::height).sum::<f32>()
    }

    fn step(&mut self, delta: isize) {
        let count = self.entries.len() as isize;
        let mut ix = match self.highlighted {
            Some(ix) => ix as isize,
            None if delta > 0 => -1,
            None => count,
        };
        for _ in 0..count {
            ix = (ix + delta).rem_euclid(count);
            if self.entries[ix as usize].selectable() {
                self.highlighted = Some(ix as usize);
                return;
            }
        }
    }
}

struct OpenMenu {
    kind: MenuKind,
    panel: Panel,
    submenu: Option<(usize, Panel)>,
    serial: usize,
    closing: bool,
    unroll: f32,
    opacity: f32,
}

impl OpenMenu {
    fn is_closing(&self) -> bool {
        self.closing
    }

    fn sample(&mut self, window: &mut Window, cx: &mut App) -> bool {
        if !self.closing {
            self.unroll = Presence::new(ElementId::from(("jot-menu-unroll", self.serial)), true)
                .transition(open_transition())
                .sample(window, cx)
                .progress;
        }
        let fade = transition_with_status(
            ElementId::from(("jot-menu-fade", self.serial)),
            if self.closing { 0. } else { 1. },
            close_transition(),
            window,
            cx,
        );
        self.opacity = fade.value;
        self.closing && fade.status == MotionStatus::Finished
    }
}

pub struct MenuBar {
    pub action_context: FocusHandle,
    focus: FocusHandle,
    open: Option<OpenMenu>,
    openings: usize,
    hit_areas: Rc<Cell<[Bounds<Pixels>; 3]>>,
}

impl MenuBar {
    pub fn new(action_context: FocusHandle, cx: &mut Context<Self>) -> Self {
        Self {
            action_context,
            focus: cx.focus_handle(),
            open: None,
            openings: 0,
            hit_areas: Rc::new(Cell::new([Bounds::default(); 3])),
        }
    }

    fn open_kind(&self) -> Option<MenuKind> {
        self.open
            .as_ref()
            .filter(|open| !open.is_closing())
            .map(|open| open.kind)
    }

    fn open_menu(&mut self, kind: MenuKind, window: &mut Window, cx: &mut Context<Self>) {
        let panel = Panel::new(kind.entries(cx), &self.action_context, window);
        self.openings += 1;
        self.open = Some(OpenMenu {
            kind,
            panel,
            submenu: None,
            serial: self.openings,
            closing: false,
            unroll: 0.,
            opacity: 1.,
        });
        self.focus.focus(window, cx);
        cx.notify();
    }

    fn close_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(open) = self.open.as_mut().filter(|open| !open.is_closing()) {
            open.closing = true;
            open.submenu = None;
        }
        self.action_context.focus(window, cx);
        cx.notify();
    }

    fn activate(
        &mut self,
        path: (usize, Option<usize>),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(open) = self.open.as_mut().filter(|open| !open.is_closing()) else {
            return;
        };
        let entry = match path {
            (ix, None) => open.panel.entries[ix].clone(),
            (_, Some(sub)) => match &open.submenu {
                Some((_, panel)) => panel.entries[sub].clone(),
                None => return,
            },
        };
        match entry {
            Entry::Submenu(..) => self.open_submenu(path.0, true, window, cx),
            Entry::Separator => {}
            entry => {
                let action = entry.action();
                self.close_menu(window, cx);
                if let Some(action) = action {
                    let target = self.action_context.clone();
                    window.defer(cx, move |window, cx| {
                        target.dispatch_action(action.as_ref(), window, cx)
                    });
                }
            }
        }
    }

    fn open_submenu(
        &mut self,
        ix: usize,
        highlight_first: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let action_context = self.action_context.clone();
        let Some(open) = self.open.as_mut().filter(|open| !open.is_closing()) else {
            return;
        };
        let Entry::Submenu(_, entries) = &open.panel.entries[ix] else {
            open.submenu = None;
            cx.notify();
            return;
        };
        if open
            .submenu
            .as_ref()
            .is_none_or(|(open_ix, _)| *open_ix != ix)
        {
            open.submenu = Some((ix, Panel::new(entries.clone(), &action_context, window)));
        }
        if highlight_first && let Some((_, panel)) = open.submenu.as_mut() {
            panel.highlighted = None;
            panel.step(1);
        }
        cx.notify();
    }

    fn hover_row(
        &mut self,
        path: (usize, Option<usize>),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(open) = self.open.as_mut().filter(|open| !open.is_closing()) else {
            return;
        };
        match path {
            (ix, None) => {
                open.panel.highlighted = Some(ix);
                if matches!(open.panel.entries[ix], Entry::Submenu(..)) {
                    self.open_submenu(ix, false, window, cx);
                } else {
                    open.submenu = None;
                }
            }
            (_, Some(sub)) => {
                if let Some((_, panel)) = open.submenu.as_mut() {
                    panel.highlighted = Some(sub);
                }
            }
        }
        cx.notify();
    }

    fn switch(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(kind) = self.open_kind() else {
            return;
        };
        let count = MenuKind::ALL.len() as isize;
        let next = (kind.index() as isize + delta).rem_euclid(count) as usize;
        self.open_menu(MenuKind::ALL[next], window, cx);
        if let Some(open) = self.open.as_mut() {
            open.panel.step(1);
        }
    }

    fn on_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        self.step_active(-1, cx);
    }

    fn on_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        self.step_active(1, cx);
    }

    fn on_first(&mut self, _: &SelectFirst, _: &mut Window, cx: &mut Context<Self>) {
        self.jump_active(1, cx);
    }

    fn on_last(&mut self, _: &SelectLast, _: &mut Window, cx: &mut Context<Self>) {
        self.jump_active(-1, cx);
    }

    fn jump_active(&mut self, delta: isize, cx: &mut Context<Self>) {
        if let Some(open) = self.open.as_mut().filter(|open| !open.is_closing()) {
            let panel = match open
                .submenu
                .as_mut()
                .filter(|(_, panel)| panel.highlighted.is_some())
            {
                Some((_, panel)) => panel,
                None => &mut open.panel,
            };
            panel.highlighted = None;
            panel.step(delta);
            cx.notify();
        }
    }

    fn step_active(&mut self, delta: isize, cx: &mut Context<Self>) {
        if let Some(open) = self.open.as_mut().filter(|open| !open.is_closing()) {
            match open
                .submenu
                .as_mut()
                .filter(|(_, panel)| panel.highlighted.is_some())
            {
                Some((_, panel)) => panel.step(delta),
                None => open.panel.step(delta),
            }
            cx.notify();
        }
    }

    fn on_confirm(&mut self, _: &Confirm, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.open.as_ref() else {
            return;
        };
        let sub = open
            .submenu
            .as_ref()
            .and_then(|(_, panel)| panel.highlighted);
        if let Some(ix) = open.panel.highlighted {
            self.activate((ix, sub), window, cx);
        }
    }

    fn on_cancel(&mut self, _: &Cancel, window: &mut Window, cx: &mut Context<Self>) {
        match self.open.as_mut().and_then(|open| open.submenu.take()) {
            Some(_) => cx.notify(),
            None => self.close_menu(window, cx),
        }
    }

    fn on_left(&mut self, _: &SelectLeft, window: &mut Window, cx: &mut Context<Self>) {
        match self.open.as_mut().and_then(|open| open.submenu.take()) {
            Some(_) => cx.notify(),
            None => self.switch(-1, window, cx),
        }
    }

    fn on_right(&mut self, _: &SelectRight, window: &mut Window, cx: &mut Context<Self>) {
        let submenu_row = self.open.as_ref().and_then(|open| {
            open.panel
                .highlighted
                .filter(|ix| matches!(open.panel.entries[*ix], Entry::Submenu(..)))
        });
        match submenu_row {
            Some(ix) => self.open_submenu(ix, true, window, cx),
            None => self.switch(1, window, cx),
        }
    }

    fn trigger(
        &self,
        kind: MenuKind,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let open = self.open.as_ref().filter(|open| open.kind == kind);
        let active = open.is_some_and(|open| !open.is_closing());
        let popup = open.map(|open| self.popup(open, window, cx));

        div()
            .id(kind.title())
            .relative()
            .flex()
            .items_center()
            .h_full()
            .px(px(10.))
            .rounded(px(4.))
            .map(|this| {
                if active {
                    this.bg(rgba(SUBTLE_FILL_SECONDARY))
                } else {
                    this.hover(|style| style.bg(rgba(SUBTLE_FILL_SECONDARY)))
                }
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                    if this.open_kind() == Some(kind) {
                        this.close_menu(window, cx);
                    } else {
                        this.open_menu(kind, window, cx);
                    }
                }),
            )
            .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                if *hovered && this.open_kind().is_some_and(|open| open != kind) {
                    this.open_menu(kind, window, cx);
                }
            }))
            .child(div().relative().top(px(1.)).child(kind.title()))
            .when_some(popup, |this, popup| {
                this.child(
                    div()
                        .absolute()
                        .left_0()
                        .top(px(32.))
                        .child(deferred(anchored().child(popup)).with_priority(1)),
                )
            })
    }

    fn popup(
        &self,
        open: &OpenMenu,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let eased = open.unroll;
        let opacity = open.opacity;
        let panel = &open.panel;
        let height = panel.height();
        let width = panel.width;
        let scale = 1. - CLOSED_RATIO * (1. - eased);
        let offset = -height * CLOSED_RATIO * (1. - eased);
        let hit_areas = self.hit_areas.clone();
        let submenu = open.submenu.as_ref().map(|(ix, sub)| {
            (
                *ix,
                self.panel_rows(sub, Some(*ix), window, cx),
                sub.width,
                sub.height(),
            )
        });
        let submenu_top = open
            .submenu
            .as_ref()
            .map(|(ix, _)| panel.row_top(*ix) - 1. - PADDING_Y);

        div()
            .relative()
            .w(px(width))
            .h(px(height))
            .opacity(opacity)
            .on_mouse_down_out(cx.listener(|this, event: &MouseDownEvent, window, cx| {
                let inside = this
                    .hit_areas
                    .get()
                    .iter()
                    .any(|area| area.contains(&event.position));
                if !inside {
                    this.close_menu(window, cx);
                }
            }))
            .child(card(width, height * scale))
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .overflow_hidden()
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .top(px(offset))
                            .w(px(width))
                            .h(px(height))
                            .child(self.panel_rows(panel, None, window, cx)),
                    ),
            )
            .child(
                canvas(
                    move |bounds, _, _| {
                        let mut areas = hit_areas.get();
                        areas[1] = bounds;
                        hit_areas.set(areas);
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .when_some(
                submenu.zip(submenu_top),
                |this, ((_, rows, sub_width, sub_height), top)| {
                    let hit_areas = self.hit_areas.clone();
                    this.child(
                        div()
                            .absolute()
                            .left(px(width - SUBMENU_OVERLAP))
                            .top(px(top))
                            .w(px(sub_width))
                            .h(px(sub_height))
                            .child(card(sub_width, sub_height))
                            .child(div().absolute().top_0().left_0().size_full().child(rows))
                            .child(
                                canvas(
                                    move |bounds, _, _| {
                                        let mut areas = hit_areas.get();
                                        areas[2] = bounds;
                                        hit_areas.set(areas);
                                    },
                                    |_, _, _, _| {},
                                )
                                .absolute()
                                .size_full(),
                            ),
                    )
                },
            )
    }

    fn panel_rows(
        &self,
        panel: &Panel,
        parent: Option<usize>,
        _: &Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let checks = panel
            .entries
            .iter()
            .any(|entry| matches!(entry, Entry::Toggle(..)));

        div()
            .absolute()
            .top(px(1. + PADDING_Y))
            .left(px(1.))
            .w(px(panel.width - 2.))
            .children(panel.entries.iter().enumerate().map(|(ix, entry)| {
                let path = match parent {
                    Some(parent) => (parent, Some(ix)),
                    None => (ix, None),
                };
                if let Entry::Separator = entry {
                    return div()
                        .id(("menu-separator", ix))
                        .h(px(ROW_SEPARATOR))
                        .py(px(1.))
                        .child(div().h(px(1.)).bg(rgb(SEPARATOR_LINE)));
                }
                let highlighted = panel.highlighted == Some(ix);
                let (label, checked) = match entry {
                    Entry::Command(label, _) | Entry::Submenu(label, _) => (*label, false),
                    Entry::Toggle(label, checked, _) => (*label, *checked),
                    Entry::Separator => unreachable!(),
                };
                let shortcut = panel.shortcuts[ix].clone();

                div()
                    .id(("menu-row", ix + parent.map_or(0, |p| 1000 * (p + 1))))
                    .h(px(entry.height()))
                    .px(px(ROW_INSET))
                    .py(px(ROW_MARGIN_Y))
                    .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                        if *hovered {
                            this.hover_row(path, window, cx);
                        }
                    }))
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.activate(path, window, cx)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .size_full()
                            .px(px(ROW_PADDING))
                            .rounded(px(4.))
                            .when(highlighted, |this| this.bg(rgba(SUBTLE_FILL_SECONDARY)))
                            .when(checks, |this| {
                                this.child(
                                    div()
                                        .flex_none()
                                        .w(px(CHECK_COLUMN))
                                        .font_family("Segoe Fluent Icons")
                                        .text_size(px(GLYPH_SIZE))
                                        .text_color(rgba(TEXT_PRIMARY))
                                        .when(checked, |this| this.child("\u{E73E}")),
                                )
                            })
                            .child(
                                div()
                                    .flex_1()
                                    .font_family(MENU_FONT)
                                    .text_size(px(LABEL_SIZE))
                                    .text_color(rgba(TEXT_PRIMARY))
                                    .child(label),
                            )
                            .when_some(shortcut, |this, shortcut| {
                                this.child(
                                    div()
                                        .flex_none()
                                        .font_family(UI_FONT)
                                        .text_size(px(SHORTCUT_SIZE))
                                        .text_color(rgba(TEXT_SECONDARY))
                                        .child(shortcut),
                                )
                            })
                            .when(matches!(entry, Entry::Submenu(..)), |this| {
                                this.child(
                                    div()
                                        .flex_none()
                                        .font_family("Segoe Fluent Icons")
                                        .text_size(px(GLYPH_SIZE))
                                        .text_color(rgba(TEXT_SECONDARY))
                                        .child("\u{E76C}"),
                                )
                            }),
                    )
            }))
    }
}

fn card(width: f32, height: f32) -> Div {
    div()
        .absolute()
        .top_0()
        .left_0()
        .w(px(width))
        .h(px(height))
        .bg(rgb(FILL))
        .border_1()
        .border_color(rgb(BORDER))
        .rounded(px(8.))
        .shadow(vec![BoxShadow {
            color: hsla(0., 0., 0., 0.14),
            offset: point(px(0.), px(8.)),
            blur_radius: px(16.),
            spread_radius: px(0.),
            inset: false,
        }])
}

fn text_width(window: &Window, text: &str, family: &'static str, size: f32) -> f32 {
    let run = TextRun {
        len: text.len(),
        font: font(family),
        color: hsla(0., 0., 0., 1.),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    window
        .text_system()
        .shape_line(SharedString::new(text), px(size), &[run], None)
        .width
        .as_f32()
}

fn shortcut_text(
    action: &dyn Action,
    focus: &FocusHandle,
    window: &Window,
) -> Option<SharedString> {
    let binding = window.highest_precedence_binding_for_action_in(action, focus)?;
    let keystroke = binding.keystrokes().first()?;
    let modifiers = keystroke.modifiers();
    let mut parts: Vec<String> = Vec::new();
    if modifiers.control {
        parts.push("Ctrl".into());
    }
    if modifiers.shift {
        parts.push("Shift".into());
    }
    if modifiers.alt {
        parts.push("Alt".into());
    }
    parts.push(match keystroke.key() {
        "delete" => "Del".into(),
        "=" | "+" => "Plus".into(),
        "-" => "Minus".into(),
        key if key.len() == 1 => key.to_uppercase(),
        key => {
            let mut chars = key.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
                .unwrap_or_default()
        }
    });
    Some(parts.join("+").into())
}

impl Render for MenuBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self
            .open
            .as_mut()
            .is_some_and(|open| open.sample(window, cx))
        {
            self.open = None;
        }
        let hit_areas = self.hit_areas.clone();

        div()
            .id("menu-bar")
            .track_focus(&self.focus)
            .key_context(CONTEXT)
            .relative()
            .flex()
            .h_full()
            .gap(px(8.))
            .font_family(MENU_FONT)
            .text_size(px(14.))
            .text_color(rgba(TEXT_PRIMARY))
            .on_action(cx.listener(Self::on_up))
            .on_action(cx.listener(Self::on_down))
            .on_action(cx.listener(Self::on_first))
            .on_action(cx.listener(Self::on_last))
            .on_action(cx.listener(Self::on_confirm))
            .on_action(cx.listener(Self::on_cancel))
            .on_action(cx.listener(Self::on_left))
            .on_action(cx.listener(Self::on_right))
            .children(MenuKind::ALL.map(|kind| self.trigger(kind, window, cx)))
            .child(
                canvas(
                    move |bounds, _, _| {
                        let mut areas = hit_areas.get();
                        areas[0] = bounds;
                        hit_areas.set(areas);
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
    }
}
