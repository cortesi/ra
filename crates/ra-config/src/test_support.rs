//! Test helpers shared across ra-config unit tests.
//!
//! Kept behind `cfg(test)` to avoid leaking into the public API surface.

use std::{
    fs,
    path::{Path, PathBuf},
};

use tempfile::TempDir;

use crate::discovery::CONFIG_FILENAME;

/// Temporary directory utility for tests.
pub struct TestDir {
    root: TempDir,
}

impl TestDir {
    /// Creates a new temporary directory tree.
    pub fn new() -> Self {
        Self {
            root: tempfile::tempdir().unwrap(),
        }
    }

    /// Returns the path to the root.
    pub fn path(&self) -> &Path {
        self.root.path()
    }

    /// Creates a directory relative to the root.
    pub fn create_dir(&self, rel_path: &str) -> PathBuf {
        let path = self.root.path().join(rel_path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    /// Creates a file with default contents (`"test"`) relative to the root.
    pub fn create_file(&self, rel_path: &str) -> PathBuf {
        let path = self.root.path().join(rel_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, "test").unwrap();
        path
    }

    /// Writes a `.ra.toml` with default content in the given subdirectory.
    pub fn create_config(&self, rel_path: &str) -> PathBuf {
        self.create_config_with_content(rel_path, "# test config\n")
    }

    /// Writes a `.ra.toml` at the root.
    pub fn create_config_at_root(&self) -> PathBuf {
        let config = self.root.path().join(CONFIG_FILENAME);
        fs::write(&config, "# root config\n").unwrap();
        config
    }

    /// Writes a `.ra.toml` with custom contents in the given subdirectory.
    pub fn create_config_with_content(&self, rel_path: &str, content: &str) -> PathBuf {
        let dir = self.root.path().join(rel_path);
        fs::create_dir_all(&dir).unwrap();
        let config = dir.join(CONFIG_FILENAME);
        fs::write(&config, content).unwrap();
        config
    }

    /// Writes a `root = true` config in the given subdirectory.
    pub fn create_root_config(&self, rel_path: &str) -> PathBuf {
        self.create_config_with_content(rel_path, "root = true\n")
    }
}
