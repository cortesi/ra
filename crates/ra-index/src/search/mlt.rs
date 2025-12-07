//! MoreLikeThis query support for finding similar documents.
//!
//! This module provides the [`MoreLikeThisParams`] configuration struct and methods
//! on [`Searcher`] for finding documents similar to a given document or set of field values.
//!
//! The implementation wraps Tantivy's `MoreLikeThisQuery` and integrates with the existing
//! search pipeline (normalization, elbow cutoff, aggregation).

use std::{collections::HashSet, iter};

use tantivy::{
    DocAddress, Term,
    collector::TopDocs,
    query::{MoreLikeThisQuery, Query, TermQuery},
    schema::{Field, IndexRecordOption, OwnedValue, Value},
};

use super::{
    SearchParams, Searcher,
    execute::{aggregate_candidates, apply_elbow, single_results_from_candidates},
    normalize::normalize_scores_across_trees,
};
use crate::{
    IndexError, QueryError, SearchCandidate, result::SearchResult as AggregatedSearchResult,
};

/// Parameters for MoreLikeThis queries.
///
/// Controls how Tantivy's `MoreLikeThisQuery` extracts and weights terms from the source
/// document to find similar content.
#[derive(Debug, Clone)]
pub struct MoreLikeThisParams {
    /// Minimum document frequency for terms. Terms appearing in fewer documents are ignored.
    pub min_doc_frequency: u64,
    /// Maximum document frequency for terms. Terms appearing in more documents are ignored.
    pub max_doc_frequency: u64,
    /// Minimum term frequency in the source document. Terms appearing fewer times are ignored.
    pub min_term_frequency: usize,
    /// Maximum number of query terms to use.
    pub max_query_terms: usize,
    /// Minimum word length. Shorter words are ignored.
    pub min_word_length: usize,
    /// Maximum word length. Longer words are ignored.
    pub max_word_length: usize,
    /// Boost factor applied to terms.
    pub boost_factor: f32,
    /// Stop words to ignore.
    pub stop_words: Vec<String>,
}

impl Default for MoreLikeThisParams {
    fn default() -> Self {
        Self {
            min_doc_frequency: 1,
            max_doc_frequency: u64::MAX / 2,
            min_term_frequency: 1,
            max_query_terms: 25,
            min_word_length: 3,
            max_word_length: 40,
            boost_factor: 1.0,
            stop_words: Vec::new(),
        }
    }
}

/// Explanation of a MoreLikeThis query for debugging.
///
/// Contains information about the source document and the generated query.
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

impl MoreLikeThisParams {
    /// Sets the minimum document frequency.
    pub fn with_min_doc_frequency(mut self, value: u64) -> Self {
        self.min_doc_frequency = value;
        self
    }

    /// Sets the maximum document frequency.
    pub fn with_max_doc_frequency(mut self, value: u64) -> Self {
        self.max_doc_frequency = value;
        self
    }

    /// Sets the minimum term frequency.
    pub fn with_min_term_frequency(mut self, value: usize) -> Self {
        self.min_term_frequency = value;
        self
    }

    /// Sets the maximum query terms.
    pub fn with_max_query_terms(mut self, value: usize) -> Self {
        self.max_query_terms = value;
        self
    }

    /// Sets the minimum word length.
    pub fn with_min_word_length(mut self, value: usize) -> Self {
        self.min_word_length = value;
        self
    }

    /// Sets the maximum word length.
    pub fn with_max_word_length(mut self, value: usize) -> Self {
        self.max_word_length = value;
        self
    }

    /// Sets the boost factor.
    pub fn with_boost_factor(mut self, value: f32) -> Self {
        self.boost_factor = value;
        self
    }

    /// Sets the stop words.
    pub fn with_stop_words(mut self, words: Vec<String>) -> Self {
        self.stop_words = words;
        self
    }

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

impl Searcher {
    /// Finds documents similar to an indexed document by ID.
    ///
    /// Looks up the document by its chunk ID (e.g., `tree:path#slug`), extracts
    /// its content, and finds similar documents using Tantivy's MoreLikeThisQuery.
    ///
    /// # Arguments
    /// * `id` - The chunk ID to find similar documents for
    /// * `mlt_params` - MoreLikeThis configuration parameters
    /// * `search_params` - Standard search parameters (limit, trees, aggregation, etc.)
    ///
    /// # Returns
    /// Aggregated search results, excluding the source document itself.
    pub fn search_more_like_this_by_id(
        &mut self,
        id: &str,
        mlt_params: &MoreLikeThisParams,
        search_params: &SearchParams,
    ) -> Result<Vec<AggregatedSearchResult>, IndexError> {
        let doc_address = self.get_doc_address(id)?.ok_or_else(|| {
            IndexError::Query(QueryError::compile(format!("document not found: {id}")))
        })?;

        let query = mlt_params.build_query_from_doc(doc_address);
        let exclude_ids: HashSet<String> = iter::once(id.to_string()).collect();

        self.run_mlt_search(Box::new(query), &exclude_ids, search_params)
    }

