//! Search result types for the hierarchical search algorithm.
//!
//! This module defines the result types used by the three-phase search algorithm:
//! - [`SearchCandidate`]: A single chunk match from the index (re-exported from search)
//! - [`SearchResult`]: Either a single match or an aggregated parent with constituents

use std::{cmp::Ordering, ops::Range};

pub use super::search::{MatchDetails, SearchCandidate};

/// A search result, either a single match or an aggregated parent.
///
/// During Phase 3 of the search algorithm, multiple sibling matches may be
/// aggregated into their parent node when enough siblings match. This enum
/// represents both cases:
/// - `Single`: An individual chunk match
/// - `Aggregated`: A parent node that aggregates multiple child matches
#[allow(clippy::large_enum_variant)] // Aggregated carries chunk bodies; boxing would add indirection
#[derive(Debug, Clone)]
pub enum SearchResult {
    /// A single chunk match.
    Single(SearchCandidate),
    /// An aggregated parent node with constituent matches.
    Aggregated {
        /// Unique chunk identifier of the parent.
        id: String,
        /// Document identifier.
        doc_id: String,
        /// Parent chunk identifier (the parent's parent), or None.
        parent_id: Option<String>,
        /// Parent chunk title.
        title: String,
        /// Tree name.
        tree: String,
        /// File path within the tree.
        path: String,
        /// Parent chunk body content.
        body: String,
        /// Breadcrumb showing hierarchy path.
        breadcrumb: String,
        /// Hierarchy depth of the parent.
        depth: u64,
        /// Document order index.
        position: u64,
        /// Byte offset where content span starts.
        byte_start: u64,
        /// Byte offset where content span ends.
        byte_end: u64,
        /// Number of siblings including this node.
        sibling_count: u64,
        /// Aggregated score (max of constituent scores).
        score: f32,
        /// Title match ranges for the parent node.
        title_match_ranges: Vec<Range<usize>>,
        /// Path match ranges for the parent node.
        path_match_ranges: Vec<Range<usize>>,
        /// The constituent matches that were aggregated.
        constituents: Vec<SearchCandidate>,
    },
}

impl SearchResult {
    /// Returns the unique identifier of this result.
    pub fn id(&self) -> &str {
        match self {
            Self::Single(candidate) => &candidate.id,
            Self::Aggregated { id, .. } => id,
        }
    }

    /// Returns the search relevance score of this result.
    pub fn score(&self) -> f32 {
        match self {
            Self::Single(candidate) => candidate.score,
            Self::Aggregated { score, .. } => *score,
        }
    }

    /// Returns the document identifier.
    pub fn doc_id(&self) -> &str {
        match self {
            Self::Single(candidate) => &candidate.doc_id,
            Self::Aggregated { doc_id, .. } => doc_id,
        }
    }

    /// Returns the parent chunk identifier, if any.
    pub fn parent_id(&self) -> Option<&str> {
        match self {
            Self::Single(candidate) => candidate.parent_id.as_deref(),
            Self::Aggregated { parent_id, .. } => parent_id.as_deref(),
        }
    }

    /// Returns the title.
    pub fn title(&self) -> &str {
        match self {
            Self::Single(candidate) => &candidate.title,
            Self::Aggregated { title, .. } => title,
        }
    }

    /// Returns the tree name.
    pub fn tree(&self) -> &str {
        match self {
            Self::Single(candidate) => &candidate.tree,
            Self::Aggregated { tree, .. } => tree,
        }
    }

    /// Returns the file path.
    pub fn path(&self) -> &str {
        match self {
            Self::Single(candidate) => &candidate.path,
            Self::Aggregated { path, .. } => path,
        }
    }

    /// Returns the body content.
    pub fn body(&self) -> &str {
        match self {
            Self::Single(candidate) => &candidate.body,
            Self::Aggregated { body, .. } => body,
        }
    }

    /// Returns the breadcrumb.
    pub fn breadcrumb(&self) -> &str {
        match self {
            Self::Single(candidate) => &candidate.breadcrumb,
            Self::Aggregated { breadcrumb, .. } => breadcrumb,
        }
    }

    /// Returns the hierarchy depth.
    pub fn depth(&self) -> u64 {
        match self {
            Self::Single(candidate) => candidate.depth,
            Self::Aggregated { depth, .. } => *depth,
        }
    }

    /// Returns the position in document order.
    pub fn position(&self) -> u64 {
        match self {
            Self::Single(candidate) => candidate.position,
            Self::Aggregated { position, .. } => *position,
        }
    }

    /// Returns the byte start offset.
    pub fn byte_start(&self) -> u64 {
        match self {
            Self::Single(candidate) => candidate.byte_start,
            Self::Aggregated { byte_start, .. } => *byte_start,
        }
    }

    /// Returns the byte end offset.
    pub fn byte_end(&self) -> u64 {
        match self {
            Self::Single(candidate) => candidate.byte_end,
            Self::Aggregated { byte_end, .. } => *byte_end,
        }
    }

