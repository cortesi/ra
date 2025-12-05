//! Weighted term extraction types.
//!
//! This module defines the core types for extracting terms with weights from documents.

/// A term extracted from a document with associated metadata.
///
/// Terms are the atomic units for building context queries. Each term carries
/// information about where it came from and how often it appeared, which is
/// used for ranking.
#[derive(Debug, Clone, PartialEq)]
pub struct WeightedTerm {
    /// The extracted term (lowercase, filtered).
    pub term: String,
    /// Semantic weight reflecting importance (higher = more relevant).
    pub weight: f32,
    /// Human-readable label for the source (e.g., "path:filename", "md:h1", "body").
    pub source: String,
    /// How many times this term appeared in the source.
    pub frequency: u32,
}

impl WeightedTerm {
    /// Creates a new weighted term with the given source label and weight.
    pub fn new(term: String, source: impl Into<String>, weight: f32) -> Self {
        Self {
            term,
            weight,
            source: source.into(),
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
    fn weighted_term_creation() {
        let term = WeightedTerm::new("test".to_string(), "md:h1", 3.0);
        assert_eq!(term.term, "test");
        assert_eq!(term.weight, 3.0);
        assert_eq!(term.source, "md:h1");
        assert_eq!(term.frequency, 1);
    }

    #[test]
    fn weighted_term_increment() {
        let mut term = WeightedTerm::new("test".to_string(), "body", 1.0);
        assert_eq!(term.frequency, 1);
        term.increment();
        assert_eq!(term.frequency, 2);
        term.increment();
        assert_eq!(term.frequency, 3);
    }

    #[test]
    fn raw_score_calculation() {
        let mut term = WeightedTerm::new("test".to_string(), "md:h1", 3.0);
        assert_eq!(term.raw_score(), 3.0); // 3.0 * 1

        term.increment();
        assert_eq!(term.raw_score(), 6.0); // 3.0 * 2

        term.increment();
        assert_eq!(term.raw_score(), 9.0); // 3.0 * 3
    }
}
