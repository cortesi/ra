//! Manifest diffing for incremental updates.
//!
//! Compares the current filesystem state against the stored manifest to
//! determine which files need to be indexed, reindexed, or removed.

use std::{collections::HashSet, path::PathBuf, time::UNIX_EPOCH};

use crate::{
    discovery::DiscoveredFile,
    manifest::{Manifest, ManifestEntry},
};

/// The result of diffing the current filesystem against the manifest.
#[derive(Debug, Default)]
pub struct ManifestDiff {
    /// Files that are new and need to be indexed.
    pub added: Vec<DiscoveredFile>,
    /// Files that have been modified since last indexing.
    pub modified: Vec<DiscoveredFile>,
    /// Absolute paths of files that have been removed.
    pub removed: Vec<PathBuf>,
}

impl ManifestDiff {
    /// Returns true if there are no changes.
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.modified.is_empty() && self.removed.is_empty()
    }

    /// Returns the total number of files that need processing.
    pub fn total_changes(&self) -> usize {
        self.added.len() + self.modified.len() + self.removed.len()
    }

    /// Returns all files that need indexing (both added and modified).
    pub fn files_to_index(&self) -> impl Iterator<Item = &DiscoveredFile> {
        self.added.iter().chain(self.modified.iter())
    }
}

/// Computes the difference between discovered files and the stored manifest.
///
/// Returns a `ManifestDiff` containing:
/// - `added`: Files present in `discovered` but not in `manifest`
/// - `modified`: Files present in both but with different mtimes
/// - `removed`: Files present in `manifest` but not in `discovered`
pub fn diff_manifest(manifest: &Manifest, discovered: &[DiscoveredFile]) -> ManifestDiff {
    let mut diff = ManifestDiff::default();

    // Track which manifest entries we've seen
    let mut seen_paths: HashSet<PathBuf> = HashSet::new();

    for file in discovered {
        seen_paths.insert(file.abs_path.clone());

        match manifest.get(&file.abs_path) {
            None => {
                // New file
                diff.added.push(file.clone());
            }
            Some(entry) => {
                // Check if modified (compare mtime at second resolution to match serialization)
                let stored_secs = entry
                    .mtime
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let current_secs = file
                    .mtime
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);

                if stored_secs != current_secs {
                    diff.modified.push(file.clone());
                }
            }
        }
    }

    // Find removed files
    for (abs_path, _) in manifest.entries() {
        if !seen_paths.contains(abs_path) {
            diff.removed.push(abs_path.clone());
        }
    }

    diff
}

/// Updates the manifest to reflect the current state after processing a diff.
///
/// - Adds entries for newly indexed files
/// - Updates entries for modified files
/// - Removes entries for deleted files
pub fn apply_diff(manifest: &mut Manifest, diff: &ManifestDiff) {
    // Remove deleted files
    for path in &diff.removed {
        manifest.remove(path);
    }

    // Add/update indexed files
    for file in diff.files_to_index() {
        manifest.insert(
            file.abs_path.clone(),
            ManifestEntry {
                tree: file.tree.clone(),
                path: file.rel_path.clone(),
                mtime: file.mtime,
            },
        );
    }
}

#[cfg(test)]
mod test {
    use std::{
        path::Path,
        time::{Duration, SystemTime},
    };

    use super::*;

    fn make_file(tree: &str, rel: &str, abs: &str, secs: u64) -> DiscoveredFile {
        DiscoveredFile {
            tree: tree.to_string(),
            rel_path: PathBuf::from(rel),
            abs_path: PathBuf::from(abs),
            mtime: SystemTime::UNIX_EPOCH + Duration::from_secs(secs),
        }
    }

    fn make_entry(tree: &str, rel: &str, secs: u64) -> ManifestEntry {
        ManifestEntry {
            tree: tree.to_string(),
            path: PathBuf::from(rel),
            mtime: SystemTime::UNIX_EPOCH + Duration::from_secs(secs),
        }
    }

