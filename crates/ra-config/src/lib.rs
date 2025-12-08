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
#[cfg(test)]
mod test_support;
mod validate;

use std::path::{Component, Path, PathBuf};

use directories::BaseDirs;
pub use discovery::{CONFIG_FILENAME, discover_config_files, global_config_path, is_global_config};

// =============================================================================
// Default value constants
//
// These constants define hardcoded defaults for all configurable parameters.
// They are public so the CLI can reference them in help text via clap's
// `default_value` attribute, ensuring documentation stays in sync.
// =============================================================================

/// Default limit for general queries (Settings.default_limit).
pub const DEFAULT_LIMIT: usize = 5;

/// Default relevance multiplier for local trees (Settings.local_boost).
pub const DEFAULT_LOCAL_BOOST: f32 = 1.5;

/// Default setting for splitting documents at headings (Settings.chunk_at_headings).
pub const DEFAULT_CHUNK_AT_HEADINGS: bool = true;

/// Default warning threshold for chunk size (Settings.max_chunk_size).
pub const DEFAULT_MAX_CHUNK_SIZE: usize = 50_000;

/// Default stemming language (SearchSettings.stemmer).
pub const DEFAULT_STEMMER: &str = "english";

/// Default fuzzy matching distance (SearchSettings.fuzzy_distance).
pub const DEFAULT_FUZZY_DISTANCE: u8 = 1;

/// Default maximum results for search (SearchSettings.limit).
pub const DEFAULT_SEARCH_LIMIT: usize = 10;

/// Default size of the aggregation pool (SearchSettings.aggregation_pool_size).
///
/// This controls how many candidates are available for hierarchical aggregation
/// before elbow cutoff. A larger pool allows more siblings to accumulate and
/// merge, improving aggregation quality.
///
/// This replaces the old `max_candidates` setting.
pub const DEFAULT_AGGREGATION_POOL_SIZE: usize = 500;

/// Default score ratio threshold for elbow cutoff (SearchSettings.cutoff_ratio).
pub const DEFAULT_CUTOFF_RATIO: f32 = 0.3;

/// Default sibling ratio threshold for aggregation (SearchSettings.aggregation_threshold).
pub const DEFAULT_AGGREGATION_THRESHOLD: f32 = 0.1;

// =============================================================================
// Search field boost defaults
//
// These constants define the query-time boost weights for different index fields.
// Higher values increase the relevance contribution from matches in that field.
// =============================================================================

/// Default boost for hierarchy field (matches in document headings).
pub const DEFAULT_BOOST_HIERARCHY: f32 = 3.0;
/// Default boost for path field (filename matches).
pub const DEFAULT_BOOST_PATH: f32 = 12.0;
/// Default boost for tags field (frontmatter metadata).
pub const DEFAULT_BOOST_TAGS: f32 = 5.0;
/// Default boost for body field (document content).
pub const DEFAULT_BOOST_BODY: f32 = 1.0;
/// Default maximum boost for top-level headings (depth 0-1).
pub const DEFAULT_BOOST_HEADING_MAX: f32 = 5.0;
/// Default decay factor for heading depth boost (each level multiplies by this).
pub const DEFAULT_BOOST_HEADING_DECAY: f32 = 0.7;

// =============================================================================
// Context markdown parser boost defaults
//
// These constants define term weights for the markdown parser used in context
// extraction. Higher weights increase term significance in context queries.
// =============================================================================

/// Default weight for terms in H1 headings.
pub const DEFAULT_MARKDOWN_H1: f32 = 3.0;
/// Default weight for terms in H2-H3 headings.
pub const DEFAULT_MARKDOWN_H2_H3: f32 = 2.0;
/// Default weight for terms in H4-H6 headings.
pub const DEFAULT_MARKDOWN_H4_H6: f32 = 1.5;
/// Default weight for terms in body text.
pub const DEFAULT_MARKDOWN_BODY: f32 = 1.0;

