use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::RangeInclusive;
use std::rc::Rc;

use windows::Win32::Graphics::DirectWrite::{
    DWRITE_BREAK_CONDITION_CAN_BREAK, DWRITE_BREAK_CONDITION_MAY_NOT_BREAK,
    DWRITE_BREAK_CONDITION_MUST_BREAK, DWRITE_LINE_BREAKPOINT, DWRITE_LINE_METRICS,
    DWRITE_READING_DIRECTION, DWRITE_READING_DIRECTION_LEFT_TO_RIGHT, DWRITE_SCRIPT_ANALYSIS,
    DWRITE_TEXT_METRICS, DWRITE_WORD_WRAPPING_WRAP, IDWriteFactory5, IDWriteNumberSubstitution,
    IDWriteTextAnalysisSink, IDWriteTextAnalysisSink_Impl, IDWriteTextAnalysisSource,
    IDWriteTextAnalysisSource_Impl, IDWriteTextAnalyzer, IDWriteTextFormat,
};
use windows::core::{HSTRING, implement};

use super::text::{TAB_STOP_COLUMNS, encode_single_line_utf16, measure_utf16};

const CHAR_WIDTH_RANGES: [RangeInclusive<u32>; 17] = [
    0x0020..=0x007e,
    0x00a0..=0x02ff,
    0x0370..=0x0482,
    0x048a..=0x052f,
    0x1e00..=0x1fff,
    0x2010..=0x2027,
    0x2030..=0x205e,
    0x2070..=0x20cf,
    0x2100..=0x2bff,
    0x3000..=0x3029,
    0x3030..=0x3098,
    0x309b..=0x30ff,
    0x3400..=0x4dbf,
    0x4e00..=0x9fff,
    0xac00..=0xd7a3,
    0xf900..=0xfaff,
    0xff01..=0xffee,
];

#[inline]
fn is_printable_ascii_or_tab(bytes: &[u8]) -> bool {
    bytes.iter().all(|&b| b == b'\t' || (0x20..=0x7e).contains(&b))
}

pub(super) fn can_wrap_with_advances(text: &str) -> bool {
    if is_printable_ascii_or_tab(text.as_bytes()) {
        return true;
    }
    text.chars()
        .all(|c| c == '\t' || CHAR_WIDTH_RANGES.iter().any(|r| r.contains(&(c as u32))))
}

pub(super) struct AdvanceTable {
    factory: IDWriteFactory5,
    format: IDWriteTextFormat,
    ascii: [f32; 95],
    max_ascii: f32,
    non_ascii: HashMap<char, f32>,
}

impl AdvanceTable {
    pub(super) fn new(factory: IDWriteFactory5, format: IDWriteTextFormat) -> Self {
        let mut ascii = [0.0f32; 95];
        let mut max_ascii = 0.0f32;
        let mut buf = [0u16; 1];
        for (i, byte) in (0x20u8..=0x7Eu8).enumerate() {
            buf[0] = u16::from(byte);
            let w = measure_utf16(&factory, &buf, &format);
            ascii[i] = w;
            if w > max_ascii {
                max_ascii = w;
            }
        }
        Self {
            factory,
            format,
            ascii,
            max_ascii,
            non_ascii: HashMap::new(),
        }
    }

    #[inline]
    pub(super) fn max_printable_ascii(&self) -> f32 {
        self.max_ascii
    }

    #[inline]
    fn tab_stop_width(&self) -> f32 {
        (self.ascii[0] * TAB_STOP_COLUMNS).max(1.0)
    }

    #[inline]
    pub(super) fn advance(&mut self, ch: char, current_x: f32) -> f32 {
        if ch == '\t' {
            let stop = self.tab_stop_width();
            let next = ((current_x / stop).floor() + 1.0) * stop;
            return (next - current_x).max(self.ascii[0].max(1.0));
        }
        let code = ch as u32;
        if (0x20..=0x7E).contains(&code) {
            return self.ascii[(code - 0x20) as usize];
        }
        if let Some(&w) = self.non_ascii.get(&ch) {
            return w;
        }
        let mut buf = [0u16; 2];
        let slice = ch.encode_utf16(&mut buf);
        let w = measure_utf16(&self.factory, slice, &self.format);
        self.non_ascii.insert(ch, w);
        w
    }

