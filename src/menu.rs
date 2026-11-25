//! The menus: the bar's File, Edit and View menus (with Alt, F10 and access keys), the editor's
//! context menu, and the find bar's options flyout.
//!
//! An open menu is a stack of panels: the menu itself and, while one is open, its submenu.
//! Keyboard focus is a spot in one of them, a row, or a button in the context menu's button row.
//! The window forwards input as it arrives and acts on the outcome: a command to run, where focus
//! should go, whether to repaint.

use std::time::{Duration, Instant};

use crate::command::{AppCommand, Command, shortcut_label};
use crate::editor::EditCommand;
use crate::geom::{Edges, Point, Rect, Size, point, transparent_black};
use crate::gfx::TextSystem;
use crate::platform::{CursorStyle, Key, KeyEvent, MouseButton};
use crate::theme::{
    CONTROL_RADIUS, ICON_FONT, OVERLAY_RADIUS, Palette, SMALL_FONT, TEXT_FONT, theme_shadow,
};
use crate::tooltip::{self, KeyTip};
use crate::types::SharedStr;
use crate::ui::{
    Anchor, AnimKey, AnimStatus, AppFrame, AppNode, AppUi, Curve, Easing, EditorSlot, FindHit,
    Focus, HitId, MenuHit, MenuKind, Transform, Transition, box_, row, text,
};
use crate::widgets::{focus_ring, subtle_brushes};

pub(crate) const MENU_BAR_HEIGHT: f32 = 32.;

const MIN_WIDTH: f32 = 232.;
const FLYOUT_MIN_WIDTH: f32 = 198.;
const CONTEXT_WIDTH: f32 = 342.;

/// What the commands' enablement and the toggles' checks depend on.
#[derive(Copy, Clone, Debug, Default)]
pub(crate) struct MenuState {
    pub(crate) can_undo: bool,
    pub(crate) can_redo: bool,
    pub(crate) has_selection: bool,
    pub(crate) has_text: bool,
    pub(crate) clipboard_has_text: bool,
    pub(crate) status_bar: bool,
    pub(crate) word_wrap: bool,
}

/// Where the window should put focus after a menu action.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum FocusRequest {
    #[default]
    Keep,
    MenuBar,
    RestoreEditor,
    Editor(EditorSlot),
}

/// What the window does after forwarding input: run `command`, move focus, repaint; `handled`
/// means the input was the menu's and goes no further.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct MenuOutcome {
    pub(crate) handled: bool,
    pub(crate) command: Option<Command>,
    pub(crate) focus: FocusRequest,
    pub(crate) dirty: bool,
}

impl MenuOutcome {
    fn handled() -> Self {
        Self {
            handled: true,
            ..Default::default()
        }
    }

    fn dirty(mut self) -> Self {
        self.dirty = true;
        self
    }

    fn focus(mut self, focus: FocusRequest) -> Self {
        self.focus = focus;
        self
    }

    fn command(mut self, command: Option<Command>) -> Self {
        self.command = command;
        self
    }
}

impl MenuKind {
    pub(crate) const ALL: [Self; 3] = [Self::File, Self::Edit, Self::View];

    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::File => "File",
            Self::Edit => "Edit",
            Self::View => "View",
        }
    }

    /// The key that opens the menu (with Alt, or while key tips show): its first letter.
    fn access_key(self) -> char {
        self.title().chars().next().unwrap_or_default()
    }

    fn cycle(self, delta: isize) -> Self {
        cycle(Some(self as usize), Self::ALL.len(), delta, true, |_| true)
            .map_or(self, |ix| Self::ALL[ix])
    }

    fn entries(self, state: MenuState) -> Vec<Entry> {
        use Entry as E;
        match self {
            Self::File => vec![
                E::command("&New tab", AppCommand::NewTab),
                E::command("New &window", AppCommand::NewWindow),
                E::command("&Open", AppCommand::Open),
                E::command("&Save", AppCommand::Save),
                E::command("Save &as", AppCommand::SaveAs),
                E::command("Save a&ll", AppCommand::SaveAll),
                Entry::Separator,
                E::command("&Close tab", AppCommand::CloseTab),
                E::command("Clos&e window", AppCommand::CloseWindow),
                E::command("E&xit", AppCommand::Exit),
            ],
            Self::Edit => vec![
                E::command_if("&Undo", state.can_undo, EditCommand::Undo),
                E::command_if("Re&do", state.can_redo, EditCommand::Redo),
                Entry::Separator,
                E::command_if("Cu&t", state.has_selection, EditCommand::Cut),
                E::command_if("&Copy", state.has_selection, EditCommand::Copy),
                E::command_if("&Paste", state.clipboard_has_text, EditCommand::Paste),
                E::command_if("De&lete", state.has_selection, EditCommand::Delete),
                Entry::Separator,
                E::command_if("&Find", state.has_text, AppCommand::Find),
                E::command_if("Find &next", state.has_text, AppCommand::FindNext),
                E::command_if("Find pre&vious", state.has_text, AppCommand::FindPrevious),
                E::command_if("&Replace", state.has_text, AppCommand::Replace),
                E::command("&Go to", AppCommand::GoTo),
                Entry::Separator,
                E::command("Select &all", EditCommand::SelectAll),
                E::command("Time/&Date", AppCommand::TimeDate),
            ],
            Self::View => vec![
                Entry::Submenu {
                    label: "&Zoom",
                    entries: vec![
                        E::command("Zoom &in", AppCommand::ZoomIn),
                        E::command("Zoom &out", AppCommand::ZoomOut),
                        E::command("&Restore default zoom", AppCommand::ResetZoom),
                    ],
                },
                Entry::toggle("&Status bar", state.status_bar, AppCommand::ToggleStatusBar),
                Entry::toggle("&Word wrap", state.word_wrap, AppCommand::ToggleWordWrap),
            ],
        }
    }
}

/// The context menu: a row of edit buttons, then Undo.
fn context_entries(state: MenuState) -> Vec<Entry> {
    let button = |label, glyph, enabled, command: EditCommand| Button {
        label,
        glyph,
        enabled,
        command: command.into(),
    };
    vec![
        Entry::Buttons(vec![
            button("Cut", "\u{E8C6}", state.has_selection, EditCommand::Cut),
            button("Copy", "\u{E8C8}", state.has_selection, EditCommand::Copy),
            button(
                "Paste",
                "\u{E77F}",
                state.clipboard_has_text,
                EditCommand::Paste,
            ),
            button("Select all", "\u{E8B3}", true, EditCommand::SelectAll),
            button(
                "Delete",
                "\u{E74D}",
                state.has_selection,
                EditCommand::Delete,
            ),
        ]),
        Entry::Command {
            label: "Undo",
            enabled: state.can_undo,
            command: EditCommand::Undo.into(),
            icon: Some("\u{E7A7}"),
        },
    ]
}

/// A menu row. Labels mark their access key with `&`.
#[derive(Clone)]
enum Entry {
    Command {
        label: &'static str,
        enabled: bool,
        command: Command,
        icon: Option<&'static str>,
    },
    Toggle {
        label: &'static str,
        checked: bool,
        command: Command,
    },
    Submenu {
        label: &'static str,
        entries: Vec<Entry>,
    },
    Separator,
    /// A row of icon buttons (the context menu's edit commands).
    Buttons(Vec<Button>),
}

#[derive(Clone, Copy)]
struct Button {
    label: &'static str,
    glyph: &'static str,
    enabled: bool,
    command: Command,
}

