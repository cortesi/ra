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
//! 2. **Elbow Cutoff**: Detect where relevance drops significantly and filter
//!    candidates. This determines the "relevant" set before aggregation.
//!
//! 3. **Adaptive Hierarchical Aggregation**: Process all relevant candidates,
//!    aggregating siblings when appropriate. See [`adaptive_aggregate`].
//!
//! 4. **Final Limit**: Truncate to the requested number of results.
//!
//! # Score Normalization
//!
//! When searching across multiple trees with different content densities, raw BM25 scores
//! are not directly comparable. A specialized tree with focused content will score much
//! higher on domain-specific terms than a general tree, even when both contain relevant
//! results.
//!
//! This module implements **top-score normalization**: each result's score is divided by
//! the maximum score within its tree, so the best result in each tree gets a score of 1.0.
//! This preserves relative ordering within trees while making cross-tree comparison fair.

use std::{cmp::Ordering, collections::HashMap};

use super::{SearchCandidate, SearchParams, aggregation::adaptive_aggregate};
use crate::{elbow::elbow_cutoff, result::SearchResult as AggregatedSearchResult};

/// Normalizes scores across multiple trees using top-score normalization.
///
/// Each result's score is divided by the maximum score in its tree, so the best
/// result in each tree gets a score of 1.0. Results are then re-sorted by normalized
/// score in descending order.
///
/// # Arguments
///
/// * `candidates` - Search candidates to normalize (will be modified in place)
/// * `tree_count` - Number of trees being searched (normalization skipped if <= 1)
///
/// # Returns
///
/// The same candidates with normalized scores, sorted by score descending.
///
/// # Behavior
///
/// - If `tree_count <= 1`, returns candidates unchanged (no normalization needed)
/// - If only one tree has results, returns candidates unchanged
/// - Trees with no results are ignored
/// - Zero or negative max scores are treated as 1.0 to avoid division issues
fn normalize_scores_across_trees(
    mut candidates: Vec<SearchCandidate>,
    tree_count: usize,
) -> Vec<SearchCandidate> {
    // Skip normalization for single-tree searches
    if tree_count <= 1 {
        return candidates;
    }

    // Find max score per tree (using owned Strings to avoid borrow issues)
    let mut max_scores: HashMap<String, f32> = HashMap::new();
    for candidate in &candidates {
        let entry = max_scores.entry(candidate.tree.clone()).or_insert(0.0);
        if candidate.score > *entry {
            *entry = candidate.score;
        }
    }

    // Skip if only one tree has results (nothing to normalize against)
    if max_scores.len() <= 1 {
        return candidates;
    }

    // Normalize each candidate's score by its tree's max
    for candidate in &mut candidates {
        let max_score = max_scores.get(&candidate.tree).copied().unwrap_or(1.0);

        // Avoid division by zero or negative scores
        let divisor = if max_score > 0.0 { max_score } else { 1.0 };
        candidate.score /= divisor;
    }

    // Re-sort by normalized score (descending)
    candidates.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(Ordering::Equal));

    candidates
}

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
/// Final search results after normalization, elbow cutoff, and aggregation.
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

    // Phase 2: Elbow cutoff on raw candidates - determines the "relevant" set
    let relevant = elbow_cutoff(normalized, params.cutoff_ratio, params.max_candidates);

    // Phase 3: Aggregate all relevant candidates
    let results = if params.disable_aggregation {
        single_results_from_candidates(relevant)
    } else {
        adaptive_aggregate(relevant, params.aggregation_threshold, parent_lookup)
    };

    // Phase 4: Apply final limit
    results.into_iter().take(params.limit).collect()
}

