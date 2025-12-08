//! Adaptive hierarchical aggregation for search results.
//!
//! This module implements an aggregation algorithm that processes all candidates
//! in score order, building aggregated results. Candidates are processed after
//! elbow cutoff has determined the "relevant" set.
//!
//! # Key Features
//!
//! 1. **Processes all relevant candidates**: No early termination during aggregation
//! 2. **Claims descendants**: When a parent enters results, all descendants are skipped
//! 3. **Cascades upward**: Aggregating siblings may trigger further aggregation with grandparents
//! 4. **Ancestor subsumption**: When an ancestor arrives after its descendants, it subsumes them
//!
//! # Algorithm
//!
//! ```text
//! for candidate in candidates (sorted by score descending):
//!     if candidate is claimed (ancestor in results):
//!         skip
//!
//!     if candidate has descendants in results:
//!         subsume descendants into candidate
//!
//!     siblings = find siblings already in results
//!     if should_aggregate(candidate, siblings):
//!         aggregate into parent, cascade if needed
//!     else:
//!         add as single result
//! ```
//!
//! # Chunk ID Format
//!
//! Chunk IDs follow the format `{tree}:{path}#{slug}` where:
//! - `{tree}:{path}` is the document ID (same for all chunks in a file)
//! - `#{slug}` identifies the specific section within the document
//! - Nested slugs use `-` separators (e.g., `#error-handling-retry-logic`)
//!
//! A chunk is an ancestor of another if:
//! - They share the same document ID (`{tree}:{path}`)
//! - The ancestor's slug is a prefix of the descendant's slug
//!
//! The document node (ID without `#`) is an ancestor of all chunks in that document.

use std::collections::{HashMap, HashSet};

use crate::{SearchCandidate, result::SearchResult};

/// Default aggregation threshold.
///
/// When the ratio of matching siblings to total siblings meets or exceeds
/// this threshold, the matches are aggregated into their parent.
pub const DEFAULT_AGGREGATION_THRESHOLD: f32 = 0.5;

/// Adaptive aggregator that builds results from candidates.
///
/// Processes all candidates, aggregating siblings when appropriate.
/// The input candidates should already be filtered by elbow cutoff.
pub struct AdaptiveAggregator {
    /// Accumulated search results.
    results: Vec<SearchResult>,
    /// IDs of chunks that have been claimed (their ancestor is in results).
    claimed: HashSet<String>,
    /// Map from result ID to index in results vec (for efficient lookup).
    result_index: HashMap<String, usize>,
    /// Aggregation threshold (fraction of siblings needed to aggregate).
    threshold: f32,
}

impl AdaptiveAggregator {
    /// Creates a new adaptive aggregator.
    ///
    /// # Arguments
    /// * `threshold` - Minimum ratio of matching/total siblings to trigger aggregation (0.0 to 1.0)
    pub fn new(threshold: f32) -> Self {
        Self {
            results: Vec::new(),
            claimed: HashSet::new(),
            result_index: HashMap::new(),
            threshold,
        }
    }

    /// Checks if a candidate is claimed (should be skipped).
    ///
    /// A candidate is claimed if:
    /// - Its ID is in the claimed set, OR
    /// - Any of its ancestors is already in the results
    pub fn is_claimed(&self, candidate: &SearchCandidate) -> bool {
        // Direct claim check
        if self.claimed.contains(&candidate.id) {
            return true;
        }

        // Check if any ancestor is in results
        for idx in self.result_index.values() {
            let result_candidate = self.results[*idx].candidate();
            if result_candidate.is_ancestor_of(candidate) {
                return true;
            }
        }

        false
    }

    /// Finds indices of results that are descendants of the candidate.
    ///
    /// A result is a descendant if the candidate is an ancestor of the result's ID.
    pub fn find_descendant_indices(&self, candidate: &SearchCandidate) -> Vec<usize> {
        self.results
            .iter()
            .enumerate()
            .filter_map(|(idx, result)| {
                if candidate.is_ancestor_of(result.candidate()) {
                    Some(idx)
                } else {
                    None
                }
            })
            .collect()
    }

