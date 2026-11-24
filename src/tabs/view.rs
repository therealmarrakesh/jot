use std::time::Duration;

use crate::color::transparent_black;
use crate::command::{AppCommand, shortcut_tooltip};
use crate::document;
use crate::geom::size;
use crate::ids::{AnimKey, AppNode, AppUi, HitId, TabHit};
use crate::platform::CursorStyle;
use crate::theme::{OVERLAY_RADIUS, Palette, SMALL_FONT};
use crate::types::SharedStr;
use crate::ui::{AnimStatus, Brushes, Easing, box_, row, text};
use crate::widgets::icon_button;

use super::model::Tab;
use super::set::{
    BUTTON_HEIGHT, BUTTON_TOP, BUTTON_WIDTH, CLOSE_GLYPH, LIST_PAD, SCROLL_INNER_PAD,
    SCROLL_OUTER_PAD, TAB_HEIGHT, TAB_WIDTH, TabSet,
};

const TAB_LABEL_INSET: f32 = 8.;
const TAB_LABEL_TRAILING: f32 = 44.;
const TAB_LABEL_TRAILING_BARE: f32 = 8.;
const UNSAVED_DOT: f32 = 6.;
const UNSAVED_DOT_TOP: f32 = 13.;
const UNSAVED_DOT_RIGHT: f32 = 19.;
const CLOSE_BUTTON_TOP: f32 = 5.;
const CLOSE_BUTTON_RIGHT: f32 = 6.;
const SCROLL_GLYPH_SIZE: f32 = 8.;
const SLIDE_DURATION: Duration = Duration::from_millis(200);
const SLIDE_EASING: Easing = Easing::CubicBezier {
    x1: 1. / 3.,
    y1: 2. / 3.,
    x2: 2. / 3.,
    y2: 1.,
};
const CARET_LEFT: &str = "\u{EDD9}";
const CARET_RIGHT: &str = "\u{EDDA}";
const ADD_GLYPH: &str = "\u{E710}";
const BUTTON_GLYPH_SIZE: f32 = 12.;

pub(crate) struct StripFrame {
    shift: f32,
    viewport: f32,
    tabs: Vec<(f32, f32)>,
    ghosts: Vec<(SharedStr, f32, f32, bool)>,
}

impl TabSet {
    pub(crate) fn sample(&mut self, column: f32, ui: &mut AppUi) -> StripFrame {
        if column != self.column {
            self.column = column;
            self.snap();
            self.layout();
            self.reveal_tab(self.active);
        }
        let snap = std::mem::take(&mut self.snap_layout);
        let snap_scroll = std::mem::take(&mut self.snap_scroll);
        let (scroll, _) = slide(AnimKey::TabScroll, 0., self.scroll, snap_scroll, ui);
        let (extent, _) = slide(AnimKey::TabExtent, TAB_WIDTH, self.extent, snap, ui);
        let mut right = 0.;
        let mut lifted_moving = false;
        let mut sampled_tabs = Vec::with_capacity(self.tabs.len());
        for ix in 0..self.tabs.len() {
            let tab = &self.tabs[ix];
            let id = tab.data.id;
            let dragged = self.drag.is_some_and(|drag| drag.id == id);
            let (x, moving) = slide(
                AnimKey::TabX(tab.slot.0),
                right,
                self.tab_target(ix),
                snap || dragged,
                ui,
            );
            let (width, _) = slide(AnimKey::TabWidth(tab.slot.0), 0., self.width, snap, ui);
            if self.lifted == Some(id) {
                lifted_moving = moving;
            }
            right = x + width;
            sampled_tabs.push((x, width));
        }
        let mut ghosts = Vec::new();
        self.ghosts.retain(|ghost| {
            let (x, sliding) = slide(AnimKey::TabX(ghost.key.0), ghost.x, ghost.x, false, ui);
            let (width, shrinking) = slide(AnimKey::TabWidth(ghost.key.0), 0., 0., false, ui);
            let moving = sliding || shrinking;
            if moving {
                ghosts.push((ghost.name.clone(), x, width, ghost.was_active));
            }
            moving
        });
        if self.drag.is_none() && !lifted_moving {
            self.lifted = None;
        }
        let viewport = if self.overflow {
            self.scroll_viewport()
        } else {
            LIST_PAD + extent
        };
        StripFrame {
            shift: LIST_PAD - scroll,
            viewport,
            tabs: sampled_tabs,
            ghosts,
        }
    }