/// Converts raw candidates into single (non-aggregated) results.
///
/// Used when aggregation is disabled via `SearchParams::disable_aggregation`.
fn single_results_from_candidates(candidates: Vec<SearchCandidate>) -> Vec<AggregatedSearchResult> {
    candidates
        .into_iter()
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
        let depth = if id.contains('#') { 1 } else { 0 };

        SearchCandidate {
            id: id.to_string(),
            doc_id: if id.contains('#') {
                id.split('#').next().unwrap().to_string()
            } else {
                id.to_string()
            },
            parent_id: parent_id.map(String::from),
            hierarchy,
            depth,
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

    // Normalization tests (merged from normalize.rs)

    #[test]
    fn single_tree_unchanged() {
        let candidates = vec![
            make_candidate("doc1", "tree-a", None, 100.0, 1),
            make_candidate("doc2", "tree-a", None, 50.0, 1),
        ];

        let result = normalize_scores_across_trees(candidates, 1);

        assert_eq!(result.len(), 2);
        assert!((result[0].score - 100.0).abs() < f32::EPSILON);
        assert!((result[1].score - 50.0).abs() < f32::EPSILON);
    }

    #[test]
    fn multi_tree_normalizes() {
        let candidates = vec![
            make_candidate("doc1", "tree-a", None, 4500.0, 1),
            make_candidate("doc2", "tree-a", None, 3000.0, 1),
            make_candidate("doc1", "tree-b", None, 800.0, 1),
            make_candidate("doc2", "tree-b", None, 600.0, 1),
        ];

        let result = normalize_scores_across_trees(candidates, 2);

        assert_eq!(result.len(), 4);

        // After normalization, both tree tops should be 1.0
        // tree-a:doc1 = 4500/4500 = 1.0
        // tree-b:doc1 = 800/800 = 1.0
        // tree-a:doc2 = 3000/4500 = 0.667
        // tree-b:doc2 = 600/800 = 0.75

        // Find the normalized scores
        let tree_a_doc1 = result
            .iter()
            .find(|c| c.id == "doc1" && c.tree == "tree-a")
            .unwrap();
        let tree_a_doc2 = result
            .iter()
            .find(|c| c.id == "doc2" && c.tree == "tree-a")
            .unwrap();
        let tree_b_doc1 = result
            .iter()
            .find(|c| c.id == "doc1" && c.tree == "tree-b")
            .unwrap();
        let tree_b_doc2 = result
            .iter()
            .find(|c| c.id == "doc2" && c.tree == "tree-b")
            .unwrap();

        assert!((tree_a_doc1.score - 1.0).abs() < 0.001);
        assert!((tree_b_doc1.score - 1.0).abs() < 0.001);
        assert!((tree_a_doc2.score - 0.667).abs() < 0.01);
        assert!((tree_b_doc2.score - 0.75).abs() < 0.001);
    }

    #[test]
    fn results_sorted_by_normalized_score() {
        let candidates = vec![
            make_candidate("doc1", "tree-a", None, 4500.0, 1), // -> 1.0
            make_candidate("doc2", "tree-a", None, 3000.0, 1), // -> 0.667
            make_candidate("doc1", "tree-b", None, 800.0, 1),  // -> 1.0
            make_candidate("doc2", "tree-b", None, 600.0, 1),  // -> 0.75
        ];

        let result = normalize_scores_across_trees(candidates, 2);

        // Should be sorted: 1.0, 1.0, 0.75, 0.667
        assert!((result[0].score - 1.0).abs() < 0.001);
        assert!((result[1].score - 1.0).abs() < 0.001);
        assert!((result[2].score - 0.75).abs() < 0.001);
        assert!((result[3].score - 0.667).abs() < 0.01);
    }

    #[test]
    fn only_one_tree_has_results() {
        // Even with tree_count=2, if only one tree has results, no normalization
        let candidates = vec![
            make_candidate("doc1", "tree-a", None, 100.0, 1),
            make_candidate("doc2", "tree-a", None, 50.0, 1),
        ];

        let result = normalize_scores_across_trees(candidates, 2);

        assert!((result[0].score - 100.0).abs() < f32::EPSILON);
        assert!((result[1].score - 50.0).abs() < f32::EPSILON);
    }

    #[test]
    fn handles_zero_scores() {
        let candidates = vec![
            make_candidate("doc1", "tree-a", None, 100.0, 1),
            make_candidate("doc2", "tree-a", None, 0.0, 1),
            make_candidate("doc1", "tree-b", None, 50.0, 1),
        ];

        let result = normalize_scores_across_trees(candidates, 2);

        // tree-a max is 100, tree-b max is 50
        let tree_a_doc1 = result
            .iter()
            .find(|c| c.id == "doc1" && c.tree == "tree-a")
            .unwrap();
        let tree_a_doc2 = result
            .iter()
            .find(|c| c.id == "doc2" && c.tree == "tree-a")
            .unwrap();
        let tree_b_doc1 = result
            .iter()
            .find(|c| c.id == "doc1" && c.tree == "tree-b")
            .unwrap();

        assert!((tree_a_doc1.score - 1.0).abs() < 0.001);
        assert!((tree_a_doc2.score - 0.0).abs() < 0.001);
        assert!((tree_b_doc1.score - 1.0).abs() < 0.001);
    }

    #[test]
    fn empty_candidates_normalize() {
        let candidates: Vec<SearchCandidate> = vec![];
        let result = normalize_scores_across_trees(candidates, 2);
        assert!(result.is_empty());
    }

    #[test]
    fn three_trees() {
        let candidates = vec![
            make_candidate("doc1", "tree-a", None, 1000.0, 1),
            make_candidate("doc1", "tree-b", None, 500.0, 1),
            make_candidate("doc1", "tree-c", None, 100.0, 1),
        ];

        let result = normalize_scores_across_trees(candidates, 3);

        // All should be normalized to 1.0 (each is the max in its tree)
        for candidate in &result {
            assert!((candidate.score - 1.0).abs() < 0.001);
        }
    }
}
