use crate::*;
use crate::win32::clipboard_has_text;

pub(super) const DEFAULT_MENU_SHOW_DELAY_MS: u64 = 400;
pub(super) const MIN_WIDTH: f32 = 232.;
pub(super) const FLYOUT_MIN_WIDTH: f32 = 198.;
pub(super) const FLYOUT_GAP: f32 = 3.;
pub(super) const CONTEXT_WIDTH: f32 = 342.;
pub(super) const ROW_ICON: f32 = 40.;
pub(super) const ICON_SIZE: f32 = 16.;
pub(super) const COMMAND_ROW: f32 = 59.;
pub(super) const COMMAND_BUTTON: f32 = 58.;
pub(super) const COMMAND_GAP: f32 = 1.;
pub(super) const COMMAND_ICON_TOP: f32 = 9.;
pub(super) const COMMAND_LABEL_PANEL: f32 = 60.;
pub(super) const COMMAND_LABEL_GAP: f32 = 4.;
pub(super) const COMMAND_LINE: f32 = 16.;
pub(super) const COMMAND_LABEL_SIZE: f32 = 12.;
pub(super) const COMMAND_DIVIDER_TOP: f32 = 14.;
pub(super) const COMMAND_DIVIDER_HEIGHT: f32 = 32.;
pub(super) const COMMAND_OUTSET: f32 = 1.;
pub(super) const OVERFLOW_TOP: f32 = 1.;
pub(super) const COMMAND_INSET: f32 = 2.;
pub(super) const ROW_COMMAND: f32 = 33.;
pub(super) const ROW_COMPACT: f32 = 32.;
pub(super) const ROW_SEPARATOR: f32 = 3.;
pub(super) const PADDING_Y: f32 = 2.;
pub(super) const ROW_INSET: f32 = 4.;
pub(super) const ROW_MARGIN_Y: f32 = 2.;
pub(super) const ROW_PADDING: f32 = 11.;
pub(super) const CHECK_COLUMN: f32 = 28.;
pub(super) const SHORTCUT_GAP: f32 = 24.;
pub(super) const CHEVRON_WIDTH: f32 = 12.;
pub(super) const LABEL_SIZE: f32 = 14.;
pub(super) const SHORTCUT_SIZE: f32 = 12.;
pub(super) const GLYPH_SIZE: f32 = 12.;
pub(super) const SUBMENU_OVERLAP: f32 = 4.;
pub(super) const BORDER: f32 = 1.;
pub(super) const BAR_ITEM_PADDING: f32 = 10.;
pub(super) const BAR_GAP: f32 = 8.;
pub(super) const CHECK_GLYPH: &str = "\u{E73E}";
pub(super) const CHEVRON_GLYPH: &str = "\u{E76C}";

#[derive(Clone, Copy)]
pub(crate) struct MenuCommand {
    pub(super) label: &'static str,
    pub(super) enabled: bool,
    pub(super) command: Command,
}

impl MenuCommand {
    fn new(label: &'static str, enabled: bool, command: Command) -> Self {
        Self {
            label,
            enabled,
            command,
        }
    }
}

pub(super) struct CommandButton {
    pub(super) command: MenuCommand,
    pub(super) glyph: &'static str,
}

pub(super) fn context_menu(editor: &EditorState) -> (Vec<CommandButton>, Vec<Entry>) {
    let selection = !editor.selected_range().is_empty();
    let button = |label, glyph, enabled, command| CommandButton {
        command: MenuCommand::new(label, enabled, command),
        glyph,
    };
    let commands = vec![
        button("Cut", "\u{E8C6}", selection, Command::Cut),
        button("Copy", "\u{E8C8}", selection, Command::Copy),
        button("Paste", "\u{E77F}", clipboard_has_text(), Command::Paste),
        button("Select all", "\u{E8B3}", true, Command::SelectAll),
        button("Delete", "\u{E74D}", selection, Command::Delete),
    ];
    let entries = vec![Entry::Command {
        command: MenuCommand::new("Undo", editor.can_undo(), Command::Undo),
        icon: Some("\u{E7A7}"),
    }];
    (commands, entries)
}

