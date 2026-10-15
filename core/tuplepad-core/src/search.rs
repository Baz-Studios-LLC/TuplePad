//! Find and replace over a rope, with literal or regular-expression patterns.

use regex::{Regex, RegexBuilder};
use ropey::Rope;

use crate::error::{CoreError, CoreResult};

/// What to look for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchQuery {
    pub pattern: String,
    pub case_sensitive: bool,
    /// Only match whole words (a word character can't touch either side of the match).
    pub whole_word: bool,
    /// Treat `pattern` as a regular expression; otherwise it's literal text.
    pub regex: bool,
}

/// Documents are searched in windows of this many chars, overlapping so matches that
/// straddle a window edge are still found (unless a single match is longer than the overlap).
const WINDOW_CHARS: usize = 16 * 1024 * 1024;
const OVERLAP_CHARS: usize = 64 * 1024;

pub(crate) fn compile(query: &SearchQuery) -> CoreResult<Option<Regex>> {
    if query.pattern.is_empty() {
        return Ok(None);
    }
    let mut pattern = if query.regex { query.pattern.clone() } else { regex::escape(&query.pattern) };
    if query.whole_word {
        // A boundary only makes sense next to a word character; "foo()" can't end with \b.
        let is_word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
        let raw = &query.pattern;
        let start = if !query.regex && !is_word(raw.chars().next()) { "" } else { r"\b" };
        let end = if !query.regex && !is_word(raw.chars().last()) { "" } else { r"\b" };
        pattern = format!("{start}(?:{pattern}){end}");
    }
    RegexBuilder::new(&pattern)
        .case_insensitive(!query.case_sensitive)
        .multi_line(true)
        .crlf(true)
        .build()
        .map(Some)
        .map_err(|e| CoreError::InvalidPattern { message: describe(e) })
}

fn describe(error: regex::Error) -> String {
    match error {
        regex::Error::Syntax(text) => {
            // The last line reads like "error: unclosed group".
            let line = text.lines().last().unwrap_or("").trim();
            let message = line.strip_prefix("error:").unwrap_or(line).trim();
            let mut chars = message.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect(),
                None => "Invalid regular expression.".into(),
            }
        }
        regex::Error::CompiledTooBig(_) => "The regular expression is too large.".into(),
        _ => "Invalid regular expression.".into(),
    }
}

/// A match as char offsets into the rope, plus its expanded replacement when replacing.
pub(crate) struct Found {
    pub start: usize,
    pub end: usize,
    pub replacement: Option<String>,
}

/// Finds matches in document order. Stops after `limit` (returns whether it stopped early).
/// With `replacement`, also expands `$1`-style references for each match (regex mode only).
pub(crate) fn find(
    rope: &Rope,
    regex: &Regex,
    query: &SearchQuery,
    replacement: Option<&str>,
    limit: usize,
) -> (Vec<Found>, bool) {
    let mut found = Vec::new();
    let stopped = visit(rope, regex, query, replacement, |f| {
        found.push(f);
        found.len() < limit
    });
    (found, stopped)
}

/// How many matches there are, without keeping them.
pub(crate) fn count(rope: &Rope, regex: &Regex, query: &SearchQuery) -> u64 {
    let mut total = 0;
    visit(rope, regex, query, None, |_| {
        total += 1;
        true
    });
    total
}

/// Calls `each` with every match in document order until it returns false. Returns whether it
/// stopped early.
pub(crate) fn visit(
    rope: &Rope,
    regex: &Regex,
    query: &SearchQuery,
    replacement: Option<&str>,
    each: impl FnMut(Found) -> bool,
) -> bool {
    visit_until(rope, regex, query, replacement, &|| false, each)
}

/// Like `find`, but gives up (returning nil) as soon as `cancelled` is set, checking between windows.
pub(crate) fn find_cancellable(
    rope: &Rope,
    regex: &Regex,
    query: &SearchQuery,
    limit: usize,
    cancelled: &dyn Fn() -> bool,
) -> Option<(Vec<Found>, bool)> {
    let mut found = Vec::new();
    let stopped = visit_until(rope, regex, query, None, cancelled, |f| {
        found.push(f);
        found.len() < limit
    });
    if cancelled() {
        return None;
    }
    Some((found, stopped))
}

/// Whether `text` has a match, checking `cancelled` between chunks of a large text (nil if cancelled).
/// Chunks overlap, so a match up to the overlap's length is never split.
pub(crate) fn has_match(text: &str, regex: &Regex, cancelled: &dyn Fn() -> bool) -> Option<bool> {
    // Small enough that Stop is quick even with a slow pattern on a slow machine.
    const CHUNK: usize = 2 * 1024 * 1024;
    const OVERLAP: usize = 64 * 1024;
    if text.len() <= CHUNK {
        return Some(regex.is_match(text));
    }
    let mut start = 0;
    loop {
        if cancelled() {
            return None;
        }
        let mut end = (start + CHUNK).min(text.len());
        while !text.is_char_boundary(end) {
            end += 1;
        }
        if regex.is_match(&text[start..end]) {
            return Some(true);
        }
        if end == text.len() {
            return Some(false);
        }
        start = end - OVERLAP;
        while !text.is_char_boundary(start) {
            start -= 1;
        }
    }
}

