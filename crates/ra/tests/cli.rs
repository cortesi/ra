//! CLI integration tests for ra commands.
//!
//! These tests focus on exit codes and basic behavioral verification,
//! not specific output formatting which may change.

// Integration tests live outside cfg(test) by design
#![allow(clippy::tests_outside_test_module)]

use std::{fs, path::Path};

use assert_cmd::Command;
use predicates::prelude::*;

/// Helper to create a temp directory for tests.
fn temp_dir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

/// Helper to get a ra command.
fn ra() -> Command {
    #[allow(deprecated)]
    Command::cargo_bin("ra").unwrap()
}

/// Helper to run `ra` with HOME isolated to the provided directory.
fn ra_with_home(home: &Path) -> Command {
    let mut cmd = ra();
    cmd.env("HOME", home);
    cmd
}

/// Strips ANSI escape sequences from a string.
fn strip_ansi(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            for c in chars.by_ref() {
                if c == 'm' {
                    break;
                }
            }
        } else {
            output.push(ch);
        }
    }

    output
}

mod init {
    use super::*;

    #[test]
    fn creates_config_file() {
        let dir = temp_dir();

        ra_with_home(dir.path())
            .current_dir(dir.path())
            .arg("init")
            .assert()
            .success();

        let config_path = dir.path().join(".ra.toml");
        assert!(config_path.exists());

        let contents = fs::read_to_string(&config_path).unwrap();
        assert!(contents.contains("# [tree."));
    }

    #[test]
    fn fails_if_config_exists() {
        let dir = temp_dir();
        fs::write(dir.path().join(".ra.toml"), "existing").unwrap();

        ra_with_home(dir.path())
            .current_dir(dir.path())
            .arg("init")
            .assert()
            .failure();
    }

    #[test]
    fn force_overwrites_existing() {
        let dir = temp_dir();
        fs::write(dir.path().join(".ra.toml"), "old content").unwrap();

        ra_with_home(dir.path())
            .current_dir(dir.path())
            .args(["init", "--force"])
            .assert()
            .success();

        let contents = fs::read_to_string(dir.path().join(".ra.toml")).unwrap();
        assert!(contents.contains("# [tree."));
    }

    #[test]
    fn updates_gitignore_when_present() {
        let dir = temp_dir();
        fs::write(dir.path().join(".gitignore"), "*.log\n").unwrap();

        ra_with_home(dir.path())
            .current_dir(dir.path())
            .arg("init")
            .assert()
            .success();

        let gitignore = fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        assert!(gitignore.contains(".ra/"));
    }

    #[test]
    fn does_not_duplicate_gitignore_entry() {
        let dir = temp_dir();
        fs::write(dir.path().join(".gitignore"), "*.log\n.ra/\n").unwrap();

        ra_with_home(dir.path())
            .current_dir(dir.path())
            .arg("init")
            .assert()
            .success();

        let gitignore = fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        assert_eq!(gitignore.matches(".ra/").count(), 1);
    }

    #[test]
    fn prints_config_preview() {
        let dir = temp_dir();

        let assert = ra_with_home(dir.path())
            .current_dir(dir.path())
            .arg("init")
            .assert()
            .success();

        let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
        assert!(
            stdout.contains("Configuration written:"),
            "output did not include preview header: {stdout}"
        );
        assert!(
            stdout.contains("tree.docs"),
            "output did not include template content: {stdout}"
        );
    }
}

mod status {
    use super::*;

    #[test]
    fn succeeds_without_config() {
        let dir = temp_dir();
        ra_with_home(dir.path())
            .current_dir(dir.path())
            .arg("status")
            .assert()
            .success();
    }

    #[test]
    fn succeeds_with_empty_config() {
        let dir = temp_dir();
        fs::write(dir.path().join(".ra.toml"), "").unwrap();

        // Empty config but valid TOML - succeeds but warns about no trees
        ra_with_home(dir.path())
            .current_dir(dir.path())
            .arg("status")
            .assert()
            .failure(); // Fails due to warnings (no trees defined)
    }

    #[test]
    fn succeeds_with_valid_trees() {
        let dir = temp_dir();
        let docs = dir.path().join("docs");
        fs::create_dir(&docs).unwrap();
        fs::write(docs.join("readme.md"), "# Test").unwrap();

        fs::write(
            dir.path().join(".ra.toml"),
            r#"[tree.docs]
path = "./docs"
include = ["**/*.md"]
"#,
        )
        .unwrap();

        ra_with_home(dir.path())
            .current_dir(dir.path())
            .arg("status")
            .assert()
            .success();
    }

    #[test]
    fn warns_on_empty_trees() {
        let dir = temp_dir();
        fs::write(dir.path().join(".ra.toml"), "# empty config\n").unwrap();

        ra_with_home(dir.path())
            .current_dir(dir.path())
            .arg("status")
            .assert()
            .failure();
    }

