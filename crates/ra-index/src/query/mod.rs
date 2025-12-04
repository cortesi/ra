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
mod builder;
mod lexer;
mod parser;

pub use builder::QueryBuilder;
