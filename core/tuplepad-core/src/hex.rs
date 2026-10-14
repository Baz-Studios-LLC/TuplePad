//! The hex view: a file's bytes, 16 to a row, read a window at a time.

use std::io::{Read, Seek, SeekFrom};

use crate::error::{CoreError, CoreResult};

#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct HexRow {
    pub offset: u64,
    /// "48 65 6c 6c 6f  2c 20 77 …" with a gap after eight bytes.
    pub hex: String,
    /// The bytes as ASCII, with "." for anything unprintable.
    pub ascii: String,
}

pub const BYTES_PER_ROW: u64 = 16;

/// `count` rows of a file starting at row `first_row`.
#[uniffi::export]
pub fn hex_rows(path: String, first_row: u64, count: u64) -> CoreResult<Vec<HexRow>> {
    let mut file = std::fs::File::open(&path).map_err(|e| CoreError::Io { message: format!("{path}: {e}") })?;
    let start = first_row * BYTES_PER_ROW;
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::with_capacity((count * BYTES_PER_ROW) as usize);
    file.take(count * BYTES_PER_ROW).read_to_end(&mut bytes)?;
    Ok(bytes
        .chunks(BYTES_PER_ROW as usize)
        .enumerate()
        .map(|(index, row)| {
            let mut hex = String::with_capacity(50);
            for (i, byte) in row.iter().enumerate() {
                if i == 8 {
                    hex.push(' ');
                }
                if i > 0 {
                    hex.push(' ');
                }
                hex.push_str(&format!("{byte:02x}"));
            }
            let ascii = row.iter().map(|&b| if (0x20..0x7f).contains(&b) { b as char } else { '.' }).collect();
            HexRow { offset: start + index as u64 * BYTES_PER_ROW, hex, ascii }
        })
        .collect())
}

/// A file's size in bytes.
#[uniffi::export]
pub fn file_size(path: String) -> CoreResult<u64> {
    Ok(std::fs::metadata(&path).map_err(|e| CoreError::Io { message: format!("{path}: {e}") })?.len())
}
