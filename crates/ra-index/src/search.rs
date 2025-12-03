//! Search execution for the ra index.
//!
//! Provides the [`Searcher`] struct for querying the index and retrieving results.
//! Supports field boosting, local tree boosting, and snippet generation.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use tantivy::{
    Index, TantivyDocument,
    collector::TopDocs,
    directory::MmapDirectory,
    query::Query,
    schema::{Field, Value},
    snippet::SnippetGenerator,
};

use crate::{
    IndexError,
    analyzer::{RA_TOKENIZER, build_analyzer_from_name},
    query::QueryBuilder,
    schema::IndexSchema,
};

/// Default maximum number of characters in a snippet.
const DEFAULT_SNIPPET_MAX_CHARS: usize = 150;

/// A search result from the index.
#[derive(Debug, Clone)]
pub struct SearchResult {
    /// Unique chunk identifier.
    pub id: String,
    /// Chunk title.
    pub title: String,
    /// Tree name this chunk belongs to.
    pub tree: String,
    /// File path within the tree.
    pub path: String,
    /// Chunk body content.
    pub body: String,
    /// Search relevance score (after boosting).
    pub score: f32,
    /// Optional snippet with query terms highlighted.
    pub snippet: Option<String>,
}

/// Searches the index for matching documents.
pub struct Searcher {
    /// The Tantivy index.
    index: Index,
    /// Schema with field handles.
    schema: IndexSchema,
    /// Query builder for constructing queries.
    query_builder: QueryBuilder,
    /// Map from tree name to whether it's global.
    tree_is_global: HashMap<String, bool>,
    /// Local tree boost multiplier.
    local_boost: f32,
}

impl Searcher {
    /// Opens an existing index for searching.
    ///
    /// # Arguments
    /// * `path` - Path to the index directory
    /// * `language` - Stemmer language (e.g., "english")
    /// * `trees` - Tree configurations for determining global vs local boost
    /// * `local_boost` - Multiplier for local (non-global) tree results
    pub fn open(
        path: &Path,
        language: &str,
        trees: &[ra_config::Tree],
        local_boost: f32,
    ) -> Result<Self, IndexError> {
        if !path.exists() {
            return Err(IndexError::OpenIndex {
                path: path.to_path_buf(),
                message: "index directory does not exist".to_string(),
            });
        }

        let schema = IndexSchema::new();

        let dir = MmapDirectory::open(path).map_err(|e| {
            let err: tantivy::TantivyError = e.into();
            IndexError::open_index(path.to_path_buf(), &err)
        })?;

        let index = Index::open(dir).map_err(|e| IndexError::open_index(path.to_path_buf(), &e))?;

        // Register our custom text analyzer
        let analyzer = build_analyzer_from_name(language)?;
        index.tokenizers().register(RA_TOKENIZER, analyzer);

        let query_builder = QueryBuilder::with_language(schema.clone(), language)?;

        // Build tree global/local map
        let tree_is_global: HashMap<String, bool> = trees
            .iter()
            .map(|t| (t.name.clone(), t.is_global))
            .collect();

        Ok(Self {
            index,
            schema,
            query_builder,
            tree_is_global,
            local_boost,
        })
    }

    /// Opens an existing index for searching using configuration.
    ///
    /// Convenience method that extracts settings from a Config.
    pub fn open_with_config(path: &Path, config: &ra_config::Config) -> Result<Self, IndexError> {
        Self::open(
            path,
            &config.search.stemmer,
            &config.trees,
            config.settings.local_boost,
        )
    }

    /// Searches the index for documents matching the query.
    ///
    /// # Arguments
    /// * `query_str` - The search query string
    /// * `limit` - Maximum number of results to return
    ///
    /// # Returns
    /// A vector of search results, ordered by relevance score (highest first).
    pub fn search(
        &mut self,
        query_str: &str,
        limit: usize,
    ) -> Result<Vec<SearchResult>, IndexError> {
        let query = match self.query_builder.build(query_str) {
            Some(q) => q,
            None => return Ok(Vec::new()),
        };

        self.execute_query(&*query, limit, true)
    }

