//! GitHub-compatible heading slug generation.
//!
//! Slugs are used to generate unique, stable identifiers for headings in markdown documents.
//! The algorithm follows GitHub's conventions:
//! - Lowercase the text
//! - Remove punctuation except hyphens and spaces
//! - Replace spaces with hyphens
//! - Collapse consecutive hyphens
//! - Trim leading/trailing hyphens
//! - Append `-N` suffix for duplicate slugs

use std::collections::HashMap;

/// Generates URL-compatible slugs from heading text.
///
/// Tracks previously generated slugs to ensure uniqueness by appending
/// numeric suffixes to duplicates.
#[derive(Debug, Default)]
pub struct Slugifier {
    /// Count of how many times each base slug has been used.
    counts: HashMap<String, usize>,
}

impl Slugifier {
    /// Creates a new slugifier with no prior slugs.
    pub fn new() -> Self {
        Self::default()
    }

    /// Generates a GitHub-compatible slug from heading text.
    ///
    /// The algorithm:
    /// 1. Convert to lowercase
    /// 2. Remove all characters except alphanumeric, hyphens, spaces, and underscores
    /// 3. Replace spaces with hyphens
    /// 4. Collapse consecutive hyphens into one
    /// 5. Trim leading and trailing hyphens
    /// 6. If empty, use "heading"
    /// 7. Append `-N` for duplicates (N starts at 1)
    pub fn slugify(&mut self, heading: &str) -> String {
        let base = Self::make_base_slug(heading);
        self.deduplicate(base)
    }

    /// Creates the base slug without deduplication.
    fn make_base_slug(heading: &str) -> String {
        let slug: String = heading
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '_' {
                    c.to_ascii_lowercase()
                } else if c == ' ' || c == '-' {
                    '-'
                } else {
                    // Remove other characters (punctuation, non-ASCII)
                    '\0'
                }
            })
            .filter(|&c| c != '\0')
            .collect();

        // Collapse consecutive hyphens
        let mut result = String::with_capacity(slug.len());
        let mut prev_hyphen = false;
        for c in slug.chars() {
            if c == '-' {
                if !prev_hyphen {
                    result.push('-');
                }
                prev_hyphen = true;
            } else {
                result.push(c);
                prev_hyphen = false;
            }
        }

        // Trim leading/trailing hyphens
        let result = result.trim_matches('-');

        if result.is_empty() {
            "heading".to_string()
        } else {
            result.to_string()
        }
    }

    /// Ensures the slug is unique, appending `-N` suffix if needed.
    fn deduplicate(&mut self, base: String) -> String {
        let count = self.counts.entry(base.clone()).or_insert(0);
        *count += 1;

        if *count == 1 {
            base
        } else {
            format!("{}-{}", base, *count - 1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_heading() {
        let mut slugifier = Slugifier::new();
        assert_eq!(slugifier.slugify("Overview"), "overview");
    }

    #[test]
    fn test_with_spaces() {
        let mut slugifier = Slugifier::new();
        assert_eq!(
            slugifier.slugify("Error Handling Patterns"),
            "error-handling-patterns"
        );
    }

    #[test]
    fn test_with_punctuation() {
        let mut slugifier = Slugifier::new();
        // Punctuation like <> and ! are removed, spaces become hyphens
        assert_eq!(slugifier.slugify("The Result<T> Type!"), "the-resultt-type");
    }

    #[test]
    fn test_duplicate_headings() {
        let mut slugifier = Slugifier::new();
        assert_eq!(slugifier.slugify("Overview"), "overview");
        assert_eq!(slugifier.slugify("Overview"), "overview-1");
        assert_eq!(slugifier.slugify("Overview"), "overview-2");
    }

    #[test]
    fn test_all_punctuation() {
        let mut slugifier = Slugifier::new();
        // When all chars are punctuation, fallback to "heading"
        assert_eq!(slugifier.slugify("!@#$%^&*()"), "heading");
    }

    #[test]
    fn test_leading_trailing_spaces() {
        let mut slugifier = Slugifier::new();
        assert_eq!(slugifier.slugify("  Hello World  "), "hello-world");
    }

    #[test]
    fn test_consecutive_hyphens() {
        let mut slugifier = Slugifier::new();
        assert_eq!(slugifier.slugify("Hello  --  World"), "hello-world");
    }

    #[test]
    fn test_underscores_preserved() {
        let mut slugifier = Slugifier::new();
        assert_eq!(slugifier.slugify("my_function_name"), "my_function_name");
    }

    #[test]
    fn test_mixed_case() {
        let mut slugifier = Slugifier::new();
        assert_eq!(slugifier.slugify("CamelCase Heading"), "camelcase-heading");
    }

    #[test]
    fn test_numbers() {
        let mut slugifier = Slugifier::new();
        assert_eq!(
            slugifier.slugify("Chapter 1: Introduction"),
            "chapter-1-introduction"
        );
    }

    #[test]
    fn test_unicode_removed() {
        let mut slugifier = Slugifier::new();
        // Non-ASCII chars are removed (GitHub behavior)
        assert_eq!(slugifier.slugify("Héllo Wörld"), "hllo-wrld");
    }

    #[test]
    fn test_empty_heading() {
        let mut slugifier = Slugifier::new();
        assert_eq!(slugifier.slugify(""), "heading");
    }

    #[test]
    fn test_only_hyphens() {
        let mut slugifier = Slugifier::new();
        assert_eq!(slugifier.slugify("---"), "heading");
    }

    #[test]
    fn test_mixed_duplicates() {
        let mut slugifier = Slugifier::new();
        assert_eq!(slugifier.slugify("Intro"), "intro");
        assert_eq!(slugifier.slugify("Setup"), "setup");
        assert_eq!(slugifier.slugify("Intro"), "intro-1");
        assert_eq!(slugifier.slugify("Setup"), "setup-1");
        assert_eq!(slugifier.slugify("Intro"), "intro-2");
    }
}
