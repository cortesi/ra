//! Query construction from ranked terms.
//!
//! This module builds a weighted OR query from the top-ranked terms.
//! Each term is boosted by its TF-IDF score to prioritize more relevant terms.

use ra_query::QueryExpr;

use crate::rank::RankedTerm;

/// Default number of terms to include in the query.
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
}

impl ContextQuery {
    /// Returns true if the query is empty (no terms).
    pub fn is_empty(&self) -> bool {
        self.included_terms.is_empty()
    }

    /// Returns the number of terms in the query.
    pub fn len(&self) -> usize {
        self.included_terms.len()
    }
}

/// Builds a context query from ranked terms.
///
/// The query is a weighted OR of all terms, with each term boosted by its
/// TF-IDF score. This prioritizes rare, important terms while still matching
/// on common ones.
///
/// # Arguments
/// * `terms` - Ranked terms sorted by score descending
/// * `limit` - Maximum number of terms to include
///
/// # Returns
/// A `ContextQuery` containing the query expression and metadata,
/// or `None` if there are no terms to include.
pub fn build_query(terms: Vec<RankedTerm>, limit: usize) -> Option<ContextQuery> {
    if terms.is_empty() {
        return None;
    }

    // Take top N terms by score (already sorted)
    let top_terms: Vec<_> = terms.into_iter().take(limit).collect();

    // Build query expressions
    let mut exprs: Vec<QueryExpr> = Vec::new();
    let mut included_terms: Vec<String> = Vec::new();

    for term in &top_terms {
        let term_expr = QueryExpr::Term(term.term.term.clone());
        let boosted = QueryExpr::boost(term_expr, term.score);
        exprs.push(boosted);
        included_terms.push(term.term.term.clone());
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
    })
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

    #[test]
    fn build_query_empty() {
        let result = build_query(Vec::new(), 10);
        assert!(result.is_none());
    }

    #[test]
    fn build_query_single_term() {
        let terms = vec![make_ranked_term("rust", 5.0)];
        let result = build_query(terms, 10).unwrap();

        assert_eq!(result.included_terms, vec!["rust"]);
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
        let result = build_query(terms, 10).unwrap();

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
    fn build_query_respects_limit() {
        let terms: Vec<RankedTerm> = (0..20)
            .map(|i| make_ranked_term(&format!("term{i}"), 20.0 - i as f32))
            .collect();

        let result = build_query(terms, 5).unwrap();

        assert_eq!(result.included_terms.len(), 5);
        // Should have top 5 by score
        assert_eq!(result.included_terms[0], "term0");
        assert_eq!(result.included_terms[4], "term4");
    }

    #[test]
    fn build_query_preserves_order() {
        // Terms are already sorted by score in rank_terms
        let terms = vec![
            make_ranked_term("high", 10.0),
            make_ranked_term("medium", 5.0),
            make_ranked_term("low", 1.0),
        ];

        let result = build_query(terms, 10).unwrap();

        // Should preserve order (already sorted by score descending)
        assert_eq!(result.included_terms[0], "high");
        assert_eq!(result.included_terms[1], "medium");
        assert_eq!(result.included_terms[2], "low");
    }

    #[test]
    fn build_query_generates_query_string() {
        let terms = vec![make_ranked_term("rust", 5.0)];
        let result = build_query(terms, 10).unwrap();

        // Should have a non-empty query string
        assert!(!result.query_string.is_empty());
        assert!(result.query_string.contains("rust"));
    }

    #[test]
    fn context_query_is_empty() {
        let terms = vec![make_ranked_term("rust", 5.0)];
        let result = build_query(terms, 10).unwrap();
        assert!(!result.is_empty());
    }
}
