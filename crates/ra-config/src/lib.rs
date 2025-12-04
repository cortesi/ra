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
mod patterns;
mod resolve;
mod templates;
mod validate;

use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
};

pub use discovery::{CONFIG_FILENAME, discover_config_files, global_config_path, is_global_config};
pub use error::ConfigError;
pub use merge::{ParsedConfig, merge_configs};
pub use parse::{
    RawConfig, RawContextSettings, RawSearchSettings, RawSettings, RawTree, parse_config,
    parse_config_file, parse_config_str,
};
pub use patterns::{CompiledContextPatterns, CompiledPatterns};
pub use resolve::resolve_tree_path;
use serde::{Deserialize, Serialize};
pub use templates::{global_template, local_template};
pub use validate::ConfigWarning;
use validate::validate_config;

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
    /// Resolved trees with their absolute paths and patterns.
    pub trees: Vec<Tree>,
    /// Directory containing the most specific config file (determines index location).
    pub config_root: Option<PathBuf>,
}

impl Config {
    /// Loads configuration by discovering and merging all relevant `.ra.toml` files.
    ///
    /// This is the main entry point for loading configuration. It:
    /// 1. Discovers all `.ra.toml` files from `cwd` up to the filesystem root
    /// 2. Appends `~/.ra.toml` if it exists
    /// 3. Parses each file
    /// 4. Merges them according to precedence rules (closest to `cwd` wins)
    ///
    /// Returns `Ok(Config::default())` if no configuration files are found.
    pub fn load(cwd: &Path) -> Result<Self, ConfigError> {
        let config_files = discover_config_files(cwd);
        Self::load_from_files(&config_files)
    }

    /// Loads configuration from a specific list of config file paths.
    ///
    /// Files should be provided in precedence order: highest precedence first.
    /// This is primarily useful for testing.
    ///
    /// Returns `Ok(Config::default())` if the list is empty.
    pub fn load_from_files(files: &[PathBuf]) -> Result<Self, ConfigError> {
        if files.is_empty() {
            return Ok(Self::default());
        }

        let parsed: Vec<ParsedConfig> = files
            .iter()
            .map(|path| {
                let config = parse_config_file(path)?;
                Ok(ParsedConfig {
                    path: path.clone(),
                    config,
                })
            })
            .collect::<Result<Vec<_>, ConfigError>>()?;

        merge_configs(&parsed)
    }

    /// Compiles the include/exclude patterns for this configuration.
    ///
    /// Returns a `CompiledPatterns` that can efficiently match file paths
    /// against the configured patterns for each tree.
    pub fn compile_patterns(&self) -> Result<CompiledPatterns, ConfigError> {
        CompiledPatterns::compile(&self.trees)
    }

    /// Validates the configuration and returns any warnings.
    ///
    /// This checks for:
    /// - Tree paths that don't exist or aren't directories
    /// - Include patterns that don't match any files
    /// - Trees that are defined but not referenced by any include pattern
    /// - Include patterns that reference undefined trees
    /// - Empty configuration (no trees defined)
    pub fn validate(&self) -> Vec<ConfigWarning> {
        validate_config(self)
    }

    /// Serializes the effective settings to TOML format.
    ///
    /// This outputs the merged configuration settings in the same format as a `.ra.toml` file,
    /// making it easy to see the effective configuration. Trees and include patterns are not
    /// included since they have resolved paths and additional metadata.
    pub fn settings_to_toml(&self) -> String {
        let serializable = SerializableSettings {
            settings: self.settings.clone(),
            search: self.search.clone(),
            context: SerializableContextSettings::from(&self.context),
        };
        toml::to_string_pretty(&serializable).expect("settings serialization should not fail")
    }
}

/// General settings for ra.
#[derive(Debug, Clone, Deserialize, Serialize)]
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
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct SearchSettings {
    /// Stemming language.
    pub stemmer: String,
}

impl Default for SearchSettings {
    fn default() -> Self {
        Self {
            stemmer: String::from("english"),
        }
    }
}

/// Settings for the `ra context` command.
#[derive(Debug, Clone, Deserialize, Serialize)]
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
            patterns: HashMap::new(),
        }
    }
}

/// Internal struct for TOML serialization of settings.
#[derive(Serialize)]
struct SerializableSettings {
    /// General settings.
    settings: Settings,
    /// Search-related settings.
    search: SearchSettings,
    /// Context command settings.
    context: SerializableContextSettings,
}

/// Context settings with sorted patterns for deterministic TOML output.
#[derive(Serialize)]
struct SerializableContextSettings {
    /// Default number of chunks to return.
    limit: usize,
    /// Ignore terms appearing less than this many times in source.
    min_term_frequency: usize,
    /// Ignore words shorter than this.
    min_word_length: usize,
    /// Ignore words longer than this.
    max_word_length: usize,
    /// Maximum bytes to analyze from large files.
    sample_size: usize,
    /// Glob pattern to search term mappings (sorted for deterministic output).
    patterns: BTreeMap<String, Vec<String>>,
}

impl From<&ContextSettings> for SerializableContextSettings {
    fn from(ctx: &ContextSettings) -> Self {
        Self {
            limit: ctx.limit,
            min_term_frequency: ctx.min_term_frequency,
            min_word_length: ctx.min_word_length,
            max_word_length: ctx.max_word_length,
            sample_size: ctx.sample_size,
            patterns: ctx
                .patterns
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        }
    }
}

/// A named knowledge tree pointing to a directory of documents.
#[derive(Debug, Clone)]
pub struct Tree {
    /// Name of the tree (used in chunk IDs).
    pub name: String,
    /// Resolved absolute path to the tree directory.
    pub path: PathBuf,
    /// Whether this tree was defined in the global `~/.ra.toml`.
    pub is_global: bool,
    /// Include patterns for files to index (defaults to ["**/*.md", "**/*.txt"]).
    pub include: Vec<String>,
    /// Exclude patterns for files to skip (defaults to empty).
    pub exclude: Vec<String>,
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
        assert!(context.patterns.is_empty());
    }

    #[test]
    fn test_config_default() {
        let config = Config::default();
        assert!(config.trees.is_empty());
        assert!(config.config_root.is_none());
    }

    #[test]
    fn test_tree_creation() {
        let tree = Tree {
            name: "docs".into(),
            path: PathBuf::from("/home/user/docs"),
            is_global: false,
            include: vec!["**/*.md".into()],
            exclude: vec![],
        };
        assert_eq!(tree.name, "docs");
        assert!(!tree.is_global);
        assert_eq!(tree.include, vec!["**/*.md"]);
        assert!(tree.exclude.is_empty());
    }

    #[test]
    fn test_settings_to_toml() {
        let config = Config::default();
        let toml = config.settings_to_toml();

        // Should produce valid TOML with expected sections
        assert!(toml.contains("[settings]"));
        assert!(toml.contains("[search]"));
        assert!(toml.contains("[context]"));
        assert!(toml.contains("[context.patterns]"));

        // Should contain default values in TOML format
        assert!(toml.contains("default_limit = 5"));
        assert!(toml.contains("stemmer = \"english\""));
        assert!(toml.contains("limit = 10"));

        // Should be parseable as valid TOML
        let parsed: toml::Value =
            toml::from_str(&toml).expect("settings_to_toml should produce valid TOML");
        assert!(parsed.get("settings").is_some());
        assert!(parsed.get("search").is_some());
        assert!(parsed.get("context").is_some());
    }
}
