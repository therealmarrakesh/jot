use crate::command::Command;
use crate::geom::Point;
use crate::gfx::TextSystem;
use crate::ids::{EditorSlot, HitId, UiState};
use crate::platform::{Key, KeyEvent};

use super::model::{Entry, MenuKind, MenuState, Panel, context_menu_items};
use super::popup::{Motion, OpenMenu, OpenSubmenu, PopupKind};

const MIN_WIDTH: f32 = 232.;
const FLYOUT_MIN_WIDTH: f32 = 198.;
const CONTEXT_WIDTH: f32 = 342.;

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum FocusRequest {
    #[default]
    Keep,
    MenuBar,
    RestoreEditor,
    Editor(EditorSlot),
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum SubmenuTimer {
    #[default]
    Keep,
    Cancel,
    Arm(Option<usize>),
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct MenuOutcome {
    pub(crate) handled: bool,
    pub(crate) command: Option<Command>,
    pub(crate) focus: FocusRequest,
    pub(crate) submenu_timer: SubmenuTimer,
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

    fn with_focus(focus: FocusRequest) -> Self {
        Self {
            handled: true,
            focus,
            dirty: true,
            ..Default::default()
        }
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
enum BarMode {
    #[default]
    Idle,
    Focused {
        kind: MenuKind,
        key_tips: bool,
    },
    PopupKeyTips,
}

#[derive(Default)]
pub(crate) struct MenuBar {
    open: Option<OpenMenu>,
    serial: usize,
    mode: BarMode,
    alt_armed: bool,
    hovered_row: Option<(usize, Option<usize>)>,
}

impl MenuBar {
    pub(crate) fn is_open(&self) -> bool {
        self.live_open().is_some()
    }

    pub(crate) fn access_active(&self) -> bool {
        matches!(
            self.mode,
            BarMode::Focused { key_tips: true, .. } | BarMode::PopupKeyTips
        )
    }

    pub(crate) fn open_kind(&self) -> Option<MenuKind> {
        self.live_open().and_then(|open| open.kind.bar_kind())
    }

    pub(super) fn open_popup(&self) -> Option<&OpenMenu> {
        self.open.as_ref()
    }

    pub(super) fn bar_focused(&self, bar_has_focus: bool) -> Option<MenuKind> {
        if self.live_open().is_some() || !bar_has_focus {
            return None;
        }
        match self.mode {
            BarMode::Focused { kind, .. } => Some(kind),
            BarMode::Idle | BarMode::PopupKeyTips => None,
        }
    }

    fn next_serial(&mut self) -> usize {
        self.serial += 1;
        self.serial
    }

    pub(super) fn live_open(&self) -> Option<&OpenMenu> {
        self.open.as_ref().filter(|o| !o.is_closing())
    }

    fn live_open_mut(&mut self) -> Option<&mut OpenMenu> {
        self.open.as_mut().filter(|o| !o.is_closing())
    }

    fn clear_key_tips(&mut self) {
        self.mode = match self.mode {
            BarMode::Focused { kind, .. } => BarMode::Focused {
                kind,
                key_tips: false,
            },
            BarMode::PopupKeyTips | BarMode::Idle => BarMode::Idle,
        };
    }

    pub(crate) fn pointer_down(&mut self) -> MenuOutcome {
        self.alt_armed = false;
        let prev = std::mem::replace(&mut self.mode, BarMode::Idle);
        let had_bar_focus = matches!(prev, BarMode::Focused { .. });
        let had_access = matches!(
            prev,
            BarMode::Focused { key_tips: true, .. } | BarMode::PopupKeyTips
        );
        MenuOutcome {
            focus: if had_bar_focus && self.live_open().is_none() {
                FocusRequest::RestoreEditor
            } else {
                FocusRequest::Keep
            },
            dirty: had_bar_focus || had_access,
            ..Default::default()
        }
    }

    pub(crate) fn cancel(&mut self) -> MenuOutcome {
        self.alt_armed = false;
        if self.live_open().is_some() {
            return self.close();
        }
        let prev = std::mem::replace(&mut self.mode, BarMode::Idle);
        match prev {
            BarMode::Focused { .. } => MenuOutcome {
                focus: FocusRequest::RestoreEditor,
                dirty: true,
                ..Default::default()
            },
            BarMode::PopupKeyTips => MenuOutcome::dirty(),
            BarMode::Idle => MenuOutcome::default(),
        }
    }

    pub(crate) fn close(&mut self) -> MenuOutcome {
        self.alt_armed = false;
        self.mode = BarMode::Idle;
        self.hovered_row = None;
        let focus = self
            .live_open_mut()
            .map(|open| {
                open.motion.closing = true;
                open.kind.return_focus()
            })
            .unwrap_or(FocusRequest::RestoreEditor);
        MenuOutcome {
            handled: true,
            command: None,
            focus,
            submenu_timer: SubmenuTimer::Cancel,
            dirty: true,
        }
    }

    pub(crate) fn toggle_trigger(
        &mut self,
        kind: MenuKind,
        state: MenuState,
        ts: &TextSystem,
    ) -> MenuOutcome {
        if self.open_kind() == Some(kind) {
            self.close()
        } else {
            self.open_kind_menu(kind, state, ts)
        }
    }

    pub(crate) fn open_kind_menu(
        &mut self,
        kind: MenuKind,
        state: MenuState,
        ts: &TextSystem,
    ) -> MenuOutcome {
        let entries = kind.entries(state);
        let panel = Panel::new(entries, MIN_WIDTH, true, ts);
        let serial = self.next_serial();
        self.show(OpenMenu::new(PopupKind::Bar(kind), panel, serial), false)
    }

    fn open_by_keyboard(
        &mut self,
        kind: MenuKind,
        state: MenuState,
        key_tips: bool,
        ts: &TextSystem,
    ) -> MenuOutcome {
        let entries = kind.entries(state);
        let mut panel = Panel::new(entries, MIN_WIDTH, true, ts);
        panel.step(1);
        let serial = self.next_serial();
        let mut open = OpenMenu::new(PopupKind::Bar(kind), panel, serial);
        open.focus_visible = true;
        self.show(open, key_tips)
    }

    pub(crate) fn open_context_menu(
        &mut self,
        anchor: Point,
        state: MenuState,
        keyboard: bool,
        ts: &TextSystem,
    ) -> MenuOutcome {
        let (commands, entries) = context_menu_items(state);
        let panel = Panel::new(entries, CONTEXT_WIDTH, false, ts);
        let focused_command = commands.iter().position(|b| b.command.enabled);
        let serial = self.next_serial();
        let mut open = OpenMenu::new(PopupKind::Context { anchor, commands }, panel, serial);
        open.focused_command = focused_command;
        open.focus_visible = keyboard;
        self.show(open, false)
    }

    pub(crate) fn open_flyout(
        &mut self,
        entries: Vec<Entry>,
        target: HitId,
        return_slot: EditorSlot,
        ts: &TextSystem,
    ) -> MenuOutcome {
        let panel = Panel::new(entries, FLYOUT_MIN_WIDTH, true, ts);
        let serial = self.next_serial();
        self.show(
            OpenMenu::new(
                PopupKind::Flyout {
                    target,
                    return_slot,
                },
                panel,
                serial,
            ),
            false,
        )
    }

    fn show(&mut self, open: OpenMenu, key_tips: bool) -> MenuOutcome {
        self.mode = if key_tips {
            BarMode::PopupKeyTips
        } else {
            BarMode::Idle
        };
        self.hovered_row = None;
        self.open = Some(open);
        MenuOutcome {
            handled: true,
            command: None,
            focus: FocusRequest::MenuBar,
            submenu_timer: SubmenuTimer::Cancel,
            dirty: true,
        }
    }

    fn close_to_bar(&mut self) -> MenuOutcome {
        let Some(open) = self.live_open_mut() else {
            return MenuOutcome::default();
        };
        let Some(kind) = open.kind.bar_kind() else {
            return self.close();
        };
        open.motion.closing = true;
        self.hovered_row = None;
        self.mode = BarMode::Focused {
            kind,
            key_tips: self.mode == BarMode::PopupKeyTips,
        };
        MenuOutcome {
            handled: true,
            command: None,
            focus: FocusRequest::MenuBar,
            submenu_timer: SubmenuTimer::Cancel,
            dirty: true,
        }
    }

    pub(crate) fn on_hover_change(
        &mut self,
        top: Option<HitId>,
        state: MenuState,
        ts: &TextSystem,
    ) -> MenuOutcome {
        if let Some(HitId::MenuTrigger(kind)) = top
            && self.open_kind().is_some_and(|open| open != kind)
        {
            return self.open_kind_menu(kind, state, ts);
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
            return MenuOutcome {
                submenu_timer: SubmenuTimer::Cancel,
                ..Default::default()
            };
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
        let submenu_timer = match target {
            Some(t) => SubmenuTimer::Arm(t),
            None => SubmenuTimer::Cancel,
        };
        MenuOutcome {
            submenu_timer,
            dirty: true,
            ..Default::default()
        }
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
            .and_then(|open| open.kind.commands().get(ix))
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
                MenuOutcome {
                    submenu_timer: SubmenuTimer::Cancel,
                    dirty: true,
                    ..Default::default()
                }
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
        bar_has_focus: bool,
        state: MenuState,
        ts: &TextSystem,
    ) -> MenuOutcome {
        let m = ev.modifiers;
        if m.alt && !m.ctrl && !m.win && matches!(ev.key, Key::F(4) | Key::Space) {
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

        if !self.access_active() {
            if let Some(ch) = access_ch
                && m.alt
                && !m.ctrl
                && !m.win
                && !m.alt_gr
            {
                let out = self.invoke_access(ch, true, state, ts);
                if out.handled {
                    return out;
                }
            }
            if self.live_open().is_some() {
                if let Some(ch) = access_ch
                    && m.none()
                {
                    let out = self.invoke_access(ch, false, state, ts);
                    if out.handled {
                        return out;
                    }
                }
                let nav = self.handle_nav_key(ev, bar_has_focus, state, ts);
                if nav.handled {
                    return nav;
                }
                return MenuOutcome::handled();
            }
            if bar_has_focus {
                return self.handle_nav_key(ev, bar_has_focus, state, ts);
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
                self.clear_key_tips();
                if bar_has_focus {
                    let mut out = self.handle_nav_key(ev, bar_has_focus, state, ts);
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
                    let out = self.invoke_access(ch, true, state, ts);
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
        if self.access_active() {
            self.mode = BarMode::Idle;
            if self.live_open().is_none() {
                return MenuOutcome::with_focus(FocusRequest::RestoreEditor);
            }
            return MenuOutcome::handled_dirty();
        }
        match self.live_open() {
            Some(open) if open.kind.bar_kind().is_none() => MenuOutcome::handled(),
            Some(_) => {
                self.mode = BarMode::PopupKeyTips;
                MenuOutcome::handled_dirty()
            }
            None => {
                let kind = match self.mode {
                    BarMode::Focused { kind, .. } => kind,
                    _ => MenuKind::File,
                };
                self.mode = BarMode::Focused {
                    kind,
                    key_tips: true,
                };
                MenuOutcome::with_focus(FocusRequest::MenuBar)
            }
        }
    }

    fn access_back(&mut self) -> MenuOutcome {
        match self.live_open_mut() {
            Some(open) => match open.live_submenu().map(|s| s.ix) {
                Some(ix) => {
                    open.panel.focused = Some(ix);
                    self.close_submenu();
                    MenuOutcome {
                        handled: true,
                        submenu_timer: SubmenuTimer::Cancel,
                        dirty: true,
                        ..Default::default()
                    }
                }
                None => self.close_to_bar(),
            },
            None => {
                self.mode = BarMode::Idle;
                MenuOutcome::with_focus(FocusRequest::RestoreEditor)
            }
        }
    }

    fn invoke_access(
        &mut self,
        key: char,
        show_access: bool,
        state: MenuState,
        ts: &TextSystem,
    ) -> MenuOutcome {
        let Some(open) = self.live_open_mut() else {
            let Some(kind) = MenuKind::ALL
                .into_iter()
                .find(|k| k.access_key() == Some(key))
            else {
                return MenuOutcome::default();
            };
            return self.open_by_keyboard(kind, state, show_access, ts);
        };
        if open.kind.bar_kind().is_none() {
            return MenuOutcome::default();
        }
        if let Some(sub) = open.live_submenu() {
            let Some(sub_ix) = sub.panel.find_key(key) else {
                return MenuOutcome::default();
            };
            let parent = sub.ix;
            self.mode = BarMode::Idle;
            return self.activate((parent, Some(sub_ix)), ts);
        }
        let Some(ix) = open.panel.find_key(key) else {
            return MenuOutcome::default();
        };
        if matches!(open.panel.entries[ix], Entry::Submenu(..)) {
            open.panel.focused = Some(ix);
            self.mode = if show_access {
                BarMode::PopupKeyTips
            } else {
                BarMode::Idle
            };
            self.open_submenu(ix, true, ts);
            MenuOutcome {
                handled: true,
                submenu_timer: SubmenuTimer::Cancel,
                dirty: true,
                ..Default::default()
            }
        } else {
            self.mode = BarMode::Idle;
            self.activate((ix, None), ts)
        }
    }

    fn handle_nav_key(
        &mut self,
        ev: KeyEvent,
        bar_has_focus: bool,
        state: MenuState,
        ts: &TextSystem,
    ) -> MenuOutcome {
        if !ev.modifiers.none() {
            return MenuOutcome::default();
        }
        match ev.key {
            Key::Up => self.step_active(-1, bar_has_focus, state, ts),
            Key::Down => self.step_active(1, bar_has_focus, state, ts),
            Key::Home => self.focus_edge(false),
            Key::End => self.focus_edge(true),
            Key::Enter | Key::Space => self.on_confirm(bar_has_focus, state, ts),
            Key::Escape => self.on_cancel(),
            Key::Left => self.on_left(state, ts),
            Key::Right => self.on_right(state, ts),
            _ => MenuOutcome::default(),
        }
    }

    fn focus_edge(&mut self, last: bool) -> MenuOutcome {
        let Some(open) = self.live_open_mut() else {
            return MenuOutcome::handled();
        };
        open.focus_edge(last);
        MenuOutcome::handled_dirty()
    }

    fn step_active(
        &mut self,
        delta: isize,
        bar_has_focus: bool,
        state: MenuState,
        ts: &TextSystem,
    ) -> MenuOutcome {
        let Some(open) = self.live_open_mut() else {
            if delta > 0
                && let Some(kind) = self.bar_focused(bar_has_focus)
            {
                return self.open_by_keyboard(kind, state, false, ts);
            }
            return MenuOutcome::handled();
        };
        open.focus_visible = true;
        if open.in_submenu() {
            if let Some(sub) = open.live_submenu_mut() {
                sub.panel.step(delta);
            }
        } else if open.kind.commands().is_empty() {
            open.panel.step(delta);
        } else {
            open.step_linear(delta);
        }
        MenuOutcome::handled_dirty()
    }

    fn on_confirm(
        &mut self,
        bar_has_focus: bool,
        state: MenuState,
        ts: &TextSystem,
    ) -> MenuOutcome {
        let Some(open) = self.live_open() else {
            if let Some(kind) = self.bar_focused(bar_has_focus) {
                return self.open_by_keyboard(kind, state, false, ts);
            }
            return MenuOutcome::handled();
        };
        if let Some(ix) = open.focused_command {
            return self.click_command(ix);
        }
        let sub = open.live_submenu().and_then(|s| s.panel.focused);
        if let Some(ix) = open.panel.focused {
            return self.activate((ix, sub), ts);
        }
        MenuOutcome::handled()
    }

    fn on_cancel(&mut self) -> MenuOutcome {
        match self.live_open() {
            Some(open) if open.live_submenu().is_some() => {
                self.close_submenu();
                MenuOutcome {
                    handled: true,
                    submenu_timer: SubmenuTimer::Cancel,
                    dirty: true,
                    ..Default::default()
                }
            }
            Some(open) if open.kind.bar_kind().is_some() => self.close_to_bar(),
            Some(_) => self.close(),
            None => {
                self.mode = BarMode::Idle;
                MenuOutcome::with_focus(FocusRequest::RestoreEditor)
            }
        }
    }

    fn on_left(&mut self, state: MenuState, ts: &TextSystem) -> MenuOutcome {
        let Some(open) = self.live_open_mut() else {
            if let BarMode::Focused { kind, key_tips } = self.mode {
                self.mode = BarMode::Focused {
                    kind: kind.cycle(-1),
                    key_tips,
                };
            }
            return MenuOutcome::handled_dirty();
        };
        open.focus_visible = true;
        if !open.kind.commands().is_empty() {
            open.step_command(-1);
            MenuOutcome::handled_dirty()
        } else if open.in_submenu() {
            self.close_submenu();
            MenuOutcome {
                handled: true,
                submenu_timer: SubmenuTimer::Cancel,
                dirty: true,
                ..Default::default()
            }
        } else if let Some(kind) = open.kind.bar_kind() {
            self.open_by_keyboard(kind.cycle(-1), state, false, ts)
        } else {
            MenuOutcome::handled()
        }
    }

    fn on_right(&mut self, state: MenuState, ts: &TextSystem) -> MenuOutcome {
        let Some(open) = self.live_open_mut() else {
            if let BarMode::Focused { kind, key_tips } = self.mode {
                self.mode = BarMode::Focused {
                    kind: kind.cycle(1),
                    key_tips,
                };
            }
            return MenuOutcome::handled_dirty();
        };
        open.focus_visible = true;
        if !open.kind.commands().is_empty() {
            open.step_command(1);
            return MenuOutcome::handled_dirty();
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
                    submenu_timer: SubmenuTimer::Cancel,
                    dirty: true,
                    ..Default::default()
                }
            }
            None => match open.kind.bar_kind() {
                Some(kind) => self.open_by_keyboard(kind.cycle(1), state, false, ts),
                None => MenuOutcome::handled(),
            },
        }
    }

    pub(crate) fn sample(&mut self, ui: &mut UiState) {
        if self.open.as_mut().is_some_and(|open| open.sample(ui)) {
            self.open = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::AppCommand;
    use crate::editor::EditCommand;
    use crate::geom::point;
    use crate::platform::Modifiers;

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

    #[test]
    fn alt_access_keys_submenu_hover_and_system_chords() {
        let ts = TextSystem::new();
        let mut bar = MenuBar::default();
        let state = MenuState {
            can_undo: true,
            has_selection: true,
            has_text: true,
            clipboard_has_text: true,
            status_bar: true,
            word_wrap: true,
            ..Default::default()
        };

        // Alt tap arms and focuses menu bar with key tips.
        assert!(!bar.on_key_down(key(Key::Alt), false, state, &ts).handled);
        let up = bar.on_key_up(key(Key::Alt));
        assert!(up.handled);
        assert_eq!(up.focus, FocusRequest::MenuBar);
        assert!(bar.access_active());

        // Pressing 'V' opens View menu with key tips, Escape returns to bar, second Escape restores editor.
        let open = bar.on_key_down(key(Key::Char('v')), true, state, &ts);
        assert!(open.handled);
        assert_eq!(bar.open_kind(), Some(MenuKind::View));
        assert!(bar.access_active());

        // Hovering row 0 (Zoom submenu) arms submenu timer; timer opens submenu; Enter activates ZoomIn.
        let hover = bar.on_hover_change(Some(HitId::MenuItem { serial: 1, ix: 0 }), state, &ts);
        assert_eq!(hover.submenu_timer, SubmenuTimer::Arm(Some(0)));
        assert!(bar.on_submenu_timer(Some(0), &ts));
        let enter_sub = bar.on_key_down(key(Key::Right), true, state, &ts);
        assert!(enter_sub.handled);
        let act = bar.on_key_down(key(Key::Enter), true, state, &ts);
        assert_eq!(act.command, Some(AppCommand::ZoomIn.into()));
        assert!(!bar.is_open());

        // Alt+F4 cancels open menu and returns handled=false for OS pass-through.
        bar.open_kind_menu(MenuKind::File, state, &ts);
        let alt_f4 = bar.on_key_down(alt_key(Key::F(4)), true, state, &ts);
        assert!(!alt_f4.handled);
        assert!(!bar.is_open());
    }

    #[test]
    fn context_menu_uses_snapshot_and_flyout_restores_slot_focus() {
        let ts = TextSystem::new();
        let mut bar = MenuBar::default();
        let state = MenuState {
            can_undo: false,
            has_selection: false,
            clipboard_has_text: true,
            ..Default::default()
        };

        bar.open_context_menu(point(10., 10.), state, true, &ts);
        assert!(bar.is_open());
        // Cut(0) and Copy(1) disabled; Paste(2) enabled -> focused command must be 2.
        let act = bar.on_key_down(key(Key::Enter), true, state, &ts);
        assert_eq!(act.command, Some(EditCommand::Paste.into()));
        assert_eq!(act.focus, FocusRequest::RestoreEditor);

        bar.open_flyout(
            vec![Entry::toggle("Match case", false, AppCommand::ToggleMatchCase)],
            HitId::FindOptions,
            EditorSlot::FindQuery,
            &ts,
        );
        let closed = bar.close();
        assert_eq!(closed.focus, FocusRequest::Editor(EditorSlot::FindQuery));
    }
}
