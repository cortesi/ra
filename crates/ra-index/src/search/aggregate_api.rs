//! Aggregated search entry points and parent lookup.
//!
//! This module provides the main search entry points that combine query execution
//! with the unified result pipeline. All search operations (string queries,
//! expression queries) flow through the same pipeline for consistent behavior.
//!
//! See the [`super::pipeline`] module for pipeline details.

use tantivy::{
    Term,
    collector::TopDocs,
    query::{Query, TermQuery},
    schema::IndexRecordOption,
};

use super::{SearchParams, Searcher, pipeline::process_candidates};
use crate::{IndexError, SearchCandidate, result::SearchResult as AggregatedSearchResult};

impl Searcher {
    /// Searches using the hierarchical algorithm with per-tree score normalization.
    ///
    /// This is the main entry point for string-based queries. Query execution is
    /// followed by the unified result pipeline (see [`super::pipeline`]).
    pub fn search_aggregated(
        &mut self,
        query_str: &str,
        params: &SearchParams,
    ) -> Result<Vec<AggregatedSearchResult>, IndexError> {
        let content_query = match self.build_query(query_str)? {
            Some(q) => q,
            None => return Ok(Vec::new()),
        };

        let query_terms = self.tokenize_query(query_str);

        self.run_aggregated_search(content_query, &query_terms, query_str, params)
    }

    /// Searches using a pre-built query expression.
    ///
    /// This is the main entry point for expression-based queries (e.g., from context search).
    /// Query execution is followed by the unified result pipeline (see [`super::pipeline`]).
    pub fn search_aggregated_expr(
        &mut self,
        expr: &ra_query::QueryExpr,
        params: &SearchParams,
    ) -> Result<Vec<AggregatedSearchResult>, IndexError> {
        use crate::query::QueryError;

        let content_query = match self.query_compiler.compile(expr).map_err(|e| {
            let query_err: QueryError = e.into();
            IndexError::Query(query_err)
        })? {
            Some(q) => q,
            None => return Ok(Vec::new()),
        };

        let query_terms = expr.extract_terms();
        let display_query = expr.to_query_string();

        self.run_aggregated_search(content_query, &query_terms, &display_query, params)
    }

    /// Executes query and processes results through the unified pipeline.
    fn run_aggregated_search(
        &mut self,
        content_query: Box<dyn Query>,
        query_terms: &[String],
        display_query: &str,
        params: &SearchParams,
    ) -> Result<Vec<AggregatedSearchResult>, IndexError> {
        use super::execute::ExecutionOptions;

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

        let candidates = self.execute_query(
            &*query,
            query_terms,
            effective_candidate_limit,
            options,
        )?;

        // Process through unified pipeline
        Ok(process_candidates(candidates, params, |parent_id| {
            self.lookup_parent(parent_id)
        }))
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
}
