use std::sync::atomic::{AtomicU64, Ordering};

use crate::geom::Axis;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct TabId(u64);

impl TabId {
    pub(crate) fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum MenuKind {
    File,
    Edit,
    View,
}

impl MenuKind {
    pub(crate) const ALL: [Self; 3] = [Self::File, Self::Edit, Self::View];

    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::File => "File",
            Self::Edit => "Edit",
            Self::View => "View",
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum EditorSlot {
    Tab(TabId),
    FindQuery,
    FindReplace,
    GoToLine,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Focus {
    Editor(EditorSlot),
    MenuBar,
    DialogButton(usize),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum CustomDraw {
    Editor(EditorSlot),
    StatusSpacer,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum HitId {
    CaptionMin,
    CaptionMax,
    CaptionClose,
    TabStrip,
    TabList,
    Tab(TabId),
    TabClose(TabId),
    NewTab,
    TabScrollLeft,
    TabScrollRight,
    MenuBar,
    MenuTrigger(MenuKind),
    MenuPanel,
    MenuItem { serial: usize, ix: usize },
    MenuCommand { serial: usize, ix: usize },
    SubmenuPanel,
    SubmenuItem { serial: usize, ix: usize },
    ThemeToggle,
    EditorArea,
    Editor(EditorSlot),
    ScrollTrack(EditorSlot, Axis),
    ScrollThumb(EditorSlot, Axis),
    FieldClear(EditorSlot),
    FindCard,
    FindToggleReplace,
    FindNext,
    FindPrev,
    FindOptions,
    FindClose,
    FindReplace,
    FindReplaceAll,
    DialogBackdrop,
    DialogCard,
    DialogButton(usize),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum AnimKey {
    Hit(HitId),
    TabX(usize),
    TabWidth(usize),
    TabScroll,
    TabExtent,
    Transition { serial: usize, track: u8 },
    ScrollOpacity(EditorSlot),
    ScrollThickness(EditorSlot, Axis),
}

impl From<HitId> for AnimKey {
    #[inline]
    fn from(hit: HitId) -> Self {
        Self::Hit(hit)
    }
}

pub(crate) type Node = crate::ui::Node<HitId, CustomDraw>;
pub(crate) type Frame = crate::ui::Frame<HitId, CustomDraw>;
pub(crate) type UiState = crate::ui::UiState<HitId, CustomDraw, AnimKey>;
