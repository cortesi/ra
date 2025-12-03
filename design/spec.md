# ra: Research Assistant

A knowledge management system for autonomous coding and writing agents.

## Overview

ra provides structured access to curated knowledge bases for AI agents. Users
maintain collections of markdown documents—project-specific and global—that
provide context for agent tasks. Because the full corpus may exceed practical
context limits, ra indexes these documents and exposes fuzzy search tools that
agents use to retrieve relevant context on demand.

### Use Cases

- Project documentation and coding style guides for development agents
- Background research and world-building notes for writing agents
- Business context and historical data for report-generation agents

### Design Principles

- **Composable**: Hierarchical configuration overlays global and local knowledge
- **Lean**: Chunk-level retrieval keeps context focused
- **Simple**: Markdown in, markdown out—no proprietary formats
- **Fast**: Tantivy-powered full-text search with incremental indexing
- **Runtime over compile-time**: Agents search for what they need when they need it, rather than anticipating needs at configuration time
- **Minimal agent API**: Agents provide keywords; ra handles query construction, field boosting, tree selection, and ranking internally

## Configuration

Configuration uses TOML files named `.ra.toml`. ra resolves configuration by:

1. Walking up the directory tree from CWD to the filesystem root, collecting any `.ra.toml` files found
2. Loading `~/.ra.toml` (if present) as the global config with lowest precedence

Configs are merged with files closer to CWD taking precedence over those further up, and all of them taking precedence over the global `~/.ra.toml`.

### Merge Semantics

- Child configurations override parent values for scalar settings
- Tree definitions are merged by name (child overrides parent if same name)
- Include patterns are concatenated (child patterns evaluated after parent)
- Context patterns (`[context.patterns]`) are merged; child patterns take precedence for identical globs

### Configuration Schema

```toml
# ~/.ra.toml (global)

[settings]
default_limit = 5           # results per query
local_boost = 1.5           # relevance multiplier for non-global trees
chunk_at_headings = true    # split documents at h1 boundaries
max_chunk_size = 50000      # warn if any chunk exceeds this (characters)

[search]
fuzzy = true                # enable fuzzy matching for typo tolerance
fuzzy_distance = 1          # Levenshtein distance (0, 1, or 2)
stemmer = "english"         # stemming language

[context]
limit = 10                  # default chunks for context command
min_term_frequency = 2      # ignore terms appearing less than this in source
min_word_length = 4         # ignore short words in content analysis
max_word_length = 30        # ignore very long tokens
sample_size = 50000         # max bytes to analyze from large files

# Glob pattern to search term mappings
# Left side is a glob, right side is additional search terms
# Multiple patterns can match the same file - terms are merged
[context.patterns]
"*.rs" = ["rust"]
"*.py" = ["python"]
"*.tsx" = ["typescript", "react"]
"*.jsx" = ["javascript", "react"]
"*.ts" = ["typescript"]
"*.js" = ["javascript"]
"*.go" = ["golang"]
"*.rb" = ["ruby"]
"*.ex" = ["elixir"]
"*.exs" = ["elixir"]
"*.clj" = ["clojure"]
"*.hs" = ["haskell"]
"*.ml" = ["ocaml"]
"*.swift" = ["swift"]
"*.kt" = ["kotlin"]
"*.java" = ["java"]
"*.c" = ["c"]
"*.cpp" = ["cpp"]
"*.h" = ["c", "cpp"]
"*.hpp" = ["cpp"]

[trees]
global = "~/docs"           # named tree pointing to a directory
```

```toml
# ./project/.ra.toml (local)

[trees]
local = "./docs"            # project-specific documentation

# Include patterns select which files from which trees to index
[[include]]
tree = "global"
pattern = "**/rust/**"      # only rust-related global docs

[[include]]
tree = "global" 
pattern = "**/git/**"       # and git-related global docs

[[include]]
tree = "local"
pattern = "**/*"            # all local docs

# Project-specific context patterns (merged with global patterns)
[context.patterns]
"src/api/**" = ["http", "handlers", "routing", "REST"]
"src/auth/**" = ["authentication", "security", "jwt"]
"src/db/**" = ["database", "queries", "migrations"]
"tests/**" = ["testing", "fixtures"]
```

