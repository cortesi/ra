//! Adaptive hierarchical aggregation for search results.
//!
//! This module implements a streaming aggregation algorithm that processes candidates
//! one-by-one in score order, building results incrementally until the limit is reached.
//!
//! # Key Features
//!
//! 1. **Streams candidates**: Process one at a time, stopping when we have enough results
//! 2. **Claims descendants**: When a parent enters results, all descendants are skipped
//! 3. **Cascades upward**: Aggregating siblings may trigger further aggregation with grandparents
//! 4. **Early termination**: Stops as soon as `limit` results are accumulated
//!
//! # Algorithm
//!
//! ```text
//! for candidate in candidates (sorted by score descending):
//!     if candidate is claimed (ancestor in results):
//!         skip
//!
//!     siblings = find siblings already in results
//!     if should_aggregate(candidate, siblings):
//!         aggregate into parent, cascade if needed
//!     else:
//!         add as single result
//!
//!     if results.len() >= limit:
//!         break
//! ```

use std::collections::{HashMap, HashSet};

use super::hierarchy::is_ancestor_of;
use crate::{SearchCandidate, result::SearchResult};

/// Default aggregation threshold.
///
/// When the ratio of matching siblings to total siblings meets or exceeds
/// this threshold, the matches are aggregated into their parent.
pub const DEFAULT_AGGREGATION_THRESHOLD: f32 = 0.5;

/// Adaptive aggregator that builds results incrementally.
///
/// Processes candidates one at a time, aggregating siblings when appropriate
/// and stopping when the target limit is reached.
pub struct AdaptiveAggregator {
    /// Accumulated search results.
    results: Vec<SearchResult>,
    /// IDs of chunks that have been claimed (their ancestor is in results).
    claimed: HashSet<String>,
    /// Map from result ID to index in results vec (for efficient lookup).
    result_index: HashMap<String, usize>,
    /// Aggregation threshold (fraction of siblings needed to aggregate).
    threshold: f32,
    /// Target number of results.
    limit: usize,
}

