//! Unified search result pipeline.
//!
//! This module provides the shared pipeline that processes raw search candidates
//! into final aggregated results. All search entry points (search, context, likethis)
//! use this pipeline to ensure consistent behavior.
//!
//! # Pipeline Phases
//!
//! 1. **Score Normalization**: For multi-tree searches, normalize scores so each
//!    tree's best result gets 1.0. See [`normalize_scores_across_trees`].
//!
//! 2. **Adaptive Hierarchical Aggregation**: Process candidates one-by-one in score order,
//!    aggregating siblings when appropriate. When a parent enters results, all its
//!    descendants are skipped. See [`adaptive_aggregate`].
//!
//! 3. **Elbow Cutoff**: Detect where relevance drops significantly and truncate.
//!    Applied on aggregated results, not raw candidates.
//!
//! 4. **Final Limit**: Truncate to the requested number of results.

use super::{
    SearchCandidate, SearchParams, adaptive::adaptive_aggregate,
    normalize::normalize_scores_across_trees,
};
use crate::{elbow::elbow_cutoff_results, result::SearchResult as AggregatedSearchResult};

/// Processes raw search candidates through the result pipeline.
///
/// This is the unified pipeline used by all search entry points. It takes
/// raw candidates from query execution and produces final aggregated results.
///
/// # Arguments
///
/// * `candidates` - Raw search candidates from query execution (sorted by score)
/// * `params` - Search parameters controlling pipeline behavior
/// * `parent_lookup` - Function to look up parent nodes by ID for aggregation
///
/// # Returns
///
/// Final search results after normalization, aggregation, and elbow cutoff.
pub fn process_candidates<F>(
    candidates: Vec<SearchCandidate>,
    params: &SearchParams,
    parent_lookup: F,
) -> Vec<AggregatedSearchResult>
where
    F: Fn(&str) -> Option<SearchCandidate>,
{
    // Phase 1: Normalize scores across trees (only for multi-tree searches)
    let normalized = normalize_scores_across_trees(candidates, params.trees.len());

    // Phase 2: Adaptive aggregation - process candidates incrementally, stop at limit
    // Uses a larger internal limit since aggregation may reduce result count
    let aggregation_limit = params.limit * 2;
    let results = if params.disable_aggregation {
        single_results_from_candidates(normalized, aggregation_limit)
    } else {
        adaptive_aggregate(
            normalized,
            params.aggregation_threshold,
            aggregation_limit,
            parent_lookup,
        )
    };

    // Phase 3: Apply elbow cutoff on aggregated results
    let filtered = elbow_cutoff_results(results, params.cutoff_ratio, params.max_candidates);

    // Phase 4: Apply final limit
    filtered.into_iter().take(params.limit).collect()
}