    /// Finds indices of results that are siblings of the candidate.
    ///
    /// Siblings share the same parent_id.
    pub fn find_sibling_indices(&self, candidate: &SearchCandidate) -> Vec<usize> {
        let Some(ref parent_id) = candidate.parent_id else {
            return Vec::new();
        };

        self.results
            .iter()
            .enumerate()
            .filter_map(|(idx, result)| {
                let result_parent = result.candidate().parent_id.as_ref()?;
                if result_parent == parent_id {
                    Some(idx)
                } else {
                    None
                }
            })
            .collect()
    }

    /// Checks if the candidate should aggregate with existing sibling results.
    ///
    /// Returns true if there's at least one sibling already in results AND
    /// `(sibling_count_in_results + 1) / total_siblings >= threshold`.
    pub fn should_aggregate(&self, candidate: &SearchCandidate, sibling_indices: &[usize]) -> bool {
        // Need at least one sibling already in results to aggregate
        if sibling_indices.is_empty() {
            return false;
        }

        if candidate.parent_id.is_none() {
            // Document-level nodes can't aggregate further
            return false;
        }

        let total_siblings = candidate.sibling_count;
        if total_siblings == 0 {
            return false;
        }

        // Count includes the candidate itself plus siblings in results
        let matching_count = sibling_indices.len() as u64 + 1;
        let ratio = matching_count as f32 / total_siblings as f32;

        ratio >= self.threshold
    }

    /// Returns the current number of results.
    #[cfg(test)]
    pub fn result_count(&self) -> usize {
        self.results.len()
    }

    /// Adds a candidate as a single (non-aggregated) result.
    pub fn add_single(&mut self, candidate: SearchCandidate) {
        let id = candidate.id.clone();
        let idx = self.results.len();
        self.results.push(SearchResult::single(candidate));
        self.result_index.insert(id, idx);
    }

    /// Removes results at the given indices and returns their candidates.
    ///
    /// Indices must be sorted in ascending order.
    fn remove_results(&mut self, indices: &[usize]) -> Vec<SearchCandidate> {
        let mut removed = Vec::with_capacity(indices.len());

        // Remove in reverse order to maintain index validity
        for &idx in indices.iter().rev() {
            let result = self.results.remove(idx);
            let id = result.candidate().id.clone();
            self.result_index.remove(&id);

            // Extract candidate(s) from the result
            match result {
                SearchResult::Single(c) => removed.push(c),
                SearchResult::Aggregated { constituents, .. } => {
                    removed.extend(constituents);
                }
            }
        }

        // Rebuild result_index since indices shifted
        self.result_index.clear();
        for (idx, result) in self.results.iter().enumerate() {
            self.result_index.insert(result.candidate().id.clone(), idx);
        }

        removed
    }

    /// Adds an aggregated result and checks for cascade opportunities.
    ///
    /// Returns true if the result was added (may be further aggregated via cascade).
    pub fn add_aggregated<F>(
        &mut self,
        parent: SearchCandidate,
        constituents: Vec<SearchCandidate>,
        parent_lookup: &F,
    ) -> bool
    where
        F: Fn(&str) -> Option<SearchCandidate>,
    {
        let parent_id = parent.id.clone();
        let idx = self.results.len();
        self.results
            .push(SearchResult::aggregated(parent, constituents));
        self.result_index.insert(parent_id.clone(), idx);

        // Check for cascade: does the new parent have siblings that should aggregate?
        self.check_cascade(&parent_id, parent_lookup)
    }

