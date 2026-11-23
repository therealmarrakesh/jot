//! OLE `IDropTarget` implementation for dragging files onto Jot windows.

use std::cell::Cell;
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;

use windows::Win32::{
    System::{
        Com::{DVASPECT_CONTENT, FORMATETC, IDataObject, TYMED_HGLOBAL},
        Ole::{
            CF_HDROP, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_NONE, IDropTarget, IDropTarget_Impl,
            ReleaseStgMedium,
        },
        SystemServices::MODIFIERKEYS_FLAGS,
    },
    UI::Shell::{DragQueryFileW, HDROP},
};
use windows::core::implement;

use super::window::{PlatformEvent, WindowId, dispatch_or_queue_event};

const DRAGDROP_GET_FILES_COUNT: u32 = 0xFFFFFFFF;

fn hdrop_format() -> FORMATETC {
    FORMATETC {
        cfFormat: CF_HDROP.0,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    }
}

fn collect_drop_paths(hdrop: HDROP) -> Vec<PathBuf> {
    // SAFETY: `hdrop` is a valid HDROP handle obtained from an active `CF_HDROP` `STGMEDIUM`.
    let file_count = unsafe { DragQueryFileW(hdrop, DRAGDROP_GET_FILES_COUNT, None) };
    let mut paths = Vec::with_capacity(file_count as usize);
    for ix in 0..file_count {
        // SAFETY: `hdrop` is valid and `ix < file_count`; querying filename length with `None`.
        let len = unsafe { DragQueryFileW(hdrop, ix, None) } as usize;
        if len == 0 {
            continue;
        }
        let mut buffer = vec![0u16; len + 1];
        // SAFETY: `buffer` has `len + 1` code units to hold the filename and NUL terminator.
        let written = unsafe { DragQueryFileW(hdrop, ix, Some(buffer.as_mut_slice())) } as usize;
        if written == 0 {
            continue;
        }
        paths.push(PathBuf::from(OsString::from_wide(&buffer[..written])));
    }
    paths
}

#[implement(IDropTarget)]
pub(super) struct JotDropTarget {
    pub(super) window_id: WindowId,
    pub(super) accepts_drop: Cell<bool>,
}

impl JotDropTarget {
    pub(super) fn new(window_id: WindowId) -> Self {
        Self {
            window_id,
            accepts_drop: Cell::new(false),
        }
    }
}

impl IDropTarget_Impl for JotDropTarget_Impl {
    fn DragEnter(
        &self,
        pdataobj: windows::core::Ref<'_, IDataObject>,
        _grfkeystate: MODIFIERKEYS_FLAGS,
        _pt: &windows::Win32::Foundation::POINTL,
        pdweffect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            let format = hdrop_format();
            let has_files = pdataobj.as_ref().is_some_and(|obj| {
                // SAFETY: `obj` is a valid COM `IDataObject` supplied by OLE for this drag.
                unsafe { obj.QueryGetData(&format) }.is_ok()
            });
            self.accepts_drop.set(has_files);
            if !pdweffect.is_null() {
                // SAFETY: OLE guarantees `pdweffect` is a valid writable pointer when non-null.
                unsafe {
                    *pdweffect = if has_files {
                        DROPEFFECT_COPY
                    } else {
                        DROPEFFECT_NONE
                    };
                }
            }
        }));
        Ok(())
    }

    fn DragOver(
        &self,
        _grfkeystate: MODIFIERKEYS_FLAGS,
        _pt: &windows::Win32::Foundation::POINTL,
        pdweffect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            if !pdweffect.is_null() {
                // SAFETY: OLE guarantees `pdweffect` is a valid writable pointer when non-null.
                unsafe {
                    *pdweffect = if self.accepts_drop.get() {
                        DROPEFFECT_COPY
                    } else {
                        DROPEFFECT_NONE
                    };
                }
            }
        }));
        Ok(())
    }

    fn DragLeave(&self) -> windows::core::Result<()> {
        self.accepts_drop.set(false);
        Ok(())
    }

    fn Drop(
        &self,
        pdataobj: windows::core::Ref<'_, IDataObject>,
        _grfkeystate: MODIFIERKEYS_FLAGS,
        _pt: &windows::Win32::Foundation::POINTL,
        pdweffect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            self.accepts_drop.set(false);
            let set_effect = |effect: DROPEFFECT| {
                if !pdweffect.is_null() {
                    // SAFETY: OLE guarantees `pdweffect` is a valid writable pointer when non-null.
                    unsafe {
                        *pdweffect = effect;
                    }
                }
            };
            let Some(data_obj) = pdataobj.as_ref() else {
                set_effect(DROPEFFECT_NONE);
                return;
            };
            let format = hdrop_format();
            // SAFETY: `data_obj` is a valid COM `IDataObject` supplied by OLE; `medium` is freed below.
            let Ok(mut medium) = (unsafe { data_obj.GetData(&format) }) else {
                set_effect(DROPEFFECT_NONE);
                return;
            };
            // SAFETY: `medium` was returned for `CF_HDROP` / `TYMED_HGLOBAL`, so `medium.u.hGlobal`
            // is the `HDROP` handle directly; `ReleaseStgMedium` releases it before returning.
            let paths = unsafe {
                let hglobal = medium.u.hGlobal;
                let collected = if !hglobal.0.is_null() {
                    collect_drop_paths(HDROP(hglobal.0))
                } else {
                    Vec::new()
                };
                ReleaseStgMedium(&mut medium);
                collected
            };
            if paths.is_empty() {
                set_effect(DROPEFFECT_NONE);
                return;
            }
            set_effect(DROPEFFECT_COPY);
            dispatch_or_queue_event(PlatformEvent::FilesDropped {
                window_id: self.window_id,
                paths,
            });
        }));
        Ok(())
    }
}
