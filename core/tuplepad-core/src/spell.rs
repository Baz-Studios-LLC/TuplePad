//! Spell checking with the bundled en_US Hunspell dictionary (from SCOWL), checked by
//! spellbook. Only prose is checked: comments and strings in code, the text (not the code) of
//! Markdown, and everything in plain text. Words that look like code (camelCase, snake_case,
//! ALLCAPS, digits) and pieces of URLs, paths and email addresses are left alone.

use std::collections::HashSet;
use std::sync::OnceLock;

use crate::buffer::TextBuffer;
use crate::syntax::HighlightStyle;
use crate::{TextPos, TextSelection};

static DICTIONARY: OnceLock<Option<spellbook::Dictionary>> = OnceLock::new();

/// Lines longer than this aren't checked.
const LONGEST_LINE: usize = 10_000;

fn dictionary() -> Option<&'static spellbook::Dictionary> {
    DICTIONARY
        .get_or_init(|| spellbook::Dictionary::new(include_str!("../dictionaries/en/en_US.aff"), include_str!("../dictionaries/en/en_US.dic")).ok())
        .as_ref()
}

/// Loads the dictionary (it takes a moment, so do it ahead of time on another thread).
pub fn prepare_spelling() {
    dictionary();
}

/// Whether the dictionary is loaded and spelling can be checked without waiting.
pub fn spelling_ready() -> bool {
    DICTIONARY.get().is_some_and(Option::is_some)
}

/// Whether a word is worth checking: not code-like.
fn checkable(word: &str) -> bool {
    let letters = word.chars().filter(|c| c.is_alphabetic()).count();
    if letters < 2 || word.chars().any(|c| c.is_ascii_digit() || c == '_') {
        return false;
    }
    let mut chars = word.chars();
    let first = chars.next().unwrap_or(' ');
    let rest: Vec<char> = chars.collect();
    // camelCase or PascalCase with more capitals inside, or ALLCAPS.
    if rest.iter().any(|c| c.is_uppercase()) {
        return false;
    }
    let _ = first;
    true
}

/// Whether a word is spelled right (`extra` holds words the user taught it).
pub fn spelled_right(word: &str, extra: &HashSet<String>) -> bool {
    if !checkable(word) || extra.contains(word) || extra.contains(&word.to_lowercase()) {
        return true;
    }
    let Some(dictionary) = dictionary() else { return true };
    dictionary.check(word) || dictionary.check(&word.replace('’', "'"))
}

/// Up to `limit` corrections for a word.
pub fn spelling_suggestions(word: &str, limit: usize) -> Vec<String> {
    let Some(dictionary) = dictionary() else { return Vec::new() };
    let mut out = Vec::new();
    dictionary.suggest(word, &mut out);
    out.truncate(limit);
    out
}

/// The words in a stretch of text, as UTF-16 column ranges (offset by `base`): letters, with
/// apostrophes inside words. Words touching `/`, `@`, `.` or `:` on both sides are skipped as
/// paths, addresses and links.
fn words_in(text: &str, base: u64) -> Vec<(u64, u64, String)> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut column = base;
    let mut i = 0;
    while i < chars.len() {
        if !chars[i].is_alphanumeric() {
            column += chars[i].len_utf16() as u64;
            i += 1;
            continue;
        }
        let start = i;
        let start_column = column;
        while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_' || ((chars[i] == '\'' || chars[i] == '’') && chars.get(i + 1).is_some_and(|c| c.is_alphabetic()))) {
            column += chars[i].len_utf16() as u64;
            i += 1;
        }
        let word: String = chars[start..i].iter().collect();
        let before = start.checked_sub(1).map(|b| chars[b]);
        let after = chars.get(i).copied();
        let joined = |c: Option<char>| matches!(c, Some('/' | '@' | '\\' | '.' | ':' | '-' | '#' | '$' | '%' | '&' | '='));
        let in_link = after == Some(':') && chars.get(i + 1) == Some(&'/');
        if !(joined(before) && joined(after)) && !in_link && before != Some('@') && before != Some('\\') && before != Some('/') {
            out.push((start_column, column, word));
        }
    }
    out
}

