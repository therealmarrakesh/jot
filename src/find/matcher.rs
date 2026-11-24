use std::ops::Range;

use ropey::Rope;

const BLOCK_SIZE: usize = 256 * 1024;
const MAX_UTF8_LEN: usize = 4;

#[inline]
fn fold_char(c: char) -> char {
    if c.is_ascii() {
        return c.to_ascii_lowercase();
    }
    match c {
        '\u{017F}' => 's',
        '\u{03C2}' => '\u{03C3}',
        '\u{03D0}' => '\u{03B2}',
        '\u{03D1}' => '\u{03B8}',
        '\u{03D5}' => '\u{03C6}',
        '\u{03D6}' => '\u{03C0}',
        '\u{03F0}' => '\u{03BA}',
        '\u{03F1}' => '\u{03C1}',
        '\u{03F5}' => '\u{03B5}',
        '\u{1E9B}' => '\u{1E61}',
        '\u{2126}' => '\u{03C9}',
        '\u{212A}' => 'k',
        '\u{212B}' => '\u{00E5}',
        _ => {
            let mut lower = c.to_lowercase();
            let first = lower.next().unwrap_or(c);
            if lower.next().is_none() { first } else { c }
        }
    }
}

#[inline]
fn match_rest_folded(haystack: &str, start: usize, rest: &[char]) -> Option<usize> {
    if rest.is_empty() {
        return Some(start);
    }
    let mut chars = haystack[start..].char_indices();
    for &expected in rest {
        let (_, ch) = chars.next()?;
        if fold_char(ch) != expected {
            return None;
        }
    }
    let end = chars
        .next()
        .map_or(haystack.len(), |(rel, _)| start + rel);
    Some(end)
}

pub(crate) struct Matcher {
    pub(super) query: String,
    pub(super) match_case: bool,
    folded: Vec<char>,
    overlap: usize,
    block: usize,
}

impl Matcher {
    pub(crate) fn new(query: &str, match_case: bool) -> Option<Self> {
        if query.is_empty() {
            return None;
        }
        let folded = if match_case {
            Vec::new()
        } else {
            query.chars().map(fold_char).collect()
        };
        let overlap = query.chars().count() * MAX_UTF8_LEN;
        Some(Self {
            query: query.to_string(),
            match_case,
            folded,
            overlap,
            block: BLOCK_SIZE.max(4 * overlap),
        })
    }

    fn fill(text: &Rope, range: Range<usize>, buffer: &mut String) -> usize {
        let start = text.floor_char_boundary(range.start);
        let end = text.ceil_char_boundary(range.end.min(text.len()));
        buffer.clear();
        for chunk in text.slice(start..end).chunks() {
            buffer.push_str(chunk);
        }
        start
    }

    fn find_at(&self, haystack: &str, at: usize) -> Option<Range<usize>> {
        if at >= haystack.len() {
            return None;
        }
        let at = haystack.ceil_char_boundary(at);
        if self.match_case {
            return haystack[at..]
                .find(&self.query)
                .map(|off| (at + off)..(at + off + self.query.len()));
        }
        let (&first, rest) = self.folded.split_first()?;
        if first.is_ascii() {
            let lower = first as u8;
            let upper = lower.to_ascii_uppercase();
            let check_non_ascii = lower == b'k' || lower == b's';
            let bytes = haystack.as_bytes();
            let mut i = at;
            while i < bytes.len() {
                let b = bytes[i];
                if b < 0x80 {
                    if (b == lower || b == upper)
                        && let Some(end) = match_rest_folded(haystack, i + 1, rest)
                    {
                        return Some(i..end);
                    }
                    i += 1;
                } else {
                    let ch = haystack[i..].chars().next()?;
                    let next = i + ch.len_utf8();
                    if check_non_ascii
                        && fold_char(ch) == first
                        && let Some(end) = match_rest_folded(haystack, next, rest)
                    {
                        return Some(i..end);
                    }
                    i = next;
                }
            }
            None
        } else {
            for (rel, ch) in haystack[at..].char_indices() {
                if fold_char(ch) == first {
                    let start = at + rel;
                    if let Some(end) = match_rest_folded(haystack, start + ch.len_utf8(), rest) {
                        return Some(start..end);
                    }
                }
            }
            None
        }
    }

    pub(crate) fn is_match(&self, text: &str) -> bool {
        if self.match_case {
            text == self.query
        } else {
            text.chars()
                .map(fold_char)
                .eq(self.folded.iter().copied())
        }
    }

    pub(crate) fn max_match_bytes(&self) -> usize {
        if self.match_case {
            self.query.len()
        } else {
            self.folded.len().saturating_mul(MAX_UTF8_LEN)
        }
    }

    pub(crate) fn next(
        &self,
        text: &Rope,
        from: usize,
        limit: usize,
        buffer: &mut String,
    ) -> Option<Range<usize>> {
        let len = text.len();
        let limit = limit.min(len);
        let mut pos = from.min(len);
        loop {
            if pos >= limit {
                return None;
            }
            let end = (pos + self.block + self.overlap)
                .min(len)
                .min(limit + self.overlap);
            let start = Self::fill(text, pos..end, buffer);
            let haystack = buffer.as_str();
            let safe = if end == len {
                limit
            } else {
                (end - self.overlap).min(limit)
            };
            if let Some(found) = self.find_at(haystack, pos - start) {
                let range = start + found.start..start + found.end;
                if range.start < safe {
                    return Some(range);
                }
            }
            if end == len {
                return None;
            }
            pos = end - self.overlap;
        }
    }

