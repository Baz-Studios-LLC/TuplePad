//! Snippets: expanding a body like `fn ${1:name}($2) {\n\t$0\n}` into text and tab stops.

/// Where a tab stop's text is, relative to where the snippet is inserted: a line of the inserted
/// text, and UTF-16 columns within it (line 0's columns count from the insertion point).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SnippetRange {
    pub line: u32,
    pub start: u32,
    pub end: u32,
}

/// A tab stop: every place it appears (typing goes into all of them).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnippetStop {
    pub ranges: Vec<SnippetRange>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnippetExpansion {
    pub text: String,
    /// In the order Tab visits them: $1, $2, … and $0 last (or the end of the snippet).
    pub stops: Vec<SnippetStop>,
}

/// Expands a snippet body. Lines after the first get `indent` in front, and tabs at the start of
/// a body line become `indent_unit`.
pub fn expand_snippet(body: String, indent: String, indent_unit: String) -> SnippetExpansion {
    let mut text = String::new();
    // (stop number, start offset, end offset) in UTF-16 units of `text`.
    let mut found: Vec<(u32, usize, usize)> = Vec::new();
    let mut units = 0usize;
    let push = |text: &mut String, units: &mut usize, s: &str| {
        text.push_str(s);
        *units += s.encode_utf16().count();
    };
    let chars: Vec<char> = body.chars().collect();
    let mut i = 0;
    let mut at_line_start = true;
    while i < chars.len() {
        let c = chars[i];
        if at_line_start && c == '\t' {
            push(&mut text, &mut units, &indent_unit);
            i += 1;
            continue;
        }
        at_line_start = false;
        match c {
            '\\' if matches!(chars.get(i + 1), Some('$' | '\\' | '}')) => {
                push(&mut text, &mut units, &chars[i + 1].to_string());
                i += 2;
            }
            '\n' => {
                push(&mut text, &mut units, "\n");
                push(&mut text, &mut units, &indent);
                at_line_start = true;
                i += 1;
            }
            '$' => {
                if let Some((number, default, next)) = parse_stop(&chars, i) {
                    let start = units;
                    push(&mut text, &mut units, &default);
                    found.push((number, start, units));
                    i = next;
                } else {
                    push(&mut text, &mut units, "$");
                    i += 1;
                }
            }
            _ => {
                push(&mut text, &mut units, &c.to_string());
                i += 1;
            }
        }
    }
    // Tab order: 1, 2, … then 0; with no $0 the caret ends after the snippet.
    let mut numbers: Vec<u32> = found.iter().map(|f| f.0).filter(|&n| n > 0).collect();
    numbers.sort_unstable();
    numbers.dedup();
    numbers.push(0);
    if !found.iter().any(|f| f.0 == 0) {
        found.push((0, units, units));
    }
    let to_range = |start: usize, end: usize| {
        // Offsets to (line, column) within the inserted text.
        let before: Vec<u16> = text.encode_utf16().take(start).collect();
        let line = before.iter().filter(|&&u| u == '\n' as u16).count() as u32;
        let line_start = before.iter().rposition(|&u| u == '\n' as u16).map_or(0, |p| p + 1);
        SnippetRange { line, start: (start - line_start) as u32, end: (end - line_start) as u32 }
    };
    let stops = numbers
        .into_iter()
        .map(|number| SnippetStop {
            ranges: found.iter().filter(|f| f.0 == number).map(|&(_, s, e)| to_range(s, e)).collect(),
        })
        .collect();
    SnippetExpansion { text, stops }
}

/// `$1`, `${1}` or `${1:default}` at `i`: the number, the default text and where parsing ends.
fn parse_stop(chars: &[char], i: usize) -> Option<(u32, String, usize)> {
    let digits = |from: usize| chars[from..].iter().take_while(|c| c.is_ascii_digit()).count();
    if chars.get(i + 1) == Some(&'{') {
        let count = digits(i + 2);
        if count == 0 {
            return None;
        }
        let number = chars[i + 2..i + 2 + count].iter().collect::<String>().parse().ok()?;
        let mut j = i + 2 + count;
        let mut default = String::new();
        if chars.get(j) == Some(&':') {
            j += 1;
            let mut depth = 0;
            while j < chars.len() {
                match chars[j] {
                    '\\' if j + 1 < chars.len() => {
                        default.push(chars[j + 1]);
                        j += 2;
                        continue;
                    }
                    '{' => depth += 1,
                    '}' if depth == 0 => break,
                    '}' => depth -= 1,
                    _ => {}
                }
                default.push(chars[j]);
                j += 1;
            }
        }
        if chars.get(j) != Some(&'}') {
            return None;
        }
        Some((number, default, j + 1))
    } else {
        let count = digits(i + 1);
        if count == 0 {
            return None;
        }
        let number = chars[i + 1..i + 1 + count].iter().collect::<String>().parse().ok()?;
        Some((number, String::new(), i + 1 + count))
    }
}

