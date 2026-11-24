use std::time::Instant;

use crate::shell::AppEvent;
use crate::color::transparent_black;
use crate::document::Format;
use crate::geom::{Size, size};
use crate::gfx::TextSystem;
use crate::ids::{CustomDraw, Focus, Frame, HitId, Node, UiState};
use crate::menu::MENU_BAR_HEIGHT;
use crate::platform::{Jobs, WindowControlArea};
use crate::tabs::{self, CAPTION_BUTTON_WIDTH, STRIP_TOP, StripFrame, TAB_HEIGHT};
use crate::theme::Palette;
use crate::ui::controls::{CONTROL_RADIUS, ICON_FONT, IconButtonStyle, SMALL_FONT, icon_button};
use crate::ui::{AvailSize, Brushes, box_, custom, row, text};

use super::{EDITOR_FONT_SIZE, MainWindow, WindowEditorTimers};

const SEPARATOR_HEIGHT: f32 = 1.;
const MENU_BAR_LEFT: f32 = 4.;
const STATUS_BAR_HEIGHT: f32 = 31.;
pub(super) const STATUS_DIVIDER_WIDTH: f32 = 1.;
const STATUS_DIVIDER_HEIGHT: f32 = 16.;
const THEME_BUTTON_WIDTH: f32 = 30.;
const THEME_BUTTON_RIGHT: f32 = 8.;
const THEME_GLYPH_SIZE: f32 = 16.;
const MOON_GLYPH: &str = "\u{E708}";
const SUN_GLYPH: &str = "\u{E706}";

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct StatusMetrics {
    line: usize,
    column: usize,
    selected: usize,
    total: usize,
    zoom_pct: u32,
    format: Format,
}

#[derive(Clone, Copy, PartialEq)]
struct MinSizeKey {
    status: Option<StatusMetrics>,
    zoom_pct: u32,
    menu_width: f32,
    scale: f32,
}

#[derive(Clone, Copy)]
pub(super) struct MinSizeCache {
    key: MinSizeKey,
    pub(super) size: Size,
}

impl MainWindow {
    pub(super) fn status_metrics(&self) -> StatusMetrics {
        let editor = self.active_editor();
        let text = editor.text();
        let (line, column) = editor.caret_line_col();
        let selection = editor.selected_range();
        let selected = text
            .byte_to_char_idx(selection.end)
            .saturating_sub(text.byte_to_char_idx(selection.start));
        StatusMetrics {
            line,
            column,
            selected,
            total: text.len_chars(),
            zoom_pct: (self.zoom() * 100.).round() as u32,
            format: self.tabs.active().format,
        }
    }

    pub(super) fn update_min_size(
        &mut self,
        metrics: Option<StatusMetrics>,
        frame: &Frame,
        p: Palette,
        ts: &TextSystem,
    ) {
        let zoom_pct = metrics.map_or((self.zoom() * 100.).round() as u32, |m| m.zoom_pct);
        let scale = self.native.scale_factor();
        let menu_width = frame
            .rect_of(HitId::MenuBar)
            .map_or(0.0, |r| r.size.width);
        let key = MinSizeKey {
            status: metrics,
            zoom_pct,
            menu_width,
            scale,
        };
        if self
            .min_size_cache
            .is_some_and(|cached| cached.key == key)
        {
            return;
        }
        let status_sz =
            metrics.map(|m| Frame::measure(self.status_bar(m, p), AvailSize::MIN, scale, ts));
        let menu_row = MENU_BAR_LEFT + menu_width + THEME_BUTTON_WIDTH + THEME_BUTTON_RIGHT;
        let width = status_sz
            .map_or(0., |s| s.width)
            .max(menu_row)
            .max(tabs::TITLE_BAR_MIN_WIDTH);
        let chrome_top = frame.rect_of(HitId::EditorArea).map_or(0., |r| r.top());
        let height = chrome_top + self.editor_min_height() + status_sz.map_or(0., |s| s.height);
        let next = size(width, height);
        let changed = self
            .min_size_cache
            .is_none_or(|cached| cached.size != next);
        self.min_size_cache = Some(MinSizeCache { key, size: next });
        if changed {
            self.native.set_min_size(Some(next));
        }
    }

