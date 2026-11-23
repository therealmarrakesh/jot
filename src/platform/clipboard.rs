//! Win32 Unicode clipboard helpers with open retries and CRLF normalization.

use std::thread;
use std::time::Duration;

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
};
use windows::core::Owned;

use crate::types::{Error, Result};

const OPEN_RETRIES: usize = 8;
const OPEN_RETRY_DELAY: Duration = Duration::from_millis(12);

pub(crate) fn clipboard_has_text() -> bool {
    // SAFETY: `IsClipboardFormatAvailable` takes no pointers and does not require opening the clipboard.
    unsafe { IsClipboardFormatAvailable(CF_UNICODETEXT.0 as u32) }.is_ok()
}

/// Writes `text` to the Windows clipboard as `CF_UNICODETEXT` with CRLF line breaks.
/// Returns `true` on success and `false` if the clipboard could not be acquired/written.
pub(crate) fn write_clipboard_text(text: &str) -> bool {
    let Some(_clip) = ClipboardGuard::open() else {
        return false;
    };
    (|| -> Result<()> {
        // SAFETY: Clipboard is currently held open by `_clip`.
        unsafe { EmptyClipboard()? };
        let crlf = normalize_for_clipboard(text);
        let wide: Vec<u16> = crlf.encode_utf16().chain(Some(0)).collect();
        set_clipboard_bytes(&wide, CF_UNICODETEXT.0 as u32)
    })()
    .is_ok()
}

pub(crate) fn read_clipboard_text() -> Option<String> {
    let _clip = ClipboardGuard::open()?;
    // SAFETY: Clipboard is open; `GetClipboardData` returns an HGLOBAL owned by the clipboard.
    let global = HGLOBAL(unsafe { GetClipboardData(CF_UNICODETEXT.0 as u32).ok() }?.0);
    let locked = LockedGlobal::lock(global)?;
    let bytes = locked.as_bytes();
    let words_len = bytes.len() / std::mem::size_of::<u16>();
    if words_len == 0 {
        return Some(String::new());
    }
    // SAFETY: `locked.ptr` comes from `GlobalLock` (8-byte aligned on x64) and covers `words_len` u16s.
    let slice = unsafe { std::slice::from_raw_parts(bytes.as_ptr() as *const u16, words_len) };
    let actual_len = slice.iter().position(|&c| c == 0).unwrap_or(words_len);
    Some(String::from_utf16_lossy(&slice[..actual_len]))
}

/// Normalizes LF-only internal buffer text to Windows CRLF for `CF_UNICODETEXT`, while keeping
/// existing CRLF sequences intact and replacing embedded NULs (which would truncate `CF_UNICODETEXT`).
fn normalize_for_clipboard(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + text.bytes().filter(|&b| b == b'\n').count());
    let mut prev_cr = false;
    for ch in text.chars() {
        match ch {
            '\0' => {
                out.push(' ');
                prev_cr = false;
            }
            '\n' if !prev_cr => {
                out.push_str("\r\n");
                prev_cr = false;
            }
            '\r' => {
                out.push('\r');
                prev_cr = true;
            }
            other => {
                out.push(other);
                prev_cr = false;
            }
        }
    }
    out
}

fn set_clipboard_bytes<T>(data: &[T], format: u32) -> Result<()> {
    // SAFETY: Allocates moveable global memory of exact byte size, copies `data`, unlocks, and
    // transfers ownership to the system clipboard only after `SetClipboardData` succeeds.
    unsafe {
        let global = Owned::new(GlobalAlloc(GMEM_MOVEABLE, std::mem::size_of_val(data))?);
        let ptr = GlobalLock(*global);
        if ptr.is_null() {
            return Err(Error::new("GlobalLock returned null"));
        }
        std::ptr::copy_nonoverlapping(data.as_ptr(), ptr as *mut T, data.len());
        let _ = GlobalUnlock(*global);
        SetClipboardData(format, Some(HANDLE(global.0)))?;
        // SetClipboardData succeeded; the OS now owns the handle.
        std::mem::forget(global);
    }
    Ok(())
}

struct ClipboardGuard;

impl ClipboardGuard {
    fn open() -> Option<Self> {
        for attempt in 0..OPEN_RETRIES {
            // SAFETY: `OpenClipboard(None)` acquires clipboard access for the current task.
            if unsafe { OpenClipboard(None) }.is_ok() {
                return Some(Self);
            }
            if attempt + 1 < OPEN_RETRIES {
                thread::sleep(OPEN_RETRY_DELAY);
            }
        }
        None
    }
}

impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        // SAFETY: Matches the successful `OpenClipboard` in `ClipboardGuard::open`.
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
        // SAFETY: `global` is a valid HGLOBAL returned by `GetClipboardData`.
        let size = unsafe { GlobalSize(global) };
        // SAFETY: `global` is a valid HGLOBAL; paired with `GlobalUnlock` in `Drop`.
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
        if self.size == 0 {
            return &[];
        }
        // SAFETY: `self.ptr` is locked and valid for `self.size` bytes until `Drop`.
        unsafe { std::slice::from_raw_parts(self.ptr, self.size) }
    }
}

impl Drop for LockedGlobal {
    fn drop(&mut self) {
        // SAFETY: Matches the successful `GlobalLock` in `LockedGlobal::lock`.
        unsafe {
            let _ = GlobalUnlock(self.global);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_for_clipboard;

    #[test]
    fn clipboard_normalization_converts_lf_to_crlf_without_doubling() {
        assert_eq!(normalize_for_clipboard("a\nb\nc"), "a\r\nb\r\nc");
        assert_eq!(normalize_for_clipboard("a\r\nb\n"), "a\r\nb\r\n");
        assert_eq!(normalize_for_clipboard("a\0b"), "a b");
    }
}