/// Converts raw candidates into single (non-aggregated) results.
///
/// Used when aggregation is disabled via `SearchParams::disable_aggregation`.
fn single_results_from_candidates(
    candidates: Vec<SearchCandidate>,
    limit: usize,
) -> Vec<AggregatedSearchResult> {
    candidates
        .into_iter()
        .take(limit)
        .map(AggregatedSearchResult::single)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_candidate(
        id: &str,
        tree: &str,
        parent_id: Option<&str>,
        score: f32,
        sibling_count: u64,
    ) -> SearchCandidate {
        let hierarchy = if id.contains('#') {
            vec!["Doc".to_string(), format!("Section {id}")]
        } else {
            vec!["Doc".to_string()]
        };

        SearchCandidate {
            id: id.to_string(),
            doc_id: if id.contains('#') {
                id.split('#').next().unwrap().to_string()
            } else {
                id.to_string()
            },
            parent_id: parent_id.map(String::from),
            hierarchy,
            tree: tree.to_string(),
            path: "test.md".to_string(),
            body: format!("Body of {id}"),
            position: 0,
            byte_start: 0,
            byte_end: 100,
            sibling_count,
            score,
            snippet: None,
            match_ranges: vec![],
            hierarchy_match_ranges: vec![],
            path_match_ranges: vec![],
            match_details: None,
        }
    }

    #[test]
    fn empty_candidates_returns_empty() {
        let params = SearchParams::default();
        let results = process_candidates(vec![], &params, |_| None);
        assert!(results.is_empty());
    }

    #[test]
    fn single_candidate_passes_through() {
        let params = SearchParams::default();
        let candidates = vec![make_candidate("local:test.md", "local", None, 5.0, 1)];

        let results = process_candidates(candidates, &params, |_| None);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].candidate().id, "local:test.md");
        assert_eq!(results[0].candidate().score, 5.0);
    }

    #[test]
    fn respects_limit() {
        let params = SearchParams {
            limit: 2,
            ..Default::default()
        };

        let candidates = vec![
            make_candidate("local:a.md", "local", None, 5.0, 1),
            make_candidate("local:b.md", "local", None, 4.0, 1),
            make_candidate("local:c.md", "local", None, 3.0, 1),
        ];

        let results = process_candidates(candidates, &params, |_| None);

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].candidate().id, "local:a.md");
        assert_eq!(results[1].candidate().id, "local:b.md");
    }

    #[test]
    fn aggregation_disabled_returns_singles() {
        let params = SearchParams {
            disable_aggregation: true,
            ..Default::default()
        };

        // Two siblings that would normally aggregate
        let candidates = vec![
            make_candidate("local:test.md#s1", "local", Some("local:test.md"), 5.0, 2),
            make_candidate("local:test.md#s2", "local", Some("local:test.md"), 4.0, 2),
        ];

        let parent = make_candidate("local:test.md", "local", None, 0.0, 1);

        let results = process_candidates(candidates, &params, |id| {
            if id == "local:test.md" {
                Some(parent.clone())
            } else {
                None
            }
        });

        // Should have 2 separate results, not aggregated
        assert_eq!(results.len(), 2);
        assert!(!results[0].is_aggregated());
        assert!(!results[1].is_aggregated());
    }

    #[test]
    fn aggregation_enabled_merges_siblings() {
        let params = SearchParams {
            aggregation_threshold: 0.5,
            ..Default::default()
        };

        // Two siblings that should aggregate (2/2 = 100% >= 50%)
        let candidates = vec![
            make_candidate("local:test.md#s1", "local", Some("local:test.md"), 5.0, 2),
            make_candidate("local:test.md#s2", "local", Some("local:test.md"), 4.0, 2),
        ];

        let parent = make_candidate("local:test.md", "local", None, 0.0, 1);

        let results = process_candidates(candidates, &params, |id| {
            if id == "local:test.md" {
                Some(parent.clone())
            } else {
                None
            }
        });

        // Should have 1 aggregated result
        assert_eq!(results.len(), 1);
        assert!(results[0].is_aggregated());
        assert_eq!(results[0].candidate().id, "local:test.md");
    }

    #[test]
    fn elbow_cutoff_applied() {
        let params = SearchParams {
            cutoff_ratio: 0.5,
            max_candidates: 10,
            ..Default::default()
        };

        // Scores with a steep drop: 10.0, 9.0, 2.0 (ratio 2.0/9.0 = 0.22 < 0.5)
        let candidates = vec![
            make_candidate("local:a.md", "local", None, 10.0, 1),
            make_candidate("local:b.md", "local", None, 9.0, 1),
            make_candidate("local:c.md", "local", None, 2.0, 1),
        ];

        let results = process_candidates(candidates, &params, |_| None);

        // Should cut off at the elbow, returning only 2 results
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].candidate().id, "local:a.md");
        assert_eq!(results[1].candidate().id, "local:b.md");
    }

    #[test]
    fn multi_tree_normalization() {
        let params = SearchParams {
            trees: vec!["tree-a".to_string(), "tree-b".to_string()],
            cutoff_ratio: 0.0, // Disable elbow to test normalization only
            max_candidates: 10,
            ..Default::default()
        };

        // tree-a has much higher raw scores than tree-b
        let candidates = vec![
            make_candidate("tree-a:doc1.md", "tree-a", None, 1000.0, 1),
            make_candidate("tree-a:doc2.md", "tree-a", None, 500.0, 1),
            make_candidate("tree-b:doc1.md", "tree-b", None, 100.0, 1),
            make_candidate("tree-b:doc2.md", "tree-b", None, 50.0, 1),
        ];

        let results = process_candidates(candidates, &params, |_| None);

        // After normalization, tree-a:doc1 and tree-b:doc1 both have score 1.0
        // So they should both appear at the top
        assert_eq!(results.len(), 4);

        // Both top docs should have normalized score of 1.0
        let scores: Vec<f32> = results.iter().map(|r| r.candidate().score).collect();
        assert!((scores[0] - 1.0).abs() < 0.001);
        assert!((scores[1] - 1.0).abs() < 0.001);
    }
}
