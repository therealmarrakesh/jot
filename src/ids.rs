use crate::menu::MenuKind;
use crate::tabs::TabId;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum EditorSlot {
    Tab(TabId),
    FindQuery,
    FindReplace,
    GoToLine,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum CustomDraw {
    Editor(EditorSlot),
    Scrollbar,
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
    MenuUnroll(usize),
    MenuAppear(usize),
    MenuFade(usize),
    DialogShow(usize),
    DialogFadeIn(usize),
    DialogHide(usize),
    DialogFadeOut(usize),
    FindShow(usize),
    FindHide(usize),
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
