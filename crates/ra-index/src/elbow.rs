//! Elbow detection for search result relevance cutoff.
//!
//! This module implements Phase 2 of the three-phase search algorithm: finding
//! the "elbow" point in search results where relevance drops significantly.
//!
//! The algorithm works by computing the ratio between adjacent scores. When the
//! ratio drops below a threshold (e.g., 0.5), we've found the elbow point where
//! results transition from highly relevant to marginally relevant.
//!
//! # Example
//!
//! Given scores `[8.0, 7.5, 7.0, 3.2, 3.0, 2.8, 0.9]`:
//! - Ratio 7.5/8.0 = 0.94 (above threshold)
//! - Ratio 7.0/7.5 = 0.93 (above threshold)
//! - Ratio 3.2/7.0 = 0.46 (below 0.5 threshold) ← elbow found
//!
//! The function returns the first 3 results (indices 0, 1, 2).

use crate::result::SearchCandidate;

/// Default cutoff ratio for elbow detection.
///
/// When the ratio between adjacent scores falls below this value,
/// we consider it the elbow point.
pub const DEFAULT_CUTOFF_RATIO: f32 = 0.5;

/// Default maximum number of results to return.
pub const DEFAULT_MAX_RESULTS: usize = 20;

/// Finds the elbow cutoff point in a list of search candidates.
///
/// The candidates must be sorted by score in descending order. The function
/// finds the first index where the ratio `score[i+1] / score[i]` falls below
/// the cutoff ratio, indicating a significant drop in relevance.
///
/// # Arguments
/// * `candidates` - Search candidates sorted by score (highest first)
/// * `cutoff_ratio` - Threshold ratio below which we cut off (0.0 to 1.0)
/// * `max_results` - Maximum number of results to return if no elbow is found
///
/// # Returns
/// A vector containing candidates up to (but not including) the elbow point,
/// or up to `max_results` if no elbow is found.
///
/// # Edge Cases
/// - Empty input returns empty output
/// - Single candidate returns that candidate
/// - Two candidates with significant drop returns just the first
/// - Zero or negative scores trigger immediate cutoff
/// - No elbow found returns up to `max_results`
pub fn elbow_cutoff(
    candidates: Vec<SearchCandidate>,
    cutoff_ratio: f32,
    max_results: usize,
) -> Vec<SearchCandidate> {
    // Handle edge cases
    if candidates.is_empty() {
        return Vec::new();
    }

    if candidates.len() == 1 {
        return candidates;
    }

    // Find the elbow point
    let mut cutoff_index = candidates.len();

    for i in 0..candidates.len() - 1 {
        let current_score = candidates[i].score;
        let next_score = candidates[i + 1].score;

        // Zero or negative scores trigger immediate cutoff
        if current_score <= 0.0 {
            cutoff_index = i;
            break;
        }

        if next_score <= 0.0 {
            cutoff_index = i + 1;
            break;
        }

        // Compute ratio and check against threshold
        let ratio = next_score / current_score;
        if ratio < cutoff_ratio {
            cutoff_index = i + 1;
            break;
        }
    }

    // Apply max_results limit
    let final_count = cutoff_index.min(max_results);

    candidates.into_iter().take(final_count).collect()
}

/// Finds the elbow cutoff point using default parameters.
///
/// Uses `DEFAULT_CUTOFF_RATIO` (0.5) and `DEFAULT_MAX_RESULTS` (20).
pub fn elbow_cutoff_default(candidates: Vec<SearchCandidate>) -> Vec<SearchCandidate> {
    elbow_cutoff(candidates, DEFAULT_CUTOFF_RATIO, DEFAULT_MAX_RESULTS)
}

#[cfg(test)]
mod test {
    use super::*;

    fn make_candidate(id: &str, score: f32) -> SearchCandidate {
        SearchCandidate {
            id: id.to_string(),
            doc_id: "local:test.md".to_string(),
            parent_id: Some("local:test.md".to_string()),
            title: format!("Title {id}"),
            tree: "local".to_string(),
            path: "test.md".to_string(),
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

    fn make_candidates(scores: &[f32]) -> Vec<SearchCandidate> {
        scores
            .iter()
            .enumerate()
            .map(|(i, &score)| make_candidate(&format!("doc{i}"), score))
            .collect()
    }

    #[test]
    fn spec_example_scores() {
        // Example from spec: [8.0, 7.5, 7.0, 3.2, 3.0, 2.8, 0.9]
        // Elbow at index 3 (ratio 3.2/7.0 = 0.46 < 0.5)
        let candidates = make_candidates(&[8.0, 7.5, 7.0, 3.2, 3.0, 2.8, 0.9]);
        let result = elbow_cutoff(candidates, 0.5, 20);

        assert_eq!(result.len(), 3);
        assert_eq!(result[0].score, 8.0);
        assert_eq!(result[1].score, 7.5);
        assert_eq!(result[2].score, 7.0);
    }

    #[test]
    fn empty_input() {
        let candidates: Vec<SearchCandidate> = vec![];
        let result = elbow_cutoff(candidates, 0.5, 20);

        assert!(result.is_empty());
    }

    #[test]
    fn single_candidate() {
        let candidates = make_candidates(&[5.0]);
        let result = elbow_cutoff(candidates, 0.5, 20);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].score, 5.0);
    }

    #[test]
    fn two_candidates_no_elbow() {
        // 4.5/5.0 = 0.9, above threshold
        let candidates = make_candidates(&[5.0, 4.5]);
        let result = elbow_cutoff(candidates, 0.5, 20);

        assert_eq!(result.len(), 2);
    }