/// Field boost configuration for search ranking.
///
/// These weights are applied at query time to control how matches in different
/// fields contribute to the final score.
#[derive(Debug, Clone, Copy)]
pub struct FieldBoosts {
    /// Boost for hierarchy field (document headings).
    pub hierarchy: f32,
    /// Boost for path field (filename matches).
    pub path: f32,
    /// Boost for tags field (frontmatter metadata).
    pub tags: f32,
    /// Boost for body field (document content).
    pub body: f32,
    /// Maximum boost for top-level headings (depth 0-1).
    pub heading_max: f32,
    /// Decay factor per heading level.
    pub heading_decay: f32,
}

impl Default for FieldBoosts {
    fn default() -> Self {
        Self {
            hierarchy: DEFAULT_BOOST_HIERARCHY,
            path: DEFAULT_BOOST_PATH,
            tags: DEFAULT_BOOST_TAGS,
            body: DEFAULT_BOOST_BODY,
            heading_max: DEFAULT_BOOST_HEADING_MAX,
            heading_decay: DEFAULT_BOOST_HEADING_DECAY,
        }
    }
}

impl FieldBoosts {
    /// Computes a depth-based boost for a chunk's heading level.
    ///
    /// - depth 0 (document) and depth 1 (h1) get the maximum boost
    /// - Each subsequent level decays by the decay factor
    ///   - h2 (depth 2): max * decay
    ///   - h3 (depth 3): max * decay²
    ///   - h4 (depth 4): max * decay³
    ///   - etc.
    pub fn heading_boost(&self, depth: u64) -> f32 {
        if depth <= 1 {
            self.heading_max
        } else {
            self.heading_max * self.heading_decay.powi((depth - 1) as i32)
        }
    }
}

/// Markdown parser weight configuration for context term extraction.
///
/// These weights control how terms from different heading levels contribute
/// to the context query.
#[derive(Debug, Clone, Copy)]
pub struct MarkdownWeights {
    /// Weight for terms in H1 headings.
    pub h1: f32,
    /// Weight for terms in H2-H3 headings.
    pub h2_h3: f32,
    /// Weight for terms in H4-H6 headings.
    pub h4_h6: f32,
    /// Weight for terms in body text.
    pub body: f32,
}

impl Default for MarkdownWeights {
    fn default() -> Self {
        Self {
            h1: DEFAULT_MARKDOWN_H1,
            h2_h3: DEFAULT_MARKDOWN_H2_H3,
            h4_h6: DEFAULT_MARKDOWN_H4_H6,
            body: DEFAULT_MARKDOWN_BODY,
        }
    }
}

impl MarkdownWeights {
    /// Returns the source label and weight for a heading level.
    pub fn heading_weight(&self, level: u8) -> (&'static str, f32) {
        match level {
            1 => ("md:h1", self.h1),
            2 | 3 => ("md:h2-h3", self.h2_h3),
            _ => ("md:h4-h6", self.h4_h6),
        }
    }
}

/// Default maximum terms for context queries (ContextSettings.terms).
pub const DEFAULT_CONTEXT_TERMS: usize = 50;

/// Default minimum term frequency (ContextSettings.min_term_frequency).
pub const DEFAULT_MIN_TERM_FREQUENCY: usize = 2;

/// Default minimum word length (ContextSettings.min_word_length).
pub const DEFAULT_MIN_WORD_LENGTH: usize = 4;

/// Default maximum word length (ContextSettings.max_word_length).
pub const DEFAULT_MAX_WORD_LENGTH: usize = 30;

/// Default sample size for large files (ContextSettings.sample_size).
pub const DEFAULT_SAMPLE_SIZE: usize = 50_000;
pub use error::ConfigError;
pub use patterns::{CompiledContextRules, CompiledPatterns, MatchedRules};
use serde::{Deserialize, Serialize};
pub use templates::{global_template, local_template};
pub use validate::ConfigWarning;
use validate::validate_config;

use crate::{
    merge::{ParsedConfig, merge_configs},
    parse::parse_config_file,
};

