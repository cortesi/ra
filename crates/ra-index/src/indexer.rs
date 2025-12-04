//! Full indexing pipeline.
//!
//! The [`Indexer`] orchestrates the complete indexing flow:
//! 1. Discover files matching tree patterns
//! 2. Compare against manifest to find changes
//! 3. Parse changed files with ra-document
//! 4. Convert to [`ChunkDocument`]s and write to index
//! 5. Update manifest with new state

use std::{
    fs,
    io::{Error as IoError, ErrorKind},
    path::{Path, PathBuf},
};

use ra_config::{CompiledPatterns, Config};

use crate::{
    ChunkDocument, DiscoveredFile, IndexError, IndexWriter, Manifest, ManifestDiff, apply_diff,
    compute_config_hash, diff_manifest, discover_files, index_directory, manifest_path,
    write_config_hash,
};

/// Statistics from an indexing operation.
#[derive(Debug, Clone, Default)]
pub struct IndexStats {
    /// Number of files processed.
    pub files_processed: usize,
    /// Number of files skipped due to errors.
    pub files_skipped: usize,
    /// Number of chunks indexed.
    pub chunks_indexed: usize,
    /// Number of files added (new).
    pub files_added: usize,
    /// Number of files updated (modified).
    pub files_updated: usize,
    /// Number of files removed.
    pub files_removed: usize,
    /// Errors encountered during parsing (file path, error message).
    pub parse_errors: Vec<(PathBuf, String)>,
}

impl IndexStats {
    /// Returns true if no errors occurred.
    pub fn is_success(&self) -> bool {
        self.parse_errors.is_empty()
    }

    /// Returns the total number of files that changed.
    pub fn total_changes(&self) -> usize {
        self.files_added + self.files_updated + self.files_removed
    }
}

/// Callback for reporting indexing progress.
pub trait ProgressReporter {
    /// Called when starting to process a file.
    fn on_file_start(&mut self, path: &Path, current: usize, total: usize);

    /// Called when a file was successfully indexed.
    fn on_file_done(&mut self, path: &Path, chunks: usize);

    /// Called when a file could not be parsed.
    fn on_file_error(&mut self, path: &Path, error: &str);

    /// Called when a file was removed from the index.
    fn on_file_removed(&mut self, path: &Path);

    /// Called when indexing is complete.
    fn on_complete(&mut self, stats: &IndexStats);
}

/// A no-op progress reporter for silent indexing.
pub struct SilentReporter;

impl ProgressReporter for SilentReporter {
    fn on_file_start(&mut self, _path: &Path, _current: usize, _total: usize) {}
    fn on_file_done(&mut self, _path: &Path, _chunks: usize) {}
    fn on_file_error(&mut self, _path: &Path, _error: &str) {}
    fn on_file_removed(&mut self, _path: &Path) {}
    fn on_complete(&mut self, _stats: &IndexStats) {}
}

/// Orchestrates the full indexing pipeline.
pub struct Indexer<'a> {
    /// The loaded configuration.
    config: &'a Config,
    /// Compiled include/exclude patterns.
    patterns: CompiledPatterns,
    /// Path to the index directory.
    index_dir: PathBuf,
}

impl<'a> Indexer<'a> {
    /// Creates a new indexer for the given configuration.
    ///
    /// Returns an error if the configuration has no config root or patterns fail to compile.
    pub fn new(config: &'a Config) -> Result<Self, IndexError> {
        let index_dir = index_directory(config).ok_or_else(|| {
            IndexError::Io(IoError::new(
                ErrorKind::NotFound,
                "no configuration root found",
            ))
        })?;

        let patterns = config.compile_patterns().map_err(|e| {
            IndexError::Io(IoError::new(
                ErrorKind::InvalidData,
                format!("failed to compile patterns: {}", e),
            ))
        })?;

        Ok(Self {
            config,
            patterns,
            index_dir,
        })
    }

