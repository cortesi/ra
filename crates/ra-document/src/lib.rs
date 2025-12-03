//! Document parsing and chunking for ra.
//!
//! This crate handles parsing markdown and plain text files into chunks suitable for indexing.
//! It supports:
//! - YAML frontmatter extraction (title, tags)
//! - Adaptive chunking at heading boundaries
//! - GitHub-compatible slug generation for chunk IDs
//! - Breadcrumb generation for hierarchy display

#![warn(missing_docs)]

mod chunker;
mod error;
mod frontmatter;
mod parse;
mod slug;

use std::path::PathBuf;

pub use chunker::{ChunkData, Heading, chunk_markdown, determine_chunk_level, extract_headings};
pub use error::DocumentError;
pub use frontmatter::{Frontmatter, parse_frontmatter};
pub use parse::{DEFAULT_MIN_CHUNK_SIZE, ParseResult, parse_file, parse_markdown, parse_text};
pub use pulldown_cmark::HeadingLevel;
pub use slug::Slugifier;

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
/// Documents are split at heading boundaries using adaptive chunking.
/// Each chunk has a unique ID formed from the tree name, file path, and heading slug.
#[derive(Debug, Clone)]
pub struct Chunk {
    /// Unique chunk identifier: `{tree}:{path}#{slug}` or `{tree}:{path}` for text files.
    pub id: String,
    /// Chunk title (from heading, frontmatter title for preamble, or filename).
    pub title: String,
    /// The chunk content (markdown or plain text).
    pub body: String,
    /// Whether this is the preamble (content before first heading at chunk level).
    pub is_preamble: bool,
    /// Breadcrumb showing hierarchy path (e.g., "> Doc › Section › Subsection").
    pub breadcrumb: String,
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
            breadcrumb: "> Getting Started › Installation".into(),
        };
        assert!(!chunk.is_preamble);
        assert!(chunk.id.contains('#'));
        assert!(chunk.breadcrumb.contains('›'));
    }

    #[test]
    fn test_preamble_chunk() {
        let chunk = Chunk {
            id: "docs:guide.md#preamble".into(),
            title: "Getting Started".into(),
            body: "This guide helps you get started.".into(),
            is_preamble: true,
            breadcrumb: "> Getting Started".into(),
        };
        assert!(chunk.is_preamble);
        assert!(chunk.id.ends_with("#preamble"));
        assert_eq!(chunk.breadcrumb, "> Getting Started");
    }
}