    #[test]
    fn two_candidates_with_elbow() {
        // 2.0/5.0 = 0.4, below threshold
        let candidates = make_candidates(&[5.0, 2.0]);
        let result = elbow_cutoff(candidates, 0.5, 20);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].score, 5.0);
    }

    #[test]
    fn no_elbow_found_returns_max_results() {
        // All ratios above threshold: 0.95, 0.95, 0.95...
        let candidates = make_candidates(&[10.0, 9.5, 9.0, 8.5, 8.0, 7.5, 7.0]);
        let result = elbow_cutoff(candidates, 0.5, 3);

        // Should return max_results (3) since no elbow found
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn elbow_before_max_results() {
        // Elbow at index 2, max_results = 10
        let candidates = make_candidates(&[10.0, 9.0, 2.0, 1.5, 1.0]);
        let result = elbow_cutoff(candidates, 0.5, 10);

        // Should return 2 (up to elbow), not 10
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn max_results_before_elbow() {
        // Elbow would be at index 5, but max_results = 3
        let candidates = make_candidates(&[10.0, 9.0, 8.0, 7.0, 6.0, 1.0]);
        let result = elbow_cutoff(candidates, 0.5, 3);

        // Should return 3 (max_results), not 5
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn zero_score_triggers_cutoff() {
        let candidates = make_candidates(&[5.0, 4.0, 0.0, 3.0]);
        let result = elbow_cutoff(candidates, 0.5, 20);

        // Should stop at index 2 (the zero score)
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn negative_score_triggers_cutoff() {
        let candidates = make_candidates(&[5.0, 4.0, -1.0, 3.0]);
        let result = elbow_cutoff(candidates, 0.5, 20);

        // Should stop at index 2 (the negative score)
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn first_score_zero_returns_empty() {
        let candidates = make_candidates(&[0.0, 5.0, 4.0]);
        let result = elbow_cutoff(candidates, 0.5, 20);

        // First score is zero, cutoff at index 0
        assert!(result.is_empty());
    }

    #[test]
    fn gradual_decline_no_elbow() {
        // Gradual decline: each ratio is 0.9 (above 0.5 threshold)
        let scores: Vec<f32> = (0..10).map(|i| 10.0 * 0.9_f32.powi(i)).collect();
        let candidates = make_candidates(&scores);
        let result = elbow_cutoff(candidates, 0.5, 20);

        // No elbow found, return all 10
        assert_eq!(result.len(), 10);
    }

    #[test]
    fn steep_drop_immediate_elbow() {
        // Immediate steep drop
        let candidates = make_candidates(&[10.0, 1.0, 0.9, 0.8]);
        let result = elbow_cutoff(candidates, 0.5, 20);

        // Elbow at index 1 (ratio 0.1 < 0.5)
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn exact_threshold_not_elbow() {
        // Ratio exactly at threshold (0.5) should NOT trigger cutoff
        let candidates = make_candidates(&[10.0, 5.0, 2.5]);
        let result = elbow_cutoff(candidates, 0.5, 20);

        // 5.0/10.0 = 0.5 (not < 0.5, so no elbow)
        // 2.5/5.0 = 0.5 (not < 0.5, so no elbow)
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn just_below_threshold_is_elbow() {
        // Ratio just below threshold
        let candidates = make_candidates(&[10.0, 4.9, 2.0]);
        let result = elbow_cutoff(candidates, 0.5, 20);

        // 4.9/10.0 = 0.49 < 0.5, elbow at index 1
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn default_function_uses_defaults() {
        let candidates = make_candidates(&[8.0, 7.5, 7.0, 3.2, 3.0, 2.8, 0.9]);
        let result = elbow_cutoff_default(candidates);

        // Same as spec example with default ratio 0.5
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn custom_cutoff_ratio() {
        let candidates = make_candidates(&[10.0, 8.0, 6.0, 4.0]);

        // With ratio 0.7: 8/10=0.8 ok, 6/8=0.75 ok, 4/6=0.67 < 0.7 elbow
        let result = elbow_cutoff(candidates.clone(), 0.7, 20);
        assert_eq!(result.len(), 3);

        // With ratio 0.9: 8/10=0.8 < 0.9 immediate elbow
        let result = elbow_cutoff(candidates, 0.9, 20);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn preserves_candidate_data() {
        let mut candidate = make_candidate("test-doc", 5.0);
        candidate.title = "Specific Title".to_string();
        candidate.body = "Specific Body".to_string();
        candidate.snippet = Some("highlighted".to_string());
        candidate.match_ranges = vec![0..5, 10..15];

        let result = elbow_cutoff(vec![candidate], 0.5, 20);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].title, "Specific Title");
        assert_eq!(result[0].body, "Specific Body");
        assert_eq!(result[0].snippet, Some("highlighted".to_string()));
        assert_eq!(result[0].match_ranges, vec![0..5, 10..15]);
    }

    #[test]
    fn many_results_with_late_elbow() {
        // 15 results with elbow at index 12
        let mut scores: Vec<f32> = (0..12).map(|i| 20.0 - i as f32 * 0.5).collect();
        scores.extend([5.0, 4.0, 3.0]); // Steep drop from ~14.5 to 5.0

        let candidates = make_candidates(&scores);
        let result = elbow_cutoff(candidates, 0.5, 20);

        // Elbow at index 12 (ratio 5.0/14.5 ≈ 0.34 < 0.5)
        assert_eq!(result.len(), 12);
    }
}
