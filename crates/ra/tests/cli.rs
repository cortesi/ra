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
        assert!(contents.contains("[trees]"));
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
        assert!(contents.contains("[trees]"));
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
        fs::write(dir.path().join(".ra.toml"), "[trees]\n").unwrap();

        ra().current_dir(dir.path())
            .arg("status")
            .assert()
            .success();
    }

    #[test]
    fn succeeds_with_trees() {
        let dir = temp_dir();
        fs::create_dir(dir.path().join("docs")).unwrap();
        fs::write(dir.path().join(".ra.toml"), "[trees]\ndocs = \"./docs\"\n").unwrap();

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
        fs::write(dir.path().join(".ra.toml"), "[trees]\n").unwrap();

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
            r#"[trees]
docs = "./docs"

[[include]]
tree = "docs"
pattern = "**/*.md"
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
            r#"[trees]
docs = "./docs"

[[include]]
tree = "docs"
pattern = "**/*.rs"
"#,
        )
        .unwrap();

        ra().current_dir(dir.path()).arg("check").assert().failure();
    }

    #[test]
    fn warns_on_undefined_tree() {
        let dir = temp_dir();
        fs::create_dir(dir.path().join("docs")).unwrap();

        fs::write(
            dir.path().join(".ra.toml"),
            r#"[trees]
docs = "./docs"

[[include]]
tree = "undefined"
pattern = "**/*.md"
"#,
        )
        .unwrap();

        ra().current_dir(dir.path()).arg("check").assert().failure();
    }

    #[test]
    fn fails_on_invalid_toml() {
        let dir = temp_dir();
        fs::write(dir.path().join(".ra.toml"), "[trees\ninvalid").unwrap();

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
