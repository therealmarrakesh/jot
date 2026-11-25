//! Documents on disk: their formats, their paths, reading them in and writing them out.
//!
//! A file is read as UTF-8 (with or without a BOM), UTF-16 (with a BOM, or sniffed without
//! one), or else the ANSI code page, streamed in chunks; its line endings become `\n`, keeping
//! which one it used (and whether it mixed them). Saving writes the text back the same way, to a
//! hidden sibling that then replaces the file, or in place where it must be.

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
const UTF32_LE_BOM: &[u8] = b"\xff\xfe\x00\x00";
const UTF32_BE_BOM: &[u8] = b"\x00\x00\xfe\xff";
/// How much is read or written at a time, and how many UTF-16 units go to the code page at once.
const IO_CHUNK: usize = 64 * 1024;
const ANSI_WIDE_CHUNK: usize = 32 * 1024;
/// The largest file that's opened.
const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;
const UNTITLED: &str = "Untitled";

// ---- 1. Formats -----------------------------------------------------------------------------

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

    /// What `\n` is written as.
    fn chars(self) -> &'static str {
        match self {
            Self::Crlf => "\r\n",
            Self::Lf => "\n",
            Self::Cr => "\r",
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
    /// The order the save dialog lists them in.
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

/// How a document is stored: its line ending (the first one, if it mixed them) and encoding,
/// and whether reading it replaced anything it couldn't decode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Format {
    pub(crate) line_ending: LineEnding,
    pub(crate) encoding: Encoding,
    pub(crate) mixed_line_endings: bool,
    pub(crate) lossy_decode: bool,
}

impl Format {
    pub(crate) fn line_ending_label(self) -> &'static str {
        match (self.mixed_line_endings, self.line_ending) {
            (false, ending) => ending.label(),
            (true, LineEnding::Crlf) => "Mixed (CRLF)",
            (true, LineEnding::Lf) => "Mixed (LF)",
            (true, LineEnding::Cr) => "Mixed (CR)",
        }
    }

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

// ---- 2. Paths -------------------------------------------------------------------------------

/// What tells two paths to the same file apart from others: resolved, without the `\\?\`
/// prefix, with backslashes, in lower case.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct PathKey(String);

impl PathKey {
    pub(crate) fn new(path: &Path) -> Self {
        let resolved = resolve(path).unwrap_or_else(|_| path.to_path_buf());
        Self(normalize_path_key(&resolved))
    }
}

