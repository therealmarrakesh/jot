use std::io::{Read, Seek, SeekFrom};

use ropey::{Rope, RopeBuilder};

use crate::platform::AnsiCodePage;

use super::{Encoding, Format, LineEnding, LoadError, UTF8_BOM, UTF16_BE_BOM, UTF16_LE_BOM};

const UTF32_LE_BOM: &[u8] = b"\xff\xfe\x00\x00";
const UTF32_BE_BOM: &[u8] = b"\x00\x00\xfe\xff";

pub(super) struct Decoded {
    pub(super) text: Rope,
    pub(super) format: Format,
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

pub(super) fn load_reader_with_page<R: Read + Seek>(
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
                if let Ok(decoded) = stream_utf16(reader, &[], chunk_size, unit, enc) {
                    return Ok(decoded);
                }
                reader.seek(SeekFrom::Start(0)).map_err(LoadError::Io)?;
                probe.clear();
            }

            match stream_utf8(reader, &probe, chunk_size, false) {
                Ok(decoded) => Ok(decoded),
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
                Err(error) => Err(error),
            }
        }
    }
}

fn stream_decode<R: Read>(
    reader: &mut R,
    initial: &[u8],
    chunk_size: usize,
    encoding: Encoding,
    mut step: impl FnMut(&[u8], bool, &mut LineNormalizer) -> Result<usize, LoadError>,
) -> Result<Decoded, LoadError> {
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
    let encoding = if has_bom {
        Encoding::Utf8Bom
    } else {
        Encoding::Utf8
    };
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
