//! Main context analysis API.
//!
//! This module provides the primary entry point for analyzing source files
//! and generating weighted context queries. It combines:
//!
//! - Path term extraction with source-based weights
//! - Content parsing (markdown, text) with structural weights
//! - TF-IDF ranking using the search index
//! - Phrase detection and validation
//! - Query construction with boosted terms

use std::path::Path;

use ra_query::QueryExpr;

use crate::{
    Stopwords, WeightedTerm, extract_path_terms,
    parser::{ContentParser, MarkdownParser, TextParser},
    phrase::{
        PhraseValidator, ValidatedPhrase, extract_bigrams, extract_trigrams, promote_phrases,
        validate_phrases,
    },
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
    /// Maximum number of phrase candidates to consider.
    pub max_phrase_candidates: usize,
    /// Maximum number of phrases to include in the final query.
    pub max_phrases: usize,
}

impl Default for AnalysisConfig {
    fn default() -> Self {
        Self {
            max_terms: DEFAULT_TERM_LIMIT,
            min_term_length: 3,
            max_phrase_candidates: 20,
            max_phrases: 5,
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
    /// Validated phrases found in the index.
    pub phrases: Vec<ValidatedPhrase>,
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
/// 4. Detects and validates phrases against the index
/// 5. Constructs a boosted OR query from the top terms/phrases
///
/// # Arguments
/// * `path` - Path to the file being analyzed (used for path term extraction)
/// * `content` - Content of the file
/// * `idf_provider` - Source for IDF values (typically the search index)
/// * `phrase_validator` - Validator for checking if phrases exist in the index
/// * `config` - Analysis configuration
///
/// # Returns
/// A `ContextAnalysis` containing extracted terms, ranked terms, phrases, and the query.
pub fn analyze_context<I, P>(
    path: &Path,
    content: &str,
    idf_provider: &I,
    phrase_validator: &P,
    config: &AnalysisConfig,
) -> ContextAnalysis
where
    I: IdfProvider,
    P: PhraseValidator,
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
            phrases: Vec::new(),
            query: None,
        };
    }

    // Rank terms using TF-IDF
    let ranked_terms = rank_terms(terms.clone(), idf_provider);

    // Extract and validate phrases
    let bigrams = extract_bigrams(&ranked_terms, config.max_phrase_candidates);
    let trigrams = extract_trigrams(&ranked_terms, config.max_phrase_candidates / 2);

    let mut candidates = bigrams;
    candidates.extend(trigrams);

    let validated_phrases = validate_phrases(candidates, phrase_validator);

    // Promote phrases (remove consumed terms)
    let promoted = promote_phrases(ranked_terms.clone(), validated_phrases, config.max_phrases);

    // Build the final query
    let query = query::build_query(
        promoted.remaining_terms,
        promoted.phrases.clone(),
        config.max_terms,
    );

