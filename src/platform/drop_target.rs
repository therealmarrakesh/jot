//! OLE `IDropTarget` implementation for dragging files onto Jot windows.

use std::path::PathBuf;

use windows::Win32::{
    System::{
        Com::{DVASPECT_CONTENT, FORMATETC, IDataObject, TYMED_HGLOBAL},
        Memory::{GlobalLock, GlobalUnlock},
        Ole::{
            CF_HDROP, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_NONE, IDropTarget, IDropTarget_Impl,
        },
        SystemServices::MODIFIERKEYS_FLAGS,
    },
    UI::Shell::HDROP,
};
use windows::core::implement;

use super::clipboard::with_file_names;
use super::window::{PlatformEvent, WindowId, dispatch_or_queue_event};

#[implement(IDropTarget)]
pub(super) struct JotDropTarget {
    pub(super) window_id: WindowId,
}

impl IDropTarget_Impl for JotDropTarget_Impl {
    fn DragEnter(
        &self,
        _pdataobj: windows::core::Ref<'_, IDataObject>,
        _grfkeystate: MODIFIERKEYS_FLAGS,
        _pt: &windows::Win32::Foundation::POINTL,
        pdweffect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        unsafe {
            *pdweffect = DROPEFFECT_COPY;
        }
        Ok(())
    }

    fn DragOver(
        &self,
        _grfkeystate: MODIFIERKEYS_FLAGS,
        _pt: &windows::Win32::Foundation::POINTL,
        pdweffect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        unsafe {
            *pdweffect = DROPEFFECT_COPY;
        }
        Ok(())
    }

    fn DragLeave(&self) -> windows::core::Result<()> {
        Ok(())
    }

    fn Drop(
        &self,
        pdataobj: windows::core::Ref<'_, IDataObject>,
        _grfkeystate: MODIFIERKEYS_FLAGS,
        _pt: &windows::Win32::Foundation::POINTL,
        pdweffect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        let Some(data_obj) = pdataobj.as_ref() else {
            unsafe {
                *pdweffect = DROPEFFECT_NONE;
            }
            return Ok(());
        };
        let format = FORMATETC {
            cfFormat: CF_HDROP.0,
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        };
        let Ok(medium) = (unsafe { data_obj.GetData(&format) }) else {
            unsafe {
                *pdweffect = DROPEFFECT_NONE;
            }
            return Ok(());
        };
        let mut paths = Vec::new();
        unsafe {
            let hglobal = medium.u.hGlobal;
            let locked = GlobalLock(hglobal);
            if !locked.is_null() {
                with_file_names(HDROP(locked), |name| {
                    paths.push(PathBuf::from(name));
                });
                let _ = GlobalUnlock(hglobal);
            }
            *pdweffect = DROPEFFECT_COPY;
        }
        if !paths.is_empty() {
            dispatch_or_queue_event(PlatformEvent::FilesDropped {
                window_id: self.window_id,
                paths,
            });
        }
        Ok(())
    }
}
