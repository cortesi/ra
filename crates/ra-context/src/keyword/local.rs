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

    /// Creates a RAKE extractor with custom stopwords.
    pub fn with_stopwords(stopwords: &Stopwords) -> Self {
        Self {
            stopwords: stopwords.as_vec(),
            phrase_length: Some(3),
        }
    }

    /// Sets the maximum phrase length.
    pub fn with_phrase_length(mut self, length: Option<usize>) -> Self {
        self.phrase_length = length;
        self
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

    /// Extracts key phrases (multi-word) from text using RAKE.
    ///
    /// Returns phrases sorted by score (highest first).
    pub fn extract_phrases(&self, text: &str) -> Vec<ScoredKeyword> {
        let params =
            RakeParams::WithDefaultsAndPhraseLength(text, &self.stopwords, self.phrase_length);
        let rake = Rake::new(params);

        rake.get_ranked_phrases_scores(usize::MAX)
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
    /// Maximum phrase length to consider.
    phrase_length: Option<usize>,
}

impl Default for TextRankExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl TextRankExtractor {
    /// Creates a new TextRank extractor with default stopwords.
    pub fn new() -> Self {
        Self {
            stopwords: Stopwords::new().as_vec(),
            phrase_length: Some(3),
        }
    }

    /// Creates a TextRank extractor with custom stopwords.
    pub fn with_stopwords(stopwords: &Stopwords) -> Self {
        Self {
            stopwords: stopwords.as_vec(),
            phrase_length: Some(3),
        }
    }

    /// Sets the maximum phrase length.
    pub fn with_phrase_length(mut self, length: Option<usize>) -> Self {
        self.phrase_length = length;
        self
    }

    /// Extracts keywords from text using TextRank.
    ///
    /// Returns keywords sorted by score (highest first).
    pub fn extract(&self, text: &str) -> Vec<ScoredKeyword> {
        let params =
            TextRankParams::WithDefaultsAndPhraseLength(text, &self.stopwords, self.phrase_length);
        let text_rank = TextRank::new(params);

        text_rank
            .get_ranked_word_scores(usize::MAX)
            .into_iter()
            .map(|(term, score)| ScoredKeyword::new(term, score))
            .collect()
    }

    /// Extracts key phrases (multi-word) from text using TextRank.
    ///
    /// Returns phrases sorted by score (highest first).
    pub fn extract_phrases(&self, text: &str) -> Vec<ScoredKeyword> {
        let params =
            TextRankParams::WithDefaultsAndPhraseLength(text, &self.stopwords, self.phrase_length);
        let text_rank = TextRank::new(params);

        text_rank
            .get_ranked_phrase_scores(usize::MAX)
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
    /// N-gram size for keyword extraction.
    ngram: usize,
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
            ngram: 3,
        }
    }

    /// Creates a YAKE extractor with custom stopwords.
    pub fn with_stopwords(stopwords: &Stopwords) -> Self {
        Self {
            stopwords: stopwords.as_vec(),
            ngram: 3,
        }
    }

    /// Sets the n-gram size for keyword extraction.
    pub fn with_ngram(mut self, ngram: usize) -> Self {
        self.ngram = ngram;
        self
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

    /// Extracts n-gram keywords from text using YAKE.
    ///
    /// Returns n-gram keywords sorted by relevance.
    pub fn extract_ngrams(&self, text: &str) -> Vec<ScoredKeyword> {
        let params = YakeParams::WithDefaults(text, &self.stopwords);
        let yake = Yake::new(params);

        let mut keywords: Vec<_> = yake
            .get_ranked_keyword_scores(usize::MAX)
            .into_iter()
            .map(|(term, score)| {
                let inverted_score = 1.0 / (score + 0.0001);
                ScoredKeyword::new(term, inverted_score)
            })
            .collect();

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
    fn rake_extracts_phrases() {
        let extractor = RakeExtractor::new();
        let phrases = extractor.extract_phrases(SAMPLE_TEXT);

        assert!(!phrases.is_empty());
        // Phrases should be multi-word
        let has_multiword = phrases.iter().any(|p| p.term.contains(' '));
        assert!(has_multiword || phrases.len() == 1);
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
