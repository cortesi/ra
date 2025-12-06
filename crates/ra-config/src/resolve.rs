//! Path resolution for tree definitions.
//!
//! Resolves relative and tilde-prefixed paths in tree definitions to absolute paths.

use std::path::{Path, PathBuf};

use directories::BaseDirs;

use crate::ConfigError;

/// Resolves a tree path to an absolute path.
///
/// Handles three cases:
/// - Tilde paths (`~/docs`) - expanded to home directory
/// - Relative paths (`./docs`, `../shared`) - resolved relative to `config_dir`
/// - Absolute paths (`/home/user/docs`) - returned as-is after validation
///
/// The path must exist and be a directory. Returns an error otherwise.
pub fn resolve_tree_path(path: &str, config_dir: &Path) -> Result<PathBuf, ConfigError> {
    let expanded = expand_tilde(path)?;

    let absolute = if expanded.is_absolute() {
        expanded
    } else {
        config_dir.join(&expanded)
    };

    // Canonicalize to resolve symlinks and .. components
    let canonical = absolute
        .canonicalize()
        .map_err(|source| ConfigError::PathResolution {
            path: absolute.clone(),
            source,
        })?;

    // Verify it's a directory
    if !canonical.is_dir() {
        return Err(ConfigError::TreePathNotDirectory { path: canonical });
    }

    Ok(canonical)
}

/// Expands a tilde prefix to the home directory.
///
/// - `~` alone becomes the home directory
/// - `~/foo` becomes home directory joined with `foo`
/// - Paths not starting with `~` are returned unchanged
fn expand_tilde(path: &str) -> Result<PathBuf, ConfigError> {
    if path == "~" {
        return home_dir();
    }

    if let Some(rest) = path.strip_prefix("~/") {
        let home = home_dir()?;
        return Ok(home.join(rest));
    }

    Ok(PathBuf::from(path))
}

/// Returns the home directory.
fn home_dir() -> Result<PathBuf, ConfigError> {
    BaseDirs::new()
        .map(|dirs| dirs.home_dir().to_path_buf())
        .ok_or(ConfigError::NoHomeDirectory)
}

#[cfg(test)]
mod tests {
    use std::{fs, process};

    use super::*;
    use crate::test_support::TestDir;

    #[test]
    fn test_resolve_relative_path() {
        let test_dir = TestDir::new();
        let docs = test_dir.create_dir("docs");
        let config_dir = test_dir.path();

        let resolved = resolve_tree_path("./docs", config_dir).unwrap();
        assert_eq!(resolved, docs.canonicalize().unwrap());
    }

    #[test]
    fn test_resolve_relative_path_without_dot() {
        let test_dir = TestDir::new();
        let docs = test_dir.create_dir("docs");
        let config_dir = test_dir.path();

        let resolved = resolve_tree_path("docs", config_dir).unwrap();
        assert_eq!(resolved, docs.canonicalize().unwrap());
    }

    #[test]
    fn test_resolve_parent_relative_path() {
        let test_dir = TestDir::new();
        let shared = test_dir.create_dir("shared/docs");
        let project = test_dir.create_dir("project");

        let resolved = resolve_tree_path("../shared/docs", &project).unwrap();
        assert_eq!(resolved, shared.canonicalize().unwrap());
    }

    #[test]
    fn test_resolve_absolute_path() {
        let test_dir = TestDir::new();
        let docs = test_dir.create_dir("docs");
        let absolute = docs.canonicalize().unwrap();

        // config_dir shouldn't matter for absolute paths
        let resolved = resolve_tree_path(absolute.to_str().unwrap(), Path::new("/other")).unwrap();
        assert_eq!(resolved, absolute);
    }

    #[test]
    fn test_resolve_tilde_path() {
        // This test creates a directory in the actual home directory temporarily
        // Skip if we can't get home directory
        let Some(base_dirs) = BaseDirs::new() else {
            return;
        };
        let home = base_dirs.home_dir();

        // Use a unique name to avoid conflicts
        let test_name = format!(".ra-test-{}", process::id());
        let test_dir = home.join(&test_name);
        fs::create_dir_all(&test_dir).unwrap();

        let result = resolve_tree_path(&format!("~/{test_name}"), Path::new("/"));
        fs::remove_dir_all(&test_dir).unwrap();

        let resolved = result.unwrap();
        assert_eq!(resolved, test_dir.canonicalize().unwrap_or(test_dir));
    }

    #[test]
    fn test_resolve_nonexistent_path_error() {
        let test_dir = TestDir::new();
        let result = resolve_tree_path("./nonexistent", test_dir.path());
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigError::PathResolution { .. }
        ));
    }

    #[test]
    fn test_resolve_file_not_directory_error() {
        let test_dir = TestDir::new();
        test_dir.create_file("file.txt");

        let result = resolve_tree_path("./file.txt", test_dir.path());
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigError::TreePathNotDirectory { .. }
        ));
    }

    #[test]
    fn test_resolve_follows_symlinks() {
        let test_dir = TestDir::new();
        let actual = test_dir.create_dir("actual");
        let link = test_dir.path().join("link");

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            symlink(&actual, &link).unwrap();
            let resolved = resolve_tree_path("./link", test_dir.path()).unwrap();
            // Canonicalize resolves symlinks, so it should point to actual
            assert_eq!(resolved, actual.canonicalize().unwrap());
        }
    }

    #[test]
    fn test_expand_tilde_alone() {
        let result = expand_tilde("~").unwrap();
        let home = BaseDirs::new().unwrap().home_dir().to_path_buf();
        assert_eq!(result, home);
    }

    #[test]
    fn test_expand_tilde_with_path() {
        let result = expand_tilde("~/docs/notes").unwrap();
        let home = BaseDirs::new().unwrap().home_dir().to_path_buf();
        assert_eq!(result, home.join("docs/notes"));
    }

    #[test]
    fn test_expand_no_tilde() {
        let result = expand_tilde("./docs").unwrap();
        assert_eq!(result, PathBuf::from("./docs"));

        let result = expand_tilde("/absolute/path").unwrap();
        assert_eq!(result, PathBuf::from("/absolute/path"));
    }

    #[test]
    fn test_expand_tilde_not_at_start() {
        // Tilde in the middle should not be expanded
        let result = expand_tilde("foo/~/bar").unwrap();
        assert_eq!(result, PathBuf::from("foo/~/bar"));
    }
}
