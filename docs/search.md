# Search

This document describes ra's search implementation: text analysis, index schema, query
processing, ranking, and result handling.


## Text Analysis Pipeline

Before indexing and searching, text passes through a four-stage analysis pipeline:

1. **Tokenization**: Split on whitespace and punctuation
2. **Lowercasing**: Normalize to lowercase
3. **Length filtering**: Drop tokens exceeding 40 characters
4. **Stemming**: Reduce words to stems (language-configurable)

The same pipeline processes both indexed content and queries, ensuring "handling" in a query
matches "handled" in a document.

**Example:**

```
"Error-Handling in Rust"
  → ["Error", "Handling", "in", "Rust"]     (tokenize)
  → ["error", "handling", "in", "rust"]     (lowercase)
  → ["error", "handl", "in", "rust"]        (stem)
```

### Supported Languages

ra supports 18 languages via Tantivy's stemmer: Arabic, Danish, Dutch, English (default),
Finnish, French, German, Greek, Hungarian, Italian, Norwegian, Portuguese, Romanian, Russian,
Spanish, Swedish, Tamil, Turkish.

Configure via `search.stemmer` in `.ra.toml`.


## Index Schema

Each chunk is indexed with these fields:

| Field | Purpose | Searchable | Stored |
|-------|---------|------------|--------|
| `id` | Chunk identifier | Exact match | Yes |
| `title` | Chunk/document title | Full-text | Yes |
| `tags` | Frontmatter tags | Full-text | Yes |
| `path` | Relative file path | Full-text | Yes |
| `path_components` | Path segments | Full-text | No |
| `tree` | Tree name | Exact match | Yes |
| `body` | Chunk content | Full-text | Yes |
| `breadcrumb` | Hierarchy path | No | Yes |
| `mtime` | Modification time | Filter/sort | No |

The `path_components` field splits paths for partial matching. For `docs/api/handlers.md`,
this indexes `["docs", "api", "handlers", "md"]`, allowing "api" to match files in the api
directory.


## Query Processing

ra supports a rich query syntax. See [query.md](query.md) for the complete reference.

### Query Structure

For each term, ra builds a multi-field query searching across title, tags, path,
path_components, and body simultaneously. Terms within a query are combined with AND.

**Example: `rust async`**

```
BooleanQuery(MUST):
├── MultiFieldQuery("rust")
│   ├── title:"rust" (boosted 3.0×)
│   ├── tags:"rust" (boosted 2.5×)
│   ├── path:"rust" (boosted 2.0×)
│   ├── path_components:"rust" (boosted 2.0×)
│   └── body:"rust" (boosted 1.0×)
└── MultiFieldQuery("async")
    ├── title:"async" (boosted 3.0×)
    ├── tags:"async" (boosted 2.5×)
    ├── path:"async" (boosted 2.0×)
    ├── path_components:"async" (boosted 2.0×)
    └── body:"async" (boosted 1.0×)
```

### Fuzzy Matching

By default, ra uses fuzzy matching with Levenshtein distance 1, tolerating single-character
edits:

- `"foz"` matches `"fox"` (substitution)
- `"hadle"` matches `"handle"` (missing letter)
- `"recieve"` matches `"receive"` (transposition)

Fuzzy matching applies to regular terms. Phrases require exact word matches (though each word
is still stemmed).

Configure via `search.fuzzy_distance` (0 disables fuzzy matching).


## Ranking

### BM25 Scoring

ra uses BM25 (Best Matching 25), the standard algorithm for Elasticsearch and Lucene. BM25
considers:

- **Term frequency**: How often terms appear in the chunk
- **Inverse document frequency**: Rarer terms score higher
- **Field length**: Shorter fields (like titles) receive higher scores

### Field Boosting

| Field | Boost |
|-------|-------|
| title | 3.0× |
| tags | 2.5× |
| path | 2.0× |
| path_components | 2.0× |
| body | 1.0× |

A match in the title contributes 3× as much to the score as the same match in the body.

### Tree Locality Boost

Local trees (defined in project `.ra.toml`) receive a boost over global trees (defined in
`~/.ra.toml`). Default: 1.5×.

This prioritizes project-specific documentation over general reference material.


## Multi-Topic Search

`ra search` joins multiple CLI arguments with OR, wrapping each in parentheses:

```bash
ra search "error handling" "exception handling"
# Equivalent to: ("error handling") OR ("exception handling")
```

The library also exposes `Searcher::search_multi` for programmatic multi-topic searches. It
runs each topic separately, deduplicates results, merges highlight ranges, and keeps the
highest score when a chunk matches multiple topics.


## Snippets and Highlighting

### Snippet Generation

For list-mode output, ra generates ~150-character snippets centered on matching terms:

```
...async/await <b>handling</b> patterns for <b>rust</b>...
```

### Match Ranges

Full results include byte ranges indicating where matches occur in the body:

- Ranges are offsets into the returned `body` text (UTF-8 byte indices)
- Sorted, non-overlapping, and merged when adjacent
- Each range corresponds to a token from the analyzer (lowercased, stemmed, possibly
  fuzzy-expanded)
- JSON output (`ra search --json`) exposes `body` and `match_ranges`
- Aggregated results omit `match_ranges`; highlights are per constituent


## Hierarchical Aggregation

ra implements a three-phase search algorithm that automatically aggregates sibling matches
into their parent sections when appropriate. See [chunking.md](chunking.md) for the complete
specification.

### Three Phases

1. **Query**: Retrieve candidates from the index up to `candidate_limit`
2. **Elbow cutoff**: Apply relevance cutoff using score ratio detection
3. **Aggregation**: Merge sibling matches into parent nodes when threshold is met

### CLI Parameters

| Flag | Default | Description |
|------|---------|-------------|
| `--candidate-limit` | 100 | Max candidates from Phase 1 |
| `--cutoff-ratio` | 0.5 | Score drop threshold for Phase 2 |
| `--aggregation-threshold` | 0.5 | Sibling ratio for Phase 3 |
| `--no-aggregation` | false | Disable hierarchical aggregation |

### Aggregated Results

Aggregated results show `[aggregated: N matches]` in the header and list constituent chunk
IDs. The parent's content is displayed with references to matching children.


## Incremental Indexing

### Manifest Tracking

ra maintains a manifest recording each file's path, tree, and modification time.

### Update Detection

On each operation, ra compares current files against the manifest:

- **Added**: Files in tree but not in manifest
- **Modified**: Files with changed modification time
- **Removed**: Files in manifest but no longer present

Only changed files are reprocessed.

### Configuration Changes

The index stores a hash of indexing-relevant configuration. When settings change (stemmer,
patterns), the index automatically rebuilds on next access.


## Performance

### Index Size

Tantivy creates a compact inverted index. Typical overhead is 30-50% of source document size.

### Query Latency

Single queries typically complete in <10ms for knowledge bases under 10,000 chunks. Multi-topic
queries scale linearly with topic count.

### Memory Usage

The index writer uses 50MB heap by default. Reading uses memory-mapped files, scaling with
concurrent readers rather than index size.
