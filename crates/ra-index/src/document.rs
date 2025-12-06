//! Document types for indexing.
//!
//! The [`ChunkDocument`] struct represents a chunk ready for indexing, combining
//! chunk-level data with document-level metadata (tags, path, tree) and
//! hierarchical information (depth, position, parent_id, etc.).

use std::time::SystemTime;

use ra_document::{Document, TreeChunk};

/// A chunk ready for indexing, combining chunk data with document metadata.
///
/// This struct contains all the information needed to index a single chunk
/// in Tantivy, including:
/// - Chunk-specific fields (id, title, body)
/// - Document-level metadata (tags, path, tree, mtime)
/// - Hierarchical information (doc_id, parent_id, depth, position, byte spans, sibling_count)
#[derive(Debug, Clone)]
pub struct ChunkDocument {
    /// Unique chunk identifier: `{tree}:{path}#{slug}` or `{tree}:{path}`.
    pub id: String,
    /// Document identifier: `{tree}:{path}` (same for all chunks in a file).
    pub doc_id: String,
    /// Parent chunk identifier, or None for document nodes.
    pub parent_id: Option<String>,
    /// Chunk title.
    pub title: String,
    /// Document tags from frontmatter.
    pub tags: Vec<String>,
    /// File path within the tree.
    pub path: String,
    /// Tree name this chunk belongs to.
    pub tree: String,
    /// Chunk body content.
    pub body: String,
    /// Breadcrumb showing hierarchy path.
    pub breadcrumb: String,
    /// Hierarchy depth: 0 for document, 1-6 for h1-h6.
    pub depth: u8,
    /// Document order index (0-based pre-order traversal).
    pub position: usize,
    /// Byte offset where content span starts.
    pub byte_start: usize,
    /// Byte offset where content span ends.
    pub byte_end: usize,
    /// Number of siblings including this node.
    pub sibling_count: usize,
    /// File modification time.
    pub mtime: SystemTime,
}

impl ChunkDocument {
    /// Creates a `ChunkDocument` from a `TreeChunk` and document metadata.
    ///
    /// # Arguments
    /// * `chunk` - The tree chunk containing body, title, breadcrumb, hierarchical info, etc.
    /// * `document` - The parent document containing metadata (tags, path, tree)
    /// * `mtime` - File modification time
    pub fn from_tree_chunk(chunk: &TreeChunk, document: &Document, mtime: SystemTime) -> Self {
        let path_str = document.path.to_string_lossy().to_string();

        Self {
            id: chunk.id.clone(),
            doc_id: chunk.doc_id.clone(),
            parent_id: chunk.parent_id.clone(),
            title: chunk.title.clone(),
            tags: document.tags.clone(),
            path: path_str,
            tree: document.tree.clone(),
            body: chunk.body.clone(),
            breadcrumb: chunk.breadcrumb.clone(),
            depth: chunk.depth,
            position: chunk.position,
            byte_start: chunk.byte_start,
            byte_end: chunk.byte_end,
            sibling_count: chunk.sibling_count,
            mtime,
        }
    }

    /// Creates all `ChunkDocument`s from a document.
    ///
    /// Extracts chunks from the document's chunk tree and converts them
    /// to indexable `ChunkDocument`s.
    ///
    /// # Arguments
    /// * `document` - The document to index
    /// * `mtime` - File modification time
    pub fn from_document(document: &Document, mtime: SystemTime) -> Vec<Self> {
        let chunks = document.chunk_tree.extract_chunks(&document.title);
        chunks
            .iter()
            .map(|chunk| Self::from_tree_chunk(chunk, document, mtime))
            .collect()
    }
}

#[cfg(test)]
mod test {
    use std::path::PathBuf;

    use ra_document::build_chunk_tree;

    use super::*;

    fn make_test_document() -> Document {
        let path = PathBuf::from("docs/api/handlers.md");
        let content = r#"Introduction to API handlers.

# Error Handling

How to handle errors in API handlers."#;
        let chunk_tree = build_chunk_tree(content, "local", &path, "API Handlers");

        Document {
            path,
            tree: "local".to_string(),
            title: "API Handlers".to_string(),
            tags: vec!["api".to_string(), "rust".to_string()],
            chunk_tree,
        }
    }

    #[test]
    fn from_tree_chunk_preserves_data() {
        let doc = make_test_document();
        let mtime = SystemTime::UNIX_EPOCH;
        let chunks = doc.chunk_tree.extract_chunks(&doc.title);
        let chunk_doc = ChunkDocument::from_tree_chunk(&chunks[0], &doc, mtime);

        // First chunk is the document node (preamble)
        assert_eq!(chunk_doc.id, "local:docs/api/handlers.md");
        assert_eq!(chunk_doc.doc_id, "local:docs/api/handlers.md");
        assert!(chunk_doc.parent_id.is_none()); // Document node has no parent
        assert_eq!(chunk_doc.title, "API Handlers");
        assert_eq!(chunk_doc.tags, vec!["api", "rust"]);
        assert_eq!(chunk_doc.path, "docs/api/handlers.md");
        assert_eq!(chunk_doc.tree, "local");
        assert!(chunk_doc.body.contains("Introduction"));
        assert_eq!(chunk_doc.depth, 0); // Document node is depth 0
        assert_eq!(chunk_doc.position, 0); // First in pre-order traversal
        assert_eq!(chunk_doc.mtime, SystemTime::UNIX_EPOCH);
    }

    #[test]
    fn from_document_creates_all_chunks() {
        let doc = make_test_document();
        let mtime = SystemTime::UNIX_EPOCH;
        let chunk_docs = ChunkDocument::from_document(&doc, mtime);

        assert_eq!(chunk_docs.len(), 2);
        // Document node (preamble)
        assert_eq!(chunk_docs[0].id, "local:docs/api/handlers.md");
        assert_eq!(chunk_docs[0].depth, 0);
        assert!(chunk_docs[0].parent_id.is_none());

        // Heading node
        assert_eq!(
            chunk_docs[1].id,
            "local:docs/api/handlers.md#error-handling"
        );
        assert_eq!(chunk_docs[1].depth, 1);
        assert_eq!(
            chunk_docs[1].parent_id,
            Some("local:docs/api/handlers.md".to_string())
        );
        assert_eq!(chunk_docs[1].doc_id, "local:docs/api/handlers.md");
    }
}
