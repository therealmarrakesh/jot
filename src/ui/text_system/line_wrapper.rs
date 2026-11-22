use crate::ui::{FontId, Pixels, TextSystem, px};
use crate::ui::collections::HashMap;
use std::{iter, sync::Arc};

/// Controls how soft-wrapped continuation lines are indented.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum IndentAdjustment {
    /// No indent - continuation lines start at column 0.
    #[cfg(test)]
    NoIndent,
    /// Match the original line's leading whitespace.
    #[default]
    SameIndent,
}

/// The GPUI line wrapper, used to wrap lines of text to a given width.
pub(crate) struct LineWrapper {
    text_system: Arc<TextSystem>,
    pub(crate) font_id: FontId,
    pub(crate) font_size: Pixels,
    cached_ascii_char_widths: [Option<Pixels>; 128],
    cached_other_char_widths: HashMap<char, Pixels>,
}

impl LineWrapper {
    /// The maximum indent that can be applied to a line.
    pub(crate) const MAX_INDENT: u32 = 256;

    pub(crate) fn new(font_id: FontId, font_size: Pixels, text_system: Arc<TextSystem>) -> Self {
        Self {
            text_system,
            font_id,
            font_size,
            cached_ascii_char_widths: [None; 128],
            cached_other_char_widths: HashMap::default(),
        }
    }

