use super::items::{
    BAR_GAP, BAR_ITEM_PADDING, BORDER, CHECK_COLUMN, CHECK_GLYPH, CHEVRON_GLYPH, COMMAND_BUTTON,
    COMMAND_DIVIDER_HEIGHT, COMMAND_DIVIDER_TOP, COMMAND_GAP, COMMAND_ICON_TOP, COMMAND_INSET,
    COMMAND_LABEL_GAP, COMMAND_LABEL_PANEL, COMMAND_LABEL_SIZE, COMMAND_LINE, COMMAND_OUTSET,
    COMMAND_ROW, CommandButton, Entry, GLYPH_SIZE, ICON_SIZE, LABEL_SIZE, MenuKind, OVERFLOW_TOP,
    PADDING_Y, Panel, ROW_INSET, ROW_MARGIN_Y, ROW_PADDING, ROW_SEPARATOR, SHORTCUT_SIZE,
    SUBMENU_OVERLAP,
};
use super::{BAR_AREA, CONTEXT, MENU_AREA, OpenMenu, OpenSubmenu, SUBMENU_AREA};
use crate::*;

const CLOSED_RATIO: f32 = 0.5;
const MENU_Z: f32 = 32.;
const SUBMENU_Z: f32 = 40.;
const SUBMENU_CLOSED_RATIO: f32 = 0.67;
const BAR_FOCUS_OUTSET: Edges<f32> = Edges {
    top: 0.,
    right: 1.,
    bottom: 1.,
    left: 1.,
};

impl Jot {
    fn menu_command_row(open: &OpenMenu, window: &Window, cx: &mut Context<Self>) -> Div {
        let p = palette(cx);
        let commands = &open.commands;
        let count = commands.len() as f32;
        let row = open.panel.width + 2. * COMMAND_OUTSET;
        let cell = ((row - (count - 1.)) / count).floor();
        let ring = open.focused_command.filter(|_| open.focus_visible);
        div()
            .absolute()
            .top(px(BORDER))
            .left(px(-COMMAND_OUTSET))
            .w(px(row))
            .h(px(COMMAND_ROW))
            .flex()
            .children(commands.iter().enumerate().flat_map(|(ix, command)| {
                let divider = (ix > 0).then(|| {
                    div()
                        .flex_none()
                        .w(px(1.))
                        .h(px(COMMAND_DIVIDER_HEIGHT))
                        .mt(px(COMMAND_DIVIDER_TOP))
                        .bg(rgb(p.menu_separator))
                });
                let button =
                    Self::menu_command_button(p, ix, command, cell, ring == Some(ix), window, cx);
                divider
                    .into_iter()
                    .map(IntoElement::into_any_element)
                    .chain(Some(button.into_any_element()))
            }))
            .child(
                div()
                    .absolute()
                    .left(px(BORDER + COMMAND_OUTSET))
                    .right(px(BORDER + COMMAND_OUTSET))
                    .bottom(px(-1.))
                    .h(px(1.))
                    .bg(rgb(p.menu_separator)),
            )
    }

    fn menu_command_button(
        p: Palette,
        ix: usize,
        button: &CommandButton,
        cell: f32,
        focused: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let face = cell - 2. * (COMMAND_GAP + COMMAND_INSET);
        let CommandButton { command, glyph } = button;
        let enabled = command.enabled;
        let color = if enabled {
            p.text_primary
        } else {
            p.text_disabled
        };
        let label_width = text_width(window, command.label, SMALL_FONT, COMMAND_LABEL_SIZE);
        let label_left = (face - COMMAND_LABEL_PANEL) / 2.
            + round_half_up((COMMAND_LABEL_PANEL - label_width) / 2.);
        div()
            .id(("menu-command", ix))
            .flex_none()
            .w(px(cell))
            .h_full()
            .pt(px(COMMAND_ROW - COMMAND_BUTTON + COMMAND_INSET))
            .px(px(COMMAND_GAP + COMMAND_INSET))
            .pb(px(COMMAND_INSET))
            .on_click(cx.listener(move |this, _, window, cx| this.menu_run_command(ix, window, cx)))
            .child(
                div()
                    .id(("menu-command-face", ix))
                    .size_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .pt(px(COMMAND_ICON_TOP))
                    .rounded(px(field::CONTROL_RADIUS))
                    .text_color(rgba(color))
                    .child(
                        div()
                            .h(px(COMMAND_LINE))
                            .line_height(px(COMMAND_LINE))
                            .font_family(ICON_FONT)
                            .text_size(px(ICON_SIZE))
                            .child(*glyph),
                    )
                    .child(
                        div()
                            .self_start()
                            .ml(px(label_left))
                            .w(px(label_width))
                            .text_center()
                            .mt(px(COMMAND_LABEL_GAP))
                            .h(px(COMMAND_LINE))
                            .line_height(px(COMMAND_LINE))
                            .font_family(SMALL_FONT)
                            .text_size(px(COMMAND_LABEL_SIZE))
                            .child(command.label),
                    )
                    .brush_transition(Brushes::subtle(p))
                    .disabled(!enabled)
                    .when(enabled, |this| {
                        this.pressed(move |style| style.text_color(rgba(p.text_secondary)))
                    })
                    .when(focused, |this| {
                        this.child(field::focus_ring(
                            p,
                            Edges::default(),
                            px(field::CONTROL_RADIUS),
                        ))
                    }),
            )
    }

