//! Command-line interface for the `ra` research assistant tool.

use std::{
    env, fs,
    io::{self, Write},
    ops::Range,
    path::Path,
    process::ExitCode,
};

use clap::{Parser, Subcommand};
use ra_config::{
    CONFIG_FILENAME, Config, ConfigWarning, discover_config_files, global_config_path,
    global_template, local_template,
};
use ra_document::{DEFAULT_MIN_CHUNK_SIZE, HeadingLevel, parse_file};
use ra_highlight::{
    Highlighter, breadcrumb, dim, format_body, header, indent_content, rule, subheader,
};
use ra_index::{
    IndexStats, IndexStatus, Indexer, ProgressReporter, SearchResult, Searcher, SilentReporter,
    detect_index_status, index_directory, open_searcher,
};
use serde::Serialize;

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

        /// Output only lines containing matches
        #[arg(long)]
        matches: bool,

        /// Output in JSON format
        #[arg(long)]
        json: bool,
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
        /// Chunk or document ID (tree:path#slug or tree:path)
        id: String,

        /// Return full document even if ID specifies a chunk
        #[arg(long)]
        full_document: bool,

        /// Output in JSON format
        #[arg(long)]
        json: bool,
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
            matches,
            json,
        } => {
            return cmd_search(&queries, limit, list, matches, json);
        }
        Commands::Context { files, limit } => {
            println!("context: {:?} (limit={})", files, limit);
        }
        Commands::Get {
            id,
            full_document,
            json,
        } => {
            return cmd_get(&id, full_document, json);
        }
        Commands::Inspect { file } => {
            return cmd_inspect(&file);
        }
        Commands::Init { global, force } => {
            return cmd_init(global, force);
        }
        Commands::Check => {
            return cmd_check();
        }
        Commands::Update => {
            return cmd_update();
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

// JSON output structures matching the spec.
// Internal structs for serialization - documentation via JSON schema.

/// A single search result for JSON output.
#[derive(Serialize)]
struct JsonSearchResult {
    /// Chunk ID.
    id: String,
    /// Tree name.
    tree: String,
    /// File path within tree.
    path: String,
    /// Chunk title.
    title: String,
    /// Breadcrumb hierarchy.
    breadcrumb: String,
    /// Search relevance score.
    score: f32,
    /// Snippet with highlighted terms.
    #[serde(skip_serializing_if = "Option::is_none")]
    snippet: Option<String>,
    /// Full chunk content.
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
}

/// Results for a single query in JSON output.
#[derive(Serialize)]
struct JsonQueryResults {
    /// The search query.
    query: String,
    /// Matching results.
    results: Vec<JsonSearchResult>,
    /// Number of results.
    total_matches: usize,
}

/// Top-level JSON output for search command.
#[derive(Serialize)]
struct JsonSearchOutput {
    /// Results grouped by query.
    queries: Vec<JsonQueryResults>,
}

/// Ensures the index is fresh, triggering an update if needed.
/// Returns the searcher if successful.
fn ensure_index_fresh(config: &Config) -> Result<Searcher, ExitCode> {
    let status = detect_index_status(config);

    match status {
        IndexStatus::Current => {
            // Index is fresh, open and return
            match open_searcher(config) {
                Ok(searcher) => Ok(searcher),
                Err(e) => {
                    eprintln!("error: failed to open index: {e}");
                    Err(ExitCode::FAILURE)
                }
            }
        }
        IndexStatus::Missing | IndexStatus::ConfigChanged => {
            // Need full reindex
            eprintln!("Index needs rebuild, updating...");
            let indexer = match Indexer::new(config) {
                Ok(indexer) => indexer,
                Err(e) => {
                    eprintln!("error: failed to initialize indexer: {e}");
                    return Err(ExitCode::FAILURE);
                }
            };

            let mut reporter = SilentReporter;
            if let Err(e) = indexer.full_reindex(&mut reporter) {
                eprintln!("error: indexing failed: {e}");
                return Err(ExitCode::FAILURE);
            }

            match open_searcher(config) {
                Ok(searcher) => Ok(searcher),
                Err(e) => {
                    eprintln!("error: failed to open index: {e}");
                    Err(ExitCode::FAILURE)
                }
            }
        }
        IndexStatus::Stale => {
            // Need incremental update
            let indexer = match Indexer::new(config) {
                Ok(indexer) => indexer,
                Err(e) => {
                    eprintln!("error: failed to initialize indexer: {e}");
                    return Err(ExitCode::FAILURE);
                }
            };

            let mut reporter = SilentReporter;
            if let Err(e) = indexer.incremental_update(&mut reporter) {
                eprintln!("error: indexing failed: {e}");
                return Err(ExitCode::FAILURE);
            }

            match open_searcher(config) {
                Ok(searcher) => Ok(searcher),
                Err(e) => {
                    eprintln!("error: failed to open index: {e}");
                    Err(ExitCode::FAILURE)
                }
            }
        }
    }
}

/// Formats a search result for full content output.
fn format_result_full(result: &SearchResult) -> String {
    let mut output = String::new();
    output.push_str(&format!("─── {} ───\n", header(&result.id)));
    output.push_str(&format!("{}\n\n", breadcrumb(&result.breadcrumb)));

    // Format body with content styling, indentation, and match highlighting
    let body = format_body(&result.body, &result.match_ranges);
    output.push_str(&body);

    if !result.body.ends_with('\n') {
        output.push('\n');
    }
    output
}

/// Formats a search result for list mode output.
fn format_result_list(result: &SearchResult) -> String {
    let mut output = String::new();
    output.push_str(&format!("─── {} ───\n", header(&result.id)));
    output.push_str(&format!("{}\n", breadcrumb(&result.breadcrumb)));

    // Show stats: match count, content size, score
    let match_count = result.match_ranges.len();
    let content_size = result.body.len();
    let stats = format!(
        "{} matches, {} chars, score {:.2}",
        match_count, content_size, result.score
    );
    output.push_str(&format!("{}\n", dim(&stats)));

    output.push('\n');
    output
}

/// Formats a search result showing only lines that contain matches.
fn format_result_matches(result: &SearchResult) -> String {
    let mut output = String::new();
    output.push_str(&format!("─── {} ───\n", header(&result.id)));
    output.push_str(&format!("{}\n\n", breadcrumb(&result.breadcrumb)));

    if result.match_ranges.is_empty() {
        output.push_str(&dim("   (no matches)\n"));
    } else {
        // Find which lines contain matches and format with content styling + indentation
        let formatted = extract_matching_lines(&result.body, &result.match_ranges);
        output.push_str(&formatted);
        output.push('\n');
    }

    output.push('\n');
    output
}

/// Extracts lines from text that contain at least one match range, with highlighting.
/// Returns formatted lines with content styling, indentation, and match highlighting.
fn extract_matching_lines(body: &str, match_ranges: &[Range<usize>]) -> String {
    use std::{collections::BTreeSet, iter};

    // Build a set of line numbers that contain matches
    let mut matching_line_nums: BTreeSet<usize> = BTreeSet::new();

    // Calculate line boundaries
    let line_starts: Vec<usize> = iter::once(0)
        .chain(body.match_indices('\n').map(|(i, _)| i + 1))
        .collect();

    // For each match range, find which line(s) it overlaps
    for range in match_ranges {
        for (line_num, &start) in line_starts.iter().enumerate() {
            let end = line_starts.get(line_num + 1).copied().unwrap_or(body.len());
            // Check if this range overlaps with this line
            if range.start < end && range.end > start {
                matching_line_nums.insert(line_num);
            }
        }
    }

    // Extract lines and their adjusted match ranges
    let mut lines_with_ranges: Vec<(&str, Vec<Range<usize>>)> = Vec::new();
    for line_num in matching_line_nums {
        let line_start = line_starts[line_num];
        let line_end = line_starts.get(line_num + 1).copied().unwrap_or(body.len());

        // Get the line content (without trailing newline)
        let line_content = body[line_start..line_end].trim_end_matches('\n');

        // Adjust match ranges to be relative to this line and filter to those in this line
        let line_ranges: Vec<Range<usize>> = match_ranges
            .iter()
            .filter_map(|r| {
                if r.start < line_end && r.end > line_start {
                    // Clamp range to line boundaries and make relative
                    let start = r.start.max(line_start) - line_start;
                    let end = r.end.min(line_end) - line_start;
                    // Don't exceed the trimmed content length
                    let trimmed_len = line_content.len();
                    if start < trimmed_len {
                        Some(start..end.min(trimmed_len))
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect();

        lines_with_ranges.push((line_content, line_ranges));
    }

    // Combine lines and format with content styling and match highlighting
    let combined: String = lines_with_ranges
        .iter()
        .map(|(line, _)| *line)
        .collect::<Vec<_>>()
        .join("\n");

    // Collect all ranges, adjusting offsets for the combined string
    let mut combined_ranges: Vec<Range<usize>> = Vec::new();
    let mut offset = 0;
    for (line, ranges) in &lines_with_ranges {
        for r in ranges {
            combined_ranges.push((r.start + offset)..(r.end + offset));
        }
        offset += line.len() + 1; // +1 for newline
    }

    format_body(&combined, &combined_ranges)
}

/// Implements the `ra search` command.
fn cmd_search(queries: &[String], limit: usize, list: bool, matches: bool, json: bool) -> ExitCode {
    let cwd = match env::current_dir() {
        Ok(cwd) => cwd,
        Err(e) => {
            eprintln!("error: could not determine current directory: {e}");
            return ExitCode::FAILURE;
        }
    };

    // Load configuration
    let config = match Config::load(&cwd) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("error: failed to load configuration: {e}");
            return ExitCode::FAILURE;
        }
    };

    if config.trees.is_empty() {
        eprintln!("error: no trees defined in configuration");
        eprintln!("Run 'ra init' to create a configuration file, then add tree definitions.");
        return ExitCode::FAILURE;
    }

    // Ensure index is fresh
    let mut searcher = match ensure_index_fresh(&config) {
        Ok(s) => s,
        Err(code) => return code,
    };

    // Execute search - use search_multi for multiple queries to deduplicate and merge highlights
    let query_refs: Vec<&str> = queries.iter().map(|s| s.as_str()).collect();
    let results = match searcher.search_multi(&query_refs, limit) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: search failed: {e}");
            return ExitCode::FAILURE;
        }
    };

    // Wrap in a single-element vec for compatibility with output logic
    let combined_query = queries.join(" ");
    let all_results: Vec<(String, Vec<SearchResult>)> = vec![(combined_query, results)];

    // Output results
    if json {
        let json_output = JsonSearchOutput {
            queries: all_results
                .iter()
                .map(|(query, results)| JsonQueryResults {
                    query: query.clone(),
                    total_matches: results.len(),
                    results: results
                        .iter()
                        .map(|r| JsonSearchResult {
                            id: r.id.clone(),
                            tree: r.tree.clone(),
                            path: r.path.clone(),
                            title: r.title.clone(),
                            breadcrumb: r.breadcrumb.clone(),
                            score: r.score,
                            snippet: r.snippet.clone(),
                            content: if list {
                                None
                            } else {
                                Some(format!("> {}\n\n{}", r.breadcrumb, r.body))
                            },
                        })
                        .collect(),
                })
                .collect(),
        };
        match serde_json::to_string_pretty(&json_output) {
            Ok(json_str) => println!("{json_str}"),
            Err(e) => {
                eprintln!("error: failed to serialize JSON: {e}");
                return ExitCode::FAILURE;
            }
        }
    } else if list {
        for (_query, results) in &all_results {
            if results.is_empty() {
                println!("{}", dim("No results found."));
            } else {
                for result in results {
                    print!("{}", format_result_list(result));
                }
            }
            println!();
        }
    } else if matches {
        // Matches-only mode: show only lines containing matches
        for (_query, results) in &all_results {
            if results.is_empty() {
                println!("{}", dim("No results found."));
            } else {
                for result in results {
                    print!("{}", format_result_matches(result));
                }
            }
        }
    } else {
        // Full content mode
        for (_query, results) in &all_results {
            if results.is_empty() {
                println!("{}", dim("No results found."));
            } else {
                for result in results {
                    print!("{}", format_result_full(result));
                    println!();
                }
            }
        }
    }

    ExitCode::SUCCESS
}

/// Parses a chunk ID into (tree, path, optional slug).
fn parse_chunk_id(id: &str) -> Option<(String, String, Option<String>)> {
    // Format: tree:path#slug or tree:path
    let colon_pos = id.find(':')?;
    let tree = id[..colon_pos].to_string();
    let rest = &id[colon_pos + 1..];

    if let Some(hash_pos) = rest.find('#') {
        let path = rest[..hash_pos].to_string();
        let slug = rest[hash_pos + 1..].to_string();
        Some((tree, path, Some(slug)))
    } else {
        Some((tree, rest.to_string(), None))
    }
}

/// Implements the `ra get` command.
fn cmd_get(id: &str, full_document: bool, json: bool) -> ExitCode {
    let cwd = match env::current_dir() {
        Ok(cwd) => cwd,
        Err(e) => {
            eprintln!("error: could not determine current directory: {e}");
            return ExitCode::FAILURE;
        }
    };

    // Load configuration
    let config = match Config::load(&cwd) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("error: failed to load configuration: {e}");
            return ExitCode::FAILURE;
        }
    };

    if config.trees.is_empty() {
        eprintln!("error: no trees defined in configuration");
        return ExitCode::FAILURE;
    }

    // Parse the ID
    let Some((tree, path, slug)) = parse_chunk_id(id) else {
        eprintln!("error: invalid ID format: {id}");
        eprintln!("Expected format: tree:path#slug or tree:path");
        return ExitCode::FAILURE;
    };

    // Ensure index is fresh
    let searcher = match ensure_index_fresh(&config) {
        Ok(s) => s,
        Err(code) => return code,
    };

    // Get results
    let results: Vec<SearchResult> = if full_document || slug.is_none() {
        // Get all chunks from the document
        match searcher.get_by_path(&tree, &path) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("error: failed to retrieve document: {e}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        // Get specific chunk by ID
        match searcher.get_by_id(id) {
            Ok(Some(r)) => vec![r],
            Ok(None) => vec![],
            Err(e) => {
                eprintln!("error: failed to retrieve chunk: {e}");
                return ExitCode::FAILURE;
            }
        }
    };

    if results.is_empty() {
        eprintln!("error: not found: {id}");
        return ExitCode::FAILURE;
    }

    // Output results
    if json {
        let json_output = JsonSearchOutput {
            queries: vec![JsonQueryResults {
                query: id.to_string(),
                total_matches: results.len(),
                results: results
                    .iter()
                    .map(|r| JsonSearchResult {
                        id: r.id.clone(),
                        tree: r.tree.clone(),
                        path: r.path.clone(),
                        title: r.title.clone(),
                        breadcrumb: r.breadcrumb.clone(),
                        score: r.score,
                        snippet: None,
                        content: Some(format!("> {}\n\n{}", r.breadcrumb, r.body)),
                    })
                    .collect(),
            }],
        };
        match serde_json::to_string_pretty(&json_output) {
            Ok(json_str) => println!("{json_str}"),
            Err(e) => {
                eprintln!("error: failed to serialize JSON: {e}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        for result in &results {
            print!("{}", format_result_full(result));
            println!();
        }
    }

    ExitCode::SUCCESS
}

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

    // Write the config file (commented out as an example)
    let template = if global {
        global_template()
    } else {
        local_template()
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

    // Show include/exclude patterns per tree
    println!("{}", subheader("Include patterns:"));
    for tree in &config.trees {
        println!("  {}:", dim(&tree.name));
        for pattern in &tree.include {
            println!("    + {pattern}");
        }
        for pattern in &tree.exclude {
            println!("    - {} {}", dim("(exclude)"), pattern);
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

    // Report index status
    let index_status = detect_index_status(&config);
    let index_path = index_directory(&config);
    print!("Index: {}", index_status.description());
    if let Some(path) = &index_path {
        println!(" ({})", path.display());
    } else {
        println!();
    }
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
                hints.push("Add a [tree.<name>] section to define knowledge trees.");
            }
            ConfigWarning::TreePathMissing { .. } => {
                hints.push("Create the missing directory or update the tree path.");
            }
            ConfigWarning::IncludePatternMatchesNothing { .. } => {
                hints.push("Check that the include pattern matches files in the tree directory.");
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

/// Implements the `ra inspect` command.
fn cmd_inspect(file: &str) -> ExitCode {
    let path = Path::new(file);

    // Check if file exists
    if !path.exists() {
        eprintln!("error: file not found: {file}");
        return ExitCode::FAILURE;
    }

    // Determine file type
    let file_type = match path.extension().and_then(|e| e.to_str()) {
        Some("md" | "markdown") => "markdown",
        Some("txt") => "text",
        Some(ext) => {
            eprintln!("error: unsupported file type: .{ext}");
            eprintln!("Supported types: .md, .markdown, .txt");
            return ExitCode::FAILURE;
        }
        None => {
            eprintln!("error: file has no extension");
            eprintln!("Supported types: .md, .markdown, .txt");
            return ExitCode::FAILURE;
        }
    };

    // Parse the file (using "inspect" as placeholder tree name)
    let result = match parse_file(path, "inspect", DEFAULT_MIN_CHUNK_SIZE) {
        Ok(result) => result,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };

    let doc = &result.document;

    // Document header - matches search result format
    println!(
        "─── {} ───",
        header(&format!("{} ({})", path.display(), file_type))
    );
    println!("{}", breadcrumb(&doc.title));
    if !doc.tags.is_empty() {
        println!("{}", dim(&format!("tags: {}", doc.tags.join(", "))));
    }

    // Chunking info
    let chunk_info = match result.chunk_level {
        Some(level) => format!(
            "h{} chunking ({}) → {} chunks",
            heading_level_to_num(level),
            result.chunk_reason,
            doc.chunks.len()
        ),
        None => format!(
            "not chunked ({}) → {} chunks",
            result.chunk_reason,
            doc.chunks.len()
        ),
    };
    println!("{}", dim(&chunk_info));
    println!();

    // Display each chunk in search result format
    for chunk in &doc.chunks {
        let chunk_label = if chunk.is_preamble {
            format!("{} (preamble)", chunk.id)
        } else {
            chunk.id.clone()
        };
        println!("─── {} ───", header(&chunk_label));
        println!("{}", breadcrumb(&chunk.breadcrumb));
        println!("{}", dim(&format!("{} chars", chunk.body.len())));
        println!();

        // Show preview of body with content styling and indentation
        let preview = chunk_preview(&chunk.body, 200);
        println!("{}", indent_content(&preview));
        println!();
    }

    ExitCode::SUCCESS
}

/// Converts HeadingLevel to a number for display.
fn heading_level_to_num(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// Creates a preview of chunk content, truncating if necessary.
fn chunk_preview(content: &str, max_len: usize) -> String {
    // Take first line or first max_len chars, whichever is shorter
    let first_line = content.lines().next().unwrap_or("");
    let preview = if first_line.len() > max_len {
        format!("{}...", &first_line[..max_len])
    } else {
        first_line.to_string()
    };
    preview.replace('\n', " ")
}

/// Progress reporter that prints to the console.
struct ConsoleReporter {
    /// Whether to print verbose progress information.
    verbose: bool,
}

impl ConsoleReporter {
    /// Creates a new console reporter.
    fn new(verbose: bool) -> Self {
        Self { verbose }
    }
}

impl ProgressReporter for ConsoleReporter {
    fn on_file_start(&mut self, path: &Path, current: usize, total: usize) {
        if self.verbose {
            println!("[{}/{}] Indexing {}", current, total, path.display());
        }
    }

    fn on_file_done(&mut self, _path: &Path, _chunks: usize) {
        // Only show in verbose mode, already shown in on_file_start
    }

    fn on_file_error(&mut self, path: &Path, error: &str) {
        eprintln!("warning: failed to index {}: {}", path.display(), error);
    }

    fn on_file_removed(&mut self, path: &Path) {
        if self.verbose {
            println!("Removed: {}", path.display());
        }
    }

    fn on_complete(&mut self, stats: &IndexStats) {
        println!();
        println!(
            "Indexed {} files ({} chunks)",
            stats.files_processed, stats.chunks_indexed
        );
        if stats.files_skipped > 0 {
            println!("{} files skipped due to errors", stats.files_skipped);
        }
        if stats.files_removed > 0 {
            println!("{} files removed from index", stats.files_removed);
        }
    }
}

/// Implements the `ra update` command.
fn cmd_update() -> ExitCode {
    let cwd = match env::current_dir() {
        Ok(cwd) => cwd,
        Err(e) => {
            eprintln!("error: could not determine current directory: {e}");
            return ExitCode::FAILURE;
        }
    };

    // Load configuration
    let config = match Config::load(&cwd) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("error: failed to load configuration: {e}");
            return ExitCode::FAILURE;
        }
    };

    if config.trees.is_empty() {
        eprintln!("error: no trees defined in configuration");
        eprintln!("Run 'ra init' to create a configuration file, then add tree definitions.");
        return ExitCode::FAILURE;
    }

    // Create indexer
    let indexer = match Indexer::new(&config) {
        Ok(indexer) => indexer,
        Err(e) => {
            eprintln!("error: failed to initialize indexer: {e}");
            return ExitCode::FAILURE;
        }
    };

    println!("Rebuilding search index...");
    println!();

    // Run full reindex
    let mut reporter = ConsoleReporter::new(true);
    match indexer.full_reindex(&mut reporter) {
        Ok(_) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: indexing failed: {e}");
            ExitCode::FAILURE
        }
    }
}
