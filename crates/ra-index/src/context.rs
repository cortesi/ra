//! Context analysis for finding relevant documentation.
//!
//! The `ContextAnalyzer` examines input files and extracts signals used to find
//! related documentation in the knowledge base. It combines three signal types:
//!
//! 1. **Path analysis**: Extract path components as search terms
//! 2. **Pattern matching**: Match file paths against configured glob patterns
//! 3. **Content analysis**: Extract significant terms using TF-IDF
//!
//! These signals are combined into a search query that finds relevant context.

use std::{
    collections::HashSet,
    fs::File,
    io::{self, Read},
    path::{Component, Path},
};

use ra_config::{CompiledContextPatterns, ContextSettings};

/// Maximum bytes to read for content sampling.
const DEFAULT_SAMPLE_SIZE: usize = 50_000;

/// Signals extracted from analyzing a file.
#[derive(Debug, Clone, Default)]
pub struct ContextSignals {
    /// Terms extracted from path components (directories and filename).
    pub path_terms: Vec<String>,
    /// Terms from matching context patterns.
    pub pattern_terms: Vec<String>,
    /// Content sample for MoreLikeThis query (first N bytes).
    pub content_sample: String,
}

impl ContextSignals {
    /// Returns true if no signals were extracted.
    pub fn is_empty(&self) -> bool {
        self.path_terms.is_empty()
            && self.pattern_terms.is_empty()
            && self.content_sample.is_empty()
    }

    /// Collects all terms (path + pattern) into a single vector.
    pub fn all_terms(&self) -> Vec<String> {
        let mut terms = self.path_terms.clone();
        terms.extend(self.pattern_terms.iter().cloned());
        terms
    }
}

/// Analyzes files to extract context signals.
pub struct ContextAnalyzer {
    /// Compiled glob patterns for pattern matching.
    patterns: CompiledContextPatterns,
    /// Maximum bytes to sample from file content.
    sample_size: usize,
    /// Minimum word length for path component extraction.
    min_word_length: usize,
}

impl ContextAnalyzer {
    /// Creates a new analyzer with the given settings and patterns.
    pub fn new(settings: &ContextSettings, patterns: CompiledContextPatterns) -> Self {
        Self {
            patterns,
            sample_size: settings.sample_size,
            min_word_length: settings.min_word_length,
        }
    }

    /// Creates an analyzer with default settings (for testing).
    pub fn with_defaults() -> Self {
        Self {
            patterns: CompiledContextPatterns::compile(&ContextSettings::default())
                .expect("default patterns should compile"),
            sample_size: DEFAULT_SAMPLE_SIZE,
            min_word_length: 4,
        }
    }

    /// Analyzes a file and extracts context signals.
    ///
    /// Returns an error if the file cannot be read.
    pub fn analyze_file(&self, path: &Path) -> io::Result<ContextSignals> {
        Ok(ContextSignals {
            path_terms: self.extract_path_terms(path),
            pattern_terms: self.patterns.match_terms(path),
            content_sample: self.sample_content(path)?,
        })
    }

    /// Extracts meaningful terms from file path components.
    ///
    /// Splits path into components and filters based on length and content.
    fn extract_path_terms(&self, path: &Path) -> Vec<String> {
        let mut terms = Vec::new();

        for component in path.components() {
            if let Component::Normal(os_str) = component
                && let Some(s) = os_str.to_str()
            {
                // Split on common delimiters and filter
                for part in s.split(['_', '-', '.']) {
                    let part = part.to_lowercase();
                    if self.is_meaningful_term(&part) {
                        terms.push(part);
                    }
                }
            }
        }

        // Deduplicate while preserving order
        let mut seen = HashSet::new();
        terms.retain(|t| seen.insert(t.clone()));

        terms
    }

    /// Checks if a term is meaningful enough to include.
    fn is_meaningful_term(&self, term: &str) -> bool {
        // Must meet minimum length
        if term.len() < self.min_word_length {
            return false;
        }

        // Skip common file extensions that don't add meaning
        let skip_extensions = [
            "rs", "py", "js", "ts", "go", "md", "txt", "html", "css", "json",
        ];
        if skip_extensions.contains(&term) {
            return false;
        }

        // Skip very common directory names
        let skip_dirs = ["src", "lib", "bin", "test", "tests", "docs", "doc"];
        if skip_dirs.contains(&term) {
            return false;
        }

        true
    }

    /// Samples content from a file for content analysis.
    ///
    /// Reads up to `sample_size` bytes from the beginning of the file.
    fn sample_content(&self, path: &Path) -> io::Result<String> {
        let mut file = File::open(path)?;
        let mut buffer = vec![0u8; self.sample_size];

        let bytes_read = file.read(&mut buffer)?;
        buffer.truncate(bytes_read);

        // Convert to string, replacing invalid UTF-8
        let content = String::from_utf8_lossy(&buffer).to_string();

        // For very short files, return as-is
        if content.len() < 100 {
            return Ok(content);
        }

        Ok(content)
    }
}

