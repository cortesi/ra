//! Query execution paths and result conversion.

use std::{
    collections::{HashMap, HashSet},
    ops::Range,
};

use tantivy::{
    TantivyDocument, Term,
    collector::TopDocs,
    query::{BooleanQuery, BoostQuery, Occur, Query, TermQuery},
    schema::{Field, IndexRecordOption, Value},
    snippet::SnippetGenerator,
    tokenizer::TextAnalyzer,
};

use super::{
    Searcher,
    types::{FieldMatch, MatchDetails, SearchCandidate},
};
use crate::IndexError;

/// Default maximum number of characters in a snippet.
const DEFAULT_SNIPPET_MAX_CHARS: usize = 150;

/// Merges two sets of byte ranges, combining overlapping or adjacent ranges.
///
/// The result is sorted by start position with no overlaps.
pub fn merge_ranges(mut a: Vec<Range<usize>>, b: Vec<Range<usize>>) -> Vec<Range<usize>> {
    a.extend(b);
    if a.is_empty() {
        return a;
    }

    a.sort_by_key(|r| r.start);

    let mut merged = Vec::with_capacity(a.len());
    let mut current = a[0].clone();

    for range in a.into_iter().skip(1) {
        if range.start <= current.end {
            current.end = current.end.max(range.end);
        } else {
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
pub(super) fn extract_match_ranges(
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

    merge_ranges(ranges, Vec::new())
}

impl Searcher {
    /// Executes a query with snippet and highlight generation.
    pub(crate) fn execute_query_with_highlights(
        &self,
        query: &dyn Query,
        query_terms: &[String],
        limit: usize,
    ) -> Result<Vec<SearchCandidate>, IndexError> {
        self.execute_query_core(query, query_terms, limit, true)
    }

    /// Executes a query without generating snippets or highlights (faster).
    pub(crate) fn execute_query_no_highlights(
        &self,
        query: &dyn Query,
        query_terms: &[String],
        limit: usize,
    ) -> Result<Vec<SearchCandidate>, IndexError> {
        self.execute_query_core(query, query_terms, limit, false)
    }

    /// Executes a query with full match detail collection.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn execute_query_with_details(
        &mut self,
        query: &dyn Query,
        original_query: &str,
        query_terms: &[String],
        limit: usize,
        include_explanation: bool,
    ) -> Result<Vec<SearchCandidate>, IndexError> {
        let reader = self
            .index
            .reader()
            .map_err(|e| IndexError::Write(e.to_string()))?;

        let searcher = reader.searcher();

        let top_docs = searcher
            .search(query, &TopDocs::with_limit(limit))
            .map_err(|e| IndexError::Write(e.to_string()))?;

        let term_mappings = self.find_term_mappings(&searcher, query_terms);

        let mut matched_terms: HashSet<String> =
            term_mappings.values().flatten().cloned().collect();
        let extra_terms = self.find_matched_terms(
            &searcher,
            query_terms,
            &[self.schema.hierarchy, self.schema.path],
        );
        matched_terms.extend(extra_terms);

        let highlight_query = self.build_highlight_query(&matched_terms);

        let snippet_generator = self.build_snippet_generator(&searcher, &highlight_query)?;

        let mut results = Vec::with_capacity(top_docs.len());

        for (score, doc_address) in top_docs {
            let doc: TantivyDocument = searcher
                .doc(doc_address)
                .map_err(|e| IndexError::Write(e.to_string()))?;

            let mut result = self.doc_to_result(&doc, score, &snippet_generator, &matched_terms);

            let is_global = self
                .tree_is_global
                .get(&result.tree)
                .copied()
                .unwrap_or(false);
            let local_boost = if is_global { 1.0 } else { self.local_boost };

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

    /// Shared execution path for searches with optional snippet generation.
    fn execute_query_core(
        &self,
        query: &dyn Query,
        query_terms: &[String],
        limit: usize,
        with_snippets: bool,
    ) -> Result<Vec<SearchCandidate>, IndexError> {
        let reader = self
            .index
            .reader()
            .map_err(|e| IndexError::Write(e.to_string()))?;

        let searcher = reader.searcher();

        let matched_terms = self.find_matched_terms(
            &searcher,
            query_terms,
            &[self.schema.body, self.schema.hierarchy, self.schema.path],
        );

        let highlight_query = if with_snippets {
            self.build_highlight_query(&matched_terms)
        } else {
            None
        };

        let snippet_generator = self.build_snippet_generator(&searcher, &highlight_query)?;

        let top_docs = searcher
            .search(query, &TopDocs::with_limit(limit))
            .map_err(|e| IndexError::Write(e.to_string()))?;

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

    /// Creates a snippet generator when a highlight query is present.
    fn build_snippet_generator(
        &self,
        searcher: &tantivy::Searcher,
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
        let all_matched_terms: HashSet<String> =
            term_mappings.values().flatten().cloned().collect();

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
        );

        let score_explanation = if include_explanation {
            query
                .explain(searcher, doc_address)
                .ok()
                .map(|e| e.to_pretty_json())
        } else {
            None
        };

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
        &mut self,
        matched_terms: &HashSet<String>,
        hierarchy_text: &str,
        body: &str,
        tags_text: &str,
        path: &str,
    ) -> (HashMap<String, FieldMatch>, HashMap<String, f32>) {
        let mut field_matches = HashMap::new();
        let mut field_scores = HashMap::new();

        for (field_name, text, field_boost) in [
            ("hierarchy", hierarchy_text, self.boosts.hierarchy),
            ("body", body, self.boosts.body),
            ("tags", tags_text, self.boosts.tags),
            ("path", path, self.boosts.path),
        ] {
            let freqs = self.count_term_frequency_in_text(text, matched_terms);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_ranges_combines_overlapping() {
        let a = vec![0..5, 10..15];
        let b = vec![3..8, 20..25];
        let merged = merge_ranges(a, b);

        assert_eq!(merged, vec![0..8, 10..15, 20..25]);
    }

    #[test]
    #[allow(clippy::single_range_in_vec_init)]
    fn merge_ranges_combines_adjacent() {
        let a = vec![0..5];
        let b = vec![5..10];
        let merged = merge_ranges(a, b);

        assert_eq!(merged, vec![0..10]);
    }

    #[test]
    fn merge_ranges_handles_empty() {
        let a: Vec<Range<usize>> = vec![];
        let b: Vec<Range<usize>> = vec![];
        let merged = merge_ranges(a, b);

        assert!(merged.is_empty());
    }

    #[test]
    #[allow(clippy::single_range_in_vec_init)]
    fn merge_ranges_preserves_non_overlapping() {
        let a = vec![0..5];
        let b = vec![10..15];
        let merged = merge_ranges(a, b);

        assert_eq!(merged, vec![0..5, 10..15]);
    }
}
