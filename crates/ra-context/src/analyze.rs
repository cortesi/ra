//! Main context analysis API.
//!
//! This module provides the primary entry point for analyzing source files
//! and generating weighted context queries. It combines:
//!
//! - Path term extraction with source-based weights
//! - Content parsing (markdown, text) with structural weights
//! - TF-IDF ranking using the search index
//! - Query construction with boosted terms

use std::path::Path;

use ra_query::QueryExpr;

use crate::{
    Stopwords, WeightedTerm, extract_path_terms,
    parser::{ContentParser, MarkdownParser, TextParser},
    query::{self, ContextQuery, DEFAULT_TERM_LIMIT},
    rank::{IdfProvider, RankedTerm, rank_terms},
};

/// Configuration for context analysis.
#[derive(Debug, Clone)]
pub struct AnalysisConfig {
    /// Maximum number of terms to include in the query.
    pub max_terms: usize,
    /// Minimum term length for extraction.
    pub min_term_length: usize,
}

impl Default for AnalysisConfig {
    fn default() -> Self {
        Self {
            max_terms: DEFAULT_TERM_LIMIT,
            min_term_length: 3,
        }
    }
}

/// Result of analyzing a source file for context.
#[derive(Debug, Clone)]
pub struct ContextAnalysis {
    /// All weighted terms extracted from the file.
    pub terms: Vec<WeightedTerm>,
    /// Ranked terms after TF-IDF scoring.
    pub ranked_terms: Vec<RankedTerm>,
    /// The constructed context query (if any terms were extracted).
    pub query: Option<ContextQuery>,
}

impl ContextAnalysis {
    /// Returns true if no useful context was extracted.
    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    /// Returns the query expression, if available.
    pub fn query_expr(&self) -> Option<&QueryExpr> {
        self.query.as_ref().map(|q| &q.expr)
    }

    /// Returns the human-readable query string, if available.
    pub fn query_string(&self) -> Option<&str> {
        self.query.as_ref().map(|q| q.query_string.as_str())
    }
}

/// Analyzes a source file to extract context for finding related documentation.
///
/// This is the main entry point for context analysis. It:
/// 1. Extracts weighted terms from the file path
/// 2. Parses file content (markdown headings get higher weights)
/// 3. Ranks terms using TF-IDF with IDF values from the search index
/// 4. Constructs a boosted OR query from the top terms
///
/// # Arguments
/// * `path` - Path to the file being analyzed (used for path term extraction)
/// * `content` - Content of the file
/// * `idf_provider` - Source for IDF values (typically the search index)
/// * `config` - Analysis configuration
///
/// # Returns
/// A `ContextAnalysis` containing extracted terms, ranked terms, and the query.
pub fn analyze_context<I>(
    path: &Path,
    content: &str,
    idf_provider: &I,
    config: &AnalysisConfig,
) -> ContextAnalysis
where
    I: IdfProvider,
{
    let stopwords = Stopwords::new();

    // Extract terms from path
    let mut terms = extract_path_terms(path, &stopwords, config.min_term_length);

    // Parse content based on file type
    let content_terms = parse_content(path, content, &stopwords, config.min_term_length);
    terms.extend(content_terms);

    // Merge duplicate terms
    terms = merge_terms(terms);

    if terms.is_empty() {
        return ContextAnalysis {
            terms: Vec::new(),
            ranked_terms: Vec::new(),
            query: None,
        };
    }

    // Rank terms using TF-IDF
    let ranked_terms = rank_terms(terms.clone(), idf_provider);

    // Build the final query from ranked terms
    let query = query::build_query(ranked_terms.clone(), config.max_terms);

    ContextAnalysis {
        terms,
        ranked_terms,
        query,
    }
}

/// Parses file content using the appropriate parser based on file type.
fn parse_content(
    path: &Path,
    content: &str,
    stopwords: &Stopwords,
    min_term_length: usize,
) -> Vec<WeightedTerm> {
    // Try markdown parser first
    let md_parser = MarkdownParser::with_settings(stopwords.clone(), min_term_length);
    if md_parser.can_parse(path) {
        return md_parser.parse(path, content);
    }

    // Fall back to text parser
    let text_parser = TextParser::with_settings(stopwords.clone(), min_term_length);
    text_parser.parse(path, content)
}

/// Merges duplicate terms by combining their frequencies.
fn merge_terms(terms: Vec<WeightedTerm>) -> Vec<WeightedTerm> {
    use std::collections::HashMap;

    let mut merged: HashMap<String, WeightedTerm> = HashMap::new();

    for term in terms {
        if let Some(existing) = merged.get_mut(&term.term) {
            existing.frequency += term.frequency;
            // Keep the higher weight
            if term.weight > existing.weight {
                existing.weight = term.weight;
                existing.source = term.source;
            }
        } else {
            merged.insert(term.term.clone(), term);
        }
    }

    merged.into_values().collect()
}

#[cfg(test)]
mod test {
    use std::collections::HashMap;

    use super::*;
    use crate::TermSource;

    /// Mock IDF provider for testing.
    ///
    /// Returns `Some(idf)` for terms that have been added, `None` otherwise.
    struct MockIdf {
        values: HashMap<String, f32>,
    }

    impl MockIdf {
        fn new() -> Self {
            Self {
                values: HashMap::new(),
            }
        }

        fn with_term(mut self, term: &str, idf: f32) -> Self {
            self.values.insert(term.to_string(), idf);
            self
        }
    }

