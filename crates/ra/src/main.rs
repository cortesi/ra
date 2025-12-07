//! Command-line interface for the `ra` research assistant tool.

use std::{
    collections::{HashMap, HashSet},
    env, fs,
    io::{self, Write},
    ops::Range,
    path::{Path, PathBuf},
    process::ExitCode,
};

use clap::{Parser, Subcommand, error::ErrorKind};
use comfy_table::{Cell, Table, presets::UTF8_FULL_CONDENSED};
use ra_config::{
    CONFIG_FILENAME, CompiledContextRules, Config, ConfigWarning, SearchDefaults,
    discover_config_files, format_path_for_display, global_config_path, global_template,
    local_template,
};
use ra_document::parse_file;
use ra_highlight::{
    Highlighter, breadcrumb, dim, format_body, header, indent_content, subheader, theme, warning,
};
use ra_index::{
    AggregatedSearchResult, ContextAnalysisResult, ContextSearch, ContextWarning, IndexStats,
    IndexStatus, Indexer, MoreLikeThisExplanation, MoreLikeThisParams, ProgressReporter,
    SearchCandidate, SearchParams, Searcher, SilentReporter, detect_index_status, index_directory,
    merge_ranges, open_searcher, parse_query,
};
use serde::Serialize;

// Default values are defined in ra-config::DEFAULT_* constants.
// Help text references these values; update both if defaults change.

/// CLI options for search parameters that can override config defaults.
///
/// Used by both `search` and `context` commands to construct `SearchParams`.
struct SearchParamsOverrides {
    /// Maximum results to return after aggregation.
    limit: Option<usize>,
    /// Maximum candidates to pass through Phase 2 into aggregation.
    max_candidates: Option<usize>,
    /// Score ratio threshold for relevance cutoff.
    cutoff_ratio: Option<f32>,
    /// Sibling ratio threshold for aggregation.
    aggregation_threshold: Option<f32>,
    /// Whether to disable hierarchical aggregation.
    no_aggregation: bool,
    /// Limit results to specific trees.
    trees: Vec<String>,
    /// Verbosity level for match details.
    verbose: u8,
}

impl SearchParamsOverrides {
    /// Builds `SearchParams` by applying CLI overrides to config defaults.
    ///
    /// This ensures consistent handling of search parameters for both `search` and `context`.
    fn build_params<D: SearchDefaults>(&self, defaults: &D) -> SearchParams {
        SearchParams {
            candidate_limit: None, // Let SearchParams derive from limit
            cutoff_ratio: self.cutoff_ratio.unwrap_or_else(|| defaults.cutoff_ratio()),
            max_candidates: self
                .max_candidates
                .unwrap_or_else(|| defaults.max_candidates()),
            aggregation_threshold: self
                .aggregation_threshold
                .unwrap_or_else(|| defaults.aggregation_threshold()),
            disable_aggregation: self.no_aggregation,
            limit: self.limit.unwrap_or_else(|| defaults.limit()),
            trees: self.trees.clone(),
            verbosity: self.verbose,
        }
    }

    /// Builds `SearchParams` with rule-based overrides applied.
    ///
    /// Precedence (highest to lowest): CLI → rule → config → hardcoded defaults.
    fn build_params_with_rule_overrides<D: SearchDefaults>(
        &self,
        defaults: &D,
        rule_overrides: &ra_config::SearchOverrides,
    ) -> SearchParams {
        // Apply rule overrides only where CLI didn't specify a value
        SearchParams {
            candidate_limit: None, // Let SearchParams derive from limit
            cutoff_ratio: self
                .cutoff_ratio
                .or(rule_overrides.cutoff_ratio)
                .unwrap_or_else(|| defaults.cutoff_ratio()),
            max_candidates: self
                .max_candidates
                .or(rule_overrides.max_candidates)
                .unwrap_or_else(|| defaults.max_candidates()),
            limit: self
                .limit
                .or(rule_overrides.limit)
                .unwrap_or_else(|| defaults.limit()),
            aggregation_threshold: self
                .aggregation_threshold
                .or(rule_overrides.aggregation_threshold)
                .unwrap_or_else(|| defaults.aggregation_threshold()),
            disable_aggregation: self.no_aggregation,
            trees: self.trees.clone(),
            verbosity: self.verbose,
        }
    }
}

#[derive(Parser)]
#[command(name = "ra")]
#[command(about = "Research Assistant - Knowledge management for AI agents")]
/// Top-level CLI options.
struct Cli {
    #[command(subcommand)]
    /// Subcommand to execute.
    command: Commands,
}

