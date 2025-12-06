//! Per-tree score normalization for multi-tree searches.
//!
//! When searching across multiple trees with different content densities, raw BM25 scores
//! are not directly comparable. A specialized tree with focused content (e.g., character
//! backgrounds for a novel) will score much higher on domain-specific terms than a general
//! tree (e.g., world-building notes), even when both contain relevant results.
//!
//! This module implements **top-score normalization**: each result's score is divided by
//! the maximum score within its tree, so the best result in each tree gets a score of 1.0.
//! This preserves relative ordering within trees while making cross-tree comparison fair.
//!
//! # Example
//!
//! Before normalization (specialized tree dominates):
//! ```text
//! tree-a:doc1.md  score=4500  (top in tree-a)
//! tree-a:doc2.md  score=3200
//! tree-a:doc3.md  score=3000
//! tree-b:doc1.md  score=800   (top in tree-b) <- elbow cuts here (800/3000 = 0.27)
//! tree-b:doc2.md  score=600
//! ```
//!
//! After normalization (fair cross-tree comparison):
//! ```text
//! tree-a:doc1.md  score=1.00  (4500/4500)
//! tree-b:doc1.md  score=1.00  (800/800)
//! tree-a:doc2.md  score=0.71  (3200/4500)
//! tree-b:doc2.md  score=0.75  (600/800)
//! tree-a:doc3.md  score=0.67  (3000/4500)
//! ```
//!
//! Now elbow cutoff sees gradual decline across both trees, not a cliff between them.
//!
//! # When Normalization Applies
//!
//! Normalization is only applied when:
//! - Multiple trees are explicitly specified in the search parameters
//! - At least two trees have results
//!
//! Single-tree searches are unchanged to maintain backwards compatibility and avoid
//! unnecessary computation.

use std::{cmp::Ordering, collections::HashMap};

use super::SearchCandidate;

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
pub fn normalize_scores_across_trees(
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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_candidate(tree: &str, id: &str, score: f32) -> SearchCandidate {
        SearchCandidate {
            id: id.to_string(),
            doc_id: format!("{tree}:{id}"),
            parent_id: None,
            title: format!("Title {id}"),
            tree: tree.to_string(),
            path: format!("{id}.md"),
            body: "Body content".to_string(),
            breadcrumb: "> Test".to_string(),
            depth: 1,
            position: 0,
            byte_start: 0,
            byte_end: 100,
            sibling_count: 1,
            score,
            snippet: None,
            match_ranges: vec![],
            title_match_ranges: vec![],
            path_match_ranges: vec![],
            match_details: None,
        }
    }

    #[test]
    fn single_tree_unchanged() {
        let candidates = vec![
            make_candidate("tree-a", "doc1", 100.0),
            make_candidate("tree-a", "doc2", 50.0),
        ];

        let result = normalize_scores_across_trees(candidates, 1);

        assert_eq!(result.len(), 2);
        assert!((result[0].score - 100.0).abs() < f32::EPSILON);
        assert!((result[1].score - 50.0).abs() < f32::EPSILON);
    }

    #[test]
    fn multi_tree_normalizes() {
        let candidates = vec![
            make_candidate("tree-a", "doc1", 4500.0),
            make_candidate("tree-a", "doc2", 3000.0),
            make_candidate("tree-b", "doc1", 800.0),
            make_candidate("tree-b", "doc2", 600.0),
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
            make_candidate("tree-a", "doc1", 4500.0), // -> 1.0
            make_candidate("tree-a", "doc2", 3000.0), // -> 0.667
            make_candidate("tree-b", "doc1", 800.0),  // -> 1.0
            make_candidate("tree-b", "doc2", 600.0),  // -> 0.75
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
            make_candidate("tree-a", "doc1", 100.0),
            make_candidate("tree-a", "doc2", 50.0),
        ];

        let result = normalize_scores_across_trees(candidates, 2);

        assert!((result[0].score - 100.0).abs() < f32::EPSILON);
        assert!((result[1].score - 50.0).abs() < f32::EPSILON);
    }

    #[test]
    fn handles_zero_scores() {
        let candidates = vec![
            make_candidate("tree-a", "doc1", 100.0),
            make_candidate("tree-a", "doc2", 0.0),
            make_candidate("tree-b", "doc1", 50.0),
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
    fn empty_candidates() {
        let candidates: Vec<SearchCandidate> = vec![];
        let result = normalize_scores_across_trees(candidates, 2);
        assert!(result.is_empty());
    }

    #[test]
    fn three_trees() {
        let candidates = vec![
            make_candidate("tree-a", "doc1", 1000.0),
            make_candidate("tree-b", "doc1", 500.0),
            make_candidate("tree-c", "doc1", 100.0),
        ];

        let result = normalize_scores_across_trees(candidates, 3);

        // All should be normalized to 1.0 (each is the max in its tree)
        for candidate in &result {
            assert!((candidate.score - 1.0).abs() < 0.001);
        }
    }
}
