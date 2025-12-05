# ra Specification

This document is the authoritative reference for ra's features. Sections marked **[Planned]**
describe features not yet implemented.


## Configuration

ra uses TOML files named `.ra.toml`. Configuration is resolved by walking up the directory
tree from the current working directory, collecting and merging any `.ra.toml` files found.
`~/.ra.toml` is loaded last with lowest precedence. Files closer to the working directory
take precedence.

### Trees

Trees are named collections of documents. Each tree specifies a root path and optional
include/exclude patterns:

```toml
[tree.docs]
path = "./docs"
include = ["**/*.md", "**/*.txt"]  # default if omitted
exclude = ["**/drafts/**"]
```

Trees defined in `~/.ra.toml` are global. Trees defined elsewhere are local. Local trees
receive a relevance boost in search results (configurable via `settings.local_boost`).

### Context Rules

Context rules customize search behavior based on file patterns. When `ra context` analyzes
a file, matching rules can inject terms, limit trees, and auto-include files:

```toml
[[context.rules]]
match = "*.rs"
trees = ["docs"]
terms = ["rust"]

[[context.rules]]
match = "src/api/**"
terms = ["http", "routing"]
include = ["docs:api/overview.md"]
```

When multiple rules match, terms and includes are concatenated (deduplicated) and tree
restrictions are intersected. See [context.md](context.md) for the full specification.

### Merge Semantics

- Scalar settings: closer files override more distant files.
- Trees: merged by name; a tree definition completely replaces any same-named tree from a
  more distant config.
- Context rules: merged per file; terms and includes concatenate, trees intersect.

Use `ra config` to see the effective merged configuration. Use `ra init` to generate a
starter configuration file.


## Document Format

ra indexes markdown (`.md`) and plain text (`.txt`) files.

### Frontmatter

YAML frontmatter in markdown files is parsed when present:

```markdown
---
title: Rust Error Handling
tags: [rust, errors, patterns]
---
```

- `title`: Indexed with elevated weight; used as the document title in results.
- `tags`: Indexed with elevated weight; supports Obsidian-style tags.

If no frontmatter title exists, the first h1 heading is used. If there's no h1, the filename
(without extension) becomes the title.

### Chunking

ra builds a hierarchical chunk tree from each document:

- The document itself is the root node at depth 0.
- Each markdown heading (h1–h6) creates a child node at its corresponding depth.
- Heading spans run from the byte after the heading line to the byte before the next heading
  of equal or lower depth (or end of file).
- Headings with empty spans (no content before the next heading) are discarded.
- All surviving nodes are indexed, including those with empty bodies, so titles remain
  searchable.

Plain text files produce a single document chunk with the entire file as its body.

ra does not split large sections. Document structure should provide sufficient granularity.
See [chunking.md](chunking.md) for the complete specification.

### Chunk Identifiers

Each chunk has a unique identifier:

- Document chunks: `{tree}:{relative_path}`
- Heading chunks: `{tree}:{relative_path}#{slug}`

Slugs are generated from heading text using a GitHub-compatible algorithm. See
[slugs.md](slugs.md) for details.


## Indexing

The search index is stored in `.ra/index/` as a sibling to the nearest `.ra.toml`. If only
the global config exists, the index is stored in `~/.ra/index/`.

ra uses Tantivy for full-text search with:

- Field boosting (title > tags > path > body)
- Configurable stemming (18 languages supported)
- Fuzzy matching with configurable Levenshtein distance
- Incremental updates based on file modification times
- Automatic full rebuild when indexing-relevant configuration changes

### Index Schema

| Field | Searchable | Stored | Boost |
|-------|------------|--------|-------|
| id | Exact match | Yes | — |
| title | Full-text | Yes | 3.0× |
| tags | Full-text | Yes | 2.5× |
| path | Full-text | Yes | 2.0× |
| path_components | Full-text | No | 2.0× |
| tree | Exact match | Yes | — |
| body | Full-text | Yes | 1.0× |
| breadcrumb | No | Yes | — |
| mtime | Filter/sort | No | — |


## Search

Search operates in three phases:

1. **Candidate retrieval**: Query the Tantivy index for up to `candidate_limit` results
   ranked by BM25.

2. **Elbow cutoff**: Detect the natural boundary where relevance drops. Cut results when
   the score ratio between adjacent results falls below `cutoff_ratio`.

3. **Hierarchical aggregation**: When multiple sibling chunks match and their count meets
   `aggregation_threshold`, merge them into their parent chunk. This cascades up the
   hierarchy.

After aggregation, any result whose ancestor also appears in results is filtered out.

### Query Syntax

ra supports a rich query language:

| Syntax | Meaning |
|--------|---------|
| `term` | Must contain term |
| `term1 term2` | Must contain both (AND) |
| `"phrase"` | Exact phrase |
| `-term` | Must NOT contain |
| `a OR b` | Either term |
| `(...)` | Grouping |
| `field:term` | Search specific field |
| `term^N` | Boost importance |

See [query.md](query.md) for the complete query language reference.

### Multi-Topic Search

`ra search` joins multiple CLI arguments with OR, wrapping each in parentheses. This makes
it easy to search for multiple topics simultaneously.

The library also exposes `Searcher::search_multi` for programmatic multi-topic searches with
merged highlights and deduplication.


## Context Analysis

The `ra context` command analyzes source files and generates search queries automatically.

### Signal Sources

1. **Path analysis**: Directory names and filename components are extracted as weighted
   terms.

2. **Content analysis**: For markdown files, terms from headings receive higher weight than
   body text. All terms are scored using TF-IDF with IDF values from the index.

3. **Tree-aware IDF**: When `--tree` is specified, IDF computation considers only the
   selected trees.

Terms that don't appear in the index are filtered out. The top N terms by score are combined
into a boosted OR query.

