//! The commands keys and menus run, and the keymap: which chord runs what, and which chords the
//! menus and tooltips show.

use crate::editor::{EditCommand, Motion};
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
    ReplaceAll,
    ToggleReplace,
    FindOptions,
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

// Which modifiers a chord takes: these exactly, or any state of the ANY ones.
const NONE: u8 = 0;
const CTRL: u8 = 1 << 0;
const ALT: u8 = 1 << 1;
const SHIFT: u8 = 1 << 2;
const ANY_SHIFT: u8 = 1 << 3;
const ANY_ALT: u8 = 1 << 4;
const ANY_CTRL: u8 = 1 << 5;
/// The chord is the one menus and tooltips show for its command.
const SHOWN: u8 = 1 << 6;

use AppCommand as A;
use Command::{App, Edit};
use EditCommand as E;
use Key as K;
use Motion as M;

const fn mv(motion: Motion) -> Command {
    Edit(E::Move {
        motion,
        extend: false,
    })
}

const fn sel(motion: Motion) -> Command {
    Edit(E::Move {
        motion,
        extend: true,
    })
}

/// Chord, key and command; the first chord that matches a key wins.
const KEYMAP: &[(u8, Key, Command)] = &[
    // App shortcuts
    (NONE, K::Escape, App(A::Cancel)),
    (CTRL | SHOWN, K::Char('n'), App(A::NewTab)),
    (CTRL | SHIFT | SHOWN, K::Char('n'), App(A::NewWindow)),
    (CTRL | SHOWN, K::Char('o'), App(A::Open)),
    (CTRL | SHOWN, K::Char('s'), App(A::Save)),
    (CTRL | SHIFT | SHOWN, K::Char('s'), App(A::SaveAs)),
    (CTRL | ALT | SHOWN, K::Char('s'), App(A::SaveAll)),
    (CTRL | SHOWN, K::Char('w'), App(A::CloseTab)),
    (CTRL | SHIFT | SHOWN, K::Char('w'), App(A::CloseWindow)),
    (CTRL, K::Tab, App(A::NextTab)),
    (CTRL, K::PageDown, App(A::NextTab)),
    (CTRL | SHIFT, K::Tab, App(A::PrevTab)),
    (CTRL, K::PageUp, App(A::PrevTab)),
    (CTRL, K::Char('1'), App(A::SelectTab(0))),
    (CTRL, K::Char('2'), App(A::SelectTab(1))),
    (CTRL, K::Char('3'), App(A::SelectTab(2))),
    (CTRL, K::Char('4'), App(A::SelectTab(3))),
    (CTRL, K::Char('5'), App(A::SelectTab(4))),
    (CTRL, K::Char('6'), App(A::SelectTab(5))),
    (CTRL, K::Char('7'), App(A::SelectTab(6))),
    (CTRL, K::Char('8'), App(A::SelectTab(7))),
    (CTRL, K::Char('9'), App(A::SelectLastTab)),
    (CTRL | SHOWN, K::Char('f'), App(A::Find)),
    (SHOWN, K::F(3), App(A::FindNext)),
    (SHIFT | SHOWN, K::F(3), App(A::FindPrevious)),
    (CTRL | SHOWN, K::Char('h'), App(A::Replace)),
    (CTRL | SHOWN, K::Char('g'), App(A::GoTo)),
    (SHOWN, K::F(5), App(A::TimeDate)),
    (SHIFT, K::F(10), App(A::ShowContextMenu)),
    (NONE, K::Menu, App(A::ShowContextMenu)),
    (CTRL | ANY_SHIFT | SHOWN, K::Char('='), App(A::ZoomIn)),
    (CTRL | ANY_SHIFT, K::Char('+'), App(A::ZoomIn)),
    (CTRL | SHOWN, K::Char('-'), App(A::ZoomOut)),
    (CTRL | SHOWN, K::Char('0'), App(A::ResetZoom)),
    // Edit shortcuts
    (ANY_SHIFT, K::Backspace, Edit(E::Backspace)),
    (SHOWN, K::Delete, Edit(E::Delete)),
    (SHIFT, K::Delete, Edit(E::Cut)),
    (CTRL, K::Insert, Edit(E::Copy)),
    (SHIFT, K::Insert, Edit(E::Paste)),
    (CTRL, K::Backspace, Edit(E::DeletePrevWord)),
    (CTRL, K::Delete, Edit(E::DeleteNextWord)),
    (ANY_CTRL, K::Enter, Edit(E::Enter)),
    (SHIFT, K::Enter, Edit(E::Enter)),
    (NONE, K::Escape, Edit(E::Escape)),
    (NONE, K::Up, mv(M::Up)),
    (NONE, K::Down, mv(M::Down)),
    (NONE, K::Left, mv(M::Left)),
    (NONE, K::Right, mv(M::Right)),
    (NONE, K::PageUp, mv(M::PageUp)),
    (NONE, K::PageDown, mv(M::PageDown)),
    (SHIFT, K::PageUp, sel(M::PageUp)),
    (SHIFT, K::PageDown, sel(M::PageDown)),
    (NONE, K::Tab, Edit(E::IndentInline)),
    (SHIFT, K::Tab, Edit(E::Outdent)),
    (CTRL, K::Char(']'), Edit(E::Indent)),
    (CTRL, K::Char('['), Edit(E::Outdent)),
    (SHIFT | ANY_ALT, K::Left, sel(M::Left)),
    (SHIFT | ANY_ALT, K::Right, sel(M::Right)),
    (SHIFT, K::Up, sel(M::Up)),
    (SHIFT, K::Down, sel(M::Down)),
    (NONE, K::Home, mv(M::LineStart)),
    (NONE, K::End, mv(M::LineEnd)),
    (CTRL, K::Home, mv(M::DocStart)),
    (CTRL, K::End, mv(M::DocEnd)),
    (CTRL | SHIFT, K::Home, sel(M::DocStart)),
    (CTRL | SHIFT, K::End, sel(M::DocEnd)),
    (SHIFT, K::Home, sel(M::LineStart)),
    (SHIFT, K::End, sel(M::LineEnd)),
    (CTRL | SHIFT, K::Left, sel(M::PrevWord)),
    (CTRL | SHIFT, K::Right, sel(M::NextWord)),
    (CTRL, K::Left, mv(M::PrevWord)),
    (CTRL, K::Right, mv(M::NextWord)),
    (CTRL | SHOWN, K::Char('a'), Edit(E::SelectAll)),
    (CTRL | SHOWN, K::Char('c'), Edit(E::Copy)),
    (CTRL | SHOWN, K::Char('x'), Edit(E::Cut)),
    (CTRL | SHOWN, K::Char('v'), Edit(E::Paste)),
    (CTRL | SHOWN, K::Char('z'), Edit(E::Undo)),
    (CTRL | SHOWN, K::Char('y'), Edit(E::Redo)),
    (CTRL | SHIFT, K::Char('z'), Edit(E::Redo)),
];

