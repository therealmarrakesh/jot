use crate::command::Command;
use crate::geom::Point;
use crate::gfx::TextSystem;
use crate::ids::{EditorSlot, Focus, HitId, MenuKind, UiState};
use crate::platform::{Key, KeyEvent, TimerHandle};
use crate::shell::{WindowTimer, WindowTimers};

use super::model::{Entry, MenuState, Panel, context_menu_items};
use super::popup::{Motion, OpenMenu, OpenSubmenu, PopupFocus, PopupKind};

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

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum RowPath {
    Root(usize),
    Sub(usize),
}

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

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum HoveredTarget {
    Item(usize),
    SubmenuItem(usize, usize),
    Command(usize),
}

#[derive(Default)]
pub(super) enum MenuMode {
    #[default]
    Idle,
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
pub(crate) struct MenuBar {
    mode: MenuMode,
    leaving: Option<Box<OpenMenu>>,
    alt_armed: bool,
    hovered_target: Option<HoveredTarget>,
    submenu_timer: Option<TimerHandle>,
}

impl MenuBar {
    pub(crate) fn is_open(&self) -> bool {
        matches!(self.mode, MenuMode::Open { .. })
    }

    pub(crate) fn access_active(&self, focus: Focus) -> bool {
        focus == Focus::MenuBar
            && matches!(
                self.mode,
                MenuMode::Bar { tips: true, .. } | MenuMode::Open { tips: true, .. }
            )
    }

    pub(crate) fn open_kind(&self) -> Option<MenuKind> {
        match &self.mode {
            MenuMode::Open { menu, .. } => menu.kind.bar_kind(),
            MenuMode::Idle | MenuMode::Bar { .. } => None,
        }
    }

    pub(super) fn mode(&self) -> &MenuMode {
        &self.mode
    }

    pub(super) fn open_popup(&self) -> Option<&OpenMenu> {
        match &self.mode {
            MenuMode::Open { menu, .. } => Some(menu),
            MenuMode::Idle | MenuMode::Bar { .. } => self.leaving.as_deref(),
        }
    }

    pub(super) fn bar_focused(&self, focus: Focus) -> Option<MenuKind> {
        if focus != Focus::MenuBar {
            return None;
        }
        match &self.mode {
            MenuMode::Bar { cursor, .. } => Some(*cursor),
            MenuMode::Idle => Some(MenuKind::File),
            MenuMode::Open { .. } => None,
        }
    }

    fn set_tips(&mut self, next: bool) {
        match &mut self.mode {
            MenuMode::Bar { tips, .. } | MenuMode::Open { tips, .. } => *tips = next,
            MenuMode::Idle => {}
        }
    }

    pub(crate) fn pointer_down(&mut self, focus: Focus) -> MenuOutcome {
        self.alt_armed = false;
        match &mut self.mode {
            MenuMode::Open { tips, .. } => {
                let had_tips = std::mem::replace(tips, false);
                if had_tips {
                    MenuOutcome::default().dirty()
                } else {
                    MenuOutcome::default()
                }
            }
            MenuMode::Bar { .. } => {
                self.mode = MenuMode::Idle;
                let req = if focus == Focus::MenuBar {
                    FocusRequest::RestoreEditor
                } else {
                    FocusRequest::Keep
                };
                MenuOutcome::default().dirty().focus(req)
            }
            MenuMode::Idle if focus == Focus::MenuBar => {
                MenuOutcome::default()
                    .dirty()
                    .focus(FocusRequest::RestoreEditor)
            }
            MenuMode::Idle => MenuOutcome::default(),
        }
    }

    pub(crate) fn cancel(&mut self, focus: Focus) -> MenuOutcome {
        self.alt_armed = false;
        if self.is_open() {
            return self.close();
        }
        let had_bar = !matches!(self.mode, MenuMode::Idle);
        self.mode = MenuMode::Idle;
        if focus == Focus::MenuBar {
            MenuOutcome::default()
                .dirty()
                .focus(FocusRequest::RestoreEditor)
        } else if had_bar {
            MenuOutcome::default().dirty()
        } else {
            MenuOutcome::default()
        }
    }

