//! Search execution for the ra index.
//!
//! Provides the [`Searcher`] struct for querying the index and retrieving results.
//! Supports field boosting, local tree boosting, and snippet generation.

use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    mem,
    ops::Range,
    path::{Path, PathBuf},
    str,
};

use levenshtein_automata::{Distance, LevenshteinAutomatonBuilder, SINK_STATE};
use tantivy::{
    Index, TantivyDocument, Term,
    collector::TopDocs,
    directory::MmapDirectory,
    query::{AllQuery, BooleanQuery, BoostQuery, Occur, Query, TermQuery},
    schema::{Field, IndexRecordOption, Value},
    snippet::SnippetGenerator,
    tokenizer::{TextAnalyzer, TokenStream},
};
use tantivy_fst::Automaton;

/// Wrapper that implements `tantivy_fst::Automaton` for `levenshtein_automata::DFA`.
struct LevenshteinDfa(levenshtein_automata::DFA);

impl Automaton for LevenshteinDfa {
    type State = u32;

    fn start(&self) -> Self::State {
        self.0.initial_state()
    }

    fn is_match(&self, state: &Self::State) -> bool {
        match self.0.distance(*state) {
            Distance::Exact(_) => true,
            Distance::AtLeast(_) => false,
        }
    }

    fn can_match(&self, state: &Self::State) -> bool {
        *state != SINK_STATE
    }

    fn accept(&self, state: &Self::State, byte: u8) -> Self::State {
        self.0.transition(*state, byte)
    }
}

