//! Index status detection.
//!
//! Determines the current state of the index relative to configuration
//! and provides functions for reading/writing the stored config hash.

use std::{fs, io, path::Path};

use ra_config::Config;

use crate::{
    config_hash::compute_config_hash,
    location::{config_hash_path, index_directory},
};

/// Status of the search index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexStatus {
    /// Index exists and matches current configuration.
    Current,
    /// Index exists but configuration has changed (needs full rebuild).
    ConfigChanged,
    /// Index exists but files have changed (needs incremental update).
    Stale,
    /// No index exists.
    Missing,
}

impl IndexStatus {
    /// Returns a human-readable description for display.
    pub fn description(&self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::ConfigChanged => "stale (config changed)",
            Self::Stale => "stale",
            Self::Missing => "missing",
        }
    }

    /// Returns true if the index needs any kind of update.
    pub fn needs_update(&self) -> bool {
        !matches!(self, Self::Current)
    }

    /// Returns true if the index needs a full rebuild (not just incremental).
    pub fn needs_rebuild(&self) -> bool {
        matches!(self, Self::ConfigChanged | Self::Missing)
    }
}

/// Reads the stored config hash from an index directory.
///
/// Returns `None` if the hash file doesn't exist or can't be read.
pub fn read_stored_hash(index_dir: &Path) -> Option<String> {
    let hash_path = config_hash_path(index_dir);
    fs::read_to_string(&hash_path)
        .ok()
        .map(|s| s.trim().to_string())
}

/// Writes the config hash to an index directory.
///
/// Creates the index directory if it doesn't exist.
pub fn write_config_hash(index_dir: &Path, hash: &str) -> io::Result<()> {
    fs::create_dir_all(index_dir)?;
    let hash_path = config_hash_path(index_dir);
    fs::write(&hash_path, hash)
}

/// Determines the current status of the index.
///
/// This checks:
/// 1. Whether the index directory exists
/// 2. Whether the stored config hash matches the current config
///
/// Note: This does NOT check for stale files (that's handled by the manifest).
/// It only checks for Missing and ConfigChanged states.
pub fn detect_index_status(config: &Config) -> IndexStatus {
    let Some(index_dir) = index_directory(config) else {
        return IndexStatus::Missing;
    };

    if !index_dir.exists() {
        return IndexStatus::Missing;
    }

    // Check if the tantivy index exists (meta.json is the marker file)
    if !index_dir.join("meta.json").exists() {
        return IndexStatus::Missing;
    }

    // Check config hash
    let current_hash = compute_config_hash(config);
    match read_stored_hash(&index_dir) {
        Some(stored_hash) if stored_hash == current_hash => IndexStatus::Current,
        Some(_) => IndexStatus::ConfigChanged,
        None => IndexStatus::ConfigChanged, // No hash stored, treat as config changed
    }
}

/// Checks if an index exists at the given path.
#[cfg(test)]
pub fn index_exists(index_dir: &Path) -> bool {
    index_dir.join("meta.json").exists()
}

#[cfg(test)]
mod test {
    use std::{fs, path::Path};

    use tempfile::TempDir;

    use super::*;

    fn config_with_root(root: &Path) -> Config {
        Config {
            config_root: Some(root.to_path_buf()),
            ..Default::default()
        }
    }

    #[test]
    fn status_description() {
        assert_eq!(IndexStatus::Current.description(), "current");
        assert_eq!(
            IndexStatus::ConfigChanged.description(),
            "stale (config changed)"
        );
        assert_eq!(IndexStatus::Stale.description(), "stale");
        assert_eq!(IndexStatus::Missing.description(), "missing");
    }

    #[test]
    fn status_needs_update() {
        assert!(!IndexStatus::Current.needs_update());
        assert!(IndexStatus::ConfigChanged.needs_update());
        assert!(IndexStatus::Stale.needs_update());
        assert!(IndexStatus::Missing.needs_update());
    }

    #[test]
    fn status_needs_rebuild() {
        assert!(!IndexStatus::Current.needs_rebuild());
        assert!(IndexStatus::ConfigChanged.needs_rebuild());
        assert!(!IndexStatus::Stale.needs_rebuild());
        assert!(IndexStatus::Missing.needs_rebuild());
    }

