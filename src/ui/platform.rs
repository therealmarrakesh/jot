mod keystroke;

use crate::ui::{
    App, AsyncWindowContext, Bounds, DevicePixels, Pixels, RenderGlyphParams, SharedString, Size,
    Window,
};
use std::fmt::{self, Debug};
use std::hash::{Hash, Hasher};
use std::ops::{self, Range};
use std::time::Duration;

pub(crate) use keystroke::*;

/// An opaque identifier for a hardware display.
#[derive(PartialEq, Eq, Hash, Copy, Clone)]
pub(crate) struct DisplayId(pub(crate) u64);

impl DisplayId {
    pub(crate) fn new(id: u64) -> Self {
        Self(id)
    }
}

impl From<u64> for DisplayId {
    fn from(id: u64) -> Self {
        Self(id)
    }
}

impl From<DisplayId> for u64 {
    fn from(id: DisplayId) -> Self {
        id.0
    }
}

impl Debug for DisplayId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DisplayId({})", self.0)
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Default)]
pub(crate) struct RequestFrameOptions {
    pub(crate) require_presentation: bool,
    pub(crate) force_render: bool,
}

#[derive(PartialEq, Eq, Hash, Clone)]
pub(crate) enum AtlasKey {
    Glyph(RenderGlyphParams),
}

impl AtlasKey {
    pub(crate) fn texture_kind(&self) -> AtlasTextureKind {
        match self {
            AtlasKey::Glyph(params) => {
                if params.is_emoji {
                    AtlasTextureKind::Polychrome
                } else {
                    AtlasTextureKind::Monochrome
                }
            }
        }
    }
}

impl From<RenderGlyphParams> for AtlasKey {
    fn from(params: RenderGlyphParams) -> Self {
        Self::Glyph(params)
    }
}

#[doc(hidden)]
pub(crate) struct AtlasTextureList<T> {
    pub(crate) textures: Vec<Option<T>>,
}

impl<T> Default for AtlasTextureList<T> {
    fn default() -> Self {
        Self {
            textures: Vec::default(),
        }
    }
}

impl<T> ops::Index<usize> for AtlasTextureList<T> {
    type Output = Option<T>;

    fn index(&self, index: usize) -> &Self::Output {
        &self.textures[index]
    }
}

impl<T> AtlasTextureList<T> {
    pub(crate) fn iter_mut(&mut self) -> impl DoubleEndedIterator<Item = &mut T> {
        self.textures.iter_mut().flatten()
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(C)]
pub(crate) struct AtlasTile {
    pub(crate) texture_id: AtlasTextureId,
    pub(crate) tile_id: TileId,
    pub(crate) padding: u32,
    pub(crate) bounds: Bounds<DevicePixels>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(C)]
pub(crate) struct AtlasTextureId {
    pub(crate) index: u32,
    pub(crate) kind: AtlasTextureKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(C)]
pub(crate) enum AtlasTextureKind {
    Monochrome = 0,
    Polychrome = 1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(C)]
pub(crate) struct TileId(pub(crate) u32);

pub(crate) struct PlatformInputHandler {
    cx: AsyncWindowContext,
    handler: Box<dyn InputHandler>,
}

impl PlatformInputHandler {
    pub(crate) fn new(cx: AsyncWindowContext, handler: Box<dyn InputHandler>) -> Self {
        Self { cx, handler }
    }

    pub(crate) fn selected_text_range(&mut self, ignore_disabled_input: bool) -> Option<UTF16Selection> {
        self.cx
            .update(|window, cx| {
                self.handler
                    .selected_text_range(ignore_disabled_input, window, cx)
            })
            .ok()
            .flatten()
    }

    pub(crate) fn replace_text_in_range(&mut self, replacement_range: Option<Range<usize>>, text: &str) {
        self.cx
            .update(|window, cx| {
                self.handler
                    .replace_text_in_range(replacement_range, text, window, cx);
            })
            .ok();
    }

    pub(crate) fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range: Option<Range<usize>>,
    ) {
        self.cx
            .update(|window, cx| {
                self.handler.replace_and_mark_text_in_range(
                    range_utf16,
                    new_text,
                    new_selected_range,
                    window,
                    cx,
                )
            })
            .ok();
    }

    pub(crate) fn bounds_for_range(&mut self, range_utf16: Range<usize>) -> Option<Bounds<Pixels>> {
        self.cx
            .update(|window, cx| self.handler.bounds_for_range(range_utf16, window, cx))
            .ok()
            .flatten()
    }

    pub(crate) fn accepts_text_input(&mut self, window: &mut Window, cx: &mut App) -> bool {
        self.handler.accepts_text_input(window, cx)
    }

    pub(crate) fn query_accepts_text_input(&mut self) -> bool {
        self.cx
            .update(|window, cx| self.handler.accepts_text_input(window, cx))
            .unwrap_or(true)
    }
}

/// A selection in a text buffer in UTF-16 code units.
#[derive(Debug)]
pub(crate) struct UTF16Selection {
    pub(crate) range: Range<usize>,
}

/// Interface for handling text input from the platform's IME system.
pub(crate) trait InputHandler: 'static {
    fn selected_text_range(
        &mut self,
        ignore_disabled_input: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<UTF16Selection>;

    fn replace_text_in_range(
        &mut self,
        replacement_range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut App,
    );

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut App,
    );

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Bounds<Pixels>>;

    fn accepts_text_input(&mut self, _window: &mut Window, _cx: &mut App) -> bool {
        true
    }
}

