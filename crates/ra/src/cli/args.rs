//! Clap argument definitions for the `ra` CLI.

use std::{env, process::exit};

use clap::{Args, CommandFactory, Parser, Subcommand, error::ErrorKind};

/// Parse a keyword extraction algorithm from a string.
fn parse_algorithm(s: &str) -> Result<ra_context::KeywordAlgorithm, String> {
    s.parse()
}

/// Top-level CLI options.
#[derive(Parser)]
#[command(name = "ra")]
#[command(about = "Research Assistant - Knowledge management for AI agents")]
pub struct Cli {
    /// Subcommand to execute.
    #[command(subcommand)]
    pub command: Commands,
}

/// Shared flags that control search parameters for search-like commands.
#[derive(Args, Debug, Clone, Default)]
pub struct SearchParamsArgs {
    /// Maximum results to return after aggregation [default: 10]
    #[arg(short = 'n', long)]
    pub limit: Option<usize>,

    /// Disable hierarchical aggregation
    #[arg(long)]
    pub no_aggregation: bool,

    /// Size of the aggregation pool (candidates available for hierarchical aggregation) [default: 500]
    #[arg(long)]
    pub aggregation_pool_size: Option<usize>,

    /// Score ratio threshold for relevance cutoff (0.0-1.0) [default: 0.3]
    #[arg(long)]
    pub cutoff_ratio: Option<f32>,

    /// Sibling ratio threshold for aggregation [default: 0.1]
    #[arg(long)]
    pub aggregation_threshold: Option<f32>,

    /// Verbosity level (-v for summary, -vv for full details)
    #[arg(short = 'v', long, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Limit results to specific trees (can be specified multiple times)
    #[arg(short = 't', long = "tree")]
    pub trees: Vec<String>,
}

/// Shared output mode flags.
#[derive(Args, Debug, Clone, Default)]
pub struct OutputArgs {
    /// Output titles and snippets only
    #[arg(long)]
    pub list: bool,

    /// Output only lines containing matches
    #[arg(long)]
    pub matches: bool,

    /// Output in JSON format
    #[arg(long)]
    pub json: bool,
}

/// Shared explain/debug flag.
#[derive(Args, Debug, Clone, Default)]
pub struct ExplainArgs {
    /// Show parsed query AST / analysis and generated query without searching
    #[arg(long)]
    pub explain: bool,
}

/// Arguments for `ra search`.
#[derive(Args, Debug, Clone)]
pub struct SearchCommand {
    /// Search queries
    #[arg(required = true)]
    pub queries: Vec<String>,

    #[command(flatten)]
    /// Search parameter overrides.
    pub params: SearchParamsArgs,

    #[command(flatten)]
    /// Output formatting flags.
    pub output: OutputArgs,

    #[command(flatten)]
    /// Explain/debug flags.
    pub explain: ExplainArgs,

    /// Fuzzy matching edit distance (0=exact, 1-2=fuzzy) [default: 1]
    #[arg(short = 'f', long)]
    pub fuzzy: Option<u8>,
}

/// Arguments for `ra context`.
#[derive(Args, Debug, Clone)]
pub struct ContextCommand {
    /// Files to analyze
    #[arg(required = true)]
    pub files: Vec<String>,

    #[command(flatten)]
    /// Search parameter overrides.
    pub params: SearchParamsArgs,

    /// Maximum terms to include in the query [default: 50]
    #[arg(long)]
    pub terms: Option<usize>,

    /// Keyword extraction algorithm: textrank (graph-based), tfidf (corpus-aware),
    /// rake (co-occurrence), yake (statistical) [default: textrank]
    #[arg(short = 'a', long, value_parser = parse_algorithm)]
    pub algorithm: Option<ra_context::KeywordAlgorithm>,

    #[command(flatten)]
    /// Output formatting flags.
    pub output: OutputArgs,

    #[command(flatten)]
    /// Explain/debug flags.
    pub explain: ExplainArgs,

    /// Fuzzy matching edit distance (0=exact, 1-2=fuzzy) [default: 1]
    #[arg(short = 'f', long)]
    pub fuzzy: Option<u8>,
}

/// Arguments for `ra get`.
#[derive(Args, Debug, Clone)]
pub struct GetCommand {
    /// Chunk or document ID (tree:path#slug or tree:path)
    pub id: String,

    /// Return full document even if ID specifies a chunk
    #[arg(long)]
    pub full_document: bool,

    /// Output in JSON format
    #[arg(long)]
    pub json: bool,
}

/// Arguments for `ra likethis`.
#[derive(Args, Debug, Clone)]
pub struct LikeThisCommand {
    /// Source: chunk ID (tree:path#slug) or file path
    pub source: String,

    #[command(flatten)]
    /// Search parameter overrides.
    pub params: SearchParamsArgs,

    #[command(flatten)]
    /// Output formatting flags.
    pub output: OutputArgs,

    #[command(flatten)]
    /// Explain/debug flags.
    pub explain: ExplainArgs,

    // MoreLikeThis-specific parameters
    /// Minimum document frequency for terms
    #[arg(long, default_value = "1")]
    pub min_doc_freq: u64,

    /// Maximum document frequency for terms
    #[arg(long)]
    pub max_doc_freq: Option<u64>,

    /// Minimum term frequency in source document
    #[arg(long, default_value = "1")]
    pub min_term_freq: usize,

    /// Maximum query terms to use
    #[arg(long, default_value = "25")]
    pub max_terms: usize,