fn mods_match(spec: u8, m: Modifiers) -> bool {
    let ctrl_ok = spec & ANY_CTRL != 0 || m.ctrl == (spec & CTRL != 0);
    let alt_ok = spec & ANY_ALT != 0 || m.alt == (spec & ALT != 0);
    let shift_ok = spec & ANY_SHIFT != 0 || m.shift == (spec & SHIFT != 0);
    ctrl_ok && alt_ok && shift_ok
}

fn shortcut<T>(ev: KeyEvent, pick: impl Fn(Command) -> Option<T>) -> Option<T> {
    if ev.modifiers.win || ev.modifiers.alt_gr {
        return None;
    }
    KEYMAP
        .iter()
        .filter(|&&(mods, key, _)| key == ev.key && mods_match(mods, ev.modifiers))
        .find_map(|&(_, _, command)| pick(command))
}

pub(crate) fn app_shortcut(ev: KeyEvent) -> Option<AppCommand> {
    shortcut(ev, |c| if let App(cmd) = c { Some(cmd) } else { None })
}

pub(crate) fn edit_shortcut(ev: KeyEvent) -> Option<EditCommand> {
    shortcut(ev, |c| if let Edit(cmd) = c { Some(cmd) } else { None })
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
        K::F(n) => out.push_str(&format!("F{n}")),
        K::Delete => out.push_str("Del"),
        _ => {}
    }
    out.into()
}

pub(crate) fn shortcut_label(command: Command) -> Option<SharedStr> {
    KEYMAP
        .iter()
        .find(|&&(mods, _, c)| mods & SHOWN != 0 && c == command)
        .map(|&(mods, key, _)| format_chord(mods, key))
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
