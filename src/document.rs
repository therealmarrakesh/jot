use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use windows_sys::Win32::Globalization::{
    CP_UTF8, GetACP, MultiByteToWideChar, WC_NO_BEST_FIT_CHARS, WideCharToMultiByte,
};

const UTF8_BOM: &[u8] = b"\xef\xbb\xbf";
const UTF16_LE_BOM: &[u8] = b"\xff\xfe";
const UTF16_BE_BOM: &[u8] = b"\xfe\xff";

pub(crate) const UNTITLED: &str = "Untitled";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum LineEnding {
    #[default]
    Crlf,
    Lf,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Encoding {
    #[default]
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
    Ansi,
}

impl Encoding {
    pub(crate) const SAVE_ORDER: [Self; 5] = [
        Self::Ansi,
        Self::Utf16Le,
        Self::Utf16Be,
        Self::Utf8,
        Self::Utf8Bom,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Utf8 => "UTF-8",
            Self::Utf8Bom => "UTF-8 with BOM",
            Self::Utf16Le => "UTF-16 LE",
            Self::Utf16Be => "UTF-16 BE",
            Self::Ansi => "ANSI",
        }
    }

    fn detect(bytes: &[u8]) -> (Self, usize) {
        [
            (Self::Utf8Bom, UTF8_BOM),
            (Self::Utf16Le, UTF16_LE_BOM),
            (Self::Utf16Be, UTF16_BE_BOM),
        ]
        .into_iter()
        .find_map(|(encoding, bom)| bytes.starts_with(bom).then_some((encoding, bom.len())))
        .unwrap_or_else(|| match std::str::from_utf8(bytes) {
            Ok(_) => (Self::Utf8, 0),
            Err(_) => (Self::Ansi, 0),
        })
    }

    fn decode(self, body: Vec<u8>) -> Option<String> {
        let units = |unit: fn([u8; 2]) -> u16| {
            let (pairs, rest) = body.as_chunks::<2>();
            rest.is_empty()
                .then(|| pairs.iter().map(|&pair| unit(pair)).collect::<Vec<_>>())
        };
        match self {
            Self::Utf8 | Self::Utf8Bom => String::from_utf8(body).ok(),
            Self::Utf16Le => String::from_utf16(&units(u16::from_le_bytes)?).ok(),
            Self::Utf16Be => String::from_utf16(&units(u16::from_be_bytes)?).ok(),
            Self::Ansi => ansi_decode(&body),
        }
    }

    fn encode(self, text: String) -> io::Result<Encoded> {
        let units = |bom: &[u8], unit: fn(u16) -> [u8; 2]| Encoded {
            bytes: bom
                .iter()
                .copied()
                .chain(text.encode_utf16().flat_map(unit))
                .collect(),
            lossy: false,
        };
        match self {
            Self::Utf8 => Ok(Encoded {
                bytes: text.into_bytes(),
                lossy: false,
            }),
            Self::Utf8Bom => Ok(Encoded {
                bytes: [UTF8_BOM, text.as_bytes()].concat(),
                lossy: false,
            }),
            Self::Utf16Le => Ok(units(UTF16_LE_BOM, u16::to_le_bytes)),
            Self::Utf16Be => Ok(units(UTF16_BE_BOM, u16::to_be_bytes)),
            Self::Ansi => ansi_encode(&text),
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Format {
    pub(crate) line_ending: LineEnding,
    pub(crate) encoding: Encoding,
}

pub(crate) struct Loaded {
    pub(crate) text: String,
    pub(crate) format: Format,
}

pub(crate) enum LoadError {
    Io(io::Error),
    Undecodable,
}

pub(crate) struct Encoded {
    pub(crate) bytes: Vec<u8>,
    pub(crate) lossy: bool,
}

pub(crate) fn load(path: &Path) -> Result<Loaded, LoadError> {
    let mut bytes = fs::read(path).map_err(LoadError::Io)?;
    let (encoding, bom) = Encoding::detect(&bytes);
    bytes.drain(..bom);
    let text = encoding.decode(bytes).ok_or(LoadError::Undecodable)?;
    let (text, line_ending) = if text.contains("\r\n") {
        (text.replace("\r\n", "\n"), LineEnding::Crlf)
    } else if text.contains('\n') {
        (text, LineEnding::Lf)
    } else {
        (text, LineEnding::Crlf)
    };
    Ok(Loaded {
        text,
        format: Format {
            line_ending,
            encoding,
        },
    })
}

pub(crate) fn encode(text: String, format: Format) -> io::Result<Encoded> {
    let body = match format.line_ending {
        LineEnding::Crlf => text.replace('\n', "\r\n"),
        LineEnding::Lf => text,
    };
    format.encoding.encode(body)
}

pub(crate) fn write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let temp = temp_path(path);
    fs::write(&temp, bytes)?;
    fs::rename(&temp, path).inspect_err(|_| {
        fs::remove_file(&temp).ok();
    })
}

fn ansi_decode(bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() {
        return Some(String::new());
    }
    let input = i32::try_from(bytes.len()).ok()?;
    let page = unsafe { GetACP() };
    let length = unsafe { MultiByteToWideChar(page, 0, bytes.as_ptr(), input, null_mut(), 0) };
    let mut wide = vec![0u16; usize::try_from(length).ok().filter(|&n| n > 0)?];
    let written =
        unsafe { MultiByteToWideChar(page, 0, bytes.as_ptr(), input, wide.as_mut_ptr(), length) };
    wide.truncate(usize::try_from(written).ok()?);
    Some(String::from_utf16_lossy(&wide))
}

fn ansi_encode(text: &str) -> io::Result<Encoded> {
    let page = unsafe { GetACP() };
    if page == CP_UTF8 {
        return Ok(Encoded {
            bytes: text.as_bytes().to_vec(),
            lossy: false,
        });
    }
    let wide: Vec<u16> = text.encode_utf16().collect();
    let Ok(input) = i32::try_from(wide.len()) else {
        return Ok(Encoded {
            bytes: text.as_bytes().to_vec(),
            lossy: true,
        });
    };
    if input == 0 {
        return Ok(Encoded {
            bytes: Vec::new(),
            lossy: false,
        });
    }
    let mut used_default = 0;
    let length = unsafe {
        WideCharToMultiByte(
            page,
            WC_NO_BEST_FIT_CHARS,
            wide.as_ptr(),
            input,
            null_mut(),
            0,
            null(),
            null_mut(),
        )
    };
    if length == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut bytes = vec![0u8; usize::try_from(length).unwrap_or(0)];
    let written = unsafe {
        WideCharToMultiByte(
            page,
            WC_NO_BEST_FIT_CHARS,
            wide.as_ptr(),
            input,
            bytes.as_mut_ptr(),
            length,
            null(),
            &mut used_default,
        )
    };
    if written == 0 {
        return Err(io::Error::last_os_error());
    }
    bytes.truncate(usize::try_from(written).unwrap_or(0));
    Ok(Encoded {
        bytes,
        lossy: used_default != 0,
    })
}

pub(crate) fn file_name(path: Option<&Path>) -> Option<String> {
    path.and_then(Path::file_name)
        .map(|name| name.to_string_lossy().into_owned())
}

pub(crate) fn display_name(path: Option<&Path>) -> String {
    file_name(path).unwrap_or_else(|| UNTITLED.into())
}

pub(crate) fn default_directory(path: Option<&Path>) -> PathBuf {
    path.and_then(Path::parent)
        .map(Path::to_path_buf)
        .or_else(|| {
            std::env::var_os("USERPROFILE").map(|home| PathBuf::from(home).join("Documents"))
        })
        .unwrap_or_else(|| PathBuf::from("."))
}

fn temp_path(path: &Path) -> PathBuf {
    let mut name = OsString::from(".");
    name.push(path.file_name().unwrap_or_default());
    name.push(".jot-save");
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::{Encoding, Format, LineEnding, encode};
    use windows_sys::Win32::Globalization::{CP_UTF8, GetACP};

    const TEXT: &str = "Café — £3.50\nNaïve 😀\n";

    fn format(encoding: Encoding, line_ending: LineEnding) -> Format {
        Format {
            line_ending,
            encoding,
        }
    }

    fn round_trip(encoding: Encoding) -> String {
        let bytes = encode(TEXT.into(), format(encoding, LineEnding::Lf))
            .unwrap()
            .bytes;
        let (detected, bom) = Encoding::detect(&bytes);
        assert_eq!(detected, encoding);
        detected.decode(bytes[bom..].to_vec()).unwrap()
    }

    #[test]
    fn unicode_encodings_round_trip() {
        for encoding in [
            Encoding::Utf8,
            Encoding::Utf8Bom,
            Encoding::Utf16Le,
            Encoding::Utf16Be,
        ] {
            assert_eq!(round_trip(encoding), TEXT);
        }
    }

    #[test]
    fn byte_order_marks_are_written() {
        let bytes = |encoding| {
            encode("a".into(), format(encoding, LineEnding::Lf))
                .unwrap()
                .bytes
        };
        assert_eq!(bytes(Encoding::Utf8), b"a");
        assert_eq!(bytes(Encoding::Utf8Bom), b"\xef\xbb\xbfa");
        assert_eq!(bytes(Encoding::Utf16Le), b"\xff\xfea\x00");
        assert_eq!(bytes(Encoding::Utf16Be), b"\xfe\xff\x00a");
    }

    #[test]
    fn invalid_utf8_without_bom_is_ansi() {
        assert_eq!(Encoding::detect(b"plain").0, Encoding::Utf8);
        assert_eq!(Encoding::detect(b"Caf\xe9").0, Encoding::Ansi);
    }

    #[test]
    fn odd_utf16_is_undecodable() {
        assert!(Encoding::Utf16Le.decode(vec![0x41, 0x00, 0x42]).is_none());
    }

    #[test]
    fn crlf_is_restored_on_save() {
        let bytes = encode("a\nb".into(), format(Encoding::Utf8, LineEnding::Crlf))
            .unwrap()
            .bytes;
        assert_eq!(bytes, b"a\r\nb");
    }

    #[test]
    fn ansi_flags_characters_it_cannot_hold() {
        if unsafe { GetACP() } == CP_UTF8 {
            return;
        }
        let ansi = format(Encoding::Ansi, LineEnding::Lf);
        let fits = encode("Cafe".into(), ansi).unwrap();
        assert!(!fits.lossy);
        assert_eq!(fits.bytes, b"Cafe");
        let lossy = encode("Cafe 😀".into(), ansi).unwrap();
        assert!(lossy.lossy);
        assert_eq!(lossy.bytes, b"Cafe ??");
    }
}
