//! Context search API for finding relevant documentation.
//!
//! This module provides [`ContextSearch`], which encapsulates the full context search flow:
//! - File analysis with rule matching
//! - Query building with injected terms
//! - Tree filtering based on matched rules
//! - Include injection into results
//!
//! This API is designed for reuse across CLI and MCP service.

use std::{collections::HashSet, fs, path::Path};

use ra_config::{CompiledContextRules, ContextSettings, MatchedRules};
use ra_context::{AnalysisConfig, ContextAnalysis, analyze_context};
use ra_query::QueryExpr;

use crate::{
    IndexError, SearchCandidate, SearchParams, Searcher, TreeFilteredSearcher, result::SearchResult,
};

/// Boost applied to terms injected from context rules.
const INJECTED_TERM_BOOST: f32 = 2.0;

/// Analysis result for a single file.
#[derive(Debug, Clone)]
pub struct FileAnalysis {
    /// Path to the analyzed file.
    pub path: String,
    /// Context analysis results (terms, ranked terms, query).
    pub analysis: ContextAnalysis,
    /// Matched rules for this file.
    pub matched_rules: MatchedRules,
}

/// Warning emitted while analyzing context files.
#[derive(Debug, Clone)]
pub struct ContextWarning {
    /// Path to the file that triggered the warning.
    pub path: String,
    /// Human-readable reason for the warning.
    pub reason: String,
}

/// Combined analysis results for multiple files.
#[derive(Debug)]
pub struct ContextAnalysisResult {
    /// Per-file analysis results.
    pub files: Vec<FileAnalysis>,
    /// Merged rules across all files.
    pub merged_rules: MatchedRules,
    /// Combined query expression (if any terms were extracted).
    pub query_expr: Option<QueryExpr>,
    /// Warnings encountered while analyzing files.
    pub warnings: Vec<ContextWarning>,
}

impl ContextAnalysisResult {
    /// Returns true if no useful context was extracted.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Returns the query string representation, if available.
    pub fn query_string(&self) -> Option<String> {
        self.query_expr.as_ref().map(|e| e.to_query_string())
    }
}

/// Context search engine that encapsulates the full context search flow.
///
/// This struct provides a reusable API for context-based documentation search,
/// suitable for both CLI and MCP service use.
pub struct ContextSearch<'a> {
    /// The underlying searcher (mutable for search operations).
    searcher: &'a mut Searcher,
    /// Compiled context rules.
    rules: CompiledContextRules,
    /// Analysis configuration.
    analysis_config: AnalysisConfig,
}

impl<'a> ContextSearch<'a> {
    /// Creates a new context search engine.
    ///
    /// # Arguments
    /// * `searcher` - The search index to use for IDF lookups and result retrieval
    /// * `context_settings` - Context configuration including rules
    /// * `max_terms` - Maximum number of terms to include in the query
    pub fn new(
        searcher: &'a mut Searcher,
        context_settings: &ContextSettings,
        max_terms: usize,
    ) -> Result<Self, IndexError> {
        let rules = CompiledContextRules::compile(context_settings)
            .map_err(|e| IndexError::Config(e.to_string()))?;

        Ok(Self {
            searcher,
            rules,
            analysis_config: AnalysisConfig {
                max_terms,
                min_term_length: 3,
            },
        })
    }

    /// Returns a reference to the underlying searcher.
    pub fn searcher(&self) -> &Searcher {
        self.searcher
    }

