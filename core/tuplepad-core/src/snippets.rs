//! Snippets: expanding a body like `fn ${1:name}($2) {\n\t$0\n}` into text and tab stops.

/// Where a tab stop's text is, relative to where the snippet is inserted: a line of the inserted
/// text, and UTF-16 columns within it (line 0's columns count from the insertion point).
#[derive(uniffi::Record, Clone, Copy, Debug, PartialEq, Eq)]
pub struct SnippetRange {
    pub line: u32,
    pub start: u32,
    pub end: u32,
}

/// A tab stop: every place it appears (typing goes into all of them).
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct SnippetStop {
    pub ranges: Vec<SnippetRange>,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct SnippetExpansion {
    pub text: String,
    /// In the order Tab visits them: $1, $2, … and $0 last (or the end of the snippet).
    pub stops: Vec<SnippetStop>,
}

/// Expands a snippet body. Lines after the first get `indent` in front, and tabs at the start of
/// a body line become `indent_unit`.
#[uniffi::export]
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
