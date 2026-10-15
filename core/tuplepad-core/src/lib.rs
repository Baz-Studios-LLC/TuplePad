//! TuplePad's text engine. Swift talks to it through UniFFI-generated bindings.

uniffi::setup_scaffolding!();

mod buffer;
mod codec;
mod diff;
mod editing;
mod encoding;
mod error;
mod folder_search;
mod fuzzy;
mod hex;
mod history;
mod json_tools;
mod markdown;
mod spell;
mod outline;
mod search;
mod session;
mod snippets;
mod syntax;
mod transform;
mod types;
mod wrap;
mod xml_tools;

pub use buffer::{AppendResult, BracketMatch, ColumnInsert, DiskStatus, FoldRange, LineResults, NumberBase, NumberPadding, SearchResults, TextBuffer, column_values};
pub use encoding::{TextEncoding, supported_encodings};
pub use codec::{Codec, HashAlgorithm, decode_text, encode_text, hash_file, hash_text};
pub use editing::{Indentation, Motion, normalize_cursors, rank_completions};
pub use diff::{DiffKind, DiffOptions, DiffResult, DiffRow, diff_texts};
pub use error::CoreError;
pub use hex::{HexRow, file_size, hex_rows};
pub use spell::{prepare_spelling, spelled_right, spelling_ready, spelling_suggestions};
pub use markdown::{MarkdownBlock, MarkdownBlockKind, highlight_code, markdown_blocks, markdown_html};
pub use json_tools::{JsonKind, JsonTreeItem, json_format, json_minify, json_path_at, json_tree, json_validate};
pub use xml_tools::{XPathItem, xml_format, xml_linearize, xml_validate, xml_xpath};
pub use folder_search::{FileMatches, FolderSearch, FolderSearchOptions, FolderSearchProgress, list_files, preview_file_replacements, replace_in_file};
pub use fuzzy::{FuzzyHit, fuzzy_rank};
pub use outline::{OutlineItem, SymbolKind};
pub use snippets::{SnippetExpansion, SnippetRange, SnippetStop, expand_snippet, shift_snippet_stops, snippet_trigger};
pub use session::{DocumentRecord, SessionRecord, SessionStore, new_document_id, now_seconds};
pub use search::{LineMatches, MatchSpan, ReplacementPreview, SearchQuery};
pub use transform::{CaseChange, LineOperation};
pub use wrap::WrappedLine;
pub use syntax::{HighlightSpan, HighlightStyle, LanguageInfo, detect_language, languages};
pub use types::{EditKind, FileInfo, LineEnding, TextPos, TextSelection, line_ending_text};

#[cfg(test)]
mod tests;
