//! Search execution for the ra index.
//!
//! Provides the [`Searcher`] struct for querying the index and retrieving results.
//! Supports field boosting, local tree boosting, snippet generation, and per-tree
//! score normalization for multi-tree searches.
//!
//! # Search Algorithm
//!
//! The search process follows these phases:
//!
//! 1. **Query Execution**: Run the query against the index, applying tree filters
//!    and field boosting to get raw BM25 scores. See [`execute`] module.
//!
//! 2. **Score Normalization** (multi-tree only): When searching across multiple trees,
//!    normalize scores so each tree's best result gets 1.0. This makes cross-tree
//!    comparison fair regardless of content density differences. See [`pipeline`] module.
//!
//! 3. **Hierarchical Aggregation**: Group sibling matches under parent nodes when
//!    enough siblings match. Aggregated results use RSS (Root Sum Square) scoring
//!    to reward coverage without letting noise accumulate linearly. See [`aggregation`].
//!
//! 4. **Elbow Cutoff**: Find the "elbow" point where relevance drops significantly
//!    and truncate results there. Applied AFTER aggregation so that aggregated
//!    results with boosted RSS scores compete fairly. See [`crate::elbow`].
//!
//! 5. **Final Limit**: Truncate to the requested number of results.

mod aggregation;
mod execute;
mod params;
mod pipeline;
mod query;
#[cfg(test)]
mod tests;
mod types;

use std::{
    collections::{HashMap, HashSet},
    fs, iter,
    path::{Path, PathBuf},
    str,
};

pub use execute::merge_ranges;
use execute::{ExecutionOptions, extract_match_ranges};
use levenshtein_automata::LevenshteinAutomatonBuilder;
pub use params::{MoreLikeThisParams, SearchParams};
pub use pipeline::PipelineStats;
use pipeline::{process_candidates, process_candidates_with_stats};
use ra_config::FieldBoosts;
use ra_context::IdfProvider;
use serde::Serialize;
use tantivy::{
    DocAddress, Index, Searcher as TvSearcher, TantivyDocument, Term,
    collector::{Count, TopDocs},
    directory::MmapDirectory,
    query::{
        AllQuery, BooleanQuery, BoostQuery, MoreLikeThisQuery, MoreLikeThisQueryBuilder, Occur,
        Query, TermQuery,
    },
    schema::{Field, IndexRecordOption, OwnedValue, Value},
    snippet::SnippetGenerator,
    tokenizer::TextAnalyzer,
};
use types::FieldMatch;
pub use types::{MatchDetails, SearchCandidate};

use crate::{
    IndexError, QueryError,
    analyzer::{RA_TOKENIZER, build_analyzer_from_name},
    query::{QueryCompiler, parse},
    result::SearchResult,
    schema::IndexSchema,
};

/// Maximum number of documents to retrieve in bulk lookup operations.
const MAX_BULK_LOOKUP: usize = 100_000;

/// Default maximum number of characters in a snippet.
const DEFAULT_SNIPPET_MAX_CHARS: usize = 150;

/// Explanation of a MoreLikeThis query for debugging.
#[derive(Debug, Clone, Serialize)]
pub struct MoreLikeThisExplanation {
    /// ID of the source document.
    pub source_id: String,
    /// Title of the source document.
    pub source_title: String,
    /// Preview of the source document body (first 200 chars).
    pub source_body_preview: String,
    /// Parameters used for the query.
    pub mlt_params: MoreLikeThisParams,
    /// Debug representation of the generated query.
    pub query_repr: String,
}

impl MoreLikeThisParams {
    /// Builds a Tantivy `MoreLikeThisQueryBuilder` with common parameters applied.
    fn base_builder(&self) -> MoreLikeThisQueryBuilder {
        MoreLikeThisQuery::builder()
            .with_min_doc_frequency(self.min_doc_frequency)
            .with_max_doc_frequency(self.max_doc_frequency)
            .with_min_term_frequency(self.min_term_frequency)
            .with_max_query_terms(self.max_query_terms)
            .with_min_word_length(self.min_word_length)
            .with_max_word_length(self.max_word_length)
            .with_boost_factor(self.boost_factor)
            .with_stop_words(self.stop_words.clone())
    }

    /// Builds a Tantivy MoreLikeThisQuery from a document address.
    fn build_query_from_doc(&self, doc_address: DocAddress) -> MoreLikeThisQuery {
        self.base_builder().with_document(doc_address)
    }

    /// Builds a Tantivy MoreLikeThisQuery from field values.
    fn build_query_from_fields(&self, fields: Vec<(Field, Vec<OwnedValue>)>) -> MoreLikeThisQuery {
        self.base_builder().with_document_fields(fields)
    }
}

