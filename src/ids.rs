use crate::geom::Axis;

crate::types::id_type!(TabId);

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
pub(crate) enum CaptionHit {
    Min,
    Max,
    Close,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum TabHit {
    Strip,
    List,
    Item(TabId),
    Close(TabId),
    New,
    ScrollLeft,
    ScrollRight,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum MenuHit {
    Bar,
    Trigger(MenuKind),
    Panel,
    Item { serial: usize, ix: usize },
    Command { serial: usize, ix: usize },
    SubmenuPanel,
    SubmenuItem { serial: usize, ix: usize },
}

impl MenuHit {
    #[inline]
    pub(crate) fn is_popup(self) -> bool {
        !matches!(self, Self::Bar | Self::Trigger(_))
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum FindHit {
    Card,
    ToggleReplace,
    Next,
    Prev,
    Options,
    Close,
    Replace,
    ReplaceAll,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum DialogHit {
    Backdrop,
    Card,
    Button(usize),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum HitId {
    Caption(CaptionHit),
    Tab(TabHit),
    Menu(MenuHit),
    Find(FindHit),
    Dialog(DialogHit),
    ThemeToggle,
    EditorArea,
    Editor(EditorSlot),
    ScrollTrack(EditorSlot, Axis),
    ScrollThumb(EditorSlot, Axis),
    FieldClear(EditorSlot),
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

pub(crate) type AppNode = crate::ui::Node<HitId, CustomDraw>;
pub(crate) type AppFrame = crate::ui::Frame<HitId, CustomDraw>;
pub(crate) type AppUi = crate::ui::UiState<HitId, CustomDraw, AnimKey>;
