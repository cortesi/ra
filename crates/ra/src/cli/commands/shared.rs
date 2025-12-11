//! Shared helpers for command implementations.

use ra_config::SearchDefaults;
use ra_index::SearchParams;

/// CLI options for search parameters that can override config defaults.
///
/// Used by `search`, `context`, and `likethis` commands.
pub struct SearchParamsOverrides {
    /// Maximum results to return after aggregation.
    pub limit: Option<usize>,
    /// Maximum candidates to pass through Phase 2 into aggregation.
    pub aggregation_pool_size: Option<usize>,
    /// Score ratio threshold for relevance cutoff.
    pub cutoff_ratio: Option<f32>,
    /// Sibling ratio threshold for aggregation.
    pub aggregation_threshold: Option<f32>,
    /// Whether to disable hierarchical aggregation.
    pub no_aggregation: bool,
    /// Limit results to specific trees.
    pub trees: Vec<String>,
    /// Verbosity level for match details.
    pub verbose: u8,
}

impl SearchParamsOverrides {
    /// Builds `SearchParams` by applying CLI overrides to config defaults.
    pub fn build_params<D: SearchDefaults>(&self, defaults: &D) -> SearchParams {
        SearchParams {
            candidate_limit: None,
            cutoff_ratio: self.cutoff_ratio.unwrap_or_else(|| defaults.cutoff_ratio()),
            aggregation_pool_size: self
                .aggregation_pool_size
                .unwrap_or_else(|| defaults.aggregation_pool_size()),
            aggregation_threshold: self
                .aggregation_threshold
                .unwrap_or_else(|| defaults.aggregation_threshold()),
            disable_aggregation: self.no_aggregation,
            limit: self.limit.unwrap_or_else(|| defaults.limit()),
            trees: self.trees.clone(),
            verbosity: self.verbose,
        }
    }

    /// Builds `SearchParams` with rule-based overrides applied.
    pub fn build_params_with_rule_overrides<D: SearchDefaults>(
        &self,
        defaults: &D,
        rule_overrides: &ra_config::SearchOverrides,
    ) -> SearchParams {
        SearchParams {
            candidate_limit: None,
            cutoff_ratio: self
                .cutoff_ratio
                .or(rule_overrides.cutoff_ratio)
                .unwrap_or_else(|| defaults.cutoff_ratio()),
            aggregation_pool_size: self
                .aggregation_pool_size
                .or(rule_overrides.aggregation_pool_size)
                .unwrap_or_else(|| defaults.aggregation_pool_size()),
            limit: self
                .limit
                .or(rule_overrides.limit)
                .unwrap_or_else(|| defaults.limit()),
            aggregation_threshold: self
                .aggregation_threshold
                .or(rule_overrides.aggregation_threshold)
                .unwrap_or_else(|| defaults.aggregation_threshold()),
            disable_aggregation: self.no_aggregation,
            trees: self.trees.clone(),
            verbosity: self.verbose,
        }
    }
}