impl Entry {
    fn command(label: &'static str, command: impl Into<Command>) -> Self {
        Self::command_if(label, true, command)
    }

    fn command_if(label: &'static str, enabled: bool, command: impl Into<Command>) -> Self {
        Self::Command {
            label,
            enabled,
            command: command.into(),
            icon: None,
        }
    }

    fn toggle(label: &'static str, checked: bool, command: impl Into<Command>) -> Self {
        Self::Toggle {
            label,
            checked,
            command: command.into(),
        }
    }

    fn height(&self) -> f32 {
        match self {
            Self::Command { icon: Some(_), .. } => ROW_ICON,
            Self::Command { icon: None, .. } => ROW_COMMAND,
            Self::Toggle { .. } | Self::Submenu { .. } => ROW_COMPACT,
            Self::Separator => ROW_SEPARATOR,
            Self::Buttons(_) => COMMAND_ROW,
        }
    }

    fn selectable(&self) -> bool {
        match self {
            Self::Command { enabled, .. } => *enabled,
            Self::Toggle { .. } | Self::Submenu { .. } => true,
            Self::Separator | Self::Buttons(_) => false,
        }
    }

    fn label(&self) -> Option<&'static str> {
        match self {
            Self::Command { label, .. }
            | Self::Toggle { label, .. }
            | Self::Submenu { label, .. } => Some(label),
            Self::Separator | Self::Buttons(_) => None,
        }
    }

    fn access_key(&self) -> Option<char> {
        let (_, key) = self
            .label()
            .filter(|_| self.selectable())?
            .split_once('&')?;
        key.chars().next().map(|c| c.to_ascii_uppercase())
    }

    /// The row's command (enabled or not).
    fn command_id(&self) -> Option<Command> {
        match self {
            Self::Command { command, .. } | Self::Toggle { command, .. } => Some(*command),
            _ => None,
        }
    }

    /// The command the row (or button `col` of a button row) runs, if it's enabled.
    fn command_at(&self, col: usize) -> Option<Command> {
        match self {
            Self::Buttons(buttons) => buttons.get(col).filter(|b| b.enabled).map(|b| b.command),
            entry if entry.selectable() => entry.command_id(),
            _ => None,
        }
    }
}

fn display_label(label: &str) -> SharedStr {
    label.replacen('&', "", 1).into()
}

/// The next of `count` items after `current` (or from the end, with none) in `delta`'s direction
/// that `accept`s, wrapping around or not.
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

/// Where keyboard focus is in an open menu: a row of the menu (level 0) or of its submenu
/// (level 1), and the button within a button row.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct Spot {
    level: usize,
    row: usize,
    col: usize,
}

impl Spot {
    const fn new(level: usize, row: usize, col: usize) -> Self {
        Self { level, row, col }
    }
}

/// One panel of an open menu.
struct Panel {
    entries: Vec<Entry>,
    min_width: f32,
    shortcuts: bool,
    motion: PopupTransition,
    /// For a submenu, the row of the menu that opened it.
    parent_row: usize,
}

impl Panel {
    fn new(
        entries: Vec<Entry>,
        min_width: f32,
        shortcuts: bool,
        fade_in: bool,
        parent_row: usize,
    ) -> Self {
        Self {
            entries,
            min_width,
            shortcuts,
            motion: PopupTransition::new(fade_in),
            parent_row,
        }
    }

    fn serial(&self) -> usize {
        self.motion.transition.serial()
    }

    /// The places focus can go, in order: selectable rows, and each enabled button of a button row.
    fn cells(&self) -> Vec<(usize, usize)> {
        self.entries
            .iter()
            .enumerate()
            .flat_map(|(row, entry)| match entry {
                Entry::Buttons(buttons) => (0..buttons.len())
                    .filter(|&col| buttons[col].enabled)
                    .map(|col| (row, col))
                    .collect(),
                entry if entry.selectable() => vec![(row, 0)],
                _ => Vec::new(),
            })
            .collect()
    }

    /// The first place focus can go, at `level`.
    fn first(&self, level: usize) -> Option<Spot> {
        self.cells()
            .first()
            .map(|&(row, col)| Spot::new(level, row, col))
    }

    /// The hit of row `ix` (button `col` of a button row).
    fn hit(&self, ix: usize, col: usize) -> HitId {
        HitId::Menu(MenuHit::Row {
            serial: self.serial(),
            ix,
            col,
        })
    }

    fn find_key(&self, key: char) -> Option<usize> {
        self.entries
            .iter()
            .position(|entry| entry.access_key() == Some(key))
    }

    fn checks(&self) -> bool {
        self.entries.iter().any(|e| {
            matches!(
                e,
                Entry::Toggle { .. } | Entry::Command { icon: Some(_), .. }
            )
        })
    }
}

/// Opens, fades and closes a popup.
struct PopupTransition {
    transition: Transition,
    fade_in: bool,
    unroll: f32,
    opacity: f32,
}

impl PopupTransition {
    fn new(fade_in: bool) -> Self {
        Self {
            transition: Transition::open_new(),
            fade_in,
            unroll: 0.,
            opacity: 1.,
        }
    }

    /// Advances the animations; whether a closing popup has finished fading out.
    fn sample(&mut self, ui: &mut AppUi) -> bool {
        let serial = self.transition.serial();
        let closing = self.transition.is_closing();
        if !closing {
            self.unroll = ui
                .progress(AnimKey::Transition { serial, track: 0 }, true, OPEN_CURVE)
                .value;
        }
        let appear = if self.fade_in {
            ui.progress(AnimKey::Transition { serial, track: 1 }, true, FADE_CURVE)
                .value
        } else {
            1.
        };
        let fade = ui.progress(
            AnimKey::Transition { serial, track: 2 },
            closing,
            FADE_CURVE,
        );
        self.opacity = (1. - fade.value) * appear;
        closing && fade.status == AnimStatus::Finished
    }
}

enum PopupKind {
    Bar(MenuKind),
    Context {
        anchor: Point,
    },
    Flyout {
        target: HitId,
        return_slot: EditorSlot,
    },
}

struct OpenMenu {
    kind: PopupKind,
    /// The menu's panel, then its submenu while one is open.
    panels: Vec<Panel>,
    /// A submenu fading out.
    leaving: Option<Panel>,
    focus: Option<Spot>,
}

impl OpenMenu {
    fn new(kind: PopupKind, entries: Vec<Entry>, min_width: f32, shortcuts: bool) -> Self {
        let fade_in = matches!(kind, PopupKind::Context { .. });
        Self {
            kind,
            panels: vec![Panel::new(entries, min_width, shortcuts, fade_in, 0)],
            leaving: None,
            focus: None,
        }
    }

    fn bar_kind(&self) -> Option<MenuKind> {
        match self.kind {
            PopupKind::Bar(kind) => Some(kind),
            PopupKind::Context { .. } | PopupKind::Flyout { .. } => None,
        }
    }

    fn is_context(&self) -> bool {
        matches!(self.kind, PopupKind::Context { .. })
    }

    fn root(&self) -> &Panel {
        &self.panels[0]
    }

    fn submenu(&self) -> Option<&Panel> {
        self.panels.get(1)
    }

    fn entry(&self, spot: Spot) -> Option<&Entry> {
        self.panels.get(spot.level)?.entries.get(spot.row)
    }

    /// The spot a row's hit is for.
    fn spot_of(&self, serial: usize, row: usize, col: usize) -> Option<Spot> {
        let level = self.panels.iter().position(|p| p.serial() == serial)?;
        Some(Spot::new(level, row, col))
    }

