//! Maps document lines to visual rows. Without word wrap every line is one row. With it, a line
//! can take several, and a table of row counts (with running totals) says where each line starts.

pub struct RowMap {
    line_count: u64,
    /// Rows per line; empty when every line is one row.
    rows: Vec<u32>,
    /// `starts[i]` is the first row of line `i`; one more entry than there are lines.
    starts: Vec<u64>,
    stale: bool,
}

impl RowMap {
    pub fn plain(line_count: u64) -> Self {
        RowMap { line_count: line_count.max(1), rows: Vec::new(), starts: Vec::new(), stale: false }
    }

    pub fn wrapped(rows: Vec<u32>) -> Self {
        let rows = if rows.is_empty() { vec![1] } else { rows };
        RowMap { line_count: rows.len() as u64, rows, starts: Vec::new(), stale: true }
    }

    /// Hides the lines inside folds (each fold's `start` stays visible): they get no rows.
    pub fn hide(&mut self, folds: &[tuplepad_core::FoldRange]) {
        if folds.is_empty() {
            return;
        }
        if self.rows.is_empty() {
            self.rows = vec![1; self.line_count as usize];
        }
        let last = self.rows.len().saturating_sub(1);
        for fold in folds {
            let (start, end) = ((fold.start as usize + 1).min(last + 1), (fold.end as usize).min(last));
            if start <= end {
                self.rows[start..=end].fill(0);
            }
        }
        self.stale = true;
    }

    pub fn is_wrapped(&self) -> bool {
        !self.rows.is_empty()
    }

    pub fn line_count(&self) -> u64 {
        self.line_count
    }

    /// Replaces the row counts of lines `start..start + old_len` with `new_rows`.
    pub fn splice(&mut self, start: u64, old_len: u64, new_rows: Vec<u32>) {
        if !self.is_wrapped() {
            self.line_count = (self.line_count + new_rows.len() as u64).saturating_sub(old_len).max(1);
            return;
        }
        let start = (start as usize).min(self.rows.len());
        let end = (start + old_len as usize).min(self.rows.len());
        self.rows.splice(start..end, new_rows);
        if self.rows.is_empty() {
            self.rows.push(1);
        }
        self.line_count = self.rows.len() as u64;
        self.stale = true;
    }

    fn refresh(&mut self) {
        if !self.stale {
            return;
        }
        self.starts.clear();
        self.starts.reserve(self.rows.len() + 1);
        let mut total = 0u64;
        for &count in &self.rows {
            self.starts.push(total);
            total += count as u64;
        }
        self.starts.push(total);
        self.stale = false;
    }

    pub fn total_rows(&mut self) -> u64 {
        if !self.is_wrapped() {
            return self.line_count;
        }
        self.refresh();
        *self.starts.last().unwrap_or(&1)
    }

    pub fn first_row(&mut self, line: u64) -> u64 {
        if !self.is_wrapped() {
            return line.min(self.line_count - 1);
        }
        self.refresh();
        self.starts[(line as usize).min(self.rows.len() - 1)]
    }

    pub fn rows_of(&self, line: u64) -> u32 {
        self.rows.get(line as usize).copied().unwrap_or(1)
    }

    /// The line holding a visual row, from the table as it is (it may be a moment out of date).
    pub fn line_at_stale(&self, row: u64) -> u64 {
        if !self.is_wrapped() || self.starts.is_empty() {
            return row.min(self.line_count.saturating_sub(1));
        }
        (self.starts.partition_point(|&start| start <= row).saturating_sub(1) as u64).min(self.line_count.saturating_sub(1))
    }

    /// The line holding a visual row, and which of its rows it is.
    pub fn line_at(&mut self, row: u64) -> (u64, u32) {
        if !self.is_wrapped() {
            return (row.min(self.line_count - 1), 0);
        }
        self.refresh();
        let total = *self.starts.last().unwrap();
        if row >= total {
            let last = self.rows.len() - 1;
            return (last as u64, self.rows[last].saturating_sub(1));
        }
        let line = self.starts.partition_point(|&start| start <= row) - 1;
        (line as u64, (row - self.starts[line]) as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapped_rows() {
        let mut map = RowMap::wrapped(vec![1, 3, 1, 2]);
        assert_eq!(map.total_rows(), 7);
        assert_eq!(map.first_row(2), 4);
        assert_eq!(map.line_at(0), (0, 0));
        assert_eq!(map.line_at(3), (1, 2));
        assert_eq!(map.line_at(4), (2, 0));
        assert_eq!(map.line_at(6), (3, 1));
        assert_eq!(map.line_at(99), (3, 1));
        map.splice(1, 1, vec![1, 1]);
        assert_eq!(map.total_rows(), 6);
        assert_eq!(map.line_at(2), (2, 0));
        // Folded lines have no rows; their rows belong to the next line that shows.
        let mut folded = RowMap::plain(6);
        folded.hide(&[tuplepad_core::FoldRange { start: 1, end: 3 }]);
        assert_eq!(folded.total_rows(), 4);
        assert_eq!(folded.rows_of(2), 0);
        assert_eq!(folded.first_row(4), 2);
        assert_eq!(folded.line_at(1), (1, 0));
        assert_eq!(folded.line_at(2), (4, 0));
        assert_eq!(folded.first_row(3), 2);
        let mut plain = RowMap::plain(10);
        assert_eq!(plain.line_at(4), (4, 0));
        plain.splice(2, 1, vec![1, 1, 1]);
        assert_eq!(plain.total_rows(), 12);
    }
}
