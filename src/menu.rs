use super::*;
use gpui_kit::base::actions::{Cancel, Confirm, SelectDown, SelectLeft, SelectRight, SelectUp};
use gpui_kit::base::{MotionStatus, Presence, Transition, transition_with_status};
use windows_sys::Win32::System::DataExchange::IsClipboardFormatAvailable;
use windows_sys::Win32::System::Ole::CF_UNICODETEXT;
use windows_sys::Win32::UI::WindowsAndMessaging::SPI_GETMENUSHOWDELAY;

const CONTEXT: &str = "JotMenu";
const OPEN_DURATION: Duration = Duration::from_millis(250);
const FADE_DURATION: Duration = Duration::from_millis(83);
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
const COMMAND_ROW: f32 = 59.;
const COMMAND_BUTTON: f32 = 58.;
const COMMAND_GAP: f32 = 1.;
const COMMAND_ICON_TOP: f32 = 9.;
const COMMAND_LABEL_PANEL: f32 = 60.;
const COMMAND_LABEL_GAP: f32 = 4.;
const COMMAND_LINE: f32 = 16.;
const COMMAND_LABEL_SIZE: f32 = 12.;
const COMMAND_DIVIDER_TOP: f32 = 14.;
const COMMAND_DIVIDER_HEIGHT: f32 = 32.;
const COMMAND_OUTSET: f32 = 1.;
const OVERFLOW_TOP: f32 = 1.;
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
const BORDER: f32 = 1.;
const BAR_ITEM_PADDING: f32 = 10.;
const BAR_GAP: f32 = 8.;
const BAR_AREA: usize = 0;
const MENU_AREA: usize = 1;
const SUBMENU_AREA: usize = 2;
const CHECK_GLYPH: &str = "\u{E73E}";
const CHEVRON_GLYPH: &str = "\u{E76C}";
const BAR_FOCUS_OUTSET: Edges<f32> = Edges {
    top: 0.,
    right: 1.,
    bottom: 1.,
    left: 1.,
};

pub(crate) fn key_bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("up", SelectUp, Some(CONTEXT)),
        KeyBinding::new("down", SelectDown, Some(CONTEXT)),
        KeyBinding::new("enter", Confirm { secondary: false }, Some(CONTEXT)),
        KeyBinding::new("space", Confirm { secondary: false }, Some(CONTEXT)),
        KeyBinding::new("escape", Cancel, Some(CONTEXT)),
        KeyBinding::new("left", SelectLeft, Some(CONTEXT)),
        KeyBinding::new("right", SelectRight, Some(CONTEXT)),
    ]
}

fn open_transition() -> Transition {
    Transition::new(OPEN_DURATION).easing(DECELERATE)
}

fn fade_transition() -> Transition {
    Transition::new(FADE_DURATION).easing(Easing::Linear)
}

pub(crate) type ActionFn = fn() -> Box<dyn Action>;

#[derive(Clone, Copy)]
pub(crate) struct Command {
    label: &'static str,
    enabled: bool,
    action: ActionFn,
}

impl Command {
    fn new(label: &'static str, enabled: bool, action: ActionFn) -> Self {
        Self {
            label,
            enabled,
            action,
        }
    }
}

struct CommandButton {
    command: Command,
    glyph: &'static str,
}

pub(crate) enum MenuEvent {
    Opened,
    Closed,
}

fn context_menu(editor: &TextareaState) -> (Vec<CommandButton>, Vec<Entry>) {
    let selection = !editor.selected_range().is_empty();
    let button = |label, glyph, enabled, action| CommandButton {
        command: Command::new(label, enabled, action),
        glyph,
    };
    let commands = vec![
        button("Cut", "\u{E8C6}", selection, || Box::new(input::Cut)),
        button("Copy", "\u{E8C8}", selection, || Box::new(input::Copy)),
        button("Paste", "\u{E77F}", clipboard_has_text(), || {
            Box::new(input::Paste)
        }),
        button("Select all", "\u{E8B3}", true, || {
            Box::new(input::SelectAll)
        }),
        button("Delete", "\u{E74D}", selection, || Box::new(input::Delete)),
    ];
    let entries = vec![Entry::Command {
        command: Command::new("Undo", editor.can_undo(), || Box::new(input::Undo)),
        icon: Some("\u{E7A7}"),
    }];
    (commands, entries)
}

fn clipboard_has_text() -> bool {
    unsafe { IsClipboardFormatAvailable(CF_UNICODETEXT.into()) != 0 }
}

fn display_label(label: &str) -> SharedString {
    label.replacen('&', "", 1).into()
}

#[derive(Clone)]
pub(crate) enum Entry {
    Command {
        command: Command,
        icon: Option<&'static str>,
    },
    Toggle {
        label: &'static str,
        checked: bool,
        action: ActionFn,
    },
    Submenu(&'static str, Vec<Entry>),
    Separator,
}

impl Entry {
    fn command(label: &'static str, action: ActionFn) -> Self {
        Self::command_if(label, true, action)
    }

    fn command_if(label: &'static str, enabled: bool, action: ActionFn) -> Self {
        Self::Command {
            command: Command::new(label, enabled, action),
            icon: None,
        }
    }

    pub(crate) fn toggle(label: &'static str, checked: bool, action: ActionFn) -> Self {
        Self::Toggle {
            label,
            checked,
            action,
        }
    }

    fn height(&self) -> f32 {
        match self {
            Self::Command { icon: Some(_), .. } => ROW_ICON,
            Self::Command { icon: None, .. } => ROW_COMMAND,
            Self::Toggle { .. } | Self::Submenu(..) => ROW_COMPACT,
            Self::Separator => ROW_SEPARATOR,
        }
    }

    fn selectable(&self) -> bool {
        match self {
            Self::Command { command, .. } => command.enabled,
            Self::Toggle { .. } | Self::Submenu(..) => true,
            Self::Separator => false,
        }
    }