    fn in_submenu(&self) -> bool {
        self.focus.is_some_and(|f| f.level == 1) && self.submenu().is_some()
    }

    fn open_submenu(&mut self, row: usize, focus_first: bool) -> bool {
        let Some(Entry::Submenu { entries, .. }) = self.root().entries.get(row) else {
            return false;
        };
        if self.submenu().is_none_or(|sub| sub.parent_row != row) {
            let panel = Panel::new(entries.clone(), MIN_WIDTH, true, false, row);
            self.panels.truncate(1);
            self.panels.push(panel);
            self.leaving = None;
        }
        let first = self.panels[1].first(1);
        let parent = Spot::new(0, row, 0);
        self.focus = Some(if focus_first {
            first.unwrap_or(parent)
        } else {
            parent
        });
        true
    }

    fn close_submenu(&mut self) -> bool {
        if self.panels.len() < 2 {
            return false;
        }
        let mut sub = self.panels.pop().expect("a submenu is open");
        if self.focus.is_some_and(|f| f.level == 1) {
            self.focus = Some(Spot::new(0, sub.parent_row, 0));
        }
        sub.motion.transition.close();
        self.leaving = Some(sub);
        true
    }

    /// Moves focus up or down through the focused panel's rows and buttons, wrapping around.
    fn step(&mut self, delta: isize) {
        let level = if self.in_submenu() { 1 } else { 0 };
        let cells = self.panels[level].cells();
        let current = self
            .focus
            .filter(|f| f.level == level)
            .and_then(|f| cells.iter().position(|&c| c == (f.row, f.col)));
        if let Some(next) = cycle(current, cells.len(), delta, true, |_| true) {
            let (row, col) = cells[next];
            self.focus = Some(Spot::new(level, row, col));
        }
    }

    /// Moves focus along a button row (not wrapping).
    fn step_button(&mut self, delta: isize) {
        let Some(spot) = self.focus else {
            return;
        };
        if let Some(Entry::Buttons(buttons)) = self.entry(spot)
            && let Some(col) = cycle(Some(spot.col), buttons.len(), delta, false, |c| {
                buttons[c].enabled
            })
        {
            self.focus = Some(Spot { col, ..spot });
        }
    }

    fn focus_edge(&mut self, last: bool) {
        let level = if self.in_submenu() { 1 } else { 0 };
        let cells = self.panels[level].cells();
        let edge = if last { cells.last() } else { cells.first() };
        let spot = edge.map(|&(row, col)| Spot::new(level, row, col));
        if level == 0 || spot.is_some() {
            self.focus = spot;
        }
    }

    /// Advances the animations; whether the menu has finished closing.
    fn sample(&mut self, ui: &mut AppUi) -> bool {
        for panel in &mut self.panels[1..] {
            panel.motion.sample(ui);
        }
        if self
            .leaving
            .as_mut()
            .is_some_and(|sub| sub.motion.sample(ui))
        {
            self.leaving = None;
        }
        self.panels[0].motion.sample(ui)
    }
}

#[derive(Default)]
enum Mode {
    #[default]
    Idle,
    /// The bar has keyboard focus (after Alt or F10), on `cursor`.
    Bar {
        cursor: MenuKind,
        tips: bool,
    },
    Open {
        menu: Box<OpenMenu>,
        tips: bool,
    },
}

#[derive(Default)]
pub(crate) struct Menus {
    mode: Mode,
    /// A menu fading out after closing.
    leaving: Option<Box<OpenMenu>>,
    /// Alt went down alone: if it comes up alone, the bar takes focus.
    alt_armed: bool,
    /// The spot under the pointer (and, in a submenu, the row that opened it).
    hovered: Option<(Spot, usize)>,
    /// When hovering opens (or, with `None`, closes) a submenu.
    pub(crate) submenu_at: Option<(Instant, Option<usize>)>,
}

impl Menus {
    pub(crate) fn is_open(&self) -> bool {
        matches!(self.mode, Mode::Open { .. })
    }

    /// Whether key tips show (and access keys work without Alt).
    pub(crate) fn access_active(&self, focus: Focus) -> bool {
        focus == Focus::MenuBar
            && matches!(
                self.mode,
                Mode::Bar { tips: true, .. } | Mode::Open { tips: true, .. }
            )
    }

    pub(crate) fn open_kind(&self) -> Option<MenuKind> {
        match &self.mode {
            Mode::Open { menu, .. } => menu.bar_kind(),
            Mode::Idle | Mode::Bar { .. } => None,
        }
    }

    fn open_menu(&mut self) -> Option<&mut OpenMenu> {
        match &mut self.mode {
            Mode::Open { menu, .. } => Some(menu),
            Mode::Idle | Mode::Bar { .. } => None,
        }
    }

    /// The menu on screen: the open one, or one fading out.
    fn shown(&self) -> Option<&OpenMenu> {
        match &self.mode {
            Mode::Open { menu, .. } => Some(menu),
            Mode::Idle | Mode::Bar { .. } => self.leaving.as_deref(),
        }
    }

    /// The menu title the bar's keyboard focus is on.
    fn bar_focused(&self, focus: Focus) -> Option<MenuKind> {
        if focus != Focus::MenuBar {
            return None;
        }
        match &self.mode {
            Mode::Bar { cursor, .. } => Some(*cursor),
            Mode::Idle => Some(MenuKind::File),
            Mode::Open { .. } => None,
        }
    }

    fn set_tips(&mut self, next: bool) {
        match &mut self.mode {
            Mode::Bar { tips, .. } | Mode::Open { tips, .. } => *tips = next,
            Mode::Idle => {}
        }
    }

    // ---- Opening and closing

    #[cfg(test)]
    pub(crate) fn open_kind_menu(&mut self, kind: MenuKind, state: MenuState) -> MenuOutcome {
        self.open_bar_menu(kind, state, false, false)
    }

    /// Opens a bar menu; from the keyboard, with its first row focused.
    fn open_bar_menu(
        &mut self,
        kind: MenuKind,
        state: MenuState,
        keyboard: bool,
        tips: bool,
    ) -> MenuOutcome {
        let mut menu = OpenMenu::new(PopupKind::Bar(kind), kind.entries(state), MIN_WIDTH, true);
        if keyboard {
            menu.focus = menu.root().first(0);
        }
        self.show(menu, tips)
    }

    pub(crate) fn open_context_menu(&mut self, anchor: Point, state: MenuState) -> MenuOutcome {
        let mut menu = OpenMenu::new(
            PopupKind::Context { anchor },
            context_entries(state),
            CONTEXT_WIDTH,
            false,
        );
        // The first enabled button (Select all always is).
        menu.focus = menu.root().first(0);
        self.show(menu, false)
    }

    pub(crate) fn open_find_options(&mut self, match_case: bool, wrap_around: bool) -> MenuOutcome {
        let entries = vec![
            Entry::toggle("Match case", match_case, AppCommand::ToggleMatchCase),
            Entry::toggle("Wrap around", wrap_around, AppCommand::ToggleWrapAround),
        ];
        let kind = PopupKind::Flyout {
            target: HitId::Find(FindHit::Options),
            return_slot: EditorSlot::FindQuery,
        };
        self.show(OpenMenu::new(kind, entries, FLYOUT_MIN_WIDTH, true), false)
    }

    fn show(&mut self, menu: OpenMenu, tips: bool) -> MenuOutcome {
        self.hovered = None;
        self.submenu_at = None;
        self.leaving = None;
        self.mode = Mode::Open {
            menu: Box::new(menu),
            tips,
        };
        MenuOutcome::handled().dirty().focus(FocusRequest::MenuBar)
    }

