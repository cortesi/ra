//! Tantivy-based search index for ra.
//!
//! This crate provides the indexing infrastructure for ra's knowledge base search.
//! It handles:
//! - Document conversion from `ra-document` chunks
//! - Index creation, writing, and management
//!
//! # Example
//!
//! ```no_run
//! use std::time::SystemTime;
//! use ra_index::{ChunkDocument, IndexWriter};
//!
//! // Open or create an index
//! let mut writer = IndexWriter::open("./index".as_ref()).unwrap();
//!
//! // Add a document
//! let doc = ChunkDocument {
//!     id: "local:docs/test.md#intro".to_string(),
//!     title: "Introduction".to_string(),
//!     tags: vec!["rust".to_string()],
//!     path: "docs/test.md".to_string(),
//!     path_components: vec!["docs".to_string(), "test".to_string(), "md".to_string()],
//!     tree: "local".to_string(),
//!     body: "Content here".to_string(),
//!     mtime: SystemTime::now(),
//! };
//! writer.add_document(&doc).unwrap();
//! writer.commit().unwrap();
//! ```

#![warn(missing_docs)]

mod document;
mod error;
mod schema;
mod writer;

pub use document::ChunkDocument;
pub use error::IndexError;
pub use writer::IndexWriter;