    #[test]
    fn read_write_config_hash() {
        let temp = TempDir::new().unwrap();
        let index_dir = temp.path().join("index");

        // Initially no hash
        assert!(read_stored_hash(&index_dir).is_none());

        // Write and read back
        write_config_hash(&index_dir, "abc123def456").unwrap();
        assert_eq!(
            read_stored_hash(&index_dir),
            Some("abc123def456".to_string())
        );

        // Overwrite
        write_config_hash(&index_dir, "new_hash_value").unwrap();
        assert_eq!(
            read_stored_hash(&index_dir),
            Some("new_hash_value".to_string())
        );
    }

    #[test]
    fn read_hash_trims_whitespace() {
        let temp = TempDir::new().unwrap();
        let index_dir = temp.path().join("index");
        fs::create_dir_all(&index_dir).unwrap();

        let hash_path = config_hash_path(&index_dir);
        fs::write(&hash_path, "  abc123  \n").unwrap();

        assert_eq!(read_stored_hash(&index_dir), Some("abc123".to_string()));
    }

    #[test]
    fn index_exists_checks_meta_json() {
        let temp = TempDir::new().unwrap();
        let index_dir = temp.path().join("index");

        // No directory
        assert!(!index_exists(&index_dir));

        // Directory exists but no meta.json
        fs::create_dir_all(&index_dir).unwrap();
        assert!(!index_exists(&index_dir));

        // meta.json exists
        fs::write(index_dir.join("meta.json"), "{}").unwrap();
        assert!(index_exists(&index_dir));
    }

    #[test]
    fn detect_status_missing_when_no_config_root() {
        let config = Config::default();
        assert_eq!(detect_index_status(&config), IndexStatus::Missing);
    }

    #[test]
    fn detect_status_missing_when_no_index_dir() {
        let temp = TempDir::new().unwrap();
        let config_path = temp.path().join(".ra.toml");
        fs::write(&config_path, "").unwrap();

        let config = config_with_root(temp.path());

        assert_eq!(detect_index_status(&config), IndexStatus::Missing);
    }

    #[test]
    fn detect_status_missing_when_no_meta_json() {
        let temp = TempDir::new().unwrap();
        let config_path = temp.path().join(".ra.toml");
        fs::write(&config_path, "").unwrap();

        // Create .ra/index but no meta.json
        let index_dir = temp.path().join(".ra").join("index");
        fs::create_dir_all(&index_dir).unwrap();

        let config = config_with_root(temp.path());

        assert_eq!(detect_index_status(&config), IndexStatus::Missing);
    }

    #[test]
    fn detect_status_config_changed_when_no_hash() {
        let temp = TempDir::new().unwrap();
        let config_path = temp.path().join(".ra.toml");
        fs::write(&config_path, "").unwrap();

        // Create index with meta.json but no config_hash
        let index_dir = temp.path().join(".ra").join("index");
        fs::create_dir_all(&index_dir).unwrap();
        fs::write(index_dir.join("meta.json"), "{}").unwrap();

        let config = config_with_root(temp.path());

        assert_eq!(detect_index_status(&config), IndexStatus::ConfigChanged);
    }

    #[test]
    fn detect_status_config_changed_when_hash_differs() {
        let temp = TempDir::new().unwrap();
        let config_path = temp.path().join(".ra.toml");
        fs::write(&config_path, "").unwrap();

        let index_dir = temp.path().join(".ra").join("index");
        fs::create_dir_all(&index_dir).unwrap();
        fs::write(index_dir.join("meta.json"), "{}").unwrap();
        write_config_hash(&index_dir, "old_hash").unwrap();

        let config = config_with_root(temp.path());

        assert_eq!(detect_index_status(&config), IndexStatus::ConfigChanged);
    }

    #[test]
    fn detect_status_current_when_hash_matches() {
        let temp = TempDir::new().unwrap();
        let config_path = temp.path().join(".ra.toml");
        fs::write(&config_path, "").unwrap();

        let config = config_with_root(temp.path());

        // Create index with matching hash
        let index_dir = temp.path().join(".ra").join("index");
        fs::create_dir_all(&index_dir).unwrap();
        fs::write(index_dir.join("meta.json"), "{}").unwrap();

        let current_hash = compute_config_hash(&config);
        write_config_hash(&index_dir, &current_hash).unwrap();

        assert_eq!(detect_index_status(&config), IndexStatus::Current);
    }
}
