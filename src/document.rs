use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use windows_sys::Win32::Globalization::{
    CP_ACP, CP_UTF8, GetACP, MultiByteToWideChar, WC_NO_BEST_FIT_CHARS, WideCharToMultiByte,
};

const UTF8_BOM: &[u8] = b"\xef\xbb\xbf";
const UTF16_LE_BOM: &[u8] = b"\xff\xfe";
const UTF16_BE_BOM: &[u8] = b"\xfe\xff";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LineEnding {
    #[default]
    Crlf,
    Lf,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Encoding {
    #[default]
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
    Ansi,
}

impl Encoding {
    pub const SAVE_ORDER: [Self; 5] = [
        Self::Ansi,
        Self::Utf16Le,
        Self::Utf16Be,
        Self::Utf8,
        Self::Utf8Bom,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Utf8 => "UTF-8",
            Self::Utf8Bom => "UTF-8 with BOM",
            Self::Utf16Le => "UTF-16 LE",
            Self::Utf16Be => "UTF-16 BE",
            Self::Ansi => "ANSI",
        }
    }

    fn detect(bytes: &[u8]) -> (Self, &[u8]) {
        [
            (Self::Utf8Bom, UTF8_BOM),
            (Self::Utf16Le, UTF16_LE_BOM),
            (Self::Utf16Be, UTF16_BE_BOM),
        ]
        .into_iter()
        .find_map(|(encoding, bom)| Some((encoding, bytes.strip_prefix(bom)?)))
        .unwrap_or_else(|| match std::str::from_utf8(bytes) {
            Ok(_) => (Self::Utf8, bytes),
            Err(_) => (Self::Ansi, bytes),
        })
    }

    fn decode(self, body: &[u8]) -> Option<String> {
        let units = |unit: fn([u8; 2]) -> u16| {
            let (pairs, rest) = body.as_chunks::<2>();
            rest.is_empty()
                .then(|| pairs.iter().map(|&pair| unit(pair)).collect::<Vec<_>>())
        };
        match self {
            Self::Utf8 | Self::Utf8Bom => String::from_utf8(body.to_vec()).ok(),
            Self::Utf16Le => String::from_utf16(&units(u16::from_le_bytes)?).ok(),
            Self::Utf16Be => String::from_utf16(&units(u16::from_be_bytes)?).ok(),
            Self::Ansi => ansi_decode(body),
        }
    }

    fn encode(self, text: &str) -> Encoded {
        let units = |bom: &[u8], unit: fn(u16) -> [u8; 2]| Encoded {
            bytes: bom
                .iter()
                .copied()
                .chain(text.encode_utf16().flat_map(unit))
                .collect(),
            lossy: false,
        };
        match self {
            Self::Utf8 => Encoded {
                bytes: text.as_bytes().to_vec(),
                lossy: false,
            },
            Self::Utf8Bom => Encoded {
                bytes: [UTF8_BOM, text.as_bytes()].concat(),
                lossy: false,
            },
            Self::Utf16Le => units(UTF16_LE_BOM, u16::to_le_bytes),
            Self::Utf16Be => units(UTF16_BE_BOM, u16::to_be_bytes),
            Self::Ansi => ansi_encode(text),
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Format {
    pub line_ending: LineEnding,
    pub encoding: Encoding,
}

pub struct Loaded {
    pub text: String,
    pub format: Format,
}

pub enum LoadError {
    Io(io::Error),
    Undecodable,
}

pub struct Encoded {
    pub bytes: Vec<u8>,
    pub lossy: bool,
}

pub fn load(path: &Path) -> Result<Loaded, LoadError> {
    let bytes = fs::read(path).map_err(LoadError::Io)?;
    let (encoding, body) = Encoding::detect(&bytes);
    let text = encoding.decode(body).ok_or(LoadError::Undecodable)?;
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

pub fn encode(text: &str, format: Format) -> Encoded {
    let body = match format.line_ending {
        LineEnding::Crlf => text.replace('\n', "\r\n"),
        LineEnding::Lf => text.to_string(),
    };
    format.encoding.encode(&body)
}

pub fn write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let temp = temp_path(path);
    fs::write(&temp, bytes)?;
    fs::rename(&temp, path).inspect_err(|_| {
        fs::remove_file(&temp).ok();
    })
}

fn ansi_code_page() -> u32 {
    match unsafe { GetACP() } {
        CP_UTF8 => CP_UTF8,
        _ => CP_ACP,
    }
}

fn ansi_decode(bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() {
        return Some(String::new());
    }
    let input = i32::try_from(bytes.len()).ok()?;
    let page = ansi_code_page();
    let length = unsafe { MultiByteToWideChar(page, 0, bytes.as_ptr(), input, null_mut(), 0) };
    let mut wide = vec![0u16; usize::try_from(length).ok().filter(|&n| n > 0)?];
    let written =
        unsafe { MultiByteToWideChar(page, 0, bytes.as_ptr(), input, wide.as_mut_ptr(), length) };
    wide.truncate(usize::try_from(written).ok()?);
    Some(String::from_utf16_lossy(&wide))
}

fn ansi_encode(text: &str) -> Encoded {
    let wide: Vec<u16> = text.encode_utf16().collect();
    let Ok(input) = i32::try_from(wide.len()) else {
        return Encoded {
            bytes: text.as_bytes().to_vec(),
            lossy: true,
        };
    };
    if input == 0 {
        return Encoded {
            bytes: Vec::new(),
            lossy: false,
        };
    }
    let page = ansi_code_page();
    if page == CP_UTF8 {
        return Encoded {
            bytes: text.as_bytes().to_vec(),
            lossy: false,
        };
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
    bytes.truncate(usize::try_from(written).unwrap_or(0));
    Encoded {
        bytes,
        lossy: used_default != 0,
    }
}

pub fn display_name(path: Option<&Path>) -> String {
    path.and_then(Path::file_name)
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Untitled".into())
}

pub fn default_directory(path: Option<&Path>) -> PathBuf {
    path.and_then(Path::parent)
        .map(Path::to_path_buf)
        .or_else(|| {
            std::env::var_os("USERPROFILE").map(|home| PathBuf::from(home).join("Documents"))
        })
        .unwrap_or_else(|| PathBuf::from("."))
}

fn temp_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    path.with_file_name(format!(".{name}.jot-save"))
}
