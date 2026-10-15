use crate::encoding::TextEncoding;

/// A position in a document. `column` counts UTF-16 code units from the start
/// of the line, so positions match what text layout and the MCP tools count.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TextPos {
    pub line: u64,
    pub column: u64,
}

impl TextPos {
    pub fn new(line: u64, column: u64) -> Self {
        TextPos { line, column }
    }
}

/// A selection or caret. `head` is where the caret is; `anchor` is where the selection started.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct TextSelection {
    pub anchor: TextPos,
    pub head: TextPos,
}

impl TextSelection {
    pub fn caret(pos: TextPos) -> Self {
        TextSelection { anchor: pos, head: pos }
    }

    pub fn start(&self) -> TextPos {
        self.anchor.min(self.head)
    }

    pub fn end(&self) -> TextPos {
        self.anchor.max(self.head)
    }
}

/// What kind of change an edit is, so consecutive keystrokes undo together.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditKind {
    /// Typing characters; a run of typing undoes as one step.
    Typing,
    /// Backspace or forward delete; a run of deletes undoes as one step.
    Deletion,
    /// Input-method composition (e.g. Japanese, Chinese, accents).
    Composition,
    /// Paste, cut, transforms and everything else: always its own undo step.
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    CrLf,
    Cr,
}

impl LineEnding {
    pub fn as_str(self) -> &'static str {
        match self {
            LineEnding::Lf => "\n",
            LineEnding::CrLf => "\r\n",
            LineEnding::Cr => "\r",
        }
    }
}

pub fn line_ending_text(ending: LineEnding) -> String {
    ending.as_str().to_string()
}

/// Facts about a document shown in the status bar.
#[derive(Clone, Debug, PartialEq)]
pub struct FileInfo {
    pub path: Option<String>,
    pub encoding: TextEncoding,
    pub line_ending: LineEnding,
    /// Some bytes weren't valid in the detected encoding and were replaced on load.
    pub had_decode_errors: bool,
    /// The file looks binary (contains NUL bytes).
    pub looks_binary: bool,
}