    #[test]
    fn warns_on_pattern_matching_nothing() {
        let dir = temp_dir();
        let docs = dir.path().join("docs");
        fs::create_dir(&docs).unwrap();
        fs::write(docs.join("readme.txt"), "test").unwrap();

        fs::write(
            dir.path().join(".ra.toml"),
            r#"[tree.docs]
path = "./docs"
include = ["**/*.rs"]
"#,
        )
        .unwrap();

        ra_with_home(dir.path())
            .current_dir(dir.path())
            .arg("status")
            .assert()
            .failure();
    }

    #[test]
    fn fails_on_invalid_toml() {
        let dir = temp_dir();
        fs::write(dir.path().join(".ra.toml"), "[tree\ninvalid").unwrap();

        ra_with_home(dir.path())
            .current_dir(dir.path())
            .arg("status")
            .assert()
            .failure()
            .stderr(predicate::str::contains("error"));
    }
}

mod inspect {
    use super::*;

    #[test]
    fn succeeds_on_markdown_file() {
        let dir = temp_dir();
        fs::write(dir.path().join("test.md"), "# Hello\n\nWorld").unwrap();

        ra_with_home(dir.path())
            .current_dir(dir.path())
            .args(["inspect", "doc", "test.md"])
            .assert()
            .success();
    }

    #[test]
    fn succeeds_on_text_file() {
        let dir = temp_dir();
        fs::write(dir.path().join("notes.txt"), "Plain text").unwrap();

        ra_with_home(dir.path())
            .current_dir(dir.path())
            .args(["inspect", "doc", "notes.txt"])
            .assert()
            .success();
    }

    #[test]
    fn fails_on_nonexistent_file() {
        let dir = temp_dir();

        ra_with_home(dir.path())
            .current_dir(dir.path())
            .args(["inspect", "doc", "missing.md"])
            .assert()
            .failure();
    }

    #[test]
    fn fails_on_unsupported_extension() {
        let dir = temp_dir();
        fs::write(dir.path().join("data.json"), "{}").unwrap();

        ra_with_home(dir.path())
            .current_dir(dir.path())
            .args(["inspect", "doc", "data.json"])
            .assert()
            .failure();
    }

    #[test]
    fn succeeds_on_large_chunked_file() {
        let dir = temp_dir();
        // Large enough to trigger chunking (> 2000 chars)
        let content = format!(
            "# Section 1\n\n{}\n\n# Section 2\n\n{}",
            "x".repeat(1100),
            "y".repeat(1100)
        );
        fs::write(dir.path().join("large.md"), content).unwrap();

        ra_with_home(dir.path())
            .current_dir(dir.path())
            .args(["inspect", "doc", "large.md"])
            .assert()
            .success();
    }
}

mod update {
    use super::*;

    #[test]
    fn fails_without_trees() {
        let dir = temp_dir();
        fs::write(dir.path().join(".ra.toml"), "# empty config\n").unwrap();

        ra_with_home(dir.path())
            .current_dir(dir.path())
            .arg("update")
            .assert()
            .failure()
            .stderr(predicate::str::contains("no trees defined"));
    }

    #[test]
    fn succeeds_with_valid_tree() {
        let dir = temp_dir();
        let docs = dir.path().join("docs");
        fs::create_dir(&docs).unwrap();
        fs::write(docs.join("readme.md"), "# Test\n\nContent").unwrap();

        fs::write(
            dir.path().join(".ra.toml"),
            r#"[tree.docs]
path = "./docs"
include = ["**/*.md"]
"#,
        )
        .unwrap();

        ra_with_home(dir.path())
            .current_dir(dir.path())
            .arg("update")
            .assert()
            .success()
            .stdout(predicate::str::contains("Indexed 1 files"));
    }

    #[test]
    fn creates_index_directory() {
        let dir = temp_dir();
        let docs = dir.path().join("docs");
        fs::create_dir(&docs).unwrap();
        fs::write(docs.join("test.md"), "# Test").unwrap();

        fs::write(
            dir.path().join(".ra.toml"),
            r#"[tree.docs]
path = "./docs"
"#,
        )
        .unwrap();

        ra_with_home(dir.path())
            .current_dir(dir.path())
            .arg("update")
            .assert()
            .success();

        assert!(dir.path().join(".ra").join("index").exists());
        assert!(dir.path().join(".ra").join("manifest.json").exists());
    }

    #[test]
    fn indexes_multiple_files() {
        let dir = temp_dir();
        let docs = dir.path().join("docs");
        fs::create_dir(&docs).unwrap();
        fs::write(docs.join("one.md"), "# One").unwrap();
        fs::write(docs.join("two.md"), "# Two").unwrap();
        fs::write(docs.join("three.txt"), "Three").unwrap();

        fs::write(
            dir.path().join(".ra.toml"),
            r#"[tree.docs]
path = "./docs"
include = ["**/*.md", "**/*.txt"]
"#,
        )
        .unwrap();

        ra_with_home(dir.path())
            .current_dir(dir.path())
            .arg("update")
            .assert()
            .success()
            .stdout(predicate::str::contains("Indexed 3 files"));
    }

