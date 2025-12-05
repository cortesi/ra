# ra: Research Assistant

ra is a knowledge management system designed for AI agents. It indexes collections of
markdown documents and exposes fast, structured search that agents use to retrieve relevant
context on demand.


## The Problem

AI agents working on complex tasks need context—project conventions, background research,
design decisions, historical data. But feeding everything into the context window up front
doesn't scale: context limits exist, and most information isn't relevant to any given task.

The alternative—having humans curate context per-task—defeats the purpose of autonomous
agents.


## The Solution

ra inverts the problem. Instead of anticipating what context an agent needs, you maintain
a searchable knowledge base. Agents query it at runtime, retrieving exactly what's relevant
to their current task.

This means:

- **Context stays fresh**: Update documentation once, all agents see it immediately.
- **Context stays focused**: Agents retrieve only what they need, preserving context budget.
- **Context stays organized**: Hierarchical document structure is preserved in search results.


## Core Concepts

**Trees** are named collections of documents. Each tree points to a directory of markdown
files. Trees can be local (defined in a project's `.ra.toml`) or global (defined in
`~/.ra.toml`). Local trees receive a relevance boost in search results.

**Chunks** are the unit of retrieval. Every markdown document is split into a hierarchy of
chunks based on headings: the document itself is one chunk, and each heading creates a
nested chunk. This structure lets ra return precisely the right level of detail—a single
subsection or an entire document—depending on what matches.

**Search** uses BM25 ranking with field boosting, fuzzy matching, and a rich query syntax.
Results are post-processed through elbow detection (to cut off irrelevant matches) and
hierarchical aggregation (to merge related sibling matches into their parent section).

**Context analysis** extracts salient terms from source files and automatically constructs
search queries. When an agent is working on `auth_handler.rs`, running `ra context
auth_handler.rs` surfaces relevant authentication documentation without manual query
construction.


## Typical Usage

### For Users

1. Create a `.ra.toml` file defining your document trees:

```toml
[tree.docs]
path = "./docs"
```

2. Run `ra update` to build the index.

3. Search your knowledge base:

```bash
ra search "error handling"
ra search title:guide rust
```

4. Get context for files you're working on:

```bash
ra context src/auth/handler.rs
```

### For Agents

Agents use the same CLI commands but typically receive them through tool definitions. The
key principle: **search before acting**. When an agent encounters a task involving
unfamiliar concepts or project-specific patterns, it should query ra rather than relying on
training data.


## Design Principles

- **Composable**: Hierarchical configuration lets global defaults coexist with project
  overrides.
- **Lean**: Chunk-level retrieval keeps context focused; no need to ingest entire documents.
- **Simple**: Markdown in, markdown out. No proprietary formats or complex pipelines.
- **Fast**: Tantivy-powered full-text search with incremental indexing.
- **Runtime over compile-time**: Agents search for what they need when they need it, rather
  than requiring all context to be anticipated in advance.


## Documentation

- [Configuration](config.md) — Setting up `.ra.toml`
- [Search](search.md) — How search works, including ranking and aggregation
- [Query Syntax](query.md) — Full query language reference
- [Context Search](context.md) — Automatic query generation from source files
- [Chunking](chunking.md) — How documents are split into searchable units
- [Slugs](slugs.md) — Chunk identifier generation
- [Specification](spec.md) — Complete feature reference including planned features
