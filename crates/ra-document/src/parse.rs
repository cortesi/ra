//! High-level document parsing API.
//!
//! Provides functions to parse markdown and text files into `Document` structs
//! with proper chunk IDs, titles, and breadcrumbs.

use std::{
    fs,
    path::{Path, PathBuf},
};

use pulldown_cmark::HeadingLevel;

use crate::{
    Chunk, ChunkData, Document, DocumentError, Frontmatter, chunk_markdown, determine_chunk_level,
    extract_headings, parse_frontmatter,
};

/// Default minimum chunk size in characters.
pub const DEFAULT_MIN_CHUNK_SIZE: usize = 2000;

/// Result of parsing a document, including metadata about the parsing process.
#[derive(Debug, Clone)]
pub struct ParseResult {
    /// The parsed document.
    pub document: Document,
    /// The heading level used for chunking, if any.
    pub chunk_level: Option<HeadingLevel>,
    /// Why the chunk level was chosen (for debugging/inspection).
    pub chunk_reason: String,
}

/// Parses a markdown string into a document.
///
/// # Arguments
/// * `content` - The markdown content to parse
/// * `path` - Relative path within the tree (used for chunk IDs)
/// * `tree` - Name of the tree this document belongs to
/// * `min_chunk_size` - Minimum document size before chunking is applied
pub fn parse_markdown(
    content: &str,
    path: &Path,
    tree: &str,
    min_chunk_size: usize,
) -> ParseResult {
    // Parse frontmatter
    let (frontmatter, body) = parse_frontmatter(content);
    let frontmatter = frontmatter.unwrap_or_default();

    // Determine document title
    let title = determine_title(&frontmatter, body, path);

    // Determine chunk level and reason
    let (chunk_level, chunk_reason) = determine_chunk_level_with_reason(body, min_chunk_size);

    // Chunk the document
    let chunk_data = chunk_markdown(body, &title, min_chunk_size);

    // Convert ChunkData to Chunk with proper IDs
    let path_str = path.to_string_lossy();
    let chunks = chunk_data
        .into_iter()
        .map(|cd| chunk_data_to_chunk(cd, tree, &path_str))
        .collect();

    let document = Document {
        path: path.to_path_buf(),
        tree: tree.to_string(),
        title,
        tags: frontmatter.tags,
        chunks,
    };

    ParseResult {
        document,
        chunk_level,
        chunk_reason,
    }
}

/// Parses a plain text file into a document.
///
/// Text files are never chunked - they become a single chunk with no fragment ID.
pub fn parse_text(content: &str, path: &Path, tree: &str) -> ParseResult {
    let title = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Untitled".to_string());

    let path_str = path.to_string_lossy();
    let chunk_id = format!("{tree}:{path_str}");
    let breadcrumb = format!("> {title}");

    let chunk = Chunk {
        id: chunk_id,
        title: title.clone(),
        body: content.to_string(),
        is_preamble: true,
        breadcrumb,
    };

    let document = Document {
        path: path.to_path_buf(),
        tree: tree.to_string(),
        title,
        tags: vec![],
        chunks: vec![chunk],
    };

    ParseResult {
        document,
        chunk_level: None,
        chunk_reason: "text files are not chunked".to_string(),
    }
}

/// Parses a file from disk, detecting type by extension.
///
/// Supported extensions:
/// - `.md`, `.markdown` - parsed as markdown with chunking
/// - `.txt` - parsed as plain text (no chunking)
pub fn parse_file(
    path: &Path,
    tree: &str,
    min_chunk_size: usize,
) -> Result<ParseResult, DocumentError> {
    let content = fs::read_to_string(path).map_err(|source| DocumentError::ReadFile {
        path: path.to_path_buf(),
        source,
    })?;

    let relative_path = path.file_name().map(PathBuf::from).unwrap_or_default();

    match path.extension().and_then(|e| e.to_str()) {
        Some("md" | "markdown") => Ok(parse_markdown(
            &content,
            &relative_path,
            tree,
            min_chunk_size,
        )),
        Some("txt") => Ok(parse_text(&content, &relative_path, tree)),
        _ => Err(DocumentError::UnsupportedFileType {
            path: path.to_path_buf(),
        }),
    }
}

/// Determines the document title from frontmatter, first h1, or filename.
fn determine_title(frontmatter: &Frontmatter, content: &str, path: &Path) -> String {
    // 1. Try frontmatter title
    if let Some(title) = &frontmatter.title {
        return title.clone();
    }

    // 2. Try first h1 heading
    let headings = extract_headings(content);
    if let Some(h1) = headings.iter().find(|h| h.level == HeadingLevel::H1) {
        return h1.text.clone();
    }

    // 3. Fall back to filename
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Untitled".to_string())
}