fn normalize_path_key(path: &Path) -> String {
    let raw = path.to_string_lossy();
    let stripped = raw.strip_prefix(r"\\?\").unwrap_or(&raw);
    stripped.replace('/', "\\").to_lowercase()
}

/// A document's file: its absolute path, and its key.
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

pub(crate) fn file_name(path: Option<&Path>) -> Option<String> {
    Some(path?.file_name()?.to_string_lossy().into_owned())
}

/// A tab's name: its file's, or "Untitled".
pub(crate) fn display_name(path: Option<&Path>) -> String {
    file_name(path).unwrap_or_else(|| UNTITLED.into())
}

/// Where the file dialogs start: the document's folder, else Documents, else here.
pub(crate) fn default_directory(path: Option<&Path>) -> PathBuf {
    path.and_then(Path::parent)
        .map(Path::to_path_buf)
        .or_else(documents_dir)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// `path` with links followed; one that doesn't exist yet goes in its folder, resolved.
fn resolve(path: &Path) -> io::Result<PathBuf> {
    match fs::canonicalize(path) {
        Ok(resolved) => Ok(resolved),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let parent = path.parent().filter(|p| !p.as_os_str().is_empty());
            let parent = parent.unwrap_or(Path::new("."));
            let resolved = fs::canonicalize(parent).or_else(|_| std::path::absolute(parent))?;
            Ok(resolved.join(path.file_name().unwrap_or_default()))
        }
        Err(_) => std::path::absolute(path),
    }
}

// ---- 3. Reading -----------------------------------------------------------------------------

/// A document read in.
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

impl From<io::Error> for LoadError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

pub(crate) fn load(doc_path: &DocPath) -> Result<Loaded, LoadError> {
    let mut file = fs::File::open(doc_path.as_path())?;
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

struct Decoded {
    text: Rope,
    format: Format,
}

/// Reads `reader` in chunks of `chunk_size`: by its BOM; as UTF-16 if it looks like UTF-16
/// without one; else as UTF-8; else in `page` (or, where that's UTF-8, in 1252, marked lossy).
fn load_reader_with_page<R: Read + Seek>(
    reader: &mut R,
    chunk_size: usize,
    page: AnsiCodePage,
) -> Result<Decoded, LoadError> {
    let chunk_size = chunk_size.max(4);
    let mut head = [0u8; 4];
    let mut len = 0;
    while len < head.len() {
        match reader.read(&mut head[len..])? {
            0 => break,
            n => len += n,
        }
    }
    let head = &head[..len];
    if head.starts_with(UTF32_LE_BOM) || head.starts_with(UTF32_BE_BOM) {
        return Err(LoadError::Undecodable);
    }
    if let Some(rest) = head.strip_prefix(UTF8_BOM) {
        return stream_utf8(reader, rest, chunk_size, Encoding::Utf8Bom);
    }
    if let Some(rest) = head.strip_prefix(UTF16_LE_BOM) {
        return stream_utf16(reader, rest, chunk_size, Encoding::Utf16Le);
    }
    if let Some(rest) = head.strip_prefix(UTF16_BE_BOM) {
        return stream_utf16(reader, rest, chunk_size, Encoding::Utf16Be);
    }

    // The first 64 bytes decide whether it's UTF-16 without a BOM.
    let mut probe = head.to_vec();
    let mut more = [0u8; 60];
    let n = reader.read(&mut more)?;
    probe.extend_from_slice(&more[..n]);
    if let Some(encoding) = sniff_bomless_utf16(&probe) {
        reader.seek(SeekFrom::Start(0))?;
        if let Ok(decoded) = stream_utf16(reader, &[], chunk_size, encoding) {
            return Ok(decoded);
        }
        reader.seek(SeekFrom::Start(0))?;
        probe.clear();
    }
    match stream_utf8(reader, &probe, chunk_size, Encoding::Utf8) {
        Err(LoadError::Undecodable) => {
            reader.seek(SeekFrom::Start(0))?;
            if !page.is_utf8() {
                return stream_ansi(reader, chunk_size, page);
            }
            // The code page is UTF-8 and the file isn't: it's read as 1252, and saving warns.
            let mut decoded = stream_ansi(reader, chunk_size, AnsiCodePage::CP_1252)?;
            decoded.format.lossy_decode = true;
            Ok(decoded)
        }
        result => result,
    }
}

/// UTF-16 without a BOM: an even number of bytes, three in four units with a zero high byte
/// (and none with a zero low byte), in either order.
fn sniff_bomless_utf16(sample: &[u8]) -> Option<Encoding> {
    if sample.len() < 4 || !sample.len().is_multiple_of(2) {
        return None;
    }
    let (units, _) = sample.as_chunks::<2>();
    let most = units.len() * 3 / 4;
    let first_nul = units.iter().filter(|u| u[0] == 0).count();
    let second_nul = units.iter().filter(|u| u[1] == 0).count();
    if second_nul >= most && first_nul == 0 {
        Some(Encoding::Utf16Le)
    } else if first_nul >= most && second_nul == 0 {
        Some(Encoding::Utf16Be)
    } else {
        None
    }
}

/// Reads `reader` (after `initial`) in chunks, handing each, with whether it's the last, to
/// `decode`, which takes in what it can (holding back a split character) and says how much.
fn stream_decode<R: Read>(
    reader: &mut R,
    initial: &[u8],
    chunk_size: usize,
    encoding: Encoding,
    mut decode: impl FnMut(&[u8], bool, &mut LineNormalizer) -> Result<usize, LoadError>,
) -> Result<Decoded, LoadError> {
    let mut lines = LineNormalizer::default();
    let mut buf = vec![0u8; chunk_size.max(initial.len()) + 4];
    let mut carry = initial.len();
    buf[..carry].copy_from_slice(initial);
    loop {
        let read = reader.read(&mut buf[carry..])?;
        let (filled, eof) = (carry + read, read == 0);
        if filled == 0 {
            break;
        }
        let taken = decode(&buf[..filled], eof, &mut lines)?;
        buf.copy_within(taken..filled, 0);
        carry = filled - taken;
        if eof {
            break;
        }
    }
    let (text, line_ending, mixed_line_endings) = lines.finish();
    let format = Format {
        line_ending,
        encoding,
        mixed_line_endings,
        lossy_decode: false,
    };
    Ok(Decoded { text, format })
}

fn stream_utf8<R: Read>(
    reader: &mut R,
    initial: &[u8],
    chunk_size: usize,
    encoding: Encoding,
) -> Result<Decoded, LoadError> {
    stream_decode(
        reader,
        initial,
        chunk_size,
        encoding,
        |bytes, eof, lines| {
            let error = match std::str::from_utf8(bytes) {
                Ok(text) => {
                    lines.push(text);
                    return Ok(bytes.len());
                }
                Err(error) => error,
            };
            let valid = error.valid_up_to();
            lines.push(std::str::from_utf8(&bytes[..valid]).expect("the valid prefix"));
            // Only a character split at the chunk's end may wait for the next.
            if error.error_len().is_some() || eof {
                return Err(LoadError::Undecodable);
            }
            Ok(valid)
        },
    )
}

fn stream_utf16<R: Read>(
    reader: &mut R,
    initial: &[u8],
    chunk_size: usize,
    encoding: Encoding,
) -> Result<Decoded, LoadError> {
    let unit = if encoding == Encoding::Utf16Le {
        u16::from_le_bytes
    } else {
        u16::from_be_bytes
    };
    let mut text = String::with_capacity(chunk_size);
    stream_decode(
        reader,
        initial,
        chunk_size,
        encoding,
        |bytes, eof, lines| {
            if eof && !bytes.len().is_multiple_of(2) {
                return Err(LoadError::Undecodable);
            }
            // Whole units, holding back a high surrogate that ends the chunk.
            let mut end = bytes.len() & !1;
            if !eof
                && end >= 2
                && (0xD800..=0xDBFF).contains(&unit([bytes[end - 2], bytes[end - 1]]))
            {
                end -= 2;
            }
            if end > 0 {
                text.clear();
                let (units, _) = bytes[..end].as_chunks::<2>();
                for ch in char::decode_utf16(units.iter().map(|&u| unit(u))) {
                    text.push(ch.map_err(|_| LoadError::Undecodable)?);
                }
                lines.push(&text);
            }
            Ok(end)
        },
    )
}

fn stream_ansi<R: Read>(
    reader: &mut R,
    chunk_size: usize,
    page: AnsiCodePage,
) -> Result<Decoded, LoadError> {
    let (mut wide, mut text) = (Vec::new(), String::new());
    stream_decode(
        reader,
        &[],
        chunk_size,
        Encoding::Ansi,
        |bytes, eof, lines| {
            // Holds back a lead byte whose trail is in the next chunk.
            let mut end = bytes.len();
            let leads = bytes
                .iter()
                .rev()
                .take_while(|&&b| page.is_dbcs_lead_byte(b));
            if !eof && leads.count() % 2 == 1 {
                end -= 1;
            }
            if end > 0 {
                if !page.decode_chunk(&bytes[..end], &mut wide, &mut text)? {
                    return Err(LoadError::Undecodable);
                }
                lines.push(&text);
            }
            Ok(end)
        },
    )
}

/// Builds the text with every line ending as `\n`, noting the first ending and whether others
/// differ; a CR that ends one chunk waits to see whether the next starts with LF.
#[derive(Default)]
struct LineNormalizer {
    builder: RopeBuilder,
    line_ending: Option<LineEnding>,
    mixed: bool,
    pending_cr: bool,
    scratch: String,
}

impl LineNormalizer {
    fn record(&mut self, ending: LineEnding) {
        match self.line_ending {
            None => self.line_ending = Some(ending),
            Some(first) => self.mixed |= first != ending,
        }
    }

    fn push(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.scratch.clear();
        let mut rest = text;
        if std::mem::take(&mut self.pending_cr) {
            let crlf = rest.starts_with('\n');
            self.record(if crlf {
                LineEnding::Crlf
            } else {
                LineEnding::Cr
            });
            self.scratch.push('\n');
            rest = &rest[usize::from(crlf)..];
        }
        // Without a CR, the text goes in as it is.
        if self.scratch.is_empty() && !rest.contains('\r') {
            if rest.contains('\n') {
                self.record(LineEnding::Lf);
            }
            self.builder.append(rest);
            return;
        }
        loop {
            let Some(cr) = rest.find('\r') else {
                if rest.contains('\n') {
                    self.record(LineEnding::Lf);
                }
                self.scratch.push_str(rest);
                break;
            };
            let (before, after) = (&rest[..cr], &rest[cr + 1..]);
            if before.contains('\n') {
                self.record(LineEnding::Lf);
            }
            self.scratch.push_str(before);
            if after.is_empty() {
                self.pending_cr = true;
                break;
            }
            let crlf = after.starts_with('\n');
            self.record(if crlf {
                LineEnding::Crlf
            } else {
                LineEnding::Cr
            });
            self.scratch.push('\n');
            rest = &after[usize::from(crlf)..];
        }
        if !self.scratch.is_empty() {
            self.builder.append(&self.scratch);
        }
    }

    /// The text, its line ending (CRLF if it has none) and whether it mixed them.
    fn finish(mut self) -> (Rope, LineEnding, bool) {
        if self.pending_cr {
            self.record(LineEnding::Cr);
            self.builder.append("\n");
        }
        let ending = self.line_ending.unwrap_or(LineEnding::Crlf);
        (self.builder.finish(), ending, self.mixed)
    }
}

// ---- 4. Writing -----------------------------------------------------------------------------

#[derive(Debug)]
pub(crate) enum SaveError {
    ReadOnly,
    /// The file couldn't be put back after a failed replace: the old one is at `backup`, the new
    /// one at `temp`.
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

/// Whether saving `rope` in `format` would lose characters the code page can't hold (or did
/// on reading).
pub(crate) fn is_lossy(rope: &Rope, format: Format) -> io::Result<bool> {
    is_lossy_with_page(rope, format, AnsiCodePage::current())
}

fn is_lossy_with_page(rope: &Rope, format: Format, page: AnsiCodePage) -> io::Result<bool> {
    if format.encoding != Encoding::Ansi {
        return Ok(false);
    }
    if format.lossy_decode || page.is_utf8() {
        return Ok(format.lossy_decode);
    }
    let mut wide = Vec::with_capacity(ANSI_WIDE_CHUNK);
    for unit in rope.chunks().flat_map(str::encode_utf16) {
        wide.push(unit);
        if wide.len() >= ANSI_WIDE_CHUNK {
            if page.wide_is_lossy(&wide)? {
                return Ok(true);
            }
            wide.clear();
        }
    }
    Ok(!wide.is_empty() && page.wide_is_lossy(&wide)?)
}

fn stream_encode<W: Write>(rope: &Rope, format: Format, out: &mut W) -> io::Result<()> {
    stream_encode_with_page(rope, format, AnsiCodePage::current(), out)
}

/// Writes `rope` in `format` to `out`: its BOM, then the text a piece at a time with each `\n`
/// as the line ending, encoded.
fn stream_encode_with_page<W: Write>(
    rope: &Rope,
    format: Format,
    page: AnsiCodePage,
    out: &mut W,
) -> io::Result<()> {
    let ending = format.line_ending.chars();
    let mut pieces = rope.chunks().flat_map(|chunk| {
        let mut lines = chunk.split('\n');
        let first = lines.next();
        first
            .into_iter()
            .chain(lines.flat_map(move |line| [ending, line]))
    });
    let mut each = |write: &mut dyn FnMut(&str) -> io::Result<()>| {
        pieces.try_for_each(|piece| {
            if piece.is_empty() {
                Ok(())
            } else {
                write(piece)
            }
        })
    };
    match format.encoding {
        Encoding::Utf16Le | Encoding::Utf16Be => {
            let le = format.encoding == Encoding::Utf16Le;
            let bytes = if le {
                u16::to_le_bytes
            } else {
                u16::to_be_bytes
            };
            out.write_all(if le { UTF16_LE_BOM } else { UTF16_BE_BOM })?;
            let mut buf = Vec::with_capacity(8 * 1024);
            each(&mut |piece| {
                for unit in piece.encode_utf16() {
                    buf.extend_from_slice(&bytes(unit));
                    if buf.len() >= 8 * 1024 {
                        out.write_all(&buf)?;
                        buf.clear();
                    }
                }
                Ok(())
            })?;
            out.write_all(&buf)
        }
        Encoding::Ansi if !page.is_utf8() => {
            // Whole characters at a time, so a surrogate pair isn't split between calls.
            let (mut wide, mut bytes) = (Vec::with_capacity(ANSI_WIDE_CHUNK), Vec::new());
            each(&mut |piece| {
                for ch in piece.chars() {
                    let mut units = [0u16; 2];
                    let units = ch.encode_utf16(&mut units);
                    if wide.len() + units.len() > ANSI_WIDE_CHUNK {
                        page.encode_wide(&wide, &mut bytes)?;
                        out.write_all(&bytes)?;
                        wide.clear();
                    }
                    wide.extend_from_slice(units);
                }
                Ok(())
            })?;
            if !wide.is_empty() {
                page.encode_wide(&wide, &mut bytes)?;
                out.write_all(&bytes)?;
            }
            Ok(())
        }
        encoding => {
            if encoding == Encoding::Utf8Bom {
                out.write_all(UTF8_BOM)?;
            }
            each(&mut |piece| out.write_all(piece.as_bytes()))
        }
    }
}

/// How many sibling names are tried, how often a busy file's replace is retried, and the
/// siblings' names.
const TEMP_ATTEMPTS: usize = 100;
const REPLACE_RETRIES: usize = 4;
const REPLACE_RETRY_DELAY: Duration = Duration::from_millis(25);
const MAX_SIBLING_STEM_CHARS: usize = 200;
const TEMP_SUFFIX: &str = ".jot-save";
const BACKUP_SUFFIX: &str = ".jot-backup";

/// Saves `rope` in `format` at `path`: written to a hidden sibling that then replaces the file
/// (keeping its attributes), or written straight into a file that's hard-linked or whose folder
/// won't take the sibling.
pub(crate) fn write_rope(path: &Path, rope: &Rope, format: Format) -> Result<(), SaveError> {
    let target = resolve(path)?;
    let existing = match fs::metadata(&target) {
        Ok(meta) if meta.permissions().readonly() => return Err(SaveError::ReadOnly),
        Ok(_) => true,
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => return Err(error.into()),
    };
    if existing && file_hard_link_count(&target).unwrap_or(1) > 1 {
        return Ok(write_in_place(&target, rope, format)?);
    }
    let (temp, file) = match create_temp(&target) {
        Ok(created) => created,
        Err(err) if existing && err.kind() == io::ErrorKind::PermissionDenied => {
            return Ok(write_in_place(&target, rope, format)?);
        }
        Err(err) => return Err(err.into()),
    };
    if let Err(error) = write_file(file, rope, format) {
        fs::remove_file(&temp).ok();
        return Err(error.into());
    }
    if existing {
        return replace(path, &target, &temp, rope, format);
    }
    fs::rename(&temp, &target).map_err(|error| {
        fs::remove_file(&temp).ok();
        error.into()
    })
}

/// Writes the document into `file`, all the way to the disk.
fn write_file(file: fs::File, rope: &Rope, format: Format) -> io::Result<()> {
    let mut writer = BufWriter::with_capacity(IO_CHUNK, file);
    stream_encode(rope, format, &mut writer)?;
    writer.into_inner().map_err(|e| e.into_error())?.sync_all()
}

fn write_in_place(target: &Path, rope: &Rope, format: Format) -> io::Result<()> {
    let file = fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(target)?;
    write_file(file, rope, format)
}

/// Swaps `temp` in for `target`, retrying while it's busy and writing in place once it stays
/// busy. If the swap fails half-way and the file can't be put back, says where both are (as
/// siblings of `path`, the name the user saved to).
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
                let shown = |p: &Path| path.with_file_name(p.file_name().unwrap_or_default());
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

/// The first sibling name with `suffix` that nothing has.
fn free_sibling(path: &Path, suffix: &str) -> io::Result<PathBuf> {
    (0..TEMP_ATTEMPTS)
        .map(|attempt| sibling(path, attempt, suffix))
        .find(|candidate| fs::symlink_metadata(candidate).is_err())
        .ok_or_else(|| io::Error::from(io::ErrorKind::AlreadyExists))
}

/// A new hidden sibling to write the document into.
fn create_temp(path: &Path) -> io::Result<(PathBuf, fs::File)> {
    for attempt in 0..TEMP_ATTEMPTS {
        let temp = sibling(path, attempt, TEMP_SUFFIX);
        let mut options = fs::OpenOptions::new();
        options
            .write(true)
            .create_new(true)
            .attributes(HIDDEN_FILE_ATTRIBUTE);
        match options.open(&temp) {
            Ok(file) => return Ok((temp, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::from(io::ErrorKind::AlreadyExists))
}

/// `.name[.attempt]suffix` beside `path`, the name cut short enough to stay a valid one.
fn sibling(path: &Path, attempt: usize, suffix: &str) -> PathBuf {
    let base = path
        .file_name()
        .map(|n| n.to_string_lossy())
        .unwrap_or_default();
    let mut name = OsString::from(".");
    name.push(
        base.chars()
            .take(MAX_SIBLING_STEM_CHARS)
            .collect::<String>(),
    );
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
