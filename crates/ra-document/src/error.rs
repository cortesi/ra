//! Error types for document parsing.

use std::{io, path::PathBuf};

use thiserror::Error;

/// Errors that can occur when parsing documents.
#[derive(Debug, Error)]
pub enum DocumentError {
    /// Failed to read a file.
    #[error("failed to read file {path}: {source}")]
    ReadFile {
        /// Path to the file that could not be read.
        path: PathBuf,
        /// Underlying I/O error.
        source: io::Error,
    },

    /// Unsupported file type.
    #[error("unsupported file type: {path}")]
    UnsupportedFileType {
        /// Path to the unsupported file.
        path: PathBuf,
    },
}
