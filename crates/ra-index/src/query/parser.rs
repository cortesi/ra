//! Query parser.
//!
//! Parses a token stream into a query AST using recursive descent.
//!
//! # Grammar
//!
//! ```text
//! query      → or_expr
//! or_expr    → and_expr ("OR" and_expr)*
//! and_expr   → unary+
//! unary      → "-" unary | primary
//! primary    → TERM | PHRASE | field_expr | "(" or_expr ")"
//! field_expr → FIELD_PREFIX (TERM | PHRASE | "(" or_expr ")")
//! ```
//!
//! # Precedence (highest to lowest)
//!
//! 1. Grouping: `(...)`
//! 2. Field prefix: `field:`
//! 3. Negation: `-`
//! 4. AND (implicit, between adjacent terms)
//! 5. OR (explicit keyword)

use std::{error::Error, fmt, mem};

use super::{
    ast::QueryExpr,
    lexer::{LexError, Token, tokenize},
};

/// Parse error with position information.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    /// Error message.
    pub message: String,
    /// Token index where error occurred (if applicable).
    pub token_index: Option<usize>,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(idx) = self.token_index {
            write!(f, "at token {}: {}", idx, self.message)
        } else {
            write!(f, "{}", self.message)
        }
    }
}

impl Error for ParseError {}

impl From<LexError> for ParseError {
    fn from(err: LexError) -> Self {
        Self {
            message: err.message,
            token_index: None,
        }
    }
}

/// Recursive descent parser for query expressions.
struct Parser {
    /// Token stream to parse.
    tokens: Vec<Token>,
    /// Current position in token stream.
    position: usize,
}

impl Parser {
    /// Creates a new parser from a token stream.
    fn new(tokens: Vec<Token>) -> Self {
        Self {
            tokens,
            position: 0,
        }
    }

    /// Parses the token stream into a query expression.
    fn parse(mut self) -> Result<Option<QueryExpr>, ParseError> {
        if self.tokens.is_empty() {
            return Ok(None);
        }

        let expr = self.parse_or_expr()?;

        if self.position < self.tokens.len() {
            return Err(ParseError {
                message: format!("unexpected token: {:?}", self.tokens[self.position]),
                token_index: Some(self.position),
            });
        }

        Ok(Some(expr))
    }

    /// Parses: or_expr → and_expr ("OR" and_expr)*
    fn parse_or_expr(&mut self) -> Result<QueryExpr, ParseError> {
        let mut left = self.parse_and_expr()?;

        while self.check(&Token::Or) {
            self.advance(); // consume OR
            let right = self.parse_and_expr()?;
            left = QueryExpr::or(vec![left, right]);
        }

        Ok(left)
    }

    /// Parses: and_expr → unary+
    fn parse_and_expr(&mut self) -> Result<QueryExpr, ParseError> {
        let mut exprs = Vec::new();

        // Parse at least one unary expression
        exprs.push(self.parse_unary()?);

        // Continue parsing while we see tokens that can start a unary
        while self.can_start_unary() {
            exprs.push(self.parse_unary()?);
        }

        Ok(QueryExpr::and(exprs))
    }

    /// Checks if the current token can start a unary expression.
    fn can_start_unary(&self) -> bool {
        matches!(
            self.peek(),
            Some(Token::Term(_))
                | Some(Token::Phrase(_))
                | Some(Token::Not)
                | Some(Token::LParen)
                | Some(Token::FieldPrefix(_))
        )
    }

    /// Parses: unary → "-" unary | primary
    fn parse_unary(&mut self) -> Result<QueryExpr, ParseError> {
        if self.check(&Token::Not) {
            self.advance(); // consume -
            let expr = self.parse_unary()?;
            return Ok(QueryExpr::Not(Box::new(expr)));
        }

        self.parse_primary()
    }

    /// Parses: primary → TERM | PHRASE | field_expr | "(" or_expr ")"
    fn parse_primary(&mut self) -> Result<QueryExpr, ParseError> {
        let token = self.peek().cloned();

        match token {
            Some(Token::Term(text)) => {
                self.advance();
                Ok(QueryExpr::Term(text))
            }

            Some(Token::Phrase(text)) => {
                self.advance();
                // Split phrase into words
                let words: Vec<String> = text.split_whitespace().map(|s| s.to_string()).collect();
                if words.is_empty() {
                    // Empty phrase, treat as empty term
                    Ok(QueryExpr::Term(String::new()))
                } else {
                    Ok(QueryExpr::Phrase(words))
                }
            }

            Some(Token::FieldPrefix(name)) => {
                self.advance();
                self.parse_field_expr(name)
            }

            Some(Token::LParen) => {
                self.advance(); // consume (
                let expr = self.parse_or_expr()?;

                if !self.check(&Token::RParen) {
                    return Err(ParseError {
                        message: "expected closing parenthesis".into(),
                        token_index: Some(self.position),
                    });
                }
                self.advance(); // consume )

                Ok(expr)
            }

            Some(Token::RParen) => Err(ParseError {
                message: "unexpected closing parenthesis".into(),
                token_index: Some(self.position),
            }),

            Some(Token::Or) => Err(ParseError {
                message: "unexpected OR (needs expression before it)".into(),
                token_index: Some(self.position),
            }),

            Some(Token::Not) => {
                // This shouldn't happen as parse_unary handles Not
                Err(ParseError {
                    message: "unexpected negation".into(),
                    token_index: Some(self.position),
                })
            }

            None => Err(ParseError {
                message: "unexpected end of query".into(),
                token_index: None,
            }),
        }
    }