    fn close(&mut self) -> MenuOutcome {
        self.alt_armed = false;
        self.hovered = None;
        self.submenu_at = None;
        let focus = match std::mem::take(&mut self.mode) {
            Mode::Open { mut menu, .. } => {
                let focus = match menu.kind {
                    PopupKind::Flyout { return_slot, .. } => FocusRequest::Editor(return_slot),
                    PopupKind::Bar(_) | PopupKind::Context { .. } => FocusRequest::RestoreEditor,
                };
                menu.panels[0].motion.transition.close();
                self.leaving = Some(menu);
                focus
            }
            Mode::Idle | Mode::Bar { .. } => FocusRequest::RestoreEditor,
        };
        MenuOutcome::handled().dirty().focus(focus)
    }

    /// Escape, or the window losing its place (moved, deactivated): closes any menu and leaves
    /// the bar.
    pub(crate) fn cancel(&mut self, focus: Focus) -> MenuOutcome {
        self.alt_armed = false;
        if self.is_open() {
            return self.close();
        }
        let had_bar = !matches!(std::mem::take(&mut self.mode), Mode::Idle);
        let on_bar = focus == Focus::MenuBar;
        MenuOutcome {
            dirty: had_bar || on_bar,
            focus: if on_bar {
                FocusRequest::RestoreEditor
            } else {
                FocusRequest::Keep
            },
            ..Default::default()
        }
    }

    fn open_submenu(&mut self, row: usize, focus_first: bool) -> bool {
        self.submenu_at = None;
        self.open_menu()
            .is_some_and(|menu| menu.open_submenu(row, focus_first))
    }

    fn close_submenu(&mut self) -> bool {
        self.submenu_at = None;
        self.open_menu().is_some_and(OpenMenu::close_submenu)
    }

    /// Runs the row (or button) at `spot`, or opens the submenu there.
    fn activate(&mut self, spot: Spot, focus_first: bool) -> MenuOutcome {
        let Some(entry) = self.open_menu().and_then(|menu| menu.entry(spot).cloned()) else {
            return MenuOutcome::default();
        };
        match entry {
            Entry::Submenu { .. } if spot.level == 0 => {
                self.open_submenu(spot.row, focus_first);
                MenuOutcome::default().dirty()
            }
            entry => match entry.command_at(spot.col) {
                Some(command) => self.close().command(Some(command)),
                None => MenuOutcome::default(),
            },
        }
    }

    // ---- The pointer

    /// A press anywhere in the window: it ends keyboard use of the bar, closes a popup it's
    /// outside of, and toggles the menu whose title it's on. Handled if the press was the menu's.
    pub(crate) fn on_mouse_down(
        &mut self,
        chain: &[HitId],
        button: MouseButton,
        focus: Focus,
        state: MenuState,
    ) -> MenuOutcome {
        self.alt_armed = false;
        let mut out = match &mut self.mode {
            Mode::Open { tips, .. } => MenuOutcome {
                dirty: std::mem::replace(tips, false),
                ..Default::default()
            },
            Mode::Bar { .. } | Mode::Idle => self.cancel(focus),
        };
        let in_menu = chain
            .iter()
            .any(|h| matches!(h, HitId::Menu(MenuHit::Panel | MenuHit::Trigger(_))));
        if self.is_open() && !in_menu {
            let closed = self.close();
            out = MenuOutcome {
                dirty: true,
                focus: closed.focus,
                ..out
            };
        }
        let taken = match (button, chain.first()) {
            (MouseButton::Left, Some(&HitId::Menu(MenuHit::Trigger(kind)))) => {
                let toggled = if self.open_kind() == Some(kind) {
                    self.close()
                } else {
                    self.open_bar_menu(kind, state, false, false)
                };
                out = MenuOutcome {
                    dirty: true,
                    focus: toggled.focus,
                    ..out
                };
                true
            }
            (MouseButton::Left, Some(HitId::Menu(hit))) => hit.is_popup(),
            (MouseButton::Left, _) => false,
            (MouseButton::Right, _) => in_menu,
            (MouseButton::Middle, _) => false,
        };
        MenuOutcome {
            handled: taken,
            ..out
        }
    }

    /// A click (press and release) on `hit`.
    pub(crate) fn on_click(&mut self, hit: MenuHit) -> MenuOutcome {
        let MenuHit::Row { serial, ix, col } = hit else {
            return MenuOutcome::default();
        };
        match self
            .open_menu()
            .and_then(|menu| menu.spot_of(serial, ix, col))
        {
            Some(spot) => self.activate(spot, false),
            None => MenuOutcome::default(),
        }
    }

    /// The pointer moved onto `top`: rows under it take focus, submenus open (or close) after a
    /// delay, and a different bar menu's title opens it.
    pub(crate) fn on_hover_change(
        &mut self,
        top: Option<HitId>,
        state: impl FnOnce() -> MenuState,
        ui: &mut AppUi,
        show_delay: Duration,
    ) -> MenuOutcome {
        if let Some(HitId::Menu(MenuHit::Trigger(kind))) = top
            && self.open_kind().is_some_and(|open| open != kind)
        {
            ui.last_input_was_keyboard = false;
            return self.open_bar_menu(kind, state(), false, false);
        }
        let Mode::Open { menu, .. } = &mut self.mode else {
            self.hovered = None;
            return MenuOutcome::default();
        };
        let spot = match top {
            Some(HitId::Menu(MenuHit::Row { serial, ix, col })) => menu.spot_of(serial, ix, col),
            _ => None,
        };
        let hovered = spot.map(|s| (s, menu.panels[s.level].parent_row));
        if hovered == self.hovered {
            return MenuOutcome::default();
        }
        let Some(spot) = spot else {
            self.hovered = None;
            self.submenu_at = None;
            return MenuOutcome::default();
        };
        let entry = menu.entry(spot);
        let open_sub = menu.submenu().map(|s| s.parent_row);
        let pending = match entry {
            Some(Entry::Buttons(_)) => None,
            _ if spot.level == 1 => None,
            Some(Entry::Submenu { .. }) if open_sub == Some(spot.row) => None,
            Some(Entry::Submenu { .. }) => Some(Some(spot.row)),
            _ => open_sub.map(|_| None),
        };
        if entry.is_some_and(|e| e.selectable() || e.command_at(spot.col).is_some()) {
            menu.focus = Some(spot);
        }
        self.hovered = hovered;
        ui.last_input_was_keyboard = false;
        self.submenu_at = pending.map(|target| (Instant::now() + show_delay, target));
        MenuOutcome::default().dirty()
    }

    /// Opens or closes the hovered submenu once its delay is up; whether anything changed.
    pub(crate) fn tick(&mut self, now: Instant) -> bool {
        let Some((_, target)) = self.submenu_at.filter(|&(at, _)| at <= now) else {
            return false;
        };
        self.submenu_at = None;
        match target {
            Some(row) => self.open_submenu(row, false),
            None => self.close_submenu(),
        }
    }

    // ---- The keyboard

    pub(crate) fn on_key_up(&mut self, ev: KeyEvent, focus: Focus) -> MenuOutcome {
        if ev.key == Key::Alt && std::mem::take(&mut self.alt_armed) {
            return self.toggle_access(focus);
        }
        MenuOutcome::default()
    }

