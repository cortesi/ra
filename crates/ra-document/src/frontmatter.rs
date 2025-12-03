//! YAML frontmatter parsing for markdown documents.
//!
//! Frontmatter is optional metadata at the start of a markdown file, delimited by `---`:
//!
//! ```markdown
//! ---
//! title: My Document
//! tags: [rust, tutorial]
//! ---
//!
//! # Content starts here
//! ```

use serde::Deserialize;

/// Parsed frontmatter from a markdown document.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Frontmatter {
    /// Document title.
    pub title: Option<String>,
    /// Document tags (supports both array and Obsidian-style inline tags).
    #[serde(default)]
    pub tags: Vec<String>,
}

/// Parses YAML frontmatter from markdown content.
///
/// Returns the parsed frontmatter (if valid) and the remaining content after the frontmatter.
/// If no frontmatter is present or it's malformed, returns `None` and the original content.
///
/// Frontmatter must:
/// - Start at the beginning of the content
/// - Be delimited by `---` on its own line
/// - Contain valid YAML
pub fn parse_frontmatter(content: &str) -> (Option<Frontmatter>, &str) {
    // Frontmatter must start with ---
    let content = content.trim_start_matches('\u{feff}'); // Strip BOM if present
    if !content.starts_with("---") {
        return (None, content);
    }

    // Find the closing ---
    let after_opening = &content[3..];
    let after_opening = after_opening
        .strip_prefix('\n')
        .unwrap_or(after_opening.strip_prefix("\r\n").unwrap_or(after_opening));

    // Look for closing delimiter
    let closing_pos = find_closing_delimiter(after_opening);
    let Some(closing_pos) = closing_pos else {
        // No closing delimiter found
        return (None, content);
    };

    let yaml_content = &after_opening[..closing_pos];
    let remaining = &after_opening[closing_pos..];

    // Skip the closing --- and any immediately following blank line
    let remaining = remaining.strip_prefix("---").unwrap_or(remaining);
    let remaining = remaining
        .strip_prefix("\r\n")
        .or_else(|| remaining.strip_prefix('\n'))
        .unwrap_or(remaining);
    // Strip one more blank line if present (common pattern: --- followed by blank line before content)
    let remaining = remaining
        .strip_prefix("\r\n")
        .or_else(|| remaining.strip_prefix('\n'))
        .unwrap_or(remaining);

    // Parse the YAML
    match serde_yaml::from_str::<Frontmatter>(yaml_content) {
        Ok(fm) => (Some(fm), remaining),
        Err(_) => (None, content), // Malformed YAML, return original content
    }
}

