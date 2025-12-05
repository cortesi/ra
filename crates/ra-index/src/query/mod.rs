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
//! - **Boosting**: `rust^2.5` - adjust term importance
//!
//! # Example
//!
//! ```ignore
//! use ra_index::query::parse;
//!
//! let expr = parse("title:guide (rust OR golang) -deprecated")?;
//! ```

mod compile;

// Re-export query types from ra-query
pub use compile::{CompileError, QueryCompiler};
pub use ra_query::{QueryError, QueryErrorKind, QueryExpr, parse};

impl From<CompileError> for QueryError {
    fn from(err: CompileError) -> Self {
        Self::compile(err.message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compile_error_converts_to_query_error() {
        let compile_err = CompileError {
            message: "unknown field: foo".into(),
        };
        let query_err: QueryError = compile_err.into();
        assert!(query_err.message().contains("unknown field"));
    }
}