    /// A key press. Alt+F4 and Alt+Space go on to Windows; Alt alone and F10 enter or leave the
    /// bar; access keys open menus and run rows (with Alt, or while key tips show); the arrows,
    /// Home, End, Enter, Space and Escape navigate.
    pub(crate) fn on_key_down(
        &mut self,
        ev: KeyEvent,
        focus: Focus,
        state: MenuState,
    ) -> MenuOutcome {
        let m = ev.modifiers;
        let plain_alt = m.alt && !m.ctrl && !m.win;
        if plain_alt && matches!(ev.key, Key::F(4) | Key::Space) {
            return MenuOutcome {
                handled: false,
                ..self.cancel(focus)
            };
        }
        if ev.key == Key::Alt && m.none() {
            if self.is_open() {
                return self.close();
            }
            self.alt_armed = true;
            return MenuOutcome::default();
        }
        self.alt_armed = false;
        if ev.key == Key::F(10) && m.none() {
            return if self.is_open() {
                self.close()
            } else {
                self.toggle_access(focus)
            };
        }
        let access = match ev.key {
            Key::Char(c) if c.is_ascii_alphanumeric() => Some(c.to_ascii_uppercase()),
            _ => None,
        };

        if !self.access_active(focus) {
            if let Some(key) = access.filter(|_| plain_alt && !m.alt_gr) {
                let out = self.invoke_access(key, true, state);
                if out.handled {
                    return out;
                }
            }
            if self.is_open() {
                if let Some(key) = access.filter(|_| m.none()) {
                    let out = self.invoke_access(key, false, state);
                    if out.handled {
                        return out;
                    }
                }
                let nav = self.navigate(ev, focus, state);
                return if nav.handled {
                    nav
                } else {
                    MenuOutcome::handled()
                };
            }
            if focus == Focus::MenuBar {
                return self.navigate(ev, focus, state);
            }
            return MenuOutcome::default();
        }

        // Key tips show: access keys work without Alt, and other keys are kept from the editor.
        match ev.key {
            Key::Escape if m.none() => self.back_out(),
            Key::Up
            | Key::Down
            | Key::Left
            | Key::Right
            | Key::Home
            | Key::End
            | Key::Tab
            | Key::Space
            | Key::Enter => {
                self.set_tips(false);
                self.navigate(ev, focus, state).dirty()
            }
            _ => match access.filter(|_| !m.ctrl && !m.win && !m.alt_gr) {
                Some(key) => {
                    let out = self.invoke_access(key, true, state);
                    if out.handled || !m.alt {
                        MenuOutcome {
                            handled: true,
                            ..out
                        }
                    } else {
                        MenuOutcome::default()
                    }
                }
                None if m.alt => MenuOutcome::default(),
                None => MenuOutcome::handled(),
            },
        }
    }

    /// Alt or F10: shows or hides key tips, entering or leaving the bar.
    fn toggle_access(&mut self, focus: Focus) -> MenuOutcome {
        if self.access_active(focus) {
            return match &mut self.mode {
                Mode::Open { tips, .. } => {
                    *tips = false;
                    MenuOutcome::handled().dirty()
                }
                Mode::Bar { .. } | Mode::Idle => {
                    self.mode = Mode::Idle;
                    MenuOutcome::handled()
                        .dirty()
                        .focus(FocusRequest::RestoreEditor)
                }
            };
        }
        match &mut self.mode {
            Mode::Open { menu, .. } if menu.bar_kind().is_none() => MenuOutcome::handled(),
            Mode::Open { tips, .. } => {
                *tips = true;
                MenuOutcome::handled().dirty()
            }
            Mode::Bar { cursor, tips } => {
                if focus != Focus::MenuBar {
                    *cursor = MenuKind::File;
                }
                *tips = true;
                MenuOutcome::handled().dirty().focus(FocusRequest::MenuBar)
            }
            Mode::Idle => {
                self.mode = Mode::Bar {
                    cursor: MenuKind::File,
                    tips: true,
                };
                MenuOutcome::handled().dirty().focus(FocusRequest::MenuBar)
            }
        }
    }

    /// An access key: opens the bar menu it names, or runs (or opens) the row it marks.
    fn invoke_access(&mut self, key: char, show_tips: bool, state: MenuState) -> MenuOutcome {
        let Mode::Open { menu, .. } = &mut self.mode else {
            return match MenuKind::ALL.into_iter().find(|k| k.access_key() == key) {
                Some(kind) => self.open_bar_menu(kind, state, true, show_tips),
                None => MenuOutcome::default(),
            };
        };
        if menu.bar_kind().is_none() {
            return MenuOutcome::default();
        }
        let level = menu.panels.len() - 1;
        let Some(row) = menu.panels[level].find_key(key) else {
            return MenuOutcome::default();
        };
        let opens_submenu =
            level == 0 && matches!(menu.panels[0].entries[row], Entry::Submenu { .. });
        if opens_submenu {
            self.set_tips(show_tips);
            self.open_submenu(row, true);
            MenuOutcome::handled().dirty()
        } else {
            self.set_tips(false);
            self.activate(Spot::new(level, row, 0), true)
        }
    }

    fn navigate(&mut self, ev: KeyEvent, focus: Focus, state: MenuState) -> MenuOutcome {
        if !ev.modifiers.none() {
            return MenuOutcome::default();
        }
        match ev.key {
            Key::Up | Key::Down => {
                let delta = if ev.key == Key::Down { 1 } else { -1 };
                match self.open_menu() {
                    Some(menu) => {
                        menu.step(delta);
                        MenuOutcome::handled().dirty()
                    }
                    None => match self.bar_focused(focus).filter(|_| delta > 0) {
                        Some(kind) => self.open_bar_menu(kind, state, true, false),
                        None => MenuOutcome::handled(),
                    },
                }
            }
            Key::Home | Key::End => match self.open_menu() {
                Some(menu) => {
                    menu.focus_edge(ev.key == Key::End);
                    MenuOutcome::handled().dirty()
                }
                None => MenuOutcome::handled(),
            },
            Key::Enter | Key::Space => match self.open_menu().map(|menu| menu.focus) {
                Some(Some(spot)) => self.activate(spot, true),
                Some(None) => MenuOutcome::handled(),
                None => match self.bar_focused(focus) {
                    Some(kind) => self.open_bar_menu(kind, state, true, false),
                    None => MenuOutcome::handled(),
                },
            },
            Key::Escape => self.back_out(),
            Key::Left | Key::Right => {
                self.sideways(if ev.key == Key::Right { 1 } else { -1 }, state)
            }
            _ => MenuOutcome::default(),
        }
    }

    /// Escape: closes the submenu, or the menu (back to the bar it came from), or leaves the bar.
    fn back_out(&mut self) -> MenuOutcome {
        match &self.mode {
            Mode::Open { menu, .. } if menu.submenu().is_some() => {
                self.close_submenu();
                MenuOutcome::handled().dirty()
            }
            Mode::Open { menu, .. } if menu.bar_kind().is_some() => {
                let Mode::Open { mut menu, tips } = std::mem::take(&mut self.mode) else {
                    unreachable!()
                };
                let cursor = menu.bar_kind().expect("a bar menu");
                menu.panels[0].motion.transition.close();
                self.leaving = Some(menu);
                self.hovered = None;
                self.submenu_at = None;
                self.mode = Mode::Bar { cursor, tips };
                MenuOutcome::handled().dirty().focus(FocusRequest::MenuBar)
            }
            Mode::Open { .. } => self.close(),
            Mode::Bar { .. } | Mode::Idle => {
                self.mode = Mode::Idle;
                MenuOutcome::handled()
                    .dirty()
                    .focus(FocusRequest::RestoreEditor)
            }
        }
    }