    /// Performs a full reindex, ignoring the manifest.
    ///
    /// This deletes all existing index data and reindexes everything from scratch.
    pub fn full_reindex<R: ProgressReporter>(
        &self,
        reporter: &mut R,
    ) -> Result<IndexStats, IndexError> {
        // Discover all files
        let discovered = discover_files(&self.config.trees, &self.patterns)?;

        // Create empty manifest (treat everything as new)
        let manifest = Manifest::new();

        // Compute diff (everything is added)
        let diff = diff_manifest(&manifest, &discovered);

        // Run indexing with empty manifest and the "full" diff
        self.index_with_diff(manifest, &diff, reporter, true)
    }

    /// Performs an incremental update, only reindexing changed files.
    pub fn incremental_update<R: ProgressReporter>(
        &self,
        reporter: &mut R,
    ) -> Result<IndexStats, IndexError> {
        // Load existing manifest
        let manifest_file = manifest_path(&self.index_dir);
        let manifest = Manifest::load(&manifest_file)?;

        // Discover current files
        let discovered = discover_files(&self.config.trees, &self.patterns)?;

        // Compute diff
        let diff = diff_manifest(&manifest, &discovered);

        // Run indexing
        self.index_with_diff(manifest, &diff, reporter, false)
    }

    /// Internal method that performs indexing given a manifest and diff.
    fn index_with_diff<R: ProgressReporter>(
        &self,
        mut manifest: Manifest,
        diff: &ManifestDiff,
        reporter: &mut R,
        is_full_reindex: bool,
    ) -> Result<IndexStats, IndexError> {
        let mut stats = IndexStats {
            files_added: diff.added.len(),
            files_updated: diff.modified.len(),
            files_removed: diff.removed.len(),
            ..Default::default()
        };

        // Early return if nothing to do
        if diff.is_empty() {
            reporter.on_complete(&stats);
            return Ok(stats);
        }

        // Open the index with the configured language
        let mut writer = IndexWriter::open(&self.index_dir, &self.config.search.stemmer)?;

        // If full reindex, delete everything first
        if is_full_reindex {
            writer.delete_all()?;
        }

        // Handle removed files
        for removed_path in &diff.removed {
            if let Some(entry) = manifest.get(removed_path) {
                writer.delete_by_path(&entry.tree, entry.path.to_string_lossy().as_ref());
                reporter.on_file_removed(removed_path);
            }
        }

        // Handle added and modified files
        let files_to_index: Vec<_> = diff.files_to_index().collect();
        let total_files = files_to_index.len();

        for (idx, file) in files_to_index.iter().enumerate() {
            reporter.on_file_start(&file.abs_path, idx + 1, total_files);

            // For modified files, delete old chunks first
            if diff.modified.iter().any(|f| f.abs_path == file.abs_path) {
                writer.delete_by_path(&file.tree, file.rel_path.to_string_lossy().as_ref());
            }

            // Parse and index the file
            match self.index_file(&mut writer, file) {
                Ok(chunk_count) => {
                    stats.files_processed += 1;
                    stats.chunks_indexed += chunk_count;
                    reporter.on_file_done(&file.abs_path, chunk_count);
                }
                Err(e) => {
                    stats.files_skipped += 1;
                    let error_msg = e.to_string();
                    stats
                        .parse_errors
                        .push((file.abs_path.clone(), error_msg.clone()));
                    reporter.on_file_error(&file.abs_path, &error_msg);
                    // Continue with other files
                }
            }
        }

        // Commit the index
        writer.commit()?;

        // Update manifest
        apply_diff(&mut manifest, diff);

        // Remove errored files from manifest so they get retried next time
        for (path, _) in &stats.parse_errors {
            manifest.remove(path);
        }

        // Save manifest
        let manifest_file = manifest_path(&self.index_dir);
        manifest.save(&manifest_file)?;

        // Save config hash
        let config_hash = compute_config_hash(self.config);
        write_config_hash(&self.index_dir, &config_hash)?;

        reporter.on_complete(&stats);
        Ok(stats)
    }