    pub(super) fn horizontal_slice(
        &mut self,
        line: &str,
        min_x: f32,
        max_x: f32,
    ) -> (usize, usize, f32, f32) {
        let mut x = 0.0f32;
        let mut start_byte = 0usize;
        let mut start_x = 0.0f32;
        let mut found_start = false;
        let mut end_byte = line.len();
        let mut found_end = false;

        for (ix, ch) in line.char_indices() {
            let adv = self.advance(ch, x);
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

    pub(super) fn fast_breaks(
        &mut self,
        line: &str,
        wrap_width: f32,
        indent_px: f32,
        analyzer: Option<&IDWriteTextAnalyzer>,
        out: &mut Vec<usize>,
    ) {
        let break_offsets = analyzer
            .map(|a| analyze_break_opportunities(a, line))
            .unwrap_or_default();
        let mut row_start = 0usize;
        let mut row_width = 0.0f32;
        let mut last_break_ix = 0usize;
        let mut break_cursor = 0usize;

        for (ix, ch) in line.char_indices() {
            let char_end = ix + ch.len_utf8();
            while break_cursor < break_offsets.len() && break_offsets[break_cursor] <= ix {
                break_cursor += 1;
            }
            let w = self.advance(ch, row_width);
            if row_width + w > wrap_width && ix > row_start {
                let split_at = if last_break_ix > row_start {
                    last_break_ix
                } else {
                    ix
                };
                out.push(split_at);
                row_start = split_at;
                row_width = indent_px;
                for c in line[split_at..char_end].chars() {
                    row_width += self.advance(c, row_width);
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

    pub(super) fn dwrite_breaks(
        &self,
        text: &str,
        first_width: f32,
        cont_width: f32,
        out: &mut Vec<usize>,
    ) {
        if text.is_empty() || !first_width.is_finite() || first_width <= 0.0 {
            return;
        }
        let utf16 = encode_single_line_utf16(text);
        if utf16.is_empty() {
            return;
        }

        let collect = |slice: &[u16], max_w: f32| -> Vec<usize> {
            let Ok(layout) = (unsafe {
                self.factory
                    .CreateTextLayout(slice, &self.format, max_w.max(1.0), f32::MAX)
            }) else {
                return Vec::new();
            };
            let _ = unsafe { layout.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP) };
            let mut m = DWRITE_TEXT_METRICS::default();
            if unsafe { layout.GetMetrics(&mut m) }.is_err() || m.lineCount <= 1 {
                return Vec::new();
            }
            let count = m.lineCount as usize;
            let mut metrics = vec![DWRITE_LINE_METRICS::default(); count];
            let mut actual = 0u32;
            let _ = unsafe { layout.GetLineMetrics(Some(&mut metrics), &mut actual) };
            let n = (actual as usize).min(metrics.len());
            if n <= 1 {
                return Vec::new();
            }
            let mut breaks = Vec::with_capacity(n - 1);
            let mut acc = 0usize;
            for lm in &metrics[..n - 1] {
                acc += lm.length as usize;
                if acc > 0 && acc < slice.len() {
                    breaks.push(acc);
                }
            }
            breaks
        };

        let mut u16_breaks = collect(&utf16, first_width);
        if u16_breaks.is_empty() {
            return;
        }
        if (cont_width - first_width).abs() > 0.01 {
            let first = u16_breaks[0];
            u16_breaks.truncate(1);
            if first < utf16.len() {
                for rel in collect(&utf16[first..], cont_width.max(1.0)) {
                    u16_breaks.push(first + rel);
                }
            }
        }

        let mut bi = 0usize;
        let mut u16_acc = 0usize;
        let mut last_byte = 0usize;
        for (byte_ix, ch) in text.char_indices() {
            while bi < u16_breaks.len() && u16_acc >= u16_breaks[bi] {
                if byte_ix > last_byte && byte_ix < text.len() {
                    out.push(byte_ix);
                    last_byte = byte_ix;
                }
                bi += 1;
            }
            if bi >= u16_breaks.len() {
                break;
            }
            u16_acc += ch.len_utf16();
        }
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
        let pos = textposition as usize;
        unsafe {
            if pos >= self.utf16.len() {
                *textstring = std::ptr::null_mut();
                *textlength = 0;
            } else {
                *textstring = self.utf16.as_ptr().add(pos) as *mut u16;
                *textlength = (self.utf16.len() - pos) as u32;
            }
        }
        Ok(())
    }

    fn GetTextBeforePosition(
        &self,
        textposition: u32,
        textstring: *mut *mut u16,
        textlength: *mut u32,
    ) -> windows::core::Result<()> {
        let pos = (textposition as usize).min(self.utf16.len());
        unsafe {
            if pos == 0 {
                *textstring = std::ptr::null_mut();
                *textlength = 0;
            } else {
                *textstring = self.utf16.as_ptr() as *mut u16;
                *textlength = pos as u32;
            }
        }
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
        let pos = (textposition as usize).min(self.utf16.len());
        unsafe {
            *textlength = (self.utf16.len() - pos) as u32;
            *localename = self.locale.as_ptr() as *mut u16;
        }
        Ok(())
    }

    fn GetNumberSubstitution(
        &self,
        textposition: u32,
        textlength: *mut u32,
        numbersubstitution: windows::core::OutRef<'_, IDWriteNumberSubstitution>,
    ) -> windows::core::Result<()> {
        let pos = (textposition as usize).min(self.utf16.len());
        unsafe {
            *textlength = (self.utf16.len() - pos) as u32;
        }
        let _ = numbersubstitution.write(None);
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
        let start = textposition as usize;
        let len = textlength as usize;
        if len == 0 || linebreakpoints.is_null() {
            return Ok(());
        }
        let mut dst = self.out.borrow_mut();
        if start.saturating_add(len) <= dst.len() {
            let src = unsafe { std::slice::from_raw_parts(linebreakpoints, len) };
            dst[start..start + len].copy_from_slice(src);
        }
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

pub(super) fn analyze_break_opportunities(
    analyzer: &IDWriteTextAnalyzer,
    text: &str,
) -> Vec<usize> {
    let mut out = Vec::new();
    if text.is_empty() {
        return out;
    }

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
