//! Phrase detection and validation.
//!
//! This module extracts candidate phrases (bigrams and trigrams) from ranked terms
//! and validates them against the search index. Validated phrases can replace their
//! constituent terms in the final query for more precise matching.

use std::{cmp::Ordering, collections::HashSet};

use crate::rank::RankedTerm;

/// A candidate phrase extracted from adjacent terms.
#[derive(Debug, Clone)]
pub struct CandidatePhrase {
    /// The words that make up the phrase.
    pub words: Vec<String>,
    /// Indices of the constituent terms in the original ranked list.
    pub term_indices: Vec<usize>,
    /// Combined score from constituent terms.
    pub score: f32,
}

impl CandidatePhrase {
    /// Returns the phrase as a space-separated string.
    pub fn as_string(&self) -> String {
        self.words.join(" ")
    }

    /// Returns the phrase words as string slices.
    pub fn as_slices(&self) -> Vec<&str> {
        self.words.iter().map(|s| s.as_str()).collect()
    }
}

/// A validated phrase that exists in the index.
#[derive(Debug, Clone)]
pub struct ValidatedPhrase {
    /// The words that make up the phrase.
    pub words: Vec<String>,
    /// Combined score from constituent terms.
    pub score: f32,
}

impl ValidatedPhrase {
    /// Returns the phrase as a space-separated string.
    pub fn as_string(&self) -> String {
        self.words.join(" ")
    }
}

/// Extracts candidate bigrams from adjacent high-scoring terms.
///
/// Terms are considered "adjacent" based on their original document position,
/// which we approximate by considering consecutive terms in the ranked list
/// that came from the same source type.
///
/// # Arguments
/// * `terms` - Ranked terms sorted by score descending
/// * `max_candidates` - Maximum number of bigram candidates to generate
pub fn extract_bigrams(terms: &[RankedTerm], max_candidates: usize) -> Vec<CandidatePhrase> {
    if terms.len() < 2 {
        return Vec::new();
    }

    let mut candidates = Vec::new();

    // Generate bigrams from consecutive pairs in the ranked list
    // We limit to top terms since low-scoring terms are unlikely to form useful phrases
    let limit = terms.len().min(max_candidates * 2);

    for i in 0..limit.saturating_sub(1) {
        for j in (i + 1)..limit {
            // Only consider terms from compatible sources (both from headings or both from body)
            if are_compatible_sources(&terms[i], &terms[j]) {
                let score = terms[i].score + terms[j].score;
                candidates.push(CandidatePhrase {
                    words: vec![terms[i].term.term.clone(), terms[j].term.term.clone()],
                    term_indices: vec![i, j],
                    score,
                });
            }
        }
    }

    // Sort by score descending and limit
    candidates.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(Ordering::Equal));
    candidates.truncate(max_candidates);

    candidates
}

/// Extracts candidate trigrams from high-scoring terms.
///
/// # Arguments
/// * `terms` - Ranked terms sorted by score descending
/// * `max_candidates` - Maximum number of trigram candidates to generate
pub fn extract_trigrams(terms: &[RankedTerm], max_candidates: usize) -> Vec<CandidatePhrase> {
    if terms.len() < 3 {
        return Vec::new();
    }

    let mut candidates = Vec::new();

    // Generate trigrams from top terms
    let limit = terms.len().min(max_candidates * 2);

    for i in 0..limit.saturating_sub(2) {
        for j in (i + 1)..limit.saturating_sub(1) {
            for k in (j + 1)..limit {
                // Only consider terms from compatible sources
                if are_compatible_sources(&terms[i], &terms[j])
                    && are_compatible_sources(&terms[j], &terms[k])
                {
                    let score = terms[i].score + terms[j].score + terms[k].score;
                    candidates.push(CandidatePhrase {
                        words: vec![
                            terms[i].term.term.clone(),
                            terms[j].term.term.clone(),
                            terms[k].term.term.clone(),
                        ],
                        term_indices: vec![i, j, k],
                        score,
                    });
                }
            }
        }
    }

    // Sort by score descending and limit
    candidates.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(Ordering::Equal));
    candidates.truncate(max_candidates);

    candidates
}

