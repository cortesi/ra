//! Configuration file parsing.
//!
//! Parses individual `.ra.toml` files into intermediate `RawConfig` structures
//! that preserve the optional nature of all fields before merging.

use std::{collections::HashMap, fs, path::Path};

use serde::Deserialize;
use serde_with::{OneOrMany, serde_as};
#[cfg(test)]
use toml::de::Error as TomlError;

use crate::ConfigError;

/// Raw configuration as parsed directly from a TOML file.
///
/// All fields are optional to support partial configs that will be merged.
/// This mirrors the TOML schema exactly.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct RawConfig {
    /// When true, stop discovery here - ignore parent and global configs.
    pub root: Option<bool>,
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
    /// Stemming language.
    pub stemmer: Option<String>,
    /// Fuzzy matching Levenshtein distance (0 = disabled).
    pub fuzzy_distance: Option<u8>,
    /// Maximum results to return after aggregation.
    pub limit: Option<usize>,
    /// Size of the aggregation pool - how many candidates are available for
    /// hierarchical aggregation before elbow cutoff.
    #[serde(alias = "max_candidates")]
    pub aggregation_pool_size: Option<usize>,
    /// Score ratio threshold for elbow cutoff (0.0-1.0).
    pub cutoff_ratio: Option<f32>,
    /// Sibling ratio threshold for hierarchical aggregation.
    pub aggregation_threshold: Option<f32>,
}

/// Raw context settings.
///
/// Context-specific settings for term extraction. Search parameters are inherited
/// from `[search]` and can be overridden per-rule in `[[context.rules]]`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct RawContextSettings {
    /// Maximum terms to include in context queries.
    pub terms: Option<usize>,
    /// Minimum term frequency for content analysis.
    pub min_term_frequency: Option<usize>,
    /// Minimum word length for content analysis.
    pub min_word_length: Option<usize>,
    /// Maximum word length for content analysis.
    pub max_word_length: Option<usize>,
    /// Maximum bytes to sample from large files.
    pub sample_size: Option<usize>,
    /// Context rules for customizing search behavior per file pattern.
    pub rules: Option<Vec<RawContextRule>>,
}

/// Raw context rule from TOML.
///
/// Each rule specifies glob patterns to match against file paths, and the
/// search behavior customizations to apply when a file matches.
#[serde_as]
#[derive(Debug, Clone, Deserialize)]
pub struct RawContextRule {
    /// Glob pattern(s) to match against file paths.
    /// Accepts either a single string or an array of strings.
    #[serde(rename = "match")]
    #[serde_as(as = "OneOrMany<_>")]
    pub patterns: Vec<String>,
    /// Limit search to these trees (default: all trees).
    pub trees: Option<Vec<String>>,
    /// Additional search terms to inject into the query.
    pub terms: Option<Vec<String>>,
    /// Files to always include in results (tree-prefixed paths like "docs:api/overview.md").
    pub include: Option<Vec<String>>,
    /// Search parameter overrides for this rule.
    pub search: Option<RawSearchSettings>,
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
/// Useful for validating template content (tests only).
#[cfg(test)]
pub fn parse_config(contents: &str) -> Result<RawConfig, TomlError> {
    toml::from_str(contents)
}

/// Checks if a config file has `root = true` set.
///
/// This is used during discovery to stop traversal at root configs.
/// Returns false if the file cannot be read or parsed.
pub fn is_root_config(path: &Path) -> bool {
    let Ok(contents) = fs::read_to_string(path) else {
        return false;
    };
    let Ok(config) = toml::from_str::<RawConfig>(&contents) else {
        return false;
    };
    config.root == Some(true)
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
stemmer = "german"
"#;
        let config = parse_config_str(toml, Path::new("test.toml")).unwrap();
        let search = config.search.unwrap();
        assert_eq!(search.stemmer, Some("german".to_string()));
    }

    #[test]
    fn test_parse_context_settings() {
        let toml = r#"
[context]
terms = 100
min_term_frequency = 3
min_word_length = 5
max_word_length = 25
sample_size = 100000
"#;
        let config = parse_config_str(toml, Path::new("test.toml")).unwrap();
        let context = config.context.unwrap();
        assert_eq!(context.terms, Some(100));
        assert_eq!(context.min_term_frequency, Some(3));
        assert_eq!(context.min_word_length, Some(5));
        assert_eq!(context.max_word_length, Some(25));
        assert_eq!(context.sample_size, Some(100_000));
    }

