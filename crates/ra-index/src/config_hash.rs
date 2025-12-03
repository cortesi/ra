//! Configuration hash computation for index versioning.
//!
//! The index stores a hash of configuration settings that affect indexing.
//! When configuration changes, the hash changes, triggering a full reindex.
//!
//! Settings that affect the hash:
//! - Schema version (internal, bumped when field definitions change)
//! - Stemmer language
//! - Size thresholds (min_chunk_size, max_chunk_size)

use std::hash::{Hash, Hasher};

use ra_config::Config;
use siphasher::sip::SipHasher24;

/// Current schema version. Bump this when index field definitions change.
pub const SCHEMA_VERSION: u32 = 1;

/// Settings that affect indexing and are included in the config hash.
///
/// Changes to any of these settings require a full reindex.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct IndexingConfig {
    /// Schema version - changes when index structure changes.
    pub schema_version: u32,
    /// Stemmer language for text analysis.
    pub stemmer: String,
    /// Maximum chunk size (warning threshold).
    pub max_chunk_size: usize,
}

impl IndexingConfig {
    /// Extracts indexing-relevant settings from a config.
    pub fn from_config(config: &Config) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            stemmer: config.search.stemmer.clone(),
            max_chunk_size: config.settings.max_chunk_size,
        }
    }

    /// Computes a hash of the indexing configuration.
    ///
    /// This hash is stored in the index and compared on subsequent opens
    /// to detect when a full reindex is needed.
    pub fn compute_hash(&self) -> u64 {
        let mut hasher = SipHasher24::new();
        self.hash(&mut hasher);
        hasher.finish()
    }

    /// Computes a hash and returns it as a hex string.
    pub fn hash_string(&self) -> String {
        format!("{:016x}", self.compute_hash())
    }
}

/// Computes a config hash from a Config.
pub fn compute_config_hash(config: &Config) -> String {
    IndexingConfig::from_config(config).hash_string()
}

#[cfg(test)]
mod test {
    use ra_config::{SearchSettings, Settings};

    use super::*;

    #[test]
    fn same_config_produces_same_hash() {
        let config1 = Config::default();
        let config2 = Config::default();

        let hash1 = compute_config_hash(&config1);
        let hash2 = compute_config_hash(&config2);

        assert_eq!(hash1, hash2);
    }

    #[test]
    fn different_stemmer_produces_different_hash() {
        let config1 = Config::default();
        let config2 = Config {
            search: SearchSettings {
                stemmer: "french".to_string(),
            },
            ..Default::default()
        };

        let hash1 = compute_config_hash(&config1);
        let hash2 = compute_config_hash(&config2);

        assert_ne!(hash1, hash2);
    }

    #[test]
    fn different_max_chunk_size_produces_different_hash() {
        let config1 = Config::default();
        let config2 = Config {
            settings: Settings {
                max_chunk_size: 100_000,
                ..Default::default()
            },
            ..Default::default()
        };

        let hash1 = compute_config_hash(&config1);
        let hash2 = compute_config_hash(&config2);

        assert_ne!(hash1, hash2);
    }

    #[test]
    fn hash_is_hex_string() {
        let config = Config::default();
        let hash = compute_config_hash(&config);

        // Should be 16 hex characters (64 bits)
        assert_eq!(hash.len(), 16);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn indexing_config_equality() {
        let ic1 = IndexingConfig {
            schema_version: SCHEMA_VERSION,
            stemmer: "english".to_string(),
            max_chunk_size: 50_000,
        };
        let ic2 = IndexingConfig {
            schema_version: SCHEMA_VERSION,
            stemmer: "english".to_string(),
            max_chunk_size: 50_000,
        };
        let ic3 = IndexingConfig {
            schema_version: SCHEMA_VERSION + 1,
            stemmer: "english".to_string(),
            max_chunk_size: 50_000,
        };

        assert_eq!(ic1, ic2);
        assert_ne!(ic1, ic3);
    }
}
