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
    fn succeeds_with_config() {
        let dir = temp_dir();
        fs::write(dir.path().join(".ra.toml"), "").unwrap();

        ra_with_home(dir.path())
            .current_dir(dir.path())
            .arg("status")
            .assert()
            .success();
    }

    #[test]
    fn succeeds_with_trees() {
        let dir = temp_dir();
        fs::create_dir(dir.path().join("docs")).unwrap();
        fs::write(
            dir.path().join(".ra.toml"),
            "[tree.docs]\npath = \"./docs\"\n",
        )
        .unwrap();

        ra_with_home(dir.path())
            .current_dir(dir.path())
            .arg("status")
            .assert()
            .success();
    }
}

mod check {
    use super::*;

    #[test]
    fn succeeds_without_config() {
        let dir = temp_dir();
        ra_with_home(dir.path())
            .current_dir(dir.path())
            .arg("check")
            .assert()
            .success();
    }

    #[test]
    fn warns_on_empty_trees() {
        let dir = temp_dir();
        fs::write(dir.path().join(".ra.toml"), "# empty config\n").unwrap();

        ra_with_home(dir.path())
            .current_dir(dir.path())
            .arg("check")
            .assert()
            .failure();
    }

    #[test]
    fn succeeds_with_valid_config() {
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
            .arg("check")
            .assert()
            .success();
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
            .arg("check")
            .assert()
            .failure();
    }

    #[test]
    fn warns_on_undefined_tree() {
        // Note: with the new format, undefined trees in patterns are no longer possible
        // since patterns are now part of the tree definition itself.
        // This test now just verifies that an empty config with no trees warns.
        let dir = temp_dir();

        fs::write(dir.path().join(".ra.toml"), "# config with no trees\n").unwrap();

        ra_with_home(dir.path())
            .current_dir(dir.path())
            .arg("check")
            .assert()
            .failure();
    }

    #[test]
    fn fails_on_invalid_toml() {
        let dir = temp_dir();
        fs::write(dir.path().join(".ra.toml"), "[tree\ninvalid").unwrap();

        ra_with_home(dir.path())
            .current_dir(dir.path())
            .arg("check")
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
            .args(["inspect", "test.md"])
            .assert()
            .success();
    }

    #[test]
    fn succeeds_on_text_file() {
        let dir = temp_dir();
        fs::write(dir.path().join("notes.txt"), "Plain text").unwrap();

        ra_with_home(dir.path())
            .current_dir(dir.path())
            .args(["inspect", "notes.txt"])
            .assert()
            .success();
    }

    #[test]
    fn fails_on_nonexistent_file() {
        let dir = temp_dir();

        ra_with_home(dir.path())
            .current_dir(dir.path())
            .args(["inspect", "missing.md"])
            .assert()
            .failure();
    }

    #[test]
    fn fails_on_unsupported_extension() {
        let dir = temp_dir();
        fs::write(dir.path().join("data.json"), "{}").unwrap();

        ra_with_home(dir.path())
            .current_dir(dir.path())
            .args(["inspect", "data.json"])
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
            .args(["inspect", "large.md"])
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

        ra_with_home(dir.path())
            .current_dir(dir.path())
            .args(["search", "--list", "rust"])
            .assert()
            .success()
            // Path is relative to tree root, not including tree path
            .stdout(predicate::str::contains("docs:rust.md"));
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

[context.patterns]
"*.rs" = ["rust"]
"src/auth/**" = ["authentication", "login"]
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
        fs::write(dir.path().join("test.rs"), "fn main() {}").unwrap();

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
        fs::write(dir.path().join("test.rs"), "fn authenticate() {}").unwrap();

        ra_with_home(dir.path())
            .current_dir(dir.path())
            .args(["context", "--list", "test.rs"])
            .assert()
            .success();
    }

    #[test]
    fn json_output_format() {
        let dir = setup_indexed_dir();
        fs::write(dir.path().join("test.rs"), "fn main() {}").unwrap();

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

        // Create multiple source files
        fs::write(dir.path().join("auth.rs"), "fn login() {}").unwrap();
        fs::write(dir.path().join("handlers.rs"), "fn handle() {}").unwrap();

        // Should analyze both and combine signals
        ra_with_home(dir.path())
            .current_dir(dir.path())
            .args(["context", "auth.rs", "handlers.rs"])
            .assert()
            .success();
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
