//! Include pattern compilation and matching.
//!
//! Compiles glob patterns from configuration into efficient matchers
//! for determining which files to index from each tree.

use std::{collections::HashMap, path::Path};

use globset::{Glob, GlobSet, GlobSetBuilder};

use crate::{ConfigError, IncludePattern};

/// Default patterns applied when no explicit patterns are specified for a tree.
const DEFAULT_PATTERNS: &[&str] = &["**/*.md", "**/*.txt"];

/// Compiled glob patterns for efficient file matching.
///
/// Patterns are organized per-tree, allowing quick lookup of whether
/// a file path should be included for indexing in a given tree.
#[derive(Debug)]
pub struct CompiledPatterns {
    /// Compiled patterns per tree name.
    tree_patterns: HashMap<String, GlobSet>,
    /// Trees that have explicit patterns defined.
    trees_with_patterns: Vec<String>,
}

impl CompiledPatterns {
    /// Compiles include patterns into an efficient matcher.
    ///
    /// If a tree has no patterns defined, default patterns (`**/*.md`, `**/*.txt`)
    /// will be used for that tree.
    ///
    /// The `all_trees` parameter lists all tree names that should have patterns.
    /// Trees not mentioned in `includes` will get default patterns.
    pub fn compile(includes: &[IncludePattern], all_trees: &[String]) -> Result<Self, ConfigError> {
        let mut tree_patterns: HashMap<String, GlobSetBuilder> = HashMap::new();
        let mut trees_with_patterns: Vec<String> = Vec::new();

        // Group patterns by tree
        for include in includes {
            trees_with_patterns.push(include.tree.clone());
            tree_patterns
                .entry(include.tree.clone())
                .or_insert_with(GlobSetBuilder::new)
                .add(compile_glob(&include.pattern)?);
        }

        // Add default patterns for trees without explicit patterns
        for tree_name in all_trees {
            if !tree_patterns.contains_key(tree_name) {
                let mut builder = GlobSetBuilder::new();
                for pattern in DEFAULT_PATTERNS {
                    builder.add(compile_glob(pattern)?);
                }
                tree_patterns.insert(tree_name.clone(), builder);
            }
        }

        // Build all GlobSets
        let tree_patterns = tree_patterns
            .into_iter()
            .map(|(name, builder)| {
                let globset = builder.build().map_err(|e| ConfigError::InvalidPattern {
                    pattern: format!("<combined patterns for {name}>"),
                    source: e,
                })?;
                Ok((name, globset))
            })
            .collect::<Result<HashMap<_, _>, ConfigError>>()?;

        Ok(Self {
            tree_patterns,
            trees_with_patterns,
        })
    }

    /// Checks if a path matches the patterns for a given tree.
    ///
    /// The path should be relative to the tree root.
    /// Returns `false` if the tree has no patterns defined.
    pub fn matches(&self, tree: &str, path: &Path) -> bool {
        self.tree_patterns
            .get(tree)
            .is_some_and(|patterns| patterns.is_match(path))
    }

    /// Returns the names of all trees that have patterns (explicit or default).
    pub fn trees(&self) -> impl Iterator<Item = &str> {
        self.tree_patterns.keys().map(String::as_str)
    }

    /// Returns whether a tree has explicit patterns defined (vs default patterns).
    pub fn has_explicit_patterns(&self, tree: &str) -> bool {
        self.trees_with_patterns.iter().any(|t| t == tree)
    }
}