    pub(super) fn title_bar(
        &mut self,
        strip: &StripFrame,
        window_active: bool,
        maximized: bool,
        p: Palette,
    ) -> Node {
        let strip_node = self.tabs.render(strip, p, &mut self.ui);
        let captions = caption_buttons(window_active, maximized, p, &mut self.ui);
        row()
            .flex_none()
            .h(STRIP_TOP + TAB_HEIGHT)
            .child(
                box_()
                    .flex_none()
                    .w(tabs::HEADER_WIDTH)
                    .h_full()
                    .window_area(WindowControlArea::Drag),
            )
            .child(
                box_()
                    .flex_none()
                    .child(
                        box_()
                            .h(STRIP_TOP)
                            .window_area(WindowControlArea::Drag),
                    )
                    .child(strip_node),
            )
            .child(
                box_()
                    .flex_1()
                    .h_full()
                    .window_area(WindowControlArea::Drag),
            )
            .child(captions)
    }

    pub(super) fn menu_bar_row(&mut self, p: Palette) -> Node {
        let menu = self
            .menu_bar
            .render_bar(self.focus == Focus::MenuBar, p, &mut self.ui);
        let theme_btn = icon_button(
            p,
            HitId::ThemeToggle,
            "Switch theme",
            if p.dark { SUN_GLYPH } else { MOON_GLYPH },
            IconButtonStyle {
                glyph_size: THEME_GLYPH_SIZE,
                width: THEME_BUTTON_WIDTH,
                height: MENU_BAR_HEIGHT,
                radius: CONTROL_RADIUS,
            },
            true,
            &mut self.ui,
        );
        box_()
            .flex_none()
            .child(
                row()
                    .h(MENU_BAR_HEIGHT)
                    .pl(MENU_BAR_LEFT)
                    .pr(THEME_BUTTON_RIGHT)
                    .bg(p.layer)
                    .child(menu)
                    .child(box_().flex_1())
                    .child(theme_btn),
            )
            .child(separator(p))
    }

    pub(super) fn editor_area(
        &mut self,
        find: Option<Node>,
        now: Instant,
        p: Palette,
        jobs: &Jobs<AppEvent>,
    ) -> Node {
        let slot = self.active_slot();
        let lh = self.line_height();
        let wid = self.id();
        let blink_timing = self.blink_timing;
        let mut timers = WindowEditorTimers::new(jobs, wid, slot, blink_timing);
        let scrollbars = self
            .tabs
            .active_mut()
            .editor
            .render_scrollbars(slot, lh, p, now, &mut self.ui, &mut timers);
        let editor_view = self.tabs.active().editor.view_with_scrollbars(
            HitId::Editor(slot),
            CustomDraw::Editor(slot),
            lh,
            scrollbars,
        );
        box_()
            .hit(HitId::EditorArea)
            .flex_1()
            .min_h(self.editor_min_height())
            .bg(p.editor)
            .child(
                box_()
                    .size_full()
                    .overflow_hidden()
                    .child(
                        box_()
                            .size_full()
                            .font_family("Consolas")
                            .text_size(EDITOR_FONT_SIZE * self.zoom())
                            .line_height(lh)
                            .text_color(p.text_primary)
                            .child(editor_view),
                    ),
            )
            .when_some(find, |this, f| this.child(f))
    }

    #[cfg(test)]
    pub(crate) fn status_bar_for_test(&self, p: Palette) -> crate::ids::Node {
        self.status_bar(self.status_metrics(), p)
    }

    pub(super) fn status_bar(&self, metrics: StatusMetrics, p: Palette) -> Node {
        box_().flex_none().child(separator(p)).child(
            row()
                .items_center()
                .gap(24.)
                .h(STATUS_BAR_HEIGHT)
                .px(22.)
                .bg(p.layer)
                .font_family(SMALL_FONT)
                .text_size(12.)
                .text_color(p.text_secondary)
                .nowrap()
                .text_child(format!("Ln {}, Col {}", metrics.line, metrics.column))
                .child(status_divider(p))
                .text_child(character_count(metrics.selected, metrics.total))
                .child(
                    custom(CustomDraw::StatusSpacer)
                        .flex_1()
                        .min_w(STATUS_DIVIDER_WIDTH)
                        .h(STATUS_DIVIDER_HEIGHT),
                )
                .text_child(format!("{}%", metrics.zoom_pct))
                .child(status_divider(p))
                .text_child(metrics.format.line_ending_label())
                .child(status_divider(p))
                .text_child(metrics.format.encoding.label()),
        )
    }
}