/// Checks if two terms come from compatible sources for phrase formation.
///
/// Terms from headings can form phrases with other heading terms.
/// Terms from body can form phrases with other body terms.
/// Path terms generally don't form phrases.
fn are_compatible_sources(a: &RankedTerm, b: &RankedTerm) -> bool {
    use crate::TermSource;

    match (&a.term.source, &b.term.source) {
        // Heading terms can combine with each other
        (TermSource::MarkdownH1, TermSource::MarkdownH1)
        | (TermSource::MarkdownH1, TermSource::MarkdownH2H3)
        | (TermSource::MarkdownH1, TermSource::MarkdownH4H6)
        | (TermSource::MarkdownH2H3, TermSource::MarkdownH1)
        | (TermSource::MarkdownH2H3, TermSource::MarkdownH2H3)
        | (TermSource::MarkdownH2H3, TermSource::MarkdownH4H6)
        | (TermSource::MarkdownH4H6, TermSource::MarkdownH1)
        | (TermSource::MarkdownH4H6, TermSource::MarkdownH2H3)
        | (TermSource::MarkdownH4H6, TermSource::MarkdownH4H6) => true,

        // Body terms can combine with each other
        (TermSource::Body, TermSource::Body) => true,

        // Path terms don't form phrases
        _ => false,
    }
}

/// Trait for validating phrases against a search index.
pub trait PhraseValidator {
    /// Checks if a phrase exists in the index.
    fn phrase_exists(&self, phrase: &[&str]) -> bool;
}

/// Validates candidate phrases against the index.
///
/// Returns only phrases that actually exist in at least one indexed document.
pub fn validate_phrases<V: PhraseValidator>(
    candidates: Vec<CandidatePhrase>,
    validator: &V,
) -> Vec<ValidatedPhrase> {
    candidates
        .into_iter()
        .filter(|candidate| {
            let slices = candidate.as_slices();
            validator.phrase_exists(&slices)
        })
        .map(|candidate| ValidatedPhrase {
            words: candidate.words,
            score: candidate.score,
        })
        .collect()
}

/// Result of phrase promotion: terms with some replaced by validated phrases.
#[derive(Debug, Clone)]
pub struct PromotedTerms {
    /// Terms that were not consumed by any phrase.
    pub remaining_terms: Vec<RankedTerm>,
    /// Validated phrases that replace some terms.
    pub phrases: Vec<ValidatedPhrase>,
}

