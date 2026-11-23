//! Streaming file I/O, encoding/BOM/line-ending detection, lossy-ANSI checks, and atomic saves.

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
const IO_CHUNK: usize = 64 * 1024;
const ANSI_WIDE_CHUNK: usize = 32 * 1024;
const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;
const TEMP_ATTEMPTS: usize = 100;
const REPLACE_RETRIES: usize = 4;
const REPLACE_RETRY_DELAY: Duration = Duration::from_millis(25);
const MAX_SIBLING_STEM_CHARS: usize = 200;
const TEMP_SUFFIX: &str = ".jot-save";
const BACKUP_SUFFIX: &str = ".jot-backup";
pub(crate) const READ_ONLY: &str = "This file is read-only.";
const NOTICES: &str = include_str!(concat!(env!("OUT_DIR"), "/notices.txt"));
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
    mixed: bool,
    pending_cr: bool,
    scratch: String,
}

impl LineNormalizer {
    fn new() -> Self {
        Self {
            builder: RopeBuilder::new(),
            line_ending: None,
            mixed: false,
            pending_cr: false,
            scratch: String::new(),
        }
    }

    #[inline]
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

pub(crate) struct Loaded {
    pub(crate) text: Rope,
    pub(crate) format: Format,
    pub(crate) path_key: String,
}

#[derive(Debug)]
pub(crate) enum LoadError {
    Io(io::Error),
    Undecodable,
    TooLarge(u64),
}

pub(crate) fn load(path: &Path) -> Result<Loaded, LoadError> {
    let mut file = fs::File::open(path).map_err(LoadError::Io)?;
    if let Ok(meta) = file.metadata()
        && meta.len() > MAX_FILE_BYTES
    {
        return Err(LoadError::TooLarge(meta.len()));
    }
    let mut loaded = load_reader_with_page(&mut file, IO_CHUNK, AnsiCodePage::current())?;
    loaded.path_key = canonical_path_key(path);
    Ok(loaded)
}

#[derive(Copy, Clone)]
enum BomKind {
    Utf8,
    Utf16Le,
    Utf16Be,
}

fn sniff_bomless_utf16(sample: &[u8]) -> Option<Encoding> {
    if sample.len() < 4 || !sample.len().is_multiple_of(2) {
        return None;
    }
    let mut even_nul = 0usize;
    let mut odd_nul = 0usize;
    let pairs = sample.len() / 2;
    for pair in sample.as_chunks::<2>().0 {
        if pair[0] == 0 {
            even_nul += 1;
        }
        if pair[1] == 0 {
            odd_nul += 1;
        }
    }
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
) -> Result<Loaded, LoadError> {
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
    // Reject UTF-32 LE/BE BOMs instead of mis-detecting FF FE 00 00 as UTF-16 LE (DOC-5).
    if head_slice.starts_with(UTF32_LE_BOM) || head_slice.starts_with(UTF32_BE_BOM) {
        return Err(LoadError::Undecodable);
    }

    let (bom, initial) = if head_slice.starts_with(UTF8_BOM) {
        (Some(BomKind::Utf8), &head_slice[UTF8_BOM.len()..])
    } else if head_slice.starts_with(UTF16_LE_BOM) {
        (Some(BomKind::Utf16Le), &head_slice[UTF16_LE_BOM.len()..])
    } else if head_slice.starts_with(UTF16_BE_BOM) {
        (Some(BomKind::Utf16Be), &head_slice[UTF16_BE_BOM.len()..])
    } else {
        (None, head_slice)
    };

    match bom {
        Some(BomKind::Utf16Le) => {
            stream_utf16(reader, initial, chunk_size, u16::from_le_bytes, Encoding::Utf16Le)
        }
        Some(BomKind::Utf16Be) => {
            stream_utf16(reader, initial, chunk_size, u16::from_be_bytes, Encoding::Utf16Be)
        }
        Some(BomKind::Utf8) => stream_utf8(reader, initial, chunk_size, true),
        None => {
            // Sniff up to 64 bytes for BOM-less UTF-16 (DOC-5) before falling through to UTF-8/ANSI.
            let mut probe = Vec::with_capacity(64);
            probe.extend_from_slice(initial);
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
                if let Ok(loaded) = stream_utf16(reader, &[], chunk_size, unit, enc) {
                    return Ok(loaded);
                }
                reader.seek(SeekFrom::Start(0)).map_err(LoadError::Io)?;
                probe.clear();
            }

            match stream_utf8(reader, &probe, chunk_size, false) {
                Ok(loaded) => Ok(loaded),
                Err(LoadError::Undecodable) => {
                    reader.seek(SeekFrom::Start(0)).map_err(LoadError::Io)?;
                    if page.is_utf8() {
                        // System ACP is UTF-8 (65001) and strict UTF-8 failed: fall back to strict
                        // CP 1252 and flag `lossy_decode` so saving warns instead of silently
                        // rewriting non-UTF-8 bytes as UTF-8 (DOC-1).
                        let mut loaded = stream_ansi(reader, chunk_size, AnsiCodePage::CP_1252)?;
                        loaded.format.lossy_decode = true;
                        Ok(loaded)
                    } else {
                        stream_ansi(reader, chunk_size, page)
                    }
                }
                Err(error) => Err(error),
            }
        }
    }
}