    /// Wrap a line of text to the given width with this wrapper's font and font size.
    pub(crate) fn wrap_line<'a>(
        &'a mut self,
        fragments: &'a [LineFragment],
        wrap_width: Pixels,
        indent_adjustment: IndentAdjustment,
    ) -> impl Iterator<Item = Boundary> + 'a {
        let mut width = px(0.);
        let mut first_non_whitespace_ix = None;
        let mut base_indent = None;
        let mut last_candidate_ix = 0;
        let mut last_candidate_width = px(0.);
        let mut last_wrap_ix = 0;
        let mut prev_c = '\0';
        let mut index = 0;
        let mut candidates = fragments
            .iter()
            .flat_map(move |fragment| fragment.chars())
            .peekable();
        iter::from_fn(move || {
            for c in candidates.by_ref() {
                let ix = index;
                index += c.len_utf8();
                if c == '\n' {
                    continue;
                }

                if Self::is_word_char(c) {
                    if prev_c == ' ' && c != ' ' && first_non_whitespace_ix.is_some() {
                        last_candidate_ix = ix;
                        last_candidate_width = width;
                    }
                } else {
                    // CJK may not be space separated, e.g.: `Hello world你好世界`
                    if c != ' ' && first_non_whitespace_ix.is_some() {
                        last_candidate_ix = ix;
                        last_candidate_width = width;
                    }
                }

                if c != ' ' && first_non_whitespace_ix.is_none() {
                    first_non_whitespace_ix = Some(ix);
                }

                let item_width = self.width_for_char(c);
                width += item_width;
                if width > wrap_width && ix > last_wrap_ix {
                    let wrap_at_candidate =
                        last_candidate_ix > 0 && width - last_candidate_width <= wrap_width;

                    if base_indent.is_none()
                        && let Some(first_non_whitespace_ix) = first_non_whitespace_ix
                    {
                        base_indent = Some(
                            Self::MAX_INDENT
                                .min(first_non_whitespace_ix.saturating_sub(last_wrap_ix) as u32),
                        );
                    }

                    let next_indent = match indent_adjustment {
                        #[cfg(test)]
                        IndentAdjustment::NoIndent => 0,
                        IndentAdjustment::SameIndent => base_indent.unwrap_or(0),
                    };

                    if wrap_at_candidate {
                        last_wrap_ix = last_candidate_ix;
                        width -= last_candidate_width;
                    } else {
                        last_wrap_ix = ix;
                        width = item_width;
                    }
                    last_candidate_ix = 0;

                    width += self.width_for_char(' ') * next_indent as f32;

                    return Some(Boundary::new(last_wrap_ix, next_indent));
                }

                prev_c = c;
            }

            None
        })
    }

    /// Any character in this list should be treated as a word character,
    /// meaning it can be part of a word that should not be wrapped.
    pub(crate) fn is_word_char(c: char) -> bool {
        // ASCII alphanumeric characters, for English, numbers: `Hello123`, etc.
        c.is_ascii_alphanumeric() ||
        // Latin script in Unicode for French, German, Spanish, etc.
        // Latin-1 Supplement
        // https://en.wikipedia.org/wiki/Latin-1_Supplement
        matches!(c, '\u{00C0}'..='\u{00FF}') ||
        // Latin Extended-A
        // https://en.wikipedia.org/wiki/Latin_Extended-A
        matches!(c, '\u{0100}'..='\u{017F}') ||
        // Latin Extended-B
        // https://en.wikipedia.org/wiki/Latin_Extended-B
        matches!(c, '\u{0180}'..='\u{024F}') ||
        // Cyrillic for Russian, Ukrainian, etc.
        // https://en.wikipedia.org/wiki/Cyrillic_script_in_Unicode
        matches!(c, '\u{0400}'..='\u{04FF}') ||

        // Vietnamese (https://vietunicode.sourceforge.net/charset/)
        matches!(c, '\u{1E00}'..='\u{1EFF}') || // Latin Extended Additional
        matches!(c, '\u{0300}'..='\u{036F}') || // Combining Diacritical Marks

        // Bengali (https://en.wikipedia.org/wiki/Bengali_(Unicode_block))
        matches!(c, '\u{0980}'..='\u{09FF}') ||

        // Some other known special characters that should be treated as word characters,
        // e.g. `a-b`, `var_name`, `I'm`/`won’t`, '@mention`, `#hashtag`, `100%`, `3.1415`,
        // `2^3`, `a~b`, `a=1`, `Self::new`, etc. Trailing punctuation like `,`, `.`, `:`, `;`
        // is included so it stays attached to the preceding word when wrapping.
        matches!(c, '-' | '_' | '.' | '\'' | '’' | '‘' | '$' | '%' | '@' | '#' | '^' | '~' | ',' | '=' | ':' | ';') ||
        // Closing punctuation never starts a line (UAX #14 LB13: no break
        // before `!`, `)`, `]`, `}`, closing quotes or an ellipsis) — `plz!`,
        // `see)`, `quoted”` wrap as one word instead of orphaning the mark on
        // the next line. `/` and `?` stay break opportunities so long paths
        // and URLs (`a/b`, `foo?b=2`) can wrap.
        matches!(c, '!' | ')' | ']' | '}' | '"' | '”' | '»' | '…') ||
        // `⋯` character is special used in Zed, to keep this at the end of the line.
        matches!(c, '⋯') ||

        // Non-breaking glue characters
        matches!(c, '\u{202F}' | '\u{00A0}' | '\u{2011}')
    }

    #[inline(always)]
    fn width_for_char(&mut self, c: char) -> Pixels {
        if (c as u32) < 128 {
            if let Some(cached_width) = self.cached_ascii_char_widths[c as usize] {
                cached_width
            } else {
                let width = self
                    .text_system
                    .layout_width(self.font_id, self.font_size, c);
                self.cached_ascii_char_widths[c as usize] = Some(width);
                width
            }
        } else if let Some(cached_width) = self.cached_other_char_widths.get(&c) {
            *cached_width
        } else {
            let width = self
                .text_system
                .layout_width(self.font_id, self.font_size, c);
            self.cached_other_char_widths.insert(c, width);
            width
        }
    }
}

/// A fragment of a line that can be wrapped.
pub(crate) enum LineFragment<'a> {
    /// A text fragment consisting of characters.
    Text {
        /// The text content of the fragment.
        text: &'a str,
    },
}

impl<'a> LineFragment<'a> {
    /// Creates a new text fragment from the given text.
    pub(crate) fn text(text: &'a str) -> Self {
        LineFragment::Text { text }
    }

    fn chars(&self) -> std::str::Chars<'a> {
        let LineFragment::Text { text } = self;
        text.chars()
    }
}

/// A boundary between two lines of text.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct Boundary {
    /// The index of the last character in a line
    pub(crate) ix: usize,
    /// The indent of the next line.
    pub(crate) next_indent: u32,
}

impl Boundary {
    fn new(ix: usize, next_indent: u32) -> Self {
        Self { ix, next_indent }
    }
}
