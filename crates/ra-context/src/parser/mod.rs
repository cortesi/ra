//! Content parsers for term extraction.
//!
//! This module provides parsers that extract weighted terms from different file types.
//! Each parser understands the structure of its file type and assigns appropriate
//! weights based on where terms appear (headings, body, etc.).

mod markdown;
mod text;

use std::path::Path;

pub use markdown::MarkdownParser;
pub use text::TextParser;

use crate::{Stopwords, WeightedTerm};

/// A parser that extracts weighted terms from file content.
pub trait ContentParser {
    /// Checks if this parser can handle the given file path.
    ///
    /// Typically based on file extension.
    fn can_parse(&self, path: &Path) -> bool;

    /// Extracts weighted terms from the file content.
    ///
    /// The parser should:
    /// - Tokenize the content appropriately for the file type
    /// - Assign weights based on structural position (headings, body, etc.)
    /// - Filter stopwords
    /// - Aggregate duplicate terms by incrementing frequency
    fn parse(&self, path: &Path, content: &str) -> Vec<WeightedTerm>;
}

/// Tokenizes text into individual terms.
///
/// Splits on whitespace and punctuation, lowercases, and filters based on length.
pub fn tokenize(text: &str, min_length: usize) -> impl Iterator<Item = String> + '_ {
    text.split(|c: char| c.is_whitespace() || c.is_ascii_punctuation())
        .map(|s| s.to_ascii_lowercase())
        .filter(move |s| s.len() >= min_length && s.chars().all(|c| c.is_alphanumeric()))
}

/// Extracts terms from text, filtering stopwords and aggregating by frequency.
///
/// # Arguments
/// * `text` - The text to extract terms from
/// * `source` - Human-readable source label (e.g., "body", "md:h1")
/// * `weight` - Semantic weight for terms from this source
/// * `stopwords` - Stopwords to filter out
/// * `min_length` - Minimum term length
pub fn extract_terms_from_text(
    text: &str,
    source: &str,
    weight: f32,
    stopwords: &Stopwords,
    min_length: usize,
) -> Vec<WeightedTerm> {
    use std::collections::HashMap;

    let mut term_counts: HashMap<String, u32> = HashMap::new();

    for token in tokenize(text, min_length) {
        if !stopwords.contains(&token) {
            *term_counts.entry(token).or_insert(0) += 1;
        }
    }

    term_counts
        .into_iter()
        .map(|(term, freq)| {
            let mut wt = WeightedTerm::new(term, source, weight);
            // Set frequency (we already counted, so set directly instead of incrementing)
            wt.frequency = freq;
            wt
        })
        .collect()
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn tokenize_basic() {
        let tokens: Vec<_> = tokenize("Hello World", 1).collect();
        assert_eq!(tokens, vec!["hello", "world"]);
    }

    #[test]
    fn tokenize_with_punctuation() {
        let tokens: Vec<_> = tokenize("fn main() { println!(\"hello\"); }", 1).collect();
        assert!(tokens.contains(&"fn".to_string()));
        assert!(tokens.contains(&"main".to_string()));
        assert!(tokens.contains(&"println".to_string()));
        assert!(tokens.contains(&"hello".to_string()));
    }

    #[test]
    fn tokenize_filters_short() {
        let tokens: Vec<_> = tokenize("a ab abc abcd", 3).collect();
        assert_eq!(tokens, vec!["abc", "abcd"]);
    }

    #[test]
    fn extract_terms_filters_stopwords() {
        let stopwords = Stopwords::new();
        let terms = extract_terms_from_text("the quick brown fox", "body", 1.0, &stopwords, 2);

        let term_strings: Vec<_> = terms.iter().map(|t| t.term.as_str()).collect();
        assert!(!term_strings.contains(&"the"));
        assert!(term_strings.contains(&"quick"));
        assert!(term_strings.contains(&"brown"));
        assert!(term_strings.contains(&"fox"));
    }

    #[test]
    fn extract_terms_counts_frequency() {
        let stopwords = Stopwords::new();
        let terms = extract_terms_from_text("rust rust rust code", "body", 1.0, &stopwords, 2);

        let rust_term = terms.iter().find(|t| t.term == "rust").unwrap();
        assert_eq!(rust_term.frequency, 3);
    }
}