    pub(crate) fn close(&mut self) -> MenuOutcome {
        self.alt_armed = false;
        self.hovered_target = None;
        self.submenu_timer = None;
        let focus = match std::mem::replace(&mut self.mode, MenuMode::Idle) {
            MenuMode::Open { mut menu, .. } => {
                let focus = menu.kind.return_focus();
                menu.motion.close();
                self.leaving = Some(menu);
                focus
            }
            MenuMode::Idle | MenuMode::Bar { .. } => FocusRequest::RestoreEditor,
        };
        MenuOutcome::handled().dirty().focus(focus)
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
        let panel = Panel::new(kind.entries(state), MIN_WIDTH, true, ts);
        self.show(OpenMenu::new(PopupKind::Bar(kind), panel, PopupFocus::None), false)
    }

    fn open_by_keyboard(
        &mut self,
        kind: MenuKind,
        state: MenuState,
        tips: bool,
        ts: &TextSystem,
    ) -> MenuOutcome {
        let panel = Panel::new(kind.entries(state), MIN_WIDTH, true, ts);
        let focus = panel.step(None, 1).map_or(PopupFocus::None, PopupFocus::Row);
        self.show(OpenMenu::new(PopupKind::Bar(kind), panel, focus), tips)
    }

    pub(crate) fn open_context_menu(
        &mut self,
        anchor: Point,
        state: MenuState,
        ts: &TextSystem,
    ) -> MenuOutcome {
        let (commands, entries) = context_menu_items(state);
        let panel = Panel::new(entries, CONTEXT_WIDTH, false, ts);
        let focus = commands
            .iter()
            .position(|b| b.command.enabled)
            .map_or(PopupFocus::None, PopupFocus::Command);
        self.show(
            OpenMenu::new(PopupKind::Context { anchor, commands }, panel, focus),
            false,
        )
    }

    pub(crate) fn open_flyout(
        &mut self,
        entries: Vec<Entry>,
        target: HitId,
        return_slot: EditorSlot,
        ts: &TextSystem,
    ) -> MenuOutcome {
        let panel = Panel::new(entries, FLYOUT_MIN_WIDTH, true, ts);
        self.show(
            OpenMenu::new(
                PopupKind::Flyout {
                    target,
                    return_slot,
                },
                panel,
                PopupFocus::None,
            ),
            false,
        )
    }

    fn show(&mut self, menu: OpenMenu, tips: bool) -> MenuOutcome {
        self.hovered_target = None;
        self.submenu_timer = None;
        self.leaving = None;
        self.mode = MenuMode::Open {
            menu: Box::new(menu),
            tips,
        };
        MenuOutcome::handled().dirty().focus(FocusRequest::MenuBar)
    }

    pub(crate) fn on_hover_change(
        &mut self,
        top: Option<HitId>,
        state: MenuState,
        ts: &TextSystem,
        ui: &mut UiState,
        timers: WindowTimers<'_>,
    ) -> MenuOutcome {
        if let Some(HitId::MenuTrigger(kind)) = top
            && self.open_kind().is_some_and(|open| open != kind)
        {
            ui.mark_pointer_input();
            return self.open_kind_menu(kind, state, ts);
        }

        let MenuMode::Open { menu, .. } = &mut self.mode else {
            self.hovered_target = None;
            return MenuOutcome::default();
        };
        let hovered = match top {
            Some(HitId::MenuItem { ix, .. }) => Some(HoveredTarget::Item(ix)),
            Some(HitId::SubmenuItem { ix: sub_ix, .. }) => menu
                .submenu
                .as_ref()
                .map(|s| HoveredTarget::SubmenuItem(s.ix, sub_ix)),
            Some(HitId::MenuCommand { ix, .. }) => Some(HoveredTarget::Command(ix)),
            _ => None,
        };
        if hovered == self.hovered_target {
            return MenuOutcome::default();
        }
        let Some(target) = hovered else {
            self.hovered_target = None;
            self.submenu_timer = None;
            return MenuOutcome::default();
        };

        let timer_target = match target {
            HoveredTarget::Item(ix) => {
                let current = menu.submenu.as_ref().map(|s| s.ix);
                match (menu.panel.entries.get(ix), current) {
                    (Some(Entry::Submenu(..)), Some(open_ix)) if open_ix == ix => None,
                    (Some(Entry::Submenu(..)), _) => Some(Some(ix)),
                    (_, Some(_)) => Some(None),
                    (_, None) => None,
                }
            }
            HoveredTarget::SubmenuItem(..) | HoveredTarget::Command(_) => None,
        };
        match target {
            HoveredTarget::Item(ix) => {
                if menu.panel.entries.get(ix).is_some_and(Entry::selectable) {
                    menu.focus = PopupFocus::Row(ix);
                }
            }
            HoveredTarget::SubmenuItem(_, sub_ix) => {
                if let Some(sub) = &menu.submenu
                    && sub.panel.entries.get(sub_ix).is_some_and(Entry::selectable)
                {
                    menu.focus = PopupFocus::SubRow(sub_ix);
                }
            }
            HoveredTarget::Command(ix) => {
                if menu
                    .kind
                    .commands()
                    .get(ix)
                    .is_some_and(|b| b.command.enabled)
                {
                    menu.focus = PopupFocus::Command(ix);
                }
            }
        }
        self.hovered_target = hovered;
        ui.mark_pointer_input();
        self.submenu_timer = timer_target.map(|t| {
            timers.after(timers.menu_show_delay(), WindowTimer::Submenu(t))
        });
        MenuOutcome::default().dirty()
    }