fn display_label(label: &str) -> SharedString {
    label.replacen('&', "", 1).into()
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
    fn command(label: &'static str, command: Command) -> Self {
        Self::command_if(label, true, command)
    }

    fn command_if(label: &'static str, enabled: bool, command: Command) -> Self {
        Self::Command {
            command: MenuCommand::new(label, enabled, command),
            icon: None,
        }
    }

    pub(crate) fn toggle(label: &'static str, checked: bool, command: Command) -> Self {
        Self::Toggle {
            label,
            checked,
            command,
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
        key.chars().next().map(|key| key.to_ascii_uppercase())
    }

    pub(super) fn command_id(&self) -> Option<Command> {
        match self {
            Self::Command { command, .. } => Some(command.command),
            Self::Toggle { command, .. } => Some(*command),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum MenuKind {
    File,
    Edit,
    View,
}

impl MenuKind {
    pub(super) const ALL: [Self; 3] = [Self::File, Self::Edit, Self::View];

    pub(super) fn title(self) -> &'static str {
        match self {
            Self::File => "File",
            Self::Edit => "Edit",
            Self::View => "View",
        }
    }

    pub(super) fn access_key(self) -> Option<char> {
        self.title().chars().next()
    }

    pub(super) fn index(self) -> usize {
        self as usize
    }

    pub(super) fn cycle(self, delta: isize) -> Self {
        cycle(Some(self.index()), Self::ALL.len(), delta, true, |_| true)
            .map_or(self, |ix| Self::ALL[ix])
    }

    pub(super) fn entries(self, editor: &EditorState, cx: &App) -> Vec<Entry> {
        match self {
            Self::File => vec![
                Entry::command("&New tab", Command::NewTab),
                Entry::command("New &window", Command::NewWindow),
                Entry::command("&Open", Command::Open),
                Entry::command("&Save", Command::Save),
                Entry::command("Save &as", Command::SaveAs),
                Entry::command("Save a&ll", Command::SaveAll),
                Entry::Separator,
                Entry::command("&Close tab", Command::CloseTab),
                Entry::command("Clos&e window", Command::CloseWindow),
                Entry::command("E&xit", Command::Exit),
            ],
            Self::Edit => {
                let selection = !editor.selected_range().is_empty();
                let clipboard_text = clipboard_has_text();
                let text = editor.text().len() > 0;
                vec![
                    Entry::command_if("&Undo", editor.can_undo(), Command::Undo),
                    Entry::Separator,
                    Entry::command_if("Cu&t", selection, Command::Cut),
                    Entry::command_if("&Copy", selection, Command::Copy),
                    Entry::command_if("&Paste", clipboard_text, Command::Paste),
                    Entry::command_if("De&lete", selection, Command::Delete),
                    Entry::Separator,
                    Entry::command_if("&Find", text, Command::Find),
                    Entry::command_if("Find &next", text, Command::FindNext),
                    Entry::command_if("Find pre&vious", text, Command::FindPrevious),
                    Entry::command_if("&Replace", text, Command::Replace),
                    Entry::command("&Go to", Command::GoTo),
                    Entry::Separator,
                    Entry::command("Select &all", Command::SelectAll),
                    Entry::command("Time/&Date", Command::TimeDate),
                ]
            }
            Self::View => {
                let view = &cx.shared.view;
                vec![
                    Entry::Submenu(
                        "&Zoom",
                        vec![
                            Entry::command("Zoom &in", Command::ZoomIn),
                            Entry::command("Zoom &out", Command::ZoomOut),
                            Entry::command("&Restore default zoom", Command::ResetZoom),
                        ],
                    ),
                    Entry::toggle("&Status bar", view.status_bar, Command::ToggleStatusBar),
                    Entry::toggle("&Word wrap", view.word_wrap, Command::ToggleWordWrap),
                ]
            }
        }
    }
}

pub(super) struct Panel {
    pub(super) entries: Vec<Entry>,
    pub(super) labels: Vec<SharedString>,
    pub(super) shortcuts: Vec<Option<SharedString>>,
    pub(super) width: f32,
    pub(super) checks: bool,
    pub(super) focused: Option<usize>,
}

impl Panel {
    pub(super) fn new(
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
                    .command_id()
                    .filter(|_| shortcuts)
                    .and_then(|cmd| shortcut_text(cmd, action_context, window))
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

    pub(super) fn height(&self) -> f32 {
        2. * (BORDER + PADDING_Y) + self.entries.iter().map(Entry::height).sum::<f32>()
    }

    pub(super) fn row_top(&self, ix: usize) -> f32 {
        BORDER + PADDING_Y + self.entries[..ix].iter().map(Entry::height).sum::<f32>()
    }

    pub(super) fn find_key(&self, key: char) -> Option<usize> {
        self.entries
            .iter()
            .position(|entry| entry.access_key() == Some(key))
    }

    pub(super) fn step(&mut self, delta: isize) {
        if let Some(ix) = cycle(self.focused, self.entries.len(), delta, true, |ix| {
            self.entries[ix].selectable()
        }) {
            self.focused = Some(ix);
        }
    }
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

pub(super) fn menu_show_delay() -> Duration {
    Duration::from_millis(
        crate::win32::menu_show_delay_ms().map_or(DEFAULT_MENU_SHOW_DELAY_MS, u64::from),
    )
}

fn shortcut_text(
    command: Command,
    focus: &FocusHandle,
    window: &Window,
) -> Option<SharedString> {
    let binding = window.shortcut_for_command_in(command, focus)?;
    let keystroke = binding.keystroke();
    let modifiers = keystroke.modifiers;
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
    parts.push(match keystroke.key.as_str() {
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