    /// Left and Right: along the button row, out of or into a submenu, or to the next bar menu.
    fn sideways(&mut self, delta: isize, state: MenuState) -> MenuOutcome {
        let Mode::Open { menu, .. } = &mut self.mode else {
            match &mut self.mode {
                Mode::Bar { cursor, .. } => *cursor = cursor.cycle(delta),
                _ => {
                    self.mode = Mode::Bar {
                        cursor: MenuKind::File.cycle(delta),
                        tips: false,
                    };
                }
            }
            return MenuOutcome::handled().dirty();
        };
        if menu.is_context() {
            menu.step_button(delta);
            return MenuOutcome::handled().dirty();
        }
        if delta < 0 && menu.in_submenu() {
            self.close_submenu();
            return MenuOutcome::handled().dirty();
        }
        let submenu_row = menu
            .focus
            .filter(|f| delta > 0 && f.level == 0)
            .filter(|f| matches!(menu.root().entries.get(f.row), Some(Entry::Submenu { .. })));
        if let Some(spot) = submenu_row {
            self.open_submenu(spot.row, true);
            return MenuOutcome::handled().dirty();
        }
        match menu.bar_kind() {
            Some(kind) => self.open_bar_menu(kind.cycle(delta), state, true, false),
            None => MenuOutcome::handled(),
        }
    }

    pub(crate) fn sample(&mut self, ui: &mut AppUi) {
        if let Some(menu) = self.open_menu() {
            menu.sample(ui);
        }
        if self.leaving.as_mut().is_some_and(|menu| menu.sample(ui)) {
            self.leaving = None;
        }
    }

    #[cfg(test)]
    pub(crate) fn hit_for_command(&self, cmd: Command) -> Option<HitId> {
        let Mode::Open { menu, .. } = &self.mode else {
            return None;
        };
        menu.panels.iter().rev().find_map(|panel| {
            panel
                .entries
                .iter()
                .enumerate()
                .find_map(|(ix, entry)| match entry {
                    Entry::Buttons(buttons) => {
                        let col = buttons.iter().position(|b| b.command == cmd)?;
                        Some(panel.hit(ix, col))
                    }
                    entry => (entry.command_id() == Some(cmd)).then(|| panel.hit(ix, 0)),
                })
        })
    }

    #[cfg(test)]
    pub(crate) fn hit_for_submenu(&self, title: &str) -> Option<HitId> {
        let Mode::Open { menu, .. } = &self.mode else {
            return None;
        };
        let root = menu.root();
        let is_titled = |e: &Entry| matches!(e, Entry::Submenu { label, .. } if display_label(label).as_ref() == title);
        root.entries
            .iter()
            .position(is_titled)
            .map(|ix| root.hit(ix, 0))
    }
}

// ---- Drawing

const BORDER: f32 = 1.;
const PADDING_Y: f32 = 2.;
const ROW_INSET: f32 = 4.;
const ROW_PADDING: f32 = 11.;
const ROW_ICON: f32 = 40.;
const ROW_COMMAND: f32 = 33.;
const ROW_COMPACT: f32 = 32.;
const ROW_SEPARATOR: f32 = 3.;
const ROW_MARGIN_Y: f32 = 2.;
const CHECK_COLUMN: f32 = 28.;
const SHORTCUT_GAP: f32 = 24.;
const CHEVRON_WIDTH: f32 = 12.;
const LABEL_SIZE: f32 = 14.;
const SHORTCUT_SIZE: f32 = 12.;
const ICON_SIZE: f32 = 16.;
const GLYPH_SIZE: f32 = 12.;
const CHECK_GLYPH: &str = "\u{E73E}";
const CHEVRON_GLYPH: &str = "\u{E76C}";

const COMMAND_ROW: f32 = 59.;
const COMMAND_BUTTON: f32 = 58.;
const COMMAND_GAP: f32 = 1.;
const COMMAND_ICON_TOP: f32 = 9.;
const COMMAND_LABEL_GAP: f32 = 4.;
const COMMAND_LINE: f32 = 16.;
const COMMAND_LABEL_SIZE: f32 = 12.;
const COMMAND_DIVIDER_TOP: f32 = 14.;
const COMMAND_DIVIDER_HEIGHT: f32 = 32.;
const COMMAND_INSET: f32 = 2.;

const OPEN_CURVE: Curve = Curve::new(Duration::from_millis(250), Easing::DECELERATE);
const FADE_CURVE: Curve = Curve::new(Duration::from_millis(83), Easing::Linear);
const CLOSED_RATIO: f32 = 0.5;
const SUBMENU_CLOSED_RATIO: f32 = 0.67;
const MENU_Z: f32 = 32.;
const SUBMENU_Z: f32 = 40.;
const FLYOUT_GAP: f32 = 3.;
const SUBMENU_OVERLAP: f32 = 4.;

const BAR_ITEM_PADDING: f32 = 10.;
const BAR_GAP: f32 = 8.;
const BAR_FOCUS_OUTSET: Edges<f32> = Edges {
    top: 0.,
    right: 1.,
    bottom: 1.,
    left: 1.,
};

fn place_below(r: Rect, _popup: Size, _vp: Size) -> Point {
    point(r.left(), r.bottom())
}

/// Above the target if there's room below the menu bar, else below it.
fn place_flyout(r: Rect, popup: Size, _vp: Size) -> Point {
    let x = r.center().x - (popup.width / 2.).floor();
    let above = r.top() - (FLYOUT_GAP + popup.height);
    let y = if above >= MENU_BAR_HEIGHT {
        above
    } else {
        r.bottom() + FLYOUT_GAP
    };
    point(x, y)
}

/// Beside the row that opened it: to the right if it fits, else to the left.
fn place_submenu(r: Rect, popup: Size, vp: Size) -> Point {
    let overlap = SUBMENU_OVERLAP - BORDER;
    let right_x = r.right() - overlap;
    let x = if right_x + popup.width <= vp.width {
        right_x
    } else {
        r.left() - popup.width + overlap
    };
    point(x, r.top() - (BORDER + PADDING_Y))
}

impl Menus {
    pub(crate) fn render_bar(&self, focus: Focus, p: Palette, ui: &mut AppUi) -> AppNode {
        let ring_on = self.bar_focused(focus);
        let triggers: Vec<AppNode> = MenuKind::ALL
            .into_iter()
            .map(|kind| {
                let hit = HitId::Menu(MenuHit::Trigger(kind));
                let bg = if self.open_kind() == Some(kind) {
                    p.subtle_tertiary
                } else {
                    ui.brush(hit, subtle_brushes(p), true)
                };
                row()
                    .hit(hit)
                    .cursor(CursorStyle::Arrow)
                    .items_center()
                    .h_full()
                    .px(BAR_ITEM_PADDING)
                    .rounded(CONTROL_RADIUS)
                    .bg(bg)
                    .child(box_().top(1.).child(text(kind.title())))
                    .when(ring_on == Some(kind), |this| {
                        this.child(focus_ring(
                            p,
                            BAR_FOCUS_OUTSET,
                            CONTROL_RADIUS + BAR_FOCUS_OUTSET.left,
                        ))
                    })
            })
            .collect();
        row()
            .hit(HitId::Menu(MenuHit::Bar))
            .flex_none()
            .h_full()
            .gap(BAR_GAP)
            .font_family(TEXT_FONT)
            .text_size(LABEL_SIZE)
            .text_color(p.text_primary)
            .children(triggers)
    }

