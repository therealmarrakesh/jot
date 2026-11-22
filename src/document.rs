use gpui_base::input::{Rope, RopeBuilder};
use std::ffi::OsString;
use std::fs;
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
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
const IO_CHUNK: usize = 64 * 1024;
const ANSI_WIDE_CHUNK: usize = 32 * 1024;
const TEMP_ATTEMPTS: usize = 100;
const TEMP_SUFFIX: &str = ".jot-save";
const BACKUP_SUFFIX: &str = ".jot-backup";
const READ_ONLY: &str = "This file is read-only.";
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
}

struct LineNormalizer {
    builder: RopeBuilder,
    line_ending: Option<LineEnding>,
    pending_cr: bool,
    scratch: String,
}

impl LineNormalizer {
    fn new() -> Self {
        Self {
            builder: RopeBuilder::new(),
            line_ending: None,
            pending_cr: false,
            scratch: String::new(),
        }
    }

    fn push(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let bytes = text.as_bytes();
        if !self.pending_cr && !bytes.contains(&b'\r') {
            if self.line_ending.is_none() && bytes.contains(&b'\n') {
                self.line_ending = Some(LineEnding::Lf);
            }
            self.builder.append(text);
            return;
        }
        self.scratch.clear();
        let mut i = 0;
        let mut start = 0;
        if self.pending_cr {
            self.pending_cr = false;
            if bytes[0] == b'\n' {
                self.line_ending.get_or_insert(LineEnding::Crlf);
                self.scratch.push('\n');
                i = 1;
                start = 1;
            } else {
                self.line_ending.get_or_insert(LineEnding::Cr);
                self.scratch.push('\n');
            }
        }
        while i < bytes.len() {
            match bytes[i] {
                b'\n' => {
                    self.line_ending.get_or_insert(LineEnding::Lf);
                    i += 1;
                }
                b'\r' => {
                    self.scratch.push_str(&text[start..i]);
                    if i + 1 == bytes.len() {
                        self.pending_cr = true;
                        start = bytes.len();
                        break;
                    } else if bytes[i + 1] == b'\n' {
                        self.line_ending.get_or_insert(LineEnding::Crlf);
                        self.scratch.push('\n');
                        i += 2;
                        start = i;
                    } else {
                        self.line_ending.get_or_insert(LineEnding::Cr);
                        self.scratch.push('\n');
                        i += 1;
                        start = i;
                    }
                }
                _ => i += 1,
            }
        }
        if start < bytes.len() {
            self.scratch.push_str(&text[start..]);
        }
        if !self.scratch.is_empty() {
            self.builder.append(&self.scratch);
        }
    }

