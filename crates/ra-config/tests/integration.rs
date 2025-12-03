//! Integration tests for ra-config.
//!
//! Tests the full configuration loading pipeline: discovery -> parse -> resolve -> merge.

// Integration tests live outside cfg(test) by design
#![allow(clippy::tests_outside_test_module)]

use std::{
    fs,
    path::{Path, PathBuf},
};

use ra_config::{Config, ConfigError};

/// Test helper to create a temporary directory structure for tests.
struct TestEnv {
    root: tempfile::TempDir,
}

impl TestEnv {
    fn new() -> Self {
        Self {
            root: tempfile::tempdir().unwrap(),
        }
    }

    fn path(&self) -> &Path {
        self.root.path()
    }

    /// Creates a directory and returns its path.
    fn create_dir(&self, rel_path: &str) -> PathBuf {
        let path = self.root.path().join(rel_path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    /// Creates a file with content and returns its path.
    fn create_file(&self, rel_path: &str, content: &str) -> PathBuf {
        let path = self.root.path().join(rel_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, content).unwrap();
        path
    }
}

#[test]
fn test_load_no_config_returns_default() {
    let env = TestEnv::new();
    let config = Config::load(env.path()).unwrap();

    assert!(config.trees.is_empty());
    assert!(config.config_root.is_none());
    // Check default settings
    assert_eq!(config.settings.default_limit, 5);
    assert_eq!(config.search.stemmer, "english");
}

#[test]
fn test_load_single_config() {
    let env = TestEnv::new();
    let docs_dir = env.create_dir("docs");

    env.create_file(
        ".ra.toml",
        &format!(
            r#"
[tree.docs]
path = "{}"

[settings]
default_limit = 10
"#,
            docs_dir.display()
        ),
    );

    let config = Config::load(env.path()).unwrap();

    assert_eq!(config.trees.len(), 1);
    assert_eq!(config.trees[0].name, "docs");
    assert_eq!(config.trees[0].path, docs_dir.canonicalize().unwrap());
    assert!(!config.trees[0].is_global);
    assert_eq!(config.settings.default_limit, 10);
    assert!(config.config_root.is_some());
}

#[test]
fn test_load_nested_configs_merging() {
    let env = TestEnv::new();

    // Create directory structure: root/project/subdir
    let root_docs = env.create_dir("root-docs");
    let project_docs = env.create_dir("project/docs");
    let subdir = env.create_dir("project/subdir");

    // Root config
    env.create_file(
        ".ra.toml",
        &format!(
            r#"
[tree.root]
path = "{}"

[settings]
default_limit = 5
local_boost = 1.0
"#,
            root_docs.display()
        ),
    );

    // Project config - overrides some settings, adds new tree
    env.create_file(
        "project/.ra.toml",
        &format!(
            r#"
[tree.local]
path = "{}"

[settings]
default_limit = 20
"#,
            project_docs.display()
        ),
    );

    // Load from the deepest directory
    let config = Config::load(&subdir).unwrap();

    // Should have both trees
    assert_eq!(config.trees.len(), 2);

    let tree_names: Vec<_> = config.trees.iter().map(|t| t.name.as_str()).collect();
    assert!(tree_names.contains(&"root"));
    assert!(tree_names.contains(&"local"));

    // default_limit should be from project config (closest)
    assert_eq!(config.settings.default_limit, 20);
    // local_boost should be from root config (not overridden)
    assert!((config.settings.local_boost - 1.0).abs() < f32::EPSILON);
}

#[test]
fn test_load_tree_shadowing() {
    let env = TestEnv::new();

    let parent_docs = env.create_dir("parent-docs");
    let child_docs = env.create_dir("child/docs");
    let child_dir = env.create_dir("child");

    // Parent config defines "docs" tree
    env.create_file(
        ".ra.toml",
        &format!(
            r#"
[tree.docs]
path = "{}"
"#,
            parent_docs.display()
        ),
    );

    // Child config redefines "docs" tree (shadows parent)
    env.create_file(
        "child/.ra.toml",
        &format!(
            r#"
[tree.docs]
path = "{}"
"#,
            child_docs.display()
        ),
    );

    let config = Config::load(&child_dir).unwrap();

    // Only one "docs" tree, pointing to child's definition
    assert_eq!(config.trees.len(), 1);
    assert_eq!(config.trees[0].name, "docs");
    assert_eq!(config.trees[0].path, child_docs.canonicalize().unwrap());
}

#[test]
fn test_load_tree_with_include_patterns() {
    let env = TestEnv::new();
    let docs = env.create_dir("docs");

    env.create_file(
        ".ra.toml",
        &format!(
            r#"
[tree.docs]
path = "{}"
include = ["**/*.md", "**/*.txt"]
"#,
            docs.display()
        ),
    );

    let config = Config::load(env.path()).unwrap();

    assert_eq!(config.trees.len(), 1);
    assert_eq!(config.trees[0].include, vec!["**/*.md", "**/*.txt"]);
    assert!(config.trees[0].exclude.is_empty());
}

#[test]
fn test_load_tree_with_exclude_patterns() {
    let env = TestEnv::new();
    let docs = env.create_dir("docs");

    env.create_file(
        ".ra.toml",
        &format!(
            r#"
[tree.docs]
path = "{}"
include = ["**/*.md"]
exclude = ["**/drafts/**", "**/private/**"]
"#,
            docs.display()
        ),
    );

    let config = Config::load(env.path()).unwrap();

    assert_eq!(
        config.trees[0].exclude,
        vec!["**/drafts/**", "**/private/**"]
    );
}

#[test]
fn test_load_relative_tree_path() {
    let env = TestEnv::new();

    // Create docs directory at root/docs
    env.create_dir("docs");

    // Config uses relative path
    env.create_file(
        ".ra.toml",
        r#"
[tree.docs]
path = "./docs"
"#,
    );

    let config = Config::load(env.path()).unwrap();

    assert_eq!(config.trees.len(), 1);
    assert_eq!(config.trees[0].name, "docs");
    // Path should be resolved to absolute
    assert!(config.trees[0].path.is_absolute());
}

#[test]
fn test_load_error_nonexistent_tree_path() {
    let env = TestEnv::new();

    env.create_file(
        ".ra.toml",
        r#"
[tree.docs]
path = "./nonexistent"
"#,
    );

    let result = Config::load(env.path());
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        ConfigError::PathResolution { .. }
    ));
}