/// Promotes validated phrases by removing their constituent terms.
///
/// When a phrase is validated, its constituent terms are removed from the
/// term list and the phrase is added instead. This prevents double-counting
/// in the final query.
///
/// # Arguments
/// * `terms` - Original ranked terms
/// * `phrases` - Validated phrases to promote
/// * `max_phrases` - Maximum number of phrases to include
pub fn promote_phrases(
    terms: Vec<RankedTerm>,
    phrases: Vec<ValidatedPhrase>,
    max_phrases: usize,
) -> PromotedTerms {
    if phrases.is_empty() {
        return PromotedTerms {
            remaining_terms: terms,
            phrases: Vec::new(),
        };
    }

    // Take top phrases by score
    let mut sorted_phrases = phrases;
    sorted_phrases.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(Ordering::Equal));
    sorted_phrases.truncate(max_phrases);

    // Collect all words consumed by phrases
    let consumed_words: HashSet<&str> = sorted_phrases
        .iter()
        .flat_map(|p| p.words.iter().map(|s| s.as_str()))
        .collect();

    // Filter out terms that are consumed by phrases
    let remaining_terms: Vec<RankedTerm> = terms
        .into_iter()
        .filter(|t| !consumed_words.contains(t.term.term.as_str()))
        .collect();

    PromotedTerms {
        remaining_terms,
        phrases: sorted_phrases,
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::{TermSource, WeightedTerm};

    fn make_ranked_term(term: &str, source: TermSource, score: f32) -> RankedTerm {
        RankedTerm {
            term: WeightedTerm::new(term.to_string(), source),
            idf: 1.0,
            score,
        }
    }

    #[test]
    fn extract_bigrams_basic() {
        let terms = vec![
            make_ranked_term("machine", TermSource::Body, 5.0),
            make_ranked_term("learning", TermSource::Body, 4.0),
            make_ranked_term("deep", TermSource::Body, 3.0),
        ];

        let bigrams = extract_bigrams(&terms, 10);

        assert!(!bigrams.is_empty());
        // Should have combinations of compatible terms
        assert!(
            bigrams
                .iter()
                .any(|b| b.words == vec!["machine", "learning"])
        );
    }

    #[test]
    fn extract_bigrams_respects_sources() {
        let terms = vec![
            make_ranked_term("heading", TermSource::MarkdownH1, 5.0),
            make_ranked_term("body", TermSource::Body, 4.0),
        ];

        let bigrams = extract_bigrams(&terms, 10);

        // Heading and body terms shouldn't form phrases
        assert!(bigrams.is_empty());
    }

    #[test]
    fn extract_bigrams_limits_candidates() {
        let terms: Vec<RankedTerm> = (0..20)
            .map(|i| make_ranked_term(&format!("term{i}"), TermSource::Body, 10.0 - i as f32))
            .collect();

        let bigrams = extract_bigrams(&terms, 5);

        assert_eq!(bigrams.len(), 5);
    }

    #[test]
    fn extract_trigrams_basic() {
        let terms = vec![
            make_ranked_term("natural", TermSource::Body, 5.0),
            make_ranked_term("language", TermSource::Body, 4.0),
            make_ranked_term("processing", TermSource::Body, 3.0),
        ];

        let trigrams = extract_trigrams(&terms, 10);

        assert!(!trigrams.is_empty());
        assert!(
            trigrams
                .iter()
                .any(|t| t.words == vec!["natural", "language", "processing"])
        );
    }

    #[test]
    fn extract_trigrams_needs_three_terms() {
        let terms = vec![
            make_ranked_term("one", TermSource::Body, 5.0),
            make_ranked_term("two", TermSource::Body, 4.0),
        ];

        let trigrams = extract_trigrams(&terms, 10);

        assert!(trigrams.is_empty());
    }

    #[test]
    fn candidate_phrase_as_string() {
        let phrase = CandidatePhrase {
            words: vec!["machine".to_string(), "learning".to_string()],
            term_indices: vec![0, 1],
            score: 9.0,
        };

        assert_eq!(phrase.as_string(), "machine learning");
    }

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
    fn validate_phrases_filters_invalid() {
        let candidates = vec![
            CandidatePhrase {
                words: vec!["machine".to_string(), "learning".to_string()],
                term_indices: vec![0, 1],
                score: 9.0,
            },
            CandidatePhrase {
                words: vec!["foo".to_string(), "bar".to_string()],
                term_indices: vec![2, 3],
                score: 5.0,
            },
        ];

        let validator = MockValidator::new().with_phrase(&["machine", "learning"]);

        let validated = validate_phrases(candidates, &validator);

        assert_eq!(validated.len(), 1);
        assert_eq!(validated[0].words, vec!["machine", "learning"]);
    }

    #[test]
    fn promote_phrases_removes_consumed_terms() {
        let terms = vec![
            make_ranked_term("machine", TermSource::Body, 5.0),
            make_ranked_term("learning", TermSource::Body, 4.0),
            make_ranked_term("deep", TermSource::Body, 3.0),
        ];

        let phrases = vec![ValidatedPhrase {
            words: vec!["machine".to_string(), "learning".to_string()],
            score: 9.0,
        }];

        let promoted = promote_phrases(terms, phrases, 5);

        // "machine" and "learning" should be removed
        assert_eq!(promoted.remaining_terms.len(), 1);
        assert_eq!(promoted.remaining_terms[0].term.term, "deep");

        // Phrase should be included
        assert_eq!(promoted.phrases.len(), 1);
        assert_eq!(promoted.phrases[0].words, vec!["machine", "learning"]);
    }

    #[test]
    fn promote_phrases_limits_count() {
        let terms = vec![
            make_ranked_term("a", TermSource::Body, 5.0),
            make_ranked_term("b", TermSource::Body, 4.0),
            make_ranked_term("c", TermSource::Body, 3.0),
            make_ranked_term("d", TermSource::Body, 2.0),
        ];

        let phrases = vec![
            ValidatedPhrase {
                words: vec!["a".to_string(), "b".to_string()],
                score: 9.0,
            },
            ValidatedPhrase {
                words: vec!["c".to_string(), "d".to_string()],
                score: 5.0,
            },
        ];

        let promoted = promote_phrases(terms, phrases, 1);

        // Only top phrase should be included
        assert_eq!(promoted.phrases.len(), 1);
        assert_eq!(promoted.phrases[0].words, vec!["a", "b"]);

        // Only c and d should remain (a and b consumed by top phrase)
        assert_eq!(promoted.remaining_terms.len(), 2);
    }

    #[test]
    fn promote_phrases_empty_phrases() {
        let terms = vec![
            make_ranked_term("foo", TermSource::Body, 5.0),
            make_ranked_term("bar", TermSource::Body, 4.0),
        ];

        let promoted = promote_phrases(terms, Vec::new(), 5);

        assert_eq!(promoted.remaining_terms.len(), 2);
        assert!(promoted.phrases.is_empty());
    }

    #[test]
    fn heading_terms_form_phrases() {
        let terms = vec![
            make_ranked_term("api", TermSource::MarkdownH1, 5.0),
            make_ranked_term("reference", TermSource::MarkdownH2H3, 4.0),
        ];

        let bigrams = extract_bigrams(&terms, 10);

        // H1 and H2H3 should be compatible
        assert!(!bigrams.is_empty());
    }

    #[test]
    fn path_terms_dont_form_phrases() {
        let terms = vec![
            make_ranked_term("src", TermSource::PathDirectory, 5.0),
            make_ranked_term("main", TermSource::PathFilename, 4.0),
        ];

        let bigrams = extract_bigrams(&terms, 10);

        // Path terms shouldn't form phrases
        assert!(bigrams.is_empty());
    }
}
