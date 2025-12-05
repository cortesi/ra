//! Weighted term extraction types.
//!
//! This module defines the core types for extracting terms with weights and source
//! attribution from documents.

use std::fmt;

/// The source location from which a term was extracted.
///
/// Different sources have different semantic weights reflecting their importance
/// for search relevance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TermSource {
    /// Term from the filename component of the path.
    PathFilename,
    /// Term from a directory component of the path.
    PathDirectory,
    /// Term from a markdown h1 heading.
    MarkdownH1,
    /// Term from a markdown h2 or h3 heading.
    MarkdownH2H3,
    /// Term from a markdown h4, h5, or h6 heading.
    MarkdownH4H6,
    /// Term from body text content.
    Body,
}

impl TermSource {
    /// Returns the default weight for this source type.
    ///
    /// Higher weights indicate terms that are more likely to be relevant
    /// for search. Weights are based on the assumption that:
    /// - Path components are highly intentional naming choices
    /// - Headings summarize content importance
    /// - Body text has the lowest signal-to-noise ratio
    pub fn default_weight(self) -> f32 {
        match self {
            Self::PathFilename => 4.0,
            Self::PathDirectory => 3.0,
            Self::MarkdownH1 => 3.0,
            Self::MarkdownH2H3 => 2.0,
            Self::MarkdownH4H6 => 1.5,
            Self::Body => 1.0,
        }
    }
}

impl fmt::Display for TermSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PathFilename => write!(f, "filename"),
            Self::PathDirectory => write!(f, "directory"),
            Self::MarkdownH1 => write!(f, "h1"),
            Self::MarkdownH2H3 => write!(f, "h2-h3"),
            Self::MarkdownH4H6 => write!(f, "h4-h6"),
            Self::Body => write!(f, "body"),
        }
    }
}

/// A term extracted from a document with associated metadata.
///
/// Terms are the atomic units for building context queries. Each term carries
/// information about where it came from and how often it appeared, which is
/// used for ranking.
#[derive(Debug, Clone)]
pub struct WeightedTerm {
    /// The extracted term (lowercase, filtered).
    pub term: String,
    /// Base weight from the source location.
    pub weight: f32,
    /// Where this term was extracted from.
    pub source: TermSource,
    /// How many times this term appeared in the source.
    pub frequency: u32,
}

impl WeightedTerm {
    /// Creates a new weighted term with frequency 1.
    pub fn new(term: String, source: TermSource) -> Self {
        Self {
            term,
            weight: source.default_weight(),
            source,
            frequency: 1,
        }
    }

    /// Creates a new weighted term with a custom weight.
    pub fn with_weight(term: String, source: TermSource, weight: f32) -> Self {
        Self {
            term,
            weight,
            source,
            frequency: 1,
        }
    }

    /// Increments the frequency count.
    pub fn increment(&mut self) {
        self.frequency += 1;
    }

    /// Computes the raw score for this term (weight * frequency).
    ///
    /// This score does not include IDF adjustment, which is applied later
    /// when ranking against the index.
    pub fn raw_score(&self) -> f32 {
        self.weight * self.frequency as f32
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn source_weights() {
        assert_eq!(TermSource::PathFilename.default_weight(), 4.0);
        assert_eq!(TermSource::PathDirectory.default_weight(), 3.0);
        assert_eq!(TermSource::MarkdownH1.default_weight(), 3.0);
        assert_eq!(TermSource::MarkdownH2H3.default_weight(), 2.0);
        assert_eq!(TermSource::MarkdownH4H6.default_weight(), 1.5);
        assert_eq!(TermSource::Body.default_weight(), 1.0);
    }

    #[test]
    fn weighted_term_creation() {
        let term = WeightedTerm::new("test".to_string(), TermSource::MarkdownH1);
        assert_eq!(term.term, "test");
        assert_eq!(term.weight, 3.0);
        assert_eq!(term.source, TermSource::MarkdownH1);
        assert_eq!(term.frequency, 1);
    }

    #[test]
    fn weighted_term_with_custom_weight() {
        let term = WeightedTerm::with_weight("test".to_string(), TermSource::Body, 2.5);
        assert_eq!(term.weight, 2.5);
    }

    #[test]
    fn weighted_term_increment() {
        let mut term = WeightedTerm::new("test".to_string(), TermSource::Body);
        assert_eq!(term.frequency, 1);
        term.increment();
        assert_eq!(term.frequency, 2);
        term.increment();
        assert_eq!(term.frequency, 3);
    }

    #[test]
    fn raw_score_calculation() {
        let mut term = WeightedTerm::new("test".to_string(), TermSource::MarkdownH1);
        assert_eq!(term.raw_score(), 3.0); // 3.0 * 1

        term.increment();
        assert_eq!(term.raw_score(), 6.0); // 3.0 * 2

        term.increment();
        assert_eq!(term.raw_score(), 9.0); // 3.0 * 3
    }

    #[test]
    fn source_display() {
        assert_eq!(format!("{}", TermSource::PathFilename), "filename");
        assert_eq!(format!("{}", TermSource::PathDirectory), "directory");
        assert_eq!(format!("{}", TermSource::MarkdownH1), "h1");
        assert_eq!(format!("{}", TermSource::MarkdownH2H3), "h2-h3");
        assert_eq!(format!("{}", TermSource::MarkdownH4H6), "h4-h6");
        assert_eq!(format!("{}", TermSource::Body), "body");
    }
}