    /// Finds documents similar to arbitrary field content.
    ///
    /// This allows finding similar documents for content not in the index,
    /// such as an external file or user-provided text.
    ///
    /// # Arguments
    /// * `fields` - Field name and content pairs (e.g., `[("body", "some text"), ("title", "Title")]`)
    /// * `mlt_params` - MoreLikeThis configuration parameters
    /// * `search_params` - Standard search parameters (limit, trees, aggregation, etc.)
    /// * `exclude_doc_ids` - Document IDs to exclude from results (e.g., if the content came from
    ///   an indexed file)
    ///
    /// # Returns
    /// Aggregated search results.
    pub fn search_more_like_this_by_fields(
        &mut self,
        fields: Vec<(&str, String)>,
        mlt_params: &MoreLikeThisParams,
        search_params: &SearchParams,
        exclude_doc_ids: &HashSet<String>,
    ) -> Result<Vec<AggregatedSearchResult>, IndexError> {
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
                // "title" is an alias for "hierarchy" for backwards compatibility
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

        // Get the source document to show its content
        let doc: tantivy::TantivyDocument = searcher
            .doc(doc_address)
            .map_err(|e| IndexError::Write(e.to_string()))?;

        // Read hierarchy as multi-value field and get title (last element)
        let hierarchy: Vec<String> = doc
            .get_all(self.schema.hierarchy)
            .filter_map(|v| v.as_str())
            .map(|s| s.to_string())
            .collect();
        let title = hierarchy.last().cloned().unwrap_or_default();
        let body = self.get_text_field(&doc, self.schema.body);

        // Build the MLT query and extract the generated query
        let query = mlt_params.build_query_from_doc(doc_address);

        // Get a string representation of the query
        let query_repr = format!("{query:?}");

        Ok(MoreLikeThisExplanation {
            source_id: id.to_string(),
            source_title: title,
            source_body_preview: body.chars().take(200).collect(),
            mlt_params: mlt_params.clone(),
            query_repr,
        })
    }

    /// Executes the MLT search with the standard pipeline.
    fn run_mlt_search(
        &self,
        query: Box<dyn Query>,
        exclude_ids: &HashSet<String>,
        params: &SearchParams,
    ) -> Result<Vec<AggregatedSearchResult>, IndexError> {
        let query = self.apply_tree_filter(query, &params.trees);

        // Phase 1: Execute query
        let effective_candidate_limit = params.effective_candidate_limit();
        let raw_results =
            self.execute_query_no_highlights(&*query, &[], effective_candidate_limit)?;

        // Filter out excluded documents
        let candidates: Vec<SearchCandidate> = raw_results
            .into_iter()
            .filter(|c| !exclude_ids.contains(&c.id) && !exclude_ids.contains(&c.doc_id))
            .collect();

        // Phase 2: Normalize scores across trees
        let normalized = normalize_scores_across_trees(candidates, params.trees.len());

        // Phase 3: Apply elbow cutoff
        let filtered = apply_elbow(normalized, params.cutoff_ratio, params.max_candidates);

        // Phase 4: Aggregate siblings
        let mut results = if params.disable_aggregation {
            single_results_from_candidates(filtered)
        } else {
            aggregate_candidates(filtered, params.aggregation_threshold, |parent_id| {
                self.lookup_parent(parent_id)
            })
        };

        // Phase 5: Apply final limit
        results.truncate(params.limit);

        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mlt_params_default() {
        let params = MoreLikeThisParams::default();
        assert_eq!(params.min_doc_frequency, 1);
        assert_eq!(params.max_doc_frequency, u64::MAX / 2);
        assert_eq!(params.min_term_frequency, 1);
        assert_eq!(params.max_query_terms, 25);
        assert_eq!(params.min_word_length, 3);
        assert_eq!(params.max_word_length, 40);
        assert_eq!(params.boost_factor, 1.0);
        assert!(params.stop_words.is_empty());
    }

    #[test]
    fn test_mlt_params_builder() {
        let params = MoreLikeThisParams::default()
            .with_min_doc_frequency(5)
            .with_max_doc_frequency(1000)
            .with_min_term_frequency(2)
            .with_max_query_terms(50)
            .with_min_word_length(4)
            .with_max_word_length(30)
            .with_boost_factor(2.0)
            .with_stop_words(vec!["the".to_string(), "a".to_string()]);

        assert_eq!(params.min_doc_frequency, 5);
        assert_eq!(params.max_doc_frequency, 1000);
        assert_eq!(params.min_term_frequency, 2);
        assert_eq!(params.max_query_terms, 50);
        assert_eq!(params.min_word_length, 4);
        assert_eq!(params.max_word_length, 30);
        assert_eq!(params.boost_factor, 2.0);
        assert_eq!(params.stop_words, vec!["the", "a"]);
    }
}
