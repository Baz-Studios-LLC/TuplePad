/// Errors that cross the FFI boundary into Swift.
#[derive(Debug, thiserror::Error, uniffi::Error)]
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
}

impl From<std::io::Error> for CoreError {
    fn from(error: std::io::Error) -> Self {
        CoreError::Io { message: error.to_string() }
    }
}

pub type CoreResult<T> = Result<T, CoreError>;
