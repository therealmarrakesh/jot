use std::rc::Rc;
use std::time::Duration;

use crate::editor::{EditorTimer, EditorTimers};
use crate::files::{FileEvent, FileRequest, PromptAnswer, PromptId};
use crate::geom::Point;
use crate::gfx::{Gpu, TextSystem};
use crate::platform::{Jobs, Platform, SystemSettings, TimerHandle, WindowAppearance, WindowId};
use crate::theme::Palette;
use crate::ui::{EditorSlot, HitId, TabId};

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

#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) enum WindowTimer {
    Editor {
        slot: EditorSlot,
        kind: EditorTimer,
    },
    TabScrollRepeat(f32),
    Submenu(Option<usize>),
    FindNotice,
}

pub(crate) enum AppEvent {
    Window {
        window: WindowId,
        timer: WindowTimer,
    },
    TooltipShowTimer {
        window: WindowId,
        hit: HitId,
    },
    TabDragPoll,
    File(FileEvent),
}

#[derive(Copy, Clone)]
pub(crate) struct WindowTimers<'a> {
    jobs: &'a Jobs<AppEvent>,
    window: WindowId,
    settings: SystemSettings,
}

impl<'a> WindowTimers<'a> {
    pub(crate) fn after(&self, delay: Duration, timer: WindowTimer) -> TimerHandle {
        self.jobs.after(delay, AppEvent::Window { window: self.window, timer })
    }

    pub(crate) fn every(&self, delay: Duration, interval: Duration, timer: WindowTimer) -> TimerHandle {
        let window = self.window;
        self.jobs.every(delay, interval, move || AppEvent::Window { window, timer })
    }

    #[inline]
    pub(crate) fn menu_show_delay(&self) -> Duration {
        self.settings.menu_show_delay
    }

    #[inline]
    pub(crate) fn for_editor(self, slot: EditorSlot) -> SlotTimers<'a> {
        SlotTimers(self, slot)
    }

    #[cfg(test)]
    pub(crate) fn for_test(jobs: &'a Jobs<AppEvent>) -> Self {
        Self { jobs, window: WindowId(0), settings: SystemSettings::default() }
    }
}

pub(crate) struct SlotTimers<'a>(WindowTimers<'a>, EditorSlot);

impl EditorTimers for SlotTimers<'_> {
    fn after(&mut self, delay: Duration, kind: EditorTimer) -> Option<TimerHandle> {
        Some(self.0.after(delay, WindowTimer::Editor { slot: self.1, kind }))
    }

    fn every(&mut self, delay: Duration, interval: Duration, kind: EditorTimer) -> Option<TimerHandle> {
        Some(self.0.every(delay, interval, WindowTimer::Editor { slot: self.1, kind }))
    }

    fn blink_timing(&self) -> (Option<Duration>, Option<Duration>) {
        (self.0.settings.caret_blink_interval, self.0.settings.caret_blink_timeout)
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
    #[inline]
    pub(crate) fn ts(&self) -> &'a TextSystem {
        &self.shell.text_system
    }

    #[inline]
    pub(crate) fn timers(&self, window: WindowId) -> WindowTimers<'a> {
        self.shell.window_timers(window)
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
    pub(crate) fn new(platform: &Rc<Platform>, gpu: Rc<Gpu>, text_system: TextSystem) -> Self {
        let jobs = Jobs::new();
        jobs.attach_platform(platform);
        let settings = platform.settings();
        let mut shell = Self {
            text_system,
            jobs,
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

    #[inline]
    pub(crate) fn appearance(&self) -> WindowAppearance {
        self.appearance
    }

    #[inline]
    pub(crate) fn window_timers(&self, window: WindowId) -> WindowTimers<'_> {
        WindowTimers {
            jobs: &self.jobs,
            window,
            settings: self.settings,
        }
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
