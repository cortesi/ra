//! Configuration file discovery.
//!
//! Discovers `.ra.toml` files by walking up the directory tree from a starting point,
//! then appending the global `~/.ra.toml` if present.

use std::path::{Path, PathBuf};

use directories::BaseDirs;

use crate::parse::is_root_config;

/// The configuration filename.
pub const CONFIG_FILENAME: &str = ".ra.toml";

/// Discovers all configuration files relevant to the given directory.
///
/// Returns paths in precedence order: closest to `cwd` first, global (`~/.ra.toml`) last.
/// Files closer to `cwd` have higher precedence during merging.
///
/// The function:
/// 1. Walks up from `cwd` to the filesystem root, collecting any `.ra.toml` files found
/// 2. Stops if a config file has `root = true` set
/// 3. Appends `~/.ra.toml` if it exists and no root config was found (lowest precedence)
///
/// Returns an empty vector if no configuration files are found.
pub fn discover_config_files(cwd: &Path) -> Vec<PathBuf> {
    let mut configs = Vec::new();
    let mut found_root = false;

    // Walk up from cwd, collecting .ra.toml files
    let mut current = Some(cwd);
    while let Some(dir) = current {
        let config_path = dir.join(CONFIG_FILENAME);
        if config_path.is_file() {
            // Check if this is a root config before adding
            let is_root = is_root_config(&config_path);
            configs.push(config_path);
            if is_root {
                found_root = true;
                break;
            }
        }
        current = dir.parent();
    }

    // Append global config if it exists, no root was found, and it isn't already included
    if !found_root
        && let Some(global_path) = global_config_path()
        && global_path.is_file()
        && !configs.contains(&global_path)
    {
        configs.push(global_path);
    }

    configs
}

/// Returns the path to the global configuration file (`~/.ra.toml`).
///
/// Returns `None` if the home directory cannot be determined.
pub fn global_config_path() -> Option<PathBuf> {
    BaseDirs::new().map(|dirs| dirs.home_dir().join(CONFIG_FILENAME))
}

