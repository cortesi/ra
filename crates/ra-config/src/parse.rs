//! Configuration file parsing.
//!
//! Parses individual `.ra.toml` files into intermediate `RawConfig` structures
//! that preserve the optional nature of all fields before merging.

use std::{collections::HashMap, fs, path::Path};

use serde::Deserialize;
use toml::de::Error as TomlError;

use crate::ConfigError;

/// Raw configuration as parsed directly from a TOML file.
///
/// All fields are optional to support partial configs that will be merged.
/// This mirrors the TOML schema exactly.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct RawConfig {
    /// General settings section.
    pub settings: Option<RawSettings>,
    /// Search settings section.
    pub search: Option<RawSearchSettings>,
    /// Context settings section.
    pub context: Option<RawContextSettings>,
    /// Tree definitions: name -> tree config.
    pub tree: Option<HashMap<String, RawTree>>,
}

/// Raw tree definition from TOML.
#[derive(Debug, Clone, Deserialize)]
pub struct RawTree {
    /// Path to the tree directory.
    pub path: String,
    /// Include patterns (optional, defaults to ["**/*.md", "**/*.txt"]).
    pub include: Option<Vec<String>>,
    /// Exclude patterns (optional, defaults to none).
    pub exclude: Option<Vec<String>>,
}

/// Raw general settings.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct RawSettings {
    /// Maximum results per query.
    pub default_limit: Option<usize>,
    /// Relevance multiplier for local trees.
    pub local_boost: Option<f32>,
    /// Whether to split documents at h1 boundaries.
    pub chunk_at_headings: Option<bool>,
    /// Warning threshold for chunk size in characters.
    pub max_chunk_size: Option<usize>,
}

/// Raw search settings.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct RawSearchSettings {
    /// Enable fuzzy matching.
    pub fuzzy: Option<bool>,
    /// Levenshtein distance for fuzzy matching.
    pub fuzzy_distance: Option<u8>,
    /// Stemming language.
    pub stemmer: Option<String>,
}

/// Raw context settings.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct RawContextSettings {
    /// Default number of chunks to return.
    pub limit: Option<usize>,
    /// Minimum term frequency for content analysis.
    pub min_term_frequency: Option<usize>,
    /// Minimum word length for content analysis.
    pub min_word_length: Option<usize>,
    /// Maximum word length for content analysis.
    pub max_word_length: Option<usize>,
    /// Maximum bytes to sample from large files.
    pub sample_size: Option<usize>,
    /// Glob pattern to search term mappings.
    pub patterns: Option<HashMap<String, Vec<String>>>,
}

/// Parses a configuration file from disk.
///
/// Returns a `RawConfig` with all fields as optionals, ready for merging.
pub fn parse_config_file(path: &Path) -> Result<RawConfig, ConfigError> {
    let contents = fs::read_to_string(path).map_err(|source| ConfigError::ReadFile {
        path: path.to_path_buf(),
        source,
    })?;

    parse_config_str(&contents, path)
}

/// Parses configuration from a TOML string.
///
/// The `path` parameter is used for error reporting.
pub fn parse_config_str(contents: &str, path: &Path) -> Result<RawConfig, ConfigError> {
    toml::from_str(contents).map_err(|source| ConfigError::ParseToml {
        path: path.to_path_buf(),
        source,
    })
}