    pub(crate) fn on_submenu_timer(&mut self, target: Option<usize>, ts: &TextSystem) -> bool {
        match target {
            Some(ix) => self.open_submenu(ix, false, ts),
            None => self.close_submenu(),
        }
    }

    pub(crate) fn click_command(&mut self, ix: usize) -> MenuOutcome {
        let MenuMode::Open { menu, .. } = &self.mode else {
            return MenuOutcome::default();
        };
        let Some(cmd) = menu
            .kind
            .commands()
            .get(ix)
            .map(|b| b.command)
            .filter(|c| c.enabled)
        else {
            return MenuOutcome::default();
        };
        self.close().command(Some(cmd.command))
    }

    pub(crate) fn activate(
        &mut self,
        path: RowPath,
        focus_first: bool,
        ts: &TextSystem,
    ) -> MenuOutcome {
        let MenuMode::Open { menu, .. } = &self.mode else {
            return MenuOutcome::default();
        };
        let entry = match path {
            RowPath::Root(ix) => menu.panel.entries.get(ix).cloned(),
            RowPath::Sub(sub_ix) => menu
                .submenu
                .as_ref()
                .and_then(|s| s.panel.entries.get(sub_ix).cloned()),
        };
        let Some(entry) = entry else {
            return MenuOutcome::default();
        };
        match (path, entry) {
            (RowPath::Root(ix), Entry::Submenu(..)) => {
                self.open_submenu(ix, focus_first, ts);
                MenuOutcome::default().dirty()
            }
            (_, entry) if !entry.selectable() => MenuOutcome::default(),
            (_, entry) => self.close().command(entry.command_id()),
        }
    }

    fn open_submenu(&mut self, ix: usize, focus_first: bool, ts: &TextSystem) -> bool {
        self.submenu_timer = None;
        let MenuMode::Open { menu, .. } = &mut self.mode else {
            return false;
        };
        let Some(Entry::Submenu(_, entries)) = menu.panel.entries.get(ix) else {
            return false;
        };
        if menu.submenu.as_ref().is_none_or(|sub| sub.ix != ix) {
            let panel = Panel::new(entries.clone(), MIN_WIDTH, true, ts);
            menu.set_submenu(OpenSubmenu {
                ix,
                panel,
                motion: Motion::new(false),
            });
        }
        menu.focus = if focus_first {
            menu.submenu
                .as_ref()
                .and_then(|sub| sub.panel.step(None, 1))
                .map_or(PopupFocus::Row(ix), PopupFocus::SubRow)
        } else {
            PopupFocus::Row(ix)
        };
        true
    }

    fn close_submenu(&mut self) -> bool {
        self.submenu_timer = None;
        match &mut self.mode {
            MenuMode::Open { menu, .. } => menu.close_submenu(),
            MenuMode::Idle | MenuMode::Bar { .. } => false,
        }
    }

    pub(crate) fn on_key_up(&mut self, ev: KeyEvent, focus: Focus) -> MenuOutcome {
        if ev.key == Key::Alt && self.alt_armed {
            self.alt_armed = false;
            return self.toggle_access(focus);
        }
        MenuOutcome::default()
    }

