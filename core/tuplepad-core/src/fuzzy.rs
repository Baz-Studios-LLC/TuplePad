//! Fuzzy matching for the command palette and quick open: the query's characters must appear
//! in order in the candidate, and matches at word starts and in runs score higher.

/// One candidate that matched, with where.
#[derive(Debug, Clone, PartialEq)]
pub struct FuzzyHit {
    /// Index into the candidates passed in.
    pub index: u32,
    pub score: i32,
    /// UTF-16 offsets of the matched characters in the candidate, for highlighting.
    pub positions: Vec<u32>,
}

const MATCH: i32 = 16;
const WORD_START: i32 = 24;
const FIRST_CHARACTER: i32 = 8;
const CONSECUTIVE: i32 = 20;
const GAP: i32 = 1;
const LEADING_GAP_LIMIT: i32 = 12;
const EXACT_CASE: i32 = 1;
/// Jumping into the middle of a word is a weak signal, so it costs more than a plain gap.
const SCATTERED: i32 = 18;

/// Ranks candidates against a query, best first, keeping at most `limit`. Hits far weaker
/// than the best one are dropped as noise. An empty query matches everything in the original order.
pub fn fuzzy_rank(query: String, candidates: Vec<String>, limit: u32) -> Vec<FuzzyHit> {
    let limit = limit as usize;
    let query: Vec<char> = query.chars().filter(|c| !c.is_whitespace()).collect();
    if query.is_empty() {
        return (0..candidates.len().min(limit))
            .map(|index| FuzzyHit { index: index as u32, score: 0, positions: Vec::new() })
            .collect();
    }
    let mut hits: Vec<(FuzzyHit, usize)> = candidates
        .iter()
        .enumerate()
        .filter_map(|(index, candidate)| {
            score(&query, candidate).map(|(score, positions)| {
                (FuzzyHit { index: index as u32, score, positions }, candidate.chars().count())
            })
        })
        .collect();
    // Higher score first; then shorter candidates; then the original order.
    hits.sort_by(|(a, a_len), (b, b_len)| {
        b.score.cmp(&a.score).then(a_len.cmp(b_len)).then(a.index.cmp(&b.index))
    });
    if let Some(best) = hits.first().map(|(hit, _)| hit.score).filter(|&best| best > 0) {
        hits.retain(|(hit, _)| hit.score * 2 >= best);
    }
    hits.truncate(limit);
    hits.into_iter().map(|(hit, _)| hit).collect()
}

/// The best alignment of `query` in `candidate`, as a score and the UTF-16 offsets matched.
pub(crate) fn score(query: &[char], candidate: &str) -> Option<(i32, Vec<u32>)> {
    let chars: Vec<char> = candidate.chars().collect();
    let (n, m) = (query.len(), chars.len());
    if n == 0 || n > m {
        return if n == 0 { Some((0, Vec::new())) } else { None };
    }
    let folded: Vec<char> = chars.iter().map(|&c| fold(c)).collect();
    let wanted: Vec<char> = query.iter().map(|&c| fold(c)).collect();

    // Quick rejection: the query must be a subsequence.
    let mut next = 0;
    for &c in &folded {
        if next < n && c == wanted[next] {
            next += 1;
        }
    }
    if next < n {
        return None;
    }

    let bonus: Vec<i32> = (0..m).map(|j| position_bonus(&chars, j)).collect();
    const NONE: i32 = i32::MIN / 2;
    // best[i][j]: the best score with query[..=i] matched and query[i] at candidate[j].
    // from[i][j]: where query[i - 1] matched on that best path.
    let mut best = vec![NONE; n * m];
    let mut from = vec![usize::MAX; n * m];
    for i in 0..n {
        // The best earlier match of query[i - 1], allowing for the gap since it.
        let mut carry = NONE;
        let mut carry_at = usize::MAX;
        for j in i..m {
            if i > 0 && j >= 2 {
                // Skipping the character at j - 1 costs one gap.
                let previous = best[(i - 1) * m + j - 2] - GAP;
                if carry > NONE {
                    carry -= GAP;
                }
                if previous > carry {
                    carry = previous;
                    carry_at = j - 2;
                }
            }
            if folded[j] != wanted[i] {
                continue;
            }
            let mut value = MATCH + bonus[j] + if chars[j] == query[i] { EXACT_CASE } else { 0 };
            let scattered = if bonus[j] == 0 { SCATTERED } else { 0 };
            if i == 0 {
                value -= (j as i32).min(LEADING_GAP_LIMIT) + scattered;
                best[j] = value;
                continue;
            }
            let adjacent = best[(i - 1) * m + j - 1];
            let (base, at) = if adjacent > NONE && adjacent + CONSECUTIVE >= carry - scattered {
                (adjacent + CONSECUTIVE, j - 1)
            } else {
                (carry - scattered, carry_at)
            };
            if base > NONE / 2 {
                best[i * m + j] = base + value;
                from[i * m + j] = at;
            }
        }
    }

    let last = (n - 1) * m;
    let (end, total) = (0..m).map(|j| (j, best[last + j])).max_by_key(|&(j, value)| (value, std::cmp::Reverse(j)))?;
    if total <= NONE {
        return None;
    }
    let mut matched = vec![0usize; n];
    let mut j = end;
    for i in (0..n).rev() {
        matched[i] = j;
        if i > 0 {
            j = from[i * m + j];
        }
    }
    // Character indices to UTF-16 offsets.
    let mut offsets = Vec::with_capacity(m);
    let mut offset = 0u32;
    for c in &chars {
        offsets.push(offset);
        offset += c.len_utf16() as u32;
    }
    Some((total, matched.into_iter().map(|j| offsets[j]).collect()))
}

/// Extra points for matching where a word starts.
fn position_bonus(chars: &[char], j: usize) -> i32 {
    if j == 0 {
        return WORD_START + FIRST_CHARACTER;
    }
    let (previous, current) = (chars[j - 1], chars[j]);
    let separator = !previous.is_alphanumeric();
    let camel = previous.is_lowercase() && current.is_uppercase();
    let digit_start = !previous.is_ascii_digit() && current.is_ascii_digit();
    if separator || camel || digit_start { WORD_START } else { 0 }
}

fn fold(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}
