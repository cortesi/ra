//! Stopword filtering for context term extraction.
//!
//! This module provides stopword detection combining:
//! - Standard English stopwords from the `stop-words` crate
//! - Rust keywords and reserved words from the official Rust Reference
//! - Rust primitive types
//! - Common Rust standard library types
//!
//! Stopwords are low-value terms that should be filtered out during term extraction
//! to focus on semantically meaningful content.

use std::collections::HashSet;

use stop_words::LANGUAGE;

/// A stopword filter combining English and Rust-specific stopwords.
///
/// Uses a `HashSet` for O(1) lookup performance. All words are stored in
/// lowercase for case-insensitive matching.
#[derive(Clone)]
pub struct Stopwords {
    words: HashSet<String>,
}

impl Default for Stopwords {
    fn default() -> Self {
        Self::new()
    }
}

impl Stopwords {
    /// Creates a new stopword filter with default English and Rust stopwords.
    pub fn new() -> Self {
        let mut words: HashSet<String> = HashSet::new();

        // Helper to add words in lowercase for case-insensitive matching
        let mut add_words = |slice: &[&str]| {
            for word in slice {
                words.insert(word.to_ascii_lowercase());
            }
        };

        // Add English stopwords from the stop-words crate (Stopwords ISO)
        add_words(stop_words::get(LANGUAGE::English));

        // Add Rust-specific stopwords
        add_words(RUST_KEYWORDS);
        add_words(RUST_RESERVED_KEYWORDS);
        add_words(RUST_WEAK_KEYWORDS);
        add_words(RUST_PRIMITIVE_TYPES);
        add_words(RUST_COMMON_PRELUDE);

        Self { words }
    }

    /// Checks if a term is a stopword.
    ///
    /// The check is case-insensitive for ASCII characters.
    pub fn contains(&self, term: &str) -> bool {
        let lower = term.to_ascii_lowercase();
        self.words.contains(&lower)
    }

    /// Returns the total number of stopwords.
    pub fn len(&self) -> usize {
        self.words.len()
    }

    /// Returns true if no stopwords are configured.
    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }
}

/// Rust strict keywords (all editions).
///
/// Source: <https://doc.rust-lang.org/reference/keywords.html>
static RUST_KEYWORDS: &[&str] = &[
    // Strict keywords (all editions)
    "as", "break", "const", "continue", "crate", "else", "enum", "extern", "false", "fn", "for",
    "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return",
    "self", "Self", "static", "struct", "super", "trait", "true", "type", "unsafe", "use", "where",
    "while", // Strict keywords (2018 edition+)
    "async", "await", "dyn",
];

/// Rust reserved keywords.
///
/// Source: <https://doc.rust-lang.org/reference/keywords.html>
static RUST_RESERVED_KEYWORDS: &[&str] = &[
    // Reserved keywords
    "abstract", "become", "box", "do", "final", "macro", "override", "priv", "typeof", "unsized",
    "virtual", "yield", // Reserved keywords (2018 edition+)
    "try",   // Reserved keywords (2024 edition+)
    "gen",
];

/// Rust weak keywords.
///
/// Source: <https://doc.rust-lang.org/reference/keywords.html>
static RUST_WEAK_KEYWORDS: &[&str] = &["macro_rules", "raw", "safe", "union"];

/// Rust primitive types.
///
/// Source: <https://doc.rust-lang.org/std/#primitives>
static RUST_PRIMITIVE_TYPES: &[&str] = &[
    // Boolean
    "bool",
    // Textual
    "char",
    "str",
    // Signed integers
    "i8",
    "i16",
    "i32",
    "i64",
    "i128",
    "isize",
    // Unsigned integers
    "u8",
    "u16",
    "u32",
    "u64",
    "u128",
    "usize",
    // Floating point
    "f32",
    "f64",
    // Other primitives
    "array",
    "slice",
    "tuple",
    "unit",
    "pointer",
    "reference",
    "never",
];

