//! Tantivy-based search index for ra.
//!
//! This crate provides the indexing and search infrastructure for ra's knowledge base.
//! It handles:
//! - Index creation and incremental updates via [`Indexer`]
//! - Full-text search with hierarchical aggregation via [`Searcher`]
//! - Index location resolution based on configuration
//! - Query parsing via [`parse_query`]
//!
//! # Indexing
//!
//! Use [`Indexer`] to build and update the search index:
//!
//! ```ignore
//! use ra_index::{Indexer, SilentReporter};
//!
//! let indexer = Indexer::new(&config)?;
//! let stats = indexer.incremental_update(&mut SilentReporter)?;
//! ```
//!
//! # Searching
//!
//! Use [`Searcher`] or [`open_searcher`] to query the index:
//!
//! ```ignore
//! use ra_index::{open_searcher, SearchParams};
//!
//! let mut searcher = open_searcher(&config)?;
//! let results = searcher.search_aggregated("rust async", &SearchParams::default())?;
//! ```

#![warn(missing_docs)]

mod aggregate;
mod analyzer;
mod config_hash;
mod context;
mod diff;
mod discovery;
mod document;
mod elbow;
mod error;
mod indexer;
mod location;
mod manifest;
mod query;
mod result;
mod schema;
mod search;
mod status;
mod writer;

// Core public API - types and functions used by the ra CLI
pub use context::{ContextAnalysisResult, ContextSearch, ContextWarning, FileAnalysis};
pub use error::IndexError;
pub use indexer::{IndexStats, Indexer, ProgressReporter, SilentReporter};
pub use location::index_directory;
pub use query::{QueryError, QueryErrorKind, QueryExpr, parse as parse_query};
pub use ra_context::is_binary_file;
pub use result::{SearchCandidate, SearchResult as AggregatedSearchResult};
pub use search::{
    MatchDetails, SearchParams, SearchResult, Searcher, TreeFilteredSearcher, merge_ranges,
    open_searcher,
};
pub use status::{IndexStatus, detect_index_status};