    fn finish(mut self) -> (Rope, LineEnding) {
        if self.pending_cr {
            self.line_ending.get_or_insert(LineEnding::Cr);
            self.builder.append("\n");
        }
        (
            self.builder.finish(),
            self.line_ending.unwrap_or(LineEnding::Crlf),
        )
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
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Format {
    pub(crate) line_ending: LineEnding,
    pub(crate) encoding: Encoding,
}

pub(crate) struct Loaded {
    pub(crate) text: Rope,
    pub(crate) format: Format,
}

#[derive(Debug)]
pub(crate) enum LoadError {
    Io(io::Error),
    Undecodable,
}

pub(crate) fn load(path: &Path) -> Result<Loaded, LoadError> {
    let mut file = fs::File::open(path).map_err(LoadError::Io)?;
    load_reader(&mut file, IO_CHUNK)
}

fn load_reader<R: Read + Seek>(reader: &mut R, chunk_size: usize) -> Result<Loaded, LoadError> {
    let chunk_size = chunk_size.max(4);
    let mut head = [0u8; 3];
    let mut head_len = 0;
    while head_len < head.len() {
        let n = reader.read(&mut head[head_len..]).map_err(LoadError::Io)?;
        if n == 0 {
            break;
        }
        head_len += n;
    }
    let head_slice = &head[..head_len];
    let (bom_encoding, initial) = if head_slice.starts_with(UTF8_BOM) {
        (Some(Encoding::Utf8Bom), &head_slice[UTF8_BOM.len()..])
    } else if head_slice.starts_with(UTF16_LE_BOM) {
        (Some(Encoding::Utf16Le), &head_slice[UTF16_LE_BOM.len()..])
    } else if head_slice.starts_with(UTF16_BE_BOM) {
        (Some(Encoding::Utf16Be), &head_slice[UTF16_BE_BOM.len()..])
    } else {
        (None, head_slice)
    };

    match bom_encoding {
        Some(Encoding::Utf16Le) => {
            stream_utf16(reader, initial, chunk_size, u16::from_le_bytes, Encoding::Utf16Le)
        }
        Some(Encoding::Utf16Be) => {
            stream_utf16(reader, initial, chunk_size, u16::from_be_bytes, Encoding::Utf16Be)
        }
        Some(Encoding::Utf8Bom) => stream_utf8(reader, initial, chunk_size, true),
        None => match stream_utf8(reader, initial, chunk_size, false) {
            Ok(loaded) => Ok(loaded),
            Err(LoadError::Undecodable) => {
                reader.seek(SeekFrom::Start(0)).map_err(LoadError::Io)?;
                let mut bytes = Vec::new();
                reader.read_to_end(&mut bytes).map_err(LoadError::Io)?;
                let decoded = ansi_decode(&bytes).ok_or(LoadError::Undecodable)?;
                let mut normalizer = LineNormalizer::new();
                normalizer.push(&decoded);
                let (text, line_ending) = normalizer.finish();
                Ok(Loaded {
                    text,
                    format: Format {
                        line_ending,
                        encoding: Encoding::Ansi,
                    },
                })
            }
            Err(error) => Err(error),
        },
        Some(Encoding::Utf8 | Encoding::Ansi) => unreachable!(),
    }
}

fn stream_utf8<R: Read>(
    reader: &mut R,
    initial: &[u8],
    chunk_size: usize,
    has_bom: bool,
) -> Result<Loaded, LoadError> {
    let mut normalizer = LineNormalizer::new();
    let mut buf = vec![0u8; chunk_size + 4];
    let mut carry = initial.len();
    buf[..carry].copy_from_slice(initial);

    loop {
        let read = reader.read(&mut buf[carry..]).map_err(LoadError::Io)?;
        let filled = carry + read;
        let eof = read == 0;
        if filled == 0 {
            break;
        }
        match std::str::from_utf8(&buf[..filled]) {
            Ok(valid) => {
                normalizer.push(valid);
                carry = 0;
            }
            Err(error) => {
                let valid = error.valid_up_to();
                if valid > 0 {
                    let prefix = std::str::from_utf8(&buf[..valid]).expect("valid UTF-8 prefix");
                    normalizer.push(prefix);
                }
                if error.error_len().is_some() || eof {
                    return Err(LoadError::Undecodable);
                }
                let remaining = filled - valid;
                buf.copy_within(valid..filled, 0);
                carry = remaining;
            }
        }
        if eof {
            break;
        }
    }

    let (text, line_ending) = normalizer.finish();
    Ok(Loaded {
        text,
        format: Format {
            line_ending,
            encoding: if has_bom {
                Encoding::Utf8Bom
            } else {
                Encoding::Utf8
            },
        },
    })
}

fn stream_utf16<R: Read>(
    reader: &mut R,
    initial: &[u8],
    chunk_size: usize,
    unit: fn([u8; 2]) -> u16,
    encoding: Encoding,
) -> Result<Loaded, LoadError> {
    let mut normalizer = LineNormalizer::new();
    let mut buf = vec![0u8; chunk_size + 4];
    let mut carry = initial.len();
    buf[..carry].copy_from_slice(initial);
    let mut decoded = String::with_capacity(chunk_size);

    loop {
        let read = reader.read(&mut buf[carry..]).map_err(LoadError::Io)?;
        let filled = carry + read;
        let eof = read == 0;
        if filled == 0 {
            break;
        }
        if eof && !filled.is_multiple_of(2) {
            return Err(LoadError::Undecodable);
        }
        let mut decode_end = filled & !1;
        if !eof && decode_end >= 2 {
            let last = unit([buf[decode_end - 2], buf[decode_end - 1]]);
            if (0xD800..=0xDBFF).contains(&last) {
                decode_end -= 2;
            }
        }
        if decode_end > 0 {
            decoded.clear();
            let (pairs, _) = buf[..decode_end].as_chunks::<2>();
            for ch in char::decode_utf16(pairs.iter().map(|&pair| unit(pair))) {
                decoded.push(ch.map_err(|_| LoadError::Undecodable)?);
            }
            normalizer.push(&decoded);
            let remaining = filled - decode_end;
            buf.copy_within(decode_end..filled, 0);
            carry = remaining;
        } else if eof {
            break;
        } else {
            carry = filled;
        }
        if eof {
            break;
        }
    }

    let (text, line_ending) = normalizer.finish();
    Ok(Loaded {
        text,
        format: Format {
            line_ending,
            encoding,
        },
    })
}

fn write_with_line_ending(
    chunk: &str,
    line_ending: LineEnding,
    mut emit: impl FnMut(&str) -> io::Result<()>,
) -> io::Result<()> {
    let replacement = match line_ending {
        LineEnding::Crlf => "\r\n",
        LineEnding::Cr => "\r",
        LineEnding::Lf => return emit(chunk),
    };
    let mut rest = chunk;
    while let Some(ix) = rest.find('\n') {
        if ix > 0 {
            emit(&rest[..ix])?;
        }
        emit(replacement)?;
        rest = &rest[ix + 1..];
    }
    if !rest.is_empty() {
        emit(rest)?;
    }
    Ok(())
}

pub(crate) fn is_lossy(rope: &Rope, format: Format) -> io::Result<bool> {
    if format.encoding != Encoding::Ansi {
        return Ok(false);
    }
    let page = unsafe { GetACP() };
    if page == CP_UTF8 {
        return Ok(false);
    }
    let mut wide = Vec::with_capacity(ANSI_WIDE_CHUNK);
    for chunk in rope.chunks() {
        for unit in chunk.encode_utf16() {
            wide.push(unit);
            if wide.len() >= ANSI_WIDE_CHUNK {
                if ansi_wide_lossy(page, &wide)? {
                    return Ok(true);
                }
                wide.clear();
            }
        }
    }
    if !wide.is_empty() && ansi_wide_lossy(page, &wide)? {
        return Ok(true);
    }
    Ok(false)
}

fn stream_encode<W: Write>(rope: &Rope, format: Format, out: &mut W) -> io::Result<()> {
    match format.encoding {
        Encoding::Utf8 => {
            for chunk in rope.chunks() {
                write_with_line_ending(chunk, format.line_ending, |part| {
                    out.write_all(part.as_bytes())
                })?;
            }
        }
        Encoding::Utf8Bom => {
            out.write_all(UTF8_BOM)?;
            for chunk in rope.chunks() {
                write_with_line_ending(chunk, format.line_ending, |part| {
                    out.write_all(part.as_bytes())
                })?;
            }
        }
        Encoding::Utf16Le => stream_encode_utf16(
            rope,
            format.line_ending,
            UTF16_LE_BOM,
            u16::to_le_bytes,
            out,
        )?,
        Encoding::Utf16Be => stream_encode_utf16(
            rope,
            format.line_ending,
            UTF16_BE_BOM,
            u16::to_be_bytes,
            out,
        )?,
        Encoding::Ansi => stream_encode_ansi(rope, format.line_ending, out)?,
    }
    Ok(())
}

fn stream_encode_utf16<W: Write>(
    rope: &Rope,
    line_ending: LineEnding,
    bom: &[u8],
    to_bytes: fn(u16) -> [u8; 2],
    out: &mut W,
) -> io::Result<()> {
    out.write_all(bom)?;
    let mut buf = Vec::with_capacity(8 * 1024);
    for chunk in rope.chunks() {
        write_with_line_ending(chunk, line_ending, |part| {
            for unit in part.encode_utf16() {
                buf.extend_from_slice(&to_bytes(unit));
                if buf.len() >= 8 * 1024 {
                    out.write_all(&buf)?;
                    buf.clear();
                }
            }
            Ok(())
        })?;
    }
    if !buf.is_empty() {
        out.write_all(&buf)?;
    }
    Ok(())
}

fn stream_encode_ansi<W: Write>(
    rope: &Rope,
    line_ending: LineEnding,
    out: &mut W,
) -> io::Result<()> {
    let page = unsafe { GetACP() };
    if page == CP_UTF8 {
        for chunk in rope.chunks() {
            write_with_line_ending(chunk, line_ending, |part| out.write_all(part.as_bytes()))?;
        }
        return Ok(());
    }
    let mut wide = Vec::with_capacity(ANSI_WIDE_CHUNK);
    let mut bytes = Vec::new();
    for chunk in rope.chunks() {
        write_with_line_ending(chunk, line_ending, |part| {
            for ch in part.chars() {
                let mut units = [0u16; 2];
                let encoded = ch.encode_utf16(&mut units);
                if wide.len() + encoded.len() > ANSI_WIDE_CHUNK {
                    encode_ansi_wide(page, &wide, &mut bytes)?;
                    out.write_all(&bytes)?;
                    wide.clear();
                }
                wide.extend_from_slice(encoded);
            }
            Ok(())
        })?;
    }
    if !wide.is_empty() {
        encode_ansi_wide(page, &wide, &mut bytes)?;
        out.write_all(&bytes)?;
    }
    Ok(())
}

pub(crate) fn write_rope(path: &Path, rope: &Rope, format: Format) -> io::Result<()> {
    let target = resolve(path)?;
    let existing = match fs::metadata(&target) {
        Ok(metadata) if metadata.permissions().readonly() => {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, READ_ONLY));
        }
        Ok(_) => true,
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => return Err(error),
    };
    let (temp, file) = create_temp(&target)?;
    let written = {
        let mut writer = BufWriter::with_capacity(IO_CHUNK, file);
        stream_encode(rope, format, &mut writer)
            .and_then(|()| writer.flush())
            .and_then(|()| writer.into_inner().map_err(|e| e.into_error()))
            .and_then(|file| file.sync_all())
    };
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

fn ansi_wide_lossy(page: u32, wide: &[u16]) -> io::Result<bool> {
    if wide.is_empty() {
        return Ok(false);
    }
    let input = i32::try_from(wide.len()).expect("chunk fits in i32");
    let mut used_default = 0;
    let written = unsafe {
        WideCharToMultiByte(
            page,
            WC_NO_BEST_FIT_CHARS,
            wide.as_ptr(),
            input,
            null_mut(),
            0,
            null(),
            &mut used_default,
        )
    };
    if written == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(used_default != 0)
}

fn encode_ansi_wide(page: u32, wide: &[u16], out: &mut Vec<u8>) -> io::Result<()> {
    out.clear();
    if wide.is_empty() {
        return Ok(());
    }
    let input = i32::try_from(wide.len()).expect("chunk fits in i32");
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
    out.resize(usize::try_from(length).unwrap_or(0), 0);
    let written = unsafe {
        WideCharToMultiByte(
            page,
            WC_NO_BEST_FIT_CHARS,
            wide.as_ptr(),
            input,
            out.as_mut_ptr(),
            length,
            null(),
            null_mut(),
        )
    };
    if written == 0 {
        return Err(io::Error::last_os_error());
    }
    out.truncate(usize::try_from(written).unwrap_or(0));
    Ok(())
}

pub(crate) fn same_path(a: &Path, b: &Path) -> bool {
    if let (Ok(ca), Ok(cb)) = (fs::canonicalize(a), fs::canonicalize(b)) {
        return path_key(&ca) == path_key(&cb);
    }
    let na = std::path::absolute(a).unwrap_or_else(|_| a.to_path_buf());
    let nb = std::path::absolute(b).unwrap_or_else(|_| b.to_path_buf());
    path_key(&na) == path_key(&nb)
}

fn path_key(path: &Path) -> String {
    let raw = path.to_string_lossy();
    let stripped = raw.strip_prefix(r"\\?\").unwrap_or(&raw);
    stripped.replace('/', "\\").to_lowercase()
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

#[cfg(test)]
mod tests {
    use super::*;

    fn load_bytes(bytes: &[u8], chunk_size: usize) -> Result<Loaded, LoadError> {
        let mut cursor = io::Cursor::new(bytes);
        load_reader(&mut cursor, chunk_size)
    }

    #[test]
    fn detects_boms_and_utf8_vs_ansi() {
        assert_eq!(load_bytes(b"hello", 4).unwrap().format.encoding, Encoding::Utf8);
        assert_eq!(
            load_bytes(b"\xef\xbb\xbfhello", 4).unwrap().format.encoding,
            Encoding::Utf8Bom
        );
        assert_eq!(
            load_bytes(b"\xff\xfeh\0", 4).unwrap().format.encoding,
            Encoding::Utf16Le
        );
        assert_eq!(
            load_bytes(b"\xfe\xff\0h", 4).unwrap().format.encoding,
            Encoding::Utf16Be
        );
        // Late invalid UTF-8 byte falls back to ANSI cleanly across chunks.
        let mut ansi = vec![b'a'; 20];
        ansi.extend_from_slice(&[0x80, 0xFF]);
        assert_eq!(load_bytes(&ansi, 5).unwrap().format.encoding, Encoding::Ansi);
    }

    #[test]
    fn splits_and_normalizes_line_endings_across_chunk_boundaries() {
        for chunk in [4, 5, 7, 64] {
            let plain = load_bytes(b"plain", chunk).unwrap();
            assert_eq!(plain.text.to_string(), "plain");
            assert_eq!(plain.format.line_ending, LineEnding::Crlf);

            let crlf = load_bytes(b"a\r\nb\r\n", chunk).unwrap();
            assert_eq!(crlf.text.to_string(), "a\nb\n");
            assert_eq!(crlf.format.line_ending, LineEnding::Crlf);

            let lf = load_bytes(b"a\nb\n", chunk).unwrap();
            assert_eq!(lf.text.to_string(), "a\nb\n");
            assert_eq!(lf.format.line_ending, LineEnding::Lf);

            let cr = load_bytes(b"a\rb\r", chunk).unwrap();
            assert_eq!(cr.text.to_string(), "a\nb\n");
            assert_eq!(cr.format.line_ending, LineEnding::Cr);

            let mixed = load_bytes(b"a\r\r\nb\r", chunk).unwrap();
            assert_eq!(mixed.text.to_string(), "a\n\nb\n");
            assert_eq!(mixed.format.line_ending, LineEnding::Cr);
        }
    }

    #[test]
    fn streaming_encode_and_decode_round_trip_including_surrogates() {
        let sample = "hello\nworld — 日本語 😀\nend";
        let rope = Rope::from(sample);
        for encoding in [
            Encoding::Utf8,
            Encoding::Utf8Bom,
            Encoding::Utf16Le,
            Encoding::Utf16Be,
        ] {
            for line_ending in [LineEnding::Crlf, LineEnding::Lf, LineEnding::Cr] {
                let format = Format {
                    line_ending,
                    encoding,
                };
                assert!(!is_lossy(&rope, format).unwrap());
                let mut bytes = Vec::new();
                stream_encode(&rope, format, &mut bytes).unwrap();
                // Torture-test tiny chunk sizes splitting multi-byte UTF-8, CRLF, and UTF-16 surrogates.
                for chunk in [4, 5, 7, 64] {
                    let loaded = load_bytes(&bytes, chunk).unwrap();
                    assert_eq!(loaded.text.to_string(), sample);
                    assert_eq!(loaded.format, format);
                }
            }
        }
        // Reject odd-length UTF-16 payload and invalid UTF-8-with-BOM.
        assert!(load_bytes(&[0xff, 0xfe, b'a'], 4).is_err());
        assert!(load_bytes(&[0xef, 0xbb, 0xbf, 0xff], 4).is_err());
    }

    #[test]
    fn same_path_ignores_windows_casing_and_separators() {
        assert!(same_path(
            Path::new(r"C:\Notes\Todo.txt"),
            Path::new(r"c:/notes/todo.txt"),
        ));
        assert_eq!(
            path_key(Path::new(r"\\?\C:\Notes\Todo.txt")),
            path_key(Path::new(r"c:\notes\todo.txt")),
        );
        assert!(!same_path(
            Path::new(r"C:\Notes\Todo.txt"),
            Path::new(r"C:\Notes\Other.txt"),
        ));
    }

    #[test]
    fn load_and_atomic_write_round_trip_and_respect_readonly() {
        let dir = std::env::temp_dir().join(format!("jot-doc-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("sample.txt");

        let format = Format {
            line_ending: LineEnding::Lf,
            encoding: Encoding::Utf8Bom,
        };
        write_rope(&file, &Rope::from("first\nline"), format).unwrap();
        let loaded = load(&file).unwrap();
        assert_eq!(loaded.text.to_string(), "first\nline");
        assert_eq!(loaded.format.encoding, Encoding::Utf8Bom);
        assert_eq!(loaded.format.line_ending, LineEnding::Lf);

        // Overwrite via ReplaceFileW path
        write_rope(&file, &Rope::from("second\nline"), format).unwrap();
        let loaded = load(&file).unwrap();
        assert_eq!(loaded.text.to_string(), "second\nline");

        // Read-only rejection
        let original = fs::metadata(&file).unwrap().permissions();
        let mut readonly = original.clone();
        readonly.set_readonly(true);
        fs::set_permissions(&file, readonly).unwrap();
        let err = write_rope(&file, &Rope::from("blocked"), format).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);

        fs::set_permissions(&file, original).unwrap();
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn multi_chunk_streaming_round_trip() {
        let dir = std::env::temp_dir().join(format!("jot-doc-large-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("large.txt");

        // Build a >200 KB Rope spanning many IO_CHUNK (64 KB) boundaries with ASCII + Unicode + emoji.
        let mut builder = RopeBuilder::new();
        for i in 0..4000 {
            builder.append(&format!("line {i:04}: alpha beta — 日本語 😀\n"));
        }
        let original = builder.finish();
        assert!(original.len() > IO_CHUNK * 2);

        for format in [
            Format {
                line_ending: LineEnding::Crlf,
                encoding: Encoding::Utf8,
            },
            Format {
                line_ending: LineEnding::Lf,
                encoding: Encoding::Utf8Bom,
            },
            Format {
                line_ending: LineEnding::Crlf,
                encoding: Encoding::Utf16Le,
            },
            Format {
                line_ending: LineEnding::Cr,
                encoding: Encoding::Utf16Be,
            },
        ] {
            write_rope(&file, &original, format).unwrap();
            let loaded = load(&file).unwrap();
            assert_eq!(loaded.format, format);
            assert_eq!(loaded.text, original);
        }

        fs::remove_dir_all(&dir).ok();
    }
}
