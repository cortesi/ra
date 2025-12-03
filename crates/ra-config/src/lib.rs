//! Configuration system for ra.
//!
//! ra uses TOML configuration files named `.ra.toml`. Configuration is resolved by walking up
//! the directory tree from the current working directory, collecting any `.ra.toml` files found,
//! then loading `~/.ra.toml` as the global config with lowest precedence.

#![warn(missing_docs)]

mod discovery;
mod error;
mod merge;
mod parse;
mod resolve;

use std::collections::HashMap;
use std::path::PathBuf;

use serde::Deserialize;

pub use discovery::{CONFIG_FILENAME, discover_config_files, global_config_path, is_global_config};
pub use error::ConfigError;
pub use merge::{ParsedConfig, merge_configs};
pub use parse::{
    RawConfig, RawContextSettings, RawIncludePattern, RawSearchSettings, RawSettings,
    parse_config_file, parse_config_str,
};
pub use resolve::resolve_tree_path;

/// Top-level merged configuration for ra.
///
/// This represents the fully resolved configuration after merging all discovered `.ra.toml`
/// files according to precedence rules.
#[derive(Debug, Clone, Default)]
pub struct Config {
    /// General settings.
    pub settings: Settings,
    /// Search-related settings.
    pub search: SearchSettings,
    /// Context command settings.
    pub context: ContextSettings,
    /// Resolved trees with their absolute paths.
    pub trees: Vec<Tree>,
    /// Include patterns determining which files to index from each tree.
    pub includes: Vec<IncludePattern>,
    /// Path to the most specific config file (determines index location).
    pub config_root: Option<PathBuf>,
}

/// General settings for ra.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Maximum results per query.
    pub default_limit: usize,
    /// Relevance multiplier for local (non-global) trees.
    pub local_boost: f32,
    /// Whether to split documents at h1 boundaries.
    pub chunk_at_headings: bool,
    /// Warning threshold for chunk size in characters.
    pub max_chunk_size: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            default_limit: 5,
            local_boost: 1.5,
            chunk_at_headings: true,
            max_chunk_size: 50_000,
        }
    }
}

/// Search-related settings.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct SearchSettings {
    /// Enable fuzzy matching for typo tolerance.
    pub fuzzy: bool,
    /// Levenshtein distance for fuzzy matching (0, 1, or 2).
    pub fuzzy_distance: u8,
    /// Stemming language.
    pub stemmer: String,
}

impl Default for SearchSettings {
    fn default() -> Self {
        Self {
            fuzzy: true,
            fuzzy_distance: 1,
            stemmer: String::from("english"),
        }
    }
}

/// Settings for the `ra context` command.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ContextSettings {
    /// Default number of chunks to return.
    pub limit: usize,
    /// Ignore terms appearing less than this many times in source.
    pub min_term_frequency: usize,
    /// Ignore words shorter than this.
    pub min_word_length: usize,
    /// Ignore words longer than this.
    pub max_word_length: usize,
    /// Maximum bytes to analyze from large files.
    pub sample_size: usize,
    /// Glob pattern to search term mappings.
    pub patterns: HashMap<String, Vec<String>>,
}

impl Default for ContextSettings {
    fn default() -> Self {
        Self {
            limit: 10,
            min_term_frequency: 2,
            min_word_length: 4,
            max_word_length: 30,
            sample_size: 50_000,
            patterns: default_context_patterns(),
        }
    }
}

