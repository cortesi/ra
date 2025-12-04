//! Query parsing and building.
//!
//! This module provides the query language for ra search:
//!
//! - **Terms**: `rust` - words that must appear
//! - **Phrases**: `"error handling"` - exact sequences
//! - **Negation**: `-deprecated` - terms that must NOT appear
//! - **OR**: `rust OR golang` - alternatives
//! - **Grouping**: `(a b) OR (c d)` - precedence control
//! - **Fields**: `title:guide` - search specific fields
//!
//! # Example
//!
//! ```ignore
//! use ra_index::query::parse;
//!
//! let expr = parse("title:guide (rust OR golang) -deprecated")?;
//! ```

mod ast;
mod compile;
mod lexer;
mod parser;

use std::{error::Error, fmt};

pub use ast::QueryExpr;
pub use compile::QueryCompiler;
pub use parser::parse;

/// A unified error type for query parsing and compilation.
///
/// This type provides detailed error messages with context, including
/// the original query string and position indicators where applicable.
#[derive(Debug, Clone)]
pub struct QueryError {
    /// The kind of error that occurred.
    pub kind: QueryErrorKind,
    /// The original query string (if available).
    pub query: Option<String>,
}

/// The specific kind of query error.
#[derive(Debug, Clone)]
pub enum QueryErrorKind {
    /// Lexer error (tokenization failed).
    Lex {
        /// Error message.
        message: String,
        /// Byte position in input.
        position: usize,
    },
    /// Parser error (invalid syntax).
    Parse {
        /// Error message.
        message: String,
        /// Approximate byte position in input (if available).
        position: Option<usize>,
    },
    /// Compilation error (invalid semantics).
    Compile {
        /// Error message.
        message: String,
    },
}

impl QueryError {
    /// Creates a lex error.
    pub fn lex(message: impl Into<String>, position: usize, query: impl Into<String>) -> Self {
        Self {
            kind: QueryErrorKind::Lex {
                message: message.into(),
                position,
            },
            query: Some(query.into()),
        }
    }

    /// Creates a parse error.
    pub fn parse(
        message: impl Into<String>,
        position: Option<usize>,
        query: Option<String>,
    ) -> Self {
        Self {
            kind: QueryErrorKind::Parse {
                message: message.into(),
                position,
            },
            query,
        }
    }

    /// Creates a compile error.
    pub fn compile(message: impl Into<String>) -> Self {
        Self {
            kind: QueryErrorKind::Compile {
                message: message.into(),
            },
            query: None,
        }
    }

    /// Sets the query string for this error.
    pub fn with_query(mut self, query: impl Into<String>) -> Self {
        self.query = Some(query.into());
        self
    }

    /// Returns the error message without context.
    pub fn message(&self) -> &str {
        match &self.kind {
            QueryErrorKind::Lex { message, .. } => message,
            QueryErrorKind::Parse { message, .. } => message,
            QueryErrorKind::Compile { message } => message,
        }
    }

    /// Returns a suggestion for common errors.
    pub fn suggestion(&self) -> Option<&'static str> {
        match &self.kind {
            QueryErrorKind::Lex { message, .. } if message.contains("unclosed quote") => {
                Some("Add a closing quote (\") to complete the phrase")
            }
            QueryErrorKind::Parse { message, .. } if message.contains("closing parenthesis") => {
                Some("Add a closing parenthesis ) to match the opening one")
            }
            QueryErrorKind::Parse { message, .. } if message.contains("OR") => {
                Some("OR requires expressions on both sides, e.g., 'rust OR golang'")
            }
            QueryErrorKind::Compile { message } if message.contains("unknown field") => {
                Some("Valid fields are: title, tags, body, path, tree")
            }
            _ => None,
        }
    }
}

impl fmt::Display for QueryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Format the error message
        let prefix = match &self.kind {
            QueryErrorKind::Lex { .. } => "query syntax error",
            QueryErrorKind::Parse { .. } => "query syntax error",
            QueryErrorKind::Compile { .. } => "query error",
        };

        writeln!(f, "{}: {}", prefix, self.message())?;

        // If we have a query and position, show it with a pointer
        if let Some(query) = &self.query {
            let position = match &self.kind {
                QueryErrorKind::Lex { position, .. } => Some(*position),
                QueryErrorKind::Parse { position, .. } => *position,
                QueryErrorKind::Compile { .. } => None,
            };

            writeln!(f, "  {}", query)?;
            if let Some(pos) = position {
                let clamped = pos.min(query.len());
                writeln!(f, "  {}^", " ".repeat(clamped))?;
            }
        }

        // Add suggestion if available
        if let Some(suggestion) = self.suggestion() {
            write!(f, "hint: {}", suggestion)?;
        }

        Ok(())
    }
}

impl Error for QueryError {}

impl From<lexer::LexError> for QueryError {
    fn from(err: lexer::LexError) -> Self {
        Self {
            kind: QueryErrorKind::Lex {
                message: err.message,
                position: err.position,
            },
            query: Some(err.input),
        }
    }
}

impl From<parser::ParseError> for QueryError {
    fn from(err: parser::ParseError) -> Self {
        Self {
            kind: QueryErrorKind::Parse {
                message: err.message,
                position: None, // Parser doesn't have byte positions
            },
            query: None,
        }
    }
}

impl From<compile::CompileError> for QueryError {
    fn from(err: compile::CompileError) -> Self {
        Self {
            kind: QueryErrorKind::Compile {
                message: err.message,
            },
            query: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lex_error_display() {
        let err = QueryError::lex("unclosed quote", 0, "\"hello world");
        let display = err.to_string();
        assert!(display.contains("unclosed quote"));
        assert!(display.contains("\"hello world"));
        assert!(display.contains("^"));
        assert!(display.contains("hint:"));
    }

    #[test]
    fn parse_error_display() {
        let err = QueryError::parse(
            "expected closing parenthesis",
            Some(5),
            Some("(rust".to_string()),
        );
        let display = err.to_string();
        assert!(display.contains("expected closing parenthesis"));
        assert!(display.contains("(rust"));
        assert!(display.contains("hint:"));
    }

    #[test]
    fn compile_error_display() {
        let err = QueryError::compile("unknown field: foo");
        let display = err.to_string();
        assert!(display.contains("unknown field: foo"));
        assert!(display.contains("hint:"));
        assert!(display.contains("Valid fields are:"));
    }

    #[test]
    fn error_with_query() {
        let err = QueryError::compile("unknown field: xyz").with_query("xyz:value");
        assert!(err.query.is_some());
        assert_eq!(err.query.unwrap(), "xyz:value");
    }

    #[test]
    fn message_extraction() {
        let err = QueryError::lex("test message", 0, "query");
        assert_eq!(err.message(), "test message");
    }

    #[test]
    fn or_error_suggestion() {
        let err = QueryError::parse("unexpected OR", None, None);
        assert!(err.suggestion().is_some());
        assert!(err.suggestion().unwrap().contains("OR requires"));
    }
}
