use std::rc::Rc;

use crate::files::{FileRequest, PromptAnswer, PromptId};
use crate::geom::Point;
use crate::gfx::{Gpu, TextSystem};
use crate::platform::{SystemSettings, WindowAppearance, WindowId};
use crate::theme::Palette;
use crate::ui::TabId;

#[derive(Copy, Clone, Debug)]
pub(crate) struct ViewSettings {
    pub(crate) status_bar: bool,
    pub(crate) word_wrap: bool,
}

impl Default for ViewSettings {
    fn default() -> Self {
        Self {
            status_bar: true,
            word_wrap: true,
        }
    }
}

pub(crate) enum Effect {
    NewWindow,
    Exit,
    ToggleStatusBar,
    ToggleTheme,
    ToggleWordWrap,
    StartTabDrag {
        window: WindowId,
        tab: TabId,
        index: usize,
        grab: Point,
        position: Point,
    },
    File(FileRequest),
    PromptResponse {
        id: PromptId,
        answer: PromptAnswer,
    },
}

pub(crate) struct Cx<'a> {
    pub(crate) shell: &'a Shell,
    fx: &'a mut Vec<Effect>,
}

impl<'a> Cx<'a> {
    pub(crate) fn ts(&self) -> &'a TextSystem {
        &self.shell.text_system
    }

    pub(crate) fn emit(&mut self, effect: Effect) {
        self.fx.push(effect);
    }
}

pub(crate) struct Shell {
    pub(crate) text_system: TextSystem,
    appearance_override: Option<WindowAppearance>,
    appearance: WindowAppearance,
    pub(crate) palette: Palette,
    pub(crate) settings: SystemSettings,
    pub(crate) view: ViewSettings,
    pub(crate) gpu: Rc<Gpu>,
}

impl Shell {
    pub(crate) fn new(gpu: Rc<Gpu>, text_system: TextSystem) -> Self {
        let settings = SystemSettings::query();
        let mut shell = Self {
            text_system,
            appearance_override: None,
            appearance: settings.appearance,
            palette: crate::theme::LIGHT,
            settings,
            view: ViewSettings::default(),
            gpu,
        };
        shell.rebuild_palette();
        shell
    }

    pub(crate) fn appearance(&self) -> WindowAppearance {
        self.appearance
    }

    pub(crate) fn toggle_theme(&mut self) {
        let next = if self.palette.dark {
            WindowAppearance::Light
        } else {
            WindowAppearance::Dark
        };
        self.appearance_override = Some(next);
        self.rebuild_palette();
    }

    pub(crate) fn apply_settings(&mut self, settings: SystemSettings) {
        self.settings = settings;
        self.rebuild_palette();
    }

    fn rebuild_palette(&mut self) {
        let next = self.appearance_override.unwrap_or(self.settings.appearance);
        let dark = matches!(next, WindowAppearance::Dark);
        self.appearance = next;
        self.palette =
            Palette::resolve(dark, self.settings.accent(dark), self.settings.high_contrast);
    }

    pub(crate) fn cx<'a>(&'a self, fx: &'a mut Vec<Effect>) -> Cx<'a> {
        Cx { shell: self, fx }
    }
}
