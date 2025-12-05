//! Query parsing and AST for ra search.
//!
//! This crate provides a query language for searching knowledge bases:
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
//! ```
//! use ra_query::parse;
//!
//! let expr = parse("title:guide (rust OR golang) -deprecated").unwrap();
//! assert!(expr.is_some());
//! ```

#![warn(missing_docs)]

mod ast;
mod error;
mod lexer;
mod parser;

pub use ast::QueryExpr;
pub use error::{LexError, ParseError, QueryError, QueryErrorKind};
pub use lexer::{Token, tokenize};
pub use parser::parse;