    fn menu_trigger(
        &self,
        kind: MenuKind,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let p = palette(cx);
        let open = self
            .menu_bar
            .open
            .as_ref()
            .filter(|open| open.kind == Some(kind));
        let active = open.is_some_and(|open| !open.is_closing());
        let popup = open.map(|open| self.menu_popup(open, window, cx));
        let ring = self.menu_bar.bar_focused(window) == Some(kind);

        let trigger = div()
            .id(kind.title())
            .relative()
            .flex()
            .items_center()
            .h_full()
            .px(px(BAR_ITEM_PADDING))
            .rounded(px(field::CONTROL_RADIUS))
            .when(active, |this| this.bg(rgba(p.subtle_tertiary)))
            .hover(move |style| style.bg(rgba(p.subtle_secondary)))
            .pressed(move |style| style.bg(rgba(p.subtle_tertiary)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                    if this.menu_bar.open_kind() == Some(kind) {
                        this.menu_close(window, cx);
                    } else {
                        this.menu_open(kind, window, cx);
                    }
                }),
            )
            .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                if *hovered && this.menu_bar.open_kind().is_some_and(|open| open != kind) {
                    this.menu_open(kind, window, cx);
                }
            }))
            .child(div().relative().top(px(1.)).child(kind.title()))
            .when(ring, |this| {
                this.child(
                    deferred(field::focus_ring(
                        p,
                        BAR_FOCUS_OUTSET,
                        px(field::CONTROL_RADIUS + BAR_FOCUS_OUTSET.left),
                    ))
                    .with_priority(2),
                )
            })
            .when_some(popup, |this, popup| {
                this.child(
                    div()
                        .absolute()
                        .left_0()
                        .top(px(MENU_BAR_HEIGHT))
                        .child(deferred(anchored().child(popup)).with_priority(1)),
                )
            });
        self.menu_bar.trigger_areas.record(kind.index(), trigger)
    }

    fn menu_sync_key_tips(&self, window: &Window, cx: &mut Context<Self>) {
        let owner = window.window_handle();
        let tips = self.menu_bar.key_tips(window);
        if tips.is_empty() {
            if keytips::is_owner(owner, cx) {
                cx.defer(move |cx| keytips::hide_owner(owner, cx));
            }
            return;
        }
        let origin = window.hwnd().client_origin();
        let scale = window.scale_factor();
        cx.defer(move |cx| keytips::show(owner, origin, scale, tips, cx));
    }

    fn menu_popup(
        &self,
        open: &OpenMenu,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let p = palette(cx);
        let panel = &open.panel;
        let width = panel.width;
        let has_commands = !open.commands.is_empty();
        let bar = if has_commands {
            COMMAND_ROW + 1. + OVERFLOW_TOP - PADDING_Y
        } else {
            0.
        };
        let height = panel.height() + bar;
        let hidden = panel.height() * CLOSED_RATIO * (1. - open.motion.unroll);
        let (top, offset) = match (has_commands, open.upward) {
            (true, _) => (0., 0.),
            (false, true) => (hidden, hidden),
            (false, false) => (0., -hidden),
        };
        let submenu = open
            .submenu
            .as_ref()
            .map(|submenu| self.menu_submenu_popup(open, submenu, cx));

        let panel = div()
            .relative()
            .w(px(width))
            .h(px(height))
            .opacity(open.motion.opacity)
            .occlude()
            .cursor_default()
            .on_mouse_down_out(cx.listener(|this, event: &MouseDownEvent, window, cx| {
                if !this.menu_bar.hit_areas.contains(event.position) {
                    this.menu_close(window, cx);
                }
            }))
            .children(reveal(
                p,
                MENU_Z,
                height,
                top,
                height - hidden,
                offset,
                div()
                    .size_full()
                    .when(has_commands, |this| {
                        this.child(Self::menu_command_row(open, window, cx))
                    })
                    .child(Self::menu_panel_rows(open, panel, None, bar, has_commands, cx)),
            ))
            .children(submenu);
        self.menu_bar.hit_areas.record(MENU_AREA, panel)
    }

    fn menu_submenu_popup(
        &self,
        open: &OpenMenu,
        submenu: &OpenSubmenu,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let p = palette(cx);
        let sub_width = submenu.panel.width;
        let sub_height = submenu.panel.height();
        let hidden = sub_height * SUBMENU_CLOSED_RATIO * (1. - submenu.motion.unroll);
        let top = open.panel.row_top(submenu.ix) - BORDER - PADDING_Y;
        let rows = Self::menu_panel_rows(open, &submenu.panel, Some(submenu.ix), 0., false, cx);
        let panel = div()
            .id("submenu")
            .absolute()
            .left(px(open.panel.width - SUBMENU_OVERLAP))
            .top(px(top))
            .w(px(sub_width))
            .h(px(sub_height))
            .opacity(submenu.motion.opacity)
            .occlude()
            .cursor_default()
            .children(reveal(
                p,
                SUBMENU_Z,
                sub_height,
                0.,
                sub_height - hidden,
                -hidden,
                rows,
            ));
        self.menu_bar.hit_areas.record(SUBMENU_AREA, panel)
    }

    fn menu_panel_rows(
        open: &OpenMenu,
        panel: &Panel,
        parent: Option<usize>,
        offset: f32,
        fade: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let p = palette(cx);
        let focus_visible = open.focus_visible && (parent.is_some() || !open.in_submenu());
        let expanded = open
            .live_submenu()
            .filter(|_| parent.is_none())
            .map(|submenu| submenu.ix);

        div()
            .absolute()
            .top(px(BORDER + PADDING_Y + offset))
            .left(px(BORDER))
            .w(px(panel.width - 2. * BORDER))
            .children(panel.entries.iter().enumerate().map(|(ix, entry)| {
                let path = match parent {
                    Some(parent) => (parent, Some(ix)),
                    None => (ix, None),
                };
                if matches!(entry, Entry::Separator) {
                    return separator_row(p, ix);
                }
                let ring = focus_visible && panel.focused == Some(ix);
                let face = row_face(
                    p,
                    panel,
                    ix,
                    fade && entry.selectable(),
                    expanded == Some(ix),
                );
                div()
                    .id(("menu-row", ix))
                    .relative()
                    .h(px(entry.height()))
                    .px(px(ROW_INSET))
                    .py(px(ROW_MARGIN_Y))
                    .when(ring, |this| {
                        this.child(field::focus_ring(
                            p,
                            Edges::default(),
                            px(field::CONTROL_RADIUS),
                        ))
                    })
                    .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                        if *hovered {
                            this.menu_hover_row(path, window, cx);
                        }
                    }))
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.menu_activate(path, window, cx)),
                    )
                    .child(face)
            }))
    }

    pub(crate) fn render_menu(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let p = palette(cx);
        if self.menu_bar.access || keytips::is_owner(window.window_handle(), cx) {
            let this = cx.entity().downgrade();
            window.defer(cx, move |window, cx| {
                this.update(cx, |jot, cx| jot.menu_sync_key_tips(window, cx))
                    .ok();
            });
        }
        if self
            .menu_bar
            .open
            .as_mut()
            .is_some_and(|open| open.sample(window, cx))
        {
            self.menu_bar.open = None;
        }
        if self
            .menu_bar
            .open
            .as_ref()
            .is_none_or(|open| open.submenu.is_none())
        {
            self.menu_bar.hit_areas.clear_submenu();
        }
        let flyout = self.menu_bar.open.as_ref().and_then(|open| {
            let anchor = open.anchor?;
            Some(
                deferred(
                    anchored()
                        .position(anchor)
                        .child(self.menu_popup(open, window, cx)),
                )
                .with_priority(1),
            )
        });

        let bar = div()
            .id("menu-bar")
            .track_focus(&self.menu_bar.focus)
            .key_context(CONTEXT)
            .relative()
            .flex()
            .h_full()
            .gap(px(BAR_GAP))
            .font_family(TEXT_FONT)
            .text_size(px(LABEL_SIZE))
            .text_color(rgba(p.text_primary))
            .on_command(cx.command_listener(Self::handle_menu_command))
            .children(MenuKind::ALL.map(|kind| self.menu_trigger(kind, window, cx)))
            .children(flyout);
        self.menu_bar.hit_areas.record(BAR_AREA, bar)
    }
}

