//! Manifest tracking for indexed files.
//!
//! The manifest stores metadata about all indexed files including their paths, tree names,
//! and modification times. It is used for incremental updates to determine which files
//! need reindexing.

use std::{
    collections::HashMap,
    fs, io,
    path::{Path, PathBuf},
    time::SystemTime,
};

use serde::{Deserialize, Serialize};

use crate::IndexError;

/// An entry in the manifest representing a single indexed file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManifestEntry {
    /// Tree name this file belongs to.
    pub tree: String,
    /// Relative path within the tree.
    pub path: PathBuf,
    /// File modification time when last indexed.
    #[serde(with = "system_time_serde")]
    pub mtime: SystemTime,
}

/// Tracks indexed files and their modification times.
///
/// The manifest is stored as JSON and used to detect which files have changed
/// since the last indexing operation, enabling efficient incremental updates.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Manifest {
    /// Map from absolute file path to manifest entry.
    #[serde(default)]
    entries: HashMap<PathBuf, ManifestEntry>,
}

impl Manifest {
    /// Creates a new empty manifest.
    pub fn new() -> Self {
        Self::default()
    }

    /// Loads a manifest from a JSON file.
    ///
    /// Returns an empty manifest if the file doesn't exist.
    /// Returns an error if the file exists but cannot be parsed.
    pub fn load(path: &Path) -> Result<Self, IndexError> {
        if !path.exists() {
            return Ok(Self::new());
        }

        let contents = fs::read_to_string(path)?;
        serde_json::from_str(&contents).map_err(|e| {
            IndexError::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("failed to parse manifest: {}", e),
            ))
        })
    }

    /// Saves the manifest to a JSON file.
    ///
    /// Creates parent directories if they don't exist.
    pub fn save(&self, path: &Path) -> Result<(), IndexError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let contents = serde_json::to_string_pretty(self).map_err(|e| {
            IndexError::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("failed to serialize manifest: {}", e),
            ))
        })?;

        fs::write(path, contents)?;
        Ok(())
    }

    /// Adds or updates an entry in the manifest.
    pub fn insert(&mut self, abs_path: PathBuf, entry: ManifestEntry) {
        self.entries.insert(abs_path, entry);
    }

    /// Removes an entry from the manifest.
    pub fn remove(&mut self, abs_path: &Path) -> Option<ManifestEntry> {
        self.entries.remove(abs_path)
    }

    /// Gets an entry by absolute path.
    pub fn get(&self, abs_path: &Path) -> Option<&ManifestEntry> {
        self.entries.get(abs_path)
    }

    /// Returns an iterator over all entries.
    pub fn entries(&self) -> impl Iterator<Item = (&PathBuf, &ManifestEntry)> {
        self.entries.iter()
    }

    /// Returns the number of entries in the manifest.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns true if the manifest is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Clears all entries from the manifest.
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

/// Serde serialization for `SystemTime` as Unix timestamp (seconds).
mod system_time_serde {
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    /// Serializes a `SystemTime` as a Unix timestamp in seconds.
    pub fn serialize<S>(time: &SystemTime, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let duration = time.duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO);
        duration.as_secs().serialize(serializer)
    }

    /// Deserializes a Unix timestamp (seconds) into a `SystemTime`.
    pub fn deserialize<'de, D>(deserializer: D) -> Result<SystemTime, D::Error>
    where
        D: Deserializer<'de>,
    {
        let secs = u64::deserialize(deserializer)?;
        Ok(UNIX_EPOCH + Duration::from_secs(secs))
    }
}

#[cfg(test)]
mod test {
    use std::time::Duration;

    use tempfile::TempDir;

    use super::*;

    #[test]
    fn manifest_round_trip() {
        let temp = TempDir::new().unwrap();
        let manifest_path = temp.path().join("manifest.json");

        let mut manifest = Manifest::new();
        manifest.insert(
            PathBuf::from("/project/docs/test.md"),
            ManifestEntry {
                tree: "docs".to_string(),
                path: PathBuf::from("test.md"),
                mtime: SystemTime::UNIX_EPOCH + Duration::from_secs(1234567890),
            },
        );
        manifest.insert(
            PathBuf::from("/project/notes/note.txt"),
            ManifestEntry {
                tree: "notes".to_string(),
                path: PathBuf::from("note.txt"),
                mtime: SystemTime::UNIX_EPOCH + Duration::from_secs(9876543210),
            },
        );

        manifest.save(&manifest_path).unwrap();

        let loaded = Manifest::load(&manifest_path).unwrap();
        assert_eq!(loaded.len(), 2);

        let entry = loaded.get(Path::new("/project/docs/test.md")).unwrap();
        assert_eq!(entry.tree, "docs");
        assert_eq!(entry.path, PathBuf::from("test.md"));
    }

    #[test]
    fn manifest_load_missing_file_returns_empty() {
        let temp = TempDir::new().unwrap();
        let manifest_path = temp.path().join("nonexistent.json");

        let manifest = Manifest::load(&manifest_path).unwrap();
        assert!(manifest.is_empty());
    }

    #[test]
    fn manifest_remove_entry() {
        let mut manifest = Manifest::new();
        let path = PathBuf::from("/test/file.md");

        manifest.insert(
            path.clone(),
            ManifestEntry {
                tree: "test".to_string(),
                path: PathBuf::from("file.md"),
                mtime: SystemTime::now(),
            },
        );
        assert_eq!(manifest.len(), 1);

        let removed = manifest.remove(&path);
        assert!(removed.is_some());
        assert!(manifest.is_empty());
    }
}