#[test]
fn test_load_error_invalid_toml() {
    let env = TestEnv::new();

    env.create_file(
        ".ra.toml",
        r#"
[tree
invalid toml
"#,
    );

    let result = Config::load(env.path());
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), ConfigError::ParseToml { .. }));
}

#[test]
fn test_load_error_tree_path_is_file() {
    let env = TestEnv::new();

    env.create_file("docs.txt", "this is a file, not a directory");
    env.create_file(
        ".ra.toml",
        r#"
[tree.docs]
path = "./docs.txt"
"#,
    );

    let result = Config::load(env.path());
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        ConfigError::TreePathNotDirectory { .. }
    ));
}

#[test]
fn test_load_with_all_settings() {
    let env = TestEnv::new();
    let docs = env.create_dir("docs");

    env.create_file(
        ".ra.toml",
        &format!(
            r#"
[tree.docs]
path = "{}"

[settings]
default_limit = 15
local_boost = 2.0
chunk_at_headings = false
max_chunk_size = 100000

[search]
stemmer = "german"

[context]
limit = 25
min_term_frequency = 5
min_word_length = 3
max_word_length = 50
sample_size = 100000

[context.patterns]
"*.py" = ["python3", "django"]
"#,
            docs.display()
        ),
    );

    let config = Config::load(env.path()).unwrap();

    // Verify all settings
    assert_eq!(config.settings.default_limit, 15);
    assert!((config.settings.local_boost - 2.0).abs() < f32::EPSILON);
    assert!(!config.settings.chunk_at_headings);
    assert_eq!(config.settings.max_chunk_size, 100000);

    assert_eq!(config.search.stemmer, "german");

    assert_eq!(config.context.limit, 25);
    assert_eq!(config.context.min_term_frequency, 5);
    assert_eq!(config.context.min_word_length, 3);
    assert_eq!(config.context.max_word_length, 50);
    assert_eq!(config.context.sample_size, 100000);
    assert!(config.context.patterns.contains_key("*.py"));
    assert_eq!(
        config.context.patterns.get("*.py"),
        Some(&vec!["python3".to_string(), "django".to_string()])
    );
}

#[test]
fn test_compile_patterns_from_config() {
    let env = TestEnv::new();
    let docs = env.create_dir("docs");

    env.create_file(
        ".ra.toml",
        &format!(
            r#"
[tree.docs]
path = "{}"
include = ["**/*.md", "**/*.rst"]
"#,
            docs.display()
        ),
    );

    let config = Config::load(env.path()).unwrap();
    let patterns = config.compile_patterns().unwrap();

    assert!(patterns.matches("docs", Path::new("readme.md")));
    assert!(patterns.matches("docs", Path::new("guide.rst")));
    assert!(!patterns.matches("docs", Path::new("code.rs")));
}

