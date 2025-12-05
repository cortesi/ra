//! Query execution paths and result conversion.

use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    mem,
};

use tantivy::{
    TantivyDocument, Term,
    collector::TopDocs,
    query::{BooleanQuery, BoostQuery, Occur, Query, TermQuery},
    schema::{Field, IndexRecordOption, Value},
    snippet::SnippetGenerator,
};

use super::{
    Searcher,
    ranges::{extract_match_ranges, merge_ranges},
    types::{FieldMatch, MatchDetails, SearchResult},
};
use crate::{
    IndexError,
    aggregate::{ParentInfo, aggregate},
    elbow::elbow_cutoff,
    result::{SearchCandidate, SearchResult as AggregatedSearchResult},
    schema::boost,
};

/// Default maximum number of characters in a snippet.
const DEFAULT_SNIPPET_MAX_CHARS: usize = 150;

impl Searcher {
    /// Searches the index for documents matching the query.
    pub fn search(
        &mut self,
        query_str: &str,
        limit: usize,
    ) -> Result<Vec<SearchResult>, IndexError> {
        let query = match self.build_query(query_str)? {
            Some(q) => q,
            None => return Ok(Vec::new()),
        };

        let query_terms = self.tokenize_query(query_str);

        self.execute_query_with_highlights(&*query, &query_terms, limit)
    }

    /// Searches the index for documents matching multiple topics.
    ///
    /// Each topic is searched independently and results are combined with deduplication.
    /// When a document matches multiple topics, the match ranges from all topics are merged
    /// and the highest score is kept.
    pub fn search_multi(
        &mut self,
        topics: &[&str],
        limit: usize,
    ) -> Result<Vec<SearchResult>, IndexError> {
        if topics.is_empty() {
            return Ok(Vec::new());
        }

        let mut results_by_id: HashMap<String, SearchResult> = HashMap::new();

        for topic in topics {
            let topic_results = self.search(topic, limit)?;

            for result in topic_results {
                results_by_id
                    .entry(result.id.clone())
                    .and_modify(|existing| {
                        if result.score > existing.score {
                            existing.score = result.score;
                        }

                        existing.match_ranges = merge_ranges(
                            mem::take(&mut existing.match_ranges),
                            result.match_ranges.clone(),
                        );
                        existing.title_match_ranges = merge_ranges(
                            mem::take(&mut existing.title_match_ranges),
                            result.title_match_ranges.clone(),
                        );
                        existing.path_match_ranges = merge_ranges(
                            mem::take(&mut existing.path_match_ranges),
                            result.path_match_ranges.clone(),
                        );

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

        let mut results: Vec<SearchResult> = results_by_id.into_values().collect();
        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(Ordering::Equal)
                .then_with(|| a.id.cmp(&b.id))
        });

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

    /// Executes a query with snippet and highlight generation.
    pub(crate) fn execute_query_with_highlights(
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

        let matched_terms = self.find_matched_terms(
            &searcher,
            query_terms,
            &[self.schema.body, self.schema.title, self.schema.path],
        );

        let highlight_query = self.build_highlight_query(&matched_terms);

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
    pub(crate) fn execute_query_no_highlights(
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
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn execute_query_with_details(
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

        let term_mappings = self.find_term_mappings(&searcher, query_terms);

        let mut matched_terms: HashSet<String> =
            term_mappings.values().flatten().cloned().collect();
        let extra_terms = self.find_matched_terms(
            &searcher,
            query_terms,
            &[self.schema.title, self.schema.path],
        );
        matched_terms.extend(extra_terms);

        let highlight_query = self.build_highlight_query(&matched_terms);

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

        let title = self.get_text_field(doc, self.schema.title);
        let body = self.get_text_field(doc, self.schema.body);
        let tags: Vec<String> = doc
            .get_all(self.schema.tags)
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect();
        let path = self.get_text_field(doc, self.schema.path);

        let mut field_matches: HashMap<String, FieldMatch> = HashMap::new();
        let mut field_scores: HashMap<String, f32> = HashMap::new();

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
                let boosted: Box<dyn Query> = Box::new(BoostQuery::new(query, boost::BODY));
                (Occur::Should, boosted)
            })
            .collect();

        Some(Box::new(BooleanQuery::new(clauses)))
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

    /// Converts a Tantivy document plus scoring context into a `SearchResult`.
    pub(crate) fn doc_to_result(
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

        let is_global = self.tree_is_global.get(&tree).copied().unwrap_or(false);
        let score = if is_global {
            base_score
        } else {
            base_score * self.local_boost
        };

        let snippet = snippet_generator.as_ref().map(|generator| {
            let snippet = generator.snippet_from_doc(doc);
            snippet.to_html()
        });

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

/// Convenience to aggregate siblings when only Phase 1 & 2 are needed.
pub fn aggregate_candidates(
    candidates: Vec<SearchCandidate>,
    aggregation_threshold: f32,
    lookup: impl Fn(&str) -> Option<ParentInfo>,
) -> Vec<AggregatedSearchResult> {
    aggregate(candidates, aggregation_threshold, lookup)
}

/// Converts raw matches into aggregated results when aggregation is disabled.
pub fn single_results_from_candidates(
    filtered: Vec<SearchCandidate>,
) -> Vec<AggregatedSearchResult> {
    filtered
        .into_iter()
        .map(AggregatedSearchResult::single)
        .collect()
}

/// Applies elbow cutoff to candidates.
pub fn apply_elbow(
    candidates: Vec<SearchCandidate>,
    cutoff_ratio: f32,
    max_results: usize,
) -> Vec<SearchCandidate> {
    elbow_cutoff(candidates, cutoff_ratio, max_results)
}
