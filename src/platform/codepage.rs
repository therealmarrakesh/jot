use std::io;

use windows::Win32::Globalization::{
    CP_UTF8, GetACP, IsDBCSLeadByteEx, MB_ERR_INVALID_CHARS, MultiByteToWideChar,
    WC_NO_BEST_FIT_CHARS, WideCharToMultiByte,
};
use windows::core::BOOL;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct AnsiCodePage(u32);

impl AnsiCodePage {
    pub(crate) const CP_1252: Self = Self(1252);

    pub(crate) fn current() -> Self {
        Self(unsafe { GetACP() })
    }

    #[cfg(test)]
    pub(crate) const fn from_raw(code_page: u32) -> Self {
        Self(code_page)
    }

    pub(crate) fn is_utf8(self) -> bool {
        self.0 == CP_UTF8
    }

    pub(crate) fn is_dbcs_lead_byte(self, byte: u8) -> bool {
        if self.0 == CP_UTF8 || self.0 == 1252 {
            return false;
        }
        unsafe { IsDBCSLeadByteEx(self.0, byte) }.is_ok()
    }

    pub(crate) fn decode_chunk(
        self,
        bytes: &[u8],
        wide: &mut Vec<u16>,
        out: &mut String,
    ) -> io::Result<bool> {
        out.clear();
        if bytes.is_empty() {
            return Ok(true);
        }
        if i32::try_from(bytes.len()).is_err() {
            return Ok(false);
        }
        let length = unsafe { MultiByteToWideChar(self.0, MB_ERR_INVALID_CHARS, bytes, None) };
        let Ok(needed) = usize::try_from(length) else {
            return Ok(false);
        };
        if needed == 0 {
            return Ok(false);
        }
        wide.clear();
        if wide.capacity() < needed {
            wide.try_reserve(needed - wide.capacity()).map_err(|_| {
                io::Error::new(io::ErrorKind::OutOfMemory, "Out of memory decoding file.")
            })?;
        }
        wide.resize(needed, 0);
        let written = unsafe {
            MultiByteToWideChar(self.0, MB_ERR_INVALID_CHARS, bytes, Some(wide.as_mut_slice()))
        };
        let Ok(written) = usize::try_from(written) else {
            return Ok(false);
        };
        if written == 0 {
            return Ok(false);
        }
        wide.truncate(written);
        out.try_reserve(written).map_err(|_| {
            io::Error::new(io::ErrorKind::OutOfMemory, "Out of memory decoding file.")
        })?;
        for ch in char::decode_utf16(wide.iter().copied()) {
            let Ok(ch) = ch else {
                return Ok(false);
            };
            out.push(ch);
        }
        Ok(true)
    }

    pub(crate) fn wide_is_lossy(self, wide: &[u16]) -> io::Result<bool> {
        if wide.is_empty() {
            return Ok(false);
        }
        let mut used_default = BOOL(0);
        let written = unsafe {
            WideCharToMultiByte(
                self.0,
                WC_NO_BEST_FIT_CHARS,
                wide,
                None,
                None,
                Some(&mut used_default),
            )
        };
        if written == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(used_default.as_bool())
    }

    pub(crate) fn encode_wide(self, wide: &[u16], out: &mut Vec<u8>) -> io::Result<()> {
        out.clear();
        if wide.is_empty() {
            return Ok(());
        }
        let length =
            unsafe { WideCharToMultiByte(self.0, WC_NO_BEST_FIT_CHARS, wide, None, None, None) };
        if length == 0 {
            return Err(io::Error::last_os_error());
        }
        out.resize(usize::try_from(length).unwrap_or(0), 0);
        let written = unsafe {
            WideCharToMultiByte(self.0, WC_NO_BEST_FIT_CHARS, wide, Some(out), None, None)
        };
        if written == 0 {
            return Err(io::Error::last_os_error());
        }
        out.truncate(usize::try_from(written).unwrap_or(0));
        Ok(())
    }
}
