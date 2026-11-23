//! OLE `IDropTarget` implementation for dragging files onto Jot windows.

use std::path::PathBuf;

use windows::Win32::{
    Foundation::{HWND, POINT},
    Graphics::Gdi::ScreenToClient,
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

use crate::ui::geometry::{point, px};
use crate::ui::platform::{ACTIVE_APP, ExternalPaths, drive_frames};
use crate::ui::window::WindowId;

use super::{PendingWin32Event, queue_pending_win32_event};

#[implement(IDropTarget)]
pub(super) struct JotDropTarget {
    pub(super) window_id: WindowId,
    pub(super) hwnd: HWND,
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
        pt: &windows::Win32::Foundation::POINTL,
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
                crate::win32::with_file_names(HDROP(locked), |name| {
                    paths.push(PathBuf::from(name));
                });
                let _ = GlobalUnlock(hglobal);
            }
            *pdweffect = DROPEFFECT_COPY;
        }
        if !paths.is_empty()
            && let Some(app_cell) = ACTIVE_APP.with(|s| s.borrow().clone())
        {
            let mut client = POINT { x: pt.x, y: pt.y };
            let _ = unsafe { ScreenToClient(self.hwnd, &mut client) };
            if let Ok(mut app) = app_cell.inner.try_borrow_mut() {
                let _ = app.update_window_impl(self.window_id, |_, window, cx| {
                    let pos = point(
                        px(client.x as f32 / window.native.scale_factor),
                        px(client.y as f32 / window.native.scale_factor),
                    );
                    window.dispatch_file_drop(ExternalPaths(paths), pos, cx);
                });
                drop(app);
                drive_frames(&app_cell);
            } else {
                queue_pending_win32_event(PendingWin32Event::DroppedFiles {
                    window_id: self.window_id,
                    paths: ExternalPaths(paths),
                    client_px: (client.x, client.y),
                });
            }
        }
        Ok(())
    }
}
