//! Hierarchical aggregation for search results.
//!
//! This module implements Phase 3 of the three-phase search algorithm: bottom-up
//! hierarchical aggregation. When multiple sibling chunks match a query, they can
//! be aggregated into their parent node if enough siblings match.
//!
//! # Algorithm
//!
//! 1. Process matches from deepest level up to depth 1
//! 2. Group matches at each depth by their parent_id
//! 3. For each group: if `match_count / sibling_count >= threshold`, aggregate
//! 4. Aggregated results replace their constituents and may cascade upward
//! 5. Results that don't meet threshold remain as single matches
//!
//! # Example
//!
//! With threshold 0.5 and a document structure:
//! ```text
//! Doc
//! ├── Section 1 (matches)
//! │   ├── Sub 1.1 (matches)
//! │   └── Sub 1.2 (matches)
//! └── Section 2
//!     └── Sub 2.1 (matches)
//! ```
//!
//! Sub 1.1 and 1.2 (2/2 = 100% >= 50%) aggregate into Section 1.
//! Section 1 (now aggregated) doesn't aggregate with Section 2 (1/2 = 50% >= 50%),
//! but Sub 2.1 alone (1/1 = 100%) could aggregate if Section 2 were a match.

use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
};

use crate::result::{SearchCandidate, SearchResult};

/// Default aggregation threshold.
///
/// When the ratio of matching siblings to total siblings meets or exceeds
/// this threshold, the matches are aggregated into their parent.
pub const DEFAULT_AGGREGATION_THRESHOLD: f32 = 0.5;

/// Information about a parent node needed for aggregation.
///
/// This is used to look up parent nodes that may not have matched the query
/// directly but are needed to aggregate their matching children.
#[derive(Debug, Clone)]
pub struct ParentInfo {
    /// Unique chunk identifier.
    pub id: String,
    /// Document identifier.
    pub doc_id: String,
    /// Parent's parent identifier, or None.
    pub parent_id: Option<String>,
    /// Title of the parent node.
    pub title: String,
    /// Tree name.
    pub tree: String,
    /// File path.
    pub path: String,
    /// Body content of the parent.
    pub body: String,
    /// Breadcrumb.
    pub breadcrumb: String,
    /// Hierarchy depth.
    pub depth: u64,
    /// Position in document order.
    pub position: u64,
    /// Byte start offset.
    pub byte_start: u64,
    /// Byte end offset.
    pub byte_end: u64,
    /// Number of siblings (including this node).
    pub sibling_count: u64,
}

impl ParentInfo {
    /// Converts this info into a SearchCandidate with zero score.
    fn to_candidate(&self) -> SearchCandidate {
        SearchCandidate {
            id: self.id.clone(),
            doc_id: self.doc_id.clone(),
            parent_id: self.parent_id.clone(),
            title: self.title.clone(),
            tree: self.tree.clone(),
            path: self.path.clone(),
            body: self.body.clone(),
            breadcrumb: self.breadcrumb.clone(),
            depth: self.depth,
            position: self.position,
            byte_start: self.byte_start,
            byte_end: self.byte_end,
            sibling_count: self.sibling_count,
            score: 0.0,
            snippet: None,
            match_ranges: vec![],
            title_match_ranges: vec![],
            path_match_ranges: vec![],
            match_details: None,
        }
    }
}

