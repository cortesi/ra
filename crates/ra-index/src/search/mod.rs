//! Search execution for the ra index.
//!
//! Provides the [`Searcher`] struct for querying the index and retrieving results.
//! Supports field boosting, local tree boosting, and snippet generation.

mod aggregate_api;
mod execute;
mod fuzzy;
mod idf;
mod open;
mod params;
mod query;
mod ranges;
#[cfg(test)]
mod tests;
mod types;

pub use idf::TreeFilteredSearcher;
pub use open::open_searcher;
#[allow(unused_imports)]
pub use params::{DEFAULT_CANDIDATE_LIMIT, SearchParams};
#[allow(unused_imports)]
pub use types::{FieldMatch, MatchDetails, SearchResult};

use std::collections::HashMap;
use std::path::PathBuf;

use levenshtein_automata::LevenshteinAutomatonBuilder;
use tantivy::Index;
use tantivy::tokenizer::TextAnalyzer;

use crate::query::QueryCompiler;
use crate::schema::IndexSchema;

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