fn visit_until(
    rope: &Rope,
    regex: &Regex,
    query: &SearchQuery,
    replacement: Option<&str>,
    cancelled: &dyn Fn() -> bool,
    mut each: impl FnMut(Found) -> bool,
) -> bool {
    let total = rope.len_chars();
    let mut window_start = 0;
    // Matches before this char were already reported by the previous window.
    let mut accept_from = 0;
    loop {
        if cancelled() {
            return true;
        }
        let window_end = (window_start + WINDOW_CHARS).min(total);
        let last_window = window_end == total;
        let text = rope.slice(window_start..window_end).to_string();
        let base_byte = rope.char_to_byte(window_start);
        // Leave matches near the end of a window to the next one, which sees them whole.
        let accept_until = if last_window { usize::MAX } else { window_end - OVERLAP_CHARS };

        for captures in regex.captures_iter(&text) {
            let whole = captures.get(0).expect("group 0 always matches");
            if whole.is_empty() {
                continue; // zero-width matches (like ^) aren't useful to find
            }
            let start = rope.byte_to_char(base_byte + whole.start());
            if start < accept_from {
                continue;
            }
            if start >= accept_until {
                break;
            }
            let end = rope.byte_to_char(base_byte + whole.end());
            let expanded = replacement.map(|template| {
                if query.regex {
                    let mut out = String::new();
                    captures.expand(template, &mut out);
                    out
                } else {
                    template.to_string()
                }
            });
            if !each(Found { start, end, replacement: expanded }) {
                return true;
            }
        }

        if last_window {
            return false;
        }
        accept_from = accept_until;
        window_start = window_end - OVERLAP_CHARS;
    }
}

/// Where a match sits in its line, in UTF-16 columns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MatchSpan {
    pub start: u32,
    pub end: u32,
}

/// The matches on one line, with a short excerpt of the line to show in search results.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineMatches {
    /// 0-based.
    pub line: u64,
    /// The line without its leading indentation, cut down around the first match if it's long.
    pub preview: String,
    /// The UTF-16 column in the line where `preview` starts.
    pub preview_start: u32,
    /// The matches, in UTF-16 columns of the whole line. A match that runs onto later lines
    /// ends at the end of this one.
    pub spans: Vec<MatchSpan>,
}

/// A line of search results is cut down to about this many UTF-16 units.
const PREVIEW_LENGTH: usize = 240;
/// …starting this far before the first match when it has to be cut.
const PREVIEW_LEAD: usize = 48;

/// Groups matches (in document order) by line, with previews. At most `max_lines` lines.
pub(crate) fn group_by_line(rope: &Rope, found: &[Found], max_lines: usize) -> Vec<LineMatches> {
    let mut lines: Vec<LineMatches> = Vec::new();
    for f in found {
        let line = rope.char_to_line(f.start);
        let first_char = rope.line_to_char(line);
        let content = line_content(rope, line);
        let start = (rope.char_to_utf16_cu(f.start) - rope.char_to_utf16_cu(first_char)) as u32;
        let end_char = f.end.min(first_char + content.len_chars());
        let end = (rope.char_to_utf16_cu(end_char) - rope.char_to_utf16_cu(first_char)) as u32;
        let span = MatchSpan { start, end: end.max(start) };
        match lines.last_mut() {
            Some(last) if last.line == line as u64 => last.spans.push(span),
            _ => {
                if lines.len() == max_lines {
                    break;
                }
                let (preview, preview_start) = preview(content, span.start as usize);
                lines.push(LineMatches { line: line as u64, preview, preview_start, spans: vec![span] });
            }
        }
    }
    lines
}

/// A line without its trailing line break.
pub(crate) fn line_content(rope: &Rope, line: usize) -> ropey::RopeSlice<'_> {
    let slice = rope.line(line);
    let len = slice.len_chars();
    let trim = match (len.checked_sub(2).map(|i| slice.char(i)), len.checked_sub(1).map(|i| slice.char(i))) {
        (Some('\r'), Some('\n')) => 2,
        (_, Some('\n' | '\r')) => 1,
        _ => 0,
    };
    slice.slice(..len - trim)
}

