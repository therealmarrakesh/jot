use std::borrow::Cow;

use crate::ui::{UiError, UiResult};
use windows::Win32::{
    Foundation::{HANDLE, HGLOBAL},
    System::{
        DataExchange::{
            CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable,
            OpenClipboard, SetClipboardData,
        },
        Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock},
        Ole::CF_UNICODETEXT,
    },
    UI::Shell::{DragQueryFileW, HDROP},
};
use windows::core::Owned;

const DRAGDROP_GET_FILES_COUNT: u32 = 0xFFFFFFFF;

pub(crate) fn clipboard_has_text() -> bool {
    unsafe { IsClipboardFormatAvailable(CF_UNICODETEXT.0 as u32) }.is_ok()
}

pub(crate) fn write_clipboard_text(text: &str) {
    let Some(_clip) = ClipboardGuard::open() else {
        return;
    };
    let _ = (|| -> UiResult<()> {
        unsafe { EmptyClipboard()? };
        let sanitized = if text.contains('\0') {
            Cow::Owned(text.replace('\0', " "))
        } else {
            Cow::Borrowed(text)
        };
        let wide: Vec<u16> = sanitized.encode_utf16().chain(Some(0)).collect();
        set_clipboard_bytes(&wide, CF_UNICODETEXT.0 as u32)
    })();
}

pub(crate) fn read_clipboard_text() -> Option<String> {
    let _clip = ClipboardGuard::open()?;
    let global = HGLOBAL(unsafe { GetClipboardData(CF_UNICODETEXT.0 as u32).ok() }?.0);
    let locked = LockedGlobal::lock(global)?;
    let bytes = locked.as_bytes();
    let words_len = bytes.len() / std::mem::size_of::<u16>();
    if words_len == 0 {
        return Some(String::new());
    }
    let slice = unsafe { std::slice::from_raw_parts(bytes.as_ptr() as *const u16, words_len) };
    let actual_len = slice.iter().position(|&c| c == 0).unwrap_or(words_len);
    Some(String::from_utf16_lossy(&slice[..actual_len]))
}

pub(crate) fn with_file_names<F>(hdrop: HDROP, mut f: F)
where
    F: FnMut(String),
{
    let file_count = unsafe { DragQueryFileW(hdrop, DRAGDROP_GET_FILES_COUNT, None) };
    for file_index in 0..file_count {
        let filename_length = unsafe { DragQueryFileW(hdrop, file_index, None) } as usize;
        let mut buffer = vec![0u16; filename_length + 1];
        let ret = unsafe { DragQueryFileW(hdrop, file_index, Some(buffer.as_mut_slice())) };
        if ret == 0 {
            continue;
        }
        if let Ok(file_name) = String::from_utf16(&buffer[0..filename_length]) {
            f(file_name);
        }
    }
}

fn set_clipboard_bytes<T>(data: &[T], format: u32) -> UiResult<()> {
    unsafe {
        let global = Owned::new(GlobalAlloc(GMEM_MOVEABLE, std::mem::size_of_val(data))?);
        let ptr = GlobalLock(*global);
        if ptr.is_null() {
            return Err(UiError::new("GlobalLock returned null"));
        }
        std::ptr::copy_nonoverlapping(data.as_ptr(), ptr as _, data.len());
        GlobalUnlock(*global).ok();
        SetClipboardData(format, Some(HANDLE(global.0)))?;
        // SetClipboardData succeeded — the system now owns the memory.
        std::mem::forget(global);
    }
    Ok(())
}

struct ClipboardGuard;

impl ClipboardGuard {
    fn open() -> Option<Self> {
        unsafe { OpenClipboard(None) }.ok().map(|()| Self)
    }
}

impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        let _ = unsafe { CloseClipboard() };
    }
}

struct LockedGlobal {
    global: HGLOBAL,
    ptr: *const u8,
    size: usize,
}

impl LockedGlobal {
    fn lock(global: HGLOBAL) -> Option<Self> {
        let size = unsafe { GlobalSize(global) };
        let ptr = unsafe { GlobalLock(global) };
        if ptr.is_null() {
            return None;
        }
        Some(Self {
            global,
            ptr: ptr as *const u8,
            size,
        })
    }

    fn as_bytes(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.ptr, self.size) }
    }
}

impl Drop for LockedGlobal {
    fn drop(&mut self) {
        unsafe { GlobalUnlock(self.global).ok() };
    }
}
