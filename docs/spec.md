# ra: Research Assistant

A knowledge management system for autonomous coding and writing agents.

## Overview

ra provides structured access to curated knowledge bases for AI agents. Users
maintain collections of markdown documents—project-specific and global—that
provide context for agent tasks. Because the full corpus may exceed practical
context limits, ra indexes these documents and exposes search tools that agents
use to retrieve relevant context on demand.

### Use Cases

- Project documentation and coding style guides for development agents
- Background research and world-building notes for writing agents
- Business context and historical data for report-generation agents

### Design Principles

- **Composable**: Hierarchical configuration overlays global and local knowledge
- **Lean**: Chunk-level retrieval keeps context focused
- **Simple**: Markdown in, markdown out—no proprietary formats
- **Fast**: Tantivy-powered full-text search with incremental indexing
- **Runtime over compile-time**: Agents search for what they need when they need
  it, rather than anticipating needs at configuration time
- **Minimal agent API**: Agents provide keywords; ra handles query construction,
  field boosting, tree selection, and ranking internally


## Configuration

Configuration uses TOML files named `.ra.toml`. ra resolves configuration by
walking up the directory tree from CWD, collecting and merging any `.ra.toml`
files found, with `~/.ra.toml` as the global config with lowest precedence.
Files closer to CWD take precedence.

### Key Concepts

**Trees** are named collections of documents. Each tree specifies a path and
optional include/exclude glob patterns. Trees defined in `~/.ra.toml` are
global; others are local. Local trees receive a relevance boost in search
results.

**Context patterns** map file globs to search terms, helping ra find relevant
documentation when analyzing source files (e.g., `*.rs` → `["rust"]`).

### Merge Semantics

- Child configurations override parent values for scalar settings
- Tree definitions are merged by name; child completely replaces parent if same
  name
- Context patterns are merged; child patterns take precedence for identical
  globs

See `ra config` for the effective merged configuration and `ra init --help` for
creating starter configurations.


## Document Format

ra indexes markdown (`.md`) and plain text (`.txt`) files.

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

If no frontmatter title exists, the first heading is used as the document
title.

### Chunking

ra uses adaptive chunking that automatically finds the right split level for
each document:

1. If document is smaller than `min_chunk_size`, keep it whole
2. Find the first heading level (h1, h2, h3, etc.) with 2+ headings
3. Chunk at that level
4. If no level qualifies, keep the document whole

Each chunk inherits the document's frontmatter and includes a breadcrumb line
showing its hierarchy path (e.g., `> Parent Heading › Child Heading`).

### Chunk Identity

Each chunk has a unique identifier: `{tree}:{relative_path}#{heading_slug}`

Slugs are GitHub-compatible (lowercase, punctuation removed, spaces become
hyphens). Content before the first heading uses `#preamble`.

Use `ra inspect doc <file>` to see exactly how ra parses and chunks a document.


## Indexing

The search index lives in `.ra/index/` as a sibling to the nearest `.ra.toml`.
ra uses Tantivy for full-text search with:

- Field boosting (title > tags > path > body)
- Stemming (configurable language)
- Incremental updates based on file modification times
- Automatic reindexing when configuration changes

Use `ra status` to check index state and `ra update` to force a rebuild.


## Search

Agents provide simple search terms. ra handles the complexity internally.

| Input | Interpretation |
|-------|----------------|
| `error handling` | Keywords, AND'd together |
| `"error handling"` | Exact phrase match |
| `"error handling" "logging"` | Multi-topic: both phrases searched |

Multi-topic research allows gathering context across several domains in a single
call, supporting the "research phase" pattern where an agent gathers broad
context before making decisions.

Results are ranked by BM25 with field boosting and tree locality adjustments.
Local trees receive a configurable boost over global trees.


## Context Analysis

The `ra context` command analyzes input files and returns relevant documentation.
It combines multiple signals:

