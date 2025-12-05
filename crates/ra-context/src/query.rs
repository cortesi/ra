//! Query construction from ranked terms and phrases.
//!
//! This module builds a weighted OR query from the top-ranked terms and phrases.
//! Each term/phrase is boosted by its TF-IDF score to prioritize more relevant terms.

use std::cmp::Ordering;

use ra_query::QueryExpr;

use crate::{phrase::ValidatedPhrase, rank::RankedTerm};

/// Default number of terms/phrases to include in the query.
pub const DEFAULT_TERM_LIMIT: usize = 15;

/// A constructed context query with metadata.
#[derive(Debug, Clone)]
pub struct ContextQuery {
    /// The generated query expression.
    pub expr: QueryExpr,
    /// Human-readable query string for display.
    pub query_string: String,
    /// Terms that were included in the query.
    pub included_terms: Vec<String>,
    /// Phrases that were included in the query.
    pub included_phrases: Vec<String>,
}

impl ContextQuery {
    /// Returns true if the query is empty (no terms or phrases).
    pub fn is_empty(&self) -> bool {
        self.included_terms.is_empty() && self.included_phrases.is_empty()
    }

    /// Returns the total number of terms and phrases in the query.
    pub fn len(&self) -> usize {
        self.included_terms.len() + self.included_phrases.len()
    }
}

/// Builds a context query from ranked terms and validated phrases.
///
/// The query is a weighted OR of all terms and phrases, with each element
/// boosted by its TF-IDF score. This prioritizes rare, important terms
/// while still matching on common ones.
///
/// # Arguments
/// * `terms` - Ranked terms sorted by score descending
/// * `phrases` - Validated phrases with their scores
/// * `limit` - Maximum number of terms/phrases to include
///
/// # Returns
/// A `ContextQuery` containing the query expression and metadata,
/// or `None` if there are no terms or phrases to include.
pub fn build_query(
    terms: Vec<RankedTerm>,
    phrases: Vec<ValidatedPhrase>,
    limit: usize,
) -> Option<ContextQuery> {
    // Combine terms and phrases into a single scored list
    let mut scored_items: Vec<ScoredItem> = Vec::new();

    for term in terms {
        scored_items.push(ScoredItem::Term {
            text: term.term.term.clone(),
            score: term.score,
        });
    }

    for phrase in phrases {
        scored_items.push(ScoredItem::Phrase {
            words: phrase.words.clone(),
            score: phrase.score,
        });
    }

    // Sort by score descending
    scored_items.sort_by(|a, b| b.score().partial_cmp(&a.score()).unwrap_or(Ordering::Equal));

    // Take top N items
    scored_items.truncate(limit);

    if scored_items.is_empty() {
        return None;
    }

    // Build query expressions
    let mut exprs: Vec<QueryExpr> = Vec::new();
    let mut included_terms: Vec<String> = Vec::new();
    let mut included_phrases: Vec<String> = Vec::new();

    for item in &scored_items {
        match item {
            ScoredItem::Term { text, score } => {
                let term_expr = QueryExpr::Term(text.clone());
                let boosted = QueryExpr::boost(term_expr, *score);
                exprs.push(boosted);
                included_terms.push(text.clone());
            }
            ScoredItem::Phrase { words, score } => {
                let phrase_expr = QueryExpr::Phrase(words.clone());
                let boosted = QueryExpr::boost(phrase_expr, *score);
                exprs.push(boosted);
                included_phrases.push(words.join(" "));
            }
        }
    }

    // Build OR query
    let expr = if exprs.len() == 1 {
        exprs.remove(0)
    } else {
        QueryExpr::or(exprs)
    };

    // Generate human-readable query string
    let query_string = expr.to_query_string();

    Some(ContextQuery {
        expr,
        query_string,
        included_terms,
        included_phrases,
    })
}

/// A scored item (term or phrase) for sorting.
#[derive(Debug, Clone)]
enum ScoredItem {
    Term { text: String, score: f32 },
    Phrase { words: Vec<String>, score: f32 },
}

