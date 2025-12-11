//! Query execution paths and result conversion.

use std::{collections::HashSet, ops::Range};

use tantivy::tokenizer::TextAnalyzer;

/// Options for executing a query.
#[derive(Debug, Clone, Default)]
pub struct ExecutionOptions<'a> {
    /// Whether to generate snippets and match ranges (expensive).
    pub with_snippets: bool,
    /// Whether to collect full match details (very expensive).
    pub with_details: bool,
    /// Original query string for details (if with_details is true).
    pub original_query: Option<&'a str>,
    /// Whether to include score explanation (if with_details is true).
    pub include_explanation: bool,
}

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

// NOTE: Searcher execution methods live in `search/mod.rs`. This module provides
// shared utilities and types used by that implementation.

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
