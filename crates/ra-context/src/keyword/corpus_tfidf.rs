//! Corpus-aware TF-IDF keyword extraction.
//!
//! This extractor uses IDF values from the search index to rank terms by their
//! distinctiveness across the entire corpus. Terms that are rare in the corpus
//! but frequent in the document get higher scores.

use std::cmp::Ordering;

use super::ScoredKeyword;
use crate::{Stopwords, WeightedTerm, rank::IdfProvider};

/// Corpus-aware TF-IDF keyword extractor.
///
/// Uses IDF values from an external provider (typically the search index) to
/// rank terms. The score formula is: `frequency × source_weight × idf`.
pub struct CorpusTfIdf<'a, P: IdfProvider> {
    /// Provider for IDF values from the corpus.
    idf_provider: &'a P,
    /// Stopwords to filter out.
    stopwords: Stopwords,
    /// Minimum term length to consider.
    min_term_length: usize,
}

impl<'a, P: IdfProvider> CorpusTfIdf<'a, P> {
    /// Creates a new corpus TF-IDF extractor.
    pub fn new(idf_provider: &'a P) -> Self {
        Self {
            idf_provider,
            stopwords: Stopwords::new(),
            min_term_length: 3,
        }
    }

    /// Extracts keywords from text using corpus TF-IDF.
    ///
    /// Returns keywords sorted by score (highest first).
    pub fn extract(&self, text: &str) -> Vec<ScoredKeyword> {
        let terms = self.tokenize_and_count(text);
        self.rank_terms(terms)
    }

    /// Extracts keywords from pre-parsed weighted terms.
    ///
    /// This is useful when terms have already been extracted with structural
    /// weights (e.g., from markdown headings).
    pub fn extract_from_weighted(&self, terms: Vec<WeightedTerm>) -> Vec<ScoredKeyword> {
        self.rank_terms(terms)
    }

    /// Tokenizes text and counts term frequencies.
    fn tokenize_and_count(&self, text: &str) -> Vec<WeightedTerm> {
        use std::collections::HashMap;

        let mut term_counts: HashMap<String, u32> = HashMap::new();

        for token in tokenize(text, self.min_term_length) {
            if !self.stopwords.contains(&token) {
                *term_counts.entry(token).or_insert(0) += 1;
            }
        }

        term_counts
            .into_iter()
            .map(|(term, freq)| {
                let mut wt = WeightedTerm::new(term, "body", 1.0);
                wt.frequency = freq;
                wt
            })
            .collect()
    }

    /// Ranks terms by TF-IDF score.
    fn rank_terms(&self, terms: Vec<WeightedTerm>) -> Vec<ScoredKeyword> {
        let mut scored: Vec<ScoredKeyword> = terms
            .into_iter()
            .filter_map(|term| {
                let idf = self.idf_provider.idf(&term.term)?;
                let score = term.frequency as f32 * term.weight * idf;
                Some(ScoredKeyword::with_source(term.term, score, term.source))
            })
            .collect();

        scored.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(Ordering::Equal)
                .then_with(|| a.term.cmp(&b.term))
        });

        scored
    }
}

/// Tokenizes text into individual terms.
fn tokenize(text: &str, min_length: usize) -> impl Iterator<Item = String> + '_ {
    text.split(|c: char| c.is_whitespace() || c.is_ascii_punctuation())
        .map(|s| s.to_ascii_lowercase())
        .filter(move |s| s.len() >= min_length && s.chars().all(|c| c.is_alphanumeric()))
}

#[cfg(test)]
mod test {
    use std::collections::HashMap;

    use super::*;

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
    fn extract_ranks_by_score() {
        let idf = MockIdf::new()
            .with_term("rare", 5.0)
            .with_term("common", 1.0);

        let extractor = CorpusTfIdf::new(&idf);
        let keywords = extractor.extract("rare common common common");

        assert_eq!(keywords.len(), 2);
        // "rare" has higher IDF so should rank first despite lower frequency
        assert_eq!(keywords[0].term, "rare");
        assert_eq!(keywords[1].term, "common");
    }

    #[test]
    fn extract_filters_unknown_terms() {
        let idf = MockIdf::new().with_term("kubernetes", 1.0);

        let extractor = CorpusTfIdf::new(&idf);
        let keywords = extractor.extract("kubernetes terraform");

        // "terraform" is not in the IDF provider, so it should be filtered out
        assert_eq!(keywords.len(), 1);
        assert_eq!(keywords[0].term, "kubernetes");
    }

    #[test]
    fn extract_filters_stopwords() {
        let idf = MockIdf::new()
            .with_term("the", 0.1)
            .with_term("kubernetes", 5.0);

        let extractor = CorpusTfIdf::new(&idf);
        let keywords = extractor.extract("the kubernetes");

        assert_eq!(keywords.len(), 1);
        assert_eq!(keywords[0].term, "kubernetes");
    }

    #[test]
    fn extract_from_weighted_preserves_source() {
        let idf = MockIdf::new()
            .with_term("heading", 2.0)
            .with_term("body", 1.0);

        let terms = vec![
            WeightedTerm::new("heading".to_string(), "md:h1", 3.0),
            WeightedTerm::new("body".to_string(), "body", 1.0),
        ];

        let extractor = CorpusTfIdf::new(&idf);
        let keywords = extractor.extract_from_weighted(terms);

        assert_eq!(keywords.len(), 2);
        // Heading term has higher weight (3.0 × 2.0 = 6.0 vs 1.0 × 1.0 = 1.0)
        assert_eq!(keywords[0].term, "heading");
        assert_eq!(keywords[0].source, Some("md:h1".to_string()));
    }
}
