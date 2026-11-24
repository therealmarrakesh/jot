use std::ops::{Index, IndexMut};
use std::path::PathBuf;

use crate::APP_NAME;
use crate::document::{self, DocPath, Format, PathKey};
use crate::editor::Rope;
use crate::files::{DocHost, Prompt, TabSnapshot};
use crate::geom::{Point, Rect, ScreenPoint, Size, point, size};
use crate::gfx::Surface;
use crate::ids::TabId;
use crate::platform::{
    MainWindowConfig, NativeWindow, WindowAppearance, WindowHwnd, WindowId, WindowsDisplay,
    cursor_screen_pos, diag,
};
use crate::shell::Shell;
use crate::tabs::TabData;
use crate::window::{MainWindow, WINDOW_HEIGHT, WINDOW_MAX_SCREEN_SHARE, WINDOW_WIDTH};

pub(crate) struct OpenWindowError {
    pub(crate) tab: Option<TabData>,
    pub(crate) message: String,
}

impl std::fmt::Debug for OpenWindowError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenWindowError")
            .field("has_tab", &self.tab.is_some())
            .field("message", &self.message)
            .finish()
    }
}

pub(crate) struct Windows {
    list: Vec<MainWindow>,
    active: Option<WindowId>,
    cascade_step: u32,
    #[cfg(test)]
    headless: bool,
}

impl Windows {
    pub(crate) fn new() -> Self {
        Self {
            list: Vec::new(),
            active: None,
            cascade_step: 0,
            #[cfg(test)]
            headless: false,
        }
    }

