//! Command-line interface for the `ra` research assistant tool.

use std::{
    collections::{HashMap, HashSet},
    env, fs,
    io::{self, Write},
    ops::Range,
    path::Path,
    process::ExitCode,
};

use clap::{Parser, Subcommand, error::ErrorKind};
use comfy_table::{Cell, Table, presets::UTF8_FULL_CONDENSED};
use ra_config::{
    CONFIG_FILENAME, CompiledContextPatterns, CompiledContextRules, Config, ConfigWarning,
    MatchedRules, discover_config_files, format_path_for_display, global_config_path,
    global_template, local_template,
};
use ra_context::{AnalysisConfig, ContextAnalysis, analyze_context};
use ra_document::parse_file;
use ra_highlight::{
    Highlighter, breadcrumb, dim, error, format_body, header, indent_content, subheader, theme,
    warning,
};
use ra_index::{
    AggregatedSearchResult, IndexStats, IndexStatus, Indexer, ProgressReporter, QueryExpr,
    SearchCandidate, SearchParams, SearchResult, Searcher, SilentReporter, TreeFilteredSearcher,
    detect_index_status, index_directory, is_binary_file, open_searcher, parse_query,
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

        /// Hard limit on number of results (default: no limit, use elbow detection)
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

        /// Maximum candidates to retrieve from index (Phase 1)
        #[arg(long, default_value = "100")]
        candidate_limit: usize,

        /// Score ratio threshold for relevance cutoff (Phase 2)
        #[arg(long, default_value = "0.5")]
        cutoff_ratio: f32,

        /// Sibling ratio threshold for aggregation (Phase 3)
        #[arg(long, default_value = "0.5")]
        aggregation_threshold: f32,

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

        /// Maximum chunks to return
        #[arg(short = 'n', long, default_value = "10")]
        limit: usize,

        /// Maximum terms to include in the query
        #[arg(long, default_value = "15")]
        terms: usize,

        /// Output titles and snippets only
        #[arg(long)]
        list: bool,

        /// Output in JSON format
        #[arg(long)]
        json: bool,

        /// Show term analysis and generated query without searching
        #[arg(long)]
        explain: bool,

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
            candidate_limit,
            cutoff_ratio,
            aggregation_threshold,
            verbose,
            trees,
        } => {
            // If no limit specified, use a very high max_results so elbow detection
            // is the only cutoff. Otherwise use the specified limit.
            let max_results = limit.unwrap_or(usize::MAX);
            let params = SearchParams {
                candidate_limit,
                cutoff_ratio,
                max_results,
                aggregation_threshold,
                disable_aggregation: no_aggregation,
                trees,
                verbosity: verbose,
            };
            return cmd_search(&queries, &params, list, matches, json, explain, verbose);
        }
        Commands::Context {
            files,
            limit,
            terms,
            list,
            json,
            explain,
            verbose,
            trees,
        } => {
            return cmd_context(&files, limit, terms, list, json, explain, verbose, &trees);
        }
        Commands::Get {
            id,
            full_document,
            json,
        } => {
            return cmd_get(&id, full_document, json);
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
            .flat_map(|fm| fm.matched_terms.iter().map(String::as_str))
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
                    if field_match.matched_terms.is_empty() {
                        continue;
                    }
                    // Show each term with its frequency
                    let term_info: Vec<String> = field_match
                        .matched_terms
                        .iter()
                        .map(|term| {
                            let freq = field_match.term_frequencies.get(term).unwrap_or(&1);
                            format!("{term} x {freq}")
                        })
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

/// Formats a search result for full content output.
fn format_result_full(result: &SearchResult) -> String {
    let mut output = String::new();
    output.push_str(&format!(
        "─── {} ───\n",
        highlight_id_with_path(&result.id, &result.path_match_ranges)
    ));
    let breadcrumb_line = highlight_breadcrumb_title(
        &result.breadcrumb,
        &result.title,
        &result.title_match_ranges,
    );
    output.push_str(&format!("{breadcrumb_line}\n\n"));

    // Format body with content styling, indentation, and match highlighting
    let body = format_body(&result.body, &result.match_ranges);
    output.push_str(&body);

    if !result.body.ends_with('\n') {
        output.push('\n');
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
fn merge_ranges(mut ranges: Vec<Range<usize>>) -> Vec<Range<usize>> {
    if ranges.is_empty() {
        return ranges;
    }
    ranges.sort_by_key(|r| r.start);
    let mut merged = Vec::with_capacity(ranges.len());
    let mut current = ranges[0].clone();
    for range in ranges.into_iter().skip(1) {
        if range.start <= current.end {
            current.end = current.end.max(range.end);
        } else {
            merged.push(current);
            current = range;
        }
    }
    merged.push(current);
    merged
}

/// Computes highlight ranges for a search result, mapping child matches into the parent body.
fn aggregated_match_ranges(result: &AggregatedSearchResult, full_body: &str) -> Vec<Range<usize>> {
    match result {
        AggregatedSearchResult::Single(candidate) => candidate.match_ranges.clone(),
        AggregatedSearchResult::Aggregated { constituents, .. } => {
            let parent_start = result.byte_start() as usize;
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
            merge_ranges(ranges)
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
fn cmd_search(
    queries: &[String],
    params: &SearchParams,
    list: bool,
    matches: bool,
    json: bool,
    explain: bool,
    verbose: u8,
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
                println!("   Phase 1: candidate_limit = {}", params.candidate_limit);
                println!("   Phase 2: cutoff_ratio = {}", params.cutoff_ratio);
                println!("   Phase 2: max_results = {}", params.max_results);
                println!(
                    "   Phase 3: aggregation_threshold = {}",
                    params.aggregation_threshold
                );
                println!(
                    "   Phase 3: aggregation = {}",
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
                    .map(|r| {
                        let constituents_count = r.constituents().map(|c| c.len()).unwrap_or(0);
                        let match_ranges = match r {
                            AggregatedSearchResult::Single(c) => Some(
                                c.match_ranges
                                    .iter()
                                    .map(|range| JsonMatchRange {
                                        offset: range.start,
                                        length: range.end - range.start,
                                    })
                                    .collect(),
                            ),
                            AggregatedSearchResult::Aggregated { .. } => None,
                        };

                        let body_field = Some(r.body().to_string());

                        JsonSearchResult {
                            id: r.id().to_string(),
                            tree: r.tree().to_string(),
                            path: r.path().to_string(),
                            title: r.title().to_string(),
                            breadcrumb: r.breadcrumb().to_string(),
                            score: r.score(),
                            snippet: if r.is_aggregated() {
                                Some(format!("[Aggregated: {} matches]", constituents_count))
                            } else {
                                None
                            },
                            body: body_field,
                            content: if list {
                                None
                            } else {
                                Some(format!("> {}\n\n{}", r.breadcrumb(), r.body()))
                            },
                            match_ranges,
                            title_match_ranges: Some(
                                r.title_match_ranges()
                                    .iter()
                                    .map(|range| JsonMatchRange {
                                        offset: range.start,
                                        length: range.end - range.start,
                                    })
                                    .collect(),
                            ),
                            path_match_ranges: Some(
                                r.path_match_ranges()
                                    .iter()
                                    .map(|range| JsonMatchRange {
                                        offset: range.start,
                                        length: range.end - range.start,
                                    })
                                    .collect(),
                            ),
                        }
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
    } else if list {
        if results.is_empty() {
            println!("{}", dim("No results found."));
        } else {
            let mut total_words = 0;
            let mut total_chars = 0;
            for result in results {
                // Read full content from source file
                let full_body = searcher
                    .read_full_content(
                        result.tree(),
                        result.path(),
                        result.byte_start(),
                        result.byte_end(),
                    )
                    .unwrap_or_else(|_| result.body().to_string());
                if verbose > 0 {
                    total_words += full_body.split_whitespace().count();
                    total_chars += full_body.len();
                }
                print!(
                    "{}",
                    format_aggregated_result_list(result, verbose, &full_body)
                );
            }
            if verbose > 0 {
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
        }
        println!();
    } else if matches {
        // Matches-only mode: show only lines containing matches
        if results.is_empty() {
            println!("{}", dim("No results found."));
        } else {
            for result in results {
                let full_body = searcher
                    .read_full_content(
                        result.tree(),
                        result.path(),
                        result.byte_start(),
                        result.byte_end(),
                    )
                    .unwrap_or_else(|_| result.body().to_string());

                print!(
                    "{}",
                    format_aggregated_result_matches(result, verbose, &full_body)
                );
            }
        }
    } else {
        // Full content mode
        if results.is_empty() {
            println!("{}", dim("No results found."));
        } else {
            let mut total_words = 0;
            let mut total_chars = 0;
            for result in results {
                // Read full content from source file
                let full_body = searcher
                    .read_full_content(
                        result.tree(),
                        result.path(),
                        result.byte_start(),
                        result.byte_end(),
                    )
                    .unwrap_or_else(|_| result.body().to_string());
                if verbose > 0 {
                    total_words += full_body.split_whitespace().count();
                    total_chars += full_body.len();
                }
                print!(
                    "{}",
                    format_aggregated_result_full(result, verbose, &full_body)
                );
                println!();
            }
            if verbose > 0 {
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
        }
    }

    ExitCode::SUCCESS
}

/// Formats an aggregated search result for full content output.
///
/// The `full_body` parameter contains the complete content from byte_start to byte_end,
/// including all child nodes' content for parent nodes.
///
/// Verbosity levels:
/// - 0: Basic output
/// - 1 (-v): Add stats, constituents summary, and match term summary
/// - 2 (-vv): Full match details including field breakdown and term frequencies
/// - 3+ (-vvv): Adds raw Tantivy score explanation
fn format_aggregated_result_full(
    result: &AggregatedSearchResult,
    verbose: u8,
    full_body: &str,
) -> String {
    let mut output = String::new();

    let header_id = highlight_id_with_path(result.id(), result.path_match_ranges());
    if verbose > 0 && result.is_aggregated() {
        let constituents = result.constituents().unwrap();
        output.push_str(&format!(
            "─── {} [aggregated: {} matches] ───\n",
            header_id,
            constituents.len()
        ));
    } else {
        output.push_str(&format!("─── {} ───\n", header_id));
    }

    let breadcrumb_line = highlight_breadcrumb_title(
        result.breadcrumb(),
        result.title(),
        result.title_match_ranges(),
    );
    output.push_str(&format!("{breadcrumb_line}\n"));

    // Show stats only in verbose mode
    if verbose > 0 {
        let word_count = full_body.split_whitespace().count();
        let stats = format!(
            "{} words, {} chars, score {:.2}",
            word_count,
            full_body.len(),
            result.score()
        );
        output.push_str(&format!("{}\n", dim(&stats)));
    }

    // Show match details when verbose
    if verbose > 0 {
        output.push_str(&format_match_details(result, verbose));
    }

    if !full_body.is_empty() {
        let ranges = aggregated_match_ranges(result, full_body);
        output.push('\n');
        output.push_str(&format_body(full_body, &ranges));
        output.push('\n');
    }

    output
}

/// Formats an aggregated search result for list mode output.
///
/// List mode shows only the document header and title - no content snippets.
fn format_aggregated_result_list(
    result: &AggregatedSearchResult,
    verbose: u8,
    full_body: &str,
) -> String {
    let mut output = String::new();

    let header_id = highlight_id_with_path(result.id(), result.path_match_ranges());
    if verbose > 0 && result.is_aggregated() {
        let constituents = result.constituents().unwrap();
        output.push_str(&format!(
            "─── {} [aggregated: {} matches] ───\n",
            header_id,
            constituents.len()
        ));
    } else {
        output.push_str(&format!("─── {} ───\n", header_id));
    }

    let breadcrumb_line = highlight_breadcrumb_title(
        result.breadcrumb(),
        result.title(),
        result.title_match_ranges(),
    );
    output.push_str(&format!("{breadcrumb_line}\n"));

    // Show stats and match details in verbose mode
    if verbose > 0 {
        let word_count = full_body.split_whitespace().count();
        let stats = format!(
            "{} words, {} chars, score {:.2}",
            word_count,
            full_body.len(),
            result.score()
        );
        output.push_str(&format!("{}\n", dim(&stats)));
        output.push_str(&format_match_details(result, verbose));
    }

    // List mode: no content - just header and title
    output.push('\n');
    output
}

/// Formats an aggregated search result showing only lines that contain matches.
fn format_aggregated_result_matches(
    result: &AggregatedSearchResult,
    verbose: u8,
    full_body: &str,
) -> String {
    let mut output = String::new();

    let header_id = highlight_id_with_path(result.id(), result.path_match_ranges());
    if verbose > 0 && result.is_aggregated() {
        let constituents = result.constituents().unwrap();
        output.push_str(&format!(
            "─── {} [aggregated: {} matches] ───\n",
            header_id,
            constituents.len()
        ));
    } else {
        output.push_str(&format!("─── {} ───\n", header_id));
    }
    let breadcrumb_line = highlight_breadcrumb_title(
        result.breadcrumb(),
        result.title(),
        result.title_match_ranges(),
    );
    output.push_str(&format!("{breadcrumb_line}\n\n"));

    // Show match details when verbose
    if verbose > 0 {
        output.push_str(&format_match_details(result, verbose));
    }

    let ranges = aggregated_match_ranges(result, full_body);
    if !ranges.is_empty() {
        let formatted = extract_matching_lines(full_body, &ranges);
        output.push_str(&formatted);
        output.push('\n');
    }

    output.push('\n');
    output
}

/// Implements the `ra context` command.
#[allow(clippy::too_many_arguments)]
fn cmd_context(
    files: &[String],
    limit: usize,
    max_terms: usize,
    list: bool,
    json: bool,
    explain: bool,
    verbose: u8,
    trees: &[String],
) -> ExitCode {
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

    // Compile context rules for matching
    let context_rules = match CompiledContextRules::compile(&config.context) {
        Ok(rules) => rules,
        Err(e) => {
            eprintln!("error: failed to compile context rules: {e}");
            return ExitCode::FAILURE;
        }
    };

    // Ensure index is fresh (needed for both explain and search modes)
    let searcher = match ensure_index_fresh(&config) {
        Ok(s) => s,
        Err(code) => return code,
    };

    // Analyze each file using the new weighted analysis
    let analysis_config = AnalysisConfig {
        max_terms,
        min_term_length: 3,
    };

    // Collect matched rules for all files (for tree/term merging)
    let mut all_matched_rules = MatchedRules::default();
    let mut analyses: Vec<(String, ContextAnalysis, MatchedRules)> = Vec::new();

    for file_str in files {
        let path = Path::new(file_str);

        // Check if file exists
        if !path.exists() {
            eprintln!("error: file not found: {file_str}");
            return ExitCode::FAILURE;
        }

        // Skip binary files with warning
        if is_binary_file(path) {
            eprintln!("warning: skipping binary file: {file_str}");
            continue;
        }

        // Match context rules for this file
        let matched = context_rules.match_rules(path);

        // Merge matched rules across all files
        merge_matched_rules(&mut all_matched_rules, &matched);

        // Read file content
        let content = match fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("warning: failed to read {file_str}: {e}");
                continue;
            }
        };

        // Use tree-filtered searcher for IDF lookups based on CLI + matched trees
        let effective_trees = compute_effective_trees(trees, &matched.trees);
        let filtered_searcher = TreeFilteredSearcher::new(&searcher, effective_trees);

        // Analyze using the tree-filtered searcher for IDF lookups
        let analysis = analyze_context(path, &content, &filtered_searcher, &analysis_config);

        if !analysis.is_empty() {
            analyses.push((file_str.clone(), analysis, matched));
        }
    }

    if analyses.is_empty() {
        eprintln!("error: no analyzable files provided");
        return ExitCode::FAILURE;
    }

    // Handle --explain mode
    if explain {
        return output_context_explain(&analyses, json);
    }

    // Build a combined query expression from all file analyses
    let combined_expr = build_combined_context_expr_with_rules(
        &analyses
            .iter()
            .map(|(f, a, _)| (f.clone(), a.clone()))
            .collect::<Vec<_>>(),
        &all_matched_rules,
    );
    let Some(expr) = combined_expr else {
        println!("{}", dim("No context terms extracted."));
        return ExitCode::SUCCESS;
    };

    // Compute final effective trees: CLI trees intersected with rule-matched trees
    let effective_trees = compute_effective_trees(trees, &all_matched_rules.trees);

    // Execute the search using the query expression directly
    let mut searcher = searcher;
    let params = SearchParams {
        candidate_limit: 100,
        cutoff_ratio: 0.5,
        max_results: limit,
        aggregation_threshold: 0.5,
        disable_aggregation: false,
        trees: effective_trees,
        verbosity: verbose,
    };

    let mut results = match searcher.search_aggregated_expr(&expr, &params) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: context search failed: {e}");
            return ExitCode::FAILURE;
        }
    };

    // Handle automatic includes from matched rules
    inject_includes(&mut results, &all_matched_rules.include, &searcher, limit);

    // Output results (use query string representation for display)
    let query_display = expr.to_query_string();
    output_aggregated_results(
        &results,
        &query_display,
        list,
        false,
        json,
        verbose,
        &searcher,
    )
}

/// Merges matched rules from a file into the accumulated rules.
fn merge_matched_rules(accumulated: &mut MatchedRules, new_rules: &MatchedRules) {
    // Terms: concatenate (deduplicated)
    for term in &new_rules.terms {
        if !accumulated.terms.contains(term) {
            accumulated.terms.push(term.clone());
        }
    }

    // Includes: concatenate (deduplicated)
    for inc in &new_rules.include {
        if !accumulated.include.contains(inc) {
            accumulated.include.push(inc.clone());
        }
    }

    // Trees: intersection
    if !new_rules.trees.is_empty() {
        if accumulated.trees.is_empty() {
            // First rule with trees - take them as-is
            accumulated.trees = new_rules.trees.clone();
        } else {
            // Intersect with existing
            accumulated.trees.retain(|t| new_rules.trees.contains(t));
        }
    }
}

/// Computes effective trees by combining CLI-provided trees with rule-matched trees.
///
/// - If CLI trees are specified, they take precedence (intersected with rule trees if any)
/// - If only rule trees are specified, use those
/// - If neither is specified, return empty (meaning all trees)
fn compute_effective_trees(cli_trees: &[String], rule_trees: &[String]) -> Vec<String> {
    match (cli_trees.is_empty(), rule_trees.is_empty()) {
        (true, true) => Vec::new(),           // No restriction
        (true, false) => rule_trees.to_vec(), // Use rule trees
        (false, true) => cli_trees.to_vec(),  // Use CLI trees
        (false, false) => {
            // Intersect CLI and rule trees
            cli_trees
                .iter()
                .filter(|t| rule_trees.contains(t))
                .cloned()
                .collect()
        }
    }
}

/// Injects automatically included files from matched rules into the results.
///
/// Include paths are in the format "tree:path". Each matching chunk is added
/// to the results if not already present.
fn inject_includes(
    results: &mut Vec<AggregatedSearchResult>,
    includes: &[String],
    searcher: &Searcher,
    limit: usize,
) {
    if includes.is_empty() {
        return;
    }

    // Track existing doc IDs to avoid duplicates
    let existing_doc_ids: HashSet<String> =
        results.iter().map(|r| r.doc_id().to_string()).collect();

    // Parse and look up each include
    for include in includes {
        // Parse tree:path format
        let Some(colon_pos) = include.find(':') else {
            continue; // Invalid format, skip
        };
        let tree = &include[..colon_pos];
        let path = &include[colon_pos + 1..];

        // Find chunks matching this tree/path combination
        if let Ok(search_results) = searcher.get_by_path(tree, path) {
            if search_results.is_empty() {
                continue;
            }

            // Build the doc ID key
            let doc_id = format!("{tree}:{path}");
            if existing_doc_ids.contains(&doc_id) {
                continue; // Already in results
            }

            // Create an aggregated result for this included file
            if results.len() < limit {
                // Convert the first result to a candidate with score 0 (manual inclusion)
                let first_result = search_results.into_iter().next().unwrap();
                let mut candidate: SearchCandidate = first_result.into();
                candidate.score = 0.0; // Manual inclusion gets score 0

                results.push(AggregatedSearchResult::single(candidate));
            }
        }
    }
}

/// Boost applied to terms injected from context rules.
const INJECTED_TERM_BOOST: f32 = 2.0;

/// Builds a combined query expression from multiple file analyses, injecting rule-matched terms.
fn build_combined_context_expr_with_rules(
    analyses: &[(String, ContextAnalysis)],
    matched_rules: &MatchedRules,
) -> Option<QueryExpr> {
    let mut exprs: Vec<QueryExpr> = analyses
        .iter()
        .filter_map(|(_, analysis)| analysis.query_expr().cloned())
        .collect();

    // Inject terms from matched rules with a moderate boost
    for term in &matched_rules.terms {
        let term_expr = QueryExpr::Term(term.clone());
        let boosted = QueryExpr::boost(term_expr, INJECTED_TERM_BOOST);
        exprs.push(boosted);
    }

    if exprs.is_empty() {
        return None;
    }

    if exprs.len() == 1 {
        return exprs.into_iter().next();
    }

    // Combine multiple queries with OR
    Some(QueryExpr::or(exprs))
}

/// Outputs explain mode information for context analysis.
fn output_context_explain(
    analyses: &[(String, ContextAnalysis, MatchedRules)],
    json: bool,
) -> ExitCode {
    if json {
        // JSON output for explain mode
        let json_output = JsonContextExplain {
            files: analyses
                .iter()
                .map(|(file, analysis, matched)| JsonFileAnalysis {
                    file: file.clone(),
                    terms: analysis
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
                    query: analysis.query_string().map(|s| s.to_string()),
                    matched_rules: JsonMatchedRules {
                        terms: matched.terms.clone(),
                        trees: matched.trees.clone(),
                        include: matched.include.clone(),
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
        for (file, analysis, matched) in analyses {
            println!("{}", subheader(&format!("File: {file}")));
            println!();

            // Show matched rules if any
            if !matched.is_empty() {
                println!("{}", subheader("Matched rules:"));
                if !matched.terms.is_empty() {
                    println!("  Terms:   {}", matched.terms.join(", "));
                }
                if !matched.trees.is_empty() {
                    println!("  Trees:   {}", matched.trees.join(", "));
                }
                if !matched.include.is_empty() {
                    println!("  Include: {}", matched.include.join(", "));
                }
                println!();
            }

            // Show ranked terms
            println!("{}", subheader("Ranked terms:"));
            if analysis.ranked_terms.is_empty() {
                println!("  {}", dim("(none)"));
            } else {
                let mut table = Table::new();
                table.load_preset(UTF8_FULL_CONDENSED);
                table.set_header(vec!["Term", "Source", "Weight", "Freq", "IDF", "Score"]);

                for rt in &analysis.ranked_terms {
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
            if let Some(expr) = analysis.query_expr() {
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
                        body: Some(r.body.clone()),
                        content: Some(format!("> {}\n\n{}", r.breadcrumb, r.body)),
                        match_ranges: Some(
                            r.match_ranges
                                .iter()
                                .map(|range| JsonMatchRange {
                                    offset: range.start,
                                    length: range.end - range.start,
                                })
                                .collect(),
                        ),
                        title_match_ranges: Some(
                            r.title_match_ranges
                                .iter()
                                .map(|range| JsonMatchRange {
                                    offset: range.start,
                                    length: range.end - range.start,
                                })
                                .collect(),
                        ),
                        path_match_ranges: Some(
                            r.path_match_ranges
                                .iter()
                                .map(|range| JsonMatchRange {
                                    offset: range.start,
                                    length: range.end - range.start,
                                })
                                .collect(),
                        ),
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
    let cwd = match env::current_dir() {
        Ok(cwd) => cwd,
        Err(e) => {
            eprintln!("error: could not determine current directory: {e}");
            return ExitCode::FAILURE;
        }
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
    let cwd = match env::current_dir() {
        Ok(cwd) => cwd,
        Err(e) => {
            eprintln!(
                "{} could not determine current directory: {e}",
                error("error:")
            );
            return ExitCode::FAILURE;
        }
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
    let config = match Config::load(&cwd) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("{} {e}", error("error:"));
            return ExitCode::FAILURE;
        }
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
    let cwd = match env::current_dir() {
        Ok(cwd) => cwd,
        Err(e) => {
            eprintln!("error: could not determine current directory: {e}");
            return ExitCode::FAILURE;
        }
    };

    // Load merged config
    let config = match Config::load(&cwd) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("error: failed to load configuration: {e}");
            return ExitCode::FAILURE;
        }
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
    let cwd = match env::current_dir() {
        Ok(cwd) => cwd,
        Err(e) => {
            eprintln!("error: could not determine current directory: {e}");
            return ExitCode::FAILURE;
        }
    };

    let config = match Config::load(&cwd) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("error: failed to load configuration: {e}");
            return ExitCode::FAILURE;
        }
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
    let cwd = match env::current_dir() {
        Ok(cwd) => cwd,
        Err(e) => {
            eprintln!("error: could not determine current directory: {e}");
            return ExitCode::FAILURE;
        }
    };

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

    // Ensure index is fresh
    let searcher = match ensure_index_fresh(&config) {
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
                title: chunk.title.clone(),
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
    let cwd = match env::current_dir() {
        Ok(cwd) => cwd,
        Err(e) => {
            eprintln!("error: could not determine current directory: {e}");
            return ExitCode::FAILURE;
        }
    };

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

    // Ensure index is fresh
    let searcher = match ensure_index_fresh(&config) {
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
            breadcrumb(&chunk.breadcrumb)
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
        let chunk_label = if chunk.depth == 0 {
            format!("{} (document)", chunk.id)
        } else {
            format!("{} (depth {})", chunk.id, chunk.depth)
        };
        println!("--- {} ---", header(&chunk_label));
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
    let cwd = match env::current_dir() {
        Ok(cwd) => cwd,
        Err(e) => {
            eprintln!("error: could not determine current directory: {e}");
            return ExitCode::FAILURE;
        }
    };

    let config = match Config::load(&cwd) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };

    // Compile context patterns
    let patterns = match CompiledContextPatterns::compile(&config.context) {
        Ok(patterns) => patterns,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };

    // Create analyzer and analyze the file
    let analyzer = ContextAnalyzer::new(&config.context, patterns);
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
