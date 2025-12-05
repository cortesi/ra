//! Markdown parser for term extraction.
//!
//! This parser understands markdown structure and assigns different weights
//! to terms based on their location (headings vs body).

use std::path::Path;

use ra_document::{HeadingInfo, extract_headings, parse_frontmatter};

use super::ContentParser;
use crate::{Stopwords, WeightedTerm, parser::extract_terms_from_text};

/// Weight for H1 headings.
const WEIGHT_H1: f32 = 3.0;
/// Weight for H2-H3 headings.
const WEIGHT_H2_H3: f32 = 2.0;
/// Weight for H4-H6 headings.
const WEIGHT_H4_H6: f32 = 1.5;
/// Weight for body text.
const WEIGHT_BODY: f32 = 1.0;

/// Parser for markdown files.
///
/// Extracts terms with weights based on structural position:
/// - H1 headings: weight 3.0
/// - H2-H3 headings: weight 2.0
/// - H4-H6 headings: weight 1.5
/// - Body text: weight 1.0
pub struct MarkdownParser {
    stopwords: Stopwords,
    min_term_length: usize,
}

impl Default for MarkdownParser {
    fn default() -> Self {
        Self::new()
    }
}

impl MarkdownParser {
    /// Creates a new markdown parser with default settings.
    pub fn new() -> Self {
        Self {
            stopwords: Stopwords::new(),
            min_term_length: 3,
        }
    }

    /// Creates a markdown parser with custom settings.
    pub fn with_settings(stopwords: Stopwords, min_term_length: usize) -> Self {
        Self {
            stopwords,
            min_term_length,
        }
    }

    /// Returns the source label and weight for a heading level.
    fn heading_weight(level: u8) -> (&'static str, f32) {
        match level {
            1 => ("md:h1", WEIGHT_H1),
            2 | 3 => ("md:h2-h3", WEIGHT_H2_H3),
            _ => ("md:h4-h6", WEIGHT_H4_H6),
        }
    }

    /// Extracts terms from headings with appropriate weights.
    fn extract_heading_terms(&self, headings: &[HeadingInfo]) -> Vec<WeightedTerm> {
        let mut terms = Vec::new();

        for heading in headings {
            let (source, weight) = Self::heading_weight(heading.level);
            let heading_terms = extract_terms_from_text(
                &heading.text,
                source,
                weight,
                &self.stopwords,
                self.min_term_length,
            );
            terms.extend(heading_terms);
        }

        terms
    }

    /// Extracts body text by removing heading lines from content.
    fn extract_body_text(&self, content: &str, headings: &[HeadingInfo]) -> String {
        if headings.is_empty() {
            return content.to_string();
        }

        // Build body by collecting text between headings
        let mut body = String::new();
        let mut last_end = 0;

        for heading in headings {
            // Add text before this heading
            if heading.heading_start > last_end {
                body.push_str(&content[last_end..heading.heading_start]);
            }
            // Skip past the heading line
            last_end = heading.heading_end;
        }

        // Add remaining text after last heading
        if last_end < content.len() {
            body.push_str(&content[last_end..]);
        }

        body
    }
}

