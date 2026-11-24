use crate::color::transparent_black;
use crate::geom::{Edges, round_half_up};
use crate::gfx::{TextStyle, TextSystem};
use crate::ids::{Focus, Frame, HitId, MenuKind, Node, UiState};
use crate::keytips::{self, KeyTip};
use crate::platform::CursorStyle;
use crate::theme::{Palette, theme_shadow};
use crate::ui::controls::{
    CONTROL_RADIUS, ICON_FONT, OVERLAY_RADIUS, SMALL_FONT, TEXT_FONT, focus_ring, subtle_brushes,
};
use crate::ui::{Anchor, box_, row, text};

use super::bar::{MenuBar, MenuMode};
use super::model::{
    BORDER, CHECK_COLUMN, CommandButton, Entry, LABEL_SIZE, PADDING_Y, Panel, ROW_INSET,
    ROW_PADDING, ROW_SEPARATOR, SHORTCUT_SIZE,
};
use super::popup::{COMMAND_ROW, OpenMenu, OpenSubmenu, PopupFocus};

const CLOSED_RATIO: f32 = 0.5;
const MENU_Z: f32 = 32.;
const SUBMENU_Z: f32 = 40.;
const SUBMENU_CLOSED_RATIO: f32 = 0.67;
const ICON_SIZE: f32 = 16.;
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
const COMMAND_INSET: f32 = 2.;
const ROW_MARGIN_Y: f32 = 2.;
const GLYPH_SIZE: f32 = 12.;
const SUBMENU_OVERLAP: f32 = 4.;
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

impl MenuBar {
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
            .hit(HitId::MenuBar)
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
        let open = self.open_popup()?;
        let (height, bar) = open.total_height();
        let width = open.panel.width;
        let has_commands = !open.kind.commands().is_empty();
        let hidden = open.panel.height() * CLOSED_RATIO * (1. - open.motion.unroll);
        let offset = if has_commands { 0. } else { -hidden };

        let cmd_row = has_commands.then(|| render_command_row(open, p, ts, ui));
        let main_rows = render_panel_rows(open, None, bar, p, ui);
        let sub_node = open
            .visible_submenu()
            .map(|sub| render_submenu_popup(open, sub, p, ui));

        let content = box_()
            .size_full()
            .when_some(cmd_row, |this, r| this.child(r))
            .child(main_rows);

        Some(
            box_()
                .hit(HitId::MenuPanel)
                .cursor(CursorStyle::Arrow)
                .anchored(open.kind.anchor())
                .w(width)
                .h(height)
                .opacity(open.motion.opacity)
                .children(reveal_card(p, MENU_Z, height, height - hidden, offset, content))
                .when_some(sub_node, |this, sub| this.child(sub))
                .when(open.is_closing(), Node::inert),
        )
    }

    pub(crate) fn key_tips(&self, focus: Focus, frame: &Frame, ts: &TextSystem) -> Vec<KeyTip> {
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
                        let anchor = frame.rect_of(HitId::MenuTrigger(kind))?;
                        Some(keytips::layout_tip(ts, key, anchor, keytips::Placement::Below))
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
    let commands = open.kind.commands();
    let count = commands.len() as f32;
    let row_w = open.panel.width + 2. * COMMAND_OUTSET;
    let cell = ((row_w - (count - 1.)) / count).floor();

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
        children.push(render_command_button(open, ix, cell, p, ts, ui));
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

fn render_command_button(
    open: &OpenMenu,
    ix: usize,
    cell: f32,
    p: Palette,
    ts: &TextSystem,
    ui: &mut UiState,
) -> Node {
    let face_w = cell - 2. * (COMMAND_GAP + COMMAND_INSET);
    let CommandButton { command, glyph } = &open.kind.commands()[ix];
    let focused = ui.last_input_was_keyboard() && open.focus == PopupFocus::Command(ix);
    let enabled = command.enabled;
    let hit = HitId::MenuCommand {
        serial: open.motion.serial(),
        ix,
    };
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
    let rows = render_panel_rows(open, Some(submenu), 0., p, ui);

    box_()
        .hit(HitId::SubmenuPanel)
        .cursor(CursorStyle::Arrow)
        .anchored(Anchor::SideOf {
            target: HitId::MenuItem {
                serial: open.motion.serial(),
                ix: submenu.ix,
            },
            overlap: SUBMENU_OVERLAP - BORDER,
            offset_y: -(BORDER + PADDING_Y),
        })
        .w(sub_w)
        .h(sub_h)
        .opacity(submenu.motion.opacity)
        .children(reveal_card(p, SUBMENU_Z, sub_h, sub_h - hidden, -hidden, rows))
        .when(submenu.motion.is_closing(), Node::inert)
}

fn render_panel_rows(
    open: &OpenMenu,
    sub: Option<&OpenSubmenu>,
    offset: f32,
    p: Palette,
    ui: &mut UiState,
) -> Node {
    let (panel, serial, is_sub, fade) = match sub {
        Some(sub) => (&sub.panel, sub.motion.serial(), true, false),
        None => (
            &open.panel,
            open.motion.serial(),
            false,
            !open.kind.commands().is_empty(),
        ),
    };
    let focus_visible = ui.last_input_was_keyboard();
    let expanded = open.submenu.as_ref().filter(|_| !is_sub).map(|s| s.ix);

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
            let hit = if is_sub {
                HitId::SubmenuItem { serial, ix }
            } else {
                HitId::MenuItem { serial, ix }
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

    box_()
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
