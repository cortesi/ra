# Search and Chunking

This document describes ra's current chunking and search implementation in
detail.


## Document Chunking

ra now chunks every heading level and records the full hierarchy. Fine-grained
leaf chunks give maximum recall; hierarchical merging reassembles larger
sections when a query merits more context.

### Chunk Tree Construction

1. **Parse headings**: Identify Markdown headings h1–h6 in order. Everything
   before the first heading is the preamble candidate.
2. **Create nodes for all headings**: Each heading becomes a node with
   `depth = heading_level` (preamble uses depth 0; document-level synthetic
   node also lives at depth 0).
3. **Span assignment**: A node’s span runs from its heading line to the line
   before the next heading of equal or lower depth. Empty spans are discarded.
4. **Leaf formation**:
   - Nodes with no children are leaves. Only leaves carry `body` text.
   - If a leaf span exceeds `max_leaf_chars` (default: 1,000), split on
     paragraph boundaries; if still too long, split on sentence boundaries.
   - If a span is shorter than `min_leaf_chars` (default: 250) and has a
     parent, absorb it into the parent instead of emitting it.
5. **Small document fast path**: If total document characters are below
   `min_document_chars` (default: 1,200), emit a single leaf covering the whole
   file plus the synthetic document node.

### Stored Metadata

Every node (including non-leaf parents) is emitted with structural metadata:

| Field | Purpose |
|-------|---------|
| `id` | Unique chunk ID (`{tree}:{path}#{slug}`) |
| `doc_id` | Document identifier (`{tree}:{path}`) |
| `parent_id` | Parent chunk ID (`None` for document/preamble) |
| `depth` | 0 = document/preamble, 1 = h1, ... |
| `position` | Document order starting at 0 |
| `title` | Heading text or document title |
| `slug` | GitHub-compatible heading slug |
| `byte_start` / `byte_end` | Offsets into the source file |

Only leaves store a `body` field. Parent nodes remain indexable via title/path
but avoid duplicating content. During merging we reconstruct a parent’s text by
concatenating matched leaves or slicing the source file using byte ranges.

### Preamble and Document Nodes

- **Preamble**: If the pre-heading content contains substantive text, emit a
  preamble node (`slug = #preamble`, depth 0) with body text. Otherwise, drop
  the preamble.
- **Document synthetic node**: Always emit a node representing the whole file
  (depth 0, no body). It enables document-level merging when many children
  match or the path/title matches the query strongly.

### Breadcrumbs

Each chunk includes a breadcrumb derived from the hierarchy and prepended to
the body when stored:

```
> Document Title › Parent Section › Chunk Title
```

Breadcrumbs make parent context searchable without inflating parent bodies.

### Chunk Identifiers and Slugs

Chunk IDs stay stable: `{tree}:{path}#{slug}`. Slug generation is unchanged:
lowercase, keep alphanumerics/hyphens/underscores, convert spaces to hyphens,
collapse repeats, and deduplicate sequential duplicates with numeric suffixes.
Special cases:
- Preamble: `#preamble`
- Plain text files (no headings): no fragment (`tree:path`)


## Text Analysis

Before indexing and searching, text passes through a four-stage analysis
pipeline:

1. **SimpleTokenizer**: Split on whitespace and punctuation
2. **LowerCaser**: Normalize to lowercase
3. **RemoveLongFilter**: Drop tokens exceeding 40 characters
4. **Stemmer**: Reduce words to stems (language-configurable)

**Example:**
```
"Error-Handling in Rust"
  → ["Error", "Handling", "in", "Rust"]     (tokenize)
  → ["error", "handling", "in", "rust"]     (lowercase)
  → ["error", "handl", "in", "rust"]        (stem)
```

The same pipeline processes both indexed content and search queries, ensuring
"handling" in a query matches "handled" in a document.

### Stemming Languages

ra supports 18 languages via Tantivy's stemmer: Arabic, Danish, Dutch, English
(default), Finnish, French, German, Greek, Hungarian, Italian, Norwegian,
Portuguese, Romanian, Russian, Spanish, Swedish, Tamil, Turkish.

Configure via `search.stemmer` in `.ra.toml`.


## Index Schema

Each chunk is indexed with these fields:

| Field | Purpose | Searchable | Stored |
|-------|---------|------------|--------|
| id | Chunk identifier | Exact match | Yes |
| title | Chunk/document title | Full-text | Yes |
| tags | Frontmatter tags | Full-text | Yes |
| path | Relative file path | Full-text | Yes |
| path_components | Path segments | Full-text | No |
| tree | Tree name | Exact match | Yes |
| body | Chunk content | Full-text | Yes |
| breadcrumb | Hierarchy path | No | Yes |
| mtime | Modification time | Filter/sort | No |

**path_components** splits the path into segments for partial matching. For
`docs/api/handlers.md`, this indexes `["docs", "api", "handlers", "md"]`,
allowing searches for "api" to match files in the api directory.


