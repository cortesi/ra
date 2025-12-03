//! Command-line interface for the `ra` research assistant tool.

use std::{
    env, fs,
    io::{self, Write},
    path::Path,
    process::ExitCode,
};

use clap::{Parser, Subcommand};
use ra_config::{
    CONFIG_FILENAME, Config, ConfigWarning, discover_config_files, global_config_path,
};
use ra_highlight::{Highlighter, dim, header, rule, subheader};

#[derive(Parser)]
#[command(name = "ra")]
#[command(about = "Research Assistant - Knowledge management for AI agents")]
/// Top-level CLI options.
struct Cli {
    #[command(subcommand)]
    /// Subcommand to execute.
    command: Commands,
}

#[derive(Subcommand)]
/// Supported `ra` subcommands.
enum Commands {
    /// Search and output matching chunks
    Search {
        /// Search queries
        #[arg(required = true)]
        queries: Vec<String>,

        /// Results per query
        #[arg(short = 'n', long, default_value = "5")]
        limit: usize,

        /// Output titles and snippets only
        #[arg(long)]
        list: bool,
    },

    /// Get relevant context for files being worked on
    Context {
        /// Files to analyze
        #[arg(required = true)]
        files: Vec<String>,

        /// Maximum chunks to return
        #[arg(short = 'n', long, default_value = "10")]
        limit: usize,
    },

    /// Retrieve a specific chunk or document by ID
    Get {
        /// Chunk or document ID
        id: String,
    },

    /// Show how ra parses a file
    Inspect {
        /// File to inspect
        file: String,
    },

    /// Initialize ra configuration in current directory
    Init {
        /// Create global ~/.ra.toml instead
        #[arg(long)]
        global: bool,

        /// Overwrite existing configuration file
        #[arg(long)]
        force: bool,
    },

    /// Validate configuration and diagnose issues
    Check,

    /// Force rebuild of search index
    Update,

    /// Show configuration, trees, and index statistics
    Status,

    /// Generate AGENTS.md, CLAUDE.md, GEMINI.md
    Agents {
        /// Print to stdout instead of writing files
        #[arg(long)]
        stdout: bool,

        /// Generate CLAUDE.md
        #[arg(long)]
        claude: bool,

        /// Generate GEMINI.md
        #[arg(long)]
        gemini: bool,

        /// Generate all agent file variants
        #[arg(long)]
        all: bool,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    match cli.command {
        Commands::Search {
            queries,
            limit,
            list,
        } => {
            println!("search: {:?} (limit={}, list={})", queries, limit, list);
        }
        Commands::Context { files, limit } => {
            println!("context: {:?} (limit={})", files, limit);
        }
        Commands::Get { id } => {
            println!("get: {}", id);
        }
        Commands::Inspect { file } => {
            println!("inspect: {}", file);
        }
        Commands::Init { global, force } => {
            return cmd_init(global, force);
        }
        Commands::Check => {
            return cmd_check();
        }
        Commands::Update => {
            println!("update");
        }
        Commands::Status => {
            return cmd_status();
        }
        Commands::Agents {
            stdout,
            claude,
            gemini,
            all,
        } => {
            println!(
                "agents (stdout={}, claude={}, gemini={}, all={})",
                stdout, claude, gemini, all
            );
        }
    }

    ExitCode::SUCCESS
}

/// Default configuration template with commented examples.
const CONFIG_TEMPLATE: &str = include_str!("../templates/config.toml");

/// Global configuration template (simpler, focuses on shared resources).
const GLOBAL_CONFIG_TEMPLATE: &str = include_str!("../templates/config-global.toml");

/// Implements the `ra init` command.
fn cmd_init(global: bool, force: bool) -> ExitCode {
    let config_path = if global {
        match global_config_path() {
            Some(path) => path,
            None => {
                eprintln!("error: could not determine home directory");
                return ExitCode::FAILURE;
            }
        }
    } else {
        let cwd = match env::current_dir() {
            Ok(cwd) => cwd,
            Err(e) => {
                eprintln!("error: could not determine current directory: {e}");
                return ExitCode::FAILURE;
            }
        };
        cwd.join(CONFIG_FILENAME)
    };

    // Check if config already exists
    if config_path.exists() && !force {
        eprintln!(
            "error: configuration file already exists: {}",
            config_path.display()
        );
        eprintln!("use --force to overwrite");
        return ExitCode::FAILURE;
    }

    // Write the config file
    let template = if global {
        GLOBAL_CONFIG_TEMPLATE
    } else {
        CONFIG_TEMPLATE
    };

    if let Err(e) = fs::write(&config_path, template) {
        eprintln!("error: failed to write {}: {e}", config_path.display());
        return ExitCode::FAILURE;
    }

    println!("Created {}", config_path.display());

    // For local configs, try to add .ra/ to .gitignore
    if !global && let Err(e) = update_gitignore(&config_path) {
        eprintln!("warning: could not update .gitignore: {e}");
    }

    ExitCode::SUCCESS
}

/// Adds `.ra/` to `.gitignore` if it exists and doesn't already contain it.
fn update_gitignore(config_path: &Path) -> io::Result<()> {
    let Some(parent) = config_path.parent() else {
        return Ok(());
    };

    let gitignore_path = parent.join(".gitignore");

    // Only update if .gitignore exists
    if !gitignore_path.exists() {
        return Ok(());
    }

    let contents = fs::read_to_string(&gitignore_path)?;

    // Check if .ra/ is already ignored
    let ra_pattern = ".ra/";
    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed == ra_pattern || trimmed == ".ra" {
            return Ok(()); // Already present
        }
    }

    // Append .ra/ to .gitignore
    let mut file = fs::OpenOptions::new().append(true).open(&gitignore_path)?;