    #[test]
    fn reports_parse_errors_gracefully() {
        let dir = temp_dir();
        let docs = dir.path().join("docs");
        fs::create_dir(&docs).unwrap();
        // Valid file
        fs::write(docs.join("valid.md"), "# Valid").unwrap();
        // Invalid UTF-8 file - will fail to parse
        fs::write(docs.join("invalid.md"), vec![0xFF, 0xFE, 0x00, 0x01]).unwrap();

        fs::write(
            dir.path().join(".ra.toml"),
            r#"[tree.docs]
path = "./docs"
"#,
        )
        .unwrap();

        // Should succeed overall but report the error
        ra_with_home(dir.path())
            .current_dir(dir.path())
            .arg("update")
            .assert()
            .success()
            .stderr(predicate::str::contains("warning: failed to index"));
    }

    #[test]
    fn reindex_updates_existing_index() {
        let dir = temp_dir();
        let docs = dir.path().join("docs");
        fs::create_dir(&docs).unwrap();
        fs::write(docs.join("test.md"), "# Original").unwrap();

        fs::write(
            dir.path().join(".ra.toml"),
            r#"[tree.docs]
path = "./docs"
"#,
        )
        .unwrap();

        // First index
        ra_with_home(dir.path())
            .current_dir(dir.path())
            .arg("update")
            .assert()
            .success();

        // Modify file
        fs::write(docs.join("test.md"), "# Updated content").unwrap();

        // Reindex should succeed
        ra_with_home(dir.path())
            .current_dir(dir.path())
            .arg("update")
            .assert()
            .success()
            .stdout(predicate::str::contains("Indexed 1 files"));
    }
}

mod search {
    use super::*;

    fn setup_indexed_dir() -> tempfile::TempDir {
        let dir = temp_dir();
        let docs = dir.path().join("docs");
        fs::create_dir(&docs).unwrap();

        // Create test files with searchable content
        fs::write(
            docs.join("rust.md"),
            "# Rust Programming\n\nRust is a systems programming language focused on safety.",
        )
        .unwrap();
        fs::write(
            docs.join("python.md"),
            "# Python Programming\n\nPython is a dynamic scripting language.",
        )
        .unwrap();

        fs::write(
            dir.path().join(".ra.toml"),
            r#"[tree.docs]
path = "./docs"
"#,
        )
        .unwrap();

        // Index the content
        ra_with_home(dir.path())
            .current_dir(dir.path())
            .arg("update")
            .assert()
            .success();

        dir
    }

