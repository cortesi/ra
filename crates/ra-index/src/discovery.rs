//! File discovery for indexing.
//!
//! Walks configured trees to discover files that should be indexed,
//! applying include/exclude patterns and filtering out binaries and
//! directory symlinks.

use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    time::SystemTime,
};

use ra_config::{CompiledPatterns, Tree};
use walkdir::WalkDir;

use crate::IndexError;

/// A file discovered for indexing.
#[derive(Debug, Clone)]
pub struct DiscoveredFile {
    /// Tree name this file belongs to.
    pub tree: String,
    /// Absolute path to the file.
    pub abs_path: PathBuf,
    /// Relative path within the tree.
    pub rel_path: PathBuf,
    /// File modification time.
    pub mtime: SystemTime,
}

/// Discovers all files that should be indexed from the given trees.
///
/// For each tree, walks the directory tree and returns files that:
/// - Match at least one include pattern (or match `**/*.md` / `**/*.txt` if no patterns)
/// - Don't match any exclude pattern
/// - Are regular files (not directories, symlinks to directories, or other special files)
/// - Are not binary files (based on file extension heuristics)
pub fn discover_files(
    trees: &[Tree],
    patterns: &CompiledPatterns,
) -> Result<Vec<DiscoveredFile>, IndexError> {
    let mut files = Vec::new();

    for tree in trees {
        if !tree.path.exists() {
            continue;
        }

        for entry in WalkDir::new(&tree.path)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| !is_hidden(e.file_name()))
        {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };

            // Skip directories
            if entry.file_type().is_dir() {
                continue;
            }

            // Skip symlinks (we don't follow them)
            if entry.file_type().is_symlink() {
                continue;
            }

            let abs_path = entry.path().to_path_buf();

            // Compute relative path from tree root
            let rel_path = match abs_path.strip_prefix(&tree.path) {
                Ok(p) => p.to_path_buf(),
                Err(_) => continue,
            };

            // Check if file matches patterns
            if !patterns.matches(&tree.name, &rel_path) {
                continue;
            }

            // Skip binary files
            if is_binary_file(&abs_path) {
                continue;
            }

            // Get modification time
            let mtime = match entry.metadata() {
                Ok(m) => m.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                Err(_) => continue,
            };

            files.push(DiscoveredFile {
                tree: tree.name.clone(),
                abs_path,
                rel_path,
                mtime,
            });
        }
    }

    Ok(files)
}

/// Checks if a filename represents a hidden file (starts with '.').
fn is_hidden(name: &OsStr) -> bool {
    name.to_str().is_some_and(|s| s.starts_with('.'))
}

/// Checks if a file is likely binary based on extension.
///
/// This is a heuristic check - files with known binary extensions are skipped.
/// Unknown extensions are assumed to be text files.
fn is_binary_file(path: &Path) -> bool {
    const BINARY_EXTENSIONS: &[&str] = &[
        // Images
        "png", "jpg", "jpeg", "gif", "bmp", "ico", "webp", "svg", "tiff", "tif", "psd", "raw",
        "heic", "heif", // Audio
        "mp3", "wav", "flac", "aac", "ogg", "wma", "m4a", "opus", // Video
        "mp4", "avi", "mkv", "mov", "wmv", "flv", "webm", "m4v", "mpeg", "mpg",
        // Archives
        "zip", "tar", "gz", "bz2", "xz", "7z", "rar", "iso", "dmg", // Executables
        "exe", "dll", "so", "dylib", "bin", "app", // Documents (binary)
        "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "odt", "ods", "odp",
        // Fonts
        "ttf", "otf", "woff", "woff2", "eot", // Database
        "db", "sqlite", "sqlite3", "mdb", // Other binary
        "class", "pyc", "pyo", "o", "a", "lib", "obj", "wasm",
    ];

    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| BINARY_EXTENSIONS.contains(&ext.to_lowercase().as_str()))
}

#[cfg(test)]
mod test {
    use std::{fs, slice};

    use tempfile::TempDir;

    use super::*;

    fn create_test_tree(temp: &TempDir) -> (Tree, PathBuf) {
        let tree_path = temp.path().join("docs");
        fs::create_dir_all(&tree_path).unwrap();

        let tree = Tree {
            name: "docs".to_string(),
            path: tree_path.clone(),
            is_global: false,
            include: vec!["**/*.md".to_string(), "**/*.txt".to_string()],
            exclude: vec![],
        };

        (tree, tree_path)
    }