    /// Analyzes files and returns combined analysis results.
    ///
    /// This performs the analysis phase without executing the search. Useful for
    /// explain mode or when you need to inspect the analysis before searching.
    ///
    /// # Arguments
    /// * `files` - Paths to files to analyze
    /// * `explicit_trees` - Tree filter from CLI (empty = all trees)
    ///
    /// # Returns
    /// Combined analysis results including per-file analyses and merged rules.
    pub fn analyze(&self, files: &[&Path], explicit_trees: &[String]) -> ContextAnalysisResult {
        let mut all_matched_rules = MatchedRules::default();
        let mut file_analyses: Vec<FileAnalysis> = Vec::new();
        let mut warnings: Vec<ContextWarning> = Vec::new();

        for path in files {
            // Skip non-existent files
            if !path.exists() {
                continue;
            }

            // Skip binary files
            if ra_context::is_binary_file(path) {
                continue;
            }

            // Match context rules for this file
            let matched = self.rules.match_rules(path);

            // Merge matched rules across all files
            all_matched_rules.merge(&matched);

            // Read file content
            let content = match fs::read_to_string(path) {
                Ok(content) => content,
                Err(error) => {
                    warnings.push(ContextWarning {
                        path: path.display().to_string(),
                        reason: error.to_string(),
                    });
                    continue;
                }
            };

            // Use tree-filtered searcher for IDF lookups based on explicit + matched trees
            let effective_trees = matched.compute_effective_trees(explicit_trees);
            let filtered_searcher = TreeFilteredSearcher::new(self.searcher, effective_trees);

            // Analyze using the tree-filtered searcher for IDF lookups
            let analysis =
                analyze_context(path, &content, &filtered_searcher, &self.analysis_config);

            if !analysis.is_empty() {
                file_analyses.push(FileAnalysis {
                    path: path.display().to_string(),
                    analysis,
                    matched_rules: matched,
                });
            }
        }

        // Build combined query expression
        let query_expr = self.build_combined_query(&file_analyses, &all_matched_rules);

        ContextAnalysisResult {
            files: file_analyses,
            merged_rules: all_matched_rules,
            query_expr,
            warnings,
        }
    }

    /// Executes a context search and returns aggregated results.
    ///
    /// This performs the full context search flow:
    /// 1. Analyze files and extract terms
    /// 2. Build a combined query with rule-injected terms
    /// 3. Execute the search with tree filtering
    /// 4. Inject auto-included files from rules
    ///
    /// # Arguments
    /// * `files` - Paths to files to analyze
    /// * `params` - Search parameters (limit, trees, etc.)
    ///
    /// # Returns
    /// Tuple of (search results, analysis result).
    pub fn search(
        &mut self,
        files: &[&Path],
        params: &SearchParams,
    ) -> Result<(Vec<SearchResult>, ContextAnalysisResult), IndexError> {
        let analysis = self.analyze(files, &params.trees);
        self.search_with_analysis(analysis, params)
    }

    /// Executes a context search using a precomputed analysis result.
    pub fn search_with_analysis(
        &mut self,
        analysis: ContextAnalysisResult,
        params: &SearchParams,
    ) -> Result<(Vec<SearchResult>, ContextAnalysisResult), IndexError> {
        let Some(ref expr) = analysis.query_expr else {
            return Ok((Vec::new(), analysis));
        };

        // Compute effective trees for the search
        let effective_trees = analysis.merged_rules.compute_effective_trees(&params.trees);

        // Create search params with effective trees
        let search_params = SearchParams {
            trees: effective_trees,
            ..params.clone()
        };

        // Execute the search
        let mut results = self.searcher.search_aggregated_expr(expr, &search_params)?;

        // Inject auto-included files from rules
        self.inject_includes(
            &mut results,
            &analysis.merged_rules.include,
            params.max_results,
        );

        Ok((results, analysis))
    }

    /// Builds a combined query expression from file analyses and matched rules.
    fn build_combined_query(
        &self,
        analyses: &[FileAnalysis],
        matched_rules: &MatchedRules,
    ) -> Option<QueryExpr> {
        let mut exprs: Vec<QueryExpr> = analyses
            .iter()
            .filter_map(|fa| fa.analysis.query_expr().cloned())
            .collect();

        // Inject terms from matched rules with a moderate boost
        for term in &matched_rules.terms {
            let term_expr = QueryExpr::Term(term.clone());
            let boosted = QueryExpr::boost(term_expr, INJECTED_TERM_BOOST);
            exprs.push(boosted);
        }

        if exprs.is_empty() {
            return None;
        }

        if exprs.len() == 1 {
            return exprs.into_iter().next();
        }

        // Combine multiple queries with OR
        Some(QueryExpr::or(exprs))
    }

