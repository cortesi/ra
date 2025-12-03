//! Configuration merging.
//!
//! Merges multiple `RawConfig` files into a single resolved `Config`,
//! applying precedence rules and resolving paths.

use std::{collections::HashMap, path::PathBuf};

use crate::{
    Config, ConfigError, ContextSettings, IncludePattern, SearchSettings, Settings, Tree,
    discovery::is_global_config,
    parse::{RawConfig, RawContextSettings, RawIncludePattern, RawSearchSettings, RawSettings},
    resolve::resolve_tree_path,
};

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
/// - Trees: merged by name, first definition wins; `is_global` based on source file
/// - Include patterns: concatenated in order (highest precedence first)
/// - Context patterns: merged, first definition for each key wins
pub fn merge_configs(configs: &[ParsedConfig]) -> Result<Config, ConfigError> {
    if configs.is_empty() {
        return Ok(Config::default());
    }

    let settings = merge_settings(configs);
    let search = merge_search_settings(configs);
    let context = merge_context_settings(configs);
    let trees = merge_trees(configs)?;
    let includes = merge_includes(configs);
    let config_root = configs
        .first()
        .map(|c| c.path.parent().unwrap().to_path_buf());

    Ok(Config {
        settings,
        search,
        context,
        trees,
        includes,
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
    if let Some(v) = raw.fuzzy {
        result.fuzzy = v;
    }
    if let Some(v) = raw.fuzzy_distance {
        result.fuzzy_distance = v;
    }
    if let Some(ref v) = raw.stemmer {
        result.stemmer = v.clone();
    }
}

/// Merges context settings.
fn merge_context_settings(configs: &[ParsedConfig]) -> ContextSettings {
    let mut result = ContextSettings::default();

    // Iterate in reverse (lowest precedence first) so higher precedence overwrites
    for parsed in configs.iter().rev() {
        if let Some(ref context) = parsed.config.context {
            apply_raw_context(&mut result, context);
        }
    }

    result
}

/// Applies raw context settings to result.
fn apply_raw_context(result: &mut ContextSettings, raw: &RawContextSettings) {
    if let Some(v) = raw.limit {
        result.limit = v;
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
    if let Some(ref patterns) = raw.patterns {
        // Merge patterns - raw values overwrite existing for same key
        for (key, value) in patterns {
            result.patterns.insert(key.clone(), value.clone());
        }
    }
}

/// Merges trees from all configs, resolving paths.
///
/// Trees are merged by name - first definition wins.
/// `is_global` is determined by whether the source config file is `~/.ra.toml`.
fn merge_trees(configs: &[ParsedConfig]) -> Result<Vec<Tree>, ConfigError> {
    let mut seen: HashMap<String, Tree> = HashMap::new();

    // Iterate in precedence order (highest first) - first definition wins
    for parsed in configs {
        let Some(ref trees) = parsed.config.trees else {
            continue;
        };

        let config_dir = parsed.path.parent().unwrap();
        let is_global = is_global_config(&parsed.path);

        for (name, path_str) in trees {
            if seen.contains_key(name) {
                // Already defined by higher-precedence config
                continue;
            }

            let resolved_path = resolve_tree_path(path_str, config_dir)?;

            seen.insert(
                name.clone(),
                Tree {
                    name: name.clone(),
                    path: resolved_path,
                    is_global,
                },
            );
        }
    }

    // Return trees in a deterministic order (by name)
    let mut trees: Vec<Tree> = seen.into_values().collect();
    trees.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(trees)
}

/// Merges include patterns from all configs.
///
/// Patterns are concatenated in precedence order (highest first).
fn merge_includes(configs: &[ParsedConfig]) -> Vec<IncludePattern> {
    let mut result = Vec::new();

    for parsed in configs {
        if let Some(ref includes) = parsed.config.include {
            for raw in includes {
                result.push(convert_include_pattern(raw));
            }
        }
    }

    result
}

/// Converts a raw include pattern to the final type.
fn convert_include_pattern(raw: &RawIncludePattern) -> IncludePattern {
    IncludePattern {
        tree: raw.tree.clone(),
        pattern: raw.pattern.clone(),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
    };

    use super::*;

    struct TestDir {
        root: tempfile::TempDir,
    }

    impl TestDir {
        fn new() -> Self {
            Self {
                root: tempfile::tempdir().unwrap(),
            }
        }

        fn path(&self) -> &Path {
            self.root.path()
        }

        fn create_dir(&self, rel_path: &str) -> PathBuf {
            let path = self.root.path().join(rel_path);
            fs::create_dir_all(&path).unwrap();
            path
        }

        fn create_config(&self, rel_path: &str, content: &str) -> PathBuf {
            let path = self.root.path().join(rel_path);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(&path, content).unwrap();
            path
        }
    }

    fn parse_test_config(path: PathBuf, toml: &str) -> ParsedConfig {
        ParsedConfig {
            path,
            config: crate::parse_config_str(toml, Path::new("test")).unwrap(),
        }
    }

    #[test]
    fn test_merge_empty_configs() {
        let result = merge_configs(&[]).unwrap();
        assert_eq!(result.settings.default_limit, 5); // default
        assert!(result.trees.is_empty());
        assert!(result.includes.is_empty());
    }

    #[test]
    fn test_merge_single_config() {
        let test_dir = TestDir::new();
        test_dir.create_dir("docs");
        let config_path = test_dir.create_config(
            ".ra.toml",
            r#"
[settings]
default_limit = 10

[trees]
local = "./docs"
"#,
        );

        let _parsed = parse_test_config(
            config_path,
            r#"
[settings]
default_limit = 10

[trees]
local = "./docs"
"#,
        );

        // Re-parse with actual path for tree resolution
        let parsed = ParsedConfig {
            path: test_dir.path().join(".ra.toml"),
            config: crate::parse_config_str(
                r#"
[settings]
default_limit = 10

[trees]
local = "./docs"
"#,
                Path::new("test"),
            )
            .unwrap(),
        };

        let result = merge_configs(&[parsed]).unwrap();
        assert_eq!(result.settings.default_limit, 10);
        assert_eq!(result.trees.len(), 1);
        assert_eq!(result.trees[0].name, "local");
    }

    #[test]
    fn test_merge_scalar_override() {
        let test_dir = TestDir::new();

        // Higher precedence config (closer to CWD)
        let high_prec = ParsedConfig {
            path: test_dir.path().join("project/.ra.toml"),
            config: crate::parse_config_str(
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
            config: crate::parse_config_str(
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
            config: crate::parse_config_str(
                r#"
[trees]
docs = "./docs"
"#,
                Path::new("test"),
            )
            .unwrap(),
        };

        let low_prec = ParsedConfig {
            path: test_dir.path().join(".ra.toml"),
            config: crate::parse_config_str(
                r#"
[trees]
docs = "./docs"
"#,
                Path::new("test"),
            )
            .unwrap(),
        };

        let result = merge_configs(&[high_prec, low_prec]).unwrap();

        assert_eq!(result.trees.len(), 1);
        // Should resolve to project/docs, not root/docs
        assert_eq!(result.trees[0].path, docs1.canonicalize().unwrap());
    }

    #[test]
    fn test_merge_trees_different_names() {
        let test_dir = TestDir::new();
        test_dir.create_dir("project/local");
        test_dir.create_dir("global");

        let high_prec = ParsedConfig {
            path: test_dir.path().join("project/.ra.toml"),
            config: crate::parse_config_str(
                r#"
[trees]
local = "./local"
"#,
                Path::new("test"),
            )
            .unwrap(),
        };

        let low_prec = ParsedConfig {
            path: test_dir.path().join(".ra.toml"),
            config: crate::parse_config_str(
                r#"
[trees]
global = "./global"
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
    fn test_merge_includes_concatenated() {
        let test_dir = TestDir::new();

        let high_prec = ParsedConfig {
            path: test_dir.path().join("project/.ra.toml"),
            config: crate::parse_config_str(
                r#"
[[include]]
tree = "local"
pattern = "**/*.md"
"#,
                Path::new("test"),
            )
            .unwrap(),
        };

        let low_prec = ParsedConfig {
            path: test_dir.path().join(".ra.toml"),
            config: crate::parse_config_str(
                r#"
[[include]]
tree = "global"
pattern = "**/rust/**"

[[include]]
tree = "global"
pattern = "**/git/**"
"#,
                Path::new("test"),
            )
            .unwrap(),
        };

        let result = merge_configs(&[high_prec, low_prec]).unwrap();

        assert_eq!(result.includes.len(), 3);
        // Order: high precedence first
        assert_eq!(result.includes[0].tree, "local");
        assert_eq!(result.includes[1].tree, "global");
        assert_eq!(result.includes[1].pattern, "**/rust/**");
        assert_eq!(result.includes[2].tree, "global");
        assert_eq!(result.includes[2].pattern, "**/git/**");
    }

    #[test]
    fn test_merge_context_patterns() {
        let test_dir = TestDir::new();

        let high_prec = ParsedConfig {
            path: test_dir.path().join("project/.ra.toml"),
            config: crate::parse_config_str(
                r#"
[context.patterns]
"*.rs" = ["rust", "systems"]
"src/api/**" = ["http"]
"#,
                Path::new("test"),
            )
            .unwrap(),
        };

        let low_prec = ParsedConfig {
            path: test_dir.path().join(".ra.toml"),
            config: crate::parse_config_str(
                r#"
[context.patterns]
"*.rs" = ["rust"]
"*.py" = ["python"]
"#,
                Path::new("test"),
            )
            .unwrap(),
        };

        let result = merge_configs(&[high_prec, low_prec]).unwrap();

        // High precedence wins for *.rs
        assert_eq!(
            result.context.patterns.get("*.rs"),
            Some(&vec!["rust".to_string(), "systems".to_string()])
        );
        // Low precedence provides *.py
        assert_eq!(
            result.context.patterns.get("*.py"),
            Some(&vec!["python".to_string()])
        );
        // High precedence provides src/api/**
        assert_eq!(
            result.context.patterns.get("src/api/**"),
            Some(&vec!["http".to_string()])
        );
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
            config: crate::parse_config_str(
                r#"
[settings]
default_limit = 3

[trees]
local = "./local"
"#,
                Path::new("test"),
            )
            .unwrap(),
        };

        // Middle precedence
        let mid = ParsedConfig {
            path: test_dir.path().join("project/.ra.toml"),
            config: crate::parse_config_str(
                r#"
[settings]
default_limit = 5
local_boost = 2.0

[trees]
shared = "./shared"

[[include]]
tree = "shared"
pattern = "**/*.md"
"#,
                Path::new("test"),
            )
            .unwrap(),
        };

        // Lowest precedence
        let root = ParsedConfig {
            path: test_dir.path().join(".ra.toml"),
            config: crate::parse_config_str(
                r#"
[settings]
default_limit = 10
local_boost = 1.5
chunk_at_headings = false

[trees]
global = "./global"

[[include]]
tree = "global"
pattern = "**/*"
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

        // Includes: concatenated in order
        assert_eq!(result.includes.len(), 2);
        assert_eq!(result.includes[0].tree, "shared");
        assert_eq!(result.includes[1].tree, "global");

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
            config: crate::parse_config_str(
                r#"
[search]
fuzzy = false
"#,
                Path::new("test"),
            )
            .unwrap(),
        };

        let low_prec = ParsedConfig {
            path: test_dir.path().join(".ra.toml"),
            config: crate::parse_config_str(
                r#"
[search]
fuzzy = true
fuzzy_distance = 2
stemmer = "german"
"#,
                Path::new("test"),
            )
            .unwrap(),
        };

        let result = merge_configs(&[high_prec, low_prec]).unwrap();

        assert!(!result.search.fuzzy); // high prec wins
        assert_eq!(result.search.fuzzy_distance, 2); // low prec provides
        assert_eq!(result.search.stemmer, "german"); // low prec provides
    }
}
