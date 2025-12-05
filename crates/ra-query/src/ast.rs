//! Query abstract syntax tree.
//!
//! Represents parsed query expressions before compilation to search engine queries.

use std::fmt;

/// A parsed query expression.
#[derive(Debug, Clone, PartialEq)]
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
        /// Field name (e.g., title, tags, body, path, tree).
        name: String,
        /// Expression to match within that field.
        expr: Box<Self>,
    },

    /// Boosted query: multiplies the score of the inner expression.
    Boost {
        /// The expression to boost.
        expr: Box<Self>,
        /// The boost factor (e.g., 2.5 means 2.5x the normal score).
        factor: f32,
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

    /// Creates a boosted expression.
    pub fn boost(expr: Self, factor: f32) -> Self {
        Self::Boost {
            expr: Box::new(expr),
            factor,
        }
    }

    /// Formats the expression as a tree structure with the given indentation level.
    fn fmt_tree(&self, f: &mut fmt::Formatter<'_>, indent: usize) -> fmt::Result {
        let prefix = "  ".repeat(indent);
        match self {
            Self::Term(s) => writeln!(f, "{prefix}Term({s:?})"),
            Self::Phrase(words) => writeln!(f, "{prefix}Phrase({words:?})"),
            Self::Not(inner) => {
                writeln!(f, "{prefix}Not")?;
                inner.fmt_tree(f, indent + 1)
            }
            Self::And(exprs) => {
                writeln!(f, "{prefix}And")?;
                for expr in exprs {
                    expr.fmt_tree(f, indent + 1)?;
                }
                Ok(())
            }
            Self::Or(exprs) => {
                writeln!(f, "{prefix}Or")?;
                for expr in exprs {
                    expr.fmt_tree(f, indent + 1)?;
                }
                Ok(())
            }
            Self::Field { name, expr } => {
                writeln!(f, "{prefix}Field({name:?})")?;
                expr.fmt_tree(f, indent + 1)
            }
            Self::Boost { expr, factor } => {
                writeln!(f, "{prefix}Boost({factor})")?;
                expr.fmt_tree(f, indent + 1)
            }
        }
    }

    /// Formats the expression as a query string (human-readable form).
    ///
    /// This produces output like: `term^2.5 OR "phrase"^3.0`
    pub fn to_query_string(&self) -> String {
        self.fmt_query_string(false)
    }

    /// Internal helper for query string formatting.
    fn fmt_query_string(&self, in_field: bool) -> String {
        match self {
            Self::Term(s) => s.clone(),
            Self::Phrase(words) => format!("\"{}\"", words.join(" ")),
            Self::Not(inner) => format!("-{}", inner.fmt_query_string(in_field)),
            Self::And(exprs) => {
                if exprs.is_empty() {
                    String::new()
                } else {
                    let parts: Vec<String> =
                        exprs.iter().map(|e| e.fmt_query_string(in_field)).collect();
                    if in_field {
                        format!("({})", parts.join(" "))
                    } else {
                        parts.join(" ")
                    }
                }
            }
            Self::Or(exprs) => {
                if exprs.is_empty() {
                    String::new()
                } else {
                    let parts: Vec<String> =
                        exprs.iter().map(|e| e.fmt_query_string(in_field)).collect();
                    if in_field || exprs.len() > 1 {
                        format!("({})", parts.join(" OR "))
                    } else {
                        parts.join(" OR ")
                    }
                }
            }
            Self::Field { name, expr } => {
                format!("{}:{}", name, expr.fmt_query_string(true))
            }
            Self::Boost { expr, factor } => {
                format!("{}^{}", expr.fmt_query_string(in_field), factor)
            }
        }
    }
}

impl fmt::Display for QueryExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.fmt_tree(f, 0)
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