### Pattern Matching

Patterns use glob syntax (via the `globset` crate) and match against paths relative to the tree root. If no include patterns are specified for a tree, all `.md` and `.txt` files are included.

### Tree Resolution

Trees defined in child configs shadow parent definitions of the same name. A tree path may be:

- Absolute: `~/docs`, `/home/user/docs`
- Relative to config file: `./docs`, `../shared/docs`

### Global vs Local Trees

Trees defined in `~/.ra.toml` are **global**. Trees defined in any other `.ra.toml` file are **local**. This distinction affects ranking: local trees receive the `local_boost` multiplier (default 1.5x), giving project-specific documentation higher relevance than global reference material.

If a child config redefines a tree with the same name as one in `~/.ra.toml`, the child's definition shadows the global one, and the tree is treated as local.

## Document Format

ra indexes markdown (`.md`) and plain text (`.txt`) files.

### Supported File Types

| Extension | Handling |
|-----------|----------|
| `.md` | Parsed for frontmatter, chunked at h1 headings |
| `.txt` | Indexed as a single chunk, filename used as title |

Binary files (images, PDFs, etc.) are silently ignored. Use `ra check` to see warnings about binary files in tree paths.

### Symlinks

Symbolic links to files are followed. Symbolic links to directories are ignored (not descended into). This avoids cycle detection complexity while still allowing symlinked individual documents.

### Frontmatter

YAML frontmatter is parsed when present in markdown files:

```markdown
---
title: Rust Error Handling Patterns
tags: [rust, errors, patterns]
---

# Content starts here
```

- `title`: Indexed with elevated weight; used in search results
- `tags`: Indexed with elevated weight; supports Obsidian-style tags

If no frontmatter title exists, the first h1 heading is used as the document title.

### Chunking

When `chunk_at_headings` is enabled (default), documents are split at h1 (`#`) boundaries. Each chunk inherits the document's frontmatter metadata and includes:

- The h1 heading as chunk title
- All content until the next h1 or end of document

**Preamble handling**: Content before the first h1 heading (if any) becomes its own chunk with:
- Title: frontmatter `title` if present, otherwise the filename
- Slug: `preamble` (e.g., `tree:path/file.md#preamble`)

Documents without h1 headings are indexed as a single chunk (same as preamble behavior).

### Chunk Identity

Each chunk has a unique identifier: `{tree}:{relative_path}#{heading_slug}`

Special cases:
- Preamble (content before first h1): `{tree}:{relative_path}#preamble`
- Documents without h1s: `{tree}:{relative_path}#preamble`
- Plain text files (`.txt`): `{tree}:{relative_path}` (no fragment)

**Heading slug algorithm** (GitHub-compatible):

1. Convert to lowercase
2. Remove punctuation except hyphens and spaces
3. Replace spaces with hyphens
4. Collapse consecutive hyphens
5. Trim leading/trailing hyphens
6. If slug already used in this document, append `-1`, `-2`, etc.

Examples:
- `"The Result<T> Type!"` → `the-resultt-type`
- First `"# Overview"` → `overview`
- Second `"# Overview"` → `overview-1`
- Content before first h1 → `preamble`

### Chunk Size Limits

The `max_chunk_size` setting (default: 50,000 characters) is a warning threshold, not a hard limit. Chunks exceeding this size are still indexed, but trigger warnings during `ra check` and indexing to alert you that some chunks may be too large for effective context use.

For documents with very long sections, consider adding more h1 headings to create natural split points.

## Indexing

### Index Location

The search index is stored in `.ra/index/` as a sibling to the most specific (closest to CWD) `.ra.toml` found during config resolution. If only `~/.ra.toml` exists, the index is stored in `~/.ra/index/`.

