//! Local keyword extraction algorithms.
//!
//! These extractors work on individual documents without requiring corpus-wide
//! statistics. They use the `keyword_extraction` crate internally.

use std::cmp::Ordering;

use keyword_extraction::{
    rake::{Rake, RakeParams},
    text_rank::{TextRank, TextRankParams},
    yake::{Yake, YakeParams},
};

/// Extended punctuation list including box-drawing characters and other markdown artifacts.
///
/// The default punctuation in `keyword_extraction` only covers Latin/Germanic languages.
/// This list adds Unicode box-drawing characters commonly found in markdown tables.
static PUNCTUATION: &[&str] = &[
    // Standard punctuation
    ".", ",", ":", ";", "!", "?", "(", ")", "[", "]", "{", "}", "\"", "'", "`", "-", "—", "–", "/",
    "\\", "|", "@", "#", "$", "%", "^", "&", "*", "+", "=", "<", ">", "~", "_",
    // Box-drawing characters (markdown tables)
    "─", "│", "┌", "┐", "└", "┘", "├", "┤", "┬", "┴", "┼", "═", "║", "╔", "╗", "╚", "╝", "╠", "╣",
    "╦", "╩", "╬", "╒", "╓", "╕", "╖", "╘", "╙", "╛", "╜", "╞", "╟", "╡", "╢", "╤", "╥", "╧", "╨",
    "╪", "╫",
];

use super::ScoredKeyword;
use crate::Stopwords;

/// RAKE (Rapid Automatic Keyword Extraction) extractor.
///
/// Extracts key phrases based on word co-occurrence patterns within the document.
/// Good for technical documentation where phrases matter.
pub struct RakeExtractor {
    /// Stopwords to filter out.
    stopwords: Vec<String>,
    /// Maximum phrase length to consider.
    phrase_length: Option<usize>,
}

impl Default for RakeExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl RakeExtractor {
    /// Creates a new RAKE extractor with default stopwords.
    pub fn new() -> Self {
        Self {
            stopwords: Stopwords::new().as_vec(),
            phrase_length: Some(3),
        }
    }

    /// Extracts keywords from text using RAKE.
    ///
    /// Returns keywords sorted by score (highest first).
    pub fn extract(&self, text: &str) -> Vec<ScoredKeyword> {
        let params =
            RakeParams::WithDefaultsAndPhraseLength(text, &self.stopwords, self.phrase_length);
        let rake = Rake::new(params);

        rake.get_ranked_keyword_scores(usize::MAX)
            .into_iter()
            .map(|(term, score)| ScoredKeyword::new(term, score))
            .collect()
    }
}

/// TextRank graph-based keyword extractor.
///
/// Uses a graph-based ranking algorithm similar to PageRank to find
/// representative terms in the document.
pub struct TextRankExtractor {
    /// Stopwords to filter out.
    stopwords: Vec<String>,
    /// Punctuation characters that delimit phrases.
    punctuation: Vec<String>,
}

/// Default window size for TextRank co-occurrence graph.
const DEFAULT_WINDOW_SIZE: usize = 2;
/// Default damping factor for PageRank iteration.
const DEFAULT_DAMPING_FACTOR: f32 = 0.85;
/// Default convergence tolerance.
const DEFAULT_TOLERANCE: f32 = 0.00005;
/// Default maximum phrase length.
const DEFAULT_PHRASE_LENGTH: usize = 3;

impl Default for TextRankExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl TextRankExtractor {
    /// Creates a new TextRank extractor with default stopwords and extended punctuation.
    pub fn new() -> Self {
        Self {
            stopwords: Stopwords::new().as_vec(),
            punctuation: PUNCTUATION.iter().map(|s| (*s).to_string()).collect(),
        }
    }