/// Determines chunk level with a human-readable reason.
fn determine_chunk_level_with_reason(
    content: &str,
    min_chunk_size: usize,
) -> (Option<HeadingLevel>, String) {
    if content.len() < min_chunk_size {
        return (
            None,
            format!(
                "document too small ({} chars < {} min)",
                content.len(),
                min_chunk_size
            ),
        );
    }

    let level = determine_chunk_level(content, min_chunk_size);

    match level {
        Some(HeadingLevel::H1) => (
            Some(HeadingLevel::H1),
            "multiple h1 headings found".to_string(),
        ),
        Some(HeadingLevel::H2) => (
            Some(HeadingLevel::H2),
            "multiple h2 headings found".to_string(),
        ),
        Some(HeadingLevel::H3) => (
            Some(HeadingLevel::H3),
            "multiple h3 headings found".to_string(),
        ),
        Some(HeadingLevel::H4) => (
            Some(HeadingLevel::H4),
            "multiple h4 headings found".to_string(),
        ),
        Some(HeadingLevel::H5) => (
            Some(HeadingLevel::H5),
            "multiple h5 headings found".to_string(),
        ),
        Some(HeadingLevel::H6) => (
            Some(HeadingLevel::H6),
            "multiple h6 headings found".to_string(),
        ),
        None => (None, "no heading level has 2+ headings".to_string()),
    }
}

/// Converts internal ChunkData to public Chunk with proper ID.
fn chunk_data_to_chunk(data: ChunkData, tree: &str, path: &str) -> Chunk {
    let id = format!("{tree}:{path}#{}", data.slug);

    Chunk {
        id,
        title: data.title,
        body: data.body,
        is_preamble: data.is_preamble,
        breadcrumb: data.breadcrumb,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_markdown_with_frontmatter() {
        let content = r#"---
title: My Guide
tags: [rust, tutorial]
---

# Introduction

Welcome to the guide.

# Getting Started

Let's begin."#;

        let result = parse_markdown(content, Path::new("guide.md"), "docs", 0);

        assert_eq!(result.document.title, "My Guide");
        assert_eq!(result.document.tags, vec!["rust", "tutorial"]);
        assert_eq!(result.document.tree, "docs");
        assert_eq!(result.chunk_level, Some(HeadingLevel::H1));
        assert_eq!(result.document.chunks.len(), 2);

        // Check chunk IDs
        assert_eq!(result.document.chunks[0].id, "docs:guide.md#introduction");
        assert_eq!(
            result.document.chunks[1].id,
            "docs:guide.md#getting-started"
        );
    }

    #[test]
    fn test_parse_markdown_title_from_h1() {
        let content = "# My Document\n\nSome content.\n\n# Another Section\n\nMore content.";

        let result = parse_markdown(content, Path::new("doc.md"), "notes", 0);

        assert_eq!(result.document.title, "My Document");
    }

    #[test]
    fn test_parse_markdown_title_from_filename() {
        let content = "Just some content without any headings.";

        let result = parse_markdown(content, Path::new("readme.md"), "docs", 0);

        assert_eq!(result.document.title, "readme");
    }

    #[test]
    fn test_parse_markdown_small_document() {
        let content = "# First\n\n# Second";

        let result = parse_markdown(content, Path::new("small.md"), "docs", 10000);

        assert_eq!(result.chunk_level, None);
        assert!(result.chunk_reason.contains("too small"));
        assert_eq!(result.document.chunks.len(), 1);
        assert!(result.document.chunks[0].is_preamble);
    }

    #[test]
    fn test_parse_markdown_preamble_chunk() {
        let content = "Intro text.\n\n# Section 1\n\nContent.\n\n# Section 2\n\nMore.";

        let result = parse_markdown(content, Path::new("doc.md"), "docs", 0);

        assert_eq!(result.document.chunks.len(), 3);
        assert!(result.document.chunks[0].is_preamble);
        assert_eq!(result.document.chunks[0].id, "docs:doc.md#preamble");
    }

    #[test]
    fn test_parse_text() {
        let content = "This is plain text content.\nNo markdown here.";

        let result = parse_text(content, Path::new("notes.txt"), "docs");

        assert_eq!(result.document.title, "notes");
        assert_eq!(result.chunk_level, None);
        assert_eq!(result.document.chunks.len(), 1);
        assert_eq!(result.document.chunks[0].id, "docs:notes.txt");
        assert!(!result.document.chunks[0].id.contains('#'));
    }

    #[test]
    fn test_chunk_ids_format() {
        let content = "# Intro\n\nText.\n\n# Setup\n\nMore text.";

        let result = parse_markdown(content, Path::new("guide.md"), "my-tree", 0);

        for chunk in &result.document.chunks {
            assert!(chunk.id.starts_with("my-tree:guide.md#"));
        }
    }

    #[test]
    fn test_breadcrumbs_included() {
        let content = "# Parent\n\n## Child 1\n\nContent.\n\n## Child 2\n\nMore.";

        let result = parse_markdown(content, Path::new("doc.md"), "docs", 0);

        // Chunks should have breadcrumbs
        for chunk in &result.document.chunks {
            assert!(chunk.breadcrumb.starts_with("> "));
        }
    }

    #[test]
    fn test_determine_title_priority() {
        // Frontmatter title takes priority
        let fm_with_title = Frontmatter {
            title: Some("Frontmatter Title".to_string()),
            tags: vec![],
        };
        let title = determine_title(
            &fm_with_title,
            "# H1 Title\n\nContent",
            Path::new("file.md"),
        );
        assert_eq!(title, "Frontmatter Title");

        // H1 is second priority
        let fm_no_title = Frontmatter::default();
        let title = determine_title(&fm_no_title, "# H1 Title\n\nContent", Path::new("file.md"));
        assert_eq!(title, "H1 Title");

        // Filename is fallback
        let title = determine_title(&fm_no_title, "Just content", Path::new("myfile.md"));
        assert_eq!(title, "myfile");
    }
}