/// Formats a path for display, using `~` for home directory or relative paths where appropriate.
///
/// - If `base` is provided and the path is under it, returns a relative path
/// - If the path is under the home directory, replaces the home prefix with `~`
/// - Otherwise returns the path as-is
pub fn format_path_for_display(path: &Path, base: Option<&Path>) -> String {
    // Try relative path first if base is provided
    if let Some(base_path) = base
        && let Some(relative) = pathdiff::diff_paths(path, base_path)
    {
        // Only use relative if it doesn't start with too many ..
        let components: Vec<_> = relative.components().collect();
        let parent_count = components
            .iter()
            .take_while(|c| matches!(c, Component::ParentDir))
            .count();
        // Use relative path if it's simpler (at most 2 parent references)
        if parent_count <= 2 {
            let rel_str = relative.display().to_string();
            // Prefer explicit ./ prefix for clarity
            if !rel_str.starts_with("..") && !rel_str.starts_with('/') {
                return format!("./{rel_str}");
            }
            return rel_str;
        }
    }

    // Try to use ~ for home directory
    if let Some(base_dirs) = BaseDirs::new() {
        let home = base_dirs.home_dir();
        if let Ok(suffix) = path.strip_prefix(home) {
            return format!("~/{}", suffix.display());
        }
    }

    // Fall back to absolute path
    path.display().to_string()
}

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
            default_limit: DEFAULT_LIMIT,
            local_boost: DEFAULT_LOCAL_BOOST,
            chunk_at_headings: DEFAULT_CHUNK_AT_HEADINGS,
            max_chunk_size: DEFAULT_MAX_CHUNK_SIZE,
        }
    }
}

/// Common search parameters shared between search and context commands.
///
/// Both `SearchSettings` and `ContextSettings` implement this trait, allowing
/// shared code for building search parameters from CLI overrides and config defaults.
pub trait SearchDefaults {
    /// Maximum results to return after aggregation.
    fn limit(&self) -> usize;
    /// Size of the aggregation pool - how many candidates are available for
    /// hierarchical aggregation before elbow cutoff.
    fn aggregation_pool_size(&self) -> usize;
    /// Score ratio threshold for elbow cutoff (0.0-1.0, lower = more results).
    fn cutoff_ratio(&self) -> f32;
    /// Sibling ratio threshold for hierarchical aggregation.
    fn aggregation_threshold(&self) -> f32;
}

/// Search-related settings.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct SearchSettings {
    /// Stemming language.
    pub stemmer: String,
    /// Fuzzy matching Levenshtein distance (0 = disabled).
    pub fuzzy_distance: u8,
    /// Maximum results to return after aggregation.
    pub limit: usize,
    /// Size of the aggregation pool - how many candidates are available for
    /// hierarchical aggregation before elbow cutoff. Larger values allow more
    /// siblings to accumulate, improving aggregation quality.
    #[serde(alias = "max_candidates")]
    pub aggregation_pool_size: usize,
    /// Score ratio threshold for elbow cutoff (0.0-1.0, lower = more results).
    pub cutoff_ratio: f32,
    /// Sibling ratio threshold for hierarchical aggregation.
    pub aggregation_threshold: f32,

    // Field boost weights (applied at query time)
    /// Boost multiplier for hierarchy field matches.
    pub boost_hierarchy: f32,
    /// Boost multiplier for path field matches.
    pub boost_path: f32,
    /// Boost multiplier for tags field matches.
    pub boost_tags: f32,
    /// Boost multiplier for body field matches.
    pub boost_body: f32,
    /// Maximum boost for top-level headings (depth 0-1).
    pub boost_heading_max: f32,
    /// Decay factor per heading level (h2 = max * decay, h3 = max * decay², etc.).
    pub boost_heading_decay: f32,

    // Markdown parser weights (for context term extraction)
    /// Weight for terms extracted from H1 headings.
    pub markdown_h1: f32,
    /// Weight for terms extracted from H2-H3 headings.
    pub markdown_h2_h3: f32,
    /// Weight for terms extracted from H4-H6 headings.
    pub markdown_h4_h6: f32,
    /// Weight for terms extracted from body text.
    pub markdown_body: f32,
}

