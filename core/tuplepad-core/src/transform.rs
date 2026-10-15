//! Text transformations behind the Text menu: line operations, case changes and whitespace tools.
//! These are pure functions over strings; `TextBuffer` applies their results as undoable edits.

use unicode_segmentation::UnicodeSegmentation;

use crate::wrap;

/// Operations on whole lines: the selected lines, or the whole document when nothing is selected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineOperation {
    SortAscending,
    SortDescending,
    SortCaseInsensitive,
    SortNumeric,
    Reverse,
    RemoveDuplicates,
    RemoveConsecutiveDuplicates,
    RemoveEmpty,
    RemoveBlank,
    Join,
    TrimTrailing,
    TrimLeading,
    TrimBoth,
    TabsToSpaces,
    LeadingSpacesToTabs,
}

impl LineOperation {
    /// Operations that change each line in place, keeping the number of lines.
    pub(crate) fn keeps_lines(self) -> bool {
        matches!(
            self,
            LineOperation::TrimTrailing
                | LineOperation::TrimLeading
                | LineOperation::TrimBoth
                | LineOperation::TabsToSpaces
                | LineOperation::LeadingSpacesToTabs
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaseChange {
    Upper,
    Lower,
    /// Capitalize Each Word.
    Title,
    /// Capitalize the first letter of each sentence.
    Sentence,
    /// sWAP tHE cASE.
    Swap,
}

pub(crate) fn apply_to_lines(operation: LineOperation, lines: Vec<String>, tab_width: usize) -> Vec<String> {
    use LineOperation::*;
    let mut lines = lines;
    match operation {
        SortAscending => lines.sort(),
        SortDescending => lines.sort_by(|a, b| b.cmp(a)),
        SortCaseInsensitive => lines.sort_by_cached_key(|line| line.to_lowercase()),
        SortNumeric => lines.sort_by(|a, b| {
            let (x, y) = (leading_number(a), leading_number(b));
            x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.cmp(b))
        }),
        Reverse => lines.reverse(),
        RemoveDuplicates => {
            let mut seen = std::collections::HashSet::new();
            lines.retain(|line| seen.insert(line.clone()));
        }
        RemoveConsecutiveDuplicates => lines.dedup(),
        RemoveEmpty => lines.retain(|line| !line.is_empty()),
        RemoveBlank => lines.retain(|line| !line.trim().is_empty()),
        Join => {
            let mut joined = String::new();
            for line in &lines {
                let piece = if joined.is_empty() { line.trim_end() } else { line.trim() };
                if piece.is_empty() {
                    continue;
                }
                if !joined.is_empty() {
                    joined.push(' ');
                }
                joined.push_str(piece);
            }
            lines = vec![joined];
        }
        TrimTrailing => lines.iter_mut().for_each(|l| *l = l.trim_end().to_string()),
        TrimLeading => lines.iter_mut().for_each(|l| *l = l.trim_start().to_string()),
        TrimBoth => lines.iter_mut().for_each(|l| *l = l.trim().to_string()),
        TabsToSpaces => lines.iter_mut().for_each(|l| *l = expand_tabs(l, tab_width)),
        LeadingSpacesToTabs => lines.iter_mut().for_each(|l| *l = leading_spaces_to_tabs(l, tab_width)),
    }
    lines
}

/// The first number in a line, for numeric sorting. Lines without one sort first.
fn leading_number(line: &str) -> Option<f64> {
    static NUMBER: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let number = NUMBER.get_or_init(|| regex::Regex::new(r"-?\d+(?:\.\d+)?(?:[eE][-+]?\d+)?").unwrap());
    number.find(line).and_then(|m| m.as_str().parse().ok())
}

/// Replaces tabs with spaces up to each tab stop.
pub(crate) fn expand_tabs(line: &str, tab_width: usize) -> String {
    let tab_width = tab_width.max(1);
    let mut out = String::with_capacity(line.len());
    let mut column = 0;
    for grapheme in line.graphemes(true) {
        if grapheme == "\t" {
            let spaces = tab_width - column % tab_width;
            out.extend(std::iter::repeat_n(' ', spaces));
            column += spaces;
        } else {
            out.push_str(grapheme);
            column += wrap::cell_width(grapheme);
        }
    }
    out
}

/// Rewrites a line's indentation as tabs (plus any leftover spaces).
pub(crate) fn leading_spaces_to_tabs(line: &str, tab_width: usize) -> String {
    let tab_width = tab_width.max(1);
    let indent_len = line.len() - line.trim_start_matches([' ', '\t']).len();
    let (indent, rest) = line.split_at(indent_len);
    let mut columns = 0;
    for c in indent.chars() {
        columns = if c == '\t' { columns + tab_width - columns % tab_width } else { columns + 1 };
    }
    let mut out = "\t".repeat(columns / tab_width);
    out.push_str(&" ".repeat(columns % tab_width));
    out.push_str(rest);
    out
}

/// Hard-wraps a line at `width` cells, breaking after spaces where possible.
pub(crate) fn split_line(line: &str, width: usize, tab_width: usize) -> Vec<String> {
    let (breaks, _) = wrap::wrap_str(line, width, tab_width, false);
    if breaks.is_empty() {
        return vec![line.to_string()];
    }
    let units: Vec<u16> = line.encode_utf16().collect();
    let mut pieces = Vec::with_capacity(breaks.len() + 1);
    let mut start = 0;
    for &end in breaks.iter().chain(std::iter::once(&(units.len() as u32))) {
        let piece = String::from_utf16_lossy(&units[start..end as usize]);
        pieces.push(piece.trim_end().to_string());
        start = end as usize;
    }
    pieces
}

pub(crate) fn change_case(text: &str, change: CaseChange) -> String {
    match change {
        CaseChange::Upper => text.to_uppercase(),
        CaseChange::Lower => text.to_lowercase(),
        CaseChange::Swap => text
            .chars()
            .flat_map(|c| -> Box<dyn Iterator<Item = char>> {
                if c.is_uppercase() {
                    Box::new(c.to_lowercase())
                } else if c.is_lowercase() {
                    Box::new(c.to_uppercase())
                } else {
                    Box::new(std::iter::once(c))
                }
            })
            .collect(),
        CaseChange::Title => {
            let mut out = String::with_capacity(text.len());
            for word in text.split_word_bounds() {
                let mut chars = word.chars();
                match chars.next() {
                    Some(first) if first.is_alphabetic() => {
                        out.extend(first.to_uppercase());
                        out.push_str(&chars.as_str().to_lowercase());
                    }
                    _ => out.push_str(word),
                }
            }
            out
        }
        CaseChange::Sentence => {
            let mut out = String::with_capacity(text.len());
            let mut start_of_sentence = true;
            for c in text.chars() {
                if c.is_alphabetic() {
                    if start_of_sentence {
                        out.extend(c.to_uppercase());
                        start_of_sentence = false;
                    } else {
                        out.extend(c.to_lowercase());
                    }
                } else {
                    out.push(c);
                    if matches!(c, '.' | '!' | '?' | '\n' | '\r') {
                        start_of_sentence = true;
                    }
                }
            }
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use LineOperation::*;

    fn run(operation: LineOperation, lines: &[&str]) -> Vec<String> {
        apply_to_lines(operation, lines.iter().map(|s| s.to_string()).collect(), 4)
    }

    #[test]
    fn sorts() {
        assert_eq!(run(SortAscending, &["b", "a", "C"]), ["C", "a", "b"]);
        assert_eq!(run(SortDescending, &["b", "a", "c"]), ["c", "b", "a"]);
        assert_eq!(run(SortCaseInsensitive, &["b", "a", "C"]), ["a", "b", "C"]);
        assert_eq!(run(SortNumeric, &["item 10", "item 9", "none", "-3 x", "2.5"]), ["none", "-3 x", "2.5", "item 9", "item 10"]);
    }

    #[test]
    fn removes_and_dedupes() {
        assert_eq!(run(RemoveDuplicates, &["a", "b", "a", "c", "b"]), ["a", "b", "c"]);
        assert_eq!(run(RemoveConsecutiveDuplicates, &["a", "a", "b", "a"]), ["a", "b", "a"]);
        assert_eq!(run(RemoveEmpty, &["a", "", "  ", "b"]), ["a", "  ", "b"]);
        assert_eq!(run(RemoveBlank, &["a", "", "  ", "b"]), ["a", "b"]);
        assert_eq!(run(Reverse, &["1", "2", "3"]), ["3", "2", "1"]);
    }

    #[test]
    fn joins_with_single_spaces() {
        assert_eq!(run(Join, &["  let x = ", "    1 +", "", "  2;"]), ["  let x = 1 + 2;"]);
    }

    #[test]
    fn whitespace_tools() {
        assert_eq!(run(TrimTrailing, &["a  ", " b\t"]), ["a", " b"]);
        assert_eq!(run(TrimLeading, &["  a ", "\tb"]), ["a ", "b"]);
        assert_eq!(run(TabsToSpaces, &["\tx\ty", "ab\tc"]), ["    x   y", "ab  c"]);
        assert_eq!(run(LeadingSpacesToTabs, &["      x", "  \ty", "z"]), ["\t  x", "\ty", "z"]);
    }

    #[test]
    fn splits_long_lines_at_spaces() {
        assert_eq!(split_line("one two three four", 9, 4), ["one two", "three", "four"]);
        assert_eq!(split_line("short", 80, 4), ["short"]);
    }

    #[test]
    fn changes_case() {
        assert_eq!(change_case("héllo World", CaseChange::Upper), "HÉLLO WORLD");
        assert_eq!(change_case("Straße", CaseChange::Upper), "STRASSE");
        assert_eq!(change_case("hELLO wORLD-wide web", CaseChange::Title), "Hello World-Wide Web");
        assert_eq!(change_case("HELLO. HOW ARE YOU? fine", CaseChange::Sentence), "Hello. How are you? Fine");
        assert_eq!(change_case("aBc", CaseChange::Swap), "AbC");
    }
}