    #[cfg(test)]
    pub(crate) fn set_headless(&mut self, headless: bool) {
        self.headless = headless;
    }

    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.list.len()
    }

    #[inline]
    pub(crate) fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    #[inline]
    pub(crate) fn iter(&self) -> std::slice::Iter<'_, MainWindow> {
        self.list.iter()
    }

    #[cfg(test)]
    #[inline]
    pub(crate) fn iter_mut(&mut self) -> std::slice::IterMut<'_, MainWindow> {
        self.list.iter_mut()
    }

    #[cfg(test)]
    #[inline]
    pub(crate) fn active_id(&self) -> Option<WindowId> {
        self.active
    }

    #[inline]
    pub(crate) fn set_active(&mut self, id: WindowId) {
        self.active = Some(id);
    }

    pub(crate) fn ix(&self, id: WindowId) -> Option<usize> {
        self.list.iter().position(|w| w.id() == id)
    }

    pub(crate) fn get(&self, id: WindowId) -> Option<&MainWindow> {
        self.list.iter().find(|w| w.id() == id)
    }

    pub(crate) fn get_mut(&mut self, id: WindowId) -> Option<&mut MainWindow> {
        self.list.iter_mut().find(|w| w.id() == id)
    }

    pub(crate) fn hwnd(&self, id: WindowId) -> Option<WindowHwnd> {
        self.get(id).map(MainWindow::hwnd)
    }

    pub(crate) fn ids(&self) -> Vec<WindowId> {
        self.list.iter().map(MainWindow::id).collect()
    }

    pub(crate) fn owner_of(&self, tab: TabId) -> Option<(usize, usize)> {
        self.list
            .iter()
            .enumerate()
            .find_map(|(wix, w)| w.tabs().index_of(tab).map(|tix| (wix, tix)))
    }

    pub(crate) fn find_by_key(&self, key: &PathKey) -> Option<(WindowId, TabId)> {
        self.list.iter().find_map(|w| {
            let tix = w.tabs().find_by_key(key)?;
            Some((w.id(), w.tabs()[tix].data.id))
        })
    }

    pub(crate) fn fallback(&self, preferred: Option<WindowId>) -> Option<WindowId> {
        preferred
            .and_then(|id| self.ix(id))
            .or_else(|| self.active.and_then(|id| self.ix(id)))
            .or_else(|| (!self.list.is_empty()).then_some(self.list.len() - 1))
            .map(|ix| self.list[ix].id())
    }

    pub(crate) fn mark_dirty(&mut self, id: WindowId) {
        if let Some(w) = self.get_mut(id) {
            w.mark_dirty();
        }
    }

    pub(crate) fn mark_all_dirty(&mut self) {
        for w in &mut self.list {
            w.mark_dirty();
        }
    }

    pub(crate) fn apply_theme(&mut self, appearance: WindowAppearance) {
        for w in &mut self.list {
            w.apply_theme(appearance);
        }
    }

    pub(crate) fn set_word_wrap(&mut self, shell: &Shell) {
        for w in &mut self.list {
            w.set_word_wrap(shell);
        }
    }

    pub(crate) fn any_dirty(&self) -> bool {
        self.list.iter().any(|w| w.tabs().has_dirty())
    }

    pub(crate) fn rescue_tab(
        &mut self,
        preferred: Option<WindowId>,
        index: usize,
        tab: TabData,
        shell: &Shell,
    ) {
        if let Some(id) = preferred
            && let Some(win) = self.get_mut(id)
        {
            win.restore_tab(tab, index, shell);
        } else if let Some(win) = self.list.last_mut() {
            let ix = win.tabs().len();
            win.restore_tab(tab, ix, shell);
        }
    }

    pub(crate) fn sweep_closed(&mut self) -> Vec<WindowId> {
        if !self.list.iter().any(MainWindow::is_pending_close) {
            return Vec::new();
        }
        let closed: Vec<WindowId> = self
            .list
            .iter()
            .filter(|w| w.is_pending_close())
            .map(MainWindow::id)
            .collect();
        self.list.retain(|w| !w.is_pending_close());
        if let Some(active) = self.active
            && self.get(active).is_none()
        {
            self.active = self.list.last().map(MainWindow::id);
        }
        closed
    }

    fn window_config(
        &mut self,
        window_size: Option<Size>,
        target_point: Option<ScreenPoint>,
    ) -> MainWindowConfig {
        let display = if let Some(pt) = target_point {
            WindowsDisplay::from_point(pt).or_else(WindowsDisplay::primary)
        } else if let Some(origin) = self
            .active
            .and_then(|id| self.get(id))
            .map(|w| w.hwnd().client_origin())
        {
            WindowsDisplay::from_point(origin).or_else(WindowsDisplay::primary)
        } else {
            #[cfg(test)]
            let fallback = if self.headless {
                WindowsDisplay::primary()
            } else {
                WindowsDisplay::from_point(cursor_screen_pos()).or_else(WindowsDisplay::primary)
            };
            #[cfg(not(test))]
            let fallback =
                WindowsDisplay::from_point(cursor_screen_pos()).or_else(WindowsDisplay::primary);
            fallback
        };
        let visible = display.map(|d| d.visible_bounds());
        let screen = visible.map_or(size(WINDOW_WIDTH, WINDOW_HEIGHT), |v| v.size);
        let target_size = window_size.unwrap_or_else(|| {
            size(
                WINDOW_WIDTH.min(screen.width * WINDOW_MAX_SCREEN_SHARE),
                WINDOW_HEIGHT.min(screen.height * WINDOW_MAX_SCREEN_SHARE),
            )
        });
        let mut bounds = display.map_or_else(
            || Rect::new(Point::ZERO, target_size),
            |d| d.centered_rect(target_size),
        );
        if target_point.is_none() && !self.list.is_empty() && window_size.is_none() {
            let step = (self.cascade_step % 8) as f32 * 28.0;
            self.cascade_step = self.cascade_step.wrapping_add(1);
            if let Some(vis) = visible {
                let max_x = (vis.right() - target_size.width).max(vis.left());
                let max_y = (vis.bottom() - target_size.height).max(vis.top());
                bounds.origin = point(
                    (bounds.origin.x + step).clamp(vis.left(), max_x),
                    (bounds.origin.y + step).clamp(vis.top(), max_y),
                );
            } else {
                bounds.origin = point(bounds.origin.x + step, bounds.origin.y + step);
            }
        }
        #[cfg(test)]
        let (focus, show) = (!self.headless, !self.headless);
        #[cfg(not(test))]
        let (focus, show) = (true, true);
        MainWindowConfig {
            bounds: Some(bounds),
            title: APP_NAME.into(),
            display_id: display.map(|d| d.id()),
            focus,
            show,
        }
    }

    pub(crate) fn open(
        &mut self,
        initial: Option<TabData>,
        window_size: Option<Size>,
        shell: &Shell,
    ) -> std::result::Result<(WindowId, WindowHwnd), Box<OpenWindowError>> {
        self.open_at(initial, window_size, None, shell)
    }

    pub(crate) fn open_at(
        &mut self,
        initial: Option<TabData>,
        window_size: Option<Size>,
        target_point: Option<ScreenPoint>,
        shell: &Shell,
    ) -> std::result::Result<(WindowId, WindowHwnd), Box<OpenWindowError>> {
        let config = self.window_config(window_size, target_point);
        let (native, surface) =
            match NativeWindow::create_main(&config, shell.appearance()).and_then(|n| {
                let (w, h) = n.physical_size();
                let s = Surface::new(
                    shell.gpu.clone(),
                    n.hwnd().raw(),
                    w,
                    h,
                    n.scale_factor(),
                )?;
                Ok((n, s))
            }) {
                Ok(created) => created,
                Err(err) => {
                    let message = err.to_string();
                    diag::debug_log(&format!("window/surface create failed: {message}"));
                    return Err(Box::new(OpenWindowError {
                        tab: initial,
                        message,
                    }));
                }
            };
        let id = native.id;
        let hwnd = native.hwnd();
        let win = MainWindow::new(native, surface, initial, shell);
        self.list.push(win);
        self.active = Some(id);
        Ok((id, hwnd))
    }

    pub(crate) fn host<'a>(&'a mut self, shell: &'a Shell) -> WindowsHost<'a> {
        WindowsHost {
            windows: self,
            shell,
        }
    }
}

