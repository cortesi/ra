//! Parameter types for search execution.

use crate::aggregate::DEFAULT_AGGREGATION_THRESHOLD;
use crate::elbow::{DEFAULT_CUTOFF_RATIO, DEFAULT_MAX_RESULTS};

/// Default number of candidates to retrieve from the index in Phase 1.
pub const DEFAULT_CANDIDATE_LIMIT: usize = 100;

/// Parameters controlling the three-phase search algorithm.
///
/// The search algorithm proceeds in three phases:
/// 1. **Phase 1 (Query)**: Retrieve up to `candidate_limit` matches from the index
/// 2. **Phase 2 (Elbow)**: Apply relevance cutoff using `cutoff_ratio` and `max_results`
/// 3. **Phase 3 (Aggregate)**: Aggregate sibling matches using `aggregation_threshold`
#[derive(Debug, Clone)]
pub struct SearchParams {
    /// Maximum candidates to retrieve in Phase 1. Default: 100.
    pub candidate_limit: usize,
    /// Score ratio threshold for Phase 2 elbow detection. Default: 0.5.
    pub cutoff_ratio: f32,
    /// Maximum results after Phase 2. Default: 20.
    pub max_results: usize,
    /// Sibling ratio threshold for Phase 3 aggregation. Default: 0.5.
    pub aggregation_threshold: f32,
    /// Whether to skip Phase 3 aggregation. Default: false.
    pub disable_aggregation: bool,
    /// Limit results to these trees. If empty, search all trees.
    ///
    /// Note: BM25 scoring uses corpus-wide statistics (document frequency, average
    /// document length) computed across all indexed documents, not just the filtered
    /// trees. This means scores reflect term importance globally. For most use cases
    /// this is acceptable since relative ranking within results remains meaningful.
    pub trees: Vec<String>,
    /// Verbosity level for match details (0 = none, 1 = summary, 2+ = full).
    pub verbosity: u8,
}

impl Default for SearchParams {
    fn default() -> Self {
        Self {
            candidate_limit: DEFAULT_CANDIDATE_LIMIT,
            cutoff_ratio: DEFAULT_CUTOFF_RATIO,
            max_results: DEFAULT_MAX_RESULTS,
            aggregation_threshold: DEFAULT_AGGREGATION_THRESHOLD,
            disable_aggregation: false,
            trees: Vec::new(),
            verbosity: 0,
        }
    }
}

impl SearchParams {
    /// Sets the trees to filter results to.
    pub fn with_trees(mut self, trees: Vec<String>) -> Self {
        self.trees = trees;
        self
    }

    /// Sets the verbosity level for match details.
    pub fn with_verbosity(mut self, verbosity: u8) -> Self {
        self.verbosity = verbosity;
        self
    }
}