impl TextBuffer {
    /// Misspelled words on lines `start_line ..< start_line + count`, as selections.
    pub fn misspellings(&self, start_line: u64, count: u64, extra: &HashSet<String>) -> Vec<TextSelection> {
        if dictionary().is_none() {
            return Vec::new();
        }
        let language = self.language();
        let lines = self.lines(start_line, count);
        let markdown = language.as_deref() == Some("markdown");
        // Data files' strings are keys, names and versions rather than prose.
        let data = matches!(language.as_deref(), Some("json" | "yaml" | "toml"));
        // Code checks only its comments and strings (data files only their comments); Markdown everything but its code.
        // Highlighting a very long line costs a lot, and it isn't checked anyway: only short lines.
        let spans = match &language {
            None => Vec::new(),
            Some(_) if lines.iter().all(|l| l.len() <= LONGEST_LINE) => self.highlights(start_line, count),
            Some(_) => (start_line..start_line + lines.len() as u64)
                .zip(&lines)
                .filter(|(_, l)| l.len() <= LONGEST_LINE)
                .flat_map(|(line, _)| self.highlights(line, 1))
                .collect(),
        };
        let code_lines: HashSet<u64> = if markdown { self.code_block_lines(start_line, count).into_iter().collect() } else { HashSet::new() };
        let mut found = Vec::new();
        for (index, text) in lines.iter().enumerate() {
            let line = start_line + index as u64;
            if text.len() > LONGEST_LINE || code_lines.contains(&line) {
                continue;
            }
            let units: Vec<u16> = text.encode_utf16().collect();
            let piece = |from: u64, to: u64| String::from_utf16_lossy(&units[(from as usize).min(units.len())..(to as usize).min(units.len())]);
            let line_spans = spans.iter().filter(|s| s.line == line);
            let regions: Vec<(u64, u64)> = match (&language, markdown) {
                (None, _) => vec![(0, units.len() as u64)],
                (Some(_), true) => {
                    // Everything except inline code and links' addresses.
                    let mut regions = vec![(0, units.len() as u64)];
                    for span in line_spans.filter(|s| matches!(s.style, HighlightStyle::Code | HighlightStyle::Link)) {
                        regions = regions
                            .into_iter()
                            .flat_map(|(a, b)| {
                                if span.end <= a || span.start >= b {
                                    vec![(a, b)]
                                } else {
                                    vec![(a, span.start.max(a)), (span.end.min(b), b)].into_iter().filter(|(x, y)| x < y).collect()
                                }
                            })
                            .collect();
                    }
                    regions
                }
                (Some(_), false) => line_spans
                    .filter(|s| s.style == HighlightStyle::Comment || (s.style == HighlightStyle::String && !data))
                    .map(|s| (s.start, s.end))
                    .collect(),
            };
            for (from, to) in regions {
                for (start, end, word) in words_in(&piece(from, to), from) {
                    if !spelled_right(&word, extra) {
                        found.push(TextSelection { anchor: TextPos::new(line, start), head: TextPos::new(line, end) });
                    }
                }
            }
        }
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn misspelled(text: &str, language: Option<&str>) -> Vec<String> {
        let buffer = TextBuffer::with_text(text.into());
        buffer.set_language(language.map(str::to_string));
        buffer.misspellings(0, buffer.line_count(), &HashSet::new()).iter().map(|s| buffer.text_in(s.start(), s.end())).collect()
    }

    #[test]
    fn checks_prose_not_code() {
        assert_eq!(misspelled("This sentense has a mistake, isn't it?", None), ["sentense"]);
        // Only comments and strings in code.
        assert_eq!(misspelled("let wrongg = \"helo there\"; // a comentt\n", Some("rust")), ["helo", "comentt"]);
        // Data files: only comments, not their keys and values.
        assert!(misspelled("{\"nmae\": \"vite buld\"}\n", Some("json")).is_empty());
        assert_eq!(misspelled("# a comentt\nkey: valeu\n", Some("yaml")), ["comentt"]);
        // Identifiers, acronyms, numbers, paths and links are left alone.
        assert!(misspelled("camelCaseWord snake_case HTTPS v2 see /usr/locl/bin and https://exampel.com or me@exmple.org", None).is_empty());
        // Markdown: the text, not its code.
        assert_eq!(misspelled("Some txet with `codde` here.\n\n```\nnott checked\n```\n", Some("markdown")), ["txet"]);
    }

    #[test]
    fn learned_words_and_suggestions() {
        let buffer = TextBuffer::with_text("TuplePad is a scribblr".into());
        let mut extra = HashSet::new();
        assert_eq!(buffer.misspellings(0, 1, &extra).len(), 1);
        extra.insert("scribblr".to_string());
        assert!(buffer.misspellings(0, 1, &extra).is_empty());
        assert!(spelling_suggestions("sentense", 5).iter().any(|s| s == "sentence"));
    }
}
