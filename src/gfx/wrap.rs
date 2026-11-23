//! Cached character widths (ASCII + on-demand non-ASCII + 8-column tab stops) and UAX #14
//! DirectWrite line-break analysis.

use std::cell::RefCell;
use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;

use windows::Win32::Graphics::DirectWrite::{
    DWRITE_BREAK_CONDITION_CAN_BREAK, DWRITE_BREAK_CONDITION_MAY_NOT_BREAK,
    DWRITE_BREAK_CONDITION_MUST_BREAK, DWRITE_FACTORY_TYPE_SHARED, DWRITE_LINE_BREAKPOINT,
    DWRITE_READING_DIRECTION, DWRITE_READING_DIRECTION_LEFT_TO_RIGHT, DWRITE_SCRIPT_ANALYSIS,
    DWriteCreateFactory, IDWriteFactory5, IDWriteNumberSubstitution, IDWriteTextAnalysisSink,
    IDWriteTextAnalysisSink_Impl, IDWriteTextAnalysisSource, IDWriteTextAnalysisSource_Impl,
    IDWriteTextAnalyzer,
};
use windows::core::{HSTRING, implement};

use super::text::{DirectWriteCore, Font, TAB_STOP_COLUMNS};

pub(crate) const MAX_WRAP_INDENT: u32 = 32;

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Boundary {
    pub(crate) ix: usize,
    pub(crate) next_indent: u32,
}

/// Cached character width table (printable ASCII + on-demand non-ASCII + 8-column tab stops)
/// for fast O(line_len) soft-wrapping.
pub(crate) struct CharWidths {
    font: Font,
    font_size: f32,
    ascii: [f32; 95], // 0x20..=0x7E
    max_ascii: f32,
    non_ascii: HashMap<char, f32>,
}

impl CharWidths {
    pub(super) fn new(font: Font, font_size: f32, dwrite: &DirectWriteCore) -> Self {
        let mut ascii = [0.0f32; 95];
        let mut max_ascii = 0.0f32;
        let format = dwrite.text_format(&font, font_size);
        let mut buf = [0u16; 1];
        for (i, byte) in (0x20u8..=0x7Eu8).enumerate() {
            buf[0] = u16::from(byte);
            let w = dwrite.measure_utf16(&buf, &format);
            ascii[i] = w;
            if w > max_ascii {
                max_ascii = w;
            }
        }
        Self {
            font,
            font_size,
            ascii,
            max_ascii,
            non_ascii: HashMap::new(),
        }
    }

    #[inline]
    pub(crate) fn max_printable_ascii(&self) -> f32 {
        self.max_ascii
    }

    #[inline]
    pub(crate) fn tab_stop_width(&self) -> f32 {
        (self.ascii[0] * TAB_STOP_COLUMNS).max(1.0)
    }

    #[inline]
    pub(crate) fn advance_at(&mut self, ch: char, current_x: f32, dwrite: &DirectWriteCore) -> f32 {
        if ch == '\t' {
            let stop = self.tab_stop_width();
            let next = ((current_x / stop).floor() + 1.0) * stop;
            return (next - current_x).max(self.ascii[0].max(1.0));
        }
        self.char_width(ch, dwrite)
    }

    fn char_width(&mut self, ch: char, dwrite: &DirectWriteCore) -> f32 {
        let code = ch as u32;
        if (0x20..=0x7E).contains(&code) {
            return self.ascii[(code - 0x20) as usize];
        }
        if let Some(&w) = self.non_ascii.get(&ch) {
            return w;
        }
        let format = dwrite.text_format(&self.font, self.font_size);
        let mut buf = [0u16; 2];
        let slice = ch.encode_utf16(&mut buf);
        let w = dwrite.measure_utf16(slice, &format);
        self.non_ascii.insert(ch, w);
        w
    }