    #[test]
    fn diff_detects_added_files() {
        let manifest = Manifest::new();
        let discovered = vec![make_file("docs", "new.md", "/docs/new.md", 1000)];

        let diff = diff_manifest(&manifest, &discovered);

        assert_eq!(diff.added.len(), 1);
        assert!(diff.modified.is_empty());
        assert!(diff.removed.is_empty());
    }

    #[test]
    fn diff_detects_modified_files() {
        let mut manifest = Manifest::new();
        manifest.insert(
            PathBuf::from("/docs/file.md"),
            make_entry("docs", "file.md", 1000),
        );

        // Same file but with newer mtime
        let discovered = vec![make_file("docs", "file.md", "/docs/file.md", 2000)];

        let diff = diff_manifest(&manifest, &discovered);

        assert!(diff.added.is_empty());
        assert_eq!(diff.modified.len(), 1);
        assert!(diff.removed.is_empty());
    }

    #[test]
    fn diff_detects_removed_files() {
        let mut manifest = Manifest::new();
        manifest.insert(
            PathBuf::from("/docs/old.md"),
            make_entry("docs", "old.md", 1000),
        );

        let discovered: Vec<DiscoveredFile> = vec![];

        let diff = diff_manifest(&manifest, &discovered);

        assert!(diff.added.is_empty());
        assert!(diff.modified.is_empty());
        assert_eq!(diff.removed.len(), 1);
    }

    #[test]
    fn diff_ignores_unchanged_files() {
        let mut manifest = Manifest::new();
        manifest.insert(
            PathBuf::from("/docs/same.md"),
            make_entry("docs", "same.md", 1000),
        );

        let discovered = vec![make_file("docs", "same.md", "/docs/same.md", 1000)];

        let diff = diff_manifest(&manifest, &discovered);

        assert!(diff.is_empty());
    }

    #[test]
    fn diff_handles_mixed_changes() {
        let mut manifest = Manifest::new();
        manifest.insert(
            PathBuf::from("/docs/unchanged.md"),
            make_entry("docs", "unchanged.md", 1000),
        );
        manifest.insert(
            PathBuf::from("/docs/modified.md"),
            make_entry("docs", "modified.md", 1000),
        );
        manifest.insert(
            PathBuf::from("/docs/removed.md"),
            make_entry("docs", "removed.md", 1000),
        );

        let discovered = vec![
            make_file("docs", "unchanged.md", "/docs/unchanged.md", 1000),
            make_file("docs", "modified.md", "/docs/modified.md", 2000),
            make_file("docs", "added.md", "/docs/added.md", 3000),
        ];

        let diff = diff_manifest(&manifest, &discovered);

        assert_eq!(diff.added.len(), 1);
        assert_eq!(diff.modified.len(), 1);
        assert_eq!(diff.removed.len(), 1);
        assert_eq!(diff.total_changes(), 3);
    }

    #[test]
    fn apply_diff_updates_manifest() {
        let mut manifest = Manifest::new();
        manifest.insert(
            PathBuf::from("/docs/old.md"),
            make_entry("docs", "old.md", 1000),
        );

        let diff = ManifestDiff {
            added: vec![make_file("docs", "new.md", "/docs/new.md", 2000)],
            modified: vec![],
            removed: vec![PathBuf::from("/docs/old.md")],
        };

        apply_diff(&mut manifest, &diff);

        assert_eq!(manifest.len(), 1);
        assert!(manifest.get(Path::new("/docs/old.md")).is_none());
        assert!(manifest.get(Path::new("/docs/new.md")).is_some());
    }

    #[test]
    fn files_to_index_iterator() {
        let diff = ManifestDiff {
            added: vec![make_file("docs", "a.md", "/docs/a.md", 1000)],
            modified: vec![make_file("docs", "b.md", "/docs/b.md", 2000)],
            removed: vec![PathBuf::from("/docs/c.md")],
        };

        let files: Vec<_> = diff.files_to_index().collect();
        assert_eq!(files.len(), 2);
    }
}