    // Add newline if file doesn't end with one
    if !contents.is_empty() && !contents.ends_with('\n') {
        writeln!(file)?;
    }

    writeln!(file, "{ra_pattern}")?;
    println!("Added {ra_pattern} to .gitignore");

    Ok(())
}

/// Implements the `ra status` command.
fn cmd_status() -> ExitCode {
    let cwd = match env::current_dir() {
        Ok(cwd) => cwd,
        Err(e) => {
            eprintln!("error: could not determine current directory: {e}");
            return ExitCode::FAILURE;
        }
    };

    // Discover config files
    let config_files = discover_config_files(&cwd);

    println!("{}", header("Configuration"));
    println!();

    if config_files.is_empty() {
        println!("{}", dim("No configuration files found."));
        println!();
        println!("Run 'ra init' to create a configuration file.");
        return ExitCode::SUCCESS;
    }

    println!("{}", subheader("Config files (highest precedence first):"));
    for path in &config_files {
        println!("  {}", path.display());
    }
    println!();

    // Load merged config
    let config = match Config::load(&cwd) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("error: failed to load configuration: {e}");
            return ExitCode::FAILURE;
        }
    };

    // Show trees
    println!("{}", subheader("Trees:"));
    if config.trees.is_empty() {
        println!("  {}", dim("(none defined)"));
    } else {
        for tree in &config.trees {
            let scope = if tree.is_global { "global" } else { "local" };
            println!(
                "  {} {} -> {}",
                tree.name,
                dim(&format!("({scope})")),
                tree.path.display()
            );
        }
    }
    println!();

    // Show include patterns
    println!("{}", subheader("Include patterns:"));
    if config.includes.is_empty() {
        println!("  {}", dim("(using defaults: **/*.md, **/*.txt)"));
    } else {
        for include in &config.includes {
            println!(
                "  {} {}",
                dim(&format!("[{}]", include.tree)),
                include.pattern
            );
        }
    }
    println!();

    // Show effective settings in TOML format with syntax highlighting
    println!("{}", subheader("Effective settings:"));
    println!("{}", rule(40));
    let highlighter = Highlighter::new();
    print!("{}", highlighter.highlight_toml(&config.settings_to_toml()));
    println!("{}", rule(40));

    ExitCode::SUCCESS
}

/// Exit codes for `ra check`.
mod exit_codes {
    use std::process::ExitCode;

    /// Configuration is valid with no warnings.
    pub const OK: ExitCode = ExitCode::SUCCESS;
    /// Configuration has warnings but is usable.
    pub const WARNINGS: ExitCode = ExitCode::FAILURE;
    /// Configuration has errors and cannot be used.
    pub const ERROR: ExitCode = ExitCode::FAILURE;
}

/// Implements the `ra check` command.
fn cmd_check() -> ExitCode {
    let cwd = match env::current_dir() {
        Ok(cwd) => cwd,
        Err(e) => {
            eprintln!("error: could not determine current directory: {e}");
            return exit_codes::ERROR;
        }
    };

    // Discover config files
    let config_files = discover_config_files(&cwd);

    println!("Checking configuration...");
    println!();

    if config_files.is_empty() {
        println!("No configuration files found.");
        println!();
        println!("Run 'ra init' to create a configuration file.");
        return exit_codes::OK;
    }

    println!("Config files:");
    for path in &config_files {
        println!("  {}", path.display());
    }
    println!();

    // Load configuration
    let config = match Config::load(&cwd) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("error: {e}");
            return exit_codes::ERROR;
        }
    };

    // Validate configuration
    let warnings = config.validate();

    // Report tree status
    println!("Trees:");
    if config.trees.is_empty() {
        println!("  (none defined)");
    } else {
        for tree in &config.trees {
            let status = if tree.path.exists() { "ok" } else { "missing" };
            println!("  {} [{}] -> {}", tree.name, status, tree.path.display());
        }
    }
    println!();

    // Report index status (placeholder for now)
    println!("Index: not yet implemented");
    println!();

    // Report warnings
    if warnings.is_empty() {
        println!("No issues found.");
        return exit_codes::OK;
    }

    println!("Warnings ({}):", warnings.len());
    for warning in &warnings {
        println!("  - {}", format_warning(warning));
    }
    println!();

    // Provide hints for common issues
    print_hints(&warnings);

    exit_codes::WARNINGS
}

/// Formats a warning with helpful context.
fn format_warning(warning: &ConfigWarning) -> String {
    warning.to_string()
}

/// Prints hints for resolving common warnings.
fn print_hints(warnings: &[ConfigWarning]) {
    let mut hints = Vec::new();

    for warning in warnings {
        match warning {
            ConfigWarning::NoTreesDefined => {
                hints.push("Add a [trees] section to define knowledge trees.");
            }
            ConfigWarning::TreePathMissing { .. } => {
                hints.push("Create the missing directory or update the tree path.");
            }
            ConfigWarning::PatternMatchesNothing { .. } => {
                hints.push("Check that the pattern matches files in the tree directory.");
            }
            ConfigWarning::UnreferencedTree { .. } => {
                hints.push("Add [[include]] patterns for unreferenced trees, or remove them.");
            }
            ConfigWarning::UndefinedTreeInPattern { .. } => {
                hints.push("Define the tree in [trees] or fix the include pattern.");
            }
            ConfigWarning::TreePathNotDirectory { .. } => {
                hints.push("Tree paths must point to directories, not files.");
            }
        }
    }

    // Deduplicate hints
    hints.sort();
    hints.dedup();

    if !hints.is_empty() {
        println!("Hints:");
        for hint in hints {
            println!("  - {hint}");
        }
    }
}
