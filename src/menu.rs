use super::*;
use gpui_kit::base::actions::{
    Cancel, Confirm, SelectDown, SelectFirst, SelectLast, SelectLeft, SelectRight, SelectUp,
};
use gpui_kit::base::{MotionStatus, Presence, Transition, transition_with_status};
use windows_sys::Win32::System::DataExchange::IsClipboardFormatAvailable;
use windows_sys::Win32::System::Ole::CF_UNICODETEXT;
use windows_sys::Win32::UI::WindowsAndMessaging::{SPI_GETMENUSHOWDELAY, SystemParametersInfoW};

const CONTEXT: &str = "JotMenu";
const OPEN_DURATION: Duration = Duration::from_millis(250);
const CLOSE_DURATION: Duration = Duration::from_millis(83);
const CLOSED_RATIO: f32 = 0.5;
const MENU_Z: f32 = 32.;
const SUBMENU_Z: f32 = 40.;
const SUBMENU_CLOSED_RATIO: f32 = 0.67;
const DEFAULT_MENU_SHOW_DELAY_MS: u64 = 400;
const MIN_WIDTH: f32 = 232.;
const FLYOUT_MIN_WIDTH: f32 = 198.;
const FLYOUT_GAP: f32 = 3.;
const CONTEXT_WIDTH: f32 = 342.;
const ROW_ICON: f32 = 40.;
const ICON_SIZE: f32 = 16.;
const COMMAND_ROW: f32 = 60.;
const COMMAND_TOP: f32 = 12.;
const COMMAND_LABEL_GAP: f32 = 4.;
const COMMAND_LINE: f32 = 16.;
const COMMAND_LABEL_SIZE: f32 = 12.;
const COMMAND_DIVIDER_TOP: f32 = 14.;
const COMMAND_DIVIDER_HEIGHT: f32 = 32.;
const COMMAND_EDGE_TRIM: f32 = 2.;
const COMMAND_INSET: f32 = 2.;
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

pub(crate) struct CommandButton {
    pub label: &'static str,
    pub glyph: &'static str,
    pub enabled: bool,
    pub action: fn() -> Box<dyn Action>,
}

pub(crate) enum MenuEvent {
    Opened,
    Closed,
}

pub(crate) fn context_menu(editor: &TextareaState) -> (Vec<CommandButton>, Vec<Entry>) {
    let selection = !editor.selected_range().is_empty();
    let commands = vec![
        CommandButton {
            label: "Cut",
            glyph: "\u{E8C6}",
            enabled: selection,
            action: || Box::new(input::Cut),
        },
        CommandButton {
            label: "Copy",
            glyph: "\u{E8C8}",
            enabled: selection,
            action: || Box::new(input::Copy),
        },
        CommandButton {
            label: "Paste",
            glyph: "\u{E77F}",
            enabled: clipboard_has_text(),
            action: || Box::new(input::Paste),
        },
        CommandButton {
            label: "Select all",
            glyph: "\u{E8B3}",
            enabled: true,
            action: || Box::new(input::SelectAll),
        },
        CommandButton {
            label: "Delete",
            glyph: "\u{E74D}",
            enabled: selection,
            action: || Box::new(input::Delete),
        },
    ];
    let entries = vec![Entry::Icon("Undo", "\u{E7A7}", editor.can_undo(), || {
        Box::new(input::Undo)
    })];
    (commands, entries)
}

pub(crate) fn clipboard_has_text() -> bool {
    unsafe { IsClipboardFormatAvailable(CF_UNICODETEXT.into()) != 0 }
}

