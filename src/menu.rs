use std::time::Duration;

use crate::app::{AppEvent, ViewSettings};
use crate::color::transparent_black;
use crate::command::{AppCommand, Command, shortcut_label};
use crate::editor::{EditCommand, Editor};
use crate::geom::{Edges, Point};
use crate::gfx::{TextStyle, TextSystem};
use crate::ids::{AnimKey, Frame, HitId, Node, UiState};
use crate::keytips::{self, KeyTip, round_half_up};
use crate::platform::{
    CursorStyle, Jobs, Key, KeyEvent, TimerHandle, WindowId, clipboard::clipboard_has_text,
    system_settings,
};
use crate::theme::{Palette, theme_shadow};
use crate::types::SharedStr;
use crate::ui::controls::{
    CONTROL_RADIUS, ICON_FONT, OVERLAY_RADIUS, SMALL_FONT, TEXT_FONT, focus_ring, subtle_brushes,
};
use crate::ui::{Anchor, AnimStatus, Easing, box_, col, row, text};
use crate::window::{Focus, MENU_BAR_HEIGHT};

const OPEN_DURATION: Duration = Duration::from_millis(250);
const FADE_DURATION: Duration = Duration::from_millis(83);
const CLOSED_RATIO: f32 = 0.5;
const MENU_Z: f32 = 32.;
const SUBMENU_Z: f32 = 40.;
const SUBMENU_CLOSED_RATIO: f32 = 0.67;

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
const CHECK_GLYPH: &str = "\u{E73E}";
const CHEVRON_GLYPH: &str = "\u{E76C}";
const BAR_FOCUS_OUTSET: Edges<f32> = Edges {
    top: 0.,
    right: 1.,
    bottom: 1.,
    left: 1.,
};

#[derive(Clone, Copy)]
pub(crate) struct MenuCommand {
    label: &'static str,
    enabled: bool,
    command: Command,
}

impl MenuCommand {
    fn new(label: &'static str, enabled: bool, command: impl Into<Command>) -> Self {
        Self {
            label,
            enabled,
            command: command.into(),
        }
    }
}

struct CommandButton {
    command: MenuCommand,
    glyph: &'static str,
}

#[derive(Clone)]
pub(crate) enum Entry {
    Command {
        command: MenuCommand,
        icon: Option<&'static str>,
    },
    Toggle {
        label: &'static str,
        checked: bool,
        command: Command,
    },
    Submenu(&'static str, Vec<Entry>),
    Separator,
}

impl Entry {
    fn command(label: &'static str, command: impl Into<Command>) -> Self {
        Self::command_if(label, true, command)
    }

    fn command_if(label: &'static str, enabled: bool, command: impl Into<Command>) -> Self {
        Self::Command {
            command: MenuCommand::new(label, enabled, command),
            icon: None,
        }
    }

    pub(crate) fn toggle(label: &'static str, checked: bool, command: impl Into<Command>) -> Self {
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
        key.chars().next().map(|c| c.to_ascii_uppercase())
    }