fn stream_ansi<R: Read>(
    reader: &mut R,
    chunk_size: usize,
    page: AnsiCodePage,
) -> Result<Loaded, LoadError> {
    let mut normalizer = LineNormalizer::new();
    let mut buf = vec![0u8; chunk_size + 4];
    let mut wide = Vec::new();
    let mut decoded = String::new();
    let mut carry = 0usize;

    loop {
        let read = reader.read(&mut buf[carry..]).map_err(LoadError::Io)?;
        let filled = carry + read;
        let eof = read == 0;
        if filled == 0 {
            break;
        }
        // On DBCS code pages, avoid splitting a trailing lead byte across chunks.
        let mut decode_end = filled;
        if !eof && decode_end > 0 {
            let mut trailing_leads = 0usize;
            let mut idx = decode_end;
            while idx > 0 && page.is_dbcs_lead_byte(buf[idx - 1]) {
                trailing_leads += 1;
                idx -= 1;
            }
            if trailing_leads % 2 == 1 {
                decode_end -= 1;
            }
        }
        if decode_end > 0 {
            let valid = page
                .decode_chunk(&buf[..decode_end], &mut wide, &mut decoded)
                .map_err(LoadError::Io)?;
            if !valid {
                return Err(LoadError::Undecodable);
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

    let (text, line_ending, mixed_line_endings) = normalizer.finish();
    Ok(Loaded {
        text,
        format: Format {
            line_ending,
            encoding: Encoding::Ansi,
            mixed_line_endings,
            lossy_decode: false,
        },
        path_key: String::new(),
    })
}

fn stream_utf8<R: Read>(
    reader: &mut R,
    initial: &[u8],
    chunk_size: usize,
    has_bom: bool,
) -> Result<Loaded, LoadError> {
    let mut normalizer = LineNormalizer::new();
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

    let (text, line_ending, mixed_line_endings) = normalizer.finish();
    Ok(Loaded {
        text,
        format: Format {
            line_ending,
            encoding: if has_bom {
                Encoding::Utf8Bom
            } else {
                Encoding::Utf8
            },
            mixed_line_endings,
            lossy_decode: false,
        },
        path_key: String::new(),
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
    let mut buf = vec![0u8; chunk_size.max(initial.len()) + 4];
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

    let (text, line_ending, mixed_line_endings) = normalizer.finish();
    Ok(Loaded {
        text,
        format: Format {
            line_ending,
            encoding,
            mixed_line_endings,
            lossy_decode: false,
        },
        path_key: String::new(),
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

pub(crate) fn is_lossy_with_page(
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
        Encoding::Ansi => stream_encode_ansi(rope, format.line_ending, page, out)?,
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
    if page.is_utf8() {
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

    // Preserve hard links by writing in place instead of breaking the link with ReplaceFileW (DOC-4).
    if existing && file_hard_link_count(&target).unwrap_or(1) > 1 {
        return write_in_place(&target, rope, format);
    }

    let (temp, file) = match create_temp(&target) {
        Ok(pair) => pair,
        Err(err) if existing && err.kind() == io::ErrorKind::PermissionDenied => {
            // Writable existing file in a read-only directory: fall back to in-place write (DOC-4).
            return write_in_place(&target, rope, format);
        }
        Err(err) => return Err(err),
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
        return Err(error);
    }
    if existing {
        replace(path, &target, &temp, rope, format)
    } else {
        fs::rename(&temp, &target).inspect_err(|_| {
            fs::remove_file(&temp).ok();
        })
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
        // Fallback for RAM disks / ImDisk / VeraCrypt / WinFsp where GetFinalPathNameByHandle fails (DOC-4).
        Err(_) => std::path::absolute(path),
    }
}

fn replace(
    path: &Path,
    target: &Path,
    temp: &Path,
    rope: &Rope,
    format: Format,
) -> io::Result<()> {
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
            Err(ReplaceFileError::UnableToMoveReplacement(error))
                if fs::rename(&backup, target).is_err() =>
            {
                let shown =
                    |sibling: &Path| path.with_file_name(sibling.file_name().unwrap_or_default());
                return Err(io::Error::new(
                    error.kind(),
                    format!(
                        "{error}\nThe original file was kept as {}\nThe new text was saved as {}",
                        shown(&backup).display(),
                        shown(temp).display()
                    ),
                ));
            }
            Err(
                ReplaceFileError::UnableToMoveReplacement(error) | ReplaceFileError::Other(error),
            ) => {
                fs::remove_file(temp).ok();
                return Err(error);
            }
        }
    }
    // Sharing/access persisted (e.g. target held open without FILE_SHARE_DELETE):
    // clean up temp and fall back to an in-place write (DOC-4).
    fs::remove_file(temp).ok();
    write_in_place(target, rope, format).map_err(|_| last_err.unwrap_or_else(|| io::Error::other("replace failed")))
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

pub(crate) fn write_notices() -> io::Result<PathBuf> {
    let dir = std::env::temp_dir().join(crate::APP_NAME);
    fs::create_dir_all(&dir)?;
    let path = dir.join(NOTICES_FILE);
    fs::write(&path, NOTICES)?;
    Ok(path)
}

/// Computes a normalized comparison key without hitting the filesystem (safe on the UI thread).
pub(crate) fn quick_path_key(path: &Path) -> String {
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    path_key(&abs)
}

/// Canonicalizes `path` once (intended for background threads) with fallback to `quick_path_key`.
pub(crate) fn canonical_path_key(path: &Path) -> String {
    match fs::canonicalize(path) {
        Ok(c) => path_key(&c),
        Err(_) => quick_path_key(path),
    }
}

#[cfg(test)]
pub(crate) fn same_path(a: &Path, b: &Path) -> bool {
    canonical_path_key(a) == canonical_path_key(b)
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
        .or_else(documents_dir)
        .unwrap_or_else(|| PathBuf::from("."))
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

    fn load_bytes(bytes: &[u8], chunk_size: usize) -> Result<Loaded, LoadError> {
        let mut cursor = io::Cursor::new(bytes);
        load_reader_with_page(&mut cursor, chunk_size, AnsiCodePage::current())
    }

    fn load_bytes_with_page(
        bytes: &[u8],
        chunk_size: usize,
        page: AnsiCodePage,
    ) -> Result<Loaded, LoadError> {
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
        // UTF-32 LE BOM must be rejected rather than mis-detected as UTF-16 LE (DOC-5).
        assert!(matches!(
            load_bytes(b"\xff\xfe\x00\x00a\0\0\0", 4),
            Err(LoadError::Undecodable)
        ));
        // BOM-less UTF-16 LE/BE detection (DOC-5).
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
        // 1. CP 1252: all 256 byte values decode cleanly and round-trip losslessly (DOC-1).
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

        // 2. is_lossy distinguishes representable "é" vs non-representable "😀" on CP 1252.
        let ansi_fmt = Format {
            encoding: Encoding::Ansi,
            ..Default::default()
        };
        assert!(!is_lossy_with_page(&Rope::from("café"), ansi_fmt, cp1252).unwrap());
        assert!(is_lossy_with_page(&Rope::from("hello 😀"), ansi_fmt, cp1252).unwrap());

        // 3. Under UTF-8 ACP (65001), invalid UTF-8 bytes like Latin-1 `caf\xe9` are flagged
        //    `lossy_decode = true` so saving prompts instead of silently rewriting (DOC-1).
        let cp65001 = AnsiCodePage::from_raw(65001);
        let latin1 = b"caf\xe9";
        let loaded_utf8_acp = load_bytes_with_page(latin1, 8, cp65001).unwrap();
        assert!(loaded_utf8_acp.format.lossy_decode);
        assert!(
            is_lossy_with_page(&loaded_utf8_acp.text, loaded_utf8_acp.format, cp65001).unwrap()
        );

        // 4. Under Shift-JIS (932), invalid DBCS lead byte fails strict decode (DOC-1).
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
        let _cleanup = CleanupDir(dir.clone());
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("sample.txt");

        let format = Format {
            line_ending: LineEnding::Lf,
            encoding: Encoding::Utf8Bom,
            ..Default::default()
        };
        write_rope(&file, &Rope::from("first\nline"), format).unwrap();
        let loaded = load(&file).unwrap();
        assert_eq!(loaded.text.to_string(), "first\nline");
        assert_eq!(loaded.format.encoding, Encoding::Utf8Bom);
        assert_eq!(loaded.format.line_ending, LineEnding::Lf);

        write_rope(&file, &Rope::from("second\nline"), format).unwrap();
        let loaded = load(&file).unwrap();
        assert_eq!(loaded.text.to_string(), "second\nline");

        // Ensure no leftover .jot-save / .jot-backup siblings remain (DOC-4).
        let entries: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(entries.len(), 1);

        // Long filename (>244 chars component) truncates sibling temp/backup name cleanly (DOC-4).
        let long_name = format!("{}.txt", "a".repeat(245));
        let long_sibling = sibling(&dir.join(&long_name), 3, BACKUP_SUFFIX);
        let sib_len = long_sibling.file_name().unwrap().to_string_lossy().len();
        assert!(sib_len < 240, "sibling filename too long: {sib_len}");

        // Target held open without FILE_SHARE_DELETE falls back to in-place write (DOC-4).
        {
            use std::os::windows::fs::OpenOptionsExt as _;
            let _holder = fs::OpenOptions::new()
                .read(true)
                .share_mode(1 | 2) // FILE_SHARE_READ | FILE_SHARE_WRITE (no FILE_SHARE_DELETE)
                .open(&file)
                .unwrap();
            write_rope(&file, &Rope::from("in-place fallback"), format).unwrap();
        }
        assert_eq!(load(&file).unwrap().text.to_string(), "in-place fallback");

        let original = fs::metadata(&file).unwrap().permissions();
        let mut readonly = original.clone();
        readonly.set_readonly(true);
        fs::set_permissions(&file, readonly).unwrap();
        let err = write_rope(&file, &Rope::from("blocked"), format).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
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
            let loaded = load(&file).unwrap();
            assert_eq!(loaded.format, format);
            assert_eq!(loaded.text, original);
        }
    }
}
