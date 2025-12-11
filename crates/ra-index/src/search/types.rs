//! Data structures returned by search.

use std::{collections::HashMap, ops::Range};

use serde::Serialize;

/// A serializable byte range.
///
/// This is used in JSON output to represent match offsets within strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ByteRange {
    /// Byte offset into the source string.
    pub offset: usize,
    /// Length in bytes of the span.
    pub length: usize,
}

impl From<&Range<usize>> for ByteRange {
    fn from(range: &Range<usize>) -> Self {
        Self {
            offset: range.start,
            length: range.end.saturating_sub(range.start),
        }
    }
}

/// Serializes std byte ranges as [`ByteRange`] objects.
fn serialize_ranges<S>(ranges: &[Range<usize>], serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    let byte_ranges: Vec<ByteRange> = ranges.iter().map(ByteRange::from).collect();
    byte_ranges.serialize(serializer)
}

/// Details about how a term matched in a specific field.
#[derive(Debug, Clone, Default, Serialize)]
pub struct FieldMatch {
    /// Term frequency for each matched term in this field.
    pub term_frequencies: HashMap<String, u32>,
}

/// Detailed information about how search terms matched a document.
#[derive(Debug, Clone, Default, Serialize)]
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
#[derive(Debug, Clone, Serialize)]
pub struct SearchCandidate {
    /// Unique chunk identifier: `{tree}:{path}#{slug}` or `{tree}:{path}`.
    pub id: String,
    /// Document identifier: `{tree}:{path}` (same for all chunks in a file).
    pub doc_id: String,
    /// Parent chunk identifier, or None for document nodes.
    pub parent_id: Option<String>,
    /// Hierarchy path from document root to this chunk.
    /// Each element is a title in the path. The last element is this chunk's title.
    pub hierarchy: Vec<String>,
    /// Heading level: 0 for document node, 1-6 for h1-h6.
    pub depth: u64,
    /// Tree name this chunk belongs to.
    pub tree: String,
    /// File path within the tree.
    pub path: String,
    /// Chunk body content.
    pub body: String,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
    /// Byte ranges within `body` where search terms match.
    ///
    /// Offsets are byte positions into the returned `body` text, already sorted and merged
    /// (no overlaps). Each range aligns to a token produced by the index analyzer after
    /// lowercasing/stemming/fuzzy expansion, so consumers can safely highlight the original
    /// substrings using these offsets.
    #[serde(
        serialize_with = "serialize_ranges",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub match_ranges: Vec<Range<usize>>,
    /// Byte ranges within `hierarchy` (specifically the title, last element) where search terms match.
    #[serde(
        rename = "title_match_ranges",
        serialize_with = "serialize_ranges",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub hierarchy_match_ranges: Vec<Range<usize>>,
    /// Byte ranges within `path` where search terms match.
    #[serde(
        serialize_with = "serialize_ranges",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub path_match_ranges: Vec<Range<usize>>,
    /// Detailed match information for verbose output.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub match_details: Option<MatchDetails>,
}

impl SearchCandidate {
    /// Returns the chunk's title (the last element of the hierarchy).
    pub fn title(&self) -> &str {
        self.hierarchy.last().map(|s| s.as_str()).unwrap_or("")
    }

    /// Returns the breadcrumb string by joining hierarchy elements.
    pub fn breadcrumb(&self) -> String {
        self.hierarchy.join(" > ")
    }

    /// Checks if this candidate is an ancestor of another candidate.
    ///
    /// A candidate is an ancestor if:
    /// - They share the same document ID
    /// - AND either:
    ///   - This candidate is the document node (id == doc_id)
    ///   - OR this candidate's slug is a prefix of the other's slug followed by `-`
    pub fn is_ancestor_of(&self, other: &Self) -> bool {
        // Same ID is not an ancestor relationship
        if self.id == other.id {
            return false;
        }

        // Must be in the same document
        if self.doc_id != other.doc_id {
            return false;
        }

        // If self is the document node, it's an ancestor of all chunks in that document
        if self.id == self.doc_id {
            return true;
        }

        // If other is the document node, it can't be a descendant of self
        if other.id == other.doc_id {
            return false;
        }

        // Both are chunks. Extract slugs relative to doc_id.
        // Format is "{doc_id}#{slug}"
        // We know id starts with doc_id and they are equal, and ids are different.
        // And neither is equal to doc_id.
        // So both must have a '#' after doc_id.
        let self_slug_start = self.doc_id.len() + 1;
        let other_slug_start = other.doc_id.len() + 1;

        // Safety check (shouldn't happen given above checks)
        if self.id.len() <= self_slug_start || other.id.len() <= other_slug_start {
            return false;
        }

        let self_slug = &self.id[self_slug_start..];
        let other_slug = &other.id[other_slug_start..];

        // Ancestor's slug must be a prefix of descendant's slug, followed by "-"
        if other_slug.len() > self_slug.len() {
            other_slug.starts_with(self_slug) && other_slug.as_bytes()[self_slug.len()] == b'-'
        } else {
            false
        }
    }
}