    pub(crate) fn previous(
        &self,
        text: &Rope,
        before: usize,
        floor: usize,
        buffer: &mut String,
    ) -> Option<Range<usize>> {
        let len = text.len();
        let floor = floor.min(len);
        let mut before = before.min(len);
        loop {
            let begin = before.saturating_sub(self.block).max(floor);
            let start = Self::fill(text, begin..(before + self.overlap).min(len), buffer);
            let haystack = buffer.as_str();
            let limit = before - start;
            let mut found = None;
            let mut cursor = 0;
            while let Some(hit) = self.find_at(haystack, cursor) {
                if hit.start >= limit {
                    break;
                }
                cursor = hit.end;
                found = Some(hit);
            }
            while let Some(later) = found
                .as_ref()
                .and_then(|range| self.find_at(haystack, range.start + 1))
                .filter(|later| later.start < limit)
            {
                found = Some(later);
            }
            if let Some(range) = found {
                return Some(start + range.start..start + range.end);
            }
            if begin <= floor {
                return None;
            }
            before = begin;
        }
    }

    pub(crate) fn all(&self, text: &Rope, buffer: &mut String) -> Vec<Range<usize>> {
        let len = text.len();
        let mut ranges = Vec::new();
        let mut pos = 0;
        loop {
            let end = (pos + self.block + self.overlap).min(len);
            let start = Self::fill(text, pos..end, buffer);
            let haystack = buffer.as_str();
            let safe = if end == len {
                usize::MAX
            } else {
                end - self.overlap
            };
            let mut resume = safe;
            let mut cursor = pos - start;
            while let Some(found) = self.find_at(haystack, cursor) {
                let range = start + found.start..start + found.end;
                if range.start >= safe {
                    break;
                }
                resume = resume.max(range.end);
                ranges.push(range);
                cursor = found.end;
            }
            if end == len {
                return ranges;
            }
            pos = resume;
        }
    }

    pub(crate) fn search(
        &self,
        text: &Rope,
        selection: Range<usize>,
        forward: bool,
        wrap: bool,
        buffer: &mut String,
    ) -> Option<(Range<usize>, bool)> {
        if forward {
            if let Some(range) = self.next(text, selection.end, usize::MAX, buffer) {
                return Some((range, false));
            }
            if !wrap {
                return None;
            }
            self.next(text, 0, selection.end, buffer)
                .map(|range| (range, true))
        } else {
            if let Some(range) = self.previous(text, selection.start, 0, buffer) {
                return Some((range, false));
            }
            if !wrap {
                return None;
            }
            self.previous(text, text.len(), selection.start, buffer)
                .map(|range| (range, true))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunked_matcher(query: &str, match_case: bool, block: usize) -> Matcher {
        let mut matcher = Matcher::new(query, match_case).unwrap();
        matcher.block = block;
        matcher
    }

    #[test]
    fn chunked_search_matches_across_block_boundaries() {
        let text = Rope::from("alpha beta alpha gamma alpha");
        let matcher = chunked_matcher("alpha", true, 4);
        let mut buffer = String::new();

        assert_eq!(matcher.all(&text, &mut buffer), vec![0..5, 11..16, 23..28]);
        assert_eq!(
            matcher.search(&text, 0..0, true, false, &mut buffer),
            Some((0..5, false))
        );
        assert_eq!(
            matcher.search(&text, 23..28, true, true, &mut buffer),
            Some((0..5, true))
        );
        assert_eq!(
            matcher.search(&text, 23..28, true, false, &mut buffer),
            None
        );
        assert_eq!(
            matcher.search(&text, 11..16, false, false, &mut buffer),
            Some((0..5, false))
        );
        assert_eq!(
            matcher.search(&text, 0..5, false, true, &mut buffer),
            Some((23..28, true))
        );
    }

    #[test]
    fn case_insensitive_unicode_folding_matches_across_chunk_boundary() {
        let text = Rope::from("abc\u{212A}def");
        let matcher = chunked_matcher("k", false, 2);
        let mut buffer = String::new();

        assert_eq!(matcher.all(&text, &mut buffer), vec![3..6]);
        assert_eq!(matcher.next(&text, 0, usize::MAX, &mut buffer), Some(3..6));
        assert_eq!(
            matcher.previous(&text, text.len(), 0, &mut buffer),
            Some(3..6)
        );
    }

    #[test]
    fn previous_finds_rightmost_overlapping_match() {
        let text = Rope::from("aaaa");
        let matcher = chunked_matcher("aa", true, 2);
        let mut buffer = String::new();

        assert_eq!(matcher.previous(&text, 2, 0, &mut buffer), Some(1..3));
        assert_eq!(matcher.previous(&text, 1, 0, &mut buffer), Some(0..2));
        assert_eq!(matcher.previous(&text, 0, 0, &mut buffer), None);
    }
}