    /// Parses and indexes a single file, returning the number of chunks indexed.
    fn index_file(
        &self,
        writer: &mut IndexWriter,
        file: &DiscoveredFile,
    ) -> Result<usize, IndexError> {
        // Read file content to get mtime-independent parsing
        let content = fs::read_to_string(&file.abs_path)?;

        // Determine file type and parse
        let ext = file.abs_path.extension().and_then(|e| e.to_str());

        let result = match ext {
            Some("md" | "markdown") => {
                ra_document::parse_markdown(&content, &file.rel_path, &file.tree)
            }
            Some("txt") => ra_document::parse_text(&content, &file.rel_path, &file.tree),
            Some(ext) => {
                return Err(IndexError::Io(IoError::new(
                    ErrorKind::InvalidData,
                    format!("unsupported file type: .{}", ext),
                )));
            }
            None => {
                return Err(IndexError::Io(IoError::new(
                    ErrorKind::InvalidData,
                    "file has no extension",
                )));
            }
        };

        // Convert to ChunkDocuments and index
        let chunk_docs = ChunkDocument::from_document(&result.document, file.mtime);
        let chunk_count = chunk_docs.len();

        writer.add_documents(&chunk_docs)?;

        Ok(chunk_count)
    }

    /// Returns the path to the index directory.
    pub fn index_dir(&self) -> &Path {
        &self.index_dir
    }
}

#[cfg(test)]
mod test {
    use std::{cell::RefCell, thread, time::Duration};

    use ra_config::Tree;
    use tempfile::TempDir;

    use super::*;

    /// Test reporter that records all events.
    #[derive(Default)]
    struct TestReporter {
        events: RefCell<Vec<String>>,
    }

    impl ProgressReporter for TestReporter {
        fn on_file_start(&mut self, path: &Path, current: usize, total: usize) {
            self.events.borrow_mut().push(format!(
                "start: {} ({}/{})",
                path.display(),
                current,
                total
            ));
        }

        fn on_file_done(&mut self, path: &Path, chunks: usize) {
            self.events
                .borrow_mut()
                .push(format!("done: {} ({} chunks)", path.display(), chunks));
        }

        fn on_file_error(&mut self, path: &Path, error: &str) {
            self.events
                .borrow_mut()
                .push(format!("error: {} - {}", path.display(), error));
        }

        fn on_file_removed(&mut self, path: &Path) {
            self.events
                .borrow_mut()
                .push(format!("removed: {}", path.display()));
        }

        fn on_complete(&mut self, stats: &IndexStats) {
            self.events.borrow_mut().push(format!(
                "complete: {} files, {} chunks, {} errors",
                stats.files_processed, stats.chunks_indexed, stats.files_skipped
            ));
        }
    }

    fn create_test_config(temp: &TempDir) -> Config {
        let tree_path = temp.path().join("docs");
        fs::create_dir_all(&tree_path).unwrap();

        Config {
            trees: vec![Tree {
                name: "docs".to_string(),
                path: tree_path,
                is_global: false,
                include: vec!["**/*.md".to_string(), "**/*.txt".to_string()],
                exclude: vec![],
            }],
            config_root: Some(temp.path().to_path_buf()),
            ..Default::default()
        }
    }

    #[test]
    fn full_reindex_indexes_all_files() {
        let temp = TempDir::new().unwrap();
        let config = create_test_config(&temp);
        let tree_path = temp.path().join("docs");

        // Create test files
        fs::write(tree_path.join("readme.md"), "# Readme\n\nContent here.").unwrap();
        fs::write(tree_path.join("notes.txt"), "Some notes").unwrap();

        let indexer = Indexer::new(&config).unwrap();
        let mut reporter = TestReporter::default();
        let stats = indexer.full_reindex(&mut reporter).unwrap();

        assert_eq!(stats.files_processed, 2);
        assert_eq!(stats.files_added, 2);
        assert_eq!(stats.files_skipped, 0);
        assert!(stats.chunks_indexed >= 2);
    }

    #[test]
    fn incremental_update_skips_unchanged_files() {
        let temp = TempDir::new().unwrap();
        let config = create_test_config(&temp);
        let tree_path = temp.path().join("docs");

        // Create initial file
        fs::write(tree_path.join("readme.md"), "# Readme").unwrap();

        let indexer = Indexer::new(&config).unwrap();

        // First index
        let mut reporter = SilentReporter;
        let stats1 = indexer.full_reindex(&mut reporter).unwrap();
        assert_eq!(stats1.files_processed, 1);

        // Second index without changes - should do nothing
        let stats2 = indexer.incremental_update(&mut reporter).unwrap();
        assert_eq!(stats2.files_processed, 0);
        assert_eq!(stats2.total_changes(), 0);
    }