impl ScoredItem {
    fn score(&self) -> f32 {
        match self {
            Self::Term { score, .. } => *score,
            Self::Phrase { score, .. } => *score,
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::{TermSource, WeightedTerm};

    fn make_ranked_term(term: &str, score: f32) -> RankedTerm {
        RankedTerm {
            term: WeightedTerm::new(term.to_string(), TermSource::Body),
            idf: 1.0,
            score,
        }
    }

    fn make_phrase(words: &[&str], score: f32) -> ValidatedPhrase {
        ValidatedPhrase {
            words: words.iter().map(|s| s.to_string()).collect(),
            score,
        }
    }

    #[test]
    fn build_query_empty() {
        let result = build_query(Vec::new(), Vec::new(), 10);
        assert!(result.is_none());
    }

    #[test]
    fn build_query_single_term() {
        let terms = vec![make_ranked_term("rust", 5.0)];
        let result = build_query(terms, Vec::new(), 10).unwrap();

        assert_eq!(result.included_terms, vec!["rust"]);
        assert!(result.included_phrases.is_empty());
        assert!(!result.is_empty());
        assert_eq!(result.len(), 1);

        // Should be a boosted term
        match &result.expr {
            QueryExpr::Boost { expr, factor } => {
                assert_eq!(**expr, QueryExpr::Term("rust".to_string()));
                assert!((factor - 5.0).abs() < 0.001);
            }
            _ => panic!("expected Boost expression"),
        }
    }

    #[test]
    fn build_query_multiple_terms() {
        let terms = vec![
            make_ranked_term("rust", 5.0),
            make_ranked_term("async", 3.0),
            make_ranked_term("tokio", 2.0),
        ];
        let result = build_query(terms, Vec::new(), 10).unwrap();

        assert_eq!(result.included_terms.len(), 3);
        assert_eq!(result.len(), 3);

        // Should be an OR of boosted terms
        match &result.expr {
            QueryExpr::Or(exprs) => {
                assert_eq!(exprs.len(), 3);
            }
            _ => panic!("expected Or expression"),
        }
    }

    #[test]
    fn build_query_with_phrases() {
        let terms = vec![make_ranked_term("rust", 5.0)];
        let phrases = vec![make_phrase(&["machine", "learning"], 8.0)];

        let result = build_query(terms, phrases, 10).unwrap();

        assert_eq!(result.included_terms, vec!["rust"]);
        assert_eq!(result.included_phrases, vec!["machine learning"]);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn build_query_respects_limit() {
        let terms: Vec<RankedTerm> = (0..20)
            .map(|i| make_ranked_term(&format!("term{i}"), 20.0 - i as f32))
            .collect();

        let result = build_query(terms, Vec::new(), 5).unwrap();

        assert_eq!(result.included_terms.len(), 5);
        // Should have top 5 by score
        assert_eq!(result.included_terms[0], "term0");
        assert_eq!(result.included_terms[4], "term4");
    }

    #[test]
    fn build_query_sorts_by_score() {
        let terms = vec![
            make_ranked_term("low", 1.0),
            make_ranked_term("high", 10.0),
            make_ranked_term("medium", 5.0),
        ];

        let result = build_query(terms, Vec::new(), 10).unwrap();

        // Should be sorted by score descending
        assert_eq!(result.included_terms[0], "high");
        assert_eq!(result.included_terms[1], "medium");
        assert_eq!(result.included_terms[2], "low");
    }

    #[test]
    fn build_query_phrases_compete_with_terms() {
        // Phrase has higher score than some terms
        let terms = vec![make_ranked_term("high", 10.0), make_ranked_term("low", 1.0)];
        let phrases = vec![make_phrase(&["machine", "learning"], 5.0)];

        let result = build_query(terms, phrases, 10).unwrap();

        // Order should be: high (10), machine learning (5), low (1)
        assert_eq!(result.included_terms[0], "high");
        assert_eq!(result.included_phrases[0], "machine learning");
        assert_eq!(result.included_terms[1], "low");
    }

    #[test]
    fn build_query_generates_query_string() {
        let terms = vec![make_ranked_term("rust", 5.0)];
        let result = build_query(terms, Vec::new(), 10).unwrap();

        // Should have a non-empty query string
        assert!(!result.query_string.is_empty());
        assert!(result.query_string.contains("rust"));
    }

    #[test]
    fn build_query_phrase_in_query_string() {
        let phrases = vec![make_phrase(&["machine", "learning"], 5.0)];
        let result = build_query(Vec::new(), phrases, 10).unwrap();

        // Query string should contain quoted phrase
        assert!(result.query_string.contains("\"machine learning\""));
    }

    #[test]
    fn context_query_is_empty() {
        let terms = vec![make_ranked_term("rust", 5.0)];
        let result = build_query(terms, Vec::new(), 10).unwrap();
        assert!(!result.is_empty());
    }
}