    pub(crate) fn on_key_down(
        &mut self,
        ev: KeyEvent,
        focus: Focus,
        state: MenuState,
        ts: &TextSystem,
    ) -> MenuOutcome {
        let m = ev.modifiers;
        if m.alt && !m.ctrl && !m.win && matches!(ev.key, Key::F(4) | Key::Space) {
            let mut out = self.cancel(focus);
            out.handled = false;
            return out;
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
            if self.is_open() {
                return self.close();
            }
            return self.toggle_access(focus);
        }

        let access_ch = match ev.key {
            Key::Char(c) if c.is_ascii_alphanumeric() => Some(c.to_ascii_uppercase()),
            _ => None,
        };

        if !self.access_active(focus) {
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
            if self.is_open() {
                if let Some(ch) = access_ch
                    && m.none()
                {
                    let out = self.invoke_access(ch, false, state, ts);
                    if out.handled {
                        return out;
                    }
                }
                let nav = self.handle_nav_key(ev, focus, state, ts);
                return if nav.handled {
                    nav
                } else {
                    MenuOutcome::handled()
                };
            }
            if focus == Focus::MenuBar {
                return self.handle_nav_key(ev, focus, state, ts);
            }
            return MenuOutcome::default();
        }

        match ev.key {
            Key::Escape if m.none() => self.on_cancel(),
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
                self.handle_nav_key(ev, focus, state, ts).dirty()
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
                    MenuOutcome::default()
                } else {
                    MenuOutcome::handled()
                }
            }
        }
    }

    fn toggle_access(&mut self, focus: Focus) -> MenuOutcome {
        if self.access_active(focus) {
            return match &mut self.mode {
                MenuMode::Open { tips, .. } => {
                    *tips = false;
                    MenuOutcome::handled().dirty()
                }
                MenuMode::Bar { .. } | MenuMode::Idle => {
                    self.mode = MenuMode::Idle;
                    MenuOutcome::handled()
                        .dirty()
                        .focus(FocusRequest::RestoreEditor)
                }
            };
        }
        match &mut self.mode {
            MenuMode::Open { menu, .. } if menu.kind.bar_kind().is_none() => MenuOutcome::handled(),
            MenuMode::Open { tips, .. } => {
                *tips = true;
                MenuOutcome::handled().dirty()
            }
            MenuMode::Bar { cursor, tips } => {
                if focus != Focus::MenuBar {
                    *cursor = MenuKind::File;
                }
                *tips = true;
                MenuOutcome::handled().dirty().focus(FocusRequest::MenuBar)
            }
            MenuMode::Idle => {
                self.mode = MenuMode::Bar {
                    cursor: MenuKind::File,
                    tips: true,
                };
                MenuOutcome::handled().dirty().focus(FocusRequest::MenuBar)
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
        let MenuMode::Open { menu, .. } = &mut self.mode else {
            let Some(kind) = MenuKind::ALL
                .into_iter()
                .find(|k| k.access_key() == Some(key))
            else {
                return MenuOutcome::default();
            };
            return self.open_by_keyboard(kind, state, show_access, ts);
        };
        if menu.kind.bar_kind().is_none() {
            return MenuOutcome::default();
        }
        if let Some(sub) = &menu.submenu {
            let Some(sub_ix) = sub.panel.find_key(key) else {
                return MenuOutcome::default();
            };
            self.set_tips(false);
            return self.activate(RowPath::Sub(sub_ix), true, ts);
        }
        let Some(ix) = menu.panel.find_key(key) else {
            return MenuOutcome::default();
        };
        if matches!(menu.panel.entries[ix], Entry::Submenu(..)) {
            self.set_tips(show_access);
            self.open_submenu(ix, true, ts);
            MenuOutcome::handled().dirty()
        } else {
            self.set_tips(false);
            self.activate(RowPath::Root(ix), true, ts)
        }
    }

    fn handle_nav_key(
        &mut self,
        ev: KeyEvent,
        focus: Focus,
        state: MenuState,
        ts: &TextSystem,
    ) -> MenuOutcome {
        if !ev.modifiers.none() {
            return MenuOutcome::default();
        }
        match ev.key {
            Key::Up => self.step_active(-1, focus, state, ts),
            Key::Down => self.step_active(1, focus, state, ts),
            Key::Home => self.focus_edge(false),
            Key::End => self.focus_edge(true),
            Key::Enter | Key::Space => self.on_confirm(focus, state, ts),
            Key::Escape => self.on_cancel(),
            Key::Left => self.on_left(state, ts),
            Key::Right => self.on_right(state, ts),
            _ => MenuOutcome::default(),
        }
    }

    fn focus_edge(&mut self, last: bool) -> MenuOutcome {
        let MenuMode::Open { menu, .. } = &mut self.mode else {
            return MenuOutcome::handled();
        };
        menu.focus_edge(last);
        MenuOutcome::handled().dirty()
    }

    fn step_active(
        &mut self,
        delta: isize,
        focus: Focus,
        state: MenuState,
        ts: &TextSystem,
    ) -> MenuOutcome {
        let MenuMode::Open { menu, .. } = &mut self.mode else {
            if delta > 0
                && let Some(kind) = self.bar_focused(focus)
            {
                return self.open_by_keyboard(kind, state, false, ts);
            }
            return MenuOutcome::handled();
        };
        menu.step(delta);
        MenuOutcome::handled().dirty()
    }

    fn on_confirm(
        &mut self,
        focus: Focus,
        state: MenuState,
        ts: &TextSystem,
    ) -> MenuOutcome {
        let MenuMode::Open { menu, .. } = &self.mode else {
            if let Some(kind) = self.bar_focused(focus) {
                return self.open_by_keyboard(kind, state, false, ts);
            }
            return MenuOutcome::handled();
        };
        match menu.focus {
            PopupFocus::Command(ix) => self.click_command(ix),
            PopupFocus::Row(ix) => self.activate(RowPath::Root(ix), true, ts),
            PopupFocus::SubRow(ix) => self.activate(RowPath::Sub(ix), true, ts),
            PopupFocus::None => MenuOutcome::handled(),
        }
    }

    fn on_cancel(&mut self) -> MenuOutcome {
        match &self.mode {
            MenuMode::Open { menu, .. } if menu.submenu.is_some() => {
                self.close_submenu();
                MenuOutcome::handled().dirty()
            }
            MenuMode::Open { menu, .. } if menu.kind.bar_kind().is_some() => {
                let MenuMode::Open { mut menu, tips } =
                    std::mem::replace(&mut self.mode, MenuMode::Idle)
                else {
                    unreachable!()
                };
                let cursor = menu.kind.bar_kind().unwrap();
                menu.motion.close();
                self.leaving = Some(menu);
                self.hovered_target = None;
                self.submenu_timer = None;
                self.mode = MenuMode::Bar { cursor, tips };
                MenuOutcome::handled().dirty().focus(FocusRequest::MenuBar)
            }
            MenuMode::Open { .. } => self.close(),
            MenuMode::Bar { .. } | MenuMode::Idle => {
                self.mode = MenuMode::Idle;
                MenuOutcome::handled()
                    .dirty()
                    .focus(FocusRequest::RestoreEditor)
            }
        }
    }

    fn cycle_bar(&mut self, delta: isize) -> MenuOutcome {
        match &mut self.mode {
            MenuMode::Bar { cursor, .. } => *cursor = cursor.cycle(delta),
            MenuMode::Idle => {
                self.mode = MenuMode::Bar {
                    cursor: MenuKind::File.cycle(delta),
                    tips: false,
                };
            }
            MenuMode::Open { .. } => {}
        }
        MenuOutcome::handled().dirty()
    }

    fn on_left(&mut self, state: MenuState, ts: &TextSystem) -> MenuOutcome {
        let MenuMode::Open { menu, .. } = &mut self.mode else {
            return self.cycle_bar(-1);
        };
        if !menu.kind.commands().is_empty() {
            menu.step_command(-1);
            MenuOutcome::handled().dirty()
        } else if menu.in_submenu() {
            self.close_submenu();
            MenuOutcome::handled().dirty()
        } else if let Some(kind) = menu.kind.bar_kind() {
            self.open_by_keyboard(kind.cycle(-1), state, false, ts)
        } else {
            MenuOutcome::handled()
        }
    }

    fn on_right(&mut self, state: MenuState, ts: &TextSystem) -> MenuOutcome {
        let MenuMode::Open { menu, .. } = &mut self.mode else {
            return self.cycle_bar(1);
        };
        if !menu.kind.commands().is_empty() {
            menu.step_command(1);
            return MenuOutcome::handled().dirty();
        }
        let submenu_row = match menu.focus {
            PopupFocus::Row(ix)
                if menu
                    .panel
                    .entries
                    .get(ix)
                    .is_some_and(|e| matches!(e, Entry::Submenu(..))) =>
            {
                Some(ix)
            }
            _ => None,
        };
        match submenu_row {
            Some(ix) => {
                self.open_submenu(ix, true, ts);
                MenuOutcome::handled().dirty()
            }
            None => match menu.kind.bar_kind() {
                Some(kind) => self.open_by_keyboard(kind.cycle(1), state, false, ts),
                None => MenuOutcome::handled(),
            },
        }
    }

    pub(crate) fn sample(&mut self, ui: &mut UiState) {
        if let MenuMode::Open { menu, .. } = &mut self.mode {
            menu.sample(ui);
        }
        if self.leaving.as_mut().is_some_and(|menu| menu.sample(ui)) {
            self.leaving = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    use crate::command::AppCommand;
    use crate::editor::EditCommand;
    use crate::geom::point;
    use crate::ids::TabId;
    use crate::platform::{Jobs, Modifiers};
    use crate::shell::AppEvent;

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
        let jobs = Jobs::new();
        let timers = WindowTimers::for_test(&jobs);
        let mut ui = UiState::default();
        let mut bar = MenuBar::default();
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

        // Alt tap arms and focuses menu bar with key tips.
        assert!(!bar.on_key_down(key(Key::Alt), ed_focus, state, &ts).handled);
        let up = bar.on_key_up(key(Key::Alt), ed_focus);
        assert!(up.handled);
        assert_eq!(up.focus, FocusRequest::MenuBar);
        assert!(bar.access_active(Focus::MenuBar));

        // Pressing 'V' opens View menu with key tips, Escape returns to bar, second Escape restores editor.
        let open = bar.on_key_down(key(Key::Char('v')), Focus::MenuBar, state, &ts);
        assert!(open.handled);
        assert_eq!(bar.open_kind(), Some(MenuKind::View));
        assert!(bar.access_active(Focus::MenuBar));

        // Hovering row 0 (Zoom submenu) arms submenu timer; timer opens submenu; Enter activates ZoomIn.
        let _ = bar.on_hover_change(
            Some(HitId::MenuItem { serial: 1, ix: 0 }),
            state,
            &ts,
            &mut ui,
            timers,
        );
        assert!(matches!(
            jobs.pop_at(Instant::now() + Duration::from_secs(5)),
            Some(AppEvent::Window {
                timer: WindowTimer::Submenu(Some(0)),
                ..
            })
        ));
        assert!(bar.on_submenu_timer(Some(0), &ts));
        let enter_sub = bar.on_key_down(key(Key::Right), Focus::MenuBar, state, &ts);
        assert!(enter_sub.handled);
        let act = bar.on_key_down(key(Key::Enter), Focus::MenuBar, state, &ts);
        assert_eq!(act.command, Some(AppCommand::ZoomIn.into()));
        assert!(!bar.is_open());

        // Alt+F4 cancels open menu and returns handled=false for OS pass-through.
        bar.open_kind_menu(MenuKind::File, state, &ts);
        let alt_f4 = bar.on_key_down(alt_key(Key::F(4)), Focus::MenuBar, state, &ts);
        assert!(!alt_f4.handled);
        assert!(!bar.is_open());
    }

    #[test]
    fn context_menu_uses_snapshot_and_flyout_restores_slot_focus() {
        let ts = TextSystem::new();
        let jobs = Jobs::new();
        let timers = WindowTimers::for_test(&jobs);
        let mut ui = UiState::default();
        let mut bar = MenuBar::default();
        let state = MenuState {
            can_undo: false,
            has_selection: false,
            clipboard_has_text: true,
            ..Default::default()
        };

        ui.mark_keyboard_input();
        bar.open_context_menu(point(10., 10.), state, &ts);
        assert!(bar.is_open());
        // Hovering a context menu command clears keyboard focus modality on UiState.
        let _ = bar.on_hover_change(
            Some(HitId::MenuCommand { serial: 1, ix: 2 }),
            state,
            &ts,
            &mut ui,
            timers,
        );
        assert!(!ui.last_input_was_keyboard());
        // Cut(0) and Copy(1) disabled; Paste(2) enabled -> focused command must be 2.
        let act = bar.on_key_down(key(Key::Enter), Focus::MenuBar, state, &ts);
        assert_eq!(act.command, Some(EditCommand::Paste.into()));
        assert_eq!(act.focus, FocusRequest::RestoreEditor);

        // Mouse-clicking a submenu item opens the submenu without pre-selecting its first child.
        bar.open_kind_menu(MenuKind::View, state, &ts);
        let _ = bar.activate(RowPath::Root(0), false, &ts);
        assert!(bar.open_popup().is_some_and(|o| o.submenu.is_some() && !o.in_submenu()));

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
