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
};

use execute::ExecutionOptions;
pub use execute::merge_ranges;
use levenshtein_automata::LevenshteinAutomatonBuilder;
pub use params::{MoreLikeThisParams, SearchParams};
pub use pipeline::PipelineStats;
use pipeline::{process_candidates, process_candidates_with_stats};
use ra_config::FieldBoosts;
use ra_context::IdfProvider;
use tantivy::{
    DocAddress, Index, Term,
    collector::{Count, TopDocs},
    directory::MmapDirectory,
    query::{AllQuery, MoreLikeThisQuery, Query, TermQuery},
    schema::{Field, IndexRecordOption, OwnedValue, Value},
    tokenizer::TextAnalyzer,
};
pub use types::{MatchDetails, SearchCandidate};

use crate::{
    IndexError, QueryError,
    analyzer::{RA_TOKENIZER, build_analyzer_from_name},
    query::QueryCompiler,
    result::SearchResult,
    schema::IndexSchema,
};

/// Maximum number of documents to retrieve in bulk lookup operations.
const MAX_BULK_LOOKUP: usize = 100_000;

/// Explanation of a MoreLikeThis query for debugging.
#[derive(Debug, Clone)]
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

#[allow(clippy::multiple_inherent_impl)]
impl MoreLikeThisParams {
    /// Builds a Tantivy MoreLikeThisQuery from a document address.
    fn build_query_from_doc(&self, doc_address: DocAddress) -> MoreLikeThisQuery {
        MoreLikeThisQuery::builder()
            .with_min_doc_frequency(self.min_doc_frequency)
            .with_max_doc_frequency(self.max_doc_frequency)
            .with_min_term_frequency(self.min_term_frequency)
            .with_max_query_terms(self.max_query_terms)
            .with_min_word_length(self.min_word_length)
            .with_max_word_length(self.max_word_length)
            .with_boost_factor(self.boost_factor)
            .with_stop_words(self.stop_words.clone())
            .with_document(doc_address)
    }

    /// Builds a Tantivy MoreLikeThisQuery from field values.
    fn build_query_from_fields(&self, fields: Vec<(Field, Vec<OwnedValue>)>) -> MoreLikeThisQuery {
        MoreLikeThisQuery::builder()
            .with_min_doc_frequency(self.min_doc_frequency)
            .with_max_doc_frequency(self.max_doc_frequency)
            .with_min_term_frequency(self.min_term_frequency)
            .with_max_query_terms(self.max_query_terms)
            .with_min_word_length(self.min_word_length)
            .with_max_word_length(self.max_word_length)
            .with_boost_factor(self.boost_factor)
            .with_stop_words(self.stop_words.clone())
            .with_document_fields(fields)
    }
}

/// Primary search entry point for the index.
#[allow(clippy::multiple_inherent_impl)]
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
