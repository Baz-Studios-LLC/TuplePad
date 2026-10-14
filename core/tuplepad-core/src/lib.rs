//! TuplePad's text engine. Swift talks to it through UniFFI-generated bindings.

uniffi::setup_scaffolding!();

mod buffer;
mod encoding;
mod error;
mod folder_search;
mod fuzzy;
mod history;
mod search;
mod syntax;
mod transform;
mod types;
mod wrap;

pub use buffer::{AppendResult, BracketMatch, DiskStatus, LineResults, SearchResults, TextBuffer};
pub use encoding::{TextEncoding, supported_encodings};
pub use error::CoreError;
pub use folder_search::{FileMatches, FolderSearch, FolderSearchOptions, FolderSearchProgress, preview_file_replacements, replace_in_file};
pub use fuzzy::{FuzzyHit, fuzzy_rank};
pub use search::{LineMatches, MatchSpan, ReplacementPreview, SearchQuery};
pub use transform::{CaseChange, LineOperation};
pub use wrap::WrappedLine;
pub use syntax::{HighlightSpan, HighlightStyle, LanguageInfo, detect_language, languages};
pub use types::{EditKind, FileInfo, LineEnding, TextPos, TextSelection, line_ending_text};

#[cfg(test)]
mod tests;
