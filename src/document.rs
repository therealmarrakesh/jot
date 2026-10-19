use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::ERROR_UNABLE_TO_MOVE_REPLACEMENT_2;
use windows_sys::Win32::Globalization::{
    CP_UTF8, GetACP, MultiByteToWideChar, WC_NO_BEST_FIT_CHARS, WideCharToMultiByte,
};
use windows_sys::Win32::Storage::FileSystem::{
    REPLACEFILE_IGNORE_ACL_ERRORS, REPLACEFILE_IGNORE_MERGE_ERRORS, ReplaceFileW,
};
use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::UI::Shell::{FOLDERID_Documents, KF_FLAG_DEFAULT, SHGetKnownFolderPath};

const UTF8_BOM: &[u8] = b"\xef\xbb\xbf";
const UTF16_LE_BOM: &[u8] = b"\xff\xfe";
const UTF16_BE_BOM: &[u8] = b"\xfe\xff";
const TEMP_ATTEMPTS: usize = 100;
const TEMP_SUFFIX: &str = ".jot-save";
const BACKUP_SUFFIX: &str = ".jot-backup";
const READ_ONLY: &str = "This file is read-only.";
const TOO_LARGE_FOR_ANSI: &str = "This text is too large to save as ANSI.";
const NOTICES: &str = include_str!("../assets/THIRD-PARTY-NOTICES.txt");
const NOTICES_FILE: &str = "THIRD-PARTY-NOTICES.txt";

pub(crate) const UNTITLED: &str = "Untitled";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum LineEnding {
    #[default]
    Crlf,
    Lf,
    Cr,
}

impl LineEnding {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Crlf => "Windows (CRLF)",
            Self::Lf => "Unix (LF)",
            Self::Cr => "Macintosh (CR)",
        }
    }

    fn split(text: String) -> (String, Self) {
        let line_ending = match text.find(['\r', '\n']) {
            Some(ix) if text[ix..].starts_with("\r\n") => Self::Crlf,
            Some(ix) if text[ix..].starts_with('\r') => Self::Cr,
            Some(_) => Self::Lf,
            None => Self::Crlf,
        };
        let text = if text.contains('\r') {
            text.replace("\r\n", "\n").replace('\r', "\n")
        } else {
            text
        };
        (text, line_ending)
    }
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
    let (text, line_ending) = LineEnding::split(text);
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
        LineEnding::Cr => text.replace('\n', "\r"),
        LineEnding::Lf => text,
    };
    format.encoding.encode(body)
}

pub(crate) fn write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let target = resolve(path)?;
    let existing = match fs::metadata(&target) {
        Ok(metadata) if metadata.permissions().readonly() => {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, READ_ONLY));
        }
        Ok(_) => true,
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => return Err(error),
    };
    let (temp, mut file) = create_temp(&target)?;
    let written = file.write_all(bytes).and_then(|()| file.sync_all());
    drop(file);
    if let Err(error) = written {
        fs::remove_file(&temp).ok();
        return Err(error);
    }
    if existing {
        replace(path, &target, &temp)
    } else {
        fs::rename(&temp, &target).inspect_err(|_| {
            fs::remove_file(&temp).ok();
        })
    }
}

fn resolve(path: &Path) -> io::Result<PathBuf> {
    match fs::canonicalize(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let parent = path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            Ok(fs::canonicalize(parent)?.join(path.file_name().unwrap_or_default()))
        }
        resolved => resolved,
    }
}

fn replace(path: &Path, target: &Path, temp: &Path) -> io::Result<()> {
    let backup = free_sibling(target, BACKUP_SUFFIX)?;
    let (replaced, replacement, backed_up) = (wide(target), wide(temp), wide(&backup));
    let done = unsafe {
        ReplaceFileW(
            replaced.as_ptr(),
            replacement.as_ptr(),
            backed_up.as_ptr(),
            REPLACEFILE_IGNORE_MERGE_ERRORS | REPLACEFILE_IGNORE_ACL_ERRORS,
            null(),
            null(),
        )
    } != 0;
    if done {
        fs::remove_file(&backup).ok();
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(ERROR_UNABLE_TO_MOVE_REPLACEMENT_2 as i32)
        && fs::rename(&backup, target).is_err()
    {
        let shown = |sibling: &Path| path.with_file_name(sibling.file_name().unwrap_or_default());
        return Err(io::Error::new(
            error.kind(),
            format!(
                "{error}\nThe original file was kept as {}\nThe new text was saved as {}",
                shown(&backup).display(),
                shown(temp).display()
            ),
        ));
    }
    fs::remove_file(temp).ok();
    Err(error)
}

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

fn free_sibling(path: &Path, suffix: &str) -> io::Result<PathBuf> {
    (0..TEMP_ATTEMPTS)
        .map(|attempt| sibling(path, attempt, suffix))
        .find(|candidate| fs::symlink_metadata(candidate).is_err())
        .ok_or_else(|| io::Error::from(io::ErrorKind::AlreadyExists))
}

fn create_temp(path: &Path) -> io::Result<(PathBuf, fs::File)> {
    for attempt in 0..TEMP_ATTEMPTS {
        let temp = sibling(path, attempt, TEMP_SUFFIX);
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
        {
            Ok(file) => return Ok((temp, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::from(io::ErrorKind::AlreadyExists))
}

pub(crate) fn write_notices() -> io::Result<PathBuf> {
    let dir = std::env::temp_dir().join(crate::APP_NAME);
    fs::create_dir_all(&dir)?;
    let path = dir.join(NOTICES_FILE);
    fs::write(&path, NOTICES)?;
    Ok(path)
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
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            TOO_LARGE_FOR_ANSI,
        ));
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
        .or_else(documents)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn documents() -> Option<PathBuf> {
    let mut path = null_mut();
    let result = unsafe {
        SHGetKnownFolderPath(
            &FOLDERID_Documents,
            KF_FLAG_DEFAULT as u32,
            null_mut(),
            &mut path,
        )
    };
    let documents = (result >= 0 && !path.is_null()).then(|| {
        let length = (0..)
            .take_while(|&ix| unsafe { *path.add(ix) } != 0)
            .count();
        PathBuf::from(OsString::from_wide(unsafe {
            std::slice::from_raw_parts(path, length)
        }))
    });
    unsafe { CoTaskMemFree(path.cast()) };
    documents
}

fn sibling(path: &Path, attempt: usize, suffix: &str) -> PathBuf {
    let mut name = OsString::from(".");
    name.push(path.file_name().unwrap_or_default());
    if attempt > 0 {
        name.push(format!(".{attempt}"));
    }
    name.push(suffix);
    path.with_file_name(name)
}
