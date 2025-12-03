//! Document parsing and chunking for ra.
//!
//! This crate handles parsing markdown and plain text files into chunks suitable for indexing.
//! It supports:
//! - YAML frontmatter extraction (title, tags)
//! - Chunking at h1 heading boundaries
//! - GitHub-compatible slug generation for chunk IDs

#![warn(missing_docs)]

mod error;
mod frontmatter;

use std::path::PathBuf;

pub use error::DocumentError;
pub use frontmatter::{Frontmatter, parse_frontmatter};

/// A parsed document ready for indexing.
#[derive(Debug, Clone)]
pub struct Document {
    /// Relative path within the tree.
    pub path: PathBuf,
    /// Name of the tree this document belongs to.
    pub tree: String,
    /// Document title (from frontmatter, first h1, or filename).
    pub title: String,
    /// Tags from frontmatter.
    pub tags: Vec<String>,
    /// Chunks extracted from the document.
    pub chunks: Vec<Chunk>,
}

/// A chunk of content from a document.
///
/// Documents are split at h1 boundaries. Each chunk has a unique ID
/// formed from the tree name, file path, and heading slug.
#[derive(Debug, Clone)]
pub struct Chunk {
    /// Unique chunk identifier: `{tree}:{path}#{slug}` or `{tree}:{path}` for text files.
    pub id: String,
    /// Chunk title (from h1 heading, frontmatter title for preamble, or filename).
    pub title: String,
    /// The chunk content (markdown or plain text).
    pub body: String,
    /// Whether this is the preamble (content before first h1).
    pub is_preamble: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_document_creation() {
        let doc = Document {
            path: PathBuf::from("guide.md"),
            tree: "docs".into(),
            title: "Getting Started".into(),
            tags: vec!["intro".into(), "setup".into()],
            chunks: vec![],
        };
        assert_eq!(doc.title, "Getting Started");
        assert_eq!(doc.tags.len(), 2);
    }

    #[test]
    fn test_chunk_creation() {
        let chunk = Chunk {
            id: "docs:guide.md#installation".into(),
            title: "Installation".into(),
            body: "## Prerequisites\n\nYou need Rust installed.".into(),
            is_preamble: false,
        };
        assert!(!chunk.is_preamble);
        assert!(chunk.id.contains('#'));
    }

    #[test]
    fn test_preamble_chunk() {
        let chunk = Chunk {
            id: "docs:guide.md#preamble".into(),
            title: "Getting Started".into(),
            body: "This guide helps you get started.".into(),
            is_preamble: true,
        };
        assert!(chunk.is_preamble);
        assert!(chunk.id.ends_with("#preamble"));
    }
}