## Query Processing

### Query Syntax

ra supports a rich query syntax with boolean operators, grouping, negation, and
field-specific searches.

#### Basic Operators

| Syntax | Meaning | Example |
|--------|---------|---------|
| `term` | Term must appear | `rust` |
| `term1 term2` | Both terms must appear (implicit AND) | `rust async` |
| `"phrase"` | Exact phrase match | `"error handling"` |
| `-term` | Term must NOT appear | `-deprecated` |
| `term1 OR term2` | Either term (case-insensitive) | `rust OR golang` |
| `(expr)` | Grouping | `(rust OR golang) async` |

#### Field-Specific Queries

| Syntax | Meaning | Example |
|--------|---------|---------|
| `title:term` | Search only in titles | `title:guide` |
| `body:term` | Search only in body text | `body:configuration` |
| `tags:term` | Search only in tags | `tags:tutorial` |
| `path:term` | Search only in file paths | `path:api` |
| `tree:name` | Filter to specific tree | `tree:docs` |

Field queries support all operators:
- `title:"getting started"` — phrase in title
- `title:(rust OR golang)` — either term in title
- `-title:deprecated` — title must NOT contain term

#### Operator Precedence

From highest to lowest:
1. Quoted phrases: `"..."`
2. Field prefixes: `field:`
3. Negation: `-`
4. Grouping: `(...)`
5. OR (explicit keyword)
6. AND (implicit, between adjacent terms)

#### Examples

```
rust async                      # rust AND async
"error handling"                # exact phrase
rust -deprecated                # rust but NOT deprecated
rust OR golang                  # either language
(rust async) OR (go goroutine)  # grouped alternatives
title:guide rust                # "guide" in title AND "rust" anywhere
tree:docs authentication        # search only in "docs" tree
title:(api OR sdk) -internal    # api or sdk in title, excluding internal
```

#### Shell Escaping

When using ra from the command line, be aware of shell interpretation:

```bash
# Quotes need escaping or outer quotes
ra search '"error handling"'           # single quotes protect double quotes
ra search "\"error handling\""         # escaped double quotes

# Parentheses need quoting
ra search '(rust OR golang) async'     # single quotes protect parens
ra search "(rust OR golang) async"     # double quotes also work

# OR is safe (no shell meaning)
ra search rust OR golang               # works without quotes

# Negation is safe (not at line start)
ra search rust -deprecated             # works without quotes
```

#### Debugging Queries

Use `--explain` to see how ra parses your query:

```bash
$ ra search --explain 'title:guide (rust OR golang)'
Query AST: And([Field { name: "title", expr: Term("guide") }, Or([Term("rust"), Term("golang")])])
```

#### Error Messages

ra provides helpful error messages for invalid queries:

```
$ ra search 'title:'
Error: expected term, phrase, or group after 'title:'

$ ra search '(rust async'
Error: expected closing parenthesis

$ ra search '"unclosed phrase'
Error: unclosed quote starting at position 0

$ ra search 'foo:bar'
Error: unknown field 'foo'. Valid fields: title, body, tags, path, tree
```

### Query Construction

For each term, ra builds a multi-field query searching across title, tags,
path, path_components, and body simultaneously. Terms are combined with AND
logic—all must match.

**Structure for `rust async`:**
```
BooleanQuery(MUST):
├── MultiFieldQuery("rust")
│   ├── title:"rust" (boosted 3.0x)
│   ├── tags:"rust" (boosted 2.5x)
│   ├── path:"rust" (boosted 2.0x)
│   ├── path_components:"rust" (boosted 2.0x)
│   └── body:"rust" (boosted 1.0x)
└── MultiFieldQuery("async")
    ├── title:"async" (boosted 3.0x)
    ├── tags:"async" (boosted 2.5x)
    ├── path:"async" (boosted 2.0x)
    ├── path_components:"async" (boosted 2.0x)
    └── body:"async" (boosted 1.0x)
```

### Fuzzy Matching

By default, ra uses fuzzy matching with Levenshtein distance 1. This tolerates
single-character edits (insertions, deletions, substitutions, transpositions).

**Examples with `fuzzy_distance=1`:**
- "foz" matches "fox" (substitution)
- "hadle" matches "handle" (missing letter)
- "recieve" matches "receive" (transposition)

Fuzzy matching applies to regular terms. Phrases require exact word matches
(though each word in the phrase is still stemmed).

Configure via `search.fuzzy_distance` (0 disables fuzzy matching).


## Ranking

### BM25 Scoring

ra uses BM25 (Best Matching 25), the same algorithm used by Elasticsearch and
Lucene. BM25 considers:

- **Term frequency**: How often terms appear in the chunk
- **Inverse document frequency**: Rarer terms score higher
- **Field length**: Shorter fields (titles) get boosted

### Field Boosting

