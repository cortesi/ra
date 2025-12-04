//! Document types for indexing.
//!
//! The [`ChunkDocument`] struct represents a chunk ready for indexing, combining
//! chunk-level data with document-level metadata (tags, path, tree).

use std::{path::Path, time::SystemTime};

use ra_document::{Chunk, Document};

/// A chunk ready for indexing, combining chunk data with document metadata.
///
/// This struct contains all the information needed to index a single chunk
/// in Tantivy, including both chunk-specific fields (id, title, body) and
/// document-level metadata (tags, path, tree, mtime).
#[derive(Debug, Clone)]
pub struct ChunkDocument {
    /// Unique chunk identifier: `{tree}:{path}#{slug}` or `{tree}:{path}`.
    pub id: String,
    /// Chunk title.
    pub title: String,
    /// Document tags from frontmatter.
    pub tags: Vec<String>,
    /// File path within the tree.
    pub path: String,
    /// Path split into components for partial matching.
    pub path_components: Vec<String>,
    /// Tree name this chunk belongs to.
    pub tree: String,
    /// Chunk body content.
    pub body: String,
    /// Breadcrumb showing hierarchy path.
    pub breadcrumb: String,
    /// File modification time.
    pub mtime: SystemTime,
}

impl ChunkDocument {
    /// Creates a `ChunkDocument` from a chunk and its parent document metadata.
    ///
    /// # Arguments
    /// * `chunk` - The chunk to index
    /// * `document` - The parent document containing metadata
    /// * `mtime` - File modification time
    pub fn from_chunk(chunk: &Chunk, document: &Document, mtime: SystemTime) -> Self {
        let path_str = document.path.to_string_lossy().to_string();
        let path_components = extract_path_components(&document.path);

        Self {
            id: chunk.id.clone(),
            title: chunk.title.clone(),
            tags: document.tags.clone(),
            path: path_str,
            path_components,
            tree: document.tree.clone(),
            body: chunk.body.clone(),
            breadcrumb: chunk.breadcrumb.clone(),
            mtime,
        }
    }

    /// Creates all `ChunkDocument`s from a document.
    ///
    /// # Arguments
    /// * `document` - The document to index
    /// * `mtime` - File modification time
    pub fn from_document(document: &Document, mtime: SystemTime) -> Vec<Self> {
        document
            .chunks
            .iter()
            .map(|chunk| Self::from_chunk(chunk, document, mtime))
            .collect()
    }
}

/// Extracts individual path components for indexing.
///
/// Splits the path on separators and filters out empty components.
/// For example, `docs/api/handlers.md` becomes `["docs", "api", "handlers", "md"]`.
fn extract_path_components(path: &Path) -> Vec<String> {
    path.iter()
        .filter_map(|c| {
            let s = c.to_string_lossy();
            if s.is_empty() {
                None
            } else {
                // Also split on dots for file extensions
                Some(
                    s.split('.')
                        .filter(|part| !part.is_empty())
                        .map(String::from)
                        .collect::<Vec<_>>(),
                )
            }
        })
        .flatten()
        .collect()
}

#[cfg(test)]
mod test {
    use std::path::PathBuf;

    use super::*;

    fn make_test_document() -> Document {
        Document {
            path: PathBuf::from("docs/api/handlers.md"),
            tree: "local".to_string(),
            title: "API Handlers".to_string(),
            tags: vec!["api".to_string(), "rust".to_string()],
            chunks: vec![
                Chunk {
                    id: "local:docs/api/handlers.md#preamble".to_string(),
                    title: "API Handlers".to_string(),
                    body: "Introduction to API handlers.".to_string(),
                    is_preamble: true,
                    breadcrumb: "API Handlers".to_string(),
                },
                Chunk {
                    id: "local:docs/api/handlers.md#error-handling".to_string(),
                    title: "Error Handling".to_string(),
                    body: "How to handle errors in API handlers.".to_string(),
                    is_preamble: false,
                    breadcrumb: "API Handlers › Error Handling".to_string(),
                },
            ],
        }
    }

    #[test]
    fn from_chunk_preserves_data() {
        let doc = make_test_document();
        let mtime = SystemTime::UNIX_EPOCH;
        let chunk_doc = ChunkDocument::from_chunk(&doc.chunks[0], &doc, mtime);

        assert_eq!(chunk_doc.id, "local:docs/api/handlers.md#preamble");
        assert_eq!(chunk_doc.title, "API Handlers");
        assert_eq!(chunk_doc.tags, vec!["api", "rust"]);
        assert_eq!(chunk_doc.path, "docs/api/handlers.md");
        assert_eq!(chunk_doc.tree, "local");
        assert_eq!(chunk_doc.body, "Introduction to API handlers.");
        assert_eq!(chunk_doc.mtime, SystemTime::UNIX_EPOCH);
    }

    #[test]
    fn from_document_creates_all_chunks() {
        let doc = make_test_document();
        let mtime = SystemTime::UNIX_EPOCH;
        let chunk_docs = ChunkDocument::from_document(&doc, mtime);

        assert_eq!(chunk_docs.len(), 2);
        assert_eq!(chunk_docs[0].id, "local:docs/api/handlers.md#preamble");
        assert_eq!(
            chunk_docs[1].id,
            "local:docs/api/handlers.md#error-handling"
        );
    }

    #[test]
    fn path_components_extracted_correctly() {
        let path = PathBuf::from("docs/api/handlers.md");
        let components = extract_path_components(&path);

        assert_eq!(components, vec!["docs", "api", "handlers", "md"]);
    }

    #[test]
    fn path_components_handles_nested_paths() {
        let path = PathBuf::from("src/core/auth/oauth.rs");
        let components = extract_path_components(&path);

        assert_eq!(components, vec!["src", "core", "auth", "oauth", "rs"]);
    }
}
