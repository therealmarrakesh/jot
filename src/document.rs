use std::ffi::OsString;
use std::fs;
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use ropey::{Rope, RopeBuilder};

use crate::platform::{
    AnsiCodePage, HIDDEN_FILE_ATTRIBUTE, ReplaceFileError, documents_dir, file_hard_link_count,
    replace_file,
};

const UTF8_BOM: &[u8] = b"\xef\xbb\xbf";
const UTF16_LE_BOM: &[u8] = b"\xff\xfe";
const UTF16_BE_BOM: &[u8] = b"\xfe\xff";
const IO_CHUNK: usize = 64 * 1024;
const ANSI_WIDE_CHUNK: usize = 32 * 1024;
const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;

const UNTITLED: &str = "Untitled";

#[derive(Debug)]
pub(crate) enum SaveError {
    ReadOnly,
    Kept {
        source: io::Error,
        backup: PathBuf,
        temp: PathBuf,
    },
    Io(io::Error),
}

impl From<io::Error> for SaveError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

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
    pub(crate) mixed_line_endings: bool,
    pub(crate) lossy_decode: bool,
}

impl Format {
    pub(crate) fn line_ending_label(self) -> &'static str {
        if self.mixed_line_endings {
            match self.line_ending {
                LineEnding::Crlf => "Mixed (CRLF)",
                LineEnding::Lf => "Mixed (LF)",
                LineEnding::Cr => "Mixed (CR)",
            }
        } else {
            self.line_ending.label()
        }
    }
}

impl Format {
    /// The format a file has once saved in this one: one line ending throughout, and nothing a
    /// lossy read replaced.
    pub(crate) fn written(self) -> Self {
        Self {
            mixed_line_endings: false,
            lossy_decode: false,
            ..self
        }
    }
}

pub(crate) struct Loaded {
    pub(crate) doc_path: DocPath,
    pub(crate) text: Rope,
    pub(crate) format: Format,
}

#[derive(Debug)]
pub(crate) enum LoadError {
    Io(io::Error),
    Undecodable,
    TooLarge(u64),
}