    /// Wraps `line` into `out` using UAX #14 DirectWrite break opportunities + cached char widths
    /// with 8-column tab-stop alignment (P-1, G-5, G-14).
    pub(crate) fn wrap_line_into(
        &mut self,
        line: &str,
        wrap_width: f32,
        dwrite: &DirectWriteCore,
        out: &mut Vec<Boundary>,
    ) {
        out.clear();
        if line.is_empty() || wrap_width <= 0.0 {
            return;
        }
        let mut indent_chars: u32 = 0;
        let mut indent_width = 0.0f32;
        for ch in line.chars() {
            if ch == ' ' || ch == '\t' {
                let adv = self.advance_at(ch, indent_width, dwrite);
                if indent_chars < MAX_WRAP_INDENT {
                    indent_chars += 1;
                    indent_width += adv;
                }
            } else {
                break;
            }
        }
        if indent_width >= wrap_width * 0.5 {
            indent_chars = 0;
            indent_width = 0.0;
        }

        let break_offsets = line_break_opportunities(line);

        let mut row_start = 0usize;
        let mut row_width = 0.0f32;
        let mut last_break_ix = 0usize;
        let mut break_cursor = 0usize;

        for (ix, ch) in line.char_indices() {
            let char_end = ix + ch.len_utf8();
            while break_cursor < break_offsets.len() && break_offsets[break_cursor] <= ix {
                break_cursor += 1;
            }
            let w = self.advance_at(ch, row_width, dwrite);
            if row_width + w > wrap_width && ix > row_start {
                let split_at = if last_break_ix > row_start {
                    last_break_ix
                } else {
                    ix
                };
                out.push(Boundary {
                    ix: split_at,
                    next_indent: indent_chars,
                });
                row_start = split_at;
                row_width = indent_width;
                for c in line[split_at..char_end].chars() {
                    row_width += self.advance_at(c, row_width, dwrite);
                }
                last_break_ix = split_at;
            } else {
                row_width += w;
            }
            if break_cursor < break_offsets.len() && break_offsets[break_cursor] == char_end {
                last_break_ix = char_end;
                break_cursor += 1;
            }
        }
    }

    /// Computes a horizontal visible byte slice `(start_byte, end_byte, start_x, total_width)`
    /// for a huge unwrapped line using cached character widths and 8-column tab stops (P-2).
    pub(crate) fn horizontal_slice(
        &mut self,
        line: &str,
        min_x: f32,
        max_x: f32,
        dwrite: &DirectWriteCore,
    ) -> (usize, usize, f32, f32) {
        let mut x = 0.0f32;
        let mut start_byte = 0usize;
        let mut start_x = 0.0f32;
        let mut found_start = false;
        let mut end_byte = line.len();
        let mut found_end = false;

        for (ix, ch) in line.char_indices() {
            let adv = self.advance_at(ch, x, dwrite);
            if !found_start && x + adv >= min_x {
                start_byte = ix;
                start_x = x;
                found_start = true;
            }
            x += adv;
            if found_start && !found_end && x >= max_x {
                end_byte = ix + ch.len_utf8();
                found_end = true;
            }
        }
        if !found_start {
            start_byte = line.len();
            start_x = x;
            end_byte = line.len();
        }
        (start_byte, end_byte, start_x, x)
    }
}

#[implement(IDWriteTextAnalysisSource)]
struct BreakSource {
    utf16: Rc<Vec<u16>>,
    locale: HSTRING,
}

impl IDWriteTextAnalysisSource_Impl for BreakSource_Impl {
    fn GetTextAtPosition(
        &self,
        textposition: u32,
        textstring: *mut *mut u16,
        textlength: *mut u32,
    ) -> windows::core::Result<()> {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            let pos = textposition as usize;
            // SAFETY: DirectWrite passes valid output pointers; returned slice borrows `self.utf16`
            // which outlives the synchronous `AnalyzeLineBreakpoints` call.
            unsafe {
                if pos >= self.utf16.len() {
                    *textstring = std::ptr::null_mut();
                    *textlength = 0;
                } else {
                    *textstring = self.utf16.as_ptr().add(pos) as *mut u16;
                    *textlength = (self.utf16.len() - pos) as u32;
                }
            }
        }));
        Ok(())
    }

    fn GetTextBeforePosition(
        &self,
        textposition: u32,
        textstring: *mut *mut u16,
        textlength: *mut u32,
    ) -> windows::core::Result<()> {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            let pos = (textposition as usize).min(self.utf16.len());
            // SAFETY: DirectWrite passes valid output pointers; `self.utf16` outlives the analysis.
            unsafe {
                if pos == 0 {
                    *textstring = std::ptr::null_mut();
                    *textlength = 0;
                } else {
                    *textstring = self.utf16.as_ptr() as *mut u16;
                    *textlength = pos as u32;
                }
            }
        }));
        Ok(())
    }

    fn GetParagraphReadingDirection(&self) -> DWRITE_READING_DIRECTION {
        DWRITE_READING_DIRECTION_LEFT_TO_RIGHT
    }

    fn GetLocaleName(
        &self,
        textposition: u32,
        textlength: *mut u32,
        localename: *mut *mut u16,
    ) -> windows::core::Result<()> {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            let pos = (textposition as usize).min(self.utf16.len());
            // SAFETY: DirectWrite passes valid output pointers; `self.locale` is NUL-terminated.
            unsafe {
                *textlength = (self.utf16.len() - pos) as u32;
                *localename = self.locale.as_ptr() as *mut u16;
            }
        }));
        Ok(())
    }

    fn GetNumberSubstitution(
        &self,
        textposition: u32,
        textlength: *mut u32,
        numbersubstitution: windows::core::OutRef<'_, IDWriteNumberSubstitution>,
    ) -> windows::core::Result<()> {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            let pos = (textposition as usize).min(self.utf16.len());
            // SAFETY: DirectWrite passes valid `textlength` output pointer.
            unsafe {
                *textlength = (self.utf16.len() - pos) as u32;
            }
            let _ = numbersubstitution.write(None);
        }));
        Ok(())
    }
}

