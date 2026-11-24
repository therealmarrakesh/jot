use std::rc::Rc;

use crate::editor::EditorTimer;
use crate::files::{FileEvent, FileRequest, PromptId};
use crate::geom::Point;
use crate::gfx::{Gpu, TextSystem};
use crate::ids::{EditorSlot, HitId, TabId};
use crate::platform::{
    Jobs, Platform, SystemSettings, WindowAppearance, WindowId, system_settings,
};
use crate::theme::Palette;

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

pub(crate) enum AppEvent {
    EditorTimer {
        window: WindowId,
        slot: EditorSlot,
        kind: EditorTimer,
    },
    TabScrollRepeat {
        window: WindowId,
        step: f32,
    },
    MenuSubmenuTimer {
        window: WindowId,
        target: Option<usize>,
    },
    FindNoticeDismiss {
        window: WindowId,
    },
    TooltipShowTimer {
        window: WindowId,
        hit: HitId,
    },
    TabDragPoll,
    File(FileEvent),
    #[cfg(test)]
    TestCloseWindow(WindowId),
    #[cfg(test)]
    TestFlag(std::sync::Arc<std::sync::atomic::AtomicBool>),
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
        button: usize,
    },
}

pub(crate) struct Cx<'a> {
    pub(crate) shell: &'a Shell,
    fx: &'a mut Vec<Effect>,
}

impl<'a> Cx<'a> {
    #[inline]
    pub(crate) fn ts(&self) -> &'a TextSystem {
        &self.shell.text_system
    }

    #[inline]
    pub(crate) fn jobs(&self) -> &'a Jobs<AppEvent> {
        &self.shell.jobs
    }

    #[inline]
    pub(crate) fn emit(&mut self, effect: Effect) {
        self.fx.push(effect);
    }
}

pub(crate) struct Shell {
    pub(crate) text_system: TextSystem,
    pub(crate) jobs: Jobs<AppEvent>,
    appearance_override: Option<WindowAppearance>,
    appearance: WindowAppearance,
    pub(crate) palette: Palette,
    pub(crate) settings: SystemSettings,
    pub(crate) view: ViewSettings,
    pub(crate) gpu: Rc<Gpu>,
}

impl Shell {
    pub(crate) fn new(platform: &Rc<Platform>, gpu: Rc<Gpu>) -> Self {
        let jobs = Jobs::new();
        jobs.attach_platform(platform);
        let settings = system_settings();
        let appearance = settings.appearance;
        let dark = matches!(appearance, WindowAppearance::Dark);
        let palette = Palette::resolve(dark, settings.accent(dark), settings.high_contrast);
        Self {
            text_system: TextSystem::new(),
            jobs,
            appearance_override: None,
            appearance,
            palette,
            settings,
            view: ViewSettings::default(),
            gpu,
        }
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
        self.sync_theme();
    }

    pub(crate) fn sync_theme(&mut self) {
        let settings = system_settings();
        self.settings = settings;
        let next = self.appearance_override.unwrap_or(settings.appearance);
        let dark = matches!(next, WindowAppearance::Dark);
        self.appearance = next;
        self.palette = Palette::resolve(dark, settings.accent(dark), settings.high_contrast);
    }

    pub(crate) fn cx<'a>(&'a self, fx: &'a mut Vec<Effect>) -> Cx<'a> {
        Cx { shell: self, fx }
    }
}
