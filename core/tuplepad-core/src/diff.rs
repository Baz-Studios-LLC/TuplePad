//! Compare: a side-by-side line diff with the changed characters inside changed lines.

use similar::{Algorithm, DiffOp};

use crate::search::MatchSpan;

#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffKind {
    Same,
    /// A line on both sides that differs; its spans mark what changed.
    Changed,
    /// Only on the right.
    Added,
    /// Only on the left.
    Removed,
}

/// One row of the side-by-side view. A side is nil where the other side has a line it lacks.
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct DiffRow {
    pub kind: DiffKind,
    /// 0-based line numbers.
    pub left_line: Option<u64>,
    pub right_line: Option<u64>,
    pub left_text: String,
    pub right_text: String,
    /// What changed within a changed line, in UTF-16 columns.
    pub left_spans: Vec<MatchSpan>,
    pub right_spans: Vec<MatchSpan>,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct DiffResult {
    pub rows: Vec<DiffRow>,
    /// The row where each run of changes starts, for jumping between them.
    pub changes: Vec<u64>,
    pub added: u64,
    pub removed: u64,
    pub changed: u64,
}

#[derive(uniffi::Record, Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiffOptions {
    pub ignore_whitespace: bool,
    pub ignore_case: bool,
}

fn split_lines(text: &str) -> Vec<&str> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut lines: Vec<&str> = text.split('\n').map(|line| line.strip_suffix('\r').unwrap_or(line)).collect();
    if text.ends_with('\n') {
        lines.pop();
    }
    lines
}

/// Compares two texts line by line.
#[uniffi::export]
pub fn diff_texts(left: String, right: String, options: DiffOptions) -> DiffResult {
    let old = split_lines(&left);
    let new = split_lines(&right);
    let key = |line: &&str| -> String {
        let mut key = if options.ignore_whitespace { line.split_whitespace().collect::<Vec<_>>().join(" ") } else { line.to_string() };
        if options.ignore_case {
            key = key.to_lowercase();
        }
        key
    };
    let old_keys: Vec<String> = old.iter().map(key).collect();
    let new_keys: Vec<String> = new.iter().map(key).collect();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let ops = similar::capture_diff_slices_deadline(Algorithm::Patience, &old_keys, &new_keys, Some(deadline));

    let mut result = DiffResult { rows: Vec::new(), changes: Vec::new(), added: 0, removed: 0, changed: 0 };
    let row = |kind, l: Option<usize>, r: Option<usize>| DiffRow {
        kind,
        left_line: l.map(|i| i as u64),
        right_line: r.map(|i| i as u64),
        left_text: l.map(|i| old[i].to_string()).unwrap_or_default(),
        right_text: r.map(|i| new[i].to_string()).unwrap_or_default(),
        left_spans: Vec::new(),
        right_spans: Vec::new(),
    };
    let mut in_change = false;
    for op in ops {
        let is_change = !matches!(op, DiffOp::Equal { .. });
        if is_change && !in_change {
            result.changes.push(result.rows.len() as u64);
        }
        in_change = is_change;
        match op {
            DiffOp::Equal { old_index, new_index, len } => {
                for i in 0..len {
                    result.rows.push(row(DiffKind::Same, Some(old_index + i), Some(new_index + i)));
                }
            }
            DiffOp::Delete { old_index, old_len, .. } => {
                result.removed += old_len as u64;
                for i in 0..old_len {
                    result.rows.push(row(DiffKind::Removed, Some(old_index + i), None));
                }
            }
            DiffOp::Insert { new_index, new_len, .. } => {
                result.added += new_len as u64;
                for i in 0..new_len {
                    result.rows.push(row(DiffKind::Added, None, Some(new_index + i)));
                }
            }
            DiffOp::Replace { old_index, old_len, new_index, new_len } => {
                let paired = old_len.min(new_len);
                for i in 0..paired {
                    let mut changed = row(DiffKind::Changed, Some(old_index + i), Some(new_index + i));
                    (changed.left_spans, changed.right_spans) = inline_changes(old[old_index + i], new[new_index + i]);
                    result.rows.push(changed);
                }
                result.changed += paired as u64;
                for i in paired..old_len {
                    result.removed += 1;
                    result.rows.push(row(DiffKind::Removed, Some(old_index + i), None));
                }
                for i in paired..new_len {
                    result.added += 1;
                    result.rows.push(row(DiffKind::Added, None, Some(new_index + i)));
                }
            }
        }
    }
    result
}

/// The words and symbols that differ between two versions of a line, as UTF-16 ranges on each side.
/// Whole words are marked (not single letters within them), and lines that have little in common
/// are marked whole.
fn inline_changes(old: &str, new: &str) -> (Vec<MatchSpan>, Vec<MatchSpan>) {
    let whole = |text: &str| vec![MatchSpan { start: 0, end: text.encode_utf16().count() as u32 }];
    if old.len() + new.len() > 4_000 {
        return (whole(old), whole(new));
    }
    let (old_tokens, new_tokens) = (tokens(old), tokens(new));
    let ops = similar::capture_diff_slices(Algorithm::Myers, &old_tokens, &new_tokens);
    if similar::get_diff_ratio(&ops, old_tokens.len(), new_tokens.len()) < 0.35 {
        return (whole(old), whole(new));
    }
    // Where each token starts, in UTF-16 units.
    let offsets = |tokens: &[&str]| {
        let mut at = 0u32;
        let mut offsets = vec![0];
        for token in tokens {
            at += token.encode_utf16().count() as u32;
            offsets.push(at);
        }
        offsets
    };
    let (old_at, new_at) = (offsets(&old_tokens), offsets(&new_tokens));
    let (mut left, mut right) = (Vec::new(), Vec::new());
    for op in &ops {
        let (tag, old_range, new_range) = op.as_tag_tuple();
        if tag == similar::DiffTag::Equal {
            continue;
        }
        if !old_range.is_empty() {
            push_span(&mut left, old_at[old_range.start], old_at[old_range.end]);
        }
        if !new_range.is_empty() {
            push_span(&mut right, new_at[new_range.start], new_at[new_range.end]);
        }
    }
    (left, right)
}

/// A line split into words, runs of spaces, and single symbols.
fn tokens(text: &str) -> Vec<&str> {
    let mut tokens = Vec::new();
    let (mut start, mut kind) = (0, 0);
    for (index, c) in text.char_indices() {
        let this = if c.is_alphanumeric() || c == '_' { 1 } else if c.is_whitespace() { 2 } else { 3 };
        if this != kind || this == 3 {
            if index > start {
                tokens.push(&text[start..index]);
            }
            (start, kind) = (index, this);
        }
    }
    if start < text.len() {
        tokens.push(&text[start..]);
    }
    tokens
}

fn push_span(spans: &mut Vec<MatchSpan>, start: u32, end: u32) {
    match spans.last_mut() {
        Some(last) if last.end == start => last.end = end,
        _ => spans.push(MatchSpan { start, end }),
    }
}