#[derive(Clone)]
pub(crate) enum Entry {
    Command(&'static str, fn() -> Box<dyn Action>),
    Disabled(&'static str, fn() -> Box<dyn Action>),
    Icon(&'static str, &'static str, bool, fn() -> Box<dyn Action>),
    Toggle(&'static str, bool, fn() -> Box<dyn Action>),
    Submenu(&'static str, Vec<Entry>),
    Separator,
}

impl Entry {
    fn command(label: &'static str, enabled: bool, action: fn() -> Box<dyn Action>) -> Self {
        if enabled {
            Self::Command(label, action)
        } else {
            Self::Disabled(label, action)
        }
    }

    fn height(&self) -> f32 {
        match self {
            Self::Command(..) | Self::Disabled(..) => ROW_COMMAND,
            Self::Icon(..) => ROW_ICON,
            Self::Toggle(..) | Self::Submenu(..) => ROW_COMPACT,
            Self::Separator => ROW_SEPARATOR,
        }
    }

    fn selectable(&self) -> bool {
        match self {
            Self::Separator | Self::Disabled(..) => false,
            Self::Icon(_, _, enabled, _) => *enabled,
            _ => true,
        }
    }

    fn action(&self) -> Option<Box<dyn Action>> {
        match self {
            Self::Command(_, action)
            | Self::Disabled(_, action)
            | Self::Toggle(_, _, action)
            | Self::Icon(_, _, _, action) => Some(action()),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MenuKind {
    File,
    Edit,
    View,
    Flyout,
}

impl MenuKind {
    const ALL: [Self; 3] = [Self::File, Self::Edit, Self::View];

    fn title(self) -> &'static str {
        match self {
            Self::File => "File",
            Self::Edit => "Edit",
            Self::View => "View",
            Self::Flyout => "",
        }
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|kind| *kind == self).unwrap_or(0)
    }

    fn entries(self, editor: &TextareaState, cx: &App) -> Vec<Entry> {
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
            Self::Edit => {
                let selection = !editor.selected_range().is_empty();
                let clipboard_text = clipboard_has_text();
                vec![
                    Entry::command("Undo", editor.can_undo(), || Box::new(input::Undo)),
                    Separator,
                    Entry::command("Cut", selection, || Box::new(input::Cut)),
                    Entry::command("Copy", selection, || Box::new(input::Copy)),
                    Entry::command("Paste", clipboard_text, || Box::new(input::Paste)),
                    Entry::command("Delete", selection, || Box::new(input::Delete)),
                    Separator,
                    Command("Find", || Box::new(Find)),
                    Command("Find next", || Box::new(FindNext)),
                    Command("Find previous", || Box::new(FindPrevious)),
                    Command("Replace", || Box::new(Replace)),
                    Command("Go to", || Box::new(GoTo)),
                    Separator,
                    Command("Select all", || Box::new(input::SelectAll)),
                    Command("Time/Date", || Box::new(TimeDate)),
                ]
            }
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
            Self::Flyout => Vec::new(),
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
    fn new(
        entries: Vec<Entry>,
        min_width: f32,
        action_context: &FocusHandle,
        window: &Window,
    ) -> Self {
        let shortcuts: Vec<_> = entries
            .iter()
            .map(|entry| {
                entry
                    .action()
                    .filter(|_| !matches!(entry, Entry::Icon(..)))
                    .and_then(|action| shortcut_text(action.as_ref(), action_context, window))
            })
            .collect();
        let checks = entries
            .iter()
            .any(|entry| matches!(entry, Entry::Toggle(..) | Entry::Icon(..)));
        let content = entries
            .iter()
            .zip(&shortcuts)
            .map(|(entry, shortcut)| {
                let label = match entry {
                    Entry::Command(label, _)
                    | Entry::Disabled(label, _)
                    | Entry::Icon(label, ..)
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
            width: width.max(min_width),
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

fn menu_show_delay() -> Duration {
    let mut millis: u32 = 0;
    let read = unsafe {
        SystemParametersInfoW(SPI_GETMENUSHOWDELAY, 0, (&mut millis as *mut u32).cast(), 0)
    };
    Duration::from_millis(if read != 0 {
        millis.into()
    } else {
        DEFAULT_MENU_SHOW_DELAY_MS
    })
}

struct Motion {
    serial: usize,
    closing: bool,
    fade_in: bool,
    unroll: f32,
    opacity: f32,
}

impl Motion {
    fn new(serial: usize, fade_in: bool) -> Self {
        Self {
            serial,
            closing: false,
            fade_in,
            unroll: 0.,
            opacity: 1.,
        }
    }

    fn sample(&mut self, window: &mut Window, cx: &mut App) -> bool {
        if !self.closing {
            self.unroll = Presence::new(ElementId::from(("jot-menu-unroll", self.serial)), true)
                .transition(open_transition())
                .sample(window, cx)
                .progress;
        }
        let appear = if self.fade_in {
            Presence::new(ElementId::from(("jot-menu-appear", self.serial)), true)
                .transition(close_transition())
                .sample(window, cx)
                .progress
        } else {
            1.
        };
        let fade = transition_with_status(
            ElementId::from(("jot-menu-fade", self.serial)),
            if self.closing { 0. } else { 1. },
            close_transition(),
            window,
            cx,
        );
        self.opacity = fade.value * appear;
        self.closing && fade.status == MotionStatus::Finished
    }
}

struct Submenu {
    ix: usize,
    panel: Panel,
    motion: Motion,
}

struct OpenMenu {
    kind: MenuKind,
    anchor: Option<Point<Pixels>>,
    upward: bool,
    commands: Vec<CommandButton>,
    return_focus: Option<FocusHandle>,
    panel: Panel,
    submenu: Option<Submenu>,
    motion: Motion,
}

impl OpenMenu {
    fn new(kind: MenuKind, panel: Panel) -> Self {
        Self {
            kind,
            anchor: None,
            upward: false,
            commands: Vec::new(),
            return_focus: None,
            panel,
            submenu: None,
            motion: Motion::new(0, false),
        }
    }

    fn is_closing(&self) -> bool {
        self.motion.closing
    }

    fn live_submenu(&self) -> Option<&Submenu> {
        self.submenu.as_ref().filter(|sub| !sub.motion.closing)
    }

    fn live_submenu_mut(&mut self) -> Option<&mut Submenu> {
        self.submenu.as_mut().filter(|sub| !sub.motion.closing)
    }

    fn sample(&mut self, window: &mut Window, cx: &mut App) -> bool {
        if self
            .submenu
            .as_mut()
            .is_some_and(|sub| sub.motion.sample(window, cx))
        {
            self.submenu = None;
        }
        self.motion.sample(window, cx)
    }
}

enum SubmenuChange {
    Keep,
    Open(usize),
    Close,
}

pub struct MenuBar {
    pub editor: Entity<TextareaState>,
    focus: FocusHandle,
    open: Option<OpenMenu>,
    serials: usize,
    submenu_timer: Task<()>,
    hit_areas: Rc<Cell<[Bounds<Pixels>; 3]>>,
}

impl MenuBar {
    pub fn new(editor: Entity<TextareaState>, cx: &mut Context<Self>) -> Self {
        Self {
            editor,
            focus: cx.focus_handle(),
            open: None,
            serials: 0,
            submenu_timer: Task::ready(()),
            hit_areas: Rc::new(Cell::new([Bounds::default(); 3])),
        }
    }

    fn action_context(&self, cx: &App) -> FocusHandle {
        self.editor.focus_handle(cx)
    }

    fn next_serial(&mut self) -> usize {
        self.serials += 1;
        self.serials
    }

    fn open_kind(&self) -> Option<MenuKind> {
        self.open
            .as_ref()
            .filter(|open| !open.is_closing())
            .map(|open| open.kind)
    }

    fn open_menu(&mut self, kind: MenuKind, window: &mut Window, cx: &mut Context<Self>) {
        let entries = kind.entries(self.editor.read(cx), cx);
        let panel = Panel::new(entries, MIN_WIDTH, &self.action_context(cx), window);
        self.show(OpenMenu::new(kind, panel), window, cx);
    }

    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    pub fn open_context_menu(
        &mut self,
        commands: Vec<CommandButton>,
        entries: Vec<Entry>,
        anchor: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let panel = Panel::new(entries, CONTEXT_WIDTH, &self.action_context(cx), window);
        self.show(
            OpenMenu {
                anchor: Some(anchor),
                commands,
                ..OpenMenu::new(MenuKind::Flyout, panel)
            },
            window,
            cx,
        );
    }

    pub fn open_flyout_above(
        &mut self,
        entries: Vec<Entry>,
        target: Bounds<Pixels>,
        return_focus: FocusHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let panel = Panel::new(entries, FLYOUT_MIN_WIDTH, &self.action_context(cx), window);
        let anchor = point(
            target.center().x - px((panel.width / 2.).floor()),
            target.top() - px(FLYOUT_GAP + panel.height()),
        );
        self.show(
            OpenMenu {
                anchor: Some(anchor),
                upward: true,
                return_focus: Some(return_focus),
                ..OpenMenu::new(MenuKind::Flyout, panel)
            },
            window,
            cx,
        );
    }

    fn show(&mut self, open: OpenMenu, window: &mut Window, cx: &mut Context<Self>) {
        let motion = Motion::new(self.next_serial(), !open.commands.is_empty());
        self.open = Some(OpenMenu { motion, ..open });
        self.submenu_timer = Task::ready(());
        self.focus.focus(window, cx);
        cx.emit(MenuEvent::Opened);
        cx.notify();
    }

    fn close_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut return_focus = None;
        if let Some(open) = self.open.as_mut().filter(|open| !open.is_closing()) {
            open.motion.closing = true;
            return_focus = open.return_focus.clone();
        }
        self.submenu_timer = Task::ready(());
        return_focus
            .unwrap_or_else(|| self.action_context(cx))
            .focus(window, cx);
        cx.emit(MenuEvent::Closed);
        cx.notify();
    }

    fn run_command(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(command) = self
            .open
            .as_ref()
            .filter(|open| !open.is_closing())
            .and_then(|open| open.commands.get(ix))
            .filter(|command| command.enabled)
        else {
            return;
        };
        let action = (command.action)();
        self.close_menu(window, cx);
        let target = self.action_context(cx);
        window.defer(cx, move |window, cx| {
            target.dispatch_action(action.as_ref(), window, cx)
        });
    }

    fn command_row(&self, commands: &[CommandButton], width: f32, cx: &mut Context<Self>) -> Div {
        let p = palette(cx);
        let count = commands.len() as f32;
        let cell = ((width - 2. - (count - 1.) + 2. * COMMAND_EDGE_TRIM) / count).floor();
        let last = commands.len().saturating_sub(1);
        div()
            .absolute()
            .top(px(1.))
            .left(px(1.))
            .w(px(width - 2.))
            .h(px(COMMAND_ROW))
            .flex()
            .children(commands.iter().enumerate().flat_map(|(ix, command)| {
                let edge = if ix == 0 || ix == last {
                    COMMAND_EDGE_TRIM
                } else {
                    0.
                };
                let color = if command.enabled {
                    p.text_primary
                } else {
                    p.text_disabled
                };
                let divider = (ix > 0).then(|| {
                    div()
                        .flex_none()
                        .w(px(1.))
                        .h(px(COMMAND_DIVIDER_HEIGHT))
                        .mt(px(COMMAND_DIVIDER_TOP))
                        .bg(rgb(p.menu_separator))
                });
                let enabled = command.enabled;
                let button = div()
                    .id(("menu-command", ix))
                    .flex_none()
                    .w(px(cell - edge))
                    .h_full()
                    .p(px(COMMAND_INSET))
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.run_command(ix, window, cx)),
                    )
                    .child(
                        div()
                            .id(("menu-command-face", ix))
                            .size_full()
                            .flex()
                            .flex_col()
                            .items_center()
                            .pt(px(COMMAND_TOP - COMMAND_INSET))
                            .rounded(px(4.))
                            .text_color(rgba(color))
                            .child(
                                div()
                                    .h(px(COMMAND_LINE))
                                    .line_height(px(COMMAND_LINE))
                                    .font_family("Segoe Fluent Icons")
                                    .text_size(px(ICON_SIZE))
                                    .child(command.glyph),
                            )
                            .child(
                                div()
                                    .mt(px(COMMAND_LABEL_GAP))
                                    .h(px(COMMAND_LINE))
                                    .line_height(px(COMMAND_LINE))
                                    .font_family(UI_FONT)
                                    .text_size(px(COMMAND_LABEL_SIZE))
                                    .child(command.label),
                            )
                            .fade_bg(if enabled {
                                Brushes {
                                    rest: transparent_black().into(),
                                    hover: rgba(p.subtle_secondary),
                                    pressed: rgba(p.subtle_tertiary),
                                }
                            } else {
                                let clear = transparent_black().into();
                                Brushes {
                                    rest: clear,
                                    hover: clear,
                                    pressed: clear,
                                }
                            })
                            .when(enabled, |this| {
                                this.active(move |style| style.text_color(rgba(p.text_secondary)))
                            }),
                    );
                divider
                    .into_iter()
                    .map(IntoElement::into_any_element)
                    .chain(Some(button.into_any_element()))
            }))
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom(px(-1.))
                    .h(px(1.))
                    .bg(rgb(p.menu_separator)),
            )
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
            (_, Some(sub)) => match open.live_submenu() {
                Some(submenu) => submenu.panel.entries[sub].clone(),
                None => return,
            },
        };
        match entry {
            Entry::Submenu(..) => self.open_submenu(path.0, true, window, cx),
            Entry::Separator | Entry::Disabled(..) => {}
            entry => {
                let action = entry.action();
                self.close_menu(window, cx);
                if let Some(action) = action {
                    let target = self.action_context(cx);
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
        let action_context = self.action_context(cx);
        let serial = self.next_serial();
        self.submenu_timer = Task::ready(());
        let Some(open) = self.open.as_mut().filter(|open| !open.is_closing()) else {
            return;
        };
        let Entry::Submenu(_, entries) = &open.panel.entries[ix] else {
            return;
        };
        if open.live_submenu().is_none_or(|submenu| submenu.ix != ix) {
            open.submenu = Some(Submenu {
                ix,
                panel: Panel::new(entries.clone(), MIN_WIDTH, &action_context, window),
                motion: Motion::new(serial, false),
            });
        }
        if highlight_first && let Some(submenu) = open.live_submenu_mut() {
            submenu.panel.highlighted = None;
            submenu.panel.step(1);
        }
        cx.notify();
    }

    fn close_submenu(&mut self, cx: &mut Context<Self>) -> bool {
        self.submenu_timer = Task::ready(());
        let closed = self
            .open
            .as_mut()
            .and_then(OpenMenu::live_submenu_mut)
            .map(|submenu| submenu.motion.closing = true)
            .is_some();
        if closed {
            cx.notify();
        }
        closed
    }

    fn schedule_submenu(
        &mut self,
        change: SubmenuChange,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = match change {
            SubmenuChange::Keep => {
                self.submenu_timer = Task::ready(());
                return;
            }
            SubmenuChange::Open(ix) => Some(ix),
            SubmenuChange::Close => None,
        };
        let delay = menu_show_delay();
        self.submenu_timer = cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(delay).await;
            this.update_in(cx, |this, window, cx| match target {
                Some(ix) => this.open_submenu(ix, false, window, cx),
                None => {
                    this.close_submenu(cx);
                }
            })
            .ok();
        });
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
        let change = match path {
            (ix, None) => {
                open.panel.highlighted = open.panel.entries[ix].selectable().then_some(ix);
                let current = open.live_submenu().map(|submenu| submenu.ix);
                match (&open.panel.entries[ix], current) {
                    (Entry::Submenu(..), Some(open_ix)) if open_ix == ix => SubmenuChange::Keep,
                    (Entry::Submenu(..), _) => SubmenuChange::Open(ix),
                    (_, Some(_)) => SubmenuChange::Close,
                    (_, None) => SubmenuChange::Keep,
                }
            }
            (_, Some(sub)) => {
                if let Some(submenu) = open.live_submenu_mut() {
                    submenu.panel.highlighted =
                        submenu.panel.entries[sub].selectable().then_some(sub);
                }
                SubmenuChange::Keep
            }
        };
        self.schedule_submenu(change, window, cx);
        cx.notify();
    }

    fn switch(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(kind) = self.open_kind().filter(|kind| *kind != MenuKind::Flyout) else {
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
            let in_submenu = open
                .live_submenu()
                .is_some_and(|submenu| submenu.panel.highlighted.is_some());
            let panel = match open.live_submenu_mut().filter(|_| in_submenu) {
                Some(submenu) => &mut submenu.panel,
                None => &mut open.panel,
            };
            panel.highlighted = None;
            panel.step(delta);
            cx.notify();
        }
    }

    fn step_active(&mut self, delta: isize, cx: &mut Context<Self>) {
        if let Some(open) = self.open.as_mut().filter(|open| !open.is_closing()) {
            let in_submenu = open
                .live_submenu()
                .is_some_and(|submenu| submenu.panel.highlighted.is_some());
            match open.live_submenu_mut().filter(|_| in_submenu) {
                Some(submenu) => submenu.panel.step(delta),
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
            .live_submenu()
            .and_then(|submenu| submenu.panel.highlighted);
        if let Some(ix) = open.panel.highlighted {
            self.activate((ix, sub), window, cx);
        }
    }

    fn on_cancel(&mut self, _: &Cancel, window: &mut Window, cx: &mut Context<Self>) {
        if !self.close_submenu(cx) {
            self.close_menu(window, cx);
        }
    }

    fn on_left(&mut self, _: &SelectLeft, window: &mut Window, cx: &mut Context<Self>) {
        if !self.close_submenu(cx) {
            self.switch(-1, window, cx);
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

    fn trigger(&self, kind: MenuKind, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let p = palette(cx);
        let open = self.open.as_ref().filter(|open| open.kind == kind);
        let active = open.is_some_and(|open| !open.is_closing());
        let popup = open.map(|open| self.popup(open, cx));

        div()
            .id(kind.title())
            .relative()
            .flex()
            .items_center()
            .h_full()
            .px(px(10.))
            .rounded(px(4.))
            .when(active, |this| this.bg(rgba(p.subtle_tertiary)))
            .hover(move |style| style.bg(rgba(p.subtle_secondary)))
            .active(move |style| style.bg(rgba(p.subtle_tertiary)))
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

    fn popup(&self, open: &OpenMenu, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let p = palette(cx);
        let panel = &open.panel;
        let width = panel.width;
        let bar = if open.commands.is_empty() {
            0.
        } else {
            COMMAND_ROW + 1.
        };
        let height = panel.height() + bar;
        let hidden = panel.height() * CLOSED_RATIO * (1. - open.motion.unroll);
        let (top, offset) = match (bar > 0., open.upward) {
            (true, _) => (0., 0.),
            (false, true) => (hidden, hidden),
            (false, false) => (0., -hidden),
        };
        let hit_areas = self.hit_areas.clone();
        let submenu = open.submenu.as_ref().map(|submenu| {
            let sub_width = submenu.panel.width;
            let sub_height = submenu.panel.height();
            let hidden = sub_height * SUBMENU_CLOSED_RATIO * (1. - submenu.motion.unroll);
            let top = panel.row_top(submenu.ix) - 1. - PADDING_Y;
            let rows = self.panel_rows(&submenu.panel, Some(submenu.ix), 0., false, cx);
            let hit_areas = self.hit_areas.clone();
            div()
                .absolute()
                .left(px(width - SUBMENU_OVERLAP))
                .top(px(top))
                .w(px(sub_width))
                .h(px(sub_height))
                .opacity(submenu.motion.opacity)
                .occlude()
                .cursor_default()
                .children(reveal(
                    p,
                    SUBMENU_Z,
                    sub_height,
                    0.,
                    sub_height - hidden,
                    -hidden,
                    rows,
                ))
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
                )
        });

        div()
            .relative()
            .w(px(width))
            .h(px(height))
            .opacity(open.motion.opacity)
            .occlude()
            .cursor_default()
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
            .children(reveal(
                p,
                MENU_Z,
                height,
                top,
                height - hidden,
                offset,
                div()
                    .size_full()
                    .when(bar > 0., |this| {
                        this.child(self.command_row(&open.commands, width, cx))
                    })
                    .child(self.panel_rows(panel, None, bar, bar > 0., cx)),
            ))
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
            .children(submenu)
    }

    fn panel_rows(
        &self,
        panel: &Panel,
        parent: Option<usize>,
        offset: f32,
        fade: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let p = palette(cx);
        let checks = panel
            .entries
            .iter()
            .any(|entry| matches!(entry, Entry::Toggle(..) | Entry::Icon(..)));

        div()
            .absolute()
            .top(px(1. + PADDING_Y + offset))
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
                        .child(div().h(px(1.)).bg(rgb(p.menu_separator)));
                }
                let highlighted = panel.highlighted == Some(ix);
                let enabled = entry.selectable();
                let (label, mark) = match entry {
                    Entry::Command(label, _)
                    | Entry::Disabled(label, _)
                    | Entry::Submenu(label, _) => (*label, None),
                    Entry::Toggle(label, checked, _) => (*label, checked.then_some("\u{E73E}")),
                    Entry::Icon(label, glyph, ..) => (*label, Some(*glyph)),
                    Entry::Separator => unreachable!(),
                };
                let mark_size = if matches!(entry, Entry::Icon(..)) {
                    ICON_SIZE
                } else {
                    GLYPH_SIZE
                };
                let shortcut = panel.shortcuts[ix].clone();
                let fade = fade && enabled;
                let mark_color = match entry {
                    _ if !enabled => Some(p.text_disabled),
                    Entry::Toggle(..) if !highlighted => Some(p.text_secondary),
                    _ if fade => None,
                    _ => Some(p.text_primary),
                };
                let label_color = match () {
                    _ if !enabled => Some(p.text_disabled),
                    _ if fade => None,
                    _ => Some(p.text_primary),
                };

                let row_id = ix + parent.map_or(0, |p| 1000 * (p + 1));
                div()
                    .id(("menu-row", row_id))
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
                    .child({
                        let face = div()
                            .id(("menu-row-face", row_id))
                            .flex()
                            .items_center()
                            .size_full()
                            .px(px(ROW_PADDING))
                            .rounded(px(4.))
                            .text_color(rgba(p.text_primary))
                            .when(checks, |this| {
                                this.child(
                                    div()
                                        .flex_none()
                                        .w(px(CHECK_COLUMN))
                                        .font_family("Segoe Fluent Icons")
                                        .text_size(px(mark_size))
                                        .when_some(mark_color, |this, color| {
                                            this.text_color(rgba(color))
                                        })
                                        .children(mark),
                                )
                            })
                            .child(
                                div()
                                    .flex_1()
                                    .font_family(MENU_FONT)
                                    .text_size(px(LABEL_SIZE))
                                    .when_some(label_color, |this, color| {
                                        this.text_color(rgba(color))
                                    })
                                    .child(label),
                            )
                            .when_some(shortcut, |this, shortcut| {
                                this.child(
                                    div()
                                        .flex_none()
                                        .font_family(UI_FONT)
                                        .text_size(px(SHORTCUT_SIZE))
                                        .text_color(rgba(if enabled {
                                            p.text_secondary
                                        } else {
                                            p.text_disabled
                                        }))
                                        .child(shortcut),
                                )
                            })
                            .when(matches!(entry, Entry::Submenu(..)), |this| {
                                this.child(
                                    div()
                                        .flex_none()
                                        .font_family("Segoe Fluent Icons")
                                        .text_size(px(GLYPH_SIZE))
                                        .text_color(rgba(p.text_secondary))
                                        .child("\u{E76C}"),
                                )
                            });
                        if fade {
                            face.fade_bg(Brushes {
                                rest: transparent_black().into(),
                                hover: rgba(p.subtle_secondary),
                                pressed: rgba(p.subtle_tertiary),
                            })
                            .highlighted(highlighted)
                            .active(move |style| style.text_color(rgba(p.text_secondary)))
                            .into_any_element()
                        } else {
                            face.when(highlighted, |this| this.bg(rgba(p.subtle_secondary)))
                                .when(enabled, |this| {
                                    this.active(move |style| style.bg(rgba(p.subtle_tertiary)))
                                })
                                .into_any_element()
                        }
                    })
            }))
    }
}

fn reveal(
    p: Palette,
    z: f32,
    height: f32,
    top: f32,
    shown: f32,
    offset: f32,
    rows: Div,
) -> [Div; 2] {
    let card = div()
        .absolute()
        .top(px(top))
        .left_0()
        .w_full()
        .h(px(shown))
        .bg(rgb(p.menu_fill))
        .border_1()
        .border_color(rgb(p.menu_border))
        .rounded(px(8.))
        .shadow(theme_shadow(p, z, 1.));
    [
        card,
        div()
            .absolute()
            .top(px(top))
            .left_0()
            .w_full()
            .h(px(shown))
            .overflow_hidden()
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top(px(offset - top))
                    .w_full()
                    .h(px(height))
                    .child(rows),
            ),
    ]
}

pub(crate) fn text_width(window: &Window, text: &str, family: &'static str, size: f32) -> f32 {
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

impl EventEmitter<MenuEvent> for MenuBar {}

impl Render for MenuBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        if self
            .open
            .as_mut()
            .is_some_and(|open| open.sample(window, cx))
        {
            self.open = None;
        }
        if self.open.as_ref().is_none_or(|open| open.submenu.is_none()) {
            let mut areas = self.hit_areas.get();
            areas[2] = Bounds::default();
            self.hit_areas.set(areas);
        }
        let hit_areas = self.hit_areas.clone();
        let flyout = self.open.as_ref().and_then(|open| {
            let anchor = open.anchor?;
            Some(deferred(anchored().position(anchor).child(self.popup(open, cx))).with_priority(1))
        });

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
            .text_color(rgba(p.text_primary))
            .on_action(cx.listener(Self::on_up))
            .on_action(cx.listener(Self::on_down))
            .on_action(cx.listener(Self::on_first))
            .on_action(cx.listener(Self::on_last))
            .on_action(cx.listener(Self::on_confirm))
            .on_action(cx.listener(Self::on_cancel))
            .on_action(cx.listener(Self::on_left))
            .on_action(cx.listener(Self::on_right))
            .children(MenuKind::ALL.map(|kind| self.trigger(kind, cx)))
            .children(flyout)
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