Context rules from configuration are used to inject additional terms, limit search to
specific trees, and auto-include files in results.


## CLI Commands

### `ra search [QUERIES]`

Search the knowledge base. Multiple arguments are joined with OR.

Options:
- `-n, --limit N`: Maximum results
- `--list`: Show titles and snippets only
- `--matches`: Show matching lines only
- `--json`: JSON output
- `--explain`: Show parsed query AST
- `--candidate-limit N`: Phase 1 limit (default: 100)
- `--cutoff-ratio N`: Phase 2 threshold (default: 0.5)
- `--aggregation-threshold N`: Phase 3 threshold (default: 0.5)
- `--no-aggregation`: Disable hierarchical aggregation
- `-v, --verbose`: Increase output verbosity

### `ra context [FILES]`

Find relevant documentation for source files.

Options:
- `-n, --limit N`: Maximum results
- `--terms N`: Maximum terms in generated query (default: 15)
- `-t, --tree NAME`: Limit to specific tree(s)
- `--list`: Show titles only
- `--json`: JSON output
- `--explain`: Show extracted terms and generated query
- `-v, --verbose`: Increase output verbosity

### `ra get [ID]`

Retrieve a specific chunk or document by identifier.

Options:
- `--full-document`: Return the entire document even if ID specifies a chunk
- `--json`: JSON output

### `ra inspect doc [FILE]`

Show how ra parses and chunks a document.

### `ra inspect ctx [FILE]`

Show context signals extracted from a file, including configured patterns.

### `ra init`

Create a starter `.ra.toml` configuration file.

Options:
- `--global`: Create in `~/.ra.toml`
- `--force`: Overwrite existing file

### `ra update`

Force a full rebuild of the search index.

### `ra status`

Show configuration files, configured trees, index status, and validation warnings.

### `ra config`

Display the effective merged configuration.

### `ra ls [WHAT]`

List indexed content.

- `ra ls trees`: List configured trees
- `ra ls docs`: List indexed documents
- `ra ls chunks`: List all indexed chunks


## [Planned] Token Limiting

The `--max-tokens` flag will limit results to fit within a token budget, using `tiktoken-rs`
with the `cl100k_base` encoding for approximate counting.

Different models use different tokenizers. cl100k_base (GPT-4's tokenizer) typically produces
counts within 10-20% of other modern tokenizers.


## [Planned] MCP Server

ra will expose an MCP server for direct agent integration using the `rmcp` crate.

### Transport

- Default: stdio transport (launched by agent runtime)
- Optional: SSE transport for persistent server mode

### Working Directory

The MCP server operates relative to its working directory when launched, which determines
configuration discovery. ra is designed for per-project use—there is no global daemon.

### Tools

#### `search`

Search the knowledge base.

```json
{
  "name": "search",
  "inputSchema": {
    "type": "object",
    "properties": {
      "queries": {
        "oneOf": [
          { "type": "string" },
          { "type": "array", "items": { "type": "string" } }
        ]
      },
      "limit": { "type": "integer" },
      "max_tokens": { "type": "integer" },
      "list": { "type": "boolean" }
    },
    "required": ["queries"]
  }
}
```

#### `context`

Get relevant context for files being worked on.

```json
{
  "name": "context",
  "inputSchema": {
    "type": "object",
    "properties": {
      "files": { "type": "array", "items": { "type": "string" } },
      "limit": { "type": "integer" },
      "max_tokens": { "type": "integer" },
      "list": { "type": "boolean" }
    },
    "required": ["files"]
  }
}
```

#### `get`

Retrieve a specific document or chunk by ID.

```json
{
  "name": "get",
  "inputSchema": {
    "type": "object",
    "properties": {
      "id": { "type": "string" },
      "full_document": { "type": "boolean" }
    },
    "required": ["id"]
  }
}
```

#### `list_sources`

List available knowledge trees and their statistics.

```json
{
  "name": "list_sources",
  "inputSchema": {
    "type": "object",
    "properties": {}
  }
}
```

### Index Freshness

The MCP server checks index freshness on each call and performs incremental updates as
needed.


## [Planned] Agent File Generation

ra will generate agent instruction files (AGENTS.md, CLAUDE.md, GEMINI.md) that teach agents
to use ra as their primary knowledge source.

### Philosophy

Traditional approaches try to anticipate what context an agent needs at generation time.
ra inverts this: generate minimal static instructions that teach the agent to search at
runtime. The agent pays one round-trip for research but gains flexibility—any documentation
can live in the searchable knowledge base without bloating the agent file.

### Template System

Templates are markdown files concatenated in order:

1. `.agents.md` in the project root (optional)
2. `~/.agents.md` (optional)

### Dynamic Injection

After concatenating templates, ra appends generated instructions including:

- Guidance to use ra before making decisions
- Usage examples for search and context commands
- Specific triggers that should prompt a search

### CLI

```
ra agents [OPTIONS]

OPTIONS:
    --stdout    Print to stdout instead of writing files
    --diff      Show unified diff of pending changes
    --quiet     Suppress diff output when writing
    --claude    Also generate CLAUDE.md
    --gemini    Also generate GEMINI.md
    --all       Generate all agent file variants
```


## Future Directions

These features are out of scope for the initial release but may be considered later:

- **Semantic search**: Hybrid retrieval combining keyword and embedding-based similarity
- **Link-aware retrieval**: Follow wiki-links to include related context
- **Watch mode**: File system watching for live index updates
- **Custom chunking**: User-defined chunking strategies
- **Multi-language stemming**: Automatic language detection per document
- **Faceted search**: Filtering by tag, tree, or custom metadata
- **Query expansion**: Automatic synonym expansion
- **Image/binary support**: Index images with descriptions, PDFs with text extraction
