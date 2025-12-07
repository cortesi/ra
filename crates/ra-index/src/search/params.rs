//! Parameter types for search execution.

use super::adaptive::DEFAULT_AGGREGATION_THRESHOLD;
use crate::elbow::DEFAULT_CUTOFF_RATIO;

/// Default final result limit after aggregation.
pub const DEFAULT_LIMIT: usize = 10;

/// Default maximum candidates to pass through Phase 2 into aggregation.
pub const DEFAULT_MAX_CANDIDATES: usize = 50;

/// Multiplier used to derive candidate_limit from limit when not explicitly set.
/// For example, if limit=10 and no candidate_limit is specified, we fetch 10*5=50 candidates.
pub const CANDIDATE_LIMIT_MULTIPLIER: usize = 5;

/// Parameters controlling the four-phase search algorithm.
///
/// The search algorithm proceeds in four phases:
/// 1. **Phase 1 (Query)**: Retrieve up to `candidate_limit` matches from the index
/// 2. **Phase 2 (Elbow)**: Apply relevance cutoff using `cutoff_ratio` and `max_candidates`
/// 3. **Phase 3 (Aggregate)**: Aggregate sibling matches using `aggregation_threshold`
/// 4. **Phase 4 (Limit)**: Truncate to final `limit` results
///
/// When `candidate_limit` is not explicitly set, it defaults to `limit * 5` to ensure
/// enough candidates flow through the pipeline for effective aggregation.
#[derive(Debug, Clone)]
pub struct SearchParams {
    /// Maximum candidates to retrieve in Phase 1.
    /// If None, derived as `limit * CANDIDATE_LIMIT_MULTIPLIER`.
    pub candidate_limit: Option<usize>,
    /// Score ratio threshold for Phase 2 elbow detection. Default: 0.5.
    pub cutoff_ratio: f32,
    /// Maximum results after Phase 2, before aggregation. Default: 50.
    pub max_candidates: usize,
    /// Sibling ratio threshold for Phase 3 aggregation. Default: 0.5.
    pub aggregation_threshold: f32,
    /// Whether to skip Phase 3 aggregation. Default: false.
    pub disable_aggregation: bool,
    /// Final result limit after aggregation. Default: 10.
    pub limit: usize,
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
            candidate_limit: None,
            cutoff_ratio: DEFAULT_CUTOFF_RATIO,
            max_candidates: DEFAULT_MAX_CANDIDATES,
            aggregation_threshold: DEFAULT_AGGREGATION_THRESHOLD,
            disable_aggregation: false,
            limit: DEFAULT_LIMIT,
            trees: Vec::new(),
            verbosity: 0,
        }
    }
}

impl SearchParams {
    /// Returns the effective candidate limit.
    ///
    /// If `candidate_limit` is explicitly set, returns that value.
    /// Otherwise, derives it as `limit * CANDIDATE_LIMIT_MULTIPLIER`.
    pub fn effective_candidate_limit(&self) -> usize {
        self.candidate_limit
            .unwrap_or(self.limit * CANDIDATE_LIMIT_MULTIPLIER)
    }

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
