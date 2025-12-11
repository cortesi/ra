//! Index writer for adding documents to the Tantivy index.

use std::{fs, path::Path, time::UNIX_EPOCH};

use tantivy::{
    DateTime, Index, IndexWriter as TantivyIndexWriter, TantivyDocument, directory::MmapDirectory,
};

use crate::{
    analyzer::{RA_TOKENIZER, build_analyzer_from_name},
    document::ChunkDocument,
    error::IndexError,
    schema::IndexSchema,
};

/// Default heap size for the index writer (50 MB).
const DEFAULT_HEAP_SIZE: usize = 50_000_000;

/// Writes documents to a Tantivy index.
///
/// The writer opens or creates an index at the specified path and provides
/// methods to add, delete, and commit documents.
pub struct IndexWriter {
    /// The underlying Tantivy writer.
    writer: TantivyIndexWriter,
    /// Schema with field handles.
    schema: IndexSchema,
}

impl IndexWriter {
    /// Opens or creates an index at the given path with the specified stemmer language.
    ///
    /// If the index doesn't exist, it will be created with the standard schema.
    /// If it exists but the schema doesn't match (e.g., after a schema version change),
    /// the old index is deleted and a new one is created.
    ///
    /// The `language` parameter is a language name string (e.g., "english", "french")
    /// that controls which stemmer is used for text analysis.
    pub fn open(path: &Path, language: &str) -> Result<Self, IndexError> {
        let schema = IndexSchema::new();

        // Ensure directory exists
        fs::create_dir_all(path)?;

        let index = Self::open_or_recreate_index(path, &schema)?;

        // Register our custom text analyzer with the configured stemmer language
        let analyzer = build_analyzer_from_name(language)?;
        index.tokenizers().register(RA_TOKENIZER, analyzer);

        let writer = index
            .writer(DEFAULT_HEAP_SIZE)
            .map_err(|e| IndexError::open_index(path.to_path_buf(), &e))?;

        Ok(Self { writer, schema })
    }

    /// Opens an existing index or creates a new one. If the schema doesn't match,
    /// deletes the old index and creates a fresh one.
    fn open_or_recreate_index(path: &Path, schema: &IndexSchema) -> Result<Index, IndexError> {
        let dir = MmapDirectory::open(path).map_err(|e| {
            let err: tantivy::TantivyError = e.into();
            IndexError::open_index(path.to_path_buf(), &err)
        })?;

        // Try to open existing index or create new one
        match Index::open_or_create(dir, schema.schema().clone()) {
            Ok(index) => Ok(index),
            Err(e) => {
                // Check if this is a schema mismatch error
                let error_msg = e.to_string();
                if error_msg.contains("schema does not match") || error_msg.contains("Schema error")
                {
                    // Delete the old index and create a new one
                    Self::delete_index_files(path)?;

                    // Recreate directory and open fresh index
                    fs::create_dir_all(path)?;
                    let dir = MmapDirectory::open(path).map_err(|e| {
                        let err: tantivy::TantivyError = e.into();
                        IndexError::open_index(path.to_path_buf(), &err)
                    })?;

                    Index::open_or_create(dir, schema.schema().clone())
                        .map_err(|e| IndexError::open_index(path.to_path_buf(), &e))
                } else {
                    Err(IndexError::open_index(path.to_path_buf(), &e))
                }
            }
        }
    }

    /// Deletes all files in an index directory.
    fn delete_index_files(path: &Path) -> Result<(), IndexError> {
        if path.exists() {
            fs::remove_dir_all(path)?;
        }
        Ok(())
    }