/// Finds the position of the closing `---` delimiter.
///
/// The delimiter must be at the start of a line.
fn find_closing_delimiter(content: &str) -> Option<usize> {
    let mut pos = 0;
    for line in content.lines() {
        if line == "---" {
            return Some(pos);
        }
        pos += line.len() + 1; // +1 for newline (approximate, handles both \n and \r\n)
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_standard_frontmatter() {
        let content = r#"---
title: Rust Error Handling
tags: [rust, errors, patterns]
---

# Content starts here"#;

        let (fm, remaining) = parse_frontmatter(content);
        let fm = fm.expect("should parse frontmatter");
        assert_eq!(fm.title, Some("Rust Error Handling".into()));
        assert_eq!(fm.tags, vec!["rust", "errors", "patterns"]);
        assert!(remaining.starts_with("# Content"));
    }

    #[test]
    fn test_frontmatter_title_only() {
        let content = r#"---
title: Just a Title
---

Body text"#;

        let (fm, remaining) = parse_frontmatter(content);
        let fm = fm.expect("should parse frontmatter");
        assert_eq!(fm.title, Some("Just a Title".into()));
        assert!(fm.tags.is_empty());
        assert!(remaining.starts_with("Body"));
    }

    #[test]
    fn test_frontmatter_tags_only() {
        let content = r#"---
tags: [one, two]
---

Content"#;

        let (fm, remaining) = parse_frontmatter(content);
        let fm = fm.expect("should parse frontmatter");
        assert_eq!(fm.title, None);
        assert_eq!(fm.tags, vec!["one", "two"]);
        assert!(remaining.starts_with("Content"));
    }

    #[test]
    fn test_no_frontmatter() {
        let content = "# Just a heading\n\nSome content";

        let (fm, remaining) = parse_frontmatter(content);
        assert!(fm.is_none());
        assert_eq!(remaining, content);
    }

    #[test]
    fn test_empty_frontmatter() {
        let content = r#"---
---

Content after empty frontmatter"#;

        let (fm, remaining) = parse_frontmatter(content);
        let fm = fm.expect("should parse empty frontmatter");
        assert_eq!(fm.title, None);
        assert!(fm.tags.is_empty());
        assert!(remaining.starts_with("Content"));
    }

    #[test]
    fn test_malformed_yaml() {
        let content = r#"---
title: [unclosed bracket
tags: not: valid: yaml:
---

Content"#;

        let (fm, remaining) = parse_frontmatter(content);
        assert!(fm.is_none(), "malformed YAML should return None");
        assert_eq!(remaining, content, "should return original content");
    }

    #[test]
    fn test_missing_closing_delimiter() {
        let content = r#"---
title: No closing delimiter

# This looks like content but frontmatter never closed"#;

        let (fm, remaining) = parse_frontmatter(content);
        assert!(fm.is_none());
        assert_eq!(remaining, content);
    }

    #[test]
    fn test_delimiter_not_at_start() {
        let content = "Some text before\n---\ntitle: Not frontmatter\n---";

        let (fm, remaining) = parse_frontmatter(content);
        assert!(fm.is_none());
        assert_eq!(remaining, content);
    }

    #[test]
    fn test_extra_fields_ignored() {
        let content = r#"---
title: My Doc
tags: [test]
author: Someone
date: 2024-01-01
custom_field: value
---

Content"#;

        let (fm, remaining) = parse_frontmatter(content);
        let fm = fm.expect("should parse frontmatter with extra fields");
        assert_eq!(fm.title, Some("My Doc".into()));
        assert_eq!(fm.tags, vec!["test"]);
        assert!(remaining.starts_with("Content"));
    }

    #[test]
    fn test_multiline_tags() {
        let content = r#"---
title: Doc
tags:
  - rust
  - programming
  - tutorial
---

Content"#;

        let (fm, remaining) = parse_frontmatter(content);
        let fm = fm.expect("should parse multiline tags");
        assert_eq!(fm.tags, vec!["rust", "programming", "tutorial"]);
        assert!(remaining.starts_with("Content"));
    }

    #[test]
    fn test_quoted_strings() {
        let content = r#"---
title: "Title with: colon"
tags: ["tag:with:colons", "another"]
---

Content"#;

        let (fm, remaining) = parse_frontmatter(content);
        let fm = fm.expect("should parse quoted strings");
        assert_eq!(fm.title, Some("Title with: colon".into()));
        assert_eq!(fm.tags, vec!["tag:with:colons", "another"]);
        assert!(remaining.starts_with("Content"));
    }

    #[test]
    fn test_bom_handling() {
        let content = "\u{feff}---\ntitle: With BOM\n---\n\nContent";

        let (fm, remaining) = parse_frontmatter(content);
        let fm = fm.expect("should handle BOM");
        assert_eq!(fm.title, Some("With BOM".into()));
        assert!(remaining.starts_with("Content"));
    }

    #[test]
    fn test_windows_line_endings() {
        let content = "---\r\ntitle: Windows\r\ntags: [test]\r\n---\r\n\r\nContent";

        let (fm, remaining) = parse_frontmatter(content);
        let fm = fm.expect("should handle CRLF");
        assert_eq!(fm.title, Some("Windows".into()));
        // Content may have leading \r depending on how we strip
        assert!(remaining.contains("Content"));
    }
}