/// Primary search entry point for the index.
pub struct Searcher {
    /// Tantivy index handle used for searching.
    pub(crate) index: Index,
    /// Schema describing indexed fields.
    pub(crate) schema: IndexSchema,
    /// Compiles parsed queries into Tantivy queries.
    pub(crate) query_compiler: QueryCompiler,
    /// Analyzer used for both querying and highlighting.
    pub(crate) analyzer: TextAnalyzer,
    /// Builder for fuzzy Levenshtein automatons.
    pub(crate) lev_builder: LevenshteinAutomatonBuilder,
    /// Maximum edit distance used for fuzzy matching.
    pub(crate) fuzzy_distance: u8,
    /// Map of tree name -> whether the tree is global (no local boost).
    pub(crate) tree_is_global: HashMap<String, bool>,
    /// Map of tree name -> filesystem path for content lookup.
    pub(crate) tree_paths: HashMap<String, PathBuf>,
    /// Boost applied to non-global tree hits.
    pub(crate) local_boost: f32,
    /// Field boost weights for scoring.
    pub(crate) boosts: FieldBoosts,
}

/// Inputs needed to build verbose match details.
struct MatchDetailsContext<'a> {
    /// Tantivy query used for scoring and optional explanation.
    query: &'a dyn Query,
    /// Original user-provided query string.
    original_query: &'a str,
    /// Stemmed/tokenized query terms.
    query_terms: &'a [String],
    /// Mapping from stemmed query terms to matched index terms.
    term_mappings: &'a HashMap<String, Vec<String>>,
    /// Raw BM25 score before boosts.
    base_score: f32,
    /// Local tree boost applied for non-global trees.
    local_boost: f32,
    /// Tantivy searcher for explanation lookup.
    searcher: &'a TvSearcher,
    /// Address of the matched document.
    doc_address: DocAddress,
    /// Whether to include a score explanation.
    include_explanation: bool,
}

impl Searcher {
    /// Opens an existing index for searching with default boost values.
    pub fn open(
        path: &Path,
        language: &str,
        trees: &[ra_config::Tree],
        local_boost: f32,
        fuzzy_distance: u8,
    ) -> Result<Self, IndexError> {
        Self::open_with_boosts(
            path,
            language,
            trees,
            local_boost,
            fuzzy_distance,
            FieldBoosts::default(),
        )
    }