    /// Checks if the newly added result should cascade (aggregate with its siblings).
    fn check_cascade<F>(&mut self, parent_id: &str, parent_lookup: &F) -> bool
    where
        F: Fn(&str) -> Option<SearchCandidate>,
    {
        // Get the parent result's parent_id (grandparent)
        let Some(idx) = self.result_index.get(parent_id).copied() else {
            return false;
        };

        let grandparent_id = self.results[idx].candidate().parent_id.clone();
        let Some(ref grandparent_id) = grandparent_id else {
            // No grandparent - we're at document level, can't cascade further
            return false;
        };

        // Find siblings of the parent (other results with same grandparent)
        let sibling_indices: Vec<usize> = self
            .results
            .iter()
            .enumerate()
            .filter_map(|(i, result)| {
                if result.candidate().parent_id.as_ref() == Some(grandparent_id) && i != idx {
                    Some(i)
                } else {
                    None
                }
            })
            .collect();

        if sibling_indices.is_empty() {
            return false;
        }

        // Check if we should aggregate with siblings
        let total_siblings = self.results[idx].candidate().sibling_count;
        let matching_count = sibling_indices.len() as u64 + 1; // +1 for the parent itself
        let ratio = matching_count as f32 / total_siblings as f32;

        if ratio < self.threshold {
            return false;
        }

        // Look up grandparent
        let Some(grandparent) = parent_lookup(grandparent_id) else {
            return false;
        };

        // Collect all indices to remove (parent + siblings)
        let mut all_indices: Vec<usize> = sibling_indices;
        all_indices.push(idx);
        all_indices.sort_unstable();

        // Remove and collect constituents
        let constituents = self.remove_results(&all_indices);

        // Add grandparent as aggregated result and continue cascading
        self.add_aggregated(grandparent, constituents, parent_lookup)
    }

    /// Consumes the aggregator and returns the accumulated results sorted by score.
    pub fn into_results(mut self) -> Vec<SearchResult> {
        // Sort by score descending - results may be out of order after aggregation/cascading
        self.results.sort_by(|a, b| {
            b.candidate()
                .score
                .partial_cmp(&a.candidate().score)
                .unwrap()
        });
        self.results
    }

    /// Returns a reference to the current results.
    #[cfg(test)]
    pub fn results(&self) -> &[SearchResult] {
        &self.results
    }

    /// Processes candidates through the adaptive aggregation algorithm.
    ///
    /// Iterates through all candidates in order (should be sorted by score descending),
    /// building aggregated results. For each candidate:
    /// - Skip if claimed (ancestor already in results)
    /// - Subsume any descendants already in results
    /// - Check if it should aggregate with existing siblings
    /// - Either add as single result or aggregate with siblings
    ///
    /// # Arguments
    /// * `candidates` - Candidates to process (should be sorted by score descending)
    /// * `parent_lookup` - Function to look up parent nodes by ID
    pub fn process<F>(&mut self, candidates: Vec<SearchCandidate>, parent_lookup: &F)
    where
        F: Fn(&str) -> Option<SearchCandidate>,
    {
        for candidate in candidates {
            // Skip if this candidate is already in results (e.g., added via cascade)
            if self.result_index.contains_key(&candidate.id) {
                continue;
            }

            if self.is_claimed(&candidate) {
                continue;
            }

            // Check if this candidate has descendants already in results.
            // If so, this ancestor subsumes them - remove descendants and add ancestor.
            let descendant_indices = self.find_descendant_indices(&candidate);
            if !descendant_indices.is_empty() {
                // Remove all descendant results - ancestor subsumes them
                let constituents = self.remove_results(&descendant_indices);
                // Add ancestor as aggregated result with descendants as constituents
                self.add_aggregated(candidate, constituents, parent_lookup);
                continue;
            }

            let sibling_indices = self.find_sibling_indices(&candidate);

            if self.should_aggregate(&candidate, &sibling_indices) {
                // Look up parent to aggregate into
                let Some(ref parent_id) = candidate.parent_id else {
                    // No parent, add as single (shouldn't happen due to should_aggregate check)
                    self.add_single(candidate);
                    continue;
                };

                let Some(parent) = parent_lookup(parent_id) else {
                    // Parent not found, add as single
                    self.add_single(candidate);
                    continue;
                };

                // Remove siblings from results and collect their candidates
                let mut constituents = self.remove_results(&sibling_indices);
                constituents.push(candidate);

                // Add parent as aggregated result (may cascade)
                self.add_aggregated(parent, constituents, parent_lookup);
            } else {
                self.add_single(candidate);
            }
        }
    }
}