    /// Extracts keywords from text using TextRank.
    ///
    /// Returns keywords sorted by score (highest first).
    pub fn extract(&self, text: &str) -> Vec<ScoredKeyword> {
        let params = TextRankParams::All(
            text,
            &self.stopwords,
            Some(&self.punctuation),
            DEFAULT_WINDOW_SIZE,
            DEFAULT_DAMPING_FACTOR,
            DEFAULT_TOLERANCE,
            Some(DEFAULT_PHRASE_LENGTH),
        );
        let text_rank = TextRank::new(params);

        text_rank
            .get_ranked_word_scores(usize::MAX)
            .into_iter()
            .map(|(term, score)| ScoredKeyword::new(term, score))
            .collect()
    }
}

/// YAKE (Yet Another Keyword Extractor).
///
/// Statistical keyword extraction that considers term position, frequency,
/// and context. Works well on short texts without training.
pub struct YakeExtractor {
    /// Stopwords to filter out.
    stopwords: Vec<String>,
}

impl Default for YakeExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl YakeExtractor {
    /// Creates a new YAKE extractor with default stopwords.
    pub fn new() -> Self {
        Self {
            stopwords: Stopwords::new().as_vec(),
        }
    }

    /// Extracts keywords from text using YAKE.
    ///
    /// Returns keywords sorted by score. Note: YAKE scores are inverted
    /// (lower = more relevant), so we negate them for consistency.
    pub fn extract(&self, text: &str) -> Vec<ScoredKeyword> {
        let params = YakeParams::WithDefaults(text, &self.stopwords);
        let yake = Yake::new(params);

        // YAKE returns lower scores for more relevant terms, so we sort ascending
        // but present as descending relevance by negating
        let mut keywords: Vec<_> = yake
            .get_ranked_term_scores(usize::MAX)
            .into_iter()
            .map(|(term, score)| {
                // Invert score so higher = more relevant (YAKE native is lower = better)
                // Use 1/(score + epsilon) to avoid division by zero
                let inverted_score = 1.0 / (score + 0.0001);
                ScoredKeyword::new(term, inverted_score)
            })
            .collect();

        // Sort by inverted score (highest first)
        keywords.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(Ordering::Equal));

        keywords
    }
}

#[cfg(test)]
mod test {
    use super::*;

    const SAMPLE_TEXT: &str = "Rust is a systems programming language focused on safety, \
        speed, and concurrency. Rust achieves memory safety without garbage collection. \
        The Rust compiler enforces memory safety through ownership and borrowing rules.";

    #[test]
    fn rake_extracts_keywords() {
        let extractor = RakeExtractor::new();
        let keywords = extractor.extract(SAMPLE_TEXT);

        assert!(!keywords.is_empty());
        // Should find programming-related terms
        let terms: Vec<_> = keywords.iter().map(|k| k.term.as_str()).collect();
        assert!(terms.contains(&"rust") || terms.contains(&"memory") || terms.contains(&"safety"));
    }

    #[test]
    fn textrank_extracts_keywords() {
        let extractor = TextRankExtractor::new();
        let keywords = extractor.extract(SAMPLE_TEXT);

        assert!(!keywords.is_empty());
        // Should find programming-related terms
        let terms: Vec<_> = keywords.iter().map(|k| k.term.as_str()).collect();
        assert!(terms.contains(&"rust") || terms.contains(&"memory") || terms.contains(&"safety"));
    }

    #[test]
    fn yake_extracts_keywords() {
        let extractor = YakeExtractor::new();
        let keywords = extractor.extract(SAMPLE_TEXT);

        assert!(!keywords.is_empty());
        // Scores should be positive (we inverted them)
        assert!(keywords.iter().all(|k| k.score > 0.0));
    }

    #[test]
    fn extractors_handle_empty_text() {
        let rake = RakeExtractor::new();
        let textrank = TextRankExtractor::new();
        let yake = YakeExtractor::new();

        // Should not panic on empty text
        let _ = rake.extract("");
        let _ = textrank.extract("");
        let _ = yake.extract("");
    }
}
