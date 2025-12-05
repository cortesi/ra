//! Term ranking using TF-IDF scoring.
//!
//! This module provides functionality to rank extracted terms by their importance
//! using a combination of:
//! - Term frequency (how often the term appears in the source)
//! - Source weight (where the term was found: heading, body, path, etc.)
//! - IDF (Inverse Document Frequency from the search index)

use std::cmp::Ordering;

use crate::WeightedTerm;

/// Computes a TF-IDF-like score for a term.
///
/// The score combines:
/// - `frequency`: How many times the term appeared in the source document
/// - `source_weight`: Weight based on where the term was found (heading > body)
/// - `idf`: Inverse document frequency from the index (rare terms score higher)
///
/// Formula: `frequency * source_weight * idf`
fn compute_score(term: &WeightedTerm, idf: f32) -> f32 {
    term.frequency as f32 * term.weight * idf
}

/// A term with its computed TF-IDF score.
#[derive(Debug, Clone)]
pub struct RankedTerm {
    /// The original weighted term.
    pub term: WeightedTerm,
    /// The IDF value from the index.
    pub idf: f32,
    /// The computed TF-IDF score.
    pub score: f32,
}

impl RankedTerm {
    /// Creates a new ranked term with computed score.
    pub fn new(term: WeightedTerm, idf: f32) -> Self {
        let score = compute_score(&term, idf);
        Self { term, idf, score }
    }
}

/// Trait for providing IDF values for terms.
///
/// This abstraction allows the ranking logic to work with different IDF sources,
/// such as a live index or cached values.
pub trait IdfProvider {
    /// Returns the IDF value for a term, or `None` if the term doesn't exist in the index.
    ///
    /// Higher values indicate rarer terms. Returns `None` for terms that don't
    /// appear in any document, which causes them to be filtered out during ranking.
    fn idf(&self, term: &str) -> Option<f32>;
}

/// Ranks terms by their TF-IDF score.
///
/// Terms are scored using `frequency * source_weight * idf` and sorted
/// in descending order (highest scores first). Terms that don't exist
/// in the index (IDF returns `None`) are filtered out.
///
/// # Arguments
/// * `terms` - The weighted terms to rank
/// * `idf_provider` - Source for IDF values
///
/// # Returns
/// Terms with computed scores, sorted by score descending.
pub fn rank_terms<P: IdfProvider>(terms: Vec<WeightedTerm>, idf_provider: &P) -> Vec<RankedTerm> {
    let mut ranked: Vec<RankedTerm> = terms
        .into_iter()
        .filter_map(|term| {
            // Only include terms that exist in the index
            let idf = idf_provider.idf(&term.term)?;
            Some(RankedTerm::new(term, idf))
        })
        .collect();

    // Sort by score descending, then by term alphabetically for stability
    ranked.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.term.term.cmp(&b.term.term))
    });

    ranked
}

/// Selects the top N ranked terms.
///
/// Convenience function that ranks terms and returns only the top results.
pub fn top_terms<P: IdfProvider>(
    terms: Vec<WeightedTerm>,
    idf_provider: &P,
    limit: usize,
) -> Vec<RankedTerm> {
    let mut ranked = rank_terms(terms, idf_provider);
    ranked.truncate(limit);
    ranked
}

#[cfg(test)]
mod test {
    use std::collections::HashMap;

    use super::*;
    use crate::TermSource;

    /// Mock IDF provider for testing.
    struct MockIdf {
        /// Map from term to IDF value.
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
    fn compute_score_basic() {
        let term = WeightedTerm {
            term: "test".to_string(),
            weight: 2.0,
            source: TermSource::MarkdownH1,
            frequency: 3,
        };

        let idf = 1.5;
        let score = compute_score(&term, idf);

        // 3 * 2.0 * 1.5 = 9.0
        assert!((score - 9.0).abs() < 0.001);
    }

