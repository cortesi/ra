//! High-level document parsing API.
//!
//! Provides functions to parse markdown and text files into `Document` structs
//! with hierarchical chunk trees.

use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::{
    Document, DocumentError, Frontmatter, build_chunk_tree, extract_headings, node::Node,
    parse_frontmatter, tree::ChunkTree,
};

/// Result of parsing a document, including metadata about the parsing process.
#[derive(Debug, Clone)]
pub struct ParseResult {
    /// The parsed document.
    pub document: Document,
    /// Whether the document has any chunks (nodes with body text).
    pub has_chunks: bool,
}

/// Parses a markdown string into a document.
///
/// # Arguments
/// * `content` - The markdown content to parse
/// * `path` - Relative path within the tree (used for chunk IDs)
/// * `tree` - Name of the tree this document belongs to
pub fn parse_markdown(content: &str, path: &Path, tree: &str) -> ParseResult {
    // Parse frontmatter
    let (frontmatter, _body) = parse_frontmatter(content);
    let frontmatter = frontmatter.unwrap_or_default();

    // Determine document title
    let title = determine_title(&frontmatter, content, path);

    // Build the hierarchical chunk tree
    // Note: We use the full content (including frontmatter) as the spec says
    // frontmatter bytes are included in the document node's body
    let chunk_tree = build_chunk_tree(content, tree, path, &title);
    let has_chunks = chunk_tree.chunk_count() > 0;

    let document = Document {
        path: path.to_path_buf(),
        tree: tree.to_string(),
        title,
        tags: frontmatter.tags,
        chunk_tree,
    };

    ParseResult {
        document,
        has_chunks,
    }
}

/// Parses a plain text file into a document.
///
/// Text files produce a single document node with the entire file as body.
/// The title is derived from the filename.
pub fn parse_text(content: &str, path: &Path, tree: &str) -> ParseResult {
    let title = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Untitled".to_string());

    // Create a document node for the entire file
    let root = Node::document(tree, path, title.clone(), content.len());
    let chunk_tree = ChunkTree::new(root, content.to_string());
    let has_chunks = chunk_tree.chunk_count() > 0;

    let document = Document {
        path: path.to_path_buf(),
        tree: tree.to_string(),
        title,
        tags: vec![],
        chunk_tree,
    };

    ParseResult {
        document,
        has_chunks,
    }
}

