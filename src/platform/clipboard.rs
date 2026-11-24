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
    unsafe { IsClipboardFormatAvailable(CF_UNICODETEXT.0 as u32) }.is_ok()
}

pub(crate) fn write_clipboard_text(text: &str) -> bool {
    let Some(_clip) = ClipboardGuard::open() else {
        return false;
    };
    (|| -> Result<()> {
        unsafe { EmptyClipboard()? };
        let crlf = normalize_for_clipboard(text);
        let wide: Vec<u16> = crlf.encode_utf16().chain(Some(0)).collect();
        set_clipboard_bytes(&wide, CF_UNICODETEXT.0 as u32)
    })()
    .is_ok()
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
    unsafe {
        let global = Owned::new(GlobalAlloc(GMEM_MOVEABLE, std::mem::size_of_val(data))?);
        let ptr = GlobalLock(*global);
        if ptr.is_null() {
            return Err(Error::new("GlobalLock returned null"));
        }
        std::ptr::copy_nonoverlapping(data.as_ptr(), ptr as *mut T, data.len());
        let _ = GlobalUnlock(*global);
        SetClipboardData(format, Some(HANDLE(global.0)))?;
        // SetClipboardData succeeded; OS owns the handle now.
        std::mem::forget(global);
    }
    Ok(())
}

struct ClipboardGuard;

impl ClipboardGuard {
    fn open() -> Option<Self> {
        for attempt in 0..OPEN_RETRIES {
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
        if self.size == 0 {
            return &[];
        }
        unsafe { std::slice::from_raw_parts(self.ptr, self.size) }
    }
}

impl Drop for LockedGlobal {
    fn drop(&mut self) {
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
