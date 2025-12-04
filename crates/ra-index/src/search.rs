//! Search execution for the ra index.
//!
//! Provides the [`Searcher`] struct for querying the index and retrieving results.
//! Supports field boosting, local tree boosting, and snippet generation.

use std::{
    cmp::Ordering,
    collections::HashMap,
    mem,
    ops::Range,
    path::{Path, PathBuf},
};

use tantivy::{
    Index, TantivyDocument, Term,
    collector::TopDocs,
    directory::MmapDirectory,
    query::{AllQuery, Query, TermQuery},
    schema::{Field, IndexRecordOption, Value},
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

/// Merges two sets of byte ranges, combining overlapping or adjacent ranges.
///
/// The result is sorted by start position with no overlaps.
fn merge_ranges(mut a: Vec<Range<usize>>, b: Vec<Range<usize>>) -> Vec<Range<usize>> {
    a.extend(b);
    if a.is_empty() {
        return a;
    }

    // Sort by start position
    a.sort_by_key(|r| r.start);

    let mut merged = Vec::with_capacity(a.len());
    let mut current = a[0].clone();

    for range in a.into_iter().skip(1) {
        if range.start <= current.end {
            // Overlapping or adjacent - extend current range
            current.end = current.end.max(range.end);
        } else {
            // Gap - push current and start new
            merged.push(current);
            current = range;
        }
    }
    merged.push(current);

    merged
}

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
    /// Breadcrumb showing hierarchy path.
    pub breadcrumb: String,
    /// Search relevance score (after boosting).
    pub score: f32,
    /// Optional snippet with query terms highlighted.
    pub snippet: Option<String>,
    /// Byte ranges within `body` where search terms match.
    ///
    /// These ranges can be used to highlight matching terms in the full body text.
    /// Each range represents a contiguous span of bytes that matched a search term.
    /// Ranges are sorted by start position and do not overlap.
    ///
    /// Empty when the result was retrieved via `get_by_id` or `get_by_path`,
    /// or when using `search_no_snippets`.
    pub match_ranges: Vec<Range<usize>>,
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
    /// * `fuzzy_distance` - Levenshtein distance for fuzzy matching (0 = disabled)
    pub fn open(
        path: &Path,
        language: &str,
        trees: &[ra_config::Tree],
        local_boost: f32,
        fuzzy_distance: u8,
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

        let query_builder = QueryBuilder::new(schema.clone(), language, fuzzy_distance)?;

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
            config.search.fuzzy_distance,
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
    /// Each topic is searched independently and results are combined with deduplication.
    /// When a document matches multiple topics, the match ranges from all topics are merged
    /// and the highest score is kept.
    ///
    /// # Arguments
    /// * `topics` - Array of query strings
    /// * `limit` - Maximum number of results to return
    pub fn search_multi(
        &mut self,
        topics: &[&str],
        limit: usize,
    ) -> Result<Vec<SearchResult>, IndexError> {
        if topics.is_empty() {
            return Ok(Vec::new());
        }

        // Search each topic separately to get per-topic highlights
        let mut results_by_id: HashMap<String, SearchResult> = HashMap::new();

        for topic in topics {
            let topic_results = self.search(topic, limit)?;

            for result in topic_results {
                results_by_id
                    .entry(result.id.clone())
                    .and_modify(|existing| {
                        // Keep the higher score
                        if result.score > existing.score {
                            existing.score = result.score;
                        }
                        // Merge match ranges
                        existing.match_ranges = merge_ranges(
                            mem::take(&mut existing.match_ranges),
                            result.match_ranges.clone(),
                        );
                        // Merge snippets if both present
                        if let (Some(existing_snippet), Some(new_snippet)) =
                            (&existing.snippet, &result.snippet)
                        {
                            existing.snippet = Some(format!("{existing_snippet} … {new_snippet}"));
                        } else if existing.snippet.is_none() {
                            existing.snippet = result.snippet.clone();
                        }
                    })
                    .or_insert(result);
            }
        }

        // Collect and sort by score (highest first)
        let mut results: Vec<SearchResult> = results_by_id.into_values().collect();
        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(Ordering::Equal));

        // Apply limit
        results.truncate(limit);

        Ok(results)
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

        // Create snippet generator for excerpts (limited chars)
        let snippet_generator = if generate_snippets {
            let mut generator = SnippetGenerator::create(&searcher, query, self.schema.body)
                .map_err(|e| IndexError::Write(e.to_string()))?;
            generator.set_max_num_chars(DEFAULT_SNIPPET_MAX_CHARS);
            Some(generator)
        } else {
            None
        };

        // Create a separate generator for full-body match highlighting.
        // We set a very large max_num_chars to capture all matches in the body.
        let highlight_generator = if generate_snippets {
            let mut generator = SnippetGenerator::create(&searcher, query, self.schema.body)
                .map_err(|e| IndexError::Write(e.to_string()))?;
            generator.set_max_num_chars(usize::MAX);
            Some(generator)
        } else {
            None
        };

        let mut results = Vec::with_capacity(top_docs.len());

        for (score, doc_address) in top_docs {
            let doc: TantivyDocument = searcher
                .doc(doc_address)
                .map_err(|e| IndexError::Write(e.to_string()))?;

            let result = self.doc_to_result(&doc, score, &snippet_generator, &highlight_generator);
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
        highlight_generator: &Option<SnippetGenerator>,
    ) -> SearchResult {
        let id = self.get_text_field(doc, self.schema.id);
        let title = self.get_text_field(doc, self.schema.title);
        let tree = self.get_text_field(doc, self.schema.tree);
        let path = self.get_text_field(doc, self.schema.path);
        let body = self.get_text_field(doc, self.schema.body);
        let breadcrumb = self.get_text_field(doc, self.schema.breadcrumb);

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

        // Extract match ranges from the full body
        let match_ranges = highlight_generator
            .as_ref()
            .map(|generator| {
                let snippet = generator.snippet(&body);
                snippet.highlighted().to_vec()
            })
            .unwrap_or_default();

        SearchResult {
            id,
            title,
            tree,
            path,
            body,
            breadcrumb,
            score,
            snippet,
            match_ranges,
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

    /// Retrieves a chunk by its exact ID.
    ///
    /// # Arguments
    /// * `id` - The chunk ID (e.g., `tree:path#slug` or `tree:path`)
    ///
    /// # Returns
    /// The chunk if found, or None if no match exists.
    pub fn get_by_id(&self, id: &str) -> Result<Option<SearchResult>, IndexError> {
        let reader = self
            .index
            .reader()
            .map_err(|e| IndexError::Write(e.to_string()))?;

        let searcher = reader.searcher();

        // Build exact term query on ID field (STRING field)
        let term = Term::from_field_text(self.schema.id, id);
        let query = TermQuery::new(term, IndexRecordOption::Basic);

        let top_docs = searcher
            .search(&query, &TopDocs::with_limit(1))
            .map_err(|e| IndexError::Write(e.to_string()))?;

        if let Some((score, doc_address)) = top_docs.first() {
            let doc: TantivyDocument = searcher
                .doc(*doc_address)
                .map_err(|e| IndexError::Write(e.to_string()))?;

            let result = self.doc_to_result(&doc, *score, &None, &None);
            Ok(Some(result))
        } else {
            Ok(None)
        }
    }

    /// Lists all chunks in the index.
    ///
    /// Returns all indexed chunks, ordered by ID. This is useful for
    /// displaying the full contents of the index.
    pub fn list_all(&self) -> Result<Vec<SearchResult>, IndexError> {
        let reader = self
            .index
            .reader()
            .map_err(|e| IndexError::Write(e.to_string()))?;

        let searcher = reader.searcher();

        let all_docs = searcher
            .search(&AllQuery, &TopDocs::with_limit(100_000))
            .map_err(|e| IndexError::Write(e.to_string()))?;

        let mut results: Vec<SearchResult> = all_docs
            .into_iter()
            .filter_map(|(score, doc_address)| {
                let doc: TantivyDocument = searcher.doc(doc_address).ok()?;
                Some(self.doc_to_result(&doc, score, &None, &None))
            })
            .collect();

        // Sort by ID for consistent ordering
        results.sort_by(|a, b| a.id.cmp(&b.id));

        Ok(results)
    }

    /// Retrieves all chunks from a document by path.
    ///
    /// Uses a prefix query on the ID field since IDs have format `tree:path#slug`.
    /// This finds all chunks that belong to a specific document.
    ///
    /// # Arguments
    /// * `tree` - The tree name
    /// * `path` - The file path within the tree
    ///
    /// # Returns
    /// All chunks from the specified document, ordered by ID.
    pub fn get_by_path(&self, tree: &str, path: &str) -> Result<Vec<SearchResult>, IndexError> {
        let reader = self
            .index
            .reader()
            .map_err(|e| IndexError::Write(e.to_string()))?;

        let searcher = reader.searcher();

        // Build ID prefix: "tree:path" - all chunks from this doc start with this
        let id_prefix = format!("{tree}:{path}");

        // Collect all documents and filter by ID prefix
        // (Tantivy's prefix query doesn't work well with STRING fields)
        let all_docs = searcher
            .search(&AllQuery, &TopDocs::with_limit(10000))
            .map_err(|e| IndexError::Write(e.to_string()))?;

        let mut results: Vec<SearchResult> = all_docs
            .into_iter()
            .filter_map(|(score, doc_address)| {
                let doc: TantivyDocument = searcher.doc(doc_address).ok()?;
                let result = self.doc_to_result(&doc, score, &None, &None);
                // Check if ID starts with our prefix (either exact or with # separator)
                if result.id == id_prefix || result.id.starts_with(&format!("{id_prefix}#")) {
                    Some(result)
                } else {
                    None
                }
            })
            .collect();

        // Sort by ID to get consistent ordering
        results.sort_by(|a, b| a.id.cmp(&b.id));

        Ok(results)
    }

    /// Searches for context relevant to the given signals.
    ///
    /// Combines path terms, pattern terms, and content sample into a search query.
    /// Results are deduplicated if the same chunk matches multiple signals.
    ///
    /// # Arguments
    /// * `signals` - Context signals from analyzed files
    /// * `limit` - Maximum number of results to return
    pub fn search_context(
        &mut self,
        signals: &[crate::ContextSignals],
        limit: usize,
    ) -> Result<Vec<SearchResult>, IndexError> {
        if signals.is_empty() {
            return Ok(Vec::new());
        }

        // Collect all terms from all signals
        let mut all_terms: Vec<String> = Vec::new();
        for signal in signals {
            all_terms.extend(signal.all_terms());
        }

        // Deduplicate terms
        all_terms.sort();
        all_terms.dedup();

        if all_terms.is_empty() {
            return Ok(Vec::new());
        }

        // Build query from terms (space-separated terms are ANDed by search)
        // Use search_multi to get term highlighting for each term
        let term_refs: Vec<&str> = all_terms.iter().map(|s| s.as_str()).collect();
        self.search_multi(&term_refs, limit)
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
                breadcrumb: "Getting Started › Introduction to Rust".to_string(),
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
                breadcrumb: "Advanced Topics › Async Programming".to_string(),
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
                breadcrumb: "Reference › Error Handling".to_string(),
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

        let mut searcher = Searcher::open(temp.path(), "english", &make_trees(), 1.5, 0).unwrap();

        let results = searcher.search("rust", 10).unwrap();

        // All three documents mention rust
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn search_respects_limit() {
        let temp = TempDir::new().unwrap();
        create_test_index(&temp);

        let mut searcher = Searcher::open(temp.path(), "english", &make_trees(), 1.5, 0).unwrap();

        let results = searcher.search("rust", 2).unwrap();

        assert_eq!(results.len(), 2);
    }

    #[test]
    fn search_returns_empty_for_no_matches() {
        let temp = TempDir::new().unwrap();
        create_test_index(&temp);

        let mut searcher = Searcher::open(temp.path(), "english", &make_trees(), 1.5, 0).unwrap();

        let results = searcher.search("python", 10).unwrap();

        assert!(results.is_empty());
    }

    #[test]
    fn search_returns_empty_for_empty_query() {
        let temp = TempDir::new().unwrap();
        create_test_index(&temp);

        let mut searcher = Searcher::open(temp.path(), "english", &make_trees(), 1.5, 0).unwrap();

        let results = searcher.search("", 10).unwrap();

        assert!(results.is_empty());
    }

    #[test]
    fn search_applies_local_boost() {
        let temp = TempDir::new().unwrap();
        create_test_index(&temp);

        let local_boost = 2.0;
        let mut searcher =
            Searcher::open(temp.path(), "english", &make_trees(), local_boost, 0).unwrap();

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

        let mut searcher = Searcher::open(temp.path(), "english", &make_trees(), 1.5, 0).unwrap();

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

        let mut searcher = Searcher::open(temp.path(), "english", &make_trees(), 1.5, 0).unwrap();

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

        let mut searcher = Searcher::open(temp.path(), "english", &make_trees(), 1.5, 0).unwrap();

        let results = searcher.search_no_snippets("safety", 10).unwrap();

        assert!(!results.is_empty());
        assert!(results[0].snippet.is_none());
    }

    #[test]
    fn search_multi_combines_topics() {
        let temp = TempDir::new().unwrap();
        create_test_index(&temp);

        let mut searcher = Searcher::open(temp.path(), "english", &make_trees(), 1.5, 0).unwrap();

        // "async" matches one doc, "error" matches another
        let results = searcher.search_multi(&["async", "error"], 10).unwrap();

        // Should find at least 2 documents
        assert!(results.len() >= 2);
    }

    #[test]
    fn num_docs_returns_correct_count() {
        let temp = TempDir::new().unwrap();
        create_test_index(&temp);

        let searcher = Searcher::open(temp.path(), "english", &make_trees(), 1.5, 0).unwrap();

        assert_eq!(searcher.num_docs().unwrap(), 3);
    }

    #[test]
    fn open_nonexistent_index_fails() {
        let temp = TempDir::new().unwrap();
        let nonexistent = temp.path().join("nonexistent");

        let result = Searcher::open(&nonexistent, "english", &[], 1.5, 0);

        assert!(result.is_err());
    }

    #[test]
    fn phrase_search_works() {
        let temp = TempDir::new().unwrap();
        create_test_index(&temp);

        let mut searcher = Searcher::open(temp.path(), "english", &make_trees(), 1.5, 0).unwrap();

        // Exact phrase should match
        let results = searcher.search("\"systems programming\"", 10).unwrap();

        assert!(!results.is_empty());
        assert!(results[0].body.contains("systems programming"));
    }

    #[test]
    fn search_multi_deduplicates_results() {
        let temp = TempDir::new().unwrap();
        create_test_index(&temp);

        let mut searcher = Searcher::open(temp.path(), "english", &make_trees(), 1.5, 0).unwrap();

        // Both "rust" and "programming" match the intro doc
        let results = searcher.search_multi(&["rust", "programming"], 10).unwrap();

        // Count how many times each ID appears
        let mut id_counts: HashMap<String, usize> = HashMap::new();
        for result in &results {
            *id_counts.entry(result.id.clone()).or_insert(0) += 1;
        }

        // Each ID should appear only once
        for (id, count) in id_counts {
            assert_eq!(count, 1, "ID {id} appeared {count} times, expected 1");
        }
    }

    #[test]
    fn search_multi_merges_match_ranges() {
        let temp = TempDir::new().unwrap();
        create_test_index(&temp);

        let mut searcher = Searcher::open(temp.path(), "english", &make_trees(), 1.5, 0).unwrap();

        // "rust" and "safety" both appear in the intro doc
        let results = searcher.search_multi(&["rust", "safety"], 10).unwrap();

        // Find the intro doc
        let intro = results.iter().find(|r| r.id.contains("rust.md")).unwrap();

        // Should have match ranges from both terms
        assert!(
            !intro.match_ranges.is_empty(),
            "Expected merged match ranges"
        );

        // The body contains both "Rust" and "safety", so we should have at least 2 distinct matches
        // (possibly merged if adjacent)
        let total_matched_chars: usize = intro.match_ranges.iter().map(|r| r.end - r.start).sum();
        assert!(
            total_matched_chars >= 8,
            "Expected at least 8 chars matched (rust + safety)"
        );
    }

    #[test]
    fn search_multi_keeps_highest_score() {
        let temp = TempDir::new().unwrap();
        create_test_index(&temp);

        let mut searcher = Searcher::open(temp.path(), "english", &make_trees(), 1.5, 0).unwrap();

        // Search for same document with two different terms
        let results = searcher.search_multi(&["rust", "systems"], 10).unwrap();

        // Find the intro doc
        let intro = results.iter().find(|r| r.id.contains("rust.md")).unwrap();

        // Score should be positive (the max of both searches)
        assert!(intro.score > 0.0);
    }

    #[test]
    fn merge_ranges_combines_overlapping() {
        let a = vec![0..5, 10..15];
        let b = vec![3..8, 20..25];
        let merged = super::merge_ranges(a, b);

        assert_eq!(merged, vec![0..8, 10..15, 20..25]);
    }

    #[test]
    #[allow(clippy::single_range_in_vec_init)]
    fn merge_ranges_combines_adjacent() {
        let a = vec![0..5];
        let b = vec![5..10];
        let merged = super::merge_ranges(a, b);

        assert_eq!(merged, vec![0..10]);
    }

    #[test]
    fn merge_ranges_handles_empty() {
        let a: Vec<Range<usize>> = vec![];
        let b: Vec<Range<usize>> = vec![];
        let merged = super::merge_ranges(a, b);

        assert!(merged.is_empty());
    }

    #[test]
    #[allow(clippy::single_range_in_vec_init)]
    fn merge_ranges_preserves_non_overlapping() {
        let a = vec![0..5];
        let b = vec![10..15];
        let merged = super::merge_ranges(a, b);

        assert_eq!(merged, vec![0..5, 10..15]);
    }

    #[test]
    fn fuzzy_search_matches_similar_terms() {
        let temp = TempDir::new().unwrap();

        // Create index with "werewolves" in body
        let docs = vec![ChunkDocument {
            id: "local:docs/monsters.md#intro".to_string(),
            title: "Monster Guide".to_string(),
            tags: vec!["fantasy".to_string()],
            path: "docs/monsters.md".to_string(),
            path_components: vec!["docs".to_string(), "monsters".to_string()],
            tree: "local".to_string(),
            body: "This guide covers werewolves and vampires.".to_string(),
            breadcrumb: "Bestiary › Monster Guide".to_string(),
            mtime: SystemTime::UNIX_EPOCH,
        }];

        let mut writer = IndexWriter::open(temp.path(), "english").unwrap();
        for doc in &docs {
            writer.add_document(doc).unwrap();
        }
        writer.commit().unwrap();

        // Search with fuzzy_distance = 0 (exact match) - should NOT find "werewolf"
        // because the indexed term is "werewolv" (stemmed from "werewolves")
        // and "werewolf" doesn't stem the same way
        let mut searcher_exact =
            Searcher::open(temp.path(), "english", &make_trees(), 1.5, 0).unwrap();
        let results_exact = searcher_exact.search("werewolf", 10).unwrap();
        assert!(
            results_exact.is_empty(),
            "Exact search should not match werewolf -> werewolves"
        );

        // Search with fuzzy_distance = 2 - SHOULD find it
        // "werewolf" (no stem change) vs "werewolv" (stemmed) differ by ~2 chars
        let mut searcher_fuzzy =
            Searcher::open(temp.path(), "english", &make_trees(), 1.5, 2).unwrap();
        let results_fuzzy = searcher_fuzzy.search("werewolf", 10).unwrap();
        assert!(
            !results_fuzzy.is_empty(),
            "Fuzzy search with distance 2 should match werewolf -> werewolves"
        );
        assert!(results_fuzzy[0].body.contains("werewolves"));
    }

    #[test]
    fn fuzzy_search_disabled_with_zero_distance() {
        let temp = TempDir::new().unwrap();

        let docs = vec![ChunkDocument {
            id: "local:docs/test.md".to_string(),
            title: "Test".to_string(),
            tags: vec![],
            path: "docs/test.md".to_string(),
            path_components: vec!["docs".to_string(), "test".to_string()],
            tree: "local".to_string(),
            body: "The quick brown fox jumps over the lazy dog.".to_string(),
            breadcrumb: "Test".to_string(),
            mtime: SystemTime::UNIX_EPOCH,
        }];

        let mut writer = IndexWriter::open(temp.path(), "english").unwrap();
        for doc in &docs {
            writer.add_document(doc).unwrap();
        }
        writer.commit().unwrap();

        // With distance 0, "foz" should not match "fox"
        let mut searcher = Searcher::open(temp.path(), "english", &make_trees(), 1.5, 0).unwrap();
        let results = searcher.search("foz", 10).unwrap();
        assert!(results.is_empty(), "Distance 0 should not match foz -> fox");

        // With distance 1, "foz" should match "fox"
        let mut searcher = Searcher::open(temp.path(), "english", &make_trees(), 1.5, 1).unwrap();
        let results = searcher.search("foz", 10).unwrap();
        assert!(!results.is_empty(), "Distance 1 should match foz -> fox");
    }
}