/// Prints custom help with hierarchical subcommand display.
fn print_hierarchical_help() {
    use clap::CommandFactory;

    let cmd = Cli::command();
    let about = cmd.get_about().map(|s| s.to_string()).unwrap_or_default();

    println!("{about}");
    println!();
    println!("Usage: ra <COMMAND>");
    println!();
    println!("Commands:");

    // Collect commands and their subcommands
    for sub in cmd.get_subcommands() {
        let name = sub.get_name();
        if name == "help" {
            continue; // Print help last
        }

        let about = sub.get_about().map(|s| s.to_string()).unwrap_or_default();
        println!("  {name:10} {about}");

        // Print nested subcommands indented
        for subsub in sub.get_subcommands() {
            let subname = subsub.get_name();
            if subname == "help" {
                continue;
            }
            let subabout = subsub
                .get_about()
                .map(|s| s.to_string())
                .unwrap_or_default();
            println!("    {subname:8} {subabout}");
        }
    }

    println!(
        "  {:<10} Print this message or the help of the given subcommand(s)",
        "help"
    );
    println!();
    println!("Options:");
    println!("  -h, --help  Print help");
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

/// Ensures at least one tree is configured, optionally printing an init hint.
fn ensure_trees(config: &Config, show_init_hint: bool) -> Result<(), ExitCode> {
    if config.trees.is_empty() {
        eprintln!("error: no trees defined in configuration");
        if show_init_hint {
            eprintln!("Run 'ra init' to create a configuration file, then add tree definitions.");
        }
        return Err(ExitCode::FAILURE);
    }
    Ok(())
}

/// Loads configuration from the current directory and checks that trees exist.
fn load_config_with_cwd(show_init_hint: bool) -> Result<(PathBuf, Config), ExitCode> {
    let cwd = current_dir_or_failure()?;
    let config = load_config_or_failure(&cwd)?;
    ensure_trees(&config, show_init_hint)?;
    Ok((cwd, config))
}

#[derive(Subcommand)]
/// Supported `ra` subcommands.
enum Commands {
    /// Search and output matching chunks
    #[command(after_help = "\
QUERY SYNTAX:
  term              Term must appear
  term1 term2       Both terms (implicit AND)
  \"phrase\"          Exact phrase match
  -term             Term must NOT appear
  term1 OR term2    Either term
  (expr)            Grouping

FIELD QUERIES:
  title:term        Search in titles only
  body:term         Search in body text only
  tags:term         Search in tags only
  path:term         Search in file paths only
  tree:name         Filter to specific tree

EXAMPLES:
  ra search rust async
  ra search '\"error handling\"'
  ra search 'rust -deprecated'
  ra search 'rust OR golang'
  ra search 'title:guide (rust OR golang)'
  ra search 'tree:docs authentication'")]
    Search {
        /// Search queries
        #[arg(required = true)]
        queries: Vec<String>,

        /// Maximum results to return after aggregation [default: 10]
        #[arg(short = 'n', long)]
        limit: Option<usize>,

        /// Output titles and snippets only
        #[arg(long)]
        list: bool,

        /// Output only lines containing matches
        #[arg(long)]
        matches: bool,

        /// Output in JSON format
        #[arg(long)]
        json: bool,

        /// Show parsed query AST (for debugging)
        #[arg(long)]
        explain: bool,

        /// Disable hierarchical aggregation
        #[arg(long)]
        no_aggregation: bool,

        /// Maximum candidates to pass through elbow cutoff into aggregation [default: 50]
        #[arg(long)]
        max_candidates: Option<usize>,

        /// Score ratio threshold for relevance cutoff (0.0-1.0) [default: 0.3]
        #[arg(long)]
        cutoff_ratio: Option<f32>,

        /// Sibling ratio threshold for aggregation [default: 0.5]
        #[arg(long)]
        aggregation_threshold: Option<f32>,

        /// Fuzzy matching edit distance (0=exact, 1-2=fuzzy) [default: 1]
        #[arg(short = 'f', long)]
        fuzzy: Option<u8>,

        /// Verbosity level (-v for summary, -vv for full details)
        #[arg(short = 'v', long, action = clap::ArgAction::Count)]
        verbose: u8,

        /// Limit results to specific trees (can be specified multiple times)
        #[arg(short = 't', long = "tree")]
        trees: Vec<String>,
    },

    /// Get relevant context for files being worked on
    Context {
        /// Files to analyze
        #[arg(required = true)]
        files: Vec<String>,

        /// Maximum results to return after aggregation [default: 10]
        #[arg(short = 'n', long)]
        limit: Option<usize>,

        /// Maximum terms to include in the query [default: 50]
        #[arg(long)]
        terms: Option<usize>,

        /// Output titles and snippets only
        #[arg(long)]
        list: bool,

        /// Output only lines containing matches
        #[arg(long)]
        matches: bool,

        /// Output in JSON format
        #[arg(long)]
        json: bool,

        /// Show term analysis and generated query without searching
        #[arg(long)]
        explain: bool,

        /// Disable hierarchical aggregation
        #[arg(long)]
        no_aggregation: bool,

        /// Maximum candidates to pass through elbow cutoff into aggregation [default: 50]
        #[arg(long)]
        max_candidates: Option<usize>,

        /// Score ratio threshold for relevance cutoff (0.0-1.0) [default: 0.3]
        #[arg(long)]
        cutoff_ratio: Option<f32>,

        /// Sibling ratio threshold for aggregation [default: 0.5]
        #[arg(long)]
        aggregation_threshold: Option<f32>,

        /// Fuzzy matching edit distance (0=exact, 1-2=fuzzy) [default: 1]
        #[arg(short = 'f', long)]
        fuzzy: Option<u8>,

        /// Verbosity level (-v for summary, -vv for full details)
        #[arg(short = 'v', long, action = clap::ArgAction::Count)]
        verbose: u8,

        /// Limit results to specific trees (can be specified multiple times)
        #[arg(short = 't', long = "tree")]
        trees: Vec<String>,
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

    /// Find documents similar to a source document or file
    #[command(
        name = "likethis",
        after_help = "\
SOURCE SPECIFICATION:
  Chunk ID      tree:path#slug or tree:path - find similar to an indexed chunk
  File path     ./path/to/file.md - find similar to a file (may not be indexed)

MORELIKETHIS PARAMETERS:
  These parameters control how Tantivy extracts terms from the source document
  to build the similarity query:

  --min-doc-freq     Ignore terms in fewer than N documents (filters rare terms)
  --max-doc-freq     Ignore terms in more than N documents (filters common terms)
  --min-term-freq    Ignore terms appearing less than N times in source
  --max-terms        Maximum query terms to use (default: 25)
  --min-word-len     Ignore words shorter than N characters (default: 3)
  --max-word-len     Ignore words longer than N characters (default: 40)
  --boost            Boost factor for term weights (default: 1.0)

EXAMPLES:
  ra likethis docs:api/auth.md              Find similar to entire document
  ra likethis docs:api/auth.md#overview     Find similar to specific section
  ra likethis ./notes/ideas.md              Find similar to external file
  ra likethis docs:guide.md -t docs         Only search in 'docs' tree
  ra likethis docs:guide.md --max-terms 50  Use more terms for broader matches"
    )]
    LikeThis {
        /// Source: chunk ID (tree:path#slug) or file path
        source: String,

        /// Maximum results to return after aggregation [default: 10]
        #[arg(short = 'n', long)]
        limit: Option<usize>,

        /// Output titles and snippets only
        #[arg(long)]
        list: bool,

        /// Output only lines containing matches
        #[arg(long)]
        matches: bool,

        /// Output in JSON format
        #[arg(long)]
        json: bool,

        /// Show extracted terms and generated query
        #[arg(long)]
        explain: bool,

        /// Disable hierarchical aggregation
        #[arg(long)]
        no_aggregation: bool,

        /// Maximum candidates to pass through elbow cutoff into aggregation [default: 50]
        #[arg(long)]
        max_candidates: Option<usize>,

        /// Score ratio threshold for relevance cutoff (0.0-1.0) [default: 0.3]
        #[arg(long)]
        cutoff_ratio: Option<f32>,

        /// Sibling ratio threshold for aggregation [default: 0.5]
        #[arg(long)]
        aggregation_threshold: Option<f32>,

        /// Verbosity level (-v for summary, -vv for full details)
        #[arg(short = 'v', long, action = clap::ArgAction::Count)]
        verbose: u8,

        /// Limit results to specific trees (can be specified multiple times)
        #[arg(short = 't', long = "tree")]
        trees: Vec<String>,

        // MoreLikeThis-specific parameters
        /// Minimum document frequency for terms
        #[arg(long, default_value = "1")]
        min_doc_freq: u64,

        /// Maximum document frequency for terms
        #[arg(long)]
        max_doc_freq: Option<u64>,

        /// Minimum term frequency in source document
        #[arg(long, default_value = "1")]
        min_term_freq: usize,

        /// Maximum query terms to use
        #[arg(long, default_value = "25")]
        max_terms: usize,

        /// Minimum word length
        #[arg(long, default_value = "3")]
        min_word_len: usize,

        /// Maximum word length
        #[arg(long, default_value = "40")]
        max_word_len: usize,

        /// Boost factor for terms
        #[arg(long, default_value = "1.0")]
        boost: f32,
    },

    /// Inspect documents or context signals
    Inspect {
        /// What to inspect
        #[command(subcommand)]
        what: InspectWhat,
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

    /// Force rebuild of search index
    Update,

    /// Show status and validate configuration
    Status,

    /// Show effective configuration settings
    Config,

    /// List trees, documents, or chunks
    Ls {
        /// Show detailed information.
        #[arg(short = 'l', long)]
        long: bool,

        /// What to list.
        #[command(subcommand)]
        what: LsWhat,
    },

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

#[derive(Clone, Copy, Subcommand)]
/// What to list with `ra ls`.
enum LsWhat {
    /// List all configured trees
    Trees,
    /// List all indexed documents
    Docs,
    /// List all indexed chunks
    Chunks,
}

#[derive(Clone, Subcommand)]
/// What to inspect with `ra inspect`.
enum InspectWhat {
    /// Show how ra parses a document
    Doc {
        /// File to inspect
        file: String,
    },
    /// Show context signals for a file
    Ctx {
        /// File to analyze for context
        file: String,
    },
}

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            // Check if this is a help request for the top-level command
            if e.kind() == ErrorKind::DisplayHelp {
                // Check if we're at the top level (no subcommand specified with --help)
                let args: Vec<_> = env::args().collect();
                if args.len() <= 2 {
                    print_hierarchical_help();
                    return ExitCode::SUCCESS;
                }
            }
            // For all other cases (including subcommand help), let clap handle it
            e.exit();
        }
    };

    match cli.command {
        Commands::Search {
            queries,
            limit,
            list,
            matches,
            json,
            explain,
            no_aggregation,
            max_candidates,
            cutoff_ratio,
            aggregation_threshold,
            fuzzy,
            verbose,
            trees,
        } => {
            let (_, config) = match load_config_with_cwd(false) {
                Ok(res) => res,
                Err(code) => return code,
            };

            let overrides = SearchParamsOverrides {
                limit,
                max_candidates,
                cutoff_ratio,
                aggregation_threshold,
                no_aggregation,
                trees,
                verbose,
            };
            let params = overrides.build_params(&config.search);
            return cmd_search(
                &queries, &params, list, matches, json, explain, verbose, fuzzy,
            );
        }
        Commands::Context {
            files,
            limit,
            terms,
            list,
            matches,
            json,
            explain,
            no_aggregation,
            max_candidates,
            cutoff_ratio,
            aggregation_threshold,
            fuzzy,
            verbose,
            trees,
        } => {
            return cmd_context(
                &files,
                limit,
                terms,
                list,
                matches,
                json,
                explain,
                no_aggregation,
                max_candidates,
                cutoff_ratio,
                aggregation_threshold,
                fuzzy,
                verbose,
                &trees,
            );
        }
        Commands::Get {
            id,
            full_document,
            json,
        } => {
            return cmd_get(&id, full_document, json);
        }
        Commands::LikeThis {
            source,
            limit,
            list,
            matches,
            json,
            explain,
            no_aggregation,
            max_candidates,
            cutoff_ratio,
            aggregation_threshold,
            verbose,
            trees,
            min_doc_freq,
            max_doc_freq,
            min_term_freq,
            max_terms,
            min_word_len,
            max_word_len,
            boost,
        } => {
            return cmd_likethis(
                &source,
                limit,
                list,
                matches,
                json,
                explain,
                no_aggregation,
                max_candidates,
                cutoff_ratio,
                aggregation_threshold,
                verbose,
                &trees,
                min_doc_freq,
                max_doc_freq,
                min_term_freq,
                max_terms,
                min_word_len,
                max_word_len,
                boost,
            );
        }
        Commands::Inspect { what } => {
            return cmd_inspect(what);
        }
        Commands::Init { global, force } => {
            return cmd_init(global, force);
        }
        Commands::Update => {
            return cmd_update();
        }
        Commands::Status => {
            return cmd_status();
        }
        Commands::Config => {
            return cmd_config();
        }
        Commands::Ls { long, what } => {
            return cmd_ls(what, long);
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
    /// Raw chunk body text (no formatting).
    #[serde(skip_serializing_if = "Option::is_none")]
    body: Option<String>,
    /// Full chunk content (legacy, includes breadcrumb prefix when present).
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    /// Match highlight ranges relative to `body` offsets (byte offset, length).
    #[serde(skip_serializing_if = "Option::is_none")]
    match_ranges: Option<Vec<JsonMatchRange>>,
    /// Title highlight ranges (byte offset, length).
    #[serde(skip_serializing_if = "Option::is_none")]
    title_match_ranges: Option<Vec<JsonMatchRange>>,
    /// Path highlight ranges (byte offset, length).
    #[serde(skip_serializing_if = "Option::is_none")]
    path_match_ranges: Option<Vec<JsonMatchRange>>,
}

/// Highlight range for JSON output.
#[derive(Serialize)]
struct JsonMatchRange {
    /// Byte offset into the body text.
    offset: usize,
    /// Length in bytes of the highlighted span.
    length: usize,
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

/// Converts byte ranges to JSON match ranges.
fn json_match_ranges(ranges: &[Range<usize>]) -> Vec<JsonMatchRange> {
    ranges
        .iter()
        .map(|range| JsonMatchRange {
            offset: range.start,
            length: range.end - range.start,
        })
        .collect()
}

/// Builds a JSON result from an aggregated search result.
fn json_from_aggregated_result(result: &AggregatedSearchResult, list: bool) -> JsonSearchResult {
    let constituents_count = result.constituents().map(|c| c.len()).unwrap_or(0);
    let match_ranges = match result {
        AggregatedSearchResult::Single(c) => Some(json_match_ranges(&c.match_ranges)),
        AggregatedSearchResult::Aggregated { .. } => None,
    };

    let c = result.candidate();
    let content_field = if list {
        None
    } else {
        Some(format!("> {}\n\n{}", c.breadcrumb(), c.body))
    };

    JsonSearchResult {
        id: c.id.clone(),
        tree: c.tree.clone(),
        path: c.path.clone(),
        title: c.title().to_string(),
        breadcrumb: c.breadcrumb(),
        score: c.score,
        snippet: if result.is_aggregated() {
            Some(format!("[Aggregated: {} matches]", constituents_count))
        } else {
            None
        },
        body: Some(c.body.clone()),
        content: content_field,
        match_ranges,
        title_match_ranges: Some(json_match_ranges(&c.hierarchy_match_ranges)),
        path_match_ranges: Some(json_match_ranges(&c.path_match_ranges)),
    }
}

/// Builds a JSON result from a single search result.
/// Rendering style for aggregated search results.
#[derive(Clone, Copy)]
enum DisplayMode {
    /// Full content output.
    Full,
    /// Header-only listing output.
    List,
    /// Matches-only line output.
    Matches,
}

/// Retrieves the full body for a search result, falling back to the indexed body.
fn read_full_body(result: &AggregatedSearchResult, searcher: &Searcher) -> String {
    let c = result.candidate();
    searcher
        .read_full_content(&c.tree, &c.path, c.byte_start, c.byte_end)
        .unwrap_or_else(|_| c.body.clone())
}

/// Ensures the index is fresh, triggering an update if needed.
/// Returns the searcher if successful.
///
/// If `fuzzy_override` is provided, it overrides the config's fuzzy_distance setting.
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

/// Formats match details for verbose output.
///
/// - verbosity 1 (-v): Shows a summary of matched terms and stemming
/// - verbosity 2 (-vv): Full match details including field breakdown and term frequencies
/// - verbosity 3+ (-vvv): Adds raw Tantivy score explanation
fn format_match_details(result: &AggregatedSearchResult, verbosity: u8) -> String {
    let mut output = String::new();

    // Get match details from the result (or first constituent for aggregated)
    let details = result.match_details();

    if let Some(details) = details {
        // Collect all terms that actually matched in this document
        let matched_in_doc: HashSet<&str> = details
            .field_matches
            .values()
            .flat_map(|fm| fm.term_frequencies.keys().map(String::as_str))
            .collect();

        // Always show matched terms at verbosity >= 1
        if verbosity >= 1 {
            // Show each query term that actually matched in this document
            if !details.original_terms.is_empty() {
                let mut terms_output = String::new();
                for (orig, stemmed) in details
                    .original_terms
                    .iter()
                    .zip(details.stemmed_terms.iter())
                {
                    // Check if this term (stemmed or fuzzy variants) matched in the document
                    let stemmed_matched = matched_in_doc.contains(stemmed.as_str());
                    let fuzzy_matches: Vec<&String> =
                        if let Some(matches) = details.term_mappings.get(stemmed) {
                            matches
                                .iter()
                                .filter(|m| *m != stemmed && matched_in_doc.contains(m.as_str()))
                                .collect()
                        } else {
                            Vec::new()
                        };

                    // Only show terms that actually matched
                    if stemmed_matched || !fuzzy_matches.is_empty() {
                        terms_output.push_str(&format!("  {}\n", orig));

                        // Show stemming if different from original
                        if orig != stemmed {
                            terms_output.push_str(&format!("    {} {}\n", dim("stem:"), stemmed));
                        }

                        // Show fuzzy matches
                        if !fuzzy_matches.is_empty() {
                            terms_output.push_str(&format!(
                                "    {} {}\n",
                                dim("fuzzy:"),
                                fuzzy_matches
                                    .iter()
                                    .map(|s| s.as_str())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            ));
                        }
                    }
                }
                if !terms_output.is_empty() {
                    output.push_str(&format!("{}\n", dim("terms:")));
                    output.push_str(&terms_output);
                }
            }
        }

        // Show full details at verbosity >= 2
        if verbosity >= 2 {
            // Show score breakdown
            output.push_str(&format!(
                "{} base={:.2}, boost={:.2}\n",
                dim("scores:"),
                details.base_score,
                details.local_boost
            ));

            // Show per-field scores
            if !details.field_scores.is_empty() {
                let mut field_scores: Vec<String> = details
                    .field_scores
                    .iter()
                    .filter(|(_, score)| **score > 0.0)
                    .map(|(field, score)| format!("{field}={score:.2}"))
                    .collect();
                field_scores.sort();
                if !field_scores.is_empty() {
                    output.push_str(&format!(
                        "  {} {}\n",
                        dim("fields:"),
                        field_scores.join(", ")
                    ));
                }
            }

            // Show per-field match details with term frequencies
            if !details.field_matches.is_empty() {
                for (field, field_match) in &details.field_matches {
                    if field_match.term_frequencies.is_empty() {
                        continue;
                    }
                    // Show each term with its frequency
                    let term_info: Vec<String> = field_match
                        .term_frequencies
                        .iter()
                        .map(|(term, freq)| format!("{term} x {freq}"))
                        .collect();
                    output.push_str(&format!(
                        "  {} {}\n",
                        dim(&format!("{field}:")),
                        term_info.join(", ")
                    ));
                }
            }
        }

        // Show raw Tantivy explanation only at verbosity >= 3 (-vvv)
        if verbosity >= 3
            && let Some(explanation) = &details.score_explanation
        {
            output.push_str(&format!("{}\n", dim("tantivy explanation:")));
            for line in explanation.lines() {
                output.push_str(&format!("  {}\n", dim(line)));
            }
        }
    } else if verbosity >= 1 {
        output.push_str(&format!("{}\n", dim("(match details not collected)")));
    }

    output
}

/// Highlights text with a base style, reapplying the base styling after each match.
fn highlight_with_base(
    text: &str,
    ranges: &[Range<usize>],
    base_prefix: &str,
    base_suffix: &str,
) -> String {
    if ranges.is_empty() {
        return format!("{base_prefix}{text}{base_suffix}");
    }

    let match_prefix = theme::MATCH.prefix();
    let match_suffix = theme::MATCH.suffix();
    let mut output = String::new();
    output.push_str(base_prefix);

    let mut cursor = 0;
    for range in ranges {
        if range.start > cursor {
            output.push_str(&text[cursor..range.start]);
        }
        output.push_str(&match_prefix);
        output.push_str(&text[range.start..range.end]);
        output.push_str(match_suffix);
        output.push_str(base_prefix);
        cursor = range.end;
    }

    if cursor < text.len() {
        output.push_str(&text[cursor..]);
    }
    output.push_str(base_suffix);
    output
}

/// Highlights the path portion inside an id (tree:path#slug) using path ranges.
fn highlight_id_with_path(id: &str, path_ranges: &[Range<usize>]) -> String {
    let Some(colon) = id.find(':') else {
        let header_prefix = theme::HEADER.prefix();
        return highlight_with_base(id, &[], &header_prefix, theme::HEADER.suffix());
    };
    let path_start = colon + 1;
    let path_end = id.find('#').unwrap_or(id.len());
    let path_len = path_end.saturating_sub(path_start);

    let header_prefix = theme::HEADER.prefix();

    let shifted: Vec<Range<usize>> = path_ranges
        .iter()
        .filter_map(|r| {
            if r.end <= path_len {
                Some((r.start + path_start)..(r.end + path_start))
            } else {
                None
            }
        })
        .collect();

    highlight_with_base(id, &shifted, &header_prefix, theme::HEADER.suffix())
}

/// Highlights the trailing title segment inside a breadcrumb using title ranges.
fn highlight_breadcrumb_title(
    breadcrumb: &str,
    title: &str,
    title_ranges: &[Range<usize>],
) -> String {
    let breadcrumb_prefix = theme::BREADCRUMB.prefix();
    if title_ranges.is_empty() || title.is_empty() {
        return highlight_with_base(
            breadcrumb,
            &[],
            &breadcrumb_prefix,
            theme::BREADCRUMB.suffix(),
        );
    }

    if let Some(pos) = breadcrumb.rfind(title) {
        let shifted: Vec<Range<usize>> = title_ranges
            .iter()
            .map(|r| (r.start + pos)..(r.end + pos))
            .collect();
        highlight_with_base(
            breadcrumb,
            &shifted,
            &breadcrumb_prefix,
            theme::BREADCRUMB.suffix(),
        )
    } else {
        highlight_with_base(
            breadcrumb,
            &[],
            &breadcrumb_prefix,
            theme::BREADCRUMB.suffix(),
        )
    }
}

/// Sorts and merges overlapping or adjacent ranges.
/// Computes highlight ranges for a search result, mapping child matches into the parent body.
fn aggregated_match_ranges(result: &AggregatedSearchResult, full_body: &str) -> Vec<Range<usize>> {
    match result {
        AggregatedSearchResult::Single(candidate) => candidate.match_ranges.clone(),
        AggregatedSearchResult::Aggregated {
            parent,
            constituents,
        } => {
            let parent_start = parent.byte_start as usize;
            let mut ranges = Vec::new();
            for child in constituents {
                let shift = child.byte_start as usize;
                if shift < parent_start {
                    continue;
                }
                let offset = shift - parent_start;
                for r in &child.match_ranges {
                    let start = offset + r.start;
                    let end = offset + r.end;
                    if end <= full_body.len() && start < end {
                        ranges.push(start..end);
                    }
                }
            }
            merge_ranges(ranges, Vec::new())
        }
    }
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
#[allow(clippy::too_many_arguments)]
fn cmd_search(
    queries: &[String],
    params: &SearchParams,
    list: bool,
    matches: bool,
    json: bool,
    explain: bool,
    verbose: u8,
    fuzzy: Option<u8>,
) -> ExitCode {
    // Handle --explain mode: parse and display AST without executing search
    if explain {
        let combined_query = queries.join(" ");
        println!("{}", subheader("Query:"));
        println!("   {combined_query}");
        println!();

        match parse_query(&combined_query) {
            Ok(Some(expr)) => {
                println!("{}", subheader("Parsed AST:"));
                // Indent each line of the AST output
                for line in expr.to_string().lines() {
                    println!("   {line}");
                }
                println!();

                // Show search parameters
                println!("{}", subheader("Search Parameters:"));
                println!(
                    "   Phase 1: candidate_limit = {}",
                    params.effective_candidate_limit()
                );
                println!("   Phase 2: cutoff_ratio = {}", params.cutoff_ratio);
                println!("   Phase 2: max_candidates = {}", params.max_candidates);
                println!(
                    "   Phase 3: aggregation_threshold = {}",
                    params.aggregation_threshold
                );
                println!("   Phase 4: limit = {}", params.limit);
                println!(
                    "   Aggregation = {}",
                    if params.disable_aggregation {
                        "disabled"
                    } else {
                        "enabled"
                    }
                );
            }
            Ok(None) => {
                println!("{}", dim("(empty query)"));
            }
            Err(e) => {
                eprintln!("error: {e}");
                return ExitCode::FAILURE;
            }
        }
        return ExitCode::SUCCESS;
    }

    let (_, config) = match load_config_with_cwd(true) {
        Ok(res) => res,
        Err(code) => return code,
    };

    // Ensure index is fresh
    let mut searcher = match ensure_index_fresh(&config, fuzzy) {
        Ok(s) => s,
        Err(code) => return code,
    };

    // Execute search using three-phase algorithm
    // Multiple query arguments are combined with OR for backwards compatibility
    let combined_query = if queries.len() == 1 {
        queries[0].clone()
    } else {
        queries
            .iter()
            .map(|q| format!("({q})"))
            .collect::<Vec<_>>()
            .join(" OR ")
    };
    let results = match searcher.search_aggregated(&combined_query, params) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: search failed: {e}");
            return ExitCode::FAILURE;
        }
    };

    // Output results
    output_aggregated_results(
        &results,
        &combined_query,
        list,
        matches,
        json,
        verbose,
        &searcher,
    )
}

/// Outputs aggregated search results in various formats.
fn output_aggregated_results(
    results: &[AggregatedSearchResult],
    query: &str,
    list: bool,
    matches: bool,
    json: bool,
    verbose: u8,
    searcher: &Searcher,
) -> ExitCode {
    if json {
        let json_output = JsonSearchOutput {
            queries: vec![JsonQueryResults {
                query: query.to_string(),
                total_matches: results.len(),
                results: results
                    .iter()
                    .map(|r| json_from_aggregated_result(r, list))
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
    } else if list {
        return output_text_results(results, verbose, searcher, DisplayMode::List);
    } else if matches {
        return output_text_results(results, verbose, searcher, DisplayMode::Matches);
    } else {
        return output_text_results(results, verbose, searcher, DisplayMode::Full);
    }

    ExitCode::SUCCESS
}

/// Renders non-JSON search results for the selected display mode.
fn output_text_results(
    results: &[AggregatedSearchResult],
    verbose: u8,
    searcher: &Searcher,
    mode: DisplayMode,
) -> ExitCode {
    if results.is_empty() {
        println!("{}", dim("No results found."));
        if matches!(mode, DisplayMode::List) {
            println!();
        }
        return ExitCode::SUCCESS;
    }

    let collect_totals = verbose > 0 && !matches!(mode, DisplayMode::Matches);
    let mut total_words = 0;
    let mut total_chars = 0;

    for result in results {
        let full_body = read_full_body(result, searcher);
        if collect_totals {
            total_words += full_body.split_whitespace().count();
            total_chars += full_body.len();
        }

        let formatted = format_aggregated_result(result, verbose, &full_body, mode);
        match mode {
            DisplayMode::Full => {
                print!("{formatted}");
                println!();
            }
            _ => print!("{formatted}"),
        }
    }

    if collect_totals {
        println!(
            "{}",
            dim(&format!(
                "─── Total: {} results, {} words, {} chars ───",
                results.len(),
                total_words,
                total_chars
            ))
        );
    }

    if matches!(mode, DisplayMode::List) {
        println!();
    }

    ExitCode::SUCCESS
}

/// Formats an aggregated search result for the given display mode.
fn format_aggregated_result(
    result: &AggregatedSearchResult,
    verbose: u8,
    full_body: &str,
    mode: DisplayMode,
) -> String {
    let mut output = String::new();

    let c = result.candidate();
    let header_id = highlight_id_with_path(&c.id, &c.path_match_ranges);
    if verbose > 0 && result.is_aggregated() {
        let count = result.constituents().unwrap().len();
        output.push_str(&format!(
            "─── {} [aggregated: {} matches] ───\n",
            header_id, count
        ));
    } else {
        output.push_str(&format!("─── {} ───\n", header_id));
    }

    let breadcrumb_line =
        highlight_breadcrumb_title(&c.breadcrumb(), c.title(), &c.hierarchy_match_ranges);
    if matches!(mode, DisplayMode::Matches) {
        output.push_str(&format!("{breadcrumb_line}\n\n"));
    } else {
        output.push_str(&format!("{breadcrumb_line}\n"));
    }

    if verbose > 0 && !matches!(mode, DisplayMode::Matches) {
        let word_count = full_body.split_whitespace().count();
        let stats = format!(
            "{} words, {} chars, score {:.2}",
            word_count,
            full_body.len(),
            c.score
        );
        output.push_str(&format!("{}\n", dim(&stats)));
    }

    if verbose > 0 {
        output.push_str(&format_match_details(result, verbose));
    }

    match mode {
        DisplayMode::Full => {
            if !full_body.is_empty() {
                let ranges = aggregated_match_ranges(result, full_body);
                output.push('\n');
                output.push_str(&format_body(full_body, &ranges));
                output.push('\n');
            }
        }
        DisplayMode::List => output.push('\n'),
        DisplayMode::Matches => {
            let ranges = aggregated_match_ranges(result, full_body);
            if !ranges.is_empty() {
                let formatted = extract_matching_lines(full_body, &ranges);
                output.push_str(&formatted);
                output.push('\n');
            }
            output.push('\n');
        }
    }

    output
}

/// Prints context analysis warnings to stderr in a consistent format.
fn print_context_warnings(warnings: &[ContextWarning]) {
    for warning in warnings {
        eprintln!(
            "warning: failed to read {}: {}",
            warning.path, warning.reason
        );
    }
}

/// Prints search overrides from matched rules if any are set.
fn print_search_overrides(overrides: &ra_config::SearchOverrides, indent: &str) {
    if overrides.is_empty() {
        return;
    }
    let mut parts = Vec::new();
    if let Some(limit) = overrides.limit {
        parts.push(format!("limit={limit}"));
    }
    if let Some(max_candidates) = overrides.max_candidates {
        parts.push(format!("max_candidates={max_candidates}"));
    }
    if let Some(cutoff_ratio) = overrides.cutoff_ratio {
        parts.push(format!("cutoff_ratio={cutoff_ratio}"));
    }
    if let Some(aggregation_threshold) = overrides.aggregation_threshold {
        parts.push(format!("aggregation_threshold={aggregation_threshold}"));
    }
    println!("{indent}Search: {}", parts.join(", "));
}

/// Implements the `ra context` command.
#[allow(clippy::too_many_arguments)]
fn cmd_context(
    files: &[String],
    limit: Option<usize>,
    max_terms: Option<usize>,
    list: bool,
    matches: bool,
    json: bool,
    explain: bool,
    no_aggregation: bool,
    max_candidates: Option<usize>,
    cutoff_ratio: Option<f32>,
    aggregation_threshold: Option<f32>,
    fuzzy: Option<u8>,
    verbose: u8,
    trees: &[String],
) -> ExitCode {
    let (_, config) = match load_config_with_cwd(true) {
        Ok(res) => res,
        Err(code) => return code,
    };

    // Apply config default for max_terms if not specified on CLI
    let max_terms = max_terms.unwrap_or(config.context.terms);

    // Ensure index is fresh (needed for both explain and search modes)
    let mut searcher = match ensure_index_fresh(&config, fuzzy) {
        Ok(s) => s,
        Err(code) => return code,
    };

    // Create context search engine
    let mut context_search = match ContextSearch::new(&mut searcher, &config.context, max_terms) {
        Ok(cs) => cs,
        Err(e) => {
            eprintln!("error: failed to initialize context search: {e}");
            return ExitCode::FAILURE;
        }
    };

    // Convert file paths, checking for existence and warning about binary files
    let mut file_paths: Vec<&Path> = Vec::new();
    for file_str in files {
        let path = Path::new(file_str.as_str());
        if !path.exists() {
            eprintln!("error: file not found: {file_str}");
            return ExitCode::FAILURE;
        }
        if ra_index::is_binary_file(path) {
            eprintln!("warning: skipping binary file: {file_str}");
            continue;
        }
        file_paths.push(path);
    }

    // Analyze files
    let analysis = context_search.analyze(&file_paths, trees);

    print_context_warnings(&analysis.warnings);

    if analysis.is_empty() {
        eprintln!("error: no analyzable files provided");
        return ExitCode::FAILURE;
    }

    // Handle --explain mode
    if explain {
        return output_context_explain(&analysis, json);
    }

    // Check if any terms were extracted
    if analysis.query_expr.is_none() {
        println!("{}", dim("No context terms extracted."));
        return ExitCode::SUCCESS;
    }

    // Execute the search with CLI overrides, then rule overrides, then config defaults
    let overrides = SearchParamsOverrides {
        limit,
        max_candidates,
        cutoff_ratio,
        aggregation_threshold,
        no_aggregation,
        trees: trees.to_vec(),
        verbose,
    };
    let params =
        overrides.build_params_with_rule_overrides(&config.search, &analysis.merged_rules.search);

    let (results, analysis) = match context_search.search_with_analysis(analysis, &params) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: context search failed: {e}");
            return ExitCode::FAILURE;
        }
    };

    // Output results
    let query_display = analysis
        .query_string()
        .unwrap_or_else(|| String::from("(empty)"));
    output_aggregated_results(
        &results,
        &query_display,
        list,
        matches,
        json,
        verbose,
        context_search.searcher(),
    )
}

/// Outputs explain mode information for context analysis.
fn output_context_explain(analysis_result: &ContextAnalysisResult, json: bool) -> ExitCode {
    if json {
        // JSON output for explain mode
        let json_output = JsonContextExplain {
            merged_rules: JsonMatchedRules {
                terms: analysis_result.merged_rules.terms.clone(),
                trees: analysis_result.merged_rules.trees.clone(),
                include: analysis_result.merged_rules.include.clone(),
                search: JsonSearchOverrides::from_config(&analysis_result.merged_rules.search),
            },
            files: analysis_result
                .files
                .iter()
                .map(|fa| JsonFileAnalysis {
                    file: fa.path.clone(),
                    terms: fa
                        .analysis
                        .ranked_terms
                        .iter()
                        .map(|rt| JsonTermAnalysis {
                            term: rt.term.term.clone(),
                            source: rt.term.source.to_string(),
                            weight: rt.term.weight,
                            frequency: rt.term.frequency,
                            idf: rt.idf,
                            score: rt.score,
                        })
                        .collect(),
                    query: fa.analysis.query_string().map(|s| s.to_string()),
                    matched_rules: JsonMatchedRules {
                        terms: fa.matched_rules.terms.clone(),
                        trees: fa.matched_rules.trees.clone(),
                        include: fa.matched_rules.include.clone(),
                        search: JsonSearchOverrides::from_config(&fa.matched_rules.search),
                    },
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
    } else {
        // Human-readable explain output

        // Show merged rules at the top (final applied rules)
        println!("{}", subheader("Applied context rules:"));
        if analysis_result.merged_rules.is_empty() {
            println!("  {}", dim("(none)"));
        } else {
            if !analysis_result.merged_rules.terms.is_empty() {
                println!(
                    "  Terms:   {}",
                    analysis_result.merged_rules.terms.join(", ")
                );
            }
            if !analysis_result.merged_rules.trees.is_empty() {
                println!(
                    "  Trees:   {}",
                    analysis_result.merged_rules.trees.join(", ")
                );
            }
            if !analysis_result.merged_rules.include.is_empty() {
                println!(
                    "  Include: {}",
                    analysis_result.merged_rules.include.join(", ")
                );
            }
            print_search_overrides(&analysis_result.merged_rules.search, "  ");
        }
        println!();

        for fa in &analysis_result.files {
            println!("{}", subheader(&format!("File: {}", fa.path)));
            println!();

            // Show per-file matched rules if any (and different from merged)
            if !fa.matched_rules.is_empty() {
                println!("{}", dim("Matched rules:"));
                if !fa.matched_rules.terms.is_empty() {
                    println!("  Terms:   {}", fa.matched_rules.terms.join(", "));
                }
                if !fa.matched_rules.trees.is_empty() {
                    println!("  Trees:   {}", fa.matched_rules.trees.join(", "));
                }
                if !fa.matched_rules.include.is_empty() {
                    println!("  Include: {}", fa.matched_rules.include.join(", "));
                }
                print_search_overrides(&fa.matched_rules.search, "  ");
                println!();
            }

            // Show ranked terms
            println!("{}", subheader("Ranked terms:"));
            if fa.analysis.ranked_terms.is_empty() {
                println!("  {}", dim("(none)"));
            } else {
                let mut table = Table::new();
                table.load_preset(UTF8_FULL_CONDENSED);
                table.set_header(vec!["Term", "Source", "Weight", "Freq", "IDF", "Score"]);

                for rt in &fa.analysis.ranked_terms {
                    table.add_row(vec![
                        Cell::new(&rt.term.term),
                        Cell::new(rt.term.source.to_string()),
                        Cell::new(format!("{:.1}", rt.term.weight)),
                        Cell::new(rt.term.frequency.to_string()),
                        Cell::new(format!("{:.2}", rt.idf)),
                        Cell::new(format!("{:.2}", rt.score)),
                    ]);
                }

                println!("{table}");
            }
            println!();

            // Show generated query as AST tree
            println!("{}", subheader("Generated query:"));
            if let Some(expr) = fa.analysis.query_expr() {
                // Use the Display impl which shows a multi-line tree structure
                let tree = expr.to_string();
                for line in tree.lines() {
                    println!("  {line}");
                }
            } else {
                println!("  {}", dim("(no query generated)"));
            }
            println!();
        }
    }

    ExitCode::SUCCESS
}

/// JSON output for context explain mode.
#[derive(Serialize)]
struct JsonContextExplain {
    /// Merged context rules across all files.
    merged_rules: JsonMatchedRules,
    /// Analysis results for each file.
    files: Vec<JsonFileAnalysis>,
}

/// JSON output for a single file's context analysis.
#[derive(Serialize)]
struct JsonFileAnalysis {
    /// File path.
    file: String,
    /// Ranked terms with scores.
    terms: Vec<JsonTermAnalysis>,
    /// Generated query string.
    query: Option<String>,
    /// Matched context rules.
    matched_rules: JsonMatchedRules,
}

/// JSON output for matched context rules.
#[derive(Serialize)]
struct JsonMatchedRules {
    /// Terms injected from matching rules.
    terms: Vec<String>,
    /// Trees to limit search to.
    trees: Vec<String>,
    /// Files to always include.
    include: Vec<String>,
    /// Search parameter overrides from rules.
    #[serde(skip_serializing_if = "JsonSearchOverrides::is_empty")]
    search: JsonSearchOverrides,
}

/// JSON output for search parameter overrides from rules.
#[derive(Serialize, Default)]
struct JsonSearchOverrides {
    /// Maximum results to return after aggregation.
    #[serde(skip_serializing_if = "Option::is_none")]
    limit: Option<usize>,
    /// Maximum candidates before aggregation.
    #[serde(skip_serializing_if = "Option::is_none")]
    max_candidates: Option<usize>,
    /// Elbow detection cutoff ratio.
    #[serde(skip_serializing_if = "Option::is_none")]
    cutoff_ratio: Option<f32>,
    /// Sibling aggregation threshold.
    #[serde(skip_serializing_if = "Option::is_none")]
    aggregation_threshold: Option<f32>,
}

impl JsonSearchOverrides {
    /// Returns true if no overrides are set.
    fn is_empty(&self) -> bool {
        self.limit.is_none()
            && self.max_candidates.is_none()
            && self.cutoff_ratio.is_none()
            && self.aggregation_threshold.is_none()
    }

    /// Creates from config search overrides.
    fn from_config(overrides: &ra_config::SearchOverrides) -> Self {
        Self {
            limit: overrides.limit,
            max_candidates: overrides.max_candidates,
            cutoff_ratio: overrides.cutoff_ratio,
            aggregation_threshold: overrides.aggregation_threshold,
        }
    }
}

/// JSON output for a single term's analysis.
#[derive(Serialize)]
struct JsonTermAnalysis {
    /// The term text.
    term: String,
    /// Source location (PathFilename, MarkdownH1, Body, etc.).
    source: String,
    /// Base weight from source.
    weight: f32,
    /// Frequency in the document.
    frequency: u32,
    /// IDF value from the index.
    idf: f32,
    /// Final TF-IDF score.
    score: f32,
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
    let (_, config) = match load_config_with_cwd(false) {
        Ok(res) => res,
        Err(code) => return code,
    };

    // Parse the ID
    let Some((tree, path, slug)) = parse_chunk_id(id) else {
        eprintln!("error: invalid ID format: {id}");
        eprintln!("Expected format: tree:path#slug or tree:path");
        return ExitCode::FAILURE;
    };

    // Ensure index is fresh
    let searcher = match ensure_index_fresh(&config, None) {
        Ok(s) => s,
        Err(code) => return code,
    };

    // Get results
    let results: Vec<SearchCandidate> = if full_document || slug.is_none() {
        match searcher.get_by_path(&tree, &path) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("error: failed to retrieve document: {e}");
                return ExitCode::FAILURE;
            }
        }
    } else {
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

    let aggregated: Vec<AggregatedSearchResult> = results
        .into_iter()
        .map(AggregatedSearchResult::Single)
        .collect();

    output_aggregated_results(&aggregated, id, false, false, json, 0, &searcher)
}

/// Implements the `ra likethis` command.
#[allow(clippy::too_many_arguments)]
fn cmd_likethis(
    source: &str,
    limit: Option<usize>,
    list: bool,
    matches: bool,
    json: bool,
    explain: bool,
    no_aggregation: bool,
    max_candidates: Option<usize>,
    cutoff_ratio: Option<f32>,
    aggregation_threshold: Option<f32>,
    verbose: u8,
    trees: &[String],
    min_doc_freq: u64,
    max_doc_freq: Option<u64>,
    min_term_freq: usize,
    max_terms: usize,
    min_word_len: usize,
    max_word_len: usize,
    boost: f32,
) -> ExitCode {
    let (_, config) = match load_config_with_cwd(true) {
        Ok(res) => res,
        Err(code) => return code,
    };

    // Build MLT parameters
    let mlt_params = MoreLikeThisParams {
        min_doc_frequency: min_doc_freq,
        max_doc_frequency: max_doc_freq.unwrap_or(u64::MAX / 2),
        min_term_frequency: min_term_freq,
        max_query_terms: max_terms,
        min_word_length: min_word_len,
        max_word_length: max_word_len,
        boost_factor: boost,
        stop_words: Vec::new(),
    };

    // Build search parameters
    let overrides = SearchParamsOverrides {
        limit,
        max_candidates,
        cutoff_ratio,
        aggregation_threshold,
        no_aggregation,
        trees: trees.to_vec(),
        verbose,
    };
    let search_params = overrides.build_params(&config.search);

    // Ensure index is fresh
    let mut searcher = match ensure_index_fresh(&config, None) {
        Ok(s) => s,
        Err(code) => return code,
    };

    // Determine if source is a chunk ID or file path
    let is_chunk_id = is_chunk_id_format(source);

    // Handle --explain mode
    if explain {
        return cmd_likethis_explain(
            source,
            is_chunk_id,
            &mlt_params,
            &search_params,
            &searcher,
            json,
        );
    }

    // Execute the search
    let results = if is_chunk_id {
        // Search by chunk ID
        match searcher.search_more_like_this_by_id(source, &mlt_params, &search_params) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("error: {e}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        // Search by file path
        match search_more_like_this_by_file(
            source,
            &mlt_params,
            &search_params,
            &mut searcher,
            &config,
        ) {
            Ok(r) => r,
            Err(code) => return code,
        }
    };

    // Output results
    let display_source = if is_chunk_id {
        source.to_string()
    } else {
        format!("file:{source}")
    };
    output_aggregated_results(
        &results,
        &display_source,
        list,
        matches,
        json,
        verbose,
        &searcher,
    )
}

/// Determines if a source string looks like a chunk ID (contains ':').
fn is_chunk_id_format(source: &str) -> bool {
    // A chunk ID has format tree:path or tree:path#slug
    // A file path might be ./foo.md, /abs/path.md, or relative/path.md
    // Heuristic: if it contains ':' but doesn't look like a Windows path (C:\...)
    // and doesn't start with './' or '/', treat it as a chunk ID
    if source.contains(':') {
        // Check for Windows absolute path like C:\
        if source.len() >= 2 && source.chars().nth(1) == Some(':') {
            return false;
        }
        return true;
    }
    false
}

/// Searches for similar documents using a file path as source.
fn search_more_like_this_by_file(
    file_path: &str,
    mlt_params: &MoreLikeThisParams,
    search_params: &SearchParams,
    searcher: &mut Searcher,
    config: &Config,
) -> Result<Vec<AggregatedSearchResult>, ExitCode> {
    let path = Path::new(file_path);

    // Check file exists
    if !path.exists() {
        eprintln!("error: file not found: {file_path}");
        return Err(ExitCode::FAILURE);
    }

    // Check not binary
    if ra_index::is_binary_file(path) {
        eprintln!("error: binary file not supported: {file_path}");
        return Err(ExitCode::FAILURE);
    }

    // Parse the file to extract content
    let parsed = match parse_file(path, "likethis") {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: failed to parse file: {e}");
            return Err(ExitCode::FAILURE);
        }
    };

    let doc = &parsed.document;

    // Extract fields for MLT query
    let mut fields: Vec<(&str, String)> = Vec::new();

    // Add title
    if !doc.title.is_empty() {
        fields.push(("title", doc.title.clone()));
    }

    // Add body from root chunk (concatenate all chunk bodies)
    let chunks = doc.chunk_tree.extract_chunks(&doc.title);
    let body: String = chunks
        .iter()
        .map(|c| c.body.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    if !body.is_empty() {
        fields.push(("body", body));
    }

    // Add tags
    if !doc.tags.is_empty() {
        fields.push(("tags", doc.tags.join(" ")));
    }

    if fields.is_empty() {
        eprintln!("error: no content extracted from file");
        return Err(ExitCode::FAILURE);
    }

    // Compute exclude doc IDs (if this file is in a tree, exclude it from results)
    let exclude_doc_ids = compute_exclude_doc_ids_for_file(path, config);

    // Execute search
    searcher
        .search_more_like_this_by_fields(fields, mlt_params, search_params, &exclude_doc_ids)
        .map_err(|e| {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        })
}

/// Computes doc IDs to exclude based on whether a file path is in a configured tree.
fn compute_exclude_doc_ids_for_file(path: &Path, config: &Config) -> HashSet<String> {
    let mut exclude = HashSet::new();

    // Try to get absolute path
    let abs_path = match path.canonicalize() {
        Ok(p) => p,
        Err(_) => return exclude,
    };

    // Check if this path is within any configured tree
    for tree in &config.trees {
        let tree_path = match tree.path.canonicalize() {
            Ok(p) => p,
            Err(_) => continue,
        };

        if let Ok(rel_path) = abs_path.strip_prefix(&tree_path) {
            // This file is in this tree - compute doc_id
            let doc_id = format!("{}:{}", tree.name, rel_path.display());
            exclude.insert(doc_id);
            break;
        }
    }

    exclude
}

/// Handles --explain mode for likethis command.
fn cmd_likethis_explain(
    source: &str,
    is_chunk_id: bool,
    mlt_params: &MoreLikeThisParams,
    search_params: &SearchParams,
    searcher: &Searcher,
    json: bool,
) -> ExitCode {
    if is_chunk_id {
        // Explain for chunk ID
        let explanation = match searcher.explain_more_like_this(source, mlt_params) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("error: {e}");
                return ExitCode::FAILURE;
            }
        };

        output_likethis_explain(&explanation, search_params, json)
    } else {
        // Explain for file path
        let path = Path::new(source);

        if !path.exists() {
            eprintln!("error: file not found: {source}");
            return ExitCode::FAILURE;
        }

        if ra_index::is_binary_file(path) {
            eprintln!("error: binary file not supported: {source}");
            return ExitCode::FAILURE;
        }

        // Parse the file
        let parsed = match parse_file(path, "likethis") {
            Ok(p) => p,
            Err(e) => {
                eprintln!("error: failed to parse file: {e}");
                return ExitCode::FAILURE;
            }
        };

        let doc = &parsed.document;
        let chunks = doc.chunk_tree.extract_chunks(&doc.title);
        let body: String = chunks
            .iter()
            .map(|c| c.body.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");

        // Create a synthetic explanation
        let explanation = MoreLikeThisExplanation {
            source_id: format!("file:{source}"),
            source_title: doc.title.clone(),
            source_body_preview: body.chars().take(200).collect(),
            mlt_params: mlt_params.clone(),
            query_repr: "(query built from file content)".to_string(),
        };

        output_likethis_explain(&explanation, search_params, json)
    }
}

/// Outputs the explain information for likethis command.
fn output_likethis_explain(
    explanation: &MoreLikeThisExplanation,
    search_params: &SearchParams,
    json: bool,
) -> ExitCode {
    if json {
        let json_output = JsonLikeThisExplain {
            source_id: explanation.source_id.clone(),
            source_title: explanation.source_title.clone(),
            source_body_preview: explanation.source_body_preview.clone(),
            mlt_params: JsonMltParams {
                min_doc_frequency: explanation.mlt_params.min_doc_frequency,
                max_doc_frequency: explanation.mlt_params.max_doc_frequency,
                min_term_frequency: explanation.mlt_params.min_term_frequency,
                max_query_terms: explanation.mlt_params.max_query_terms,
                min_word_length: explanation.mlt_params.min_word_length,
                max_word_length: explanation.mlt_params.max_word_length,
                boost_factor: explanation.mlt_params.boost_factor,
            },
            search_params: JsonSearchParams {
                candidate_limit: search_params.effective_candidate_limit(),
                cutoff_ratio: search_params.cutoff_ratio,
                max_candidates: search_params.max_candidates,
                limit: search_params.limit,
                aggregation_threshold: search_params.aggregation_threshold,
                disable_aggregation: search_params.disable_aggregation,
                trees: search_params.trees.clone(),
            },
        };

        match serde_json::to_string_pretty(&json_output) {
            Ok(json_str) => println!("{json_str}"),
            Err(e) => {
                eprintln!("error: failed to serialize JSON: {e}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        // Human-readable output
        println!("{}", subheader("Source:"));
        println!("   ID:    {}", explanation.source_id);
        println!("   Title: {}", explanation.source_title);
        println!();

        println!("{}", subheader("Content preview:"));
        println!("   {}...", explanation.source_body_preview);
        println!();

        println!("{}", subheader("MoreLikeThis Parameters:"));
        println!(
            "   min_doc_frequency:  {}",
            explanation.mlt_params.min_doc_frequency
        );
        println!(
            "   max_doc_frequency:  {}",
            explanation.mlt_params.max_doc_frequency
        );
        println!(
            "   min_term_frequency: {}",
            explanation.mlt_params.min_term_frequency
        );
        println!(
            "   max_query_terms:    {}",
            explanation.mlt_params.max_query_terms
        );
        println!(
            "   min_word_length:    {}",
            explanation.mlt_params.min_word_length
        );
        println!(
            "   max_word_length:    {}",
            explanation.mlt_params.max_word_length
        );
        println!(
            "   boost_factor:       {}",
            explanation.mlt_params.boost_factor
        );
        println!();

        println!("{}", subheader("Search Parameters:"));
        println!(
            "   Phase 1: candidate_limit = {}",
            search_params.effective_candidate_limit()
        );
        println!("   Phase 2: cutoff_ratio = {}", search_params.cutoff_ratio);
        println!(
            "   Phase 2: max_candidates = {}",
            search_params.max_candidates
        );
        println!(
            "   Phase 3: aggregation_threshold = {}",
            search_params.aggregation_threshold
        );
        println!("   Phase 4: limit = {}", search_params.limit);
        println!(
            "   Aggregation = {}",
            if search_params.disable_aggregation {
                "disabled"
            } else {
                "enabled"
            }
        );
        if !search_params.trees.is_empty() {
            println!("   Trees: {}", search_params.trees.join(", "));
        }
    }

    ExitCode::SUCCESS
}

/// JSON output for likethis explain mode.
#[derive(Serialize)]
struct JsonLikeThisExplain {
    /// Source document or file ID.
    source_id: String,
    /// Title of the source document.
    source_title: String,
    /// Preview of the source body content.
    source_body_preview: String,
    /// MoreLikeThis parameters used.
    mlt_params: JsonMltParams,
    /// Search parameters used.
    search_params: JsonSearchParams,
}

/// JSON output for MLT parameters.
#[derive(Serialize)]
struct JsonMltParams {
    /// Minimum document frequency for terms.
    min_doc_frequency: u64,
    /// Maximum document frequency for terms.
    max_doc_frequency: u64,
    /// Minimum term frequency in source.
    min_term_frequency: usize,
    /// Maximum query terms to use.
    max_query_terms: usize,
    /// Minimum word length.
    min_word_length: usize,
    /// Maximum word length.
    max_word_length: usize,
    /// Boost factor for terms.
    boost_factor: f32,
}

/// JSON output for search parameters.
#[derive(Serialize)]
struct JsonSearchParams {
    /// Maximum candidates from index.
    candidate_limit: usize,
    /// Score ratio threshold for cutoff.
    cutoff_ratio: f32,
    /// Maximum candidates before aggregation.
    max_candidates: usize,
    /// Maximum results to return after aggregation.
    limit: usize,
    /// Sibling ratio threshold for aggregation.
    aggregation_threshold: f32,
    /// Whether aggregation is disabled.
    disable_aggregation: bool,
    /// Trees to limit results to.
    trees: Vec<String>,
}

/// Implements the `ra init` command.
fn cmd_init(global: bool, force: bool) -> ExitCode {
    let cwd = match current_dir_or_failure() {
        Ok(cwd) => cwd,
        Err(code) => return code,
    };

    // Determine if we're in the home directory
    let is_home_dir = global_config_path()
        .and_then(|p| p.parent().map(|h| h == cwd))
        .unwrap_or(false);

    // Use global template if --global flag or if we're in the home directory
    let use_global = global || is_home_dir;

    let config_path = if use_global {
        match global_config_path() {
            Some(path) => path,
            None => {
                eprintln!("error: could not determine home directory");
                return ExitCode::FAILURE;
            }
        }
    } else {
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

    // Write the config file (commented-out example)
    let template = if use_global {
        global_template()
    } else {
        local_template()
    };

    if let Err(e) = fs::write(&config_path, &template) {
        eprintln!("error: failed to write {}: {e}", config_path.display());
        return ExitCode::FAILURE;
    }

    println!("Created {}", config_path.display());

    // Show the written config with indentation and syntax highlighting for clarity
    let highlighter = Highlighter::new();
    println!();
    println!("{}", subheader("Configuration written:"));
    let highlighted = highlighter.highlight_toml(&template);
    println!("{}", indent_content(&highlighted));

    // For local configs, try to add .ra/ to .gitignore
    if !use_global && let Err(e) = update_gitignore(&config_path) {
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
///
/// Shows configuration files, trees, index status, and validates the configuration.
fn cmd_status() -> ExitCode {
    let cwd = match current_dir_or_failure() {
        Ok(cwd) => cwd,
        Err(code) => return code,
    };

    // Discover config files
    let config_files = discover_config_files(&cwd);

    if config_files.is_empty() {
        println!("{}", dim("No configuration files found."));
        println!();
        println!(
            "Run {} to create a configuration file.",
            subheader("ra init")
        );
        return ExitCode::SUCCESS;
    }

    println!("{}", subheader("Config files:"));
    for path in &config_files {
        let display_path = format_path_for_display(path, Some(&cwd));
        println!("   {display_path}");
    }
    println!();

    // Load merged config
    let config = match load_config_or_failure(&cwd) {
        Ok(config) => config,
        Err(code) => return code,
    };

    // Show trees with status
    println!("{}", subheader("Trees:"));
    if config.trees.is_empty() {
        println!("   {}", dim("(none defined)"));
    } else {
        for tree in &config.trees {
            let scope = if tree.is_global { "global" } else { "local" };
            // Format path: relative to config_root for local trees, ~ for global
            let base = if tree.is_global {
                None
            } else {
                config.config_root.as_deref()
            };
            let display_path = format_path_for_display(&tree.path, base);
            if tree.path.exists() {
                println!(
                    "   {} {} {}",
                    tree.name,
                    dim(&format!("({scope})")),
                    dim(&format!("-> {display_path}"))
                );
            } else {
                println!(
                    "   {} {} {} {}",
                    tree.name,
                    dim(&format!("({scope})")),
                    dim(&format!("-> {display_path}")),
                    warning("[missing]")
                );
            }
        }
    }
    println!();

    // Show include/exclude patterns per tree
    if !config.trees.is_empty() {
        println!("{}", subheader("Patterns:"));
        for tree in &config.trees {
            println!("   {}:", tree.name);
            for pattern in &tree.include {
                println!("      + {pattern}");
            }
            for pattern in &tree.exclude {
                println!("      - {pattern}");
            }
        }
        println!();
    }

    // Show index status
    let index_status = detect_index_status(&config);
    let index_path = index_directory(&config);
    print!("{}\n   {}", subheader("Index:"), index_status.description());
    if let Some(path) = &index_path {
        println!(" {}", dim(&format!("({})", path.display())));
    } else {
        println!();
    }
    println!();

    // Validate configuration and report warnings
    let warnings = config.validate();

    if warnings.is_empty() {
        println!("No issues found.");
        return ExitCode::SUCCESS;
    }

    println!("{}", subheader(&format!("Warnings ({}):", warnings.len())));
    for w in &warnings {
        println!("   {}", warning(&format_warning(w)));
    }
    println!();

    // Print hints for common issues
    print_hints(&warnings);

    ExitCode::FAILURE
}

/// Implements the `ra config` command.
fn cmd_config() -> ExitCode {
    let cwd = match current_dir_or_failure() {
        Ok(cwd) => cwd,
        Err(code) => return code,
    };

    let config = match load_config_or_failure(&cwd) {
        Ok(config) => config,
        Err(code) => return code,
    };

    // Output effective settings in TOML format with syntax highlighting
    let highlighter = Highlighter::new();
    print!("{}", highlighter.highlight_toml(&config.settings_to_toml()));

    ExitCode::SUCCESS
}

/// Implements the `ra ls` command.
fn cmd_ls(what: LsWhat, long: bool) -> ExitCode {
    match what {
        LsWhat::Trees => cmd_ls_trees(long),
        LsWhat::Docs => cmd_ls_docs(long),
        LsWhat::Chunks => cmd_ls_chunks(long),
    }
}

/// Lists all configured trees.
fn cmd_ls_trees(long: bool) -> ExitCode {
    let cwd = match current_dir_or_failure() {
        Ok(cwd) => cwd,
        Err(code) => return code,
    };

    let config = match load_config_or_failure(&cwd) {
        Ok(config) => config,
        Err(code) => return code,
    };

    if config.trees.is_empty() {
        println!("{}", dim("No trees configured."));
        return ExitCode::SUCCESS;
    }

    for tree in &config.trees {
        let scope = if tree.is_global { "global" } else { "local" };
        println!(
            "{} {} {}",
            header(&tree.name),
            dim(&format!("({scope})")),
            dim(&format!("→ {}", tree.path.display()))
        );

        if long {
            // Show include patterns
            for pattern in &tree.include {
                println!("  {} {}", dim("+"), pattern);
            }
            // Show exclude patterns
            for pattern in &tree.exclude {
                println!("  {} {}", dim("-"), pattern);
            }
            println!();
        }
    }

    ExitCode::SUCCESS
}

/// Document info collected for listing.
struct DocInfo {
    /// Tree name.
    tree: String,
    /// File path.
    path: String,
    /// Document title.
    title: String,
    /// Number of chunks in this document.
    chunk_count: usize,
    /// Total body size across all chunks.
    total_size: usize,
}

/// Lists all indexed documents.
fn cmd_ls_docs(long: bool) -> ExitCode {
    let (_, config) = match load_config_with_cwd(false) {
        Ok(res) => res,
        Err(code) => return code,
    };

    // Ensure index is fresh
    let searcher = match ensure_index_fresh(&config, None) {
        Ok(s) => s,
        Err(code) => return code,
    };

    // Get all chunks and extract unique documents
    let chunks = match searcher.list_all() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: failed to list chunks: {e}");
            return ExitCode::FAILURE;
        }
    };

    // Collect unique documents with stats
    let mut docs: Vec<DocInfo> = Vec::new();
    let mut doc_map: HashMap<String, usize> = HashMap::new();

    for chunk in &chunks {
        let doc_key = format!("{}:{}", chunk.tree, chunk.path);
        if let Some(&idx) = doc_map.get(&doc_key) {
            docs[idx].chunk_count += 1;
            docs[idx].total_size += chunk.body.len();
        } else {
            doc_map.insert(doc_key, docs.len());
            docs.push(DocInfo {
                tree: chunk.tree.clone(),
                path: chunk.path.clone(),
                title: chunk.title().to_string(),
                chunk_count: 1,
                total_size: chunk.body.len(),
            });
        }
    }

    if docs.is_empty() {
        println!("{}", dim("No documents indexed."));
        return ExitCode::SUCCESS;
    }

    for doc in &docs {
        println!(
            "{} {} {}",
            header(&format!("{}:{}", doc.tree, doc.path)),
            dim("—"),
            breadcrumb(&doc.title)
        );
        if long {
            println!(
                "  {}",
                dim(&format!(
                    "{} chunks, {} chars",
                    doc.chunk_count, doc.total_size
                ))
            );
            println!();
        }
    }

    ExitCode::SUCCESS
}

/// Lists all indexed chunks.
fn cmd_ls_chunks(long: bool) -> ExitCode {
    let (_, config) = match load_config_with_cwd(false) {
        Ok(res) => res,
        Err(code) => return code,
    };

    // Ensure index is fresh
    let searcher = match ensure_index_fresh(&config, None) {
        Ok(s) => s,
        Err(code) => return code,
    };

    // Get all chunks
    let chunks = match searcher.list_all() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: failed to list chunks: {e}");
            return ExitCode::FAILURE;
        }
    };

    if chunks.is_empty() {
        println!("{}", dim("No chunks indexed."));
        return ExitCode::SUCCESS;
    }

    for chunk in &chunks {
        println!(
            "{} {} {}",
            header(&chunk.id),
            dim("—"),
            breadcrumb(&chunk.breadcrumb())
        );
        if long {
            println!("  {}", dim(&format!("{} chars", chunk.body.len())));
            println!();
        }
    }

    ExitCode::SUCCESS
}

/// Formats a warning with helpful context.
fn format_warning(w: &ConfigWarning) -> String {
    w.to_string()
}

/// Prints hints for resolving common warnings.
fn print_hints(warnings: &[ConfigWarning]) {
    let mut hints = Vec::new();

    for w in warnings {
        match w {
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
        println!("{}", subheader("Hints:"));
        for hint in hints {
            println!("   {}", dim(hint));
        }
    }
}

/// Implements the `ra inspect` command.
fn cmd_inspect(what: InspectWhat) -> ExitCode {
    match what {
        InspectWhat::Doc { file } => cmd_inspect_doc(&file),
        InspectWhat::Ctx { file } => cmd_inspect_ctx(&file),
    }
}

/// Implements `ra inspect doc` - show how ra parses a document.
fn cmd_inspect_doc(file: &str) -> ExitCode {
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
    let result = match parse_file(path, "inspect") {
        Ok(result) => result,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };

    let doc = &result.document;

    // Document header - matches search result format
    println!(
        "--- {} ---",
        header(&format!("{} ({})", path.display(), file_type))
    );
    println!("{}", breadcrumb(&doc.title));
    if !doc.tags.is_empty() {
        println!("{}", dim(&format!("tags: {}", doc.tags.join(", "))));
    }

    // Extract chunks from the tree
    let chunks = doc.chunk_tree.extract_chunks(&doc.title);

    // Chunking info
    let chunk_info = format!(
        "hierarchical chunking -> {} nodes, {} chunks",
        doc.chunk_tree.node_count(),
        chunks.len()
    );
    println!("{}", dim(&chunk_info));
    println!();

    // Display each chunk in search result format
    for chunk in &chunks {
        let chunk_label = if chunk.depth() == 0 {
            format!("{} (document)", chunk.id)
        } else {
            format!("{} (depth {})", chunk.id, chunk.depth())
        };
        println!("--- {} ---", header(&chunk_label));
        // Build breadcrumb from hierarchy
        let bc = chunk.hierarchy.join(" > ");
        println!("{}", breadcrumb(&bc));
        println!("{}", dim(&format!("{} chars", chunk.body.len())));
        println!();

        // Show preview of body with content styling and indentation
        let preview = chunk_preview(&chunk.body, 200);
        println!("{}", indent_content(&preview));
        println!();
    }

    ExitCode::SUCCESS
}

/// Implements `ra inspect ctx` - show context signals for a file.
fn cmd_inspect_ctx(file: &str) -> ExitCode {
    use ra_context::{ContextAnalyzer, is_binary_file};

    let path = Path::new(file);

    // Check if file exists
    if !path.exists() {
        eprintln!("error: file not found: {file}");
        return ExitCode::FAILURE;
    }

    // Check if binary
    if is_binary_file(path) {
        eprintln!("error: binary file: {file}");
        return ExitCode::FAILURE;
    }

    // Load config to get context patterns
    let cwd = match current_dir_or_failure() {
        Ok(cwd) => cwd,
        Err(code) => return code,
    };

    let config = match load_config_or_failure(&cwd) {
        Ok(config) => config,
        Err(code) => return code,
    };

    // Compile context rules
    let rules = match CompiledContextRules::compile(&config.context) {
        Ok(rules) => rules,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };

    // Create analyzer and analyze the file
    let analyzer = ContextAnalyzer::new(&config.context, rules);
    let signals = match analyzer.analyze_file(path) {
        Ok(signals) => signals,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };

    // Display results
    println!("{}", subheader("Context signals:"));
    println!("   {}", path.display());
    println!();

    println!("{}", subheader("Path terms:"));
    if signals.path_terms.is_empty() {
        println!("   {}", dim("(none)"));
    } else {
        for term in &signals.path_terms {
            println!("   {term}");
        }
    }
    println!();

    println!("{}", subheader("Pattern terms:"));
    if signals.pattern_terms.is_empty() {
        println!("   {}", dim("(none)"));
    } else {
        for term in &signals.pattern_terms {
            println!("   {term}");
        }
    }
    println!();

    println!("{}", subheader("Combined search terms:"));
    let all_terms = signals.all_terms();
    if all_terms.is_empty() {
        println!("   {}", dim("(none)"));
    } else {
        println!("   {}", all_terms.join(", "));
    }

    ExitCode::SUCCESS
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
    let (_, config) = match load_config_with_cwd(true) {
        Ok(res) => res,
        Err(code) => return code,
    };

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

#[cfg(test)]
mod tests {
    use clap::CommandFactory;
    use ra_config::{
        DEFAULT_AGGREGATION_THRESHOLD, DEFAULT_CONTEXT_TERMS, DEFAULT_CUTOFF_RATIO,
        DEFAULT_MAX_CANDIDATES, DEFAULT_SEARCH_LIMIT,
    };

    use super::*;

    /// Gets help text for a subcommand's argument.
    fn get_arg_help(cmd: &clap::Command, subcmd: &str, arg: &str) -> String {
        cmd.get_subcommands()
            .find(|c| c.get_name() == subcmd)
            .and_then(|c| c.get_arguments().find(|a| a.get_id() == arg))
            .and_then(|a| a.get_help().map(|h| h.to_string()))
            .unwrap_or_default()
    }

    /// Verifies that CLI help text contains the correct default values.
    ///
    /// This test catches drift between the DEFAULT_* constants in ra-config
    /// and the help text strings in command definitions.
    #[test]
    fn cli_help_defaults_match_constants() {
        let cmd = Cli::command();

        // Check search command defaults
        let limit_help = get_arg_help(&cmd, "search", "limit");
        assert!(
            limit_help.contains(&format!("[default: {}]", DEFAULT_SEARCH_LIMIT)),
            "search --limit help should contain default {}: {limit_help}",
            DEFAULT_SEARCH_LIMIT
        );

        let max_candidates_help = get_arg_help(&cmd, "search", "max_candidates");
        assert!(
            max_candidates_help.contains(&format!("[default: {}]", DEFAULT_MAX_CANDIDATES)),
            "search --max-candidates help should contain default {}: {max_candidates_help}",
            DEFAULT_MAX_CANDIDATES
        );

        let cutoff_help = get_arg_help(&cmd, "search", "cutoff_ratio");
        assert!(
            cutoff_help.contains(&format!("[default: {}]", DEFAULT_CUTOFF_RATIO)),
            "search --cutoff-ratio help should contain default {}: {cutoff_help}",
            DEFAULT_CUTOFF_RATIO
        );

        let agg_help = get_arg_help(&cmd, "search", "aggregation_threshold");
        assert!(
            agg_help.contains(&format!("[default: {}]", DEFAULT_AGGREGATION_THRESHOLD)),
            "search --aggregation-threshold help should contain default {}: {agg_help}",
            DEFAULT_AGGREGATION_THRESHOLD
        );

        // Check context command defaults
        let ctx_limit_help = get_arg_help(&cmd, "context", "limit");
        assert!(
            ctx_limit_help.contains(&format!("[default: {}]", DEFAULT_SEARCH_LIMIT)),
            "context --limit help should contain default {}: {ctx_limit_help}",
            DEFAULT_SEARCH_LIMIT
        );

        let terms_help = get_arg_help(&cmd, "context", "terms");
        assert!(
            terms_help.contains(&format!("[default: {}]", DEFAULT_CONTEXT_TERMS)),
            "context --terms help should contain default {}: {terms_help}",
            DEFAULT_CONTEXT_TERMS
        );

        // Check likethis command defaults
        let lt_limit_help = get_arg_help(&cmd, "likethis", "limit");
        assert!(
            lt_limit_help.contains(&format!("[default: {}]", DEFAULT_SEARCH_LIMIT)),
            "likethis --limit help should contain default {}: {lt_limit_help}",
            DEFAULT_SEARCH_LIMIT
        );
    }
}
