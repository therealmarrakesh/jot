mod items;
mod render;

use crate::*;
use crate::ui::{AnimStatus, animate_from};
use items::{
    BORDER, CONTEXT_WIDTH, CommandButton, FLYOUT_GAP, FLYOUT_MIN_WIDTH, MIN_WIDTH, MenuKind,
    Panel, ROW_INSET, ROW_MARGIN_Y, context_menu, cycle, menu_show_delay,
};

pub(crate) use items::Entry;

const CONTEXT: &str = "JotMenu";
const OPEN_DURATION: Duration = Duration::from_millis(250);
const FADE_DURATION: Duration = Duration::from_millis(83);
const BAR_AREA: usize = 0;
const MENU_AREA: usize = 1;
const SUBMENU_AREA: usize = 2;

pub(crate) fn key_bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("up", Command::SelectUp, Some(CONTEXT)),
        KeyBinding::new("down", Command::SelectDown, Some(CONTEXT)),
        KeyBinding::new("enter", Command::Confirm, Some(CONTEXT)),
        KeyBinding::new("space", Command::Confirm, Some(CONTEXT)),
        KeyBinding::new("escape", Command::Cancel, Some(CONTEXT)),
        KeyBinding::new("left", Command::SelectLeft, Some(CONTEXT)),
        KeyBinding::new("right", Command::SelectRight, Some(CONTEXT)),
    ]
}

#[derive(Clone)]
pub(crate) struct MenuHitAreas(Rc<Cell<[Bounds<Pixels>; 3]>>);

impl MenuHitAreas {
    fn new() -> Self {
        Self(Rc::new(Cell::new([Bounds::default(); 3])))
    }

    pub(crate) fn bar_width(&self) -> f32 {
        self.0.get()[BAR_AREA].size.width.as_f32()
    }

    fn area(&self, ix: usize) -> Bounds<Pixels> {
        self.0.get()[ix]
    }

    fn contains(&self, point: Point<Pixels>) -> bool {
        self.0.get().iter().any(|area| area.contains(&point))
    }

    fn clear_submenu(&self) {
        let mut areas = self.0.get();
        areas[SUBMENU_AREA] = Bounds::default();
        self.0.set(areas);
    }

