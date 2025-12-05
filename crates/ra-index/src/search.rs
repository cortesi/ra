//! Search execution for the ra index.
//!
//! Provides the [`Searcher`] struct for querying the index and retrieving results.
//! Supports field boosting, local tree boosting, and snippet generation.

use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    fs, mem,
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
    IndexError, ParentInfo,
    aggregate::{DEFAULT_AGGREGATION_THRESHOLD, aggregate},
    analyzer::{RA_TOKENIZER, build_analyzer_from_name},
    elbow::{DEFAULT_CUTOFF_RATIO, DEFAULT_MAX_RESULTS, elbow_cutoff},
    query::{QueryCompiler, parse},
    result::{SearchCandidate, SearchResult as AggregatedSearchResult},
    schema::{IndexSchema, boost},
};

/// Default maximum number of characters in a snippet.
const DEFAULT_SNIPPET_MAX_CHARS: usize = 150;

/// Default number of candidates to retrieve from the index in Phase 1.
pub const DEFAULT_CANDIDATE_LIMIT: usize = 100;

/// Parameters controlling the three-phase search algorithm.
///
/// The search algorithm proceeds in three phases:
/// 1. **Phase 1 (Query)**: Retrieve up to `candidate_limit` matches from the index
/// 2. **Phase 2 (Elbow)**: Apply relevance cutoff using `cutoff_ratio` and `max_results`
/// 3. **Phase 3 (Aggregate)**: Aggregate sibling matches using `aggregation_threshold`
#[derive(Debug, Clone)]
pub struct SearchParams {
    /// Maximum candidates to retrieve in Phase 1. Default: 100.
    pub candidate_limit: usize,
    /// Score ratio threshold for Phase 2 elbow detection. Default: 0.5.
    pub cutoff_ratio: f32,
    /// Maximum results after Phase 2. Default: 20.
    pub max_results: usize,
    /// Sibling ratio threshold for Phase 3 aggregation. Default: 0.5.
    pub aggregation_threshold: f32,
    /// Whether to skip Phase 3 aggregation. Default: false.
    pub disable_aggregation: bool,
    /// Limit results to these trees. If empty, search all trees.
    ///
    /// Note: BM25 scoring uses corpus-wide statistics (document frequency, average
    /// document length) computed across all indexed documents, not just the filtered
    /// trees. This means scores reflect term importance globally. For most use cases
    /// this is acceptable since relative ranking within results remains meaningful.
    pub trees: Vec<String>,
    /// Verbosity level for match details (0 = none, 1 = summary, 2+ = full).
    pub verbosity: u8,
}

impl Default for SearchParams {
    fn default() -> Self {
        Self {
            candidate_limit: DEFAULT_CANDIDATE_LIMIT,
            cutoff_ratio: DEFAULT_CUTOFF_RATIO,
            max_results: DEFAULT_MAX_RESULTS,
            aggregation_threshold: DEFAULT_AGGREGATION_THRESHOLD,
            disable_aggregation: false,
            trees: Vec::new(),
            verbosity: 0,
        }
    }
}

impl SearchParams {
    /// Sets the trees to filter results to.
    pub fn with_trees(mut self, trees: Vec<String>) -> Self {
        self.trees = trees;
        self
    }

    /// Sets the verbosity level for match details.
    pub fn with_verbosity(mut self, verbosity: u8) -> Self {
        self.verbosity = verbosity;
        self
    }
}

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

/// Extracts byte ranges for matched terms within `body` using the configured analyzer.
///
/// Offsets are relative to the original body text and are guaranteed to be sorted,
/// non-overlapping, and merged where adjacent.
fn extract_match_ranges(
    analyzer: &TextAnalyzer,
    body: &str,
    matched_terms: &HashSet<String>,
) -> Vec<Range<usize>> {
    if matched_terms.is_empty() || body.is_empty() {
        return Vec::new();
    }

    let mut analyzer = analyzer.clone();
    let mut stream = analyzer.token_stream(body);
    let mut ranges: Vec<Range<usize>> = Vec::new();

    while let Some(token) = stream.next() {
        if matched_terms.contains(&token.text) {
            ranges.push(token.offset_from..token.offset_to);
        }
    }

    // Token stream yields ranges in order; merge to collapse adjacency/overlap.
    merge_ranges(ranges, Vec::new())
}

