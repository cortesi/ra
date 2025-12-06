//! Search execution for the ra index.
//!
//! Provides the [`Searcher`] struct for querying the index and retrieving results.
//! Supports field boosting, local tree boosting, snippet generation, and per-tree
//! score normalization for multi-tree searches.
//!
//! # Search Algorithm
//!
//! The search process follows these phases:
//!
//! 1. **Query Execution**: Run the query against the index, applying tree filters
//!    and field boosting to get raw BM25 scores.
//!
//! 2. **Score Normalization** (multi-tree only): When searching across multiple trees,
//!    normalize scores so each tree's best result gets 1.0. This makes cross-tree
//!    comparison fair regardless of content density differences. See [`normalize`]
//!    for details.
//!
//! 3. **Elbow Cutoff**: Find the "elbow" point where relevance drops significantly
//!    and truncate results there. With normalized scores, this works correctly
//!    across trees. See [`crate::elbow`] for the algorithm.
//!
//! 4. **Hierarchical Aggregation**: Group sibling matches under parent nodes when
//!    enough siblings match. See [`crate::aggregate`] for details.

mod aggregate_api;
mod execute;
mod fuzzy;
mod idf;
mod mlt;
mod normalize;
mod open;
mod params;
mod query;
mod ranges;
#[cfg(test)]
mod tests;
mod types;

use std::{collections::HashMap, path::PathBuf};

pub use idf::TreeFilteredSearcher;
use levenshtein_automata::LevenshteinAutomatonBuilder;
pub use mlt::{MoreLikeThisExplanation, MoreLikeThisParams};
pub use open::open_searcher;
#[allow(unused_imports)]
pub use params::{DEFAULT_CANDIDATE_LIMIT, SearchParams};
pub use ranges::merge_ranges;
use tantivy::{Index, tokenizer::TextAnalyzer};
pub use types::{MatchDetails, SearchCandidate};

use crate::{query::QueryCompiler, schema::IndexSchema};

/// Default fuzzy edit distance used when compiling queries.
pub const DEFAULT_FUZZY_DISTANCE: u8 = 1;

/// Primary search entry point for the index.
#[allow(clippy::multiple_inherent_impl)]
pub struct Searcher {
    /// Tantivy index handle used for searching.
    pub(crate) index: Index,
    /// Schema describing indexed fields.
    pub(crate) schema: IndexSchema,
    /// Compiles parsed queries into Tantivy queries.
    pub(crate) query_compiler: QueryCompiler,
    /// Analyzer used for both querying and highlighting.
    pub(crate) analyzer: TextAnalyzer,
    /// Builder for fuzzy Levenshtein automatons.
    pub(crate) lev_builder: LevenshteinAutomatonBuilder,
    /// Maximum edit distance used for fuzzy matching.
    pub(crate) fuzzy_distance: u8,
    /// Map of tree name -> whether the tree is global (no local boost).
    pub(crate) tree_is_global: HashMap<String, bool>,
    /// Map of tree name -> filesystem path for content lookup.
    pub(crate) tree_paths: HashMap<String, PathBuf>,
    /// Boost applied to non-global tree hits.
    pub(crate) local_boost: f32,
}
