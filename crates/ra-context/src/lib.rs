//! Context analysis for finding relevant documentation.
//!
//! The `ContextAnalyzer` examines input files and extracts signals used to find
//! related documentation in the knowledge base. It combines three signal types:
//!
//! 1. **Path analysis**: Extract path components as search terms
//! 2. **Pattern matching**: Match file paths against configured glob patterns
//! 3. **Content analysis**: Extract significant terms using keyword extraction
//!
//! These signals are combined into a search query that finds relevant context.
//!
//! ## Keyword Extraction Algorithms
//!
//! Multiple algorithms are available for keyword extraction:
//!
//! - **TextRank** (default): Graph-based ranking similar to PageRank
//! - **TF-IDF**: Corpus-aware ranking using index statistics
//! - **RAKE**: Rapid Automatic Keyword Extraction based on co-occurrence
//! - **YAKE**: Statistical extraction, good for short texts

#![warn(missing_docs)]

mod analyze;
pub mod keyword;
mod parser;
mod query;
mod rank;
mod stopwords;
mod term;

use std::{
    collections::HashSet,
    fs::File,
    io::{self, Read},
    path::{Component, Path},
};

pub use analyze::{AnalysisConfig, ContextAnalysis, analyze_context};
pub use keyword::{
    CorpusTfIdf, KeywordAlgorithm, RakeExtractor, ScoredKeyword, TextRankExtractor, YakeExtractor,
};
pub use query::ContextQuery;
use ra_config::{CompiledContextRules, ContextSettings};
pub use rank::{IdfProvider, RankedTerm};
pub use stopwords::Stopwords;
pub use term::WeightedTerm;

/// Token extracted from a path component with filename flag.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PathToken {
    /// Lowercased token text split from a path component.
    term: String,
    /// True when the token originated from the file name component.
    is_filename: bool,
}

/// Weight for filename path components.
const WEIGHT_PATH_FILENAME: f32 = 4.0;
/// Weight for directory path components.
const WEIGHT_PATH_DIRECTORY: f32 = 3.0;

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
    /// Compiled context rules for pattern matching.
    rules: CompiledContextRules,
    /// Maximum bytes to sample from file content.
    sample_size: usize,
    /// Minimum word length for path component extraction.
    min_word_length: usize,
}

impl ContextAnalyzer {
    /// Creates a new analyzer with the given settings and rules.
    pub fn new(settings: &ContextSettings, rules: CompiledContextRules) -> Self {
        Self {
            rules,
            sample_size: settings.sample_size,
            min_word_length: settings.min_word_length,
        }
    }

