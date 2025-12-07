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
| `hierarchy` | Hierarchy path (multi-value) | Full-text | Yes |
| `tags` | Frontmatter tags | Full-text | Yes |
| `path` | Relative file path | Full-text | Yes |
| `tree` | Tree name | Exact match | Yes |
| `body` | Chunk content | Full-text | Yes |
| `mtime` | Modification time | Filter/sort | No |

The `hierarchy` field is a multi-value text field where each element represents a level in the
document hierarchy. For a section "Installation" under "Getting Started", this indexes
`["Getting Started", "Installation"]` as separate values. This allows searches to match both the
section title and its ancestors. BM25 naturally ranks shallower matches higher (documents with
fewer hierarchy elements score higher for the same term match).

The `path` field is tokenized on path separators and dots. For `docs/api/handlers.md`, this
indexes `["docs", "api", "handlers", "md"]`, allowing "api" to match files in the api directory.


## Query Processing

ra supports a rich query syntax. See [query.md](query.md) for the complete reference.

### Query Structure

For each term, ra builds a multi-field query searching across hierarchy, tags, path, and body
simultaneously. Terms within a query are combined with AND.

**Example: `rust async`**

```
BooleanQuery(MUST):
├── MultiFieldQuery("rust")
│   ├── hierarchy:"rust" (boosted 10.0×)
│   ├── tags:"rust" (boosted 5.0×)
│   ├── path:"rust" (boosted 8.0×)
│   └── body:"rust" (boosted 1.0×)
└── MultiFieldQuery("async")
    ├── hierarchy:"async" (boosted 10.0×)
    ├── tags:"async" (boosted 5.0×)
    ├── path:"async" (boosted 8.0×)
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
| hierarchy | 10.0× |
| path | 8.0× |
| tags | 5.0× |
| body | 1.0× |

A match in the hierarchy contributes 10× as much to the score as the same match in the body.

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


## Search Parameters

ra's search algorithm has configurable parameters that control result quality and quantity.
These can be set in `.ra.toml` under `[search]` or `[context]`, or overridden via CLI flags.

The search pipeline has four phases, each with its own parameters:

### Phase 1: Candidate Retrieval

**Derived from**: `limit × 5` (by default)

The maximum number of raw matches to retrieve from the index. This is derived automatically
from `limit` to ensure enough candidates flow through the pipeline for effective aggregation.

For example, with `limit = 10`, the pipeline fetches 50 candidates from the index.


### Phase 2: Relevance Cutoff (Elbow Detection)

**Config**: `cutoff_ratio` | **Default**: 0.3
**Config**: `max_candidates` | **Default**: 50

Controls how aggressively ra filters **raw candidates** based on score drops. When the
ratio between consecutive candidate scores falls below `cutoff_ratio`, ra truncates there.
The `max_candidates` parameter sets a hard cap on candidates entering aggregation.

This phase determines the "relevant" set of candidates that will be aggregated.


### Phase 3: Adaptive Aggregation

**Config**: `aggregation_threshold` | **Default**: 0.5

Controls hierarchical aggregation of sibling matches into parent sections. All candidates
that pass elbow cutoff are processed in score order:

- When siblings meet the threshold, they're merged into their parent
- When an ancestor arrives after its descendants, it subsumes them
- Aggregation can cascade upward through multiple levels
- All relevant candidates are processed (no early termination)

See [chunking.md](chunking.md) for the complete aggregation specification.


### Phase 4: Final Limit

**Config**: `limit` | **Default**: 10

The maximum number of results to return after aggregation. This is the final output limit
that controls how many results the user sees.


### Elbow Detection Details

The elbow algorithm detects the point in the score curve where relevance drops significantly:

```
Score curve with elbow at position 4:

  100 ┤████
   80 ┤████████
   60 ┤████████████
   40 ┤████████████████
   15 ┤████████████████████  ← elbow (15/40 = 0.375 < 0.3? no, continue)
   12 ┤████████████████████████
    3 ┤████████████████████████████  ← cutoff (3/12 = 0.25 < 0.3? yes, stop)
```

Tuning guidance for `cutoff_ratio`:
- **0.5**: Aggressive filtering; only tightly clustered high-relevance results
- **0.3**: Balanced (default); allows gradual score decline
- **0.1**: Permissive; includes results with significant score gaps
- **0.0**: Disabled; return up to `max_candidates` candidates regardless of score distribution


### Multi-Tree Score Normalization

When searching across multiple trees with different content densities, ra automatically
normalizes scores so results from each tree compete fairly.

The problem: A specialized tree (e.g., project-specific notes) scores much higher on
domain-specific terms than a general tree (e.g., reference documentation), even when both
contain relevant results.

The solution: Before applying elbow cutoff, ra normalizes scores within each tree so each
tree's best result gets a score of 1.0:

```
Before normalization:           After normalization:
  tree-a:doc1  4500             tree-a:doc1  1.00
  tree-a:doc2  3200             tree-b:doc1  1.00
  tree-a:doc3  3000             tree-b:doc2  0.75
  tree-b:doc1   800  ← cutoff   tree-a:doc2  0.71
  tree-b:doc2   600             tree-a:doc3  0.67
```

This happens automatically when multiple trees are specified. Single-tree searches use raw
scores.


## Hierarchical Aggregation

ra implements an adaptive algorithm that automatically aggregates sibling matches
into their parent sections. See [chunking.md](chunking.md) for the complete specification.

### Pipeline Phases

1. **Query**: Retrieve candidates from the index (derived from `limit × 5`)
2. **Normalize**: For multi-tree searches, normalize scores per tree
3. **Elbow cutoff**: Apply relevance cutoff to raw candidates
4. **Adaptive aggregation**: Process all relevant candidates, aggregating siblings
5. **Limit**: Truncate to final `limit` results

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
