/// What can go wrong in the core, each with a message fit to show people.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("{message}")]
    Io { message: String },

    #[error("The text contains characters that {encoding} can't represent, such as “{character}”.")]
    Unmappable { encoding: String, character: String },

    #[error("“{name}” isn't a text encoding TuplePad knows.")]
    UnknownEncoding { name: String },

    #[error("This document hasn't been saved to a file yet.")]
    NoPath,

    #[error("{message}")]
    InvalidPattern { message: String },

    /// The text isn't valid for a tool (malformed JSON or XML, bad Base64…). Line and column
    /// are 1-based; 0 when unknown.
    #[error("{message}")]
    Invalid { message: String, line: u64, column: u64 },
}

impl CoreError {
    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        CoreError::Invalid { message: message.into(), line: 0, column: 0 }
    }
}

impl From<std::io::Error> for CoreError {
    fn from(error: std::io::Error) -> Self {
        CoreError::Io { message: friendly_io(&error) }
    }
}

/// An I/O error as a sentence people can act on (the file's name is shown beside it already).
pub fn friendly_io(error: &std::io::Error) -> String {
    use std::io::ErrorKind::*;
    match error.kind() {
        NotFound => "The file isn't there anymore.".into(),
        PermissionDenied => "You don't have permission to read or change it.".into(),
        IsADirectory => "It's a folder.".into(),
        StorageFull => "The disk is full.".into(),
        ReadOnlyFilesystem => "The disk it's on can't be changed.".into(),
        OutOfMemory => "There isn't enough memory.".into(),
        _ => {
            // "Something (os error 5)" reads better without the code.
            let text = error.to_string();
            let text = match text.rfind(" (os error ") {
                Some(at) => text[..at].to_string(),
                None => text,
            };
            let mut chars = text.chars();
            let text = chars.next().map(|c| c.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or_default();
            if text.ends_with('.') { text } else { format!("{text}.") }
        }
    }
}

pub type CoreResult<T> = Result<T, CoreError>;
