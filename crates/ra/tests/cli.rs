//! CLI integration tests for ra commands.
//!
//! These tests focus on exit codes and basic behavioral verification,
//! not specific output formatting which may change.

// Integration tests live outside cfg(test) by design
#![allow(clippy::tests_outside_test_module)]

use std::fs;

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

mod init {
    use super::*;

    #[test]
    fn creates_config_file() {
        let dir = temp_dir();

        ra().current_dir(dir.path()).arg("init").assert().success();

        let config_path = dir.path().join(".ra.toml");
        assert!(config_path.exists());

        let contents = fs::read_to_string(&config_path).unwrap();
        assert!(contents.contains("# [tree."));
    }

    #[test]
    fn fails_if_config_exists() {
        let dir = temp_dir();
        fs::write(dir.path().join(".ra.toml"), "existing").unwrap();

        ra().current_dir(dir.path()).arg("init").assert().failure();
    }

    #[test]
    fn force_overwrites_existing() {
        let dir = temp_dir();
        fs::write(dir.path().join(".ra.toml"), "old content").unwrap();

        ra().current_dir(dir.path())
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

        ra().current_dir(dir.path()).arg("init").assert().success();

        let gitignore = fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        assert!(gitignore.contains(".ra/"));
    }

    #[test]
    fn does_not_duplicate_gitignore_entry() {
        let dir = temp_dir();
        fs::write(dir.path().join(".gitignore"), "*.log\n.ra/\n").unwrap();

        ra().current_dir(dir.path()).arg("init").assert().success();

        let gitignore = fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        assert_eq!(gitignore.matches(".ra/").count(), 1);
    }
}

mod status {
    use super::*;

    #[test]
    fn succeeds_without_config() {
        let dir = temp_dir();
        ra().current_dir(dir.path())
            .arg("status")
            .assert()
            .success();
    }

    #[test]
    fn succeeds_with_config() {
        let dir = temp_dir();
        fs::write(dir.path().join(".ra.toml"), "").unwrap();

        ra().current_dir(dir.path())
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

        ra().current_dir(dir.path())
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
        ra().current_dir(dir.path()).arg("check").assert().success();
    }

    #[test]
    fn warns_on_empty_trees() {
        let dir = temp_dir();
        fs::write(dir.path().join(".ra.toml"), "# empty config\n").unwrap();

        ra().current_dir(dir.path()).arg("check").assert().failure();
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

        ra().current_dir(dir.path()).arg("check").assert().success();
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

        ra().current_dir(dir.path()).arg("check").assert().failure();
    }

    #[test]
    fn warns_on_undefined_tree() {
        // Note: with the new format, undefined trees in patterns are no longer possible
        // since patterns are now part of the tree definition itself.
        // This test now just verifies that an empty config with no trees warns.
        let dir = temp_dir();

        fs::write(dir.path().join(".ra.toml"), "# config with no trees\n").unwrap();

        ra().current_dir(dir.path()).arg("check").assert().failure();
    }

    #[test]
    fn fails_on_invalid_toml() {
        let dir = temp_dir();
        fs::write(dir.path().join(".ra.toml"), "[tree\ninvalid").unwrap();

        ra().current_dir(dir.path())
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

        ra().current_dir(dir.path())
            .args(["inspect", "test.md"])
            .assert()
            .success();
    }

    #[test]
    fn succeeds_on_text_file() {
        let dir = temp_dir();
        fs::write(dir.path().join("notes.txt"), "Plain text").unwrap();

        ra().current_dir(dir.path())
            .args(["inspect", "notes.txt"])
            .assert()
            .success();
    }

    #[test]
    fn fails_on_nonexistent_file() {
        let dir = temp_dir();

        ra().current_dir(dir.path())
            .args(["inspect", "missing.md"])
            .assert()
            .failure();
    }

    #[test]
    fn fails_on_unsupported_extension() {
        let dir = temp_dir();
        fs::write(dir.path().join("data.json"), "{}").unwrap();

        ra().current_dir(dir.path())
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

        ra().current_dir(dir.path())
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

        ra().current_dir(dir.path())
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

        ra().current_dir(dir.path())
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

        ra().current_dir(dir.path())
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

        ra().current_dir(dir.path())
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
        ra().current_dir(dir.path())
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
        ra().current_dir(dir.path())
            .arg("update")
            .assert()
            .success();

        // Modify file
        fs::write(docs.join("test.md"), "# Updated content").unwrap();

        // Reindex should succeed
        ra().current_dir(dir.path())
            .arg("update")
            .assert()
            .success()
            .stdout(predicate::str::contains("Indexed 1 files"));
    }
}