impl AdaptiveAggregator {
    /// Creates a new adaptive aggregator.
    ///
    /// # Arguments
    /// * `threshold` - Minimum ratio of matching/total siblings to trigger aggregation (0.0 to 1.0)
    /// * `limit` - Target number of results to accumulate
    pub fn new(threshold: f32, limit: usize) -> Self {
        Self {
            results: Vec::with_capacity(limit),
            claimed: HashSet::new(),
            result_index: HashMap::new(),
            threshold,
            limit,
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
        for result_id in self.result_index.keys() {
            if is_ancestor_of(result_id, &candidate.id) {
                return true;
            }
        }

        false
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

    /// Returns true if we've reached the target limit.
    pub fn is_full(&self) -> bool {
        self.results.len() >= self.limit
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

    /// Consumes the aggregator and returns the accumulated results.
    pub fn into_results(self) -> Vec<SearchResult> {
        self.results
    }

    /// Returns a reference to the current results.
    #[cfg(test)]
    pub fn results(&self) -> &[SearchResult] {
        &self.results
    }

    /// Processes candidates through the adaptive aggregation algorithm.
    ///
    /// Iterates through candidates in order (should be sorted by score descending),
    /// building results incrementally. For each candidate:
    /// - Skip if claimed (ancestor already in results)
    /// - Check if it should aggregate with existing siblings
    /// - Either add as single result or aggregate with siblings
    /// - Stop when limit is reached
    ///
    /// # Arguments
    /// * `candidates` - Candidates to process (should be sorted by score descending)
    /// * `parent_lookup` - Function to look up parent nodes by ID
    pub fn process<F>(&mut self, candidates: Vec<SearchCandidate>, parent_lookup: &F)
    where
        F: Fn(&str) -> Option<SearchCandidate>,
    {
        for candidate in candidates {
            if self.is_full() {
                break;
            }

            if self.is_claimed(&candidate) {
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
/// candidates in score order, aggregating siblings when appropriate and stopping
/// when the limit is reached.
///
/// # Arguments
/// * `candidates` - Raw search candidates (should be sorted by score descending)
/// * `threshold` - Minimum ratio of matching/total siblings to trigger aggregation (0.0 to 1.0)
/// * `limit` - Target number of results
/// * `parent_lookup` - Function to look up parent nodes by ID
///
/// # Returns
/// Aggregated search results, up to `limit` items.
pub fn adaptive_aggregate<F>(
    candidates: Vec<SearchCandidate>,
    threshold: f32,
    limit: usize,
    parent_lookup: F,
) -> Vec<SearchResult>
where
    F: Fn(&str) -> Option<SearchCandidate>,
{
    let mut aggregator = AdaptiveAggregator::new(threshold, limit);
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

        SearchCandidate {
            id: id.to_string(),
            doc_id: id.split('#').next().unwrap_or(id).to_string(),
            parent_id: parent_id.map(String::from),
            hierarchy,
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
        let agg = AdaptiveAggregator::new(0.5, 10);
        assert_eq!(agg.result_count(), 0);
        assert!(!agg.is_full());
    }

    #[test]
    fn is_full_at_limit() {
        let mut agg = AdaptiveAggregator::new(0.5, 2);
        agg.add_single(make_candidate("local:a.md", None, 5.0, 1));
        assert!(!agg.is_full());

        agg.add_single(make_candidate("local:b.md", None, 4.0, 1));
        assert!(agg.is_full());
    }

    #[test]
    fn is_claimed_direct() {
        let mut agg = AdaptiveAggregator::new(0.5, 10);
        agg.claimed.insert("local:test.md#intro".to_string());

        let candidate = make_candidate("local:test.md#intro", Some("local:test.md"), 5.0, 2);
        assert!(agg.is_claimed(&candidate));
    }

    #[test]
    fn is_claimed_via_ancestor() {
        let mut agg = AdaptiveAggregator::new(0.5, 10);

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
        let mut agg = AdaptiveAggregator::new(0.5, 10);

        // Add one document as result
        let doc_a = make_candidate("local:a.md", None, 5.0, 1);
        agg.add_single(doc_a);

        // Chunk from different document should not be claimed
        let chunk_b = make_candidate("local:b.md#intro", Some("local:b.md"), 4.0, 2);
        assert!(!agg.is_claimed(&chunk_b));
    }

    #[test]
    fn find_sibling_indices_empty() {
        let agg = AdaptiveAggregator::new(0.5, 10);
        let candidate = make_candidate("local:test.md#intro", Some("local:test.md"), 5.0, 2);
        assert!(agg.find_sibling_indices(&candidate).is_empty());
    }

    #[test]
    fn find_sibling_indices_finds_siblings() {
        let mut agg = AdaptiveAggregator::new(0.5, 10);

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
        let mut agg = AdaptiveAggregator::new(0.5, 10);

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
        let mut agg = AdaptiveAggregator::new(0.5, 10);

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
        let mut agg = AdaptiveAggregator::new(0.5, 10);

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
        let agg = AdaptiveAggregator::new(0.5, 10);

        // Document-level node (no parent) can't aggregate further
        let doc = make_candidate("local:test.md", None, 5.0, 1);
        assert!(!agg.should_aggregate(&doc, &[]));
    }

    #[test]
    fn add_single_updates_state() {
        let mut agg = AdaptiveAggregator::new(0.5, 10);

        let candidate = make_candidate("local:test.md#intro", Some("local:test.md"), 5.0, 2);
        agg.add_single(candidate);

        assert_eq!(agg.result_count(), 1);
        assert!(agg.result_index.contains_key("local:test.md#intro"));
        assert_eq!(agg.results()[0].candidate().id, "local:test.md#intro");
    }

    #[test]
    fn remove_results_removes_and_reindexes() {
        let mut agg = AdaptiveAggregator::new(0.5, 10);

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
        let mut agg = AdaptiveAggregator::new(0.5, 10);

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
        let mut agg = AdaptiveAggregator::new(0.5, 10);

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
        let mut agg = AdaptiveAggregator::new(0.5, 10);

        agg.add_single(make_candidate("local:a.md", None, 5.0, 1));
        agg.add_single(make_candidate("local:b.md", None, 4.0, 1));

        let results = agg.into_results();
        assert_eq!(results.len(), 2);
    }

    // Stage 5 tests: process() and adaptive_aggregate()

    #[test]
    fn process_empty_candidates() {
        let mut agg = AdaptiveAggregator::new(0.5, 10);
        agg.process(vec![], &|_| None);
        assert_eq!(agg.result_count(), 0);
    }

    #[test]
    fn process_single_candidate() {
        let mut agg = AdaptiveAggregator::new(0.5, 10);
        let candidates = vec![make_candidate("local:test.md", None, 5.0, 1)];

        agg.process(candidates, &|_| None);

        assert_eq!(agg.result_count(), 1);
        assert_eq!(agg.results()[0].candidate().id, "local:test.md");
    }

    #[test]
    fn process_stops_at_limit() {
        let mut agg = AdaptiveAggregator::new(0.5, 2);
        let candidates = vec![
            make_candidate("local:a.md", None, 5.0, 1),
            make_candidate("local:b.md", None, 4.0, 1),
            make_candidate("local:c.md", None, 3.0, 1),
            make_candidate("local:d.md", None, 2.0, 1),
        ];

        agg.process(candidates, &|_| None);

        assert_eq!(agg.result_count(), 2);
        assert_eq!(agg.results()[0].candidate().id, "local:a.md");
        assert_eq!(agg.results()[1].candidate().id, "local:b.md");
    }

    #[test]
    fn process_skips_claimed_descendants() {
        let mut agg = AdaptiveAggregator::new(0.5, 10);

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
        let mut agg = AdaptiveAggregator::new(0.5, 10);

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
        let mut agg = AdaptiveAggregator::new(0.5, 10);

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
        let mut agg = AdaptiveAggregator::new(0.5, 10);

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
        let mut agg = AdaptiveAggregator::new(0.5, 10);

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

        let results = adaptive_aggregate(candidates, 0.5, 10, |id| {
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
    fn adaptive_aggregate_respects_limit() {
        let candidates = vec![
            make_candidate("local:a.md", None, 5.0, 1),
            make_candidate("local:b.md", None, 4.0, 1),
            make_candidate("local:c.md", None, 3.0, 1),
        ];

        let results = adaptive_aggregate(candidates, 0.5, 2, |_| None);

        assert_eq!(results.len(), 2);
    }

    #[test]
    fn process_parent_not_found_adds_single() {
        let mut agg = AdaptiveAggregator::new(0.5, 10);

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
}
