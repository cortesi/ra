//! Index writer for adding documents to the Tantivy index.

use std::{fs, path::Path, time::UNIX_EPOCH};

use tantivy::{
    DateTime, Index, IndexWriter as TantivyIndexWriter, TantivyDocument, directory::MmapDirectory,
};

use crate::{document::ChunkDocument, error::IndexError, schema::IndexSchema};

/// Default heap size for the index writer (50 MB).
const DEFAULT_HEAP_SIZE: usize = 50_000_000;

/// Writes documents to a Tantivy index.
///
/// The writer opens or creates an index at the specified path and provides
/// methods to add, delete, and commit documents.
pub struct IndexWriter {
    /// The Tantivy index.
    index: Index,
    /// The underlying Tantivy writer.
    writer: TantivyIndexWriter,
    /// Schema with field handles.
    schema: IndexSchema,
}

impl IndexWriter {
    /// Opens or creates an index at the given path.
    ///
    /// If the index doesn't exist, it will be created with the standard schema.
    /// If it exists, it will be opened and validated against the expected schema.
    pub fn open(path: &Path) -> Result<Self, IndexError> {
        let schema = IndexSchema::new();

        // Ensure directory exists
        fs::create_dir_all(path)?;

        let dir = MmapDirectory::open(path).map_err(|e| {
            let err: tantivy::TantivyError = e.into();
            IndexError::open_index(path.to_path_buf(), &err)
        })?;

        // Try to open existing index or create new one
        let index = Index::open_or_create(dir, schema.schema().clone())
            .map_err(|e| IndexError::open_index(path.to_path_buf(), &e))?;

        let writer = index
            .writer(DEFAULT_HEAP_SIZE)
            .map_err(|e| IndexError::open_index(path.to_path_buf(), &e))?;

        Ok(Self {
            index,
            writer,
            schema,
        })
    }

    /// Adds a chunk document to the index.
    ///
    /// The document is staged for writing but not committed until [`commit`] is called.
    pub fn add_document(&mut self, doc: &ChunkDocument) -> Result<(), IndexError> {
        let mut tantivy_doc = TantivyDocument::new();

        tantivy_doc.add_text(self.schema.id, &doc.id);
        tantivy_doc.add_text(self.schema.title, &doc.title);

        // Add tags as a single concatenated string (each tag will be tokenized)
        let tags_str = doc.tags.join(" ");
        tantivy_doc.add_text(self.schema.tags, &tags_str);

        tantivy_doc.add_text(self.schema.path, &doc.path);

        // Add path components as space-separated string for tokenization
        let path_components_str = doc.path_components.join(" ");
        tantivy_doc.add_text(self.schema.path_components, &path_components_str);

        tantivy_doc.add_text(self.schema.tree, &doc.tree);
        tantivy_doc.add_text(self.schema.body, &doc.body);

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
    pub fn rollback(&mut self) -> Result<(), IndexError> {
        self.writer.rollback().map_err(|e| IndexError::commit(&e))?;
        Ok(())
    }

    /// Deletes all documents from the index.
    pub fn delete_all(&mut self) -> Result<(), IndexError> {
        self.writer
            .delete_all_documents()
            .map_err(|e| IndexError::write(&e))?;
        Ok(())
    }

    /// Returns the number of documents in the index.
    ///
    /// Note: This requires creating a reader and may not reflect uncommitted changes.
    pub fn num_docs(&self) -> Result<u64, IndexError> {
        let reader = self
            .index
            .reader()
            .map_err(|e| IndexError::Write(e.to_string()))?;
        Ok(reader.searcher().num_docs())
    }
}

#[cfg(test)]
mod test {
    use std::time::SystemTime;

    use tempfile::TempDir;

    use super::*;

    fn make_test_chunk_doc() -> ChunkDocument {
        ChunkDocument {
            id: "local:docs/test.md#intro".to_string(),
            title: "Introduction".to_string(),
            tags: vec!["rust".to_string(), "tutorial".to_string()],
            path: "docs/test.md".to_string(),
            path_components: vec!["docs".to_string(), "test".to_string(), "md".to_string()],
            tree: "local".to_string(),
            body: "This is the introduction.".to_string(),
            mtime: SystemTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn creates_index_in_empty_directory() {
        let temp = TempDir::new().unwrap();
        let writer = IndexWriter::open(temp.path()).unwrap();

        // Verify index was created
        assert!(temp.path().join("meta.json").exists());
        drop(writer);
    }

    #[test]
    fn adds_and_commits_document() {
        let temp = TempDir::new().unwrap();
        let mut writer = IndexWriter::open(temp.path()).unwrap();

        let doc = make_test_chunk_doc();
        writer.add_document(&doc).unwrap();
        writer.commit().unwrap();

        // Verify document was indexed
        assert_eq!(writer.num_docs().unwrap(), 1);
    }

    #[test]
    fn adds_multiple_documents() {
        let temp = TempDir::new().unwrap();
        let mut writer = IndexWriter::open(temp.path()).unwrap();

        let docs = vec![
            ChunkDocument {
                id: "local:a.md#one".to_string(),
                title: "One".to_string(),
                tags: vec![],
                path: "a.md".to_string(),
                path_components: vec!["a".to_string(), "md".to_string()],
                tree: "local".to_string(),
                body: "First".to_string(),
                mtime: SystemTime::UNIX_EPOCH,
            },
            ChunkDocument {
                id: "local:b.md#two".to_string(),
                title: "Two".to_string(),
                tags: vec![],
                path: "b.md".to_string(),
                path_components: vec!["b".to_string(), "md".to_string()],
                tree: "local".to_string(),
                body: "Second".to_string(),
                mtime: SystemTime::UNIX_EPOCH,
            },
        ];

        writer.add_documents(&docs).unwrap();
        writer.commit().unwrap();

        assert_eq!(writer.num_docs().unwrap(), 2);
    }

    #[test]
    fn reopens_existing_index() {
        let temp = TempDir::new().unwrap();

        // Create and populate index
        {
            let mut writer = IndexWriter::open(temp.path()).unwrap();
            writer.add_document(&make_test_chunk_doc()).unwrap();
            writer.commit().unwrap();
        }

        // Reopen and verify
        {
            let writer = IndexWriter::open(temp.path()).unwrap();
            assert_eq!(writer.num_docs().unwrap(), 1);
        }
    }

    #[test]
    fn delete_all_removes_documents() {
        let temp = TempDir::new().unwrap();
        let mut writer = IndexWriter::open(temp.path()).unwrap();

        writer.add_document(&make_test_chunk_doc()).unwrap();
        writer.commit().unwrap();

        writer.delete_all().unwrap();
        writer.commit().unwrap();

        assert_eq!(writer.num_docs().unwrap(), 0);
    }

    #[test]
    fn rollback_discards_uncommitted_changes() {
        let temp = TempDir::new().unwrap();
        let mut writer = IndexWriter::open(temp.path()).unwrap();

        writer.add_document(&make_test_chunk_doc()).unwrap();
        writer.rollback().unwrap();

        // After rollback, we need to check if we can add again
        // The rollback should have discarded the uncommitted document
        writer.commit().unwrap();

        assert_eq!(writer.num_docs().unwrap(), 0);
    }
}
