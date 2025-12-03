//! Adaptive markdown chunking.
//!
//! Splits markdown documents at heading boundaries using adaptive chunking:
//! - Find the first heading level with 2+ headings
//! - Split the document at that level
//! - Skip chunking for small documents (under min_chunk_size)

use pulldown_cmark::{Event, HeadingLevel, Parser, Tag, TagEnd};

use crate::Slugifier;

/// A parsed heading from markdown content.
#[derive(Debug, Clone)]
pub struct Heading {
    /// The heading level (h1-h6).
    pub level: HeadingLevel,
    /// The heading text.
    pub text: String,
    /// Byte offset where the heading starts in the source.
    pub start: usize,
    /// Byte offset where the heading ends in the source.
    pub end: usize,
}

/// A chunk of markdown content with metadata.
#[derive(Debug, Clone)]
pub struct ChunkData {
    /// The slug for this chunk (used in fragment ID).
    pub slug: String,
    /// The chunk title.
    pub title: String,
    /// The chunk body content.
    pub body: String,
    /// Whether this is the preamble (content before first heading at chunk level).
    pub is_preamble: bool,
    /// Breadcrumb showing hierarchy path.
    pub breadcrumb: String,
}

/// Extracts all headings from markdown content with their positions.
pub fn extract_headings(content: &str) -> Vec<Heading> {
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
                    headings.push(Heading {
                        level,
                        text,
                        start,
                        end: range.end,
                    });
                }
            }
            _ => {}
        }
    }

    headings
}

/// Determines which heading level to chunk at.
///
/// Returns the first heading level that has 2 or more headings,
/// or None if no such level exists or the document is too small.
pub fn determine_chunk_level(content: &str, min_size: usize) -> Option<HeadingLevel> {
    if content.len() < min_size {
        return None;
    }

    let headings = extract_headings(content);

    // Count headings at each level
    let mut counts = [0usize; 6];
    for heading in &headings {
        let idx = heading_level_to_index(heading.level);
        counts[idx] += 1;
    }

    // Find first level with 2+ headings
    for (idx, &count) in counts.iter().enumerate() {
        if count >= 2 {
            return index_to_heading_level(idx);
        }
    }

    None
}

/// Chunks markdown content at the determined heading level.
///
/// Returns a vector of chunks, each containing:
/// - A slug for the chunk ID
/// - The chunk title
/// - The chunk body
/// - Whether it's the preamble
/// - A breadcrumb string showing the hierarchy path
pub fn chunk_markdown(content: &str, doc_title: &str, min_chunk_size: usize) -> Vec<ChunkData> {
    let chunk_level = determine_chunk_level(content, min_chunk_size);

    let Some(chunk_level) = chunk_level else {
        // No chunking - return entire document as single chunk
        return vec![ChunkData {
            slug: "preamble".to_string(),
            title: doc_title.to_string(),
            body: content.to_string(),
            is_preamble: true,
            breadcrumb: format!("> {}", doc_title),
        }];
    };

    let headings = extract_headings(content);
    let mut chunks = Vec::new();
    let mut slugifier = Slugifier::new();

    // Track parent headings for breadcrumbs
    let mut parent_stack: Vec<(HeadingLevel, String)> = Vec::new();

    // Find headings at chunk level and their positions
    let chunk_headings: Vec<&Heading> =
        headings.iter().filter(|h| h.level == chunk_level).collect();

    // Handle preamble (content before first chunk-level heading)
    // Only create preamble if there's non-heading content (text, paragraphs, etc.)
    let first_chunk_start = chunk_headings
        .first()
        .map(|h| h.start)
        .unwrap_or(content.len());
    if first_chunk_start > 0 {
        let preamble_content = content[..first_chunk_start].trim();
        if !preamble_content.is_empty() && has_non_heading_content(preamble_content) {
            chunks.push(ChunkData {
                slug: "preamble".to_string(),
                title: doc_title.to_string(),
                body: preamble_content.to_string(),
                is_preamble: true,
                breadcrumb: format!("> {}", doc_title),
            });
        }
    }

    // Process each chunk-level heading
    for (i, heading) in chunk_headings.iter().enumerate() {
        let chunk_start = heading.start;
        let chunk_end = chunk_headings
            .get(i + 1)
            .map(|h| h.start)
            .unwrap_or(content.len());

        let body = content[chunk_start..chunk_end].trim().to_string();
        let slug = slugifier.slugify(&heading.text);

        // Build breadcrumb from parent headings
        update_parent_stack(&mut parent_stack, &headings, heading, chunk_level);
        let breadcrumb = build_breadcrumb(doc_title, &parent_stack, &heading.text);

        chunks.push(ChunkData {
            slug,
            title: heading.text.clone(),
            body,
            is_preamble: false,
            breadcrumb,
        });
    }

    chunks
}

