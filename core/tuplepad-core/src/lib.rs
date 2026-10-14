//! TuplePad's text engine. Swift talks to it through UniFFI-generated bindings.

uniffi::setup_scaffolding!();

mod buffer;
mod encoding;
mod error;
mod history;
mod search;
mod syntax;
mod types;

pub use buffer::{SearchResults, TextBuffer};
pub use encoding::{TextEncoding, supported_encodings};
pub use error::CoreError;
pub use search::SearchQuery;
pub use syntax::{HighlightSpan, HighlightStyle, LanguageInfo, detect_language, languages};
pub use types::{EditKind, FileInfo, LineEnding, TextPos, TextSelection, line_ending_text};

#[cfg(test)]
mod tests;