impl Default for SearchSettings {
    fn default() -> Self {
        Self {
            stemmer: String::from(DEFAULT_STEMMER),
            fuzzy_distance: DEFAULT_FUZZY_DISTANCE,
            limit: DEFAULT_SEARCH_LIMIT,
            aggregation_pool_size: DEFAULT_AGGREGATION_POOL_SIZE,
            cutoff_ratio: DEFAULT_CUTOFF_RATIO,
            aggregation_threshold: DEFAULT_AGGREGATION_THRESHOLD,
            // Field boosts
            boost_hierarchy: DEFAULT_BOOST_HIERARCHY,
            boost_path: DEFAULT_BOOST_PATH,
            boost_tags: DEFAULT_BOOST_TAGS,
            boost_body: DEFAULT_BOOST_BODY,
            boost_heading_max: DEFAULT_BOOST_HEADING_MAX,
            boost_heading_decay: DEFAULT_BOOST_HEADING_DECAY,
            // Markdown parser weights
            markdown_h1: DEFAULT_MARKDOWN_H1,
            markdown_h2_h3: DEFAULT_MARKDOWN_H2_H3,
            markdown_h4_h6: DEFAULT_MARKDOWN_H4_H6,
            markdown_body: DEFAULT_MARKDOWN_BODY,
        }
    }
}

impl SearchSettings {
    /// Returns the field boost configuration from these settings.
    pub fn field_boosts(&self) -> FieldBoosts {
        FieldBoosts {
            hierarchy: self.boost_hierarchy,
            path: self.boost_path,
            tags: self.boost_tags,
            body: self.boost_body,
            heading_max: self.boost_heading_max,
            heading_decay: self.boost_heading_decay,
        }
    }

    /// Returns the markdown parser weight configuration from these settings.
    pub fn markdown_weights(&self) -> MarkdownWeights {
        MarkdownWeights {
            h1: self.markdown_h1,
            h2_h3: self.markdown_h2_h3,
            h4_h6: self.markdown_h4_h6,
            body: self.markdown_body,
        }
    }
}

impl SearchDefaults for SearchSettings {
    fn limit(&self) -> usize {
        self.limit
    }
    fn aggregation_pool_size(&self) -> usize {
        self.aggregation_pool_size
    }
    fn cutoff_ratio(&self) -> f32 {
        self.cutoff_ratio
    }
    fn aggregation_threshold(&self) -> f32 {
        self.aggregation_threshold
    }
}

/// Settings for the `ra context` command.
///
/// Contains only context-specific term extraction settings. Search parameters
/// are inherited from `SearchSettings` and can be overridden per-rule.
#[derive(Debug, Clone)]
pub struct ContextSettings {
    /// Maximum terms to include in context queries.
    ///
    /// Higher values capture more semantic diversity (locations, characters,
    /// concepts) which is important for prose content where significant terms
    /// may appear infrequently. Default is 50.
    pub terms: usize,
    /// Ignore terms appearing less than this many times in source.
    pub min_term_frequency: usize,
    /// Ignore words shorter than this.
    pub min_word_length: usize,
    /// Ignore words longer than this.
    pub max_word_length: usize,
    /// Maximum bytes to analyze from large files.
    pub sample_size: usize,
    /// Context rules for customizing search behavior per file pattern.
    pub rules: Vec<ContextRule>,
}

/// A resolved context rule.
///
/// Specifies how context search should behave for files matching certain patterns.
#[derive(Debug, Clone)]
pub struct ContextRule {
    /// Glob patterns to match against file paths.
    pub patterns: Vec<String>,
    /// Limit search to these trees (empty means all trees).
    pub trees: Vec<String>,
    /// Additional search terms to inject into the query.
    pub terms: Vec<String>,
    /// Files to always include in results (tree-prefixed paths like "docs:api/overview.md").
    pub include: Vec<String>,
    /// Search parameter overrides for this rule.
    pub search: Option<SearchOverrides>,
}

/// Search parameter overrides that can be specified per context rule.
///
/// All fields are optional; unset fields inherit from `[search]`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct SearchOverrides {
    /// Maximum results to return after aggregation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    /// Size of the aggregation pool - how many candidates are available for
    /// hierarchical aggregation before elbow cutoff.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aggregation_pool_size: Option<usize>,
    /// Score ratio threshold for elbow cutoff (0.0-1.0, lower = more results).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cutoff_ratio: Option<f32>,
    /// Sibling ratio threshold for hierarchical aggregation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aggregation_threshold: Option<f32>,
}

impl SearchOverrides {
    /// Returns true if all fields are None.
    pub fn is_empty(&self) -> bool {
        self.limit.is_none()
            && self.aggregation_pool_size.is_none()
            && self.cutoff_ratio.is_none()
            && self.aggregation_threshold.is_none()
    }
}