    /// Adds a chunk document to the index.
    ///
    /// The document is staged for writing but not committed until [`commit`] is called.
    #[allow(clippy::needless_pass_by_ref_mut)] // Semantic mutability - Tantivy uses interior mutability
    pub fn add_document(&mut self, doc: &ChunkDocument) -> Result<(), IndexError> {
        let mut tantivy_doc = TantivyDocument::new();

        tantivy_doc.add_text(self.schema.id, &doc.id);
        tantivy_doc.add_text(self.schema.doc_id, &doc.doc_id);
        tantivy_doc.add_text(
            self.schema.parent_id,
            doc.parent_id.as_deref().unwrap_or(""),
        );

        // Add hierarchy as multi-value field (each element is a separate value)
        for element in &doc.hierarchy {
            tantivy_doc.add_text(self.schema.hierarchy, element);
        }

        // Add tags as a single concatenated string (each tag will be tokenized)
        let tags_str = doc.tags.join(" ");
        tantivy_doc.add_text(self.schema.tags, &tags_str);

        tantivy_doc.add_text(self.schema.path, &doc.path);
        tantivy_doc.add_text(self.schema.tree, &doc.tree);
        tantivy_doc.add_text(self.schema.body, &doc.body);

        // Hierarchical metadata
        tantivy_doc.add_u64(self.schema.depth, doc.depth as u64);
        tantivy_doc.add_u64(self.schema.position, doc.position as u64);
        tantivy_doc.add_u64(self.schema.byte_start, doc.byte_start as u64);
        tantivy_doc.add_u64(self.schema.byte_end, doc.byte_end as u64);
        tantivy_doc.add_u64(self.schema.sibling_count, doc.sibling_count as u64);

        // Convert SystemTime to Tantivy DateTime
        let datetime = DateTime::from_timestamp_secs(
            doc.mtime
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0),
        );
        tantivy_doc.add_date(self.schema.mtime, datetime);