/// Options for creating a new window.
#[derive(Debug)]
pub(crate) struct WindowOptions {
    pub(crate) window_bounds: Option<WindowBounds>,
    pub(crate) titlebar: Option<TitlebarOptions>,
    pub(crate) focus: bool,
    pub(crate) show: bool,
    pub(crate) kind: WindowKind,
    pub(crate) is_movable: bool,
    pub(crate) inactive_frame_interval: Option<Duration>,
    pub(crate) is_resizable: bool,
    pub(crate) is_minimizable: bool,
    pub(crate) display_id: Option<DisplayId>,
    pub(crate) window_background: WindowBackgroundAppearance,
    pub(crate) window_min_size: Option<Size<Pixels>>,
}

#[derive(Debug)]
pub(crate) struct WindowParams {
    pub(crate) bounds: Bounds<Pixels>,
    pub(crate) titlebar: Option<TitlebarOptions>,
    pub(crate) kind: WindowKind,
    pub(crate) is_movable: bool,
    pub(crate) is_resizable: bool,
    pub(crate) is_minimizable: bool,
    pub(crate) focus: bool,
    pub(crate) show: bool,
    pub(crate) display_id: Option<DisplayId>,
    pub(crate) window_min_size: Option<Size<Pixels>>,
}

/// Represents how a window should be opened.
#[derive(Debug, Copy, Clone, PartialEq)]
pub(crate) enum WindowBounds {
    Windowed(Bounds<Pixels>),
    Maximized(Bounds<Pixels>),
}

impl Default for WindowBounds {
    fn default() -> Self {
        WindowBounds::Windowed(Bounds::default())
    }
}

impl WindowBounds {
    pub(crate) fn get_bounds(&self) -> Bounds<Pixels> {
        match self {
            WindowBounds::Windowed(bounds) => *bounds,
            WindowBounds::Maximized(bounds) => *bounds,
        }
    }
}

impl Default for WindowOptions {
    fn default() -> Self {
        Self {
            window_bounds: None,
            titlebar: Some(TitlebarOptions {
                title: Default::default(),
                appears_transparent: Default::default(),
            }),
            focus: true,
            show: true,
            kind: WindowKind::Normal,
            is_movable: true,
            inactive_frame_interval: Some(Duration::from_micros(33_333)),
            is_resizable: true,
            is_minimizable: true,
            display_id: None,
            window_background: WindowBackgroundAppearance::default(),
            window_min_size: None,
        }
    }
}

/// Options for configuring a window's titlebar.
#[derive(Debug, Default)]
pub(crate) struct TitlebarOptions {
    pub(crate) title: Option<SharedString>,
    pub(crate) appears_transparent: bool,
}

/// The kind of Win32 window to create.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WindowKind {
    Normal,
    Overlay,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum WindowAppearance {
    #[default]
    Light,
    Dark,
}

#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub(crate) enum WindowBackgroundAppearance {
    #[default]
    Opaque,
    Transparent,
    MicaAltBackdrop,
}

#[derive(Clone, Debug)]
pub(crate) struct PathPromptOptions {
    pub(crate) directories: bool,
    pub(crate) multiple: bool,
    pub(crate) prompt: Option<SharedString>,
}

#[derive(Copy, Clone, Default, Debug, PartialEq, Eq, Hash)]
pub(crate) enum CursorStyle {
    #[default]
    Arrow,
    IBeam,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ClipboardItem {
    pub(crate) entries: Vec<ClipboardEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ClipboardEntry {
    String(ClipboardString),
    ExternalPaths(crate::ui::ExternalPaths),
}

impl ClipboardItem {
    pub(crate) fn new_string(text: String) -> Self {
        Self {
            entries: vec![ClipboardEntry::String(ClipboardString::new(text))],
        }
    }

    pub(crate) fn text(&self) -> Option<String> {
        let mut answer = String::new();

        for entry in self.entries.iter() {
            if let ClipboardEntry::String(ClipboardString { text, metadata: _ }) = entry {
                answer.push_str(text);
            }
        }

        if answer.is_empty() {
            for entry in self.entries.iter() {
                if let ClipboardEntry::ExternalPaths(paths) = entry {
                    for path in &paths.0 {
                        use std::fmt::Write as _;
                        _ = write!(answer, "{}", path.display());
                    }
                }
            }
        }

        if !answer.is_empty() {
            Some(answer)
        } else {
            None
        }
    }

    pub(crate) fn entries(&self) -> &[ClipboardEntry] {
        &self.entries
    }
}

impl From<ClipboardString> for ClipboardEntry {
    fn from(value: ClipboardString) -> Self {
        Self::String(value)
    }
}

impl From<String> for ClipboardEntry {
    fn from(value: String) -> Self {
        Self::from(ClipboardString::from(value))
    }
}

impl From<ClipboardEntry> for ClipboardItem {
    fn from(value: ClipboardEntry) -> Self {
        Self {
            entries: vec![value],
        }
    }
}

impl From<String> for ClipboardItem {
    fn from(value: String) -> Self {
        Self::from(ClipboardEntry::from(value))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ClipboardString {
    pub(crate) text: String,
    pub(crate) metadata: Option<String>,
}

impl ClipboardString {
    pub(crate) fn new(text: String) -> Self {
        Self {
            text,
            metadata: None,
        }
    }

    pub(crate) fn text_hash(text: &str) -> u64 {
        let mut hasher = crate::ui::collections::FxHasher::default();
        text.hash(&mut hasher);
        hasher.finish()
    }
}

impl From<String> for ClipboardString {
    fn from(value: String) -> Self {
        Self {
            text: value,
            metadata: None,
        }
    }
}