    fn command_id(&self) -> Option<Command> {
        match self {
            Self::Command { command, .. } => Some(command.command),
            Self::Toggle { command, .. } => Some(*command),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum MenuKind {
    File,
    Edit,
    View,
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

    fn access_key(self) -> Option<char> {
        self.title().chars().next()
    }

    fn cycle(self, delta: isize) -> Self {
        cycle(Some(self as usize), Self::ALL.len(), delta, true, |_| true)
            .map_or(self, |ix| Self::ALL[ix])
    }

    fn entries(self, editor: &Editor, view: ViewSettings) -> Vec<Entry> {
        match self {
            Self::File => vec![
                Entry::command("&New tab", AppCommand::NewTab),
                Entry::command("New &window", AppCommand::NewWindow),
                Entry::command("&Open", AppCommand::Open),
                Entry::command("&Save", AppCommand::Save),
                Entry::command("Save &as", AppCommand::SaveAs),
                Entry::command("Save a&ll", AppCommand::SaveAll),
                Entry::Separator,
                Entry::command("&Close tab", AppCommand::CloseTab),
                Entry::command("Clos&e window", AppCommand::CloseWindow),
                Entry::command("E&xit", AppCommand::Exit),
            ],
            Self::Edit => {
                let selection = !editor.selected_range().is_empty();
                let clipboard_text = clipboard_has_text();
                let has_text = editor.text().len() > 0;
                vec![
                    Entry::command_if("&Undo", editor.can_undo(), EditCommand::Undo),
                    Entry::command_if("Re&do", editor.can_redo(), EditCommand::Redo),
                    Entry::Separator,
                    Entry::command_if("Cu&t", selection, EditCommand::Cut),
                    Entry::command_if("&Copy", selection, EditCommand::Copy),
                    Entry::command_if("&Paste", clipboard_text, EditCommand::Paste),
                    Entry::command_if("De&lete", selection, EditCommand::Delete),
                    Entry::Separator,
                    Entry::command_if("&Find", has_text, AppCommand::Find),
                    Entry::command_if("Find &next", has_text, AppCommand::FindNext),
                    Entry::command_if("Find pre&vious", has_text, AppCommand::FindPrevious),
                    Entry::command_if("&Replace", has_text, AppCommand::Replace),
                    Entry::command("&Go to", AppCommand::GoTo),
                    Entry::Separator,
                    Entry::command("Select &all", EditCommand::SelectAll),
                    Entry::command("Time/&Date", AppCommand::TimeDate),
                ]
            }
            Self::View => vec![
                Entry::Submenu(
                    "&Zoom",
                    vec![
                        Entry::command("Zoom &in", AppCommand::ZoomIn),
                        Entry::command("Zoom &out", AppCommand::ZoomOut),
                        Entry::command("&Restore default zoom", AppCommand::ResetZoom),
                    ],
                ),
                Entry::toggle("&Status bar", view.status_bar, AppCommand::ToggleStatusBar),
                Entry::toggle("&Word wrap", view.word_wrap, AppCommand::ToggleWordWrap),
            ],
        }
    }
}

fn context_menu_items(editor: &Editor) -> (Vec<CommandButton>, Vec<Entry>) {
    let selection = !editor.selected_range().is_empty();
    let button = |label, glyph, enabled, command: EditCommand| CommandButton {
        command: MenuCommand::new(label, enabled, command),
        glyph,
    };
    let commands = vec![
        button("Cut", "\u{E8C6}", selection, EditCommand::Cut),
        button("Copy", "\u{E8C8}", selection, EditCommand::Copy),
        button("Paste", "\u{E77F}", clipboard_has_text(), EditCommand::Paste),
        button("Select all", "\u{E8B3}", true, EditCommand::SelectAll),
        button("Delete", "\u{E74D}", selection, EditCommand::Delete),
    ];
    let entries = vec![Entry::Command {
        command: MenuCommand::new("Undo", editor.can_undo(), EditCommand::Undo),
        icon: Some("\u{E7A7}"),
    }];
    (commands, entries)
}

fn display_label(label: &str) -> SharedStr {
    label.replacen('&', "", 1).into()
}

fn menu_show_delay() -> Duration {
    Duration::from_millis(u64::from(system_settings().menu_show_delay_ms))
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

struct Panel {
    entries: Vec<Entry>,
    labels: Vec<SharedStr>,
    shortcuts: Vec<Option<SharedStr>>,
    width: f32,
    checks: bool,
    focused: Option<usize>,
}

impl Panel {
    fn new(entries: Vec<Entry>, min_width: f32, show_shortcuts: bool, ts: &TextSystem) -> Self {
        let labels: Vec<_> = entries
            .iter()
            .map(|e| e.label().map(display_label).unwrap_or_default())
            .collect();
        let shortcuts: Vec<_> = entries
            .iter()
            .map(|e| {
                e.command_id()
                    .filter(|_| show_shortcuts)
                    .and_then(shortcut_label)
            })
            .collect();
        let checks = entries.iter().any(|e| {
            matches!(
                e,
                Entry::Toggle { .. } | Entry::Command { icon: Some(_), .. }
            )
        });
        let label_style = TextStyle::regular(TEXT_FONT, LABEL_SIZE);
        let shortcut_style = TextStyle::regular(SMALL_FONT, SHORTCUT_SIZE);
        let content = entries
            .iter()
            .zip(&labels)
            .zip(&shortcuts)
            .map(|((entry, label), shortcut)| {
                let lw = ts.measure(label, &label_style).ceil();
                let trailing = match (entry, shortcut) {
                    (Entry::Submenu(..), _) => SHORTCUT_GAP + CHEVRON_WIDTH,
                    (_, Some(sc)) => SHORTCUT_GAP + ts.measure(sc, &shortcut_style).ceil(),
                    _ => 0.,
                };
                lw + trailing
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

    fn focus_edge(&mut self, last: bool) {
        self.focused = if last {
            self.entries.iter().rposition(Entry::selectable)
        } else {
            self.entries.iter().position(Entry::selectable)
        };
    }
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

    fn sample(&mut self, ui: &mut UiState) -> bool {
        if !self.closing {
            self.unroll = ui
                .animate_f32_from(
                    AnimKey::MenuUnroll(self.serial),
                    0.0,
                    1.0,
                    OPEN_DURATION,
                    Easing::DECELERATE,
                )
                .value;
        }
        let appear = if self.fade_in {
            ui.animate_f32_from(
                AnimKey::MenuAppear(self.serial),
                0.0,
                1.0,
                FADE_DURATION,
                Easing::Linear,
            )
            .value
        } else {
            1.
        };
        let fade = ui.animate_f32_from(
            AnimKey::MenuFade(self.serial),
            1.0,
            if self.closing { 0. } else { 1. },
            FADE_DURATION,
            Easing::Linear,
        );
        self.opacity = fade.value * appear;
        self.closing && fade.status == AnimStatus::Finished
    }
}

struct OpenSubmenu {
    ix: usize,
    panel: Panel,
    motion: Motion,
}

struct OpenMenu {
    kind: Option<MenuKind>,
    anchor: Anchor<HitId>,
    commands: Vec<CommandButton>,
    return_focus: Option<Focus>,
    panel: Panel,
    submenu: Option<OpenSubmenu>,
    motion: Motion,
    focused_command: Option<usize>,
    focus_visible: bool,
}

impl OpenMenu {
    fn new(kind: Option<MenuKind>, anchor: Anchor<HitId>, panel: Panel) -> Self {
        Self {
            kind,
            anchor,
            commands: Vec::new(),
            return_focus: None,
            panel,
            submenu: None,
            motion: Motion::new(0, false),
            focused_command: None,
            focus_visible: false,
        }
    }

    fn is_closing(&self) -> bool {
        self.motion.closing
    }

    fn live_submenu(&self) -> Option<&OpenSubmenu> {
        self.submenu.as_ref().filter(|s| !s.motion.closing)
    }

    fn live_submenu_mut(&mut self) -> Option<&mut OpenSubmenu> {
        self.submenu.as_mut().filter(|s| !s.motion.closing)
    }

    fn focused_submenu(&self) -> Option<&OpenSubmenu> {
        self.live_submenu().filter(|s| s.panel.focused.is_some())
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

    fn sample(&mut self, ui: &mut UiState) -> bool {
        if self.submenu.as_mut().is_some_and(|sub| sub.motion.sample(ui)) {
            self.submenu = None;
        }
        self.motion.sample(ui)
    }

    fn total_height(&self) -> (f32, f32) {
        let bar = if !self.commands.is_empty() {
            COMMAND_ROW + 1. + OVERFLOW_TOP - PADDING_Y
        } else {
            0.
        };
        (self.panel.height() + bar, bar)
    }
}

#[derive(Default)]
pub(crate) struct MenuOutcome {
    pub(crate) handled: bool,
    pub(crate) command: Option<Command>,
    pub(crate) focus: Option<Focus>,
    pub(crate) restore_editor_focus: bool,
    pub(crate) dirty: bool,
}

impl MenuOutcome {
    fn handled() -> Self {
        Self {
            handled: true,
            ..Default::default()
        }
    }

    fn handled_dirty() -> Self {
        Self {
            handled: true,
            dirty: true,
            ..Default::default()
        }
    }

    fn dirty() -> Self {
        Self {
            dirty: true,
            ..Default::default()
        }
    }
}

#[derive(Default)]
pub(crate) struct MenuBar {
    open: Option<OpenMenu>,
    serial: usize,
    submenu_timer: TimerHandle,
    bar_focus: Option<MenuKind>,
    access: bool,
    alt_armed: bool,
    hovered_row: Option<(usize, Option<usize>)>,
}

impl MenuBar {
    pub(crate) fn is_open(&self) -> bool {
        self.live_open().is_some()
    }

    pub(crate) fn access_active(&self) -> bool {
        self.access
    }

    pub(crate) fn open_kind(&self) -> Option<MenuKind> {
        self.live_open().and_then(|open| open.kind)
    }

    fn bar_focused(&self, focus: Focus) -> Option<MenuKind> {
        self.bar_focus
            .filter(|_| self.live_open().is_none() && focus == Focus::MenuBar)
    }

    fn next_serial(&mut self) -> usize {
        self.serial += 1;
        self.serial
    }

    fn live_open(&self) -> Option<&OpenMenu> {
        self.open.as_ref().filter(|o| !o.is_closing())
    }

    fn live_open_mut(&mut self) -> Option<&mut OpenMenu> {
        self.open.as_mut().filter(|o| !o.is_closing())
    }

    pub(crate) fn pointer_down(&mut self) -> MenuOutcome {
        self.alt_armed = false;
        let had_bar_focus = self.bar_focus.take().is_some();
        let had_access = std::mem::take(&mut self.access);
        let restore = had_bar_focus && self.live_open().is_none();
        MenuOutcome {
            restore_editor_focus: restore,
            dirty: had_bar_focus || had_access,
            ..Default::default()
        }
    }

    pub(crate) fn cancel(&mut self) -> MenuOutcome {
        self.alt_armed = false;
        let mut out = MenuOutcome::default();
        if self.live_open().is_some() {
            out = self.close();
        } else if self.bar_focus.is_some() {
            self.bar_focus = None;
            out.restore_editor_focus = true;
            out.dirty = true;
        }
        self.bar_focus = None;
        if self.access {
            self.access = false;
            out.dirty = true;
        }
        out
    }

    pub(crate) fn close(&mut self) -> MenuOutcome {
        self.alt_armed = false;
        self.access = false;
        self.hovered_row = None;
        let return_focus = self.live_open_mut().and_then(|open| {
            open.motion.closing = true;
            open.return_focus.take()
        });
        self.submenu_timer.cancel();
        MenuOutcome {
            handled: true,
            command: None,
            focus: return_focus,
            restore_editor_focus: return_focus.is_none(),
            dirty: true,
        }
    }

    pub(crate) fn toggle_trigger(
        &mut self,
        kind: MenuKind,
        editor: &Editor,
        view: ViewSettings,
        ts: &TextSystem,
    ) -> MenuOutcome {
        if self.open_kind() == Some(kind) {
            self.close()
        } else {
            self.open_kind_menu(kind, editor, view, ts)
        }
    }

    pub(crate) fn open_kind_menu(
        &mut self,
        kind: MenuKind,
        editor: &Editor,
        view: ViewSettings,
        ts: &TextSystem,
    ) -> MenuOutcome {
        let entries = kind.entries(editor, view);
        let panel = Panel::new(entries, MIN_WIDTH, true, ts);
        self.show(OpenMenu::new(
            Some(kind),
            Anchor::BelowStart {
                target: HitId::MenuTrigger(kind),
                gap: 0.,
            },
            panel,
        ))
    }

    fn open_by_keyboard(
        &mut self,
        kind: MenuKind,
        editor: &Editor,
        view: ViewSettings,
        ts: &TextSystem,
    ) -> MenuOutcome {
        let out = self.open_kind_menu(kind, editor, view, ts);
        if let Some(open) = self.open.as_mut() {
            open.panel.step(1);
            open.focus_visible = true;
        }
        out
    }

    pub(crate) fn open_context_menu(
        &mut self,
        anchor: Point,
        editor: &Editor,
        keyboard: bool,
        ts: &TextSystem,
    ) -> MenuOutcome {
        let (commands, entries) = context_menu_items(editor);
        let panel = Panel::new(entries, CONTEXT_WIDTH, false, ts);
        let focused_command = commands.iter().position(|b| b.command.enabled);
        self.show(OpenMenu {
            commands,
            focused_command,
            focus_visible: keyboard,
            ..OpenMenu::new(None, Anchor::Point(anchor), panel)
        })
    }

    pub(crate) fn open_flyout(
        &mut self,
        entries: Vec<Entry>,
        target: HitId,
        return_focus: Focus,
        ts: &TextSystem,
    ) -> MenuOutcome {
        let panel = Panel::new(entries, FLYOUT_MIN_WIDTH, true, ts);
        self.show(OpenMenu {
            return_focus: Some(return_focus),
            ..OpenMenu::new(
                None,
                Anchor::AboveOrBelowCenter {
                    target,
                    gap: FLYOUT_GAP,
                    min_top: MENU_BAR_HEIGHT,
                },
                panel,
            )
        })
    }

    fn show(&mut self, open: OpenMenu) -> MenuOutcome {
        self.bar_focus = None;
        self.hovered_row = None;
        let motion = Motion::new(self.next_serial(), !open.commands.is_empty());
        self.open = Some(OpenMenu { motion, ..open });
        self.submenu_timer.cancel();
        MenuOutcome {
            handled: true,
            command: None,
            focus: Some(Focus::MenuBar),
            restore_editor_focus: false,
            dirty: true,
        }
    }

    fn close_to_bar(&mut self) -> MenuOutcome {
        let Some(open) = self.live_open_mut() else {
            return MenuOutcome::default();
        };
        let Some(kind) = open.kind else {
            return self.close();
        };
        open.motion.closing = true;
        self.submenu_timer.cancel();
        self.hovered_row = None;
        self.bar_focus = Some(kind);
        MenuOutcome {
            handled: true,
            command: None,
            focus: Some(Focus::MenuBar),
            restore_editor_focus: false,
            dirty: true,
        }
    }

    pub(crate) fn on_hover_change(
        &mut self,
        window_id: WindowId,
        top: Option<HitId>,
        editor: &Editor,
        view: ViewSettings,
        ts: &TextSystem,
        jobs: &Jobs<AppEvent>,
    ) -> MenuOutcome {
        if let Some(HitId::MenuTrigger(kind)) = top
            && self.open_kind().is_some_and(|open| open != kind)
        {
            return self.open_kind_menu(kind, editor, view, ts);
        }

        let Some(open) = self.live_open() else {
            self.hovered_row = None;
            return MenuOutcome::default();
        };
        let row_path = match top {
            Some(HitId::MenuItem { ix, .. }) => Some((ix, None)),
            Some(HitId::SubmenuItem { ix: sub_ix, .. }) => {
                open.live_submenu().map(|s| (s.ix, Some(sub_ix)))
            }
            _ => None,
        };
        if row_path == self.hovered_row {
            return MenuOutcome::default();
        }
        let Some(path) = row_path else {
            self.hovered_row = None;
            self.submenu_timer.cancel();
            return MenuOutcome::default();
        };

        let target = match path {
            (ix, None) => {
                let current = open.live_submenu().map(|s| s.ix);
                match (open.panel.entries.get(ix), current) {
                    (Some(Entry::Submenu(..)), Some(open_ix)) if open_ix == ix => None,
                    (Some(Entry::Submenu(..)), _) => Some(Some(ix)),
                    (_, Some(_)) => Some(None),
                    (_, None) => None,
                }
            }
            (_, Some(_)) => None,
        };
        if let Some(open) = self.live_open_mut() {
            open.focus_visible = false;
            match path {
                (ix, None) => {
                    if open.panel.entries.get(ix).is_some_and(Entry::selectable) {
                        open.panel.focused = Some(ix);
                        open.focused_command = None;
                    }
                }
                (_, Some(sub_ix)) => {
                    if let Some(sub) = open.live_submenu_mut()
                        && sub.panel.entries.get(sub_ix).is_some_and(Entry::selectable)
                    {
                        sub.panel.focused = Some(sub_ix);
                    }
                }
            }
        }
        self.hovered_row = row_path;
        match target {
            Some(target) => {
                self.submenu_timer = jobs.after(
                    menu_show_delay(),
                    AppEvent::MenuSubmenuTimer {
                        window: window_id,
                        target,
                    },
                );
            }
            None => self.submenu_timer.cancel(),
        }
        MenuOutcome::dirty()
    }

    pub(crate) fn on_submenu_timer(&mut self, target: Option<usize>, ts: &TextSystem) -> bool {
        match target {
            Some(ix) => self.open_submenu(ix, false, ts),
            None => self.close_submenu(),
        }
    }

    pub(crate) fn click_item(&mut self, path: (usize, Option<usize>), ts: &TextSystem) -> MenuOutcome {
        self.activate(path, ts)
    }

    pub(crate) fn click_command(&mut self, ix: usize) -> MenuOutcome {
        let Some(cmd) = self
            .live_open()
            .and_then(|open| open.commands.get(ix))
            .map(|b| b.command)
            .filter(|c| c.enabled)
        else {
            return MenuOutcome::default();
        };
        let mut out = self.close();
        out.command = Some(cmd.command);
        out
    }

    fn activate(&mut self, path: (usize, Option<usize>), ts: &TextSystem) -> MenuOutcome {
        let Some(open) = self.live_open() else {
            return MenuOutcome::default();
        };
        let entry = match path {
            (ix, None) => open.panel.entries.get(ix).cloned(),
            (_, Some(sub)) => open
                .live_submenu()
                .and_then(|s| s.panel.entries.get(sub).cloned()),
        };
        let Some(entry) = entry else {
            return MenuOutcome::default();
        };
        match entry {
            Entry::Submenu(..) => {
                self.open_submenu(path.0, true, ts);
                MenuOutcome::dirty()
            }
            entry if !entry.selectable() => MenuOutcome::default(),
            entry => {
                let mut out = self.close();
                out.command = entry.command_id();
                out
            }
        }
    }

    fn open_submenu(&mut self, ix: usize, highlight_first: bool, ts: &TextSystem) -> bool {
        let serial = self.next_serial();
        self.submenu_timer.cancel();
        let Some(open) = self.live_open_mut() else {
            return false;
        };
        let Some(Entry::Submenu(_, entries)) = open.panel.entries.get(ix) else {
            return false;
        };
        if open.live_submenu().is_none_or(|sub| sub.ix != ix) {
            open.submenu = Some(OpenSubmenu {
                ix,
                panel: Panel::new(entries.clone(), MIN_WIDTH, true, ts),
                motion: Motion::new(serial, false),
            });
        }
        open.focus_visible |= highlight_first;
        if highlight_first && let Some(sub) = open.live_submenu_mut() {
            sub.panel.focused = None;
            sub.panel.step(1);
        }
        true
    }

    fn close_submenu(&mut self) -> bool {
        self.submenu_timer.cancel();
        if let Some(sub) = self.open.as_mut().and_then(OpenMenu::live_submenu_mut) {
            sub.motion.closing = true;
            true
        } else {
            false
        }
    }

    pub(crate) fn on_key_up(&mut self, ev: KeyEvent) -> MenuOutcome {
        if ev.key == Key::Alt && self.alt_armed {
            self.alt_armed = false;
            return self.toggle_access();
        }
        MenuOutcome::default()
    }

    pub(crate) fn on_key_down(
        &mut self,
        ev: KeyEvent,
        focus: Focus,
        editor: &Editor,
        view: ViewSettings,
        ts: &TextSystem,
    ) -> MenuOutcome {
        let m = ev.modifiers;
        if m.alt && !m.ctrl && !m.win && matches!(ev.key, Key::F(4) | Key::Space) {
            self.alt_armed = false;
            let mut out = self.cancel();
            out.handled = false;
            return out;
        }
        if ev.key == Key::Alt && m.none() {
            if self.live_open().is_some() {
                self.alt_armed = false;
                return self.close();
            }
            self.alt_armed = true;
            return MenuOutcome::default();
        }
        self.alt_armed = false;

        if ev.key == Key::F(10) && m.none() {
            if self.live_open().is_some() {
                return self.close();
            }
            return self.toggle_access();
        }

        let access_ch = match ev.key {
            Key::Char(c) if c.is_ascii_alphanumeric() => Some(c.to_ascii_uppercase()),
            _ => None,
        };

        if !self.access {
            if let Some(ch) = access_ch
                && m.alt
                && !m.ctrl
                && !m.win
                && !m.alt_gr
            {
                let out = self.invoke_access(ch, true, editor, view, ts);
                if out.handled {
                    return out;
                }
            }
            if self.live_open().is_some() {
                if let Some(ch) = access_ch
                    && m.none()
                {
                    let out = self.invoke_access(ch, false, editor, view, ts);
                    if out.handled {
                        return out;
                    }
                }
                let nav = self.handle_nav_key(ev, focus, editor, view, ts);
                if nav.handled {
                    return nav;
                }
                return MenuOutcome::handled();
            }
            if focus == Focus::MenuBar {
                return self.handle_nav_key(ev, focus, editor, view, ts);
            }
            return MenuOutcome::default();
        }

        match ev.key {
            Key::Escape if m.none() => self.access_back(),
            Key::Up
            | Key::Down
            | Key::Left
            | Key::Right
            | Key::Home
            | Key::End
            | Key::Tab
            | Key::Space
            | Key::Enter => {
                self.access = false;
                if focus == Focus::MenuBar {
                    let mut out = self.handle_nav_key(ev, focus, editor, view, ts);
                    out.dirty = true;
                    out
                } else {
                    MenuOutcome::dirty()
                }
            }
            _ => {
                if let Some(ch) = access_ch
                    && !m.ctrl
                    && !m.win
                    && !m.alt_gr
                {
                    let out = self.invoke_access(ch, true, editor, view, ts);
                    if out.handled || !m.alt {
                        return MenuOutcome {
                            handled: true,
                            ..out
                        };
                    }
                    return MenuOutcome::default();
                }
                if m.alt {
                    return MenuOutcome::default();
                }
                MenuOutcome::handled()
            }
        }
    }

    fn toggle_access(&mut self) -> MenuOutcome {
        if self.access {
            self.access = false;
            if self.live_open().is_none() {
                self.bar_focus = None;
                return MenuOutcome {
                    handled: true,
                    restore_editor_focus: true,
                    dirty: true,
                    ..Default::default()
                };
            }
            return MenuOutcome::handled_dirty();
        }
        match self.live_open() {
            Some(open) if open.kind.is_none() => MenuOutcome::handled(),
            Some(_) => {
                self.access = true;
                MenuOutcome::handled_dirty()
            }
            None => {
                if self.bar_focus.is_none() {
                    self.bar_focus = Some(MenuKind::File);
                }
                self.access = true;
                MenuOutcome {
                    handled: true,
                    focus: Some(Focus::MenuBar),
                    dirty: true,
                    ..Default::default()
                }
            }
        }
    }

    fn access_back(&mut self) -> MenuOutcome {
        match self.live_open_mut() {
            Some(open) => match open.live_submenu().map(|s| s.ix) {
                Some(ix) => {
                    open.panel.focused = Some(ix);
                    self.close_submenu();
                    MenuOutcome::handled_dirty()
                }
                None => self.close_to_bar(),
            },
            None => {
                self.access = false;
                self.bar_focus = None;
                MenuOutcome {
                    handled: true,
                    restore_editor_focus: true,
                    dirty: true,
                    ..Default::default()
                }
            }
        }
    }

    fn invoke_access(
        &mut self,
        key: char,
        show_access: bool,
        editor: &Editor,
        view: ViewSettings,
        ts: &TextSystem,
    ) -> MenuOutcome {
        let Some(open) = self.live_open_mut() else {
            let Some(kind) = MenuKind::ALL
                .into_iter()
                .find(|k| k.access_key() == Some(key))
            else {
                return MenuOutcome::default();
            };
            let out = self.open_by_keyboard(kind, editor, view, ts);
            self.access = show_access;
            return out;
        };
        if open.kind.is_none() {
            return MenuOutcome::default();
        }
        if let Some(sub) = open.live_submenu() {
            let Some(sub_ix) = sub.panel.find_key(key) else {
                return MenuOutcome::default();
            };
            let parent = sub.ix;
            self.access = false;
            return self.activate((parent, Some(sub_ix)), ts);
        }
        let Some(ix) = open.panel.find_key(key) else {
            return MenuOutcome::default();
        };
        if matches!(open.panel.entries[ix], Entry::Submenu(..)) {
            open.panel.focused = Some(ix);
            self.access = show_access;
            self.open_submenu(ix, true, ts);
            MenuOutcome::handled_dirty()
        } else {
            self.access = false;
            self.activate((ix, None), ts)
        }
    }

    fn handle_nav_key(
        &mut self,
        ev: KeyEvent,
        focus: Focus,
        editor: &Editor,
        view: ViewSettings,
        ts: &TextSystem,
    ) -> MenuOutcome {
        if !ev.modifiers.none() {
            return MenuOutcome::default();
        }
        match ev.key {
            Key::Up => self.step_active(-1, focus, editor, view, ts),
            Key::Down => self.step_active(1, focus, editor, view, ts),
            Key::Home => self.focus_edge(false),
            Key::End => self.focus_edge(true),
            Key::Enter | Key::Space => self.on_confirm(focus, editor, view, ts),
            Key::Escape => self.on_cancel(),
            Key::Left => self.on_left(editor, view, ts),
            Key::Right => self.on_right(editor, view, ts),
            _ => MenuOutcome::default(),
        }
    }

    fn focus_edge(&mut self, last: bool) -> MenuOutcome {
        let Some(open) = self.live_open_mut() else {
            return MenuOutcome::handled();
        };
        open.focus_visible = true;
        if open.in_submenu() {
            if let Some(sub) = open.live_submenu_mut() {
                sub.panel.focus_edge(last);
            }
        } else if open.commands.is_empty() {
            open.panel.focus_edge(last);
        } else if last {
            open.focused_command = None;
            open.panel.focus_edge(true);
            if open.panel.focused.is_none() {
                open.focused_command = open.commands.iter().rposition(|b| b.command.enabled);
            }
        } else {
            open.focused_command = open.commands.iter().position(|b| b.command.enabled);
            open.panel.focused = if open.focused_command.is_none() {
                open.panel.entries.iter().position(Entry::selectable)
            } else {
                None
            };
        }
        MenuOutcome::handled_dirty()
    }

    fn step_active(
        &mut self,
        delta: isize,
        focus: Focus,
        editor: &Editor,
        view: ViewSettings,
        ts: &TextSystem,
    ) -> MenuOutcome {
        let Some(open) = self.live_open_mut() else {
            if delta > 0
                && let Some(kind) = self.bar_focused(focus)
            {
                return self.open_by_keyboard(kind, editor, view, ts);
            }
            return MenuOutcome {
                handled: true,
                ..Default::default()
            };
        };
        open.focus_visible = true;
        if open.in_submenu() {
            if let Some(sub) = open.live_submenu_mut() {
                sub.panel.step(delta);
            }
        } else if open.commands.is_empty() {
            open.panel.step(delta);
        } else {
            open.step_linear(delta);
        }
        MenuOutcome {
            handled: true,
            dirty: true,
            ..Default::default()
        }
    }

    fn on_confirm(
        &mut self,
        focus: Focus,
        editor: &Editor,
        view: ViewSettings,
        ts: &TextSystem,
    ) -> MenuOutcome {
        let Some(open) = self.live_open() else {
            if let Some(kind) = self.bar_focused(focus) {
                return self.open_by_keyboard(kind, editor, view, ts);
            }
            return MenuOutcome {
                handled: true,
                ..Default::default()
            };
        };
        if let Some(ix) = open.focused_command {
            return self.click_command(ix);
        }
        let sub = open.live_submenu().and_then(|s| s.panel.focused);
        if let Some(ix) = open.panel.focused {
            return self.activate((ix, sub), ts);
        }
        MenuOutcome {
            handled: true,
            ..Default::default()
        }
    }

    fn on_cancel(&mut self) -> MenuOutcome {
        match self.live_open() {
            Some(open) if open.live_submenu().is_some() => {
                self.close_submenu();
                MenuOutcome::handled_dirty()
            }
            Some(open) if open.kind.is_some() => self.close_to_bar(),
            Some(_) => self.close(),
            None => {
                self.bar_focus = None;
                MenuOutcome {
                    handled: true,
                    restore_editor_focus: true,
                    dirty: true,
                    ..Default::default()
                }
            }
        }
    }

    fn on_left(&mut self, editor: &Editor, view: ViewSettings, ts: &TextSystem) -> MenuOutcome {
        let Some(open) = self.live_open_mut() else {
            if let Some(kind) = self.bar_focus {
                self.bar_focus = Some(kind.cycle(-1));
            }
            return MenuOutcome {
                handled: true,
                dirty: true,
                ..Default::default()
            };
        };
        open.focus_visible = true;
        if !open.commands.is_empty() {
            open.step_command(-1);
            MenuOutcome {
                handled: true,
                dirty: true,
                ..Default::default()
            }
        } else if open.in_submenu() {
            self.close_submenu();
            MenuOutcome {
                handled: true,
                dirty: true,
                ..Default::default()
            }
        } else if let Some(kind) = open.kind {
            self.open_by_keyboard(kind.cycle(-1), editor, view, ts)
        } else {
            MenuOutcome {
                handled: true,
                ..Default::default()
            }
        }
    }

    fn on_right(&mut self, editor: &Editor, view: ViewSettings, ts: &TextSystem) -> MenuOutcome {
        let Some(open) = self.live_open_mut() else {
            if let Some(kind) = self.bar_focus {
                self.bar_focus = Some(kind.cycle(1));
            }
            return MenuOutcome {
                handled: true,
                dirty: true,
                ..Default::default()
            };
        };
        open.focus_visible = true;
        if !open.commands.is_empty() {
            open.step_command(1);
            return MenuOutcome {
                handled: true,
                dirty: true,
                ..Default::default()
            };
        }
        let submenu_row = open.panel.focused.filter(|ix| {
            !open.in_submenu()
                && open
                    .panel
                    .entries
                    .get(*ix)
                    .is_some_and(|e| matches!(e, Entry::Submenu(..)))
        });
        match submenu_row {
            Some(ix) => {
                self.open_submenu(ix, true, ts);
                MenuOutcome {
                    handled: true,
                    dirty: true,
                    ..Default::default()
                }
            }
            None => {
                if let Some(kind) = open.kind {
                    self.open_by_keyboard(kind.cycle(1), editor, view, ts)
                } else {
                    MenuOutcome {
                        handled: true,
                        ..Default::default()
                    }
                }
            }
        }
    }

    pub(crate) fn sample(&mut self, ui: &mut UiState) {
        if self.open.as_mut().is_some_and(|open| open.sample(ui)) {
            self.open = None;
        }
    }

    pub(crate) fn render_bar(&self, focus: Focus, p: Palette, ui: &mut UiState) -> Node {
        let bar_focused = self.bar_focused(focus);
        let triggers: Vec<Node> = MenuKind::ALL
            .into_iter()
            .map(|kind| {
                let hit = HitId::MenuTrigger(kind);
                let active = self.open_kind() == Some(kind);
                let ring = bar_focused == Some(kind);
                let bg = if active {
                    p.subtle_tertiary
                } else {
                    ui.brush(hit, subtle_brushes(p), true)
                };
                row()
                    .hit(hit)
                    .cursor(CursorStyle::Arrow)
                    .relative()
                    .items_center()
                    .h_full()
                    .px(BAR_ITEM_PADDING)
                    .rounded(CONTROL_RADIUS)
                    .bg(bg)
                    .child(box_().relative().top(1.).child(text(kind.title())))
                    .when(ring, |this| {
                        this.child(focus_ring(
                            p,
                            BAR_FOCUS_OUTSET,
                            CONTROL_RADIUS + BAR_FOCUS_OUTSET.left,
                        ))
                    })
            })
            .collect();

        row()
            .hit(HitId::MenuBar)
            .relative()
            .flex_none()
            .h_full()
            .gap(BAR_GAP)
            .font_family(TEXT_FONT)
            .text_size(LABEL_SIZE)
            .text_color(p.text_primary)
            .children(triggers)
    }

    pub(crate) fn render_popup(
        &self,
        p: Palette,
        ts: &TextSystem,
        ui: &mut UiState,
    ) -> Option<Node> {
        let open = self.open.as_ref()?;
        let (height, bar) = open.total_height();
        let width = open.panel.width;
        let has_commands = !open.commands.is_empty();
        let hidden = open.panel.height() * CLOSED_RATIO * (1. - open.motion.unroll);
        let offset = if has_commands { 0. } else { -hidden };

        let cmd_row = has_commands.then(|| render_command_row(open, p, ts, ui));
        let main_rows = render_panel_rows(
            open,
            &open.panel,
            open.motion.serial,
            None,
            bar,
            has_commands,
            p,
            ui,
        );
        let sub_node = open
            .submenu
            .as_ref()
            .map(|sub| render_submenu_popup(open, sub, p, ui));

        let content = box_()
            .size_full()
            .when_some(cmd_row, |this, r| this.child(r))
            .child(main_rows);

        Some(
            box_()
                .hit(HitId::MenuPanel)
                .cursor(CursorStyle::Arrow)
                .anchored(open.anchor)
                .w(width)
                .h(height)
                .opacity(open.motion.opacity)
                .children(reveal_card(p, MENU_Z, height, height - hidden, offset, content))
                .when_some(sub_node, |this, sub| this.child(sub))
                .when(open.is_closing(), Node::inert),
        )
    }

    pub(crate) fn key_tips(&self, frame: &Frame, ts: &TextSystem) -> Vec<KeyTip> {
        if !self.access {
            return Vec::new();
        }
        let Some(open) = self.live_open() else {
            return MenuKind::ALL
                .into_iter()
                .filter_map(|kind| {
                    let key = kind.access_key()?;
                    let anchor = frame.rect_of(HitId::MenuTrigger(kind))?;
                    Some(keytips::layout_tip(ts, key, anchor, keytips::Placement::Below))
                })
                .collect();
        };
        if open.kind.is_none() {
            return Vec::new();
        }
        let (panel, serial, is_sub, motion) = match open.focused_submenu() {
            Some(sub) => (&sub.panel, sub.motion.serial, true, &sub.motion),
            None => (&open.panel, open.motion.serial, false, &open.motion),
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
                let hit = if is_sub {
                    HitId::SubmenuItem { serial, ix }
                } else {
                    HitId::MenuItem { serial, ix }
                };
                let face = frame.rect_of(hit)?.inset(Edges::xy(ROW_INSET, ROW_MARGIN_Y));
                Some(keytips::layout_tip(ts, key, face, keytips::Placement::Left))
            })
            .collect()
    }
}

fn render_command_row(open: &OpenMenu, p: Palette, ts: &TextSystem, ui: &mut UiState) -> Node {
    let commands = &open.commands;
    let count = commands.len() as f32;
    let row_w = open.panel.width + 2. * COMMAND_OUTSET;
    let cell = ((row_w - (count - 1.)) / count).floor();
    let ring = open.focused_command.filter(|_| open.focus_visible);

    let mut children = Vec::with_capacity(commands.len() * 2);
    for (ix, button) in commands.iter().enumerate() {
        if ix > 0 {
            children.push(
                box_()
                    .flex_none()
                    .w(1.)
                    .h(COMMAND_DIVIDER_HEIGHT)
                    .mt(COMMAND_DIVIDER_TOP)
                    .bg(p.menu_separator),
            );
        }
        children.push(render_command_button(
            p,
            open.motion.serial,
            ix,
            button,
            cell,
            ring == Some(ix),
            ts,
            ui,
        ));
    }

    row()
        .absolute()
        .top(BORDER)
        .left(-COMMAND_OUTSET)
        .w(row_w)
        .h(COMMAND_ROW)
        .children(children)
        .child(
            box_()
                .absolute()
                .left(BORDER + COMMAND_OUTSET)
                .right(BORDER + COMMAND_OUTSET)
                .bottom(-1.)
                .h(1.)
                .bg(p.menu_separator),
        )
}

#[allow(clippy::too_many_arguments)]
fn render_command_button(
    p: Palette,
    serial: usize,
    ix: usize,
    button: &CommandButton,
    cell: f32,
    focused: bool,
    ts: &TextSystem,
    ui: &mut UiState,
) -> Node {
    let face_w = cell - 2. * (COMMAND_GAP + COMMAND_INSET);
    let CommandButton { command, glyph } = button;
    let enabled = command.enabled;
    let hit = HitId::MenuCommand { serial, ix };
    let bg = ui.brush(hit, subtle_brushes(p), enabled);
    let fg = if !enabled {
        p.text_disabled
    } else if ui.is_pressed(hit) {
        p.text_secondary
    } else {
        p.text_primary
    };
    let label_width = ts
        .measure(
            command.label,
            &TextStyle::regular(SMALL_FONT, COMMAND_LABEL_SIZE),
        )
        .ceil();
    let label_left = (face_w - COMMAND_LABEL_PANEL) / 2.
        + round_half_up((COMMAND_LABEL_PANEL - label_width) / 2.);

    box_()
        .hit(hit)
        .cursor(CursorStyle::Arrow)
        .flex_none()
        .w(cell)
        .h_full()
        .pt(COMMAND_ROW - COMMAND_BUTTON + COMMAND_INSET)
        .px(COMMAND_GAP + COMMAND_INSET)
        .pb(COMMAND_INSET)
        .child(
            col()
                .relative()
                .size_full()
                .items_center()
                .pt(COMMAND_ICON_TOP)
                .rounded(CONTROL_RADIUS)
                .bg(bg)
                .text_color(fg)
                .child(
                    box_()
                        .h(COMMAND_LINE)
                        .line_height(COMMAND_LINE)
                        .font_family(ICON_FONT)
                        .text_size(ICON_SIZE)
                        .child(text(*glyph)),
                )
                .child(
                    box_()
                        .self_start()
                        .ml(label_left)
                        .w(label_width)
                        .text_center()
                        .mt(COMMAND_LABEL_GAP)
                        .h(COMMAND_LINE)
                        .line_height(COMMAND_LINE)
                        .font_family(SMALL_FONT)
                        .text_size(COMMAND_LABEL_SIZE)
                        .child(text(command.label)),
                )
                .when(focused, |this| {
                    this.child(focus_ring(p, Edges::ZERO, CONTROL_RADIUS))
                }),
        )
}

fn render_submenu_popup(
    open: &OpenMenu,
    submenu: &OpenSubmenu,
    p: Palette,
    ui: &mut UiState,
) -> Node {
    let sub_w = submenu.panel.width;
    let sub_h = submenu.panel.height();
    let hidden = sub_h * SUBMENU_CLOSED_RATIO * (1. - submenu.motion.unroll);
    let rows = render_panel_rows(
        open,
        &submenu.panel,
        submenu.motion.serial,
        Some(submenu.ix),
        0.,
        false,
        p,
        ui,
    );

    box_()
        .hit(HitId::SubmenuPanel)
        .cursor(CursorStyle::Arrow)
        .anchored(Anchor::SideOf {
            target: HitId::MenuItem {
                serial: open.motion.serial,
                ix: submenu.ix,
            },
            overlap: SUBMENU_OVERLAP - BORDER,
            offset_y: -(BORDER + PADDING_Y),
        })
        .w(sub_w)
        .h(sub_h)
        .opacity(submenu.motion.opacity)
        .children(reveal_card(p, SUBMENU_Z, sub_h, sub_h - hidden, -hidden, rows))
        .when(submenu.motion.closing, Node::inert)
}

#[allow(clippy::too_many_arguments)]
fn render_panel_rows(
    open: &OpenMenu,
    panel: &Panel,
    serial: usize,
    parent: Option<usize>,
    offset: f32,
    fade: bool,
    p: Palette,
    ui: &mut UiState,
) -> Node {
    let focus_visible = open.focus_visible && (parent.is_some() || !open.in_submenu());
    let expanded = open
        .live_submenu()
        .filter(|_| parent.is_none())
        .map(|s| s.ix);

    let rows: Vec<Node> = panel
        .entries
        .iter()
        .enumerate()
        .map(|(ix, entry)| {
            if matches!(entry, Entry::Separator) {
                return box_()
                    .h(ROW_SEPARATOR)
                    .py(1.)
                    .child(box_().h(1.).bg(p.menu_separator));
            }
            let hit = match parent {
                Some(_) => HitId::SubmenuItem { serial, ix },
                None => HitId::MenuItem { serial, ix },
            };
            let ring = focus_visible && panel.focused == Some(ix);
            let face = render_row_face(
                p,
                panel,
                ix,
                hit,
                fade && entry.selectable(),
                expanded == Some(ix),
                ui,
            );
            box_()
                .hit(hit)
                .cursor(CursorStyle::Arrow)
                .relative()
                .h(entry.height())
                .px(ROW_INSET)
                .py(ROW_MARGIN_Y)
                .when(ring, |this| {
                    this.child(focus_ring(p, Edges::ZERO, CONTROL_RADIUS))
                })
                .child(face)
        })
        .collect();

    col()
        .absolute()
        .top(BORDER + PADDING_Y + offset)
        .left(BORDER)
        .w(panel.width - 2. * BORDER)
        .children(rows)
}

fn render_row_face(
    p: Palette,
    panel: &Panel,
    ix: usize,
    hit: HitId,
    fade: bool,
    expanded: bool,
    ui: &mut UiState,
) -> Node {
    let entry = &panel.entries[ix];
    let label = panel.labels[ix].clone();
    let checks = panel.checks;
    let shortcut = panel.shortcuts[ix].clone();
    let enabled = entry.selectable();
    let hovered = ui.is_hovered(hit);
    let pressed = ui.is_pressed(hit);

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
        Entry::Toggle { .. } => {
            if hovered {
                p.text_primary
            } else {
                p.text_secondary
            }
        }
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

    row()
        .items_center()
        .size_full()
        .px(ROW_PADDING)
        .rounded(CONTROL_RADIUS)
        .bg(bg)
        .text_color(p.text_primary)
        .when(checks, |this| {
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
                .child(text(label)),
        )
        .when_some(shortcut, |this, sc| {
            this.child(
                box_()
                    .flex_none()
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
        .when(matches!(entry, Entry::Submenu(..)), |this| {
            this.child(
                box_()
                    .flex_none()
                    .font_family(ICON_FONT)
                    .text_size(GLYPH_SIZE)
                    .line_height(GLYPH_SIZE)
                    .text_color(p.text_secondary)
                    .child(text(CHEVRON_GLYPH)),
            )
        })
}

fn reveal_card(
    p: Palette,
    z: f32,
    height: f32,
    shown: f32,
    offset: f32,
    rows: Node,
) -> [Node; 2] {
    let card = box_()
        .absolute()
        .top(0.)
        .left(0.)
        .w_full()
        .h(shown)
        .border(BORDER, p.flyout_stroke)
        .rounded(OVERLAY_RADIUS)
        .shadows(theme_shadow(p.dark, z))
        .child(
            box_()
                .size_full()
                .rounded(OVERLAY_RADIUS - BORDER)
                .bg(p.menu_fill),
        );
    [
        card,
        box_()
            .absolute()
            .top(0.)
            .left(0.)
            .w_full()
            .h(shown)
            .overflow_hidden()
            .child(
                box_()
                    .absolute()
                    .left(0.)
                    .top(offset)
                    .w_full()
                    .h(height)
                    .child(rows),
            ),
    ]
}