/// Parses a file from disk, detecting type by extension.
///
/// Supported extensions:
/// - `.md`, `.markdown` - parsed as markdown with hierarchical chunking
/// - `.txt` - parsed as plain text (single document node)
pub fn parse_file(path: &Path, tree: &str) -> Result<ParseResult, DocumentError> {
    let content = fs::read_to_string(path).map_err(|source| DocumentError::ReadFile {
        path: path.to_path_buf(),
        source,
    })?;

    let relative_path = path.file_name().map(PathBuf::from).unwrap_or_default();

    match path.extension().and_then(|e| e.to_str()) {
        Some("md" | "markdown") => Ok(parse_markdown(&content, &relative_path, tree)),
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
    if let Some(h1) = headings.iter().find(|h| h.level == 1) {
        return h1.text.clone();
    }

    // 3. Fall back to filename
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Untitled".to_string())
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

        let result = parse_markdown(content, Path::new("guide.md"), "docs");

        assert_eq!(result.document.title, "My Guide");
        assert_eq!(result.document.tags, vec!["rust", "tutorial"]);
        assert_eq!(result.document.tree, "docs");
        assert!(result.has_chunks);

        // Extract chunks from the tree
        let chunks = result
            .document
            .chunk_tree
            .extract_chunks(&result.document.title);

        // Should have preamble (frontmatter included) + 2 headings
        // Note: frontmatter is included in preamble per spec
        assert!(chunks.len() >= 2);

        // Check that heading chunks exist
        let ids: Vec<&str> = chunks.iter().map(|c| c.id.as_str()).collect();
        assert!(ids.iter().any(|id| id.contains("#introduction")));
        assert!(ids.iter().any(|id| id.contains("#getting-started")));
    }

    #[test]
    fn test_parse_markdown_title_from_h1() {
        let content = "# My Document\n\nSome content.\n\n# Another Section\n\nMore content.";

        let result = parse_markdown(content, Path::new("doc.md"), "notes");

        assert_eq!(result.document.title, "My Document");
    }

    #[test]
    fn test_parse_markdown_title_from_filename() {
        let content = "Just some content without any headings.";

        let result = parse_markdown(content, Path::new("readme.md"), "docs");

        assert_eq!(result.document.title, "readme");
    }

    #[test]
    fn test_parse_markdown_preamble_chunk() {
        let content = "Intro text.\n\n# Section 1\n\nContent.\n\n# Section 2\n\nMore.";

        let result = parse_markdown(content, Path::new("doc.md"), "docs");

        let chunks = result
            .document
            .chunk_tree
            .extract_chunks(&result.document.title);

        // Should have preamble (document node) + 2 sections
        assert_eq!(chunks.len(), 3);

        // First chunk should be the document node (preamble)
        assert_eq!(chunks[0].id, "docs:doc.md");
        assert_eq!(chunks[0].depth, 0);
        assert!(chunks[0].body.contains("Intro text"));
    }

    #[test]
    fn test_parse_text() {
        let content = "This is plain text content.\nNo markdown here.";

        let result = parse_text(content, Path::new("notes.txt"), "docs");

        assert_eq!(result.document.title, "notes");
        assert!(result.has_chunks);

        let chunks = result
            .document
            .chunk_tree
            .extract_chunks(&result.document.title);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].id, "docs:notes.txt");
        assert!(!chunks[0].id.contains('#'));
        assert_eq!(chunks[0].depth, 0);
    }

    #[test]
    fn test_chunk_ids_format() {
        let content = "# Intro\n\nText.\n\n# Setup\n\nMore text.";

        let result = parse_markdown(content, Path::new("guide.md"), "my-tree");

        let chunks = result
            .document
            .chunk_tree
            .extract_chunks(&result.document.title);

        // Heading chunks should have fragment IDs
        for chunk in chunks.iter().filter(|c| c.depth > 0) {
            assert!(chunk.id.starts_with("my-tree:guide.md#"));
        }
    }

    #[test]
    fn test_breadcrumbs_included() {
        let content =
            "# Parent\n\nParent content.\n\n## Child 1\n\nContent.\n\n## Child 2\n\nMore.";

        let result = parse_markdown(content, Path::new("doc.md"), "docs");

        let chunks = result
            .document
            .chunk_tree
            .extract_chunks(&result.document.title);

        // All chunks should have breadcrumbs starting with >
        for chunk in &chunks {
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

    #[test]
    fn test_empty_document() {
        let content = "";

        let result = parse_markdown(content, Path::new("empty.md"), "docs");

        // Empty document has no chunks
        assert!(!result.has_chunks);
        assert_eq!(result.document.chunk_tree.chunk_count(), 0);
    }

    #[test]
    fn test_whitespace_only_document() {
        let content = "   \n\t\n  ";

        let result = parse_markdown(content, Path::new("whitespace.md"), "docs");

        // Whitespace-only document has no chunks
        assert!(!result.has_chunks);
        assert_eq!(result.document.chunk_tree.chunk_count(), 0);
    }

    #[test]
    fn test_only_headings_no_content() {
        // Document with only headings at same level (each has empty span)
        let content = "# H1\n# H2\n# H3";

        let result = parse_markdown(content, Path::new("headings.md"), "docs");

        // No chunks because all headings have empty spans
        assert!(!result.has_chunks);
        assert_eq!(result.document.chunk_tree.chunk_count(), 0);
    }

    #[test]
    fn test_empty_text_file() {
        let content = "";

        let result = parse_text(content, Path::new("empty.txt"), "docs");

        assert!(!result.has_chunks);
        assert_eq!(result.document.chunk_tree.chunk_count(), 0);
    }

    #[test]
    fn test_hierarchical_structure() {
        let content = r#"Preamble.

# Section 1

Section 1 intro.

## Subsection 1.1

Content 1.1.

## Subsection 1.2

Content 1.2.

# Section 2

Section 2 content.
"#;

        let result = parse_markdown(content, Path::new("doc.md"), "docs");

        // Tree structure:
        // Doc (preamble)
        //   ├── Section 1
        //   │   ├── Subsection 1.1
        //   │   └── Subsection 1.2
        //   └── Section 2

        let tree = &result.document.chunk_tree;
        assert_eq!(tree.node_count(), 5);

        // Check hierarchy via parent_id
        // Note: slugs are "subsection-11" because periods are removed from "Subsection 1.1"
        let s1 = tree.get_node("docs:doc.md#section-1").unwrap();
        assert_eq!(s1.parent_id, Some("docs:doc.md".to_string()));

        let s11 = tree.get_node("docs:doc.md#subsection-11").unwrap();
        assert_eq!(s11.parent_id, Some("docs:doc.md#section-1".to_string()));

        let s12 = tree.get_node("docs:doc.md#subsection-12").unwrap();
        assert_eq!(s12.parent_id, Some("docs:doc.md#section-1".to_string()));

        let s2 = tree.get_node("docs:doc.md#section-2").unwrap();
        assert_eq!(s2.parent_id, Some("docs:doc.md".to_string()));
    }

    #[test]
    fn test_parse_real_docs_chunking() {
        // Test parsing the actual chunking.md spec file
        let content = include_str!("../../../docs/chunking.md");
        let result = parse_markdown(content, Path::new("chunking.md"), "docs");

        assert!(result.has_chunks);
        assert_eq!(result.document.title, "Chunking");

        // Should have hierarchical structure with multiple sections
        let chunks = result
            .document
            .chunk_tree
            .extract_chunks(&result.document.title);
        assert!(chunks.len() >= 5); // Should have at least 5 chunks

        // Check that some expected sections exist
        let titles: Vec<&str> = chunks.iter().map(|c| c.title.as_str()).collect();
        assert!(titles.contains(&"Overview"));
        assert!(titles.contains(&"Terminology"));
    }

    #[test]
    fn test_parse_real_docs_search() {
        // Test parsing the actual search.md file
        let content = include_str!("../../../docs/search.md");
        let result = parse_markdown(content, Path::new("search.md"), "docs");

        assert!(result.has_chunks);
        assert_eq!(result.document.title, "Search");

        // Should have chunks
        let tree = &result.document.chunk_tree;
        assert!(tree.chunk_count() > 0);
    }

    #[test]
    fn test_parse_real_docs_spec() {
        // Test parsing the actual spec.md file
        let content = include_str!("../../../docs/spec.md");
        let result = parse_markdown(content, Path::new("spec.md"), "docs");

        assert!(result.has_chunks);

        // Should have hierarchical structure
        let tree = &result.document.chunk_tree;
        assert!(tree.node_count() >= 1);
    }
}