/// Parses configuration from a TOML string without path context.
///
/// Useful for validating template content.
pub fn parse_config(contents: &str) -> Result<RawConfig, TomlError> {
    toml::from_str(contents)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_empty_config() {
        let config = parse_config_str("", Path::new("test.toml")).unwrap();
        assert!(config.settings.is_none());
        assert!(config.search.is_none());
        assert!(config.context.is_none());
        assert!(config.tree.is_none());
    }

    #[test]
    fn test_parse_minimal_tree() {
        let toml = r#"
[tree.docs]
path = "./docs"
"#;
        let config = parse_config_str(toml, Path::new("test.toml")).unwrap();
        assert!(config.settings.is_none());
        let trees = config.tree.unwrap();
        let docs = trees.get("docs").unwrap();
        assert_eq!(docs.path, "./docs");
        assert!(docs.include.is_none());
        assert!(docs.exclude.is_none());
    }

    #[test]
    fn test_parse_tree_with_patterns() {
        let toml = r#"
[tree.docs]
path = "./docs"
include = ["**/*.md", "**/*.txt"]
exclude = ["**/drafts/**"]
"#;
        let config = parse_config_str(toml, Path::new("test.toml")).unwrap();
        let trees = config.tree.unwrap();
        let docs = trees.get("docs").unwrap();
        assert_eq!(docs.path, "./docs");
        assert_eq!(
            docs.include,
            Some(vec!["**/*.md".to_string(), "**/*.txt".to_string()])
        );
        assert_eq!(docs.exclude, Some(vec!["**/drafts/**".to_string()]));
    }

    #[test]
    fn test_parse_full_settings() {
        let toml = r#"
[settings]
default_limit = 10
local_boost = 2.0
chunk_at_headings = false
max_chunk_size = 100000
"#;
        let config = parse_config_str(toml, Path::new("test.toml")).unwrap();
        let settings = config.settings.unwrap();
        assert_eq!(settings.default_limit, Some(10));
        assert_eq!(settings.local_boost, Some(2.0));
        assert_eq!(settings.chunk_at_headings, Some(false));
        assert_eq!(settings.max_chunk_size, Some(100_000));
    }

    #[test]
    fn test_parse_partial_settings() {
        let toml = r#"
[settings]
default_limit = 3
"#;
        let config = parse_config_str(toml, Path::new("test.toml")).unwrap();
        let settings = config.settings.unwrap();
        assert_eq!(settings.default_limit, Some(3));
        assert!(settings.local_boost.is_none());
        assert!(settings.chunk_at_headings.is_none());
    }

    #[test]
    fn test_parse_search_settings() {
        let toml = r#"
[search]
fuzzy = false
fuzzy_distance = 2
stemmer = "german"
"#;
        let config = parse_config_str(toml, Path::new("test.toml")).unwrap();
        let search = config.search.unwrap();
        assert_eq!(search.fuzzy, Some(false));
        assert_eq!(search.fuzzy_distance, Some(2));
        assert_eq!(search.stemmer, Some("german".to_string()));
    }

    #[test]
    fn test_parse_context_settings() {
        let toml = r#"
[context]
limit = 20
min_term_frequency = 3
min_word_length = 5
max_word_length = 25
sample_size = 100000
"#;
        let config = parse_config_str(toml, Path::new("test.toml")).unwrap();
        let context = config.context.unwrap();
        assert_eq!(context.limit, Some(20));
        assert_eq!(context.min_term_frequency, Some(3));
        assert_eq!(context.min_word_length, Some(5));
        assert_eq!(context.max_word_length, Some(25));
        assert_eq!(context.sample_size, Some(100_000));
    }

    #[test]
    fn test_parse_context_patterns() {
        let toml = r#"
[context.patterns]
"*.rs" = ["rust", "systems"]
"*.py" = ["python"]
"src/api/**" = ["http", "handlers"]
"#;
        let config = parse_config_str(toml, Path::new("test.toml")).unwrap();
        let context = config.context.unwrap();
        let patterns = context.patterns.unwrap();
        assert_eq!(
            patterns.get("*.rs"),
            Some(&vec!["rust".to_string(), "systems".to_string()])
        );
        assert_eq!(patterns.get("*.py"), Some(&vec!["python".to_string()]));
        assert_eq!(
            patterns.get("src/api/**"),
            Some(&vec!["http".to_string(), "handlers".to_string()])
        );
    }

    #[test]
    fn test_parse_full_config() {
        let toml = r#"
[settings]
default_limit = 5
local_boost = 1.5

[search]
fuzzy = true
stemmer = "english"

[context]
limit = 10

[context.patterns]
"*.rs" = ["rust"]

[tree.global]
path = "~/docs"

[tree.local]
path = "./docs"
include = ["**/*"]
"#;
        let config = parse_config_str(toml, Path::new("test.toml")).unwrap();

        let settings = config.settings.unwrap();
        assert_eq!(settings.default_limit, Some(5));

        let search = config.search.unwrap();
        assert_eq!(search.fuzzy, Some(true));

        let context = config.context.unwrap();
        assert_eq!(context.limit, Some(10));
        assert!(context.patterns.is_some());

        let trees = config.tree.unwrap();
        assert_eq!(trees.len(), 2);
        assert_eq!(trees.get("global").unwrap().path, "~/docs");
        assert_eq!(trees.get("local").unwrap().path, "./docs");
    }

    #[test]
    fn test_parse_invalid_toml() {
        let toml = "this is not valid toml [[[";
        let result = parse_config_str(toml, Path::new("test.toml"));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ConfigError::ParseToml { .. }));
    }

    #[test]
    fn test_parse_unknown_fields_ignored() {
        let toml = r#"
[settings]
default_limit = 5
unknown_field = "ignored"

[unknown_section]
foo = "bar"
"#;
        // Unknown fields should be silently ignored (serde default behavior)
        let result = parse_config_str(toml, Path::new("test.toml"));
        assert!(result.is_ok());
        let config = result.unwrap();
        let settings = config.settings.unwrap();
        assert_eq!(settings.default_limit, Some(5));
    }

    #[test]
    fn test_parse_wrong_type_error() {
        let toml = r#"
[settings]
default_limit = "not a number"
"#;
        let result = parse_config_str(toml, Path::new("test.toml"));
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_config_file_not_found() {
        let result = parse_config_file(Path::new("/nonexistent/path/.ra.toml"));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ConfigError::ReadFile { .. }));
    }

    #[test]
    fn test_parse_multiple_trees() {
        let toml = r#"
[tree.global]
path = "~/docs"

[tree.local]
path = "./docs"

[tree.project]
path = "../shared/docs"
include = ["**/*.md"]

[tree.reference]
path = "/absolute/path/docs"
exclude = ["**/private/**"]
"#;
        let config = parse_config_str(toml, Path::new("test.toml")).unwrap();
        let trees = config.tree.unwrap();
        assert_eq!(trees.len(), 4);
        assert_eq!(trees.get("global").unwrap().path, "~/docs");
        assert_eq!(trees.get("local").unwrap().path, "./docs");
        assert_eq!(trees.get("project").unwrap().path, "../shared/docs");
        assert_eq!(trees.get("reference").unwrap().path, "/absolute/path/docs");
    }
}
