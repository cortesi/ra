//! Shared context for running CLI commands.

use std::{
    env,
    path::{Path, PathBuf},
    process::ExitCode,
};

use ra_config::Config;
use ra_index::{
    IndexStatus, Indexer, Searcher, SilentReporter, detect_index_status, open_searcher,
};

/// Command execution context built once per CLI invocation.
pub struct CommandContext {
    /// Current working directory.
    pub cwd: PathBuf,
    /// Loaded configuration (may be default if no config files found).
    pub config: Config,
    /// Cached searcher opened for this invocation.
    searcher: Option<Searcher>,
}

impl CommandContext {
    /// Loads the current directory and configuration.
    pub fn load() -> Result<Self, ExitCode> {
        let cwd = current_dir_or_failure()?;
        let config = load_config_or_failure(&cwd)?;
        Ok(Self {
            cwd,
            config,
            searcher: None,
        })
    }

    /// Loads only the current directory, skipping configuration parsing.
    ///
    /// Used for commands like `init` or `inspect doc` that should work even when
    /// an existing config file is invalid.
    pub fn load_cwd_only() -> Result<Self, ExitCode> {
        let cwd = current_dir_or_failure()?;
        Ok(Self {
            cwd,
            config: Config::default(),
            searcher: None,
        })
    }

    /// Ensures at least one tree is configured, optionally printing an init hint.
    pub fn require_trees(&self, show_init_hint: bool) -> Result<(), ExitCode> {
        if self.config.trees.is_empty() {
            eprintln!("error: no trees defined in configuration");
            if show_init_hint {
                eprintln!(
                    "Run 'ra init' to create a configuration file, then add tree definitions."
                );
            }
            return Err(ExitCode::FAILURE);
        }
        Ok(())
    }

    /// Returns a mutable searcher, opening or rebuilding the index if needed.
    ///
    /// If `fuzzy_override` is provided, it overrides the config's fuzzy_distance setting.
    pub fn searcher(
        &mut self,
        fuzzy_override: Option<u8>,
        show_init_hint: bool,
    ) -> Result<&mut Searcher, ExitCode> {
        if self.searcher.is_some() {
            return Ok(self.searcher.as_mut().expect("searcher checked"));
        }

        self.require_trees(show_init_hint)?;

        let searcher = ensure_index_fresh(&self.config, fuzzy_override)?;
        self.searcher = Some(searcher);
        Ok(self.searcher.as_mut().expect("searcher just set"))
    }
}

/// Returns the current working directory or exits with a consistent error.
fn current_dir_or_failure() -> Result<PathBuf, ExitCode> {
    env::current_dir().map_err(|e| {
        eprintln!("error: could not determine current directory: {e}");
        ExitCode::FAILURE
    })
}

/// Loads configuration from the provided directory or exits with an error.
fn load_config_or_failure(cwd: &Path) -> Result<Config, ExitCode> {
    Config::load(cwd).map_err(|e| {
        eprintln!("error: failed to load configuration: {e}");
        ExitCode::FAILURE
    })
}

/// Ensures the index is fresh, triggering an update if needed.
fn ensure_index_fresh(config: &Config, fuzzy_override: Option<u8>) -> Result<Searcher, ExitCode> {
    match detect_index_status(config) {
        IndexStatus::Current => open_searcher_or_failure(config, fuzzy_override),
        IndexStatus::Missing | IndexStatus::ConfigChanged => {
            rebuild_index_and_open(config, IndexRefresh::Full, fuzzy_override)
        }
        IndexStatus::Stale => {
            rebuild_index_and_open(config, IndexRefresh::Incremental, fuzzy_override)
        }
    }
}

/// Index refresh modes.
#[derive(Clone, Copy)]
enum IndexRefresh {
    /// Full rebuild of the index.
    Full,
    /// Incremental update of the index.
    Incremental,
}

/// Opens the searcher, exiting with a consistent error on failure.
fn open_searcher_or_failure(
    config: &Config,
    fuzzy_override: Option<u8>,
) -> Result<Searcher, ExitCode> {
    match open_searcher(config, fuzzy_override) {
        Ok(searcher) => Ok(searcher),
        Err(e) => {
            eprintln!("error: failed to open index: {e}");
            Err(ExitCode::FAILURE)
        }
    }
}

/// Rebuilds or updates the index, then opens the searcher.
fn rebuild_index_and_open(
    config: &Config,
    mode: IndexRefresh,
    fuzzy_override: Option<u8>,
) -> Result<Searcher, ExitCode> {
    if matches!(mode, IndexRefresh::Full) {
        eprintln!("Index needs rebuild, updating...");
    }

    let indexer = match Indexer::new(config) {
        Ok(indexer) => indexer,
        Err(e) => {
            eprintln!("error: failed to initialize indexer: {e}");
            return Err(ExitCode::FAILURE);
        }
    };

    let mut reporter = SilentReporter;
    let update = match mode {
        IndexRefresh::Full => indexer.full_reindex(&mut reporter),
        IndexRefresh::Incremental => indexer.incremental_update(&mut reporter),
    };

    if let Err(e) = update {
        eprintln!("error: indexing failed: {e}");
        return Err(ExitCode::FAILURE);
    }

    open_searcher_or_failure(config, fuzzy_override)
}