    pub(crate) fn render(&self, sampled: &StripFrame, p: Palette, ui: &mut AppUi) -> AppNode {
        let shift = sampled.shift;
        let viewport = sampled.viewport;
        let lifted = self.lifted.and_then(|id| self.index_of(id));
        let cover = lifted.map(|ix| {
            let (x, width) = sampled.tabs[ix];
            (shift + x, shift + x + width)
        });
        let hovered_ix = self
            .tabs
            .iter()
            .position(|t| ui.is_hovered(HitId::Tab(TabHit::Item(t.data.id))));

        let ghosts: Vec<AppNode> = sampled
            .ghosts
            .iter()
            .map(|(name, x, width, was_active)| {
                let bg = if *was_active {
                    p.layer
                } else {
                    transparent_black()
                };
                tab_frame(
                    shift + x,
                    *width,
                    cover,
                    tab_face(
                        p,
                        *was_active,
                        bg,
                        TAB_LABEL_TRAILING_BARE,
                        name.clone(),
                    ),
                )
            })
            .collect();

        let tab_nodes: Vec<AppNode> = (0..self.tabs.len())
            .filter(|&ix| Some(ix) != lifted)
            .chain(lifted)
            .map(|ix| {
                let (x, width) = sampled.tabs[ix];
                tab_frame(
                    shift + x,
                    width,
                    cover.filter(|_| Some(ix) != lifted),
                    render_tab(&self.tabs, ix, self.active, hovered_ix, p, ui),
                )
            })
            .collect();

        let scroll_back = self.buttons.then(|| self.scroll_button(false, p, ui));
        let scroll_fwd = self.buttons.then(|| self.scroll_button(true, p, ui));
        let new_tab = icon_button(
            p,
            HitId::Tab(TabHit::New),
            shortcut_tooltip("New tab", AppCommand::NewTab.into()),
            (ADD_GLYPH, BUTTON_GLYPH_SIZE),
            size(BUTTON_WIDTH, BUTTON_HEIGHT),
            true,
            ui,
        )
        .ml(self.new_tab_gap())
        .mt(BUTTON_TOP);

        row()
            .hit(HitId::Tab(TabHit::Strip))
            .h(TAB_HEIGHT)
            .when_some(scroll_back, |this, btn| this.child(btn))
            .child(
                box_()
                    .hit(HitId::Tab(TabHit::List))
                    .flex_none()
                    .h_full()
                    .w(viewport)
                    .overflow_hidden()
                    .children(ghosts)
                    .children(tab_nodes),
            )
            .when_some(scroll_fwd, |this, btn| this.child(btn))
            .child(new_tab)
    }

    fn scroll_button(&self, forward: bool, p: Palette, ui: &mut AppUi) -> AppNode {
        let target = self.scroll;
        let enabled = if forward {
            target < self.max_scroll() - 0.1
        } else {
            target > 0.1
        };
        let (hit, tip, glyph) = if forward {
            (
                HitId::Tab(TabHit::ScrollRight),
                "Scroll tab list forward",
                CARET_RIGHT,
            )
        } else {
            (
                HitId::Tab(TabHit::ScrollLeft),
                "Scroll tab list backward",
                CARET_LEFT,
            )
        };
        let (left, right) = if forward {
            (SCROLL_INNER_PAD, SCROLL_OUTER_PAD)
        } else {
            (SCROLL_OUTER_PAD, SCROLL_INNER_PAD)
        };
        let button = icon_button(
            p,
            hit,
            tip,
            (glyph, SCROLL_GLYPH_SIZE),
            size(BUTTON_WIDTH, BUTTON_HEIGHT),
            enabled,
            ui,
        )
        .mt(BUTTON_TOP)
        .when(enabled, |this| this.text_color(p.text_secondary));
        box_()
            .flex_none()
            .h_full()
            .pl(left)
            .pr(right)
            .child(button)
    }
}

fn slide(key: AnimKey, seed: f32, target: f32, snap: bool, ui: &mut AppUi) -> (f32, bool) {
    let duration = if snap { Duration::ZERO } else { SLIDE_DURATION };
    let sample = ui.animate_f32_from(key, seed, target, duration, SLIDE_EASING);
    (sample.value.round(), sample.status == AnimStatus::Running)
}

fn visible(x: f32, right: f32, cover: Option<(f32, f32)>) -> (f32, f32) {
    match cover {
        Some((left, end)) if left < right && x < end => {
            if left <= x {
                (end.min(right), right)
            } else {
                (x, left.min(right))
            }
        }
        _ => (x, right),
    }
}

