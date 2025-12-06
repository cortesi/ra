//! Data structures returned by search.

use std::{collections::HashMap, ops::Range};

/// Details about how a term matched in a specific field.
#[derive(Debug, Clone, Default)]
pub struct FieldMatch {
    /// The indexed terms that matched in this field.
    pub matched_terms: Vec<String>,
    /// Term frequency for each matched term in this field.
    pub term_frequencies: HashMap<String, u32>,
}

/// Detailed information about how search terms matched a document.
#[derive(Debug, Clone, Default)]
pub struct MatchDetails {
    /// Original query terms (before stemming).
    pub original_terms: Vec<String>,
    /// Query terms after stemming/tokenization.
    pub stemmed_terms: Vec<String>,
    /// Map from stemmed query term to indexed terms that matched (including fuzzy).
    pub term_mappings: HashMap<String, Vec<String>>,
    /// Per-field term matches.
    pub field_matches: HashMap<String, FieldMatch>,
    /// Base BM25 score returned by Tantivy.
    pub base_score: f32,
    /// Per-field contribution weights.
    pub field_scores: HashMap<String, f32>,
    /// Local boost applied for non-global trees.
    pub local_boost: f32,
    /// Optional Tantivy explanation in pretty JSON.
    pub score_explanation: Option<String>,
}

impl MatchDetails {
    /// Returns true if any match detail was populated.
    pub fn is_populated(&self) -> bool {
        !(self.field_matches.is_empty() && self.term_mappings.is_empty())
    }

    /// Returns the total number of matched terms across all fields.
    pub fn total_matches(&self) -> u32 {
        self.field_matches
            .values()
            .flat_map(|f| f.term_frequencies.values())
            .sum()
    }
}

/// A single search candidate from the index.
///
/// This represents a chunk that matched a search query, with all the metadata
/// needed for display, scoring, and hierarchical aggregation.
#[derive(Debug, Clone)]
pub struct SearchCandidate {
    /// Unique chunk identifier: `{tree}:{path}#{slug}` or `{tree}:{path}`.
    pub id: String,
    /// Document identifier: `{tree}:{path}` (same for all chunks in a file).
    pub doc_id: String,
    /// Parent chunk identifier, or None for document nodes.
    pub parent_id: Option<String>,
    /// Chunk title.
    pub title: String,
    /// Tree name this chunk belongs to.
    pub tree: String,
    /// File path within the tree.
    pub path: String,
    /// Chunk body content.
    pub body: String,
    /// Breadcrumb showing hierarchy path.
    pub breadcrumb: String,
    /// Hierarchy depth: 0 for document, 1-6 for h1-h6.
    pub depth: u64,
    /// Document order index (0-based pre-order traversal).
    pub position: u64,
    /// Byte offset where content span starts.
    pub byte_start: u64,
    /// Byte offset where content span ends.
    pub byte_end: u64,
    /// Number of siblings including this node.
    pub sibling_count: u64,
    /// Search relevance score (after boosting).
    pub score: f32,
    /// Optional snippet with query terms highlighted.
    pub snippet: Option<String>,
    /// Byte ranges within `body` where search terms match.
    ///
    /// Offsets are byte positions into the returned `body` text, already sorted and merged
    /// (no overlaps). Each range aligns to a token produced by the index analyzer after
    /// lowercasing/stemming/fuzzy expansion, so consumers can safely highlight the original
    /// substrings using these offsets.
    pub match_ranges: Vec<Range<usize>>,
    /// Byte ranges within `title` where search terms match.
    pub title_match_ranges: Vec<Range<usize>>,
    /// Byte ranges within `path` where search terms match.
    pub path_match_ranges: Vec<Range<usize>>,
    /// Detailed match information for verbose output.
    pub match_details: Option<MatchDetails>,
}