/// Returns the default context patterns for common file extensions.
fn default_context_patterns() -> HashMap<String, Vec<String>> {
    let mut patterns = HashMap::new();
    patterns.insert("*.rs".into(), vec!["rust".into()]);
    patterns.insert("*.py".into(), vec!["python".into()]);
    patterns.insert("*.tsx".into(), vec!["typescript".into(), "react".into()]);
    patterns.insert("*.jsx".into(), vec!["javascript".into(), "react".into()]);
    patterns.insert("*.ts".into(), vec!["typescript".into()]);
    patterns.insert("*.js".into(), vec!["javascript".into()]);
    patterns.insert("*.go".into(), vec!["golang".into()]);
    patterns.insert("*.rb".into(), vec!["ruby".into()]);
    patterns.insert("*.ex".into(), vec!["elixir".into()]);
    patterns.insert("*.exs".into(), vec!["elixir".into()]);
    patterns.insert("*.clj".into(), vec!["clojure".into()]);
    patterns.insert("*.hs".into(), vec!["haskell".into()]);
    patterns.insert("*.ml".into(), vec!["ocaml".into()]);
    patterns.insert("*.swift".into(), vec!["swift".into()]);
    patterns.insert("*.kt".into(), vec!["kotlin".into()]);
    patterns.insert("*.java".into(), vec!["java".into()]);
    patterns.insert("*.c".into(), vec!["c".into()]);
    patterns.insert("*.cpp".into(), vec!["cpp".into()]);
    patterns.insert("*.h".into(), vec!["c".into(), "cpp".into()]);
    patterns.insert("*.hpp".into(), vec!["cpp".into()]);
    patterns
}

/// A named knowledge tree pointing to a directory of documents.
#[derive(Debug, Clone)]
pub struct Tree {
    /// Name of the tree (used in include patterns and chunk IDs).
    pub name: String,
    /// Resolved absolute path to the tree directory.
    pub path: PathBuf,
    /// Whether this tree was defined in the global `~/.ra.toml`.
    pub is_global: bool,
}

/// An include pattern that selects files from a tree for indexing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncludePattern {
    /// Name of the tree this pattern applies to.
    pub tree: String,
    /// Glob pattern to match files within the tree.
    pub pattern: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_settings_defaults() {
        let settings = Settings::default();
        assert_eq!(settings.default_limit, 5);
        assert!((settings.local_boost - 1.5).abs() < f32::EPSILON);
        assert!(settings.chunk_at_headings);
        assert_eq!(settings.max_chunk_size, 50_000);
    }

    #[test]
    fn test_search_settings_defaults() {
        let search = SearchSettings::default();
        assert!(search.fuzzy);
        assert_eq!(search.fuzzy_distance, 1);
        assert_eq!(search.stemmer, "english");
    }

    #[test]
    fn test_context_settings_defaults() {
        let context = ContextSettings::default();
        assert_eq!(context.limit, 10);
        assert_eq!(context.min_term_frequency, 2);
        assert_eq!(context.min_word_length, 4);
        assert_eq!(context.max_word_length, 30);
        assert_eq!(context.sample_size, 50_000);
        assert!(context.patterns.contains_key("*.rs"));
        assert_eq!(context.patterns.get("*.rs"), Some(&vec!["rust".into()]));
    }

    #[test]
    fn test_default_context_patterns_coverage() {
        let patterns = default_context_patterns();
        let expected_extensions = [
            "*.rs", "*.py", "*.tsx", "*.jsx", "*.ts", "*.js", "*.go", "*.rb", "*.ex", "*.exs",
            "*.clj", "*.hs", "*.ml", "*.swift", "*.kt", "*.java", "*.c", "*.cpp", "*.h", "*.hpp",
        ];
        for ext in expected_extensions {
            assert!(patterns.contains_key(ext), "Missing pattern for {ext}");
        }
    }

    #[test]
    fn test_config_default() {
        let config = Config::default();
        assert!(config.trees.is_empty());
        assert!(config.includes.is_empty());
        assert!(config.config_root.is_none());
    }

    #[test]
    fn test_tree_creation() {
        let tree = Tree {
            name: "docs".into(),
            path: PathBuf::from("/home/user/docs"),
            is_global: false,
        };
        assert_eq!(tree.name, "docs");
        assert!(!tree.is_global);
    }

    #[test]
    fn test_include_pattern_equality() {
        let p1 = IncludePattern {
            tree: "global".into(),
            pattern: "**/*.md".into(),
        };
        let p2 = IncludePattern {
            tree: "global".into(),
            pattern: "**/*.md".into(),
        };
        let p3 = IncludePattern {
            tree: "local".into(),
            pattern: "**/*.md".into(),
        };
        assert_eq!(p1, p2);
        assert_ne!(p1, p3);
    }
}