    /// Minimum word length
    #[arg(long, default_value = "3")]
    pub min_word_len: usize,

    /// Maximum word length
    #[arg(long, default_value = "40")]
    pub max_word_len: usize,

    /// Boost factor for terms
    #[arg(long, default_value = "1.0")]
    pub boost: f32,
}

/// Arguments for `ra init`.
#[derive(Args, Debug, Clone)]
pub struct InitCommand {
    /// Create global ~/.ra.toml instead
    #[arg(long)]
    pub global: bool,

    /// Overwrite existing configuration file
    #[arg(long)]
    pub force: bool,
}

/// Arguments for `ra ls`.
#[derive(Args, Debug, Clone)]
pub struct LsCommand {
    /// Show detailed information.
    #[arg(short = 'l', long)]
    pub long: bool,

    /// What to list.
    #[command(subcommand)]
    pub what: LsWhat,
}

/// Arguments for `ra agents`.
#[derive(Args, Debug, Clone)]
pub struct AgentsCommand {
    /// Print to stdout instead of writing files
    #[arg(long)]
    pub stdout: bool,

    /// Generate CLAUDE.md
    #[arg(long)]
    pub claude: bool,

    /// Generate GEMINI.md
    #[arg(long)]
    pub gemini: bool,

    /// Generate all agent file variants
    #[arg(long)]
    pub all: bool,
}

/// Supported `ra` subcommands.
#[derive(Subcommand)]
pub enum Commands {
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
    Search(SearchCommand),

    /// Get relevant context for files being worked on
    Context(ContextCommand),

    /// Retrieve a specific chunk or document by ID
    Get(GetCommand),

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
    LikeThis(LikeThisCommand),

    /// Inspect documents or context signals
    Inspect {
        /// What to inspect
        #[command(subcommand)]
        what: InspectWhat,
    },

    /// Initialize ra configuration in current directory
    Init(InitCommand),

    /// Force rebuild of search index
    Update,

    /// Show status and validate configuration
    Status,

    /// Show effective configuration settings
    Config,

    /// List trees, documents, or chunks
    Ls(LsCommand),

    /// Generate AGENTS.md, CLAUDE.md, GEMINI.md
    Agents(AgentsCommand),
}

/// What to list with `ra ls`.
#[derive(Clone, Copy, Subcommand, Debug)]
pub enum LsWhat {
    /// List all configured trees
    Trees,
    /// List all indexed documents
    Docs,
    /// List all indexed chunks
    Chunks,
}

/// What to inspect with `ra inspect`.
#[derive(Clone, Subcommand)]
pub enum InspectWhat {
    /// Show how ra parses a document
    Doc {
        /// File to inspect
        file: String,

        /// Keyword extraction algorithm: tfidf, rake, textrank, yake [default: textrank]
        #[arg(short = 'a', long, value_parser = parse_algorithm)]
        algorithm: Option<ra_context::KeywordAlgorithm>,

        /// Maximum keywords to display [default: 20]
        #[arg(short = 'n', long)]
        limit: Option<usize>,
    },
    /// Show context signals for a file
    Ctx {
        /// File to analyze for context
        file: String,
    },
}

/// Parses CLI arguments, printing hierarchical help for top-level `--help`.
pub fn parse_cli() -> Cli {
    match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            if e.kind() == ErrorKind::DisplayHelp {
                let args: Vec<_> = env::args().collect();
                if args.len() <= 2 {
                    print_hierarchical_help();
                    exit(0);
                }
            }
            e.exit();
        }
    }
}

/// Prints custom help with hierarchical subcommand display.
fn print_hierarchical_help() {
    let cmd = Cli::command();
    let about = cmd.get_about().map(|s| s.to_string()).unwrap_or_default();

    println!("{about}");
    println!();
    println!("Usage: ra <COMMAND>");
    println!();
    println!("Commands:");

    for sub in cmd.get_subcommands() {
        let name = sub.get_name();
        if name == "help" {
            continue;
        }

        let about = sub.get_about().map(|s| s.to_string()).unwrap_or_default();
        println!("  {name:10} {about}");

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

#[cfg(test)]
mod tests {
    use ra_config::{
        DEFAULT_AGGREGATION_POOL_SIZE, DEFAULT_AGGREGATION_THRESHOLD, DEFAULT_CONTEXT_TERMS,
        DEFAULT_CUTOFF_RATIO, DEFAULT_SEARCH_LIMIT,
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

        let limit_help = get_arg_help(&cmd, "search", "limit");
        assert!(
            limit_help.contains(&format!("[default: {}]", DEFAULT_SEARCH_LIMIT)),
            "search --limit help should contain default {}: {limit_help}",
            DEFAULT_SEARCH_LIMIT
        );

        let aggregation_pool_size_help = get_arg_help(&cmd, "search", "aggregation_pool_size");
        assert!(
            aggregation_pool_size_help
                .contains(&format!("[default: {}]", DEFAULT_AGGREGATION_POOL_SIZE)),
            "search --aggregation-pool-size help should contain default {}: {aggregation_pool_size_help}",
            DEFAULT_AGGREGATION_POOL_SIZE
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

        let lt_limit_help = get_arg_help(&cmd, "likethis", "limit");
        assert!(
            lt_limit_help.contains(&format!("[default: {}]", DEFAULT_SEARCH_LIMIT)),
            "likethis --limit help should contain default {}: {lt_limit_help}",
            DEFAULT_SEARCH_LIMIT
        );
    }
}
