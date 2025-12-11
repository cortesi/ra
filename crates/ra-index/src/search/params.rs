//! Parameter types for search execution.

use super::aggregation::DEFAULT_AGGREGATION_THRESHOLD;
use crate::elbow::DEFAULT_CUTOFF_RATIO;

/// Default final result limit after aggregation.
pub const DEFAULT_LIMIT: usize = 10;

/// Default size of the aggregation pool.
///
/// This controls how many candidates are available for hierarchical aggregation
/// before elbow cutoff. A larger pool allows more siblings to accumulate and
/// merge, improving aggregation quality at the cost of processing more candidates.
pub const DEFAULT_AGGREGATION_POOL_SIZE: usize = 500;

/// Multiplier used to derive candidate_limit from limit when not explicitly set.
///
/// For example, if limit=10 and no candidate_limit is specified, we fetch 10*50=500 candidates.
/// This ensures we have enough raw candidates to fill the aggregation pool (default 500)
/// before filtering.
pub const CANDIDATE_LIMIT_MULTIPLIER: usize = 50;

/// Parameters controlling the four-phase search algorithm.
///
/// The search algorithm proceeds in four phases:
/// 1. **Phase 1 (Query)**: Retrieve up to `candidate_limit` matches from the index
/// 2. **Phase 2 (Normalize)**: Normalize scores across trees (multi-tree only)
/// 3. **Phase 3 (Aggregate)**: Aggregate sibling matches using `aggregation_threshold`
/// 4. **Phase 4 (Elbow)**: Apply relevance cutoff using `cutoff_ratio` and `aggregation_pool_size`
/// 5. **Phase 5 (Limit)**: Truncate to final `limit` results
///
/// When `candidate_limit` is not explicitly set, it defaults to `limit * 50` to ensure
/// enough candidates flow through the pipeline for effective aggregation.
#[derive(Debug, Clone)]
pub struct SearchParams {
    /// Maximum candidates to retrieve in Phase 1.
    /// If None, derived as `limit * CANDIDATE_LIMIT_MULTIPLIER`.
    pub candidate_limit: Option<usize>,
    /// Score ratio threshold for Phase 4 elbow detection. Default: 0.5.
    pub cutoff_ratio: f32,
    /// Size of the aggregation pool - maximum results after Phase 4 elbow cutoff.
    ///
    /// This is a buffer for the aggregation algorithm, not a UI limit. Larger values
    /// allow more siblings to accumulate before filtering, improving aggregation quality.
    /// The final `limit` parameter controls what the user sees.
    pub aggregation_pool_size: usize,
    /// Sibling ratio threshold for Phase 3 aggregation. Default: 0.1.
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
            aggregation_pool_size: DEFAULT_AGGREGATION_POOL_SIZE,
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
}

/// Parameters for MoreLikeThis queries.
///
/// Controls how Tantivy's `MoreLikeThisQuery` extracts and weights terms from the source
/// document to find similar content.
#[derive(Debug, Clone)]
pub struct MoreLikeThisParams {
    /// Minimum document frequency for terms. Terms appearing in fewer documents are ignored.
    pub min_doc_frequency: u64,
    /// Maximum document frequency for terms. Terms appearing in more documents are ignored.
    pub max_doc_frequency: u64,
    /// Minimum term frequency in the source document. Terms appearing fewer times are ignored.
    pub min_term_frequency: usize,
    /// Maximum number of query terms to use.
    pub max_query_terms: usize,
    /// Minimum word length. Shorter words are ignored.
    pub min_word_length: usize,
    /// Maximum word length. Longer words are ignored.
    pub max_word_length: usize,
    /// Boost factor applied to terms.
    pub boost_factor: f32,
    /// Stop words to ignore.
    pub stop_words: Vec<String>,
}

impl Default for MoreLikeThisParams {
    fn default() -> Self {
        Self {
            min_doc_frequency: 1,
            max_doc_frequency: u64::MAX / 2,
            min_term_frequency: 1,
            max_query_terms: 25,
            min_word_length: 3,
            max_word_length: 40,
            boost_factor: 1.0,
            stop_words: Vec::new(),
        }
    }
}
