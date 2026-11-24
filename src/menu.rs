use std::time::Duration;

use crate::command::{AppCommand, Command, shortcut_label};
use crate::editor::EditCommand;
use crate::geom::{Edges, Point, Rect, Size, point, transparent_black};
use crate::gfx::TextSystem;
use crate::platform::{CursorStyle, Key, KeyEvent, TimerHandle};
use crate::shell::{WindowTimer, WindowTimers};
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
enum MenuMode {
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
pub(crate) struct Menus {
    mode: MenuMode,
    leaving: Option<Box<OpenMenu>>,
    alt_armed: bool,
    hovered_target: Option<HoveredTarget>,
    submenu_timer: Option<TimerHandle>,
}

impl Menus {
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

    fn mode(&self) -> &MenuMode {
        &self.mode
    }

    fn open_popup(&self) -> Option<&OpenMenu> {
        match &self.mode {
            MenuMode::Open { menu, .. } => Some(menu),
            MenuMode::Idle | MenuMode::Bar { .. } => self.leaving.as_deref(),
        }
    }

    fn bar_focused(&self, focus: Focus) -> Option<MenuKind> {
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

    pub(crate) fn toggle_trigger(&mut self, kind: MenuKind, state: MenuState) -> MenuOutcome {
        if self.open_kind() == Some(kind) {
            self.close()
        } else {
            self.open_kind_menu(kind, state)
        }
    }

    pub(crate) fn open_kind_menu(&mut self, kind: MenuKind, state: MenuState) -> MenuOutcome {
        let panel = Panel::new(kind.entries(state), MIN_WIDTH, true);
        self.show(OpenMenu::new(PopupKind::Bar(kind), panel, PopupFocus::None), false)
    }

    fn open_by_keyboard(&mut self, kind: MenuKind, state: MenuState, tips: bool) -> MenuOutcome {
        let panel = Panel::new(kind.entries(state), MIN_WIDTH, true);
        let focus = panel.step(None, 1).map_or(PopupFocus::None, PopupFocus::Row);
        self.show(OpenMenu::new(PopupKind::Bar(kind), panel, focus), tips)
    }

    pub(crate) fn open_context_menu(&mut self, anchor: Point, state: MenuState) -> MenuOutcome {
        let (commands, entries) = context_menu_items(state);
        let panel = Panel::new(entries, CONTEXT_WIDTH, false);
        let focus = commands
            .iter()
            .position(|b| b.command.enabled)
            .map_or(PopupFocus::None, PopupFocus::Command);
        self.show(
            OpenMenu::new(PopupKind::Context { anchor, commands }, panel, focus),
            false,
        )
    }

    pub(crate) fn open_find_options(
        &mut self,
        match_case: bool,
        wrap_around: bool,
    ) -> MenuOutcome {
        let entries = vec![
            Entry::toggle("Match case", match_case, AppCommand::ToggleMatchCase),
            Entry::toggle("Wrap around", wrap_around, AppCommand::ToggleWrapAround),
        ];
        self.open_flyout(
            entries,
            HitId::Find(FindHit::Options),
            EditorSlot::FindQuery,
        )
    }

    fn open_flyout(
        &mut self,
        entries: Vec<Entry>,
        target: HitId,
        return_slot: EditorSlot,
    ) -> MenuOutcome {
        let panel = Panel::new(entries, FLYOUT_MIN_WIDTH, true);
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
        state: impl FnOnce() -> MenuState,
        ui: &mut AppUi,
        timers: WindowTimers<'_>,
    ) -> MenuOutcome {
        if let Some(HitId::Menu(MenuHit::Trigger(kind))) = top
            && self.open_kind().is_some_and(|open| open != kind)
        {
            ui.last_input_was_keyboard = false;
            return self.open_kind_menu(kind, state());
        }

        let MenuMode::Open { menu, .. } = &mut self.mode else {
            self.hovered_target = None;
            return MenuOutcome::default();
        };
        let hovered = match top {
            Some(HitId::Menu(MenuHit::Item { ix, .. })) => Some(HoveredTarget::Item(ix)),
            Some(HitId::Menu(MenuHit::SubmenuItem { ix: sub_ix, .. })) => menu
                .submenu
                .as_ref()
                .map(|s| HoveredTarget::SubmenuItem(s.ix, sub_ix)),
            Some(HitId::Menu(MenuHit::Command { ix, .. })) => Some(HoveredTarget::Command(ix)),
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
        ui.last_input_was_keyboard = false;
        self.submenu_timer = timer_target.map(|t| {
            timers.after(timers.menu_show_delay(), WindowTimer::Submenu(t))
        });
        MenuOutcome::default().dirty()
    }

    pub(crate) fn on_submenu_timer(&mut self, target: Option<usize>) -> bool {
        match target {
            Some(ix) => self.open_submenu(ix, false),
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

    pub(crate) fn activate(&mut self, path: RowPath, focus_first: bool) -> MenuOutcome {
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
                self.open_submenu(ix, focus_first);
                MenuOutcome::default().dirty()
            }
            (_, entry) if !entry.selectable() => MenuOutcome::default(),
            (_, entry) => self.close().command(entry.command_id()),
        }
    }

    fn open_submenu(&mut self, ix: usize, focus_first: bool) -> bool {
        self.submenu_timer = None;
        let MenuMode::Open { menu, .. } = &mut self.mode else {
            return false;
        };
        let Some(Entry::Submenu(_, entries)) = menu.panel.entries.get(ix) else {
            return false;
        };
        if menu.submenu.as_ref().is_none_or(|sub| sub.ix != ix) {
            let panel = Panel::new(entries.clone(), MIN_WIDTH, true);
            menu.set_submenu(OpenSubmenu {
                ix,
                panel,
                motion: PopupTransition::new(false),
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
        state: impl FnOnce() -> MenuState,
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

        let mut state = Some(state);
        if !self.access_active(focus) {
            if let Some(ch) = access_ch
                && m.alt
                && !m.ctrl
                && !m.win
                && !m.alt_gr
            {
                let out = self.invoke_access(ch, true, || (state.take().unwrap())());
                if out.handled {
                    return out;
                }
            }
            if self.is_open() {
                if let Some(ch) = access_ch
                    && m.none()
                {
                    let out = self.invoke_access(ch, false, || (state.take().unwrap())());
                    if out.handled {
                        return out;
                    }
                }
                let nav = self.handle_nav_key(ev, focus, || (state.take().unwrap())());
                return if nav.handled {
                    nav
                } else {
                    MenuOutcome::handled()
                };
            }
            if focus == Focus::MenuBar {
                return self.handle_nav_key(ev, focus, || (state.take().unwrap())());
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
                self.handle_nav_key(ev, focus, || (state.take().unwrap())())
                    .dirty()
            }
            _ => {
                if let Some(ch) = access_ch
                    && !m.ctrl
                    && !m.win
                    && !m.alt_gr
                {
                    let out = self.invoke_access(ch, true, || (state.take().unwrap())());
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
        state: impl FnOnce() -> MenuState,
    ) -> MenuOutcome {
        let MenuMode::Open { menu, .. } = &mut self.mode else {
            let Some(kind) = MenuKind::ALL
                .into_iter()
                .find(|k| k.access_key() == Some(key))
            else {
                return MenuOutcome::default();
            };
            return self.open_by_keyboard(kind, state(), show_access);
        };
        if menu.kind.bar_kind().is_none() {
            return MenuOutcome::default();
        }
        if let Some(sub) = &menu.submenu {
            let Some(sub_ix) = sub.panel.find_key(key) else {
                return MenuOutcome::default();
            };
            self.set_tips(false);
            return self.activate(RowPath::Sub(sub_ix), true);
        }
        let Some(ix) = menu.panel.find_key(key) else {
            return MenuOutcome::default();
        };
        if matches!(menu.panel.entries[ix], Entry::Submenu(..)) {
            self.set_tips(show_access);
            self.open_submenu(ix, true);
            MenuOutcome::handled().dirty()
        } else {
            self.set_tips(false);
            self.activate(RowPath::Root(ix), true)
        }
    }

    fn handle_nav_key(
        &mut self,
        ev: KeyEvent,
        focus: Focus,
        state: impl FnOnce() -> MenuState,
    ) -> MenuOutcome {
        if !ev.modifiers.none() {
            return MenuOutcome::default();
        }
        match ev.key {
            Key::Up => self.step_active(-1, focus, state),
            Key::Down => self.step_active(1, focus, state),
            Key::Home => self.focus_edge(false),
            Key::End => self.focus_edge(true),
            Key::Enter | Key::Space => self.on_confirm(focus, state),
            Key::Escape => self.on_cancel(),
            Key::Left => self.on_left(state),
            Key::Right => self.on_right(state),
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
        state: impl FnOnce() -> MenuState,
    ) -> MenuOutcome {
        let MenuMode::Open { menu, .. } = &mut self.mode else {
            if delta > 0
                && let Some(kind) = self.bar_focused(focus)
            {
                return self.open_by_keyboard(kind, state(), false);
            }
            return MenuOutcome::handled();
        };
        menu.step(delta);
        MenuOutcome::handled().dirty()
    }

    fn on_confirm(&mut self, focus: Focus, state: impl FnOnce() -> MenuState) -> MenuOutcome {
        let MenuMode::Open { menu, .. } = &self.mode else {
            if let Some(kind) = self.bar_focused(focus) {
                return self.open_by_keyboard(kind, state(), false);
            }
            return MenuOutcome::handled();
        };
        match menu.focus {
            PopupFocus::Command(ix) => self.click_command(ix),
            PopupFocus::Row(ix) => self.activate(RowPath::Root(ix), true),
            PopupFocus::SubRow(ix) => self.activate(RowPath::Sub(ix), true),
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

    fn on_left(&mut self, state: impl FnOnce() -> MenuState) -> MenuOutcome {
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
            self.open_by_keyboard(kind.cycle(-1), state(), false)
        } else {
            MenuOutcome::handled()
        }
    }

    fn on_right(&mut self, state: impl FnOnce() -> MenuState) -> MenuOutcome {
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
                self.open_submenu(ix, true);
                MenuOutcome::handled().dirty()
            }
            None => match menu.kind.bar_kind() {
                Some(kind) => self.open_by_keyboard(kind.cycle(1), state(), false),
                None => MenuOutcome::handled(),
            },
        }
    }

    pub(crate) fn sample(&mut self, ui: &mut AppUi) {
        if let MenuMode::Open { menu, .. } = &mut self.mode {
            menu.sample(ui);
        }
        if self.leaving.as_mut().is_some_and(|menu| menu.sample(ui)) {
            self.leaving = None;
        }
    }

    #[cfg(test)]
    pub(crate) fn hit_for_command(&self, cmd: Command) -> Option<HitId> {
        let MenuMode::Open { menu, .. } = &self.mode else {
            return None;
        };
        if let Some(sub) = &menu.submenu
            && let Some(ix) = sub
                .panel
                .entries
                .iter()
                .position(|e| e.command_id() == Some(cmd))
        {
            return Some(HitId::Menu(MenuHit::SubmenuItem {
                serial: sub.motion.serial(),
                ix,
            }));
        }
        if let Some(ix) = menu
            .kind
            .commands()
            .iter()
            .position(|b| b.command.command == cmd)
        {
            return Some(HitId::Menu(MenuHit::Command {
                serial: menu.motion.serial(),
                ix,
            }));
        }
        menu.panel
            .entries
            .iter()
            .position(|e| e.command_id() == Some(cmd))
            .map(|ix| {
                HitId::Menu(MenuHit::Item {
                    serial: menu.motion.serial(),
                    ix,
                })
            })
    }

    #[cfg(test)]
    pub(crate) fn hit_for_submenu(&self, title: &str) -> Option<HitId> {
        let MenuMode::Open { menu, .. } = &self.mode else {
            return None;
        };
        menu.panel
            .entries
            .iter()
            .enumerate()
            .find_map(|(ix, e)| {
                (matches!(e, Entry::Submenu(..)) && menu.panel.labels[ix].as_ref() == title)
                    .then_some(HitId::Menu(MenuHit::Item {
                        serial: menu.motion.serial(),
                        ix,
                    }))
            })
    }
}

const BORDER: f32 = 1.;
const PADDING_Y: f32 = 2.;
const ROW_INSET: f32 = 4.;
const ROW_PADDING: f32 = 11.;
const ROW_ICON: f32 = 40.;
const ROW_COMMAND: f32 = 33.;
const ROW_COMPACT: f32 = 32.;
const ROW_SEPARATOR: f32 = 3.;
const CHECK_COLUMN: f32 = 28.;
const SHORTCUT_GAP: f32 = 24.;
const CHEVRON_WIDTH: f32 = 12.;
const LABEL_SIZE: f32 = 14.;
const SHORTCUT_SIZE: f32 = 12.;

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

impl MenuKind {
    fn access_key(self) -> Option<char> {
        self.title().chars().next()
    }

    fn cycle(self, delta: isize) -> Self {
        cycle(Some(self as usize), Self::ALL.len(), delta, true, |_| true)
            .map_or(self, |ix| Self::ALL[ix])
    }

    fn entries(self, state: MenuState) -> Vec<Entry> {
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

fn context_menu_items(state: MenuState) -> (Vec<CommandButton>, Vec<Entry>) {
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
    min_width: f32,
    checks: bool,
}

impl Panel {
    fn new(entries: Vec<Entry>, min_width: f32, show_shortcuts: bool) -> Self {
        let labels = entries
            .iter()
            .map(|e| e.label().map(display_label).unwrap_or_default())
            .collect();
        let shortcuts = entries
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
        Self {
            entries,
            labels,
            shortcuts,
            min_width,
            checks,
        }
    }

    fn find_key(&self, key: char) -> Option<usize> {
        self.entries
            .iter()
            .position(|entry| entry.access_key() == Some(key))
    }

    fn step(&self, current: Option<usize>, delta: isize) -> Option<usize> {
        cycle(current, self.entries.len(), delta, true, |ix| {
            self.entries[ix].selectable()
        })
    }

    fn edge(&self, last: bool) -> Option<usize> {
        if last {
            self.entries.iter().rposition(Entry::selectable)
        } else {
            self.entries.iter().position(Entry::selectable)
        }
    }
}

const OPEN_CURVE: Curve = Curve::new(Duration::from_millis(250), Easing::DECELERATE);
const FADE_CURVE: Curve = Curve::new(Duration::from_millis(83), Easing::Linear);
const FLYOUT_GAP: f32 = 3.;
const SUBMENU_OVERLAP: f32 = 4.;
const COMMAND_ROW: f32 = 59.;

fn place_below(r: Rect, _popup: Size, _vp: Size) -> Point {
    point(r.left(), r.bottom())
}

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

enum PopupKind {
    Bar(MenuKind),
    Context {
        anchor: Point,
        commands: Vec<CommandButton>,
    },
    Flyout {
        target: HitId,
        return_slot: EditorSlot,
    },
}

impl PopupKind {
    fn bar_kind(&self) -> Option<MenuKind> {
        match self {
            Self::Bar(kind) => Some(*kind),
            Self::Context { .. } | Self::Flyout { .. } => None,
        }
    }

    fn commands(&self) -> &[CommandButton] {
        match self {
            Self::Context { commands, .. } => commands,
            Self::Bar(_) | Self::Flyout { .. } => &[],
        }
    }

    fn anchor(&self) -> Anchor<HitId> {
        match self {
            Self::Bar(kind) => Anchor::Target {
                target: HitId::Menu(MenuHit::Trigger(*kind)),
                place: place_below,
            },
            Self::Context { anchor, .. } => Anchor::Point(*anchor),
            Self::Flyout { target, .. } => Anchor::Target {
                target: *target,
                place: place_flyout,
            },
        }
    }

    fn return_focus(&self) -> FocusRequest {
        match self {
            Self::Flyout { return_slot, .. } => FocusRequest::Editor(*return_slot),
            Self::Bar(_) | Self::Context { .. } => FocusRequest::RestoreEditor,
        }
    }
}

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

    #[inline]
    fn serial(&self) -> usize {
        self.transition.serial()
    }

    #[inline]
    fn is_closing(&self) -> bool {
        self.transition.is_closing()
    }

    fn close(&mut self) {
        self.transition.close();
    }

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
        let fade = ui.progress(AnimKey::Transition { serial, track: 2 }, closing, FADE_CURVE);
        self.opacity = (1. - fade.value) * appear;
        closing && fade.status == AnimStatus::Finished
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
enum PopupFocus {
    #[default]
    None,
    Command(usize),
    Row(usize),
    SubRow(usize),
}

struct OpenSubmenu {
    ix: usize,
    panel: Panel,
    motion: PopupTransition,
}

struct OpenMenu {
    kind: PopupKind,
    panel: Panel,
    submenu: Option<OpenSubmenu>,
    leaving_submenu: Option<OpenSubmenu>,
    motion: PopupTransition,
    focus: PopupFocus,
}

impl OpenMenu {
    fn new(kind: PopupKind, panel: Panel, focus: PopupFocus) -> Self {
        let fade_in = !kind.commands().is_empty();
        Self {
            kind,
            panel,
            submenu: None,
            leaving_submenu: None,
            motion: PopupTransition::new(fade_in),
            focus,
        }
    }

    fn is_closing(&self) -> bool {
        self.motion.is_closing()
    }

    fn visible_submenu(&self) -> Option<&OpenSubmenu> {
        self.submenu.as_ref().or(self.leaving_submenu.as_ref())
    }

    fn focused_submenu(&self) -> Option<&OpenSubmenu> {
        matches!(self.focus, PopupFocus::SubRow(_))
            .then_some(self.submenu.as_ref())
            .flatten()
    }

    fn in_submenu(&self) -> bool {
        self.focused_submenu().is_some()
    }

    fn set_submenu(&mut self, sub: OpenSubmenu) {
        self.submenu = Some(sub);
        self.leaving_submenu = None;
    }

    fn close_submenu(&mut self) -> bool {
        let Some(mut sub) = self.submenu.take() else {
            return false;
        };
        if matches!(self.focus, PopupFocus::SubRow(_)) {
            self.focus = PopupFocus::Row(sub.ix);
        }
        sub.motion.close();
        self.leaving_submenu = Some(sub);
        true
    }

    fn step(&mut self, delta: isize) {
        if let PopupFocus::SubRow(ix) = self.focus
            && let Some(sub) = &self.submenu
        {
            if let Some(next) = sub.panel.step(Some(ix), delta) {
                self.focus = PopupFocus::SubRow(next);
            }
            return;
        }
        let cmds = self.kind.commands();
        if cmds.is_empty() {
            let current = match self.focus {
                PopupFocus::Row(ix) => Some(ix),
                _ => None,
            };
            if let Some(next) = self.panel.step(current, delta) {
                self.focus = PopupFocus::Row(next);
            }
            return;
        }
        let commands = cmds.len();
        let current = match self.focus {
            PopupFocus::Command(ix) => Some(ix),
            PopupFocus::Row(ix) => Some(commands + ix),
            PopupFocus::None | PopupFocus::SubRow(_) => None,
        };
        let count = commands + self.panel.entries.len();
        if let Some(at) = cycle(current, count, delta, true, |at| {
            if at < commands {
                cmds[at].command.enabled
            } else {
                self.panel.entries[at - commands].selectable()
            }
        }) {
            self.focus = if at < commands {
                PopupFocus::Command(at)
            } else {
                PopupFocus::Row(at - commands)
            };
        }
    }

    fn step_command(&mut self, delta: isize) {
        let PopupFocus::Command(ix) = self.focus else {
            return;
        };
        let cmds = self.kind.commands();
        if let Some(next) = cycle(Some(ix), cmds.len(), delta, false, |i| {
            cmds[i].command.enabled
        }) {
            self.focus = PopupFocus::Command(next);
        }
    }

    fn focus_edge(&mut self, last: bool) {
        if self.in_submenu() {
            if let Some(sub) = &self.submenu
                && let Some(ix) = sub.panel.edge(last)
            {
                self.focus = PopupFocus::SubRow(ix);
            }
            return;
        }
        let cmds = self.kind.commands();
        if cmds.is_empty() {
            self.focus = self.panel.edge(last).map_or(PopupFocus::None, PopupFocus::Row);
        } else if last {
            self.focus = self
                .panel
                .edge(true)
                .map(PopupFocus::Row)
                .or_else(|| {
                    cmds.iter()
                        .rposition(|b| b.command.enabled)
                        .map(PopupFocus::Command)
                })
                .unwrap_or(PopupFocus::None);
        } else {
            self.focus = cmds
                .iter()
                .position(|b| b.command.enabled)
                .map(PopupFocus::Command)
                .or_else(|| self.panel.edge(false).map(PopupFocus::Row))
                .unwrap_or(PopupFocus::None);
        }
    }

    fn sample(&mut self, ui: &mut AppUi) -> bool {
        if let Some(sub) = &mut self.submenu {
            sub.motion.sample(ui);
        }
        if self
            .leaving_submenu
            .as_mut()
            .is_some_and(|sub| sub.motion.sample(ui))
        {
            self.leaving_submenu = None;
        }
        self.motion.sample(ui)
    }
}

const CLOSED_RATIO: f32 = 0.5;
const MENU_Z: f32 = 32.;
const SUBMENU_Z: f32 = 40.;
const SUBMENU_CLOSED_RATIO: f32 = 0.67;
const ICON_SIZE: f32 = 16.;
const COMMAND_BUTTON: f32 = 58.;
const COMMAND_GAP: f32 = 1.;
const COMMAND_ICON_TOP: f32 = 9.;
const COMMAND_LABEL_GAP: f32 = 4.;
const COMMAND_LINE: f32 = 16.;
const COMMAND_LABEL_SIZE: f32 = 12.;
const COMMAND_DIVIDER_TOP: f32 = 14.;
const COMMAND_DIVIDER_HEIGHT: f32 = 32.;
const COMMAND_INSET: f32 = 2.;
const ROW_MARGIN_Y: f32 = 2.;
const GLYPH_SIZE: f32 = 12.;
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

impl Menus {
    pub(crate) fn render_bar(&self, focus: Focus, p: Palette, ui: &mut AppUi) -> AppNode {
        let bar_focused = self.bar_focused(focus);
        let triggers: Vec<AppNode> = MenuKind::ALL
            .into_iter()
            .map(|kind| {
                let hit = HitId::Menu(MenuHit::Trigger(kind));
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
                    .items_center()
                    .h_full()
                    .px(BAR_ITEM_PADDING)
                    .rounded(CONTROL_RADIUS)
                    .bg(bg)
                    .child(box_().top(1.).child(text(kind.title())))
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
        let open = self.open_popup()?;
        let has_commands = !open.kind.commands().is_empty();
        let shown_ratio = 1. - CLOSED_RATIO * (1. - open.motion.unroll);

        let cmd_row = has_commands.then(|| render_command_row(open, p, ui));
        let main_rows = render_panel_rows(open, None, p, ui);
        let sub_node = open
            .visible_submenu()
            .map(|sub| render_submenu_popup(open, sub, p, ui));

        let card = box_()
            .hit(HitId::Menu(MenuHit::Panel))
            .cursor(CursorStyle::Arrow)
            .min_w(open.panel.min_width)
            .border(BORDER, p.flyout_stroke)
            .rounded(OVERLAY_RADIUS)
            .shadows(theme_shadow(p.dark, MENU_Z))
            .transform(Transform::RevealY {
                shown_ratio,
                slide: !has_commands,
            })
            .child(
                box_()
                    .w_full()
                    .rounded(OVERLAY_RADIUS - BORDER)
                    .bg(p.menu_fill)
                    .when_some(cmd_row, AppNode::child)
                    .child(main_rows),
            );

        Some(
            box_()
                .anchored(open.kind.anchor())
                .opacity(open.motion.opacity)
                .child(card)
                .when_some(sub_node, AppNode::child)
                .when(open.is_closing(), AppNode::inert),
        )
    }

    pub(crate) fn key_tips(&self, focus: Focus, frame: &AppFrame, ts: &TextSystem) -> Vec<KeyTip> {
        if !self.access_active(focus) {
            return Vec::new();
        }
        let open = match self.mode() {
            MenuMode::Open { menu, .. } => menu,
            MenuMode::Bar { .. } => {
                return MenuKind::ALL
                    .into_iter()
                    .filter_map(|kind| {
                        let key = kind.access_key()?;
                        let anchor = frame.rect_of(HitId::Menu(MenuHit::Trigger(kind)))?;
                        Some(tooltip::layout_tip(ts, key, anchor, tooltip::Placement::Below))
                    })
                    .collect();
            }
            MenuMode::Idle => return Vec::new(),
        };
        if open.kind.bar_kind().is_none() {
            return Vec::new();
        }
        let (panel, serial, is_sub, motion) = match open.focused_submenu() {
            Some(sub) => (&sub.panel, sub.motion.serial(), true, &sub.motion),
            None => (&open.panel, open.motion.serial(), false, &open.motion),
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
                    HitId::Menu(MenuHit::SubmenuItem { serial, ix })
                } else {
                    HitId::Menu(MenuHit::Item { serial, ix })
                };
                let face = frame.rect_of(hit)?.inset(Edges::xy(ROW_INSET, ROW_MARGIN_Y));
                Some(tooltip::layout_tip(ts, key, face, tooltip::Placement::Left))
            })
            .collect()
    }
}

fn render_command_row(open: &OpenMenu, p: Palette, ui: &mut AppUi) -> AppNode {
    let commands = open.kind.commands();
    let mut children = Vec::with_capacity(commands.len() * 2);
    for ix in 0..commands.len() {
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
        children.push(render_command_button(open, ix, p, ui));
    }

    box_()
        .w_full()
        .child(row().w_full().h(COMMAND_ROW).children(children))
        .child(box_().w_full().h(1.).bg(p.menu_separator))
}

fn render_command_button(open: &OpenMenu, ix: usize, p: Palette, ui: &mut AppUi) -> AppNode {
    let CommandButton { command, glyph } = &open.kind.commands()[ix];
    let focused = ui.last_input_was_keyboard && open.focus == PopupFocus::Command(ix);
    let enabled = command.enabled;
    let hit = HitId::Menu(MenuHit::Command {
        serial: open.motion.serial(),
        ix,
    });
    let bg = ui.brush(hit, subtle_brushes(p), enabled);
    let fg = if !enabled {
        p.text_disabled
    } else if ui.is_pressed(hit) {
        p.text_secondary
    } else {
        p.text_primary
    };

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
                        .mt(COMMAND_LABEL_GAP)
                        .h(COMMAND_LINE)
                        .line_height(COMMAND_LINE)
                        .font_family(SMALL_FONT)
                        .text_size(COMMAND_LABEL_SIZE)
                        .nowrap()
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
    ui: &mut AppUi,
) -> AppNode {
    let shown_ratio = 1. - SUBMENU_CLOSED_RATIO * (1. - submenu.motion.unroll);
    let rows = render_panel_rows(open, Some(submenu), p, ui);

    box_()
        .hit(HitId::Menu(MenuHit::SubmenuPanel))
        .cursor(CursorStyle::Arrow)
        .anchored(Anchor::Target {
            target: HitId::Menu(MenuHit::Item {
                serial: open.motion.serial(),
                ix: submenu.ix,
            }),
            place: place_submenu,
        })
        .min_w(submenu.panel.min_width)
        .border(BORDER, p.flyout_stroke)
        .rounded(OVERLAY_RADIUS)
        .shadows(theme_shadow(p.dark, SUBMENU_Z))
        .opacity(submenu.motion.opacity)
        .transform(Transform::RevealY {
            shown_ratio,
            slide: true,
        })
        .child(
            box_()
                .w_full()
                .rounded(OVERLAY_RADIUS - BORDER)
                .bg(p.menu_fill)
                .child(rows),
        )
        .when(submenu.motion.is_closing(), AppNode::inert)
}

fn render_panel_rows(
    open: &OpenMenu,
    sub: Option<&OpenSubmenu>,
    p: Palette,
    ui: &mut AppUi,
) -> AppNode {
    let (panel, serial, is_sub, fade) = match sub {
        Some(sub) => (&sub.panel, sub.motion.serial(), true, false),
        None => (
            &open.panel,
            open.motion.serial(),
            false,
            !open.kind.commands().is_empty(),
        ),
    };
    let focus_visible = ui.last_input_was_keyboard;
    let expanded = open.submenu.as_ref().filter(|_| !is_sub).map(|s| s.ix);

    let rows: Vec<AppNode> = panel
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
            let hit = if is_sub {
                HitId::Menu(MenuHit::SubmenuItem { serial, ix })
            } else {
                HitId::Menu(MenuHit::Item { serial, ix })
            };
            let focused_here = if is_sub {
                open.focus == PopupFocus::SubRow(ix)
            } else {
                open.focus == PopupFocus::Row(ix)
            };
            let ring = focus_visible && focused_here;
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
                .h(entry.height())
                .px(ROW_INSET)
                .py(ROW_MARGIN_Y)
                .when(ring, |this| {
                    this.child(focus_ring(p, Edges::ZERO, CONTROL_RADIUS))
                })
                .child(face)
        })
        .collect();

    box_().w_full().py(PADDING_Y).children(rows)
}

fn render_row_face(
    p: Palette,
    panel: &Panel,
    ix: usize,
    hit: HitId,
    fade: bool,
    expanded: bool,
    ui: &mut AppUi,
) -> AppNode {
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
                    .ml(SHORTCUT_GAP)
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
                    .ml(SHORTCUT_GAP)
                    .w(CHEVRON_WIDTH)
                    .font_family(ICON_FONT)
                    .text_size(GLYPH_SIZE)
                    .line_height(GLYPH_SIZE)
                    .text_color(p.text_secondary)
                    .child(text(CHEVRON_GLYPH)),
            )
        })
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use crate::editor::EditCommand;
    use crate::geom::point;
    use crate::ui::TabId;
    use crate::platform::{Jobs, Modifiers};
    use crate::shell::AppEvent;

    use super::*;

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
        let jobs = Jobs::new();
        let timers = WindowTimers::for_test(&jobs);
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

        // Alt tap arms and focuses menu bar with key tips.
        assert!(!bar.on_key_down(key(Key::Alt), ed_focus, || state).handled);
        let up = bar.on_key_up(key(Key::Alt), ed_focus);
        assert!(up.handled);
        assert_eq!(up.focus, FocusRequest::MenuBar);
        assert!(bar.access_active(Focus::MenuBar));

        // Pressing 'V' opens View menu with key tips, Escape returns to bar, second Escape restores editor.
        let open = bar.on_key_down(key(Key::Char('v')), Focus::MenuBar, || state);
        assert!(open.handled);
        assert_eq!(bar.open_kind(), Some(MenuKind::View));
        assert!(bar.access_active(Focus::MenuBar));

        // Hovering row 0 (Zoom submenu) arms submenu timer; timer opens submenu; Enter activates ZoomIn.
        let _ = bar.on_hover_change(
            Some(HitId::Menu(MenuHit::Item { serial: 1, ix: 0 })),
            || state,
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
        assert!(bar.on_submenu_timer(Some(0)));
        let enter_sub = bar.on_key_down(key(Key::Right), Focus::MenuBar, || state);
        assert!(enter_sub.handled);
        let act = bar.on_key_down(key(Key::Enter), Focus::MenuBar, || state);
        assert_eq!(act.command, Some(AppCommand::ZoomIn.into()));
        assert!(!bar.is_open());

        // Alt+F4 cancels open menu and returns handled=false for OS pass-through.
        bar.open_kind_menu(MenuKind::File, state);
        let alt_f4 = bar.on_key_down(alt_key(Key::F(4)), Focus::MenuBar, || state);
        assert!(!alt_f4.handled);
        assert!(!bar.is_open());
    }

    #[test]
    fn context_menu_uses_snapshot_and_flyout_restores_slot_focus() {
        let jobs = Jobs::new();
        let timers = WindowTimers::for_test(&jobs);
        let mut ui = AppUi::default();
        let mut bar = Menus::default();
        let state = MenuState {
            can_undo: false,
            has_selection: false,
            clipboard_has_text: true,
            ..Default::default()
        };

        ui.last_input_was_keyboard = true;
        bar.open_context_menu(point(10., 10.), state);
        assert!(bar.is_open());
        // Hovering a context menu command clears keyboard focus modality on UiState.
        let _ = bar.on_hover_change(
            Some(HitId::Menu(MenuHit::Command { serial: 1, ix: 2 })),
            || state,
            &mut ui,
            timers,
        );
        assert!(!ui.last_input_was_keyboard);
        // Cut(0) and Copy(1) disabled; Paste(2) enabled -> focused command must be 2.
        let act = bar.on_key_down(key(Key::Enter), Focus::MenuBar, || state);
        assert_eq!(act.command, Some(EditCommand::Paste.into()));
        assert_eq!(act.focus, FocusRequest::RestoreEditor);

        // Mouse-clicking a submenu item opens the submenu without pre-selecting its first child.
        bar.open_kind_menu(MenuKind::View, state);
        let _ = bar.activate(RowPath::Root(0), false);
        assert!(bar.open_popup().is_some_and(|o| o.submenu.is_some() && !o.in_submenu()));

        bar.open_find_options(false, true);
        let closed = bar.close();
        assert_eq!(closed.focus, FocusRequest::Editor(EditorSlot::FindQuery));
    }
}