#[test]
fn test_compile_patterns_with_exclude() {
    let env = TestEnv::new();
    let docs = env.create_dir("docs");

    env.create_file(
        ".ra.toml",
        &format!(
            r#"
[tree.docs]
path = "{}"
include = ["**/*.md"]
exclude = ["**/drafts/**"]
"#,
            docs.display()
        ),
    );

    let config = Config::load(env.path()).unwrap();
    let patterns = config.compile_patterns().unwrap();

    assert!(patterns.matches("docs", Path::new("readme.md")));
    assert!(patterns.matches("docs", Path::new("guide/intro.md")));
    assert!(!patterns.matches("docs", Path::new("drafts/wip.md")));
    assert!(!patterns.matches("docs", Path::new("guide/drafts/new.md")));
}

#[test]
fn test_compile_patterns_default_when_no_includes() {
    let env = TestEnv::new();
    let docs = env.create_dir("docs");

    env.create_file(
        ".ra.toml",
        &format!(
            r#"
[tree.docs]
path = "{}"
"#,
            docs.display()
        ),
    );

    let config = Config::load(env.path()).unwrap();
    let patterns = config.compile_patterns().unwrap();

    // Default patterns should apply: **/*.md, **/*.txt
    assert!(patterns.matches("docs", Path::new("readme.md")));
    assert!(patterns.matches("docs", Path::new("notes.txt")));
    assert!(!patterns.matches("docs", Path::new("code.rs")));
}

#[test]
fn test_load_from_files_empty_list() {
    let config = Config::load_from_files(&[]).unwrap();
    assert!(config.trees.is_empty());
    assert!(config.config_root.is_none());
}

#[test]
fn test_load_from_files_single_file() {
    let env = TestEnv::new();
    let docs = env.create_dir("docs");

    let config_path = env.create_file(
        ".ra.toml",
        &format!(
            r#"
[tree.docs]
path = "{}"

[settings]
default_limit = 42
"#,
            docs.display()
        ),
    );

    let config = Config::load_from_files(&[config_path]).unwrap();

    assert_eq!(config.trees.len(), 1);
    assert_eq!(config.settings.default_limit, 42);
}

#[test]
fn test_load_from_files_precedence() {
    let env = TestEnv::new();
    let docs = env.create_dir("docs");

    // First file (higher precedence)
    let high_prec = env.create_file(
        "high/.ra.toml",
        r#"
[settings]
default_limit = 100
"#,
    );

    // Second file (lower precedence)
    let low_prec = env.create_file(
        "low/.ra.toml",
        &format!(
            r#"
[tree.docs]
path = "{}"

[settings]
default_limit = 1
local_boost = 3.0
"#,
            docs.display()
        ),
    );

    // Pass files in precedence order (high first)
    let config = Config::load_from_files(&[high_prec, low_prec]).unwrap();

    // default_limit should be from high-prec config
    assert_eq!(config.settings.default_limit, 100);
    // local_boost should be from low-prec config (not in high-prec)
    assert!((config.settings.local_boost - 3.0).abs() < f32::EPSILON);
    // Tree should be from low-prec config
    assert_eq!(config.trees.len(), 1);
}

#[test]
fn test_context_patterns_merge() {
    let env = TestEnv::new();
    let docs = env.create_dir("docs");
    let project = env.create_dir("project");

    // Parent config with context patterns
    env.create_file(
        ".ra.toml",
        &format!(
            r#"
[tree.docs]
path = "{}"

[context.patterns]
"*.rs" = ["rust", "cargo"]
"*.py" = ["python"]
"#,
            docs.display()
        ),
    );

    // Child config overrides one pattern, adds another
    env.create_file(
        "project/.ra.toml",
        r#"
[context.patterns]
"*.rs" = ["rust-lang"]
"*.go" = ["golang"]
"#,
    );

    let config = Config::load(&project).unwrap();

    // *.rs should be overridden by child
    assert_eq!(
        config.context.patterns.get("*.rs"),
        Some(&vec!["rust-lang".to_string()])
    );
    // *.py should be from parent
    assert_eq!(
        config.context.patterns.get("*.py"),
        Some(&vec!["python".to_string()])
    );
    // *.go should be from child
    assert_eq!(
        config.context.patterns.get("*.go"),
        Some(&vec!["golang".to_string()])
    );
}