/// Performs adaptive hierarchical aggregation on search candidates.
///
/// This is the main entry point for the adaptive aggregation algorithm. It processes
/// all candidates in score order, aggregating siblings when appropriate.
///
/// The candidates should already be filtered by elbow cutoff to include only
/// relevant results. This function aggregates everything that passes through.
///
/// # Arguments
/// * `candidates` - Search candidates (should be sorted by score descending, already filtered)
/// * `threshold` - Minimum ratio of matching/total siblings to trigger aggregation (0.0 to 1.0)
/// * `parent_lookup` - Function to look up parent nodes by ID
///
/// # Returns
/// Aggregated search results (sorted by score descending).
pub fn adaptive_aggregate<F>(
    candidates: Vec<SearchCandidate>,
    threshold: f32,
    parent_lookup: F,
) -> Vec<SearchResult>
where
    F: Fn(&str) -> Option<SearchCandidate>,
{
    let mut aggregator = AdaptiveAggregator::new(threshold);
    aggregator.process(candidates, &parent_lookup);
    aggregator.into_results()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_candidate(
        id: &str,
        parent_id: Option<&str>,
        score: f32,
        sibling_count: u64,
    ) -> SearchCandidate {
        let hierarchy = if id.contains('#') {
            let parts: Vec<&str> = id.split('#').collect();
            let mut h = vec!["Doc".to_string()];
            for (i, _) in parts.iter().skip(1).enumerate() {
                h.push(format!("Section {}", i + 1));
            }
            h
        } else {
            vec!["Doc".to_string()]
        };

        let depth = if id.contains('#') {
            hierarchy.len() as u64 - 1
        } else {
            0
        };
        SearchCandidate {
            id: id.to_string(),
            doc_id: id.split('#').next().unwrap_or(id).to_string(),
            parent_id: parent_id.map(String::from),
            hierarchy,
            depth,
            tree: "local".to_string(),
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
    fn new_creates_empty_aggregator() {
        let agg = AdaptiveAggregator::new(0.5);
        assert_eq!(agg.result_count(), 0);
    }

    #[test]
    fn is_claimed_direct() {
        let mut agg = AdaptiveAggregator::new(0.5);
        agg.claimed.insert("local:test.md#intro".to_string());

        let candidate = make_candidate("local:test.md#intro", Some("local:test.md"), 5.0, 2);
        assert!(agg.is_claimed(&candidate));
    }

    #[test]
    fn is_claimed_via_ancestor() {
        let mut agg = AdaptiveAggregator::new(0.5);

        // Add document as result
        let doc = make_candidate("local:test.md", None, 5.0, 1);
        agg.add_single(doc);

        // Child should be claimed because parent is in results
        let child = make_candidate("local:test.md#intro", Some("local:test.md"), 4.0, 2);
        assert!(agg.is_claimed(&child));

        // Grandchild should also be claimed
        let grandchild = make_candidate(
            "local:test.md#intro-details",
            Some("local:test.md#intro"),
            3.0,
            2,
        );
        assert!(agg.is_claimed(&grandchild));
    }

    #[test]
    fn is_not_claimed_different_doc() {
        let mut agg = AdaptiveAggregator::new(0.5);

        // Add one document as result
        let doc_a = make_candidate("local:a.md", None, 5.0, 1);
        agg.add_single(doc_a);

        // Chunk from different document should not be claimed
        let chunk_b = make_candidate("local:b.md#intro", Some("local:b.md"), 4.0, 2);
        assert!(!agg.is_claimed(&chunk_b));
    }

    #[test]
    fn find_sibling_indices_empty() {
        let agg = AdaptiveAggregator::new(0.5);
        let candidate = make_candidate("local:test.md#intro", Some("local:test.md"), 5.0, 2);
        assert!(agg.find_sibling_indices(&candidate).is_empty());
    }

    #[test]
    fn find_sibling_indices_finds_siblings() {
        let mut agg = AdaptiveAggregator::new(0.5);

        // Add two siblings
        agg.add_single(make_candidate(
            "local:test.md#s1",
            Some("local:test.md"),
            5.0,
            3,
        ));
        agg.add_single(make_candidate(
            "local:test.md#s2",
            Some("local:test.md"),
            4.0,
            3,
        ));

        // Third sibling should find the other two
        let s3 = make_candidate("local:test.md#s3", Some("local:test.md"), 3.0, 3);
        let indices = agg.find_sibling_indices(&s3);
        assert_eq!(indices.len(), 2);
    }

    #[test]
    fn find_sibling_indices_ignores_non_siblings() {
        let mut agg = AdaptiveAggregator::new(0.5);

        // Add chunk from different parent
        agg.add_single(make_candidate(
            "local:other.md#s1",
            Some("local:other.md"),
            5.0,
            2,
        ));

        // Should not find as sibling
        let candidate = make_candidate("local:test.md#s1", Some("local:test.md"), 4.0, 2);
        assert!(agg.find_sibling_indices(&candidate).is_empty());
    }

    #[test]
    fn should_aggregate_at_threshold() {
        let mut agg = AdaptiveAggregator::new(0.5);

        // Add one sibling (1 of 2)
        agg.add_single(make_candidate(
            "local:test.md#s1",
            Some("local:test.md"),
            5.0,
            2,
        ));

        // Second sibling: (1 + 1) / 2 = 100% >= 50%
        let s2 = make_candidate("local:test.md#s2", Some("local:test.md"), 4.0, 2);
        let indices = agg.find_sibling_indices(&s2);
        assert!(agg.should_aggregate(&s2, &indices));
    }

    #[test]
    fn should_aggregate_below_threshold() {
        let mut agg = AdaptiveAggregator::new(0.5);

        // Add one sibling (1 of 5)
        agg.add_single(make_candidate(
            "local:test.md#s1",
            Some("local:test.md"),
            5.0,
            5,
        ));

        // Second sibling: (1 + 1) / 5 = 40% < 50%
        let s2 = make_candidate("local:test.md#s2", Some("local:test.md"), 4.0, 5);
        let indices = agg.find_sibling_indices(&s2);
        assert!(!agg.should_aggregate(&s2, &indices));
    }

    #[test]
    fn should_aggregate_document_level() {
        let agg = AdaptiveAggregator::new(0.5);

        // Document-level node (no parent) can't aggregate further
        let doc = make_candidate("local:test.md", None, 5.0, 1);
        assert!(!agg.should_aggregate(&doc, &[]));
    }

    #[test]
    fn add_single_updates_state() {
        let mut agg = AdaptiveAggregator::new(0.5);

        let candidate = make_candidate("local:test.md#intro", Some("local:test.md"), 5.0, 2);
        agg.add_single(candidate);

        assert_eq!(agg.result_count(), 1);
        assert!(agg.result_index.contains_key("local:test.md#intro"));
        assert_eq!(agg.results()[0].candidate().id, "local:test.md#intro");
    }

    #[test]
    fn remove_results_removes_and_reindexes() {
        let mut agg = AdaptiveAggregator::new(0.5);

        agg.add_single(make_candidate("local:a.md", None, 5.0, 1));
        agg.add_single(make_candidate("local:b.md", None, 4.0, 1));
        agg.add_single(make_candidate("local:c.md", None, 3.0, 1));

        // Remove middle element
        let removed = agg.remove_results(&[1]);

        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].id, "local:b.md");
        assert_eq!(agg.result_count(), 2);
        assert!(!agg.result_index.contains_key("local:b.md"));

        // Check indices are correct after removal
        assert_eq!(*agg.result_index.get("local:a.md").unwrap(), 0);
        assert_eq!(*agg.result_index.get("local:c.md").unwrap(), 1);
    }

    #[test]
    fn add_aggregated_creates_aggregated_result() {
        let mut agg = AdaptiveAggregator::new(0.5);

        let parent = make_candidate("local:test.md", None, 0.0, 1);
        let constituents = vec![
            make_candidate("local:test.md#s1", Some("local:test.md"), 5.0, 2),
            make_candidate("local:test.md#s2", Some("local:test.md"), 4.0, 2),
        ];

        agg.add_aggregated(parent, constituents, &|_| None);

        assert_eq!(agg.result_count(), 1);
        assert!(agg.results()[0].is_aggregated());
        assert_eq!(agg.results()[0].constituents().unwrap().len(), 2);
        // Score should be max of constituents
        assert_eq!(agg.results()[0].candidate().score, 5.0);
    }

    #[test]
    fn cascade_aggregation() {
        let mut agg = AdaptiveAggregator::new(0.5);

        // First, add section1 as an aggregated result
        let section1 = make_candidate("local:test.md#s1", Some("local:test.md"), 0.0, 2);
        let s1_children = vec![
            make_candidate("local:test.md#s1-a", Some("local:test.md#s1"), 5.0, 2),
            make_candidate("local:test.md#s1-b", Some("local:test.md#s1"), 4.0, 2),
        ];

        let doc = make_candidate("local:test.md", None, 0.0, 1);
        let section2 = make_candidate("local:test.md#s2", Some("local:test.md"), 0.0, 2);

        // Add section1 aggregated
        agg.add_aggregated(section1, s1_children, &|id| match id {
            "local:test.md" => Some(doc.clone()),
            "local:test.md#s2" => Some(section2.clone()),
            _ => None,
        });

        // Now add section2 as aggregated - this should trigger cascade to document
        let s2_children = vec![
            make_candidate("local:test.md#s2-a", Some("local:test.md#s2"), 3.0, 2),
            make_candidate("local:test.md#s2-b", Some("local:test.md#s2"), 2.0, 2),
        ];

        agg.add_aggregated(section2.clone(), s2_children, &|id| match id {
            "local:test.md" => Some(doc.clone()),
            _ => None,
        });

        // Both sections should cascade into the document
        assert_eq!(agg.result_count(), 1);
        assert!(agg.results()[0].is_aggregated());
        assert_eq!(agg.results()[0].candidate().id, "local:test.md");
        // Should have 4 constituents (2 from each section)
        assert_eq!(agg.results()[0].constituents().unwrap().len(), 4);
    }

    #[test]
    fn into_results_returns_accumulated() {
        let mut agg = AdaptiveAggregator::new(0.5);

        agg.add_single(make_candidate("local:a.md", None, 5.0, 1));
        agg.add_single(make_candidate("local:b.md", None, 4.0, 1));

        let results = agg.into_results();
        assert_eq!(results.len(), 2);
    }

    // Stage 5 tests: process() and adaptive_aggregate()

    #[test]
    fn process_empty_candidates() {
        let mut agg = AdaptiveAggregator::new(0.5);
        agg.process(vec![], &|_| None);
        assert_eq!(agg.result_count(), 0);
    }

    #[test]
    fn process_single_candidate() {
        let mut agg = AdaptiveAggregator::new(0.5);
        let candidates = vec![make_candidate("local:test.md", None, 5.0, 1)];

        agg.process(candidates, &|_| None);

        assert_eq!(agg.result_count(), 1);
        assert_eq!(agg.results()[0].candidate().id, "local:test.md");
    }

    #[test]
    fn process_handles_multiple_candidates() {
        let mut agg = AdaptiveAggregator::new(0.5);
        let candidates = vec![
            make_candidate("local:a.md", None, 5.0, 1),
            make_candidate("local:b.md", None, 4.0, 1),
            make_candidate("local:c.md", None, 3.0, 1),
            make_candidate("local:d.md", None, 2.0, 1),
        ];

        agg.process(candidates, &|_| None);

        // All candidates should be processed (no limit)
        assert_eq!(agg.result_count(), 4);
    }

    #[test]
    fn process_skips_claimed_descendants() {
        let mut agg = AdaptiveAggregator::new(0.5);

        // Document first, then its children - children should be skipped
        let candidates = vec![
            make_candidate("local:test.md", None, 10.0, 1),
            make_candidate("local:test.md#s1", Some("local:test.md"), 5.0, 2),
            make_candidate("local:test.md#s2", Some("local:test.md"), 4.0, 2),
            make_candidate("local:other.md", None, 3.0, 1),
        ];

        agg.process(candidates, &|_| None);

        // Should have only 2 results: test.md and other.md
        assert_eq!(agg.result_count(), 2);
        assert_eq!(agg.results()[0].candidate().id, "local:test.md");
        assert_eq!(agg.results()[1].candidate().id, "local:other.md");
    }

    #[test]
    fn process_aggregates_siblings() {
        let mut agg = AdaptiveAggregator::new(0.5);

        let doc = make_candidate("local:test.md", None, 0.0, 1);

        // Two siblings that should aggregate (2/2 = 100% >= 50%)
        let candidates = vec![
            make_candidate("local:test.md#s1", Some("local:test.md"), 5.0, 2),
            make_candidate("local:test.md#s2", Some("local:test.md"), 4.0, 2),
        ];

        agg.process(candidates, &|id| {
            if id == "local:test.md" {
                Some(doc.clone())
            } else {
                None
            }
        });

        // Should have 1 aggregated result
        assert_eq!(agg.result_count(), 1);
        assert!(agg.results()[0].is_aggregated());
        assert_eq!(agg.results()[0].candidate().id, "local:test.md");
    }

    #[test]
    fn process_no_aggregate_below_threshold() {
        let mut agg = AdaptiveAggregator::new(0.5);

        let doc = make_candidate("local:test.md", None, 0.0, 1);

        // Two of five siblings (2/5 = 40% < 50%)
        let candidates = vec![
            make_candidate("local:test.md#s1", Some("local:test.md"), 5.0, 5),
            make_candidate("local:test.md#s2", Some("local:test.md"), 4.0, 5),
        ];

        agg.process(candidates, &|id| {
            if id == "local:test.md" {
                Some(doc.clone())
            } else {
                None
            }
        });

        // Should have 2 separate results, not aggregated
        assert_eq!(agg.result_count(), 2);
        assert!(!agg.results()[0].is_aggregated());
        assert!(!agg.results()[1].is_aggregated());
    }

    #[test]
    fn process_interleaved_documents() {
        let mut agg = AdaptiveAggregator::new(0.5);

        let doc_a = make_candidate("local:a.md", None, 0.0, 1);
        let doc_b = make_candidate("local:b.md", None, 0.0, 1);

        // Interleaved candidates from two documents
        let candidates = vec![
            make_candidate("local:a.md#s1", Some("local:a.md"), 10.0, 2),
            make_candidate("local:b.md#s1", Some("local:b.md"), 9.0, 2),
            make_candidate("local:a.md#s2", Some("local:a.md"), 8.0, 2),
            make_candidate("local:b.md#s2", Some("local:b.md"), 7.0, 2),
        ];

        agg.process(candidates, &|id| match id {
            "local:a.md" => Some(doc_a.clone()),
            "local:b.md" => Some(doc_b.clone()),
            _ => None,
        });

        // Both documents should aggregate
        assert_eq!(agg.result_count(), 2);
        assert!(agg.results()[0].is_aggregated());
        assert!(agg.results()[1].is_aggregated());
    }

    #[test]
    fn process_cascading_aggregation() {
        let mut agg = AdaptiveAggregator::new(0.5);

        let doc = make_candidate("local:test.md", None, 0.0, 1);
        let s1 = make_candidate("local:test.md#s1", Some("local:test.md"), 0.0, 2);
        let s2 = make_candidate("local:test.md#s2", Some("local:test.md"), 0.0, 2);

        // Four grandchildren: 2 under s1, 2 under s2
        let candidates = vec![
            make_candidate("local:test.md#s1-a", Some("local:test.md#s1"), 10.0, 2),
            make_candidate("local:test.md#s1-b", Some("local:test.md#s1"), 9.0, 2),
            make_candidate("local:test.md#s2-a", Some("local:test.md#s2"), 8.0, 2),
            make_candidate("local:test.md#s2-b", Some("local:test.md#s2"), 7.0, 2),
        ];

        agg.process(candidates, &|id| match id {
            "local:test.md" => Some(doc.clone()),
            "local:test.md#s1" => Some(s1.clone()),
            "local:test.md#s2" => Some(s2.clone()),
            _ => None,
        });

        // Should cascade all the way to document level
        assert_eq!(agg.result_count(), 1);
        assert!(agg.results()[0].is_aggregated());
        assert_eq!(agg.results()[0].candidate().id, "local:test.md");
        // All 4 grandchildren should be constituents
        assert_eq!(agg.results()[0].constituents().unwrap().len(), 4);
    }

    #[test]
    fn adaptive_aggregate_entry_point() {
        let doc = make_candidate("local:test.md", None, 0.0, 1);

        let candidates = vec![
            make_candidate("local:test.md#s1", Some("local:test.md"), 5.0, 2),
            make_candidate("local:test.md#s2", Some("local:test.md"), 4.0, 2),
        ];

        let results = adaptive_aggregate(candidates, 0.5, |id| {
            if id == "local:test.md" {
                Some(doc.clone())
            } else {
                None
            }
        });

        assert_eq!(results.len(), 1);
        assert!(results[0].is_aggregated());
    }

    #[test]
    fn adaptive_aggregate_processes_all() {
        let candidates = vec![
            make_candidate("local:a.md", None, 5.0, 1),
            make_candidate("local:b.md", None, 4.0, 1),
            make_candidate("local:c.md", None, 3.0, 1),
        ];

        let results = adaptive_aggregate(candidates, 0.5, |_| None);

        // All candidates processed (no limit in aggregation)
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn process_parent_not_found_adds_single() {
        let mut agg = AdaptiveAggregator::new(0.5);

        // Two siblings that should aggregate, but parent lookup fails
        let candidates = vec![
            make_candidate("local:test.md#s1", Some("local:test.md"), 5.0, 2),
            make_candidate("local:test.md#s2", Some("local:test.md"), 4.0, 2),
        ];

        // Parent lookup always returns None
        agg.process(candidates, &|_| None);

        // Should have 2 separate results since parent wasn't found
        assert_eq!(agg.result_count(), 2);
        assert!(!agg.results()[0].is_aggregated());
        assert!(!agg.results()[1].is_aggregated());
    }

    #[test]
    fn ancestor_arriving_after_descendants_subsumes_them() {
        // This tests the bug where children arrive before parent:
        // - Child A (high score) arrives first, added as single
        // - Child B arrives, added as single
        // - Parent arrives later with lower score, should subsume children
        let mut agg = AdaptiveAggregator::new(0.5);

        // Children arrive first with high scores
        let candidates = vec![
            make_candidate("local:test.md#child-a", Some("local:test.md"), 10.0, 3),
            make_candidate("local:test.md#child-b", Some("local:test.md"), 9.0, 3),
            // Parent arrives later with lower score
            make_candidate("local:test.md", None, 5.0, 1),
        ];

        agg.process(candidates, &|_| None);

        // Should have 1 aggregated result: the parent subsuming the children
        assert_eq!(agg.result_count(), 1);
        assert!(agg.results()[0].is_aggregated());
        assert_eq!(agg.results()[0].candidate().id, "local:test.md");
        // Parent should have the children as constituents
        assert_eq!(agg.results()[0].constituents().unwrap().len(), 2);
    }

    #[test]
    fn ancestor_subsumes_nested_descendants() {
        // Document arrives after both children and grandchildren
        let mut agg = AdaptiveAggregator::new(0.5);

        let doc = make_candidate("local:test.md", None, 1.0, 1);
        let s1 = make_candidate("local:test.md#s1", Some("local:test.md"), 0.0, 2);

        // Grandchildren arrive first
        let candidates = vec![
            make_candidate("local:test.md#s1-a", Some("local:test.md#s1"), 10.0, 2),
            make_candidate("local:test.md#s1-b", Some("local:test.md#s1"), 9.0, 2),
            // Section arrives - should subsume grandchildren
            make_candidate("local:test.md#s1", Some("local:test.md"), 5.0, 2),
            // Document arrives last - should subsume section
            make_candidate("local:test.md", None, 2.0, 1),
        ];

        agg.process(candidates, &|id| match id {
            "local:test.md" => Some(doc.clone()),
            "local:test.md#s1" => Some(s1.clone()),
            _ => None,
        });

        // Should cascade all the way to document
        assert_eq!(agg.result_count(), 1);
        assert!(agg.results()[0].is_aggregated());
        assert_eq!(agg.results()[0].candidate().id, "local:test.md");
    }

    #[test]
    fn find_descendant_indices_works() {
        let mut agg = AdaptiveAggregator::new(0.5);

        // Add some children
        agg.add_single(make_candidate(
            "local:test.md#child-a",
            Some("local:test.md"),
            5.0,
            2,
        ));
        agg.add_single(make_candidate(
            "local:test.md#child-b",
            Some("local:test.md"),
            4.0,
            2,
        ));
        // Add unrelated document
        agg.add_single(make_candidate("local:other.md", None, 3.0, 1));

        // Parent should find its children as descendants
        let parent = make_candidate("local:test.md", None, 2.0, 1);
        let descendants = agg.find_descendant_indices(&parent);
        assert_eq!(descendants.len(), 2);

        // Unrelated doc should find no descendants
        let unrelated = make_candidate("local:another.md", None, 1.0, 1);
        let no_descendants = agg.find_descendant_indices(&unrelated);
        assert!(no_descendants.is_empty());
    }
}
