use crate::command::{AppCommand, Command, shortcut_label};
use crate::editor::EditCommand;
use crate::gfx::{TextStyle, TextSystem};
use crate::ids::MenuKind;
use crate::types::SharedStr;
use crate::ui::controls::{SMALL_FONT, TEXT_FONT};

pub(super) const BORDER: f32 = 1.;
pub(super) const PADDING_Y: f32 = 2.;
pub(super) const ROW_INSET: f32 = 4.;
pub(super) const ROW_PADDING: f32 = 11.;
pub(super) const ROW_ICON: f32 = 40.;
pub(super) const ROW_COMMAND: f32 = 33.;
pub(super) const ROW_COMPACT: f32 = 32.;
pub(super) const ROW_SEPARATOR: f32 = 3.;
pub(super) const CHECK_COLUMN: f32 = 28.;
pub(super) const SHORTCUT_GAP: f32 = 24.;
pub(super) const CHEVRON_WIDTH: f32 = 12.;
pub(super) const LABEL_SIZE: f32 = 14.;
pub(super) const SHORTCUT_SIZE: f32 = 12.;

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

#[derive(Clone, Copy)]
pub(crate) struct MenuCommand {
    pub(super) label: &'static str,
    pub(super) enabled: bool,
    pub(super) command: Command,
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

pub(super) struct CommandButton {
    pub(super) command: MenuCommand,
    pub(super) glyph: &'static str,
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

    pub(super) fn height(&self) -> f32 {
        match self {
            Self::Command { icon: Some(_), .. } => ROW_ICON,
            Self::Command { icon: None, .. } => ROW_COMMAND,
            Self::Toggle { .. } | Self::Submenu(..) => ROW_COMPACT,
            Self::Separator => ROW_SEPARATOR,
        }
    }

    pub(super) fn selectable(&self) -> bool {
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

    pub(super) fn access_key(&self) -> Option<char> {
        let (_, key) = self
            .label()
            .filter(|_| self.selectable())?
            .split_once('&')?;
        key.chars().next().map(|c| c.to_ascii_uppercase())
    }

    pub(super) fn command_id(&self) -> Option<Command> {
        match self {
            Self::Command { command, .. } => Some(command.command),
            Self::Toggle { command, .. } => Some(*command),
            _ => None,
        }
    }
}

impl MenuKind {
    pub(super) fn access_key(self) -> Option<char> {
        self.title().chars().next()
    }

    pub(super) fn cycle(self, delta: isize) -> Self {
        cycle(Some(self as usize), Self::ALL.len(), delta, true, |_| true)
            .map_or(self, |ix| Self::ALL[ix])
    }

    pub(super) fn entries(self, state: MenuState) -> Vec<Entry> {
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
            Self::Edit => vec![
                Entry::command_if("&Undo", state.can_undo, EditCommand::Undo),
                Entry::command_if("Re&do", state.can_redo, EditCommand::Redo),
                Entry::Separator,
                Entry::command_if("Cu&t", state.has_selection, EditCommand::Cut),
                Entry::command_if("&Copy", state.has_selection, EditCommand::Copy),
                Entry::command_if("&Paste", state.clipboard_has_text, EditCommand::Paste),
                Entry::command_if("De&lete", state.has_selection, EditCommand::Delete),
                Entry::Separator,
                Entry::command_if("&Find", state.has_text, AppCommand::Find),
                Entry::command_if("Find &next", state.has_text, AppCommand::FindNext),
                Entry::command_if("Find pre&vious", state.has_text, AppCommand::FindPrevious),
                Entry::command_if("&Replace", state.has_text, AppCommand::Replace),
                Entry::command("&Go to", AppCommand::GoTo),
                Entry::Separator,
                Entry::command("Select &all", EditCommand::SelectAll),
                Entry::command("Time/&Date", AppCommand::TimeDate),
            ],
            Self::View => vec![
                Entry::Submenu(
                    "&Zoom",
                    vec![
                        Entry::command("Zoom &in", AppCommand::ZoomIn),
                        Entry::command("Zoom &out", AppCommand::ZoomOut),
                        Entry::command("&Restore default zoom", AppCommand::ResetZoom),
                    ],
                ),
                Entry::toggle("&Status bar", state.status_bar, AppCommand::ToggleStatusBar),
                Entry::toggle("&Word wrap", state.word_wrap, AppCommand::ToggleWordWrap),
            ],
        }
    }
}

pub(super) fn context_menu_items(state: MenuState) -> (Vec<CommandButton>, Vec<Entry>) {
    let button = |label, glyph, enabled, command: EditCommand| CommandButton {
        command: MenuCommand::new(label, enabled, command),
        glyph,
    };
    let commands = vec![
        button("Cut", "\u{E8C6}", state.has_selection, EditCommand::Cut),
        button("Copy", "\u{E8C8}", state.has_selection, EditCommand::Copy),
        button("Paste", "\u{E77F}", state.clipboard_has_text, EditCommand::Paste),
        button("Select all", "\u{E8B3}", true, EditCommand::SelectAll),
        button("Delete", "\u{E74D}", state.has_selection, EditCommand::Delete),
    ];
    let entries = vec![Entry::Command {
        command: MenuCommand::new("Undo", state.can_undo, EditCommand::Undo),
        icon: Some("\u{E7A7}"),
    }];
    (commands, entries)
}

fn display_label(label: &str) -> SharedStr {
    label.replacen('&', "", 1).into()
}

pub(super) fn cycle(
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

pub(super) struct Panel {
    pub(super) entries: Vec<Entry>,
    pub(super) labels: Vec<SharedStr>,
    pub(super) shortcuts: Vec<Option<SharedStr>>,
    pub(super) width: f32,
    pub(super) checks: bool,
}

impl Panel {
    pub(super) fn new(
        entries: Vec<Entry>,
        min_width: f32,
        show_shortcuts: bool,
        ts: &TextSystem,
    ) -> Self {
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
        }
    }

    pub(super) fn height(&self) -> f32 {
        2. * (BORDER + PADDING_Y) + self.entries.iter().map(Entry::height).sum::<f32>()
    }

    pub(super) fn find_key(&self, key: char) -> Option<usize> {
        self.entries
            .iter()
            .position(|entry| entry.access_key() == Some(key))
    }

    pub(super) fn step(&self, current: Option<usize>, delta: isize) -> Option<usize> {
        cycle(current, self.entries.len(), delta, true, |ix| {
            self.entries[ix].selectable()
        })
    }

    pub(super) fn edge(&self, last: bool) -> Option<usize> {
        if last {
            self.entries.iter().rposition(Entry::selectable)
        } else {
            self.entries.iter().position(Entry::selectable)
        }
    }
}