    fn label(&self) -> Option<&'static str> {
        match self {
            Self::Command { command, .. } => Some(command.label),
            Self::Toggle { label, .. } | Self::Submenu(label, _) => Some(label),
            Self::Separator => None,
        }
    }

    fn access_key(&self) -> Option<char> {
        let (_, key) = self
            .label()
            .filter(|_| self.selectable())?
            .split_once('&')?;
        key.chars().next().map(|key| key.to_ascii_uppercase())
    }

    fn action(&self) -> Option<Box<dyn Action>> {
        match self {
            Self::Command { command, .. } => Some((command.action)()),
            Self::Toggle { action, .. } => Some(action()),
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

    fn access_key(self) -> Option<char> {
        self.title().chars().next()
    }

    fn index(self) -> usize {
        self as usize
    }

    fn cycle(self, delta: isize) -> Self {
        cycle(Some(self.index()), Self::ALL.len(), delta, true, |_| true)
            .map_or(self, |ix| Self::ALL[ix])
    }

    fn entries(self, editor: &TextareaState, cx: &App) -> Vec<Entry> {
        match self {
            Self::File => vec![
                Entry::command("&New tab", || Box::new(NewTab)),
                Entry::command("New &window", || Box::new(NewWindow)),
                Entry::command("&Open", || Box::new(Open)),
                Entry::command("&Save", || Box::new(Save)),
                Entry::command("Save &as", || Box::new(SaveAs)),
                Entry::command("Save a&ll", || Box::new(SaveAll)),
                Entry::Separator,
                Entry::command("&Close tab", || Box::new(CloseTab)),
                Entry::command("Clos&e window", || Box::new(CloseWindow)),
                Entry::command("E&xit", || Box::new(Exit)),
            ],
            Self::Edit => {
                let selection = !editor.selected_range().is_empty();
                let clipboard_text = clipboard_has_text();
                let text = editor.text().len() > 0;
                vec![
                    Entry::command_if("&Undo", editor.can_undo(), || Box::new(input::Undo)),
                    Entry::Separator,
                    Entry::command_if("Cu&t", selection, || Box::new(input::Cut)),
                    Entry::command_if("&Copy", selection, || Box::new(input::Copy)),
                    Entry::command_if("&Paste", clipboard_text, || Box::new(input::Paste)),
                    Entry::command_if("De&lete", selection, || Box::new(input::Delete)),
                    Entry::Separator,
                    Entry::command_if("&Find", text, || Box::new(Find)),
                    Entry::command_if("Find &next", text, || Box::new(FindNext)),
                    Entry::command_if("Find pre&vious", text, || Box::new(FindPrevious)),
                    Entry::command_if("&Replace", text, || Box::new(Replace)),
                    Entry::command("&Go to", || Box::new(GoTo)),
                    Entry::Separator,
                    Entry::command("Select &all", || Box::new(input::SelectAll)),
                    Entry::command("Time/&Date", || Box::new(TimeDate)),
                ]
            }
            Self::View => {
                let view = cx.global::<ViewSettings>();
                vec![
                    Entry::Submenu(
                        "&Zoom",
                        vec![
                            Entry::command("Zoom &in", || Box::new(ZoomIn)),
                            Entry::command("Zoom &out", || Box::new(ZoomOut)),
                            Entry::command("&Restore default zoom", || Box::new(ResetZoom)),
                        ],
                    ),
                    Entry::toggle("&Status bar", view.status_bar, || Box::new(ToggleStatusBar)),
                    Entry::toggle("&Word wrap", view.word_wrap, || Box::new(ToggleWordWrap)),
                ]
            }
        }
    }
}

struct Panel {
    entries: Vec<Entry>,
    labels: Vec<SharedString>,
    shortcuts: Vec<Option<SharedString>>,
    width: f32,
    checks: bool,
    focused: Option<usize>,
}

impl Panel {
    fn new(
        entries: Vec<Entry>,
        min_width: f32,
        shortcuts: bool,
        action_context: &FocusHandle,
        window: &Window,
    ) -> Self {
        let labels: Vec<_> = entries
            .iter()
            .map(|entry| entry.label().map(display_label).unwrap_or_default())
            .collect();
        let shortcuts: Vec<_> = entries
            .iter()
            .map(|entry| {
                entry
                    .action()
                    .filter(|_| shortcuts)
                    .and_then(|action| shortcut_text(action.as_ref(), action_context, window))
            })
            .collect();
        let checks = entries.iter().any(|entry| {
            matches!(
                entry,
                Entry::Toggle { .. } | Entry::Command { icon: Some(_), .. }
            )
        });
        let content = entries
            .iter()
            .zip(&labels)
            .zip(&shortcuts)
            .map(|((entry, label), shortcut)| {
                let label = text_width(window, label, TEXT_FONT, LABEL_SIZE);
                let trailing = match (entry, shortcut) {
                    (Entry::Submenu(..), _) => SHORTCUT_GAP + CHEVRON_WIDTH,
                    (_, Some(shortcut)) => {
                        SHORTCUT_GAP + text_width(window, shortcut, SMALL_FONT, SHORTCUT_SIZE)
                    }
                    _ => 0.,
                };
                label + trailing
            })
            .fold(0., f32::max);
        let check = if checks { CHECK_COLUMN } else { 0. };
        let width = (2. * (BORDER + ROW_INSET + ROW_PADDING) + check + content).ceil();

        Self {
            entries,
            labels,
            shortcuts,
            width: width.max(min_width),
            checks,
            focused: None,
        }
    }

    fn height(&self) -> f32 {
        2. * (BORDER + PADDING_Y) + self.entries.iter().map(Entry::height).sum::<f32>()
    }

    fn row_top(&self, ix: usize) -> f32 {
        BORDER + PADDING_Y + self.entries[..ix].iter().map(Entry::height).sum::<f32>()
    }

    fn find_key(&self, key: char) -> Option<usize> {
        self.entries
            .iter()
            .position(|entry| entry.access_key() == Some(key))
    }

    fn step(&mut self, delta: isize) {
        if let Some(ix) = cycle(self.focused, self.entries.len(), delta, true, |ix| {
            self.entries[ix].selectable()
        }) {
            self.focused = Some(ix);
        }
    }
}

fn cycle(
    current: Option<usize>,
    count: usize,
    delta: isize,
    wrap: bool,
    accept: impl Fn(usize) -> bool,
) -> Option<usize> {
    let count = count as isize;
    let mut ix = current.map_or(if delta > 0 { -1 } else { count }, |ix| ix as isize);
    (0..count).find_map(|_| {
        ix += delta;
        if wrap {
            ix = ix.rem_euclid(count);
        } else if !(0..count).contains(&ix) {
            return None;
        }
        let at = ix as usize;
        accept(at).then_some(at)
    })
}

fn menu_show_delay() -> Duration {
    Duration::from_millis(
        system_u32(SPI_GETMENUSHOWDELAY).map_or(DEFAULT_MENU_SHOW_DELAY_MS, u64::from),
    )
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
                .transition(fade_transition())
                .sample(window, cx)
                .progress
        } else {
            1.
        };
        let fade = transition_with_status(
            ElementId::from(("jot-menu-fade", self.serial)),
            if self.closing { 0. } else { 1. },
            fade_transition(),
            window,
            cx,
        );
        self.opacity = fade.value * appear;
        self.closing && fade.status == MotionStatus::Finished
    }
}