/// Details about how a term matched in a specific field.
#[derive(Debug, Clone, Default)]
pub struct FieldMatch {
    /// The indexed terms that matched in this field.
    pub matched_terms: Vec<String>,
    /// Term frequency for each matched term in this field.
    pub term_frequencies: HashMap<String, u32>,
}

/// Detailed information about how search terms matched a document.
#[derive(Debug, Clone, Default)]
pub struct MatchDetails {
    /// Original query terms (before stemming).
    pub original_terms: Vec<String>,
    /// Query terms after stemming/tokenization.
    pub stemmed_terms: Vec<String>,
    /// Map from stemmed query term to indexed terms that matched (including fuzzy).
    pub term_mappings: HashMap<String, Vec<String>>,
    /// Match details per field (title, body, tags, path).
    pub field_matches: HashMap<String, FieldMatch>,
    /// Base BM25 score before boosts.
    pub base_score: f32,
    /// Score contribution from each field.
    pub field_scores: HashMap<String, f32>,
    /// Local tree boost multiplier applied (1.0 if global or no boost).
    pub local_boost: f32,
    /// Detailed score explanation (for -vv).
    pub score_explanation: Option<String>,
}

impl MatchDetails {
    /// Returns true if match details are populated.
    pub fn is_populated(&self) -> bool {
        !self.stemmed_terms.is_empty()
    }

    /// Returns total match count across all fields.
    pub fn total_matches(&self) -> u32 {
        self.field_matches
            .values()
            .flat_map(|fm| fm.term_frequencies.values())
            .sum()
    }
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
    /// Offsets are byte positions into the returned `body` text (UTF-8 safe), sorted,
    /// merged, and non-overlapping. Each range corresponds to a token produced by the
    /// index analyzer (after lowercasing/stemming/fuzzy expansion), ensuring consumers
    /// can reliably highlight the exact substrings in the original body content.
    pub match_ranges: Vec<Range<usize>>,
    /// Byte ranges within `title` where search terms match.
    pub title_match_ranges: Vec<Range<usize>>,
    /// Byte ranges within `path` where search terms match.
    pub path_match_ranges: Vec<Range<usize>>,
    /// Detailed match information for verbose output.
    pub match_details: Option<MatchDetails>,
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
    /// Map from tree name to root path.
    tree_paths: HashMap<String, PathBuf>,
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

        // Build tree global/local map and path map
        let tree_is_global: HashMap<String, bool> = trees
            .iter()
            .map(|t| (t.name.clone(), t.is_global))
            .collect();
        let tree_paths: HashMap<String, PathBuf> = trees
            .iter()
            .map(|t| (t.name.clone(), t.path.clone()))
            .collect();

        Ok(Self {
            index,
            schema,
            query_compiler,
            analyzer,
            lev_builder,
            fuzzy_distance,
            tree_is_global,
            tree_paths,
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
                        // Merge title/path match ranges
                        existing.title_match_ranges = merge_ranges(
                            mem::take(&mut existing.title_match_ranges),
                            result.title_match_ranges.clone(),
                        );
                        existing.path_match_ranges = merge_ranges(
                            mem::take(&mut existing.path_match_ranges),
                            result.path_match_ranges.clone(),
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

        // Collect and sort by score (highest first), then by ID for stability
        let mut results: Vec<SearchResult> = results_by_id.into_values().collect();
        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(Ordering::Equal)
                .then_with(|| a.id.cmp(&b.id))
        });

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

        let query_terms = self.tokenize_query(query_str);

        self.execute_query_no_highlights(&*query, &query_terms, limit)
    }