fn separator(p: Palette) -> Node {
    box_()
        .flex_none()
        .h(SEPARATOR_HEIGHT)
        .bg(p.stroke_on_layer)
}

fn status_divider(p: Palette) -> Node {
    box_()
        .flex_none()
        .w(STATUS_DIVIDER_WIDTH)
        .h(STATUS_DIVIDER_HEIGHT)
        .bg(p.stroke)
}

fn group_thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn character_count(selected: usize, total: usize) -> String {
    let noun = if total == 1 {
        "character"
    } else {
        "characters"
    };
    if selected > 0 {
        format!(
            "{} of {} {noun}",
            group_thousands(selected),
            group_thousands(total)
        )
    } else {
        format!("{} {noun}", group_thousands(total))
    }
}

#[derive(Clone, Copy)]
enum Caption {
    Minimize,
    Maximize,
    Restore,
    Close,
}

impl Caption {
    fn hit(self) -> HitId {
        match self {
            Self::Minimize => HitId::CaptionMin,
            Self::Maximize | Self::Restore => HitId::CaptionMax,
            Self::Close => HitId::CaptionClose,
        }
    }

    fn glyph(self) -> &'static str {
        match self {
            Self::Minimize => "\u{E921}",
            Self::Maximize => "\u{E922}",
            Self::Restore => "\u{E923}",
            Self::Close => "\u{E8BB}",
        }
    }

    fn area(self) -> WindowControlArea {
        match self {
            Self::Minimize => WindowControlArea::Min,
            Self::Maximize | Self::Restore => WindowControlArea::Max,
            Self::Close => WindowControlArea::Close,
        }
    }

    fn render(self, active: bool, p: Palette, ui: &mut UiState) -> Node {
        let hit = self.hit();
        let (hover_bg, pressed_bg, hover_fg, pressed_fg) = match self {
            Self::Close => (
                p.close_hover,
                p.close_pressed,
                p.close_glyph_hover,
                p.close_glyph_pressed,
            ),
            _ => (
                p.layer_hover,
                p.subtle_tertiary,
                p.caption_glyph,
                p.caption_glyph_pressed,
            ),
        };
        let rest_fg = if active {
            p.caption_glyph
        } else {
            p.caption_glyph_inactive
        };
        let bg = ui.brush(
            hit,
            Brushes {
                rest: transparent_black(),
                hover: hover_bg,
                pressed: pressed_bg,
            },
            true,
        );
        let fg = if ui.is_pressed(hit) {
            pressed_fg
        } else if ui.is_hovered(hit) {
            hover_fg
        } else {
            rest_fg
        };

        row()
            .hit(hit)
            .flex_none()
            .w(CAPTION_BUTTON_WIDTH)
            .h(30.)
            .items_center()
            .justify_center()
            .bg(bg)
            .font_family(ICON_FONT)
            .text_size(10.)
            .text_color(fg)
            .window_area(self.area())
            .child(
                box_()
                    .when(matches!(self, Self::Minimize), |this| {
                        this.left(1.).top(1.)
                    })
                    .child(text(self.glyph())),
            )
    }
}

fn caption_buttons(active: bool, maximized: bool, p: Palette, ui: &mut UiState) -> Node {
    let maximize = if maximized {
        Caption::Restore
    } else {
        Caption::Maximize
    };
    box_()
        .flex_none()
        .h_full()
        .child(
            row()
                .child(Caption::Minimize.render(active, p, ui))
                .child(maximize.render(active, p, ui))
                .child(Caption::Close.render(active, p, ui)),
        )
        .child(box_().flex_1().window_area(WindowControlArea::Drag))
}