    /// Parses the expression after a field prefix.
    fn parse_field_expr(&mut self, name: String) -> Result<QueryExpr, ParseError> {
        let token = self.peek().cloned();

        let expr = match token {
            Some(Token::Term(text)) => {
                self.advance();
                QueryExpr::Term(text)
            }

            Some(Token::Phrase(text)) => {
                self.advance();
                let words: Vec<String> = text.split_whitespace().map(|s| s.to_string()).collect();
                if words.is_empty() {
                    QueryExpr::Term(String::new())
                } else {
                    QueryExpr::Phrase(words)
                }
            }

            Some(Token::LParen) => {
                self.advance(); // consume (
                let inner = self.parse_or_expr()?;

                if !self.check(&Token::RParen) {
                    return Err(ParseError {
                        message: "expected closing parenthesis after field expression".into(),
                        token_index: Some(self.position),
                    });
                }
                self.advance(); // consume )

                inner
            }

            _ => {
                return Err(ParseError {
                    message: format!("expected term, phrase, or group after '{}:'", name),
                    token_index: Some(self.position),
                });
            }
        };

        Ok(QueryExpr::Field {
            name,
            expr: Box::new(expr),
        })
    }

    /// Returns the current token without consuming it.
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.position)
    }

    /// Checks if the current token matches the given token.
    fn check(&self, token: &Token) -> bool {
        self.peek()
            .map(|t| mem::discriminant(t) == mem::discriminant(token))
            .unwrap_or(false)
    }

    /// Advances to the next token.
    fn advance(&mut self) {
        if self.position < self.tokens.len() {
            self.position += 1;
        }
    }
}