pub(crate) fn load(doc_path: &DocPath) -> Result<Loaded, LoadError> {
    let mut file = fs::File::open(doc_path.as_path()).map_err(LoadError::Io)?;
    if let Ok(meta) = file.metadata()
        && meta.len() > MAX_FILE_BYTES
    {
        return Err(LoadError::TooLarge(meta.len()));
    }
    let decoded = load_reader_with_page(&mut file, IO_CHUNK, AnsiCodePage::current())?;
    Ok(Loaded {
        doc_path: doc_path.clone(),
        text: decoded.text,
        format: decoded.format,
    })
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct PathKey(String);

impl PathKey {
    pub(crate) fn new(path: &Path) -> Self {
        let resolved = resolve(path).unwrap_or_else(|_| path.to_path_buf());
        Self(normalize_path_key(&resolved))
    }
}

#[derive(Clone, Debug)]
pub(crate) struct DocPath {
    path: PathBuf,
    key: PathKey,
}

impl DocPath {
    pub(crate) fn new(path: PathBuf) -> Self {
        let path = std::path::absolute(&path).unwrap_or(path);
        let key = PathKey::new(&path);
        Self { path, key }
    }

    pub(crate) fn as_path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn key(&self) -> &PathKey {
        &self.key
    }
}

fn normalize_path_key(path: &Path) -> String {
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
        .or_else(documents_dir)
        .unwrap_or_else(|| PathBuf::from("."))
}

const UTF32_LE_BOM: &[u8] = b"\xff\xfe\x00\x00";
const UTF32_BE_BOM: &[u8] = b"\x00\x00\xfe\xff";

struct Decoded {
    text: Rope,
    format: Format,
}

#[derive(Default)]
struct LineNormalizer {
    builder: RopeBuilder,
    line_ending: Option<LineEnding>,
    mixed: bool,
    pending_cr: bool,
    scratch: String,
}

impl LineNormalizer {
    fn record_ending(&mut self, ending: LineEnding) {
        match self.line_ending {
            None => self.line_ending = Some(ending),
            Some(first) if first != ending => self.mixed = true,
            Some(_) => {}
        }
    }

    fn push(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let bytes = text.as_bytes();
        if !self.pending_cr && !bytes.contains(&b'\r') {
            if bytes.contains(&b'\n') {
                self.record_ending(LineEnding::Lf);
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
                self.record_ending(LineEnding::Crlf);
                self.scratch.push('\n');
                i = 1;
                start = 1;
            } else {
                self.record_ending(LineEnding::Cr);
                self.scratch.push('\n');
            }
        }
        while i < bytes.len() {
            match bytes[i] {
                b'\n' => {
                    self.record_ending(LineEnding::Lf);
                    i += 1;
                }
                b'\r' => {
                    self.scratch.push_str(&text[start..i]);
                    if i + 1 == bytes.len() {
                        self.pending_cr = true;
                        start = bytes.len();
                        break;
                    } else if bytes[i + 1] == b'\n' {
                        self.record_ending(LineEnding::Crlf);
                        self.scratch.push('\n');
                        i += 2;
                        start = i;
                    } else {
                        self.record_ending(LineEnding::Cr);
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

    fn finish(mut self) -> (Rope, LineEnding, bool) {
        if self.pending_cr {
            self.record_ending(LineEnding::Cr);
            self.builder.append("\n");
        }
        (
            self.builder.finish(),
            self.line_ending.unwrap_or(LineEnding::Crlf),
            self.mixed,
        )
    }
}

fn sniff_bomless_utf16(sample: &[u8]) -> Option<Encoding> {
    if sample.len() < 4 || !sample.len().is_multiple_of(2) {
        return None;
    }
    let chunks = sample.as_chunks::<2>().0;
    let pairs = chunks.len();
    let even_nul = chunks.iter().filter(|p| p[0] == 0).count();
    let odd_nul = chunks.iter().filter(|p| p[1] == 0).count();
    if odd_nul >= (pairs * 3) / 4 && even_nul == 0 {
        Some(Encoding::Utf16Le)
    } else if even_nul >= (pairs * 3) / 4 && odd_nul == 0 {
        Some(Encoding::Utf16Be)
    } else {
        None
    }
}

fn load_reader_with_page<R: Read + Seek>(
    reader: &mut R,
    chunk_size: usize,
    page: AnsiCodePage,
) -> Result<Decoded, LoadError> {
    let chunk_size = chunk_size.max(4);
    let mut head = [0u8; 4];
    let mut head_len = 0;
    while head_len < head.len() {
        let n = reader.read(&mut head[head_len..]).map_err(LoadError::Io)?;
        if n == 0 {
            break;
        }
        head_len += n;
    }
    let head_slice = &head[..head_len];
    if head_slice.starts_with(UTF32_LE_BOM) || head_slice.starts_with(UTF32_BE_BOM) {
        return Err(LoadError::Undecodable);
    }

    if let Some(rest) = head_slice.strip_prefix(UTF8_BOM) {
        return stream_utf8(reader, rest, chunk_size, true);
    }
    if let Some(rest) = head_slice.strip_prefix(UTF16_LE_BOM) {
        return stream_utf16(reader, rest, chunk_size, u16::from_le_bytes, Encoding::Utf16Le);
    }
    if let Some(rest) = head_slice.strip_prefix(UTF16_BE_BOM) {
        return stream_utf16(reader, rest, chunk_size, u16::from_be_bytes, Encoding::Utf16Be);
    }

    let mut probe = Vec::with_capacity(64);
    probe.extend_from_slice(head_slice);
    let mut extra = [0u8; 60];
    let extra_read = reader.read(&mut extra).map_err(LoadError::Io)?;
    probe.extend_from_slice(&extra[..extra_read]);
    if let Some(enc) = sniff_bomless_utf16(&probe) {
        reader.seek(SeekFrom::Start(0)).map_err(LoadError::Io)?;
        let unit = if enc == Encoding::Utf16Le {
            u16::from_le_bytes
        } else {
            u16::from_be_bytes
        };
        if let Ok(decoded) = stream_utf16(reader, &[], chunk_size, unit, enc) {
            return Ok(decoded);
        }
        reader.seek(SeekFrom::Start(0)).map_err(LoadError::Io)?;
        probe.clear();
    }

    match stream_utf8(reader, &probe, chunk_size, false) {
        Err(LoadError::Undecodable) => {
            reader.seek(SeekFrom::Start(0)).map_err(LoadError::Io)?;
            if page.is_utf8() {
                // ACP 65001 failed strict UTF-8; fall back to CP 1252 and flag lossy_decode so save warns.
                let mut decoded = stream_ansi(reader, chunk_size, AnsiCodePage::CP_1252)?;
                decoded.format.lossy_decode = true;
                Ok(decoded)
            } else {
                stream_ansi(reader, chunk_size, page)
            }
        }
        result => result,
    }
}

fn stream_decode<R: Read>(
    reader: &mut R,
    initial: &[u8],
    chunk_size: usize,
    encoding: Encoding,
    mut step: impl FnMut(&[u8], bool, &mut LineNormalizer) -> Result<usize, LoadError>,
) -> Result<Decoded, LoadError> {
    let mut normalizer = LineNormalizer::default();
    let mut buf = vec![0u8; chunk_size.max(initial.len()) + 4];
    let mut carry = initial.len();
    buf[..carry].copy_from_slice(initial);

    loop {
        let read = reader.read(&mut buf[carry..]).map_err(LoadError::Io)?;
        let filled = carry + read;
        let eof = read == 0;
        if filled == 0 {
            break;
        }
        let consumed = step(&buf[..filled], eof, &mut normalizer)?;
        let remaining = filled - consumed;
        if remaining > 0 {
            buf.copy_within(consumed..filled, 0);
        }
        carry = remaining;
        if eof {
            break;
        }
    }

    let (text, line_ending, mixed_line_endings) = normalizer.finish();
    Ok(Decoded {
        text,
        format: Format {
            line_ending,
            encoding,
            mixed_line_endings,
            lossy_decode: false,
        },
    })
}

fn stream_ansi<R: Read>(
    reader: &mut R,
    chunk_size: usize,
    page: AnsiCodePage,
) -> Result<Decoded, LoadError> {
    let mut wide = Vec::new();
    let mut decoded = String::new();
    stream_decode(reader, &[], chunk_size, Encoding::Ansi, |bytes, eof, norm| {
        let mut end = bytes.len();
        if !eof && end > 0 {
            let trailing_leads = bytes
                .iter()
                .rev()
                .take_while(|&&b| page.is_dbcs_lead_byte(b))
                .count();
            if trailing_leads % 2 == 1 {
                end -= 1;
            }
        }
        if end > 0 {
            if !page
                .decode_chunk(&bytes[..end], &mut wide, &mut decoded)
                .map_err(LoadError::Io)?
            {
                return Err(LoadError::Undecodable);
            }
            norm.push(&decoded);
        }
        Ok(end)
    })
}

fn stream_utf8<R: Read>(
    reader: &mut R,
    initial: &[u8],
    chunk_size: usize,
    has_bom: bool,
) -> Result<Decoded, LoadError> {
    let encoding = if has_bom { Encoding::Utf8Bom } else { Encoding::Utf8 };
    stream_decode(reader, initial, chunk_size, encoding, |bytes, eof, norm| {
        match std::str::from_utf8(bytes) {
            Ok(valid) => {
                norm.push(valid);
                Ok(bytes.len())
            }
            Err(error) => {
                let valid = error.valid_up_to();
                if valid > 0 {
                    let prefix = std::str::from_utf8(&bytes[..valid]).expect("valid UTF-8 prefix");
                    norm.push(prefix);
                }
                if error.error_len().is_some() || eof {
                    return Err(LoadError::Undecodable);
                }
                Ok(valid)
            }
        }
    })
}

fn stream_utf16<R: Read>(
    reader: &mut R,
    initial: &[u8],
    chunk_size: usize,
    unit: fn([u8; 2]) -> u16,
    encoding: Encoding,
) -> Result<Decoded, LoadError> {
    let mut decoded = String::with_capacity(chunk_size);
    stream_decode(reader, initial, chunk_size, encoding, |bytes, eof, norm| {
        if eof && !bytes.len().is_multiple_of(2) {
            return Err(LoadError::Undecodable);
        }
        let mut end = bytes.len() & !1;
        if !eof && end >= 2 {
            let last = unit([bytes[end - 2], bytes[end - 1]]);
            if (0xD800..=0xDBFF).contains(&last) {
                end -= 2;
            }
        }
        if end > 0 {
            decoded.clear();
            let (pairs, _) = bytes[..end].as_chunks::<2>();
            for ch in char::decode_utf16(pairs.iter().map(|&pair| unit(pair))) {
                decoded.push(ch.map_err(|_| LoadError::Undecodable)?);
            }
            norm.push(&decoded);
        }
        Ok(end)
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
    is_lossy_with_page(rope, format, AnsiCodePage::current())
}

fn is_lossy_with_page(
    rope: &Rope,
    format: Format,
    page: AnsiCodePage,
) -> io::Result<bool> {
    if format.encoding != Encoding::Ansi {
        return Ok(false);
    }
    if format.lossy_decode {
        return Ok(true);
    }
    if page.is_utf8() {
        return Ok(false);
    }
    let mut wide = Vec::with_capacity(ANSI_WIDE_CHUNK);
    for chunk in rope.chunks() {
        for unit in chunk.encode_utf16() {
            wide.push(unit);
            if wide.len() >= ANSI_WIDE_CHUNK {
                if page.wide_is_lossy(&wide)? {
                    return Ok(true);
                }
                wide.clear();
            }
        }
    }
    if !wide.is_empty() && page.wide_is_lossy(&wide)? {
        return Ok(true);
    }
    Ok(false)
}

fn stream_encode<W: Write>(rope: &Rope, format: Format, out: &mut W) -> io::Result<()> {
    stream_encode_with_page(rope, format, AnsiCodePage::current(), out)
}

fn stream_encode_with_page<W: Write>(
    rope: &Rope,
    format: Format,
    page: AnsiCodePage,
    out: &mut W,
) -> io::Result<()> {
    match format.encoding {
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
        Encoding::Ansi if !page.is_utf8() => {
            stream_encode_ansi(rope, format.line_ending, page, out)?;
        }
        encoding => {
            if encoding == Encoding::Utf8Bom {
                out.write_all(UTF8_BOM)?;
            }
            for chunk in rope.chunks() {
                write_with_line_ending(chunk, format.line_ending, |part| {
                    out.write_all(part.as_bytes())
                })?;
            }
        }
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
    page: AnsiCodePage,
    out: &mut W,
) -> io::Result<()> {
    let mut wide = Vec::with_capacity(ANSI_WIDE_CHUNK);
    let mut bytes = Vec::new();
    for chunk in rope.chunks() {
        write_with_line_ending(chunk, line_ending, |part| {
            for ch in part.chars() {
                let mut units = [0u16; 2];
                let encoded = ch.encode_utf16(&mut units);
                if wide.len() + encoded.len() > ANSI_WIDE_CHUNK {
                    page.encode_wide(&wide, &mut bytes)?;
                    out.write_all(&bytes)?;
                    wide.clear();
                }
                wide.extend_from_slice(encoded);
            }
            Ok(())
        })?;
    }
    if !wide.is_empty() {
        page.encode_wide(&wide, &mut bytes)?;
        out.write_all(&bytes)?;
    }
    Ok(())
}

const TEMP_ATTEMPTS: usize = 100;
const REPLACE_RETRIES: usize = 4;
const REPLACE_RETRY_DELAY: Duration = Duration::from_millis(25);
const MAX_SIBLING_STEM_CHARS: usize = 200;
const TEMP_SUFFIX: &str = ".jot-save";
const BACKUP_SUFFIX: &str = ".jot-backup";

fn write_in_place(target: &Path, rope: &Rope, format: Format) -> io::Result<()> {
    let file = fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(target)?;
    let mut writer = BufWriter::with_capacity(IO_CHUNK, file);
    stream_encode(rope, format, &mut writer)?;
    writer.flush()?;
    let file = writer.into_inner().map_err(|e| e.into_error())?;
    file.sync_all()
}

pub(crate) fn write_rope(path: &Path, rope: &Rope, format: Format) -> Result<(), SaveError> {
    let target = resolve(path)?;
    let existing = match fs::metadata(&target) {
        Ok(metadata) if metadata.permissions().readonly() => return Err(SaveError::ReadOnly),
        Ok(_) => true,
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => return Err(error.into()),
    };

    if existing && file_hard_link_count(&target).unwrap_or(1) > 1 {
        return write_in_place(&target, rope, format).map_err(SaveError::from);
    }

    let (temp, file) = match create_temp(&target) {
        Ok(pair) => pair,
        Err(err) if existing && err.kind() == io::ErrorKind::PermissionDenied => {
            return write_in_place(&target, rope, format).map_err(SaveError::from);
        }
        Err(err) => return Err(err.into()),
    };

    let written = {
        let mut writer = BufWriter::with_capacity(IO_CHUNK, file);
        stream_encode(rope, format, &mut writer)
            .and_then(|()| writer.flush())
            .and_then(|()| writer.into_inner().map_err(|e| e.into_error()))
            .and_then(|file| file.sync_all())
    };
    if let Err(error) = written {
        fs::remove_file(&temp).ok();
        return Err(error.into());
    }
    if existing {
        replace(path, &target, &temp, rope, format)
    } else {
        fs::rename(&temp, &target)
            .inspect_err(|_| {
                fs::remove_file(&temp).ok();
            })
            .map_err(SaveError::from)
    }
}

fn resolve(path: &Path) -> io::Result<PathBuf> {
    match fs::canonicalize(path) {
        Ok(resolved) => Ok(resolved),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let parent = path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            let resolved_parent = fs::canonicalize(parent)
                .or_else(|_| std::path::absolute(parent))?;
            Ok(resolved_parent.join(path.file_name().unwrap_or_default()))
        }
        Err(_) => std::path::absolute(path),
    }
}

fn replace(
    path: &Path,
    target: &Path,
    temp: &Path,
    rope: &Rope,
    format: Format,
) -> Result<(), SaveError> {
    let mut last_err = None;
    for attempt in 0..REPLACE_RETRIES {
        let backup = free_sibling(target, BACKUP_SUFFIX)?;
        match replace_file(target, temp, &backup) {
            Ok(()) => {
                fs::remove_file(&backup).ok();
                return Ok(());
            }
            Err(ReplaceFileError::SharingOrAccess(error)) => {
                fs::remove_file(&backup).ok();
                last_err = Some(error);
                if attempt + 1 < REPLACE_RETRIES {
                    thread::sleep(REPLACE_RETRY_DELAY);
                }
            }
            Err(ReplaceFileError::UnableToMoveReplacement(source))
                if fs::rename(&backup, target).is_err() =>
            {
                let shown =
                    |sibling: &Path| path.with_file_name(sibling.file_name().unwrap_or_default());
                return Err(SaveError::Kept {
                    source,
                    backup: shown(&backup),
                    temp: shown(temp),
                });
            }
            Err(
                ReplaceFileError::UnableToMoveReplacement(error) | ReplaceFileError::Other(error),
            ) => {
                fs::remove_file(temp).ok();
                return Err(error.into());
            }
        }
    }
    fs::remove_file(temp).ok();
    write_in_place(target, rope, format)
        .map_err(|_| SaveError::Io(last_err.unwrap_or_else(|| io::Error::other("replace failed"))))
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
            .attributes(HIDDEN_FILE_ATTRIBUTE)
            .open(&temp)
        {
            Ok(file) => return Ok((temp, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::from(io::ErrorKind::AlreadyExists))
}

fn sibling(path: &Path, attempt: usize, suffix: &str) -> PathBuf {
    let base = path
        .file_name()
        .map(|n| n.to_string_lossy())
        .unwrap_or_default();
    let truncated: String = base.chars().take(MAX_SIBLING_STEM_CHARS).collect();
    let mut name = OsString::from(".");
    name.push(truncated);
    if attempt > 0 {
        name.push(format!(".{attempt}"));
    }
    name.push(suffix);
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load_bytes(bytes: &[u8], chunk_size: usize) -> Result<Decoded, LoadError> {
        let mut cursor = io::Cursor::new(bytes);
        load_reader_with_page(&mut cursor, chunk_size, AnsiCodePage::current())
    }

    fn load_bytes_with_page(
        bytes: &[u8],
        chunk_size: usize,
        page: AnsiCodePage,
    ) -> Result<Decoded, LoadError> {
        let mut cursor = io::Cursor::new(bytes);
        load_reader_with_page(&mut cursor, chunk_size, page)
    }

    struct CleanupDir(PathBuf);
    impl Drop for CleanupDir {
        fn drop(&mut self) {
            if let Ok(entries) = fs::read_dir(&self.0) {
                for entry in entries.flatten() {
                    if let Ok(meta) = entry.metadata() {
                        let mut perms = meta.permissions();
                        if perms.readonly() {
                            #[allow(clippy::permissions_set_readonly_false)]
                            perms.set_readonly(false);
                            let _ = fs::set_permissions(entry.path(), perms);
                        }
                    }
                }
            }
            let _ = fs::remove_dir_all(&self.0);
        }
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
        assert!(matches!(
            load_bytes(b"\xff\xfe\x00\x00a\0\0\0", 4),
            Err(LoadError::Undecodable)
        ));
        let bomless_le = load_bytes(b"a\0b\0c\0", 4).unwrap();
        assert_eq!(bomless_le.format.encoding, Encoding::Utf16Le);
        assert_eq!(bomless_le.text.to_string(), "abc");
        let bomless_be = load_bytes(b"\0a\0b\0c", 4).unwrap();
        assert_eq!(bomless_be.format.encoding, Encoding::Utf16Be);
        assert_eq!(bomless_be.text.to_string(), "abc");

        let mut ansi = vec![b'a'; 20];
        ansi.extend_from_slice(&[0x80, 0xFF]);
        let loaded = load_bytes_with_page(&ansi, 5, AnsiCodePage::from_raw(1252)).unwrap();
        assert_eq!(loaded.format.encoding, Encoding::Ansi);
    }

    #[test]
    fn injected_ansi_code_pages_round_trip_and_flag_lossy() {
        let cp1252 = AnsiCodePage::from_raw(1252);
        let all_bytes: Vec<u8> = (0u8..=255u8)
            .filter(|&b| b != b'\r' && b != b'\n')
            .collect();
        let loaded = load_bytes_with_page(&all_bytes, 16, cp1252).unwrap();
        assert_eq!(loaded.format.encoding, Encoding::Ansi);
        assert!(!loaded.format.lossy_decode);
        assert!(!is_lossy_with_page(&loaded.text, loaded.format, cp1252).unwrap());
        let mut reencoded = Vec::new();
        stream_encode_with_page(&loaded.text, loaded.format, cp1252, &mut reencoded).unwrap();
        assert_eq!(reencoded, all_bytes);

        let ansi_fmt = Format {
            encoding: Encoding::Ansi,
            ..Default::default()
        };
        assert!(!is_lossy_with_page(&Rope::from("café"), ansi_fmt, cp1252).unwrap());
        assert!(is_lossy_with_page(&Rope::from("hello 😀"), ansi_fmt, cp1252).unwrap());

        let cp65001 = AnsiCodePage::from_raw(65001);
        let latin1 = b"caf\xe9";
        let loaded_utf8_acp = load_bytes_with_page(latin1, 8, cp65001).unwrap();
        assert!(loaded_utf8_acp.format.lossy_decode);
        assert!(
            is_lossy_with_page(&loaded_utf8_acp.text, loaded_utf8_acp.format, cp65001).unwrap()
        );

        let cp932 = AnsiCodePage::from_raw(932);
        let invalid_sjis = [0x81u8, 0x00u8, 0xFFu8];
        assert!(matches!(
            load_bytes_with_page(&invalid_sjis, 8, cp932),
            Err(LoadError::Undecodable)
        ));
    }

    #[test]
    fn splits_and_normalizes_line_endings_across_chunk_boundaries() {
        for chunk in [4, 5, 7, 64] {
            let plain = load_bytes(b"plain", chunk).unwrap();
            assert_eq!(plain.text.to_string(), "plain");
            assert_eq!(plain.format.line_ending, LineEnding::Crlf);
            assert!(!plain.format.mixed_line_endings);

            let crlf = load_bytes(b"a\r\nb\r\n", chunk).unwrap();
            assert_eq!(crlf.text.to_string(), "a\nb\n");
            assert_eq!(crlf.format.line_ending, LineEnding::Crlf);
            assert!(!crlf.format.mixed_line_endings);

            let lf = load_bytes(b"a\nb\n", chunk).unwrap();
            assert_eq!(lf.text.to_string(), "a\nb\n");
            assert_eq!(lf.format.line_ending, LineEnding::Lf);
            assert!(!lf.format.mixed_line_endings);

            let cr = load_bytes(b"a\rb\r", chunk).unwrap();
            assert_eq!(cr.text.to_string(), "a\nb\n");
            assert_eq!(cr.format.line_ending, LineEnding::Cr);
            assert!(!cr.format.mixed_line_endings);

            let mixed = load_bytes(b"a\r\r\nb\r", chunk).unwrap();
            assert_eq!(mixed.text.to_string(), "a\n\nb\n");
            assert_eq!(mixed.format.line_ending, LineEnding::Cr);
            assert!(mixed.format.mixed_line_endings);
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
                    ..Default::default()
                };
                assert!(!is_lossy(&rope, format).unwrap());
                let mut bytes = Vec::new();
                stream_encode(&rope, format, &mut bytes).unwrap();
                for chunk in [4, 5, 7, 64] {
                    let loaded = load_bytes(&bytes, chunk).unwrap();
                    assert_eq!(loaded.text.to_string(), sample);
                    assert_eq!(loaded.format, format);
                    let mut reencoded = Vec::new();
                    stream_encode(&loaded.text, loaded.format, &mut reencoded).unwrap();
                    assert_eq!(reencoded, bytes, "byte-exact round-trip mismatch");
                }
            }
        }
        assert!(load_bytes(&[0xff, 0xfe, b'a'], 4).is_err());
        assert!(load_bytes(&[0xef, 0xbb, 0xbf, 0xff], 4).is_err());
    }

    #[test]
    fn same_path_ignores_windows_casing_and_separators() {
        assert_eq!(
            PathKey::new(Path::new(r"C:\Notes\Todo.txt")),
            PathKey::new(Path::new(r"c:/notes/todo.txt")),
        );
        assert_eq!(
            normalize_path_key(Path::new(r"\\?\C:\Notes\Todo.txt")),
            normalize_path_key(Path::new(r"c:\notes\todo.txt")),
        );
        assert_ne!(
            PathKey::new(Path::new(r"C:\Notes\Todo.txt")),
            PathKey::new(Path::new(r"C:\Notes\Other.txt")),
        );
    }

    #[test]
    fn load_and_atomic_write_round_trip_and_respect_readonly() {
        let dir = std::env::temp_dir().join(format!("jot-doc-test-{}", std::process::id()));
        let _cleanup = CleanupDir(dir.clone());
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("sample.txt");

        let format = Format {
            line_ending: LineEnding::Lf,
            encoding: Encoding::Utf8Bom,
            ..Default::default()
        };
        write_rope(&file, &Rope::from("first\nline"), format).unwrap();
        let loaded = load(&DocPath::new(file.clone())).unwrap();
        assert_eq!(loaded.text.to_string(), "first\nline");
        assert_eq!(loaded.format.encoding, Encoding::Utf8Bom);
        assert_eq!(loaded.format.line_ending, LineEnding::Lf);

        write_rope(&file, &Rope::from("second\nline"), format).unwrap();
        let loaded = load(&DocPath::new(file.clone())).unwrap();
        assert_eq!(loaded.text.to_string(), "second\nline");

        let entries: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(entries.len(), 1);

        let long_name = format!("{}.txt", "a".repeat(245));
        let long_sibling = sibling(&dir.join(&long_name), 3, BACKUP_SUFFIX);
        let sib_len = long_sibling.file_name().unwrap().to_string_lossy().len();
        assert!(sib_len < 240, "sibling filename too long: {sib_len}");

        {
            use std::os::windows::fs::OpenOptionsExt as _;
            let _holder = fs::OpenOptions::new()
                .read(true)
                .share_mode(1 | 2)
                .open(&file)
                .unwrap();
            write_rope(&file, &Rope::from("in-place fallback"), format).unwrap();
        }
        assert_eq!(
            load(&DocPath::new(file.clone())).unwrap().text.to_string(),
            "in-place fallback"
        );

        let original = fs::metadata(&file).unwrap().permissions();
        let mut readonly = original.clone();
        readonly.set_readonly(true);
        fs::set_permissions(&file, readonly).unwrap();
        let err = write_rope(&file, &Rope::from("blocked"), format).unwrap_err();
        assert!(matches!(err, SaveError::ReadOnly));
    }

    #[test]
    fn multi_chunk_streaming_round_trip() {
        let dir = std::env::temp_dir().join(format!("jot-doc-large-{}", std::process::id()));
        let _cleanup = CleanupDir(dir.clone());
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("large.txt");

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
                ..Default::default()
            },
            Format {
                line_ending: LineEnding::Lf,
                encoding: Encoding::Utf8Bom,
                ..Default::default()
            },
            Format {
                line_ending: LineEnding::Crlf,
                encoding: Encoding::Utf16Le,
                ..Default::default()
            },
            Format {
                line_ending: LineEnding::Cr,
                encoding: Encoding::Utf16Be,
                ..Default::default()
            },
        ] {
            write_rope(&file, &original, format).unwrap();
            let loaded = load(&DocPath::new(file.clone())).unwrap();
            assert_eq!(loaded.format, format);
            assert_eq!(loaded.text, original);
        }
    }

    #[test]
    fn byte_exact_document_round_trips_and_locked_target_fallback() {
        let dir = std::env::temp_dir().join(format!("jot-roundtrip-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);

        let cases: &[(&str, Vec<u8>)] = &[
            ("empty.txt", b"".to_vec()),
            ("bom_only.txt", b"\xEF\xBB\xBF".to_vec()),
            ("utf8_crlf.txt", b"one\r\ntwo\r\n".to_vec()),
            ("utf8_lf_notrail.txt", b"one\ntwo".to_vec()),
            ("utf8_cr.txt", b"one\rtwo\r".to_vec()),
            (
                "utf8_bom.txt",
                b"\xEF\xBB\xBFhello \xF0\x9F\x8E\x89\r\n".to_vec(),
            ),
            ("utf16le.txt", {
                let mut v = vec![0xFF, 0xFE];
                for u in "hi 日本語\r\n".encode_utf16() {
                    v.extend_from_slice(&u.to_le_bytes());
                }
                v
            }),
            ("utf16be.txt", {
                let mut v = vec![0xFE, 0xFF];
                for u in "hi 日本語\n".encode_utf16() {
                    v.extend_from_slice(&u.to_be_bytes());
                }
                v
            }),
        ];

        for (name, bytes) in cases {
            let p = dir.join(name);
            std::fs::write(&p, bytes).unwrap();
            let loaded = load(&DocPath::new(p.clone())).unwrap();
            assert!(
                !loaded.format.mixed_line_endings,
                "{name} should not be mixed"
            );
            write_rope(&p, &loaded.text, loaded.format).unwrap();
            assert_eq!(
                std::fs::read(&p).unwrap(),
                *bytes,
                "round-trip mismatch for {name}"
            );
        }

        let mixed_path = dir.join("mixed.txt");
        std::fs::write(&mixed_path, b"a\r\nb\nc\r\n").unwrap();
        let mixed = load(&DocPath::new(mixed_path.clone())).unwrap();
        assert!(mixed.format.mixed_line_endings);

        use std::os::windows::fs::OpenOptionsExt;
        let locked_path = dir.join("locked.txt");
        std::fs::write(&locked_path, b"before").unwrap();
        let _holder = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0x0000_0001 | 0x0000_0002)
            .open(&locked_path)
            .unwrap();
        let rope = crate::text::Rope::from("after locked write");
        write_rope(&locked_path, &rope, Format::default()).unwrap();
        drop(_holder);
        assert_eq!(std::fs::read(&locked_path).unwrap(), b"after locked write");
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains(".jot-"))
            .collect();
        assert!(leftovers.is_empty(), "leftover temp files: {leftovers:?}");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
