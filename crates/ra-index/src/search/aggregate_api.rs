//! Aggregated search entry points and parent lookup.

use tantivy::Term;
use tantivy::collector::TopDocs;
use tantivy::query::TermQuery;
use tantivy::schema::IndexRecordOption;

use super::execute::{aggregate_candidates, apply_elbow, single_results_from_candidates};
use super::{SearchParams, Searcher};
use crate::IndexError;
use crate::aggregate::ParentInfo;
use crate::result::{SearchCandidate, SearchResult as AggregatedSearchResult};

impl Searcher {
    /// Searches using the three-phase hierarchical algorithm.
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
        let filtered = apply_elbow(candidates, params.cutoff_ratio, params.max_results);

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
        let filtered = apply_elbow(candidates, params.cutoff_ratio, params.max_results);

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
