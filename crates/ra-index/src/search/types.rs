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

/// Search result with optional snippet and match metadata.
#[derive(Debug, Clone)]
pub struct SearchResult {
    /// Unique chunk identifier (`{tree}:{path}#slug` or `{tree}:{path}`).
    pub id: String,
    /// Document identifier (`{tree}:{path}`).
    pub doc_id: String,
    /// Parent chunk identifier, or None for document nodes.
    pub parent_id: Option<String>,
    /// Chunk title.
    pub title: String,
    /// Tree name the chunk belongs to.
    pub tree: String,
    /// File path within the tree.
    pub path: String,
    /// Chunk body content.
    pub body: String,
    /// Hierarchy breadcrumb.
    pub breadcrumb: String,
    /// Hierarchy depth (0 = document, 1-6 = heading).
    pub depth: u64,
    /// Document order index (0-based pre-order traversal).
    pub position: u64,
    /// Byte offset where content span starts.
    pub byte_start: u64,
    /// Byte offset where content span ends.
    pub byte_end: u64,
    /// Number of siblings including this node.
    pub sibling_count: u64,
    /// Relevance score (after local boost).
    pub score: f32,
    /// Optional HTML snippet with highlights.
    pub snippet: Option<String>,
    /// Byte ranges for matched terms in the body.
    pub match_ranges: Vec<Range<usize>>,
    /// Byte ranges for matched terms in the title.
    pub title_match_ranges: Vec<Range<usize>>,
    /// Byte ranges for matched terms in the path.
    pub path_match_ranges: Vec<Range<usize>>,
    /// Optional per-field match details and explanations.
    pub match_details: Option<MatchDetails>,
}