    /// Returns the sibling count.
    pub fn sibling_count(&self) -> u64 {
        match self {
            Self::Single(candidate) => candidate.sibling_count,
            Self::Aggregated { sibling_count, .. } => *sibling_count,
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

    /// Returns title match ranges.
    pub fn title_match_ranges(&self) -> &[Range<usize>] {
        match self {
            Self::Single(candidate) => &candidate.title_match_ranges,
            Self::Aggregated {
                title_match_ranges, ..
            } => title_match_ranges,
        }
    }

    /// Returns path match ranges.
    pub fn path_match_ranges(&self) -> &[Range<usize>] {
        match self {
            Self::Single(candidate) => &candidate.path_match_ranges,
            Self::Aggregated {
                path_match_ranges, ..
            } => path_match_ranges,
        }
    }

    /// Returns body match ranges.
    pub fn match_ranges(&self) -> &[Range<usize>] {
        match self {
            Self::Single(candidate) => &candidate.match_ranges,
            Self::Aggregated { .. } => &[],
        }
    }

    /// Creates a single result from a candidate.
    pub fn single(candidate: SearchCandidate) -> Self {
        Self::Single(candidate)
    }

    /// Creates an aggregated result from a parent node and its constituent matches.
    ///
    /// The score is computed as the maximum score among all constituents.
    pub fn aggregated(parent: SearchCandidate, constituents: Vec<SearchCandidate>) -> Self {
        let max_score = constituents
            .iter()
            .map(|c| c.score)
            .fold(parent.score, f32::max);

        Self::Aggregated {
            id: parent.id,
            doc_id: parent.doc_id,
            parent_id: parent.parent_id,
            title: parent.title,
            tree: parent.tree,
            path: parent.path,
            body: parent.body,
            breadcrumb: parent.breadcrumb,
            depth: parent.depth,
            position: parent.position,
            byte_start: parent.byte_start,
            byte_end: parent.byte_end,
            sibling_count: parent.sibling_count,
            score: max_score,
            title_match_ranges: parent.title_match_ranges,
            path_match_ranges: parent.path_match_ranges,
            constituents,
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    fn make_candidate(id: &str, score: f32, depth: u64) -> SearchCandidate {
        SearchCandidate {
            id: id.to_string(),
            doc_id: "local:test.md".to_string(),
            parent_id: if depth > 0 {
                Some("local:test.md".to_string())
            } else {
                None
            },
            title: format!("Title {id}"),
            tree: "local".to_string(),
            path: "test.md".to_string(),
            body: "Body content".to_string(),
            breadcrumb: "> Test".to_string(),
            depth,
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
    fn single_result_accessors() {
        let candidate = make_candidate("local:test.md#intro", 5.0, 1);
        let result = SearchResult::single(candidate);

        assert_eq!(result.id(), "local:test.md#intro");
        assert_eq!(result.score(), 5.0);
        assert_eq!(result.doc_id(), "local:test.md");
        assert_eq!(result.parent_id(), Some("local:test.md"));
        assert_eq!(result.title(), "Title local:test.md#intro");
        assert_eq!(result.tree(), "local");
        assert_eq!(result.path(), "test.md");
        assert_eq!(result.depth(), 1);
        assert!(!result.is_aggregated());
        assert!(result.constituents().is_none());
    }

    #[test]
    fn aggregated_result_accessors() {
        let parent = make_candidate("local:test.md", 2.0, 0);
        let child1 = make_candidate("local:test.md#section-1", 8.0, 1);
        let child2 = make_candidate("local:test.md#section-2", 6.0, 1);

        let result = SearchResult::aggregated(parent, vec![child1, child2]);

        assert_eq!(result.id(), "local:test.md");
        // Score should be max of constituents (8.0) since it's > parent score (2.0)
        assert_eq!(result.score(), 8.0);
        assert_eq!(result.doc_id(), "local:test.md");
        assert!(result.parent_id().is_none()); // Document node has no parent
        assert_eq!(result.depth(), 0);
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
        assert_eq!(result.score(), 10.0);
    }

    #[test]
    fn aggregated_with_no_constituents() {
        let parent = make_candidate("local:test.md", 5.0, 0);
        let result = SearchResult::aggregated(parent, vec![]);

        // Score should be parent's score when no constituents
        assert_eq!(result.score(), 5.0);
        assert!(result.constituents().unwrap().is_empty());
    }

    #[test]
    fn single_result_clone() {
        let candidate = make_candidate("local:test.md#intro", 5.0, 1);
        let result = SearchResult::single(candidate);
        let cloned = result.clone();

        assert_eq!(result.id(), cloned.id());
        assert_eq!(result.score(), cloned.score());
    }

    #[test]
    #[allow(clippy::single_range_in_vec_init)]
    fn candidate_with_match_ranges() {
        let mut candidate = make_candidate("local:test.md#intro", 5.0, 1);
        candidate.match_ranges = vec![0..5, 10..15];
        candidate.title_match_ranges = vec![0..5];
        candidate.path_match_ranges = vec![0..5];
        candidate.snippet = Some("highlighted <b>text</b>".to_string());

        let result = SearchResult::single(candidate);

        if let SearchResult::Single(c) = result {
            assert_eq!(c.match_ranges.len(), 2);
            assert_eq!(c.title_match_ranges.len(), 1);
            assert_eq!(c.path_match_ranges.len(), 1);
            assert_eq!(c.snippet, Some("highlighted <b>text</b>".to_string()));
        } else {
            panic!("Expected Single variant");
        }
    }
}