Different fields have different relevance weights:

| Field | Boost |
|-------|-------|
| title | 3.0x |
| tags | 2.5x |
| path | 2.0x |
| path_components | 2.0x |
| body | 1.0x |

A match in the title contributes 3x as much to the score as the same match in
the body.

### Tree Locality Boost

Local trees (defined in project `.ra.toml`) receive a boost over global trees
(defined in `~/.ra.toml`). Default: 1.5x.

This prioritizes project-specific documentation over general reference material
while maintaining BM25 relevance within each category.


## Multi-Topic Search

When multiple queries are provided, ra searches each separately and merges
results:

```
ra search "error handling" "logging patterns"
```

**Merge behavior:**
- Each chunk appears once in final results
- If both queries match a chunk, the higher score is kept
- Match ranges are merged for highlighting
- Snippets are concatenated with " … "

This supports the "research phase" pattern where agents gather context across
several topics before acting.


## Snippets and Highlighting

### Snippet Generation

For list-mode output, ra generates ~150-character snippets centered on matching
terms. Snippets include HTML `<b>` tags around matches:

```
...async/await <b>handling</b> patterns for <b>rust</b>...
```

### Match Ranges

Full search results include byte ranges indicating where matches occur in the
body. This enables precise highlighting in output formatting.

- Ranges are offsets into the returned `body` text (UTF-8 byte indices).
- They are sorted, non-overlapping, and merged when adjacent.
- Each range corresponds to a token emitted by the index analyzer (lowercased,
  stemmed, and possibly fuzzy-expanded), so highlighting the substring at
  `offset..offset+length` marks the exact word that satisfied the query.
- Multi-topic searches merge ranges from all topics using the same invariants.
- JSON output (`ra search --json`) exposes `body` and `match_ranges` for every
  result; use these fields together to render highlights accurately. Aggregated
  results omit `match_ranges` because highlights are per constituent.

### Developer Notes

- Highlight extraction is analyzer-driven: token offsets from the configured
  analyzer (lowercase + stemmer) are the single source of truth. Keep analyzer
  changes in sync with tests that assert range alignment.
- Do not derive highlights from Tantivy snippets; they may diverge from fuzzy
  matches. Use the stored `match_ranges` instead.


## Incremental Indexing

### Manifest Tracking

ra maintains a manifest recording each file's:
- Path and tree
- Modification time
- Content hash

### Update Detection

On each operation, ra compares current files against the manifest:
- **Added**: Files in tree but not in manifest
- **Modified**: Files with changed mtime or hash
- **Removed**: Files in manifest but no longer present

Only changed files are reprocessed, making updates fast for large knowledge
bases.

### Configuration Changes

The index stores a hash of indexing-relevant configuration. If settings change
(stemmer, chunk sizes), the index automatically rebuilds on next access.


## Performance Characteristics

### Index Size

Tantivy creates a compact inverted index. Typical overhead is 30-50% of source
document size, depending on content characteristics.

### Query Latency

Single queries typically complete in <10ms for knowledge bases under 10,000
chunks. Multi-topic queries scale linearly with topic count.

### Memory Usage

The index writer uses 50MB heap by default. Reading uses memory-mapped files,
so memory pressure scales with concurrent readers rather than index size.


---

## Hierarchical Aggregation

ra implements a three-phase search algorithm that automatically aggregates
sibling matches into their parent sections when appropriate. This provides
unified context instead of fragmenting related content.

For the authoritative specification of the chunking and aggregation algorithms,
see [docs/chunking.md](chunking.md).

### Three-Phase Search Algorithm

1. **Phase 1 (Query)**: Retrieve candidates from the index up to `candidate_limit`
2. **Phase 2 (Elbow)**: Apply relevance cutoff using score ratio detection
3. **Phase 3 (Aggregate)**: Merge sibling matches into parent nodes

### Aggregation Behavior

When multiple sibling chunks match a query and their count exceeds the
aggregation threshold (default: 50% of siblings), they are merged into their
parent node. This cascades up the hierarchy.

**Example**: If a document has:
```
# Error Handling
## Result Type      <- matches "error"
## Option Type      <- matches "error"
```

Both h2 sections match, meeting the threshold (2/2 = 100%), so they aggregate
into the "Error Handling" parent section.

### CLI Parameters

Control the search algorithm via CLI flags:

| Flag | Default | Description |
|------|---------|-------------|
| `--candidate-limit` | 100 | Max candidates from Phase 1 |
| `--cutoff-ratio` | 0.5 | Score drop threshold for Phase 2 |
| `--aggregation-threshold` | 0.5 | Sibling ratio for Phase 3 |
| `--no-aggregation` | false | Disable hierarchical aggregation |

### Aggregated Results Display

Aggregated results show `[aggregated: N matches]` in the header and list
constituent chunk IDs. The parent's body content is displayed with references
to which child chunks matched.
