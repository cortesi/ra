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

Agents provide simple search terms:

| Input | Interpretation |
|-------|----------------|
| `error handling` | Both terms must appear (AND) |
| `"error handling"` | Exact phrase |
| `"error handling" logging` | Phrase AND term |

Multiple quoted strings perform multi-topic search—each phrase is searched
separately, results are merged with deduplication.

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

# Next Steps

This section describes planned improvements to search and chunking.


## Extended Query Syntax

The current query syntax is minimal: bare terms are AND'd, quoted strings are
phrases. We need to expose more of Tantivy's query capabilities.

### Proposed Syntax

| Syntax | Meaning | Example |
|--------|---------|---------|
| `term` | Term must appear | `rust` |
| `"phrase"` | Exact phrase | `"error handling"` |
| `-term` | Term must NOT appear | `-deprecated` |
| `term1 OR term2` | Either term | `rust OR golang` |
| `(a b) OR (c d)` | Grouping | `(rust async) OR (go goroutine)` |
| `field:term` | Field-specific | `title:guide` |
| `tree:name` | Filter by tree | `tree:docs` |
| `path:prefix` | Filter by path | `path:api/` |

### Operator Precedence

From highest to lowest:
1. Quoted phrases: `"..."`
2. Field prefixes: `field:`
3. Negation: `-`
4. Grouping: `(...)`
5. OR (explicit)
6. AND (implicit, between adjacent terms)

**Examples:**

```
rust -deprecated                    # rust AND NOT deprecated
rust OR golang                      # rust OR golang
"error handling" -legacy            # phrase AND NOT legacy
title:guide rust                    # title contains "guide" AND body contains "rust"
tree:local authentication           # only search "local" tree
(rust async) OR (go goroutine)      # grouped alternatives
```

### Implementation

The query parser will produce an AST:

```rust
enum QueryExpr {
    Term(String),
    Phrase(Vec<String>),
    Not(Box<QueryExpr>),
    And(Vec<QueryExpr>),
    Or(Vec<QueryExpr>),
    Field { field: String, expr: Box<QueryExpr> },
}
```

This maps directly to Tantivy's query types:
- `Term` / `Phrase` → existing logic
- `Not` → `BooleanQuery` with `Occur::MustNot`
- `And` → `BooleanQuery` with `Occur::Must`
- `Or` → `BooleanQuery` with `Occur::Should` (with minimum_should_match=1)
- `Field` → query on specific field only (no multi-field expansion)

Special field handlers:
- `tree:` → filter using the STRING-indexed tree field
- `path:` → prefix query on path field


## Hierarchical Chunk Merging

Currently, chunks are flat—each is an independent search result. When multiple
chunks from the same document section match, we return them separately. This
fragments context that should be unified.

### The Problem

Consider a document structured as:

```markdown
# Guide
## Error Handling          <- h2 chunk
### Result Type            <- h3 (child of Error Handling)
### Option Type            <- h3 (child of Error Handling)
## Logging                 <- h2 chunk
```

If chunked at h2, searching for "error" returns "Error Handling" as one chunk.
But if chunked at h3, searching for "error" might match both "Result Type" and
"Option Type" separately, when what we really want is to return the entire
"Error Handling" section.

Similarly, if the filename is `error-handling.md` and we search for "error",
we may want to return the entire file rather than individual chunks.

### Design: Chunk Hierarchy

Model chunks as a tree rather than a flat list. Each chunk knows its parent,
and we can merge child chunks into their parent when appropriate.

#### New Index Fields

Add these fields to the schema:

| Field | Type | Purpose |
|-------|------|---------|
| `doc_id` | STRING | Document identifier (`tree:path`) |
| `parent_id` | STRING | Parent chunk ID (empty for top-level) |
| `depth` | u64 | Nesting depth (0 = document, 1 = h1, 2 = h2, ...) |
| `position` | u64 | Ordering within document (0, 1, 2, ...) |

The `doc_id` field enables efficient "all chunks from document X" queries.
The `parent_id` enables walking up the hierarchy.
The `depth` and `position` fields enable reconstruction of document structure.

#### Hierarchy Construction

During chunking, track the full hierarchy:

```rust
struct ChunkNode {
    chunk: Chunk,
    parent_id: Option<String>,
    depth: u32,
    position: u32,
    children: Vec<String>,  // child chunk IDs
}
```

For a document chunked at h3:
- Preamble: depth=0, parent=None
- h1 sections: depth=1, parent=preamble (or None)
- h2 sections: depth=2, parent=containing h1
- h3 chunks: depth=3, parent=containing h2

#### Merge Algorithm

After search returns raw chunk matches, apply hierarchical merging:

```
1. Group matches by doc_id
2. For each document with multiple matches:
   a. Build the chunk tree from parent_id relationships
   b. Walk up from each matching chunk
   c. If a parent chunk contains N matching children where N >= threshold:
      - Replace the N children with the parent
      - Score the parent as max(child scores) + bonus
   d. Recurse up the tree (merged parents may themselves merge)
3. Return merged results
```

**Merge threshold**: A parent absorbs children when:
- 2+ children match the same query, OR
- A child match + parent title/path match the query

**Score combination**: When merging, the parent's score is:
```
merged_score = max(child_scores) * (1 + 0.1 * num_children)
```

This rewards broader matches while preserving relative ranking.

#### Example

Query: `error`

Raw matches:
- `docs:guide.md#result-type` (score: 2.1)
- `docs:guide.md#option-type` (score: 1.8)
- `docs:api.md#error-codes` (score: 3.2)

Hierarchy for guide.md:
```
guide.md#preamble (depth=0)
└── guide.md#error-handling (depth=1, title matches "error")
    ├── guide.md#result-type (depth=2, matched)
    └── guide.md#option-type (depth=2, matched)
```

Merge decision:
- Two siblings match under `#error-handling`
- Parent title also matches query
- Merge: return `#error-handling` instead of both children

Final results:
- `docs:guide.md#error-handling` (score: 2.1 * 1.2 = 2.52)
- `docs:api.md#error-codes` (score: 3.2)

#### Document-Level Merging

The same logic applies at the document level. If:
- The filename matches the query (`error-handling.md` for query `error`), AND
- Multiple chunks from that file match

Then consider returning the entire document as a single result.

Implementation: add a synthetic "document chunk" at depth=0 that represents
the full file. Its body is the concatenation of all chunks (or the raw file
content). When path/filename match is strong and child matches are numerous,
merge to document level.

#### Configurable Behavior

Add settings to control merging:

```toml
[search]
merge_threshold = 2          # min children to trigger merge
merge_bonus = 0.1            # score bonus per merged child
max_merge_depth = 2          # don't merge above this depth (0 = allow doc-level)
```

#### Backward Compatibility

The new fields are additions—existing indexes can be rebuilt with `ra update`.
The merge algorithm runs post-search, so it's transparent to the underlying
Tantivy queries.

### Implementation Phases

**Phase 1: Index Schema Changes**
- Add `doc_id`, `parent_id`, `depth`, `position` fields
- Update chunker to emit hierarchy metadata
- Update indexer to store new fields
- Bump schema version (triggers rebuild)

**Phase 2: Query Syntax**
- Implement extended parser with NOT, OR, grouping
- Add field-specific query support
- Add `tree:` and `path:` filters
- Update CLI and MCP tool schemas

**Phase 3: Merge Algorithm**
- Implement post-search merge pass
- Add configuration options
- Handle edge cases (single-chunk docs, deep nesting)

**Phase 4: Refinement**
- Tune merge thresholds based on real usage
- Consider snippet generation for merged results
- Optimize performance for large result sets
