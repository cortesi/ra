//! CLI integration tests for ra commands.

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

        ra().current_dir(dir.path())
            .arg("init")
            .assert()
            .success()
            .stdout(predicate::str::contains("Created"));

        let config_path = dir.path().join(".ra.toml");
        assert!(config_path.exists());

        let contents = fs::read_to_string(&config_path).unwrap();
        assert!(contents.contains("[trees]"));
        assert!(contents.contains("[settings]"));
    }

    #[test]
    fn fails_if_config_exists() {
        let dir = temp_dir();
        fs::write(dir.path().join(".ra.toml"), "existing").unwrap();

        ra().current_dir(dir.path())
            .arg("init")
            .assert()
            .failure()
            .stderr(predicate::str::contains("already exists"))
            .stderr(predicate::str::contains("--force"));
    }

    #[test]
    fn force_overwrites_existing() {
        let dir = temp_dir();
        fs::write(dir.path().join(".ra.toml"), "old content").unwrap();

        ra().current_dir(dir.path())
            .args(["init", "--force"])
            .assert()
            .success()
            .stdout(predicate::str::contains("Created"));

        let contents = fs::read_to_string(dir.path().join(".ra.toml")).unwrap();
        assert!(contents.contains("[trees]"));
        assert!(!contents.contains("old content"));
    }

    #[test]
    fn updates_gitignore() {
        let dir = temp_dir();
        fs::write(dir.path().join(".gitignore"), "*.log\n").unwrap();

        ra().current_dir(dir.path())
            .arg("init")
            .assert()
            .success()
            .stdout(predicate::str::contains(".ra/"));

        let gitignore = fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        assert!(gitignore.contains(".ra/"));
    }

    #[test]
    fn does_not_duplicate_gitignore_entry() {
        let dir = temp_dir();
        fs::write(dir.path().join(".gitignore"), "*.log\n.ra/\n").unwrap();

        ra().current_dir(dir.path()).arg("init").assert().success();

        let gitignore = fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        // Count occurrences of .ra/
        let count = gitignore.matches(".ra/").count();
        assert_eq!(count, 1);
    }
}

mod status {
    use super::*;

    #[test]
    fn shows_no_config_message() {
        let dir = temp_dir();

        ra().current_dir(dir.path())
            .arg("status")
            .assert()
            .success()
            .stdout(predicate::str::contains("No configuration files found"))
            .stdout(predicate::str::contains("ra init"));
    }

    #[test]
    fn shows_config_files() {
        let dir = temp_dir();
        fs::write(
            dir.path().join(".ra.toml"),
            r#"
[trees]
"#,
        )
        .unwrap();

        ra().current_dir(dir.path())
            .arg("status")
            .assert()
            .success()
            .stdout(predicate::str::contains("Config files"))
            .stdout(predicate::str::contains(".ra.toml"));
    }

    #[test]
    fn shows_default_settings() {
        let dir = temp_dir();
        fs::write(
            dir.path().join(".ra.toml"),
            r#"
[trees]
"#,
        )
        .unwrap();

        ra().current_dir(dir.path())
            .arg("status")
            .assert()
            .success()
            // Settings are output in TOML format with ANSI color codes interspersed.
            // Check that the key names appear (the values and section headers have escape codes).
            .stdout(predicate::str::contains("default_limit"))
            .stdout(predicate::str::contains("fuzzy"))
            .stdout(predicate::str::contains("stemmer"))
            .stdout(predicate::str::contains("settings"))
            .stdout(predicate::str::contains("search"))
            .stdout(predicate::str::contains("context"));
    }

    #[test]
    fn shows_trees() {
        let dir = temp_dir();
        let docs = dir.path().join("docs");
        fs::create_dir(&docs).unwrap();

        fs::write(
            dir.path().join(".ra.toml"),
            r#"
[trees]
docs = "./docs"
"#,
        )
        .unwrap();

        ra().current_dir(dir.path())
            .arg("status")
            .assert()
            .success()
            .stdout(predicate::str::contains("Trees:"))
            .stdout(predicate::str::contains("docs"));
    }
}

mod check {
    use super::*;

    #[test]
    fn no_config_exits_success() {
        let dir = temp_dir();

        ra().current_dir(dir.path())
            .arg("check")
            .assert()
            .success()
            .stdout(predicate::str::contains("No configuration files found"));
    }

    #[test]
    fn empty_trees_warns() {
        let dir = temp_dir();
        fs::write(
            dir.path().join(".ra.toml"),
            r#"
[trees]
"#,
        )
        .unwrap();

        ra().current_dir(dir.path())
            .arg("check")
            .assert()
            .failure() // exit code 1 for warnings
            .stdout(predicate::str::contains("no trees are defined"));
    }

    #[test]
    fn valid_config_exits_success() {
        let dir = temp_dir();
        let docs = dir.path().join("docs");
        fs::create_dir(&docs).unwrap();
        fs::write(docs.join("readme.md"), "# Test").unwrap();

        fs::write(
            dir.path().join(".ra.toml"),
            r#"
[trees]
docs = "./docs"

[[include]]
tree = "docs"
pattern = "**/*.md"
"#,
        )
        .unwrap();

        ra().current_dir(dir.path())
            .arg("check")
            .assert()
            .success()
            .stdout(predicate::str::contains("No issues found"));
    }

    #[test]
    fn pattern_no_match_warns() {
        let dir = temp_dir();
        let docs = dir.path().join("docs");
        fs::create_dir(&docs).unwrap();
        fs::write(docs.join("readme.txt"), "test").unwrap();

        fs::write(
            dir.path().join(".ra.toml"),
            r#"
[trees]
docs = "./docs"

[[include]]
tree = "docs"
pattern = "**/*.rs"
"#,
        )
        .unwrap();

        ra().current_dir(dir.path())
            .arg("check")
            .assert()
            .failure()
            .stdout(predicate::str::contains("matches no files"));
    }

    #[test]
    fn undefined_tree_warns() {
        let dir = temp_dir();
        let docs = dir.path().join("docs");
        fs::create_dir(&docs).unwrap();

        fs::write(
            dir.path().join(".ra.toml"),
            r#"
[trees]
docs = "./docs"

[[include]]
tree = "undefined"
pattern = "**/*.md"
"#,
        )
        .unwrap();

        ra().current_dir(dir.path())
            .arg("check")
            .assert()
            .failure()
            .stdout(predicate::str::contains("references undefined tree"));
    }

    #[test]
    fn invalid_toml_errors() {
        let dir = temp_dir();
        fs::write(
            dir.path().join(".ra.toml"),
            r#"
[trees
invalid toml
"#,
        )
        .unwrap();

        ra().current_dir(dir.path())
            .arg("check")
            .assert()
            .failure()
            .stderr(predicate::str::contains("error"));
    }

    #[test]
    fn shows_hints() {
        let dir = temp_dir();
        fs::write(
            dir.path().join(".ra.toml"),
            r#"
[trees]
"#,
        )
        .unwrap();

        ra().current_dir(dir.path())
            .arg("check")
            .assert()
            .failure()
            .stdout(predicate::str::contains("Hints:"));
    }
}