    #[test]
    fn fails_without_trees() {
        let dir = temp_dir();
        fs::write(dir.path().join(".ra.toml"), "# empty config\n").unwrap();

        ra_with_home(dir.path())
            .current_dir(dir.path())
            .args(["search", "test"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("no trees defined"));
    }

    #[test]
    fn finds_matching_documents() {
        let dir = setup_indexed_dir();

        ra_with_home(dir.path())
            .current_dir(dir.path())
            .args(["search", "rust"])
            .assert()
            .success()
            .stdout(predicate::str::contains("Rust Programming"));
    }

    #[test]
    fn returns_no_results_message() {
        let dir = setup_indexed_dir();

        ra_with_home(dir.path())
            .current_dir(dir.path())
            .args(["search", "nonexistent"])
            .assert()
            .success()
            .stdout(predicate::str::contains("No results found"));
    }

    #[test]
    fn supports_multiple_queries() {
        let dir = setup_indexed_dir();

        ra_with_home(dir.path())
            .current_dir(dir.path())
            .args(["search", "rust", "python"])
            .assert()
            .success()
            .stdout(predicate::str::contains("Rust"))
            .stdout(predicate::str::contains("Python"));
    }

    #[test]
    fn list_mode_shows_titles() {
        let dir = setup_indexed_dir();

        let assert = ra_with_home(dir.path())
            .current_dir(dir.path())
            .args(["search", "--list", "rust"])
            .assert()
            .success();

        let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
        let plain = strip_ansi(&stdout);

        // Path is relative to tree root, not including tree path
        assert!(
            plain.contains("docs:rust.md"),
            "list output missing path: {plain}"
        );
    }

    #[test]
    fn json_output_format() {
        let dir = setup_indexed_dir();

        ra_with_home(dir.path())
            .current_dir(dir.path())
            .args(["search", "--json", "rust"])
            .assert()
            .success()
            .stdout(predicate::str::contains("\"queries\""))
            .stdout(predicate::str::contains("\"id\""))
            .stdout(predicate::str::contains("\"title\""));
    }

    #[test]
    fn json_includes_match_ranges_and_body() {
        let dir = setup_indexed_dir();

        let output = ra_with_home(dir.path())
            .current_dir(dir.path())
            .args(["search", "--json", "--no-aggregation", "rust"])
            .assert()
            .success();

        let stdout = String::from_utf8_lossy(&output.get_output().stdout);
        let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();

        let result = &json["queries"][0]["results"][0];
        let body = result["body"].as_str().expect("body present");
        let ranges = result["match_ranges"].as_array().expect("ranges present");
        let title_ranges = result["title_match_ranges"]
            .as_array()
            .expect("title ranges present");
        let path_ranges = result["path_match_ranges"]
            .as_array()
            .expect("path ranges present");

        assert!(!ranges.is_empty(), "expected at least one match range");
        assert!(!title_ranges.is_empty(), "expected title match ranges");
        assert!(!path_ranges.is_empty(), "expected path match ranges");

        // Ensure ranges slice valid substrings and contain the matched token "rust".
        let mut contains_rust = false;
        for r in ranges {
            let offset = r["offset"].as_u64().unwrap() as usize;
            let length = r["length"].as_u64().unwrap() as usize;

            assert!(offset + length <= body.len(), "range out of bounds");
            let slice = &body[offset..offset + length];
            if slice.to_lowercase() == "rust" {
                contains_rust = true;
            }
        }

        assert!(
            contains_rust,
            "match ranges should include the 'rust' token"
        );
    }

    #[test]
    fn respects_limit() {
        let dir = temp_dir();
        let docs = dir.path().join("docs");
        fs::create_dir(&docs).unwrap();

        // Create many files with "test" content
        for i in 0..10 {
            fs::write(
                docs.join(format!("file{i}.md")),
                format!("# Test File {i}\n\nThis is test content number {i}."),
            )
            .unwrap();
        }

        fs::write(
            dir.path().join(".ra.toml"),
            r#"[tree.docs]
path = "./docs"
"#,
        )
        .unwrap();

        ra_with_home(dir.path())
            .current_dir(dir.path())
            .arg("update")
            .assert()
            .success();

        // With limit of 3, should only show 3 results in JSON
        let output = ra_with_home(dir.path())
            .current_dir(dir.path())
            .args(["search", "-n", "3", "--json", "test"])
            .assert()
            .success();

        // Parse JSON and check results count
        let stdout = String::from_utf8_lossy(&output.get_output().stdout);
        let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        let results = json["queries"][0]["results"].as_array().unwrap();
        assert!(
            results.len() <= 3,
            "Expected at most 3 results, found {}",
            results.len()
        );
    }

    #[test]
    fn triggers_auto_index_when_missing() {
        let dir = temp_dir();
        let docs = dir.path().join("docs");
        fs::create_dir(&docs).unwrap();
        fs::write(docs.join("test.md"), "# Test\n\nContent here.").unwrap();

        fs::write(
            dir.path().join(".ra.toml"),
            r#"[tree.docs]
path = "./docs"
"#,
        )
        .unwrap();

        // Don't call update first - search should auto-index
        ra_with_home(dir.path())
            .current_dir(dir.path())
            .args(["search", "test"])
            .assert()
            .success()
            .stderr(predicate::str::contains("Index needs rebuild"));
    }

    #[test]
    fn query_syntax_error_shows_context() {
        let dir = setup_indexed_dir();

        // Unclosed quote should show error with context
        ra_with_home(dir.path())
            .current_dir(dir.path())
            .args(["search", "\"unclosed"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("syntax error"))
            .stderr(predicate::str::contains("unclosed quote"));
    }

    #[test]
    fn query_syntax_error_unclosed_paren() {
        let dir = setup_indexed_dir();

        // Unclosed parenthesis
        ra_with_home(dir.path())
            .current_dir(dir.path())
            .args(["search", "(rust"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("syntax error"))
            .stderr(predicate::str::contains("parenthesis"));
    }

    #[test]
    fn query_unknown_field_error() {
        let dir = setup_indexed_dir();

        // Unknown field should show hint about valid fields
        ra_with_home(dir.path())
            .current_dir(dir.path())
            .args(["search", "unknown:value"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("unknown field"))
            .stderr(predicate::str::contains("hint"));
    }

    #[test]
    fn negation_excludes_results() {
        let dir = setup_indexed_dir();

        // Search for programming but exclude python (as single query string)
        ra_with_home(dir.path())
            .current_dir(dir.path())
            .args(["search", "programming -python"])
            .assert()
            .success()
            .stdout(predicate::str::contains("Rust"))
            .stdout(predicate::str::is_match("Python").unwrap().not());
    }

    #[test]
    fn or_finds_either_term() {
        let dir = setup_indexed_dir();

        // Search for rust OR python - should find both
        let output = ra_with_home(dir.path())
            .current_dir(dir.path())
            .args(["search", "--json", "rust OR python"])
            .assert()
            .success();

        let stdout = String::from_utf8_lossy(&output.get_output().stdout);
        let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        let results = json["queries"][0]["results"].as_array().unwrap();
        assert!(
            results.len() >= 2,
            "Expected at least 2 results for OR query"
        );
    }

    #[test]
    fn grouping_with_or() {
        let dir = setup_indexed_dir();

        // (rust OR python) should find both
        ra_with_home(dir.path())
            .current_dir(dir.path())
            .args(["search", "(rust OR python)"])
            .assert()
            .success()
            .stdout(predicate::str::contains("Rust").or(predicate::str::contains("Python")));
    }

    #[test]
    fn field_specific_title_search() {
        let dir = setup_indexed_dir();

        // title:rust should find only the Rust document
        ra_with_home(dir.path())
            .current_dir(dir.path())
            .args(["search", "title:rust"])
            .assert()
            .success()
            .stdout(predicate::str::contains("Rust Programming"));
    }

    #[test]
    fn tree_filter() {
        let dir = setup_indexed_dir();

        // tree:docs should work (it's the only tree) - as single query string
        ra_with_home(dir.path())
            .current_dir(dir.path())
            .args(["search", "tree:docs programming"])
            .assert()
            .success()
            .stdout(predicate::str::contains("Programming"));

        // tree:nonexistent should find nothing - as single query string
        ra_with_home(dir.path())
            .current_dir(dir.path())
            .args(["search", "tree:nonexistent programming"])
            .assert()
            .success()
            .stdout(predicate::str::contains("No results found"));
    }

    #[test]
    fn phrase_search() {
        let dir = setup_indexed_dir();

        // Exact phrase should match
        ra_with_home(dir.path())
            .current_dir(dir.path())
            .args(["search", "\"systems programming\""])
            .assert()
            .success()
            .stdout(predicate::str::contains("Rust"));
    }

    #[test]
    fn complex_query() {
        let dir = setup_indexed_dir();

        // Complex query combining multiple operators (as single query string)
        ra_with_home(dir.path())
            .current_dir(dir.path())
            .args(["search", "title:(rust OR python) -dynamic"])
            .assert()
            .success()
            .stdout(predicate::str::contains("Rust"));
    }

    #[test]
    fn explain_shows_ast() {
        let dir = setup_indexed_dir();

        // --explain should show the parsed AST
        ra_with_home(dir.path())
            .current_dir(dir.path())
            .args(["search", "--explain", "rust OR golang"])
            .assert()
            .success()
            .stdout(predicate::str::contains("Parsed AST"))
            .stdout(predicate::str::contains("Or"));
    }
}

mod context {
    use super::*;

    fn setup_indexed_dir() -> tempfile::TempDir {
        let dir = temp_dir();
        let docs = dir.path().join("docs");
        fs::create_dir(&docs).unwrap();

        // Create documentation about Rust authentication
        fs::write(
            docs.join("auth.md"),
            "# Authentication Guide\n\nHow to handle user login and authentication in Rust.",
        )
        .unwrap();

        // Create documentation about handlers
        fs::write(
            docs.join("handlers.md"),
            "# HTTP Handlers\n\nImplementing request handlers for your API.",
        )
        .unwrap();

        fs::write(
            dir.path().join(".ra.toml"),
            r#"[tree.docs]
path = "./docs"

[[context.rules]]
match = "*.rs"
terms = ["rust"]

[[context.rules]]
match = "src/auth/**"
terms = ["authentication", "login"]
"#,
        )
        .unwrap();

        // Index the content
        ra_with_home(dir.path())
            .current_dir(dir.path())
            .arg("update")
            .assert()
            .success();

        dir
    }

    #[test]
    fn fails_without_trees() {
        let dir = temp_dir();
        fs::write(dir.path().join(".ra.toml"), "# empty config\n").unwrap();
        fs::write(dir.path().join("test.rs"), "fn main() {}").unwrap();

        ra_with_home(dir.path())
            .current_dir(dir.path())
            .args(["context", "test.rs"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("no trees defined"));
    }

    #[test]
    fn fails_on_nonexistent_file() {
        let dir = setup_indexed_dir();

        ra_with_home(dir.path())
            .current_dir(dir.path())
            .args(["context", "nonexistent.rs"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("file not found"));
    }

    #[test]
    fn skips_binary_files_with_warning() {
        let dir = setup_indexed_dir();

        // Create a binary file and a text file
        fs::write(dir.path().join("image.png"), [0x89, 0x50, 0x4E, 0x47]).unwrap();
        fs::write(dir.path().join("code.rs"), "fn main() {}").unwrap();

        // Should warn about binary but still succeed if there's a text file
        ra_with_home(dir.path())
            .current_dir(dir.path())
            .args(["context", "image.png", "code.rs"])
            .assert()
            .success()
            .stderr(predicate::str::contains("skipping binary file"));
    }

    #[test]
    fn fails_when_only_binary_files() {
        let dir = setup_indexed_dir();

        // Create only a binary file
        fs::write(dir.path().join("image.png"), [0x89, 0x50, 0x4E, 0x47]).unwrap();

        ra_with_home(dir.path())
            .current_dir(dir.path())
            .args(["context", "image.png"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("no analyzable files"));
    }

    #[test]
    fn finds_context_for_source_file() {
        let dir = setup_indexed_dir();

        // Create a source file that should match against auth docs
        let src = dir.path().join("src");
        fs::create_dir_all(src.join("auth")).unwrap();
        fs::write(src.join("auth").join("login.rs"), "fn login() {}").unwrap();

        // Should find auth-related documentation
        ra_with_home(dir.path())
            .current_dir(dir.path())
            .args(["context", "src/auth/login.rs"])
            .assert()
            .success();
    }

    #[test]
    fn respects_limit() {
        let dir = setup_indexed_dir();
        // Use terms that exist in the indexed docs (rust, programming, systems, safety)
        fs::write(
            dir.path().join("test.rs"),
            "// Rust systems programming with safety guarantees",
        )
        .unwrap();

        // With limit of 1, should only show 1 result in JSON
        let output = ra_with_home(dir.path())
            .current_dir(dir.path())
            .args(["context", "-n", "1", "--json", "test.rs"])
            .assert()
            .success();

        let stdout = String::from_utf8_lossy(&output.get_output().stdout);
        let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        let results = json["queries"][0]["results"].as_array().unwrap();
        assert!(
            results.len() <= 1,
            "Expected at most 1 result, found {}",
            results.len()
        );
    }

    #[test]
    fn list_mode_output() {
        let dir = setup_indexed_dir();
        // Use terms that exist in the indexed docs
        fs::write(dir.path().join("test.rs"), "// Programming language").unwrap();

        ra_with_home(dir.path())
            .current_dir(dir.path())
            .args(["context", "--list", "test.rs"])
            .assert()
            .success();
    }

    #[test]
    fn json_output_format() {
        let dir = setup_indexed_dir();
        // Use terms that exist in the indexed docs
        fs::write(dir.path().join("test.rs"), "// Rust programming language").unwrap();

        ra_with_home(dir.path())
            .current_dir(dir.path())
            .args(["context", "--json", "test.rs"])
            .assert()
            .success()
            .stdout(predicate::str::contains("\"queries\""));
    }

    #[test]
    fn multiple_files_combined() {
        let dir = setup_indexed_dir();

        // Create multiple source files with terms that exist in the index
        fs::write(dir.path().join("systems.rs"), "// Rust systems programming").unwrap();
        fs::write(
            dir.path().join("scripting.rs"),
            "// Python scripting language",
        )
        .unwrap();

        // Should analyze both and combine signals
        ra_with_home(dir.path())
            .current_dir(dir.path())
            .args(["context", "systems.rs", "scripting.rs"])
            .assert()
            .success();
    }

    #[test]
    fn explain_shows_terms_and_query() {
        let dir = setup_indexed_dir();
        fs::write(
            dir.path().join("test.rs"),
            "// Authentication handler\nfn login() {}",
        )
        .unwrap();

        let output = ra_with_home(dir.path())
            .current_dir(dir.path())
            .args(["context", "--explain", "test.rs"])
            .assert()
            .success();

        let stdout = String::from_utf8_lossy(&output.get_output().stdout);
        let plain = strip_ansi(&stdout);

        // Should show ranked terms section
        assert!(
            plain.contains("Ranked terms"),
            "output should contain ranked terms section: {plain}"
        );
        // Should show generated query
        assert!(
            plain.contains("Generated query"),
            "output should contain generated query section: {plain}"
        );
    }

    #[test]
    fn explain_json_format() {
        let dir = setup_indexed_dir();
        fs::write(
            dir.path().join("test.md"),
            "# Authentication\n\nHandling user logins.",
        )
        .unwrap();

        let output = ra_with_home(dir.path())
            .current_dir(dir.path())
            .args(["context", "--explain", "--json", "test.md"])
            .assert()
            .success();

        let stdout = String::from_utf8_lossy(&output.get_output().stdout);
        let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();

        // Should have files array
        assert!(
            json["files"].is_array(),
            "JSON should have files array: {json}"
        );

        let file = &json["files"][0];
        // Should have file path
        assert!(file["file"].is_string(), "file entry should have file path");
        // Should have terms array
        assert!(
            file["terms"].is_array(),
            "file entry should have terms array"
        );
        // Should have query
        assert!(
            file["query"].is_string() || file["query"].is_null(),
            "file entry should have query field"
        );

        // Check that terms have expected fields
        if let Some(terms) = file["terms"].as_array()
            && !terms.is_empty()
        {
            let term = &terms[0];
            assert!(term["term"].is_string(), "term should have term field");
            assert!(term["source"].is_string(), "term should have source field");
            assert!(term["weight"].is_number(), "term should have weight field");
            assert!(term["score"].is_number(), "term should have score field");
        }
    }

    #[test]
    fn terms_flag_limits_query_terms() {
        let dir = setup_indexed_dir();

        // Create a file with many distinct terms
        fs::write(
            dir.path().join("many_terms.md"),
            "# Alpha Beta Gamma Delta Epsilon Zeta Eta Theta Iota Kappa\n\n\
             Lambda Mu Nu Xi Omicron Pi Rho Sigma Tau Upsilon",
        )
        .unwrap();

        // Use explain with low term limit to verify fewer terms in query
        let output = ra_with_home(dir.path())
            .current_dir(dir.path())
            .args([
                "context",
                "--explain",
                "--json",
                "--terms",
                "3",
                "many_terms.md",
            ])
            .assert()
            .success();

        let stdout = String::from_utf8_lossy(&output.get_output().stdout);
        let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();

        // Check the generated query doesn't have too many terms
        if let Some(query) = json["files"][0]["query"].as_str() {
            // Count boosted terms in the query (each term appears as term^score)
            let boost_count = query.matches('^').count();
            // Should have at most 3 boosted terms (may have fewer if terms merge)
            assert!(
                boost_count <= 3,
                "Expected at most 3 terms in query, found {boost_count}: {query}"
            );
        }
    }

    #[test]
    fn explain_shows_matched_rules() {
        let dir = setup_indexed_dir();

        // Create a Rust file that should match the "*.rs" rule
        fs::write(dir.path().join("test.rs"), "fn main() { login() }").unwrap();

        let output = ra_with_home(dir.path())
            .current_dir(dir.path())
            .args(["context", "--explain", "test.rs"])
            .assert()
            .success();

        let stdout = String::from_utf8_lossy(&output.get_output().stdout);
        let plain = strip_ansi(&stdout);

        // Should show matched rules section
        assert!(
            plain.contains("Matched rules"),
            "output should show matched rules section: {plain}"
        );
        // Should show the term "rust" from the matched rule
        assert!(
            plain.contains("rust"),
            "output should include 'rust' term from matched rule: {plain}"
        );
    }

    #[test]
    fn explain_json_includes_matched_rules() {
        let dir = setup_indexed_dir();

        // Create a file matching the *.rs rule
        fs::write(dir.path().join("handler.rs"), "fn handle() {}").unwrap();

        let output = ra_with_home(dir.path())
            .current_dir(dir.path())
            .args(["context", "--explain", "--json", "handler.rs"])
            .assert()
            .success();

        let stdout = String::from_utf8_lossy(&output.get_output().stdout);
        let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();

        // Should have matched_rules in the file analysis
        let file = &json["files"][0];
        assert!(
            file["matched_rules"].is_object(),
            "file entry should have matched_rules object: {file}"
        );

        let matched_rules = &file["matched_rules"];
        assert!(
            matched_rules["terms"].is_array(),
            "matched_rules should have terms array"
        );
        assert!(
            matched_rules["trees"].is_array(),
            "matched_rules should have trees array"
        );
        assert!(
            matched_rules["include"].is_array(),
            "matched_rules should have include array"
        );
    }

    #[test]
    fn rules_inject_terms_into_query() {
        let dir = setup_indexed_dir();

        // Create an auth file that matches both *.rs and src/auth/** rules
        let src = dir.path().join("src");
        fs::create_dir_all(src.join("auth")).unwrap();
        fs::write(src.join("auth").join("login.rs"), "fn login() {}").unwrap();

        let output = ra_with_home(dir.path())
            .current_dir(dir.path())
            .args(["context", "--explain", "--json", "src/auth/login.rs"])
            .assert()
            .success();

        let stdout = String::from_utf8_lossy(&output.get_output().stdout);
        let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();

        let matched_rules = &json["files"][0]["matched_rules"];
        let terms = matched_rules["terms"].as_array().unwrap();

        // Should have terms from both matching rules
        let term_strs: Vec<&str> = terms.iter().filter_map(|t| t.as_str()).collect();
        assert!(
            term_strs.contains(&"rust"),
            "Should contain 'rust' from *.rs rule: {:?}",
            term_strs
        );
        assert!(
            term_strs.contains(&"authentication") || term_strs.contains(&"login"),
            "Should contain terms from src/auth/** rule: {:?}",
            term_strs
        );
    }

    #[test]
    fn rules_with_tree_filter() {
        let dir = temp_dir();
        let docs = dir.path().join("docs");
        let examples = dir.path().join("examples");
        fs::create_dir(&docs).unwrap();
        fs::create_dir(&examples).unwrap();

        // Create docs in both trees
        fs::write(docs.join("guide.md"), "# Guide\n\nRust programming guide.").unwrap();
        fs::write(examples.join("sample.md"), "# Sample\n\nExample Rust code.").unwrap();

        fs::write(
            dir.path().join(".ra.toml"),
            r#"[tree.docs]
path = "./docs"

[tree.examples]
path = "./examples"

[[context.rules]]
match = "*.rs"
trees = ["docs"]
terms = ["rust"]
"#,
        )
        .unwrap();

        // Index the content
        ra_with_home(dir.path())
            .current_dir(dir.path())
            .arg("update")
            .assert()
            .success();

        // Create a test file
        fs::write(dir.path().join("test.rs"), "fn main() {}").unwrap();

        // Run context with explain to see matched rules
        let output = ra_with_home(dir.path())
            .current_dir(dir.path())
            .args(["context", "--explain", "--json", "test.rs"])
            .assert()
            .success();

        let stdout = String::from_utf8_lossy(&output.get_output().stdout);
        let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();

        // Check that matched_rules has trees restriction
        let matched_rules = &json["files"][0]["matched_rules"];
        let trees = matched_rules["trees"].as_array().unwrap();
        assert!(
            trees.iter().any(|t| t.as_str() == Some("docs")),
            "Should have 'docs' in trees filter: {:?}",
            trees
        );
    }
}

mod get {
    use super::*;

    fn setup_indexed_dir() -> tempfile::TempDir {
        let dir = temp_dir();
        let docs = dir.path().join("docs");
        fs::create_dir(&docs).unwrap();

        // Create a file large enough to trigger chunking (> 2000 chars with multiple h1s)
        // Each section needs substantial content
        let content = format!(
            r#"# Getting Started

This is the introduction to the guide. {}

# Installation

How to install the software. {}

# Configuration

How to configure things. {}
"#,
            "x".repeat(800),
            "y".repeat(800),
            "z".repeat(800)
        );

        fs::write(docs.join("guide.md"), content).unwrap();

        fs::write(
            dir.path().join(".ra.toml"),
            r#"[tree.docs]
path = "./docs"
"#,
        )
        .unwrap();

        // Index the content
        ra_with_home(dir.path())
            .current_dir(dir.path())
            .arg("update")
            .assert()
            .success();

        dir
    }

    #[test]
    fn fails_with_invalid_id_format() {
        let dir = setup_indexed_dir();

        ra_with_home(dir.path())
            .current_dir(dir.path())
            .args(["get", "invalid-no-colon"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("invalid ID format"));
    }

    #[test]
    fn fails_when_not_found() {
        let dir = setup_indexed_dir();

        ra_with_home(dir.path())
            .current_dir(dir.path())
            .args(["get", "docs:nonexistent.md#intro"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("not found"));
    }

    #[test]
    fn retrieves_chunk_by_id() {
        let dir = setup_indexed_dir();

        // The path is relative to the tree root (not including the tree's path)
        ra_with_home(dir.path())
            .current_dir(dir.path())
            .args(["get", "docs:guide.md#installation"])
            .assert()
            .success()
            .stdout(predicate::str::contains("Installation"));
    }

    #[test]
    fn full_document_returns_all_chunks() {
        let dir = setup_indexed_dir();

        let output = ra_with_home(dir.path())
            .current_dir(dir.path())
            .args(["get", "--full-document", "docs:guide.md#installation"])
            .assert()
            .success();

        let stdout = String::from_utf8_lossy(&output.get_output().stdout);
        // Should contain all sections
        assert!(stdout.contains("Getting Started") || stdout.contains("preamble"));
        assert!(stdout.contains("Installation"));
        assert!(stdout.contains("Configuration"));
    }

    #[test]
    fn json_output_format() {
        let dir = setup_indexed_dir();

        ra_with_home(dir.path())
            .current_dir(dir.path())
            .args(["get", "--json", "docs:guide.md#installation"])
            .assert()
            .success()
            .stdout(predicate::str::contains("\"queries\""))
            .stdout(predicate::str::contains("\"id\""))
            .stdout(predicate::str::contains("\"content\""));
    }

    #[test]
    fn path_without_slug_returns_document() {
        let dir = setup_indexed_dir();

        let output = ra_with_home(dir.path())
            .current_dir(dir.path())
            .args(["get", "docs:guide.md"])
            .assert()
            .success();

        let stdout = String::from_utf8_lossy(&output.get_output().stdout);
        // Should return all chunks since no specific slug was given
        assert!(stdout.contains("Installation"));
        assert!(stdout.contains("Configuration"));
    }
}
