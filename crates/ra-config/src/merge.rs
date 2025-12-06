//! Configuration merging.
//!
//! Merges multiple `RawConfig` files into a single resolved `Config`,
//! applying precedence rules and resolving paths.

use std::{collections::HashMap, path::PathBuf};

use crate::{
    Config, ConfigError, ContextRule, ContextSettings, SearchOverrides, SearchSettings, Settings,
    Tree,
    discovery::is_global_config,
    parse::{
        RawConfig, RawContextRule, RawContextSettings, RawSearchSettings, RawSettings, RawTree,
    },
    resolve::resolve_tree_path,
};

/// Default include patterns when none are specified.
const DEFAULT_INCLUDE_PATTERNS: &[&str] = &["**/*.md", "**/*.txt"];

/// A parsed config file with its source path.
pub struct ParsedConfig {
    /// Path to the config file.
    pub path: PathBuf,
    /// Parsed raw configuration.
    pub config: RawConfig,
}

/// Merges multiple configuration files into a single resolved `Config`.
///
/// Configs should be provided in precedence order: highest precedence first (closest to CWD),
/// lowest precedence last (global config).
///
/// Merge rules:
/// - Scalar settings: first defined value wins (highest precedence)
/// - Trees: merged by name, first definition wins completely (path, include, exclude)
/// - Context patterns: merged, first definition for each key wins
pub fn merge_configs(configs: &[ParsedConfig]) -> Result<Config, ConfigError> {
    if configs.is_empty() {
        return Ok(Config::default());
    }

    let settings = merge_settings(configs);
    let search = merge_search_settings(configs);
    let context = merge_context_settings(configs);
    let trees = merge_trees(configs)?;
    let config_root = configs
        .first()
        .map(|c| c.path.parent().unwrap().to_path_buf());

    Ok(Config {
        settings,
        search,
        context,
        trees,
        config_root,
    })
}

/// Merges general settings, taking first defined value for each field.
fn merge_settings(configs: &[ParsedConfig]) -> Settings {
    let mut result = Settings::default();

    // Iterate in reverse (lowest precedence first) so higher precedence overwrites
    for parsed in configs.iter().rev() {
        if let Some(ref settings) = parsed.config.settings {
            apply_raw_settings(&mut result, settings);
        }
    }

    result
}

/// Applies raw settings to result, overwriting any present values.
fn apply_raw_settings(result: &mut Settings, raw: &RawSettings) {
    if let Some(v) = raw.default_limit {
        result.default_limit = v;
    }
    if let Some(v) = raw.local_boost {
        result.local_boost = v;
    }
    if let Some(v) = raw.chunk_at_headings {
        result.chunk_at_headings = v;
    }
    if let Some(v) = raw.max_chunk_size {
        result.max_chunk_size = v;
    }
}

/// Merges search settings.
fn merge_search_settings(configs: &[ParsedConfig]) -> SearchSettings {
    let mut result = SearchSettings::default();

    // Iterate in reverse (lowest precedence first) so higher precedence overwrites
    for parsed in configs.iter().rev() {
        if let Some(ref search) = parsed.config.search {
            apply_raw_search(&mut result, search);
        }
    }

    result
}

/// Applies raw search settings to result.
fn apply_raw_search(result: &mut SearchSettings, raw: &RawSearchSettings) {
    if let Some(ref v) = raw.stemmer {
        result.stemmer = v.clone();
    }
    if let Some(v) = raw.fuzzy_distance {
        result.fuzzy_distance = v;
    }
    if let Some(v) = raw.limit {
        result.limit = v;
    }
    if let Some(v) = raw.candidate_limit {
        result.candidate_limit = v;
    }
    if let Some(v) = raw.cutoff_ratio {
        result.cutoff_ratio = v;
    }
    if let Some(v) = raw.aggregation_threshold {
        result.aggregation_threshold = v;
    }
}