/// Aggregates search candidates based on hierarchical relationships.
///
/// This implements Phase 3 of the search algorithm. Candidates are processed
/// bottom-up, grouping siblings and aggregating them into their parent when
/// the ratio of matching siblings to total siblings meets the threshold.
///
/// # Arguments
/// * `candidates` - Search candidates to potentially aggregate
/// * `threshold` - Minimum ratio of matching/total siblings to trigger aggregation (0.0 to 1.0)
/// * `parent_lookup` - Function to look up parent node information by ID
///
/// # Returns
/// A vector of SearchResults, where some may be aggregated and others single matches.
pub fn aggregate<F>(
    candidates: Vec<SearchCandidate>,
    threshold: f32,
    parent_lookup: F,
) -> Vec<SearchResult>
where
    F: Fn(&str) -> Option<ParentInfo>,
{
    if candidates.is_empty() {
        return Vec::new();
    }

    // Track which candidates have been aggregated (by id)
    let mut aggregated_ids: HashMap<String, bool> = HashMap::new();

    // Track results at each stage - maps id -> (candidate or aggregated result, depth)
    let mut current_results: HashMap<String, (ResultOrCandidate, u64)> = HashMap::new();

    // Initialize with all candidates
    for candidate in candidates {
        let depth = candidate.depth;
        let id = candidate.id.clone();
        current_results.insert(id.clone(), (ResultOrCandidate::Candidate(candidate), depth));
        aggregated_ids.insert(id, false);
    }

    // Find max depth
    let max_depth = current_results.values().map(|(_, d)| *d).max().unwrap_or(0);

    // Process from max_depth down to 1 (depth 0 is document level, can't aggregate further)
    for current_depth in (1..=max_depth).rev() {
        // Collect items at current depth, grouped by parent_id
        let mut groups: HashMap<String, Vec<String>> = HashMap::new();

        for (id, (_, depth)) in &current_results {
            if *depth == current_depth && !aggregated_ids.get(id).copied().unwrap_or(false) {
                // Get parent_id for this result
                if let Some((result_or_candidate, _)) = current_results.get(id)
                    && let Some(parent_id) = result_or_candidate.parent_id()
                {
                    groups
                        .entry(parent_id.to_string())
                        .or_default()
                        .push(id.clone());
                }
            }
        }

        // Process each group
        for (parent_id, child_ids) in groups {
            if child_ids.is_empty() {
                continue;
            }

            // Get sibling count from any child (they all share the same parent)
            let sibling_count = current_results
                .get(&child_ids[0])
                .map(|(r, _)| r.sibling_count())
                .unwrap_or(1);

            let match_count = child_ids.len() as u64;
            let ratio = match_count as f32 / sibling_count as f32;

            // Check if we should aggregate
            if ratio >= threshold {
                // Look up parent info
                if let Some(parent_info) = parent_lookup(&parent_id) {
                    // Collect constituents
                    let mut constituents: Vec<SearchCandidate> = Vec::new();
                    for child_id in &child_ids {
                        if let Some((result_or_candidate, _)) = current_results.remove(child_id) {
                            // Flatten: if it's already aggregated, include its constituents
                            match result_or_candidate {
                                ResultOrCandidate::Candidate(c) => constituents.push(c),
                                ResultOrCandidate::Result(SearchResult::Single(c)) => {
                                    constituents.push(c)
                                }
                                ResultOrCandidate::Result(SearchResult::Aggregated {
                                    constituents: inner,
                                    ..
                                }) => {
                                    constituents.extend(inner);
                                }
                            }
                        }
                        aggregated_ids.insert(child_id.clone(), true);
                    }

                    // Check if parent already exists as a direct match
                    let parent_candidate =
                        if let Some((existing, _)) = current_results.remove(&parent_id) {
                            aggregated_ids.insert(parent_id.clone(), true);
                            match existing {
                                ResultOrCandidate::Candidate(c) => c,
                                ResultOrCandidate::Result(SearchResult::Single(c)) => c,
                                ResultOrCandidate::Result(SearchResult::Aggregated {
                                    constituents: inner,
                                    ..
                                }) => {
                                    // Parent was already aggregated - merge constituents
                                    constituents.extend(inner);
                                    parent_info.to_candidate()
                                }
                            }
                        } else {
                            parent_info.to_candidate()
                        };

                    // Create aggregated result
                    let aggregated = SearchResult::aggregated(parent_candidate, constituents);
                    let parent_depth = parent_info.depth;

                    current_results.insert(
                        parent_id.clone(),
                        (ResultOrCandidate::Result(aggregated), parent_depth),
                    );
                    aggregated_ids.insert(parent_id, false); // Can still be aggregated upward
                }
            }
        }
    }

    // Collect final results
    let mut results: Vec<SearchResult> = current_results
        .into_values()
        .map(|(r, _)| r.into_result())
        .collect();

    // Remove descendants whose ancestors appear in results.
    // If a parent document appears, its children shouldn't appear separately.
    let result_ids: HashSet<String> = results.iter().map(|r| r.id().to_string()).collect();

    // Build a map from id -> parent_id for ancestor traversal
    let parent_map: HashMap<String, Option<String>> = results
        .iter()
        .map(|r| (r.id().to_string(), r.parent_id().map(String::from)))
        .collect();

    results.retain(|r| {
        // Check if any ancestor of this result is also in the results
        let id = r.id();

        // First check: document-level ancestor via ID prefix
        if let Some(hash_pos) = id.find('#') {
            let doc_id = &id[..hash_pos];
            if result_ids.contains(doc_id) && id != doc_id {
                return false;
            }
        }

        // Second check: traverse parent chain to find any ancestor in results
        let mut current_parent = r.parent_id().map(String::from);
        while let Some(ref pid) = current_parent {
            if result_ids.contains(pid) {
                return false;
            }
            // Move to grandparent
            current_parent = parent_map.get(pid).and_then(|p| p.clone());
        }

        true
    });

    // Sort by score descending, then by ID for stability
    results.sort_by(|a, b| {
        b.score()
            .partial_cmp(&a.score())
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.id().cmp(b.id()))
    });

    results
}