struct OpenSubmenu {
    ix: usize,
    panel: Panel,
    motion: Motion,
}

struct OpenMenu {
    kind: Option<MenuKind>,
    anchor: Option<Point<Pixels>>,
    upward: bool,
    commands: Vec<CommandButton>,
    return_focus: Option<FocusHandle>,
    panel: Panel,
    submenu: Option<OpenSubmenu>,
    motion: Motion,
    focused_command: Option<usize>,
    focus_visible: bool,
}

impl OpenMenu {
    fn new(kind: Option<MenuKind>, panel: Panel) -> Self {
        Self {
            kind,
            anchor: None,
            upward: false,
            commands: Vec::new(),
            return_focus: None,
            panel,
            submenu: None,
            motion: Motion::new(0, false),
            focused_command: None,
            focus_visible: false,
        }
    }

    fn focused_submenu(&self) -> Option<&OpenSubmenu> {
        self.live_submenu()
            .filter(|submenu| submenu.panel.focused.is_some())
    }

    fn in_submenu(&self) -> bool {
        self.focused_submenu().is_some()
    }

    fn step_linear(&mut self, delta: isize) {
        let commands = self.commands.len();
        let current = self
            .focused_command
            .or(self.panel.focused.map(|ix| commands + ix));
        let count = commands + self.panel.entries.len();
        if let Some(at) = cycle(current, count, delta, true, |at| {
            if at < commands {
                self.commands[at].command.enabled
            } else {
                self.panel.entries[at - commands].selectable()
            }
        }) {
            self.focused_command = (at < commands).then_some(at);
            self.panel.focused = at.checked_sub(commands);
        }
    }

    fn step_command(&mut self, delta: isize) {
        let Some(ix) = self.focused_command else {
            return;
        };
        if let Some(ix) = cycle(Some(ix), self.commands.len(), delta, false, |ix| {
            self.commands[ix].command.enabled
        }) {
            self.focused_command = Some(ix);
        }
    }

    fn is_closing(&self) -> bool {
        self.motion.closing
    }

    fn live_submenu(&self) -> Option<&OpenSubmenu> {
        self.submenu.as_ref().filter(|sub| !sub.motion.closing)
    }