        self.writer
            .add_document(tantivy_doc)
            .map_err(|e| IndexError::write(&e))?;
        Ok(())
    }

    /// Adds multiple chunk documents to the index.
    pub fn add_documents(&mut self, docs: &[ChunkDocument]) -> Result<(), IndexError> {
        for doc in docs {
            self.add_document(doc)?;
        }
        Ok(())
    }

    /// Deletes all documents with the given tree and path.
    ///
    /// This is used for incremental updates when a file is modified or removed.
    #[allow(clippy::needless_pass_by_ref_mut)] // Semantic mutability - Tantivy uses interior mutability
    pub fn delete_by_path(&mut self, tree: &str, path: &str) {
        // Delete by term on the id field prefix
        // IDs are formatted as `{tree}:{path}#{slug}` or `{tree}:{path}`
        // We need to delete all chunks from this file
        let prefix = format!("{tree}:{path}");

        // Use the id field for deletion since it's indexed as STRING
        let term = tantivy::Term::from_field_text(self.schema.id, &prefix);
        self.writer.delete_term(term);

        // Also delete exact match for files without fragment
        let term_exact = tantivy::Term::from_field_text(self.schema.id, &format!("{prefix}#"));
        self.writer.delete_term(term_exact);
    }

    /// Commits all pending changes to the index.
    ///
    /// This makes all added and deleted documents visible to readers.
    pub fn commit(&mut self) -> Result<(), IndexError> {
        self.writer.commit().map_err(|e| IndexError::commit(&e))?;
        Ok(())
    }

    /// Rolls back any uncommitted changes.
    #[cfg(test)]
    pub fn rollback(&mut self) -> Result<(), IndexError> {
        self.writer.rollback().map_err(|e| IndexError::commit(&e))?;
        Ok(())
    }

    /// Deletes all documents from the index.
    #[allow(clippy::needless_pass_by_ref_mut)] // Semantic mutability - Tantivy uses interior mutability
    pub fn delete_all(&mut self) -> Result<(), IndexError> {
        self.writer
            .delete_all_documents()
            .map_err(|e| IndexError::write(&e))?;
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use std::{path::Path, time::SystemTime};

    use tantivy::Index;
    use tempfile::TempDir;

    use super::*;

    fn num_docs_in_dir(path: &Path) -> u64 {
        let index = Index::open_in_dir(path).unwrap();
        let reader = index.reader().unwrap();
        reader.searcher().num_docs()
    }

    fn make_test_chunk_doc() -> ChunkDocument {
        ChunkDocument {
            id: "local:docs/test.md#intro".to_string(),
            doc_id: "local:docs/test.md".to_string(),
            parent_id: Some("local:docs/test.md".to_string()),
            hierarchy: vec!["Test Document".to_string(), "Introduction".to_string()],
            depth: 1,
            tags: vec!["rust".to_string(), "tutorial".to_string()],
            path: "docs/test.md".to_string(),
            tree: "local".to_string(),
            body: "This is the introduction.".to_string(),
            position: 1,
            byte_start: 50,
            byte_end: 150,
            sibling_count: 2,
            mtime: SystemTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn creates_index_in_empty_directory() {
        let temp = TempDir::new().unwrap();
        let writer = IndexWriter::open(temp.path(), "english").unwrap();

        // Verify index was created
        assert!(temp.path().join("meta.json").exists());
        drop(writer);
    }

    #[test]
    fn adds_and_commits_document() {
        let temp = TempDir::new().unwrap();
        let mut writer = IndexWriter::open(temp.path(), "english").unwrap();

        let doc = make_test_chunk_doc();
        writer.add_document(&doc).unwrap();
        writer.commit().unwrap();

        // Verify document was indexed
        assert_eq!(num_docs_in_dir(temp.path()), 1);
    }

    #[test]
    fn adds_multiple_documents() {
        let temp = TempDir::new().unwrap();
        let mut writer = IndexWriter::open(temp.path(), "english").unwrap();

        let docs = vec![
            ChunkDocument {
                id: "local:a.md#one".to_string(),
                doc_id: "local:a.md".to_string(),
                parent_id: Some("local:a.md".to_string()),
                hierarchy: vec!["A".to_string(), "One".to_string()],
                depth: 1,
                tags: vec![],
                path: "a.md".to_string(),
                tree: "local".to_string(),
                body: "First".to_string(),
                position: 1,
                byte_start: 0,
                byte_end: 50,
                sibling_count: 1,
                mtime: SystemTime::UNIX_EPOCH,
            },
            ChunkDocument {
                id: "local:b.md#two".to_string(),
                doc_id: "local:b.md".to_string(),
                parent_id: Some("local:b.md".to_string()),
                hierarchy: vec!["B".to_string(), "Two".to_string()],
                depth: 1,
                tags: vec![],
                path: "b.md".to_string(),
                tree: "local".to_string(),
                body: "Second".to_string(),
                position: 1,
                byte_start: 0,
                byte_end: 60,
                sibling_count: 1,
                mtime: SystemTime::UNIX_EPOCH,
            },
        ];

        writer.add_documents(&docs).unwrap();
        writer.commit().unwrap();

        assert_eq!(num_docs_in_dir(temp.path()), 2);
    }

    #[test]
    fn reopens_existing_index() {
        let temp = TempDir::new().unwrap();

        // Create and populate index
        {
            let mut writer = IndexWriter::open(temp.path(), "english").unwrap();
            writer.add_document(&make_test_chunk_doc()).unwrap();
            writer.commit().unwrap();
        }

        // Reopen and verify
        {
            let writer = IndexWriter::open(temp.path(), "english").unwrap();
            assert_eq!(num_docs_in_dir(temp.path()), 1);
            drop(writer);
        }
    }

    #[test]
    fn delete_all_removes_documents() {
        let temp = TempDir::new().unwrap();
        let mut writer = IndexWriter::open(temp.path(), "english").unwrap();

        writer.add_document(&make_test_chunk_doc()).unwrap();
        writer.commit().unwrap();

        writer.delete_all().unwrap();
        writer.commit().unwrap();

        assert_eq!(num_docs_in_dir(temp.path()), 0);
    }

    #[test]
    fn rollback_discards_uncommitted_changes() {
        let temp = TempDir::new().unwrap();
        let mut writer = IndexWriter::open(temp.path(), "english").unwrap();

        writer.add_document(&make_test_chunk_doc()).unwrap();
        writer.rollback().unwrap();

        // After rollback, we need to check if we can add again
        // The rollback should have discarded the uncommitted document
        writer.commit().unwrap();

        assert_eq!(num_docs_in_dir(temp.path()), 0);
    }
}