/// Parses a query string into an AST.
///
/// Returns `Ok(None)` for empty queries, `Ok(Some(expr))` for valid queries,
/// or `Err(ParseError)` for invalid syntax.
pub fn parse(input: &str) -> Result<Option<QueryExpr>, ParseError> {
    let tokens = tokenize(input)?;
    Parser::new(tokens).parse()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn term(s: &str) -> QueryExpr {
        QueryExpr::Term(s.into())
    }

    fn phrase(words: &[&str]) -> QueryExpr {
        QueryExpr::Phrase(words.iter().map(|s| s.to_string()).collect())
    }

    fn not(e: QueryExpr) -> QueryExpr {
        QueryExpr::Not(Box::new(e))
    }

    fn and(exprs: Vec<QueryExpr>) -> QueryExpr {
        QueryExpr::and(exprs)
    }

    fn or(exprs: Vec<QueryExpr>) -> QueryExpr {
        QueryExpr::or(exprs)
    }

    fn field(name: &str, e: QueryExpr) -> QueryExpr {
        QueryExpr::Field {
            name: name.into(),
            expr: Box::new(e),
        }
    }

    #[test]
    fn empty_query() {
        assert_eq!(parse("").unwrap(), None);
        assert_eq!(parse("   ").unwrap(), None);
    }

    #[test]
    fn single_term() {
        assert_eq!(parse("rust").unwrap(), Some(term("rust")));
    }

    #[test]
    fn multiple_terms_and() {
        assert_eq!(
            parse("rust async").unwrap(),
            Some(and(vec![term("rust"), term("async")]))
        );
    }

    #[test]
    fn three_terms_and() {
        assert_eq!(
            parse("rust async await").unwrap(),
            Some(and(vec![term("rust"), term("async"), term("await")]))
        );
    }

    #[test]
    fn quoted_phrase() {
        assert_eq!(
            parse("\"error handling\"").unwrap(),
            Some(phrase(&["error", "handling"]))
        );
    }

    #[test]
    fn phrase_with_terms() {
        assert_eq!(
            parse("rust \"error handling\"").unwrap(),
            Some(and(vec![term("rust"), phrase(&["error", "handling"])]))
        );
    }

    #[test]
    fn simple_or() {
        assert_eq!(
            parse("rust OR golang").unwrap(),
            Some(or(vec![term("rust"), term("golang")]))
        );
    }

    #[test]
    fn or_with_multiple_terms() {
        // "rust async OR golang" = (rust AND async) OR golang
        assert_eq!(
            parse("rust async OR golang").unwrap(),
            Some(or(vec![
                and(vec![term("rust"), term("async")]),
                term("golang")
            ]))
        );
    }

    #[test]
    fn chained_or() {
        assert_eq!(
            parse("rust OR golang OR python").unwrap(),
            Some(or(vec![term("rust"), term("golang"), term("python")]))
        );
    }

    #[test]
    fn simple_negation() {
        assert_eq!(parse("-deprecated").unwrap(), Some(not(term("deprecated"))));
    }

    #[test]
    fn negation_with_term() {
        assert_eq!(
            parse("rust -deprecated").unwrap(),
            Some(and(vec![term("rust"), not(term("deprecated"))]))
        );
    }

    #[test]
    fn double_negation() {
        assert_eq!(parse("--foo").unwrap(), Some(not(not(term("foo")))));
    }

    #[test]
    fn simple_grouping() {
        assert_eq!(
            parse("(rust async)").unwrap(),
            Some(and(vec![term("rust"), term("async")]))
        );
    }

    #[test]
    fn grouped_or() {
        // "(rust OR golang) async" = (rust OR golang) AND async
        assert_eq!(
            parse("(rust OR golang) async").unwrap(),
            Some(and(vec![
                or(vec![term("rust"), term("golang")]),
                term("async")
            ]))
        );
    }

    #[test]
    fn complex_grouping() {
        // "(a b) OR (c d)"
        assert_eq!(
            parse("(a b) OR (c d)").unwrap(),
            Some(or(vec![
                and(vec![term("a"), term("b")]),
                and(vec![term("c"), term("d")])
            ]))
        );
    }

    #[test]
    fn nested_groups() {
        assert_eq!(
            parse("((a OR b) c)").unwrap(),
            Some(and(vec![or(vec![term("a"), term("b")]), term("c")]))
        );
    }

    #[test]
    fn field_with_term() {
        assert_eq!(
            parse("title:guide").unwrap(),
            Some(field("title", term("guide")))
        );
    }

    #[test]
    fn field_with_phrase() {
        assert_eq!(
            parse("title:\"getting started\"").unwrap(),
            Some(field("title", phrase(&["getting", "started"])))
        );
    }

    #[test]
    fn field_with_group() {
        assert_eq!(
            parse("title:(rust OR golang)").unwrap(),
            Some(field("title", or(vec![term("rust"), term("golang")])))
        );
    }

    #[test]
    fn field_with_other_terms() {
        assert_eq!(
            parse("title:guide rust").unwrap(),
            Some(and(vec![field("title", term("guide")), term("rust")]))
        );
    }

    #[test]
    fn multiple_fields() {
        assert_eq!(
            parse("title:guide tags:tutorial").unwrap(),
            Some(and(vec![
                field("title", term("guide")),
                field("tags", term("tutorial"))
            ]))
        );
    }

    #[test]
    fn tree_field() {
        assert_eq!(
            parse("tree:docs").unwrap(),
            Some(field("tree", term("docs")))
        );
    }

    #[test]
    fn path_field() {
        assert_eq!(
            parse("path:api/handlers").unwrap(),
            Some(field("path", term("api/handlers")))
        );
    }

    #[test]
    fn complex_query() {
        // "title:guide (rust OR golang) -deprecated"
        assert_eq!(
            parse("title:guide (rust OR golang) -deprecated").unwrap(),
            Some(and(vec![
                field("title", term("guide")),
                or(vec![term("rust"), term("golang")]),
                not(term("deprecated"))
            ]))
        );
    }

    #[test]
    fn negated_group() {
        assert_eq!(
            parse("-(a b)").unwrap(),
            Some(not(and(vec![term("a"), term("b")])))
        );
    }

    #[test]
    fn negated_field() {
        assert_eq!(
            parse("-title:deprecated").unwrap(),
            Some(not(field("title", term("deprecated"))))
        );
    }

    #[test]
    fn error_unclosed_paren() {
        let err = parse("(rust async").unwrap_err();
        assert!(err.message.contains("closing parenthesis"));
    }

    #[test]
    fn error_unexpected_rparen() {
        let err = parse("rust)").unwrap_err();
        assert!(err.message.contains("unexpected"));
    }

    #[test]
    fn error_or_at_start() {
        let err = parse("OR rust").unwrap_err();
        assert!(err.message.contains("OR"));
    }

    #[test]
    fn error_or_at_end() {
        let err = parse("rust OR").unwrap_err();
        assert!(err.message.contains("end of query"));
    }

    #[test]
    fn error_field_without_value() {
        let err = parse("title:").unwrap_err();
        assert!(err.message.contains("expected"));
    }

    #[test]
    fn error_unclosed_quote() {
        let err = parse("\"unclosed").unwrap_err();
        assert!(err.message.contains("unclosed"));
    }

    #[test]
    fn or_case_insensitive() {
        assert_eq!(
            parse("rust or golang").unwrap(),
            Some(or(vec![term("rust"), term("golang")]))
        );
    }

    #[test]
    fn phrase_or_phrase() {
        assert_eq!(
            parse("\"error handling\" OR \"exception handling\"").unwrap(),
            Some(or(vec![
                phrase(&["error", "handling"]),
                phrase(&["exception", "handling"])
            ]))
        );
    }
}
