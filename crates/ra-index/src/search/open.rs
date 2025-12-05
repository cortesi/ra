//! Construction and filesystem helpers for `Searcher`.

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

use levenshtein_automata::LevenshteinAutomatonBuilder;
use tantivy::{Index, directory::MmapDirectory};

use super::Searcher;
use crate::{
    IndexError,
    analyzer::{RA_TOKENIZER, build_analyzer_from_name},
    query::QueryCompiler,
    schema::IndexSchema,
};

impl Searcher {
    /// Opens an existing index for searching.
    pub fn open(
        path: &Path,
        language: &str,
        trees: &[ra_config::Tree],
        local_boost: f32,
    ) -> Result<Self, IndexError> {
        if !path.exists() {
            return Err(IndexError::OpenIndex {
                path: path.to_path_buf(),
                message: "index directory does not exist".to_string(),
            });
        }

        let schema = IndexSchema::new();

        let dir = MmapDirectory::open(path).map_err(|e| {
            let err: tantivy::TantivyError = e.into();
            IndexError::open_index(path.to_path_buf(), &err)
        })?;

        let index = Index::open(dir).map_err(|e| IndexError::open_index(path.to_path_buf(), &e))?;

        let analyzer = build_analyzer_from_name(language)?;
        index.tokenizers().register(RA_TOKENIZER, analyzer.clone());

        let fuzzy_distance = super::DEFAULT_FUZZY_DISTANCE;
        let query_compiler = QueryCompiler::new(schema.clone(), language, fuzzy_distance)?;

        let lev_builder = LevenshteinAutomatonBuilder::new(fuzzy_distance, true);

        let tree_is_global: HashMap<String, bool> = trees
            .iter()
            .map(|t| (t.name.clone(), t.is_global))
            .collect();
        let tree_paths: HashMap<String, PathBuf> = trees
            .iter()
            .map(|t| (t.name.clone(), t.path.clone()))
            .collect();

        Ok(Self {
            index,
            schema,
            query_compiler,
            analyzer,
            lev_builder,
            fuzzy_distance,
            tree_is_global,
            tree_paths,
            local_boost,
        })
    }

    /// Opens an existing index for searching using configuration.
    pub fn open_with_config(path: &Path, config: &ra_config::Config) -> Result<Self, IndexError> {
        Self::open(
            path,
            &config.search.stemmer,
            &config.trees,
            config.settings.local_boost,
        )
    }

    /// Reads the full content of a chunk by reading the source file span.
    pub fn read_full_content(
        &self,
        tree: &str,
        path: &str,
        byte_start: u64,
        byte_end: u64,
    ) -> Result<String, IndexError> {
        let tree_root = self
            .tree_paths
            .get(tree)
            .ok_or_else(|| IndexError::Write(format!("unknown tree: {tree}")))?;

        let file_path = tree_root.join(path);
        let content = fs::read_to_string(&file_path).map_err(|e| {
            IndexError::Write(format!("failed to read {}: {e}", file_path.display()))
        })?;

        let start = byte_start as usize;
        let end = byte_end as usize;

        if end > content.len() || start > end {
            return Err(IndexError::Write(format!(
                "invalid byte range [{start}, {end}) for file of {} bytes",
                content.len()
            )));
        }

        Ok(content[start..end].to_string())
    }
}

/// Creates an index directory path and opens it for searching.
pub fn open_searcher(config: &ra_config::Config) -> Result<Searcher, IndexError> {
    let index_dir = crate::index_directory(config).ok_or_else(|| IndexError::OpenIndex {
        path: PathBuf::new(),
        message: "no configuration found".to_string(),
    })?;

    Searcher::open_with_config(&index_dir, config)
}