impl Default for ContextSettings {
    fn default() -> Self {
        Self {
            terms: DEFAULT_CONTEXT_TERMS,
            min_term_frequency: DEFAULT_MIN_TERM_FREQUENCY,
            min_word_length: DEFAULT_MIN_WORD_LENGTH,
            max_word_length: DEFAULT_MAX_WORD_LENGTH,
            sample_size: DEFAULT_SAMPLE_SIZE,
            rules: Vec::new(),
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

/// Context settings serializable to TOML.
#[derive(Serialize)]
struct SerializableContextSettings {
    /// Ignore terms appearing less than this many times in source.
    min_term_frequency: usize,
    /// Ignore words shorter than this.
    min_word_length: usize,
    /// Ignore words longer than this.
    max_word_length: usize,
    /// Maximum bytes to analyze from large files.
    sample_size: usize,
    /// Context rules for customizing search behavior.
    rules: Vec<SerializableContextRule>,
}

/// A context rule serializable to TOML.
#[derive(Serialize)]
struct SerializableContextRule {
    /// Glob pattern(s) to match.
    #[serde(rename = "match")]
    patterns: StringOrVec,
    /// Trees to limit search to.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    trees: Vec<String>,
    /// Terms to inject.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    terms: Vec<String>,
    /// Files to always include.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    include: Vec<String>,
    /// Search parameter overrides.
    #[serde(skip_serializing_if = "Option::is_none")]
    search: Option<SearchOverrides>,
}

/// Helper for serializing either a single string or array.
#[derive(Clone)]
enum StringOrVec {
    /// A single string value.
    Single(String),
    /// Multiple string values.
    Multiple(Vec<String>),
}

impl Serialize for StringOrVec {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Single(s) => serializer.serialize_str(s),
            Self::Multiple(v) => v.serialize(serializer),
        }
    }
}

impl From<&ContextSettings> for SerializableContextSettings {
    fn from(ctx: &ContextSettings) -> Self {
        Self {
            min_term_frequency: ctx.min_term_frequency,
            min_word_length: ctx.min_word_length,
            max_word_length: ctx.max_word_length,
            sample_size: ctx.sample_size,
            rules: ctx
                .rules
                .iter()
                .map(SerializableContextRule::from)
                .collect(),
        }
    }
}

impl From<&ContextRule> for SerializableContextRule {
    fn from(rule: &ContextRule) -> Self {
        let patterns = if rule.patterns.len() == 1 {
            StringOrVec::Single(rule.patterns[0].clone())
        } else {
            StringOrVec::Multiple(rule.patterns.clone())
        };
        Self {
            patterns,
            trees: rule.trees.clone(),
            terms: rule.terms.clone(),
            include: rule.include.clone(),
            search: rule.search.clone(),
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
        assert_eq!(settings.default_limit, DEFAULT_LIMIT);
        assert!((settings.local_boost - DEFAULT_LOCAL_BOOST).abs() < f32::EPSILON);
        assert_eq!(settings.chunk_at_headings, DEFAULT_CHUNK_AT_HEADINGS);
        assert_eq!(settings.max_chunk_size, DEFAULT_MAX_CHUNK_SIZE);
    }

    #[test]
    fn test_search_settings_defaults() {
        let search = SearchSettings::default();
        assert_eq!(search.stemmer, DEFAULT_STEMMER);
        assert_eq!(search.fuzzy_distance, DEFAULT_FUZZY_DISTANCE);
        assert_eq!(search.limit, DEFAULT_SEARCH_LIMIT);
        assert_eq!(search.aggregation_pool_size, DEFAULT_AGGREGATION_POOL_SIZE);
        assert!((search.cutoff_ratio - DEFAULT_CUTOFF_RATIO).abs() < f32::EPSILON);
        assert!(
            (search.aggregation_threshold - DEFAULT_AGGREGATION_THRESHOLD).abs() < f32::EPSILON
        );
        // Field boosts
        assert!((search.boost_hierarchy - DEFAULT_BOOST_HIERARCHY).abs() < f32::EPSILON);
        assert!((search.boost_path - DEFAULT_BOOST_PATH).abs() < f32::EPSILON);
        assert!((search.boost_tags - DEFAULT_BOOST_TAGS).abs() < f32::EPSILON);
        assert!((search.boost_body - DEFAULT_BOOST_BODY).abs() < f32::EPSILON);
        assert!((search.boost_heading_max - DEFAULT_BOOST_HEADING_MAX).abs() < f32::EPSILON);
        assert!((search.boost_heading_decay - DEFAULT_BOOST_HEADING_DECAY).abs() < f32::EPSILON);
        // Markdown weights
        assert!((search.markdown_h1 - DEFAULT_MARKDOWN_H1).abs() < f32::EPSILON);
        assert!((search.markdown_h2_h3 - DEFAULT_MARKDOWN_H2_H3).abs() < f32::EPSILON);
        assert!((search.markdown_h4_h6 - DEFAULT_MARKDOWN_H4_H6).abs() < f32::EPSILON);
        assert!((search.markdown_body - DEFAULT_MARKDOWN_BODY).abs() < f32::EPSILON);
    }

    #[test]
    fn test_field_boosts() {
        let boosts = FieldBoosts::default();
        // Test heading boost calculation
        assert!((boosts.heading_boost(0) - 5.0).abs() < f32::EPSILON); // Document
        assert!((boosts.heading_boost(1) - 5.0).abs() < f32::EPSILON); // H1
        assert!((boosts.heading_boost(2) - 3.5).abs() < f32::EPSILON); // H2
        assert!((boosts.heading_boost(3) - 2.45).abs() < 0.01); // H3
    }

    #[test]
    fn test_markdown_weights() {
        let weights = MarkdownWeights::default();
        assert_eq!(weights.heading_weight(1), ("md:h1", 3.0));
        assert_eq!(weights.heading_weight(2), ("md:h2-h3", 2.0));
        assert_eq!(weights.heading_weight(3), ("md:h2-h3", 2.0));
        assert_eq!(weights.heading_weight(4), ("md:h4-h6", 1.5));
    }

    #[test]
    fn test_context_settings_defaults() {
        let context = ContextSettings::default();
        assert_eq!(context.terms, DEFAULT_CONTEXT_TERMS);
        assert_eq!(context.min_term_frequency, DEFAULT_MIN_TERM_FREQUENCY);
        assert_eq!(context.min_word_length, DEFAULT_MIN_WORD_LENGTH);
        assert_eq!(context.max_word_length, DEFAULT_MAX_WORD_LENGTH);
        assert_eq!(context.sample_size, DEFAULT_SAMPLE_SIZE);
        assert!(context.rules.is_empty());
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

        // Should contain default values in TOML format
        assert!(toml.contains("default_limit = 5"));
        assert!(toml.contains("stemmer = \"english\""));
        assert!(toml.contains("min_term_frequency = 2")); // context-specific setting

        // Should be parseable as valid TOML
        let parsed: toml::Value =
            toml::from_str(&toml).expect("settings_to_toml should produce valid TOML");
        assert!(parsed.get("settings").is_some());
        assert!(parsed.get("search").is_some());
        assert!(parsed.get("context").is_some());
    }

    #[test]
    fn test_format_path_relative_to_base() {
        let base = PathBuf::from("/home/user/project");
        let path = PathBuf::from("/home/user/project/docs");

        let result = format_path_for_display(&path, Some(&base));
        assert_eq!(result, "./docs");
    }

    #[test]
    fn test_format_path_parent_dir() {
        let base = PathBuf::from("/home/user/project/sub");
        let path = PathBuf::from("/home/user/project/docs");

        let result = format_path_for_display(&path, Some(&base));
        assert_eq!(result, "../docs");
    }

    #[test]
    fn test_format_path_no_base_uses_home() {
        // This test depends on having a home directory set
        if let Some(base_dirs) = BaseDirs::new() {
            let home = base_dirs.home_dir();
            let path = home.join("some/path");

            let result = format_path_for_display(&path, None);
            assert_eq!(result, "~/some/path");
        }
    }

    #[test]
    fn test_format_path_outside_base_and_home() {
        let base = PathBuf::from("/home/user/project");
        let path = PathBuf::from("/var/log/app.log");

        let result = format_path_for_display(&path, Some(&base));
        // Should fall back to absolute since it's far from base and not in home
        assert!(result.starts_with('/'));
    }
}