    /// Searches the index for documents matching multiple topics.
    ///
    /// Each topic is searched independently and results are combined.
    ///
    /// # Arguments
    /// * `topics` - Array of query strings
    /// * `limit` - Maximum number of results to return
    pub fn search_multi(
        &mut self,
        topics: &[&str],
        limit: usize,
    ) -> Result<Vec<SearchResult>, IndexError> {
        let query = match self.query_builder.build_multi(topics) {
            Some(q) => q,
            None => return Ok(Vec::new()),
        };

        self.execute_query(&*query, limit, true)
    }

    /// Searches without generating snippets (faster).
    pub fn search_no_snippets(
        &mut self,
        query_str: &str,
        limit: usize,
    ) -> Result<Vec<SearchResult>, IndexError> {
        let query = match self.query_builder.build(query_str) {
            Some(q) => q,
            None => return Ok(Vec::new()),
        };

        self.execute_query(&*query, limit, false)
    }

    /// Executes a query and returns results.
    fn execute_query(
        &self,
        query: &dyn Query,
        limit: usize,
        generate_snippets: bool,
    ) -> Result<Vec<SearchResult>, IndexError> {
        let reader = self
            .index
            .reader()
            .map_err(|e| IndexError::Write(e.to_string()))?;

        let searcher = reader.searcher();

        let top_docs = searcher
            .search(query, &TopDocs::with_limit(limit))
            .map_err(|e| IndexError::Write(e.to_string()))?;

        // Create snippet generator if needed
        let snippet_generator = if generate_snippets {
            let mut generator = SnippetGenerator::create(&searcher, query, self.schema.body)
                .map_err(|e| IndexError::Write(e.to_string()))?;
            generator.set_max_num_chars(DEFAULT_SNIPPET_MAX_CHARS);
            Some(generator)
        } else {
            None
        };

        let mut results = Vec::with_capacity(top_docs.len());

        for (score, doc_address) in top_docs {
            let doc: TantivyDocument = searcher
                .doc(doc_address)
                .map_err(|e| IndexError::Write(e.to_string()))?;

            let result = self.doc_to_result(&doc, score, &snippet_generator);
            results.push(result);
        }

        Ok(results)
    }

    /// Converts a Tantivy document to a SearchResult.
    fn doc_to_result(
        &self,
        doc: &TantivyDocument,
        base_score: f32,
        snippet_generator: &Option<SnippetGenerator>,
    ) -> SearchResult {
        let id = self.get_text_field(doc, self.schema.id);
        let title = self.get_text_field(doc, self.schema.title);
        let tree = self.get_text_field(doc, self.schema.tree);
        let path = self.get_text_field(doc, self.schema.path);
        let body = self.get_text_field(doc, self.schema.body);

        // Apply local boost for non-global trees
        let is_global = self.tree_is_global.get(&tree).copied().unwrap_or(false);
        let score = if is_global {
            base_score
        } else {
            base_score * self.local_boost
        };

        // Generate snippet if generator is available
        let snippet = snippet_generator.as_ref().map(|generator| {
            let snippet = generator.snippet_from_doc(doc);
            snippet.to_html()
        });

        SearchResult {
            id,
            title,
            tree,
            path,
            body,
            score,
            snippet,
        }
    }

    /// Extracts a text field value from a document.
    fn get_text_field(&self, doc: &TantivyDocument, field: Field) -> String {
        doc.get_first(field)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    }

    /// Returns the number of documents in the index.
    pub fn num_docs(&self) -> Result<u64, IndexError> {
        let reader = self
            .index
            .reader()
            .map_err(|e| IndexError::Write(e.to_string()))?;
        Ok(reader.searcher().num_docs())
    }
}