    pub(crate) fn render_popup(&self, p: Palette, ui: &mut AppUi) -> Option<AppNode> {
        let menu = self.shown()?;
        let root = menu.root();
        let submenu = menu.submenu().or(menu.leaving.as_ref()).map(|sub| {
            let anchor = Anchor::Target {
                target: root.hit(sub.parent_row, 0),
                place: place_submenu,
            };
            render_panel(menu, sub, 1, p, ui)
                .anchored(anchor)
                .opacity(sub.motion.opacity)
                .when(sub.motion.transition.is_closing(), AppNode::inert)
        });
        Some(
            box_()
                .anchored(match &menu.kind {
                    PopupKind::Bar(kind) => Anchor::Target {
                        target: HitId::Menu(MenuHit::Trigger(*kind)),
                        place: place_below,
                    },
                    PopupKind::Context { anchor } => Anchor::Point(*anchor),
                    PopupKind::Flyout { target, .. } => Anchor::Target {
                        target: *target,
                        place: place_flyout,
                    },
                })
                .opacity(root.motion.opacity)
                .child(render_panel(menu, root, 0, p, ui))
                .when_some(submenu, AppNode::child)
                .when(root.motion.transition.is_closing(), AppNode::inert),
        )
    }

    /// The access keys to show over the bar's titles or the open menu's rows.
    pub(crate) fn key_tips(&self, focus: Focus, frame: &AppFrame, ts: &TextSystem) -> Vec<KeyTip> {
        if !self.access_active(focus) {
            return Vec::new();
        }
        let menu = match &self.mode {
            Mode::Open { menu, .. } if menu.bar_kind().is_some() => menu,
            Mode::Bar { .. } => {
                return MenuKind::ALL
                    .into_iter()
                    .filter_map(|kind| {
                        let anchor = frame.rect_of(HitId::Menu(MenuHit::Trigger(kind)))?;
                        Some(tooltip::layout_tip(
                            ts,
                            kind.access_key(),
                            anchor,
                            tooltip::Placement::Below,
                        ))
                    })
                    .collect();
            }
            _ => return Vec::new(),
        };
        let panel = if menu.in_submenu() {
            &menu.panels[1]
        } else {
            menu.root()
        };
        if panel.motion.unroll < 1. {
            return Vec::new();
        }
        panel
            .entries
            .iter()
            .enumerate()
            .filter_map(|(ix, entry)| {
                let key = entry.access_key()?;
                let face = frame.rect_of(panel.hit(ix, 0))?;
                let face = face.inset(Edges::xy(ROW_INSET, ROW_MARGIN_Y));
                Some(tooltip::layout_tip(ts, key, face, tooltip::Placement::Left))
            })
            .collect()
    }
}

/// A panel's card: a menu's (`level` 0) or a submenu's (1).
fn render_panel(
    menu: &OpenMenu,
    panel: &Panel,
    level: usize,
    p: Palette,
    ui: &mut AppUi,
) -> AppNode {
    let context = menu.is_context() && level == 0;
    let (closed_ratio, z) = if level == 0 {
        (CLOSED_RATIO, MENU_Z)
    } else {
        (SUBMENU_CLOSED_RATIO, SUBMENU_Z)
    };
    let buttons = panel
        .entries
        .iter()
        .take_while(|e| matches!(e, Entry::Buttons(_)))
        .count();
    let rows: Vec<AppNode> = (buttons..panel.entries.len())
        .map(|ix| render_row(menu, panel, level, ix, context, p, ui))
        .collect();
    let fill = box_()
        .w_full()
        .rounded(OVERLAY_RADIUS - BORDER)
        .bg(p.menu_fill)
        .children(
            (0..buttons)
                .map(|ix| render_buttons(menu, panel, ix, p, ui))
                .collect::<Vec<_>>(),
        )
        .child(box_().w_full().py(PADDING_Y).children(rows));
    box_()
        .hit(HitId::Menu(MenuHit::Panel))
        .cursor(CursorStyle::Arrow)
        .min_w(panel.min_width)
        .border(BORDER, p.flyout_stroke)
        .rounded(OVERLAY_RADIUS)
        .shadows(theme_shadow(p.dark, z))
        .transform(Transform::RevealY {
            shown_ratio: 1. - closed_ratio * (1. - panel.motion.unroll),
            slide: !context,
        })
        .child(fill)
}

/// The context menu's button row, with a divider under it.
fn render_buttons(
    menu: &OpenMenu,
    panel: &Panel,
    ix: usize,
    p: Palette,
    ui: &mut AppUi,
) -> AppNode {
    let Entry::Buttons(buttons) = &panel.entries[ix] else {
        unreachable!("a button row")
    };
    let mut children = Vec::with_capacity(buttons.len() * 2);
    for (col, button) in buttons.iter().enumerate() {
        if col > 0 {
            children.push(
                box_()
                    .flex_none()
                    .w(1.)
                    .h(COMMAND_DIVIDER_HEIGHT)
                    .mt(COMMAND_DIVIDER_TOP)
                    .bg(p.menu_separator),
            );
        }
        let spot = Spot::new(0, ix, col);
        let focused = ui.last_input_was_keyboard && menu.focus == Some(spot);
        let hit = panel.hit(ix, col);
        let bg = ui.brush(hit, subtle_brushes(p), button.enabled);
        let fg = if !button.enabled {
            p.text_disabled
        } else if ui.is_pressed(hit) {
            p.text_secondary
        } else {
            p.text_primary
        };
        let line = |size: f32, font| {
            box_()
                .h(COMMAND_LINE)
                .line_height(COMMAND_LINE)
                .font_family(font)
                .text_size(size)
        };
        children.push(
            box_()
                .hit(hit)
                .cursor(CursorStyle::Arrow)
                .flex_1()
                .h_full()
                .pt(COMMAND_ROW - COMMAND_BUTTON + COMMAND_INSET)
                .px(COMMAND_GAP + COMMAND_INSET)
                .pb(COMMAND_INSET)
                .child(
                    box_()
                        .size_full()
                        .items_center()
                        .pt(COMMAND_ICON_TOP)
                        .rounded(CONTROL_RADIUS)
                        .bg(bg)
                        .text_color(fg)
                        .child(line(ICON_SIZE, ICON_FONT).child(text(button.glyph)))
                        .child(
                            line(COMMAND_LABEL_SIZE, SMALL_FONT)
                                .mt(COMMAND_LABEL_GAP)
                                .nowrap()
                                .child(text(button.label)),
                        )
                        .when(focused, |this| {
                            this.child(focus_ring(p, Edges::ZERO, CONTROL_RADIUS))
                        }),
                ),
        );
    }
    box_()
        .w_full()
        .child(row().w_full().h(COMMAND_ROW).children(children))
        .child(box_().w_full().h(1.).bg(p.menu_separator))
}

