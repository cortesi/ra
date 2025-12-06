//! Tree construction from markdown content.
//!
//! This module provides functions to build a hierarchical `ChunkTree` from markdown content
//! by parsing headings and establishing parent-child relationships based on heading depth.

use std::path::Path;

use pulldown_cmark::{Event, HeadingLevel, Parser, Tag, TagEnd};

use crate::{
    node::{HeadingParams, Node},
    slug::Slugifier,
    tree::ChunkTree,
};

/// Information about a parsed heading.
#[derive(Debug, Clone)]
pub struct HeadingInfo {
    /// The heading level (1-6 for h1-h6).
    pub level: u8,
    /// The heading text (including inline code).
    pub text: String,
    /// Byte offset where the heading line starts.
    pub heading_start: usize,
    /// Byte offset where the heading line ends.
    pub heading_end: usize,
}

/// Extracts all headings from markdown content with byte offsets for the heading line.
pub fn extract_headings(content: &str) -> Vec<HeadingInfo> {
    let parser = Parser::new(content);
    let mut headings = Vec::new();
    let mut current_heading: Option<(HeadingLevel, usize, String)> = None;

    for (event, range) in parser.into_offset_iter() {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                current_heading = Some((level, range.start, String::new()));
            }
            Event::Text(text) | Event::Code(text) => {
                if let Some((_, _, ref mut heading_text)) = current_heading {
                    heading_text.push_str(&text);
                }
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some((level, start, text)) = current_heading.take() {
                    headings.push(HeadingInfo {
                        level: heading_level_to_u8(level),
                        text,
                        heading_start: start,
                        heading_end: range.end,
                    });
                }
            }
            _ => {}
        }
    }

    headings
}