    #[test]
    fn discover_files_finds_matching_files() {
        let temp = TempDir::new().unwrap();
        let (tree, tree_path) = create_test_tree(&temp);

        // Create some files
        fs::write(tree_path.join("readme.md"), "# Readme").unwrap();
        fs::write(tree_path.join("notes.txt"), "Notes").unwrap();
        fs::write(tree_path.join("image.png"), "fake png").unwrap();
        fs::create_dir(tree_path.join("subdir")).unwrap();
        fs::write(tree_path.join("subdir/nested.md"), "Nested").unwrap();

        let patterns = CompiledPatterns::compile(slice::from_ref(&tree)).unwrap();
        let files = discover_files(slice::from_ref(&tree), &patterns).unwrap();

        assert_eq!(files.len(), 3);

        let paths: Vec<_> = files.iter().map(|f| f.rel_path.clone()).collect();
        assert!(paths.contains(&PathBuf::from("readme.md")));
        assert!(paths.contains(&PathBuf::from("notes.txt")));
        assert!(paths.contains(&PathBuf::from("subdir/nested.md")));
    }

    #[test]
    fn discover_files_excludes_binary_files() {
        let temp = TempDir::new().unwrap();
        let tree_path = temp.path().join("docs");
        fs::create_dir_all(&tree_path).unwrap();

        let tree = Tree {
            name: "docs".to_string(),
            path: tree_path.clone(),
            is_global: false,
            include: vec!["**/*".to_string()],
            exclude: vec![],
        };

        // Create binary files
        fs::write(tree_path.join("image.png"), "fake png").unwrap();
        fs::write(tree_path.join("archive.zip"), "fake zip").unwrap();
        fs::write(tree_path.join("text.md"), "# Text").unwrap();

        let patterns = CompiledPatterns::compile(slice::from_ref(&tree)).unwrap();
        let files = discover_files(slice::from_ref(&tree), &patterns).unwrap();

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].rel_path, PathBuf::from("text.md"));
    }

    #[test]
    fn discover_files_skips_hidden_directories() {
        let temp = TempDir::new().unwrap();
        let (tree, tree_path) = create_test_tree(&temp);

        // Create hidden directory with files
        fs::create_dir(tree_path.join(".hidden")).unwrap();
        fs::write(tree_path.join(".hidden/secret.md"), "Secret").unwrap();
        fs::write(tree_path.join("visible.md"), "Visible").unwrap();

        let patterns = CompiledPatterns::compile(slice::from_ref(&tree)).unwrap();
        let files = discover_files(slice::from_ref(&tree), &patterns).unwrap();

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].rel_path, PathBuf::from("visible.md"));
    }

    #[test]
    fn discover_files_applies_exclude_patterns() {
        let temp = TempDir::new().unwrap();
        let tree_path = temp.path().join("docs");
        fs::create_dir_all(&tree_path).unwrap();
        fs::create_dir(tree_path.join("drafts")).unwrap();

        let tree = Tree {
            name: "docs".to_string(),
            path: tree_path.clone(),
            is_global: false,
            include: vec!["**/*.md".to_string()],
            exclude: vec!["**/drafts/**".to_string()],
        };

        fs::write(tree_path.join("published.md"), "Published").unwrap();
        fs::write(tree_path.join("drafts/draft.md"), "Draft").unwrap();

        let patterns = CompiledPatterns::compile(slice::from_ref(&tree)).unwrap();
        let files = discover_files(slice::from_ref(&tree), &patterns).unwrap();

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].rel_path, PathBuf::from("published.md"));
    }

    #[test]
    fn is_binary_file_detects_binary_extensions() {
        assert!(is_binary_file(Path::new("image.png")));
        assert!(is_binary_file(Path::new("archive.ZIP")));
        assert!(is_binary_file(Path::new("video.mp4")));
        assert!(is_binary_file(Path::new("doc.pdf")));

        assert!(!is_binary_file(Path::new("readme.md")));
        assert!(!is_binary_file(Path::new("notes.txt")));
        assert!(!is_binary_file(Path::new("code.rs")));
        assert!(!is_binary_file(Path::new("no_extension")));
    }

    #[test]
    fn discover_files_handles_missing_tree() {
        let tree = Tree {
            name: "missing".to_string(),
            path: PathBuf::from("/nonexistent/path"),
            is_global: false,
            include: vec!["**/*.md".to_string()],
            exclude: vec![],
        };

        let patterns = CompiledPatterns::compile(slice::from_ref(&tree)).unwrap();
        let files = discover_files(slice::from_ref(&tree), &patterns).unwrap();

        assert!(files.is_empty());
    }
}