fn separator_row(p: Palette, ix: usize) -> Stateful<Div> {
    div()
        .id(("menu-separator", ix))
        .h(px(ROW_SEPARATOR))
        .py(px(1.))
        .child(div().h(px(1.)).bg(rgb(p.menu_separator)))
}

fn row_face(p: Palette, panel: &Panel, ix: usize, fade: bool, expanded: bool) -> AnyElement {
    let entry = &panel.entries[ix];
    let label = panel.labels[ix].clone();
    let checks = panel.checks;
    let shortcut = panel.shortcuts[ix].clone();
    let enabled = entry.selectable();
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
        _ if !enabled => Some(p.text_disabled),
        Entry::Toggle { .. } => Some(p.text_secondary),
        _ if fade => None,
        _ => Some(p.text_primary),
    };
    let label_color = if !enabled {
        Some(p.text_disabled)
    } else if fade {
        None
    } else {
        Some(p.text_primary)
    };
    let face = div()
        .id(("menu-row-face", ix))
        .flex()
        .items_center()
        .size_full()
        .px(px(ROW_PADDING))
        .rounded(px(field::CONTROL_RADIUS))
        .group("menu-row")
        .text_color(rgba(p.text_primary))
        .when(checks, |this| {
            this.child(
                div()
                    .flex_none()
                    .w(px(CHECK_COLUMN))
                    .font_family(ICON_FONT)
                    .text_size(px(mark_size))
                    .line_height(px(mark_size))
                    .when_some(mark_color, |this, color| this.text_color(rgba(color)))
                    .when(enabled && matches!(entry, Entry::Toggle { .. }), |this| {
                        this.group_hover("menu-row", move |style| {
                            style.text_color(rgba(p.text_primary))
                        })
                    })
                    .children(mark),
            )
        })
        .child(
            div()
                .flex_1()
                .font_family(TEXT_FONT)
                .text_size(px(LABEL_SIZE))
                .whitespace_nowrap()
                .when_some(label_color, |this, color| this.text_color(rgba(color)))
                .child(label),
        )
        .when_some(shortcut, |this, shortcut| {
            this.child(
                div()
                    .flex_none()
                    .font_family(SMALL_FONT)
                    .text_size(px(SHORTCUT_SIZE))
                    .whitespace_nowrap()
                    .text_color(rgba(if enabled {
                        p.text_secondary
                    } else {
                        p.text_disabled
                    }))
                    .child(shortcut),
            )
        })
        .when(matches!(entry, Entry::Submenu(..)), |this| {
            this.child(
                div()
                    .flex_none()
                    .font_family(ICON_FONT)
                    .text_size(px(GLYPH_SIZE))
                    .line_height(px(GLYPH_SIZE))
                    .text_color(rgba(p.text_secondary))
                    .child(CHEVRON_GLYPH),
            )
        });
    if fade {
        face.brush_transition(Brushes::subtle(p))
            .pressed(move |style| style.text_color(rgba(p.text_secondary)))
            .into_any_element()
    } else {
        face.when(expanded, |this| this.bg(rgba(p.subtle_secondary)))
            .when(enabled, |this| {
                this.hover(move |style| style.bg(rgba(p.subtle_secondary)))
                    .pressed(move |style| style.bg(rgba(p.subtle_tertiary)))
            })
            .into_any_element()
    }
}

fn reveal(
    p: Palette,
    z: f32,
    height: f32,
    top: f32,
    shown: f32,
    offset: f32,
    rows: Div,
) -> [Div; 2] {
    let card = div()
        .absolute()
        .top(px(top))
        .left_0()
        .w_full()
        .h(px(shown))
        .border_1()
        .border_color(rgba(p.flyout_stroke))
        .rounded(px(field::OVERLAY_RADIUS))
        .shadow(theme_shadow(p.dark, z, 1.))
        .child(
            div()
                .size_full()
                .rounded(px(field::OVERLAY_RADIUS - BORDER))
                .bg(rgb(p.menu_fill)),
        );
    [
        card,
        div()
            .absolute()
            .top(px(top))
            .left_0()
            .w_full()
            .h(px(shown))
            .overflow_hidden()
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top(px(offset - top))
                    .w_full()
                    .h(px(height))
                    .child(rows),
            ),
    ]
}