/// Checks if a path is the global configuration file.
pub fn is_global_config(path: &Path) -> bool {
    global_config_path().is_some_and(|global| path == global)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    /// Creates a temporary directory structure for testing.
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

        fn create_config(&self, rel_path: &str) -> PathBuf {
            let dir = self.root.path().join(rel_path);
            fs::create_dir_all(&dir).unwrap();
            let config = dir.join(CONFIG_FILENAME);
            fs::write(&config, "# test config\n").unwrap();
            config
        }

        fn create_config_at_root(&self) -> PathBuf {
            let config = self.root.path().join(CONFIG_FILENAME);
            fs::write(&config, "# root config\n").unwrap();
            config
        }

        fn create_config_with_content(&self, rel_path: &str, content: &str) -> PathBuf {
            let dir = self.root.path().join(rel_path);
            fs::create_dir_all(&dir).unwrap();
            let config = dir.join(CONFIG_FILENAME);
            fs::write(&config, content).unwrap();
            config
        }

        fn create_root_config(&self, rel_path: &str) -> PathBuf {
            self.create_config_with_content(rel_path, "root = true\n")
        }
    }

    #[test]
    fn test_discover_no_configs() {
        let test_dir = TestDir::new();
        let subdir = test_dir.create_dir("a/b/c");

        let configs = discover_config_files(&subdir);

        // Should only contain global config if it exists
        for config in &configs {
            assert!(is_global_config(config), "unexpected config: {config:?}");
        }
    }

    #[test]
    fn test_discover_single_config() {
        let test_dir = TestDir::new();
        let config = test_dir.create_config_at_root();
        let subdir = test_dir.create_dir("a/b/c");

        let configs = discover_config_files(&subdir);

        // Filter out global config for comparison
        let local_configs: Vec<_> = configs.iter().filter(|p| !is_global_config(p)).collect();

        assert_eq!(local_configs.len(), 1);
        assert_eq!(local_configs[0], &config);
    }

    #[test]
    fn test_discover_multiple_configs_precedence_order() {
        let test_dir = TestDir::new();
        let root_config = test_dir.create_config_at_root();
        let mid_config = test_dir.create_config("a/b");
        let leaf_config = test_dir.create_config("a/b/c/d");
        let working_dir = test_dir.create_dir("a/b/c/d/e");

        let configs = discover_config_files(&working_dir);

        // Filter out global config for comparison
        let local_configs: Vec<_> = configs.iter().filter(|p| !is_global_config(p)).collect();

        // Should be in order: closest to cwd first
        assert_eq!(local_configs.len(), 3);
        assert_eq!(local_configs[0], &leaf_config);
        assert_eq!(local_configs[1], &mid_config);
        assert_eq!(local_configs[2], &root_config);
    }

    #[test]
    fn test_discover_from_directory_with_config() {
        let test_dir = TestDir::new();
        let config = test_dir.create_config_at_root();

        let configs = discover_config_files(test_dir.path());

        let local_configs: Vec<_> = configs.iter().filter(|p| !is_global_config(p)).collect();

        assert_eq!(local_configs.len(), 1);
        assert_eq!(local_configs[0], &config);
    }

    #[test]
    fn test_global_config_path_returns_some() {
        // This test verifies the function returns a path (we can't easily test the actual value
        // since it depends on the system's home directory)
        let path = global_config_path();
        assert!(path.is_some());
        assert!(path.unwrap().ends_with(CONFIG_FILENAME));
    }

    #[test]
    fn test_is_global_config() {
        let global = global_config_path().unwrap();
        assert!(is_global_config(&global));

        let not_global = PathBuf::from("/some/other/path/.ra.toml");
        assert!(!is_global_config(&not_global));
    }

    #[test]
    fn test_discover_skips_non_file_config() {
        let test_dir = TestDir::new();
        // Create a directory named .ra.toml instead of a file
        let fake_config = test_dir.path().join(CONFIG_FILENAME);
        fs::create_dir_all(&fake_config).unwrap();

        let subdir = test_dir.create_dir("subdir");

        let configs = discover_config_files(&subdir);

        // Should not include the directory
        let local_configs: Vec<_> = configs.iter().filter(|p| !is_global_config(p)).collect();
        assert!(local_configs.is_empty());
    }

    #[test]
    fn test_root_config_stops_discovery() {
        let test_dir = TestDir::new();
        // Parent config that should be ignored
        let _parent_config = test_dir.create_config_at_root();
        // Root config in middle - should stop here
        let root_config = test_dir.create_root_config("project");
        let working_dir = test_dir.create_dir("project/src");

        let configs = discover_config_files(&working_dir);

        // Should only include the root config, not parent or global
        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0], root_config);
    }

    #[test]
    fn test_root_config_at_cwd() {
        let test_dir = TestDir::new();
        let _parent_config = test_dir.create_config_at_root();
        let root_config = test_dir.create_root_config("project");

        let configs = discover_config_files(&test_dir.path().join("project"));

        // Should only include the root config
        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0], root_config);
    }

    #[test]
    fn test_root_config_includes_child_configs() {
        let test_dir = TestDir::new();
        let _parent_config = test_dir.create_config_at_root();
        let root_config = test_dir.create_root_config("project");
        let child_config = test_dir.create_config("project/sub");
        let working_dir = test_dir.create_dir("project/sub/deep");

        let configs = discover_config_files(&working_dir);

        // Should include child and root, but not parent or global
        assert_eq!(configs.len(), 2);
        assert_eq!(configs[0], child_config);
        assert_eq!(configs[1], root_config);
    }

    #[test]
    fn test_root_false_does_not_stop_discovery() {
        let test_dir = TestDir::new();
        let parent_config = test_dir.create_config_at_root();
        // Explicit root = false should not stop discovery
        let mid_config = test_dir.create_config_with_content("project", "root = false\n");
        let working_dir = test_dir.create_dir("project/src");

        let configs = discover_config_files(&working_dir);

        let local_configs: Vec<_> = configs.iter().filter(|p| !is_global_config(p)).collect();

        // Should include both configs
        assert_eq!(local_configs.len(), 2);
        assert_eq!(local_configs[0], &mid_config);
        assert_eq!(local_configs[1], &parent_config);
    }
}