/// Updates the parent stack based on headings before the current chunk heading.
fn update_parent_stack(
    parent_stack: &mut Vec<(HeadingLevel, String)>,
    all_headings: &[Heading],
    current: &Heading,
    chunk_level: HeadingLevel,
) {
    parent_stack.clear();

    // Find all headings before this one that are at a higher level (lower number)
    for heading in all_headings {
        if heading.start >= current.start {
            break;
        }

        if heading.level < chunk_level {
            // This is a parent heading - update the stack
            // Remove any parents at same or lower level
            while parent_stack
                .last()
                .is_some_and(|(level, _)| *level >= heading.level)
            {
                parent_stack.pop();
            }
            parent_stack.push((heading.level, heading.text.clone()));
        }
    }
}

/// Builds a breadcrumb string from the document title and parent headings.
fn build_breadcrumb(
    doc_title: &str,
    parent_stack: &[(HeadingLevel, String)],
    chunk_title: &str,
) -> String {
    let mut parts = vec![doc_title.to_string()];
    for (_, title) in parent_stack {
        parts.push(title.clone());
    }
    parts.push(chunk_title.to_string());

    format!("> {}", parts.join(" › "))
}

/// Checks if content has any non-heading elements (paragraphs, lists, etc.).
fn has_non_heading_content(content: &str) -> bool {
    let parser = Parser::new(content);
    for event in parser {
        match event {
            // Text outside of headings indicates real content
            Event::Text(_) | Event::Code(_) => {
                // We need to check if we're inside a heading or not
                // Simpler approach: check if there's any paragraph, list, etc.
            }
            Event::Start(Tag::Paragraph)
            | Event::Start(Tag::List(_))
            | Event::Start(Tag::BlockQuote(_))
            | Event::Start(Tag::CodeBlock(_))
            | Event::Start(Tag::Table(_)) => return true,
            _ => {}
        }
    }
    false
}

/// Converts a heading level to an array index (0-5).
fn heading_level_to_index(level: HeadingLevel) -> usize {
    match level {
        HeadingLevel::H1 => 0,
        HeadingLevel::H2 => 1,
        HeadingLevel::H3 => 2,
        HeadingLevel::H4 => 3,
        HeadingLevel::H5 => 4,
        HeadingLevel::H6 => 5,
    }
}