/// Checks if a file is likely binary based on its extension.
pub fn is_binary_file(path: &Path) -> bool {
    let binary_extensions = [
        // Compiled/executable
        "exe", "dll", "so", "dylib", "o", "a", "lib", "obj", "class", "pyc", "pyo", "wasm",
        // Archives
        "zip", "tar", "gz", "bz2", "xz", "7z", "rar", "jar", "war", "ear", // Images
        "png", "jpg", "jpeg", "gif", "bmp", "ico", "svg", "webp", "tiff", "psd",
        // Audio/Video
        "mp3", "mp4", "wav", "flac", "ogg", "avi", "mkv", "mov", "wmv", "webm",
        // Documents (binary)
        "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "odt", // Databases
        "db", "sqlite", "mdb", // Fonts
        "ttf", "otf", "woff", "woff2", "eot", // Other binary formats
        "bin", "dat", "pak", "bundle",
    ];

    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| binary_extensions.contains(&ext.to_lowercase().as_str()))
}

#[cfg(test)]
mod test {
    use std::{fs, io::Write, path::PathBuf};

    use tempfile::TempDir;

    use super::*;

    fn create_test_file(dir: &TempDir, name: &str, content: &str) -> PathBuf {
        let path = dir.path().join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let mut file = File::create(&path).unwrap();
        file.write_all(content.as_bytes()).unwrap();
        path
    }

    #[test]
    fn extract_path_terms_basic() {
        let analyzer = ContextAnalyzer::with_defaults();
        let path = Path::new("src/auth/oauth_handler.rs");
        let terms = analyzer.extract_path_terms(path);

        assert!(terms.contains(&"auth".to_string()));
        assert!(terms.contains(&"oauth".to_string()));
        assert!(terms.contains(&"handler".to_string()));

        // Should skip common dirs and extensions
        assert!(!terms.contains(&"src".to_string()));
        assert!(!terms.contains(&"rs".to_string()));
    }

    #[test]
    fn extract_path_terms_with_dashes() {
        let analyzer = ContextAnalyzer::with_defaults();
        let path = Path::new("api/user-management/create-account.rs");
        let terms = analyzer.extract_path_terms(path);

        assert!(terms.contains(&"user".to_string()));
        assert!(terms.contains(&"management".to_string()));
        assert!(terms.contains(&"create".to_string()));
        assert!(terms.contains(&"account".to_string()));
    }

    #[test]
    fn extract_path_terms_deduplicates() {
        let analyzer = ContextAnalyzer::with_defaults();
        let path = Path::new("auth/auth_service.rs");
        let terms = analyzer.extract_path_terms(path);

        // Should only have one "auth"
        assert_eq!(terms.iter().filter(|t| *t == "auth").count(), 1);
    }

    #[test]
    fn sample_content_small_file() {
        let dir = TempDir::new().unwrap();
        let path = create_test_file(&dir, "small.txt", "Hello, world!");

        let analyzer = ContextAnalyzer::with_defaults();
        let content = analyzer.sample_content(&path).unwrap();

        assert_eq!(content, "Hello, world!");
    }

    #[test]
    fn sample_content_truncates_large_file() {
        let dir = TempDir::new().unwrap();
        let large_content = "x".repeat(100_000);
        let path = create_test_file(&dir, "large.txt", &large_content);

        let mut analyzer = ContextAnalyzer::with_defaults();
        analyzer.sample_size = 1000;
        let content = analyzer.sample_content(&path).unwrap();

        assert_eq!(content.len(), 1000);
    }

    #[test]
    fn analyze_file_combines_signals() {
        let dir = TempDir::new().unwrap();
        let path = create_test_file(
            &dir,
            "src/auth/login.rs",
            "fn authenticate() { /* login logic */ }",
        );

        let analyzer = ContextAnalyzer::with_defaults();
        let signals = analyzer.analyze_file(&path).unwrap();

        // Should have path terms
        assert!(signals.path_terms.contains(&"auth".to_string()));
        assert!(signals.path_terms.contains(&"login".to_string()));

        // Should have content
        assert!(signals.content_sample.contains("authenticate"));
    }

    #[test]
    fn is_binary_file_detects_binaries() {
        assert!(is_binary_file(Path::new("app.exe")));
        assert!(is_binary_file(Path::new("lib.so")));
        assert!(is_binary_file(Path::new("image.png")));
        assert!(is_binary_file(Path::new("archive.zip")));
        assert!(is_binary_file(Path::new("doc.pdf")));
    }

    #[test]
    fn is_binary_file_allows_text() {
        assert!(!is_binary_file(Path::new("main.rs")));
        assert!(!is_binary_file(Path::new("README.md")));
        assert!(!is_binary_file(Path::new("config.json")));
        assert!(!is_binary_file(Path::new("script.py")));
        assert!(!is_binary_file(Path::new("style.css")));
    }

    #[test]
    fn is_binary_file_handles_no_extension() {
        assert!(!is_binary_file(Path::new("Makefile")));
        assert!(!is_binary_file(Path::new("Dockerfile")));
    }
}