/// Internal enum to track candidates and results during aggregation.
enum ResultOrCandidate {
    /// A single search candidate (not yet aggregated).
    Candidate(SearchCandidate),
    /// An aggregated or single result.
    Result(SearchResult),
}

impl ResultOrCandidate {
    /// Returns the parent ID of this item.
    fn parent_id(&self) -> Option<&str> {
        match self {
            Self::Candidate(c) => c.parent_id.as_deref(),
            Self::Result(r) => r.parent_id(),
        }
    }

    /// Returns the sibling count of this item.
    fn sibling_count(&self) -> u64 {
        match self {
            Self::Candidate(c) => c.sibling_count,
            Self::Result(r) => r.sibling_count(),
        }
    }

    /// Converts this item into a SearchResult.
    fn into_result(self) -> SearchResult {
        match self {
            Self::Candidate(c) => SearchResult::single(c),
            Self::Result(r) => r,
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    fn make_candidate(
        id: &str,
        parent_id: Option<&str>,
        score: f32,
        depth: u64,
        sibling_count: u64,
    ) -> SearchCandidate {
        SearchCandidate {
            id: id.to_string(),
            doc_id: "local:test.md".to_string(),
            parent_id: parent_id.map(String::from),
            title: format!("Title {id}"),
            tree: "local".to_string(),
            path: "test.md".to_string(),
            body: format!("Body of {id}"),
            breadcrumb: format!("> {id}"),
            depth,
            position: 0,
            byte_start: 0,
            byte_end: 100,
            sibling_count,
            score,
            snippet: None,
            match_ranges: vec![],
            title_match_ranges: vec![],
            path_match_ranges: vec![],
            match_details: None,
        }
    }

    fn make_parent_info(
        id: &str,
        parent_id: Option<&str>,
        depth: u64,
        sibling_count: u64,
    ) -> ParentInfo {
        ParentInfo {
            id: id.to_string(),
            doc_id: "local:test.md".to_string(),
            parent_id: parent_id.map(String::from),
            title: format!("Title {id}"),
            tree: "local".to_string(),
            path: "test.md".to_string(),
            body: format!("Body of {id}"),
            breadcrumb: format!("> {id}"),
            depth,
            position: 0,
            byte_start: 0,
            byte_end: 100,
            sibling_count,
        }
    }

    #[test]
    fn empty_input() {
        let results = aggregate(vec![], 0.5, |_| None);
        assert!(results.is_empty());
    }

    #[test]
    fn single_candidate_no_aggregation() {
        let candidate = make_candidate("local:test.md#intro", Some("local:test.md"), 5.0, 1, 3);
        let results = aggregate(vec![candidate], 0.5, |_| None);

        assert_eq!(results.len(), 1);
        assert!(!results[0].is_aggregated());
        assert_eq!(results[0].id(), "local:test.md#intro");
    }

    #[test]
    fn two_siblings_below_threshold() {
        // 2 out of 5 siblings = 40%, below 50% threshold
        let c1 = make_candidate("local:test.md#s1", Some("local:test.md"), 5.0, 1, 5);
        let c2 = make_candidate("local:test.md#s2", Some("local:test.md"), 4.0, 1, 5);

        let results = aggregate(vec![c1, c2], 0.5, |_| None);

        assert_eq!(results.len(), 2);
        assert!(!results[0].is_aggregated());
        assert!(!results[1].is_aggregated());
    }

    #[test]
    fn two_siblings_at_threshold() {
        // 2 out of 4 siblings = 50%, at threshold - should aggregate
        let c1 = make_candidate("local:test.md#s1", Some("local:test.md"), 5.0, 1, 4);
        let c2 = make_candidate("local:test.md#s2", Some("local:test.md"), 4.0, 1, 4);

        let parent = make_parent_info("local:test.md", None, 0, 1);

        let results = aggregate(vec![c1, c2], 0.5, |id| {
            if id == "local:test.md" {
                Some(parent.clone())
            } else {
                None
            }
        });

        assert_eq!(results.len(), 1);
        assert!(results[0].is_aggregated());
        assert_eq!(results[0].id(), "local:test.md");
        assert_eq!(results[0].constituents().unwrap().len(), 2);
    }

    #[test]
    fn all_siblings_aggregate() {
        // 3 out of 3 siblings = 100%, well above threshold
        let c1 = make_candidate("local:test.md#s1", Some("local:test.md"), 8.0, 1, 3);
        let c2 = make_candidate("local:test.md#s2", Some("local:test.md"), 6.0, 1, 3);
        let c3 = make_candidate("local:test.md#s3", Some("local:test.md"), 4.0, 1, 3);

        let parent = make_parent_info("local:test.md", None, 0, 1);

        let results = aggregate(vec![c1, c2, c3], 0.5, |id| {
            if id == "local:test.md" {
                Some(parent.clone())
            } else {
                None
            }
        });

        assert_eq!(results.len(), 1);
        assert!(results[0].is_aggregated());
        assert_eq!(results[0].id(), "local:test.md");
        // Score should be max of constituents = 8.0
        assert_eq!(results[0].score(), 8.0);
        assert_eq!(results[0].constituents().unwrap().len(), 3);
    }

    #[test]
    fn parent_also_matched_directly() {
        // Parent matched with score 10.0, children with 5.0 and 4.0
        let parent_match = make_candidate("local:test.md", None, 10.0, 0, 1);
        let c1 = make_candidate("local:test.md#s1", Some("local:test.md"), 5.0, 1, 2);
        let c2 = make_candidate("local:test.md#s2", Some("local:test.md"), 4.0, 1, 2);

        let parent_info = make_parent_info("local:test.md", None, 0, 1);

        let results = aggregate(vec![parent_match, c1, c2], 0.5, |id| {
            if id == "local:test.md" {
                Some(parent_info.clone())
            } else {
                None
            }
        });

        assert_eq!(results.len(), 1);
        assert!(results[0].is_aggregated());
        // Score should be max of parent (10.0) and children (5.0, 4.0) = 10.0
        assert_eq!(results[0].score(), 10.0);
    }

    #[test]
    fn cascading_aggregation() {
        // Deep nesting: subsections aggregate to section, section aggregates to doc
        // Doc
        // └── Section (depth 1, 1 sibling)
        //     ├── Sub1 (depth 2, 2 siblings)
        //     └── Sub2 (depth 2, 2 siblings)

        let sub1 = make_candidate(
            "local:test.md#section#sub1",
            Some("local:test.md#section"),
            5.0,
            2,
            2,
        );
        let sub2 = make_candidate(
            "local:test.md#section#sub2",
            Some("local:test.md#section"),
            4.0,
            2,
            2,
        );

        let section_info = make_parent_info("local:test.md#section", Some("local:test.md"), 1, 1);
        let doc_info = make_parent_info("local:test.md", None, 0, 1);

        let results = aggregate(vec![sub1, sub2], 0.5, |id| match id {
            "local:test.md#section" => Some(section_info.clone()),
            "local:test.md" => Some(doc_info.clone()),
            _ => None,
        });

        // Should cascade: sub1+sub2 -> section -> doc
        assert_eq!(results.len(), 1);
        assert!(results[0].is_aggregated());
        assert_eq!(results[0].id(), "local:test.md");
        // Both original subsections should be in constituents
        assert_eq!(results[0].constituents().unwrap().len(), 2);
    }

    #[test]
    fn mixed_depths_partial_aggregation() {
        // Some siblings aggregate, others don't
        // Doc
        // ├── Section1 (2 children, both match -> aggregate to s1)
        // │   ├── Sub1.1
        // │   └── Sub1.2
        // └── Section2 (3 children, only 1 matches -> no aggregate)
        //     ├── Sub2.1 (matches)
        //     ├── Sub2.2
        //     └── Sub2.3
        //
        // After depth 2 processing:
        // - sub1_1 + sub1_2 aggregate to s1 (2/2 = 100% >= 50%)
        // - sub2_1 doesn't aggregate (1/3 = 33% < 50%)
        //
        // After depth 1 processing:
        // - s1 (aggregated) is at depth 1, sibling_count=2
        // - s1 alone = 1/2 = 50% >= 50%, so it aggregates to doc!
        //
        // Finally: sub2_1 is filtered out because its ancestor doc is in results

        let sub1_1 = make_candidate("local:test.md#s1#sub1", Some("local:test.md#s1"), 5.0, 2, 2);
        let sub1_2 = make_candidate("local:test.md#s1#sub2", Some("local:test.md#s1"), 4.0, 2, 2);
        let sub2_1 = make_candidate("local:test.md#s2#sub1", Some("local:test.md#s2"), 3.0, 2, 3);

        let section1_info = make_parent_info("local:test.md#s1", Some("local:test.md"), 1, 2);
        let section2_info = make_parent_info("local:test.md#s2", Some("local:test.md"), 1, 2);
        let doc_info = make_parent_info("local:test.md", None, 0, 1);

        let results = aggregate(vec![sub1_1, sub1_2, sub2_1], 0.5, |id| match id {
            "local:test.md#s1" => Some(section1_info.clone()),
            "local:test.md#s2" => Some(section2_info.clone()),
            "local:test.md" => Some(doc_info.clone()),
            _ => None,
        });

        // Results: Only doc remains - sub2_1 is filtered because doc is its ancestor
        assert_eq!(results.len(), 1);

        let doc_result = &results[0];
        assert_eq!(doc_result.id(), "local:test.md");
        assert!(doc_result.is_aggregated());

        // The doc result should have the original subsections as constituents
        let constituents = doc_result.constituents().unwrap();
        assert_eq!(constituents.len(), 2);
    }

    #[test]
    fn no_parent_lookup_no_aggregation() {
        // Even with all siblings matching, no aggregation without parent info
        let c1 = make_candidate("local:test.md#s1", Some("local:test.md"), 5.0, 1, 2);
        let c2 = make_candidate("local:test.md#s2", Some("local:test.md"), 4.0, 1, 2);

        let results = aggregate(vec![c1, c2], 0.5, |_| None);

        assert_eq!(results.len(), 2);
        assert!(!results[0].is_aggregated());
        assert!(!results[1].is_aggregated());
    }

    #[test]
    fn different_parents_no_cross_aggregation() {
        // Siblings from different parents don't aggregate together
        let c1 = make_candidate("local:a.md#s1", Some("local:a.md"), 5.0, 1, 2);
        let c2 = make_candidate("local:b.md#s1", Some("local:b.md"), 4.0, 1, 2);

        let parent_a = make_parent_info("local:a.md", None, 0, 1);
        let parent_b = make_parent_info("local:b.md", None, 0, 1);

        let results = aggregate(vec![c1, c2], 0.5, |id| match id {
            "local:a.md" => Some(parent_a.clone()),
            "local:b.md" => Some(parent_b.clone()),
            _ => None,
        });

        // Each has only 1/2 siblings = 50%, but they're from different parents
        // So each group has 1 match, which is 1/2 = 50% - at threshold
        // But wait, sibling_count is 2 for each, meaning there are 2 siblings
        // We only have 1 match from each parent, so 1/2 = 50% >= 50% should aggregate

        // Actually with 1 match out of 2 siblings = 50%, it should aggregate each separately
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn results_sorted_by_score() {
        let c1 = make_candidate("local:test.md#low", Some("local:test.md"), 2.0, 1, 5);
        let c2 = make_candidate("local:test.md#high", Some("local:test.md"), 8.0, 1, 5);
        let c3 = make_candidate("local:test.md#mid", Some("local:test.md"), 5.0, 1, 5);

        let results = aggregate(vec![c1, c2, c3], 0.5, |_| None);

        assert_eq!(results.len(), 3);
        assert_eq!(results[0].score(), 8.0);
        assert_eq!(results[1].score(), 5.0);
        assert_eq!(results[2].score(), 2.0);
    }

    #[test]
    fn threshold_zero_always_aggregates() {
        // With threshold 0, even 1 match out of many should aggregate
        let c1 = make_candidate("local:test.md#s1", Some("local:test.md"), 5.0, 1, 100);

        let parent = make_parent_info("local:test.md", None, 0, 1);

        let results = aggregate(vec![c1], 0.0, |id| {
            if id == "local:test.md" {
                Some(parent.clone())
            } else {
                None
            }
        });

        assert_eq!(results.len(), 1);
        assert!(results[0].is_aggregated());
    }

    #[test]
    fn threshold_one_requires_all_siblings() {
        // With threshold 1.0, need all siblings to match
        let c1 = make_candidate("local:test.md#s1", Some("local:test.md"), 5.0, 1, 2);

        let parent = make_parent_info("local:test.md", None, 0, 1);

        // Only 1 out of 2 siblings = 50% < 100%
        let results = aggregate(vec![c1], 1.0, |id| {
            if id == "local:test.md" {
                Some(parent.clone())
            } else {
                None
            }
        });

        assert_eq!(results.len(), 1);
        assert!(!results[0].is_aggregated());
    }

    #[test]
    fn preserves_match_ranges_in_constituents() {
        let mut c1 = make_candidate("local:test.md#s1", Some("local:test.md"), 5.0, 1, 2);
        c1.match_ranges = vec![0..10, 20..30];
        c1.snippet = Some("highlighted".to_string());

        let c2 = make_candidate("local:test.md#s2", Some("local:test.md"), 4.0, 1, 2);

        let parent = make_parent_info("local:test.md", None, 0, 1);

        let results = aggregate(vec![c1, c2], 0.5, |id| {
            if id == "local:test.md" {
                Some(parent.clone())
            } else {
                None
            }
        });

        let constituents = results[0].constituents().unwrap();
        let c1_result = constituents.iter().find(|c| c.id == "local:test.md#s1");
        assert!(c1_result.is_some());
        assert_eq!(c1_result.unwrap().match_ranges, vec![0..10, 20..30]);
        assert_eq!(c1_result.unwrap().snippet, Some("highlighted".to_string()));
    }
}