    /// Injects automatically included files from matched rules at the top of results.
    ///
    /// Include paths are in the format "tree:path". Each matching document is inserted
    /// at the beginning of results (in order), ensuring they appear first.
    fn inject_includes(&self, results: &mut Vec<SearchResult>, includes: &[String], limit: usize) {
        if includes.is_empty() {
            return;
        }

        // Track existing doc IDs to avoid duplicates
        let existing_doc_ids: HashSet<String> =
            results.iter().map(|r| r.doc_id().to_string()).collect();

        // Collect includes to prepend (in reverse order since we'll insert at front)
        let mut to_prepend: Vec<SearchResult> = Vec::new();

        // Parse and look up each include
        for include in includes {
            // Parse tree:path format
            let Some(colon_pos) = include.find(':') else {
                continue; // Invalid format, skip
            };
            let tree = &include[..colon_pos];
            let path = &include[colon_pos + 1..];

            // Find chunks matching this tree/path combination
            if let Ok(search_results) = self.searcher.get_by_path(tree, path) {
                if search_results.is_empty() {
                    continue;
                }

                // Build the doc ID key
                let doc_id = format!("{tree}:{path}");
                if existing_doc_ids.contains(&doc_id) {
                    continue; // Already in results
                }

                // Convert the first result to a candidate with high score for priority
                let first_result = search_results.into_iter().next().unwrap();
                let mut candidate: SearchCandidate = first_result.into();
                candidate.score = f32::MAX; // Manual inclusion gets max score to stay at top

                to_prepend.push(SearchResult::single(candidate));
            }
        }

        // Prepend includes at the start of results, maintaining their order
        if !to_prepend.is_empty() {
            // Truncate results if needed to make room for includes
            let max_search_results = limit.saturating_sub(to_prepend.len());
            results.truncate(max_search_results);

            // Prepend the includes
            to_prepend.append(results);
            *results = to_prepend;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use ra_config::ContextSettings;
    use tempfile::TempDir;

    use super::*;
    use crate::writer::IndexWriter;

    // Compile-time check that the boost is reasonable
    const _: () = {
        assert!(INJECTED_TERM_BOOST > 1.0);
        assert!(INJECTED_TERM_BOOST <= 5.0);
    };

    #[test]
    fn test_injected_term_boost_value() {
        // Verify the constant exists and has a specific value
        assert!((INJECTED_TERM_BOOST - 2.0).abs() < f32::EPSILON);
    }

    #[test]
    fn analyze_emits_warning_for_unreadable_file() {
        let index_dir = TempDir::new().unwrap();

        // Create an empty index so the searcher can open successfully.
        let mut writer = IndexWriter::open(index_dir.path(), "english").unwrap();
        writer.commit().unwrap();

        let trees = vec![ra_config::Tree {
            name: "local".to_string(),
            path: index_dir.path().to_path_buf(),
            is_global: false,
            include: Vec::new(),
            exclude: Vec::new(),
        }];

        let mut searcher = Searcher::open(index_dir.path(), "english", &trees, 1.0).unwrap();
        let settings = ContextSettings::default();
        let context_search = ContextSearch::new(&mut searcher, &settings, settings.limit).unwrap();

        // Use a directory path to force read_to_string to fail.
        let unreadable_path = index_dir.path().join("dir_as_file");
        fs::create_dir(&unreadable_path).unwrap();

        let result = context_search.analyze(&[unreadable_path.as_path()], &[]);

        assert!(result.files.is_empty());
        assert_eq!(result.warnings.len(), 1);
        assert!(result.warnings[0].path.ends_with("dir_as_file"));
    }
}