    #[test]
    fn test_parse_context_rules() {
        let toml = r#"
[[context.rules]]
match = "*.rs"
terms = ["rust", "systems"]

[[context.rules]]
match = "*.py"
terms = ["python"]

[[context.rules]]
match = "src/api/**"
terms = ["http", "handlers"]
trees = ["docs"]
"#;
        let config = parse_config_str(toml, Path::new("test.toml")).unwrap();
        let context = config.context.unwrap();
        let rules = context.rules.unwrap();
        assert_eq!(rules.len(), 3);
        assert_eq!(rules[0].patterns, vec!["*.rs"]);
        assert_eq!(
            rules[0].terms,
            Some(vec!["rust".to_string(), "systems".to_string()])
        );
        assert_eq!(rules[1].patterns, vec!["*.py"]);
        assert_eq!(rules[1].terms, Some(vec!["python".to_string()]));
        assert_eq!(rules[2].patterns, vec!["src/api/**"]);
        assert_eq!(
            rules[2].terms,
            Some(vec!["http".to_string(), "handlers".to_string()])
        );
        assert_eq!(rules[2].trees, Some(vec!["docs".to_string()]));
    }

    #[test]
    fn test_parse_context_rules_with_multiple_match_patterns() {
        let toml = r#"
[[context.rules]]
match = ["*.tsx", "*.jsx"]
terms = ["react", "components"]
"#;
        let config = parse_config_str(toml, Path::new("test.toml")).unwrap();
        let context = config.context.unwrap();
        let rules = context.rules.unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].patterns, vec!["*.tsx", "*.jsx"]);
        assert_eq!(
            rules[0].terms,
            Some(vec!["react".to_string(), "components".to_string()])
        );
    }

    #[test]
    fn test_parse_context_rules_with_include() {
        let toml = r#"
[[context.rules]]
match = "src/api/**"
terms = ["http"]
include = ["docs:api/overview.md", "docs:api/auth.md"]
"#;
        let config = parse_config_str(toml, Path::new("test.toml")).unwrap();
        let context = config.context.unwrap();
        let rules = context.rules.unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(
            rules[0].include,
            Some(vec![
                "docs:api/overview.md".to_string(),
                "docs:api/auth.md".to_string()
            ])
        );
    }

    #[test]
    fn test_parse_full_config() {
        let toml = r#"
[settings]
default_limit = 5
local_boost = 1.5

[search]
stemmer = "english"
limit = 10

[context]
min_term_frequency = 3

[[context.rules]]
match = "*.rs"
terms = ["rust"]

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
        assert_eq!(search.stemmer, Some("english".to_string()));
        assert_eq!(search.limit, Some(10));

        let context = config.context.unwrap();
        assert_eq!(context.min_term_frequency, Some(3));
        assert!(context.rules.is_some());
        assert_eq!(context.rules.as_ref().unwrap().len(), 1);

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

    #[test]
    fn test_parse_root_true() {
        let toml = "root = true\n";
        let config = parse_config_str(toml, Path::new("test.toml")).unwrap();
        assert_eq!(config.root, Some(true));
    }

    #[test]
    fn test_parse_root_false() {
        let toml = "root = false\n";
        let config = parse_config_str(toml, Path::new("test.toml")).unwrap();
        assert_eq!(config.root, Some(false));
    }

    #[test]
    fn test_parse_root_not_specified() {
        let toml = "[settings]\ndefault_limit = 5\n";
        let config = parse_config_str(toml, Path::new("test.toml")).unwrap();
        assert_eq!(config.root, None);
    }

    #[test]
    fn test_is_root_config_true() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join(".ra.toml");
        fs::write(&config_path, "root = true\n").unwrap();
        assert!(is_root_config(&config_path));
    }

    #[test]
    fn test_is_root_config_false() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join(".ra.toml");
        fs::write(&config_path, "root = false\n").unwrap();
        assert!(!is_root_config(&config_path));
    }

    #[test]
    fn test_is_root_config_not_specified() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join(".ra.toml");
        fs::write(&config_path, "[settings]\ndefault_limit = 5\n").unwrap();
        assert!(!is_root_config(&config_path));
    }

    #[test]
    fn test_is_root_config_nonexistent() {
        let path = Path::new("/nonexistent/.ra.toml");
        assert!(!is_root_config(path));
    }
}