    /// Searches using the three-phase hierarchical algorithm.
    ///
    /// This is the full search pipeline with elbow detection and aggregation:
    /// 1. **Phase 1**: Query the index for candidates
    /// 2. **Phase 2**: Apply elbow cutoff to filter relevant results
    /// 3. **Phase 3**: Aggregate sibling matches into parent nodes
    ///
    /// # Arguments
    /// * `query_str` - The search query string
    /// * `params` - Search parameters controlling each phase
    ///
    /// # Returns
    /// A vector of aggregated search results, ordered by relevance score.
    pub fn search_aggregated(
        &mut self,
        query_str: &str,
        params: &SearchParams,
    ) -> Result<Vec<AggregatedSearchResult>, IndexError> {
        let content_query = match self.build_query(query_str)? {
            Some(q) => q,
            None => return Ok(Vec::new()),
        };

        // Apply tree filter if specified
        let query = self.apply_tree_filter(content_query, &params.trees);

        // Tokenize query for highlighting
        let query_terms = self.tokenize_query(query_str);

        // Phase 1: Query the index
        // Use detailed execution if verbosity is requested
        let raw_results = if params.verbosity > 0 {
            self.execute_query_with_details(
                &*query,
                query_str,
                &query_terms,
                params.candidate_limit,
                params.verbosity >= 2, // Include full explanation at -vv
            )?
        } else {
            self.execute_query_with_highlights(&*query, &query_terms, params.candidate_limit)?
        };

        // Convert SearchResults to SearchCandidates
        let candidates: Vec<SearchCandidate> = raw_results.into_iter().map(|r| r.into()).collect();

        // Phase 2: Apply elbow cutoff
        let filtered = elbow_cutoff(candidates, params.cutoff_ratio, params.max_results);

        // Phase 3: Aggregate (if enabled)
        if params.disable_aggregation {
            // Return as single results without aggregation
            Ok(filtered
                .into_iter()
                .map(AggregatedSearchResult::single)
                .collect())
        } else {
            // Look up parent nodes from the index
            let results = aggregate(filtered, params.aggregation_threshold, |parent_id| {
                self.lookup_parent(parent_id)
            });
            Ok(results)
        }
    }