    ContextAnalysis {
        terms,
        ranked_terms,
        phrases: promoted.phrases,
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

    /// Mock phrase validator for testing.
    struct MockValidator {
        valid_phrases: Vec<Vec<String>>,
    }

    impl MockValidator {
        fn new() -> Self {
            Self {
                valid_phrases: Vec::new(),
            }
        }

        fn with_phrase(mut self, words: &[&str]) -> Self {
            self.valid_phrases
                .push(words.iter().map(|s| s.to_string()).collect());
            self
        }
    }

    impl PhraseValidator for MockValidator {
        fn phrase_exists(&self, phrase: &[&str]) -> bool {
            let phrase_vec: Vec<String> = phrase.iter().map(|s| s.to_string()).collect();
            self.valid_phrases.contains(&phrase_vec)
        }
    }

    #[test]
    fn analyze_empty_content() {
        let idf = MockIdf::new();
        let validator = MockValidator::new();
        let config = AnalysisConfig::default();

        // Use a path with only short components that get filtered
        let analysis = analyze_context(Path::new("a.b"), "", &idf, &validator, &config);

        assert!(analysis.is_empty());
        assert!(analysis.query.is_none());
    }

    #[test]
    fn analyze_extracts_path_terms() {
        let idf = MockIdf::new();
        let validator = MockValidator::new();
        let config = AnalysisConfig::default();

        let analysis = analyze_context(
            Path::new("src/auth/oauth_handler.rs"),
            "// empty file",
            &idf,
            &validator,
            &config,
        );

        let term_strings: Vec<&str> = analysis.terms.iter().map(|t| t.term.as_str()).collect();
        assert!(term_strings.contains(&"oauth"));
        assert!(term_strings.contains(&"handler"));
    }

    #[test]
    fn analyze_extracts_content_terms() {
        let idf = MockIdf::new();
        let validator = MockValidator::new();
        let config = AnalysisConfig::default();

        let analysis = analyze_context(
            Path::new("doc.md"),
            "# Authentication Guide\n\nThis explains OAuth authentication.",
            &idf,
            &validator,
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
        let validator = MockValidator::new();
        let config = AnalysisConfig::default();

        let analysis = analyze_context(
            Path::new("a.b"), // minimal path to avoid path term interference
            "kubernetes container container container",
            &idf,
            &validator,
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
        let validator = MockValidator::new();
        let config = AnalysisConfig::default();

        let analysis = analyze_context(
            Path::new("auth/login.rs"),
            "authentication logic here",
            &idf,
            &validator,
            &config,
        );

        assert!(analysis.query.is_some());
        let query = analysis.query.unwrap();
        assert!(!query.is_empty());
        assert!(!query.query_string.is_empty());
    }

    #[test]
    fn analyze_detects_phrases() {
        // Give both terms high IDF so they rank highly
        let idf = MockIdf::new()
            .with_term("kubernetes", 5.0)
            .with_term("deployment", 5.0);
        // Note: phrase order depends on ranking order, which is alphabetical for equal scores
        // "deployment" comes before "kubernetes" alphabetically, so the phrase is ["deployment", "kubernetes"]
        let validator = MockValidator::new().with_phrase(&["deployment", "kubernetes"]);
        let config = AnalysisConfig {
            max_phrase_candidates: 50,
            ..Default::default()
        };

        // Use a minimal path (single char) to avoid path term extraction
        let analysis = analyze_context(
            Path::new("x"),
            "kubernetes deployment strategies for kubernetes deployment",
            &idf,
            &validator,
            &config,
        );

        // Debug output
        let term_info: Vec<_> = analysis
            .ranked_terms
            .iter()
            .map(|t| (&t.term.term, t.term.source, t.score))
            .collect();

        // Should have detected the phrase
        assert!(
            !analysis.phrases.is_empty(),
            "expected phrases but got none. ranked_terms: {:?}",
            term_info
        );
    }

    #[test]
    fn analyze_merges_duplicate_terms() {
        let idf = MockIdf::new();
        let validator = MockValidator::new();
        let config = AnalysisConfig::default();

        // "authentication" appears in both path and content
        let analysis = analyze_context(
            Path::new("authentication/service.rs"),
            "authentication service authentication",
            &idf,
            &validator,
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
        let validator = MockValidator::new();
        let config = AnalysisConfig {
            max_terms: 2,
            min_term_length: 8, // 8 chars minimum
            max_phrase_candidates: 10,
            max_phrases: 2,
        };

        let analysis = analyze_context(
            Path::new("a.b"), // minimal path
            "short medium longerterm longest",
            &idf,
            &validator,
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
        let validator = MockValidator::new();
        let config = AnalysisConfig::default();

        let analysis = analyze_context(
            Path::new("test.md"),
            "# Title\n\nContent here",
            &idf,
            &validator,
            &config,
        );

        assert!(!analysis.is_empty());
        assert!(analysis.query_expr().is_some());
        assert!(analysis.query_string().is_some());
    }
}