Example: If CWD is `/home/user/projects/foo/src` and the nearest config is `/home/user/projects/foo/.ra.toml`, the index lives at `/home/user/projects/foo/.ra/index/`.

### Index Schema

Each chunk is indexed with the following fields:

| Field | Type | Options | Weight |
|-------|------|---------|--------|
| `id` | Text | STORED | — |
| `title` | Text | TEXT, STORED | 3.0 |
| `tags` | Text | TEXT, STORED | 2.5 |
| `path` | Text | TEXT, STORED | 2.0 |
| `path_components` | Text | TEXT | 2.0 |
| `tree` | Text | STRING, STORED, FAST | 1.0 |
| `body` | Text | TEXT, STORED | 1.0 |
| `mtime` | Date | INDEXED, FAST | — |

Notes on Tantivy field options:
- **TEXT**: Tokenized and indexed with positions (enables phrase queries)
- **STRING**: Indexed as a single token (for exact matching)
- **STORED**: Original value retrievable from index
- **FAST**: Columnar storage for fast filtering/sorting

The `path_components` field contains the path split into individual directory/file segments, enabling matches on partial paths.

### Incremental Updates

ra maintains a manifest of indexed files with their modification times. On search:

1. Compare current file mtimes against manifest
2. If any files are stale, added, or removed, trigger incremental reindex
3. Only reprocess changed files

Explicit rebuild via `ra update` forces full reindexing.

### Index Versioning

The index stores a hash of configuration settings that affect indexing:

- Schema version (internal, bumped when field definitions change)
- Stemmer language
- Text analyzer settings
- Chunking settings (`chunk_at_headings`)

On any ra operation that reads the index:

1. Compute hash of current config
2. Compare against stored hash in index metadata
3. If mismatch, trigger full reindex automatically

This ensures the index always reflects current configuration. Users don't need to remember to run `ra update` after changing settings like `stemmer`.

`ra check` reports whether the index matches current config:
- "index: current" - hash matches
- "index: stale (config changed)" - will rebuild on next search
- "index: missing" - no index exists

### Concurrent Access

Multiple ra processes (or agents) may access the same index simultaneously. Tantivy provides safe concurrent reads and serialized writes. Index corruption is not possible under concurrent access.

When a write is in progress, readers continue to see the previous consistent state until the write commits.

## Search

ra's search is powered by Tantivy, a Lucene-inspired full-text search engine. This section documents both what we get from Tantivy and how ra configures it.

### Tokenization & Text Analysis

ra uses a custom text analyzer pipeline:

1. **SimpleTokenizer**: Splits on whitespace and punctuation
2. **LowerCaser**: Normalizes to lowercase
3. **RemoveLongFilter**: Drops tokens exceeding 40 characters
4. **Stemmer**: Reduces words to stems (English by default)

Tantivy provides stemmers for 18 languages: Arabic, Danish, Dutch, English, Finnish, French, German, Greek, Hungarian, Italian, Norwegian, Portuguese, Romanian, Russian, Spanish, Swedish, Tamil, and Turkish. Third-party tokenizers exist for Chinese, Japanese, and Korean.

### Query Processing

Agents provide simple search terms. ra handles the complexity internally.

**External API:**

| Input | Interpretation |
|-------|----------------|
| `error handling` | Keywords, AND'd together, fuzzy-matched |
| `"error handling"` | Exact phrase match |
| `"error handling" "logging"` | Multi-topic: both phrases searched, results combined |

That's it. No field specifiers, no boolean operators, no tree selection. The agent describes what it wants to know; ra figures out how to find it.

**Multi-topic research:**

Agents often need context across several domains before acting. Rather than multiple round-trips:

```
ra search "error handling" "logging conventions" "API structure"
```

Returns results for all topics in a single response, labeled by query. This supports the "research phase" pattern where an agent gathers broad context before making decisions.

**Internal query construction:**

Behind the simple API, ra builds sophisticated Tantivy queries:

- Terms are AND'd for precision (agents need focused results, not exhaustive recall)
- Fuzzy matching applied per config (see Fuzzy Matching section below)
- Quoted strings become phrase queries
- All text fields searched with configured boosts (title 3.0x, tags 2.5x, path 2.0x, body 1.0x)
- Results from all configured trees, ranked by BM25 + locality boost

The full Tantivy query syntax (boolean operators, field specifiers, ranges, slop) remains available via a `--raw` flag for debugging and power users, but is deliberately undocumented for agent use.

### Fuzzy Matching

Fuzzy matching is controlled by two settings:

- `fuzzy = true|false`: Master switch for fuzzy matching (default: true)
- `fuzzy_distance = 0|1|2`: Levenshtein distance when enabled (default: 1)

When `fuzzy = true`:
- Terms longer than 4 characters are fuzzy-matched at the configured distance
- Terms of 4 characters or fewer are matched exactly (avoids false positives on short words)
- Transpositions count as distance 1 (e.g., "teh" matches "the")

When `fuzzy = false`:
- All terms are matched exactly, no edit distance tolerance

Example: With defaults, `ra search "rust eror handling"` finds "rust error handling" despite the typo ("eror" is 4+ chars, so fuzzy applies).

### Result Ranking

Results are ranked by BM25, the same algorithm used by Elasticsearch and Lucene. BM25 considers:

- **Term frequency**: How often the term appears in the document
- **Inverse document frequency**: Rarer terms are weighted higher
- **Field length normalization**: Shorter fields (like titles) get boosted

ra applies additional ranking adjustments:

1. **Field boosting**: Title (3.0x), tags (2.5x), path (2.0x), body (1.0x)
2. **Tree locality boost**: Local trees (those not defined in `~/.ra.toml`) get the `local_boost` multiplier (default 1.5x)

### Snippet Generation

For list mode output, ra uses Tantivy's SnippetGenerator to produce excerpts with highlighted matches. Snippets are limited to approximately 150 characters and prioritize fragments containing query terms.

### Content Analysis (MoreLikeThis)

The `ra context` command uses Tantivy's MoreLikeThisQuery to analyze input files and find relevant documentation. MoreLikeThis:

- Extracts significant terms from input based on TF-IDF
- Filters by term frequency, word length, and stop words
- Builds a weighted query from extracted terms
- Returns documents similar to the input

Configuration options in `[context]` tune this behavior (min term frequency, word length bounds, etc.).

### Additional Tantivy Features

These Tantivy features are available but not exposed in the default CLI:

- **RegexQuery**: Match terms against regular expressions.
- **DisjunctionMaxQuery**: Score by best-matching clause rather than sum. Useful when searching across fields with different semantics.

These could be exposed via future CLI extensions.

### Output Modes

**Full context mode** (`ra search`): Returns complete chunk content with metadata wrapper, suitable for direct inclusion in agent context.

**List mode** (`ra search --list`): Returns chunk identifiers and titles with highlighted excerpts, suitable for human review or agent decision-making about what to retrieve.

## Command Line Interface

```
ra - Research Assistant

USAGE:
    ra <COMMAND>

COMMANDS:
    search <QUERY>...   Search and output matching chunks
    context <FILE>...   Get relevant context for files being worked on
    get <ID>            Retrieve a specific chunk or document by ID
    inspect <FILE>      Show how ra parses a file (debug)
    init                Initialize ra configuration in current directory
    check               Validate configuration and diagnose issues
    update              Force rebuild of search index
    status              Show configuration, trees, and index statistics
    agents              Generate AGENTS.md, CLAUDE.md, GEMINI.md
    help                Print help information

OPTIONS:
    -n, --limit <N>        Results per query (default: 5)
    --max-tokens <N>       Total token budget across all results
    --list                 Output titles and snippets only, not full content
    --json                 Output in JSON format
    --raw                  Pass query directly to Tantivy (power users)
```

### Commands

#### `ra init`

Creates a starter `.ra.toml` in the current directory:

```bash
ra init                    # create .ra.toml with defaults
ra init --global           # create ~/.ra.toml
```