    fn live_submenu_mut(&mut self) -> Option<&mut OpenSubmenu> {
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

pub(crate) struct MenuBar {
    jot: WeakEntity<Jot>,
    focus: FocusHandle,
    open: Option<OpenMenu>,
    serial: usize,
    submenu_timer: Task<()>,
    hit_areas: Rc<Cell<[Bounds<Pixels>; 3]>>,
    trigger_areas: Rc<Cell<[Bounds<Pixels>; 3]>>,
    bar_focus: Option<MenuKind>,
    access: bool,
    bounds: Bounds<Pixels>,
    _window_changes: [Subscription; 2],
}

impl MenuBar {
    pub(crate) fn new(jot: WeakEntity<Jot>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let _window_changes = [
            cx.observe_window_activation(window, |this, window, cx| {
                if !window.is_window_active() {
                    this.cancel(window, cx);
                }
            }),
            cx.observe_window_bounds(window, |this, window, cx| {
                let bounds = window.bounds();
                if bounds != this.bounds {
                    this.bounds = bounds;
                    this.cancel(window, cx);
                }
            }),
        ];
        Self {
            jot,
            focus: cx.focus_handle(),
            open: None,
            serial: 0,
            submenu_timer: Task::ready(()),
            hit_areas: Rc::new(Cell::new([Bounds::default(); 3])),
            trigger_areas: Rc::new(Cell::new([Bounds::default(); 3])),
            bar_focus: None,
            access: false,
            bounds: window.bounds(),
            _window_changes,
        }
    }

    pub(crate) fn is_open(&self) -> bool {
        self.live_open().is_some()
    }

    pub(crate) fn access_active(&self) -> bool {
        self.access
    }

    pub(crate) fn exit_access(&mut self, cx: &mut Context<Self>) {
        if self.access {
            self.access = false;
            cx.notify();
        }
    }

    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.live_open().is_some() {
            self.close_menu(window, cx);
        } else if self.bar_focus.is_some() {
            self.leave_bar(window, cx);
        }
        self.bar_focus = None;
        self.exit_access(cx);
    }

    pub(crate) fn width(&self) -> f32 {
        self.hit_areas.get()[BAR_AREA].size.width.as_f32()
    }

    pub(crate) fn pointer_down(&mut self, cx: &mut Context<Self>) {
        self.bar_focus = None;
        self.exit_access(cx);
    }

    pub(crate) fn key_down(
        &mut self,
        keystroke: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let modifiers = keystroke.modifiers;
        if keystroke.key == "alt" && !modifiers.modified() {
            self.toggle_access(window, cx);
            return true;
        }
        let key = match keystroke.key.as_bytes() {
            [byte] if byte.is_ascii_alphanumeric() => Some(char::from(byte.to_ascii_uppercase())),
            _ => None,
        };
        if !self.access {
            return match key {
                Some(key) if modifiers.alt && !modifiers.control && !modifiers.platform => {
                    self.invoke_access(key, window, cx)
                }
                _ => false,
            };
        }
        match keystroke.key.as_str() {
            "escape" => {
                self.access_back(window, cx);
                true
            }
            "up" | "down" | "left" | "right" | "tab" | "space" | "enter" => {
                self.exit_access(cx);
                false
            }
            _ => {
                if let Some(key) = key.filter(|_| !modifiers.control && !modifiers.platform) {
                    self.invoke_access(key, window, cx);
                }
                true
            }
        }
    }

    fn toggle_access(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.access {
            self.exit_access(cx);
            return;
        }
        match self.live_open() {
            Some(open) if open.kind.is_none() => return,
            Some(_) => {}
            None => {
                self.bar_focus = Some(MenuKind::File);
                self.focus.focus(window, cx);
            }
        }
        self.access = true;
        cx.notify();
    }

    fn access_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.live_open_mut() {
            Some(open) => match open.live_submenu().map(|submenu| submenu.ix) {
                Some(ix) => {
                    open.panel.focused = Some(ix);
                    self.close_submenu(cx);
                }
                None => self.close_to_bar(window, cx),
            },
            None => {
                self.access = false;
                self.leave_bar(window, cx);
            }
        }
    }

    fn invoke_access(&mut self, key: char, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(open) = self.live_open_mut() else {
            let Some(kind) = MenuKind::ALL
                .into_iter()
                .find(|kind| kind.access_key() == Some(key))
            else {
                return false;
            };
            self.open_by_keyboard(kind, window, cx);
            self.access = true;
            return true;
        };
        if open.kind.is_none() {
            return false;
        }
        if let Some(submenu) = open.focused_submenu() {
            let Some(sub) = submenu.panel.find_key(key) else {
                return false;
            };
            let parent = submenu.ix;
            self.access = false;
            self.activate((parent, Some(sub)), window, cx);
            return true;
        }
        let Some(ix) = open.panel.find_key(key) else {
            return false;
        };
        if matches!(open.panel.entries[ix], Entry::Submenu(..)) {
            open.panel.focused = Some(ix);
            self.access = true;
            self.open_submenu(ix, true, window, cx);
        } else {
            self.access = false;
            self.activate((ix, None), window, cx);
        }
        true
    }

    fn open_by_keyboard(&mut self, kind: MenuKind, window: &mut Window, cx: &mut Context<Self>) {
        self.open_menu(kind, window, cx);
        if let Some(open) = self.open.as_mut() {
            open.panel.step(1);
            open.focus_visible = true;
        }
    }

