//! Keyword extraction algorithms.
//!
//! This module provides multiple algorithms for extracting keywords from text:
//!
//! - **Corpus TF-IDF**: Uses index-wide statistics to find rare/distinctive terms.
//!   Best when you have an index and want corpus-aware ranking.
//! - **RAKE**: Rapid Automatic Keyword Extraction based on word co-occurrence.
//!   Good for technical documentation.
//! - **TextRank**: Graph-based ranking similar to PageRank.
//!   Good for summarization-style extraction.
//! - **YAKE**: Yet Another Keyword Extractor using statistical features.
//!   Good for short texts, no training required.

mod corpus_tfidf;
mod local;

use std::{fmt, str};

pub use corpus_tfidf::CorpusTfIdf;
pub use local::{RakeExtractor, TextRankExtractor, YakeExtractor};

/// Available keyword extraction algorithms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KeywordAlgorithm {
    /// Corpus-aware TF-IDF using index statistics.
    /// Requires an `IdfProvider` to look up term frequencies across the corpus.
    TfIdf,
    /// RAKE (Rapid Automatic Keyword Extraction).
    /// Extracts key phrases based on word co-occurrence patterns.
    Rake,
    /// TextRank graph-based ranking.
    /// Similar to PageRank, good for extracting representative terms.
    #[default]
    TextRank,
    /// YAKE (Yet Another Keyword Extractor).
    /// Statistical approach using term position, frequency, and context.
    Yake,
}

impl KeywordAlgorithm {
    /// Returns a brief description of the algorithm.
    pub fn description(&self) -> &'static str {
        match self {
            Self::TfIdf => "Corpus-aware TF-IDF using index statistics",
            Self::Rake => "RAKE - key phrases based on word co-occurrence",
            Self::TextRank => "Graph-based ranking similar to PageRank",
            Self::Yake => "Statistical approach, no training needed",
        }
    }
}

impl fmt::Display for KeywordAlgorithm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TfIdf => write!(f, "tfidf"),
            Self::Rake => write!(f, "rake"),
            Self::TextRank => write!(f, "textrank"),
            Self::Yake => write!(f, "yake"),
        }
    }
}

impl str::FromStr for KeywordAlgorithm {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "tfidf" | "tf-idf" | "tf_idf" => Ok(Self::TfIdf),
            "rake" => Ok(Self::Rake),
            "textrank" | "text-rank" | "text_rank" => Ok(Self::TextRank),
            "yake" => Ok(Self::Yake),
            _ => Err(format!(
                "unknown algorithm '{}', expected one of: tfidf, rake, textrank, yake",
                s
            )),
        }
    }
}

/// A keyword with its computed score.
#[derive(Debug, Clone, PartialEq)]
pub struct ScoredKeyword {
    /// The extracted keyword or phrase.
    pub term: String,
    /// The relevance score (higher = more relevant).
    pub score: f32,
    /// Optional source label (e.g., "md:h1", "body", "path:filename").
    pub source: Option<String>,
}

impl ScoredKeyword {
    /// Creates a new scored keyword.
    pub fn new(term: impl Into<String>, score: f32) -> Self {
        Self {
            term: term.into(),
            score,
            source: None,
        }
    }

    /// Creates a scored keyword with a source label.
    pub fn with_source(term: impl Into<String>, score: f32, source: impl Into<String>) -> Self {
        Self {
            term: term.into(),
            score,
            source: Some(source.into()),
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn algorithm_from_str() {
        assert_eq!(
            "tfidf".parse::<KeywordAlgorithm>().unwrap(),
            KeywordAlgorithm::TfIdf
        );
        assert_eq!(
            "tf-idf".parse::<KeywordAlgorithm>().unwrap(),
            KeywordAlgorithm::TfIdf
        );
        assert_eq!(
            "rake".parse::<KeywordAlgorithm>().unwrap(),
            KeywordAlgorithm::Rake
        );
        assert_eq!(
            "textrank".parse::<KeywordAlgorithm>().unwrap(),
            KeywordAlgorithm::TextRank
        );
        assert_eq!(
            "text-rank".parse::<KeywordAlgorithm>().unwrap(),
            KeywordAlgorithm::TextRank
        );
        assert_eq!(
            "yake".parse::<KeywordAlgorithm>().unwrap(),
            KeywordAlgorithm::Yake
        );
        assert!("unknown".parse::<KeywordAlgorithm>().is_err());
    }

    #[test]
    fn algorithm_display() {
        assert_eq!(KeywordAlgorithm::TfIdf.to_string(), "tfidf");
        assert_eq!(KeywordAlgorithm::Rake.to_string(), "rake");
        assert_eq!(KeywordAlgorithm::TextRank.to_string(), "textrank");
        assert_eq!(KeywordAlgorithm::Yake.to_string(), "yake");
    }

    #[test]
    fn scored_keyword_creation() {
        let kw = ScoredKeyword::new("test", 1.5);
        assert_eq!(kw.term, "test");
        assert_eq!(kw.score, 1.5);
        assert!(kw.source.is_none());

        let kw = ScoredKeyword::with_source("test", 2.0, "md:h1");
        assert_eq!(kw.source, Some("md:h1".to_string()));
    }
}
