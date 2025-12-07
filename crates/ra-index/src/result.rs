//! Search result types for the hierarchical search algorithm.
//!
//! This module defines the result types used by the three-phase search algorithm:
//! - [`SearchCandidate`]: A single chunk match from the index (re-exported from search)
//! - [`SearchResult`]: Either a single match or an aggregated parent with constituents

use std::cmp::Ordering;

pub use super::search::{MatchDetails, SearchCandidate};

/// A search result, either a single match or an aggregated parent.
///
/// During Phase 3 of the search algorithm, multiple sibling matches may be
/// aggregated into their parent node when enough siblings match. This enum
/// represents both cases:
/// - `Single`: An individual chunk match
/// - `Aggregated`: A parent node that aggregates multiple child matches
#[derive(Debug, Clone)]
pub enum SearchResult {
    /// A single chunk match.
    Single(SearchCandidate),
    /// An aggregated parent node with constituent matches.
    Aggregated {
        /// The parent node containing all metadata (id, title, body, score, etc.)
        parent: SearchCandidate,
        /// The constituent matches that were aggregated.
        constituents: Vec<SearchCandidate>,
    },
}

impl SearchResult {
    /// Returns the underlying candidate for this result.
    ///
    /// For `Single` results, returns the matched candidate.
    /// For `Aggregated` results, returns the parent node.
    pub fn candidate(&self) -> &SearchCandidate {
        match self {
            Self::Single(c) | Self::Aggregated { parent: c, .. } => c,
        }
    }

    /// Returns true if this is an aggregated result.
    pub fn is_aggregated(&self) -> bool {
        matches!(self, Self::Aggregated { .. })
    }

    /// Returns the constituent matches if this is an aggregated result.
    pub fn constituents(&self) -> Option<&[SearchCandidate]> {
        match self {
            Self::Single(_) => None,
            Self::Aggregated { constituents, .. } => Some(constituents),
        }
    }

    /// Returns match details if available.
    ///
    /// For single results, returns the candidate's match details.
    /// For aggregated results, returns the highest-scoring constituent's details,
    /// which is most likely to have the most comprehensive match information.
    pub fn match_details(&self) -> Option<&MatchDetails> {
        match self {
            Self::Single(candidate) => candidate.match_details.as_ref(),
            Self::Aggregated { constituents, .. } => {
                // Find the constituent with the highest score
                constituents
                    .iter()
                    .max_by(|a, b| a.score.partial_cmp(&b.score).unwrap_or(Ordering::Equal))
                    .and_then(|c| c.match_details.as_ref())
            }
        }
    }

    /// Creates a single result from a candidate.
    pub fn single(candidate: SearchCandidate) -> Self {
        Self::Single(candidate)
    }