    #[test]
    fn rank_terms_sorts_by_score() {
        let terms = vec![
            WeightedTerm::new("low".to_string(), TermSource::Body), // weight 1.0
            WeightedTerm::new("high".to_string(), TermSource::MarkdownH1), // weight 3.0
            WeightedTerm::new("medium".to_string(), TermSource::MarkdownH2H3), // weight 2.0
        ];

        // All terms have same IDF, so ranking is by weight
        let idf = MockIdf::new()
            .with_term("low", 1.0)
            .with_term("high", 1.0)
            .with_term("medium", 1.0);

        let ranked = rank_terms(terms, &idf);

        assert_eq!(ranked[0].term.term, "high");
        assert_eq!(ranked[1].term.term, "medium");
        assert_eq!(ranked[2].term.term, "low");
    }

    #[test]
    fn rank_terms_considers_idf() {
        let terms = vec![
            WeightedTerm::new("common".to_string(), TermSource::Body), // weight 1.0
            WeightedTerm::new("rare".to_string(), TermSource::Body),   // weight 1.0
        ];

        // "rare" has higher IDF, so should rank higher despite same weight
        let idf = MockIdf::new()
            .with_term("common", 1.0)
            .with_term("rare", 5.0);

        let ranked = rank_terms(terms, &idf);

        assert_eq!(ranked[0].term.term, "rare");
        assert_eq!(ranked[1].term.term, "common");
    }

    #[test]
    fn rank_terms_considers_frequency() {
        let mut frequent = WeightedTerm::new("frequent".to_string(), TermSource::Body);
        frequent.frequency = 5;

        let mut infrequent = WeightedTerm::new("infrequent".to_string(), TermSource::Body);
        infrequent.frequency = 1;

        let terms = vec![infrequent, frequent];

        let idf = MockIdf::new()
            .with_term("frequent", 1.0)
            .with_term("infrequent", 1.0);

        let ranked = rank_terms(terms, &idf);

        assert_eq!(ranked[0].term.term, "frequent");
        assert_eq!(ranked[1].term.term, "infrequent");
    }

    #[test]
    fn rank_terms_stable_ordering() {
        // Two terms with identical scores should be ordered alphabetically
        let terms = vec![
            WeightedTerm::new("zebra".to_string(), TermSource::Body),
            WeightedTerm::new("alpha".to_string(), TermSource::Body),
        ];

        let idf = MockIdf::new()
            .with_term("zebra", 1.0)
            .with_term("alpha", 1.0);

        let ranked = rank_terms(terms, &idf);

        // Same score, so alphabetical order
        assert_eq!(ranked[0].term.term, "alpha");
        assert_eq!(ranked[1].term.term, "zebra");
    }

    #[test]
    fn top_terms_limits_results() {
        let terms = vec![
            WeightedTerm::new("a".to_string(), TermSource::Body),
            WeightedTerm::new("b".to_string(), TermSource::Body),
            WeightedTerm::new("c".to_string(), TermSource::Body),
            WeightedTerm::new("d".to_string(), TermSource::Body),
            WeightedTerm::new("e".to_string(), TermSource::Body),
        ];

        let idf = MockIdf::new()
            .with_term("a", 1.0)
            .with_term("b", 1.0)
            .with_term("c", 1.0)
            .with_term("d", 1.0)
            .with_term("e", 1.0);
        let top = top_terms(terms, &idf, 3);

        assert_eq!(top.len(), 3);
    }

    #[test]
    fn unknown_terms_are_filtered_out() {
        let terms = vec![
            WeightedTerm::new("known".to_string(), TermSource::Body),
            WeightedTerm::new("unknown".to_string(), TermSource::Body),
        ];

        // Only "known" is in the index
        let idf = MockIdf::new().with_term("known", 1.0);

        let ranked = rank_terms(terms, &idf);

        // Unknown term should be filtered out entirely
        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].term.term, "known");
    }

    #[test]
    fn ranked_term_preserves_original() {
        let original = WeightedTerm {
            term: "test".to_string(),
            weight: 3.0,
            source: TermSource::MarkdownH1,
            frequency: 2,
        };

        let ranked = RankedTerm::new(original, 1.5);

        assert_eq!(ranked.term.term, "test");
        assert_eq!(ranked.term.weight, 3.0);
        assert_eq!(ranked.term.source, TermSource::MarkdownH1);
        assert_eq!(ranked.term.frequency, 2);
        assert_eq!(ranked.idf, 1.5);
    }
}