    /// Opens an existing index for searching with custom boost values.
    pub fn open_with_boosts(
        path: &Path,
        language: &str,
        trees: &[ra_config::Tree],
        local_boost: f32,
        fuzzy_distance: u8,
        boosts: FieldBoosts,
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

        let analyzer = build_analyzer_from_name(language)?;
        index.tokenizers().register(RA_TOKENIZER, analyzer.clone());

        let query_compiler = QueryCompiler::new(schema.clone(), language, fuzzy_distance, boosts)?;

        let lev_builder = LevenshteinAutomatonBuilder::new(fuzzy_distance, true);

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
            boosts,
        })
    }

    /// Opens an existing index for searching using configuration.
    pub fn open_with_config(path: &Path, config: &ra_config::Config) -> Result<Self, IndexError> {
        Self::open_with_boosts(
            path,
            &config.search.stemmer,
            &config.trees,
            config.settings.local_boost,
            config.search.fuzzy_distance,
            config.search.field_boosts(),
        )
    }

    /// Computes doc IDs to exclude from results based on input file paths.
    ///
    /// Each input path is canonicalized and compared against tree roots. If a file
    /// is within a configured tree, its doc ID is returned in `tree:relative_path`
    /// format. Relative paths are normalized to forward slashes.
    pub fn compute_exclude_doc_ids(&self, files: &[&Path]) -> HashSet<String> {
        let mut exclude = HashSet::new();

        for path in files {
            let Ok(canonical) = path.canonicalize() else {
                continue;
            };

            for (tree_name, tree_path) in &self.tree_paths {
                let Ok(tree_canonical) = tree_path.canonicalize() else {
                    continue;
                };

                if let Ok(relative) = canonical.strip_prefix(&tree_canonical) {
                    let relative_str = relative.to_string_lossy().replace('\\', "/");
                    exclude.insert(format!("{tree_name}:{relative_str}"));
                    break;
                }
            }
        }

        exclude
    }

    /// Reads the full content of a chunk by reading the source file span.
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

    /// Searches using the hierarchical algorithm with per-tree score normalization.
    pub fn search_aggregated(
        &mut self,
        query_str: &str,
        params: &SearchParams,
    ) -> Result<Vec<SearchResult>, IndexError> {
        Ok(self.search_aggregated_with_stats(query_str, params)?.0)
    }

    /// Searches and returns pipeline statistics along with results.
    pub fn search_aggregated_with_stats(
        &mut self,
        query_str: &str,
        params: &SearchParams,
    ) -> Result<(Vec<SearchResult>, PipelineStats), IndexError> {
        let content_query = match self.build_query(query_str)? {
            Some(q) => q,
            None => {
                return Ok((
                    Vec::new(),
                    PipelineStats::empty(params.cutoff_ratio, params.aggregation_pool_size),
                ));
            }
        };

        let query_terms = self.tokenize_query(query_str);

        self.run_aggregated_search_with_stats(content_query, &query_terms, query_str, params)
    }

    /// Searches using a pre-built query expression.
    pub fn search_aggregated_expr(
        &mut self,
        expr: &ra_query::QueryExpr,
        params: &SearchParams,
    ) -> Result<Vec<SearchResult>, IndexError> {
        Ok(self.search_aggregated_expr_with_stats(expr, params)?.0)
    }

    /// Searches using a pre-built query expression and returns pipeline statistics.
    pub fn search_aggregated_expr_with_stats(
        &mut self,
        expr: &ra_query::QueryExpr,
        params: &SearchParams,
    ) -> Result<(Vec<SearchResult>, PipelineStats), IndexError> {
        let content_query = match self.query_compiler.compile(expr).map_err(|e| {
            let query_err: QueryError = e.into();
            IndexError::Query(query_err)
        })? {
            Some(q) => q,
            None => {
                return Ok((
                    Vec::new(),
                    PipelineStats::empty(params.cutoff_ratio, params.aggregation_pool_size),
                ));
            }
        };

        let query_terms = expr.extract_terms();
        let display_query = expr.to_query_string();

        self.run_aggregated_search_with_stats(content_query, &query_terms, &display_query, params)
    }

    /// Executes query and processes results through the unified pipeline.
    fn run_aggregated_search_with_stats(
        &self,
        content_query: Box<dyn Query>,
        query_terms: &[String],
        display_query: &str,
        params: &SearchParams,
    ) -> Result<(Vec<SearchResult>, PipelineStats), IndexError> {
        let query = self.apply_tree_filter(content_query, &params.trees);

        // Execute query and get raw candidates
        let effective_candidate_limit = params.effective_candidate_limit();

        let options = if params.verbosity > 0 {
            ExecutionOptions {
                with_snippets: true,
                with_details: true,
                original_query: Some(display_query),
                include_explanation: params.verbosity >= 2,
            }
        } else {
            ExecutionOptions {
                with_snippets: true,
                with_details: false,
                original_query: None,
                include_explanation: false,
            }
        };

        let candidates =
            self.execute_query(&*query, query_terms, effective_candidate_limit, &options)?;

        // Process through unified pipeline
        Ok(process_candidates_with_stats(
            candidates,
            params,
            |parent_id| self.lookup_parent(parent_id),
        ))
    }

    /// Looks up a parent node by ID for aggregation.
    ///
    /// Returns a `SearchCandidate` with zero score and empty match data, suitable
    /// for use as a parent node during hierarchical aggregation.
    pub(super) fn lookup_parent(&self, parent_id: &str) -> Option<SearchCandidate> {
        let reader = self.index.reader().ok()?;
        let searcher = reader.searcher();

        let term = Term::from_field_text(self.schema.id, parent_id);
        let query = TermQuery::new(term, IndexRecordOption::Basic);

        let top_docs = searcher.search(&query, &TopDocs::with_limit(1)).ok()?;

        let (_, doc_address) = top_docs.first()?;
        let doc: tantivy::TantivyDocument = searcher.doc(*doc_address).ok()?;
        Some(self.read_candidate_from_doc(&doc))
    }

    /// Returns the number of documents in the index.
    pub fn num_docs(&self) -> Result<u64, IndexError> {
        let reader = self
            .index
            .reader()
            .map_err(|e| IndexError::Write(e.to_string()))?;
        Ok(reader.searcher().num_docs())
    }

    /// Computes the IDF (Inverse Document Frequency) for a term.
    pub fn term_idf(&self, term: &str) -> Result<Option<f32>, IndexError> {
        self.term_idf_in_trees(term, &[])
    }

    /// Computes IDF for a term, optionally filtered to specific trees.
    pub fn term_idf_in_trees(
        &self,
        term: &str,
        trees: &[String],
    ) -> Result<Option<f32>, IndexError> {
        let reader = self
            .index
            .reader()
            .map_err(|e| IndexError::Write(e.to_string()))?;
        let searcher = reader.searcher();

        let mut analyzer = self.analyzer.clone();
        let mut stream = analyzer.token_stream(term);
        let stemmed_term = if let Some(token) = stream.next() {
            token.text.clone()
        } else {
            term.to_string()
        };

        let term_query: Box<dyn Query> = Box::new(TermQuery::new(
            Term::from_field_text(self.schema.body, &stemmed_term),
            IndexRecordOption::Basic,
        ));

        let query = self.apply_tree_filter(term_query, trees);

        let doc_freq = searcher
            .search(&query, &Count)
            .map_err(|e| IndexError::Write(e.to_string()))?;

        if doc_freq == 0 {
            return Ok(None);
        }

        let total_docs = if trees.is_empty() {
            searcher.num_docs() as f32
        } else {
            let tree_filter = self.build_tree_filter(trees);
            match tree_filter {
                Some(filter) => searcher
                    .search(&filter, &Count)
                    .map_err(|e| IndexError::Write(e.to_string()))?
                    as f32,
                None => searcher.num_docs() as f32,
            }
        };

        let idf = ((total_docs + 1.0) / (doc_freq as f32 + 1.0)).ln() + 1.0;

        Ok(Some(idf))
    }

    /// Retrieves a chunk by its exact ID.
    pub fn get_by_id(&self, id: &str) -> Result<Option<SearchCandidate>, IndexError> {
        let reader = self
            .index
            .reader()
            .map_err(|e| IndexError::Write(e.to_string()))?;

        let searcher = reader.searcher();

        let term = Term::from_field_text(self.schema.id, id);
        let query = TermQuery::new(term, IndexRecordOption::Basic);

        let top_docs = searcher
            .search(&query, &TopDocs::with_limit(1))
            .map_err(|e| IndexError::Write(e.to_string()))?;

        if let Some((_, doc_address)) = top_docs.first() {
            let doc: tantivy::TantivyDocument = searcher
                .doc(*doc_address)
                .map_err(|e| IndexError::Write(e.to_string()))?;

            Ok(Some(self.read_candidate_from_doc(&doc)))
        } else {
            Ok(None)
        }
    }

    /// Lists all chunks in the index, ordered by ID.
    pub fn list_all(&self) -> Result<Vec<SearchCandidate>, IndexError> {
        let reader = self
            .index
            .reader()
            .map_err(|e| IndexError::Write(e.to_string()))?;

        let searcher = reader.searcher();

        let all_docs = searcher
            .search(&AllQuery, &TopDocs::with_limit(MAX_BULK_LOOKUP))
            .map_err(|e| IndexError::Write(e.to_string()))?;

        let mut results: Vec<SearchCandidate> = all_docs
            .into_iter()
            .filter_map(|(_, doc_address)| {
                let doc: tantivy::TantivyDocument = searcher.doc(doc_address).ok()?;
                Some(self.read_candidate_from_doc(&doc))
            })
            .collect();

        results.sort_by(|a, b| a.id.cmp(&b.id));

        Ok(results)
    }

    /// Retrieves all chunks from a document by path.
    pub fn get_by_path(&self, tree: &str, path: &str) -> Result<Vec<SearchCandidate>, IndexError> {
        let reader = self
            .index
            .reader()
            .map_err(|e| IndexError::Write(e.to_string()))?;

        let searcher = reader.searcher();

        let doc_id = format!("{tree}:{path}");
        let term = Term::from_field_text(self.schema.doc_id, &doc_id);
        let query = TermQuery::new(term, IndexRecordOption::Basic);

        let matching_docs = searcher
            .search(&query, &TopDocs::with_limit(MAX_BULK_LOOKUP))
            .map_err(|e| IndexError::Write(e.to_string()))?;

        let mut results: Vec<SearchCandidate> = matching_docs
            .into_iter()
            .filter_map(|(_, doc_address)| {
                let doc: tantivy::TantivyDocument = searcher.doc(doc_address).ok()?;
                Some(self.read_candidate_from_doc(&doc))
            })
            .collect();

        results.sort_by(|a, b| a.id.cmp(&b.id));

        Ok(results)
    }

    /// Finds documents similar to an indexed document by ID.
    pub fn search_more_like_this_by_id(
        &mut self,
        id: &str,
        mlt_params: &MoreLikeThisParams,
        search_params: &SearchParams,
    ) -> Result<Vec<SearchResult>, IndexError> {
        let doc_address = self.get_doc_address(id)?.ok_or_else(|| {
            IndexError::Query(QueryError::compile(format!("document not found: {id}")))
        })?;

        let query = mlt_params.build_query_from_doc(doc_address);
        let exclude_ids: HashSet<String> = iter::once(id.to_string()).collect();

        self.run_mlt_search(Box::new(query), &exclude_ids, search_params)
    }

    /// Finds documents similar to arbitrary field content.
    pub fn search_more_like_this_by_fields(
        &mut self,
        fields: Vec<(&str, String)>,
        mlt_params: &MoreLikeThisParams,
        search_params: &SearchParams,
        exclude_doc_ids: &HashSet<String>,
    ) -> Result<Vec<SearchResult>, IndexError> {
        let tantivy_fields = self.convert_field_names_to_tantivy(fields)?;
        let query = mlt_params.build_query_from_fields(tantivy_fields);

        self.run_mlt_search(Box::new(query), exclude_doc_ids, search_params)
    }

    /// Looks up a document's address by its ID.
    ///
    /// The ID is the unique chunk identifier in the format `tree:path#slug` or `tree:path`.
    pub fn get_doc_address(&self, id: &str) -> Result<Option<DocAddress>, IndexError> {
        let reader = self
            .index
            .reader()
            .map_err(|e| IndexError::Write(e.to_string()))?;
        let searcher = reader.searcher();

        let term = Term::from_field_text(self.schema.id, id);
        let query = TermQuery::new(term, IndexRecordOption::Basic);

        let top_docs = searcher
            .search(&query, &TopDocs::with_limit(1))
            .map_err(|e| IndexError::Write(e.to_string()))?;

        Ok(top_docs.first().map(|(_, addr)| *addr))
    }

    /// Converts field name strings to Tantivy Field handles with values.
    fn convert_field_names_to_tantivy(
        &self,
        fields: Vec<(&str, String)>,
    ) -> Result<Vec<(Field, Vec<OwnedValue>)>, IndexError> {
        let mut result = Vec::with_capacity(fields.len());

        for (name, value) in fields {
            let field = match name {
                "body" => self.schema.body,
                "title" | "hierarchy" => self.schema.hierarchy,
                "tags" => self.schema.tags,
                "path" => self.schema.path,
                _ => {
                    return Err(IndexError::Query(QueryError::compile(format!(
                        "unknown field: {name}"
                    ))));
                }
            };
            result.push((field, vec![OwnedValue::Str(value)]));
        }

        Ok(result)
    }

    /// Returns information about the MLT query without executing search.
    ///
    /// This is useful for `--explain` mode to show what terms are extracted
    /// and what the generated query looks like.
    ///
    /// # Arguments
    /// * `id` - The chunk ID to analyze
    /// * `mlt_params` - MoreLikeThis configuration parameters
    ///
    /// # Returns
    /// A tuple of (source document title, extracted terms from the query).
    pub fn explain_more_like_this(
        &self,
        id: &str,
        mlt_params: &MoreLikeThisParams,
    ) -> Result<MoreLikeThisExplanation, IndexError> {
        let doc_address = self.get_doc_address(id)?.ok_or_else(|| {
            IndexError::Query(QueryError::compile(format!("document not found: {id}")))
        })?;

        let reader = self
            .index
            .reader()
            .map_err(|e| IndexError::Write(e.to_string()))?;
        let searcher = reader.searcher();

        let doc: tantivy::TantivyDocument = searcher
            .doc(doc_address)
            .map_err(|e| IndexError::Write(e.to_string()))?;

        let hierarchy: Vec<String> = doc
            .get_all(self.schema.hierarchy)
            .filter_map(|v| v.as_str())
            .map(|s| s.to_string())
            .collect();
        let title = hierarchy.last().cloned().unwrap_or_default();
        let body = self.get_text_field(&doc, self.schema.body);

        let query = mlt_params.build_query_from_doc(doc_address);
        let query_repr = format!("{query:?}");

        Ok(MoreLikeThisExplanation {
            source_id: id.to_string(),
            source_title: title,
            source_body_preview: body.chars().take(200).collect(),
            mlt_params: mlt_params.clone(),
            query_repr,
        })
    }

    /// Executes the MLT search with the unified pipeline.
    fn run_mlt_search(
        &self,
        query: Box<dyn Query>,
        exclude_ids: &HashSet<String>,
        params: &SearchParams,
    ) -> Result<Vec<SearchResult>, IndexError> {
        let query = self.apply_tree_filter(query, &params.trees);
        let effective_candidate_limit = params.effective_candidate_limit();

        let options = ExecutionOptions {
            with_snippets: false,
            with_details: false,
            original_query: None,
            include_explanation: false,
        };

        let raw_results = self.execute_query(&*query, &[], effective_candidate_limit, &options)?;

        let candidates: Vec<SearchCandidate> = raw_results
            .into_iter()
            .filter(|c| !exclude_ids.contains(&c.id) && !exclude_ids.contains(&c.doc_id))
            .collect();

        Ok(process_candidates(candidates, params, |parent_id| {
            self.lookup_parent(parent_id)
        }))
    }

    /// Parses and compiles a query string into a Tantivy query.
    pub(crate) fn build_query(
        &mut self,
        query_str: &str,
    ) -> Result<Option<Box<dyn Query>>, IndexError> {
        let expr = parse(query_str).map_err(|e| {
            let query_err: QueryError = e;
            IndexError::Query(query_err.with_query(query_str))
        })?;

        match expr {
            Some(e) => {
                let result = self.query_compiler.compile(&e).map_err(|e| {
                    let query_err: QueryError = e.into();
                    IndexError::Query(query_err.with_query(query_str))
                })?;
                Ok(result)
            }
            None => Ok(None),
        }
    }

    /// Builds a tree filter query for the given tree names.
    pub(crate) fn build_tree_filter(&self, trees: &[String]) -> Option<Box<dyn Query>> {
        if trees.is_empty() {
            return None;
        }

        if trees.len() == 1 {
            let term = Term::from_field_text(self.schema.tree, &trees[0]);
            return Some(Box::new(TermQuery::new(term, IndexRecordOption::Basic)));
        }

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
    pub(crate) fn apply_tree_filter(
        &self,
        content_query: Box<dyn Query>,
        trees: &[String],
    ) -> Box<dyn Query> {
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
    pub(crate) fn tokenize_query(&mut self, query_str: &str) -> Vec<String> {
        let filtered: String = query_str
            .split_whitespace()
            .filter(|word| {
                let upper = word.to_uppercase();
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

    /// Finds term mappings from query terms to indexed terms (including fuzzy matches).
    ///
    /// Returns a map where keys are query terms and values are the indexed terms
    /// they matched across the specified fields.
    pub(crate) fn find_term_mappings(
        &self,
        searcher: &TvSearcher,
        query_terms: &[String],
        fields: &[Field],
    ) -> HashMap<String, Vec<String>> {
        let mut mappings: HashMap<String, Vec<String>> = HashMap::new();

        if self.fuzzy_distance == 0 {
            for term in query_terms {
                mappings.insert(term.clone(), vec![term.clone()]);
            }
            return mappings;
        }

        for segment_reader in searcher.segment_readers() {
            for field in fields {
                let Ok(inverted_index) = segment_reader.inverted_index(*field) else {
                    continue;
                };
                let term_dict = inverted_index.terms();

                for query_term in query_terms {
                    let dfa = query::LevenshteinDfa(self.lev_builder.build_dfa(query_term));
                    let mut stream = term_dict.search(dfa).into_stream().unwrap();

                    let entry = mappings.entry(query_term.clone()).or_default();
                    while stream.advance() {
                        if let Ok(term_str) = str::from_utf8(stream.key())
                            && !entry.iter().any(|t| t == term_str)
                        {
                            entry.push(term_str.to_string());
                        }
                    }
                }
            }
        }

        for term in query_terms {
            mappings
                .entry(term.clone())
                .or_insert_with(|| vec![term.clone()]);
        }

        mappings
    }

    /// Executes a query with configurable options.
    pub(crate) fn execute_query(
        &self,
        query: &dyn Query,
        query_terms: &[String],
        limit: usize,
        options: &ExecutionOptions<'_>,
    ) -> Result<Vec<SearchCandidate>, IndexError> {
        let reader = self
            .index
            .reader()
            .map_err(|e| IndexError::Write(e.to_string()))?;

        let searcher = reader.searcher();

        let top_docs = searcher
            .search(query, &TopDocs::with_limit(limit))
            .map_err(|e| IndexError::Write(e.to_string()))?;

        let (matched_terms, term_mappings) = if options.with_details {
            let mappings = self.find_term_mappings(&searcher, query_terms, &[self.schema.body]);
            let mut terms: HashSet<String> = mappings.values().flatten().cloned().collect();

            let extra = self.find_term_mappings(
                &searcher,
                query_terms,
                &[self.schema.hierarchy, self.schema.path],
            );
            terms.extend(extra.values().flatten().cloned());

            (terms, Some(mappings))
        } else {
            let mappings = self.find_term_mappings(
                &searcher,
                query_terms,
                &[self.schema.body, self.schema.hierarchy, self.schema.path],
            );
            let terms: HashSet<String> = mappings.values().flatten().cloned().collect();
            (terms, None)
        };

        // Setup highlighting
        let (_highlight_query, snippet_generator) = if options.with_snippets || options.with_details
        {
            let hq = self.build_highlight_query(&matched_terms);
            let sg = self.build_snippet_generator(&searcher, &hq)?;
            (hq, sg)
        } else {
            (None, None)
        };

        let mut results = Vec::with_capacity(top_docs.len());

        // Prepare analyzer for details if needed (must be mutable, so we clone it)
        let mut details_analyzer = if options.with_details {
            Some(self.analyzer.clone())
        } else {
            None
        };

        for (score, doc_address) in top_docs {
            let doc: TantivyDocument = searcher
                .doc(doc_address)
                .map_err(|e| IndexError::Write(e.to_string()))?;

            let mut result = self.doc_to_result(&doc, score, &snippet_generator, &matched_terms);

            if let (Some(mappings), Some(original_query), Some(analyzer)) = (
                &term_mappings,
                options.original_query,
                details_analyzer.as_mut(),
            ) {
                let is_global = self
                    .tree_is_global
                    .get(&result.tree)
                    .copied()
                    .unwrap_or(false);
                let local_boost = if is_global { 1.0 } else { self.local_boost };

                let ctx = MatchDetailsContext {
                    query,
                    original_query,
                    query_terms,
                    term_mappings: mappings,
                    base_score: score,
                    local_boost,
                    searcher: &searcher,
                    doc_address,
                    include_explanation: options.include_explanation,
                };
                let details = self.collect_match_details(&doc, &ctx, analyzer);
                result.match_details = Some(details);
            }

            results.push(result);
        }

        Ok(results)
    }

    /// Creates a snippet generator when a highlight query is present.
    fn build_snippet_generator(
        &self,
        searcher: &TvSearcher,
        highlight_query: &Option<Box<dyn Query>>,
    ) -> Result<Option<SnippetGenerator>, IndexError> {
        if let Some(hq) = highlight_query {
            let mut generator = SnippetGenerator::create(searcher, hq.as_ref(), self.schema.body)
                .map_err(|e| IndexError::Write(e.to_string()))?;
            generator.set_max_num_chars(DEFAULT_SNIPPET_MAX_CHARS);
            Ok(Some(generator))
        } else {
            Ok(None)
        }
    }

    /// Collects detailed match information for a search result.
    fn collect_match_details(
        &self,
        doc: &TantivyDocument,
        ctx: &MatchDetailsContext<'_>,
        analyzer: &mut TextAnalyzer,
    ) -> MatchDetails {
        let all_matched_terms: HashSet<String> =
            ctx.term_mappings.values().flatten().cloned().collect();

        // Get hierarchy as multi-value field
        let hierarchy: Vec<String> = doc
            .get_all(self.schema.hierarchy)
            .filter_map(|v| v.as_str())
            .map(|s| s.to_string())
            .collect();
        let hierarchy_text = hierarchy.join(" ");
        let body = self.get_text_field(doc, self.schema.body);
        let tags_text: String = doc
            .get_all(self.schema.tags)
            .filter_map(|v| v.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let path = self.get_text_field(doc, self.schema.path);

        let (field_matches, field_scores) = self.analyze_field_matches(
            &all_matched_terms,
            &hierarchy_text,
            &body,
            &tags_text,
            &path,
            analyzer,
        );

        let score_explanation = if ctx.include_explanation {
            ctx.query
                .explain(ctx.searcher, ctx.doc_address)
                .ok()
                .map(|e| e.to_pretty_json())
        } else {
            None
        };

        let original_terms: Vec<String> = ctx
            .original_query
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
            stemmed_terms: ctx.query_terms.to_vec(),
            term_mappings: ctx.term_mappings.clone(),
            field_matches,
            base_score: ctx.base_score,
            field_scores,
            local_boost: ctx.local_boost,
            score_explanation,
        }
    }

    /// Builds a highlight query from actual matched terms.
    fn build_highlight_query(&self, matched_terms: &HashSet<String>) -> Option<Box<dyn Query>> {
        if matched_terms.is_empty() {
            return None;
        }

        let clauses: Vec<(Occur, Box<dyn Query>)> = matched_terms
            .iter()
            .map(|term_text| {
                let term = Term::from_field_text(self.schema.body, term_text);
                let query: Box<dyn Query> =
                    Box::new(TermQuery::new(term, IndexRecordOption::WithFreqs));
                let boosted: Box<dyn Query> = Box::new(BoostQuery::new(query, self.boosts.body));
                (Occur::Should, boosted)
            })
            .collect();

        Some(Box::new(BooleanQuery::new(clauses)))
    }

    /// Analyzes term matches across all searchable fields.
    ///
    /// Returns field match details and per-field scores based on term frequencies and boosts.
    fn analyze_field_matches(
        &self,
        matched_terms: &HashSet<String>,
        hierarchy_text: &str,
        body: &str,
        tags_text: &str,
        path: &str,
        analyzer: &mut TextAnalyzer,
    ) -> (HashMap<String, FieldMatch>, HashMap<String, f32>) {
        let mut field_matches = HashMap::new();
        let mut field_scores = HashMap::new();

        for (field_name, text, field_boost) in [
            ("hierarchy", hierarchy_text, self.boosts.hierarchy),
            ("body", body, self.boosts.body),
            ("tags", tags_text, self.boosts.tags),
            ("path", path, self.boosts.path),
        ] {
            let freqs = self.count_term_frequency_in_text(text, matched_terms, analyzer);
            if !freqs.is_empty() {
                let score: f32 = freqs.values().map(|&c| c as f32).sum::<f32>() * field_boost;
                field_scores.insert(field_name.to_string(), score);
                field_matches.insert(
                    field_name.to_string(),
                    FieldMatch {
                        term_frequencies: freqs,
                    },
                );
            }
        }

        (field_matches, field_scores)
    }

    /// Counts how often matched terms occur in the provided text.
    fn count_term_frequency_in_text(
        &self,
        text: &str,
        terms: &HashSet<String>,
        analyzer: &mut TextAnalyzer,
    ) -> HashMap<String, u32> {
        let mut freqs: HashMap<String, u32> = HashMap::new();
        let mut stream = analyzer.token_stream(text);
        while let Some(token) = stream.next() {
            if terms.contains(&token.text) {
                *freqs.entry(token.text.clone()).or_insert(0) += 1;
            }
        }
        freqs
    }

    /// Reads all metadata fields from a Tantivy document into a `SearchCandidate`.
    ///
    /// Returns a candidate with zero score and empty match data. Use this as a base
    /// for building search results or for parent lookups during aggregation.
    pub(crate) fn read_candidate_from_doc(&self, doc: &TantivyDocument) -> SearchCandidate {
        let id = self.get_text_field(doc, self.schema.id);
        let doc_id = self.get_text_field(doc, self.schema.doc_id);
        let parent_id_str = self.get_text_field(doc, self.schema.parent_id);
        let parent_id = if parent_id_str.is_empty() {
            None
        } else {
            Some(parent_id_str)
        };
        let hierarchy: Vec<String> = doc
            .get_all(self.schema.hierarchy)
            .filter_map(|v| v.as_str())
            .map(|s| s.to_string())
            .collect();
        let tree = self.get_text_field(doc, self.schema.tree);
        let path = self.get_text_field(doc, self.schema.path);
        let body = self.get_text_field(doc, self.schema.body);
        let depth = self.get_u64_field(doc, self.schema.depth);
        let position = self.get_u64_field(doc, self.schema.position);
        let byte_start = self.get_u64_field(doc, self.schema.byte_start);
        let byte_end = self.get_u64_field(doc, self.schema.byte_end);
        let sibling_count = self.get_u64_field(doc, self.schema.sibling_count);

        SearchCandidate {
            id,
            doc_id,
            parent_id,
            hierarchy,
            depth,
            tree,
            path,
            body,
            position,
            byte_start,
            byte_end,
            sibling_count,
            score: 0.0,
            snippet: None,
            match_ranges: vec![],
            hierarchy_match_ranges: vec![],
            path_match_ranges: vec![],
            match_details: None,
        }
    }

    /// Converts a Tantivy document plus scoring context into a `SearchCandidate`.
    pub(crate) fn doc_to_result(
        &self,
        doc: &TantivyDocument,
        base_score: f32,
        snippet_generator: &Option<SnippetGenerator>,
        matched_terms: &HashSet<String>,
    ) -> SearchCandidate {
        let mut candidate = self.read_candidate_from_doc(doc);

        // Apply heading depth boost and local tree boost
        let is_global = self
            .tree_is_global
            .get(&candidate.tree)
            .copied()
            .unwrap_or(false);
        let heading_boost = self.boosts.heading_boost(candidate.depth);
        candidate.score = if is_global {
            base_score * heading_boost
        } else {
            base_score * heading_boost * self.local_boost
        };

        // Generate snippet if generator provided
        candidate.snippet = snippet_generator.as_ref().map(|generator| {
            let snippet = generator.snippet_from_doc(doc);
            snippet.to_html()
        });

        // Extract match ranges
        candidate.match_ranges =
            extract_match_ranges(&self.analyzer, &candidate.body, matched_terms);
        let title = candidate.hierarchy.last().map(|s| s.as_str()).unwrap_or("");
        candidate.hierarchy_match_ranges =
            extract_match_ranges(&self.analyzer, title, matched_terms);
        candidate.path_match_ranges =
            extract_match_ranges(&self.analyzer, &candidate.path, matched_terms);

        candidate
    }

    /// Reads a text field from a document, returning an empty string if missing.
    pub(crate) fn get_text_field(&self, doc: &TantivyDocument, field: Field) -> String {
        doc.get_first(field)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    }

    /// Reads a u64 field from a document, returning zero if missing.
    pub(crate) fn get_u64_field(&self, doc: &TantivyDocument, field: Field) -> u64 {
        doc.get_first(field).and_then(|v| v.as_u64()).unwrap_or(0)
    }
}

impl IdfProvider for Searcher {
    fn idf(&self, term: &str) -> Option<f32> {
        self.term_idf(term).ok().flatten()
    }
}

/// A wrapper around `Searcher` that filters IDF lookups to specific trees.
pub struct TreeFilteredSearcher<'a> {
    /// Underlying searcher providing IDF values.
    searcher: &'a Searcher,
    /// Trees to limit IDF calculations to.
    trees: Vec<String>,
}

impl<'a> TreeFilteredSearcher<'a> {
    /// Creates a new tree-filtered searcher.
    pub fn new(searcher: &'a Searcher, trees: Vec<String>) -> Self {
        Self { searcher, trees }
    }
}

impl IdfProvider for TreeFilteredSearcher<'_> {
    fn idf(&self, term: &str) -> Option<f32> {
        self.searcher
            .term_idf_in_trees(term, &self.trees)
            .ok()
            .flatten()
    }
}

/// Creates an index directory path and opens it for searching.
///
/// If `fuzzy_override` is provided, it overrides the config's fuzzy_distance setting.
pub fn open_searcher(
    config: &ra_config::Config,
    fuzzy_override: Option<u8>,
) -> Result<Searcher, IndexError> {
    let index_dir = crate::index_directory(config).ok_or_else(|| IndexError::OpenIndex {
        path: PathBuf::new(),
        message: "no configuration found".to_string(),
    })?;

    let fuzzy_distance = fuzzy_override.unwrap_or(config.search.fuzzy_distance);
    Searcher::open_with_boosts(
        &index_dir,
        &config.search.stemmer,
        &config.trees,
        config.settings.local_boost,
        fuzzy_distance,
        config.search.field_boosts(),
    )
}
