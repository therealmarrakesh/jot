use std::io::{self, Write};

use ropey::Rope;

use crate::platform::AnsiCodePage;

use super::{ANSI_WIDE_CHUNK, Encoding, Format, LineEnding, UTF8_BOM, UTF16_BE_BOM, UTF16_LE_BOM};

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

pub(super) fn is_lossy_with_page(
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

pub(super) fn stream_encode<W: Write>(rope: &Rope, format: Format, out: &mut W) -> io::Result<()> {
    stream_encode_with_page(rope, format, AnsiCodePage::current(), out)
}

pub(super) fn stream_encode_with_page<W: Write>(
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
