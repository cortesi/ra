//! Index location resolution.
//!
//! Determines where the search index should be stored based on configuration.
//! The index is stored in `.ra/index/` under the directory containing the most
//! specific `.ra.toml`, or in `~/.ra/index/` if only the global config exists.

use std::path::{Path, PathBuf};

use directories::BaseDirs;
use ra_config::{CONFIG_FILENAME, Config, is_global_config};

/// Directory name for ra data (sibling to .ra.toml).
const RA_DIR: &str = ".ra";
/// Subdirectory within .ra for the index.
const INDEX_DIR: &str = "index";

/// Computes the index directory path based on configuration.
///
/// The index location is determined by the most specific (closest to CWD) `.ra.toml` file:
/// - If a local `.ra.toml` exists, the index is stored in `.ra/index/` sibling to that file
/// - If only `~/.ra.toml` exists, the index is stored in `~/.ra/index/`
/// - If no config exists, returns `None`
///
/// # Arguments
/// * `config` - The loaded configuration. `config_root` should point to the directory
///   containing the highest-precedence config file.
pub fn index_directory(config: &Config) -> Option<PathBuf> {
    config.config_root.as_ref().map(|config_root| {
        // `config_root` is normally the directory containing the winning .ra.toml.
        // Be lenient if callers still pass the file path by normalizing to its parent.
        let root_dir = match config_root.file_name() {
            Some(name) if name == CONFIG_FILENAME => {
                config_root.parent().unwrap_or(config_root.as_path())
            }
            _ => config_root.as_path(),
        };

        root_dir.join(RA_DIR).join(INDEX_DIR)
    })
}

/// Returns the global index directory path (`~/.ra/index/`).
///
/// Returns `None` if the home directory cannot be determined.
pub fn global_index_directory() -> Option<PathBuf> {
    BaseDirs::new().map(|dirs| dirs.home_dir().join(RA_DIR).join(INDEX_DIR))
}

/// Returns the path to the manifest file for an index.
///
/// The manifest tracks indexed files and their modification times.
pub fn manifest_path(index_dir: &Path) -> PathBuf {
    index_dir
        .parent()
        .unwrap_or(index_dir)
        .join("manifest.json")
}

/// Returns the path to the config hash file for an index.
///
/// The config hash is used to detect when the index needs rebuilding
/// due to configuration changes.
pub fn config_hash_path(index_dir: &Path) -> PathBuf {
    index_dir.join("config_hash")
}

/// Determines if a config file is local (not the global ~/.ra.toml).
pub fn is_local_config(config_path: &Path) -> bool {
    !is_global_config(config_path)
}

#[cfg(test)]
mod test {
    use std::fs;

    use ra_config::{CONFIG_FILENAME, global_config_path};
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn index_directory_with_local_config() {
        let temp = TempDir::new().unwrap();
        let config_root = temp.path();
        fs::write(config_root.join(CONFIG_FILENAME), "# test config\n").unwrap();

        let config = Config {
            config_root: Some(config_root.to_path_buf()),
            ..Default::default()
        };

        let index_dir = index_directory(&config).unwrap();
        assert_eq!(index_dir, config_root.join(".ra").join("index"));
    }

    #[test]
    fn index_directory_with_nested_config() {
        let temp = TempDir::new().unwrap();
        let project_dir = temp.path().join("project");
        fs::create_dir_all(&project_dir).unwrap();
        fs::write(project_dir.join(CONFIG_FILENAME), "# project config\n").unwrap();

        let config = Config {
            config_root: Some(project_dir.clone()),
            ..Default::default()
        };

        let index_dir = index_directory(&config).unwrap();
        assert_eq!(index_dir, project_dir.join(".ra").join("index"));
    }

    #[test]
    fn index_directory_accepts_config_file_path_for_compatibility() {
        let temp = TempDir::new().unwrap();
        let config_path = temp.path().join(CONFIG_FILENAME);
        fs::write(&config_path, "# legacy config\n").unwrap();

        // Some callers may still pass the config file path. Ensure we normalize it.
        let config = Config {
            config_root: Some(config_path),
            ..Default::default()
        };

        let index_dir = index_directory(&config).unwrap();
        assert_eq!(index_dir, temp.path().join(".ra").join("index"));
    }

    #[test]
    fn index_directory_none_when_no_config() {
        let config = Config::default();
        assert!(index_directory(&config).is_none());
    }

    #[test]
    fn global_index_directory_returns_path() {
        let dir = global_index_directory();
        assert!(dir.is_some());
        let path = dir.unwrap();
        assert!(path.ends_with(".ra/index"));
    }

    #[test]
    fn manifest_path_sibling_to_index() {
        let index_dir = PathBuf::from("/home/user/project/.ra/index");
        let manifest = manifest_path(&index_dir);
        assert_eq!(
            manifest,
            PathBuf::from("/home/user/project/.ra/manifest.json")
        );
    }

    #[test]
    fn config_hash_path_in_index_dir() {
        let index_dir = PathBuf::from("/home/user/project/.ra/index");
        let hash_path = config_hash_path(&index_dir);
        assert_eq!(
            hash_path,
            PathBuf::from("/home/user/project/.ra/index/config_hash")
        );
    }

    #[test]
    fn is_local_config_distinguishes_global() {
        // A non-global path should be local
        let local = PathBuf::from("/home/user/project/.ra.toml");
        assert!(is_local_config(&local));

        // The global config path should not be local
        if let Some(global) = global_config_path() {
            assert!(!is_local_config(&global));
        }
    }
}