/// Merges context settings.
fn merge_context_settings(configs: &[ParsedConfig]) -> ContextSettings {
    let mut result = ContextSettings::default();

    // For scalar values, iterate in reverse (lowest precedence first) so higher precedence
    // overwrites. For rules, iterate in forward order (highest precedence first) so those
    // rules are checked first when matching.

    // First pass: scalar values (reverse order)
    for parsed in configs.iter().rev() {
        if let Some(ref context) = parsed.config.context {
            apply_raw_context_scalars(&mut result, context);
        }
    }

    // Second pass: rules (forward order - high precedence first)
    for parsed in configs {
        if let Some(ref context) = parsed.config.context
            && let Some(ref rules) = context.rules
        {
            for rule in rules {
                result.rules.push(convert_context_rule(rule));
            }
        }
    }

    result
}

/// Applies raw context scalar settings to result (not rules).
fn apply_raw_context_scalars(result: &mut ContextSettings, raw: &RawContextSettings) {
    if let Some(v) = raw.terms {
        result.terms = v;
    }
    if let Some(v) = raw.min_term_frequency {
        result.min_term_frequency = v;
    }
    if let Some(v) = raw.min_word_length {
        result.min_word_length = v;
    }
    if let Some(v) = raw.max_word_length {
        result.max_word_length = v;
    }
    if let Some(v) = raw.sample_size {
        result.sample_size = v;
    }
}

/// Converts a raw context rule to the resolved type.
fn convert_context_rule(raw: &RawContextRule) -> ContextRule {
    let search = raw.search.as_ref().map(|s| SearchOverrides {
        limit: s.limit,
        candidate_limit: s.candidate_limit,
        cutoff_ratio: s.cutoff_ratio,
        aggregation_threshold: s.aggregation_threshold,
    });

    ContextRule {
        patterns: raw.patterns.clone(),
        trees: raw.trees.clone().unwrap_or_default(),
        terms: raw.terms.clone().unwrap_or_default(),
        include: raw.include.clone().unwrap_or_default(),
        search,
    }
}

/// Merges trees from all configs, resolving paths.
///
/// Trees are merged by name - first definition wins completely.
/// `is_global` is determined by whether the source config file is `~/.ra.toml`.
fn merge_trees(configs: &[ParsedConfig]) -> Result<Vec<Tree>, ConfigError> {
    let mut seen: HashMap<String, Tree> = HashMap::new();

    // Iterate in precedence order (highest first) - first definition wins
    for parsed in configs {
        let Some(ref trees) = parsed.config.tree else {
            continue;
        };

        let config_dir = parsed.path.parent().unwrap();
        let is_global = is_global_config(&parsed.path);

        for (name, raw_tree) in trees {
            if seen.contains_key(name) {
                // Already defined by higher-precedence config
                continue;
            }

            let resolved_path = resolve_tree_path(&raw_tree.path, config_dir)?;

            seen.insert(
                name.clone(),
                convert_tree(name, raw_tree, resolved_path, is_global),
            );
        }
    }

    // Return trees in a deterministic order (by name)
    let mut trees: Vec<Tree> = seen.into_values().collect();
    trees.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(trees)
}