/// Common Rust standard library types and values from the prelude.
///
/// These are ubiquitous in Rust code and documentation, providing little
/// discriminative value for search.
///
/// Source: <https://doc.rust-lang.org/std/prelude/index.html>
static RUST_COMMON_PRELUDE: &[&str] = &[
    // Option enum and variants
    "Option",
    "Some",
    "None",
    // Result enum and variants
    "Result",
    "Ok",
    "Err",
    // Common traits (extremely common in docs)
    "Clone",
    "Copy",
    "Default",
    "Drop",
    "Eq",
    "PartialEq",
    "Ord",
    "PartialOrd",
    "Hash",
    "Debug",
    "Display",
    "From",
    "Into",
    "TryFrom",
    "TryInto",
    "AsRef",
    "AsMut",
    "Deref",
    "DerefMut",
    "Iterator",
    "IntoIterator",
    "Extend",
    "Send",
    "Sync",
    "Sized",
    "Unpin",
    "Fn",
    "FnMut",
    "FnOnce",
    "ToOwned",
    "ToString",
    // Common types
    "Vec",
    "String",
    "Box",
    "Rc",
    "Arc",
    "Cell",
    "RefCell",
    "Mutex",
    "RwLock",
    // Common macros (often appear in docs)
    "println",
    "print",
    "eprintln",
    "eprint",
    "format",
    "vec",
    "panic",
    "assert",
    "assert_eq",
    "assert_ne",
    "debug_assert",
    "debug_assert_eq",
    "debug_assert_ne",
    "todo",
    "unimplemented",
    "unreachable",
];

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn contains_english_stopwords() {
        let sw = Stopwords::new();
        // Common English stopwords from stop-words crate
        assert!(sw.contains("the"));
        assert!(sw.contains("and"));
        assert!(sw.contains("is"));
        assert!(sw.contains("in"));
        assert!(sw.contains("to"));
        assert!(sw.contains("of"));
    }

    #[test]
    fn contains_rust_keywords() {
        let sw = Stopwords::new();
        // Strict keywords
        assert!(sw.contains("fn"));
        assert!(sw.contains("let"));
        assert!(sw.contains("mut"));
        assert!(sw.contains("impl"));
        assert!(sw.contains("struct"));
        assert!(sw.contains("enum"));
        assert!(sw.contains("trait"));
        assert!(sw.contains("async"));
        assert!(sw.contains("await"));
    }

    #[test]
    fn contains_rust_reserved_keywords() {
        let sw = Stopwords::new();
        assert!(sw.contains("abstract"));
        assert!(sw.contains("final"));
        assert!(sw.contains("override"));
        assert!(sw.contains("try"));
        assert!(sw.contains("gen"));
    }

    #[test]
    fn contains_rust_primitive_types() {
        let sw = Stopwords::new();
        assert!(sw.contains("bool"));
        assert!(sw.contains("i32"));
        assert!(sw.contains("u64"));
        assert!(sw.contains("f64"));
        assert!(sw.contains("str"));
        assert!(sw.contains("usize"));
    }

    #[test]
    fn contains_rust_prelude() {
        let sw = Stopwords::new();
        assert!(sw.contains("Option"));
        assert!(sw.contains("Result"));
        assert!(sw.contains("Some"));
        assert!(sw.contains("None"));
        assert!(sw.contains("Ok"));
        assert!(sw.contains("Err"));
        assert!(sw.contains("Vec"));
        assert!(sw.contains("String"));
        assert!(sw.contains("Clone"));
        assert!(sw.contains("Debug"));
    }

    #[test]
    fn case_insensitive_lowercase() {
        let sw = Stopwords::new();
        // English stopwords are lowercase - should match any case
        assert!(sw.contains("the"));
        assert!(sw.contains("The"));
        assert!(sw.contains("THE"));
        // Rust keywords are lowercase - should match any case
        assert!(sw.contains("fn"));
        assert!(sw.contains("Fn"));
        assert!(sw.contains("FN"));
    }

    #[test]
    fn case_sensitive_for_types() {
        let sw = Stopwords::new();
        // Rust types are PascalCase - only exact or lowercase match works
        // "Option" is in the list
        assert!(sw.contains("Option"));
        // "option" (lowercase of Option) matches because we check lowercase
        assert!(sw.contains("option"));
        // "OPTION" -> lowercase "option" matches
        assert!(sw.contains("OPTION"));
    }

    #[test]
    fn non_stopwords_not_matched() {
        let sw = Stopwords::new();
        // Domain-specific terms should not be stopwords
        assert!(!sw.contains("authentication"));
        assert!(!sw.contains("database"));
        assert!(!sw.contains("kubernetes"));
        assert!(!sw.contains("encryption"));
        assert!(!sw.contains("tantivy"));
        assert!(!sw.contains("tokio"));
    }

    #[test]
    fn has_reasonable_count() {
        let sw = Stopwords::new();
        // Should have a substantial number of stopwords
        // English (~175) + Rust keywords (~40) + reserved (~14) + weak (~4) +
        // primitives (~20) + prelude (~60) = ~300+
        assert!(sw.len() > 200);
        assert!(!sw.is_empty());
    }
}
