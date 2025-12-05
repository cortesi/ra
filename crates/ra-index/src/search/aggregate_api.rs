//! Aggregated search entry points and parent lookup.
//!
//! This module provides the main search entry points that combine all phases of the
//! search algorithm:
//!
//! 1. Query execution with tree filtering
//! 2. Per-tree score normalization (for multi-tree searches)
//! 3. Elbow cutoff for relevance filtering
//! 4. Hierarchical aggregation of sibling matches
//!
//! See the [`crate::search`] module documentation for an overview of the algorithm.

use tantivy::{Term, collector::TopDocs, query::TermQuery, schema::IndexRecordOption};

use super::{
    SearchParams, Searcher,
    execute::{aggregate_candidates, apply_elbow, single_results_from_candidates},
    normalize::normalize_scores_across_trees,
};
use crate::{
    IndexError,
    aggregate::ParentInfo,
    result::{SearchCandidate, SearchResult as AggregatedSearchResult},
};

impl Searcher {
    /// Searches using the hierarchical algorithm with per-tree score normalization.
    ///
    /// This is the main entry point for string-based queries. The search proceeds through
    /// four phases:
    ///
    /// 1. **Query Execution**: Parse and execute the query with tree filtering
    /// 2. **Score Normalization**: For multi-tree searches, normalize scores so each
    ///    tree's best result gets 1.0 (see [`normalize_scores_across_trees`])
    /// 3. **Elbow Cutoff**: Truncate at the relevance cliff (see [`apply_elbow`])
    /// 4. **Aggregation**: Group sibling matches under parents (see [`aggregate_candidates`])
    pub fn search_aggregated(
        &mut self,
        query_str: &str,
        params: &SearchParams,
    ) -> Result<Vec<AggregatedSearchResult>, IndexError> {
        let content_query = match self.build_query(query_str)? {
            Some(q) => q,
            None => return Ok(Vec::new()),
        };

        let query = self.apply_tree_filter(content_query, &params.trees);
        let query_terms = self.tokenize_query(query_str);

        // Phase 1: Execute query and get raw results
        let raw_results = if params.verbosity > 0 {
            self.execute_query_with_details(
                &*query,
                query_str,
                &query_terms,
                params.candidate_limit,
                params.verbosity >= 2,
            )?
        } else {
            self.execute_query_with_highlights(&*query, &query_terms, params.candidate_limit)?
        };

        let candidates: Vec<SearchCandidate> =
            raw_results.into_iter().map(SearchCandidate::from).collect();

        // Phase 2: Normalize scores across trees (only for multi-tree searches)
        // This ensures fair comparison when trees have different content densities.
        let normalized = normalize_scores_across_trees(candidates, params.trees.len());

        // Phase 3: Apply elbow cutoff on normalized scores
        let filtered = apply_elbow(normalized, params.cutoff_ratio, params.max_results);

        // Phase 4: Aggregate siblings under parent nodes
        if params.disable_aggregation {
            Ok(single_results_from_candidates(filtered))
        } else {
            Ok(aggregate_candidates(
                filtered,
                params.aggregation_threshold,
                |parent_id| self.lookup_parent(parent_id),
            ))
        }
    }

    /// Searches using a pre-built query expression.
    ///
    /// This is the main entry point for expression-based queries (e.g., from context search).
    /// The search proceeds through four phases:
    ///
    /// 1. **Query Execution**: Compile and execute the expression with tree filtering
    /// 2. **Score Normalization**: For multi-tree searches, normalize scores so each
    ///    tree's best result gets 1.0 (see [`normalize_scores_across_trees`])
    /// 3. **Elbow Cutoff**: Truncate at the relevance cliff (see [`apply_elbow`])
    /// 4. **Aggregation**: Group sibling matches under parents (see [`aggregate_candidates`])
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

        let query = self.apply_tree_filter(content_query, &params.trees);
        let query_terms = expr.extract_terms();

        // Phase 1: Execute query and get raw results
        let raw_results = if params.verbosity > 0 {
            self.execute_query_with_details(
                &*query,
                &expr.to_query_string(),
                &query_terms,
                params.candidate_limit,
                params.verbosity >= 2,
            )?
        } else {
            self.execute_query_with_highlights(&*query, &query_terms, params.candidate_limit)?
        };

        let candidates: Vec<SearchCandidate> =
            raw_results.into_iter().map(SearchCandidate::from).collect();

        // Phase 2: Normalize scores across trees (only for multi-tree searches)
        // This ensures fair comparison when trees have different content densities.
        let normalized = normalize_scores_across_trees(candidates, params.trees.len());

        // Phase 3: Apply elbow cutoff on normalized scores
        let filtered = apply_elbow(normalized, params.cutoff_ratio, params.max_results);

        // Phase 4: Aggregate siblings under parent nodes
        if params.disable_aggregation {
            Ok(single_results_from_candidates(filtered))
        } else {
            Ok(aggregate_candidates(
                filtered,
                params.aggregation_threshold,
                |parent_id| self.lookup_parent(parent_id),
            ))
        }
    }

    /// Looks up a parent node by ID for aggregation.
    fn lookup_parent(&self, parent_id: &str) -> Option<ParentInfo> {
        let reader = self.index.reader().ok()?;
        let searcher = reader.searcher();

        let term = Term::from_field_text(self.schema.id, parent_id);
        let query = TermQuery::new(term, IndexRecordOption::Basic);

        let top_docs = searcher.search(&query, &TopDocs::with_limit(1)).ok()?;

        if let Some((_, doc_address)) = top_docs.first() {
            let doc: tantivy::TantivyDocument = searcher.doc(*doc_address).ok()?;

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
}
