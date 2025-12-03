//! Tantivy-based search index for ra.
//!
//! This crate provides the indexing infrastructure for ra's knowledge base search.
//! It handles:
//! - Document conversion from `ra-document` chunks
//! - Index creation, writing, and management
//! - Index location resolution based on configuration
//! - Configuration hash tracking for index versioning
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

mod config_hash;
mod diff;
mod discovery;
mod document;
mod error;
mod location;
mod manifest;
mod schema;
mod status;
mod writer;

pub use config_hash::{IndexingConfig, SCHEMA_VERSION, compute_config_hash};
pub use diff::{ManifestDiff, apply_diff, diff_manifest};
pub use discovery::{DiscoveredFile, discover_files, discover_tree_files, file_mtime};
pub use document::ChunkDocument;
pub use error::IndexError;
pub use location::{
    config_hash_path, global_index_directory, index_directory, is_local_config, manifest_path,
};
pub use manifest::{Manifest, ManifestEntry};
pub use status::{
    IndexStatus, detect_index_status, index_exists, read_stored_hash, write_config_hash,
};
pub use writer::IndexWriter;