/// Converts a raw tree to the final type with defaults applied.
fn convert_tree(name: &str, raw: &RawTree, resolved_path: PathBuf, is_global: bool) -> Tree {
    let include = raw.include.clone().unwrap_or_else(|| {
        DEFAULT_INCLUDE_PATTERNS
            .iter()
            .map(|s| (*s).to_string())
            .collect()
    });

    let exclude = raw.exclude.clone().unwrap_or_default();

    Tree {
        name: name.to_string(),
        path: resolved_path,
        is_global,
        include,
        exclude,
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::{parse::parse_config_str, test_support::TestDir};

    #[test]
    fn test_merge_empty_configs() {
        let result = merge_configs(&[]).unwrap();
        assert_eq!(result.settings.default_limit, 5); // default
        assert!(result.trees.is_empty());
    }

    #[test]
    fn test_merge_single_config() {
        let test_dir = TestDir::new();
        test_dir.create_dir("docs");

        let parsed = ParsedConfig {
            path: test_dir.path().join(".ra.toml"),
            config: parse_config_str(
                r#"
[settings]
default_limit = 10

[tree.local]
path = "./docs"
"#,
                Path::new("test"),
            )
            .unwrap(),
        };

        let result = merge_configs(&[parsed]).unwrap();
        assert_eq!(result.settings.default_limit, 10);
        assert_eq!(result.trees.len(), 1);
        assert_eq!(result.trees[0].name, "local");
        // Should have default include patterns
        assert_eq!(result.trees[0].include, vec!["**/*.md", "**/*.txt"]);
        assert!(result.trees[0].exclude.is_empty());
    }

    #[test]
    fn test_merge_tree_with_patterns() {
        let test_dir = TestDir::new();
        test_dir.create_dir("docs");

        let parsed = ParsedConfig {
            path: test_dir.path().join(".ra.toml"),
            config: parse_config_str(
                r#"
[tree.local]
path = "./docs"
include = ["**/*.md"]
exclude = ["**/drafts/**"]
"#,
                Path::new("test"),
            )
            .unwrap(),
        };

        let result = merge_configs(&[parsed]).unwrap();
        assert_eq!(result.trees[0].include, vec!["**/*.md"]);
        assert_eq!(result.trees[0].exclude, vec!["**/drafts/**"]);
    }

    #[test]
    fn test_merge_scalar_override() {
        let test_dir = TestDir::new();

        // Higher precedence config (closer to CWD)
        let high_prec = ParsedConfig {
            path: test_dir.path().join("project/.ra.toml"),
            config: parse_config_str(
                r#"
[settings]
default_limit = 20
"#,
                Path::new("test"),
            )
            .unwrap(),
        };

        // Lower precedence config
        let low_prec = ParsedConfig {
            path: test_dir.path().join(".ra.toml"),
            config: parse_config_str(
                r#"
[settings]
default_limit = 5
local_boost = 2.0
"#,
                Path::new("test"),
            )
            .unwrap(),
        };

        let result = merge_configs(&[high_prec, low_prec]).unwrap();

        // High precedence wins for default_limit
        assert_eq!(result.settings.default_limit, 20);
        // Low precedence provides local_boost (not overridden)
        assert!((result.settings.local_boost - 2.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_merge_trees_first_wins() {
        let test_dir = TestDir::new();
        let docs1 = test_dir.create_dir("project/docs");
        let _docs2 = test_dir.create_dir("docs");

        let high_prec = ParsedConfig {
            path: test_dir.path().join("project/.ra.toml"),
            config: parse_config_str(
                r#"
[tree.docs]
path = "./docs"
include = ["**/*.md"]
"#,
                Path::new("test"),
            )
            .unwrap(),
        };

        let low_prec = ParsedConfig {
            path: test_dir.path().join(".ra.toml"),
            config: parse_config_str(
                r#"
[tree.docs]
path = "./docs"
include = ["**/*"]
"#,
                Path::new("test"),
            )
            .unwrap(),
        };

        let result = merge_configs(&[high_prec, low_prec]).unwrap();

        assert_eq!(result.trees.len(), 1);
        // Should resolve to project/docs, not root/docs
        assert_eq!(result.trees[0].path, docs1.canonicalize().unwrap());
        // High precedence includes should win
        assert_eq!(result.trees[0].include, vec!["**/*.md"]);
    }

    #[test]
    fn test_merge_trees_different_names() {
        let test_dir = TestDir::new();
        test_dir.create_dir("project/local");
        test_dir.create_dir("global");

        let high_prec = ParsedConfig {
            path: test_dir.path().join("project/.ra.toml"),
            config: parse_config_str(
                r#"
[tree.local]
path = "./local"
"#,
                Path::new("test"),
            )
            .unwrap(),
        };

        let low_prec = ParsedConfig {
            path: test_dir.path().join(".ra.toml"),
            config: parse_config_str(
                r#"
[tree.global]
path = "./global"
"#,
                Path::new("test"),
            )
            .unwrap(),
        };

        let result = merge_configs(&[high_prec, low_prec]).unwrap();

        assert_eq!(result.trees.len(), 2);
        // Trees should be sorted by name
        assert_eq!(result.trees[0].name, "global");
        assert_eq!(result.trees[1].name, "local");
    }

    #[test]
    fn test_merge_context_rules() {
        let test_dir = TestDir::new();

        let high_prec = ParsedConfig {
            path: test_dir.path().join("project/.ra.toml"),
            config: parse_config_str(
                r#"
[[context.rules]]
match = "*.rs"
terms = ["rust", "systems"]

[[context.rules]]
match = "src/api/**"
terms = ["http"]
"#,
                Path::new("test"),
            )
            .unwrap(),
        };

        let low_prec = ParsedConfig {
            path: test_dir.path().join(".ra.toml"),
            config: parse_config_str(
                r#"
[[context.rules]]
match = "*.py"
terms = ["python"]
"#,
                Path::new("test"),
            )
            .unwrap(),
        };

        let result = merge_configs(&[high_prec, low_prec]).unwrap();

        // Rules are accumulated with high precedence first
        assert_eq!(result.context.rules.len(), 3);

        // High precedence rules come first
        assert_eq!(result.context.rules[0].patterns, vec!["*.rs"]);
        assert_eq!(
            result.context.rules[0].terms,
            vec!["rust".to_string(), "systems".to_string()]
        );
        assert_eq!(result.context.rules[1].patterns, vec!["src/api/**"]);
        assert_eq!(result.context.rules[1].terms, vec!["http".to_string()]);

        // Low precedence rules come last
        assert_eq!(result.context.rules[2].patterns, vec!["*.py"]);
        assert_eq!(result.context.rules[2].terms, vec!["python".to_string()]);
    }

    #[test]
    fn test_merge_three_way() {
        let test_dir = TestDir::new();
        test_dir.create_dir("project/sub/local");
        test_dir.create_dir("project/shared");
        test_dir.create_dir("global");

        // Highest precedence (deepest)
        let leaf = ParsedConfig {
            path: test_dir.path().join("project/sub/.ra.toml"),
            config: parse_config_str(
                r#"
[settings]
default_limit = 3

[tree.local]
path = "./local"
"#,
                Path::new("test"),
            )
            .unwrap(),
        };

        // Middle precedence
        let mid = ParsedConfig {
            path: test_dir.path().join("project/.ra.toml"),
            config: parse_config_str(
                r#"
[settings]
default_limit = 5
local_boost = 2.0

[tree.shared]
path = "./shared"
include = ["**/*.md"]
"#,
                Path::new("test"),
            )
            .unwrap(),
        };

        // Lowest precedence
        let root = ParsedConfig {
            path: test_dir.path().join(".ra.toml"),
            config: parse_config_str(
                r#"
[settings]
default_limit = 10
local_boost = 1.5
chunk_at_headings = false

[tree.global]
path = "./global"
include = ["**/*"]
"#,
                Path::new("test"),
            )
            .unwrap(),
        };

        let result = merge_configs(&[leaf, mid, root]).unwrap();

        // Settings: leaf wins default_limit, mid wins local_boost, root wins chunk_at_headings
        assert_eq!(result.settings.default_limit, 3);
        assert!((result.settings.local_boost - 2.0).abs() < f32::EPSILON);
        assert!(!result.settings.chunk_at_headings);

        // Trees: all three should be present
        assert_eq!(result.trees.len(), 3);

        // Config root should be the highest precedence config's directory
        assert_eq!(
            result.config_root,
            Some(test_dir.path().join("project/sub"))
        );
    }

    #[test]
    fn test_merge_search_settings() {
        let test_dir = TestDir::new();

        let high_prec = ParsedConfig {
            path: test_dir.path().join("project/.ra.toml"),
            config: parse_config_str(
                r#"
[search]
stemmer = "french"
"#,
                Path::new("test"),
            )
            .unwrap(),
        };

        let low_prec = ParsedConfig {
            path: test_dir.path().join(".ra.toml"),
            config: parse_config_str(
                r#"
[search]
stemmer = "german"
"#,
                Path::new("test"),
            )
            .unwrap(),
        };

        let result = merge_configs(&[high_prec, low_prec]).unwrap();

        assert_eq!(result.search.stemmer, "french"); // high prec wins
    }
}