impl Index<usize> for Windows {
    type Output = MainWindow;

    #[inline]
    fn index(&self, index: usize) -> &Self::Output {
        &self.list[index]
    }
}

impl IndexMut<usize> for Windows {
    #[inline]
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        &mut self.list[index]
    }
}

pub(crate) struct WindowsHost<'a> {
    windows: &'a mut Windows,
    shell: &'a Shell,
}

impl DocHost for WindowsHost<'_> {
    fn tab_info(&self, tab: TabId) -> Option<TabSnapshot> {
        let (wix, tix) = self.windows.owner_of(tab)?;
        let w = &self.windows[wix];
        let tabs = w.tabs();
        let data = &tabs[tix].data;
        Some(TabSnapshot {
            window: w.id(),
            name: tabs.tab_name(tix),
            path: data.path().map(PathBuf::from),
            format: data.format,
            dirty: data.is_dirty(),
            revision: data.editor.model().revision(),
        })
    }

    fn window_dirty_tabs(&self, window: WindowId) -> Vec<TabId> {
        self.windows
            .get(window)
            .map(|w| w.tabs().dirty_tabs())
            .unwrap_or_default()
    }

    fn active_tab_path(&self, window: WindowId) -> Option<PathBuf> {
        self.windows
            .get(window)?
            .tabs()
            .active()
            .path()
            .map(PathBuf::from)
    }

    fn fallback_window(&self, preferred: WindowId) -> Option<WindowId> {
        self.windows.fallback(Some(preferred))
    }

    fn find_tab_by_key(&self, key: &PathKey) -> Option<(WindowId, TabId)> {
        self.windows.find_by_key(key)
    }

    fn activate_tab(&mut self, window: WindowId, tab: TabId, bring_to_front: bool) {
        let Some(w) = self.windows.get_mut(window) else {
            return;
        };
        let Some(tix) = w.tabs().index_of(tab) else {
            return;
        };
        let hwnd = w.hwnd();
        w.activate_tab(tix, self.shell);
        if bring_to_front {
            hwnd.bring_to_front();
        }
    }

    fn begin_save(&mut self, tab: TabId) -> Option<(u64, Rope)> {
        let (wix, _) = self.windows.owner_of(tab)?;
        self.windows[wix].begin_tab_save(tab)
    }

    fn mark_saved(&mut self, tab: TabId, snapshot: u64, doc_path: DocPath, format: Format) {
        if let Some((wix, _)) = self.windows.owner_of(tab) {
            self.windows[wix].mark_tab_saved(tab, snapshot, doc_path, format);
        }
    }

    fn adopt_loaded(&mut self, window: WindowId, loaded: document::Loaded) {
        if let Some(w) = self.windows.get_mut(window) {
            w.adopt_loaded_file(loaded, self.shell);
        }
    }

    fn remove_tab(&mut self, tab: TabId) {
        if let Some((wix, tix)) = self.windows.owner_of(tab) {
            self.windows[wix].remove_tab(tix, self.shell);
        }
    }

    fn mark_window_close(&mut self, window: WindowId) {
        if let Some(w) = self.windows.get_mut(window) {
            w.request_close();
        }
    }

    fn show_prompt(&mut self, window: WindowId, prompt: Prompt) {
        if let Some(w) = self.windows.get_mut(window) {
            w.show_prompt(prompt, self.shell);
        }
    }
}