Also adds `.ra/` to `.gitignore` if a git repository is detected.

#### `ra check`

Validates configuration and reports issues:

```bash
ra check
```

Checks performed:

- Configuration syntax and schema validity
- Tree paths exist and are accessible
- Include patterns match at least one file
- Warns if any chunks exceed `max_chunk_size`
- Warns if binary files are present in tree paths
- Index status (current, stale, or missing)

Exit codes: 0 = OK, 1 = warnings, 2 = errors.

#### `ra status`

Displays current configuration and index state:

```bash
ra status
```

Output includes:

- Effective merged configuration (all `.ra.toml` files combined)
- List of configured trees with resolved paths
- Document and chunk counts per tree
- Index location, size, and status (current/stale/missing)
- Last index update time

#### `ra inspect`

Debug command showing exactly how ra parses a file, without modifying the index:

```bash
ra inspect path/to/document.md
```

Output includes:

- Detected file type
- Parsed frontmatter (title, tags, other fields)
- Chunk breakdown with:
  - Generated chunk ID and slug
  - Title (from heading or fallback)
  - Character count
  - Size warnings if exceeding `max_chunk_size`
- Any parse errors or warnings

Useful for debugging why a document isn't appearing in search results or understanding how chunking decisions are made.

Example output:

```
File: docs/guide.md
Type: markdown
Frontmatter:
  title: "Getting Started"
  tags: [intro, setup]

Chunks:
  1. docs/guide.md#preamble
     Title: "Getting Started" (from frontmatter)
     Size: 234 chars

  2. docs/guide.md#installation
     Title: "Installation"
     Size: 1,892 chars

  3. docs/guide.md#configuration
     Title: "Configuration"
     Size: 3,421 chars
```

#### `ra search`

Search with optional list mode:

```bash
# Full content output (default)
ra search "error handling"

# Titles and snippets only
ra search --list "error handling"

# Multi-topic research
ra search "error handling" "logging" "API design"
```

Results are grouped by query in the output.

#### `ra context`

Analyzes input files and returns relevant knowledge base context. Designed for the "research phase" where an agent needs context before working on specific files.

```bash
# Context for a single file
ra context src/api/handlers.rs

# Context for multiple files
ra context src/api/handlers.rs src/models/user.rs

# Limit results
ra context --limit 5 src/main.rs

# Recently changed files
ra context $(git diff --name-only HEAD~1)
```

**Algorithm:**

1. **Path analysis**: Extract path components as search terms (e.g., `src/auth/oauth.rs` adds "auth", "oauth")

2. **Pattern matching**: Match file path against `[context.patterns]` globs and collect all matching search terms (e.g., `src/api/handlers.rs` might match both `*.rs` → "rust" and `src/api/**` → "http", "handlers")

3. **Content analysis**: Use Tantivy's MoreLikeThisQuery to extract significant terms from file content based on TF-IDF

4. **Query execution**: Combine path, pattern, and content signals into a unified search

5. **Deduplication**: When multiple input files match the same chunk, deduplicate and merge scores

**Options:**

```
ra context <FILE>... [OPTIONS]

OPTIONS:
    -n, --limit <N>        Maximum chunks to return (default: 10)
    --max-tokens <N>       Token budget for results
    --list                 Output titles and snippets only
    --json                 Output in JSON format
```

**Edge cases:**

- Binary files are skipped with a warning
- Very short files rely more heavily on path and extension signals
- Large files are sampled (first 50KB) for content analysis

### Output Format


Default output wraps content with metadata:

```
─── global:rust/error-handling.md#result-type ───
# The Result Type

Content of the chunk...

─── local:docs/api.md#errors ───
# Errors

More content...
```

JSON output (with `--json`):