    /// Creates an aggregated result from a parent node and its constituent matches.
    ///
    /// The score is computed as the maximum score among all constituents.
    pub fn aggregated(mut parent: SearchCandidate, constituents: Vec<SearchCandidate>) -> Self {
        let max_score = constituents
            .iter()
            .map(|c| c.score)
            .fold(parent.score, f32::max);
        parent.score = max_score;

        Self::Aggregated {
            parent,
            constituents,
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    fn make_candidate(id: &str, score: f32, depth: u64) -> SearchCandidate {
        // Build hierarchy based on depth
        let mut hierarchy = vec!["Doc".to_string()];
        for i in 0..depth {
            hierarchy.push(format!("Section {}", i + 1));
        }
        if !id.is_empty()
            && depth > 0
            && let Some(last) = hierarchy.last_mut()
        {
            *last = format!("Title {id}");
        }

        SearchCandidate {
            id: id.to_string(),
            doc_id: "local:test.md".to_string(),
            parent_id: if depth > 0 {
                Some("local:test.md".to_string())
            } else {
                None
            },
            hierarchy,
            depth,
            tree: "local".to_string(),
            path: "test.md".to_string(),
            body: "Body content".to_string(),
            position: 0,
            byte_start: 0,
            byte_end: 100,
            sibling_count: 1,
            score,
            snippet: None,
            match_ranges: vec![],
            hierarchy_match_ranges: vec![],
            path_match_ranges: vec![],
            match_details: None,
        }
    }

    #[test]
    fn single_result_accessors() {
        let candidate = make_candidate("local:test.md#intro", 5.0, 1);
        let result = SearchResult::single(candidate);

        let c = result.candidate();
        assert_eq!(c.id, "local:test.md#intro");
        assert_eq!(c.score, 5.0);
        assert_eq!(c.doc_id, "local:test.md");
        assert_eq!(c.parent_id.as_deref(), Some("local:test.md"));
        assert_eq!(c.title(), "Title local:test.md#intro");
        assert_eq!(c.tree, "local");
        assert_eq!(c.path, "test.md");
        assert_eq!(c.depth, 1);
        assert!(!result.is_aggregated());
        assert!(result.constituents().is_none());
    }

    #[test]
    fn aggregated_result_accessors() {
        let parent = make_candidate("local:test.md", 2.0, 0);
        let child1 = make_candidate("local:test.md#section-1", 8.0, 1);
        let child2 = make_candidate("local:test.md#section-2", 6.0, 1);

        let result = SearchResult::aggregated(parent, vec![child1, child2]);

        let c = result.candidate();
        assert_eq!(c.id, "local:test.md");
        // Score should be max of constituents (8.0) since it's > parent score (2.0)
        assert_eq!(c.score, 8.0);
        assert_eq!(c.doc_id, "local:test.md");
        assert!(c.parent_id.is_none()); // Document node has no parent
        assert_eq!(c.depth, 0);
        assert!(result.is_aggregated());

        let constituents = result.constituents().unwrap();
        assert_eq!(constituents.len(), 2);
        assert_eq!(constituents[0].id, "local:test.md#section-1");
        assert_eq!(constituents[1].id, "local:test.md#section-2");
    }

    #[test]
    fn aggregated_score_is_max_of_constituents() {
        let parent = make_candidate("local:test.md", 10.0, 0);
        let child1 = make_candidate("local:test.md#a", 3.0, 1);
        let child2 = make_candidate("local:test.md#b", 7.0, 1);

        let result = SearchResult::aggregated(parent, vec![child1, child2]);

        // Parent score (10.0) is higher than max constituent (7.0)
        assert_eq!(result.candidate().score, 10.0);
    }

    #[test]
    fn aggregated_with_no_constituents() {
        let parent = make_candidate("local:test.md", 5.0, 0);
        let result = SearchResult::aggregated(parent, vec![]);

        // Score should be parent's score when no constituents
        assert_eq!(result.candidate().score, 5.0);
        assert!(result.constituents().unwrap().is_empty());
    }

    #[test]
    fn single_result_clone() {
        let candidate = make_candidate("local:test.md#intro", 5.0, 1);
        let result = SearchResult::single(candidate);
        let cloned = result.clone();

        assert_eq!(result.candidate().id, cloned.candidate().id);
        assert_eq!(result.candidate().score, cloned.candidate().score);
    }

    #[test]
    #[allow(clippy::single_range_in_vec_init)]
    fn candidate_with_match_ranges() {
        let mut candidate = make_candidate("local:test.md#intro", 5.0, 1);
        candidate.match_ranges = vec![0..5, 10..15];
        candidate.hierarchy_match_ranges = vec![0..5];
        candidate.path_match_ranges = vec![0..5];
        candidate.snippet = Some("highlighted <b>text</b>".to_string());

        let result = SearchResult::single(candidate);

        if let SearchResult::Single(c) = result {
            assert_eq!(c.match_ranges.len(), 2);
            assert_eq!(c.hierarchy_match_ranges.len(), 1);
            assert_eq!(c.path_match_ranges.len(), 1);
            assert_eq!(c.snippet, Some("highlighted <b>text</b>".to_string()));
        } else {
            panic!("Expected Single variant");
        }
    }
}
