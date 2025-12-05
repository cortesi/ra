//! Plain text parser for term extraction.
//!
//! This parser handles plain text files by treating all content as body text.
//! It serves as a fallback for file types that don't have a specialized parser.

use std::path::Path;

use super::ContentParser;
use crate::{Stopwords, WeightedTerm, parser::extract_terms_from_text};

/// Weight for body text.
const WEIGHT_BODY: f32 = 1.0;

/// Parser for plain text files.
///
/// Extracts all terms with body-level weight since plain text has no
/// structural hierarchy.
pub struct TextParser {
    stopwords: Stopwords,
    min_term_length: usize,
}

impl Default for TextParser {
    fn default() -> Self {
        Self::new()
    }
}

impl TextParser {
    /// Creates a new text parser with default settings.
    pub fn new() -> Self {
        Self {
            stopwords: Stopwords::new(),
            min_term_length: 3,
        }
    }

    /// Creates a text parser with custom settings.
    pub fn with_settings(stopwords: Stopwords, min_term_length: usize) -> Self {
        Self {
            stopwords,
            min_term_length,
        }
    }
}

impl ContentParser for TextParser {
    fn can_parse(&self, path: &Path) -> bool {
        // Text parser is the fallback - it can parse anything
        // But we prefer it for .txt files explicitly
        path.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("txt"))
    }

    fn parse(&self, _path: &Path, content: &str) -> Vec<WeightedTerm> {
        extract_terms_from_text(
            content,
            "body",
            WEIGHT_BODY,
            &self.stopwords,
            self.min_term_length,
        )
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn can_parse_txt_files() {
        let parser = TextParser::new();
        assert!(parser.can_parse(Path::new("file.txt")));
        assert!(parser.can_parse(Path::new("file.TXT")));
        assert!(!parser.can_parse(Path::new("file.md")));
        assert!(!parser.can_parse(Path::new("file.rs")));
    }

    #[test]
    fn parse_extracts_terms() {
        let parser = TextParser::new();
        let terms = parser.parse(
            Path::new("test.txt"),
            "Authentication and authorization are fundamental security concepts.",
        );

        let term_strings: Vec<_> = terms.iter().map(|t| t.term.as_str()).collect();
        assert!(term_strings.contains(&"authentication"));
        assert!(term_strings.contains(&"authorization"));
        assert!(term_strings.contains(&"fundamental"));
        assert!(term_strings.contains(&"security"));
        assert!(term_strings.contains(&"concepts"));
    }

    #[test]
    fn parse_filters_stopwords() {
        let parser = TextParser::new();
        // Use Rust keywords which are definitely stopwords
        let terms = parser.parse(Path::new("test.txt"), "The struct implements a trait.");

        let term_strings: Vec<_> = terms.iter().map(|t| t.term.as_str()).collect();
        // "the", "struct", "implements", "a", "trait" are stopwords
        assert!(!term_strings.contains(&"the"));
        assert!(!term_strings.contains(&"struct"));
        assert!(!term_strings.contains(&"trait"));
    }

    #[test]
    fn parse_assigns_body_weight() {
        let parser = TextParser::new();
        let terms = parser.parse(Path::new("test.txt"), "encryption decryption");

        for term in &terms {
            assert_eq!(term.source, "body");
            assert_eq!(term.weight, 1.0);
        }
    }

    #[test]
    fn parse_counts_frequency() {
        let parser = TextParser::new();
        let terms = parser.parse(
            Path::new("test.txt"),
            "kubernetes kubernetes kubernetes docker",
        );

        let k8s_term = terms.iter().find(|t| t.term == "kubernetes").unwrap();
        assert_eq!(k8s_term.frequency, 3);

        let docker_term = terms.iter().find(|t| t.term == "docker").unwrap();
        assert_eq!(docker_term.frequency, 1);
    }
}