```json
{
  "queries": [
    {
      "query": "error handling",
      "results": [
        {
          "id": "global:rust/error-handling.md#result-type",
          "tree": "global",
          "path": "rust/error-handling.md",
          "title": "The Result Type",
          "score": 12.5,
          "snippet": "...the <em>Result</em> type for <em>error handling</em>...",
          "content": "# The Result Type\n\nContent..."
        }
      ],
      "total_matches": 8
    }
  ]
}
```

With `--list`, the `content` field is omitted and only `snippet` is included, reducing output size for browsing results.

### Token Limiting

The `--max-tokens` flag limits results to fit within a token budget. ra uses `tiktoken-rs` with the `cl100k_base` encoding for token counting, which provides reasonable accuracy across modern LLMs.

Note: Different models use different tokenizers, so counts are approximate. cl100k_base (GPT-4's tokenizer) typically produces counts within 10-20% of Claude's tokenizer. For code-heavy content, this is much more reliable than character-based heuristics, which can undercount by 50% or more.

## MCP Server

ra exposes an MCP server for agent integration using the `rmcp` crate.

### Transport

- Default: stdio transport (launched by agent runtime)
- Optional: SSE transport for persistent server mode

### Working Directory

The MCP server operates relative to its current working directory when launched. This determines which `.ra.toml` files are discovered and merged. ra is designed for per-project use—there is no global daemon.

Agents or runtimes launching ra should set the working directory to the project root.

### Tools

#### `search`

Search the knowledge base and return matching chunks. Supports single queries or multi-topic research.

```json
{
  "name": "search",
  "description": "Search the knowledge base. Use array of queries for multi-topic research in one call.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "queries": {
        "oneOf": [
          { "type": "string" },
          { "type": "array", "items": { "type": "string" } }
        ],
        "description": "Search term(s). Quote for exact phrase. Array for multi-topic."
      },
      "limit": {
        "type": "integer",
        "description": "Results per query (default: 5)"
      },
      "max_tokens": {
        "type": "integer",
        "description": "Total token budget across all results"
      },
      "list": {
        "type": "boolean",
        "description": "Return snippets only, omit full content (default: false)"
      }
    },
    "required": ["queries"]
  }
}
```

Example multi-topic call:
```json
{
  "queries": ["error handling", "logging conventions", "API structure"],
  "limit": 3
}
```

#### `context`

Analyze files and return relevant knowledge base context. Ideal for the research phase before working on specific files.

```json
{
  "name": "context",
  "description": "Get relevant knowledge base context for files being worked on. Analyzes file paths, extensions, and content.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "files": {
        "type": "array",
        "items": { "type": "string" },
        "description": "File paths to analyze for context"
      },
      "limit": {
        "type": "integer",
        "description": "Maximum chunks to return (default: 10)"
      },
      "max_tokens": {
        "type": "integer",
        "description": "Token budget for results"
      },
      "list": {
        "type": "boolean",
        "description": "Return snippets only, omit full content (default: false)"
      }
    },
    "required": ["files"]
  }
}
```

Example:
```json
{
  "files": ["src/api/handlers.rs", "src/models/user.rs"],
  "limit": 10
}
```

#### `get`

Retrieve a specific chunk or full document by identifier.

```json
{
  "name": "get",
  "description": "Retrieve a specific document or chunk by ID",
  "inputSchema": {
    "type": "object",
    "properties": {
      "id": {
        "type": "string",
        "description": "Chunk ID (tree:path#heading) or document ID (tree:path)"
      },
      "full_document": {
        "type": "boolean",
        "description": "Return full document even if ID specifies a chunk"
      }
    },
    "required": ["id"]
  }
}
```

#### `list_sources`

Introspect available trees and their statistics.

```json
{
  "name": "list_sources",
  "description": "List available knowledge trees and their statistics",
  "inputSchema": {
    "type": "object",
    "properties": {}
  }
}
```

### Index Freshness

The MCP server checks index freshness on each `search` call and performs incremental updates as needed. This ensures agents always search current content without requiring explicit update calls.

## Agent File Generation

ra generates agent instruction files (AGENTS.md, CLAUDE.md, GEMINI.md) that teach agents to use ra as their primary knowledge source.

### Philosophy

Traditional approaches to agent configuration try to anticipate what context an agent needs and bake it in at generation time—conditional includes based on project language, framework detection, etc. This front-loads complexity and scales poorly.

ra inverts this: generate minimal static instructions that teach the agent to search for what it needs at runtime. The agent pays one round-trip for "planning and research," but gains flexibility—any number of languages, frameworks, conventions, and architectural patterns can live in the searchable knowledge base without bloating the agent file.

### Template System

Templates are markdown files that ra concatenates to produce the final agent files:

1. **Project template**: `.agents.md` in the project root (optional)
2. **Global template**: `~/.agents.md` (optional)

The project template appears first in output, followed by the global template. No conditional logic—that complexity moves to runtime search.

### Dynamic Injection

After concatenating templates, ra appends a generated section containing:

- **Clear instructions**: Emphatic guidance to use ra before making decisions
- **Usage examples**: How to search single topics and multi-topic research
- **Search triggers**: Specific situations that should prompt a search

### Generated Instructions

The injected instructions are designed to override agent default behavior of proceeding with general knowledge. Key messaging:

1. **ra is the source of truth**: Project conventions differ from training data; ra contains the authoritative versions
2. **Search before acting**: Query ra before writing code, suggesting refactors, or making architectural decisions
3. **Search triggers**: Specific situations that should prompt a search (new file, unfamiliar terminology, style questions)

Example generated section:

```markdown
## ra Knowledge Base

This project uses ra for knowledge management. **Search ra before making significant decisions.**

### Why This Matters

This project's conventions, patterns, and standards WILL differ from your training data. Proceeding without consulting ra means you will miss project-specific requirements that override general best practices.

### How to Use

**Get context for files you're working on:**
- `ra context src/api/handlers.rs` - context relevant to this file
- `ra context src/*.rs` - context for multiple files

**Search for specific topics:**
- `ra search "error handling"`
- `ra search "error handling" "logging"` - multi-topic in one call

### When to Use

- **Starting work on a file**: Run `ra context` on the files you'll modify
- **Before writing new code**: Search for relevant patterns and conventions
- **Encountering unfamiliar terms**: Search to understand project-specific concepts
- **Choosing between approaches**: Search for guidance on patterns
```

### CLI Options

```
ra agents [OPTIONS]

OPTIONS:
    --stdout          Print to stdout instead of writing files
    --diff            Show unified diff of pending changes, don't write
    --quiet           Suppress diff output when writing
    --claude          Also generate CLAUDE.md
    --gemini          Also generate GEMINI.md
    --all             Generate all agent file variants
```

By default, `ra agents` writes only `AGENTS.md`. Use `--all` to generate all variants, or select specific ones with `--claude` and `--gemini`.

## Dependencies

| Crate | Purpose |
|-------|---------|
| `tantivy` | Full-text search engine |
| `rmcp` | MCP server implementation |
| `tiktoken-rs` | Token counting for context limits |
| `globset` | Glob pattern matching |
| `serde` | Serialization |
| `toml` | Configuration parsing |
| `pulldown-cmark` | Markdown parsing |
| `clap` | CLI argument parsing |
| `directories` | Platform-specific paths |

## Future Directions

These features are explicitly out of scope for v1 but may be considered later:

- **Semantic search**: Hybrid retrieval combining Tantivy keyword search with embedding-based similarity
- **Link-aware retrieval**: Follow wiki-links and relative paths to include related context
- **Watch mode**: File system watching for live index updates
- **Custom chunking**: User-defined chunking strategies (by h2, by paragraph, etc.)
- **Multi-language stemming**: Automatic language detection or per-document stemmer configuration
- **Faceted search**: Expose Tantivy's faceting for filtering by tag, tree, or custom metadata
- **Fast field sorting**: Use Tantivy's columnar storage for sorting results by date or other metadata
- **Query expansion**: Automatic synonym expansion or related term boosting
- **Image/binary support**: Index images with descriptions, PDFs with text extraction