/// Converts a pulldown_cmark HeadingLevel to a u8 (1-6).
fn heading_level_to_u8(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// A heading with its computed content span.
#[derive(Debug)]
struct HeadingWithSpan {
    /// The original heading information.
    info: HeadingInfo,
    /// Byte offset where the content span starts (after heading line newline).
    span_start: usize,
    /// Byte offset where the content span ends.
    span_end: usize,
}

/// Builds a hierarchical chunk tree from markdown content.
///
/// The algorithm:
/// 1. Creates a document node as root (depth 0, span [0, content.len()))
/// 2. Parses all headings with their byte positions
/// 3. For each heading, computes its span (byte after heading line to next equal/lower heading)
/// 4. Attaches each heading to the nearest preceding heading with strictly lower depth
/// 5. Discards headings with empty spans (consecutive headings with no content between)
/// 6. Assigns positions via pre-order traversal
/// 7. Computes sibling counts
pub fn build_chunk_tree(content: &str, tree_name: &str, path: &Path, doc_title: &str) -> ChunkTree {
    let headings = extract_headings(content);
    let mut slugifier = Slugifier::default();

    // Create the document node as root
    let mut root = Node::document(tree_name, path, doc_title.to_string(), content.len());

    // Track where the first heading starts (for preamble calculation)
    let first_heading_start = headings.first().map(|h| h.heading_start);

    if headings.is_empty() {
        // No headings - just return document with no children
        let mut tree = ChunkTree::new(root, content.to_string());
        tree.assign_positions();
        tree.assign_sibling_counts();
        return tree;
    }

    // Calculate spans for each heading
    let headings_with_spans = calculate_heading_spans(&headings, content);

    // Filter out headings with empty spans
    let valid_headings: Vec<HeadingWithSpan> = headings_with_spans
        .into_iter()
        .filter(|h| h.span_start < h.span_end)
        .collect();

    // Build the tree using a stack-based approach
    // Stack contains (node, depth) pairs representing the path from root to current insertion point
    let mut stack: Vec<(Node, u8)> = vec![(root, 0)];

    for heading in valid_headings {
        let slug = slugifier.slugify(&heading.info.text);

        // Pop nodes from stack until we find a parent with depth < heading depth
        while stack.len() > 1 && stack.last().unwrap().1 >= heading.info.level {
            let (child, _) = stack.pop().unwrap();
            stack.last_mut().unwrap().0.children.push(child);
        }

        let parent_id = stack.last().unwrap().0.id.clone();

        let node = Node::heading(
            tree_name,
            path,
            HeadingParams {
                parent_id,
                depth: heading.info.level,
                title: heading.info.text,
                slug,
                heading_line_start: heading.info.heading_start,
                byte_start: heading.span_start,
                byte_end: heading.span_end,
            },
        );

        stack.push((node, heading.info.level));
    }

    // Pop remaining nodes and attach to parents
    while stack.len() > 1 {
        let (child, _) = stack.pop().unwrap();
        stack.last_mut().unwrap().0.children.push(child);
    }

    root = stack.pop().unwrap().0;

    // Create tree with first_heading_start for proper preamble calculation
    let mut tree = match first_heading_start {
        Some(start) => ChunkTree::with_first_heading(root, content.to_string(), start),
        None => ChunkTree::new(root, content.to_string()),
    };
    tree.assign_positions();
    tree.assign_sibling_counts();
    tree
}

/// Calculates the content span for each heading.
///
/// A heading's span starts at the first byte after the heading line
/// and ends at the byte before the next heading of equal or lower depth (or end of document).
fn calculate_heading_spans(headings: &[HeadingInfo], content: &str) -> Vec<HeadingWithSpan> {
    let mut result = Vec::with_capacity(headings.len());
    let content_len = content.len();

    for (i, heading) in headings.iter().enumerate() {
        // Span starts after the heading line
        // Skip the newline after heading if present
        let mut span_start = heading.heading_end;
        if span_start < content_len && content.as_bytes()[span_start] == b'\n' {
            span_start += 1;
        }

        // Find end: next heading of equal or lower depth, or end of document
        let span_end = headings[i + 1..]
            .iter()
            .find(|h| h.level <= heading.level)
            .map(|h| h.heading_start)
            .unwrap_or(content_len);

        result.push(HeadingWithSpan {
            info: heading.clone(),
            span_start,
            span_end,
        });
    }

    result
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn test_extract_headings() {
        let content = "# Heading 1\n\nSome text\n\n## Heading 2\n\nMore text";
        let headings = extract_headings(content);

        assert_eq!(headings.len(), 2);
        assert_eq!(headings[0].level, 1);
        assert_eq!(headings[0].text, "Heading 1");
        assert_eq!(headings[1].level, 2);
        assert_eq!(headings[1].text, "Heading 2");
    }

    #[test]
    fn test_extract_headings_with_code() {
        let content = "# The `Result<T>` Type\n\nContent";
        let headings = extract_headings(content);

        assert_eq!(headings.len(), 1);
        assert_eq!(headings[0].text, "The Result<T> Type");
    }

    #[test]
    fn test_heading_byte_offsets() {
        let content = "# Title\n\nParagraph\n\n## Section\n\nMore";
        let headings = extract_headings(content);

        assert_eq!(headings.len(), 2);
        assert_eq!(headings[0].heading_start, 0);
        // pulldown_cmark includes the trailing newline in the heading range
        assert_eq!(
            &content[headings[0].heading_start..headings[0].heading_end],
            "# Title\n"
        );
        assert_eq!(
            &content[headings[1].heading_start..headings[1].heading_end],
            "## Section\n"
        );
    }

    #[test]
    fn test_build_chunk_tree_simple() {
        let content = "Preamble text.\n\n# Section 1\n\nContent 1.\n\n# Section 2\n\nContent 2.";
        let path = PathBuf::from("test.md");
        let tree = build_chunk_tree(content, "docs", &path, "Test Doc");

        // Should have document node + 2 h1 nodes
        assert_eq!(tree.node_count(), 3);
        assert_eq!(tree.root().title, "Test Doc");
        assert_eq!(tree.root().children.len(), 2);
        assert_eq!(tree.root().children[0].title, "Section 1");
        assert_eq!(tree.root().children[1].title, "Section 2");
    }

    #[test]
    fn test_build_chunk_tree_nested() {
        let content = "# Parent\n\nParent content.\n\n## Child 1\n\nChild 1 content.\n\n## Child 2\n\nChild 2 content.";
        let path = PathBuf::from("test.md");
        let tree = build_chunk_tree(content, "docs", &path, "Test Doc");

        // Document -> H1 -> [H2, H2]
        assert_eq!(tree.node_count(), 4);
        assert_eq!(tree.root().children.len(), 1);

        let h1 = &tree.root().children[0];
        assert_eq!(h1.title, "Parent");
        assert_eq!(h1.depth, 1);
        assert_eq!(h1.children.len(), 2);
        assert_eq!(h1.children[0].title, "Child 1");
        assert_eq!(h1.children[1].title, "Child 2");
    }

    #[test]
    fn test_build_chunk_tree_deep_nesting() {
        let content = "# H1\n\nH1 content.\n\n## H2\n\nH2 content.\n\n### H3\n\nH3 content.";
        let path = PathBuf::from("test.md");
        let tree = build_chunk_tree(content, "docs", &path, "Doc");

        // Document -> H1 -> H2 -> H3
        assert_eq!(tree.node_count(), 4);

        let h1 = &tree.root().children[0];
        assert_eq!(h1.title, "H1");
        assert_eq!(h1.children.len(), 1);

        let h2 = &h1.children[0];
        assert_eq!(h2.title, "H2");
        assert_eq!(h2.children.len(), 1);

        let h3 = &h2.children[0];
        assert_eq!(h3.title, "H3");
        assert!(h3.children.is_empty());
    }

    #[test]
    fn test_build_chunk_tree_preamble_only() {
        let content = "Just some text without any headings.";
        let path = PathBuf::from("test.md");
        let tree = build_chunk_tree(content, "docs", &path, "My Doc");

        // Only document node
        assert_eq!(tree.node_count(), 1);
        assert_eq!(tree.root().title, "My Doc");
        assert!(tree.root().children.is_empty());
        assert!(tree.has_body(tree.root()));
    }

    #[test]
    fn test_build_chunk_tree_consecutive_headings() {
        // H1 followed by H2 with no content between H1 line and H2 line.
        // H1's span runs from after "# H1\n" to end (since H2 is deeper, it doesn't terminate H1)
        // H2 becomes a child of H1
        let content = "# H1\n## H2\n\nActual content here.";
        let path = PathBuf::from("test.md");
        let tree = build_chunk_tree(content, "docs", &path, "Doc");

        // Structure: Document -> H1 -> H2
        // H1's span includes H2 and content, but H1's body is empty (before H2's heading)
        // H2's body is "Actual content here."
        assert_eq!(tree.node_count(), 3);
        assert_eq!(tree.root().children.len(), 1);
        assert_eq!(tree.root().children[0].title, "H1");
        assert_eq!(tree.root().children[0].children.len(), 1);
        assert_eq!(tree.root().children[0].children[0].title, "H2");

        // H1 has empty body (no content between its span_start and H2's heading_start)
        let h1 = &tree.root().children[0];
        assert!(!tree.has_body(h1)); // H1 body is empty

        // H2 has content
        let h2 = &h1.children[0];
        assert!(tree.has_body(h2));
        assert_eq!(tree.body(h2).trim(), "Actual content here.");
    }

    #[test]
    fn test_build_chunk_tree_consecutive_same_level() {
        // Consecutive H1s with no content between them - first H1's span is empty
        let content = "# H1\n# H2\n\nActual content here.";
        let path = PathBuf::from("test.md");
        let tree = build_chunk_tree(content, "docs", &path, "Doc");

        // H1's span is [5, 5) (empty) because H2 at same level terminates it
        // H1 is filtered out, only H2 remains
        assert_eq!(tree.node_count(), 2);
        assert_eq!(tree.root().children.len(), 1);
        assert_eq!(tree.root().children[0].title, "H2");
    }

    #[test]
    fn test_build_chunk_tree_mixed_depths() {
        let content =
            "# A\n\nA content.\n\n## B\n\nB content.\n\n# C\n\nC content.\n\n## D\n\nD content.";
        let path = PathBuf::from("test.md");
        let tree = build_chunk_tree(content, "docs", &path, "Doc");

        // Document -> [H1(A) -> H2(B), H1(C) -> H2(D)]
        assert_eq!(tree.root().children.len(), 2);

        let a = &tree.root().children[0];
        assert_eq!(a.title, "A");
        assert_eq!(a.children.len(), 1);
        assert_eq!(a.children[0].title, "B");

        let c = &tree.root().children[1];
        assert_eq!(c.title, "C");
        assert_eq!(c.children.len(), 1);
        assert_eq!(c.children[0].title, "D");
    }

    #[test]
    fn test_build_chunk_tree_positions() {
        let content = "# A\n\nContent.\n\n## B\n\nContent.\n\n# C\n\nContent.";
        let path = PathBuf::from("test.md");
        let tree = build_chunk_tree(content, "docs", &path, "Doc");

        let positions: Vec<(String, usize)> = tree
            .iter_preorder()
            .map(|n| (n.title.clone(), n.position))
            .collect();

        // Pre-order: Doc, A, B, C
        assert_eq!(positions[0], ("Doc".to_string(), 0));
        assert_eq!(positions[1], ("A".to_string(), 1));
        assert_eq!(positions[2], ("B".to_string(), 2));
        assert_eq!(positions[3], ("C".to_string(), 3));
    }

    #[test]
    fn test_build_chunk_tree_sibling_counts() {
        let content = "# A\n\nA.\n\n# B\n\nB.\n\n# C\n\nC.";
        let path = PathBuf::from("test.md");
        let tree = build_chunk_tree(content, "docs", &path, "Doc");

        // Document has sibling_count = 1
        assert_eq!(tree.root().sibling_count, 1);

        // All H1s are siblings (count = 3)
        for child in &tree.root().children {
            assert_eq!(child.sibling_count, 3);
        }
    }

    #[test]
    fn test_build_chunk_tree_slugs() {
        let content = "# Overview\n\nFirst.\n\n# Overview\n\nSecond.";
        let path = PathBuf::from("test.md");
        let tree = build_chunk_tree(content, "docs", &path, "Doc");

        let slugs: Vec<Option<String>> = tree
            .root()
            .children
            .iter()
            .map(|n| n.slug.clone())
            .collect();

        assert_eq!(
            slugs,
            vec![Some("overview".to_string()), Some("overview-1".to_string())]
        );
    }

    #[test]
    fn test_build_chunk_tree_ids() {
        let content = "# Section\n\nContent.";
        let path = PathBuf::from("guide.md");
        let tree = build_chunk_tree(content, "docs", &path, "Guide");

        assert_eq!(tree.root().id, "docs:guide.md");
        assert_eq!(tree.root().doc_id, "docs:guide.md");

        let section = &tree.root().children[0];
        assert_eq!(section.id, "docs:guide.md#section");
        assert_eq!(section.doc_id, "docs:guide.md");
        assert_eq!(section.parent_id, Some("docs:guide.md".to_string()));
    }

    #[test]
    fn test_build_chunk_tree_body_extraction() {
        let content = "Preamble.\n\n# Section\n\nSection body.";
        let path = PathBuf::from("test.md");
        let tree = build_chunk_tree(content, "docs", &path, "Doc");

        // Document body is preamble (content before first heading)
        let doc_body = tree.body(tree.root()).trim();
        assert_eq!(doc_body, "Preamble.");

        // Section body
        let section = &tree.root().children[0];
        let section_body = tree.body(section).trim();
        assert_eq!(section_body, "Section body.");
    }

    #[test]
    fn test_build_chunk_tree_skips_level() {
        // H1 followed directly by H3 (skipping H2)
        let content = "# H1\n\nH1 content.\n\n### H3\n\nH3 content.";
        let path = PathBuf::from("test.md");
        let tree = build_chunk_tree(content, "docs", &path, "Doc");

        // H3 should still be a child of H1 (nearest parent with lower depth)
        assert_eq!(tree.node_count(), 3);

        let h1 = &tree.root().children[0];
        assert_eq!(h1.title, "H1");
        assert_eq!(h1.depth, 1);
        assert_eq!(h1.children.len(), 1);

        let h3 = &h1.children[0];
        assert_eq!(h3.title, "H3");
        assert_eq!(h3.depth, 3);
    }

    #[test]
    fn test_build_chunk_tree_empty_document() {
        let content = "";
        let path = PathBuf::from("empty.md");
        let tree = build_chunk_tree(content, "docs", &path, "Empty");

        assert_eq!(tree.node_count(), 1);
        assert_eq!(tree.root().title, "Empty");
        assert!(!tree.has_body(tree.root())); // Empty content
    }

    #[test]
    fn test_build_chunk_tree_only_headings() {
        // Document with only headings, no body content between them
        // All at same level so each has empty span
        let content = "# H1\n# H2\n# H3";
        let path = PathBuf::from("test.md");
        let tree = build_chunk_tree(content, "docs", &path, "Doc");

        // All headings have empty spans, so all filtered out
        // Only document node remains
        assert_eq!(tree.node_count(), 1);
        assert!(!tree.has_body(tree.root())); // No preamble either

        // iter_chunks should return nothing
        assert_eq!(tree.chunk_count(), 0);
    }

    #[test]
    fn test_build_chunk_tree_complex_hierarchy() {
        let content = r#"Preamble.

# Introduction

Intro text.

## Background

Background text.

### Details

Details text.

## Methods

Methods text.

# Results

Results text.

## Analysis

Analysis text.
"#;
        let path = PathBuf::from("paper.md");
        let tree = build_chunk_tree(content, "papers", &path, "My Paper");

        // Structure:
        // Doc (preamble)
        //   ├── Introduction
        //   │   ├── Background
        //   │   │   └── Details
        //   │   └── Methods
        //   └── Results
        //       └── Analysis

        assert_eq!(tree.node_count(), 7);

        let intro = &tree.root().children[0];
        assert_eq!(intro.title, "Introduction");
        assert_eq!(intro.children.len(), 2);

        let background = &intro.children[0];
        assert_eq!(background.title, "Background");
        assert_eq!(background.children.len(), 1);
        assert_eq!(background.children[0].title, "Details");

        let methods = &intro.children[1];
        assert_eq!(methods.title, "Methods");
        assert!(methods.children.is_empty());

        let results = &tree.root().children[1];
        assert_eq!(results.title, "Results");
        assert_eq!(results.children.len(), 1);
        assert_eq!(results.children[0].title, "Analysis");
    }

    #[test]
    fn test_span_calculation() {
        // Test with H1 followed by another H1 (same level terminates span)
        let content = "# H1\n\nH1 body\n\n# H2\n\nH2 body";
        let headings = extract_headings(content);
        let spans = calculate_heading_spans(&headings, content);

        // H1 span: from after "# H1\n\n" (skipping the blank line) to before "# H2"
        // pulldown_cmark includes \n in heading, so heading_end=5, we skip byte 5 (\n), span_start=6
        assert_eq!(
            &content[spans[0].span_start..spans[0].span_end],
            "H1 body\n\n"
        );

        // H2 span: from after "# H2\n\n" to end
        assert_eq!(&content[spans[1].span_start..spans[1].span_end], "H2 body");
    }

    #[test]
    fn test_span_calculation_nested() {
        // Test with H1 followed by H2 (H2 does NOT terminate H1's span)
        let content = "# H1\n\nH1 body\n\n## H2\n\nH2 body";
        let headings = extract_headings(content);
        let spans = calculate_heading_spans(&headings, content);

        // H1 span: from after "# H1\n\n" to end (H2 doesn't terminate H1)
        assert_eq!(
            &content[spans[0].span_start..spans[0].span_end],
            "H1 body\n\n## H2\n\nH2 body"
        );

        // H2 span: from after "## H2\n\n" to end
        assert_eq!(&content[spans[1].span_start..spans[1].span_end], "H2 body");
    }
}