    /// Creates an analyzer with default settings (for testing).
    pub fn with_defaults() -> Self {
        Self {
            rules: CompiledContextRules::compile(&ContextSettings::default())
                .expect("default rules should compile"),
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
            pattern_terms: self.rules.match_terms(path),
            content_sample: self.sample_content(path)?,
        })
    }

    /// Extracts meaningful terms from file path components.
    ///
    /// Splits path into components and filters based on length and content.
    fn extract_path_terms(&self, path: &Path) -> Vec<String> {
        let mut terms = Vec::new();

        for token in tokenize_path(path) {
            if self.is_meaningful_term(&token.term) {
                terms.push(token.term);
            }
        }

        dedup_preserve_order(terms)
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

/// Extracts weighted terms from a file path.
///
/// Path components are split on common delimiters and assigned weights based on
/// their position:
/// - Filename components: weight 4.0 (source: "path:filename")
/// - Directory components: weight 3.0 (source: "path:dir")
///
/// Terms are filtered against stopwords and deduplicated.
pub(crate) fn extract_path_terms(
    path: &Path,
    stopwords: &Stopwords,
    min_length: usize,
) -> Vec<WeightedTerm> {
    let mut terms: Vec<WeightedTerm> = Vec::new();

    for token in tokenize_path(path) {
        if token.term.len() < min_length {
            continue;
        }
        if !token.term.chars().all(|c| c.is_alphanumeric()) {
            continue;
        }
        if stopwords.contains(&token.term) {
            continue;
        }

        let (source, weight) = if token.is_filename {
            ("path:filename", WEIGHT_PATH_FILENAME)
        } else {
            ("path:dir", WEIGHT_PATH_DIRECTORY)
        };

        if let Some(existing) = terms.iter_mut().find(|t| t.term == token.term) {
            existing.increment();
            if weight > existing.weight {
                existing.source = source.to_string();
                existing.weight = weight;
            }
        } else {
            terms.push(WeightedTerm::new(token.term, source, weight));
        }
    }

    terms
}

/// Splits a path into lowercase tokens, tagging whether they came from the filename.
fn tokenize_path(path: &Path) -> Vec<PathToken> {
    let components: Vec<_> = path.components().collect();
    let num_components = components.len();
    let mut tokens = Vec::new();

    for (idx, component) in components.into_iter().enumerate() {
        if let Component::Normal(os_str) = component
            && let Some(s) = os_str.to_str()
        {
            let is_filename = idx == num_components.saturating_sub(1);
            for part in s.split(['_', '-', '.']) {
                if part.is_empty() {
                    continue;
                }
                tokens.push(PathToken {
                    term: part.to_ascii_lowercase(),
                    is_filename,
                });
            }
        }
    }

    tokens
}

/// Deduplicates strings while preserving original order.
fn dedup_preserve_order(mut terms: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    terms.retain(|t| seen.insert(t.clone()));
    terms
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

    #[test]
    fn extract_path_terms_assigns_weights() {
        let stopwords = Stopwords::new();
        let path = Path::new("api/authentication/oauth_handler.rs");
        let terms = extract_path_terms(path, &stopwords, 3);

        // Find terms by name
        let find_term = |name: &str| terms.iter().find(|t| t.term == name);

        // Directory terms get directory weight
        let api = find_term("api").unwrap();
        assert_eq!(api.source, "path:dir");
        assert_eq!(api.weight, 3.0);

        let auth = find_term("authentication").unwrap();
        assert_eq!(auth.source, "path:dir");
        assert_eq!(auth.weight, 3.0);

        // Filename terms get filename weight
        let oauth = find_term("oauth").unwrap();
        assert_eq!(oauth.source, "path:filename");
        assert_eq!(oauth.weight, 4.0);

        let handler = find_term("handler").unwrap();
        assert_eq!(handler.source, "path:filename");
        assert_eq!(handler.weight, 4.0);
    }

    #[test]
    fn extract_path_terms_filters_stopwords() {
        let stopwords = Stopwords::new();
        // Use Rust keywords which are definitely stopwords
        // "async" and "static" are keywords, "handler" is domain-specific
        let path = Path::new("async/static_handler.rs");
        let terms = extract_path_terms(path, &stopwords, 3);

        let term_strings: Vec<_> = terms.iter().map(|t| t.term.as_str()).collect();

        // "async", "static" are Rust keywords (stopwords)
        assert!(!term_strings.contains(&"async"));
        assert!(!term_strings.contains(&"static"));

        // "handler" should remain (domain-specific)
        assert!(term_strings.contains(&"handler"));
    }

    #[test]
    fn extract_path_terms_deduplicates_with_higher_weight() {
        let stopwords = Stopwords::new();
        // "oauth" appears in both directory and filename
        let path = Path::new("oauth/oauth_service.rs");
        let terms = extract_path_terms(path, &stopwords, 3);

        // Should only have one "oauth" entry
        let oauth_terms: Vec<_> = terms.iter().filter(|t| t.term == "oauth").collect();
        assert_eq!(oauth_terms.len(), 1);

        // Should have filename weight (higher) and frequency 2
        let oauth = oauth_terms[0];
        assert_eq!(oauth.source, "path:filename");
        assert_eq!(oauth.weight, 4.0);
        assert_eq!(oauth.frequency, 2);
    }
}