impl ContentParser for MarkdownParser {
    fn can_parse(&self, path: &Path) -> bool {
        path.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|ext| {
                ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown")
            })
    }

    fn parse(&self, _path: &Path, content: &str) -> Vec<WeightedTerm> {
        // Skip frontmatter
        let (_frontmatter, content) = parse_frontmatter(content);

        // Extract headings
        let headings = extract_headings(content);

        // Extract terms from headings with appropriate weights
        let mut terms = self.extract_heading_terms(&headings);

        // Extract body text (content minus heading lines)
        let body = self.extract_body_text(content, &headings);

        // Extract terms from body
        let body_terms = extract_terms_from_text(
            &body,
            "body",
            WEIGHT_BODY,
            &self.stopwords,
            self.min_term_length,
        );
        terms.extend(body_terms);

        terms
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn can_parse_markdown_files() {
        let parser = MarkdownParser::new();
        assert!(parser.can_parse(Path::new("file.md")));
        assert!(parser.can_parse(Path::new("file.MD")));
        assert!(parser.can_parse(Path::new("file.markdown")));
        assert!(!parser.can_parse(Path::new("file.txt")));
        assert!(!parser.can_parse(Path::new("file.rs")));
    }

    #[test]
    fn parse_extracts_heading_terms() {
        let parser = MarkdownParser::new();
        let content = "# Authentication Guide\n\nSome body text.";
        let terms = parser.parse(Path::new("test.md"), content);

        let auth_term = terms.iter().find(|t| t.term == "authentication");
        assert!(auth_term.is_some());

        let auth = auth_term.unwrap();
        assert_eq!(auth.source, "md:h1");
        assert_eq!(auth.weight, 3.0);
    }

    #[test]
    fn parse_assigns_correct_heading_weights() {
        let parser = MarkdownParser::new();
        let content = r#"# Primary Heading
## Secondary Heading
### Tertiary Heading
#### Quaternary Heading
"#;
        let terms = parser.parse(Path::new("test.md"), content);

        let find_term = |name: &str| terms.iter().find(|t| t.term == name);

        let primary = find_term("primary").unwrap();
        assert_eq!(primary.source, "md:h1");
        assert_eq!(primary.weight, 3.0);

        let secondary = find_term("secondary").unwrap();
        assert_eq!(secondary.source, "md:h2-h3");
        assert_eq!(secondary.weight, 2.0);

        let tertiary = find_term("tertiary").unwrap();
        assert_eq!(tertiary.source, "md:h2-h3");
        assert_eq!(tertiary.weight, 2.0);

        let quaternary = find_term("quaternary").unwrap();
        assert_eq!(quaternary.source, "md:h4-h6");
        assert_eq!(quaternary.weight, 1.5);
    }

    #[test]
    fn parse_extracts_body_terms() {
        let parser = MarkdownParser::new();
        let content = "# Title\n\nKubernetes orchestrates containers efficiently.";
        let terms = parser.parse(Path::new("test.md"), content);

        let k8s_term = terms.iter().find(|t| t.term == "kubernetes");
        assert!(k8s_term.is_some());

        let k8s = k8s_term.unwrap();
        assert_eq!(k8s.source, "body");
        assert_eq!(k8s.weight, 1.0);
    }

    #[test]
    fn parse_skips_frontmatter() {
        let parser = MarkdownParser::new();
        let content = r#"---
title: Frontmatter Title
tags: [rust, guide]
---

# Actual Content

Body text here.
"#;
        let terms = parser.parse(Path::new("test.md"), content);

        // Terms from frontmatter should not appear
        let term_strings: Vec<_> = terms.iter().map(|t| t.term.as_str()).collect();
        assert!(!term_strings.contains(&"frontmatter"));

        // But heading terms should
        assert!(term_strings.contains(&"actual"));
        assert!(term_strings.contains(&"content"));
    }

    #[test]
    fn parse_filters_stopwords() {
        let parser = MarkdownParser::new();
        // Use Rust keywords which are definitely stopwords
        let content = "# The Struct Implements\n\nThis trait defines behavior.";
        let terms = parser.parse(Path::new("test.md"), content);

        let term_strings: Vec<_> = terms.iter().map(|t| t.term.as_str()).collect();
        // "the", "struct", "implements", "this", "trait", "defines" are stopwords
        assert!(!term_strings.contains(&"the"));
        assert!(!term_strings.contains(&"struct"));
        assert!(!term_strings.contains(&"trait"));
    }

    #[test]
    fn heading_weight_classification() {
        assert_eq!(MarkdownParser::heading_weight(1), ("md:h1", 3.0));
        assert_eq!(MarkdownParser::heading_weight(2), ("md:h2-h3", 2.0));
        assert_eq!(MarkdownParser::heading_weight(3), ("md:h2-h3", 2.0));
        assert_eq!(MarkdownParser::heading_weight(4), ("md:h4-h6", 1.5));
        assert_eq!(MarkdownParser::heading_weight(5), ("md:h4-h6", 1.5));
        assert_eq!(MarkdownParser::heading_weight(6), ("md:h4-h6", 1.5));
    }
}