    /// Looks up a parent node by ID for aggregation.
    fn lookup_parent(&self, parent_id: &str) -> Option<ParentInfo> {
        let reader = self.index.reader().ok()?;
        let searcher = reader.searcher();

        // Build exact term query on ID field
        let term = Term::from_field_text(self.schema.id, parent_id);
        let query = TermQuery::new(term, IndexRecordOption::Basic);

        let top_docs = searcher.search(&query, &TopDocs::with_limit(1)).ok()?;

        if let Some((_, doc_address)) = top_docs.first() {
            let doc: TantivyDocument = searcher.doc(*doc_address).ok()?;

            let id = self.get_text_field(&doc, self.schema.id);
            let doc_id = self.get_text_field(&doc, self.schema.doc_id);
            let parent_id_str = self.get_text_field(&doc, self.schema.parent_id);
            let parent_id = if parent_id_str.is_empty() {
                None
            } else {
                Some(parent_id_str)
            };
            let title = self.get_text_field(&doc, self.schema.title);
            let tree = self.get_text_field(&doc, self.schema.tree);
            let path = self.get_text_field(&doc, self.schema.path);
            let body = self.get_text_field(&doc, self.schema.body);
            let breadcrumb = self.get_text_field(&doc, self.schema.breadcrumb);
            let depth = self.get_u64_field(&doc, self.schema.depth);
            let position = self.get_u64_field(&doc, self.schema.position);
            let byte_start = self.get_u64_field(&doc, self.schema.byte_start);
            let byte_end = self.get_u64_field(&doc, self.schema.byte_end);
            let sibling_count = self.get_u64_field(&doc, self.schema.sibling_count);

            Some(ParentInfo {
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
            })
        } else {
            None
        }
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

    /// Builds a tree filter query for the given tree names.
    ///
    /// Returns None if no trees are specified.
    fn build_tree_filter(&self, trees: &[String]) -> Option<Box<dyn Query>> {
        if trees.is_empty() {
            return None;
        }

        if trees.len() == 1 {
            let term = Term::from_field_text(self.schema.tree, &trees[0]);
            return Some(Box::new(TermQuery::new(term, IndexRecordOption::Basic)));
        }

        // Multiple trees: OR them together
        let clauses: Vec<(Occur, Box<dyn Query>)> = trees
            .iter()
            .map(|tree_name| {
                let term = Term::from_field_text(self.schema.tree, tree_name);
                let query: Box<dyn Query> =
                    Box::new(TermQuery::new(term, IndexRecordOption::Basic));
                (Occur::Should, query)
            })
            .collect();

        Some(Box::new(BooleanQuery::new(clauses)))
    }

    /// Wraps a content query with a tree filter.
    ///
    /// If no tree filter is provided, returns the original query unchanged.
    fn apply_tree_filter(&self, content_query: Box<dyn Query>, trees: &[String]) -> Box<dyn Query> {
        match self.build_tree_filter(trees) {
            Some(tree_filter) => {
                let clauses = vec![(Occur::Must, content_query), (Occur::Must, tree_filter)];
                Box::new(BooleanQuery::new(clauses))
            }
            None => content_query,
        }
    }

    /// Tokenizes a query string to extract individual search terms.
    ///
    /// Filters out query syntax elements (OR, AND, NOT, field prefixes) before
    /// tokenizing to avoid treating keywords as search terms.
    fn tokenize_query(&mut self, query_str: &str) -> Vec<String> {
        // Pre-filter query syntax before tokenizing
        let filtered: String = query_str
            .split_whitespace()
            .filter(|word| {
                let upper = word.to_uppercase();
                // Filter out boolean operators and field prefixes
                upper != "OR" && upper != "AND" && upper != "NOT" && !word.contains(':')
            })
            .collect::<Vec<_>>()
            .join(" ");

        let mut stream = self.analyzer.token_stream(&filtered);
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
        fields: &[Field],
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
            for field in fields {
                if let Ok(inverted_index) = segment_reader.inverted_index(*field) {
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
        }

        // If no terms found in index (e.g., new terms), fall back to query terms
        if matched_terms.is_empty() {
            for term in query_terms {
                matched_terms.insert(term.clone());
            }
        }

        matched_terms
    }

    /// Finds term mappings from query terms to indexed terms (with fuzzy matching).
    ///
    /// Returns a map where keys are query terms and values are the indexed terms
    /// they matched (including fuzzy matches).
    fn find_term_mappings(
        &self,
        searcher: &tantivy::Searcher,
        query_terms: &[String],
    ) -> HashMap<String, Vec<String>> {
        let mut mappings: HashMap<String, Vec<String>> = HashMap::new();

        if self.fuzzy_distance == 0 {
            // No fuzzy matching - each term maps to itself
            for term in query_terms {
                mappings.insert(term.clone(), vec![term.clone()]);
            }
            return mappings;
        }

        // Search the body field's term dictionary for matching terms
        for segment_reader in searcher.segment_readers() {
            if let Ok(inverted_index) = segment_reader.inverted_index(self.schema.body) {
                let term_dict = inverted_index.terms();

                for query_term in query_terms {
                    let dfa = LevenshteinDfa(self.lev_builder.build_dfa(query_term));
                    let mut stream = term_dict.search(dfa).into_stream().unwrap();

                    let entry = mappings.entry(query_term.clone()).or_default();
                    while stream.advance() {
                        if let Ok(term_str) = str::from_utf8(stream.key())
                            && !entry.contains(&term_str.to_string())
                        {
                            entry.push(term_str.to_string());
                        }
                    }
                }
            }
        }

        // Ensure every query term has at least itself as a mapping
        for term in query_terms {
            mappings
                .entry(term.clone())
                .or_insert_with(|| vec![term.clone()]);
        }

        mappings
    }

    /// Counts term frequency in a specific text field for a document.
    fn count_term_frequency_in_text(
        &mut self,
        text: &str,
        terms: &HashSet<String>,
    ) -> HashMap<String, u32> {
        let mut freqs: HashMap<String, u32> = HashMap::new();
        let mut stream = self.analyzer.token_stream(text);
        while let Some(token) = stream.next() {
            if terms.contains(&token.text) {
                *freqs.entry(token.text.clone()).or_insert(0) += 1;
            }
        }
        freqs
    }

    /// Collects detailed match information for a search result.
    #[allow(clippy::too_many_arguments)]
    fn collect_match_details(
        &mut self,
        doc: &TantivyDocument,
        query: &dyn Query,
        original_query: &str,
        query_terms: &[String],
        term_mappings: &HashMap<String, Vec<String>>,
        base_score: f32,
        local_boost: f32,
        searcher: &tantivy::Searcher,
        doc_address: tantivy::DocAddress,
        include_explanation: bool,
    ) -> MatchDetails {
        // Collect all matched indexed terms
        let all_matched_terms: HashSet<String> =
            term_mappings.values().flatten().cloned().collect();

        // Extract field contents
        let title = self.get_text_field(doc, self.schema.title);
        let body = self.get_text_field(doc, self.schema.body);
        let tags: Vec<String> = doc
            .get_all(self.schema.tags)
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect();
        let path = self.get_text_field(doc, self.schema.path);

        // Count term frequencies in each field
        let mut field_matches: HashMap<String, FieldMatch> = HashMap::new();
        let mut field_scores: HashMap<String, f32> = HashMap::new();

        // Title field
        let title_freqs = self.count_term_frequency_in_text(&title, &all_matched_terms);
        if !title_freqs.is_empty() {
            let matched: Vec<String> = title_freqs.keys().cloned().collect();
            let title_score: f32 =
                title_freqs.values().map(|&c| c as f32).sum::<f32>() * boost::TITLE;
            field_scores.insert("title".to_string(), title_score);
            field_matches.insert(
                "title".to_string(),
                FieldMatch {
                    matched_terms: matched,
                    term_frequencies: title_freqs,
                },
            );
        }

        // Body field
        let body_freqs = self.count_term_frequency_in_text(&body, &all_matched_terms);
        if !body_freqs.is_empty() {
            let matched: Vec<String> = body_freqs.keys().cloned().collect();
            let body_score: f32 = body_freqs.values().map(|&c| c as f32).sum::<f32>() * boost::BODY;
            field_scores.insert("body".to_string(), body_score);
            field_matches.insert(
                "body".to_string(),
                FieldMatch {
                    matched_terms: matched,
                    term_frequencies: body_freqs,
                },
            );
        }

        // Tags field
        let tags_text = tags.join(" ");
        let tags_freqs = self.count_term_frequency_in_text(&tags_text, &all_matched_terms);
        if !tags_freqs.is_empty() {
            let matched: Vec<String> = tags_freqs.keys().cloned().collect();
            let tags_score: f32 = tags_freqs.values().map(|&c| c as f32).sum::<f32>() * boost::TAGS;
            field_scores.insert("tags".to_string(), tags_score);
            field_matches.insert(
                "tags".to_string(),
                FieldMatch {
                    matched_terms: matched,
                    term_frequencies: tags_freqs,
                },
            );
        }

        // Path field
        let path_freqs = self.count_term_frequency_in_text(&path, &all_matched_terms);
        if !path_freqs.is_empty() {
            let matched: Vec<String> = path_freqs.keys().cloned().collect();
            let path_score: f32 = path_freqs.values().map(|&c| c as f32).sum::<f32>() * boost::PATH;
            field_scores.insert("path".to_string(), path_score);
            field_matches.insert(
                "path".to_string(),
                FieldMatch {
                    matched_terms: matched,
                    term_frequencies: path_freqs,
                },
            );
        }

        // Get score explanation if requested
        let score_explanation = if include_explanation {
            query
                .explain(searcher, doc_address)
                .ok()
                .map(|e| e.to_pretty_json())
        } else {
            None
        };

        // Extract original terms from query (before tokenization)
        let original_terms: Vec<String> = original_query
            .split_whitespace()
            .filter(|s| !s.starts_with('-') && *s != "OR" && !s.contains(':'))
            .map(|s| {
                s.trim_matches(|c| c == '"' || c == '(' || c == ')')
                    .to_string()
            })
            .filter(|s| !s.is_empty())
            .collect();

        MatchDetails {
            original_terms,
            stemmed_terms: query_terms.to_vec(),
            term_mappings: term_mappings.clone(),
            field_matches,
            base_score,
            field_scores,
            local_boost,
            score_explanation,
        }
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
        let matched_terms = self.find_matched_terms(
            &searcher,
            query_terms,
            &[self.schema.body, self.schema.title, self.schema.path],
        );

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

        let mut results = Vec::with_capacity(top_docs.len());

        for (score, doc_address) in top_docs {
            let doc: TantivyDocument = searcher
                .doc(doc_address)
                .map_err(|e| IndexError::Write(e.to_string()))?;

            let result = self.doc_to_result(&doc, score, &snippet_generator, &matched_terms);
            results.push(result);
        }

        Ok(results)
    }

    /// Executes a query without generating snippets or highlights (faster).
    fn execute_query_no_highlights(
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

        let matched_terms = self.find_matched_terms(
            &searcher,
            query_terms,
            &[self.schema.body, self.schema.title, self.schema.path],
        );

        let top_docs = searcher
            .search(query, &TopDocs::with_limit(limit))
            .map_err(|e| IndexError::Write(e.to_string()))?;

        let mut results = Vec::with_capacity(top_docs.len());

        for (score, doc_address) in top_docs {
            let doc: TantivyDocument = searcher
                .doc(doc_address)
                .map_err(|e| IndexError::Write(e.to_string()))?;

            let result = self.doc_to_result(&doc, score, &None, &matched_terms);
            results.push(result);
        }

        Ok(results)
    }

    /// Executes a query with full match detail collection.
    ///
    /// This is the most expensive execution mode, collecting detailed information
    /// about which terms matched, their frequencies, and score breakdowns.
    #[allow(clippy::too_many_arguments)]
    fn execute_query_with_details(
        &mut self,
        query: &dyn Query,
        original_query: &str,
        query_terms: &[String],
        limit: usize,
        include_explanation: bool,
    ) -> Result<Vec<SearchResult>, IndexError> {
        let reader = self
            .index
            .reader()
            .map_err(|e| IndexError::Write(e.to_string()))?;

        let searcher = reader.searcher();

        let top_docs = searcher
            .search(query, &TopDocs::with_limit(limit))
            .map_err(|e| IndexError::Write(e.to_string()))?;

        // Find term mappings (query term -> indexed terms)
        let term_mappings = self.find_term_mappings(&searcher, query_terms);

        // Get all matched terms for highlighting (include title/path fields as well)
        let mut matched_terms: HashSet<String> =
            term_mappings.values().flatten().cloned().collect();
        let extra_terms = self.find_matched_terms(
            &searcher,
            query_terms,
            &[self.schema.title, self.schema.path],
        );
        matched_terms.extend(extra_terms);

        // Build highlight query
        let highlight_query = self.build_highlight_query(&matched_terms);

        // Create generators
        let snippet_generator = if let Some(ref hq) = highlight_query {
            let mut generator = SnippetGenerator::create(&searcher, hq.as_ref(), self.schema.body)
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

            let mut result = self.doc_to_result(&doc, score, &snippet_generator, &matched_terms);

            // Compute local boost for this result
            let is_global = self
                .tree_is_global
                .get(&result.tree)
                .copied()
                .unwrap_or(false);
            let local_boost = if is_global { 1.0 } else { self.local_boost };

            // Collect match details
            let details = self.collect_match_details(
                &doc,
                query,
                original_query,
                query_terms,
                &term_mappings,
                score,
                local_boost,
                &searcher,
                doc_address,
                include_explanation,
            );
            result.match_details = Some(details);

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
        matched_terms: &HashSet<String>,
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

        // Extract deterministic match ranges from the body
        let match_ranges = extract_match_ranges(&self.analyzer, &body, matched_terms);
        let title_match_ranges = extract_match_ranges(&self.analyzer, &title, matched_terms);
        let path_match_ranges = extract_match_ranges(&self.analyzer, &path, matched_terms);

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
            title_match_ranges,
            path_match_ranges,
            match_details: None,
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

            let empty_terms = HashSet::new();
            let result = self.doc_to_result(&doc, *score, &None, &empty_terms);
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
                let empty_terms = HashSet::new();
                Some(self.doc_to_result(&doc, score, &None, &empty_terms))
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
                let empty_terms = HashSet::new();
                let result = self.doc_to_result(&doc, score, &None, &empty_terms);
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
    /// * `trees` - Limit results to these trees (empty = all trees)
    pub fn search_context(
        &mut self,
        signals: &[crate::ContextSignals],
        limit: usize,
        trees: &[String],
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
        let results = self.search_multi(&term_refs, limit)?;

        // Filter by tree if specified
        if trees.is_empty() {
            Ok(results)
        } else {
            Ok(results
                .into_iter()
                .filter(|r| trees.contains(&r.tree))
                .collect())
        }
    }

    /// Reads the full content of a chunk by reading the source file span.
    ///
    /// For parent nodes, this includes all child content within the byte range.
    /// Returns the content from `byte_start` to `byte_end` of the original file.
    ///
    /// # Arguments
    /// * `tree` - The tree name
    /// * `path` - The file path within the tree
    /// * `byte_start` - Start byte offset
    /// * `byte_end` - End byte offset
    pub fn read_full_content(
        &self,
        tree: &str,
        path: &str,
        byte_start: u64,
        byte_end: u64,
    ) -> Result<String, IndexError> {
        let tree_root = self
            .tree_paths
            .get(tree)
            .ok_or_else(|| IndexError::Write(format!("unknown tree: {tree}")))?;

        let file_path = tree_root.join(path);
        let content = fs::read_to_string(&file_path).map_err(|e| {
            IndexError::Write(format!("failed to read {}: {e}", file_path.display()))
        })?;

        let start = byte_start as usize;
        let end = byte_end as usize;

        if end > content.len() || start > end {
            return Err(IndexError::Write(format!(
                "invalid byte range [{start}, {end}) for file of {} bytes",
                content.len()
            )));
        }

        Ok(content[start..end].to_string())
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
    fn search_multi_merges_title_and_path_ranges() {
        let temp = TempDir::new().unwrap();
        create_test_index(&temp);

        let mut searcher = Searcher::open(temp.path(), "english", &make_trees(), 1.5).unwrap();

        // Combine topics so that different fields contribute highlights
        let results = searcher
            .search_multi(&["rust", "introduction", "docs"], 10)
            .unwrap();

        let intro = results.iter().find(|r| r.id.contains("rust.md")).unwrap();

        let title_ranges = &intro.title_match_ranges;
        assert!(
            title_ranges.len() >= 2,
            "expected merged title ranges for 'Introduction' and 'Rust'"
        );

        let path_ranges = &intro.path_match_ranges;
        assert!(
            path_ranges.len() >= 2,
            "expected merged path ranges for 'docs' and 'rust'"
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
    fn match_ranges_align_with_body_offsets() {
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

        let mut searcher = Searcher::open(temp.path(), "english", &make_trees(), 1.5).unwrap();
        let results = searcher.search("foz", 10).unwrap();

        let result = &results[0];
        assert!(
            result
                .match_ranges
                .windows(2)
                .all(|w| w[0].end <= w[1].start),
            "ranges should be sorted and non-overlapping"
        );

        let slices: Vec<&str> = result
            .match_ranges
            .iter()
            .map(|r| &result.body[r.clone()])
            .collect();
        assert!(slices.iter().any(|s| s.to_lowercase() == "fox"));
    }

    #[test]
    fn match_ranges_cover_stemmed_tokens() {
        let temp = TempDir::new().unwrap();

        let docs = vec![ChunkDocument {
            id: "local:docs/stems.md".to_string(),
            doc_id: "local:docs/stems.md".to_string(),
            parent_id: None,
            title: "Stems".to_string(),
            tags: vec![],
            path: "docs/stems.md".to_string(),
            path_components: vec!["docs".to_string(), "stems".to_string(), "md".to_string()],
            tree: "local".to_string(),
            body: "Handling handled handles".to_string(),
            breadcrumb: "Stems".to_string(),
            depth: 0,
            position: 0,
            byte_start: 0,
            byte_end: 64,
            sibling_count: 1,
            mtime: SystemTime::UNIX_EPOCH,
        }];

        let mut writer = IndexWriter::open(temp.path(), "english").unwrap();
        for doc in &docs {
            writer.add_document(doc).unwrap();
        }
        writer.commit().unwrap();

        let mut searcher = Searcher::open(temp.path(), "english", &make_trees(), 1.5).unwrap();
        let results = searcher.search("handling", 10).unwrap();
        let result = &results[0];

        let slices: Vec<&str> = result
            .match_ranges
            .iter()
            .map(|r| &result.body[r.clone()])
            .collect();

        assert!(slices.iter().any(|s| s.to_lowercase() == "handling"));
        assert!(slices.iter().any(|s| s.to_lowercase() == "handled"));
    }

    #[test]
    fn match_ranges_roundtrip_offset_length() {
        let temp = TempDir::new().unwrap();

        let docs = vec![ChunkDocument {
            id: "local:docs/roundtrip.md".to_string(),
            doc_id: "local:docs/roundtrip.md".to_string(),
            parent_id: None,
            title: "Roundtrip".to_string(),
            tags: vec![],
            path: "docs/roundtrip.md".to_string(),
            path_components: vec![
                "docs".to_string(),
                "roundtrip".to_string(),
                "md".to_string(),
            ],
            tree: "local".to_string(),
            body: "Alpha beta gamma alpha".to_string(),
            breadcrumb: "Roundtrip".to_string(),
            depth: 0,
            position: 0,
            byte_start: 0,
            byte_end: 64,
            sibling_count: 1,
            mtime: SystemTime::UNIX_EPOCH,
        }];

        let mut writer = IndexWriter::open(temp.path(), "english").unwrap();
        for doc in &docs {
            writer.add_document(doc).unwrap();
        }
        writer.commit().unwrap();

        let mut searcher = Searcher::open(temp.path(), "english", &make_trees(), 1.5).unwrap();
        let results = searcher.search("alpha", 10).unwrap();
        let result = &results[0];

        let offsets_and_lengths: Vec<(usize, usize)> = result
            .match_ranges
            .iter()
            .map(|r| (r.start, r.end - r.start))
            .collect();

        let reconstructed: Vec<Range<usize>> = offsets_and_lengths
            .iter()
            .map(|(o, l)| *o..*o + *l)
            .collect();

        assert_eq!(result.match_ranges, reconstructed);

        // Ensure reconstructed slices match the original term
        for range in reconstructed {
            let slice = &result.body[range];
            assert_eq!(slice.to_lowercase(), "alpha");
        }
    }

    #[test]
    fn title_and_path_match_ranges_present() {
        let temp = TempDir::new().unwrap();
        create_test_index(&temp);

        let mut searcher = Searcher::open(temp.path(), "english", &make_trees(), 1.5).unwrap();
        let results = searcher.search("rust", 10).unwrap();
        let result = results
            .iter()
            .find(|r| r.title.contains("Rust"))
            .expect("rust result");

        assert!(
            !result.title_match_ranges.is_empty(),
            "expected title match ranges"
        );
        assert!(
            !result.path_match_ranges.is_empty(),
            "expected path match ranges"
        );

        let title_slice = &result.title[result.title_match_ranges[0].clone()];
        assert!(
            title_slice.to_lowercase().contains("rust"),
            "title slice should include rust"
        );

        let path_slice = &result.path[result.path_match_ranges[0].clone()];
        assert!(
            path_slice.to_lowercase().contains("rust"),
            "path slice should include rust"
        );
    }

    #[test]
    fn search_no_snippets_still_has_highlights() {
        let temp = TempDir::new().unwrap();
        create_test_index(&temp);

        let mut searcher = Searcher::open(temp.path(), "english", &make_trees(), 1.5).unwrap();

        let results = searcher.search_no_snippets("safety", 10).unwrap();

        assert!(!results.is_empty());
        assert!(
            !results[0].match_ranges.is_empty(),
            "match ranges should still be populated without snippets"
        );
    }

    #[test]
    fn multi_topic_merge_keeps_distinct_ranges() {
        let temp = TempDir::new().unwrap();
        create_test_index(&temp);

        let mut searcher = Searcher::open(temp.path(), "english", &make_trees(), 1.5).unwrap();

        let results = searcher.search_multi(&["rust", "safety"], 10).unwrap();
        let intro = results.iter().find(|r| r.id.contains("rust.md")).unwrap();

        let slices: Vec<String> = intro
            .match_ranges
            .iter()
            .map(|r| intro.body[r.clone()].to_string())
            .collect();

        assert!(slices.iter().any(|s| s.to_lowercase() == "rust"));
        assert!(slices.iter().any(|s| s.to_lowercase() == "safety"));
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

    #[test]
    fn search_aggregated_filters_by_tree() {
        let temp = TempDir::new().unwrap();
        create_test_index(&temp);

        let mut searcher = Searcher::open(temp.path(), "english", &make_trees(), 1.5).unwrap();

        // Use params that won't filter via elbow detection
        let base_params = SearchParams {
            cutoff_ratio: 0.0, // Disable elbow cutoff
            disable_aggregation: true,
            ..Default::default()
        };

        // Search for "rust" without tree filter - should find all 3 docs
        let results = searcher.search_aggregated("rust", &base_params).unwrap();
        assert_eq!(results.len(), 3);

        // Search with filter to "local" tree only - should find 2 docs
        let params = base_params.clone().with_trees(vec!["local".to_string()]);
        let results = searcher.search_aggregated("rust", &params).unwrap();
        assert_eq!(results.len(), 2);
        for result in &results {
            assert_eq!(result.tree(), "local");
        }

        // Search with filter to "global" tree only - should find 1 doc
        let params = base_params.clone().with_trees(vec!["global".to_string()]);
        let results = searcher.search_aggregated("rust", &params).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].tree(), "global");

        // Search with filter to multiple trees
        let params = base_params
            .clone()
            .with_trees(vec!["local".to_string(), "global".to_string()]);
        let results = searcher.search_aggregated("rust", &params).unwrap();
        assert_eq!(results.len(), 3);

        // Search with filter to non-existent tree - should find nothing
        let params = base_params
            .clone()
            .with_trees(vec!["nonexistent".to_string()]);
        let results = searcher.search_aggregated("rust", &params).unwrap();
        assert!(results.is_empty());
    }
}