/// Converts an array index (0-5) to a heading level.
fn index_to_heading_level(idx: usize) -> Option<HeadingLevel> {
    match idx {
        0 => Some(HeadingLevel::H1),
        1 => Some(HeadingLevel::H2),
        2 => Some(HeadingLevel::H3),
        3 => Some(HeadingLevel::H4),
        4 => Some(HeadingLevel::H5),
        5 => Some(HeadingLevel::H6),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_headings() {
        let content = "# Heading 1\n\nSome text\n\n## Heading 2\n\nMore text";
        let headings = extract_headings(content);

        assert_eq!(headings.len(), 2);
        assert_eq!(headings[0].text, "Heading 1");
        assert_eq!(headings[0].level, HeadingLevel::H1);
        assert_eq!(headings[1].text, "Heading 2");
        assert_eq!(headings[1].level, HeadingLevel::H2);
    }

    #[test]
    fn test_extract_headings_with_code() {
        let content = "# The `Result<T>` Type\n\nContent";
        let headings = extract_headings(content);

        assert_eq!(headings.len(), 1);
        assert_eq!(headings[0].text, "The Result<T> Type");
    }

    #[test]
    fn test_determine_chunk_level_multiple_h1() {
        let content = "# First\n\n# Second\n\n# Third";
        let level = determine_chunk_level(content, 0);
        assert_eq!(level, Some(HeadingLevel::H1));
    }

    #[test]
    fn test_determine_chunk_level_single_h1_multiple_h2() {
        let content = "# Title\n\n## Section 1\n\n## Section 2\n\n## Section 3";
        let level = determine_chunk_level(content, 0);
        assert_eq!(level, Some(HeadingLevel::H2));
    }

    #[test]
    fn test_determine_chunk_level_no_repeated() {
        let content = "# Title\n\n## Section\n\n### Subsection";
        let level = determine_chunk_level(content, 0);
        assert_eq!(level, None);
    }

    #[test]
    fn test_determine_chunk_level_too_small() {
        let content = "# First\n\n# Second";
        let level = determine_chunk_level(content, 10000);
        assert_eq!(level, None);
    }

    #[test]
    fn test_chunk_markdown_multiple_h1() {
        let content = "# First Section\n\nFirst content.\n\n# Second Section\n\nSecond content.";
        let chunks = chunk_markdown(content, "Test Doc", 0);

        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].title, "First Section");
        assert_eq!(chunks[0].slug, "first-section");
        assert!(!chunks[0].is_preamble);
        assert_eq!(chunks[1].title, "Second Section");
        assert_eq!(chunks[1].slug, "second-section");
    }

    #[test]
    fn test_chunk_markdown_with_preamble() {
        let content =
            "Intro paragraph.\n\n# First Section\n\nContent.\n\n# Second Section\n\nMore content.";
        let chunks = chunk_markdown(content, "Test Doc", 0);

        assert_eq!(chunks.len(), 3);
        assert!(chunks[0].is_preamble);
        assert_eq!(chunks[0].title, "Test Doc");
        assert_eq!(chunks[0].slug, "preamble");
        assert!(chunks[0].body.contains("Intro paragraph"));

        assert!(!chunks[1].is_preamble);
        assert_eq!(chunks[1].title, "First Section");
    }

    #[test]
    fn test_chunk_markdown_no_chunking_small_doc() {
        let content = "# First\n\n# Second";
        let chunks = chunk_markdown(content, "Test Doc", 10000);

        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].is_preamble);
        assert_eq!(chunks[0].title, "Test Doc");
        assert!(chunks[0].body.contains("# First"));
    }

    #[test]
    fn test_chunk_markdown_no_repeated_levels() {
        let content = "# Title\n\n## Section\n\n### Subsection";
        let chunks = chunk_markdown(content, "Test Doc", 0);

        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].is_preamble);
    }

    #[test]
    fn test_chunk_markdown_empty() {
        let content = "";
        let chunks = chunk_markdown(content, "Test Doc", 0);

        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].is_preamble);
    }

    #[test]
    fn test_breadcrumb_simple() {
        let content = "# First\n\nContent 1.\n\n# Second\n\nContent 2.";
        let chunks = chunk_markdown(content, "My Doc", 0);

        assert_eq!(chunks[0].breadcrumb, "> My Doc › First");
        assert_eq!(chunks[1].breadcrumb, "> My Doc › Second");
    }

    #[test]
    fn test_breadcrumb_nested() {
        let content = "# Parent\n\n## Child 1\n\nContent.\n\n## Child 2\n\nMore content.";
        let chunks = chunk_markdown(content, "My Doc", 0);

        // Chunking at h2 level, so h1 "Parent" is a parent
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].breadcrumb, "> My Doc › Parent › Child 1");
        assert_eq!(chunks[1].breadcrumb, "> My Doc › Parent › Child 2");
    }

    #[test]
    fn test_breadcrumb_preamble() {
        let content = "Intro.\n\n# Section\n\nContent.\n\n# Another\n\nMore.";
        let chunks = chunk_markdown(content, "My Doc", 0);

        assert_eq!(chunks[0].breadcrumb, "> My Doc");
        assert!(chunks[0].is_preamble);
    }

    #[test]
    fn test_breadcrumb_deep_nesting() {
        // Single h1, single h2, multiple h3 -> chunks at h3
        let content =
            "# Title\n\n## Chapter\n\n### Section 1\n\nContent.\n\n### Section 2\n\nMore.";
        let chunks = chunk_markdown(content, "Doc", 0);

        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].breadcrumb, "> Doc › Title › Chapter › Section 1");
        assert_eq!(chunks[1].breadcrumb, "> Doc › Title › Chapter › Section 2");
    }

    #[test]
    fn test_duplicate_heading_slugs() {
        let content = "# Overview\n\nFirst.\n\n# Overview\n\nSecond.";
        let chunks = chunk_markdown(content, "Doc", 0);

        assert_eq!(chunks[0].slug, "overview");
        assert_eq!(chunks[1].slug, "overview-1");
    }
}