/// An excerpt of a line for search results, and the UTF-16 column it starts at.
fn preview(content: ropey::RopeSlice<'_>, first_match: usize) -> (String, u32) {
    let indent = content.chars().take_while(|c| *c == ' ' || *c == '\t').count();
    let total = content.len_utf16_cu();
    // Skip indentation, and if the line is long, start a little before the first match.
    let mut start = content.char_to_utf16_cu(indent).min(first_match);
    if total - start > PREVIEW_LENGTH && first_match > start + PREVIEW_LEAD {
        start = first_match - PREVIEW_LEAD;
    }
    let start_char = content.utf16_cu_to_char(start);
    let end_char = content.utf16_cu_to_char((start + PREVIEW_LENGTH).min(total));
    let start = content.char_to_utf16_cu(start_char);
    (content.slice(start_char..end_char).to_string(), start as u32)
}

/// One changed stretch of lines, before and after a replacement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplacementPreview {
    /// 0-based first line of the stretch.
    pub line: u64,
    pub before: String,
    pub after: String,
}

/// What replacing every match would do, line by line. At most `max_groups` stretches.
pub(crate) fn preview_replacements(rope: &Rope, found: &[Found], max_groups: usize) -> Vec<ReplacementPreview> {
    let mut groups: Vec<(usize, usize, Vec<&Found>)> = Vec::new();
    for f in found {
        let first = rope.char_to_line(f.start);
        let last = rope.char_to_line(f.end.max(f.start));
        match groups.last_mut() {
            Some(group) if first <= group.1 => {
                group.1 = group.1.max(last);
                group.2.push(f);
            }
            _ => {
                if groups.len() == max_groups {
                    break;
                }
                groups.push((first, last, vec![f]));
            }
        }
    }
    groups
        .into_iter()
        .map(|(first, last, matches)| {
            let start = rope.line_to_char(first);
            let end = rope.line_to_char(last) + line_content(rope, last).len_chars();
            let before = rope.slice(start..end).to_string();
            let mut after = String::with_capacity(before.len());
            let mut copied = start;
            for f in matches {
                after.extend(rope.slice(copied..f.start).chunks());
                after.push_str(f.replacement.as_deref().unwrap_or(""));
                copied = f.end;
            }
            after.extend(rope.slice(copied..end).chunks());
            ReplacementPreview { line: first as u64, before, after }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(pattern: &str) -> SearchQuery {
        SearchQuery { pattern: pattern.into(), case_sensitive: false, whole_word: false, regex: false }
    }

    fn spans(text: &str, query: &SearchQuery) -> Vec<(usize, usize)> {
        let rope = Rope::from_str(text);
        let regex = compile(query).unwrap().unwrap();
        find(&rope, &regex, query, None, usize::MAX).0.into_iter().map(|f| (f.start, f.end)).collect()
    }

    #[test]
    fn literal_patterns_are_escaped_and_case_insensitive_by_default() {
        assert_eq!(spans("a.b A.B axb", &query("a.b")), vec![(0, 3), (4, 7)]);
        let sensitive = SearchQuery { case_sensitive: true, ..query("a.b") };
        assert_eq!(spans("a.b A.B", &sensitive), vec![(0, 3)]);
    }

    #[test]
    fn whole_words() {
        let q = SearchQuery { whole_word: true, ..query("cat") };
        assert_eq!(spans("cat concat cat_ cats (cat)", &q), vec![(0, 3), (22, 25)]);
        // Patterns ending in punctuation still work.
        let q = SearchQuery { whole_word: true, ..query("run()") };
        assert_eq!(spans("run() rerun()", &q), vec![(0, 5)]);
    }

    #[test]
    fn regex_lines_and_offsets_are_in_chars() {
        let q = SearchQuery { regex: true, ..query(r"^\w+$") };
        assert_eq!(spans("héllo\r\nwörld\nx y", &q), vec![(0, 5), (7, 12)]);
        let q = SearchQuery { regex: true, ..query("👋+") };
        assert_eq!(spans("hi 👋👋 there", &q), vec![(3, 5)]);
    }

    #[test]
    fn invalid_patterns_explain_themselves() {
        let q = SearchQuery { regex: true, ..query("(unclosed") };
        assert!(matches!(compile(&q), Err(CoreError::InvalidPattern { .. })));
        assert!(compile(&query("")).unwrap().is_none());
    }

    #[test]
    fn matches_across_window_edges_are_found_once() {
        // A document bigger than one window, with matches right around the boundary.
        let mut text = "x".repeat(WINDOW_CHARS - OVERLAP_CHARS - 3);
        text.push_str("needle");
        text.push_str(&"y".repeat(OVERLAP_CHARS));
        text.push_str("needle");
        text.push_str(&"z".repeat(WINDOW_CHARS));
        text.push_str("needle");
        let found = spans(&text, &query("needle"));
        assert_eq!(found.len(), 3, "{found:?}");
        let mut starts: Vec<_> = found.iter().map(|f| f.0).collect();
        starts.dedup();
        assert_eq!(starts.len(), 3);
    }
}