    fn close_to_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.live_open_mut() else {
            return;
        };
        let Some(kind) = open.kind else {
            return self.close_menu(window, cx);
        };
        open.motion.closing = true;
        self.submenu_timer = Task::ready(());
        self.bar_focus = Some(kind);
        self.focus.focus(window, cx);
        cx.emit(MenuEvent::Closed);
        cx.notify();
    }

    fn leave_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.bar_focus = None;
        if let Some(target) = self.action_context(cx) {
            target.focus(window, cx);
        }
        cx.notify();
    }

    fn bar_focused(&self, window: &Window) -> Option<MenuKind> {
        self.bar_focus
            .filter(|_| self.live_open().is_none() && self.focus.is_focused(window))
    }

    fn move_bar_focus(&mut self, delta: isize, cx: &mut Context<Self>) {
        if let Some(kind) = self.bar_focus {
            self.bar_focus = Some(kind.cycle(delta));
            cx.notify();
        }
    }

    fn editor(&self, cx: &App) -> Option<Entity<TextareaState>> {
        self.jot
            .upgrade()
            .map(|jot| jot.read(cx).active_editor().clone())
    }

    fn action_context(&self, cx: &App) -> Option<FocusHandle> {
        self.editor(cx).map(|editor| editor.focus_handle(cx))
    }

    fn next_serial(&mut self) -> usize {
        self.serial += 1;
        self.serial
    }

    fn live_open(&self) -> Option<&OpenMenu> {
        self.open.as_ref().filter(|open| !open.is_closing())
    }

    fn live_open_mut(&mut self) -> Option<&mut OpenMenu> {
        self.open.as_mut().filter(|open| !open.is_closing())
    }

    fn open_kind(&self) -> Option<MenuKind> {
        self.live_open().and_then(|open| open.kind)
    }

    fn open_menu(&mut self, kind: MenuKind, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.editor(cx) else {
            return;
        };
        let entries = kind.entries(editor.read(cx), cx);
        let panel = Panel::new(entries, MIN_WIDTH, true, &editor.focus_handle(cx), window);
        self.show(OpenMenu::new(Some(kind), panel), window, cx);
    }

    pub(crate) fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    pub(crate) fn open_context_menu(
        &mut self,
        editor: &Entity<TextareaState>,
        anchor: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (commands, entries) = context_menu(editor.read(cx));
        let panel = Panel::new(
            entries,
            CONTEXT_WIDTH,
            false,
            &editor.focus_handle(cx),
            window,
        );
        let focused_command = commands.iter().position(|button| button.command.enabled);
        self.show(
            OpenMenu {
                anchor: Some(anchor),
                commands,
                focused_command,
                focus_visible: window.last_input_was_keyboard(),
                ..OpenMenu::new(None, panel)
            },
            window,
            cx,
        );
    }

    pub(crate) fn open_flyout_above(
        &mut self,
        editor: &Entity<TextareaState>,
        entries: Vec<Entry>,
        target: Bounds<Pixels>,
        return_focus: FocusHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let panel = Panel::new(
            entries,
            FLYOUT_MIN_WIDTH,
            true,
            &editor.focus_handle(cx),
            window,
        );
        let anchor = point(
            target.center().x - px((panel.width / 2.).floor()),
            target.top() - px(FLYOUT_GAP + panel.height()),
        );
        self.show(
            OpenMenu {
                anchor: Some(anchor),
                upward: true,
                return_focus: Some(return_focus),
                ..OpenMenu::new(None, panel)
            },
            window,
            cx,
        );
    }

    fn show(&mut self, open: OpenMenu, window: &mut Window, cx: &mut Context<Self>) {
        self.bar_focus = None;
        let motion = Motion::new(self.next_serial(), !open.commands.is_empty());
        self.open = Some(OpenMenu { motion, ..open });
        self.submenu_timer = Task::ready(());
        self.focus.focus(window, cx);
        cx.emit(MenuEvent::Opened);
        cx.notify();
    }

    fn close_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.access = false;
        let return_focus = self.live_open_mut().and_then(|open| {
            open.motion.closing = true;
            open.return_focus.take()
        });
        self.submenu_timer = Task::ready(());
        if let Some(target) = return_focus.or_else(|| self.action_context(cx)) {
            target.focus(window, cx);
        }
        cx.emit(MenuEvent::Closed);
        cx.notify();
    }

    fn run_command(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(command) = self
            .live_open()
            .and_then(|open| open.commands.get(ix))
            .map(|button| button.command)
            .filter(|command| command.enabled)
        else {
            return;
        };
        let action = (command.action)();
        self.dispatch(action, window, cx);
    }

    fn dispatch(&mut self, action: Box<dyn Action>, window: &mut Window, cx: &mut Context<Self>) {
        self.close_menu(window, cx);
        let Some(target) = self.action_context(cx) else {
            return;
        };
        window.defer(cx, move |window, cx| {
            target.dispatch_action(action.as_ref(), window, cx);
        });
    }

    fn command_row(open: &OpenMenu, window: &Window, cx: &mut Context<Self>) -> Div {
        let p = palette(cx);
        let commands = &open.commands;
        let count = commands.len() as f32;
        let row = open.panel.width + 2. * COMMAND_OUTSET;
        let cell = ((row - (count - 1.)) / count).floor();
        let ring = open.focused_command.filter(|_| open.focus_visible);
        div()
            .absolute()
            .top(px(BORDER))
            .left(px(-COMMAND_OUTSET))
            .w(px(row))
            .h(px(COMMAND_ROW))
            .flex()
            .children(commands.iter().enumerate().flat_map(|(ix, command)| {
                let divider = (ix > 0).then(|| {
                    div()
                        .flex_none()
                        .w(px(1.))
                        .h(px(COMMAND_DIVIDER_HEIGHT))
                        .mt(px(COMMAND_DIVIDER_TOP))
                        .bg(rgb(p.menu_separator))
                });
                let button =
                    Self::command_button(p, ix, command, cell, ring == Some(ix), window, cx);
                divider
                    .into_iter()
                    .map(IntoElement::into_any_element)
                    .chain(Some(button.into_any_element()))
            }))
            .child(
                div()
                    .absolute()
                    .left(px(BORDER + COMMAND_OUTSET))
                    .right(px(BORDER + COMMAND_OUTSET))
                    .bottom(px(-1.))
                    .h(px(1.))
                    .bg(rgb(p.menu_separator)),
            )
    }

    fn command_button(
        p: Palette,
        ix: usize,
        button: &CommandButton,
        cell: f32,
        focused: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let face = cell - 2. * (COMMAND_GAP + COMMAND_INSET);
        let CommandButton { command, glyph } = button;
        let enabled = command.enabled;
        let color = if enabled {
            p.text_primary
        } else {
            p.text_disabled
        };
        let label_width = text_width(window, command.label, SMALL_FONT, COMMAND_LABEL_SIZE);
        let label_left = (face - COMMAND_LABEL_PANEL) / 2.
            + round_half_up((COMMAND_LABEL_PANEL - label_width) / 2.);
        div()
            .id(("menu-command", ix))
            .flex_none()
            .w(px(cell))
            .h_full()
            .pt(px(COMMAND_ROW - COMMAND_BUTTON + COMMAND_INSET))
            .px(px(COMMAND_GAP + COMMAND_INSET))
            .pb(px(COMMAND_INSET))
            .on_click(cx.listener(move |this, _, window, cx| this.run_command(ix, window, cx)))
            .child(
                div()
                    .id(("menu-command-face", ix))
                    .size_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .pt(px(COMMAND_ICON_TOP))
                    .rounded(px(field::CONTROL_RADIUS))
                    .text_color(rgba(color))
                    .child(
                        div()
                            .h(px(COMMAND_LINE))
                            .line_height(px(COMMAND_LINE))
                            .font_family(ICON_FONT)
                            .text_size(px(ICON_SIZE))
                            .child(*glyph),
                    )
                    .child(
                        div()
                            .self_start()
                            .ml(px(label_left))
                            .w(px(label_width))
                            .text_center()
                            .mt(px(COMMAND_LABEL_GAP))
                            .h(px(COMMAND_LINE))
                            .line_height(px(COMMAND_LINE))
                            .font_family(SMALL_FONT)
                            .text_size(px(COMMAND_LABEL_SIZE))
                            .child(command.label),
                    )
                    .brush_transition(if enabled {
                        Brushes::subtle(p)
                    } else {
                        Brushes::solid(transparent_black().into())
                    })
                    .when(enabled, |this| {
                        this.active(move |style| style.text_color(rgba(p.text_secondary)))
                    })
                    .when(focused, |this| {
                        this.child(field::focus_ring(
                            p,
                            Edges::default(),
                            px(field::CONTROL_RADIUS),
                        ))
                    }),
            )
    }

    fn activate(
        &mut self,
        path: (usize, Option<usize>),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(open) = self.live_open() else {
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
            entry if !entry.selectable() => {}
            entry => {
                if let Some(action) = entry.action() {
                    self.dispatch(action, window, cx);
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
        let Some(action_context) = self.action_context(cx) else {
            return;
        };
        let serial = self.next_serial();
        self.submenu_timer = Task::ready(());
        let Some(open) = self.live_open_mut() else {
            return;
        };
        let Entry::Submenu(_, entries) = &open.panel.entries[ix] else {
            return;
        };
        if open.live_submenu().is_none_or(|submenu| submenu.ix != ix) {
            open.submenu = Some(OpenSubmenu {
                ix,
                panel: Panel::new(entries.clone(), MIN_WIDTH, true, &action_context, window),
                motion: Motion::new(serial, false),
            });
        }
        open.focus_visible |= highlight_first;
        if highlight_first && let Some(submenu) = open.live_submenu_mut() {
            submenu.panel.focused = None;
            submenu.panel.step(1);
        }
        cx.notify();
    }

    fn close_submenu(&mut self, cx: &mut Context<Self>) {
        self.submenu_timer = Task::ready(());
        if let Some(submenu) = self.open.as_mut().and_then(OpenMenu::live_submenu_mut) {
            submenu.motion.closing = true;
            cx.notify();
        }
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
                None => this.close_submenu(cx),
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
        let Some(open) = self.live_open() else {
            return;
        };
        let change = match path {
            (ix, None) => {
                let current = open.live_submenu().map(|submenu| submenu.ix);
                match (&open.panel.entries[ix], current) {
                    (Entry::Submenu(..), Some(open_ix)) if open_ix == ix => SubmenuChange::Keep,
                    (Entry::Submenu(..), _) => SubmenuChange::Open(ix),
                    (_, Some(_)) => SubmenuChange::Close,
                    (_, None) => SubmenuChange::Keep,
                }
            }
            (_, Some(_)) => SubmenuChange::Keep,
        };
        self.schedule_submenu(change, window, cx);
        cx.notify();
    }

    fn switch(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(kind) = self.open_kind() else {
            return;
        };
        self.open_by_keyboard(kind.cycle(delta), window, cx);
    }

    fn on_up(&mut self, _: &SelectUp, window: &mut Window, cx: &mut Context<Self>) {
        self.step_active(-1, window, cx);
    }

    fn on_down(&mut self, _: &SelectDown, window: &mut Window, cx: &mut Context<Self>) {
        self.step_active(1, window, cx);
    }

    fn step_active(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.live_open_mut() else {
            if delta > 0
                && let Some(kind) = self.bar_focused(window)
            {
                self.open_by_keyboard(kind, window, cx);
            }
            return;
        };
        open.focus_visible = true;
        if open.in_submenu() {
            if let Some(submenu) = open.live_submenu_mut() {
                submenu.panel.step(delta);
            }
        } else if open.commands.is_empty() {
            open.panel.step(delta);
        } else {
            open.step_linear(delta);
        }
        cx.notify();
    }

    fn on_confirm(&mut self, _: &Confirm, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.live_open() else {
            if let Some(kind) = self.bar_focused(window) {
                self.open_by_keyboard(kind, window, cx);
            }
            return;
        };
        if let Some(ix) = open.focused_command {
            self.run_command(ix, window, cx);
            return;
        }
        let sub = open
            .live_submenu()
            .and_then(|submenu| submenu.panel.focused);
        if let Some(ix) = open.panel.focused {
            self.activate((ix, sub), window, cx);
        }
    }

    fn on_cancel(&mut self, _: &Cancel, window: &mut Window, cx: &mut Context<Self>) {
        match self.live_open() {
            Some(open) if open.in_submenu() => self.close_submenu(cx),
            Some(open) if open.kind.is_some() => self.close_to_bar(window, cx),
            Some(_) => self.close_menu(window, cx),
            None => self.leave_bar(window, cx),
        }
    }

    fn on_left(&mut self, _: &SelectLeft, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.live_open_mut() else {
            self.move_bar_focus(-1, cx);
            return;
        };
        open.focus_visible = true;
        if !open.commands.is_empty() {
            open.step_command(-1);
            cx.notify();
        } else if open.in_submenu() {
            self.close_submenu(cx);
        } else {
            self.switch(-1, window, cx);
        }
    }

    fn on_right(&mut self, _: &SelectRight, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.live_open_mut() else {
            self.move_bar_focus(1, cx);
            return;
        };
        open.focus_visible = true;
        if !open.commands.is_empty() {
            open.step_command(1);
            cx.notify();
            return;
        }
        let submenu_row = open.panel.focused.filter(|ix| {
            !open.in_submenu() && matches!(open.panel.entries[*ix], Entry::Submenu(..))
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
        let p = palette(cx);
        let open = self.open.as_ref().filter(|open| open.kind == Some(kind));
        let active = open.is_some_and(|open| !open.is_closing());
        let popup = open.map(|open| self.popup(open, window, cx));
        let ring = self.bar_focused(window) == Some(kind);

        let trigger = div()
            .id(kind.title())
            .relative()
            .flex()
            .items_center()
            .h_full()
            .px(px(BAR_ITEM_PADDING))
            .rounded(px(field::CONTROL_RADIUS))
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
            .when(ring, |this| {
                this.child(
                    deferred(field::focus_ring(
                        p,
                        BAR_FOCUS_OUTSET,
                        px(field::CONTROL_RADIUS + BAR_FOCUS_OUTSET.left),
                    ))
                    .with_priority(2),
                )
            })
            .when_some(popup, |this, popup| {
                this.child(
                    div()
                        .absolute()
                        .left_0()
                        .top(px(MENU_BAR_HEIGHT))
                        .child(deferred(anchored().child(popup)).with_priority(1)),
                )
            });
        record_bounds(self.trigger_areas.clone(), kind.index(), trigger)
    }

    fn key_tips(&self, window: &Window) -> Vec<keytips::KeyTip> {
        if !self.access {
            return Vec::new();
        }
        let Some(open) = self.live_open() else {
            let areas = self.trigger_areas.get();
            return MenuKind::ALL
                .into_iter()
                .filter_map(|kind| {
                    let key = kind.access_key()?;
                    let anchor = areas[kind.index()];
                    Some(keytips::layout(
                        window,
                        key,
                        anchor,
                        keytips::Placement::Below,
                    ))
                })
                .collect();
        };
        if open.kind.is_none() {
            return Vec::new();
        }
        let areas = self.hit_areas.get();
        let (panel, area, motion) = match open.focused_submenu() {
            Some(submenu) => (&submenu.panel, areas[SUBMENU_AREA], &submenu.motion),
            None => (&open.panel, areas[MENU_AREA], &open.motion),
        };
        if motion.unroll < 1. {
            return Vec::new();
        }
        panel
            .entries
            .iter()
            .enumerate()
            .filter_map(|(ix, entry)| {
                let key = entry.access_key()?;
                let face = Bounds::new(
                    area.origin
                        + point(px(BORDER + ROW_INSET), px(panel.row_top(ix) + ROW_MARGIN_Y)),
                    size(
                        px(panel.width - 2. * (BORDER + ROW_INSET)),
                        px(entry.height() - 2. * ROW_MARGIN_Y),
                    ),
                );
                Some(keytips::layout(window, key, face, keytips::Placement::Left))
            })
            .collect()
    }

    fn sync_key_tips(&self, window: &Window, cx: &mut Context<Self>) {
        let tips = self.key_tips(window);
        let owner = window.window_handle();
        let origin = client_origin(window_hwnd(window));
        let scale = window.scale_factor();
        cx.defer(move |cx| keytips::show(owner, origin, scale, tips, cx));
    }

    fn popup(
        &self,
        open: &OpenMenu,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let p = palette(cx);
        let panel = &open.panel;
        let width = panel.width;
        let has_commands = !open.commands.is_empty();
        let bar = if has_commands {
            COMMAND_ROW + 1. + OVERFLOW_TOP - PADDING_Y
        } else {
            0.
        };
        let height = panel.height() + bar;
        let hidden = panel.height() * CLOSED_RATIO * (1. - open.motion.unroll);
        let (top, offset) = match (has_commands, open.upward) {
            (true, _) => (0., 0.),
            (false, true) => (hidden, hidden),
            (false, false) => (0., -hidden),
        };
        let submenu = open
            .submenu
            .as_ref()
            .map(|submenu| self.submenu_popup(open, submenu, cx));

        let panel = div()
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
                    .when(has_commands, |this| {
                        this.child(Self::command_row(open, window, cx))
                    })
                    .child(Self::panel_rows(open, panel, None, bar, has_commands, cx)),
            ))
            .children(submenu);
        record_bounds(self.hit_areas.clone(), MENU_AREA, panel)
    }

    fn submenu_popup(
        &self,
        open: &OpenMenu,
        submenu: &OpenSubmenu,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let p = palette(cx);
        let sub_width = submenu.panel.width;
        let sub_height = submenu.panel.height();
        let hidden = sub_height * SUBMENU_CLOSED_RATIO * (1. - submenu.motion.unroll);
        let top = open.panel.row_top(submenu.ix) - BORDER - PADDING_Y;
        let rows = Self::panel_rows(open, &submenu.panel, Some(submenu.ix), 0., false, cx);
        let panel = div()
            .id("submenu")
            .absolute()
            .left(px(open.panel.width - SUBMENU_OVERLAP))
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
            ));
        record_bounds(self.hit_areas.clone(), SUBMENU_AREA, panel)
    }

    fn panel_rows(
        open: &OpenMenu,
        panel: &Panel,
        parent: Option<usize>,
        offset: f32,
        fade: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let p = palette(cx);
        let focus_visible = open.focus_visible && (parent.is_some() || !open.in_submenu());
        let expanded = open
            .live_submenu()
            .filter(|_| parent.is_none())
            .map(|submenu| submenu.ix);

        div()
            .absolute()
            .top(px(BORDER + PADDING_Y + offset))
            .left(px(BORDER))
            .w(px(panel.width - 2. * BORDER))
            .children(panel.entries.iter().enumerate().map(|(ix, entry)| {
                let path = match parent {
                    Some(parent) => (parent, Some(ix)),
                    None => (ix, None),
                };
                if matches!(entry, Entry::Separator) {
                    return separator_row(p, ix);
                }
                let ring = focus_visible && panel.focused == Some(ix);
                let face = row_face(
                    p,
                    panel,
                    ix,
                    fade && entry.selectable(),
                    expanded == Some(ix),
                );
                div()
                    .id(("menu-row", ix))
                    .relative()
                    .h(px(entry.height()))
                    .px(px(ROW_INSET))
                    .py(px(ROW_MARGIN_Y))
                    .when(ring, |this| {
                        this.child(field::focus_ring(
                            p,
                            Edges::default(),
                            px(field::CONTROL_RADIUS),
                        ))
                    })
                    .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                        if *hovered {
                            this.hover_row(path, window, cx);
                        }
                    }))
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.activate(path, window, cx)),
                    )
                    .child(face)
            }))
    }
}