    impl IdfProvider for MockIdf {
        fn idf(&self, term: &str) -> Option<f32> {
            self.values.get(term).copied()
        }
    }

    #[test]
    fn analyze_empty_content() {
        let idf = MockIdf::new();
        let config = AnalysisConfig::default();

        // Use a path with only short components that get filtered
        let analysis = analyze_context(Path::new("a.b"), "", &idf, &config);

        assert!(analysis.is_empty());
        assert!(analysis.query.is_none());
    }

    #[test]
    fn analyze_extracts_path_terms() {
        let idf = MockIdf::new();
        let config = AnalysisConfig::default();

        let analysis = analyze_context(
            Path::new("src/auth/oauth_handler.rs"),
            "// empty file",
            &idf,
            &config,
        );

        let term_strings: Vec<&str> = analysis.terms.iter().map(|t| t.term.as_str()).collect();
        assert!(term_strings.contains(&"oauth"));
        assert!(term_strings.contains(&"handler"));
    }

    #[test]
    fn analyze_extracts_content_terms() {
        let idf = MockIdf::new();
        let config = AnalysisConfig::default();

        let analysis = analyze_context(
            Path::new("doc.md"),
            "# Authentication Guide\n\nThis explains OAuth authentication.",
            &idf,
            &config,
        );

        let term_strings: Vec<&str> = analysis.terms.iter().map(|t| t.term.as_str()).collect();
        assert!(term_strings.contains(&"authentication"));
        assert!(term_strings.contains(&"oauth"));
    }

    #[test]
    fn analyze_ranks_terms() {
        let idf = MockIdf::new()
            .with_term("kubernetes", 5.0)
            .with_term("container", 1.0);
        let config = AnalysisConfig::default();

        let analysis = analyze_context(
            Path::new("a.b"), // minimal path to avoid path term interference
            "kubernetes container container container",
            &idf,
            &config,
        );

        // "kubernetes" should rank higher due to higher IDF
        assert!(!analysis.ranked_terms.is_empty());
        let first = &analysis.ranked_terms[0];
        assert_eq!(first.term.term, "kubernetes");
    }

    #[test]
    fn analyze_builds_query() {
        // Provide IDF values for terms so they're not filtered out
        let idf = MockIdf::new()
            .with_term("auth", 2.0)
            .with_term("login", 2.0)
            .with_term("authentication", 2.0)
            .with_term("logic", 2.0);
        let config = AnalysisConfig::default();

        let analysis = analyze_context(
            Path::new("auth/login.rs"),
            "authentication logic here",
            &idf,
            &config,
        );

        assert!(analysis.query.is_some());
        let query = analysis.query.unwrap();
        assert!(!query.is_empty());
        assert!(!query.query_string.is_empty());
    }

    #[test]
    fn analyze_merges_duplicate_terms() {
        let idf = MockIdf::new();
        let config = AnalysisConfig::default();

        // "authentication" appears in both path and content
        let analysis = analyze_context(
            Path::new("authentication/service.rs"),
            "authentication service authentication",
            &idf,
            &config,
        );

        // Should have merged duplicate "authentication" entries
        let auth_terms: Vec<_> = analysis
            .terms
            .iter()
            .filter(|t| t.term == "authentication")
            .collect();
        assert_eq!(auth_terms.len(), 1);
        // Frequency should be combined (1 from path + 2 from content = 3)
        assert!(
            auth_terms[0].frequency > 1,
            "expected frequency > 1, got {}",
            auth_terms[0].frequency
        );
    }

    #[test]
    fn analyze_respects_config() {
        let idf = MockIdf::new();
        let config = AnalysisConfig {
            max_terms: 2,
            min_term_length: 8, // 8 chars minimum
        };

        let analysis = analyze_context(
            Path::new("a.b"), // minimal path
            "short medium longerterm longest",
            &idf,
            &config,
        );

        // Should filter by min_term_length (8 chars)
        let term_strings: Vec<&str> = analysis.terms.iter().map(|t| t.term.as_str()).collect();
        assert!(!term_strings.contains(&"short")); // 5 chars - too short
        assert!(!term_strings.contains(&"medium")); // 6 chars - too short
        assert!(!term_strings.contains(&"longest")); // 7 chars - too short
        assert!(term_strings.contains(&"longerterm")); // 10 chars - ok
    }

    #[test]
    fn merge_terms_keeps_higher_weight() {
        let terms = vec![
            WeightedTerm {
                term: "test".to_string(),
                weight: 1.0,
                source: TermSource::Body,
                frequency: 1,
            },
            WeightedTerm {
                term: "test".to_string(),
                weight: 3.0,
                source: TermSource::MarkdownH1,
                frequency: 2,
            },
        ];

        let merged = merge_terms(terms);

        assert_eq!(merged.len(), 1);
        let term = &merged[0];
        assert_eq!(term.term, "test");
        assert_eq!(term.weight, 3.0);
        assert_eq!(term.source, TermSource::MarkdownH1);
        assert_eq!(term.frequency, 3);
    }

    #[test]
    fn context_analysis_accessors() {
        // Provide IDF values for terms so they're not filtered out
        let idf = MockIdf::new()
            .with_term("test", 2.0)
            .with_term("title", 2.0)
            .with_term("content", 2.0);
        let config = AnalysisConfig::default();

        let analysis = analyze_context(
            Path::new("test.md"),
            "# Title\n\nContent here",
            &idf,
            &config,
        );

        assert!(!analysis.is_empty());
        assert!(analysis.query_expr().is_some());
        assert!(analysis.query_string().is_some());
    }
}