/// The snippet trigger word just before a caret (letters, digits, `_` and `-`), and the UTF-16
/// column where it starts.
pub fn snippet_trigger(line: &str, column: u64) -> Option<(u64, String)> {
    let units: Vec<u16> = line.encode_utf16().collect();
    let end = (column as usize).min(units.len());
    let before = String::from_utf16_lossy(&units[..end]);
    let word: String = before.chars().rev().take_while(|&c| c.is_alphanumeric() || c == '_' || c == '-').collect::<Vec<_>>().into_iter().rev().collect();
    if word.is_empty() {
        return None;
    }
    let start = end - word.encode_utf16().count();
    Some((start as u64, word))
}

/// While filling in a snippet: after typing into the current stop (whose ranges were `current`
/// and whose carets are now `carets`), later stops on the same lines move along by however much
/// the current one grew or shrank.
pub fn shift_snippet_stops(
    stops: Vec<Vec<crate::TextSelection>>,
    current: usize,
    carets: &[crate::TextSelection],
) -> Vec<Vec<crate::TextSelection>> {
    use crate::{TextPos, TextSelection};
    let mut stops = stops;
    let Some(ranges) = stops.get(current).cloned() else { return stops };
    let mut ranges = ranges;
    ranges.sort_by_key(|r| r.start());
    let mut carets = carets.to_vec();
    carets.sort_by_key(|c| c.start());
    if ranges.len() != carets.len() {
        return stops;
    }
    // Shifts accumulate along a line: work from the right so earlier edits don't move later ones.
    for (range, caret) in ranges.iter().zip(&carets).rev() {
        if caret.end().line != range.end().line {
            continue;
        }
        let shift = caret.end().column as i64 - range.end().column as i64;
        if shift == 0 {
            continue;
        }
        let moved = |p: TextPos| TextPos::new(p.line, (p.column as i64 + shift).max(0) as u64);
        for (index, stop) in stops.iter_mut().enumerate() {
            if index == current {
                continue;
            }
            for other in stop.iter_mut() {
                if other.start().line == range.end().line && other.start().column >= range.end().column {
                    *other = TextSelection { anchor: moved(other.anchor), head: moved(other.head) };
                }
            }
        }
    }
    stops[current] = carets;
    stops
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{TextPos, TextSelection};

    fn sel(line: u64, start: u64, end: u64) -> TextSelection {
        TextSelection { anchor: TextPos::new(line, start), head: TextPos::new(line, end) }
    }

    #[test]
    fn triggers_before_the_caret() {
        assert_eq!(snippet_trigger("    fn", 6), Some((4, "fn".to_string())));
        assert_eq!(snippet_trigger("x = my-snip", 11), Some((4, "my-snip".to_string())));
        assert_eq!(snippet_trigger("fn ", 3), None);
        assert_eq!(snippet_trigger("été", 3), Some((0, "été".to_string())));
    }

    #[test]
    fn later_stops_follow_typing() {
        // `fn name(args) {` with stop 1 at "name" (3..7) and stop 2 at "args" (8..12).
        let stops = vec![vec![sel(0, 3, 7)], vec![sel(0, 8, 12)], vec![sel(1, 4, 4)]];
        // "name" was replaced by "go": the caret ends at column 5.
        let shifted = shift_snippet_stops(stops, 0, &[TextSelection::caret(TextPos::new(0, 5))]);
        assert_eq!(shifted[1], vec![sel(0, 6, 10)]);
        assert_eq!(shifted[2], vec![sel(1, 4, 4)]);
        assert_eq!(shifted[0], vec![TextSelection::caret(TextPos::new(0, 5))]);
    }
}