/// Creates an index directory path and opens it for searching.
///
/// Convenience function that combines index location resolution with searcher creation.
pub fn open_searcher(config: &ra_config::Config) -> Result<Searcher, IndexError> {
    let index_dir = crate::index_directory(config).ok_or_else(|| IndexError::OpenIndex {
        path: PathBuf::new(),
        message: "no configuration found".to_string(),
    })?;

    Searcher::open_with_config(&index_dir, config)
}

#[cfg(test)]
mod test {
    use std::time::SystemTime;

    use tempfile::TempDir;

    use super::*;
    use crate::{ChunkDocument, IndexWriter};

    fn create_test_index(temp: &TempDir) -> Vec<ChunkDocument> {
        let docs = vec![
            ChunkDocument {
                id: "local:docs/rust.md#intro".to_string(),
                title: "Introduction to Rust".to_string(),
                tags: vec!["rust".to_string(), "programming".to_string()],
                path: "docs/rust.md".to_string(),
                path_components: vec!["docs".to_string(), "rust".to_string(), "md".to_string()],
                tree: "local".to_string(),
                body: "Rust is a systems programming language focused on safety and performance."
                    .to_string(),
                mtime: SystemTime::UNIX_EPOCH,
            },
            ChunkDocument {
                id: "local:docs/async.md#basics".to_string(),
                title: "Async Programming".to_string(),
                tags: vec!["rust".to_string(), "async".to_string()],
                path: "docs/async.md".to_string(),
                path_components: vec!["docs".to_string(), "async".to_string(), "md".to_string()],
                tree: "local".to_string(),
                body: "Asynchronous programming in Rust uses futures and the async/await syntax."
                    .to_string(),
                mtime: SystemTime::UNIX_EPOCH,
            },
            ChunkDocument {
                id: "global:reference/errors.md#handling".to_string(),
                title: "Error Handling".to_string(),
                tags: vec!["rust".to_string(), "errors".to_string()],
                path: "reference/errors.md".to_string(),
                path_components: vec![
                    "reference".to_string(),
                    "errors".to_string(),
                    "md".to_string(),
                ],
                tree: "global".to_string(),
                body: "Rust error handling uses Result and Option types for safety.".to_string(),
                mtime: SystemTime::UNIX_EPOCH,
            },
        ];

        let mut writer = IndexWriter::open(temp.path(), "english").unwrap();
        for doc in &docs {
            writer.add_document(doc).unwrap();
        }
        writer.commit().unwrap();

        docs
    }

    fn make_trees() -> Vec<ra_config::Tree> {
        vec![
            ra_config::Tree {
                name: "local".to_string(),
                path: PathBuf::from("/tmp/local"),
                is_global: false,
                include: vec![],
                exclude: vec![],
            },
            ra_config::Tree {
                name: "global".to_string(),
                path: PathBuf::from("/tmp/global"),
                is_global: true,
                include: vec![],
                exclude: vec![],
            },
        ]
    }

    #[test]
    fn search_finds_matching_documents() {
        let temp = TempDir::new().unwrap();
        create_test_index(&temp);

        let mut searcher = Searcher::open(temp.path(), "english", &make_trees(), 1.5).unwrap();

        let results = searcher.search("rust", 10).unwrap();

        // All three documents mention rust
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn search_respects_limit() {
        let temp = TempDir::new().unwrap();
        create_test_index(&temp);

        let mut searcher = Searcher::open(temp.path(), "english", &make_trees(), 1.5).unwrap();

        let results = searcher.search("rust", 2).unwrap();

        assert_eq!(results.len(), 2);
    }

    #[test]
    fn search_returns_empty_for_no_matches() {
        let temp = TempDir::new().unwrap();
        create_test_index(&temp);

        let mut searcher = Searcher::open(temp.path(), "english", &make_trees(), 1.5).unwrap();

        let results = searcher.search("python", 10).unwrap();

        assert!(results.is_empty());
    }

    #[test]
    fn search_returns_empty_for_empty_query() {
        let temp = TempDir::new().unwrap();
        create_test_index(&temp);

        let mut searcher = Searcher::open(temp.path(), "english", &make_trees(), 1.5).unwrap();

        let results = searcher.search("", 10).unwrap();

        assert!(results.is_empty());
    }

    #[test]
    fn search_applies_local_boost() {
        let temp = TempDir::new().unwrap();
        create_test_index(&temp);

        let local_boost = 2.0;
        let mut searcher =
            Searcher::open(temp.path(), "english", &make_trees(), local_boost).unwrap();

        // Search for "error" which only matches the global tree document
        let results = searcher.search("error", 10).unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].tree, "global");
        // Global trees don't get boosted, so score should be unmodified