fn tab_frame(x: f32, width: f32, cover: Option<(f32, f32)>, content: AppNode) -> AppNode {
    let (left, right) = visible(x, x + width, cover);
    box_()
        .absolute()
        .top(0.)
        .left(left)
        .w((right - left).max(0.))
        .h(TAB_HEIGHT)
        .overflow_hidden()
        .child(
            box_()
                .absolute()
                .top(0.)
                .left(x - left)
                .w(width)
                .h_full()
                .child(content),
        )
}

fn render_tab(
    tabs: &[Tab],
    ix: usize,
    active_ix: usize,
    hovered_ix: Option<usize>,
    p: Palette,
    ui: &mut AppUi,
) -> AppNode {
    let tab = &tabs[ix];
    let id = tab.data.id;
    let active = ix == active_ix;
    let is_hovered = hovered_ix == Some(ix);
    let separator =
        !active && ix + 1 != active_ix && hovered_ix != Some(ix) && hovered_ix != Some(ix + 1);
    let dirty = tab.data.is_dirty();
    let show_close = is_hovered || (active && !dirty);
    let show_dot = dirty && !show_close;
    let trailing = if show_close || show_dot {
        TAB_LABEL_TRAILING
    } else {
        TAB_LABEL_TRAILING_BARE
    };
    let inset = if active { 1. } else { 0. };
    let name = document::display_name(tab.data.path());
    let tip: SharedStr = tab
        .data
        .path()
        .map_or_else(|| name.clone(), |path| path.display().to_string())
        .into();

    let hit = HitId::Tab(TabHit::Item(id));
    let face_bg = if active {
        p.layer
    } else {
        ui.brush(
            hit,
            Brushes {
                rest: transparent_black(),
                hover: p.layer_hover,
                pressed: p.layer_hover,
            },
            true,
        )
    };

    let close_node = show_close.then(|| {
        icon_button(
            p,
            HitId::Tab(TabHit::Close(id)),
            shortcut_tooltip("Close tab", AppCommand::CloseTab.into()),
            (CLOSE_GLYPH, BUTTON_GLYPH_SIZE),
            size(BUTTON_WIDTH, BUTTON_HEIGHT),
            true,
            ui,
        )
        .absolute()
        .top(CLOSE_BUTTON_TOP - inset)
        .right(CLOSE_BUTTON_RIGHT - inset)
    });

    box_()
        .hit(hit)
        .tooltip(tip)
        .cursor(CursorStyle::Arrow)
        .size_full()
        .overflow_hidden()
        .when(active, |this| {
            this.border_t(1.)
                .border_l(1.)
                .border_r(1.)
                .border_color(p.stroke)
                .rounded_t(OVERLAY_RADIUS)
        })
        .child(tab_face(p, active, face_bg, trailing, name))
        .when(show_dot, |this| this.child(unsaved_dot(p, inset)))
        .when(separator, |this| this.child(tab_separator(p)))
        .when_some(close_node, |this, btn| this.child(btn))
}

fn tab_face(
    p: Palette,
    active: bool,
    bg: crate::color::Rgba,
    trailing: f32,
    name: impl Into<SharedStr>,
) -> AppNode {
    row()
        .items_center()
        .size_full()
        .pl(TAB_LABEL_INSET)
        .pr(trailing)
        .font_family(SMALL_FONT)
        .text_size(12.)
        .bg(bg)
        .when(active, |this| {
            this.rounded_t(OVERLAY_RADIUS - 1.)
                .text_color(p.text_primary)
                .font_semibold()
        })
        .when(!active, |this| {
            this.rounded_t(OVERLAY_RADIUS)
                .text_color(p.text_secondary)
        })
        .child(
            box_()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .nowrap()
                .child(text(name)),
        )
}

fn unsaved_dot(p: Palette, inset: f32) -> AppNode {
    box_()
        .absolute()
        .top(UNSAVED_DOT_TOP - inset)
        .right(UNSAVED_DOT_RIGHT - inset)
        .size(UNSAVED_DOT)
        .rounded_full()
        .bg(p.text_tertiary)
}

fn tab_separator(p: Palette) -> AppNode {
    box_()
        .absolute()
        .right(0.)
        .top(8.)
        .w(1.)
        .h(16.)
        .bg(p.divider)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visible_clips_covered_tab_edges() {
        assert_eq!(visible(0., 100., None), (0., 100.));
        assert_eq!(visible(0., 100., Some((120., 200.))), (0., 100.));
        assert_eq!(visible(0., 100., Some((0., 40.))), (40., 100.));
        assert_eq!(visible(0., 100., Some((60., 140.))), (0., 60.));
        assert_eq!(visible(0., 100., Some((-10., 120.))), (100., 100.));
    }
}