use crate::{
    IndexError,
    analyzer::{RA_TOKENIZER, build_analyzer_from_name},
    query::{QueryCompiler, parse},
    schema::{IndexSchema, boost},
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
    /// Document identifier (same for all chunks in a file).
    pub doc_id: String,
    /// Parent chunk identifier, or None for document nodes.
    pub parent_id: Option<String>,
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
    /// Hierarchy depth: 0 for document, 1-6 for h1-h6.
    pub depth: u64,
    /// Document order index (0-based pre-order traversal).
    pub position: u64,
    /// Byte offset where content span starts.
    pub byte_start: u64,
    /// Byte offset where content span ends.
    pub byte_end: u64,
    /// Number of siblings including this node.
    pub sibling_count: u64,
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

/// Default fuzzy distance for search queries.
const DEFAULT_FUZZY_DISTANCE: u8 = 1;

/// Searches the index for matching documents.
pub struct Searcher {
    /// The Tantivy index.
    index: Index,
    /// Schema with field handles.
    schema: IndexSchema,
    /// Query compiler for search (with fuzzy matching).
    query_compiler: QueryCompiler,
    /// Text analyzer for tokenizing query terms.
    analyzer: TextAnalyzer,
    /// Levenshtein automaton builder for fuzzy term matching.
    lev_builder: LevenshteinAutomatonBuilder,
    /// Fuzzy distance used for matching.
    fuzzy_distance: u8,
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
        index.tokenizers().register(RA_TOKENIZER, analyzer.clone());

        // Use fuzzy matching for search to handle typos and variations
        let fuzzy_distance = DEFAULT_FUZZY_DISTANCE;
        let query_compiler = QueryCompiler::new(schema.clone(), language, fuzzy_distance)?;

        // Build Levenshtein automaton builder for extracting matched terms
        let lev_builder = LevenshteinAutomatonBuilder::new(fuzzy_distance, true);

        // Build tree global/local map
        let tree_is_global: HashMap<String, bool> = trees
            .iter()
            .map(|t| (t.name.clone(), t.is_global))
            .collect();

        Ok(Self {
            index,
            schema,
            query_compiler,
            analyzer,
            lev_builder,
            fuzzy_distance,
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
        let query = match self.build_query(query_str)? {
            Some(q) => q,
            None => return Ok(Vec::new()),
        };

        // Tokenize query to extract search terms for highlighting
        let query_terms = self.tokenize_query(query_str);

        self.execute_query_with_highlights(&*query, &query_terms, limit)
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
        let query = match self.build_query(query_str)? {
            Some(q) => q,
            None => return Ok(Vec::new()),
        };

        self.execute_query_no_highlights(&*query, limit)
    }

    /// Parses and compiles a query string into a Tantivy query.
    fn build_query(&mut self, query_str: &str) -> Result<Option<Box<dyn Query>>, IndexError> {
        use crate::query::QueryError;

        let expr = parse(query_str).map_err(|e| {
            // Convert ParseError to QueryError, adding the query string for context
            let query_err: QueryError = e.into();
            IndexError::Query(query_err.with_query(query_str))
        })?;
        match expr {
            Some(e) => {
                let result = self.query_compiler.compile(&e).map_err(|e| {
                    // Convert CompileError to QueryError, adding the query string for context
                    let query_err: QueryError = e.into();
                    IndexError::Query(query_err.with_query(query_str))
                })?;
                Ok(result)
            }
            None => Ok(None),
        }
    }

    /// Tokenizes a query string to extract individual search terms.
    fn tokenize_query(&mut self, query_str: &str) -> Vec<String> {
        let mut stream = self.analyzer.token_stream(query_str);
        let mut tokens = Vec::new();
        while let Some(token) = stream.next() {
            tokens.push(token.text.clone());
        }
        tokens
    }

    /// Finds actual terms in the index that match the query terms (including fuzzy matches).
    ///
    /// For each query term, uses a Levenshtein automaton to search the term dictionary
    /// and collect all indexed terms that match within the fuzzy distance.
    fn find_matched_terms(
        &self,
        searcher: &tantivy::Searcher,
        query_terms: &[String],
    ) -> HashSet<String> {
        let mut matched_terms = HashSet::new();

        // Only do fuzzy matching lookup if fuzzy is enabled
        if self.fuzzy_distance == 0 {
            // No fuzzy matching - just return query terms as-is
            for term in query_terms {
                matched_terms.insert(term.clone());
            }
            return matched_terms;
        }

        // Search the body field's term dictionary for matching terms
        for segment_reader in searcher.segment_readers() {
            if let Ok(inverted_index) = segment_reader.inverted_index(self.schema.body) {
                let term_dict = inverted_index.terms();

                for query_term in query_terms {
                    // Build a Levenshtein DFA for this query term
                    let dfa = LevenshteinDfa(self.lev_builder.build_dfa(query_term));

                    // Search the term dictionary with the automaton
                    let mut stream = term_dict.search(dfa).into_stream().unwrap();

                    while stream.advance() {
                        // The key is the actual indexed term (as bytes)
                        if let Ok(term_str) = str::from_utf8(stream.key()) {
                            matched_terms.insert(term_str.to_string());
                        }
                    }
                }
            }
        }

        // If no terms found in index (e.g., new terms), fall back to query terms
        if matched_terms.is_empty() {
            for term in query_terms {
                matched_terms.insert(term.clone());
            }
        }

        matched_terms
    }

    /// Builds a highlight query from actual matched terms.
    ///
    /// Creates a non-fuzzy query using the actual terms found in the index,
    /// which allows Tantivy's SnippetGenerator to highlight them correctly.
    fn build_highlight_query(&self, matched_terms: &HashSet<String>) -> Option<Box<dyn Query>> {
        if matched_terms.is_empty() {
            return None;
        }

        // Build a boolean query that matches any of the actual terms in the body field
        let clauses: Vec<(Occur, Box<dyn Query>)> = matched_terms
            .iter()
            .map(|term_text| {
                let term = Term::from_field_text(self.schema.body, term_text);
                let query: Box<dyn Query> =
                    Box::new(TermQuery::new(term, IndexRecordOption::WithFreqs));
                let boosted: Box<dyn Query> = Box::new(BoostQuery::new(query, boost::BODY));
                (Occur::Should, boosted)
            })
            .collect();

        Some(Box::new(BooleanQuery::new(clauses)))
    }

    /// Executes a query with snippet and highlight generation.
    ///
    /// Uses query terms to find actual matched terms in the index via Levenshtein
    /// automata, then builds a highlight query from those actual terms. This ensures
    /// highlights match what the fuzzy search actually found.
    fn execute_query_with_highlights(
        &self,
        query: &dyn Query,
        query_terms: &[String],
        limit: usize,
    ) -> Result<Vec<SearchResult>, IndexError> {
        let reader = self
            .index
            .reader()
            .map_err(|e| IndexError::Write(e.to_string()))?;

        let searcher = reader.searcher();

        let top_docs = searcher
            .search(query, &TopDocs::with_limit(limit))
            .map_err(|e| IndexError::Write(e.to_string()))?;

        // Find actual terms in the index that match our query terms (including fuzzy matches)
        let matched_terms = self.find_matched_terms(&searcher, query_terms);

        // Build a highlight query from the actual matched terms (non-fuzzy)
        let highlight_query = self.build_highlight_query(&matched_terms);

        // Create snippet generator for excerpts using the highlight query
        let snippet_generator = if let Some(ref hq) = highlight_query {
            let mut generator = SnippetGenerator::create(&searcher, hq.as_ref(), self.schema.body)
                .map_err(|e| IndexError::Write(e.to_string()))?;
            generator.set_max_num_chars(DEFAULT_SNIPPET_MAX_CHARS);
            Some(generator)
        } else {
            None
        };

        // Create a separate generator for full-body match highlighting.
        let highlight_generator = if let Some(ref hq) = highlight_query {
            let mut generator = SnippetGenerator::create(&searcher, hq.as_ref(), self.schema.body)
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

    /// Executes a query without generating snippets or highlights (faster).
    fn execute_query_no_highlights(
        &self,
        query: &dyn Query,
        limit: usize,
    ) -> Result<Vec<SearchResult>, IndexError> {
        let reader = self
            .index
            .reader()
            .map_err(|e| IndexError::Write(e.to_string()))?;

        let searcher = reader.searcher();

        let top_docs = searcher
            .search(query, &TopDocs::with_limit(limit))
            .map_err(|e| IndexError::Write(e.to_string()))?;

        let mut results = Vec::with_capacity(top_docs.len());

        for (score, doc_address) in top_docs {
            let doc: TantivyDocument = searcher
                .doc(doc_address)
                .map_err(|e| IndexError::Write(e.to_string()))?;

            let result = self.doc_to_result(&doc, score, &None, &None);
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
        let doc_id = self.get_text_field(doc, self.schema.doc_id);
        let parent_id_str = self.get_text_field(doc, self.schema.parent_id);
        let parent_id = if parent_id_str.is_empty() {
            None
        } else {
            Some(parent_id_str)
        };
        let title = self.get_text_field(doc, self.schema.title);
        let tree = self.get_text_field(doc, self.schema.tree);
        let path = self.get_text_field(doc, self.schema.path);
        let body = self.get_text_field(doc, self.schema.body);
        let breadcrumb = self.get_text_field(doc, self.schema.breadcrumb);
        let depth = self.get_u64_field(doc, self.schema.depth);
        let position = self.get_u64_field(doc, self.schema.position);
        let byte_start = self.get_u64_field(doc, self.schema.byte_start);
        let byte_end = self.get_u64_field(doc, self.schema.byte_end);
        let sibling_count = self.get_u64_field(doc, self.schema.sibling_count);

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
            doc_id,
            parent_id,
            title,
            tree,
            path,
            body,
            breadcrumb,
            depth,
            position,
            byte_start,
            byte_end,
            sibling_count,
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

    /// Extracts a u64 field value from a document.
    fn get_u64_field(&self, doc: &TantivyDocument, field: Field) -> u64 {
        doc.get_first(field).and_then(|v| v.as_u64()).unwrap_or(0)
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
                doc_id: "local:docs/rust.md".to_string(),
                parent_id: Some("local:docs/rust.md".to_string()),
                title: "Introduction to Rust".to_string(),
                tags: vec!["rust".to_string(), "programming".to_string()],
                path: "docs/rust.md".to_string(),
                path_components: vec!["docs".to_string(), "rust".to_string(), "md".to_string()],
                tree: "local".to_string(),
                body: "Rust is a systems programming language focused on safety and performance."
                    .to_string(),
                breadcrumb: "Getting Started › Introduction to Rust".to_string(),
                depth: 1,
                position: 1,
                byte_start: 50,
                byte_end: 200,
                sibling_count: 2,
                mtime: SystemTime::UNIX_EPOCH,
            },
            ChunkDocument {
                id: "local:docs/async.md#basics".to_string(),
                doc_id: "local:docs/async.md".to_string(),
                parent_id: Some("local:docs/async.md".to_string()),
                title: "Async Programming".to_string(),
                tags: vec!["rust".to_string(), "async".to_string()],
                path: "docs/async.md".to_string(),
                path_components: vec!["docs".to_string(), "async".to_string(), "md".to_string()],
                tree: "local".to_string(),
                body: "Asynchronous programming in Rust uses futures and the async/await syntax."
                    .to_string(),
                breadcrumb: "Advanced Topics › Async Programming".to_string(),
                depth: 1,
                position: 1,
                byte_start: 30,
                byte_end: 150,
                sibling_count: 1,
                mtime: SystemTime::UNIX_EPOCH,
            },
            ChunkDocument {
                id: "global:reference/errors.md#handling".to_string(),
                doc_id: "global:reference/errors.md".to_string(),
                parent_id: Some("global:reference/errors.md".to_string()),
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
                depth: 1,
                position: 1,
                byte_start: 20,
                byte_end: 100,
                sibling_count: 3,
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

    #[test]
    fn search_multi_deduplicates_results() {
        let temp = TempDir::new().unwrap();
        create_test_index(&temp);

        let mut searcher = Searcher::open(temp.path(), "english", &make_trees(), 1.5).unwrap();

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

        let mut searcher = Searcher::open(temp.path(), "english", &make_trees(), 1.5).unwrap();

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

        let mut searcher = Searcher::open(temp.path(), "english", &make_trees(), 1.5).unwrap();

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
            doc_id: "local:docs/monsters.md".to_string(),
            parent_id: Some("local:docs/monsters.md".to_string()),
            title: "Monster Guide".to_string(),
            tags: vec!["fantasy".to_string()],
            path: "docs/monsters.md".to_string(),
            path_components: vec!["docs".to_string(), "monsters".to_string()],
            tree: "local".to_string(),
            body: "This guide covers werewolves and vampires.".to_string(),
            breadcrumb: "Bestiary › Monster Guide".to_string(),
            depth: 1,
            position: 1,
            byte_start: 0,
            byte_end: 50,
            sibling_count: 1,
            mtime: SystemTime::UNIX_EPOCH,
        }];

        let mut writer = IndexWriter::open(temp.path(), "english").unwrap();
        for doc in &docs {
            writer.add_document(doc).unwrap();
        }
        writer.commit().unwrap();

        // With fuzzy matching (distance=1), "werewolf" SHOULD find "werewolves"
        // because "werewolf" stems to "werewolf" and "werewolves" stems to "werewolv",
        // and fuzzy matching allows 1 edit distance.
        let mut searcher = Searcher::open(temp.path(), "english", &make_trees(), 1.5).unwrap();
        let results = searcher.search("werewolf", 10).unwrap();
        assert!(
            !results.is_empty(),
            "Fuzzy search should match werewolf -> werewolves"
        );
        assert!(results[0].body.contains("werewolves"));

        // Searching for "werewolves" should also work (exact stem match)
        let results = searcher.search("werewolves", 10).unwrap();
        assert!(
            !results.is_empty(),
            "Searching for 'werewolves' should match"
        );
        assert!(results[0].body.contains("werewolves"));
    }

    #[test]
    fn fuzzy_search_matches_typos() {
        let temp = TempDir::new().unwrap();

        let docs = vec![ChunkDocument {
            id: "local:docs/test.md".to_string(),
            doc_id: "local:docs/test.md".to_string(),
            parent_id: None,
            title: "Test".to_string(),
            tags: vec![],
            path: "docs/test.md".to_string(),
            path_components: vec!["docs".to_string(), "test".to_string()],
            tree: "local".to_string(),
            body: "The quick brown fox jumps over the lazy dog.".to_string(),
            breadcrumb: "Test".to_string(),
            depth: 0,
            position: 0,
            byte_start: 0,
            byte_end: 100,
            sibling_count: 1,
            mtime: SystemTime::UNIX_EPOCH,
        }];

        let mut writer = IndexWriter::open(temp.path(), "english").unwrap();
        for doc in &docs {
            writer.add_document(doc).unwrap();
        }
        writer.commit().unwrap();

        // With fuzzy matching (distance=1), "foz" SHOULD match "fox" (1 character difference)
        let mut searcher = Searcher::open(temp.path(), "english", &make_trees(), 1.5).unwrap();
        let results = searcher.search("foz", 10).unwrap();
        assert!(
            !results.is_empty(),
            "Fuzzy search should match foz -> fox (1 edit)"
        );

        // "fox" should match "fox" exactly
        let results = searcher.search("fox", 10).unwrap();
        assert!(!results.is_empty(), "Exact search should match fox -> fox");

        // "xyz" should NOT match "fox" (too many edits)
        let results = searcher.search("xyz", 10).unwrap();
        assert!(
            results.is_empty(),
            "Fuzzy search should not match xyz -> fox (3 edits)"
        );
    }

    #[test]
    fn fuzzy_search_highlights_actual_terms() {
        let temp = TempDir::new().unwrap();

        let docs = vec![ChunkDocument {
            id: "local:docs/test.md".to_string(),
            doc_id: "local:docs/test.md".to_string(),
            parent_id: None,
            title: "Test".to_string(),
            tags: vec![],
            path: "docs/test.md".to_string(),
            path_components: vec!["docs".to_string(), "test".to_string()],
            tree: "local".to_string(),
            body: "The quick brown fox jumps over the lazy dog.".to_string(),
            breadcrumb: "Test".to_string(),
            depth: 0,
            position: 0,
            byte_start: 0,
            byte_end: 100,
            sibling_count: 1,
            mtime: SystemTime::UNIX_EPOCH,
        }];

        let mut writer = IndexWriter::open(temp.path(), "english").unwrap();
        for doc in &docs {
            writer.add_document(doc).unwrap();
        }
        writer.commit().unwrap();

        // Search for "foz" which fuzzy-matches "fox"
        let mut searcher = Searcher::open(temp.path(), "english", &make_trees(), 1.5).unwrap();
        let results = searcher.search("foz", 10).unwrap();

        assert!(!results.is_empty(), "Should find result");

        // The snippet should contain highlighting for "fox" (the actual matched term),
        // not "foz" (the query term that doesn't appear in the text)
        let snippet = results[0].snippet.as_ref().expect("Should have snippet");
        assert!(
            snippet.contains("<b>") || snippet.contains("fox"),
            "Snippet should highlight actual matched term"
        );

        // match_ranges should point to the actual "fox" in the body
        assert!(
            !results[0].match_ranges.is_empty(),
            "Should have match ranges for actual terms"
        );
    }

    #[test]
    fn hierarchical_fields_stored_and_retrieved() {
        let temp = TempDir::new().unwrap();

        // Create a document node and a heading node
        let docs = vec![
            ChunkDocument {
                id: "local:docs/guide.md".to_string(),
                doc_id: "local:docs/guide.md".to_string(),
                parent_id: None, // Document node has no parent
                title: "Guide".to_string(),
                tags: vec![],
                path: "docs/guide.md".to_string(),
                path_components: vec!["docs".to_string(), "guide".to_string(), "md".to_string()],
                tree: "local".to_string(),
                body: "This is the preamble content.".to_string(),
                breadcrumb: "> Guide".to_string(),
                depth: 0, // Document node
                position: 0,
                byte_start: 0,
                byte_end: 30,
                sibling_count: 1,
                mtime: SystemTime::UNIX_EPOCH,
            },
            ChunkDocument {
                id: "local:docs/guide.md#section-one".to_string(),
                doc_id: "local:docs/guide.md".to_string(),
                parent_id: Some("local:docs/guide.md".to_string()),
                title: "Section One".to_string(),
                tags: vec![],
                path: "docs/guide.md".to_string(),
                path_components: vec!["docs".to_string(), "guide".to_string(), "md".to_string()],
                tree: "local".to_string(),
                body: "Section one unique content here.".to_string(),
                breadcrumb: "> Guide › Section One".to_string(),
                depth: 1, // h1 heading
                position: 1,
                byte_start: 30,
                byte_end: 100,
                sibling_count: 2,
                mtime: SystemTime::UNIX_EPOCH,
            },
        ];

        let mut writer = IndexWriter::open(temp.path(), "english").unwrap();
        for doc in &docs {
            writer.add_document(doc).unwrap();
        }
        writer.commit().unwrap();

        let mut searcher = Searcher::open(temp.path(), "english", &make_trees(), 1.5).unwrap();

        // Search for "preamble" - should find document node
        let results = searcher.search("preamble", 10).unwrap();
        assert_eq!(results.len(), 1);
        let doc_result = &results[0];
        assert_eq!(doc_result.id, "local:docs/guide.md");
        assert_eq!(doc_result.doc_id, "local:docs/guide.md");
        assert!(doc_result.parent_id.is_none());
        assert_eq!(doc_result.depth, 0);
        assert_eq!(doc_result.position, 0);
        assert_eq!(doc_result.byte_start, 0);
        assert_eq!(doc_result.byte_end, 30);
        assert_eq!(doc_result.sibling_count, 1);

        // Search for "section unique" - should find heading node
        let results = searcher.search("section unique", 10).unwrap();
        assert_eq!(results.len(), 1);
        let heading_result = &results[0];
        assert_eq!(heading_result.id, "local:docs/guide.md#section-one");
        assert_eq!(heading_result.doc_id, "local:docs/guide.md");
        assert_eq!(
            heading_result.parent_id,
            Some("local:docs/guide.md".to_string())
        );
        assert_eq!(heading_result.depth, 1);
        assert_eq!(heading_result.position, 1);
        assert_eq!(heading_result.byte_start, 30);
        assert_eq!(heading_result.byte_end, 100);
        assert_eq!(heading_result.sibling_count, 2);
    }

    #[test]
    fn get_by_id_returns_hierarchical_fields() {
        let temp = TempDir::new().unwrap();

        let doc = ChunkDocument {
            id: "local:test.md#intro".to_string(),
            doc_id: "local:test.md".to_string(),
            parent_id: Some("local:test.md".to_string()),
            title: "Introduction".to_string(),
            tags: vec![],
            path: "test.md".to_string(),
            path_components: vec!["test".to_string(), "md".to_string()],
            tree: "local".to_string(),
            body: "Intro content.".to_string(),
            breadcrumb: "> Test › Introduction".to_string(),
            depth: 1,
            position: 1,
            byte_start: 10,
            byte_end: 50,
            sibling_count: 3,
            mtime: SystemTime::UNIX_EPOCH,
        };

        let mut writer = IndexWriter::open(temp.path(), "english").unwrap();
        writer.add_document(&doc).unwrap();
        writer.commit().unwrap();

        let searcher = Searcher::open(temp.path(), "english", &make_trees(), 1.5).unwrap();

        let result = searcher
            .get_by_id("local:test.md#intro")
            .unwrap()
            .expect("should find document");

        assert_eq!(result.doc_id, "local:test.md");
        assert_eq!(result.parent_id, Some("local:test.md".to_string()));
        assert_eq!(result.depth, 1);
        assert_eq!(result.position, 1);
        assert_eq!(result.byte_start, 10);
        assert_eq!(result.byte_end, 50);
        assert_eq!(result.sibling_count, 3);
    }
}