    #[test]
    fn incremental_update_detects_new_files() {
        let temp = TempDir::new().unwrap();
        let config = create_test_config(&temp);
        let tree_path = temp.path().join("docs");

        // Create initial file
        fs::write(tree_path.join("readme.md"), "# Readme").unwrap();

        let indexer = Indexer::new(&config).unwrap();
        let mut reporter = SilentReporter;

        // First index
        indexer.full_reindex(&mut reporter).unwrap();

        // Add new file
        fs::write(tree_path.join("new.md"), "# New file").unwrap();

        // Incremental update should find the new file
        let stats = indexer.incremental_update(&mut reporter).unwrap();
        assert_eq!(stats.files_added, 1);
        assert_eq!(stats.files_processed, 1);
    }

    #[test]
    fn incremental_update_detects_modified_files() {
        let temp = TempDir::new().unwrap();
        let config = create_test_config(&temp);
        let tree_path = temp.path().join("docs");

        // Create initial file
        let file_path = tree_path.join("readme.md");
        fs::write(&file_path, "# Readme").unwrap();

        let indexer = Indexer::new(&config).unwrap();
        let mut reporter = SilentReporter;

        // First index
        indexer.full_reindex(&mut reporter).unwrap();

        // Modify file - need to change mtime
        thread::sleep(Duration::from_secs(1));
        fs::write(&file_path, "# Readme Updated").unwrap();

        // Incremental update should find the modified file
        let stats = indexer.incremental_update(&mut reporter).unwrap();
        assert_eq!(stats.files_updated, 1);
        assert_eq!(stats.files_processed, 1);
    }

    #[test]
    fn incremental_update_handles_removed_files() {
        let temp = TempDir::new().unwrap();
        let config = create_test_config(&temp);
        let tree_path = temp.path().join("docs");

        // Create initial files
        fs::write(tree_path.join("keep.md"), "# Keep").unwrap();
        fs::write(tree_path.join("remove.md"), "# Remove").unwrap();

        let indexer = Indexer::new(&config).unwrap();
        let mut reporter = SilentReporter;

        // First index
        indexer.full_reindex(&mut reporter).unwrap();

        // Remove a file
        fs::remove_file(tree_path.join("remove.md")).unwrap();

        // Incremental update should detect removal
        let stats = indexer.incremental_update(&mut reporter).unwrap();
        assert_eq!(stats.files_removed, 1);
    }

    #[test]
    fn handles_unparseable_files_gracefully() {
        let temp = TempDir::new().unwrap();
        let config = create_test_config(&temp);
        let tree_path = temp.path().join("docs");

        // Create valid file
        fs::write(tree_path.join("valid.md"), "# Valid").unwrap();
        // Create invalid file (binary content pretending to be text)
        fs::write(tree_path.join("invalid.md"), vec![0xFF, 0xFE, 0x00, 0x01]).unwrap();

        let indexer = Indexer::new(&config).unwrap();
        let mut reporter = TestReporter::default();
        let stats = indexer.full_reindex(&mut reporter).unwrap();

        // Should have processed valid file, skipped invalid
        assert_eq!(stats.files_processed, 1);
        assert_eq!(stats.files_skipped, 1);
        assert_eq!(stats.parse_errors.len(), 1);
    }

    #[test]
    fn manifest_is_saved_and_loaded() {
        let temp = TempDir::new().unwrap();
        let config = create_test_config(&temp);
        let tree_path = temp.path().join("docs");

        fs::write(tree_path.join("test.md"), "# Test").unwrap();

        let indexer = Indexer::new(&config).unwrap();
        let mut reporter = SilentReporter;

        // First index
        indexer.full_reindex(&mut reporter).unwrap();

        // Check manifest exists
        let manifest_file = manifest_path(indexer.index_dir());
        assert!(manifest_file.exists());

        // Load manifest and verify
        let manifest = Manifest::load(&manifest_file).unwrap();
        assert_eq!(manifest.len(), 1);
    }
}
