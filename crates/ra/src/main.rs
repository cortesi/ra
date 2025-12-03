use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "ra")]
#[command(about = "Research Assistant - Knowledge management for AI agents")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
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

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Search { queries, limit, list } => {
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
        Commands::Init { global } => {
            println!("init (global={})", global);
        }
        Commands::Check => {
            println!("check");
        }
        Commands::Update => {
            println!("update");
        }
        Commands::Status => {
            println!("status");
        }
        Commands::Agents { stdout, claude, gemini, all } => {
            println!("agents (stdout={}, claude={}, gemini={}, all={})", stdout, claude, gemini, all);
        }
    }
}