/// Compiles a single glob pattern.
fn compile_glob(pattern: &str) -> Result<Glob, ConfigError> {
    Glob::new(pattern).map_err(|e| ConfigError::InvalidPattern {
        pattern: pattern.to_string(),
        source: e,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compile_empty_patterns() {
        let patterns = CompiledPatterns::compile(&[], &[]).unwrap();
        assert_eq!(patterns.trees().count(), 0);
    }

    #[test]
    fn test_compile_single_pattern() {
        let includes = vec![IncludePattern {
            tree: "docs".into(),
            pattern: "**/*.md".into(),
        }];

        let patterns = CompiledPatterns::compile(&includes, &["docs".into()]).unwrap();

        assert!(patterns.matches("docs", Path::new("readme.md")));
        assert!(patterns.matches("docs", Path::new("guide/intro.md")));
        assert!(!patterns.matches("docs", Path::new("code.rs")));
    }

    #[test]
    fn test_compile_multiple_patterns_same_tree() {
        let includes = vec![
            IncludePattern {
                tree: "docs".into(),
                pattern: "**/*.md".into(),
            },
            IncludePattern {
                tree: "docs".into(),
                pattern: "**/*.txt".into(),
            },
        ];

        let patterns = CompiledPatterns::compile(&includes, &["docs".into()]).unwrap();

        assert!(patterns.matches("docs", Path::new("readme.md")));
        assert!(patterns.matches("docs", Path::new("notes.txt")));
        assert!(!patterns.matches("docs", Path::new("code.rs")));
    }

    #[test]
    fn test_compile_patterns_multiple_trees() {
        let includes = vec![
            IncludePattern {
                tree: "global".into(),
                pattern: "**/rust/**".into(),
            },
            IncludePattern {
                tree: "local".into(),
                pattern: "**/*.md".into(),
            },
        ];

        let patterns =
            CompiledPatterns::compile(&includes, &["global".into(), "local".into()]).unwrap();

        assert!(patterns.matches("global", Path::new("rust/guide.md")));
        assert!(patterns.matches("global", Path::new("docs/rust/errors.txt")));
        assert!(!patterns.matches("global", Path::new("python/guide.md")));

        assert!(patterns.matches("local", Path::new("readme.md")));
        assert!(!patterns.matches("local", Path::new("readme.txt")));
    }

    #[test]
    fn test_default_patterns_applied() {
        // Tree with no explicit patterns should get defaults
        let patterns = CompiledPatterns::compile(&[], &["docs".into()]).unwrap();

        // Default patterns are **/*.md and **/*.txt
        assert!(patterns.matches("docs", Path::new("readme.md")));
        assert!(patterns.matches("docs", Path::new("notes.txt")));
        assert!(patterns.matches("docs", Path::new("deep/nested/file.md")));
        assert!(!patterns.matches("docs", Path::new("code.rs")));
    }

    #[test]
    fn test_explicit_patterns_override_defaults() {
        // Tree with explicit patterns should NOT get defaults
        let includes = vec![IncludePattern {
            tree: "docs".into(),
            pattern: "**/*.rst".into(),
        }];

        let patterns = CompiledPatterns::compile(&includes, &["docs".into()]).unwrap();

        // Only .rst files should match, not .md or .txt
        assert!(patterns.matches("docs", Path::new("guide.rst")));
        assert!(!patterns.matches("docs", Path::new("readme.md")));
        assert!(!patterns.matches("docs", Path::new("notes.txt")));
    }

    #[test]
    fn test_has_explicit_patterns() {
        let includes = vec![IncludePattern {
            tree: "explicit".into(),
            pattern: "**/*.md".into(),
        }];

        let patterns =
            CompiledPatterns::compile(&includes, &["explicit".into(), "defaulted".into()]).unwrap();

        assert!(patterns.has_explicit_patterns("explicit"));
        assert!(!patterns.has_explicit_patterns("defaulted"));
    }

    #[test]
    fn test_matches_unknown_tree() {
        let patterns = CompiledPatterns::compile(&[], &["docs".into()]).unwrap();
        assert!(!patterns.matches("unknown", Path::new("readme.md")));
    }

    #[test]
    fn test_invalid_pattern_error() {
        let includes = vec![IncludePattern {
            tree: "docs".into(),
            pattern: "[invalid".into(), // Unclosed bracket
        }];

        let result = CompiledPatterns::compile(&includes, &["docs".into()]);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigError::InvalidPattern { .. }
        ));
    }

    #[test]
    fn test_complex_glob_patterns() {
        let includes = vec![
            IncludePattern {
                tree: "docs".into(),
                pattern: "src/**/*.md".into(),
            },
            IncludePattern {
                tree: "docs".into(),
                pattern: "!src/internal/**".into(),
            },
        ];

        let patterns = CompiledPatterns::compile(&includes, &["docs".into()]).unwrap();

        assert!(patterns.matches("docs", Path::new("src/guide.md")));
        assert!(patterns.matches("docs", Path::new("src/api/readme.md")));
        // Note: globset doesn't support negation patterns directly,
        // so this just tests the pattern compiles
    }

    #[test]
    fn test_pattern_with_extensions() {
        let includes = vec![IncludePattern {
            tree: "docs".into(),
            pattern: "**/*.{md,txt,rst}".into(),
        }];

        let patterns = CompiledPatterns::compile(&includes, &["docs".into()]).unwrap();

        assert!(patterns.matches("docs", Path::new("readme.md")));
        assert!(patterns.matches("docs", Path::new("notes.txt")));
        assert!(patterns.matches("docs", Path::new("guide.rst")));
        assert!(!patterns.matches("docs", Path::new("code.rs")));
    }

    #[test]
    fn test_trees_iterator() {
        let includes = vec![
            IncludePattern {
                tree: "alpha".into(),
                pattern: "**/*.md".into(),
            },
            IncludePattern {
                tree: "beta".into(),
                pattern: "**/*.txt".into(),
            },
        ];

        let patterns =
            CompiledPatterns::compile(&includes, &["alpha".into(), "beta".into()]).unwrap();

        let mut trees: Vec<_> = patterns.trees().collect();
        trees.sort();
        assert_eq!(trees, vec!["alpha", "beta"]);
    }
}