#[implement(IDWriteTextAnalysisSink)]
struct BreakSink {
    out: Rc<RefCell<Vec<DWRITE_LINE_BREAKPOINT>>>,
}

impl IDWriteTextAnalysisSink_Impl for BreakSink_Impl {
    fn SetScriptAnalysis(
        &self,
        _textposition: u32,
        _textlength: u32,
        _scriptanalysis: *const DWRITE_SCRIPT_ANALYSIS,
    ) -> windows::core::Result<()> {
        Ok(())
    }

    fn SetLineBreakpoints(
        &self,
        textposition: u32,
        textlength: u32,
        linebreakpoints: *const DWRITE_LINE_BREAKPOINT,
    ) -> windows::core::Result<()> {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            let start = textposition as usize;
            let len = textlength as usize;
            if len == 0 || linebreakpoints.is_null() {
                return;
            }
            let mut dst = self.out.borrow_mut();
            if start.saturating_add(len) <= dst.len() {
                // SAFETY: `len > 0` and `linebreakpoints` is non-null and valid for `len` entries.
                let src = unsafe { std::slice::from_raw_parts(linebreakpoints, len) };
                dst[start..start + len].copy_from_slice(src);
            }
        }));
        Ok(())
    }

    fn SetBidiLevel(
        &self,
        _textposition: u32,
        _textlength: u32,
        _explicitlevel: u8,
        _resolvedlevel: u8,
    ) -> windows::core::Result<()> {
        Ok(())
    }

    fn SetNumberSubstitution(
        &self,
        _textposition: u32,
        _textlength: u32,
        _numbersubstitution: windows::core::Ref<'_, IDWriteNumberSubstitution>,
    ) -> windows::core::Result<()> {
        Ok(())
    }
}

thread_local! {
    static TEXT_ANALYZER: RefCell<Option<IDWriteTextAnalyzer>> = const { RefCell::new(None) };
}

/// Returns UTF-8 byte offsets strictly inside `text` where UAX #14 allows a line break.
pub(crate) fn line_break_opportunities(text: &str) -> Vec<usize> {
    let mut out = Vec::new();
    if text.is_empty() {
        return out;
    }
    let analyzer = TEXT_ANALYZER.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            // SAFETY: Shared DirectWrite factory and text analyzer creation require no preconditions.
            let created = unsafe {
                DWriteCreateFactory::<IDWriteFactory5>(DWRITE_FACTORY_TYPE_SHARED)
                    .and_then(|factory| factory.CreateTextAnalyzer())
            };
            if let Ok(a) = created {
                *slot = Some(a);
            }
        }
        slot.clone()
    });
    let Some(analyzer) = analyzer else {
        return out;
    };

    let utf16: Rc<Vec<u16>> = Rc::new(text.encode_utf16().collect());
    let len = utf16.len();
    let breakpoints = Rc::new(RefCell::new(vec![DWRITE_LINE_BREAKPOINT::default(); len]));
    let source: IDWriteTextAnalysisSource = BreakSource {
        utf16,
        locale: HSTRING::from(""),
    }
    .into();
    let sink: IDWriteTextAnalysisSink = BreakSink {
        out: breakpoints.clone(),
    }
    .into();

    // SAFETY: `source` and `sink` are valid COM implementations covering `[0..len)`.
    if unsafe { analyzer.AnalyzeLineBreakpoints(&source, 0, len as u32, &sink) }.is_err() {
        return out;
    }

    let bp = breakpoints.borrow();
    let can_break = DWRITE_BREAK_CONDITION_CAN_BREAK.0 as u8;
    let may_not_break = DWRITE_BREAK_CONDITION_MAY_NOT_BREAK.0 as u8;
    let must_break = DWRITE_BREAK_CONDITION_MUST_BREAK.0 as u8;
    let mut u16_pos = 0usize;
    for (byte_ix, ch) in text.char_indices() {
        let char_u16 = ch.len_utf16();
        let last_u16 = u16_pos + char_u16 - 1;
        let next_u16 = u16_pos + char_u16;
        let next_byte = byte_ix + ch.len_utf8();
        if next_byte < text.len() && next_u16 < bp.len() {
            let after = (bp[last_u16]._bitfield >> 2) & 0b11;
            let before = bp[next_u16]._bitfield & 0b11;
            let allowed = if after == must_break || before == must_break {
                true
            } else if after == may_not_break || before == may_not_break {
                false
            } else {
                after == can_break || before == can_break
            };
            if allowed {
                out.push(next_byte);
            }
        }
        u16_pos = next_u16;
    }
    out
}
