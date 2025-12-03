//! Error types for ra configuration.

use std::io;
use std::path::PathBuf;

use thiserror::Error;
use toml::de;

/// Errors that can occur when loading or processing configuration.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// Failed to read a configuration file.
    #[error("failed to read config file {path}: {source}")]
    ReadFile {
        /// Path to the file that could not be read.
        path: PathBuf,
        /// Underlying I/O error.
        source: io::Error,
    },

    /// Failed to parse TOML configuration.
    #[error("failed to parse config file {path}: {source}")]
    ParseToml {
        /// Path to the file that could not be parsed.
        path: PathBuf,
        /// Underlying TOML parse error.
        source: de::Error,
    },

    /// A tree path does not exist or is not accessible.
    #[error("tree path does not exist: {path}")]
    TreePathNotFound {
        /// The path that was not found.
        path: PathBuf,
    },

    /// A tree path is not a directory.
    #[error("tree path is not a directory: {path}")]
    TreePathNotDirectory {
        /// The path that is not a directory.
        path: PathBuf,
    },

    /// An include pattern references an undefined tree.
    #[error("include pattern references undefined tree: {tree}")]
    UndefinedTree {
        /// Name of the undefined tree.
        tree: String,
    },

    /// Failed to compile a glob pattern.
    #[error("invalid glob pattern '{pattern}': {source}")]
    InvalidPattern {
        /// The invalid pattern.
        pattern: String,
        /// Underlying glob error.
        source: globset::Error,
    },

    /// Failed to determine home directory.
    #[error("could not determine home directory")]
    NoHomeDirectory,

    /// Failed to canonicalize a path.
    #[error("failed to resolve path {path}: {source}")]
    PathResolution {
        /// The path that could not be resolved.
        path: PathBuf,
        /// Underlying I/O error.
        source: io::Error,
    },
}
