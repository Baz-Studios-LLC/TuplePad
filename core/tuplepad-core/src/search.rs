//! Find and replace over a rope, with literal or regular-expression patterns.

use regex::{Regex, RegexBuilder};
use ropey::Rope;

use crate::error::{CoreError, CoreResult};

/// What to look for.
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
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
    let total = rope.len_chars();
    let mut found = Vec::new();
    let mut window_start = 0;
    // Matches before this char were already reported by the previous window.
    let mut accept_from = 0;
    loop {
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
            found.push(Found { start, end, replacement: expanded });
            if found.len() >= limit {
                return (found, true);
            }
        }

        if last_window {
            return (found, false);
        }
        accept_from = accept_until;
        window_start = window_end - OVERLAP_CHARS;
    }
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
