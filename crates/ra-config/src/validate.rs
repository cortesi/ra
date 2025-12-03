//! Configuration validation.
//!
//! Validates a loaded configuration and reports warnings for potential issues.

use std::{fmt, fs, path::Path};

use globset::Glob;

use crate::{Config, Tree};

/// A non-fatal warning about the configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigWarning {
    /// A tree path does not exist.
    TreePathMissing {
        /// Name of the tree.
        tree: String,
        /// Path that doesn't exist.
        path: String,
    },
    /// A tree path exists but is not a directory.
    TreePathNotDirectory {
        /// Name of the tree.
        tree: String,
        /// Path that is not a directory.
        path: String,
    },
    /// An include pattern doesn't match any files.
    IncludePatternMatchesNothing {
        /// Name of the tree.
        tree: String,
        /// Pattern that matched nothing.
        pattern: String,
    },
    /// No trees are defined.
    NoTreesDefined,
}

impl fmt::Display for ConfigWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TreePathMissing { tree, path } => {
                write!(f, "tree '{tree}' path does not exist: {path}")
            }
            Self::TreePathNotDirectory { tree, path } => {
                write!(f, "tree '{tree}' path is not a directory: {path}")
            }
            Self::IncludePatternMatchesNothing { tree, pattern } => {
                write!(
                    f,
                    "include pattern '{pattern}' for tree '{tree}' matches no files"
                )
            }
            Self::NoTreesDefined => {
                write!(f, "no trees are defined in configuration")
            }
        }
    }
}

/// Validates the configuration and returns any warnings.
///
/// This checks for:
/// - Tree paths that don't exist or aren't directories
/// - Include patterns that don't match any files
/// - Empty configuration (no trees defined)
pub fn validate_config(config: &Config) -> Vec<ConfigWarning> {
    let mut warnings = Vec::new();

    // Check for empty configuration
    if config.trees.is_empty() {
        warnings.push(ConfigWarning::NoTreesDefined);
        return warnings;
    }

    // Validate each tree
    for tree in &config.trees {
        warnings.extend(validate_tree(tree));
    }

    warnings
}

/// Validates a single tree and its include patterns.
fn validate_tree(tree: &Tree) -> Vec<ConfigWarning> {
    let mut warnings = Vec::new();

    // Check tree path exists and is a directory
    if !tree.path.exists() {
        warnings.push(ConfigWarning::TreePathMissing {
            tree: tree.name.clone(),
            path: tree.path.display().to_string(),
        });
        return warnings; // Can't validate patterns if path doesn't exist
    }

    if !tree.path.is_dir() {
        warnings.push(ConfigWarning::TreePathNotDirectory {
            tree: tree.name.clone(),
            path: tree.path.display().to_string(),
        });
        return warnings; // Can't validate patterns if path isn't a directory
    }

    // Check include patterns for this tree
    for pattern in &tree.include {
        if !pattern_matches_any_file(&tree.path, pattern) {
            warnings.push(ConfigWarning::IncludePatternMatchesNothing {
                tree: tree.name.clone(),
                pattern: pattern.clone(),
            });
        }
    }

    warnings
}

/// Checks if a glob pattern matches any files in a directory.
fn pattern_matches_any_file(tree_path: &Path, pattern: &str) -> bool {
    let Ok(glob) = Glob::new(pattern) else {
        return false; // Invalid pattern, will be caught elsewhere
    };
    let matcher = glob.compile_matcher();

    // Walk the tree and check if any file matches
    walk_and_match(tree_path, tree_path, &matcher)
}

/// Recursively walks a directory and checks if any file matches the pattern.
fn walk_and_match(root: &Path, current: &Path, matcher: &globset::GlobMatcher) -> bool {
    let Ok(entries) = fs::read_dir(current) else {
        return false;
    };

    for entry in entries.flatten() {
        let path = entry.path();

        // Get path relative to root for matching
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };

        if path.is_file() && matcher.is_match(relative) {
            return true;
        }

        if path.is_dir() && walk_and_match(root, &path, matcher) {
            return true;
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use super::*;

    fn make_tree(name: &str, path: &str, include: Vec<&str>, exclude: Vec<&str>) -> Tree {
        Tree {
            name: name.into(),
            path: PathBuf::from(path),
            is_global: false,
            include: include.into_iter().map(String::from).collect(),
            exclude: exclude.into_iter().map(String::from).collect(),
        }
    }

    #[test]
    fn test_validate_empty_config() {
        let config = Config::default();
        let warnings = config.validate();
        assert_eq!(warnings.len(), 1);
        assert!(matches!(warnings[0], ConfigWarning::NoTreesDefined));
    }

    #[test]
    fn test_validate_tree_path_missing() {
        let config = Config {
            trees: vec![make_tree(
                "docs",
                "/nonexistent/path/12345",
                vec!["**/*.md"],
                vec![],
            )],
            ..Default::default()
        };

        let warnings = config.validate();
        assert!(
            warnings.iter().any(
                |w| matches!(w, ConfigWarning::TreePathMissing { tree, .. } if tree == "docs")
            )
        );
    }

    #[test]
    fn test_validate_pattern_matches_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        // Create a file that won't match the pattern
        fs::write(tmp.path().join("file.txt"), "test").unwrap();

        let config = Config {
            trees: vec![make_tree(
                "docs",
                tmp.path().to_str().unwrap(),
                vec!["**/*.rs"], // No .rs files exist
                vec![],
            )],
            ..Default::default()
        };

        let warnings = config.validate();
        assert!(warnings.iter().any(
            |w| matches!(w, ConfigWarning::IncludePatternMatchesNothing { tree, pattern } if tree == "docs" && pattern == "**/*.rs")
        ));
    }

    #[test]
    fn test_validate_pattern_matches_file() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("readme.md"), "# Hello").unwrap();

        let config = Config {
            trees: vec![make_tree(
                "docs",
                tmp.path().to_str().unwrap(),
                vec!["**/*.md"],
                vec![],
            )],
            ..Default::default()
        };

        let warnings = config.validate();
        // Should not have IncludePatternMatchesNothing warning
        assert!(
            !warnings
                .iter()
                .any(|w| matches!(w, ConfigWarning::IncludePatternMatchesNothing { .. }))
        );
    }

    #[test]
    fn test_validate_pattern_matches_nested_file() {
        let tmp = tempfile::tempdir().unwrap();
        let nested = tmp.path().join("subdir");
        fs::create_dir(&nested).unwrap();
        fs::write(nested.join("doc.md"), "# Nested").unwrap();

        let config = Config {
            trees: vec![make_tree(
                "docs",
                tmp.path().to_str().unwrap(),
                vec!["**/*.md"],
                vec![],
            )],
            ..Default::default()
        };

        let warnings = config.validate();
        assert!(
            !warnings
                .iter()
                .any(|w| matches!(w, ConfigWarning::IncludePatternMatchesNothing { .. }))
        );
    }

    #[test]
    fn test_warning_display() {
        let warning = ConfigWarning::TreePathMissing {
            tree: "docs".into(),
            path: "/some/path".into(),
        };
        assert_eq!(
            warning.to_string(),
            "tree 'docs' path does not exist: /some/path"
        );

        let warning = ConfigWarning::NoTreesDefined;
        assert_eq!(warning.to_string(), "no trees are defined in configuration");
    }
}