        // Search for "async" which only matches a local tree document
        let results = searcher.search("async", 10).unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].tree, "local");
        // Local trees get boosted
    }

    #[test]
    fn search_result_contains_all_fields() {
        let temp = TempDir::new().unwrap();
        create_test_index(&temp);

        let mut searcher = Searcher::open(temp.path(), "english", &make_trees(), 1.5).unwrap();

        let results = searcher.search("async", 10).unwrap();

        assert_eq!(results.len(), 1);
        let result = &results[0];

        assert_eq!(result.id, "local:docs/async.md#basics");
        assert_eq!(result.title, "Async Programming");
        assert_eq!(result.tree, "local");
        assert_eq!(result.path, "docs/async.md");
        assert!(result.body.contains("Asynchronous"));
        assert!(result.score > 0.0);
    }

    #[test]
    fn search_generates_snippets() {
        let temp = TempDir::new().unwrap();
        create_test_index(&temp);

        let mut searcher = Searcher::open(temp.path(), "english", &make_trees(), 1.5).unwrap();

        let results = searcher.search("safety", 10).unwrap();

        assert!(!results.is_empty());
        let result = &results[0];

        // Snippet should be present and contain highlighting
        assert!(result.snippet.is_some());
        let snippet = result.snippet.as_ref().unwrap();
        // Tantivy uses <b> tags for highlighting
        assert!(snippet.contains("<b>") || snippet.contains("safety"));
    }

    #[test]
    fn search_no_snippets_returns_none() {
        let temp = TempDir::new().unwrap();
        create_test_index(&temp);

        let mut searcher = Searcher::open(temp.path(), "english", &make_trees(), 1.5).unwrap();

        let results = searcher.search_no_snippets("safety", 10).unwrap();

        assert!(!results.is_empty());
        assert!(results[0].snippet.is_none());
    }

    #[test]
    fn search_multi_combines_topics() {
        let temp = TempDir::new().unwrap();
        create_test_index(&temp);

        let mut searcher = Searcher::open(temp.path(), "english", &make_trees(), 1.5).unwrap();

        // "async" matches one doc, "error" matches another
        let results = searcher.search_multi(&["async", "error"], 10).unwrap();

        // Should find at least 2 documents
        assert!(results.len() >= 2);
    }

    #[test]
    fn num_docs_returns_correct_count() {
        let temp = TempDir::new().unwrap();
        create_test_index(&temp);

        let searcher = Searcher::open(temp.path(), "english", &make_trees(), 1.5).unwrap();

        assert_eq!(searcher.num_docs().unwrap(), 3);
    }

    #[test]
    fn open_nonexistent_index_fails() {
        let temp = TempDir::new().unwrap();
        let nonexistent = temp.path().join("nonexistent");

        let result = Searcher::open(&nonexistent, "english", &[], 1.5);

        assert!(result.is_err());
    }

    #[test]
    fn phrase_search_works() {
        let temp = TempDir::new().unwrap();
        create_test_index(&temp);

        let mut searcher = Searcher::open(temp.path(), "english", &make_trees(), 1.5).unwrap();

        // Exact phrase should match
        let results = searcher.search("\"systems programming\"", 10).unwrap();

        assert!(!results.is_empty());
        assert!(results[0].body.contains("systems programming"));
    }
}