/// Row `ix` of a panel; in the context menu (`context`), rows highlight like its buttons.
fn render_row(
    menu: &OpenMenu,
    panel: &Panel,
    level: usize,
    ix: usize,
    context: bool,
    p: Palette,
    ui: &mut AppUi,
) -> AppNode {
    let entry = &panel.entries[ix];
    if matches!(entry, Entry::Separator) {
        return box_()
            .h(ROW_SEPARATOR)
            .py(1.)
            .child(box_().h(1.).bg(p.menu_separator));
    }
    let hit = panel.hit(ix, 0);
    let ring = ui.last_input_was_keyboard && menu.focus == Some(Spot::new(level, ix, 0));
    let expanded = level == 0 && menu.submenu().is_some_and(|s| s.parent_row == ix);
    let enabled = entry.selectable();
    let fade = context && enabled;
    let (hovered, pressed) = (ui.is_hovered(hit), ui.is_pressed(hit));

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
        _ if !enabled => p.text_disabled,
        Entry::Toggle { .. } if hovered => p.text_primary,
        Entry::Toggle { .. } => p.text_secondary,
        _ if fade && pressed => p.text_secondary,
        _ => p.text_primary,
    };
    let label_color = if !enabled {
        p.text_disabled
    } else if fade && pressed {
        p.text_secondary
    } else {
        p.text_primary
    };
    let bg = if fade {
        ui.brush(hit, subtle_brushes(p), enabled)
    } else if enabled && pressed {
        p.subtle_tertiary
    } else if (enabled && hovered) || expanded {
        p.subtle_secondary
    } else {
        transparent_black()
    };
    let shortcut = entry
        .command_id()
        .filter(|_| panel.shortcuts)
        .and_then(shortcut_label);
    let side = |this: AppNode| this.flex_none().ml(SHORTCUT_GAP);

    let face = row()
        .items_center()
        .size_full()
        .px(ROW_PADDING)
        .rounded(CONTROL_RADIUS)
        .bg(bg)
        .text_color(p.text_primary)
        .when(panel.checks(), |this| {
            this.child(
                box_()
                    .flex_none()
                    .w(CHECK_COLUMN)
                    .font_family(ICON_FONT)
                    .text_size(mark_size)
                    .line_height(mark_size)
                    .text_color(mark_color)
                    .when_some(mark, |b, g| b.child(text(g))),
            )
        })
        .child(
            box_()
                .flex_1()
                .font_family(TEXT_FONT)
                .text_size(LABEL_SIZE)
                .nowrap()
                .text_color(label_color)
                .child(text(entry.label().map(display_label).unwrap_or_default())),
        )
        .when_some(shortcut, |this, sc| {
            this.child(
                side(box_())
                    .font_family(SMALL_FONT)
                    .text_size(SHORTCUT_SIZE)
                    .nowrap()
                    .text_color(if enabled {
                        p.text_secondary
                    } else {
                        p.text_disabled
                    })
                    .child(text(sc)),
            )
        })
        .when(matches!(entry, Entry::Submenu { .. }), |this| {
            this.child(
                side(box_())
                    .w(CHEVRON_WIDTH)
                    .font_family(ICON_FONT)
                    .text_size(GLYPH_SIZE)
                    .line_height(GLYPH_SIZE)
                    .text_color(p.text_secondary)
                    .child(text(CHEVRON_GLYPH)),
            )
        });
    box_()
        .hit(hit)
        .cursor(CursorStyle::Arrow)
        .h(entry.height())
        .px(ROW_INSET)
        .py(ROW_MARGIN_Y)
        .when(ring, |this| {
            this.child(focus_ring(p, Edges::ZERO, CONTROL_RADIUS))
        })
        .child(face)
}

#[cfg(test)]
mod tests {
    use crate::geom::point;
    use crate::platform::Modifiers;
    use crate::ui::TabId;

    use super::*;

    const SHOW_DELAY: Duration = Duration::from_millis(400);

    fn key(k: Key) -> KeyEvent {
        KeyEvent {
            key: k,
            modifiers: Modifiers::default(),
        }
    }

    fn alt_key(k: Key) -> KeyEvent {
        KeyEvent {
            key: k,
            modifiers: Modifiers {
                alt: true,
                ..Default::default()
            },
        }
    }

    fn row_hit(menus: &Menus, level: usize, ix: usize, col: usize) -> Option<HitId> {
        let Mode::Open { menu, .. } = &menus.mode else {
            return None;
        };
        Some(HitId::Menu(MenuHit::Row {
            serial: menu.panels.get(level)?.serial(),
            ix,
            col,
        }))
    }

    #[test]
    fn alt_access_keys_submenu_hover_and_system_chords() {
        let mut ui = AppUi::default();
        let mut bar = Menus::default();
        let ed_focus = Focus::Editor(EditorSlot::Tab(TabId::next()));
        let state = MenuState {
            can_undo: true,
            has_selection: true,
            has_text: true,
            clipboard_has_text: true,
            status_bar: true,
            word_wrap: true,
            ..Default::default()
        };

        // An Alt tap focuses the bar with key tips.
        assert!(!bar.on_key_down(key(Key::Alt), ed_focus, state).handled);
        let up = bar.on_key_up(key(Key::Alt), ed_focus);
        assert!(up.handled);
        assert_eq!(up.focus, FocusRequest::MenuBar);
        assert!(bar.access_active(Focus::MenuBar));

        // V opens View, keeping key tips.
        let open = bar.on_key_down(key(Key::Char('v')), Focus::MenuBar, state);
        assert!(open.handled);
        assert_eq!(bar.open_kind(), Some(MenuKind::View));
        assert!(bar.access_active(Focus::MenuBar));

        // Hovering the Zoom row arms its submenu, which opens when due; Right enters it, Enter runs
        // Zoom in.
        let zoom = row_hit(&bar, 0, 0, 0);
        let _ = bar.on_hover_change(zoom, || state, &mut ui, SHOW_DELAY);
        let (due, target) = bar
            .submenu_at
            .expect("hovering a submenu row arms its delay");
        assert_eq!(target, Some(0));
        assert!(!bar.tick(due - Duration::from_millis(1)));
        assert!(bar.tick(due));
        assert!(bar.submenu_at.is_none());
        assert!(
            bar.on_key_down(key(Key::Right), Focus::MenuBar, state)
                .handled
        );
        let run = bar.on_key_down(key(Key::Enter), Focus::MenuBar, state);
        assert_eq!(run.command, Some(AppCommand::ZoomIn.into()));
        assert!(!bar.is_open());

        // Alt+F4 closes the menu and goes on to Windows.
        bar.open_kind_menu(MenuKind::File, state);
        let alt_f4 = bar.on_key_down(alt_key(Key::F(4)), Focus::MenuBar, state);
        assert!(!alt_f4.handled);
        assert!(!bar.is_open());
    }

    #[test]
    fn context_menu_buttons_take_focus_and_the_flyout_returns_focus_to_its_field() {
        let mut ui = AppUi::default();
        let mut bar = Menus::default();
        let state = MenuState {
            clipboard_has_text: true,
            ..Default::default()
        };

        ui.last_input_was_keyboard = true;
        bar.open_context_menu(point(10., 10.), state);
        assert!(bar.is_open());
        // Hovering a button ends keyboard focus display. Cut and Copy are disabled, so Paste is
        // the first enabled button and Enter runs it.
        let paste = row_hit(&bar, 0, 0, 2);
        let _ = bar.on_hover_change(paste, || state, &mut ui, SHOW_DELAY);
        assert!(!ui.last_input_was_keyboard);
        let run = bar.on_key_down(key(Key::Enter), Focus::MenuBar, state);
        assert_eq!(run.command, Some(EditCommand::Paste.into()));
        assert_eq!(run.focus, FocusRequest::RestoreEditor);

        // Clicking a submenu row opens it without focusing its first row.
        bar.open_kind_menu(MenuKind::View, state);
        let zoom = row_hit(&bar, 0, 0, 0).unwrap();
        let HitId::Menu(zoom) = zoom else {
            unreachable!()
        };
        let _ = bar.on_click(zoom);
        assert!(
            bar.shown()
                .is_some_and(|m| m.submenu().is_some() && !m.in_submenu())
        );

        bar.open_find_options(false, true);
        assert_eq!(
            bar.close().focus,
            FocusRequest::Editor(EditorSlot::FindQuery)
        );
    }
}
