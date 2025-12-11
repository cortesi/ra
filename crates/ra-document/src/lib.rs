//! Document parsing and chunking for ra.
//!
//! This crate handles parsing markdown and plain text files into hierarchical chunk trees
//! suitable for indexing. It supports:
//! - YAML frontmatter extraction (title, tags)
//! - Hierarchical chunking based on heading structure
//! - GitHub-compatible slug generation for chunk IDs
//! - Hierarchy path generation for search and display

#![warn(missing_docs)]

mod build;
mod error;
mod frontmatter;
mod id;
mod node;
mod parse;
mod slug;
mod tree;

use std::path::PathBuf;

pub use build::{HeadingInfo, extract_headings};
pub use error::DocumentError;
pub use frontmatter::{Frontmatter, parse_frontmatter};
pub use id::{ChunkId, DocId, IdError};
pub use parse::{ParseResult, parse_file, parse_markdown, parse_text};
pub use tree::{ChunkTree, TreeChunk};

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
    /// The hierarchical chunk tree for this document.
    pub(crate) chunk_tree: ChunkTree,
}

impl Document {
    /// Extracts all chunks from the document's tree, ready for indexing.
    pub fn extract_chunks(&self) -> Vec<TreeChunk> {
        self.chunk_tree.extract_chunks(&self.title)
    }

    /// Returns the total number of nodes in the document tree.
    pub fn node_count(&self) -> usize {
        self.chunk_tree.node_count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::build_chunk_tree;

    #[test]
    fn test_document_creation() {
        let path = PathBuf::from("guide.md");
        let chunk_tree = build_chunk_tree("Some content", "docs", &path, "Getting Started");
        let doc = Document {
            path,
            tree: "docs".into(),
            title: "Getting Started".into(),
            tags: vec!["intro".into(), "setup".into()],
            chunk_tree,
        };
        assert_eq!(doc.title, "Getting Started");
        assert_eq!(doc.tags.len(), 2);
    }

    #[test]
    fn test_tree_chunk_creation() {
        let chunk = TreeChunk {
            id: "docs:guide.md#installation".into(),
            doc_id: "docs:guide.md".into(),
            parent_id: Some("docs:guide.md".into()),
            body: "You need Rust installed.".into(),
            hierarchy: vec!["Getting Started".into(), "Installation".into()],
            depth: 1,
            position: 1,
            byte_start: 0,
            byte_end: 100,
            sibling_count: 1,
        };
        assert!(chunk.id.contains('#'));
        assert_eq!(chunk.title(), "Installation");
        assert_eq!(chunk.depth, 1);
    }

    #[test]
    fn test_document_chunk() {
        let chunk = TreeChunk {
            id: "docs:guide.md".into(),
            doc_id: "docs:guide.md".into(),
            parent_id: None,
            body: "This guide helps you get started.".into(),
            hierarchy: vec!["Getting Started".into()],
            depth: 0,
            position: 0,
            byte_start: 0,
            byte_end: 50,
            sibling_count: 1,
        };
        assert!(!chunk.id.contains('#'));
        assert_eq!(chunk.title(), "Getting Started");
        assert_eq!(chunk.depth, 0);
        assert!(chunk.parent_id.is_none());
    }
}
