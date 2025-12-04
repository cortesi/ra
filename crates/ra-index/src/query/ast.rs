//! Query abstract syntax tree.
//!
//! Represents parsed query expressions before compilation to Tantivy queries.

/// A parsed query expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryExpr {
    /// A single search term.
    Term(String),

    /// An exact phrase (sequence of terms).
    Phrase(Vec<String>),

    /// Negation: results must NOT match this expression.
    Not(Box<Self>),

    /// Conjunction: all sub-expressions must match.
    And(Vec<Self>),

    /// Disjunction: at least one sub-expression must match.
    Or(Vec<Self>),

    /// Field-scoped query: search only within a specific field.
    Field {
        /// Field name (title, tags, body, path, tree).
        name: String,
        /// Expression to match within that field.
        expr: Box<Self>,
    },
}

impl QueryExpr {
    /// Creates an And expression, flattening nested Ands.
    pub fn and(exprs: Vec<Self>) -> Self {
        let flattened: Vec<Self> = exprs
            .into_iter()
            .flat_map(|e| match e {
                Self::And(inner) => inner,
                other => vec![other],
            })
            .collect();

        match flattened.len() {
            0 => Self::And(vec![]),
            1 => flattened.into_iter().next().unwrap(),
            _ => Self::And(flattened),
        }
    }

    /// Creates an Or expression, flattening nested Ors.
    pub fn or(exprs: Vec<Self>) -> Self {
        let flattened: Vec<Self> = exprs
            .into_iter()
            .flat_map(|e| match e {
                Self::Or(inner) => inner,
                other => vec![other],
            })
            .collect();

        match flattened.len() {
            0 => Self::Or(vec![]),
            1 => flattened.into_iter().next().unwrap(),
            _ => Self::Or(flattened),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn and_flattens_nested() {
        let nested = QueryExpr::and(vec![
            QueryExpr::Term("a".into()),
            QueryExpr::And(vec![
                QueryExpr::Term("b".into()),
                QueryExpr::Term("c".into()),
            ]),
        ]);

        assert_eq!(
            nested,
            QueryExpr::And(vec![
                QueryExpr::Term("a".into()),
                QueryExpr::Term("b".into()),
                QueryExpr::Term("c".into()),
            ])
        );
    }

    #[test]
    fn and_single_element_unwraps() {
        let single = QueryExpr::and(vec![QueryExpr::Term("a".into())]);
        assert_eq!(single, QueryExpr::Term("a".into()));
    }

    #[test]
    fn or_flattens_nested() {
        let nested = QueryExpr::or(vec![
            QueryExpr::Term("a".into()),
            QueryExpr::Or(vec![
                QueryExpr::Term("b".into()),
                QueryExpr::Term("c".into()),
            ]),
        ]);

        assert_eq!(
            nested,
            QueryExpr::Or(vec![
                QueryExpr::Term("a".into()),
                QueryExpr::Term("b".into()),
                QueryExpr::Term("c".into()),
            ])
        );
    }

    #[test]
    fn or_single_element_unwraps() {
        let single = QueryExpr::or(vec![QueryExpr::Term("a".into())]);
        assert_eq!(single, QueryExpr::Term("a".into()));
    }
}
