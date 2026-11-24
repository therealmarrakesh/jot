use std::time::Instant;

use crate::app::Cx;
use crate::editor::{EditorPaintParams, EditorPaintStyle};
use crate::geom::{Rect, point, size};
use crate::gfx::{fill, font};
use crate::ids::{CustomDraw, EditorSlot, Focus, Frame};
use crate::ui::controls::{BODY_LINE, BODY_SIZE, TEXT_FONT};
use crate::ui::{AvailSize, box_, col};

use super::chrome::STATUS_DIVIDER_WIDTH;
use super::{EDITOR_FONT_SIZE, MainWindow};

impl MainWindow {
    pub(crate) fn render_frame(&mut self, now: Instant, cx: &Cx<'_>) -> bool {
        let p = cx.palette;
        let view = cx.view;
        let reduce_motion = cx.settings.reduce_motion;
        let caret_width_px = cx.settings.caret_width_px;
        let ts = cx.ts;
        let jobs = cx.jobs;
        self.dirty = false;
        let logical = self.native.logical_size();
        let scale = self.native.scale_factor();
        let window_active = self.native.is_active();
        let maximized = self.native.is_maximized();
        let status_metrics = view.status_bar.then(|| self.status_metrics());

        self.ui.begin_frame(now, reduce_motion);
        let strip = self.tabs.sample(logical.width, &mut self.ui);
        self.menu_bar.sample(&mut self.ui);

        let title_bar = self.title_bar(&strip, window_active, maximized, p);
        let menu_bar = self.menu_bar_row(p);
        let find_node = self
            .find
            .render(logical.width, self.focused_editor_slot(), p, &mut self.ui);
        let editor_node = self.editor_area(find_node, now, p, jobs);
        let status_node = status_metrics.map(|m| self.status_bar(m, p));
        let menu_popup = self.menu_bar.render_popup(p, ts, &mut self.ui);
        let dialog_node = self.dialogs.render(self.focus, p, &mut self.ui);

        let chrome = col()
            .size_full()
            .child(title_bar)
            .child(menu_bar)
            .child(editor_node)
            .when_some(status_node, |this, s| this.child(s));
        let root = box_()
            .relative()
            .size_full()
            .child(chrome)
            .when_some(menu_popup, |this, m| this.child(m))
            .when_some(dialog_node, |this, d| this.child(d));

        let frame = Frame::layout(
            root,
            AvailSize::definite(logical.width, logical.height),
            scale,
            ts,
        );
        self.native.set_control_areas(frame.control_areas());
        self.update_min_size(view.status_bar, &frame, p, ts);

        let editor_style = EditorPaintStyle {
            font: font("Consolas"),
            font_size: EDITOR_FONT_SIZE * self.zoom(),
            line_height: self.line_height(),
            color: p.text_primary,
        };
        let field_style = EditorPaintStyle {
            font: font(TEXT_FONT),
            font_size: BODY_SIZE,
            line_height: BODY_LINE + 2.,
            color: p.text_primary,
        };
        let caret_height = self.caret_height();
        let active_slot = self.active_slot();
        let editor_focused = self.focus == Focus::Editor(active_slot);
        let show_selection = editor_focused || self.menu_bar.is_open() || self.find.is_open();
        let caret_hidden = self.menu_bar.access_active();
        let mut editor_needs_redraw = false;

        self.active_editor_mut().set_caret_height(caret_height);
        self.active_editor_mut().set_caret_hidden(caret_hidden);

        let editor_rects: Vec<(EditorSlot, Rect)> = frame
            .custom_rects()
            .filter_map(|(token, rect)| match token {
                CustomDraw::Editor(slot) => Some((slot, rect)),
                _ => None,
            })
            .collect();
        let focus = self.focus;
        for (slot, rect) in editor_rects {
            let (style, focused, show_sel) = match slot {
                EditorSlot::Tab(_) => (&editor_style, editor_focused, show_selection),
                _ => {
                    let f = focus == Focus::Editor(slot);
                    (&field_style, f, f)
                }
            };
            if let Some(ed) = self.editor_by_slot_mut(slot) {
                let outcome = ed.prepare(
                    rect,
                    EditorPaintParams {
                        style,
                        palette: p,
                        window_active,
                        focused,
                        show_selection: show_sel,
                        scale,
                        caret_width_px,
                    },
                    ts,
                );
                editor_needs_redraw |= outcome.needs_redraw;
            }
        }

        let tabs = &self.tabs;
        let find = &self.find;
        let dialogs = &self.dialogs;
        let needs_retry = self.surface.draw(p.window_clear, |renderer| {
            frame.paint(renderer, &mut |token, rect, renderer| match token {
                CustomDraw::Editor(slot) => {
                    let ed = match slot {
                        EditorSlot::Tab(id) => tabs.editor(id),
                        EditorSlot::FindQuery => Some(&find.query),
                        EditorSlot::FindReplace => Some(&find.replacement),
                        EditorSlot::GoToLine => Some(&dialogs.line),
                    };
                    if let Some(ed) = ed {
                        ed.paint(p, renderer);
                    }
                }
                CustomDraw::StatusSpacer => {
                    let width = rect.size.width;
                    if width < STATUS_DIVIDER_WIDTH + 0.5 {
                        let x = rect.origin.x + ((width - STATUS_DIVIDER_WIDTH) / 2.).round();
                        let line = Rect::new(
                            point(x, rect.origin.y),
                            size(STATUS_DIVIDER_WIDTH, rect.size.height),
                        );
                        renderer.paint_quad(fill(line, p.stroke));
                    }
                }
            });
        });

        let ime_caret = self
            .focused_editor_slot()
            .and_then(|slot| self.editor_by_slot(slot))
            .and_then(|ed| ed.ime_caret_bounds());
        self.native
            .set_text_caret(ime_caret, window_active && !caret_hidden);

        self.has_painted = true;
        let ui_needs_vsync = self.ui.finish_frame(frame);
        if let Some(ptr) = self.ui.pointer() {
            self.sync_cursor(ptr.pos);
        }
        let needs_vsync = ui_needs_vsync || editor_needs_redraw || needs_retry;
        if needs_vsync {
            self.dirty = true;
        }
        needs_vsync
    }
}
