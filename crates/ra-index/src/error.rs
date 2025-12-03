//! Error types for the ra-index crate.

use std::{io, path::PathBuf};

use thiserror::Error;

/// Errors that can occur when working with the search index.
#[derive(Debug, Error)]
pub enum IndexError {
    /// Failed to open or create the index.
    #[error("failed to open index at {path}: {message}")]
    OpenIndex {
        /// Path to the index directory.
        path: PathBuf,
        /// Error message.
        message: String,
    },

    /// Failed to write to the index.
    #[error("failed to write to index: {0}")]
    Write(String),

    /// Failed to commit changes to the index.
    #[error("failed to commit index: {0}")]
    Commit(String),

    /// I/O error.
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    /// Invalid stemmer language.
    #[error("unsupported stemmer language: {0}")]
    InvalidLanguage(String),
}

impl IndexError {
    /// Creates an `OpenIndex` error from a path and Tantivy error.
    pub(crate) fn open_index(path: PathBuf, source: &tantivy::TantivyError) -> Self {
        Self::OpenIndex {
            path,
            message: source.to_string(),
        }
    }

    /// Creates a `Write` error from a Tantivy error.
    pub(crate) fn write(source: &tantivy::TantivyError) -> Self {
        Self::Write(source.to_string())
    }

    /// Creates a `Commit` error from a Tantivy error.
    pub(crate) fn commit(source: &tantivy::TantivyError) -> Self {
        Self::Commit(source.to_string())
    }
}
