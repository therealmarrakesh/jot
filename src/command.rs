use crate::editor::EditCommand;
use crate::platform::{Key, KeyEvent, Modifiers};
use crate::types::SharedStr;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum AppCommand {
    NewTab,
    NewWindow,
    Open,
    Save,
    SaveAs,
    SaveAll,
    CloseTab,
    NextTab,
    PrevTab,
    SelectTab(usize),
    SelectLastTab,
    CloseWindow,
    Exit,
    Find,
    FindNext,
    FindPrevious,
    Replace,
    ReplaceNext,
    GoTo,
    TimeDate,
    ZoomIn,
    ZoomOut,
    ResetZoom,
    ToggleStatusBar,
    ToggleWordWrap,
    ToggleMatchCase,
    ToggleWrapAround,
    ShowContextMenu,
    Cancel,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Command {
    App(AppCommand),
    Edit(EditCommand),
}

impl From<AppCommand> for Command {
    fn from(cmd: AppCommand) -> Self {
        Self::App(cmd)
    }
}

impl From<EditCommand> for Command {
    fn from(cmd: EditCommand) -> Self {
        Self::Edit(cmd)
    }
}

const NONE: u8 = 0;
const CTRL: u8 = 1 << 0;
const ALT: u8 = 1 << 1;
const SHIFT: u8 = 1 << 2;
const ANY_SHIFT: u8 = 1 << 3;
const ANY_ALT: u8 = 1 << 4;
const ANY_CTRL: u8 = 1 << 5;

struct Binding {
    mods: u8,
    key: Key,
    command: Command,
    show_label: bool,
}

const fn bind(mods: u8, key: Key, command: Command) -> Binding {
    Binding {
        mods,
        key,
        command,
        show_label: false,
    }
}

const fn shown(mods: u8, key: Key, command: Command) -> Binding {
    Binding {
        mods,
        key,
        command,
        show_label: true,
    }
}

use AppCommand as A;
use Command::{App, Edit};
use EditCommand as E;
use Key as K;

const KEYMAP: &[Binding] = &[
    // App shortcuts
    bind(NONE, K::Escape, App(A::Cancel)),
    shown(CTRL, K::Char('n'), App(A::NewTab)),
    shown(CTRL | SHIFT, K::Char('n'), App(A::NewWindow)),
    shown(CTRL, K::Char('o'), App(A::Open)),
    shown(CTRL, K::Char('s'), App(A::Save)),
    shown(CTRL | SHIFT, K::Char('s'), App(A::SaveAs)),
    shown(CTRL | ALT, K::Char('s'), App(A::SaveAll)),
    shown(CTRL, K::Char('w'), App(A::CloseTab)),
    shown(CTRL | SHIFT, K::Char('w'), App(A::CloseWindow)),
    bind(CTRL, K::Tab, App(A::NextTab)),
    bind(CTRL, K::PageDown, App(A::NextTab)),
    bind(CTRL | SHIFT, K::Tab, App(A::PrevTab)),
    bind(CTRL, K::PageUp, App(A::PrevTab)),
    bind(CTRL, K::Char('1'), App(A::SelectTab(0))),
    bind(CTRL, K::Char('2'), App(A::SelectTab(1))),
    bind(CTRL, K::Char('3'), App(A::SelectTab(2))),
    bind(CTRL, K::Char('4'), App(A::SelectTab(3))),
    bind(CTRL, K::Char('5'), App(A::SelectTab(4))),
    bind(CTRL, K::Char('6'), App(A::SelectTab(5))),
    bind(CTRL, K::Char('7'), App(A::SelectTab(6))),
    bind(CTRL, K::Char('8'), App(A::SelectTab(7))),
    bind(CTRL, K::Char('9'), App(A::SelectLastTab)),
    shown(CTRL, K::Char('f'), App(A::Find)),
    shown(NONE, K::F(3), App(A::FindNext)),
    shown(SHIFT, K::F(3), App(A::FindPrevious)),
    shown(CTRL, K::Char('h'), App(A::Replace)),
    shown(CTRL, K::Char('g'), App(A::GoTo)),
    shown(NONE, K::F(5), App(A::TimeDate)),
    bind(SHIFT, K::F(10), App(A::ShowContextMenu)),
    bind(NONE, K::Menu, App(A::ShowContextMenu)),
    shown(CTRL | ANY_SHIFT, K::Char('='), App(A::ZoomIn)),
    bind(CTRL | ANY_SHIFT, K::Char('+'), App(A::ZoomIn)),
    shown(CTRL, K::Char('-'), App(A::ZoomOut)),
    shown(CTRL, K::Char('0'), App(A::ResetZoom)),
    // Edit shortcuts
    bind(ANY_SHIFT, K::Backspace, Edit(E::Backspace)),
    shown(NONE, K::Delete, Edit(E::Delete)),
    bind(SHIFT, K::Delete, Edit(E::Cut)),
    bind(CTRL, K::Insert, Edit(E::Copy)),
    bind(SHIFT, K::Insert, Edit(E::Paste)),
    bind(CTRL, K::Backspace, Edit(E::DeletePrevWord)),
    bind(CTRL, K::Delete, Edit(E::DeleteNextWord)),
    bind(ANY_CTRL, K::Enter, Edit(E::Enter)),
    bind(SHIFT, K::Enter, Edit(E::Enter)),
    bind(NONE, K::Escape, Edit(E::Escape)),
    bind(NONE, K::Up, Edit(E::MoveUp)),
    bind(NONE, K::Down, Edit(E::MoveDown)),
    bind(NONE, K::Left, Edit(E::MoveLeft)),
    bind(NONE, K::Right, Edit(E::MoveRight)),
    bind(NONE, K::PageUp, Edit(E::MovePageUp)),
    bind(NONE, K::PageDown, Edit(E::MovePageDown)),
    bind(SHIFT, K::PageUp, Edit(E::SelectPageUp)),
    bind(SHIFT, K::PageDown, Edit(E::SelectPageDown)),
    bind(NONE, K::Tab, Edit(E::IndentInline)),
    bind(SHIFT, K::Tab, Edit(E::OutdentInline)),
    bind(CTRL, K::Char(']'), Edit(E::Indent)),
    bind(CTRL, K::Char('['), Edit(E::Outdent)),
    bind(SHIFT | ANY_ALT, K::Left, Edit(E::SelectLeft)),
    bind(SHIFT | ANY_ALT, K::Right, Edit(E::SelectRight)),
    bind(SHIFT, K::Up, Edit(E::SelectUp)),
    bind(SHIFT, K::Down, Edit(E::SelectDown)),
    bind(NONE, K::Home, Edit(E::MoveHome)),
    bind(NONE, K::End, Edit(E::MoveEnd)),
    bind(CTRL, K::Home, Edit(E::MoveToStart)),
    bind(CTRL, K::End, Edit(E::MoveToEnd)),
    bind(CTRL | SHIFT, K::Home, Edit(E::SelectToStart)),
    bind(CTRL | SHIFT, K::End, Edit(E::SelectToEnd)),
    bind(SHIFT, K::Home, Edit(E::SelectToStartOfLine)),
    bind(SHIFT, K::End, Edit(E::SelectToEndOfLine)),
    bind(CTRL | SHIFT, K::Left, Edit(E::SelectPrevWord)),
    bind(CTRL | SHIFT, K::Right, Edit(E::SelectNextWord)),
    bind(CTRL, K::Left, Edit(E::MovePrevWord)),
    bind(CTRL, K::Right, Edit(E::MoveNextWord)),
    shown(CTRL, K::Char('a'), Edit(E::SelectAll)),
    shown(CTRL, K::Char('c'), Edit(E::Copy)),
    shown(CTRL, K::Char('x'), Edit(E::Cut)),
    shown(CTRL, K::Char('v'), Edit(E::Paste)),
    shown(CTRL, K::Char('z'), Edit(E::Undo)),
    shown(CTRL, K::Char('y'), Edit(E::Redo)),
    bind(CTRL | SHIFT, K::Char('z'), Edit(E::Redo)),
];

fn mods_match(spec: u8, m: Modifiers) -> bool {
    let ctrl_ok = spec & ANY_CTRL != 0 || m.ctrl == (spec & CTRL != 0);
    let alt_ok = spec & ANY_ALT != 0 || m.alt == (spec & ALT != 0);
    let shift_ok = spec & ANY_SHIFT != 0 || m.shift == (spec & SHIFT != 0);
    ctrl_ok && alt_ok && shift_ok
}

pub(crate) fn app_shortcut(ev: KeyEvent) -> Option<AppCommand> {
    if ev.modifiers.win || ev.modifiers.alt_gr {
        return None;
    }
    KEYMAP.iter().find_map(|b| match b.command {
        Command::App(cmd) if b.key == ev.key && mods_match(b.mods, ev.modifiers) => Some(cmd),
        _ => None,
    })
}

pub(crate) fn edit_shortcut(ev: KeyEvent) -> Option<EditCommand> {
    if ev.modifiers.win || ev.modifiers.alt_gr {
        return None;
    }
    KEYMAP.iter().find_map(|b| match b.command {
        Command::Edit(cmd) if b.key == ev.key && mods_match(b.mods, ev.modifiers) => Some(cmd),
        _ => None,
    })
}

fn format_chord(mods: u8, key: Key) -> SharedStr {
    let mut out = String::with_capacity(16);
    if mods & CTRL != 0 {
        out.push_str("Ctrl+");
    }
    if mods & ALT != 0 {
        out.push_str("Alt+");
    }
    if mods & SHIFT != 0 {
        out.push_str("Shift+");
    }
    match key {
        K::Char('=') | K::Char('+') => out.push_str("Plus"),
        K::Char('-') => out.push_str("Minus"),
        K::Char(c) => out.push(c.to_ascii_uppercase()),
        K::F(n) => {
            out.push('F');
            out.push_str(&n.to_string());
        }
        K::Delete => out.push_str("Del"),
        _ => {}
    }
    out.into()
}

pub(crate) fn shortcut_label(command: Command) -> Option<SharedStr> {
    KEYMAP
        .iter()
        .find(|b| b.show_label && b.command == command)
        .map(|b| format_chord(b.mods, b.key))
}

pub(crate) fn shortcut_tooltip(title: &str, command: Command) -> SharedStr {
    match shortcut_label(command) {
        Some(sc) => format!("{title} ({sc})").into(),
        None => SharedStr::from(title.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(ctrl: bool, alt: bool, shift: bool, key: Key) -> KeyEvent {
        KeyEvent {
            key,
            modifiers: Modifiers {
                ctrl,
                alt,
                shift,
                win: false,
                alt_gr: false,
            },
        }
    }

    #[test]
    fn keymap_matches_and_derives_labels() {
        assert_eq!(
            shortcut_label(AppCommand::NewTab.into()).as_deref(),
            Some("Ctrl+N")
        );
        assert_eq!(
            shortcut_label(AppCommand::SaveAll.into()).as_deref(),
            Some("Ctrl+Alt+S")
        );
        assert_eq!(
            shortcut_label(AppCommand::FindPrevious.into()).as_deref(),
            Some("Shift+F3")
        );
        assert_eq!(
            shortcut_label(AppCommand::ZoomIn.into()).as_deref(),
            Some("Ctrl+Plus")
        );
        assert_eq!(
            shortcut_label(AppCommand::ZoomOut.into()).as_deref(),
            Some("Ctrl+Minus")
        );
        assert_eq!(
            shortcut_label(EditCommand::Delete.into()).as_deref(),
            Some("Del")
        );
        assert_eq!(
            shortcut_label(EditCommand::Redo.into()).as_deref(),
            Some("Ctrl+Y")
        );
        assert_eq!(
            shortcut_tooltip("New tab", AppCommand::NewTab.into()).as_str(),
            "New tab (Ctrl+N)"
        );
        assert_eq!(
            app_shortcut(ev(true, false, true, K::Char('='))),
            Some(AppCommand::ZoomIn)
        );
        assert_eq!(
            edit_shortcut(ev(true, false, true, K::Char('z'))),
            Some(EditCommand::Redo)
        );
        assert_eq!(
            edit_shortcut(KeyEvent {
                key: K::Char('s'),
                modifiers: Modifiers {
                    ctrl: true,
                    alt: true,
                    shift: false,
                    win: false,
                    alt_gr: true,
                },
            }),
            None
        );
    }
}