fn separator_row(p: Palette, ix: usize) -> Stateful<Div> {
    div()
        .id(("menu-separator", ix))
        .h(px(ROW_SEPARATOR))
        .py(px(1.))
        .child(div().h(px(1.)).bg(rgb(p.menu_separator)))
}

fn row_face(p: Palette, panel: &Panel, ix: usize, fade: bool, expanded: bool) -> AnyElement {
    let entry = &panel.entries[ix];
    let label = panel.labels[ix].clone();
    let checks = panel.checks;
    let shortcut = panel.shortcuts[ix].clone();
    let enabled = entry.selectable();
    let mark = match entry {
        Entry::Toggle { checked, .. } => checked.then_some(CHECK_GLYPH),
        Entry::Command { icon, .. } => *icon,
        _ => None,
    };
    let mark_size = if matches!(entry, Entry::Command { icon: Some(_), .. }) {
        ICON_SIZE
    } else {
        GLYPH_SIZE
    };
    let mark_color = match entry {
        _ if !enabled => Some(p.text_disabled),
        Entry::Toggle { .. } => Some(p.text_secondary),
        _ if fade => None,
        _ => Some(p.text_primary),
    };
    let label_color = if !enabled {
        Some(p.text_disabled)
    } else if fade {
        None
    } else {
        Some(p.text_primary)
    };
    let face = div()
        .id(("menu-row-face", ix))
        .flex()
        .items_center()
        .size_full()
        .px(px(ROW_PADDING))
        .rounded(px(field::CONTROL_RADIUS))
        .group("menu-row")
        .text_color(rgba(p.text_primary))
        .when(checks, |this| {
            this.child(
                div()
                    .flex_none()
                    .w(px(CHECK_COLUMN))
                    .font_family(ICON_FONT)
                    .text_size(px(mark_size))
                    .line_height(px(mark_size))
                    .when_some(mark_color, |this, color| this.text_color(rgba(color)))
                    .when(enabled && matches!(entry, Entry::Toggle { .. }), |this| {
                        this.group_hover("menu-row", move |style| {
                            style.text_color(rgba(p.text_primary))
                        })
                    })
                    .children(mark),
            )
        })
        .child(
            div()
                .flex_1()
                .font_family(TEXT_FONT)
                .text_size(px(LABEL_SIZE))
                .whitespace_nowrap()
                .when_some(label_color, |this, color| this.text_color(rgba(color)))
                .child(label),
        )
        .when_some(shortcut, |this, shortcut| {
            this.child(
                div()
                    .flex_none()
                    .font_family(SMALL_FONT)
                    .text_size(px(SHORTCUT_SIZE))
                    .whitespace_nowrap()
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
                    .font_family(ICON_FONT)
                    .text_size(px(GLYPH_SIZE))
                    .line_height(px(GLYPH_SIZE))
                    .text_color(rgba(p.text_secondary))
                    .child(CHEVRON_GLYPH),
            )
        });
    if fade {
        face.brush_transition(Brushes::subtle(p))
            .active(move |style| style.text_color(rgba(p.text_secondary)))
            .into_any_element()
    } else {
        face.when(expanded, |this| this.bg(rgba(p.subtle_secondary)))
            .when(enabled, |this| {
                this.hover(move |style| style.bg(rgba(p.subtle_secondary)))
                    .active(move |style| style.bg(rgba(p.subtle_tertiary)))
            })
            .into_any_element()
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
        .border_1()
        .border_color(rgba(p.flyout_stroke))
        .rounded(px(field::OVERLAY_RADIUS))
        .shadow(theme_shadow(p.dark, z, 1.))
        .child(
            div()
                .size_full()
                .rounded(px(field::OVERLAY_RADIUS - BORDER))
                .bg(rgb(p.menu_fill)),
        );
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

fn record_bounds(
    areas: Rc<Cell<[Bounds<Pixels>; 3]>>,
    ix: usize,
    child: impl IntoElement,
) -> measure::Measured {
    measure::measured(child, move |bounds, _, _| {
        let mut all = areas.get();
        all[ix] = bounds;
        areas.set(all);
    })
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
        let this = cx.entity().downgrade();
        window.defer(cx, move |window, cx| {
            this.update(cx, |menu_bar, cx| menu_bar.sync_key_tips(window, cx))
                .ok();
        });
        if self
            .open
            .as_mut()
            .is_some_and(|open| open.sample(window, cx))
        {
            self.open = None;
        }
        if self.open.as_ref().is_none_or(|open| open.submenu.is_none()) {
            let mut areas = self.hit_areas.get();
            areas[SUBMENU_AREA] = Bounds::default();
            self.hit_areas.set(areas);
        }
        let flyout = self.open.as_ref().and_then(|open| {
            let anchor = open.anchor?;
            Some(
                deferred(
                    anchored()
                        .position(anchor)
                        .child(self.popup(open, window, cx)),
                )
                .with_priority(1),
            )
        });

        let bar = div()
            .id("menu-bar")
            .track_focus(&self.focus)
            .key_context(CONTEXT)
            .relative()
            .flex()
            .h_full()
            .gap(px(BAR_GAP))
            .font_family(TEXT_FONT)
            .text_size(px(LABEL_SIZE))
            .text_color(rgba(p.text_primary))
            .on_action(cx.listener(Self::on_up))
            .on_action(cx.listener(Self::on_down))
            .on_action(cx.listener(Self::on_confirm))
            .on_action(cx.listener(Self::on_cancel))
            .on_action(cx.listener(Self::on_left))
            .on_action(cx.listener(Self::on_right))
            .children(MenuKind::ALL.map(|kind| self.trigger(kind, window, cx)))
            .children(flyout);
        record_bounds(self.hit_areas.clone(), BAR_AREA, bar)
    }
}
