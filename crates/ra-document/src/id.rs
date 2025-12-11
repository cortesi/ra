//! Identifier types for documents and chunks.
//!
//! IDs are represented as strings in the format `tree:path` for documents and
//! `tree:path#slug` for chunks. These newtypes centralize parsing and formatting
//! to avoid ad-hoc string handling across crates.

use std::{fmt, path::Path, str::FromStr};

use thiserror::Error;

/// Errors that can occur when parsing document or chunk IDs.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum IdError {
    /// The input did not match the expected `tree:path[#slug]` format.
    #[error("invalid id format")]
    InvalidFormat,
}

/// A document identifier in `tree:path` form.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DocId {
    /// Name of the tree this document belongs to.
    pub tree: String,
    /// Path relative to the tree root.
    pub path: String,
}

impl DocId {
    /// Constructs a doc ID from a tree name and filesystem path.
    pub fn from_path(tree: &str, path: &Path) -> Self {
        Self {
            tree: tree.to_string(),
            path: path.to_string_lossy().replace('\\', "/"),
        }
    }

    /// Parses a doc ID from `tree:path` format.
    pub fn parse(id: &str) -> Result<Self, IdError> {
        let Some((tree, path)) = id.split_once(':') else {
            return Err(IdError::InvalidFormat);
        };

        if tree.is_empty() || path.is_empty() {
            return Err(IdError::InvalidFormat);
        }

        if tree.len() == 1 && id.chars().nth(1) == Some(':') {
            return Err(IdError::InvalidFormat);
        }

        Ok(Self {
            tree: tree.to_string(),
            path: path.replace('\\', "/"),
        })
    }
}

impl fmt::Display for DocId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.tree, self.path)
    }
}

impl FromStr for DocId {
    type Err = IdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

/// A chunk identifier in `tree:path#slug` form.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ChunkId {
    /// The parent document ID.
    pub doc_id: DocId,
    /// Optional slug for a specific section within the document.
    pub slug: Option<String>,
}

impl ChunkId {
    /// Constructs a chunk ID from tree, path, and optional slug.
    pub fn from_path(tree: &str, path: &Path, slug: Option<&str>) -> Self {
        Self {
            doc_id: DocId::from_path(tree, path),
            slug: slug.map(str::to_string),
        }
    }

    /// Parses a chunk ID from `tree:path#slug` or `tree:path` format.
    pub fn parse(id: &str) -> Result<Self, IdError> {
        let Some((tree, rest)) = id.split_once(':') else {
            return Err(IdError::InvalidFormat);
        };

        if tree.is_empty() || rest.is_empty() {
            return Err(IdError::InvalidFormat);
        }

        if tree.len() == 1 && id.chars().nth(1) == Some(':') {
            return Err(IdError::InvalidFormat);
        }

        let (path, slug) = match rest.split_once('#') {
            Some((p, s)) if !p.is_empty() => (p, Some(s)),
            Some(_) => return Err(IdError::InvalidFormat),
            None => (rest, None),
        };

        Ok(Self {
            doc_id: DocId {
                tree: tree.to_string(),
                path: path.replace('\\', "/"),
            },
            slug: slug.filter(|s| !s.is_empty()).map(str::to_string),
        })
    }

    /// Returns true if this ID refers to a whole document.
    pub fn is_document(&self) -> bool {
        self.slug.is_none()
    }
}

impl fmt::Display for ChunkId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.slug {
            Some(slug) => write!(f, "{}#{}", self.doc_id, slug),
            None => write!(f, "{}", self.doc_id),
        }
    }
}

impl FromStr for ChunkId {
    type Err = IdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn doc_id_parses_and_formats() {
        let id: DocId = "docs:guide.md".parse().unwrap();
        assert_eq!(id.tree, "docs");
        assert_eq!(id.path, "guide.md");
        assert_eq!(id.to_string(), "docs:guide.md");
    }

    #[test]
    fn chunk_id_parses_with_slug() {
        let id: ChunkId = "docs:guide.md#intro".parse().unwrap();
        assert_eq!(id.doc_id.to_string(), "docs:guide.md");
        assert_eq!(id.slug.as_deref(), Some("intro"));
        assert_eq!(id.to_string(), "docs:guide.md#intro");
        assert!(!id.is_document());
    }

    #[test]
    fn chunk_id_parses_without_slug() {
        let id: ChunkId = "docs:guide.md".parse().unwrap();
        assert_eq!(id.doc_id.to_string(), "docs:guide.md");
        assert!(id.slug.is_none());
        assert!(id.is_document());
    }

    #[test]
    fn invalid_ids_error() {
        assert!("nope".parse::<ChunkId>().is_err());
        assert!(":path".parse::<ChunkId>().is_err());
        assert!("tree:".parse::<ChunkId>().is_err());
        assert!("C:\\foo\\bar".parse::<ChunkId>().is_err());
    }
}