    fn record(&self, ix: usize, child: impl IntoElement) -> measure::Measured {
        let areas = self.0.clone();
        measure::measured(child, move |bounds, _, _| {
            let mut all = areas.get();
            all[ix] = bounds;
            areas.set(all);
        })
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

    fn sample(&mut self, window: &mut Window, cx: &mut App) -> bool {
        if !self.closing {
            self.unroll = animate_from(
                ("jot-menu-unroll", self.serial),
                0.0_f32,
                1.0_f32,
                OPEN_DURATION,
                DECELERATE,
                window,
                cx,
            )
            .value;
        }
        let appear = if self.fade_in {
            animate_from(
                ("jot-menu-appear", self.serial),
                0.0_f32,
                1.0_f32,
                FADE_DURATION,
                Easing::Linear,
                window,
                cx,
            )
            .value
        } else {
            1.
        };
        let fade = animate_from(
            ("jot-menu-fade", self.serial),
            1.0_f32,
            if self.closing { 0. } else { 1. },
            FADE_DURATION,
            Easing::Linear,
            window,
            cx,
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
    focus: FocusHandle,
    open: Option<OpenMenu>,
    serial: usize,
    submenu_timer: Task<()>,
    hit_areas: MenuHitAreas,
    trigger_areas: MenuHitAreas,
    bar_focus: Option<MenuKind>,
    access: bool,
    bounds: Bounds<Pixels>,
    _window_bounds: Subscription,
}

impl MenuBar {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Jot>) -> Self {
        let _window_bounds = cx.observe_window_bounds(window, |this, window, cx| {
            let bounds = window.bounds();
            if bounds != this.menu_bar.bounds {
                this.menu_bar.bounds = bounds;
                this.menu_cancel(window, cx);
            }
        });
        Self {
            focus: cx.focus_handle(),
            open: None,
            serial: 0,
            submenu_timer: Task::ready(()),
            hit_areas: MenuHitAreas::new(),
            trigger_areas: MenuHitAreas::new(),
            bar_focus: None,
            access: false,
            bounds: window.bounds(),
            _window_bounds,
        }
    }

    pub(crate) fn is_open(&self) -> bool {
        self.live_open().is_some()
    }

    pub(crate) fn access_active(&self) -> bool {
        self.access
    }

    pub(crate) fn width(&self) -> f32 {
        self.hit_areas.bar_width()
    }

    pub(crate) fn hit_areas(&self) -> MenuHitAreas {
        self.hit_areas.clone()
    }

    pub(crate) fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    fn bar_focused(&self, window: &Window) -> Option<MenuKind> {
        self.bar_focus
            .filter(|_| self.live_open().is_none() && self.focus.is_focused(window))
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

    fn key_tips(&self, window: &Window) -> Vec<keytips::KeyTip> {
        if !self.access {
            return Vec::new();
        }
        let Some(open) = self.live_open() else {
            return MenuKind::ALL
                .into_iter()
                .filter_map(|kind| {
                    let key = kind.access_key()?;
                    let anchor = self.trigger_areas.area(kind.index());
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
        let (panel, area, motion) = match open.focused_submenu() {
            Some(submenu) => (&submenu.panel, self.hit_areas.area(SUBMENU_AREA), &submenu.motion),
            None => (&open.panel, self.hit_areas.area(MENU_AREA), &open.motion),
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
}

impl Jot {
    fn menu_exit_access(&mut self, cx: &mut Context<Self>) {
        if self.menu_bar.access {
            self.menu_bar.access = false;
            cx.notify();
        }
    }

    pub(crate) fn menu_cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.menu_bar.live_open().is_some() {
            self.menu_close(window, cx);
        } else if self.menu_bar.bar_focus.is_some() {
            self.menu_leave_bar(window, cx);
        }
        self.menu_bar.bar_focus = None;
        self.menu_exit_access(cx);
    }

    pub(crate) fn menu_pointer_down(&mut self, cx: &mut Context<Self>) {
        self.menu_bar.bar_focus = None;
        self.menu_exit_access(cx);
    }

    pub(crate) fn menu_key_down(
        &mut self,
        keystroke: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let modifiers = keystroke.modifiers;
        if matches!(keystroke.key.as_str(), "alt" | "f10") && !modifiers.modified() {
            self.menu_toggle_access(window, cx);
            return true;
        }
        let key = match keystroke.key.as_bytes() {
            [byte] if byte.is_ascii_alphanumeric() => Some(char::from(byte.to_ascii_uppercase())),
            _ => None,
        };
        if !self.menu_bar.access {
            return match key {
                Some(key) if modifiers.alt && !modifiers.control && !modifiers.platform => {
                    self.menu_invoke_access(key, window, cx)
                }
                _ => false,
            };
        }
        match keystroke.key.as_str() {
            "escape" => {
                self.menu_access_back(window, cx);
                true
            }
            "up" | "down" | "left" | "right" | "tab" | "space" | "enter" => {
                self.menu_exit_access(cx);
                false
            }
            _ => {
                if let Some(key) = key.filter(|_| !modifiers.control && !modifiers.platform) {
                    self.menu_invoke_access(key, window, cx);
                }
                true
            }
        }
    }

    fn menu_toggle_access(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.menu_bar.access {
            self.menu_exit_access(cx);
            if self.menu_bar.live_open().is_none() {
                self.menu_leave_bar(window, cx);
            }
            return;
        }
        match self.menu_bar.live_open() {
            Some(open) if open.kind.is_none() => return,
            Some(_) => {}
            None => {
                if self.menu_bar.bar_focus.is_none() {
                    self.menu_bar.bar_focus = Some(MenuKind::File);
                }
                self.menu_bar.focus.focus(window, cx);
            }
        }
        self.menu_bar.access = true;
        cx.notify();
    }

    fn menu_access_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.menu_bar.live_open_mut() {
            Some(open) => match open.live_submenu().map(|submenu| submenu.ix) {
                Some(ix) => {
                    open.panel.focused = Some(ix);
                    self.menu_close_submenu(cx);
                }
                None => self.menu_close_to_bar(window, cx),
            },
            None => {
                self.menu_bar.access = false;
                self.menu_leave_bar(window, cx);
            }
        }
    }

    fn menu_invoke_access(&mut self, key: char, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(open) = self.menu_bar.live_open_mut() else {
            let Some(kind) = MenuKind::ALL
                .into_iter()
                .find(|kind| kind.access_key() == Some(key))
            else {
                return false;
            };
            self.menu_open_by_keyboard(kind, window, cx);
            self.menu_bar.access = true;
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
            self.menu_bar.access = false;
            self.menu_activate((parent, Some(sub)), window, cx);
            return true;
        }
        let Some(ix) = open.panel.find_key(key) else {
            return false;
        };
        if matches!(open.panel.entries[ix], Entry::Submenu(..)) {
            open.panel.focused = Some(ix);
            self.menu_bar.access = true;
            self.menu_open_submenu(ix, true, window, cx);
        } else {
            self.menu_bar.access = false;
            self.menu_activate((ix, None), window, cx);
        }
        true
    }

    fn menu_open_by_keyboard(&mut self, kind: MenuKind, window: &mut Window, cx: &mut Context<Self>) {
        self.menu_open(kind, window, cx);
        if let Some(open) = self.menu_bar.open.as_mut() {
            open.panel.step(1);
            open.focus_visible = true;
        }
    }

    fn menu_close_to_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.menu_bar.live_open_mut() else {
            return;
        };
        let Some(kind) = open.kind else {
            return self.menu_close(window, cx);
        };
        open.motion.closing = true;
        self.menu_bar.submenu_timer = Task::ready(());
        self.menu_bar.bar_focus = Some(kind);
        self.menu_bar.focus.focus(window, cx);
        self.sync_selection_focus(cx);
        cx.notify();
    }

    fn menu_leave_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.menu_bar.bar_focus = None;
        self.active_editor().focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    fn menu_move_bar_focus(&mut self, delta: isize, cx: &mut Context<Self>) {
        if let Some(kind) = self.menu_bar.bar_focus {
            self.menu_bar.bar_focus = Some(kind.cycle(delta));
            cx.notify();
        }
    }

    fn menu_open(&mut self, kind: MenuKind, window: &mut Window, cx: &mut Context<Self>) {
        let editor = self.active_editor();
        let entries = kind.entries(editor.read(cx), cx);
        let panel = Panel::new(entries, MIN_WIDTH, true, &editor.focus_handle(cx), window);
        self.menu_show(OpenMenu::new(Some(kind), panel), window, cx);
    }

    pub(crate) fn open_context_menu(
        &mut self,
        anchor: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let editor = self.active_editor();
        let (commands, entries) = context_menu(editor.read(cx));
        let panel = Panel::new(
            entries,
            CONTEXT_WIDTH,
            false,
            &editor.focus_handle(cx),
            window,
        );
        let focused_command = commands.iter().position(|button| button.command.enabled);
        self.menu_show(
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
            &self.active_editor().focus_handle(cx),
            window,
        );
        let anchor = point(
            target.center().x - px((panel.width / 2.).floor()),
            target.top() - px(FLYOUT_GAP + panel.height()),
        );
        self.menu_show(
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

    fn menu_show(&mut self, open: OpenMenu, window: &mut Window, cx: &mut Context<Self>) {
        self.menu_bar.bar_focus = None;
        let motion = Motion::new(self.menu_bar.next_serial(), !open.commands.is_empty());
        self.menu_bar.open = Some(OpenMenu { motion, ..open });
        self.menu_bar.submenu_timer = Task::ready(());
        self.menu_bar.focus.focus(window, cx);
        self.sync_selection_focus(cx);
        cx.notify();
    }

    fn menu_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.menu_bar.access = false;
        let return_focus = self.menu_bar.live_open_mut().and_then(|open| {
            open.motion.closing = true;
            open.return_focus.take()
        });
        self.menu_bar.submenu_timer = Task::ready(());
        let target = return_focus.unwrap_or_else(|| self.active_editor().focus_handle(cx));
        target.focus(window, cx);
        self.sync_selection_focus(cx);
        cx.notify();
    }

    fn menu_run_command(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(command) = self
            .menu_bar
            .live_open()
            .and_then(|open| open.commands.get(ix))
            .map(|button| button.command)
            .filter(|command| command.enabled)
        else {
            return;
        };
        self.menu_dispatch(command.command, window, cx);
    }

    fn menu_dispatch(&mut self, command: Command, window: &mut Window, cx: &mut Context<Self>) {
        self.menu_close(window, cx);
        let target = self.active_editor().focus_handle(cx);
        window.defer(cx, move |window, cx| {
            target.dispatch_command(command, window, cx);
        });
    }

    fn menu_activate(
        &mut self,
        path: (usize, Option<usize>),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(open) = self.menu_bar.live_open() else {
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
            Entry::Submenu(..) => self.menu_open_submenu(path.0, true, window, cx),
            entry if !entry.selectable() => {}
            entry => {
                if let Some(command) = entry.command_id() {
                    self.menu_dispatch(command, window, cx);
                }
            }
        }
    }

    fn menu_open_submenu(
        &mut self,
        ix: usize,
        highlight_first: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let action_context = self.active_editor().focus_handle(cx);
        let serial = self.menu_bar.next_serial();
        self.menu_bar.submenu_timer = Task::ready(());
        let Some(open) = self.menu_bar.live_open_mut() else {
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

    fn menu_close_submenu(&mut self, cx: &mut Context<Self>) {
        self.menu_bar.submenu_timer = Task::ready(());
        if let Some(submenu) = self.menu_bar.open.as_mut().and_then(OpenMenu::live_submenu_mut) {
            submenu.motion.closing = true;
            cx.notify();
        }
    }

    fn menu_schedule_submenu(
        &mut self,
        change: SubmenuChange,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = match change {
            SubmenuChange::Keep => {
                self.menu_bar.submenu_timer = Task::ready(());
                return;
            }
            SubmenuChange::Open(ix) => Some(ix),
            SubmenuChange::Close => None,
        };
        let delay = menu_show_delay();
        self.menu_bar.submenu_timer = cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(delay).await;
            this.update_in(cx, |this, window, cx| match target {
                Some(ix) => this.menu_open_submenu(ix, false, window, cx),
                None => this.menu_close_submenu(cx),
            })
            .ok();
        });
    }

    fn menu_hover_row(
        &mut self,
        path: (usize, Option<usize>),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(open) = self.menu_bar.live_open() else {
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
        self.menu_schedule_submenu(change, window, cx);
        cx.notify();
    }

    fn menu_switch(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(kind) = self.menu_bar.open_kind() else {
            return;
        };
        self.menu_open_by_keyboard(kind.cycle(delta), window, cx);
    }

    fn handle_menu_command(
        &mut self,
        command: Command,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        match command {
            Command::SelectUp => self.menu_step_active(-1, window, cx),
            Command::SelectDown => self.menu_step_active(1, window, cx),
            Command::Confirm => self.menu_on_confirm(window, cx),
            Command::Cancel => self.menu_on_cancel(window, cx),
            Command::SelectLeft => self.menu_on_left(window, cx),
            Command::SelectRight => self.menu_on_right(window, cx),
            _ => return false,
        }
        true
    }

    fn menu_step_active(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.menu_bar.live_open_mut() else {
            if delta > 0
                && let Some(kind) = self.menu_bar.bar_focused(window)
            {
                self.menu_open_by_keyboard(kind, window, cx);
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

    fn menu_on_confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.menu_bar.live_open() else {
            if let Some(kind) = self.menu_bar.bar_focused(window) {
                self.menu_open_by_keyboard(kind, window, cx);
            }
            return;
        };
        if let Some(ix) = open.focused_command {
            self.menu_run_command(ix, window, cx);
            return;
        }
        let sub = open
            .live_submenu()
            .and_then(|submenu| submenu.panel.focused);
        if let Some(ix) = open.panel.focused {
            self.menu_activate((ix, sub), window, cx);
        }
    }

    fn menu_on_cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.menu_bar.live_open() {
            Some(open) if open.in_submenu() => self.menu_close_submenu(cx),
            Some(open) if open.kind.is_some() => self.menu_close_to_bar(window, cx),
            Some(_) => self.menu_close(window, cx),
            None => self.menu_leave_bar(window, cx),
        }
    }

    fn menu_on_left(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.menu_bar.live_open_mut() else {
            self.menu_move_bar_focus(-1, cx);
            return;
        };
        open.focus_visible = true;
        if !open.commands.is_empty() {
            open.step_command(-1);
            cx.notify();
        } else if open.in_submenu() {
            self.menu_close_submenu(cx);
        } else {
            self.menu_switch(-1, window, cx);
        }
    }

    fn menu_on_right(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.menu_bar.live_open_mut() else {
            self.menu_move_bar_focus(1, cx);
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
            Some(ix) => self.menu_open_submenu(ix, true, window, cx),
            None => self.menu_switch(1, window, cx),
        }
    }
}
