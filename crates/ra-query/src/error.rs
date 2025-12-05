//! Error types for query parsing and compilation.
//!
//! This module provides error types for lexing, parsing, and compiling query expressions.

use std::{error::Error, fmt};

/// Lexer error with position information.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LexError {
    /// Error message.
    pub message: String,
    /// Byte position in input where error occurred.
    pub position: usize,
    /// The original input string.
    pub input: String,
}

impl LexError {
    /// Creates a new lexer error.
    pub fn new(message: impl Into<String>, position: usize, input: &str) -> Self {
        Self {
            message: message.into(),
            position,
            input: input.to_string(),
        }
    }

    /// Formats the error with a position indicator showing where the error occurred.
    pub fn format_with_context(&self) -> String {
        let mut result = String::new();
        result.push_str(&format!("query syntax error: {}\n", self.message));
        result.push_str(&format!("  {}\n", self.input));
        result.push_str(&format!("  {}^", " ".repeat(self.position)));
        result
    }
}

impl fmt::Display for LexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.format_with_context())
    }
}

impl Error for LexError {}

/// Parse error with position information.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    /// Error message.
    pub message: String,
    /// Token index where error occurred (if applicable).
    pub token_index: Option<usize>,
}

impl ParseError {
    /// Creates a new parse error.
    pub fn new(message: impl Into<String>, token_index: Option<usize>) -> Self {
        Self {
            message: message.into(),
            token_index,
        }
    }
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

/// A unified error type for query parsing.
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

impl From<LexError> for QueryError {
    fn from(err: LexError) -> Self {
        Self {
            kind: QueryErrorKind::Lex {
                message: err.message,
                position: err.position,
            },
            query: Some(err.input),
        }
    }
}

impl From<ParseError> for QueryError {
    fn from(err: ParseError) -> Self {
        Self {
            kind: QueryErrorKind::Parse {
                message: err.message,
                position: None, // Parser doesn't have byte positions
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
    fn error_with_query() {
        let err = QueryError::parse("test error", None, None).with_query("xyz:value");
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

    #[test]
    fn compile_error_display() {
        let err = QueryError::compile("unknown field: foo");
        let display = err.to_string();
        assert!(display.contains("unknown field: foo"));
        assert!(display.contains("hint:"));
        assert!(display.contains("Valid fields are:"));
    }
}