1. **Path analysis**: Extract path components as search terms
2. **Pattern matching**: Match against configured `[context.patterns]` globs
3. **Content analysis**: Extract significant terms via TF-IDF (MoreLikeThis)

This supports the workflow where an agent runs `ra context` on files it's about
to modify, getting relevant project conventions and patterns before writing
code.


## Token Limiting

**Status: Not yet implemented**

The `--max-tokens` flag will limit results to fit within a token budget. ra will
use `tiktoken-rs` with the `cl100k_base` encoding for approximate token
counting.

Different models use different tokenizers, so counts are approximate.
cl100k_base (GPT-4's tokenizer) typically produces counts within 10-20% of other
modern tokenizers.


## MCP Server

**Status: Not yet implemented**

ra will expose an MCP server for agent integration using the `rmcp` crate.

### Transport

- Default: stdio transport (launched by agent runtime)
- Optional: SSE transport for persistent server mode

### Working Directory

The MCP server operates relative to its current working directory when launched.
This determines which `.ra.toml` files are discovered. ra is designed for
per-project use—there is no global daemon.

### Tools

#### `search`

Search the knowledge base and return matching chunks.

```json
{
  "name": "search",
  "description": "Search the knowledge base. Use array of queries for multi-topic research.",
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
        "description": "Results per query (default from config)"
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

#### `context`

Analyze files and return relevant knowledge base context.

```json
{
  "name": "context",
  "description": "Get relevant context for files being worked on.",
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
        "description": "Maximum chunks to return (default from config)"
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

#### `get`

Retrieve a specific chunk or document by identifier.

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

The MCP server checks index freshness on each call and performs incremental
updates as needed, ensuring agents always search current content.


## Agent File Generation

**Status: Not yet implemented**

ra will generate agent instruction files (AGENTS.md, CLAUDE.md, GEMINI.md) that
teach agents to use ra as their primary knowledge source.

### Philosophy

Traditional approaches try to anticipate what context an agent needs at
generation time. ra inverts this: generate minimal static instructions that
teach the agent to search at runtime. The agent pays one round-trip for
research, but gains flexibility—any documentation can live in the searchable
knowledge base without bloating the agent file.

### Template System

Templates are markdown files that ra concatenates:

1. **Project template**: `.agents.md` in the project root (optional)
2. **Global template**: `~/.agents.md` (optional)

The project template appears first, followed by the global template.

### Dynamic Injection

After concatenating templates, ra appends generated instructions containing:

- Clear guidance to use ra before making decisions
- Usage examples for search and context commands
- Specific triggers that should prompt a search

Example generated section:

```markdown
## ra Knowledge Base

This project uses ra for knowledge management. **Search ra before making
significant decisions.**

### Why This Matters

This project's conventions WILL differ from your training data. Proceeding
without consulting ra means you will miss project-specific requirements.

### How to Use

**Get context for files you're working on:**
- `ra context src/api/handlers.rs`
- `ra context src/*.rs`

**Search for specific topics:**
- `ra search "error handling"`
- `ra search "error handling" "logging"`

### When to Use

- **Starting work on a file**: Run `ra context` on the files you'll modify
- **Before writing new code**: Search for relevant patterns
- **Encountering unfamiliar terms**: Search for project-specific concepts
- **Choosing between approaches**: Search for guidance
```

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

These features are explicitly out of scope for v1 but may be considered later:

- **Semantic search**: Hybrid retrieval combining keyword and embedding-based
  similarity
- **Link-aware retrieval**: Follow wiki-links to include related context
- **Watch mode**: File system watching for live index updates
- **Custom chunking**: User-defined chunking strategies
- **Multi-language stemming**: Automatic language detection per document
- **Faceted search**: Filtering by tag, tree, or custom metadata
- **Query expansion**: Automatic synonym expansion
- **Image/binary support**: Index images with descriptions, PDFs with text
  extraction
